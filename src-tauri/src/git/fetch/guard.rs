//! fetch 가드의 판정 로직. `src/main/git/fetch.ts` 의 순수 함수(허용 목록·URL 판정·설정 파싱·검사 결과·git 버전·오류 분류·fetch 인자·환경)의 이식이다.
//! 이 파일은 보안 경계다 — **TS 와 한 글자라도 다르면 안 되고, 더 엄격하게도 바꾸지 않는다**(1:1 이식. 바꾸려면 TS 에 먼저 고친다).
//! 글자 단위 차이가 나는 곳(JS `\s`·`trim`·`.`·`$`·`i` 플래그·UTF-16)은 [`super`] 의 설명을 본다.

use std::collections::HashMap;
use std::sync::OnceLock;

use serde::Serialize;
use sha2::{Digest, Sha256};

use super::pattern::Pattern;
use super::{is_line_terminator, js_to_lowercase, js_trim, slice_utf16_lossy};
use crate::git::scope::is_js_space;

// ── 허용 목록 ─────────────────────────────────────────────────────────────

/// TS `ALLOWED_KEYS` 의 `source`(플래그 없음). 골든 `lists.json` 과 글자 그대로 같아야 한다 — 매처가 이 문자열을 해석한다
pub const ALLOWED_KEY_SOURCES: [&str; 13] = [
    r"^core\.(repositoryformatversion|filemode|logallrefupdates|ignorecase|precomposeunicode|symlinks|autocrlf|eol|safecrlf|longpaths|quotepath|checkstat|trustctime|untrackedcache|fscache|preloadindex|commentchar|hidedotfiles|abbrev|whitespace|worktree|sparsecheckout|sparsecheckoutcone|hookspath|excludesfile)$",
    r"^remote\..+\.(url|pushurl|push|tagopt|prune|mirror|skipdefaultupdate|skipfetchall|promisor|partialclonefilter)$",
    r"^branch\..+\.(remote|merge|rebase|pushremote|description|vscode-merge-base|github-pr-owner-number)$",
    r"^user\.(name|email)$",
    r"^init\.defaultbranch$",
    r"^pull\.(rebase|ff)$",
    r"^push\.(default|autosetupremote)$",
    r"^fetch\.prune$",
    r"^lfs\.repositoryformatversion$",
    r"^extensions\.(objectformat|worktreeconfig|refstorage|partialclone)$",
    r"^submodule\.(.+\.)?(url|active)$",
    r"^include\.path$",
    r"^includeif\..+\.path$",
];

/// TS `ALLOWED_VALUES`: 값까지 맞아야 허용하는 키
pub const ALLOWED_VALUES: &[(&str, &[&str])] = &[
    ("filter.lfs.clean", &["git-lfs clean -- %f"]),
    ("filter.lfs.smudge", &["git-lfs smudge -- %f", "git-lfs smudge --skip -- %f"]),
    ("filter.lfs.process", &["git-lfs filter-process", "git-lfs filter-process --skip"]),
    ("filter.lfs.required", &["true"]),
    ("core.fsmonitor", &["true", "false"]),
    ("core.bare", &["false"]),
];

pub const REMOTE_URL_KEY_SOURCE: &str = r"^remote\..+\.(url|pushurl)$";
pub const SUBMODULE_URL_KEY_SOURCE: &str = r"^submodule\..+\.url$";

// 닫힌 문법 밖이라 손으로 옮긴 정규식의 TS 소스. 골든 `lists.json` 의 handWritten 과 같아야 한다(바뀌면 구현을 다시 본다)
pub const HTTP_USERINFO_SOURCE: &str = r"^(https?:\/\/)([^/?#]*)@";
pub const HTTP_USERINFO_FLAGS: &str = "i";
pub const NT_NAMESPACE_SOURCE: &str = r"[\\/]\?\?[\\/]";
pub const WORKTREE_CONFIG_ABSENT_SOURCE: &str = r"unknown option [`']worktree'|unable to read config file '[^']*config\.worktree': No such file or directory";

struct Patterns {
    allowed: Vec<Pattern>,
    remote_url: Pattern,
    submodule_url: Pattern,
}

fn patterns() -> &'static Patterns {
    static P: OnceLock<Patterns> = OnceLock::new();
    P.get_or_init(|| {
        let compile = |s: &str| Pattern::compile(s).unwrap_or_else(|e| panic!("허용 목록 정규식이 닫힌 문법 밖이다({s}): {e}"));
        Patterns {
            allowed: ALLOWED_KEY_SOURCES.iter().map(|s| compile(s)).collect(),
            remote_url: compile(REMOTE_URL_KEY_SOURCE),
            submodule_url: compile(SUBMODULE_URL_KEY_SOURCE),
        }
    })
}

/// `/^remote\..+\.fetch$/` 의 `(.+)`: 이름을 돌려준다(줄 끝 문자가 있거나 비면 None)
fn remote_fetch_name(key: &str) -> Option<&str> {
    let name = key.strip_prefix("remote.")?.strip_suffix(".fetch")?;
    (!name.is_empty() && !name.chars().any(is_line_terminator)).then_some(name)
}

