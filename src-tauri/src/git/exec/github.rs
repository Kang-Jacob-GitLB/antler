//! GitHub 원격 전용 저장소 캐시의 git 실행기(`src/main/executors/LocalExecutor.ts` 의 `envFor` + `RepoService.createExecutor` 의 github 갈래, Phase 6b PR 2).
//!
//! 로컬 git 을 그대로 띄우되 **환경만** 다르다: 대상 경로가 앱 캐시 모양(`<기준>/<소문자 owner>/<소문자 repo>.git`, [`GithubCachePath`])을 통과할 때만
//! [`github_env`](crate::git::github_cache::github_env)(토큰·lazy fetch·https)를 붙이고, 아니면 `GIT_ENV` 그대로(전송 금지)다.
//! `run`·`stream` **둘 다** 같은 환경이다(lazy fetch 는 log·diff 스트림에서도 필요하다). 호출자가 환경을 고르지 못한다 —
//! `RunOpts.fetch_env` 는 무시한다(fetch 의 일반 저장소 환경이 이 환경을 덮어쓰지 못한다).
//!
//! 토큰은 호출 때마다 [`TokenSource`] 에서 읽어(로그인·로그아웃이 바로 반영된다) 환경 변수로만 넘기고, `run` 의 결과 stdout·stderr·오류 message 와
//! `stream` 의 `finish` 오류(message·stderr)에서는 가린다. `stream` 의 stdout 은 가리지 않는다(blob 같은 내용이다) —
//! 그래서 환경 설정을 드러내는 명령(`config --list` 등)의 출력은 `run` 으로만 읽는다(`read_config_entries`).
//! 캐시 경로에서는 부모 환경의 `GIT_*`(TLS 검증 끄기·trace·저장소 지정 등)를 물려주지 않는다([`inherited_env_to_remove`]).
//! 훅 차단 디렉터리가 쓸 수 없으면 환경을 만들지 못하고 실패한다(보호 없이 실행하지 않는다).

use std::sync::Arc;

use super::common::{build_git_args, merged_env, to_strings, RunOpts, RunResult, Spec};
use super::error::ExecError;
use super::process::{run_process, stream_process, GitStream};
use crate::git::fetch::guard::hooks_path_for;
use crate::git::github_cache::{github_env, GithubCachePath, GithubEnvOpts};
use crate::net::redact::{redact_token, SecretString};

/// 호출 때마다 읽는 GitHub 토큰(없으면 로그아웃 상태)
pub type TokenSource = Arc<dyn Fn() -> Option<SecretString> + Send + Sync>;
/// 사용자 설정에서 가져온 네트워크 키(`read_network_config` 의 결과). 읽는 쪽이 준비가 끝났는지를 정한다
pub type NetConfigSource = Arc<dyn Fn() -> Vec<(String, String)> + Send + Sync>;

#[derive(Clone)]
pub struct GithubExecutor {
    git_binary: String,
    cache_dir: String,
    no_hooks_dir: String,
    token: TokenSource,
    net_config: NetConfigSource,
    /// 허용 전송(시험만 바꾼다 — 프로덕션은 `https`)
    #[cfg(feature = "test-hooks")]
    allow_protocol: Option<String>,
}

impl GithubExecutor {
    /// `cache_dir`: 앱 캐시 기준 폴더. `no_hooks_dir`: 훅 차단용 앱 소유의 빈 디렉터리(드라이브 문자 경로)
    pub fn new(git_binary: impl Into<String>, cache_dir: impl Into<String>, no_hooks_dir: impl Into<String>, token: TokenSource, net_config: NetConfigSource) -> Self {
        GithubExecutor {
            git_binary: git_binary.into(),
            cache_dir: cache_dir.into(),
            no_hooks_dir: no_hooks_dir.into(),
            token,
            net_config,
            #[cfg(feature = "test-hooks")]
            allow_protocol: None,
        }
    }

