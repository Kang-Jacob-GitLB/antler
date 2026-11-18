//! GitHub 원격 전용 저장소 캐시 실행 기반(`antler_lib::git::github_cache`·`GitExecutor::Github`·`run_github_fetch`) 통합 테스트(Phase 6b PR 2).
//! TS `tests/githubRemote.test.ts` 의 설정 검사·`githubEnv`·실제 git 시나리오를 이름 단위로 옮겼다(대응표: `tests/golden/repos/test-map.json`).
//! TS 와의 값 패리티는 `tests/golden/github/*.json`(`tests/goldenGithub.test.ts` 가 TS 로 만든다)이 고정한다 — 판정·환경·가짜 실행기에 남은 argv 만 본다.
//!
//! 격리: 이 바이너리가 시작될 때(`isolate`) 깨끗한 전역 설정(`tests/gitconfig`)과 `GIT_CONFIG_NOSYSTEM` 을 넣고 프록시·askpass 환경을 비운다.
//! 적대적 전역 설정에서의 동작은 `github_cache_isolation.rs`(전역 설정이 다른 바이너리)가 본다. 네트워크는 127.0.0.1 과 `file:///` 만 쓴다.

mod ghcache;

use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use antler_lib::git::exec::{FetchEnv, GitExecutor, LocalExecutor, RunOpts};
use antler_lib::git::fetch::guard::{fetch_config_args, hooks_path_for_test, inspect_entries, parse_config_list, version_at_least, ConfigArgsOpts, FollowRedirects};
use antler_lib::git::fetch::run::{fetch_args_for, git_version, run_fetch, run_github_fetch, FetchFilter, FetchRunOpts, GithubFetchOpts};
use antler_lib::git::github_cache::{
    check_github_config, ensure_github_head, expected_github_config, github_env, init_github_repo, init_github_repo_for_test, inspect_github_repo, inspect_github_repo_for_test, pick_network_config,
    GithubCachePath, GithubEnvOpts, EMPTY_GIT_CONFIG,
};
use antler_lib::git::github_link::{github_remote_url, GITHUB_ORIGIN};
use ghcache::{file_url, link, make_source, raw, raw_git, text, Cache, Fake, TempDir};
use serde_json::Value;

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

// ---------------------------------------------------------------------------------------------------------------------
// 골든(tests/golden/github/*.json — TS 가 만든다)
// ---------------------------------------------------------------------------------------------------------------------

fn golden(name: &str) -> Value {
    let path = ghcache::repo_root().join("tests/golden/github").join(name);
    serde_json::from_str(&fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))).expect("골든 JSON")
}

fn s(v: &Value) -> String {
    v.as_str().expect("문자열").to_string()
}

fn strs(v: &Value) -> Vec<String> {
    v.as_array().expect("배열").iter().map(s).collect()
}

fn pairs(v: &Value) -> Vec<(String, String)> {
    v.as_array().expect("배열").iter().map(|p| (s(&p[0]), s(&p[1]))).collect()
}

fn calls_of(v: &Value) -> Vec<Vec<String>> {
    v.as_array().expect("배열").iter().map(strs).collect()
}

fn link_of(v: &Value) -> antler_lib::git::github_link::GithubLink {
    link(v["owner"].as_str().unwrap(), v["repo"].as_str().unwrap(), v["defaultBranch"].as_str())
}

fn script_of(v: Option<&Value>) -> HashMap<String, ghcache::Reply> {
    let Some(Value::Object(m)) = v else { return HashMap::new() };
    m.iter().map(|(k, r)| (k.clone(), (r[0].as_i64().unwrap() as i32, r.get(1).map_or(String::new(), s), r.get(2).map_or(String::new(), s)))).collect()
}

const GOLDEN_CACHE: &str = "C:\\golden\\cache";
const GOLDEN_DIR: &str = "C:\\golden\\cache\\octocat\\hello.git";
const GOLDEN_HOOKS: &str = "C:/golden/hooks";

fn golden_path() -> GithubCachePath {
    GithubCachePath::new(GOLDEN_CACHE, GOLDEN_DIR).expect("골든 경로")
}

fn url_of(l: &antler_lib::git::github_link::GithubLink, origin: Option<&str>) -> String {
    github_remote_url(&l.owner, &l.repo, origin.unwrap_or(GITHUB_ORIGIN))
}

#[test]
fn 골든_설정_정확_일치_검사가_ts_와_같다() {
    let g = golden("config.json");
    for e in g["expected"].as_array().unwrap() {
        let l = link_of(&e["link"]);
        assert_eq!(expected_github_config(&url_of(&l, e["origin"].as_str())), pairs(&e["config"]));
    }
    let cases = g["cases"].as_array().unwrap();
    assert!(cases.len() > 60, "사례가 줄었다");
    for c in cases {
        let l = link_of(&c["link"]);
        let entries = parse_config_list(c["raw"].as_str().unwrap());
        let got = check_github_config(&entries, &expected_github_config(&url_of(&l, c["origin"].as_str())));
        assert_eq!(got, strs(&c["problems"]), "{}", c["name"]);
    }
}