fn utf16(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

/// TS `fetchRefspecOk`. `[+]refs/heads/*:refs/remotes/<n>/*` 또는 `[+]refs/heads/<x>:refs/remotes/<n>/<x>` 만 true.
/// 접두 비교는 **소문자로 바꾼** dst 의 앞 N 단위, `dstRest` 는 **원래** dst 의 N 단위 뒤다 — 소문자화로 길이가 바뀌는 문자(`İ`)가 있으면
/// 둘이 어긋나는데, 그 어긋남까지 TS 와 같게 둔다(길이·자르기는 UTF-16 코드 단위).
pub fn fetch_refspec_ok(key: &str, value: Option<&str>) -> bool {
    let (Some(name), Some(value)) = (remote_fetch_name(key), value) else {
        return false;
    };
    let mut parts = value.split(':');
    let (Some(src), Some(dst), None) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    let src = src.strip_prefix('+').unwrap_or(src);
    let prefix = utf16(&format!("refs/remotes/{name}/"));
    let Some(src_rest) = src.strip_prefix("refs/heads/") else {
        return false;
    };
    let lowered = utf16(&js_to_lowercase(dst));
    if lowered[..prefix.len().min(lowered.len())] != prefix[..] {
        return false;
    }
    let dst_units = utf16(dst);
    let dst_rest: &[u16] = dst_units.get(prefix.len()..).unwrap_or(&[]);
    if src_rest == "*" {
        return dst_rest == [u16::from(b'*')];
    }
    !src_rest.is_empty()
        && !src_rest.chars().any(|c| matches!(c, '*' | '^' | '~' | '?' | '[' | '\\') || is_js_space(c))
        && !src_rest.contains("..")
        && utf16(src_rest) == dst_rest
}

/// TS `isAllowedConfig`: 키(소문자로 정규화된)와 값이 허용 목록에 있는가.
/// TS 의 `ALLOWED_VALUES[key]` 는 일반 객체 조회라 키가 `constructor`·`__proto__` 면 TypeError 를 던진다(git 키는 점이 있어야 해 실제로는 오지 않는다).
/// 여기서는 그냥 허용 목록에 없는 키로 본다.
pub fn is_allowed_config(key: &str, value: Option<&str>) -> bool {
    if remote_fetch_name(key).is_some() {
        return fetch_refspec_ok(key, value);
    }
    if let Some((_, fixed)) = ALLOWED_VALUES.iter().find(|(k, _)| *k == key) {
        return value.is_some_and(|v| fixed.contains(&v));
    }
    patterns().allowed.iter().any(|p| p.is_match(key))
}

// ── remote URL ────────────────────────────────────────────────────────────

fn is_sep(c: char) -> bool {
    c == '\\' || c == '/'
}

/// `NT_NAMESPACE` = `/[\\/]\?\?[\\/]/` — `\??\`, `/??/` 등이 어디에 있어도
pub fn has_nt_namespace(s: &str) -> bool {
    // 구분자·`?` 는 모두 ASCII 라 UTF-8 바이트 창으로 봐도 같다(여러 바이트 글자 안에는 ASCII 바이트가 없다). 글자 배열을 만들지 않는다
    s.as_bytes().windows(4).any(|w| matches!(w[0], b'\\' | b'/') && w[1] == b'?' && w[2] == b'?' && matches!(w[3], b'\\' | b'/'))
}

/// TS `unsafeRemoteUrl`: 거부할 remote URL 이면 사유, 괜찮으면 None
pub fn unsafe_remote_url(url: &str) -> Option<String> {
    let u = js_trim(url);
    if u.starts_with('-') {
        return Some("옵션처럼 보이는 URL".into());
    }
    let mut chars = u.chars();
    // UNC·네트워크 경로: `^[\\/]{2}`
    if chars.next().is_some_and(is_sep) && chars.next().is_some_and(is_sep) {
        return Some("UNC·네트워크 경로".into());
    }
    if has_nt_namespace(u) {
        return Some("NT 네임스페이스 경로(\\??\\)".into());
    }
    // `/^file:(.*)$/is`: i 는 ASCII 접기, s 라서 `(.*)` 는 줄바꿈도 맞춘다 → 나머지 전부
    if u.as_bytes().get(..5).is_some_and(|b| b.eq_ignore_ascii_case(b"file:")) {
        let rest = &u[5..];
        let mut r = rest.chars();
        let (a, b, c, d) = (r.next(), r.next(), r.next(), r.next());
        let two_then_other = a.is_some_and(is_sep) && b.is_some_and(is_sep) && c.is_some_and(|c| !is_sep(c));
        let four = a.is_some_and(is_sep) && b.is_some_and(is_sep) && c.is_some_and(is_sep) && d.is_some_and(is_sep);
        if two_then_other || four {
            return Some("호스트가 있는 file URL(네트워크 경로)".into());
        }
    }
    // `/^([A-Za-z0-9+.-]+)::/`
    let run_len = u.bytes().take_while(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'.' | b'-')).count();
    if run_len > 0 && u[run_len..].starts_with("::") {
        return Some(format!("{}:: 전송 도우미", u[..run_len].to_ascii_lowercase()));
    }
    if option_like_host(u) {
        return Some("옵션처럼 보이는 호스트".into());
    }
    None
}

