//! GitHub 원격 전용 저장소(github kind)의 git 조회 채널을 Rust 가 처리하는지(Phase 6b PR 6) 통합 테스트.
//! 실제 git 을 띄우되 네트워크는 쓰지 않는다 — `file:///` 소스 저장소와 127.0.0.1 의 응답 없는 서버뿐이다(실제 GitHub 에는 닿지 않는다).
//!
//! 보는 것:
//! - 소유: GitHub 서비스·캐시 실행기가 모두 있을 때만 조회(PR 6)와 fetch·감시(PR 7, `tests/github_fetch.rs`)를 Rust 가 맡는다 — 아니면 풀 수 없다(`Unserved`).
//! - 패리티: 같은 저장소의 local 결과와 같은 채널 결과(TS 는 같은 `RepoService` 코드를 실행기만 바꿔 탄다), 그리고 github 만의 갈래(refs 의 worktrees·refsDetails 의 서브모듈·onRemote·wip·gh:repo·lenient).
//! - `repos:remove`: 진행 중인 조회·열린 로그 커서를 정리해 설정 소유자가 지우기 전에 캐시 폴더를 쥔 핸들이 없다.
//! - 비밀: 토큰은 어떤 응답·오류에도 없다.
//!
//! 격리: 이 바이너리가 시작될 때(`isolate`) 깨끗한 전역 설정과 `GIT_CONFIG_NOSYSTEM` 을 넣고 프록시·askpass 환경을 비운다.

mod ghcache;

use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use antler_lib::api_host::GithubHost;
use antler_lib::git::exec::GithubExecutor;
use antler_lib::git::fetch::run::{run_github_fetch, GithubFetchOpts};
use antler_lib::git::github_cache::{ensure_github_head, init_github_repo_for_test, remove_github_cache, GithubCachePath};
use antler_lib::github::ports::{GhRepoRef, RepoQuery};
use antler_lib::github::service::{GitHubService, ServiceOptions};
use antler_lib::mirror::{GhLink, MirrorRepo, RepoMirror};
use antler_lib::net::redact::{git_basic_token, SecretString};
use antler_lib::router::{Routed, Router, RUST_CHANNELS};
use antler_lib::secret::memory::MemoryStore;
use antler_lib::secret::Vault;
use futures_util::future::{BoxFuture, FutureExt};
use ghcache::{link, make_source, raw_git, Cache, Source};
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

// unsafe: main 이전 코드는 Rust 런타임 초기화 전에 돈다 — 여기서는 환경 변수만 넣고 다른 것에 기대지 않는다
#[ctor::ctor(unsafe)]
fn isolate() {
    std::env::set_var("GIT_CONFIG_GLOBAL", Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("gitconfig"));
    std::env::set_var("GIT_CONFIG_NOSYSTEM", "1");
    std::env::set_var("GCM_INTERACTIVE", "never");
    for k in ["HTTP_PROXY", "http_proxy", "HTTPS_PROXY", "https_proxy", "ALL_PROXY", "all_proxy", "GIT_ASKPASS", "SSH_ASKPASS", "GIT_SSH", "GIT_SSH_COMMAND", "GIT_ALLOW_PROTOCOL", "GIT_TERMINAL_PROMPT"] {
        std::env::remove_var(k);
    }
    std::env::set_var("NO_PROXY", "127.0.0.1,localhost");
}

/// 토큰 비노출 시험이 찾는 가짜 토큰
const TOKEN: &str = "ghp_route_canary_token_0123456789";

/// 저장소도 클라이언트 ID 도 없는 질의(오프라인)
struct NoRepos;

impl RepoQuery for NoRepos {
    fn gh_repo<'a>(&'a self, _: &'a str) -> BoxFuture<'a, Result<Option<GhRepoRef>, String>> {
        async { Ok(None) }.boxed()
    }
    fn gh_remotes<'a>(&'a self, _: &'a str, _: CancellationToken) -> BoxFuture<'a, Result<Vec<GhRepoRef>, String>> {
        async { Ok(Vec::new()) }.boxed()
    }
    fn pushed_shas_by_author<'a>(&'a self, _: &'a str, _: &'a str, _: usize, _: CancellationToken) -> BoxFuture<'a, Result<Vec<String>, String>> {
        async { Ok(Vec::new()) }.boxed()
    }
    fn invalidate_gh_remotes(&self) {}
    fn github_client_id(&self) -> BoxFuture<'_, Option<String>> {
        async { None }.boxed()
    }
    fn repo_ids(&self) -> BoxFuture<'_, Vec<String>> {
        async { Vec::new() }.boxed()
    }
}

fn gh_host() -> GithubHost {
    let vault = Arc::new(Vault::new(Arc::new(MemoryStore::new())));
    let service = GitHubService::new(vault, Arc::new(NoRepos), None, ServiceOptions::default()).expect("운영 클라이언트");
    GithubHost { service, opener: Arc::new(|_| Ok(())) }
}

fn token_exec(cache: &Cache, protocol: &str) -> GithubExecutor {
    GithubExecutor::new("git", cache.cache_dir.clone(), cache.hooks.clone(), Arc::new(|| Some(SecretString::new(TOKEN))), Arc::new(Vec::new)).with_protocol_for_test(protocol)
}

async fn call(r: &Router, ch: &str, args: Value) -> Value {
    match r.route(ch, &args).await {
        Routed::Done(v) => v,
        Routed::Unserved => panic!("{ch} {args} 를 풀지 못했다"),
    }
}

fn is_unserved(r: Routed) -> bool {
    matches!(r, Routed::Unserved)
}

/// `repos:watch`·`req:cancel` 의 응답(라우터가 `{ok:true}` 로 답한다)
fn is_acked(r: Routed) -> bool {
    matches!(r, Routed::Done(v) if v == json!({ "ok": true }))
}

fn data(env: &Value) -> &Value {
    assert_eq!(env["ok"], true, "{env}");
    &env["data"]
}

