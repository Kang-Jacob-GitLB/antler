//! OpenSSH 클라이언트 설정(`~/.ssh/config`) 파서(`src/main/executors/ssh/config.ts`). 읽기 전용이며 접속 대상 해석에 필요한 키워드만 다룬다.
//!
//! 지원: Host(복수 패턴·와일드카드 `*` `?`·부정 `!`), HostName(`%h`), User, Port, IdentityFile(복수·`~`·`%d %u %h %r %p %n`),
//!       ProxyJump(쉼표 체인·user@host:port·none), IdentitiesOnly, IdentityAgent(none/SSH_AUTH_SOCK/경로), UserKnownHostsFile, HostKeyAlias,
//!       Include(`~/.ssh` 기준 상대 경로, 마지막 경로 요소의 `*` `?` 글롭만 best-effort, 깊이 16).
//! 의미론: OpenSSH 와 같이 "먼저 얻은 값이 이긴다"(IdentityFile 만 모든 일치 블록에서 누적).
//! 미지원: Match — `Match all` 만 전역으로 취급하고 그 외 조건의 Match 블록은 통째로 적용하지 않는다. ProxyCommand 는 실행하지 않는다 —
//!       예외로 `ssh <별칭> socat - UNIX-CONNECT:<절대경로>` 한 모양만 "그 별칭을 점프 홉으로, 그 안의 유닉스 소켓을 대상으로"로 읽는다([`parse_streamlocal_proxy`]).
//!
//! **조용히 무시하면 위험한 설정은 [`ResolvedHost::unsupported`] 로 알린다**: ProxyCommand 를 무시하면 bastion 을 건너뛰고 내부 이름으로 직접 접속하고,
//! 이 호스트에 걸릴 수 있는 Match 블록을 무시하면 의도와 다른 신원으로 접속한다. 연결 쪽은 `unsupported` 가 있으면 거부해야 한다(이 약화는 금지).
//!
//! Rust 가 TS 보다 **엄격한** 곳(역시 `unsupported`): 별칭·HostName·HostKeyAlias 의 비 ASCII, 범위 밖 포트(최종·ProxyJump 홉), RevokedHostKeys.
//!
//! TS 와 같게 옮긴 곳: 경로는 Node `path.win32` 규칙으로 정규화하고([`super::win32_path`]), 파일·환경변수·로컬 사용자 이름은 호출자가 넣는다
//! ([`ConfigFs`]·[`Ctx`]) — 골든이 임시 경로에 기대지 않게 한다. 정규식은 쓰지 않았다(줄 형식·`\s`·`.` 의 JS 의미는 [`split_key_value`] 에 손으로 풀었다).

use std::collections::HashMap;
use std::fs;

use super::posix_path as posix;
use super::win32_path as win32;
use super::glob_match;
use crate::git::fetch::{is_line_terminator, js_to_lowercase, js_trim};
use crate::git::scope::is_js_space;

const MAX_INCLUDE_DEPTH: usize = 16;

const PORT_RANGE: std::ops::RangeInclusive<u32> = 1..=65535;

/// `unsupported` 문구(연결 쪽은 `unsupported` 가 있으면 거부한다)
const NON_ASCII_HOST: &str = "비 ASCII 호스트 이름(punycode 로 적으세요)";
const BAD_PORT: &str = "포트가 1~65535 범위 밖";

// ── 파일 시스템·환경 주입 ────────────────────────────────────────────────────

/// Include 가 읽는 파일 시스템. 골든·시험은 [`MemFs`], 운영은 [`RealFs`]
pub trait ConfigFs {
    /// 디렉터리의 항목 이름들(순서 무관). 없거나 읽지 못하면 None
    fn read_dir(&self, dir: &str) -> Option<Vec<String>>;
    /// 일반 파일의 내용(UTF-8 이 아닌 바이트는 U+FFFD). 없거나 파일이 아니거나 읽지 못하면 None
    fn read_file(&self, path: &str) -> Option<String>;
}

/// 실제 파일 시스템
pub struct RealFs;

impl ConfigFs for RealFs {
    fn read_dir(&self, dir: &str) -> Option<Vec<String>> {
        let rd = fs::read_dir(dir).ok()?;
        Some(rd.filter_map(Result::ok).map(|e| e.file_name().to_string_lossy().into_owned()).collect())
    }

    fn read_file(&self, path: &str) -> Option<String> {
        if !fs::metadata(path).ok()?.is_file() {
            return None;
        }
        fs::read(path).ok().map(|b| String::from_utf8_lossy(&b).into_owned())
    }
}

/// 메모리 파일 시스템(Windows 규칙: 경로는 win32 로 정규화하고 대소문자를 구분하지 않는다). `tests/goldenSsh.test.ts` 의 가짜 `node:fs` 와 같은 규칙이다
#[derive(Default)]
pub struct MemFs {
    /// (비교 키, 원래 경로, 내용)
    files: Vec<(String, String, String)>,
}

fn vkey(p: &str) -> String {
    js_to_lowercase(&win32::normalize(p)).trim_end_matches('\\').to_string()
}

impl MemFs {
    pub fn new<I: IntoIterator<Item = (String, String)>>(files: I) -> Self {
        MemFs { files: files.into_iter().map(|(p, c)| (vkey(&p), p, c)).collect() }
    }
}

impl ConfigFs for MemFs {
    fn read_dir(&self, dir: &str) -> Option<Vec<String>> {
        let want = vkey(dir);
        Some(self.files.iter().filter(|(k, _, _)| vkey(&win32::dirname(k)) == want).map(|(_, orig, _)| win32::basename(orig)).collect())
    }