#[test]
fn 골든_네트워크_설정_고르기가_ts_와_같다() {
    for c in golden("network.json")["cases"].as_array().unwrap() {
        let got = pick_network_config(&parse_config_list(c["raw"].as_str().unwrap()));
        assert_eq!(got, pairs(&c["picked"]), "{}", c["name"]);
    }
}

#[test]
fn 골든_github_env_가_ts_와_같다() {
    let g = golden("env.json");
    let hooks = hooks_path_for_test(true, Some(GOLDEN_HOOKS), true, |_| true).unwrap();
    for c in g["cases"].as_array().unwrap() {
        let name = c["name"].as_str().unwrap();
        let path = GithubCachePath::new(&s(&c["cacheDir"]), &s(&c["gitDir"])).unwrap_or_else(|| panic!("{name}: 캐시 모양이 아니다"));
        let net = pairs(&c["netConfig"]);
        let protocol = c["allowProtocol"].as_str();
        let token = c["token"].as_str();
        let env = github_env(token, &path, &GithubEnvOpts { hooks_path: &hooks, net_config: &net, allow_protocol: protocol });
        // TS 의 EMPTY_GIT_CONFIG 는 import 시점 플랫폼 값이라 골든에는 자리 표시로 있다 — 이 플랫폼의 값으로 푼다
        let want: Vec<(String, String)> = pairs(&c["env"]).into_iter().map(|(k, v)| if v == "<빈 설정>" { (k, EMPTY_GIT_CONFIG.to_string()) } else { (k, v) }).collect();
        assert_eq!(env, want, "{name}");
    }
    let cfg = g["fetchConfig"].as_array().unwrap();
    let args = |f| fetch_config_args(&hooks, ConfigArgsOpts { credential_helpers: false, follow_redirects: f });
    assert_eq!(args(FollowRedirects::False), strs(&cfg[0]));
    assert_eq!(args(FollowRedirects::Initial), strs(&cfg[1]));
}

#[tokio::test]
async fn 골든_head_맞추기의_호출과_결과가_ts_와_같다() {
    let g = golden("head.json");
    let cases = g["cases"].as_array().unwrap();
    assert!(cases.len() >= 15);
    for c in cases {
        let fake = Fake::scripted("github", script_of(Some(&c["script"])), 1);
        let got = ensure_github_head(&fake, &golden_path(), &link_of(&c["link"]), None, &strs(&c["netArgs"])).await;
        let name = c["name"].as_str().unwrap();
        match (got, c.get("error")) {
            (Ok(branch), None) => assert_eq!(branch.as_deref(), c["result"].as_str(), "{name}"),
            (Err(e), Some(want)) => assert_eq!(e.to_string(), s(want), "{name}"),
            (got, _) => panic!("{name}: {got:?} ≠ {c}"),
        }
        assert_eq!(fake.calls(), calls_of(&c["calls"]), "{name}");
    }
}

#[tokio::test]
async fn 골든_init_과_검사의_호출이_ts_와_같다() {
    let g = golden("init.json");
    for c in g["init"].as_array().unwrap() {
        let fake = Fake::scripted("github", script_of(c.get("script")), 0);
        let l = link_of(&c["link"]);
        let got = match c["origin"].as_str() {
            Some(o) => init_github_repo_for_test(&fake, &golden_path(), &l, o, None).await,
            None => init_github_repo(&fake, &golden_path(), &l, None).await,
        };
        match (got, c.get("error")) {
            (Ok(()), None) => {}
            (Err(e), Some(want)) => assert_eq!(e.to_string(), s(want), "{c}"),
            (got, _) => panic!("{got:?} ≠ {c}"),
        }
        assert_eq!(fake.calls(), calls_of(&c["calls"]), "{}", c["name"]);
    }
    for c in g["inspect"].as_array().unwrap() {
        let fake = Fake::scripted("github", script_of(c.get("script")), 1);
        let got = inspect_github_repo(&fake, &golden_path(), &link_of(&c["link"]), None).await;
        let name = c["name"].as_str().unwrap();
        match (got, c.get("error")) {
            (Ok(p), None) => assert_eq!(p, strs(&c["problems"]), "{name}"),
            (Err(e), Some(want)) => assert_eq!(e.to_string(), s(want), "{name}"),
            (got, _) => panic!("{name}: {got:?} ≠ {c}"),
        }
        assert_eq!(fake.calls(), calls_of(&c["calls"]), "{name}");
    }
}