/// 소스에 커밋 둘을 더한다: 이름을 바꾸며 고친 커밋(오프라인에서는 이름 변경 감지에 blob 이 필요하다), 서브모듈(.gitmodules + gitlink)을 더한 커밋
fn extend_source(src: &Source) -> (String, String) {
    let dir = src.dir("octocat", "hello");
    raw_git(&dir, &["mv", "f.txt", "g.txt"]);
    fs::write(dir.join("g.txt"), "one\ntwo\nthree\nfour\nfive\n").unwrap();
    raw_git(&dir, &["add", "g.txt"]);
    raw_git(&dir, &["commit", "-q", "-m", "third"]);
    let third = raw_git(&dir, &["rev-parse", "HEAD"]);
    fs::write(dir.join(".gitmodules"), "[submodule \"lib\"]\n\tpath = lib\n\turl = https://example.com/lib.git\n").unwrap();
    raw_git(&dir, &["add", ".gitmodules"]);
    raw_git(&dir, &["update-index", "--add", "--cacheinfo", &format!("160000,{},lib", src.first)]);
    raw_git(&dir, &["commit", "-q", "-m", "fourth"]);
    let fourth = raw_git(&dir, &["rev-parse", "HEAD"]);
    (third, fourth)
}

/// github 캐시(소스에서 `blob:none` 으로 받았다) + 같은 소스의 local 저장소 + 둘을 푸는 라우터
struct Rig {
    src: Source,
    cache: Cache,
    dir: GithubCachePath,
    router: Arc<Router>,
    /// 라우터와 같은 미러·실행기(복제하면 같은 표를 본다) — 미러를 바꾸고 진행 중인 프로세스 수를 보는 시험용
    mirror: RepoMirror,
    exec: GithubExecutor,
    third: String,
    fourth: String,
}

const G: &str = "g";
const L: &str = "l";

impl Rig {
    async fn new(protocol: &str) -> Self {
        Self::with(protocol, true, None).await
    }

    /// `fetched` 가 거짓이면 init 만 하고 받지 않은 캐시다(`branch` 는 init 이 HEAD 를 가리킬 기본 브랜치)
    async fn with(protocol: &str, fetched: bool, branch: Option<&str>) -> Self {
        let src = make_source("octocat", "hello");
        let (third, fourth) = extend_source(&src);
        let cache = Cache::new();
        let dir = cache.repo("octocat", "hello");
        // 준비(init·fetch·HEAD)는 file 전송을 허용한 별도 실행기로 한다
        let prep = cache.executor("file", None);
        init_github_repo_for_test(&prep, &dir, &link("octocat", "hello", branch), &src.origin(), None).await.expect("init");
        if fetched {
            run_github_fetch(&prep, &dir, &GithubFetchOpts { cancel: None, no_hooks_dir: &cache.hooks }).await.expect("fetch");
            ensure_github_head(&prep, &dir, &link("octocat", "hello", None), None, &[]).await.expect("head");
        }
        let mirror = RepoMirror::default();
        let local_path = src.dir("octocat", "hello").to_string_lossy().into_owned();
        let repos = vec![
            MirrorRepo { id: G.into(), kind: "github".into(), path: dir.as_str().to_string(), distro: None, ssh_host: None },
            MirrorRepo { id: L.into(), kind: "local".into(), path: local_path, distro: None, ssh_host: None },
        ];
        let links = std::collections::HashMap::from([(G.to_string(), GhLink { owner: "octocat".into(), repo: "hello".into() })]);
        assert!(mirror.apply_with_links(1, repos, links));
        let router = Router::new(false, mirror.clone());
        router.set_github(gh_host());
        let exec = token_exec(&cache, protocol);
        router.set_github_cache(exec.clone());
        Rig { src, cache, dir, router: Arc::new(router), mirror, exec, third, fourth }
    }