    fn read_file(&self, path: &str) -> Option<String> {
        let want = vkey(path);
        self.files.iter().find(|(k, _, _)| *k == want).map(|(_, _, c)| c.clone())
    }
}

/// 해석에 필요한 바깥 값: 로컬 사용자 이름(`%u`·기본 User)과 환경변수(IdentityAgent 의 `$VAR`)
pub struct Ctx<'a> {
    pub local_user: &'a str,
    pub env: &'a dyn Fn(&str) -> Option<String>,
}

/// 프로세스 환경변수(Windows 는 이름의 대소문자를 구분하지 않는다 — Node `process.env` 와 같다). 값이 유니코드가 아니면 없는 것으로 본다
pub fn process_env(name: &str) -> Option<String> {
    if name.is_empty() || name.contains(['=', '\0']) {
        return None;
    }
    std::env::var(name).ok()
}

/// 로컬 사용자 이름. Windows 는 `GetUserNameW`(Node `os.userInfo().username` 과 같은 출처), 실패하면 `USERNAME`·`USER`, 그래도 없으면 `user`
pub fn local_user_name() -> String {
    #[cfg(windows)]
    if let Some(n) = windows_user_name() {
        return n;
    }
    std::env::var("USERNAME").or_else(|_| std::env::var("USER")).ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "user".to_string())
}

#[cfg(windows)]
fn windows_user_name() -> Option<String> {
    use windows::core::PWSTR;
    use windows::Win32::System::WindowsProgramming::GetUserNameW;

    // UNLEN(256) + NUL
    let mut buf = [0u16; 257];
    let mut len = buf.len() as u32;
    // SAFETY: buf 는 len 개의 u16 을 담는 살아 있는 버퍼이고, 함수가 NUL 을 포함한 길이를 len 에 쓴다
    unsafe { GetUserNameW(Some(PWSTR(buf.as_mut_ptr())), &mut len) }.ok()?;
    let n = (len as usize).saturating_sub(1).min(buf.len());
    let name = String::from_utf16_lossy(&buf[..n]);
    (!name.is_empty()).then_some(name)
}