#[tokio::test]
async fn 골든_github_fetch_인자가_ts_와_같다() {
    let hooks = TempDir::new("hooks");
    let hooks_arg = hooks.str().replace('\\', "/");
    for c in golden("fetch.json")["cases"].as_array().unwrap() {
        let replies: Vec<ghcache::Reply> = c["replies"].as_array().unwrap().iter().map(|r| (r[0].as_i64().unwrap() as i32, r.get(1).map_or(String::new(), s), r.get(2).map_or(String::new(), s))).collect();
        let fake = Fake::sequenced("github", replies);
        let got = run_github_fetch(&fake, &golden_path(), &GithubFetchOpts { cancel: None, no_hooks_dir: &hooks.str() }).await;
        let name = c["name"].as_str().unwrap();
        match (got, c.get("error")) {
            (Ok(()), None) => {}
            (Err(e), Some(want)) => assert_eq!(e.to_string(), s(want), "{name}"),
            (got, _) => panic!("{name}: {got:?} ≠ {c}"),
        }
        // 훅 폴더만 이 컴퓨터의 임시 경로다 — 골든의 자리 표시로 바꿔 비교한다
        let calls: Vec<Vec<String>> = fake.calls().into_iter().map(|a| a.into_iter().map(|x| x.replace(&hooks_arg, GOLDEN_HOOKS)).collect()).collect();
        assert_eq!(calls, calls_of(&c["calls"]), "{name}");
    }
    // 같은 인자를 조각으로 조립해도 같다(필터는 FETCH_ARGS 바로 뒤, 닫힌 값 하나)
    let hp = hooks_path_for_test(true, Some(GOLDEN_HOOKS), true, |_| true).unwrap();
    let cfg = fetch_config_args(&hp, ConfigArgsOpts { credential_helpers: false, follow_redirects: FollowRedirects::False });
    let args = fetch_args_for(&cfg, "origin", false, Some(FetchFilter::BlobNone));
    let at = args.iter().position(|a| a == "fetch").unwrap();
    assert_eq!(&args[at..at + 6], ["fetch", "--prune", "--no-prune-tags", "--no-recurse-submodules", "--filter=blob:none", "--no-write-fetch-head"]);
}

// ---------------------------------------------------------------------------------------------------------------------
// 실제 git: 초기화 → fetch(blob:none) → lazy fetch
// ---------------------------------------------------------------------------------------------------------------------

fn path_of(p: &GithubCachePath) -> PathBuf {
    PathBuf::from(p.as_str())
}

/// 소스 저장소(`<srv>/octocat/hello.git`)와 캐시·실행기·저장소 폴더를 한 번에 준비한다
struct Env {
    src: ghcache::Source,
    cache: Cache,
    exec: GitExecutor,
    dir: GithubCachePath,
}

fn env_with(protocol: &str, token: Option<&'static str>) -> Env {
    let src = make_source("octocat", "hello");
    let cache = Cache::new();
    let exec = cache.executor(protocol, token);
    let dir = cache.repo("octocat", "hello");
    Env { src, cache, exec, dir }
}

impl Env {
    async fn init(&self, branch: Option<&str>) {
        init_github_repo_for_test(&self.exec, &self.dir, &link("octocat", "hello", branch), &self.src.origin(), None).await.expect("init");
    }

    async fn inspect(&self) -> Vec<String> {
        inspect_github_repo_for_test(&self.exec, &self.dir, &link("octocat", "hello", None), &self.src.origin(), None).await.expect("inspect")
    }

    async fn fetch(&self) {
        run_github_fetch(&self.exec, &self.dir, &GithubFetchOpts { cancel: None, no_hooks_dir: &self.cache.hooks }).await.expect("fetch");
    }

    fn git(&self, args: &[&str]) -> String {
        raw_git(&path_of(&self.dir), args)
    }
}

#[tokio::test]
async fn init_결과가_정확_일치_검사를_통과하고_fetch_뒤에도_통과한다() {
    let e = env_with("file", None);
    e.init(None).await;
    // 사용자의 init 템플릿·훅이 복사되지 않는다
    assert!(!path_of(&e.dir).join("hooks").join("pre-commit.sample").exists());
    assert_eq!(e.inspect().await, Vec::<String>::new());
    // 프로덕션 URL 로는 맞지 않는다(테스트 URL 로 바꾼 저장소가 실수로 통과하지 않는다)
    let prod = inspect_github_repo(&e.exec, &e.dir, &link("octocat", "hello", None), None).await.unwrap();
    assert!(prod.join(",").contains("remote.origin.url"), "{prod:?}");

    e.fetch().await;
    assert_eq!(e.git(&["rev-parse", "refs/remotes/origin/main"]), e.src.second);
    // fetch --filter 가 설정을 덧쓰지 않았는지: 두 번째 fetch 전에 다시 검사해도 통과한다
    assert_eq!(e.inspect().await, Vec::<String>::new());
    e.fetch().await;
    assert_eq!(e.inspect().await, Vec::<String>::new());
}

#[tokio::test]
async fn 받은_뒤에_설정이_바뀌면_검사가_막는다() {
    let e = env_with("file", None);
    e.init(None).await;
    e.git(&["config", "--local", "core.sshCommand", "evil"]);
    assert!(e.inspect().await.join(",").contains("core.sshcommand"));
    e.git(&["config", "--local", "--unset", "core.sshCommand"]);
    e.git(&["config", "--local", "remote.origin.promisor", "false"]);
    assert!(e.inspect().await.join(",").contains("remote.origin.promisor"));
    // include 로 고정 키를 가리는 시도도 막힌다
    e.git(&["config", "--local", "remote.origin.promisor", "true"]);
    fs::write(path_of(&e.dir).join("extra.cfg"), "[remote \"origin\"]\n\tpromisor = false\n").unwrap();
    e.git(&["config", "--local", "include.path", "extra.cfg"]);
    assert!(e.inspect().await.join(",").contains("include.path"));
}