    fn git(&self, args: &[&str]) -> String {
        raw_git(Path::new(self.dir.as_str()), args)
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        self.router.shutdown();
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// 소유
// ---------------------------------------------------------------------------------------------------------------------

#[tokio::test]
async fn github_저장소의_조회_fetch_감시는_같은_조건으로_rust_가_처리한다() {
    let rig = Rig::new("file").await;
    let r = &rig.router;
    assert!(r.github_cache_active());
    let env = call(r, "git:commit", json!([G, rig.src.second])).await;
    assert_eq!(data(&env)["hash"], rig.src.second);
    assert_eq!(r.rust_handled(), 1);
    // fetch·감시의 소유도 같은 조건으로 Rust 다(Phase 6b PR 7) — `rust_fetch_kinds` 의 github 와 미러의 조회 해석이 같은 플래그 하나다.
    // 이 시험의 라우터에는 fetch 서비스가 없어 `git:fetch` 는 "준비되지 않았다" 오류다(어디로도 넘기지 않는다). 서비스가 붙은 흐름은 `tests/github_fetch.rs`
    let fetch = call(r, "git:fetch", json!([G, true])).await;
    assert_eq!(fetch["ok"], false, "{fetch}");
    assert!(is_acked(r.route("repos:watch", &json!([G])).await));
    assert_eq!(r.watching().as_deref(), Some(G), "github 저장소도 Rust 가 감시한다");
    // local 저장소로 옮기면 감시가 바뀐다
    assert!(is_acked(r.route("repos:watch", &json!([L])).await));
    assert_eq!(r.watching().as_deref(), Some(L));
    assert!(is_acked(r.route("repos:watch", &json!([G])).await));
    assert_eq!(r.watching().as_deref(), Some(G));
    // 소유 술어: 종류 목록의 github 는 같은 플래그(github_cache_active)를 따른다
    assert!(antler_lib::router::rust_fetch_kinds(false, true).contains("github") && !antler_lib::router::rust_fetch_kinds(true, false).contains("github"));
    assert_eq!(antler_lib::router::rust_fetch_kinds(r.ssh_active(), r.github_cache_active()), "local,wsl,github");
}

#[tokio::test]
async fn 서비스_실행기_중_하나라도_없으면_github_는_풀_수_없다() {
    let src_dir = TempDirPath::new("none");
    let repos = || {
        let m = RepoMirror::default();
        assert!(m.apply(1, vec![MirrorRepo { id: G.into(), kind: "github".into(), path: src_dir.0.join("o").join("r.git").to_string_lossy().into_owned(), distro: None, ssh_host: None }]));
        m
    };
    let cache = Cache::new();
    let args = json!([G, "abcdef12"]);
    // 실행기를 주입하지 않았다
    let r = Router::new(false, repos());
    r.set_github(gh_host());
    assert!(is_unserved(r.route("git:commit", &args).await));
    assert!(!r.github_cache_active());
    // GitHub 서비스보다 먼저 주입하면 무시된다(토큰의 출처가 그 서비스 하나다)
    let r = Router::new(false, repos());
    r.set_github_cache(token_exec(&cache, "file"));
    r.set_github(gh_host());
    assert!(!r.github_cache_active());
    assert!(is_unserved(r.route("git:commit", &args).await));
    // 서비스 없이 실행기만
    let r = Router::new(false, repos());
    r.set_github_cache(token_exec(&cache, "file"));
    assert!(!r.github_cache_active());
    for ch in RUST_CHANNELS {
        assert!(is_unserved(r.route(ch, &json!([G, "abcdef12"])).await), "{ch}");
    }
    // 앱 경계는 명시적 오류로 답한다
    assert_eq!(r.invoke("git:commit", &args).await["error"], "GitHub 원격 저장소를 쓸 수 없는 환경입니다");
}

// ---------------------------------------------------------------------------------------------------------------------
// 패리티: 같은 저장소의 local 결과와 같다
// ---------------------------------------------------------------------------------------------------------------------

#[tokio::test]
async fn 같은_저장소의_조회_결과가_local_과_같다() {
    let rig = Rig::new("file").await;
    let r = &rig.router;
    let (first, second, third, fourth) = (rig.src.first.clone(), rig.src.second.clone(), rig.third.clone(), rig.fourth.clone());
    let same = |ch: &'static str, args: Box<dyn Fn(&str) -> Value>| {
        let r = r.clone();
        async move {
            let (g, l) = (call(&r, ch, args(G)).await, call(&r, ch, args(L)).await);
            assert_eq!(g, l, "{ch}");
            g
        }
    };
    // 커밋 상세: github 는 blob 이 없어 통계(numstat)를 lazy fetch 로 받는다
    let c = same("git:commit", Box::new({
        let h = second.clone();
        move |id| json!([id, h, "req-commit"])
    }))
    .await;
    assert_eq!(data(&c)["additions"], 1);
    same("git:commit", Box::new({
        let h = first.clone();
        move |id| json!([id, h])
    }))
    .await;
    same("git:compare", Box::new({
        let (a, b) = (first.clone(), third.clone());
        move |id| json!([id, a, b])
    }))
    .await;
    same("git:fileDiff", Box::new({
        let (h, p) = (second.clone(), first.clone());
        move |id| json!([id, { "hash": h, "parent": p, "path": "f.txt" }])
    }))
    .await;
    same("git:fileHistory", Box::new({
        let h = second.clone();
        move |id| json!([id, { "path": "f.txt", "rev": h }])
    }))
    .await;
    same("git:blame", Box::new({
        let h = third.clone();
        move |id| json!([id, { "path": "g.txt", "rev": h }])
    }))
    .await;
    same("git:tree", Box::new({
        let h = fourth.clone();
        move |id| json!([id, { "rev": h, "dir": "" }])
    }))
    .await;
    same("git:blob", Box::new({
        let h = fourth.clone();
        move |id| json!([id, { "rev": h, "path": "g.txt" }])
    }))
    .await;
    same("git:resolveRev", Box::new({
        let h = fourth.clone();
        move |id| json!([id, h])
    }))
    .await;
    same("git:search", Box::new(|id| json!([id, "second"]))).await;
    same("git:repoExtras", Box::new(|id| json!([id]))).await;
    same("git:lfsPaths", Box::new({
        let h = second.clone();
        move |id| json!([id, { "rev": h, "paths": ["f.txt"] }])
    }))
    .await;
    // 로그: 장식(refs)은 저장소마다 다르므로 해시 목록을 비교한다
    let log = |id: &'static str| {
        let r = r.clone();
        async move {
            let env = call(&r, "git:log", json!([id, { "count": 10, "skip": 0 }, "req-log"])).await;
            data(&env)["commits"].as_array().unwrap().iter().map(|c| c["hash"].as_str().unwrap().to_string()).collect::<Vec<_>>()
        }
    };
    assert_eq!(log(G).await, log(L).await);
    assert_eq!(log(G).await[0], fourth);
    // 활동: 같은 저장소의 같은 범위
    let act = |id: &'static str| {
        let r = r.clone();
        async move { call(&r, "git:activity", json!([id])).await }
    };
    assert_eq!(act(G).await, act(L).await);
}