// ── 파싱 ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigOption {
    /// 소문자 키워드
    pub key: String,
    pub args: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigBlock {
    /// 전역(Host 이전) 또는 `Match all` 이면 None — 항상 적용
    pub patterns: Option<Vec<String>>,
    /// 적용 불가 Match 블록
    pub skipped: bool,
    /// skipped Match 블록의 조건(키워드·값 쌍) — 대상 호스트에 해당할 수 있는지 판단용
    pub match_args: Option<Vec<String>>,
    pub options: Vec<ConfigOption>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshConfig {
    pub home_dir: String,
    pub blocks: Vec<ConfigBlock>,
}

/// 따옴표·공백·행 중간 주석(`#` 로 시작하는 토큰)을 처리해 인자로 쪼갠다. 백슬래시는 Windows 경로 때문에 그대로 둔다(`\"` 만 따옴표 안에서 `"`)
fn split_args(rest: &str) -> Vec<String> {
    let chars: Vec<char> = rest.chars().collect();
    let mut args = Vec::new();
    let mut cur = String::new();
    let mut has = false;
    let mut quoted = false;
    let mut i = 0;
    while i < chars.len() {
        let ch = chars[i];
        if quoted {
            if ch == '\\' && chars.get(i + 1) == Some(&'"') {
                cur.push('"');
                i += 1;
            } else if ch == '"' {
                quoted = false;
            } else {
                cur.push(ch);
            }
        } else if ch == '"' {
            quoted = true;
            has = true;
        } else if ch == ' ' || ch == '\t' {
            if has {
                args.push(std::mem::take(&mut cur));
            }
            cur.clear();
            has = false;
        } else if ch == '#' && !has {
            break;
        } else {
            cur.push(ch);
            has = true;
        }
        i += 1;
    }
    if has {
        args.push(cur);
    }
    args
}

/// `^([A-Za-z0-9]+)(?:\s*=\s*|\s+)(.*)$` — (키, 값). `.` 는 줄 끝 문자를 맞추지 않아 값 안에 있으면 줄 전체가 무시된다
fn split_key_value(line: &str) -> Option<(&str, &str)> {
    let key_end = line.find(|c: char| !c.is_ascii_alphanumeric()).unwrap_or(line.len());
    if key_end == 0 {
        return None;
    }
    let rest = &line[key_end..];
    let after_ws = rest.trim_start_matches(is_js_space);
    let value = if let Some(v) = after_ws.strip_prefix('=') {
        v.trim_start_matches(is_js_space)
    } else if after_ws.len() != rest.len() {
        after_ws
    } else {
        return None;
    };
    if value.chars().any(is_line_terminator) {
        return None;
    }
    Some((&line[..key_end], value))
}

pub fn has_wildcard(s: &str) -> bool {
    s.contains(['*', '?'])
}

fn has_glob_char(s: &str) -> bool {
    s.contains(['*', '?', '['])
}

/// 설정 값의 경로 규칙. 홈이 `/` 로 시작하면 POSIX(macOS·Linux), 아니면 Node `path.win32`(골든 사양) 규칙이다.
/// win32 규칙을 macOS 에서 쓰면 `/Users/me/.ssh/id_rsa` 가 `\Users\me\.ssh\id_rsa` 가 돼 키·Include 파일을 읽지 못한다
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PathStyle {
    Win32,
    Posix,
}

impl PathStyle {
    fn for_home(home_dir: &str) -> Self {
        if home_dir.starts_with('/') {
            PathStyle::Posix
        } else {
            PathStyle::Win32
        }
    }

    fn normalize(self, p: &str) -> String {
        match self {
            PathStyle::Win32 => win32::normalize(p),
            PathStyle::Posix => posix::normalize(p),
        }
    }

    fn join(self, parts: &[&str]) -> String {
        match self {
            PathStyle::Win32 => win32::join(parts),
            PathStyle::Posix => posix::join(parts),
        }
    }

    fn is_absolute(self, p: &str) -> bool {
        match self {
            PathStyle::Win32 => win32::is_absolute(p),
            PathStyle::Posix => posix::is_absolute(p),
        }
    }

    fn dirname(self, p: &str) -> String {
        match self {
            PathStyle::Win32 => win32::dirname(p),
            PathStyle::Posix => posix::dirname(p),
        }
    }

    fn basename(self, p: &str) -> String {
        match self {
            PathStyle::Win32 => win32::basename(p),
            PathStyle::Posix => posix::basename(p),
        }
    }
}

fn expand_include_glob(pattern: &str, style: PathStyle, fs: &dyn ConfigFs) -> Vec<String> {
    if !has_glob_char(pattern) {
        return vec![pattern.to_string()];
    }
    let dir = style.dirname(pattern);
    if has_glob_char(&dir) {
        return Vec::new(); // 디렉터리 부분의 글롭은 지원하지 않는다
    }
    let base = style.basename(pattern);
    let mut names: Vec<String> = fs.read_dir(&dir).unwrap_or_default().into_iter().filter(|n| glob_match(&base, n)).collect();
    // JS 기본 sort 는 UTF-16 코드 단위 순서다
    names.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
    names.iter().map(|n| style.join(&[&dir, n])).collect()
}

fn expand_tilde(p: &str, home_dir: &str) -> String {
    if p == "~" {
        return home_dir.to_string();
    }
    let style = PathStyle::for_home(home_dir);
    // `~\` 는 Windows 에서만 홈이다(POSIX 에서 `\` 는 파일 이름의 일부)
    if p.starts_with("~/") || (style == PathStyle::Win32 && p.starts_with("~\\")) {
        return style.join(&[home_dir, &p[2..]]);
    }
    p.to_string()
}

fn parse_into(text: &str, cfg: &mut SshConfig, mut cur: usize, depth: usize, fs: &dyn ConfigFs) {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    for raw in text.split('\n') {
        let line = js_trim(raw);
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = split_key_value(line) else {
            continue;
        };
        let key = key.to_ascii_lowercase();
        let args = split_args(value);
        if args.is_empty() {
            continue;
        }
        match key.as_str() {
            "host" => {
                cfg.blocks.push(ConfigBlock { patterns: Some(args), skipped: false, match_args: None, options: Vec::new() });
                cur = cfg.blocks.len() - 1;
            }
            "match" => {
                let all = args.len() == 1 && js_to_lowercase(&args[0]) == "all";
                cfg.blocks.push(ConfigBlock { patterns: None, skipped: !all, match_args: (!all).then_some(args), options: Vec::new() });
                cur = cfg.blocks.len() - 1;
            }
            "include" => {
                if depth >= MAX_INCLUDE_DEPTH {
                    continue;
                }
                for arg in &args {
                    let style = PathStyle::for_home(&cfg.home_dir);
                    let p = expand_tilde(arg, &cfg.home_dir);
                    let abs = if style.is_absolute(&p) { p } else { style.join(&[&cfg.home_dir, ".ssh", &p]) };
                    for file in expand_include_glob(&abs, style, fs) {
                        let Some(content) = fs.read_file(&file) else {
                            continue;
                        };
                        // 포함된 파일 안의 Host/Match 는 그 파일 안에서만 유효하다(끝나면 부모 상태로 복귀). 시작할 때는 부모의 현재 블록을 이어 받는다
                        parse_into(&content, cfg, cur, depth + 1, fs);
                    }
                }
            }
            _ => cfg.blocks[cur].options.push(ConfigOption { key, args }),
        }
    }
}

/// 설정 텍스트를 파싱한다. `home_dir` 은 `~` 와 Include 상대 경로의 기준
pub fn parse_ssh_config(text: &str, home_dir: &str, fs: &dyn ConfigFs) -> SshConfig {
    let global = ConfigBlock { patterns: None, skipped: false, match_args: None, options: Vec::new() };
    let mut cfg = SshConfig { home_dir: home_dir.to_string(), blocks: vec![global] };
    parse_into(text, &mut cfg, 0, 0, fs);
    cfg
}

/// 설정 파일을 읽어 파싱한다. 파일이 없으면 빈 설정(`user@host` 직접 입력은 동작해야 한다)
pub fn load_ssh_config(config_path: &str, home_dir: &str, fs: &dyn ConfigFs) -> SshConfig {
    let text = fs.read_file(config_path).unwrap_or_default();
    parse_ssh_config(&text, home_dir, fs)
}

// ── 해석 ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JumpHop {
    /// `user@host:port` 원형(재해석에 쓴다)
    pub spec: String,
    pub user: Option<String>,
    pub host: String,
    pub port: Option<u32>,
}