#[tokio::test]
async fn head_를_받아_온_기본_브랜치로_맞춘다() {
    let e = env_with("file", None);
    e.init(None).await;
    e.fetch().await;
    // git 2.48+ 는 fetch 때 refs/remotes/origin/HEAD 를 만든다. 없는 경우(옛 git)도 같게 되는지 지우고 확인한다
    e.git(&["update-ref", "-d", "--no-deref", "refs/remotes/origin/HEAD"]);
    let l = link("octocat", "hello", None);
    assert_eq!(ensure_github_head(&e.exec, &e.dir, &l, None, &[]).await.unwrap().as_deref(), Some("main"));
    assert_eq!(e.git(&["symbolic-ref", "HEAD"]), "refs/remotes/origin/main");
    assert_eq!(e.git(&["symbolic-ref", "refs/remotes/origin/HEAD"]), "refs/remotes/origin/main");
    assert_eq!(e.git(&["rev-parse", "HEAD"]), e.src.second);
    // 레코드의 기본 브랜치가 실제와 다르면(이름이 바뀜) 원격에 물어 알아낸다
    assert_eq!(ensure_github_head(&e.exec, &e.dir, &link("octocat", "hello", Some("gone")), None, &[]).await.unwrap().as_deref(), Some("main"));
}

#[tokio::test]
async fn 원격에서_기본_브랜치_이름이_바뀌면_다시_알아내_head_를_옮긴다() {
    let e = env_with("file", None);
    let l = link("octocat", "hello", Some("main"));
    e.init(Some("main")).await;
    e.fetch().await;
    assert_eq!(ensure_github_head(&e.exec, &e.dir, &l, None, &[]).await.unwrap().as_deref(), Some("main"));
    raw_git(&e.src.dir("octocat", "hello"), &["branch", "-m", "main", "trunk"]);
    e.fetch().await; // --prune 이 refs/remotes/origin/main 을 지운다
    let gone = e.exec.run(e.dir.as_str(), &["rev-parse", "--verify", "--quiet", "refs/remotes/origin/main"], &RunOpts::default()).await.unwrap();
    assert_ne!(gone.code, 0);
    // 레코드(main)가 가리키는 ref 가 없으니 origin/HEAD → ls-remote 순서로 trunk 를 찾는다
    assert_eq!(ensure_github_head(&e.exec, &e.dir, &l, None, &[]).await.unwrap().as_deref(), Some("trunk"));
    assert_eq!(e.git(&["symbolic-ref", "HEAD"]), "refs/remotes/origin/trunk");
    assert_eq!(e.git(&["rev-parse", "HEAD"]), e.src.second);
}

#[tokio::test]
async fn 기본_브랜치를_미리_알면_init_이_head_를_가리켜_둔다() {
    let e = env_with("file", None);
    e.init(Some("main")).await;
    assert_eq!(e.git(&["symbolic-ref", "HEAD"]), "refs/remotes/origin/main");
    // 쓸 수 없는 이름은 init 이 거부한다(링크 검증이 버린 값이 다른 길로 오지 못한다)
    let bad = env_with("file", None);
    let err = init_github_repo_for_test(&bad.exec, &bad.dir, &link("octocat", "hello", Some("-x")), &bad.src.origin(), None).await.unwrap_err();
    assert!(err.to_string().contains("기본 브랜치"), "{err}");
}

async fn lazy_supported() -> bool {
    let v = git_version(&GitExecutor::Local(LocalExecutor::default())).await;
    version_at_least(v, [2.0, 45.0, 1.0])
}

#[tokio::test]
async fn lazy_fetch_는_github_환경에서만_받지_않은_blob_이_필요한_명령이_성공한다() {
    if !lazy_supported().await {
        return eprintln!("[건너뜀] GIT_NO_LAZY_FETCH 는 git 2.45.1+");
    }
    let e = env_with("file", None);
    let l = link("octocat", "hello", None);
    e.init(None).await;
    e.fetch().await;
    ensure_github_head(&e.exec, &e.dir, &l, None, &[]).await.unwrap();
    // 커밋·트리는 있고 blob 은 없다
    assert!(e.git(&["rev-list", "--objects", "--missing=print", "HEAD"]).lines().any(|l| l.starts_with('?')));

    let numstat = ["diff-tree", "-r", "--numstat", "--no-commit-id", e.src.first.as_str(), e.src.second.as_str()];
    // 기본 환경(GIT_NO_LAZY_FETCH=1, 전송 금지): 로그는 되지만 blob 이 필요한 numstat 은 실패한다
    let off = GitExecutor::Local(LocalExecutor::default());
    let o = RunOpts::default();
    let log = off.run(e.dir.as_str(), &["log", "--format=%s", "HEAD"], &o).await.unwrap();
    assert_eq!(text(&log.stdout).lines().collect::<Vec<_>>(), ["second", "first"]);
    assert_ne!(off.run(e.dir.as_str(), &numstat, &o).await.unwrap().code, 0);
    // github 환경: 필요한 blob 만 그때 받아 성공한다. `stream` 도 같은 환경이다(log·diff 스트림의 lazy fetch)
    let open = e.exec.run(e.dir.as_str(), &numstat, &o).await.unwrap();
    assert_eq!(open.stderr, "");
    assert_eq!(open.code, 0);
    assert!(String::from_utf8_lossy(&open.stdout).contains("1\t0\tf.txt"));
    // 받은 뒤에는 기본 환경에서도 읽힌다
    assert_eq!(off.run(e.dir.as_str(), &numstat, &o).await.unwrap().code, 0);

    // stream 도 같은 환경이다: 아직 받지 않은 blob 을 `cat-file` 스트림이 lazy fetch 로 받는다(커밋·트리만 받은 새 캐시)
    let e2 = env_with("file", None);
    e2.init(None).await;
    e2.fetch().await;
    let blob = e2.git(&["rev-parse", &format!("{}:f.txt", e2.src.first)]);
    let mut st = e2.exec.stream(e2.dir.as_str(), &["cat-file", "blob", &blob], &o).await.unwrap();
    let mut buf = Vec::new();
    tokio::io::AsyncReadExt::read_to_end(&mut st, &mut buf).await.unwrap();
    st.finish().await.unwrap();
    assert_eq!(String::from_utf8_lossy(&buf), "one\n");
}

