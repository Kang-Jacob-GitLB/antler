//! GitHub 원격 전용 저장소 캐시의 실행 기반(`src/main/git/githubCache.ts` 119-367 의 이식, Phase 6b PR 2 — 앱에 연결하지 않는다).
//!
//! 앱 캐시(`<userData>/remote-repos/<owner>/<repo>.git`)는 사용자 저장소에 쓰지 않는다는 규칙의 **유일한 예외**다. 이 모듈이 그 예외를 한 곳에 가둔다:
//! - [`GithubCachePath`] — 캐시 모양(`github_cache_slug`)을 통과한 경로만 담는 newtype. 환경을 만들고 저장소를 만드는 함수는 이 타입만 받는다.
//! - [`github_env`] — 토큰·lazy fetch·`https` 를 여는 실행 환경. **이 파일 한 곳에서만 만든다**(`GIT_NO_LAZY_FETCH` 를 `"0"` 으로 쓰는 곳과 `extraheader` 도 여기뿐이다 —
//!   `tests/github_cache.rs` 가 소스로 강제한다). 토큰은 환경 변수(`GIT_CONFIG_VALUE_n`)로만 git 에 간다(인자·디스크·로그 없음).
//! - 검사·초기화·HEAD 맞추기 — 로컬 설정은 허용 목록이 아니라 **기대값과 정확히 일치**하는지로 본다([`check_github_config`]). 일반 저장소의 fetch 허용 목록은 그대로다.
//!
//! git 은 [`GitRunner`] 로만 부른다(실제는 [`crate::git::exec::GitExecutor::Github`], 환경은 실행기가 붙인다).

use tokio_util::sync::CancellationToken;
use zeroize::Zeroizing;

use crate::git::commit::utf16_len;
use crate::git::exec::ssh::win32_path;
use crate::git::exec::{ExecError, RunOpts, GIT_ENV};
use crate::git::fetch::guard::{is_allowed_config, parse_config_list, ConfigEntry, HooksPath};
use crate::git::fetch::run::{read_config_entries, GitRunner};
use crate::git::fetch::js_trim;
use crate::git::github_link::{github_cache_slug, github_remote_url, is_safe_branch, GithubLink, GITHUB_ORIGIN};
use crate::net::redact::{git_basic_token, usable_token};

/// 캐시 모양(`<기준>/<소문자 owner>/<소문자 repo>.git`)을 통과한 저장소 폴더. [`GithubCachePath::new`] 만 만든다
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GithubCachePath {
    /// 호출자가 준 그대로(`git -C` 에 준다)
    path: String,
    /// `GIT_DIR` 로 줄 값: `path.win32.resolve` — 정규화하고 끝 구분자를 뗀다
    git_dir: String,
}

impl GithubCachePath {
    /// `path` 가 `cache_dir` 바로 아래 `<owner>/<repo>.git`(소문자)일 때만. 아니면 `None` — 그 경로의 git 은 환경이 열리지 않는다(`GIT_ENV` 그대로)
    pub fn new(cache_dir: &str, path: &str) -> Option<Self> {
        github_cache_slug(cache_dir, path)?;
        // slug 를 통과했으므로 루트가 아니다 — 끝 구분자만 뗀다
        let git_dir = win32_path::normalize(path).trim_end_matches('\\').to_string();
        Some(GithubCachePath { path: path.to_string(), git_dir })
    }

    pub fn as_str(&self) -> &str {
        &self.path
    }

    pub fn git_dir(&self) -> &str {
        &self.git_dir
    }
}

// ── 설정 ────────────────────────────────────────────────────────────────

/// init 직후 로컬 설정에 우리가 써 두는 키와 값(키는 `git config --list` 가 돌려주는 소문자, 쓰는 순서대로). `url` 이 `remote.origin.url` 이다
pub fn expected_github_config(url: &str) -> Vec<(String, String)> {
    [
        ("core.repositoryformatversion", "1"),
        ("extensions.partialclone", "origin"),
        ("remote.origin.url", url),
        ("remote.origin.promisor", "true"),
        ("remote.origin.partialclonefilter", "blob:none"),
    ]
    .iter()
    .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
    .collect()
}