/// `IdentityAgent`. TS 의 `undefined`(기본)·`null`(사용 안 함)·문자열
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdentityAgent {
    /// 기본(SSH_AUTH_SOCK 등)
    Default,
    /// `none` — 사용 안 함
    Disabled,
    /// 소켓/파이프 경로
    Path(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedHost {
    /// 입력으로 받은 별칭/호스트(설정 조회 키)
    pub alias: String,
    pub host_name: String,
    pub user: String,
    pub port: u32,
    pub identity_files: Vec<String>,
    pub proxy_jump: Vec<JumpHop>,
    pub identities_only: bool,
    pub identity_agent: IdentityAgent,
    /// 지원하지 않아 무시할 수 없는 설정이 있으면 그 설명(ProxyCommand, 이 호스트에 해당할 수 있는 Match 블록). **있으면 연결하지 않는다**
    pub unsupported: Option<String>,
    /// `UserKnownHostsFile` 로 지정한 known_hosts 경로들(비면 기본 `~/.ssh/known_hosts`)
    pub known_hosts_files: Vec<String>,
    /// `HostKeyAlias` — known_hosts 조회에 HostName 대신 쓸 이름
    pub host_key_alias: Option<String>,
    /// `ProxyCommand ssh <별칭> socat - UNIX-CONNECT:<경로>` 로 읽은 원격 유닉스 소켓 경로. 있으면 `proxy_jump` 는 그 별칭 한 홉이고,
    /// 연결 쪽은 그 홉 위에서 direct-tcpip 대신 이 경로로 direct-streamlocal 채널을 연다
    pub proxy_socket: Option<String>,
}

/// `ProxyCommand` 인자가 정확히 `ssh <별칭> socat - UNIX-CONNECT:<절대경로>` 이면 (별칭, 경로). 그 밖의 모양은 모두 `None`(= 거부).
/// 명령은 실행하지 않는다 — 같은 뜻의 연결(별칭으로 SSH 접속 → 그 서버의 유닉스 소켓)을 직접 연다. 그래서 셸이 다르게 풀 여지가 있는 글자는 받지 않는다:
/// 별칭은 `[A-Za-z0-9._-]`(첫 글자 `-` 금지, 옵션·`user@`·`%` 토큰 없음), 경로는 `/` 로 시작하는 `[A-Za-z0-9._/-]`(`..` 조각 금지)
pub fn parse_streamlocal_proxy(args: &[String]) -> Option<(String, String)> {
    const PREFIX: &str = "UNIX-CONNECT:";
    let [ssh, jump, socat, dash, target] = args else {
        return None;
    };
    if ssh != "ssh" || socat != "socat" || dash != "-" {
        return None;
    }
    let path = target.get(..PREFIX.len()).filter(|p| p.eq_ignore_ascii_case(PREFIX)).map(|_| &target[PREFIX.len()..])?;
    let jump_ok = !jump.starts_with('-') && !jump.is_empty() && jump.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b));
    let path_ok = path.starts_with('/') && path.bytes().all(|b| b.is_ascii_alphanumeric() || b"._/-".contains(&b)) && !path.split('/').any(|seg| seg == "..");
    (jump_ok && path_ok).then(|| (jump.clone(), path.to_string()))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostSpec {
    pub user: Option<String>,
    pub host: String,
    pub port: Option<u32>,
}

/// 10진 숫자 열을 포트로 읽는다(TS 는 `Number()` 라 어떤 크기든 받는다 — Rust 는 u32 로 포화시킨다. 범위 밖 값은 [`resolve_host`] 가 `unsupported` 로 표시한다)
fn digits_to_port(s: &str) -> u32 {
    s.parse::<u32>().unwrap_or(u32::MAX)
}

fn all_digits(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

/// `[user@]host[:port]` (host 는 `[::1]` 형태의 대괄호 IPv6 허용). 그 외 모양이면 통째로 host.
pub fn parse_host_spec(spec: &str) -> HostSpec {
    let mut rest = js_trim(spec);
    if rest.get(..6).is_some_and(|h| h.eq_ignore_ascii_case("ssh://")) {
        rest = &rest[6..];
    }
    rest = rest.trim_end_matches('/');
    let mut user = None;
    if let Some(at) = rest.rfind('@').filter(|&a| a > 0) {
        user = Some(rest[..at].to_string());
        rest = &rest[at + 1..];
    }
    let mut port = None;
    let bracket = rest.strip_prefix('[').and_then(|r| r.find(']').map(|i| (&r[..i], &r[i + 1..]))).filter(|(inner, _)| !inner.is_empty());
    let host;
    if let Some((inner, after)) = bracket.filter(|(_, after)| after.is_empty() || after.strip_prefix(':').is_some_and(all_digits)) {
        host = inner;
        if let Some(p) = after.strip_prefix(':') {
            port = Some(digits_to_port(p));
        }
    } else if let Some((h, p)) = rest.split_once(':').filter(|(h, p)| !h.is_empty() && all_digits(p)) {
        // 콜론이 하나일 때만 포트(여러 개면 IPv6 리터럴)
        host = h;
        port = Some(digits_to_port(p));
    } else {
        host = rest;
    }
    HostSpec { user, host: host.to_string(), port }
}

fn parse_proxy_jump(value: &str) -> Vec<JumpHop> {
    if js_to_lowercase(value) == "none" {
        return Vec::new();
    }
    value
        .split(',')
        .map(js_trim)
        .filter(|s| !s.is_empty())
        .map(|spec| {
            let h = parse_host_spec(spec);
            JumpHop { spec: spec.to_string(), user: h.user, host: h.host, port: h.port }
        })
        .collect()
}

fn valid_port(s: Option<&str>) -> Option<u32> {
    let s = s.filter(|s| all_digits(s))?;
    Some(digits_to_port(s)).filter(|n| (1..=65535).contains(n))
}

/// `%x` 토큰을 푼다. `%%` 는 `%`, 모르는 토큰은 그대로 둔다
fn expand_tokens(s: &str, tokens: &[(char, &str)]) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c == '%' {
            if let Some(&n) = it.peek().filter(|&&n| !is_line_terminator(n)) {
                it.next();
                if n == '%' {
                    out.push('%');
                } else if let Some((_, v)) = tokens.iter().find(|(k, _)| *k == n) {
                    out.push_str(v);
                } else {
                    out.push('%');
                    out.push(n);
                }
                continue;
            }
        }
        out.push(c);
    }
    out
}