#[tokio::test]
async fn github_환경은_캐시_경로의_저장소에만_열린다() {
    let e = env_with("file", Some("ghp_golden_fake_token"));
    e.init(None).await;
    let o = RunOpts::default();
    let alias = "alias.e=!printf %s \"$GIT_NO_LAZY_FETCH:$GIT_ALLOW_PROTOCOL\"";
    let inside = e.exec.run(e.dir.as_str(), &["-c", alias, "e"], &o).await.unwrap();
    assert_eq!(text(&inside.stdout), "0:file");
    // 환경 전체: 토큰은 캐시 경로의 git 에만 있고, 한 값(Authorization)뿐이다
    let envs = e.exec.run(e.dir.as_str(), &["-c", "alias.e=!env", "e"], &o).await.unwrap();
    let out = String::from_utf8_lossy(&envs.stdout).into_owned();
    assert!(out.contains("Authorization: Basic "), "캐시 경로에서 토큰 헤더가 없다");
    // (값은 보지 않는다: 셸이 경로형 환경 변수를 POSIX 모양으로 바꿀 수 있다)
    assert!(out.contains("GIT_DIR=") && out.contains("GIT_CEILING_DIRECTORIES="), "GIT_DIR 이 고정돼야 한다");
    assert!(!out.contains("ghp_golden_fake_token"), "토큰 원문이 환경에 있다");

    // 캐시 밖 경로(캐시 모양이 아니거나 다른 폴더): 기본 환경(전송 금지)이고 토큰이 없다
    let plain = TempDir::new("plain");
    let out = raw("git").args(["init", "-q", "-b", "main"]).arg(plain.path()).output().unwrap();
    assert!(out.status.success());
    for dir in [plain.str(), e.cache.root.str()] {
        let r = e.exec.run(&dir, &["-c", alias, "e"], &o).await.unwrap();
        assert_eq!(text(&r.stdout), "1:none", "{dir}");
        let r = e.exec.run(&dir, &["-c", "alias.e=!env", "e"], &o).await.unwrap();
        let out = String::from_utf8_lossy(&r.stdout);
        assert!(!out.contains("Authorization") && !out.contains("GIT_CEILING_DIRECTORIES=") && !out.contains("GIT_CONFIG_COUNT"), "{dir}: {out}");
    }
}

#[tokio::test]
async fn 오류_문구에서_토큰과_base64_를_가린다() {
    let e = env_with("file", Some("ghp_golden_fake_token"));
    e.init(None).await;
    // 환경의 헤더 값(`Authorization: Basic <base64>`)을 stderr 로 내고 실패한다
    let r = e.exec.run(e.dir.as_str(), &["-c", "alias.e=!echo \"$GIT_CONFIG_VALUE_10 ghp_golden_fake_token\" >&2; exit 3", "e"], &RunOpts::default()).await.unwrap();
    assert_eq!(r.code, 3);
    assert!(r.stderr.contains("Authorization: Basic ***"), "{}", r.stderr);
    assert!(!r.stderr.contains("ghp_golden_fake_token") && !r.stderr.contains("Z2hwX2dvbGRlbl9mYWtlX3Rva2Vu"), "{}", r.stderr);
}

#[tokio::test]
async fn stream_의_실패_문구와_run_의_stdout_에서도_토큰을_가린다() {
    let e = env_with("file", Some("ghp_golden_fake_token"));
    e.init(None).await;
    let b64 = "Z2hwX2dvbGRlbl9mYWtlX3Rva2Vu";
    // stream: stdout 은 그대로 읽고, 실패하면 `finish` 의 오류 문구(message·stderr)가 가려진다
    let alias = "alias.e=!echo \"$GIT_CONFIG_VALUE_10\" >&2; exit 3";
    let mut st = e.exec.stream(e.dir.as_str(), &["-c", alias, "e"], &RunOpts::default()).await.unwrap();
    let mut buf = Vec::new();
    tokio::io::AsyncReadExt::read_to_end(&mut st, &mut buf).await.unwrap();
    let err = st.finish().await.unwrap_err();
    let shown = format!("{err:?} {err}");
    assert!(shown.contains("Authorization: Basic ***"), "{shown}");
    assert!(!shown.contains("ghp_golden_fake_token") && !shown.contains(b64), "{shown}");
    // run: 환경의 헤더를 보여 주는 `config --list`(command scope)의 stdout 에도 토큰이 없다
    let list = e.exec.run(e.dir.as_str(), &["config", "--list", "--show-scope"], &RunOpts::default()).await.unwrap();
    let out = String::from_utf8_lossy(&list.stdout).into_owned();
    assert!(out.contains("extraheader=Authorization: Basic ***"), "대조: 환경의 헤더가 config 목록에 보여야 한다: {out}");
    assert!(!out.contains("ghp_golden_fake_token") && !out.contains(b64), "{out}");
}