/// `git init` 이 플랫폼에 따라 덧붙이는 키(값은 기존 허용 목록 규칙을 따른다). bare 라서 `core.bare=true` 도 항상 있는데,
/// 이 값은 일반 저장소 검사(`is_allowed_config`)에서는 허용하지 않으므로 여기서만 따로 받는다
const INIT_EXTRA_KEYS: [&str; 5] = ["core.filemode", "core.symlinks", "core.ignorecase", "core.logallrefupdates", "core.precomposeunicode"];

/// init 이 저장소 형식에 따라 덧붙일 수 있는 키와 허용 값. git 3.0 은 기본 ref 저장 형식이 reftable 이라 `extensions.refstorage=reftable` 을 쓴다.
/// 없어도 되고(필수 아님), 있으면 여기 적힌 값이어야 한다
const INIT_FORMAT_VALUES: [(&str, &[&str]); 2] = [("extensions.refstorage", &["files", "reftable"]), ("extensions.objectformat", &["sha1"])];

/// 원격 전용 저장소의 로컬(local/worktree scope) 설정이 우리가 만든 것과 정확히 같은지 본다. 문제가 없으면 빈 목록, 있으면 사유가 붙은 키 목록(값은 담지 않는다).
/// - 고정 키([`expected_github_config`])는 값이 정확히 같고 정확히 한 번 있어야 한다. 허용 목록으로 되돌아가지 않는다(`is_allowed_config` 는 `remote.*.url`·`promisor` 를 아무 값이나 통과시킨다)
/// - 고정 키가 아닌 것은 `INIT_EXTRA_KEYS` 에 `is_allowed_config` 를 통과하거나 `core.bare=true`, 또는 `INIT_FORMAT_VALUES` 의 값일 때만 허용한다.
///   `include.path` 는 고정 키를 가릴 수 있어 어느 쪽에도 없으므로 막힌다
pub fn check_github_config(entries: &[ConfigEntry], expected: &[(String, String)]) -> Vec<String> {
    let mut problems = Vec::new();
    let mut counts = vec![0usize; expected.len()];
    for e in entries {
        if e.scope != "local" && e.scope != "worktree" {
            continue;
        }
        if let Some(i) = expected.iter().position(|(k, _)| *k == e.key) {
            counts[i] += 1;
            if e.value.as_deref() != Some(expected[i].1.as_str()) {
                problems.push(format!("{} (예상과 다름)", e.key));
            }
        } else if e.key == "core.bare" {
            if e.value.as_deref() != Some("true") {
                problems.push(format!("{} (예상과 다름)", e.key));
            }
        } else if let Some((_, allowed)) = INIT_FORMAT_VALUES.iter().find(|(k, _)| *k == e.key) {
            if !e.value.as_deref().is_some_and(|v| allowed.contains(&v)) {
                problems.push(format!("{} (예상과 다름)", e.key));
            }
        } else if !(INIT_EXTRA_KEYS.contains(&e.key.as_str()) && is_allowed_config(&e.key, e.value.as_deref())) {
            problems.push(format!("{} (알 수 없는 설정)", e.key));
        }
    }
    for ((key, _), n) in expected.iter().zip(counts) {
        match n {
            0 => problems.push(format!("{key} (없음)")),
            1 => {}
            _ => problems.push(format!("{key} (중복)")),
        }
    }
    problems
}

fn cancel_opts(cancel: Option<&CancellationToken>) -> RunOpts {
    RunOpts { cancel: cancel.cloned(), ..RunOpts::default() }
}

/// fetch 직전에 하는 검사: 설정이 정확히 일치하고 remote 가 origin 하나뿐이어야 한다(옛 형식 `.git/remotes` 파일까지 `git remote` 가 읽는다).
/// 통과하면 빈 목록, 아니면 사유 목록. 설정을 읽지 못하면 Err(검사할 수 없으므로 fetch 하지 않는다)
pub async fn inspect_github_repo<R: GitRunner>(r: &R, dir: &GithubCachePath, link: &GithubLink, cancel: Option<&CancellationToken>) -> Result<Vec<String>, ExecError> {
    inspect_with_url(r, dir, &github_remote_url(&link.owner, &link.repo, GITHUB_ORIGIN), cancel).await
}

/// [`inspect_github_repo`] 에 원격 URL 의 출처를 주입한다. **테스트 전용**(프로덕션은 `https://github.com` 고정)
#[cfg(feature = "test-hooks")]
#[doc(hidden)]
pub async fn inspect_github_repo_for_test<R: GitRunner>(r: &R, dir: &GithubCachePath, link: &GithubLink, origin: &str, cancel: Option<&CancellationToken>) -> Result<Vec<String>, ExecError> {
    inspect_with_url(r, dir, &github_remote_url(&link.owner, &link.repo, origin), cancel).await
}