/// `/^[A-Za-z][A-Za-z0-9+.-]*:\/\/([^/@]*@)?-/`: 스킴 뒤에 (첫 `@` 까지의 userinfo 가 있든 없든) `-` 로 시작하는 호스트
fn option_like_host(u: &str) -> bool {
    let b = u.as_bytes();
    if !b.first().is_some_and(u8::is_ascii_alphabetic) {
        return false;
    }
    let scheme_len = b.iter().take_while(|c| c.is_ascii_alphanumeric() || matches!(c, b'+' | b'.' | b'-')).count();
    let Some(rest) = u[scheme_len..].strip_prefix("://") else {
        return false;
    };
    if rest.starts_with('-') {
        return true;
    }
    // `[^/@]*@` 는 `/` 나 `@` 를 만나기 전까지만 간다 — 그 자리가 `@` 일 때만 userinfo 다
    match rest.find(['/', '@']) {
        Some(p) if rest.as_bytes()[p] == b'@' => rest[p + 1..].starts_with('-'),
        _ => false,
    }
}

/// `HTTP_USERINFO` = `/^(https?:\/\/)([^/?#]*)@/i` 로 가린 URL(`$1<userinfo>@` + 나머지). userinfo 가 없으면 None.
/// `[^/?#]*@` 는 탐욕적이라 authority 안의 **마지막** `@` 까지 잡는다.
pub fn redact_userinfo(url: &str) -> Option<String> {
    let b = url.as_bytes();
    let scheme = if b.get(..8).is_some_and(|s| s.eq_ignore_ascii_case(b"https://")) {
        8
    } else if b.get(..7).is_some_and(|s| s.eq_ignore_ascii_case(b"http://")) {
        7
    } else {
        return None;
    };
    let after = &url[scheme..];
    let authority_end = after.find(['/', '?', '#']).unwrap_or(after.len());
    let at = after[..authority_end].rfind('@')?;
    Some(format!("{}<userinfo>@{}", &url[..scheme], &after[at + 1..]))
}

// ── 설정 파싱·검사 ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ConfigEntry {
    pub scope: String,
    /// 소문자로 정규화한 키
    pub key: String,
    pub value: Option<String>,
}

fn parse_record(scope: &str, rec: &str) -> ConfigEntry {
    let (key, value) = match rec.find('\n') {
        Some(nl) => (&rec[..nl], Some(rec[nl + 1..].to_string())),
        None => (rec, None),
    };
    ConfigEntry { scope: scope.to_string(), key: js_to_lowercase(key), value }
}

/// `git config --list --show-scope -z` 출력 파싱. 항목은 `scope\0key\nvalue\0`(값 없는 키는 `scope\0key\0`) 두 토큰씩
pub fn parse_config_list(out: &str) -> Vec<ConfigEntry> {
    let tokens: Vec<&str> = out.split('\0').collect();
    tokens.as_chunks::<2>().0.iter().map(|[scope, rec]| parse_record(scope, rec)).collect()
}

