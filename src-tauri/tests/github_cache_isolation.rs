//! GitHub 캐시 실행 기반의 **사용자 git 설정 격리** 시험(Phase 6b PR 2) — 프로세스 전역 git 설정이 적대적인 별도 바이너리다.
//! TS `githubRemote.test.ts` 의 "실제 git: 사용자 git 설정과 격리"·`readNetworkConfig` 를 옮겼다. 런타임 `set_var` 는 다른 스레드가 환경을 읽는 중이면 위험해
//! (TS 의 `withEnv` 를 옮길 수 없다) 적대적 값은 시작 때(`isolate`, ctor) 한 번 넣는다. 준비용 git(`ghcache::raw`)은 깨끗한 전역 설정을 따로 받는다.
//! 적대적 전역 설정(`tests/gitconfig_github`): `url.insteadOf`(ssh)·`safe.bareRepository=explicit`·`init.defaultRefFormat=reftable`·프록시·`sslVerify=false`·`credential.helper`.
//! 환경 변수로도 같은 것을 넣는다: `GIT_DEFAULT_REF_FORMAT=reftable`, `GIT_CONFIG_COUNT`(command scope 의 프록시).

mod ghcache;

use antler_lib::git::exec::{GitExecutor, LocalExecutor, RunOpts};
use antler_lib::git::fetch::guard::version_at_least;
use antler_lib::git::fetch::run::{git_version, run_github_fetch, GithubFetchOpts};
use antler_lib::git::github_cache::{ensure_github_head, init_github_repo_for_test, inspect_github_repo_for_test, read_network_config};
use ghcache::{link, make_source, raw, text, Cache};

// unsafe: main 이전 코드는 Rust 런타임 초기화 전에 돈다 — 여기서는 환경 변수만 넣고 다른 것에 기대지 않는다
#[ctor::ctor(unsafe)]
fn isolate() {
    std::env::set_var("GIT_CONFIG_GLOBAL", std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("gitconfig_github"));
    std::env::set_var("GIT_CONFIG_NOSYSTEM", "1");
    std::env::set_var("GCM_INTERACTIVE", "never");
    for k in ["HTTP_PROXY", "http_proxy", "HTTPS_PROXY", "https_proxy", "ALL_PROXY", "all_proxy", "GIT_ASKPASS", "SSH_ASKPASS", "GIT_SSH", "GIT_SSH_COMMAND", "GIT_ALLOW_PROTOCOL", "GIT_TERMINAL_PROMPT"] {
        std::env::remove_var(k);
    }
    std::env::set_var("GIT_DEFAULT_REF_FORMAT", "reftable");
    // 환경으로 주입된 설정(command scope) — 사용자 네트워크 설정으로 가져오지 않고, github 환경이 상속하지 않는다
    std::env::set_var("GIT_CONFIG_COUNT", "1");
    std::env::set_var("GIT_CONFIG_KEY_0", "http.proxy");
    std::env::set_var("GIT_CONFIG_VALUE_0", "http://from-env:1");
}

async fn whole_flow(cache: &Cache, exec: &GitExecutor, src: &ghcache::Source) {
    let dir = cache.repo("octocat", "iso");
    let l = link("octocat", "iso", None);
    init_github_repo_for_test(exec, &dir, &l, &src.origin(), None).await.expect("init");
    assert_eq!(inspect_github_repo_for_test(exec, &dir, &l, &src.origin(), None).await.expect("inspect"), Vec::<String>::new());
    run_github_fetch(exec, dir.as_str(), &GithubFetchOpts { cancel: None, no_hooks_dir: &cache.hooks }).await.expect("fetch");
    assert_eq!(ensure_github_head(exec, &dir, &l, None, &[]).await.unwrap().as_deref(), Some("main"));
    let o = RunOpts::default();
    let log = exec.run(dir.as_str(), &["log", "--format=%s", "HEAD"], &o).await.unwrap();
    assert_eq!(log.code, 0, "{}", log.stderr);
    assert_eq!(text(&log.stdout).lines().collect::<Vec<_>>(), ["second", "first"]);
    let v = git_version(&GitExecutor::Local(LocalExecutor::default())).await;
    if version_at_least(v, [2.0, 45.0, 1.0]) {
        let numstat = exec.run(dir.as_str(), &["diff-tree", "-r", "--numstat", "--no-commit-id", src.first.as_str(), src.second.as_str()], &o).await.unwrap();
        assert_eq!(numstat.stderr, "");
        assert!(String::from_utf8_lossy(&numstat.stdout).contains("1\t0\tf.txt"));
    }
}