#[tokio::test]
async fn github_만의_갈래는_ts_repo_service_와_같다() {
    let rig = Rig::new("file").await;
    let r = &rig.router;
    // refs: bare 캐시 자신이 'bare' 작업 트리로 나오므로 비운다(local 은 작업 트리가 있다)
    let g = call(r, "git:refs", json!([G])).await;
    let l = call(r, "git:refs", json!([L])).await;
    assert_eq!(data(&g)["worktrees"], json!([]));
    assert!(!data(&l)["worktrees"].as_array().unwrap().is_empty());
    // refsDetails: 서브모듈을 읽지 않는다(.gitmodules blob 을 받으러 가지 않는다) — local 은 같은 저장소에서 하나를 본다
    let g = call(r, "git:refsDetails", json!([G])).await;
    let l = call(r, "git:refsDetails", json!([L])).await;
    assert_eq!(data(&g)["submodules"], json!([]));
    assert_eq!(data(&l)["submodules"].as_array().unwrap().len(), 1, "대조군: 같은 소스의 local 은 서브모듈을 본다 — {l}");
    // onRemote: 항상 true — 인자 검증도 git 도 없다(local 은 잘못된 rev 를 거부한다)
    for rev in [json!("deadbeef"), json!("--output=x"), json!(null), json!(5)] {
        let env = call(r, "git:onRemote", json!([G, rev])).await;
        assert_eq!(env, json!({ "ok": true, "data": true }));
    }
    assert_eq!(call(r, "git:onRemote", json!([L, "--output=x"])).await["ok"], false);
    // wip: 작업 트리가 없다 — 빈 결과(첫 페이지 문·큐·허용 목록이 없다)
    let s = call(r, "git:wipStatus", json!([G, "req-wip"])).await;
    assert_eq!(data(&s)["staged"], json!([]));
    assert_eq!(data(&s)["unstaged"], json!([]));
    assert_eq!(data(&s)["untracked"], json!([]));
    assert_eq!(data(&s)["conflicted"], 0);
    // wipDiff: 요청 검증 없이 경로만 되돌려 주는 빈 diff(`typeof req?.path === 'string' ? req.path : ''`)
    let d = call(r, "git:wipDiff", json!([G, { "path": "a/b.txt", "area": "bogus" }])).await;
    assert_eq!(data(&d)["path"], "a/b.txt");
    assert_eq!(data(&d)["hunks"], json!([]));
    assert_eq!(data(&call(r, "git:wipDiff", json!([G, "x"])).await)["path"], "");
    assert_eq!(call(r, "git:wipDiff", json!([L, { "path": "a/b.txt", "area": "bogus" }])).await["ok"], false);
    // gh:repo: 레코드의 링크로 git 없이 답한다 — remote origin 의 URL(file://)에서 추론하면 null 이다
    let gh = call(r, "git:wipStatus", json!([G])).await;
    assert_eq!(gh["ok"], true);
    let env = call(r, "gh:repo", json!([G])).await;
    assert_eq!(env, json!({ "ok": true, "data": { "owner": "octocat", "repo": "hello", "remote": "origin" } }));
    assert_eq!(call(r, "gh:repo", json!([L])).await, json!({ "ok": true, "data": null }));
    // GitHub 서비스가 채널을 거치지 않고 묻는 길(`RoutedRepos` → `Router::gh_repo`)도 같은 답이다
    let by_service = r.gh_repo(G).await.expect("Rust 가 맡는다").expect("링크로 답한다");
    assert_eq!(by_service, Some(GhRepoRef { owner: "octocat".into(), repo: "hello".into(), remote: "origin".into() }));
    // origin 이 GitHub 주소가 아니라 remote 추론으로는 아무것도 없다(file://) — 그래도 Rust 가 git 으로 답한다(대체 질의로 넘기지 않는다)
    assert_eq!(r.gh_remotes(G, &CancellationToken::new()).await.expect("Rust 가 맡는다").expect("git 으로 읽는다"), Vec::<GhRepoRef>::new());
    assert!(r.pushed_shas_by_author(G, "a@b.c", 5, &CancellationToken::new()).await.expect("Rust 가 맡는다").is_ok());
}

#[tokio::test]
async fn 아직_받지_않은_저장소는_조회가_에러_대신_빈_결과다() {
    for branch in [Some("main"), None] {
        let rig = Rig::with("file", false, branch).await;
        let r = &rig.router;
        let log = call(r, "git:log", json!([G, { "skip": 0, "count": 50 }])).await;
        assert_eq!(data(&log)["commits"], json!([]), "{branch:?}: {log}");
        let refs = call(r, "git:refs", json!([G])).await;
        for key in ["local", "remotes", "tags", "stashes", "submodules", "worktrees"] {
            assert_eq!(data(&refs)[key], json!([]), "{branch:?} {key}: {refs}");
        }
        let details = call(r, "git:refsDetails", json!([G])).await;
        assert_eq!(data(&details), &json!({ "tracking": {}, "submodules": [] }), "{branch:?}");
        let activity = call(r, "git:activity", json!([G])).await;
        assert_eq!(data(&activity)["total"], 0, "{branch:?}: {activity}");
        let search = call(r, "git:search", json!([G, "abc"])).await;
        assert_eq!(data(&search)["hashes"], json!([]), "{branch:?}: {search}");
    }
}

#[tokio::test]
async fn 링크가_없는_github_레코드의_gh_repo_는_remote_에서_추론한다() {
    let rig = Rig::new("file").await;
    // 같은 캐시를 링크 없이 미러에 올린다
    let mirror = RepoMirror::default();
    assert!(mirror.apply(1, vec![MirrorRepo { id: "nolink".into(), kind: "github".into(), path: rig.dir.as_str().to_string(), distro: None, ssh_host: None }]));
    let r = Router::new(false, mirror);
    r.set_github(gh_host());
    r.set_github_cache(token_exec(&rig.cache, "file"));
    // origin 은 file:// 이라 추론할 GitHub 저장소가 없다
    assert_eq!(call(&r, "gh:repo", json!(["nolink"])).await, json!({ "ok": true, "data": null }));
    // origin 을 GitHub 주소로 바꾸면 git remote 에서 추론한다(링크가 없을 때만 git 을 부른다)
    rig.git(&["remote", "set-url", "origin", "https://github.com/Octo/Repo.git"]);
    let r2 = Router::new(false, {
        let m = RepoMirror::default();
        assert!(m.apply(1, vec![MirrorRepo { id: "nolink2".into(), kind: "github".into(), path: rig.dir.as_str().to_string(), distro: None, ssh_host: None }]));
        m
    });
    r2.set_github(gh_host());
    r2.set_github_cache(token_exec(&rig.cache, "file"));
    let env = call(&r2, "gh:repo", json!(["nolink2"])).await;
    assert_eq!(data(&env)["owner"], "Octo", "{env}");
    r.shutdown();
    r2.shutdown();
}

