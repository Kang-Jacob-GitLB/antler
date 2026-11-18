//! GitHub 캐시 실행기가 **부모 프로세스의 위험한 git 환경 변수를 물려주지 않는지** 보는 시험(Phase 6b PR 2 보안 수정) — 프로세스 전역 환경이
//! 적대적인 별도 바이너리다(`github_cache_isolation.rs` 와 같은 이유로 시작 때 한 번만 넣는다). 준비용 git(`ghcache::raw`)은 이 변수들을 지우고 돈다.
//! 넣는 값: `GIT_SSL_NO_VERIFY`(빈 값이어도 TLS 검증을 끈다)·`GIT_TRACE*`·`GIT_CURL_VERBOSE`·`GIT_TRACE_REDACT=0`(Authorization 기록)·
//! `GIT_COMMON_DIR`(다른 저장소의 `config` 를 쓰게 만든다)·`GIT_NAMESPACE`·`GIT_ALTERNATE_OBJECT_DIRECTORIES`.

mod ghcache;

use std::fs;
use std::path::PathBuf;

use antler_lib::git::exec::{GitExecutor, LocalExecutor, RunOpts};
use antler_lib::git::github_cache::init_github_repo_for_test;
use ghcache::{file_url, link, raw, raw_git, Cache, TempDir};

const TOKEN: &str = "ghp_golden_fake_token";

fn victim_dir() -> PathBuf {
    std::env::temp_dir().join(format!("antler-envvictim-{}", std::process::id()))
}

fn trace_file(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("antler-envtrace-{name}-{}.log", std::process::id()))
}

// unsafe: main 이전 코드는 Rust 런타임 초기화 전에 돈다 — 여기서는 환경 변수만 넣고 다른 것에 기대지 않는다
#[ctor::ctor(unsafe)]
fn hostile_parent_env() {
    std::env::set_var("GIT_SSL_NO_VERIFY", "");
    std::env::set_var("GIT_TRACE", trace_file("trace"));
    std::env::set_var("GIT_TRACE_CURL", trace_file("curl"));
    std::env::set_var("GIT_TRACE2_EVENT", trace_file("trace2"));
    std::env::set_var("GIT_TRACE_REDACT", "0");
    std::env::set_var("GIT_CURL_VERBOSE", "1");
    std::env::set_var("GIT_COMMON_DIR", victim_dir().join(".git"));
    std::env::set_var("GIT_NAMESPACE", "evil");
    std::env::set_var("GIT_ALTERNATE_OBJECT_DIRECTORIES", victim_dir().join("alt"));
}

const HOSTILE: [&str; 9] =
    ["GIT_SSL_NO_VERIFY", "GIT_TRACE", "GIT_TRACE_CURL", "GIT_TRACE2_EVENT", "GIT_TRACE_REDACT", "GIT_CURL_VERBOSE", "GIT_COMMON_DIR", "GIT_NAMESPACE", "GIT_ALTERNATE_OBJECT_DIRECTORIES"];

/// 두 시험이 같은 피해 저장소 경로(`GIT_COMMON_DIR` 는 프로세스 전역이다)를 쓰므로 하나씩 돈다
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// 피해 저장소(사용자의 일반 저장소 흉내)를 만든다. 준비용 git 은 위험한 변수를 지우고 돈다
fn make_victim() -> (PathBuf, Vec<u8>) {
    let dir = victim_dir();
    fs::create_dir_all(&dir).unwrap();
    raw_git(&dir, &["init", "-q", "-b", "main"]);
    let config = fs::read(dir.join(".git").join("config")).unwrap();
    (dir, config)
}