#[tokio::test]
async fn 전역_설정이_insteadof_safe_bare_repository_reftable_이어도_끝까지_동작한다() {
    let (src, cache) = (make_source("octocat", "iso"), Cache::new());
    let exec = cache.executor("file", None);
    // 대조: 격리하지 않은 실행기에서는 이 전역 설정이 bare 저장소 접근을 막는다(git 2.38+) — 이 시험이 공허하지 않다는 증거
    let v = git_version(&GitExecutor::Local(LocalExecutor::default())).await;
    if version_at_least(v, [2.0, 38.0, 0.0]) {
        let control = ghcache::TempDir::new("control-bare");
        assert!(raw("git").args(["init", "-q", "--bare"]).arg(control.path()).output().unwrap().status.success());
        let r = GitExecutor::Local(LocalExecutor::default()).run(&control.str(), &["rev-parse", "--git-dir"], &RunOpts::default()).await.unwrap();
        assert_ne!(r.code, 0, "전역 safe.bareRepository=explicit 가 적용되지 않았다 — 적대적 설정이 걸리지 않은 시험이다");
    }
    whole_flow(&cache, &exec, &src).await;
}

#[tokio::test]
async fn init_이_reftable_저장소를_만들어도_검사를_통과하고_fetch_log_가_된다() {
    let v = git_version(&GitExecutor::Local(LocalExecutor::default())).await;
    if !version_at_least(v, [2.0, 45.0, 0.0]) {
        return eprintln!("[건너뜀] GIT_DEFAULT_REF_FORMAT 은 git 2.45+");
    }
    let (src, cache) = (make_source("octocat", "iso"), Cache::new());
    let exec = cache.executor("file", None);
    whole_flow(&cache, &exec, &src).await;
    let dir = cache.repo("octocat", "iso");
    let fmt = exec.run(dir.as_str(), &["config", "--local", "extensions.refstorage"], &RunOpts::default()).await.unwrap();
    assert_eq!(text(&fmt.stdout), "reftable", "환경의 GIT_DEFAULT_REF_FORMAT 이 init 에 닿지 않았다");
}

#[tokio::test]
async fn 사용자_설정에서_네트워크_키만_읽고_환경의_git_config_에_오염되지_않는다() {
    let got = read_network_config("git").await;
    // sslVerify·extraHeader·insteadOf·credential 은 가져오지 않고, 환경으로 주입된 프록시(command scope)도 아니다
    assert_eq!(got, [("http.proxy".to_string(), "http://proxy.corp:8080".to_string()), ("http.sslcainfo".to_string(), "/etc/ca.pem".to_string())]);
    // git 을 실행하지 못하면 빈 목록이다
    assert!(read_network_config("git-does-not-exist-antler").await.is_empty());
    // 대조: 같은 환경에서 날 `git config --list --show-scope` 는 command scope 의 프록시를 보여 준다
    let out = raw("git").args(["config", "--list", "--show-scope"]).env("GIT_CONFIG_GLOBAL", std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("gitconfig_github")).env("GIT_CONFIG_COUNT", "1").env("GIT_CONFIG_KEY_0", "http.proxy").env("GIT_CONFIG_VALUE_0", "http://from-env:1").output().unwrap();
    assert!(String::from_utf8_lossy(&out.stdout).contains("command\thttp.proxy=http://from-env:1"));
}