#[tokio::test]
async fn 활동은_github_저장소도_기억한_시그니처를_쓰고_fetch_알림이_버린다() {
    // Phase 6b PR 7: fetch·감시가 Rust 로 왔으므로 local 과 같다 — 기억한 시그니처를 재사용하고, fetch 가 ref 를 바꾸면 알림(`fetchChanged`)이 기억을 버린다
    use antler_lib::git::log_service::NoteOp;
    let rig = Rig::new("file").await;
    let r = &rig.router;
    let before = call(r, "git:activity", json!([G])).await;
    let n_before = data(&before).to_string();
    rig.git(&["update-ref", "refs/remotes/origin/extra", &rig.src.first]);
    let new_commit = rig.git(&["commit-tree", "-p", &rig.fourth, "-m", "late", &format!("{}^{{tree}}", rig.fourth)]);
    rig.git(&["update-ref", "refs/remotes/origin/main", &new_commit]);
    // 알림이 없으면 기억한 값이 쓰인다(감시하지 않는 저장소의 기억은 알림 없이 낡을 수 있다 — local 과 같다)
    let stale = call(r, "git:activity", json!([G])).await;
    assert_eq!(data(&stale).to_string(), n_before, "기억한 시그니처를 재사용해야 한다");
    // Rust fetch 서비스가 ref 변경 뒤 보내는 알림(`HostEvent::RepoChanged` 앞의 note)
    (r.note_sink())(NoteOp::FetchChanged, G);
    let after = call(r, "git:activity", json!([G])).await;
    assert_ne!(data(&after).to_string(), n_before, "fetch 알림 뒤에도 낡은 활동을 돌려줬다");
}

// ---------------------------------------------------------------------------------------------------------------------
// lenient: 오프라인에서도 목록은 돌려주고 통계 없는 결과는 캐시하지 않는다
// ---------------------------------------------------------------------------------------------------------------------

fn offline(rig: &Rig) -> impl Drop {
    struct Back(PathBuf, PathBuf);
    impl Drop for Back {
        fn drop(&mut self) {
            let _ = fs::rename(&self.1, &self.0);
        }
    }
    let from = rig.src.dir("octocat", "hello");
    let to = rig.src.srv.path().join("moved-away");
    fs::rename(&from, &to).expect("소스를 치운다");
    Back(from, to)
}

#[tokio::test]
async fn 오프라인이면_커밋_상세와_비교가_통계_없이_목록만_돌려주고_캐시하지_않는다() {
    let rig = Rig::new("file").await;
    let r = &rig.router;
    let gone = offline(&rig);
    // 커밋 상세: name-status 는 되고 numstat 이 blob 을 받지 못해 실패한다 → 통계 없는 목록
    let env = call(r, "git:commit", json!([G, rig.src.second])).await;
    let files = data(&env)["files"].as_array().unwrap().clone();
    assert_eq!(files.len(), 1, "{env}");
    assert!(files[0]["additions"].is_null() && files[0]["binary"] == false, "{env}");
    // 이름을 바꾸며 고친 커밋: -M 이 blob 을 받지 못해 실패하면 이름 변경 감지 없이 읽는다(삭제 + 추가)
    let env = call(r, "git:commit", json!([G, rig.third])).await;
    let statuses: Vec<&str> = data(&env)["files"].as_array().unwrap().iter().map(|f| f["status"].as_str().unwrap()).collect();
    assert!(statuses.contains(&"D") || statuses.contains(&"R"), "{env}");
    // 비교도 같다
    let env = call(r, "git:compare", json!([G, rig.src.first, rig.src.second])).await;
    assert!(data(&env)["files"][0]["additions"].is_null(), "{env}");
    // local 대조군(lenient 가 아니다)은 같은 처지에서 던진다 — 소스 저장소가 사라졌으니 local 은 아예 없다: 실행기 종류가 lenient 를 정한다
    drop(gone);
    // 돌아오면 캐시되지 않았으므로 완전한 결과가 나온다
    let env = call(r, "git:commit", json!([G, rig.src.second])).await;
    assert_eq!(data(&env)["files"][0]["additions"], 1, "통계 없는 결과가 캐시에 굳었다 — {env}");
    let env = call(r, "git:compare", json!([G, rig.src.first, rig.src.second])).await;
    assert_eq!(data(&env)["files"][0]["additions"], 1, "{env}");
    // 이제는 완전한 결과가 캐시에 있다 — 다시 오프라인이어도 같은 결과
    let _gone = offline(&rig);
    let env = call(r, "git:commit", json!([G, rig.src.second])).await;
    assert_eq!(data(&env)["files"][0]["additions"], 1, "{env}");
}

// ---------------------------------------------------------------------------------------------------------------------
// 토큰 비노출
// ---------------------------------------------------------------------------------------------------------------------