    /// 허용 전송을 바꾼다(`file`·`http`). **테스트 전용** — 프로덕션 경로는 `https` 고정이다
    #[cfg(feature = "test-hooks")]
    #[doc(hidden)]
    pub fn with_protocol_for_test(mut self, protocol: impl Into<String>) -> Self {
        self.allow_protocol = Some(protocol.into());
        self
    }

    /// 실행 파일(캐시 키)
    pub fn git_binary(&self) -> &str {
        &self.git_binary
    }

    #[cfg(feature = "test-hooks")]
    fn protocol(&self) -> Option<&str> {
        self.allow_protocol.as_deref()
    }

    #[cfg(not(feature = "test-hooks"))]
    fn protocol(&self) -> Option<&str> {
        None
    }

    /// 이 경로에서 git 을 돌릴 `Spec` 과, 오류 문구에서 가릴 토큰. 캐시 밖 경로면 토큰을 읽지도 않는다
    fn spec<S: AsRef<str>>(&self, repo_path: &str, args: &[S]) -> Result<(Spec, Option<SecretString>), ExecError> {
        let (env, token, env_remove) = match GithubCachePath::new(&self.cache_dir, repo_path) {
            Some(path) => {
                let hooks = hooks_path_for(true, Some(&self.no_hooks_dir)).map_err(|message| ExecError::Other { message })?;
                let token = (self.token)();
                let net = (self.net_config)();
                let env = github_env(token.as_ref().map(SecretString::expose), &path, &GithubEnvOpts { hooks_path: &hooks, net_config: &net, allow_protocol: self.protocol() });
                // 캐시 경로의 git 은 부모의 위험한 git 환경 변수를 물려받지 않는다(캐시 밖 경로는 토큰이 없는 일반 local 환경이다)
                (env, token, inherited_env_to_remove(std::env::vars_os().filter_map(|(k, _)| k.into_string().ok())))
            }
            None => (merged_env(None), None, Vec::new()),
        };
        let spec = Spec { program: self.git_binary.clone(), argv: build_git_args(repo_path, args), env, env_remove, cwd: None, git_args: to_strings(args), wsl: false };
        Ok((spec, token))
    }

    /// stdout 을 전부 모아 돌려준다(상한 `opts.max_buffer`, 기본 64MB). 종료 코드가 0 이 아니어도 `Ok`. `opts.fetch_env` 는 무시한다
    pub async fn run<S: AsRef<str>>(&self, repo_path: &str, args: &[S], opts: &RunOpts) -> Result<RunResult, ExecError> {
        let (spec, token) = self.spec(repo_path, args)?;
        let token = token.as_ref().map(SecretString::expose);
        match run_process(&spec, opts).await {
            Ok(r) => Ok(RunResult { stdout: redact_bytes(r.stdout, token), stderr: redact_token(&r.stderr, token), ..r }),
            Err(e) => Err(redact_error(e, token)),
        }
    }

    /// stdout 을 바이트 스트림으로 준다. 같은 환경이다. 스트림이 내는 오류 문구(`finish`)에서는 토큰을 가린다
    pub fn stream<S: AsRef<str>>(&self, repo_path: &str, args: &[S], opts: &RunOpts) -> Result<GitStream, ExecError> {
        let (spec, token) = self.spec(repo_path, args)?;
        Ok(stream_process(&spec, opts)?.redacting(token))
    }
}