async fn inspect_with_url<R: GitRunner>(r: &R, dir: &GithubCachePath, url: &str, cancel: Option<&CancellationToken>) -> Result<Vec<String>, ExecError> {
    let opts = cancel_opts(cancel);
    let mut problems = check_github_config(&read_config_entries(r, dir.as_str(), &opts).await?, &expected_github_config(url));
    let out = r.run(dir.as_str(), &["remote".to_string()], &opts).await?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let names: Vec<&str> = stdout.split('\n').map(|n| n.strip_suffix('\r').unwrap_or(n)).filter(|n| !n.is_empty()).collect();
    if out.code != 0 || names != ["origin"] {
        problems.push("remote (origin 하나여야 함)".to_string());
    }
    Ok(problems)
}

// ── 실행 환경 ───────────────────────────────────────────────────────────

/// 격리된 github 환경으로 넘겨 주는 사용자 설정 키(전부 소문자). 회사 프록시·사설 인증서 뒤에서도 fetch 되게 하려는 것이다.
/// `http.sslVerify` 처럼 보안을 낮추는 키와 `url.*.insteadOf`·`credential.*` 는 일부러 넣지 않는다
const NETWORK_CONFIG_KEYS: [&str; 7] =
    ["http.proxy", "http.sslcainfo", "http.sslcapath", "http.sslbackend", "http.schannelcheckrevoke", "http.schannelusesslcainfo", "http.noproxy"];

/// `git config --list` 항목 중 시스템·전역 scope 의 네트워크 키만 고른다. 같은 키가 여럿이면 마지막 값(git 이 쓰는 값)만 남기고
/// 순서는 처음 나온 자리다(JS `Map` 과 같다)
pub fn pick_network_config(entries: &[ConfigEntry]) -> Vec<(String, String)> {
    let mut last: Vec<(String, String)> = Vec::new();
    for e in entries {
        let Some(value) = e.value.as_deref() else { continue };
        if (e.scope != "system" && e.scope != "global") || !NETWORK_CONFIG_KEYS.contains(&e.key.as_str()) || utf16_len(value) > 4096 {
            continue;
        }
        match last.iter_mut().find(|(k, _)| *k == e.key) {
            Some(slot) => slot.1 = value.to_string(),
            None => last.push((e.key.clone(), value.to_string())),
        }
    }
    last
}

/// 사용자 전역·시스템 git 설정에서 네트워크 키만 읽는다(앱 시작 때 한 번). github 환경은 사용자 설정을 통째로 끊으므로([`github_env`])
/// 프록시·인증서만 이렇게 따로 옮긴다. 읽을 때는 `GIT_CONFIG_COUNT/KEY/VALUE/PARAMETERS` 와 `GIT_DIR` 류를 비워 이미 격리된 값이나
/// 어떤 저장소의 설정이 섞이지 않게 한다. 읽지 못하면(git 없음·옛 git·시간 초과) 빈 목록이다
pub async fn read_network_config(git_binary: &str) -> Vec<(String, String)> {
    let mut picked = match crate::git::exec::read_user_config_list(git_binary).await {
        Some(out) => pick_network_config(&parse_config_list(&out)),
        None => Vec::new(),
    };
    // 사용자 설정에 프록시가 없으면 환경 변수의 것을 옮긴다(`github_env` 가 프록시를 고정하므로 그대로 두면 꺼진다)
    if let Some(p) = env_https_proxy(|n| std::env::var(n).ok()) {
        if !picked.iter().any(|(k, _)| k == "http.proxy") {
            picked.push(("http.proxy".to_string(), p));
        }
    }
    picked
}

/// 부모 환경의 https 프록시(curl 이 https 대상에 쓰는 순서: `https_proxy` → `all_proxy`, 소문자 먼저). 없으면 `None`
pub fn env_https_proxy(get: impl Fn(&str) -> Option<String>) -> Option<String> {
    ["https_proxy", "HTTPS_PROXY", "all_proxy", "ALL_PROXY"].iter().find_map(|n| get(n).filter(|v| !v.is_empty())).filter(|v| utf16_len(v) <= 4096)
}