#[tokio::test]
async fn 어떤_채널의_응답과_오류에도_토큰이_없다() {
    let rig = Rig::new("file").await;
    let r = &rig.router;
    let (second, fourth) = (rig.src.second.clone(), rig.fourth.clone());
    let b64 = git_basic_token(TOKEN);
    let mut seen = 0;
    let calls: Vec<(&str, Value)> = vec![
        ("git:commit", json!([G, second])),
        ("git:commit", json!([G, "ffffffffffffffffffffffffffffffffffffffff"])),
        ("git:log", json!([G, { "count": 5, "skip": 0 }, "r1"])),
        ("git:compare", json!([G, rig.src.first, second])),
        ("git:compare", json!([G, "ffffffff", second])),
        ("git:fileDiff", json!([G, { "hash": second, "parent": rig.src.first, "path": "f.txt" }])),
        ("git:fileDiff", json!([G, { "hash": second, "path": "does-not-exist" }])),
        ("git:fileHistory", json!([G, { "path": "f.txt", "rev": second }])),
        ("git:blame", json!([G, { "path": "g.txt", "rev": fourth }])),
        ("git:blame", json!([G, { "path": "nope.txt", "rev": fourth }])),
        ("git:resolveRev", json!([G, "no-such-ref"])),
        ("git:tree", json!([G, { "rev": fourth, "dir": "no/such/dir" }])),
        ("git:blob", json!([G, { "rev": fourth, "path": "g.txt" }])),
        ("git:onRemote", json!([G, second])),
        ("git:refs", json!([G])),
        ("git:refsDetails", json!([G])),
        ("git:search", json!([G, "third"])),
        ("git:activity", json!([G])),
        ("git:repoExtras", json!([G])),
        ("git:lfsPaths", json!([G, { "rev": second, "paths": ["f.txt"] }])),
        ("git:wipStatus", json!([G])),
        ("git:wipDiff", json!([G, { "path": "f.txt", "area": "unstaged" }])),
        ("gh:repo", json!([G])),
    ];
    for (ch, args) in calls {
        let env = call(r, ch, args.clone()).await.to_string();
        assert!(!env.contains(TOKEN) && !env.contains(&b64), "{ch} {args}: {env}");
        seen += 1;
    }
    assert_eq!(seen, 23);
    // 설정을 드러내는 명령(`config --list`)이 환경의 헤더를 보여 줘도 run 의 출력에서 가려진다 — 실행기 시험(github_cache_exec.rs)이 본다.
    // 여기서는 라우터가 쓰는 실행기가 토큰을 쥐고 있어도 환경이 캐시 모양 경로에만 열린다는 것까지 본다: 캐시 밖 local 저장소의 응답에도 토큰이 없다
    let env = call(r, "git:repoExtras", json!([L])).await.to_string();
    assert!(!env.contains(TOKEN) && !env.contains(&b64));

    // lazy fetch 가 실패하는 경로: git 이 토큰 헤더를 쥔 채 원격에 말을 걸었다가 실패한 오류 문구(받지 않은 blob 을 소스가 없는 상태에서 읽는다)
    let fresh = Rig::new("file").await;
    let _gone = offline(&fresh);
    let (second, first, fourth) = (fresh.src.second.clone(), fresh.src.first.clone(), fresh.fourth.clone());
    let mut failed = 0;
    for (ch, args) in [
        ("git:fileDiff", json!([G, { "hash": second, "parent": first, "path": "f.txt" }])),
        ("git:blob", json!([G, { "rev": fourth, "path": "g.txt" }])),
        ("git:blame", json!([G, { "path": "g.txt", "rev": fourth }])),
    ] {
        let env = call(&fresh.router, ch, args.clone()).await;
        assert!(!env.to_string().contains(TOKEN) && !env.to_string().contains(&b64), "{ch}: {env}");
        failed += usize::from(env["ok"] == false);
    }
    assert!(failed >= 1, "오프라인 lazy fetch 가 한 번도 실패하지 않아 이 검사가 공허하다");
}

// ---------------------------------------------------------------------------------------------------------------------
// repos:remove
// ---------------------------------------------------------------------------------------------------------------------

/// 요청을 받고도 응답하지 않는 서버(lazy fetch 가 멈춘 원격)
struct Silent {
    port: u16,
    accepted: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Silent {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let (accepted, stop) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicBool::new(false)));
        let (a2, s2) = (accepted.clone(), stop.clone());
        let thread = std::thread::spawn(move || {
            let mut held = Vec::new();
            while !s2.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((mut conn, _)) => {
                        conn.set_nonblocking(false).unwrap();
                        conn.set_read_timeout(Some(Duration::from_millis(500))).unwrap();
                        // 요청 머리를 읽고도 답하지 않고 연결을 쥔다
                        let mut b = [0u8; 1024];
                        let _ = conn.read(&mut b);
                        a2.fetch_add(1, Ordering::SeqCst);
                        held.push(conn);
                    }
                    Err(_) => std::thread::sleep(Duration::from_millis(10)),
                }
            }
            for mut c in held {
                let _ = c.write_all(b"");
            }
        });
        Silent { port, accepted, stop, thread: Some(thread) }
    }
}