#[tokio::test]
async fn fetch_환경을_줘도_github_환경이_그대로다() {
    let e = env_with("http", None);
    e.init(None).await;
    let fe = FetchEnv { allow_file: true, ssh_command: "ssh -x".to_string() };
    let o = RunOpts { fetch_env: Some(fe), ..RunOpts::default() };
    let r = e.exec.run(e.dir.as_str(), &["-c", "alias.e=!printf %s \"$GIT_ALLOW_PROTOCOL:$GIT_SSH_COMMAND\"", "e"], &o).await.unwrap();
    assert_eq!(text(&r.stdout), "http:", "일반 fetch 환경이 github 환경을 덮어썼다");
}

#[tokio::test]
async fn 일반_fetch_와_github_fetch_는_서로의_실행기로_부르면_실패한다() {
    let e = env_with("file", None);
    e.init(None).await;
    let ins = inspect_entries(&[]);
    let normal = FetchRunOpts { cancel: None, no_hooks_dir: Some(&e.cache.hooks), allow_file: false, credential_helpers: false };
    let err = run_fetch(&e.exec, e.dir.as_str(), &ins, &normal).await.unwrap_err();
    assert!(err.to_string().contains("run_github_fetch"), "{err}");
    let local = GitExecutor::Local(LocalExecutor::default());
    let err = run_github_fetch(&local, &e.dir, &GithubFetchOpts { cancel: None, no_hooks_dir: &e.cache.hooks }).await.unwrap_err();
    assert!(err.to_string().contains("GitHub 캐시 실행기"), "{err}");
    // 아무것도 받지 않았다
    assert!(raw("git").arg("-C").arg(path_of(&e.dir)).args(["rev-parse", "--verify", "--quiet", "refs/remotes/origin/main"]).output().unwrap().status.code() != Some(0));
}

#[tokio::test]
async fn github_fetch_는_훅_디렉터리가_쓸_수_없으면_실행하지_않는다() {
    let e = env_with("file", None);
    e.init(None).await;
    let err = run_github_fetch(&e.exec, &e.dir, &GithubFetchOpts { cancel: None, no_hooks_dir: "C:/없는/디렉터리" }).await.unwrap_err();
    assert!(err.to_string().contains("훅 차단"), "{err}");
    // 비어 있지 않은 디렉터리도 마찬가지다
    fs::write(Path::new(&e.cache.hooks).join("post-merge"), "x").unwrap();
    let err = run_github_fetch(&e.exec, &e.dir, &GithubFetchOpts { cancel: None, no_hooks_dir: &e.cache.hooks }).await.unwrap_err();
    assert!(err.to_string().contains("훅 차단"), "{err}");
    // 캐시 경로의 일반 조회도 같은 이유로 실패한다(보호 없이 실행하지 않는다)
    assert!(e.exec.run(e.dir.as_str(), &["rev-parse", "--git-dir"], &RunOpts::default()).await.is_err());
}

#[tokio::test]
async fn reftable_저장소도_검사를_통과하고_fetch_log_가_된다() {
    let v = git_version(&GitExecutor::Local(LocalExecutor::default())).await;
    if !version_at_least(v, [2.0, 45.0, 0.0]) {
        return eprintln!("[건너뜀] --ref-format 은 git 2.45+");
    }
    let e = env_with("file", None);
    let o = RunOpts::default();
    // init_github_repo 는 `git init` 을 고정 인자로 부른다 — 형식만 reftable 로 바꿔 같은 순서를 직접 밟는다
    let init = e.exec.run(e.dir.as_str(), &["init", "-q", "--bare", "--template=", "--ref-format=reftable"], &o).await.unwrap();
    assert_eq!(init.code, 0, "{}", init.stderr);
    for (k, v) in expected_github_config(&github_remote_url("octocat", "hello", &e.src.origin())) {
        assert_eq!(e.exec.run(e.dir.as_str(), &["config", "--local", &k, &v], &o).await.unwrap().code, 0);
    }
    let fmt = e.exec.run(e.dir.as_str(), &["config", "--local", "extensions.refstorage"], &o).await.unwrap();
    assert_eq!(text(&fmt.stdout), "reftable");
    assert_eq!(e.inspect().await, Vec::<String>::new());
    e.fetch().await;
    let l = link("octocat", "hello", None);
    assert_eq!(ensure_github_head(&e.exec, &e.dir, &l, None, &[]).await.unwrap().as_deref(), Some("main"));
    let log = e.exec.run(e.dir.as_str(), &["log", "--format=%s", "HEAD"], &o).await.unwrap();
    assert_eq!(text(&log.stdout).lines().collect::<Vec<_>>(), ["second", "first"]);
}