/// `--show-scope` 없는 `git config --<scope> --list -z` 출력 파싱. 항목은 `key\nvalue\0` 한 토큰씩
pub fn parse_scoped_config_list(out: &str, scope: &str) -> Vec<ConfigEntry> {
    out.split('\0').filter(|r| !r.is_empty()).map(|r| parse_record(scope, r)).collect()
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Dangerous {
    pub key: String,
    pub value: String,
    /// 표시하지 않는 원본(지문 계산용, 예: 비밀번호가 든 URL). 없으면 필드를 내지 않는다
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Forbidden {
    pub key: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Remote {
    pub name: String,
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigInspection {
    pub has_remote: bool,
    /// 허용 목록에 없는 저장소 설정(알 수 없는 설정)
    pub dangerous: Vec<Dangerous>,
    pub remote_names: Vec<String>,
    pub remotes: Vec<Remote>,
    pub remotes_fingerprint: String,
    /// 허용 여부와 무관하게 거부하는 항목(remote URL)
    pub forbidden: Vec<Forbidden>,
    /// 사용자(시스템·전역·명령줄 scope)가 정한 core.sshCommand. 저장소 안의 값은 절대 쓰지 않는다
    pub user_ssh_command: Option<String>,
    pub fingerprint: String,
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn sha256_json(pairs: &[[&str; 2]]) -> String {
    // serde_json 의 문자열 이스케이프는 JSON.stringify 와 같다(짝 없는 서로게이트는 Rust 문자열에 없다)
    let json = serde_json::to_string(pairs).expect("문자열 쌍은 직렬화된다");
    hex(&Sha256::digest(json.as_bytes()))
}

/// 위험 항목의 지문: sha256(JSON.stringify([[key, raw ?? value], …]))
pub fn fingerprint_of(dangerous: &[Dangerous]) -> String {
    let pairs: Vec<[&str; 2]> = dangerous.iter().map(|d| [d.key.as_str(), d.raw.as_deref().unwrap_or(&d.value)]).collect();
    sha256_json(&pairs)
}

fn remotes_fingerprint_of(remotes: &[Remote]) -> String {
    let pairs: Vec<[&str; 2]> = remotes.iter().map(|r| [r.name.as_str(), r.url.as_str()]).collect();
    sha256_json(&pairs)
}

/// TS `inspectEntries`
pub fn inspect_entries(entries: &[ConfigEntry]) -> ConfigInspection {
    let p = patterns();
    let mut dangerous = Vec::new();
    let mut forbidden = Vec::new();
    let mut has_remote = false;
    let mut user_ssh_command: Option<String> = None;
    for e in entries {
        let repo_scoped = e.scope == "local" || e.scope == "worktree";
        // 나중에 나온 값이 이긴다. 빈 값은 설정도 초기화도 하지 않는다
        if e.key == "core.sshcommand" && !repo_scoped {
            if let Some(v) = e.value.as_deref().filter(|v| !v.is_empty()) {
                user_ssh_command = Some(v.to_string());
            }
        }
        let is_remote_url = p.remote_url.is_match(&e.key);
        if is_remote_url || p.submodule_url.is_match(&e.key) {
            if is_remote_url && e.key.ends_with(".url") {
                has_remote = true;
            }
            if let Some(reason) = unsafe_remote_url(e.value.as_deref().unwrap_or("")) {
                forbidden.push(Forbidden { key: e.key.clone(), reason });
            }
        }
        if repo_scoped && !is_allowed_config(&e.key, e.value.as_deref()) {
            dangerous.push(Dangerous { key: e.key.clone(), value: e.value.clone().unwrap_or_default(), raw: None });
        }
    }
    let fingerprint = fingerprint_of(&dangerous);
    ConfigInspection {
        has_remote,
        dangerous,
        remote_names: Vec::new(),
        remotes: Vec::new(),
        remotes_fingerprint: String::new(),
        forbidden,
        user_ssh_command,
        fingerprint,
    }
}

/// `git remote` / `git remote get-url --all -- <n>` 한 번의 결과: 성공이면 stdout, 실패면 stderr
#[derive(Debug, Clone, Copy)]
pub enum GitOut<'a> {
    Ok(&'a str),
    Failed(&'a str),
}

fn split_lines(out: &str) -> impl Iterator<Item = &str> {
    out.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l))
}

/// TS `inspectRemoteUrls` 의 첫 단계(`git remote`). 읽지 못하면 막을 항목을 더하고 None(검사를 여기서 끝낸다 — 지문은 갱신하지 않는다)
pub fn apply_remote_list(ins: &mut ConfigInspection, list: GitOut<'_>) -> Option<Vec<String>> {
    match list {
        GitOut::Failed(stderr) => {
            ins.forbidden.push(Forbidden { key: "remote".into(), reason: format!("remote 목록을 읽지 못함: {}", slice_utf16_lossy(js_trim(stderr), 120)) });
            None
        }
        GitOut::Ok(stdout) => {
            let names: Vec<String> = split_lines(stdout).filter(|n| !n.is_empty()).map(str::to_string).collect();
            if !names.is_empty() {
                ins.has_remote = true;
            }
            ins.remote_names = names.clone();
            Some(names)
        }
    }
}

/// 둘째 단계(remote 마다 `get-url --all`). userinfo 가 있는 http(s) URL 은 가려서 위험 항목으로 올린다(원본은 지문에만 쓴다)
pub fn apply_remote_urls(ins: &mut ConfigInspection, name: &str, urls: GitOut<'_>) {
    let stdout = match urls {
        GitOut::Failed(_) => {
            ins.forbidden.push(Forbidden { key: format!("remote.{name}"), reason: "URL 을 읽지 못함".into() });
            return;
        }
        GitOut::Ok(stdout) => stdout,
    };
    for url in split_lines(stdout) {
        if url.is_empty() {
            continue;
        }
        ins.remotes.push(Remote { name: name.to_string(), url: url.to_string() });
        let key = format!("remote.{name}.url");
        if let Some(reason) = unsafe_remote_url(url) {
            ins.forbidden.push(Forbidden { key, reason });
        } else if let Some(shown) = redact_userinfo(url) {
            ins.dangerous.push(Dangerous { key, value: shown, raw: Some(url.to_string()) });
        }
    }
}

/// 마지막 단계: 지문 계산(remote 이름 목록을 읽는 데 성공한 경우에만 부른다)
pub fn finish_remote_urls(ins: &mut ConfigInspection) {
    ins.fingerprint = fingerprint_of(&ins.dangerous);
    ins.remotes_fingerprint = remotes_fingerprint_of(&ins.remotes);
}

// ── 설정 읽기 판정(실행은 뒤 단계) ─────────────────────────────────────────

fn config_read_error_message(stderr: &str) -> String {
    format!("저장소 설정을 검사하지 못해 fetch 하지 않습니다: {}", js_trim(stderr))
}

/// `/a.*b/i`: ASCII 만 접고(`ſ`·`K` 는 s·k 가 아니다), `.` 가 줄 끝 문자를 못 맞추므로 **같은 줄에서 이 순서**여야 한다
fn line_has_in_order(text: &str, first: &str, then: &str) -> bool {
    let lc = text.to_ascii_lowercase();
    lc.match_indices(first).any(|(i, _)| {
        let rest = &lc[i + first.len()..];
        let line = &rest[..rest.find(is_line_terminator).unwrap_or(rest.len())];
        line.contains(then)
    })
}

/// stderr 가 `--show-scope` 를 모르는 git 의 것인가(`/unknown option.*show-scope/i`)
pub fn unknown_show_scope(stderr: &str) -> bool {
    line_has_in_order(stderr, "unknown option", "show-scope")
}

/// stderr 가 `--no-write-fetch-head` 를 모르는 git 의 것인가(`/unknown option.*write-fetch-head/i`) — 그 옵션 없이 한 번 더 한다
pub fn unknown_write_fetch_head(stderr: &str) -> bool {
    line_has_in_order(stderr, "unknown option", "write-fetch-head")
}

/// TS `WORKTREE_CONFIG_ABSENT`: `--worktree` 를 모르는 git, 또는 config.worktree 가 없음(메시지는 LC_ALL=C.UTF-8 이라 영어, 대소문자 구분)
pub fn worktree_config_absent(stderr: &str) -> bool {
    if stderr.contains("unknown option `worktree'") || stderr.contains("unknown option 'worktree'") {
        return true;
    }
    // `unable to read config file '[^']*config\.worktree': No such file or directory`:
    // `[^']*` 는 따옴표를 건너뛰지 못하므로 접두 뒤 첫 `'` 바로 앞 15글자가 `config.worktree` 여야 한다
    const PREFIX: &str = "unable to read config file '";
    const NAME: &str = "config.worktree";
    const TAIL: &str = "': No such file or directory";
    stderr.match_indices(PREFIX).any(|(i, _)| {
        let rest = &stderr[i + PREFIX.len()..];
        rest.find('\'').is_some_and(|q| rest[..q].ends_with(NAME) && rest[q..].starts_with(TAIL))
    })
}

#[derive(Debug, Clone, PartialEq)]
pub enum ShowScopeOutcome {
    /// 출력을 `parse_config_list` 로 읽는다
    Parsed,
    /// 범위별 읽기([`ScopeWalk`])로 넘어간다
    Fallback,
    /// 검사할 수 없으므로 fetch 하지 않는다(메시지)
    Error(String),
}

/// `git config --list --show-scope --includes -z` 한 번의 결과로 하는 선택
pub fn show_scope_outcome(code: i32, stderr: &str) -> ShowScopeOutcome {
    if code == 0 {
        ShowScopeOutcome::Parsed
    } else if unknown_show_scope(stderr) {
        ShowScopeOutcome::Fallback
    } else {
        ShowScopeOutcome::Error(config_read_error_message(stderr))
    }
}

/// `extensions.worktreeconfig` 가 켜졌는가: 값 없는 키·참 값(false·no·off·0·빈 값이 아님). 애매하면 읽는 쪽으로
fn worktree_config_on(entries: &[ConfigEntry]) -> bool {
    entries.iter().any(|e| {
        e.key == "extensions.worktreeconfig"
            && e.value.as_deref().is_none_or(|v| !matches!(js_trim(v).to_ascii_lowercase().as_str(), "false" | "no" | "off" | "0" | ""))
    })
}

/// 옛 git(`--show-scope` 없음)의 범위별 읽기: system → global → local → worktree 순서(git 이 적용하는 순서, TS `readConfigByScope`).
/// **지금 읽어야 할 범위 하나**를 값으로 든다. 호출자는 [`ScopeWalk::start`] 로 시작해 `scope()` 를 실행하고 결과를 [`ScopeWalk::feed`] 에 넣는다.
/// `feed` 는 자신을 소비하고 다음 범위([`WalkStep::Read`])나 끝([`WalkStep::Done`])을 돌려주므로, 범위를 건너뛰거나 순서를 바꾸거나
/// 엉뚱한 범위의 결과를 넣을 수 없다(`worktree` 는 `extensions.worktreeconfig` 가 켜져 있을 때만 차례가 온다).
#[derive(Debug)]
#[must_use = "범위를 읽고 feed 로 결과를 넣어야 한다"]
pub struct ScopeWalk {
    scope: &'static str,
    /// 앞 범위들에서 읽은 항목
    entries: Vec<ConfigEntry>,
}

const SCOPES: [&str; 4] = ["system", "global", "local", "worktree"];

/// [`ScopeWalk::feed`] 의 결과
#[derive(Debug)]
#[must_use]
pub enum WalkStep {
    /// 다음 범위를 읽는다
    Read(ScopeWalk),
    /// 모두 읽었다(범위 순서대로 쌓인 항목)
    Done(Vec<ConfigEntry>),
}

impl ScopeWalk {
    /// 첫 범위(system)부터 시작한다
    pub fn start() -> Self {
        Self { scope: SCOPES[0], entries: Vec::new() }
    }

    /// 지금 읽을 범위(`git config --<scope> --list --includes -z`)
    pub fn scope(&self) -> &'static str {
        self.scope
    }

    /// `scope()` 의 실행 결과. system·global 은 파일이 없으면 실패하므로 비어 있는 것으로 보고, local 은 반드시 읽혀야 하며,
    /// worktree 는 "옵션을 모름·파일 없음"만 비어 있는 것으로 본다. 그 밖의 실패는 Err(메시지).
    /// worktree 는 `extensions.worktreeconfig` 가 켜져 있을 때만 읽는다(꺼져 있으면 `--worktree` 가 .git/config 를 다시 돌려줘 local 과 겹친다)
    pub fn feed(mut self, code: i32, stdout: &str, stderr: &str) -> Result<WalkStep, String> {
        if code == 0 {
            self.entries.extend(parse_scoped_config_list(stdout, self.scope));
        } else if self.scope == "local" || (self.scope == "worktree" && !worktree_config_absent(stderr)) {
            return Err(config_read_error_message(stderr));
        }
        let at = SCOPES.iter().position(|&s| s == self.scope).expect("범위는 SCOPES 안에 있다");
        let next = SCOPES[at + 1..].iter().find(|&&s| s != "worktree" || worktree_config_on(&self.entries));
        Ok(match next {
            Some(&scope) => WalkStep::Read(ScopeWalk { scope, entries: self.entries }),
            None => WalkStep::Done(self.entries),
        })
    }
}

// ── git 버전 ──────────────────────────────────────────────────────────────

/// `git version 2.49.0.windows.1` → [2, 49, 0]. JS `Number` 처럼 f64 로 받는다(아주 긴 숫자에서도 패닉하지 않는다)
pub type GitVersion = [f64; 3];

/// file 전송을 허용해도 되는 최소 버전. 그 아래는 CVE-2024-32004 에 취약하다
pub const FILE_TRANSPORT_MIN_GIT: GitVersion = [2.0, 45.0, 1.0];

fn digits(s: &str) -> Option<(&str, &str)> {
    let n = s.bytes().take_while(u8::is_ascii_digit).count();
    (n > 0).then(|| s.split_at(n))
}

/// `/git version (\d+)\.(\d+)(?:\.(\d+))?/` — 비앵커 검색이라 앞의 "git version " 이 맞지 않아도 뒤의 것이 맞을 수 있다. `\d` 는 ASCII 만
pub fn parse_git_version(text: &str) -> Option<GitVersion> {
    const KEY: &str = "git version ";
    let num = |s: &str| s.parse::<f64>().unwrap_or(f64::NAN);
    text.match_indices(KEY).find_map(|(i, _)| {
        let (major, rest) = digits(&text[i + KEY.len()..])?;
        let (minor, rest) = digits(rest.strip_prefix('.')?)?;
        let patch = rest.strip_prefix('.').and_then(digits).map_or(0.0, |(p, _)| num(p));
        Some([num(major), num(minor), patch])
    })
}

pub fn version_at_least(v: Option<GitVersion>, min: GitVersion) -> bool {
    let Some(v) = v else {
        return false;
    };
    for i in 0..3 {
        if v[i] != min[i] {
            return v[i] > min[i];
        }
    }
    true
}

// ── 오류 분류·백오프 ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FetchFailure {
    Auth,
    Network,
    Protocol,
}

/// `/transport '[^']*' not allowed/i`: `[^']*` 는 첫 따옴표에서 멈추고(줄바꿈은 지나간다) 곧바로 `' not allowed` 가 와야 한다
fn transport_not_allowed(lc: &str) -> bool {
    const PREFIX: &str = "transport '";
    lc.match_indices(PREFIX).any(|(i, _)| {
        let rest = &lc[i + PREFIX.len()..];
        rest.find('\'').is_some_and(|q| rest[q..].starts_with("' not allowed"))
    })
}

/// TS `classifyFailure` 의 문구 판정(`text` 는 오류 메시지). 취소(AbortError)는 호출자가 `Network` 로 본다.
/// 정규식은 모두 `i` 플래그이고 리터럴뿐이라 ASCII 소문자화 뒤 부분 문자열 검색과 같다(`ſ`·`K` 는 접히지 않는다).
pub fn classify_failure(text: &str, github: bool) -> FetchFailure {
    let lc = text.to_ascii_lowercase();
    let has = |needles: &[&str]| needles.iter().any(|n| lc.contains(n));
    if transport_not_allowed(&lc) {
        return FetchFailure::Protocol;
    }
    // GitHub 는 권한이 없는 저장소를 "Repository not found"(404)로 숨긴다
    if github && has(&["repository not found", "returned error: 401", "returned error: 403", "returned error: 404", "invalid username or token", "bad credentials"]) {
        return FetchFailure::Auth;
    }
    if has(&[
        "authentication failed",
        "permission denied",
        "could not read username",
        "could not read password",
        "terminal prompts disabled",
        "host key verification failed",
        "publickey",
        "access denied",
        "invalid credentials",
    ]) {
        FetchFailure::Auth
    } else {
        FetchFailure::Network
    }
}

/// 자동 fetch 다음 실행까지의 대기(ms). 실패가 쌓이면 2배씩 늘리되 30분을 넘기지 않는다(JS 숫자처럼 f64)
pub fn next_delay_ms(minutes: f64, failures: u32) -> f64 {
    let base = minutes * 60_000.0;
    (base * 2f64.powi(failures.min(10) as i32)).min(base.max(30.0 * 60_000.0))
}

// ── fetch 인자·환경 ───────────────────────────────────────────────────────

pub const FETCH_ARGS: [&str; 4] = ["fetch", "--prune", "--no-prune-tags", "--no-recurse-submodules"];

/// `http.followRedirects` 값. TS 의 `'initial' | 'false'` — 다른 값은 만들 수 없다
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FollowRedirects {
    /// 첫 요청의 리다이렉트만 따른다(git 의 기본값, TS 의 생략 시 값)
    #[default]
    Initial,
    /// 따르지 않는다(GitHub 원격 전용 저장소: Authorization 헤더가 리다이렉트 너머로 가지 않게)
    False,
}

impl FollowRedirects {
    fn as_str(self) -> &'static str {
        match self {
            Self::Initial => "initial",
            Self::False => "false",
        }
    }
}

/// `core.hooksPath` 로 줄 훅 차단용 경로. [`hooks_path_for`] 만 만든다 — 빈 값이나 상대 경로(저장소 안의 훅 디렉터리로 풀릴 수 있다)는 담지 못한다
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HooksPath(String);

impl HooksPath {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// 훅 차단 디렉터리로 쓸 수 있는 절대 경로인가(슬래시로 바꾼 뒤).
/// Windows 는 드라이브 문자(`X:/…`)만 인정한다 — `/tmp/…` 같은 루트 상대 경로는 git.exe 가 현재 드라이브 기준으로 풀어 저장소 드라이브의 경로가 된다(`/dev/null` 문제와 같다).
/// 그 밖의 플랫폼은 `/…` 만 인정한다(`C:/…` 는 상대 경로다)
fn is_absolute_path(p: &str, platform_win32: bool) -> bool {
    let b = p.as_bytes();
    if platform_win32 {
        b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && b[2] == b'/'
    } else {
        b.first() == Some(&b'/')
    }
}

/// 훅 차단용 디렉터리가 존재하고 비어 있는가
pub(super) fn is_empty_dir(dir: &str) -> bool {
    std::fs::read_dir(dir).is_ok_and(|mut d| d.next().is_none())
}

/// TS `hooksPathFor`. Windows 의 로컬 git.exe 는 `/dev/null` 을 `<저장소 드라이브>:\dev\null\` 로 풀어 쓰므로(그 경로에 훅을 심을 수 있다)
/// 로컬은 앱이 소유한 절대 경로의 **빈** 디렉터리를 쓴다. 그 밖(WSL·SSH)은 `/dev/null`, 로컬이라도 Windows 가 아니면 디렉터리가 없을 때 `/dev/null`.
/// `no_hooks_dir` 가 None 이거나 빈 문자열이면 없는 것이다. 절대 경로가 아니거나(TS 와 달리 거부한다) 비어 있지 않은 디렉터리면 Err.
/// 플랫폼은 항상 실제 플랫폼(`cfg!(windows)`)이다 — 호출자가 정하지 못한다
pub fn hooks_path_for(local: bool, no_hooks_dir: Option<&str>) -> Result<HooksPath, String> {
    hooks_path_for_test(local, no_hooks_dir, cfg!(windows), is_empty_dir)
}

/// [`hooks_path_for`] 에 플랫폼과 디렉터리 검사를 주입하는 형태. **테스트 전용**이다 — 앱 코드는 `hooks_path_for` 만 부른다
/// (플랫폼을 속이거나 검사를 건너뛰는 주입을 쓰지 않는다. 운영 코드에 이 이름이 없음은 `tests/fetch_service.rs` 가 소스로 강제한다)
#[doc(hidden)]
pub fn hooks_path_for_test(local: bool, no_hooks_dir: Option<&str>, platform_win32: bool, check_empty: impl FnOnce(&str) -> bool) -> Result<HooksPath, String> {
    if !local {
        return Ok(HooksPath("/dev/null".into()));
    }
    match no_hooks_dir.filter(|d| !d.is_empty()) {
        Some(dir) => {
            if !check_empty(dir) {
                return Err(format!("훅 차단용 디렉터리가 없거나 비어 있지 않아 fetch 하지 않습니다: {dir}"));
            }
            let path = dir.replace('\\', "/");
            // `\\?\C:\…`(canonicalize 가 Windows 에서 만드는 모양)·`\\host\share`·`\??\…` 는 UNC·NT 네임스페이스다 — `//` 로 시작하거나 `/??/` 가 있으면 거부한다
            if path.starts_with("//") || has_nt_namespace(&path) {
                return Err(format!("훅 차단용 디렉터리가 UNC·NT 네임스페이스 경로라 fetch 하지 않습니다: {dir}"));
            }
            if !is_absolute_path(&path, platform_win32) {
                return Err(format!("훅 차단용 디렉터리가 절대 경로가 아니라 fetch 하지 않습니다: {dir}"));
            }
            Ok(HooksPath(path))
        }
        None if platform_win32 => Err("훅 차단용 빈 디렉터리가 없어 fetch 하지 않습니다".into()),
        None => Ok(HooksPath("/dev/null".into())),
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ConfigArgsOpts {
    pub credential_helpers: bool,
    pub follow_redirects: FollowRedirects,
}

/// TS `fetchConfigArgs`: fetch 에만 쓰는 -c 설정(저장소 설정·훅이 fetch 중에 코드를 실행하지 못하게 덮어쓴다). 순서까지 TS 와 같다
pub fn fetch_config_args(hooks_path: &HooksPath, opts: ConfigArgsOpts) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    // 값이 빈 credential.helper 는 도우미 목록을 비운다
    if !opts.credential_helpers {
        out.extend(["-c".to_string(), "credential.helper=".to_string()]);
    }
    let settings = [
        "maintenance.auto=false".to_string(),
        "gc.auto=0".to_string(),
        "fetch.writeCommitGraph=false".to_string(),
        format!("core.hooksPath={}", hooks_path.as_str()),
        "fetch.bundleURI=".to_string(),
        "transfer.bundleURI=false".to_string(),
        format!("http.followRedirects={}", opts.follow_redirects.as_str()),
        "http.emptyAuth=false".to_string(),
        "http.allowNTLMAuth=false".to_string(),
        "http.delegation=none".to_string(),
        "credential.allowWindowsAuth=false".to_string(),
    ];
    for s in settings {
        out.push("-c".to_string());
        out.push(s);
    }
    out
}

pub const SSH_COMMAND_DEFAULT: &str = "ssh -o BatchMode=yes -o ConnectTimeout=15";

/// TS `resolveSshCommand`: 사용자(시스템·전역) core.sshCommand > (local 만) 환경변수 GIT_SSH_COMMAND > GIT_SSH > BatchMode 기본.
/// 저장소 안의 값은 어디에도 없다. 빈 값은 없는 것과 같다. `env` 는 키를 그대로(대소문자 구분) 찾는다.
pub fn resolve_ssh_command(user_ssh_command: Option<&str>, kind: &str, env: &HashMap<String, String>, platform_win32: bool) -> String {
    if let Some(c) = user_ssh_command.filter(|c| !c.is_empty()) {
        return c.to_string();
    }
    if kind == "local" {
        if let Some(c) = env.get("GIT_SSH_COMMAND").filter(|c| !c.is_empty()) {
            return c.clone();
        }
        if let Some(ssh) = env.get("GIT_SSH").filter(|c| !c.is_empty()) {
            let p = if platform_win32 { ssh.replace('\\', "/") } else { ssh.clone() };
            return format!("'{}'", p.replace('\'', "'\\''"));
        }
    }
    SSH_COMMAND_DEFAULT.to_string()
}

/// TS `fetchEnv`: fetch 전용 환경변수(순서가 WSL argv 의 `env K=V…` 순서를 정한다). GIT_ENV 위에 덮어쓴다
pub fn fetch_env(ssh_command: &str, allow_file: bool) -> Vec<(&'static str, String)> {
    vec![
        // ext:: · fd:: 와 원격 도우미는 제외한다. file 은 자동 fetch 에서 빼고, 수동에서도 CVE-2024-32004 가 고쳐진 git 에서만 둔다
        ("GIT_ALLOW_PROTOCOL", if allow_file { "https:http:ssh:git:file" } else { "https:http:ssh:git" }.to_string()),
        ("GIT_TERMINAL_PROMPT", "0".to_string()),
        ("GIT_ASKPASS", String::new()),
        ("SSH_ASKPASS_REQUIRE", "never".to_string()),
        ("GCM_INTERACTIVE", "never".to_string()),
        ("GIT_SSH_COMMAND", ssh_command.to_string()),
    ]
}