fn is_word(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// `${NAME}` · `$NAME` 을 환경변수로 푼다(없으면 빈 문자열). `\w` 는 ASCII 만이다
fn expand_env(s: &str, env: &dyn Fn(&str) -> Option<String>) -> String {
    let chars: Vec<char> = s.chars().collect();
    let n = chars.len();
    let mut out = String::new();
    let mut i = 0;
    while i < n {
        if chars[i] == '$' {
            if chars.get(i + 1) == Some(&'{') {
                let mut j = i + 2;
                while j < n && is_word(chars[j]) {
                    j += 1;
                }
                if j > i + 2 && chars.get(j) == Some(&'}') {
                    out.push_str(&env(&chars[i + 2..j].iter().collect::<String>()).unwrap_or_default());
                    i = j + 1;
                    continue;
                }
            }
            let mut j = i + 1;
            while j < n && is_word(chars[j]) {
                j += 1;
            }
            if j > i + 1 {
                out.push_str(&env(&chars[i + 1..j].iter().collect::<String>()).unwrap_or_default());
                i = j;
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

fn first_arg<'a>(first: &HashMap<&str, &'a [String]>, key: &str) -> Option<&'a str> {
    first.get(key).and_then(|a| a.first()).map(String::as_str)
}

fn host_name_for(first: &HashMap<&str, &[String]>, alias: &str) -> String {
    match first_arg(first, "hostname").filter(|s| !s.is_empty()) {
        Some(raw) => expand_tokens(raw, &[('h', alias)]),
        None => alias.to_string(),
    }
}

/// 해석하지 않은 Match 블록이 이 호스트에 걸릴 수 있는지 보수적으로 판단한다.
/// host/originalhost 조건이 있고 그 패턴이 별칭·HostName 어느 쪽과도 맞지 않으면 무관하다고 본다.
/// 그 밖의 조건(exec, user, localuser, canonical …)만 있으면 판단할 수 없으므로 걸릴 수 있다고 본다.
fn match_may_apply(args: &[String], names: &[&str]) -> bool {
    let mut i = 0;
    while i + 1 < args.len() {
        let crit = js_to_lowercase(args[i].strip_prefix('!').unwrap_or(&args[i]));
        i += 2;
        if crit != "host" && crit != "originalhost" {
            continue;
        }
        let negated = args[i - 2].starts_with('!');
        let hit = names.iter().any(|n| args[i - 1].split(',').any(|p| !p.starts_with('!') && glob_match(p, n)));
        if if negated { hit } else { !hit } {
            return false;
        }
    }
    true
}

fn block_applies(block: &ConfigBlock, host: &str) -> bool {
    if block.skipped {
        return false;
    }
    let Some(patterns) = &block.patterns else {
        return true;
    };
    let mut positive = false;
    for p in patterns {
        if let Some(neg) = p.strip_prefix('!') {
            if glob_match(neg, host) {
                return false;
            }
        } else if glob_match(p, host) {
            positive = true;
        }
    }
    positive
}

/// 별칭 또는 `[user@]host[:port]` 를 해석한다. spec 의 user/port 는 설정 파일 값보다 우선한다(ssh CLI 와 동일).
/// 설정 조회는 spec 의 host 부분을 별칭으로 삼는다.
pub fn resolve_host(config: &SshConfig, spec: &str, ctx: &Ctx) -> ResolvedHost {
    let given = parse_host_spec(spec);
    let alias = given.host.clone();
    let mut first: HashMap<&str, &[String]> = HashMap::new();
    let mut identity_files: Vec<&str> = Vec::new();
    for block in &config.blocks {
        if !block_applies(block, &alias) {
            continue;
        }
        for o in &block.options {
            if o.key == "identityfile" {
                identity_files.push(&o.args[0]);
            } else {
                first.entry(o.key.as_str()).or_insert(o.args.as_slice());
            }
        }
    }

    let host_name = host_name_for(&first, &alias);
    let user = given.user.or_else(|| first_arg(&first, "user").map(String::from)).unwrap_or_else(|| ctx.local_user.to_string());
    let port = given.port.or_else(|| valid_port(first_arg(&first, "port"))).unwrap_or(22);

    let port_s = port.to_string();
    let tokens: [(char, &str); 6] = [('d', &config.home_dir), ('u', ctx.local_user), ('h', &host_name), ('r', &user), ('p', &port_s), ('n', &alias)];
    let style = PathStyle::for_home(&config.home_dir);
    let normalized = |f: &str| style.normalize(&expand_tilde(&expand_tokens(f, &tokens), &config.home_dir));
    let files: Vec<String> = identity_files.iter().filter(|f| js_to_lowercase(f) != "none").map(|f| normalized(f)).collect();

    // 조용히 무시하면 위험한 설정: ProxyCommand 를 무시하면 bastion 을 건너뛰고 내부 이름으로 직접 접속하고,
    // 이 호스트에 걸릴 수 있는 Match 블록(IdentitiesOnly·User 등)을 무시하면 의도와 다른 신원으로 접속한다
    let mut problems: Vec<String> = Vec::new();
    // ProxyCommand 는 socat 유닉스 소켓 경유 한 모양만 받는다. ProxyJump 와 같이 있으면 OpenSSH 는 먼저 나온 쪽을 쓰므로 모호해 거부한다
    let mut socket_proxy: Option<(String, String)> = None;
    if let Some(pc) = first.get("proxycommand").copied().filter(|pc| pc.first().is_some_and(|c| !c.is_empty() && js_to_lowercase(c) != "none")) {
        socket_proxy = parse_streamlocal_proxy(pc).filter(|_| !first.contains_key("proxyjump"));
        if socket_proxy.is_none() {
            problems.push("ProxyCommand".to_string());
        }
    }
    for block in &config.blocks {
        let margs = block.match_args.as_deref().unwrap_or(&[]);
        if block.skipped && !block.options.is_empty() && match_may_apply(margs, &[&alias, &host_name_for(&first, &alias)]) {
            problems.push(format!("Match {}", margs.join(" ")));
        }
    }

    // 관리자 폐기 목록(RevokedHostKeys)은 읽지 않는다 — 조용히 무시하면 폐기된 키를 수락하므로 거부한다(TS 는 무시한다: 더 엄격한 차이)
    if first_arg(&first, "revokedhostkeys").is_some() {
        problems.push("RevokedHostKeys".to_string());
    }
    // 비교하는 이름 쪽을 ASCII 로 막는다 — 이름이 ASCII 면 어떤 패턴이든 ASCII 접기가 TS(JS 비유니코드 `i`)와 같다(`mod.rs` "이식 규칙")
    let host_key_alias = first_arg(&first, "hostkeyalias").filter(|s| !s.is_empty()).map(String::from);
    if [alias.as_str(), host_name.as_str()].into_iter().chain(host_key_alias.as_deref()).any(|n| !n.is_ascii()) {
        problems.push(NON_ASCII_HOST.to_string());
    }
    // 포트는 최종 값과 모든 ProxyJump 홉이 1..=65535 여야 한다(digits_to_port 는 u32 로 포화시킨다)
    let proxy_jump: Vec<JumpHop> = match &socket_proxy {
        Some((jump, _)) => parse_proxy_jump(jump),
        None => first.get("proxyjump").map(|pj| parse_proxy_jump(&pj.join(" "))).unwrap_or_default(),
    };
    if !PORT_RANGE.contains(&port) || proxy_jump.iter().any(|h| h.port.is_some_and(|p| !PORT_RANGE.contains(&p))) {
        problems.push(BAD_PORT.to_string());
    }

    // UserKnownHostsFile 은 공백으로 여러 파일을 줄 수 있다. 'none' 이면 지정 없음으로 본다
    let known_hosts_files: Vec<String> =
        first.get("userknownhostsfile").copied().unwrap_or(&[]).iter().filter(|f| js_to_lowercase(f) != "none").map(|f| normalized(f)).collect();

    let identity_agent = match first_arg(&first, "identityagent") {
        None => IdentityAgent::Default,
        Some(raw) if js_to_lowercase(raw) == "none" => IdentityAgent::Disabled,
        Some("SSH_AUTH_SOCK") => IdentityAgent::Default,
        Some(raw) => IdentityAgent::Path(expand_tilde(&expand_env(&expand_tokens(raw, &tokens), ctx.env), &config.home_dir)),
    };

    ResolvedHost {
        alias,
        host_name,
        user,
        port,
        identity_files: files,
        proxy_jump,
        identities_only: first_arg(&first, "identitiesonly").is_some_and(|v| js_to_lowercase(v) == "yes"),
        identity_agent,
        unsupported: (!problems.is_empty()).then(|| problems.join(", ")),
        known_hosts_files,
        host_key_alias,
        proxy_socket: socket_proxy.map(|(_, path)| path),
    }
}

/// UI 에 보여줄 호스트 요약
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostInfo {
    pub alias: String,
    pub host_name: String,
    pub user: String,
    pub port: u32,
    pub proxy_jump: Option<String>,
}

/// UI 에 보여줄 목록. 와일드카드·부정 패턴은 제외하고 구체적인 별칭만 낸다(등장 순서, 중복 제거).
pub fn list_hosts(config: &SshConfig, ctx: &Ctx) -> Vec<HostInfo> {
    let mut seen: Vec<&str> = Vec::new();
    let mut out = Vec::new();
    for block in &config.blocks {
        let (false, Some(patterns)) = (block.skipped, &block.patterns) else {
            continue;
        };
        for p in patterns {
            if p.starts_with('!') || has_wildcard(p) || seen.contains(&p.as_str()) {
                continue;
            }
            seen.push(p);
            let r = resolve_host(config, p, ctx);
            out.push(HostInfo {
                alias: p.clone(),
                host_name: r.host_name,
                user: r.user,
                port: r.port,
                proxy_jump: (!r.proxy_jump.is_empty()).then(|| r.proxy_jump.iter().map(|h| h.spec.as_str()).collect::<Vec<_>>().join(",")),
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx<'a>(env: &'a dyn Fn(&str) -> Option<String>) -> Ctx<'a> {
        Ctx { local_user: "me", env }
    }

    fn no_env(_: &str) -> Option<String> {
        None
    }

    const HOME: &str = "C:\\h";

    fn resolve(text: &str, spec: &str) -> ResolvedHost {
        let cfg = parse_ssh_config(text, HOME, &MemFs::default());
        resolve_host(&cfg, spec, &ctx(&no_env))
    }

    #[test]
    fn 값_구분과_줄_모양() {
        assert_eq!(split_key_value("User=u1"), Some(("User", "u1")));
        assert_eq!(split_key_value("USER = u1"), Some(("USER", "u1")));
        assert_eq!(split_key_value("Host\u{a0}x"), Some(("Host", "x")));
        assert_eq!(split_key_value("Host-x y"), None);
        assert_eq!(split_key_value("한글 x"), None);
        assert_eq!(split_key_value("Host a\rb"), None, "값 안의 줄 끝 문자는 줄을 버린다");
        assert_eq!(split_key_value("Host = = x"), Some(("Host", "= x")));
    }

    #[test]
    fn 인자_쪼개기() {
        assert_eq!(split_args(r#"a "b c" d#e # f"#), ["a", "b c", "d#e"]);
        assert_eq!(split_args(r#""C:\Users\me\my key""#), [r"C:\Users\me\my key"]);
        assert_eq!(split_args(r#""a\"b""#), [r#"a"b"#]);
        assert_eq!(split_args(r#""닫히지 않음"#), ["닫히지 않음"]);
        assert_eq!(split_args(r#""""#), [""]);
    }

    #[test]
    fn 호스트_스펙() {
        let h = parse_host_spec("ssh://U@Host:2200/");
        assert_eq!((h.user.as_deref(), h.host.as_str(), h.port), (Some("U"), "Host", Some(2200)));
        let h = parse_host_spec("[fe80::1]:2222");
        assert_eq!((h.user, h.host.as_str(), h.port), (None, "fe80::1", Some(2222)));
        assert_eq!(parse_host_spec("fe80::1").host, "fe80::1");
        assert_eq!(parse_host_spec("[a]b").host, "[a]b");
        assert_eq!(parse_host_spec("@h").host, "@h");
    }

    #[test]
    fn 토큰과_환경변수() {
        assert_eq!(expand_tokens("%%h %h %x %", &[('h', "H")]), "%h H %x %");
        let env = |n: &str| (n == "A").then(|| "1".to_string());
        assert_eq!(expand_env("${A}$A$B${A$", &env), "11${A$");
        assert_eq!(expand_env("$", &env), "$");
    }

    #[test]
    fn proxy_command_와_match_는_unsupported() {
        assert_eq!(resolve("Host inner\n  ProxyCommand ssh -W %h:%p bastion\n", "inner").unsupported.as_deref(), Some("ProxyCommand"));
        assert_eq!(resolve("Host inner\n  ProxyCommand none\n", "inner").unsupported, None);
    }

    #[test]
    fn proxy_command_는_socat_유닉스_소켓_경유_한_모양만_점프로_읽는다() {
        let r = resolve("Host pc\n  HostName 10.0.0.2\n  ProxyCommand ssh Z840C socat - UNIX-CONNECT:/home3/jo/.board-bridge/ssh.sock\n", "pc");
        assert_eq!(r.unsupported, None);
        assert_eq!(r.proxy_socket.as_deref(), Some("/home3/jo/.board-bridge/ssh.sock"));
        assert_eq!(r.proxy_jump.iter().map(|h| h.spec.as_str()).collect::<Vec<_>>(), ["Z840C"]);
        assert_eq!(r.proxy_jump[0].host, "Z840C");
        // 소문자 주소 종류도 socat 은 받는다
        assert!(resolve("Host pc\n  ProxyCommand ssh j socat - unix-connect:/s\n", "pc").proxy_socket.is_some());
        // 그 밖의 모양은 전부 거부(명령을 실행하지 않으므로 뜻이 다를 수 있는 것은 받지 않는다)
        for pc in [
            "ssh -W %h:%p j",
            "ssh j socat - UNIX-CONNECT:rel/s",
            "ssh j socat - UNIX-CONNECT:/a/../s",
            "ssh j socat - UNIX-CONNECT:$HOME/s",
            "ssh j socat - UNIX-CONNECT:~/s",
            "ssh j socat - UNIX-CONNECT:/s;id",
            "ssh j socat - UNIX-CONNECT:/%h",
            "ssh -oX=y socat - UNIX-CONNECT:/s",
            "ssh u@j socat - UNIX-CONNECT:/s",
            "ssh j socat - UNIX-CONNECT:/s extra",
            "ssh j socat STDIO UNIX-CONNECT:/s",
            "ssh j socat - TCP:127.0.0.1:22",
            "/usr/bin/ssh j socat - UNIX-CONNECT:/s",
        ] {
            let r = resolve(&format!("Host pc\n  ProxyCommand {pc}\n"), "pc");
            assert_eq!(r.unsupported.as_deref(), Some("ProxyCommand"), "{pc}");
            assert_eq!(r.proxy_socket, None, "{pc}");
        }
        // ProxyJump 와 같이 있으면(어느 쪽이 이길지 모호) 거부
        let both = resolve("Host pc\n  ProxyJump b\n  ProxyCommand ssh j socat - UNIX-CONNECT:/s\n", "pc");
        assert_eq!(both.unsupported.as_deref(), Some("ProxyCommand"));
        assert_eq!(both.proxy_socket, None);
        let cfg = "Match host *.corp exec \"true\"\n  IdentitiesOnly yes\nMatch user root\n  User admin\n";
        assert_eq!(resolve(cfg, "box.home").unsupported.as_deref(), Some("Match user root"));
        assert_eq!(resolve(cfg, "db.corp").unsupported.as_deref(), Some("Match host *.corp exec true, Match user root"));
        assert_eq!(resolve("Match host other\n  User x\n", "mine").unsupported, None);
        assert_eq!(resolve("Match exec \"true\"\n", "mine").unsupported, None);
    }

    #[test]
    fn 비_ascii_호스트_이름은_unsupported() {
        let bad = Some(NON_ASCII_HOST);
        assert_eq!(resolve("", "Éxample").unsupported.as_deref(), bad, "별칭");
        assert_eq!(resolve("Host a\n  HostName 例.test\n", "a").unsupported.as_deref(), bad, "HostName");
        assert_eq!(resolve("Host a\n  HostKeyAlias é\n", "a").unsupported.as_deref(), bad, "HostKeyAlias");
        assert_eq!(resolve("Host a\n  HostName %h\n", "a").unsupported, None);
        // 켈빈 기호·ſ 같은 대소문자 접기 변종도 같은 관문에 걸린다
        assert_eq!(resolve("", "\u{212a}").unsupported.as_deref(), bad);
        assert_eq!(resolve("", "\u{17f}").unsupported.as_deref(), bad);
        // ASCII 이름이면 비 ASCII 패턴이 있어도 TS 와 같다(비 ASCII 패턴은 ASCII 이름에 맞지 않는다)
        assert_eq!(resolve("Host É*\n  Port 9\n", "x").port, 22);
    }

    #[test]
    fn 포트는_1_65535_밖이면_unsupported() {
        let bad = Some(BAD_PORT);
        assert_eq!(resolve("", "h:0").unsupported.as_deref(), bad);
        assert_eq!(resolve("", "h:65536").unsupported.as_deref(), bad);
        assert_eq!(resolve("", "h:99999999999999999999").unsupported.as_deref(), bad);
        assert_eq!(resolve("", "h:65535").unsupported, None);
        assert_eq!(resolve("", "h:1").unsupported, None);
        // 설정 파일의 Port 는 범위 밖이면 무시하므로 문제가 아니다
        assert_eq!(resolve("Host h\n  Port 70000\n", "h").unsupported, None);
        // ProxyJump 홉
        assert_eq!(resolve("Host h\n  ProxyJump b@j:0\n", "h").unsupported.as_deref(), bad);
        assert_eq!(resolve("Host h\n  ProxyJump ok,j:70000\n", "h").unsupported.as_deref(), bad);
        assert_eq!(resolve("Host h\n  ProxyJump j:22,k:65535\n", "h").unsupported, None);
    }

    #[test]
    fn revoked_host_keys_는_unsupported() {
        assert_eq!(resolve("Host h\n  RevokedHostKeys ~/revoked\n", "h").unsupported.as_deref(), Some("RevokedHostKeys"));
        assert_eq!(resolve("Host *\n  revokedhostkeys /etc/ssh/revoked\n", "h").unsupported.as_deref(), Some("RevokedHostKeys"));
        assert_eq!(resolve("Host other\n  RevokedHostKeys x\n", "h").unsupported, None, "이 호스트에 적용되지 않으면 무관하다");
        // 여러 문제는 쉼표로 이어 붙는다
        assert_eq!(resolve("Host h\n  RevokedHostKeys x\n  ProxyCommand c\n", "h:0").unsupported.as_deref(), Some("ProxyCommand, RevokedHostKeys, 포트가 1~65535 범위 밖"));
    }

    #[test]
    fn include_는_깊이_16_에서_멈춘다() {
        let fs = MemFs::new([("C:\\h\\.ssh\\loop".to_string(), "Host *\n  IdentityFile k\nInclude loop\n".to_string())]);
        let cfg = parse_ssh_config("Include loop\n", HOME, &fs);
        let r = resolve_host(&cfg, "x", &ctx(&no_env));
        assert_eq!(r.identity_files.len(), 16);
    }

    #[test]
    fn 실제_파일_시스템에서_include_글롭() {
        let dir = std::env::temp_dir().join(format!("antler-ssh-inc-{}", std::process::id()));
        let conf = dir.join(".ssh").join("conf.d");
        fs::create_dir_all(&conf).unwrap();
        fs::write(conf.join("10-a.conf"), "Host inc-a\n  Port 1111\n").unwrap();
        fs::write(conf.join("20-b.conf"), "\u{feff}Host inc-b\n  Port 2222\n").unwrap();
        fs::write(conf.join("skip.txt"), "Host inc-c\n  Port 3\n").unwrap();
        let home = dir.to_string_lossy().into_owned();
        let cfg = parse_ssh_config("Include conf.d/*.conf nope/*\nHost *\n  User tail\n", &home, &RealFs);
        assert_eq!(resolve_host(&cfg, "inc-a", &ctx(&no_env)).port, 1111);
        assert_eq!(resolve_host(&cfg, "inc-b", &ctx(&no_env)).port, 2222);
        assert_eq!(resolve_host(&cfg, "inc-c", &ctx(&no_env)).port, 22);
        assert_eq!(resolve_host(&cfg, "inc-a", &ctx(&no_env)).user, "tail");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn posix_홈은_identity_file_과_include_를_posix_경로로_푼다() {
        const POSIX_HOME: &str = "/Users/me";
        let fs = MemFs::new([("/Users/me/.ssh/conf.d/10-a.conf".to_string(), "Host inc-a\n  Port 1111\n".to_string())]);
        let cfg = parse_ssh_config(
            "Include conf.d/*.conf\nHost box\n  IdentityFile ~/.ssh/id_rsa\n  IdentityFile %d/.ssh/k_%r\n  IdentityFile /etc/ssh/../keys/x\n  User jo\n",
            POSIX_HOME,
            &fs,
        );
        let r = resolve_host(&cfg, "box", &ctx(&no_env));
        assert_eq!(r.identity_files, vec!["/Users/me/.ssh/id_rsa", "/Users/me/.ssh/k_jo", "/etc/keys/x"]);
        assert_eq!(resolve_host(&cfg, "inc-a", &ctx(&no_env)).port, 1111);
        assert_eq!(expand_tilde("~\\x", POSIX_HOME), "~\\x");
        assert_eq!(expand_tilde("~", POSIX_HOME), POSIX_HOME);
    }

    #[test]
    fn windows_홈은_여전히_win32_경로다() {
        let r = resolve("Host box\n  IdentityFile ~/.ssh/id_rsa\n  IdentityFile ~\\.ssh\\k\n", "box");
        assert_eq!(r.identity_files, vec!["C:\\h\\.ssh\\id_rsa", "C:\\h\\.ssh\\k"]);
    }

    #[test]
    fn 로컬_사용자_이름이_비지_않는다() {
        assert!(!local_user_name().is_empty());
    }
}