/// `GIT_CONFIG_GLOBAL` 에 줘서 사용자 전역 설정을 읽지 않게 하는 빈 파일(git 이 `NUL`·`/dev/null` 을 직접 처리한다)
pub const EMPTY_GIT_CONFIG: &str = if cfg!(windows) { "NUL" } else { "/dev/null" };

/// [`github_env`] 의 입력. 토큰은 따로 받는다
pub struct GithubEnvOpts<'a> {
    /// `core.hooksPath` 로 줄 앱 소유의 빈 디렉터리(`hooks_path_for(true, …)` 의 결과)
    pub hooks_path: &'a HooksPath,
    /// 사용자 설정에서 가져온 네트워크 키([`read_network_config`]). 기본: 없음
    pub net_config: &'a [(String, String)],
    /// 허용 전송. 프로덕션은 `None`(= `https` 고정)이고 시험만 `Some` 을 준다(실행기의 `test-hooks` 진입점)
    pub allow_protocol: Option<&'a str>,
}

/// JS `{...env, [k]: v}`: 이미 있는 키는 제자리에서 값만 바뀌고 새 키는 뒤에 붙는다
fn set(env: &mut Vec<(String, String)>, k: &str, v: &str) {
    match env.iter_mut().find(|(ek, _)| ek == k) {
        Some(slot) => slot.1 = v.to_string(),
        None => env.push((k.to_string(), v.to_string())),
    }
}