/// 캐시 경로의 git 이 부모 환경에서 **물려받지 않을** 이름: `GIT_` 로 시작하는 전부(대소문자 무시 — Windows 환경 변수가 그렇다)와 `SSLKEYLOGFILE`.
/// 허용 목록 방식이 아니라 "GIT_* 전부 제거 + 예외 둘"이다: git 이 읽는 `GIT_*` 중 이 실행기에 필요한 것은 [`github_env`](crate::git::github_cache::github_env) 가
/// 모두 직접 넣고, 부모 것은 위험하기만 하다 — `GIT_SSL_NO_VERIFY`(빈 값이어도 TLS 검증을 끈다)·`GIT_TRACE*`/`GIT_CURL_VERBOSE`(`GIT_TRACE_REDACT=0` 와 함께
/// Authorization 을 기록)·`GIT_EXEC_PATH`·`GIT_ALTERNATE_OBJECT_DIRECTORIES`·`GIT_NAMESPACE`·저장소 지정(`GIT_COMMON_DIR`·`GIT_OBJECT_DIRECTORY`·
/// `GIT_INDEX_FILE`·`GIT_WORK_TREE`·`GIT_CONFIG*`) 등. 목록을 일일이 세는 차단 방식은 새 변수가 생길 때마다 구멍이 된다.
/// 사용자의 사설 인증서 설정(`GIT_SSL_CAINFO`·`GIT_SSL_CAPATH`)만 남긴다(회사 프록시 뒤 fetch 용 — 신뢰를 넓힐 뿐 검증을 끄지 않는다)
fn inherited_env_to_remove(parent_names: impl Iterator<Item = String>) -> Vec<String> {
    parent_names
        .filter(|name| {
            let n = name.to_ascii_uppercase();
            (n.starts_with("GIT_") && n != "GIT_SSL_CAINFO" && n != "GIT_SSL_CAPATH") || n == "SSLKEYLOGFILE"
        })
        .collect()
}

/// stdout 바이트에서 토큰과 그 base64 를 가린다(`git config --list` 가 환경의 `Authorization` 헤더를 보여 주는 것을 막는 방어선).
/// 토큰이 없으면(로그아웃·캐시 밖) 그대로 돌려준다
fn redact_bytes(stdout: Vec<u8>, token: Option<&str>) -> Vec<u8> {
    let Some(token) = token.filter(|t| crate::net::redact::usable_token(t)) else { return stdout };
    let mut out = stdout;
    for needle in [crate::net::redact::git_basic_token(token).into_bytes(), token.as_bytes().to_vec()] {
        let mut replaced = Vec::with_capacity(out.len());
        let mut i = 0;
        while i < out.len() {
            if out[i..].starts_with(&needle) {
                replaced.extend_from_slice(b"***");
                i += needle.len();
            } else {
                replaced.push(out[i]);
                i += 1;
            }
        }
        out = replaced;
    }
    out
}