impl Drop for Silent {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// 캐시를 file 소스로 받은 뒤 origin 을 응답 없는 서버로 돌려 둔 판 — 받지 않은 blob 이 필요한 명령은 lazy fetch 에서 멈춘다
async fn stuck_rig(server: &Silent) -> Rig {
    let rig = Rig::new("http").await;
    rig.git(&["remote", "set-url", "origin", &format!("http://127.0.0.1:{}/octocat/hello.git", server.port)]);
    rig
}

#[tokio::test]
async fn repos_remove_는_진행_중인_조회와_열린_로그_커서를_정리하고_곧바로_폴더를_지울_수_있다() {
    let server = Silent::start();
    let rig = stuck_rig(&server).await;
    let r = rig.router.clone();
    // 열린 로그 커서(한 페이지만 읽었다)
    let env = call(&r, "git:log", json!([G, { "count": 1, "skip": 0 }, "req-log"])).await;
    assert_eq!(data(&env)["hasMore"], true, "{env}");
    assert_eq!(r.log_cursor_count(), 1);
    // 진행 중인 조회: 받지 않은 blob 이 필요해 lazy fetch 에서 멈춘다
    let stuck = {
        let (r, hash, parent) = (r.clone(), rig.src.second.clone(), rig.src.first.clone());
        tokio::spawn(async move { r.route("git:fileDiff", &json!([G, { "hash": hash, "parent": parent, "path": "f.txt" }, "req-diff"])).await })
    };
    let waited = std::time::Instant::now();
    while server.accepted.load(Ordering::SeqCst) == 0 {
        assert!(waited.elapsed() < Duration::from_secs(20), "lazy fetch 가 서버에 닿지 않았다");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(!stuck.is_finished());
    assert!(rig.exec.live_processes(rig.dir.as_str()) > 0, "멈춘 조회의 git 이 진행 중으로 세어져야 한다");
    // local 저장소의 질의는 이 범위에 들지 않는다
    let other = call(&r, "git:commit", json!([L, rig.src.second])).await;
    assert_eq!(other["ok"], true);

    // 설정 소유자의 삭제(`config_host::remove_repo`)가 부르는 정리: Rust 쪽 핸들을 푼다
    r.release_github_repo(G).await;
    let Routed::Done(env) = tokio::time::timeout(Duration::from_secs(10), stuck).await.expect("끊긴 조회가 끝난다").unwrap() else { panic!("Rust 가 풀지 못했다") };
    assert_eq!(env["ok"], false, "{env}");
    assert_eq!(env["code"], "ABORTED", "{env}");
    // route 가 돌아온 시점에 그 저장소의 git 프로세스는 모두 끝나 있다(drain 이 프로세스 종료를 기다렸다)
    assert_eq!(rig.exec.live_processes(rig.dir.as_str()), 0, "정리가 끝났는데 git 이 남았다");
    assert_eq!(r.log_cursor_count(), 0, "열린 커서가 남았다");
    // 폴더를 지우는 시점: TS 와 같은 재시도 예산(10회×100ms)으로 지워져야 한다 — 커서나 멈춘 git 이 살아 있으면 Windows 는 폴더를 지우지 못한다.
    // 앱에 연결하지 않은 Rust 판 삭제 함수(`remove_github_cache`)를 실제 캐시에 써 본다
    let dir = PathBuf::from(rig.dir.as_str());
    assert_eq!(remove_github_cache(&rig.dir), Ok(true), "캐시 폴더를 쥔 핸들이 남았다");
    assert!(!dir.exists());
    // 설정 소유자가 레코드를 지워 미러가 바뀌기 전까지 그 id 의 새 조회는 "삭제 중"으로 거부된다(범위에 들어 폴더를 다시 쥐지 않는다)
    let env = call(&r, "git:commit", json!([G, rig.src.second])).await;
    assert_eq!(env["ok"], false, "{env}");
    assert!(env["error"].as_str().unwrap_or_default().contains("삭제하는 중"), "{env}");
    assert_eq!(rig.exec.live_processes(rig.dir.as_str()), 0);
    // 미러에서 사라지면(레코드가 지워졌다) 표식이 풀리고, 같은 id 가 다시 github 로 풀리면 새 범위를 받는다(끊긴 범위를 재사용하지 않는다) — 폴더가 없으니 git 오류일 뿐 ABORTED 가 아니다
    let local = MirrorRepo { id: L.into(), kind: "local".into(), path: rig.src.dir("octocat", "hello").to_string_lossy().into_owned(), distro: None, ssh_host: None };
    assert!(rig.mirror.apply(2, vec![local.clone()]));
    r.on_mirror_changed();
    let github = MirrorRepo { id: G.into(), kind: "github".into(), path: rig.dir.as_str().to_string(), distro: None, ssh_host: None };
    let links = std::collections::HashMap::from([(G.to_string(), GhLink { owner: "octocat".into(), repo: "hello".into() })]);
    assert!(rig.mirror.apply_with_links(3, vec![github, local], links));
    let env = call(&r, "git:commit", json!([G, rig.src.second])).await;
    assert_ne!(env["code"], "ABORTED", "{env}");
    assert!(!env["error"].as_str().unwrap_or_default().contains("삭제하는 중"), "{env}");
}

#[tokio::test]
async fn repos_remove_뒤_미러가_바뀌기_전까지_그_id_의_새_조회를_거부한다() {
    let rig = Rig::new("file").await;
    let r = &rig.router;
    r.release_github_repo(G).await;
    for ch_args in [("git:commit", json!([G, rig.src.second])), ("git:log", json!([G, { "count": 1, "skip": 0 }]))] {
        let env = call(r, ch_args.0, ch_args.1).await;
        assert_eq!(env["ok"], false, "{env}");
        assert!(env["error"].as_str().unwrap_or_default().contains("삭제하는 중"), "{env}");
    }
    // 다른 저장소(local)는 영향이 없다
    assert_eq!(call(r, "git:commit", json!([L, rig.src.second])).await["ok"], true);
}

#[tokio::test]
async fn repos_remove_는_github_가_아닌_저장소와_모르는_id_에는_아무것도_끊지_않는다() {
    let rig = Rig::new("file").await;
    let r = &rig.router;
    // 열린 local 커서
    let env = call(r, "git:log", json!([L, { "count": 1, "skip": 0 }, "req-log"])).await;
    assert_eq!(data(&env)["hasMore"], true);
    assert_eq!(r.log_cursor_count(), 1);
    for id in [L, "nope", ""] {
        r.release_github_repo(id).await;
    }
    assert_eq!(r.log_cursor_count(), 1, "local 저장소의 커서는 그대로다");
}

struct TempDirPath(PathBuf);

impl TempDirPath {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("antler-ghr-{tag}-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        TempDirPath(dir)
    }
}

impl Drop for TempDirPath {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// 캐시 삭제(`remove_github_cache`) — `GithubCachePath` 로만, 링크는 따라가지 않는다
// ---------------------------------------------------------------------------------------------------------------------

#[test]
fn 캐시_삭제는_캐시_모양_경로만_받고_빈_owner_폴더도_정리한다() {
    let cache = Cache::new();
    let dir = cache.repo("octocat", "hello");
    let other = cache.repo("octocat", "other");
    fs::write(Path::new(dir.as_str()).join("HEAD"), "ref: refs/heads/main\n").unwrap();
    assert_eq!(remove_github_cache(&dir), Ok(true));
    assert!(!Path::new(dir.as_str()).exists());
    // owner 폴더는 다른 저장소가 남아 있어 그대로다
    assert!(Path::new(other.as_str()).exists());
    assert_eq!(remove_github_cache(&other), Ok(true));
    // 비게 된 owner 폴더도 정리된다
    assert!(!Path::new(&cache.cache_dir).join("octocat").exists());
    assert!(Path::new(&cache.cache_dir).exists(), "캐시 기준 폴더는 지우지 않는다");
    // 이미 없으면 false
    assert_eq!(remove_github_cache(&dir), Ok(false));
}

#[test]
fn 캐시_밖_경로와_모양이_틀린_경로는_삭제_타입을_만들_수_없다() {
    let cache = Cache::new();
    let outside = TempDirPath::new("outside");
    fs::create_dir_all(outside.0.join("octocat").join("hello.git")).unwrap();
    for bad in [
        outside.0.join("octocat").join("hello.git").to_string_lossy().into_owned(),
        cache.cache_dir.clone(),
        format!("{}\\octocat", cache.cache_dir),
        format!("{}\\octocat\\hello.git\\objects", cache.cache_dir),
        format!("{}\\..\\octocat\\hello.git", cache.cache_dir),
        format!("{}\\Octocat\\hello.git", cache.cache_dir),
        format!("{}\\octocat\\hello", cache.cache_dir),
        format!("{}\\octocat\\hello.git\0", cache.cache_dir),
    ] {
        assert!(GithubCachePath::new(&cache.cache_dir, &bad).is_none(), "{bad:?}");
    }
    assert!(outside.0.join("octocat").join("hello.git").exists());
}

/// 디렉터리 링크(Windows 는 정션, 그 밖은 심볼릭 링크)를 만든다
fn make_dir_link(link: &Path, target: &Path) {
    #[cfg(windows)]
    {
        #[allow(clippy::disallowed_methods)]
        let ok = std::process::Command::new("cmd").args(["/C", "mklink", "/J"]).arg(link).arg(target).output().expect("mklink").status.success();
        assert!(ok, "정션을 만들지 못했다");
    }
    #[cfg(not(windows))]
    std::os::unix::fs::symlink(target, link).unwrap();
}

#[test]
fn 캐시_삭제는_owner_나_저장소_폴더가_링크이면_따라가지_않고_지우지_않는다() {
    let outside = TempDirPath::new("victim");
    fs::write(outside.0.join("keep.txt"), "x").unwrap();
    // 저장소 폴더가 링크
    let cache = Cache::new();
    let owner_dir = Path::new(&cache.cache_dir).join("octocat");
    fs::create_dir_all(&owner_dir).unwrap();
    let repo_dir = owner_dir.join("hello.git");
    make_dir_link(&repo_dir, &outside.0);
    let path = GithubCachePath::new(&cache.cache_dir, &repo_dir.to_string_lossy()).unwrap();
    assert_eq!(remove_github_cache(&path), Ok(false));
    assert!(outside.0.join("keep.txt").exists(), "링크 너머를 지웠다");
    // owner 폴더가 링크
    let cache2 = Cache::new();
    fs::create_dir_all(outside.0.join("hello.git")).unwrap();
    fs::write(outside.0.join("hello.git").join("keep2.txt"), "x").unwrap();
    let owner_link = Path::new(&cache2.cache_dir).join("octocat");
    make_dir_link(&owner_link, &outside.0);
    let path2 = GithubCachePath::new(&cache2.cache_dir, &owner_link.join("hello.git").to_string_lossy()).unwrap();
    assert_eq!(remove_github_cache(&path2), Ok(false));
    assert!(outside.0.join("hello.git").join("keep2.txt").exists(), "owner 링크 너머를 지웠다");
    assert!(outside.0.join("keep.txt").exists());
    // 링크 자체도 그대로다(지우지 않는다)
    assert!(fs::symlink_metadata(&repo_dir).is_ok() && fs::symlink_metadata(&owner_link).is_ok());
}

// ---------------------------------------------------------------------------------------------------------------------
// 소스 검사
// ---------------------------------------------------------------------------------------------------------------------

fn src(rel: &str) -> String {
    fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}")).replace("\r\n", "\n")
}

/// 운영 코드(들여쓰기 없는 `#[cfg(test)]` 앞까지)
fn prod(text: &str) -> &str {
    text.find("\n#[cfg(test)]\nmod ").map_or(text, |i| &text[..i])
}

#[test]
fn github_실행_대상은_미러_한_곳에서만_만들고_앱은_서비스_뒤에_실행기를_주입한다() {
    let mirror = src("src/mirror.rs");
    assert_eq!(prod(&mirror).matches("Resolved::Rust(Target::Github").count(), 1, "github 를 푸는 곳은 resolve 한 곳이다");
    for file in ["src/router.rs", "src/lib.rs", "src/watch_host.rs", "src/git/fetch/service.rs", "src/fetch_host.rs", "src/routed_repos.rs", "src/git/log_service.rs"] {
        let text = src(file);
        let p = prod(&text);
        assert!(!p.contains("Target::Github { path"), "{file}: 대상을 직접 만들지 않는다");
    }
    // fetch·감시의 소유 술어(종류 목록)의 github 는 조회와 같은 플래그 하나(`github_active`)로만 더해진다(PR 7)
    let router = src("src/router.rs");
    let kinds = prod(&router).split("pub fn rust_fetch_kinds").nth(1).unwrap().split("\n}\n").next().unwrap();
    assert!(kinds.contains("github_active") && kinds.matches("github").count() >= 3, "{kinds}");
    // 앱 배선: 실행기는 GitHub 서비스(set_github) 뒤에, 라우터를 등록하기 전에 주입한다
    let lib = src("src/lib.rs");
    let (host, cache, manage) = (lib.find("router.set_github(api_host::GithubHost").unwrap(), lib.find("router.set_github_cache(").unwrap(), lib.find("app.manage(router)").unwrap());
    assert!(host < cache && cache < manage);
    assert_eq!(prod(&lib).matches("GithubExecutor::new(").count(), 1);
    // fetch 서비스의 실행기 공장은 github 에 일반 실행기를 주지 않는다 — 라우터가 받은 캐시 실행기의 복제이고, 없으면 닫힌 실행기다
    let arm = lib.split("Target::Github { .. } =>").nth(1).unwrap().split("};").next().unwrap();
    assert!(arm.contains("GitExecutor::Github(exec.clone())") && arm.contains("closed.dispose()") && !arm.contains("LocalExecutor"), "{arm}");
    // 토큰의 출처는 GitHub 서비스 하나다
    assert!(lib.contains("token_svc.git_token()"));
}