/// GitHub 원격 전용 저장소에서 git 을 실행할 때의 환경. 조회 명령·lazy fetch·fetch 가 모두 이 함수를 거친다
/// (`GithubExecutor` 가 캐시 경로 안의 저장소에서만 이 환경을 쓴다 — 경로는 [`GithubCachePath`] 로 이미 판정됐다).
/// - `GIT_NO_LAZY_FETCH` 는 끈다(blob:none 저장소가 필요한 blob 을 그때그때 받는다). 사용자 환경에 1 이 있어도 이기도록 `"0"` 으로 둔다
/// - 전송은 https 만. LFS 객체는 받지 않는다. 프롬프트·askpass·GCM 대화는 막는다
/// - `credential.helper` 를 비워 사용자 자격 증명 도우미(GCM 등)를 부르지 않고, 토큰이 있으면 github.com 에만 Authorization 헤더를 붙인다.
///   토큰은 인자·디스크가 아니라 이 환경 변수(`GIT_CONFIG_VALUE_n`)로만 git 에 전달된다. **쓸 수 없는 토큰**([`usable_token`])은 넣지 않는다
/// - 격리: 시스템·전역 설정을 읽지 않고(`GIT_CONFIG_NOSYSTEM`·`GIT_CONFIG_GLOBAL`=빈 파일), 상속된 `GIT_CONFIG_COUNT`/`PARAMETERS` 도 버린다.
///   사용자의 insteadOf(ssh 로 바꿔 전송 금지에 걸림)·`init.defaultRefFormat`·`safe.bareRepository` 가 이 기능을 깨지 못하게 하려는 것이다.
///   프록시·인증서만 `net_config` 로 command scope 에 옮긴다
/// - 캐시 저장소의 로컬 config 가 TLS·프록시를 못 바꾸게 command scope 로 고정한다: `http.sslVerify=true`(일반·github.com URL 범위 둘 다),
///   `http.proxy`·`http.https://github.com/.proxy` = 사용자(또는 환경 변수) 프록시, 없으면 빈 값(프록시 없음)
/// - `GIT_DIR` 로 저장소를 고정하고 `GIT_CEILING_DIRECTORIES` 로 상위 탐색을 막는다. 캐시 폴더가 비었거나 깨져 있으면 `-C <폴더>` 가
///   상위 폴더의 다른 저장소로 올라가 그 저장소에 config 를 쓰거나 lazy fetch 환경을 적용하는 일이 없도록 그냥 실패시킨다
/// - 유지보수·훅·번들 URI·리다이렉트 추종은 끈다(리다이렉트를 따라가면 Authorization 헤더가 다른 호스트로 갈 수 있다)
///
/// 값과 **순서**는 TS `githubEnv` 와 같다(`tests/golden/github/env.json`). 돌려주는 값에 토큰이 든다 — 실행기 `Spec` 이 버릴 때 0 으로 덮는다
pub fn github_env(token: Option<&str>, path: &GithubCachePath, o: &GithubEnvOpts<'_>) -> Vec<(String, String)> {
    let mut env: Vec<(String, String)> = GIT_ENV.iter().map(|(k, v)| ((*k).to_string(), (*v).to_string())).collect();
    let ceiling = win32_path::dirname(path.git_dir()).replace('\\', "/");
    for (k, v) in [
        ("GIT_NO_LAZY_FETCH", "0"),
        ("GIT_ALLOW_PROTOCOL", o.allow_protocol.unwrap_or("https")),
        ("GIT_LFS_SKIP_SMUDGE", "1"),
        ("GIT_TERMINAL_PROMPT", "0"),
        ("GIT_ASKPASS", ""),
        ("SSH_ASKPASS_REQUIRE", "never"),
        ("GCM_INTERACTIVE", "never"),
        ("GIT_DIR", path.git_dir()),
        ("GIT_CEILING_DIRECTORIES", ceiling.as_str()),
        ("GIT_CONFIG_NOSYSTEM", "1"),
        ("GIT_CONFIG_GLOBAL", EMPTY_GIT_CONFIG),
        ("GIT_CONFIG_PARAMETERS", ""),
    ] {
        set(&mut env, k, v);
    }
    let mut extra: Vec<(&str, Zeroizing<String>)> = o.net_config.iter().map(|(k, v)| (k.as_str(), Zeroizing::new(v.clone()))).collect();
    // 사용자 프록시가 없으면 빈 값으로 고정한다(`read_network_config` 가 환경 변수의 프록시를 이미 net_config 에 옮겨 둔다)
    let user_proxy = o.net_config.iter().find(|(k, _)| k == "http.proxy").map(|(_, v)| v.as_str());
    if user_proxy.is_none() {
        extra.push(("http.proxy", Zeroizing::new(String::new())));
    }
    for (k, v) in [
        ("credential.helper", ""),
        ("maintenance.auto", "false"),
        ("gc.auto", "0"),
        ("core.hooksPath", o.hooks_path.as_str()),
        ("transfer.bundleURI", "false"),
        ("http.followRedirects", "false"),
        ("http.sslVerify", "true"),
        ("http.https://github.com/.sslVerify", "true"),
        ("http.https://github.com/.proxy", user_proxy.unwrap_or("")),
    ] {
        extra.push((k, Zeroizing::new(v.to_string())));
    }
    if let Some(token) = token.filter(|t| usable_token(t)) {
        extra.push(("http.https://github.com/.extraheader", Zeroizing::new(format!("Authorization: Basic {}", git_basic_token(token)))));
    }
    for (i, (k, v)) in extra.iter().enumerate() {
        set(&mut env, &format!("GIT_CONFIG_KEY_{i}"), k);
        set(&mut env, &format!("GIT_CONFIG_VALUE_{i}"), v);
    }
    set(&mut env, "GIT_CONFIG_COUNT", &extra.len().to_string());
    env
}

// ── 초기화·HEAD ─────────────────────────────────────────────────────────

fn git_failure(args: &[String], code: i32, stderr: &str) -> ExecError {
    let message = match js_trim(stderr) {
        "" => format!("git {} 실패({code})", args.first().map_or("", String::as_str)),
        t => t.to_string(),
    };
    ExecError::Git { message, code, stderr: stderr.to_string(), args: args.to_vec() }
}

