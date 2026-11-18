//! GitHub 캐시 실행기의 git 이 **캐시 로컬 config 로 TLS 검증을 끄거나 프록시를 바꾸지 못하는지** 보는 시험(TS `githubEnvPin.test.ts` 의 짝).
//! 로컬 쓰기 권한을 가진 쪽이 캐시 bare 저장소의 config 에 `http.sslVerify=false`·`http.proxy=…` 를 심으면, 조회 경로의 lazy fetch 는
//! (정확 일치 검사가 fetch 앞에서만 돈다) Authorization 을 실은 채 그 프록시를 지나고 TLS 검증도 꺼진다 — command scope 의 고정값이 이겨야 한다.

mod ghcache;

use std::sync::Arc;

use antler_lib::git::exec::{GitExecutor, GithubExecutor, RunOpts};
use antler_lib::git::github_cache::env_https_proxy;
use antler_lib::net::redact::SecretString;
use ghcache::{raw_git, text, Cache};

const URL: &str = "https://github.com/octocat/hello.git";

struct Poisoned {
    dir: antler_lib::git::github_cache::GithubCachePath,
}

/// 공격자가 심은 로컬 설정(일반 키와 github.com URL 범위 키 둘 다)
fn poisoned(cache: &Cache) -> Poisoned {
    let dir = cache.repo("octocat", "hello");
    raw_git(std::path::Path::new(dir.as_str()), &["init", "-q", "--bare", "."]);
    for (k, v) in [
        ("http.sslVerify", "false"),
        ("http.proxy", "http://attacker:8080"),
        ("http.https://github.com/.sslVerify", "false"),
        ("http.https://github.com/.proxy", "http://attacker:8080"),
    ] {
        raw_git(std::path::Path::new(dir.as_str()), &["config", "--local", k, v]);
    }
    Poisoned { dir }
}

async fn effective(exec: &GitExecutor, p: &Poisoned, args: &[&str]) -> String {
    let mut a = vec!["config"];
    a.extend_from_slice(args);
    let r = exec.run(p.dir.as_str(), &a, &RunOpts::default()).await.unwrap();
    assert_eq!(r.code, 0, "{}", r.stderr);
    text(&r.stdout)
}

fn github_exec(cache: &Cache, token: Option<&'static str>, net: Vec<(String, String)>) -> GitExecutor {
    GitExecutor::Github(
        GithubExecutor::new("git", cache.cache_dir.clone(), cache.hooks.clone(), Arc::new(move || token.map(SecretString::new)), Arc::new(move || net.clone()))
            .with_protocol_for_test("https"),
    )
}

#[tokio::test]
async fn 음성_대조_고정하지_않은_git_에서는_로컬_값이_이긴다() {
    let cache = Cache::new();
    let p = poisoned(&cache);
    let dir = std::path::Path::new(p.dir.as_str());
    assert_eq!(raw_git(dir, &["config", "--get", "http.sslVerify"]), "false");
    assert_eq!(raw_git(dir, &["config", "--get", "http.proxy"]), "http://attacker:8080");
    assert_eq!(raw_git(dir, &["config", "--get-urlmatch", "http.sslVerify", URL]), "false");
    assert_eq!(raw_git(dir, &["config", "--get-urlmatch", "http.proxy", URL]), "http://attacker:8080");
}

#[tokio::test]
async fn 로컬_config_가_sslverify_를_끄거나_프록시를_바꿔도_git_이_쓰는_값은_고정값이다() {
    let cache = Cache::new();
    let p = poisoned(&cache);
    for token in [Some("ghp_golden_fake_token"), None] {
        let exec = github_exec(&cache, token, Vec::new());
        assert_eq!(effective(&exec, &p, &["--get", "http.sslVerify"]).await, "true");
        assert_eq!(effective(&exec, &p, &["--get", "http.proxy"]).await, "");
        assert_eq!(effective(&exec, &p, &["--get-urlmatch", "http.sslVerify", URL]).await, "true");
        assert_eq!(effective(&exec, &p, &["--get-urlmatch", "http.proxy", URL]).await, "");
    }
}

#[tokio::test]
async fn 사용자_프록시는_그대로_싣고_로컬_값이_이기지_못한다() {
    let cache = Cache::new();
    let p = poisoned(&cache);
    let exec = github_exec(&cache, Some("ghp_golden_fake_token"), vec![("http.proxy".to_string(), "http://proxy.corp:8080".to_string())]);
    assert_eq!(effective(&exec, &p, &["--get", "http.proxy"]).await, "http://proxy.corp:8080");
    assert_eq!(effective(&exec, &p, &["--get-urlmatch", "http.proxy", URL]).await, "http://proxy.corp:8080");
}

#[test]
fn 환경_변수_프록시는_https_proxy_우선_all_proxy_차선이고_빈_값_너무_긴_값은_버린다() {
    let from = |pairs: &'static [(&'static str, &'static str)]| env_https_proxy(move |n| pairs.iter().find(|(k, _)| *k == n).map(|(_, v)| (*v).to_string()));
    assert_eq!(from(&[]), None);
    assert_eq!(from(&[("ALL_PROXY", "http://a"), ("all_proxy", "http://b")]).as_deref(), Some("http://b"));
    assert_eq!(from(&[("HTTPS_PROXY", "http://c"), ("all_proxy", "http://b")]).as_deref(), Some("http://c"));
    assert_eq!(from(&[("https_proxy", ""), ("HTTPS_PROXY", "http://c")]).as_deref(), Some("http://c"));
    assert_eq!(from(&[("http_proxy", "http://only-http")]), None);
    let long = "x".repeat(5000);
    assert_eq!(env_https_proxy(|n| (n == "HTTPS_PROXY").then(|| long.clone())), None);
}