#[tokio::test]
async fn 캐시_폴더가_비었거나_깨져_있으면_상위_저장소로_올라가지_않고_실패한다() {
    let e = env_with("file", None);
    // 캐시 기준 폴더가 다른 저장소 안에 있는 상황: 바깥을 저장소로 만든다
    let out = raw("git").args(["init", "-q"]).arg(e.cache.root.path()).output().unwrap();
    assert!(out.status.success());
    let outer_cfg = e.cache.root.path().join(".git").join("config");
    let before = fs::read_to_string(&outer_cfg).unwrap();
    let o = RunOpts::default();
    // dir 은 비어 있다(init 하지 않았다)
    let write = e.exec.run(e.dir.as_str(), &["config", "--local", "remote.origin.promisor", "true"], &o).await.unwrap();
    assert_ne!(write.code, 0);
    assert_ne!(e.exec.run(e.dir.as_str(), &["log", "--format=%H"], &o).await.unwrap().code, 0);
    assert_ne!(e.exec.run(e.dir.as_str(), &["rev-parse", "--git-dir"], &o).await.unwrap().code, 0);
    // 설정 검사도 상위 저장소의 설정을 보지 않으므로 fetch 앞에서 차단된다(읽지 못하거나 필수 키 없음)
    let problems = inspect_github_repo(&e.exec, &e.dir, &link("octocat", "hello", None), None).await;
    assert!(problems.map_or(true, |p| !p.is_empty()));
    assert_eq!(fs::read_to_string(&outer_cfg).unwrap(), before, "상위 저장소의 설정이 바뀌었다");
    // 대조: 격리가 없으면 같은 명령이 상위 저장소에 쓴다
    let plain = GitExecutor::Local(LocalExecutor::default());
    assert_eq!(plain.run(e.dir.as_str(), &["config", "--local", "remote.origin.promisor", "true"], &o).await.unwrap().code, 0);
    assert_ne!(fs::read_to_string(&outer_cfg).unwrap(), before);
}

#[tokio::test]
async fn 읽기_전용_pack_이_든_캐시_폴더도_지워진다() {
    let e = env_with("file", None);
    e.init(None).await;
    e.fetch().await;
    let pack_dir = path_of(&e.dir).join("objects").join("pack");
    let packs: Vec<PathBuf> = fs::read_dir(&pack_dir).unwrap().map(|x| x.unwrap().path()).filter(|p| p.extension().is_some_and(|x| x == "pack")).collect();
    assert!(!packs.is_empty(), "fetch 가 pack 을 만들지 않았다");
    // git 은 pack 을 읽기 전용으로 만든다 — 아니면 이 시험이 공허하므로 직접 건다
    for p in &packs {
        let mut perm = fs::metadata(p).unwrap().permissions();
        perm.set_readonly(true);
        fs::set_permissions(p, perm).unwrap();
        assert!(fs::metadata(p).unwrap().permissions().readonly());
    }
    fs::remove_dir_all(path_of(&e.dir)).expect("읽기 전용 pack 이 든 폴더를 지운다(PR 9 의 삭제 구현이 속성을 풀 필요가 없다)");
    assert!(!path_of(&e.dir).exists());
}

// ---------------------------------------------------------------------------------------------------------------------
// 리다이렉트: 127.0.0.1 가짜 서버
// ---------------------------------------------------------------------------------------------------------------------