#[tokio::test]
async fn 부모의_위험한_git_환경_변수가_캐시_경로의_git_에_닿지_않는다() {
    let _serial = SERIAL.lock().await;
    let (victim, _) = make_victim();
    // 대조: 이 바이너리의 부모 환경에는 정말로 들어 있고, 격리하지 않은 로컬 실행기의 git 은 그대로 받는다
    for k in HOSTILE {
        assert!(std::env::var_os(k).is_some(), "{k} 가 부모 환경에 없다 — 이 시험이 공허하다");
    }
    let local = GitExecutor::Local(LocalExecutor::default());
    let o = RunOpts::default();
    let seen = local.run(&victim.to_string_lossy(), &["-c", "alias.e=!env", "e"], &o).await.unwrap();
    let seen = String::from_utf8_lossy(&seen.stdout).to_uppercase();
    assert!(seen.contains("GIT_SSL_NO_VERIFY=") && seen.contains("GIT_TRACE_CURL=") && seen.contains("GIT_COMMON_DIR="), "대조군이 환경을 받지 못했다: {seen}");

    // 캐시 경로의 git 은 받지 않는다(저장소를 만든 뒤 alias 로 실제 환경을 본다 — 토큰 헤더가 환경에 있는 경로다)
    // 대조군이 남긴 trace 파일을 치우고 시작한다
    for name in ["trace", "curl", "trace2"] {
        let _ = fs::remove_file(trace_file(name));
    }
    let cache = Cache::new();
    let exec = cache.executor("file", Some(TOKEN));
    let dir = cache.repo("octocat", "env");
    let origin = TempDir::new("origin");
    init_github_repo_for_test(&exec, &dir, &link("octocat", "env", None), &file_url(origin.path()), None).await.expect("init");
    let r = exec.run(dir.as_str(), &["-c", "alias.e=!env", "e"], &o).await.unwrap();
    assert_eq!(r.code, 0, "{}", r.stderr);
    let out = String::from_utf8_lossy(&r.stdout).to_uppercase();
    for k in HOSTILE {
        assert!(!out.lines().any(|l| l.starts_with(&format!("{k}="))), "{k} 가 캐시 경로의 git 환경에 남았다:\n{out}");
    }
    assert!(out.contains("GIT_DIR=") && out.contains("GIT_CONFIG_COUNT="), "github 환경이 지워졌다(지우기가 덧씌움보다 먼저여야 한다):\n{out}");
    // trace 파일은 만들어지지 않았다(토큰 헤더가 기록될 길이 없다)
    for name in ["trace", "curl", "trace2"] {
        assert!(!trace_file(name).exists(), "{name}: 캐시 경로의 git 이 GIT_TRACE 를 따라 기록했다");
    }
    let _ = fs::remove_dir_all(&victim);
}

#[tokio::test]
async fn git_common_dir_상속으로_사용자_저장소의_config_가_덮어써지지_않는다() {
    let _serial = SERIAL.lock().await;
    let (victim, before) = make_victim();
    let cache = Cache::new();
    let exec = cache.executor("file", None);
    let dir = cache.repo("octocat", "common");
    let origin = TempDir::new("origin");
    init_github_repo_for_test(&exec, &dir, &link("octocat", "common", None), &file_url(origin.path()), None).await.expect("init");
    // 피해 저장소의 .git/config 는 그대로다(init 의 `git config --local` 이 GIT_COMMON_DIR 를 따라가지 않았다)
    assert_eq!(fs::read(victim.join(".git").join("config")).unwrap(), before, "init 이 사용자 저장소의 config 를 바꿨다");
    // 설정은 캐시 저장소 자신에게 들어갔다
    let cfg = exec.run(dir.as_str(), &["config", "--local", "remote.origin.url"], &RunOpts::default()).await.unwrap();
    assert_eq!(cfg.code, 0, "{}", cfg.stderr);
    // 대조: 같은 환경을 거르지 않고 쓰면(날 git + GIT_DIR) 피해 저장소의 config 가 바뀐다 — 이 시험이 공허하지 않다는 증거
    let out = raw("git").env("GIT_DIR", dir.git_dir()).env("GIT_COMMON_DIR", victim.join(".git")).args(["config", "--local", "antler.probe", "1"]).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_ne!(fs::read(victim.join(".git").join("config")).unwrap(), before, "대조군이 피해 저장소를 바꾸지 못했다 — 시험이 공허하다");
    let _ = fs::remove_dir_all(&victim);
}