async fn must_run<R: GitRunner>(r: &R, dir: &GithubCachePath, args: &[&str], opts: &RunOpts) -> Result<String, ExecError> {
    let args: Vec<String> = args.iter().map(|s| (*s).to_string()).collect();
    let out = r.run(dir.as_str(), &args, opts).await?;
    if out.code != 0 {
        return Err(git_failure(&args, out.code, &out.stderr));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// 빈 폴더 `dir` 에 원격 전용 저장소를 만든다(네트워크 없음). `--template=` 로 사용자의 init 템플릿·훅이 복사되지 않게 하고,
/// 기본 브랜치를 알면 HEAD 를 `refs/remotes/origin/<브랜치>` 로 미리 가리켜 둔다(첫 fetch 가 끝나면 그대로 그 브랜치가 열린다).
/// 호출자는 `dir` 을 이미 만들어 두어야 하고, 실패하면 `dir` 을 정리해야 한다. 기본 브랜치가 `is_safe_branch` 를 통과하지 않으면 Err
pub async fn init_github_repo<R: GitRunner>(r: &R, dir: &GithubCachePath, link: &GithubLink, cancel: Option<&CancellationToken>) -> Result<(), ExecError> {
    init_with_url(r, dir, link, &github_remote_url(&link.owner, &link.repo, GITHUB_ORIGIN), cancel).await
}

/// [`init_github_repo`] 에 원격 URL 의 출처를 주입한다. **테스트 전용**(프로덕션은 `https://github.com` 고정)
#[cfg(feature = "test-hooks")]
#[doc(hidden)]
pub async fn init_github_repo_for_test<R: GitRunner>(r: &R, dir: &GithubCachePath, link: &GithubLink, origin: &str, cancel: Option<&CancellationToken>) -> Result<(), ExecError> {
    init_with_url(r, dir, link, &github_remote_url(&link.owner, &link.repo, origin), cancel).await
}

async fn init_with_url<R: GitRunner>(r: &R, dir: &GithubCachePath, link: &GithubLink, url: &str, cancel: Option<&CancellationToken>) -> Result<(), ExecError> {
    if let Some(b) = &link.default_branch {
        if !is_safe_branch(b) {
            return Err(ExecError::Other { message: "쓸 수 없는 기본 브랜치 이름입니다".to_string() });
        }
    }
    let opts = cancel_opts(cancel);
    must_run(r, dir, &["init", "-q", "--bare", "--template="], &opts).await?;
    for (key, value) in expected_github_config(url) {
        must_run(r, dir, &["config", "--local", &key, &value], &opts).await?;
    }
    if let Some(b) = &link.default_branch {
        must_run(r, dir, &["symbolic-ref", "HEAD", &format!("refs/remotes/origin/{b}")], &opts).await?;
    }
    Ok(())
}

/// `ls-remote --symref origin HEAD` 출력에서 기본 브랜치: `/^ref: refs\/heads\/(\S+)\tHEAD$/m` 의 첫 일치
fn ls_remote_head(out: &str) -> Option<&str> {
    out.split(['\n', '\r', '\u{2028}', '\u{2029}']).find_map(|line| {
        let b = line.strip_prefix("ref: refs/heads/")?.strip_suffix("\tHEAD")?;
        (!b.is_empty() && !b.chars().any(crate::git::scope::is_js_space)).then_some(b)
    })
}

/// fetch 뒤에 HEAD 가 받아 온 기본 브랜치를 가리키게 맞춘다(그래야 로그의 HEAD·사이드바 기본 브랜치가 열린다).
/// 기본 브랜치는 ① 레코드에 있고 실제로 받아졌으면 그것, ② `refs/remotes/origin/HEAD`(git 2.48+ 가 fetch 때 만든다),
/// ③ `ls-remote --symref origin HEAD` 순서로 알아낸다. 알아낸 브랜치 이름을 돌려주고, 모르면(빈 저장소 등) `None`. 이름은 `is_safe_branch` 를 통과해야 한다.
/// ls-remote 는 네트워크 호출이라 fetch 와 같은 환경(실행기)·인자(`net_args`)로 돌린다. HEAD 갱신은 캐시 저장소 안의 ref 쓰기다
pub async fn ensure_github_head<R: GitRunner>(
    r: &R,
    dir: &GithubCachePath,
    link: &GithubLink,
    cancel: Option<&CancellationToken>,
    net_args: &[String],
) -> Result<Option<String>, ExecError> {
    let opts = cancel_opts(cancel);
    let has = |b: String| {
        let opts = opts.clone();
        async move {
            let args = ["rev-parse", "--verify", "--quiet", &format!("refs/remotes/origin/{b}^{{commit}}")].map(str::to_string);
            Ok::<bool, ExecError>(r.run(dir.as_str(), &args, &opts).await?.code == 0)
        }
    };
    let symref = |name: &str| {
        let opts = opts.clone();
        let args = ["symbolic-ref", "--quiet", name].map(str::to_string);
        async move {
            let out = r.run(dir.as_str(), &args, &opts).await?;
            Ok::<Option<String>, ExecError>((out.code == 0).then(|| js_trim(&String::from_utf8_lossy(&out.stdout)).to_string()))
        }
    };
    let mut branch: Option<String> = None;
    if let Some(b) = &link.default_branch {
        if has(b.clone()).await? {
            branch = Some(b.clone());
        }
    }
    if branch.is_none() {
        let sym = symref("refs/remotes/origin/HEAD").await?.unwrap_or_default();
        if let Some(b) = sym.strip_prefix("refs/remotes/origin/") {
            // `(.+)` 는 줄 끝 문자를 맞추지 않는다
            if !b.is_empty() && !b.contains(['\n', '\r', '\u{2028}', '\u{2029}']) && is_safe_branch(b) && has(b.to_string()).await? {
                branch = Some(b.to_string());
            }
        }
    }
    if branch.is_none() {
        let mut args: Vec<String> = net_args.to_vec();
        args.extend(["ls-remote", "--symref", "origin", "HEAD"].map(str::to_string));
        let out = r.run(dir.as_str(), &args, &opts).await?;
        if out.code == 0 {
            if let Some(b) = ls_remote_head(&String::from_utf8_lossy(&out.stdout)) {
                if is_safe_branch(b) && has(b.to_string()).await? {
                    branch = Some(b.to_string());
                }
            }
        }
    }
    let Some(branch) = branch else { return Ok(None) };
    let target = format!("refs/remotes/origin/{branch}");
    for name in ["HEAD", "refs/remotes/origin/HEAD"] {
        if symref(name).await?.as_deref() != Some(target.as_str()) {
            must_run(r, dir, &["symbolic-ref", name, &target], &opts).await?;
        }
    }
    Ok(Some(branch))
}

/// 캐시 폴더 삭제를 다시 시도하는 횟수·간격(TS `REMOVE_RETRIES`·`REMOVE_RETRY_DELAY_MS`) — 백신·인덱서가 파일을 쥐고 있을 수 있다
const REMOVE_RETRIES: u32 = 10;
const REMOVE_RETRY_DELAY: std::time::Duration = std::time::Duration::from_millis(100);

/// 캐시 폴더를 지운다(TS `removeCacheDir`). **`GithubCachePath` 만 받는다** — `<캐시 기준>/<owner>/<repo>.git` 모양을 통과한 경로라 다른 곳을 지울 수 없다.
/// owner 폴더나 저장소 폴더가 링크(정션 포함)면 링크 너머의 실제 폴더를 지우게 되므로 아무것도 지우지 않고 `Ok(false)` 다. 폴더가 없어도 `Ok(false)`.
/// 지웠으면 `Ok(true)` 이고 비게 된 owner 폴더도 정리한다(다른 저장소가 있으면 남는다). 파일을 쥔 프로세스가 있어 끝내 못 지우면 `Err`.
///
/// 읽기 전용 pack 이 든 폴더도 `remove_dir_all` 이 그대로 지운다(`github_cache_exec.rs` 시험). **연결은 PR 9** — 지금 `repos:remove` 의 삭제는 fetch·감시를 쥔 사이드카가
/// 멈춘 뒤에 한다(TS `RepoService.removeGithubCache` — 레코드의 owner/repo 와 캐시 경로가 맞을 때만 부른다). 호출자는 이 함수를 부르기 전에 이 저장소의 git(조회·fetch·감시)을 모두 멈춰야 한다
pub fn remove_github_cache(path: &GithubCachePath) -> Result<bool, String> {
    let repo = std::path::Path::new(path.as_str());
    let Some(owner) = repo.parent() else { return Ok(false) };
    match (std::fs::symlink_metadata(owner), std::fs::symlink_metadata(repo)) {
        (Ok(o), Ok(r)) => {
            if o.file_type().is_symlink() || r.file_type().is_symlink() {
                return Ok(false);
            }
        }
        // 없다
        _ => return Ok(false),
    }
    let mut last = None;
    for attempt in 0..=REMOVE_RETRIES {
        match std::fs::remove_dir_all(repo) {
            Ok(()) => {
                last = None;
                break;
            }
            // 이미 지워졌다
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                last = None;
                break;
            }
            Err(e) => {
                last = Some(e);
                if attempt < REMOVE_RETRIES {
                    std::thread::sleep(REMOVE_RETRY_DELAY);
                }
            }
        }
    }
    if let Some(e) = last {
        return Err(e.to_string());
    }
    // 소유자 폴더가 비었으면 함께 정리한다(비어 있지 않으면 실패하므로 무시한다)
    let _ = std::fs::remove_dir(owner);
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::fetch::guard::hooks_path_for_test;

    fn entry(key: &str, value: Option<&str>, scope: &str) -> ConfigEntry {
        ConfigEntry { scope: scope.to_string(), key: key.to_string(), value: value.map(str::to_string) }
    }

    #[test]
    fn 캐시_경로만_담는다() {
        let base = "C:\\golden\\cache";
        let p = GithubCachePath::new(base, "C:\\golden\\cache\\octocat\\hello.git").unwrap();
        assert_eq!(p.git_dir(), "C:\\golden\\cache\\octocat\\hello.git");
        // 끝 구분자·슬래시 표기는 GIT_DIR 에서 정규화된다
        let q = GithubCachePath::new(base, "C:/golden/cache/octocat/hello.git/").unwrap();
        assert_eq!(q.git_dir(), "C:\\golden\\cache\\octocat\\hello.git");
        for bad in ["C:\\golden\\cache", "C:\\golden\\cache\\octocat", "C:\\golden\\cache\\Octocat\\hello.git", "C:\\golden\\other\\octocat\\hello.git", "relative\\octocat\\hello.git"] {
            assert!(GithubCachePath::new(base, bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn 환경은_캐시_밖_경로에는_만들_수_없고_토큰은_한_값에만_있다() {
        let hooks = hooks_path_for_test(true, Some("C:/hooks"), true, |_| true).unwrap();
        let path = GithubCachePath::new("C:\\golden\\cache", "C:\\golden\\cache\\o\\r.git").unwrap();
        let env = github_env(Some("ghp_secret"), &path, &GithubEnvOpts { hooks_path: &hooks, net_config: &[], allow_protocol: None });
        let b64 = git_basic_token("ghp_secret");
        let holders: Vec<&str> = env.iter().filter(|(_, v)| v.contains("ghp_secret") || v.contains(&b64)).map(|(k, _)| k.as_str()).collect();
        assert_eq!(holders.len(), 1, "{holders:?}");
        assert!(holders[0].starts_with("GIT_CONFIG_VALUE_"));
        assert!(env.contains(&("GIT_NO_LAZY_FETCH".to_string(), "0".to_string())));
        assert_eq!(env.last().unwrap(), &("GIT_CONFIG_COUNT".to_string(), "11".to_string()));
    }

    #[test]
    fn 쓸_수_없는_토큰은_헤더에_넣지_않는다() {
        let hooks = hooks_path_for_test(true, Some("C:/hooks"), true, |_| true).unwrap();
        let path = GithubCachePath::new("C:\\golden\\cache", "C:\\golden\\cache\\o\\r.git").unwrap();
        for t in ["a\nb", "", "a b", &"x".repeat(4097)] {
            let env = github_env(Some(t), &path, &GithubEnvOpts { hooks_path: &hooks, net_config: &[], allow_protocol: None });
            assert!(env.iter().all(|(_, v)| !v.contains("Authorization")), "{t:?}");
            assert_eq!(env.last().unwrap().1, "10", "토큰 없이 설정은 열 개다");
        }
    }

    #[test]
    fn ls_remote_출력에서_기본_브랜치를_찾는다() {
        assert_eq!(ls_remote_head("ref: refs/heads/main\tHEAD\nabc\tHEAD\n"), Some("main"));
        assert_eq!(ls_remote_head("ref: refs/heads/release/1.x\tHEAD\r\nabc\tHEAD"), Some("release/1.x"));
        assert_eq!(ls_remote_head("abc\tHEAD\n"), None);
        assert_eq!(ls_remote_head("ref: refs/heads/\tHEAD"), None);
    }

    #[test]
    fn 설정_검사는_범위와_개수를_본다() {
        let expected = expected_github_config("https://github.com/o/r.git");
        let healthy: Vec<ConfigEntry> = expected.iter().map(|(k, v)| entry(k, Some(v), "local")).collect();
        assert!(check_github_config(&healthy, &expected).is_empty());
        let mut bad = healthy.clone();
        bad.push(entry("include.path", Some("../x"), "local"));
        assert_eq!(check_github_config(&bad, &expected), ["include.path (알 수 없는 설정)"]);
        let mut dup = healthy;
        dup.push(entry("remote.origin.url", Some("https://evil.example/x.git"), "local"));
        assert_eq!(check_github_config(&dup, &expected), ["remote.origin.url (예상과 다름)", "remote.origin.url (중복)"]);
    }
}