/// 요청(첫 줄 + 헤더)을 모으는 가짜 HTTP 서버. `redirect_to` 가 있으면 모든 요청을 302 로 돌린다(없으면 404)
struct Server {
    port: u16,
    requests: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Server {
    fn start(redirect_to: Option<u16>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let (reqs, stop2) = (requests.clone(), stop.clone());
        let thread = std::thread::spawn(move || {
            while !stop2.load(Ordering::SeqCst) {
                let Ok((mut conn, _)) = listener.accept() else {
                    std::thread::sleep(Duration::from_millis(10));
                    continue;
                };
                conn.set_nonblocking(false).unwrap();
                conn.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
                let mut head = Vec::new();
                let mut b = [0u8; 1];
                while !head.ends_with(b"\r\n\r\n") && conn.read(&mut b).is_ok_and(|n| n == 1) {
                    head.push(b[0]);
                }
                let head = String::from_utf8_lossy(&head).into_owned();
                let path = head.lines().next().and_then(|l| l.split(' ').nth(1)).unwrap_or("/").to_string();
                reqs.lock().unwrap().push(head);
                let resp = match redirect_to {
                    Some(p) => format!("HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:{p}{path}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"),
                    None => "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_string(),
                };
                let _ = conn.write_all(resp.as_bytes());
            }
        });
        Server { port, requests, stop, thread: Some(thread) }
    }

    fn requests(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

#[tokio::test]
async fn 리다이렉트를_따라가지_않고_토큰이_다른_호스트로_가지_않는다() {
    let target = Server::start(None);
    let front = Server::start(Some(target.port));
    let cache = Cache::new();
    let exec = cache.executor("http", Some("ghp_golden_fake_token"));
    let dir = cache.repo("octocat", "hello");
    init_github_repo_for_test(&exec, &dir, &link("octocat", "hello", None), &format!("http://127.0.0.1:{}", front.port), None).await.unwrap();
    let o = RunOpts::default();

    // 대조: `-c http.followRedirects=initial`(일반 fetch 의 기본값)은 환경의 false 를 이겨 리다이렉트를 따라간다 — 이 시험이 공허하지 않다는 증거
    let followed = exec.run(dir.as_str(), &["-c", "http.followRedirects=initial", "ls-remote", "origin"], &o).await.unwrap();
    assert_ne!(followed.code, 0);
    assert!(!target.requests().is_empty(), "대조군이 리다이렉트를 따라가지 않았다 — 가짜 서버가 git 의 요청을 받지 못한 것이다");
    let seen = target.requests().len();

    // github 경로: fetch 도 ls-remote 도 리다이렉트 너머로 가지 않는다
    let r = exec.run(dir.as_str(), &["ls-remote", "origin"], &o).await.unwrap();
    assert_ne!(r.code, 0);
    let err = run_github_fetch(&exec, &dir, &GithubFetchOpts { cancel: None, no_hooks_dir: &cache.hooks }).await.unwrap_err();
    assert!(!err.to_string().contains("ghp_golden_fake_token"));
    assert_eq!(target.requests().len(), seen, "github 경로가 리다이렉트를 따라갔다");
    // 토큰 헤더(`http.https://github.com/.extraheader`)는 github.com 이 아닌 호스트에는 가지 않는다
    for req in front.requests().iter().chain(target.requests().iter()) {
        assert!(!req.to_ascii_lowercase().contains("authorization"), "{req}");
    }
    assert!(!front.requests().is_empty());
}

// ---------------------------------------------------------------------------------------------------------------------
// 소스 검사: 환경을 만드는 곳은 한 곳이다
// ---------------------------------------------------------------------------------------------------------------------

fn rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        if p.is_dir() {
            rs_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

/// 주석이 아닌 줄만 있는 소스(파일별)
fn sources() -> Vec<(String, String)> {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rs_files(&src, &mut files);
    assert!(files.len() > 40, "src 를 찾지 못했다");
    files
        .iter()
        .map(|f| {
            let rel = f.strip_prefix(&src).unwrap().to_string_lossy().replace('\\', "/");
            let code = fs::read_to_string(f).unwrap().lines().filter(|l| !l.trim_start().starts_with("//")).collect::<Vec<_>>().join("\n");
            (rel, code)
        })
        .collect()
}

#[test]
fn github_환경을_만드는_곳은_github_cache_rs_한_곳이다() {
    let all = sources();
    let holders = |needle: &str| {
        let mut v = all.iter().filter(|(_, c)| c.contains(needle)).map(|(r, _)| r.as_str()).collect::<Vec<_>>();
        v.sort_unstable();
        v
    };
    // 토큰 헤더 키·lazy fetch 를 여는 값은 이 파일에만 있다
    assert_eq!(holders("extraheader"), ["git/github_cache.rs"]);
    assert_eq!(holders("(\"GIT_NO_LAZY_FETCH\", \"0\")"), ["git/github_cache.rs"]);
    assert_eq!(holders("\"GIT_LFS_SKIP_SMUDGE\""), ["git/github_cache.rs"]);
    // 환경 함수를 부르는 곳은 실행기 하나뿐이다
    assert_eq!(holders("github_env("), ["git/exec/github.rs", "git/github_cache.rs"]);
    // 캐시 모양 판정을 거치지 않고 `GithubCachePath` 를 만들 수 없다(필드 직접 구성은 같은 파일 안에서만)
    assert_eq!(holders("GithubCachePath {"), ["git/github_cache.rs"]);
    // 훅 경로는 일반 local 규칙(`hooks_path_for`) 한 갈래만 쓴다 — github 모듈이 `_for_test` 주입을 쓰지 않는다
    for rel in ["git/exec/github.rs", "git/github_cache.rs"] {
        let code = &all.iter().find(|(r, _)| r == rel).unwrap().1;
        let prod = code.split("#[cfg(test)]").next().unwrap();
        assert!(!prod.contains("hooks_path_for_test") && !prod.contains("platform_win32"), "{rel}");
    }
}

#[test]
fn 환경을_가진_실행기_상태는_토큰을_찍지_않는다() {
    // Spec 은 Debug 로 환경 값을 찍지 않고 버릴 때 0 으로 덮는다(exec/common.rs)
    let all = sources();
    let common = &all.iter().find(|(r, _)| r == "git/exec/common.rs").unwrap().1;
    assert!(common.contains("impl std::fmt::Debug for Spec") && common.contains("impl Drop for Spec"));
    assert!(!common.contains("#[derive(Debug, Clone)]\npub(super) struct Spec"));
}

// 골든 입력 확인용: 가짜 파일 URL 도우미가 쓰는 모양(퍼센트 인코딩)
#[test]
fn 파일_url_은_공백을_인코딩한다() {
    assert_eq!(file_url(Path::new("C:\\a b\\c")), "file:///C:/a%20b/c");
}