/// 오류 문구의 토큰·그 base64 를 가린다(`redactToken`)
fn redact_error(e: ExecError, token: Option<&str>) -> ExecError {
    match e {
        ExecError::Git { message, code, stderr, args } => ExecError::Git { message: redact_token(&message, token), code, stderr: redact_token(&stderr, token), args },
        ExecError::SpawnFailed { message } => ExecError::SpawnFailed { message: redact_token(&message, token) },
        ExecError::Other { message } => ExecError::Other { message: redact_token(&message, token) },
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exec(token: Option<&'static str>) -> GithubExecutor {
        GithubExecutor::new("git", "C:\\golden\\cache", "C:\\hooks", Arc::new(move || token.map(SecretString::new)), Arc::new(Vec::new))
    }

    #[test]
    fn spec_은_캐시_밖_경로에는_기본_환경만_준다() {
        // 훅 디렉터리 검사(실제 디스크)에 닿기 전에 슬러그로 갈린다
        let (s, token) = exec(Some("ghp_secret")).spec("C:\\elsewhere\\o\\r.git", &["log"]).unwrap();
        assert!(token.is_none(), "캐시 밖 경로는 토큰을 읽지 않는다");
        assert_eq!(s.env, merged_env(None));
        assert!(s.env.iter().all(|(_, v)| !v.contains("ghp_secret")));
    }

    #[test]
    fn 훅_디렉터리를_쓸_수_없으면_캐시_경로에서도_실패한다() {
        // 존재하지 않는 훅 디렉터리: 보호 없이 실행하지 않는다
        let err = exec(None).spec("C:\\golden\\cache\\o\\r.git", &["log"]).unwrap_err();
        assert!(matches!(err, ExecError::Other { .. }), "{err:?}");
    }

    #[test]
    fn fetch_인자에_토큰이_없고_환경의_한_값에만_있다() {
        let hooks = std::env::temp_dir().join(format!("antler-ghexec-hooks-{}", std::process::id()));
        std::fs::create_dir_all(&hooks).unwrap();
        let ex = GithubExecutor::new("git", "C:\\golden\\cache", hooks.to_string_lossy().into_owned(), Arc::new(|| Some(SecretString::new("ghp_secret"))), Arc::new(Vec::new));
        let r = ex.spec("C:\\golden\\cache\\o\\r.git", &["fetch", "--filter=blob:none"]);
        let _ = std::fs::remove_dir_all(&hooks);
        let (s, _) = r.unwrap();
        let b64 = crate::net::redact::git_basic_token("ghp_secret");
        assert!(s.argv.iter().chain(&s.git_args).all(|a| !a.contains("ghp_secret") && !a.contains(&b64)), "{:?}", s.argv);
        let holders: Vec<&str> = s.env.iter().filter(|(_, v)| v.contains(&b64)).map(|(k, _)| k.as_str()).collect();
        assert_eq!(holders.len(), 1);
        assert!(holders[0].starts_with("GIT_CONFIG_VALUE_"));
        assert!(s.env.iter().all(|(_, v)| !v.contains("ghp_secret")));
    }

    #[test]
    fn spec_의_debug_는_환경_값을_찍지_않는다() {
        let s = Spec { program: "git".into(), argv: vec![], env: vec![("GIT_CONFIG_VALUE_6".into(), "Authorization: Basic c2VjcmV0".into())], env_remove: vec![], cwd: None, git_args: vec![], wsl: false };
        let shown = format!("{s:?}");
        assert!(shown.contains("GIT_CONFIG_VALUE_6") && !shown.contains("c2VjcmV0"), "{shown}");
    }

    #[test]
    fn 부모_환경에서_git_변수를_대소문자_무시하고_지운다() {
        let names = ["GIT_SSL_NO_VERIFY", "git_trace_curl", "GIT_TRACE", "GIT_TRACE2_EVENT", "GIT_TRACE_REDACT", "GIT_CURL_VERBOSE", "GIT_EXEC_PATH", "Git_Common_Dir", "GIT_ALTERNATE_OBJECT_DIRECTORIES", "GIT_NAMESPACE", "GIT_OBJECT_DIRECTORY", "GIT_INDEX_FILE", "GIT_WORK_TREE", "GIT_CONFIG_GLOBAL", "GIT_CONFIG_COUNT", "GIT_DIR", "SSLKEYLOGFILE", "KEEP_ME", "PATH", "HTTPS_PROXY", "GIT_SSL_CAINFO", "git_ssl_capath"];
        let removed = inherited_env_to_remove(names.iter().map(|s| s.to_string()));
        for gone in &names[..17] {
            assert!(removed.iter().any(|r| r == gone), "{gone}");
        }
        for kept in &names[17..] {
            assert!(!removed.iter().any(|r| r == kept), "{kept}");
        }
    }

    #[test]
    fn stdout_바이트에서도_토큰과_base64_를_가린다() {
        let b64 = crate::net::redact::git_basic_token("ghp_secret");
        let raw = format!("a ghp_secret b Authorization: Basic {b64}\n").into_bytes();
        assert_eq!(String::from_utf8(redact_bytes(raw.clone(), Some("ghp_secret"))).unwrap(), "a *** b Authorization: Basic ***\n");
        assert_eq!(redact_bytes(raw.clone(), None), raw);
        let binary = vec![0u8, 255, 1, 2];
        assert_eq!(redact_bytes(binary.clone(), Some("ghp_secret")), binary);
    }

    #[test]
    fn 오류_문구에서_토큰을_가린다() {
        let e = redact_error(ExecError::Git { message: "boom ghp_secret".into(), code: 128, stderr: "fatal ghp_secret".into(), args: vec![] }, Some("ghp_secret"));
        assert_eq!(e, ExecError::Git { message: "boom ***".into(), code: 128, stderr: "fatal ***".into(), args: vec![] });
    }
}
