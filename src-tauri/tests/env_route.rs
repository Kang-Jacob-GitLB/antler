//! 환경 채널 5개(`env:wslDistros`·`env:sshHosts`·`env:test`·`env:listDir`·`repos:scan`, Phase 6b PR 8)의 라우팅·입력 검증·결과 통합 테스트.
//! 기대값은 TS 핸들러(`src/main/ipc/handlers.ts` 의 `checkEnvInput`·핸들러)와 `RepoService`(`scanRepos`·`testEnv`·`listRemoteDir`)가 낼 봉투다: 오류는 `{ok:false,error,code:"ERROR"}`.
//! TS 쪽은 electron 을 띄워야 해 핸들러를 가짜 host 로 돌리는 골든 하네스를 만들지 않았다 — 기대값은 TS 코드를 읽고 손으로 적었다(PR 5 의 `app_route.rs` 와 같은 방식).
//!
//! **대화상자·`wsl.exe`·사용자의 `~/.ssh`·ssh-agent 를 쓰지 않는다**: 확인 창·WSL 설치 목록·실행기·별칭 목록은 가짜 포트이고, SSH 는 127.0.0.1 의 프로세스 안 `russh::server`(`tests/ssh/server.rs`)와
//! 임시 홈·`AgentSetting::Disabled` 만 쓴다. 로컬 찾기는 임시 폴더(정션 포함)에서 한다. 시간 의존은 `wait_for`(조건 + 상한)다.

use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use antler_lib::env_host::{ssh_hosts_for_home, EnvHost, SshHostInfo};
use antler_lib::git::exec::ssh::config::local_user_name;
use antler_lib::git::exec::ssh::win32_path;
use antler_lib::git::exec::ssh::SshPool;
use antler_lib::git::exec::{GitExecutor, LocalExecutor};
use antler_lib::mirror::RepoMirror;
use antler_lib::router::{Routed, Router, API_CHANNELS, APP_CHANNELS, ENV_CHANNELS, RUST_CHANNELS};
use serde_json::{json, Value};

#[path = "ssh/common.rs"]
mod common;
#[path = "ssh/server.rs"]
mod server;

use common::{git, known_hosts_line, make_repo, wait_for, Home, TempDir};
use server::fake_sftp::{FakeTree, Sftp};
use server::{new_key, Opts, TestServer};

// unsafe: main 이전 코드는 Rust 런타임 초기화 전에 돈다 — 여기서는 환경 변수만 넣고 다른 것에 기대지 않는다
#[ctor::ctor(unsafe)]
fn isolate() {
    let global = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("gitconfig");
    std::env::set_var("GIT_CONFIG_GLOBAL", global);
    std::env::set_var("GIT_CONFIG_NOSYSTEM", "1");
    std::env::set_var("GCM_INTERACTIVE", "never");
}

fn read_src(rel: &str) -> String {
    fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

fn ipc_channels() -> Vec<String> {
    let text = fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().join("tests/golden/exec/channels.json")).unwrap();
    let v: Value = serde_json::from_str(&text).unwrap();
    v["channels"].as_array().unwrap().iter().map(|c| c.as_str().unwrap().to_string()).collect()
}

fn err(msg: &str) -> Value {
    json!({ "ok": false, "error": msg, "code": "ERROR" })
}

const NET_PATH: &str = "네트워크 공유 경로는 지원하지 않습니다(WSL 경로는 \\\\wsl.localhost\\… 로 고르세요)";

/// 포트 호출 기록
#[derive(Default)]
struct Calls {
    distros: AtomicUsize,
    /// 확인 창에 넘어간 메시지 상자 인자
    confirms: Mutex<Vec<Value>>,
    /// local·wsl 실행기를 만든 요청(kind, 배포판)
    execs: Mutex<Vec<(String, Option<String>)>>,
}

impl Calls {
    fn quiet(&self) -> bool {
        self.distros.load(Ordering::SeqCst) == 0 && self.confirms.lock().unwrap().is_empty() && self.execs.lock().unwrap().is_empty()
    }
}

/// 가짜 포트. `allow` 가 참이면 확인 창이 "연결"을 누른다. 실행기는 wsl 도 로컬 git 으로 돌려 `wsl.exe` 를 부르지 않는다
fn host_with(calls: &Arc<Calls>, installed: &[&str], ssh_hosts: antler_lib::env_host::SshHostsFn, allow: &Arc<AtomicBool>) -> EnvHost {
    let (c1, c2, c3) = (calls.clone(), calls.clone(), calls.clone());
    let installed: Vec<String> = installed.iter().map(|s| (*s).to_string()).collect();
    let allow = allow.clone();
    EnvHost::new(
        Arc::new(move || {
            c1.distros.fetch_add(1, Ordering::SeqCst);
            let list = installed.clone();
            Box::pin(async move { list })
        }),
        ssh_hosts,
        Arc::new(move |arg| {
            c2.confirms.lock().unwrap().push(arg.clone());
            allow.load(Ordering::SeqCst)
        }),
        Arc::new(move |kind, distro| {
            c3.execs.lock().unwrap().push((kind.to_string(), distro.map(str::to_string)));
            GitExecutor::Local(LocalExecutor::default())
        }),
    )
}

fn host(calls: &Arc<Calls>, installed: &[&str]) -> EnvHost {
    host_with(calls, installed, Arc::new(Vec::new), &Arc::new(AtomicBool::new(true)))
}

fn router(host: Option<EnvHost>) -> Router {
    let r = Router::new(false, RepoMirror::default());
    if let Some(h) = host {
        r.set_env(h);
    }
    r
}

async fn done(r: &Router, channel: &str, args: Value) -> Value {
    match r.route(channel, &args).await {
        Routed::Done(v) => v,
        Routed::Unserved => panic!("{channel} 를 풀지 못했다"),
    }
}

// ── 목록·라우팅 ────────────────────────────────────────────────────────────────

#[test]
fn 환경_채널은_ipc_채널의_부분집합이고_다른_소유_목록과_겹치지_않는다() {
    let all = ipc_channels();
    assert_eq!(ENV_CHANNELS, ["env:wslDistros", "env:sshHosts", "env:test", "env:listDir", "repos:scan"]);
    for ch in ENV_CHANNELS {
        assert!(all.iter().any(|c| c == ch), "{ch} 가 IPC_CHANNELS 에 없다(tests/golden/exec/channels.json)");
        assert!(
            !RUST_CHANNELS.contains(ch) && !APP_CHANNELS.contains(ch) && !API_CHANNELS.contains(ch),
            "{ch} 가 다른 Rust 채널 목록에도 있다"
        );
    }
}

#[tokio::test]
async fn 풀이_없으면_ssh_종류는_ssh_를_사용할_수_없다고_답하고_나머지는_정상_처리한다() {
    let calls = Arc::new(Calls::default());
    let r = router(Some(host(&calls, &["Ubuntu"])));
    assert!(!r.ssh_active());
    // env:wslDistros·env:sshHosts·local·wsl 종류는 풀과 무관하다
    assert_eq!(done(&r, "env:wslDistros", json!([])).await, json!({ "ok": true, "data": ["Ubuntu"] }));
    assert_eq!(done(&r, "env:sshHosts", json!([])).await, json!({ "ok": true, "data": [] }));
    assert_eq!(done(&r, "env:test", json!([{ "kind": "local" }])).await["ok"], true);
    assert_eq!(done(&r, "env:test", json!([{ "kind": "wsl", "distro": "Ubuntu" }])).await["ok"], true);
    assert_eq!(r.rust_handled(), 4);
    // ssh 종류(`env:listDir` 는 항상 ssh) — 풀이 없으면 연결하지 않고 오류다
    for (ch, args) in [
        ("env:test", json!([{ "kind": "ssh", "sshHost": "h" }])),
        ("repos:scan", json!([{ "kind": "ssh", "sshHost": "h", "path": "~" }])),
        ("env:listDir", json!([{ "sshHost": "h", "path": "" }])),
    ] {
        let env = done(&r, ch, args.clone()).await;
        assert_eq!((env["ok"].clone(), env["code"].clone()), (json!(false), json!("ERROR")), "{ch} {args}");
    }
    assert_eq!(r.rust_handled(), 7);
}

#[tokio::test]
async fn 포트가_없으면_환경_채널은_풀_수_없다() {
    let calls = Arc::new(Calls::default());
    let none = router(None);
    for ch in ENV_CHANNELS {
        assert!(matches!(none.route(ch, &json!([{ "kind": "local", "path": "C:\\x" }])).await, Routed::Unserved), "{ch}");
    }
    assert_eq!(none.rust_handled(), 0);
    assert!(calls.quiet(), "포트가 불렸다");
}

#[tokio::test]
async fn 배열이_아닌_인자도_빈_인자로_보고_검증이_거부한다() {
    let calls = Arc::new(Calls::default());
    let r = router(Some(host(&calls, &[])));
    for args in [json!("x"), json!(null), json!({ "kind": "local" })] {
        assert_eq!(done(&r, "env:test", args.clone()).await, err("잘못된 입력입니다"), "{args}");
        assert_eq!(done(&r, "repos:scan", args).await, err("잘못된 입력입니다"));
    }
    assert!(calls.quiet());
}

// ── 입력 검증(확인 창·연결·실행기 없이 거부) ────────────────────────────────────────

#[tokio::test]
async fn 잘못된_종류와_경로와_그룹은_확인_창_목록_실행기_없이_거부한다() {
    let calls = Arc::new(Calls::default());
    let r = router(Some(host(&calls, &["Ubuntu"])));
    let bad_kind = err("잘못된 입력입니다");
    for input in [json!(null), json!(5), json!("local"), json!([]), json!({}), json!({ "kind": "github", "path": "C:\\x" }), json!({ "kind": "x" }), json!({ "kind": 5 }), json!({ "kind": null })] {
        assert_eq!(done(&r, "env:test", json!([input.clone()])).await, bad_kind, "env:test {input}");
        assert_eq!(done(&r, "repos:scan", json!([input.clone()])).await, bad_kind, "repos:scan {input}");
    }
    assert_eq!(done(&r, "env:test", json!([])).await, bad_kind);
    assert_eq!(done(&r, "repos:scan", json!([])).await, bad_kind);
    let bad_path = err("잘못된 경로입니다");
    let long = "a".repeat(4097);
    // 4097 UTF-16 단위(이모지는 2단위라 2049 개)
    let long_emoji = "😀".repeat(2049);
    for kind in ["local", "wsl"] {
        for p in [json!(null), json!(5), json!(""), json!("   "), json!("\u{a0}\u{feff}\n"), json!("a\u{0}b"), json!(long.clone()), json!(long_emoji.clone())] {
            assert_eq!(done(&r, "repos:scan", json!([{ "kind": kind, "path": p.clone(), "distro": "Ubuntu" }])).await, bad_path, "{kind} {p}");
        }
    }
    assert_eq!(done(&r, "repos:scan", json!([{ "kind": "local" }])).await, bad_path, "path 없음");
    // 네트워크 경로: NT 네임스페이스는 어디에 있어도(모든 종류), UNC 는 local·wsl 만. 공백으로 시작하는 UNC 도 거른다
    let net = err(NET_PATH);
    for kind in ["local", "wsl"] {
        for p in ["\\\\host\\share", "//host/share", " \\\\host\\share", "\\\\?\\UNC\\host\\share", "\\??\\UNC\\host\\share", "C:\\a\\??\\b", "/??/UNC/host/share"] {
            assert_eq!(done(&r, "repos:scan", json!([{ "kind": kind, "path": p, "distro": "Ubuntu" }])).await, net, "{kind} {p}");
        }
    }
    // 그룹: 문자열이고 200 UTF-16 단위 이하(경로 검사 뒤)
    let bad_group = err("잘못된 그룹 이름입니다");
    for g in [json!(5), json!(null), json!(["a"]), json!("g".repeat(201)), json!("😀".repeat(101))] {
        assert_eq!(done(&r, "repos:scan", json!([{ "kind": "local", "path": "C:\\x", "group": g.clone() }])).await, bad_group, "{g}");
    }
    assert!(calls.quiet(), "거부된 입력이 포트를 불렀다: 목록 {} 확인 {:?} 실행기 {:?}", calls.distros.load(Ordering::SeqCst), calls.confirms.lock().unwrap(), calls.execs.lock().unwrap());
}

#[tokio::test]
async fn wsl_은_설치된_배포판만_통과하고_없으면_실행기를_만들지_않는다() {
    let calls = Arc::new(Calls::default());
    let r = router(Some(host(&calls, &["Ubuntu", "Debian"])));
    let missing = |n: &str| err(&format!("설치된 WSL 배포판이 아닙니다: {n}"));
    assert_eq!(done(&r, "env:test", json!([{ "kind": "wsl", "distro": "Arch" }])).await, missing("Arch"));
    assert_eq!(done(&r, "env:test", json!([{ "kind": "wsl" }])).await, missing("(없음)"));
    assert_eq!(done(&r, "env:test", json!([{ "kind": "wsl", "distro": 5 }])).await, missing("(없음)"));
    // UNC 로 고른 경우 경로 안의 배포판을 본다(local 로 보내도 같다 — 설치 확인을 거친다)
    for kind in ["local", "wsl"] {
        assert_eq!(done(&r, "repos:scan", json!([{ "kind": kind, "path": "\\\\wsl.localhost\\Arch\\home\\a", "distro": "Ubuntu" }])).await, missing("Arch"), "{kind}");
    }
    // 형식이 틀린 이름은 이름을 되돌려 싣지 않고 거부한다
    let not_installed = err("설치된 WSL 배포판이 아닙니다");
    for d in ["-x", "", "a\u{202E}b", "a\\b", "a/b", "a..b", "a\nb"] {
        assert_eq!(done(&r, "env:test", json!([{ "kind": "wsl", "distro": d }])).await, not_installed, "{d:?}");
    }
    assert!(calls.execs.lock().unwrap().is_empty(), "설치되지 않은 배포판으로 실행기를 만들었다");
    // 설치된 이름은 대소문자를 맞춰 설치된 표기로 실행기를 만든다(실행은 가짜라 로컬 git)
    let ok = done(&r, "env:test", json!([{ "kind": "wsl", "distro": "ubuntu" }])).await;
    assert_eq!(ok["ok"], true, "{ok}");
    assert!(ok["data"].as_str().unwrap().starts_with("git version"));
    assert_eq!(*calls.execs.lock().unwrap(), [("wsl".to_string(), Some("Ubuntu".to_string()))]);
    // 설치된 배포판이 하나도 없으면(WSL 이 없는 PC) 어떤 이름도 통과하지 못하고 실행기도 없다 — 운영 포트는 레지스트리 관문 뒤에서만 `wsl.exe` 를 부른다
    let calls2 = Arc::new(Calls::default());
    let none = router(Some(host(&calls2, &[])));
    assert_eq!(done(&none, "env:test", json!([{ "kind": "wsl", "distro": "Ubuntu" }])).await, missing("Ubuntu"));
    assert_eq!(done(&none, "env:wslDistros", json!([])).await, json!({ "ok": true, "data": [] }));
    assert!(calls2.execs.lock().unwrap().is_empty() && calls2.confirms.lock().unwrap().is_empty());
}

// ── env:sshHosts·env:wslDistros ──────────────────────────────────────────────

#[tokio::test]
async fn ssh_별칭은_config_의_구체_별칭만_ts_모양으로_돌려준다() {
    let home = TempDir::new("hosts");
    let ssh = home.path().join(".ssh");
    fs::create_dir_all(&ssh).unwrap();
    fs::write(ssh.join("config"), "Host a\n  HostName h1\n  User u1\n  Port 2200\n  ProxyJump b\nHost b\n  HostName h2\nHost *.wild !skip\n  User w\n").unwrap();
    let home_str = home.path().to_string_lossy().into_owned();
    let calls = Arc::new(Calls::default());
    let r = router(Some(host_with(&calls, &[], Arc::new(move || ssh_hosts_for_home(&home_str)), &Arc::new(AtomicBool::new(true)))));
    assert_eq!(
        done(&r, "env:sshHosts", json!([])).await,
        json!({ "ok": true, "data": [
            { "alias": "a", "hostName": "h1", "user": "u1", "port": 2200, "proxyJump": "b" },
            { "alias": "b", "hostName": "h2", "user": local_user_name(), "port": 22 }
        ] })
    );
    // 파일이 없으면 빈 배열
    assert!(ssh_hosts_for_home(&home.path().join("none").to_string_lossy()).is_empty());
    let _: Vec<SshHostInfo> = ssh_hosts_for_home(&home.path().to_string_lossy());
}

// ── repos:scan·env:test (local) ──────────────────────────────────────────────

fn repo_names(v: &Value) -> Vec<String> {
    v["data"]["repos"].as_array().unwrap().iter().map(|r| r["name"].as_str().unwrap().to_string()).collect()
}

/// parent/{b(저장소), a(저장소), plain, .hidden(저장소), file.txt}
fn make_parent() -> TempDir {
    let parent = TempDir::new("parent");
    for name in ["b", "a", ".hidden"] {
        fs::create_dir(parent.path().join(name)).unwrap();
        git(&parent.path().join(name), &["init", "-q"]);
    }
    fs::create_dir(parent.path().join("plain")).unwrap();
    fs::write(parent.path().join("file.txt"), "x").unwrap();
    parent
}

#[tokio::test]
async fn 로컬_찾기는_하위_저장소를_이름순으로_찾고_숨김과_일반_폴더는_뺀다() {
    let calls = Arc::new(Calls::default());
    let r = router(Some(host(&calls, &[])));
    let parent = make_parent();
    let root = parent.path().to_string_lossy().into_owned();
    let v = done(&r, "repos:scan", json!([{ "kind": "local", "path": root }])).await;
    assert_eq!(
        v,
        json!({ "ok": true, "data": {
            "path": root, "isRepo": false, "truncated": false,
            "repos": [
                { "name": "a", "path": win32_path::join(&[&root, "a"]) },
                { "name": "b", "path": win32_path::join(&[&root, "b"]) }
            ]
        } })
    );
    // 앞뒤 공백은 JS trim 으로 지운 값이 결과 경로다
    let padded = done(&r, "repos:scan", json!([{ "kind": "local", "path": format!("  {root}\n"), "group": "묶음" }])).await;
    assert_eq!(padded["data"]["path"], json!(root));
    assert_eq!(repo_names(&padded), ["a", "b"]);
    // 없는 폴더는 오류 봉투다
    let missing = done(&r, "repos:scan", json!([{ "kind": "local", "path": win32_path::join(&[&root, "nope"]) }])).await;
    assert_eq!(missing["ok"], false);
    assert_eq!(missing["code"], "ERROR");
    assert!(calls.confirms.lock().unwrap().is_empty() && calls.distros.load(Ordering::SeqCst) == 0);
}

#[tokio::test]
async fn 고른_폴더가_저장소이거나_저장소_안의_하위_폴더면_is_repo_로_알리고_하위는_보지_않는다() {
    let calls = Arc::new(Calls::default());
    let r = router(Some(host(&calls, &[])));
    let repo = make_repo(0);
    let root = repo.path().to_string_lossy().into_owned();
    let v = done(&r, "repos:scan", json!([{ "kind": "local", "path": root }])).await;
    assert_eq!(v, json!({ "ok": true, "data": { "path": root, "isRepo": true, "repos": [], "truncated": false } }));
    // 저장소 안의 하위 폴더(그 안에 중첩 저장소가 있어도) 도 저장소로 본다
    let sub = repo.path().join("libs");
    fs::create_dir_all(sub.join("nested")).unwrap();
    git(&sub.join("nested"), &["init", "-q"]);
    let sub_str = sub.to_string_lossy().into_owned();
    let v = done(&r, "repos:scan", json!([{ "kind": "local", "path": sub_str }])).await;
    assert_eq!(v["data"], json!({ "path": sub_str, "isRepo": true, "repos": [], "truncated": false }));
}

/// 정션을 만들고 Drop 에서 정션부터 `remove_dir`(rmdir)로 지운다 — 정션을 따라 원본을 지우지 않는다
#[cfg(windows)]
struct Junctions(Vec<std::path::PathBuf>);

#[cfg(windows)]
impl Junctions {
    fn make(&mut self, link: &Path, target: &Path) {
        // git 이 아니라 cmd 의 mklink 를 부른다(정션은 std 로 만들 수 없다). 시험 전용이라 clippy 의 spawn 금지를 푼다
        #[allow(clippy::disallowed_methods)]
        let out = std::process::Command::new("cmd").arg("/c").arg("mklink").arg("/J").arg(link).arg(target).output().expect("cmd 실행");
        assert!(out.status.success(), "mklink /J 실패: {} {}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        self.0.push(link.to_path_buf());
    }
}

#[cfg(windows)]
impl Drop for Junctions {
    fn drop(&mut self) {
        for j in self.0.iter().rev() {
            let _ = fs::remove_dir(j);
        }
    }
}

#[cfg(windows)]
#[tokio::test]
async fn 로컬_찾기는_정션_사슬을_따라가_저장소를_찾는다() {
    let calls = Arc::new(Calls::default());
    let r = router(Some(host(&calls, &[])));
    let outside = TempDir::new("outside");
    let real = outside.path().join("real");
    let plain = outside.path().join("plain");
    fs::create_dir_all(&real).unwrap();
    fs::create_dir_all(&plain).unwrap();
    git(&real, &["init", "-q"]);
    let parent = TempDir::new("jparent");
    let mut j = Junctions(Vec::new());
    // link2 -> link1 -> real(저장소) 사슬, 저장소가 아닌 폴더를 가리키는 정션, 파일 옆의 일반 폴더
    j.make(&parent.path().join("link1"), &real);
    j.make(&parent.path().join("link2"), &parent.path().join("link1"));
    j.make(&parent.path().join("noRepo"), &plain);
    fs::create_dir(parent.path().join("own")).unwrap();
    git(&parent.path().join("own"), &["init", "-q"]);
    let root = parent.path().to_string_lossy().into_owned();
    let v = done(&r, "repos:scan", json!([{ "kind": "local", "path": root }])).await;
    assert_eq!(v["ok"], true, "{v}");
    assert_eq!(repo_names(&v), ["link1", "link2", "own"], "{v}");
    assert_eq!(v["data"]["repos"][0]["path"], json!(win32_path::join(&[&root, "link1"])));
}

/// 고른 폴더(루트)가 링크 사슬(상한 16 초과)로 이어지면 실행기를 만들기도 전에 거부한다 — 원격 링크는 만들 수 없으므로 상한 초과로 루트 가드의 연결을 확인한다
/// (원격 판정 자체의 구성 요소 단위 시험은 tests/fsguard.rs)
#[cfg(windows)]
#[tokio::test]
async fn 로컬_찾기의_루트가_링크_사슬_상한을_넘으면_git_을_돌리기_전에_거부한다() {
    let calls = Arc::new(Calls::default());
    let r = router(Some(host(&calls, &[])));
    let base = TempDir::new("rootchain");
    let real = base.path().join("real");
    fs::create_dir_all(real.join("repo")).unwrap();
    git(&real.join("repo"), &["init", "-q"]);
    let mut j = Junctions(Vec::new());
    let mut prev = real.clone();
    let mut links = Vec::new();
    for i in 0..17 {
        let link = base.path().join(format!("c{i}"));
        j.make(&link, &prev);
        links.push(link.clone());
        prev = link;
    }
    // 홉 3 은 통과해 저장소를 찾는다(대조)
    let ok = done(&r, "repos:scan", json!([{ "kind": "local", "path": links[2].to_string_lossy() }])).await;
    assert_eq!(ok["ok"], true, "{ok}");
    assert_eq!(repo_names(&ok), ["repo"]);
    let after_ok = calls.execs.lock().unwrap().len();
    // 홉 17 은 거부 — 실행기도 만들지 않는다
    let v = done(&r, "repos:scan", json!([{ "kind": "local", "path": links[16].to_string_lossy() }])).await;
    assert_eq!(v, err(antler_lib::fsguard::REMOTE_LINK_ROOT));
    assert_eq!(calls.execs.lock().unwrap().len(), after_ok, "거부된 루트에서 git 실행기를 만들었다");
}

#[tokio::test]
async fn 로컬_연결_테스트는_git_버전을_돌려준다() {
    let calls = Arc::new(Calls::default());
    let r = router(Some(host(&calls, &[])));
    let v = done(&r, "env:test", json!([{ "kind": "local" }])).await;
    assert_eq!(v["ok"], true);
    let text = v["data"].as_str().unwrap();
    assert!(text.starts_with("git version "), "{text}");
    assert_eq!(text, text.trim());
    assert_eq!(*calls.execs.lock().unwrap(), [("local".to_string(), None)]);
}

#[tokio::test]
async fn wsl_공유_경로로_고른_local_은_설치_확인을_거치고_통과하기_전에_실행하지_않는다() {
    // 설치 목록이 비면 UNC(wsl.localhost) 를 읽지도 git 을 돌리지도 않는다
    let calls = Arc::new(Calls::default());
    let r = router(Some(host(&calls, &[])));
    let v = done(&r, "repos:scan", json!([{ "kind": "local", "path": "\\\\wsl.localhost\\Ubuntu\\home\\me" }])).await;
    assert_eq!(v, err("설치된 WSL 배포판이 아닙니다: Ubuntu"));
    assert_eq!(calls.distros.load(Ordering::SeqCst), 1);
    assert!(calls.execs.lock().unwrap().is_empty());
}

// ── ssh(루프백 서버) ─────────────────────────────────────────────────────────

struct Rig {
    server: TestServer,
    home: Arc<Home>,
    pool: Arc<SshPool>,
    repo: TempDir,
    calls: Arc<Calls>,
    allow: Arc<AtomicBool>,
    router: Router,
}

enum Cfg {
    /// `Host test` 별칭이 있다 — 확인 창 없이 연결한다
    Alias,
    /// 구체 별칭이 없다(`Host *` 만) — `user@127.0.0.1:포트` 직접 입력은 설정에 없는 호스트라 확인 창을 거친다
    Wildcard,
}

impl Rig {
    async fn new(cfg: Cfg, sftp: Sftp, known: bool) -> Rig {
        let client = new_key();
        let mut opts = Opts::new(vec![client.public_key().clone()]);
        opts.sftp = sftp;
        let server = TestServer::start(opts).await;
        let home = Arc::new(Home::new(&client));
        let key = home.key_for_config();
        home.write_config(&match cfg {
            Cfg::Alias => format!("Host test\n  HostName 127.0.0.1\n  Port {}\n  User tester\n  IdentityFile {key}\n  IdentitiesOnly yes\n", server.port),
            Cfg::Wildcard => format!("Host *\n  IdentityFile {key}\n  IdentitiesOnly yes\n"),
        });
        if known {
            home.write_known_hosts(&known_hosts_line(server.port, &server.host_key));
        }
        let h = home.clone();
        let pool = SshPool::new(move |host| h.executor(host));
        let calls = Arc::new(Calls::default());
        let allow = Arc::new(AtomicBool::new(true));
        let hh = home.clone();
        let env_host = host_with(&calls, &[], Arc::new(move || ssh_hosts_for_home(&hh.home_str())), &allow);
        let router = Router::new(false, RepoMirror::default());
        router.set_ssh_pool(pool.clone());
        router.set_env(env_host);
        Rig { server, home, pool, repo: make_repo(0), calls, allow, router }
    }

    /// 설정에 없는 직접 입력 호스트
    fn direct(&self) -> String {
        format!("tester@127.0.0.1:{}", self.server.port)
    }

    fn confirms(&self) -> usize {
        self.calls.confirms.lock().unwrap().len()
    }
}

fn work_tree() -> Arc<FakeTree> {
    let mut t = FakeTree::new("/home/tester");
    t.dir("/home/tester/work/app/.git").dir("/home/tester/work/lib").dir("/home/tester/.hidden").file("/home/tester/notes.txt");
    Arc::new(t)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn 잘못된_ssh_호스트는_확인_창도_연결도_없이_거부한다() {
    let rig = Rig::new(Cfg::Wildcard, Sftp::Off, true).await;
    assert!(rig.router.ssh_active());
    let long = "h".repeat(256);
    let bad = bad_hosts(&long);
    for h in &bad {
        let want = err("잘못된 SSH 호스트입니다");
        let mut input = json!({ "kind": "ssh" });
        if let Some(h) = h {
            input["sshHost"] = h.clone();
        }
        assert_eq!(done(&rig.router, "env:test", json!([input.clone()])).await, want, "env:test {h:?}");
        input["path"] = json!("/x");
        assert_eq!(done(&rig.router, "repos:scan", json!([input])).await, want, "repos:scan {h:?}");
        let mut list = json!({ "path": "" });
        if let Some(h) = h {
            list["sshHost"] = h.clone();
        }
        assert_eq!(done(&rig.router, "env:listDir", json!([list])).await, want, "env:listDir {h:?}");
    }
    // 경로 검사가 호스트 검사보다 먼저다(env:listDir): 둘 다 틀리면 경로 오류
    assert_eq!(done(&rig.router, "env:listDir", json!([{ "sshHost": "a b", "path": "a\u{0}" }])).await, err("잘못된 경로입니다"));
    assert_eq!(done(&rig.router, "env:listDir", json!([{ "sshHost": rig.direct(), "path": "p".repeat(4097) }])).await, err("잘못된 경로입니다"));
    // 입력이 객체가 아니어도 호스트 오류다
    assert_eq!(done(&rig.router, "env:listDir", json!(["x"])).await, err("잘못된 SSH 호스트입니다"));
    assert_eq!(rig.confirms(), 0);
    assert_eq!(rig.server.connections(), 0, "거부된 입력이 연결을 맺었다");
    assert!(rig.pool.hosts().is_empty());
}

fn bad_hosts(long: &str) -> Vec<Option<Value>> {
    let mut v: Vec<Option<Value>> = ["a b", "a\tb", "-oProxyCommand=x", "  -x", "a\0b", "a\u{7}b", long, "", "   ", "\u{a0}"].iter().map(|s| Some(json!(s))).collect();
    v.extend([None, Some(json!(5)), Some(json!(null)), Some(json!(["a"]))]);
    v
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn 설정의_별칭은_확인_창_없이_연결하고_저장_전_연결은_끝나면_닫힌다() {
    let rig = Rig::new(Cfg::Alias, Sftp::Off, true).await;
    let v = done(&rig.router, "env:test", json!([{ "kind": "ssh", "sshHost": " test " }])).await;
    assert_eq!(v["ok"], true, "{v}");
    assert!(v["data"].as_str().unwrap().starts_with("git version "));
    assert_eq!(rig.confirms(), 0, "설정에 있는 별칭에 확인 창이 떴다");
    assert_eq!(rig.server.connections(), 1);
    // 미러에 없는 호스트라 마지막 잡기가 풀리면 풀에서 빠지고 연결이 닫힌다
    wait_for("저장 전 호스트 정리", || rig.pool.hosts().is_empty()).await;
    wait_for("연결 닫힘", || rig.server.open_connections() == 0).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn 설정에_없는_호스트는_한_번만_묻고_승인을_기억하며_취소하면_연결하지_않는다() {
    let rig = Rig::new(Cfg::Wildcard, Sftp::Tree(work_tree()), true).await;
    let host = rig.direct();
    // 취소: 오류 봉투, 연결 없음, 기억하지 않는다(다시 묻는다)
    rig.allow.store(false, Ordering::SeqCst);
    for n in 1..=2 {
        assert_eq!(done(&rig.router, "env:test", json!([{ "kind": "ssh", "sshHost": host }])).await, err("연결을 취소했습니다"));
        assert_eq!(rig.confirms(), n);
    }
    assert_eq!(rig.server.connections(), 0);
    // 확인 창 인자: 네이티브 대화상자 모양·취소가 기본·호스트 이름이 본문에 든다
    let arg = rig.calls.confirms.lock().unwrap()[0].clone();
    assert_eq!((arg["kind"].as_str(), arg["title"].as_str(), arg["cancelId"].as_i64()), (Some("info"), Some("SSH 연결 확인"), Some(1)));
    assert_eq!(arg["buttons"], json!(["연결", "취소"]));
    assert!(arg["message"].as_str().unwrap().contains(&format!("{host} 에 연결할까요?")));
    // 승인: 채널이 달라도(env:test·env:listDir·repos:scan) 같은 호스트는 다시 묻지 않는다
    rig.allow.store(true, Ordering::SeqCst);
    let v = done(&rig.router, "env:test", json!([{ "kind": "ssh", "sshHost": format!("  {host}  ") }])).await;
    assert_eq!(v["ok"], true, "{v}");
    assert_eq!(rig.confirms(), 3);
    let l = done(&rig.router, "env:listDir", json!([{ "sshHost": host, "path": "~/work" }])).await;
    assert_eq!(l["ok"], true, "{l}");
    let s = done(&rig.router, "repos:scan", json!([{ "kind": "ssh", "sshHost": host, "path": "/home/tester/work" }])).await;
    assert_eq!(s["ok"], true, "{s}");
    assert_eq!(rig.confirms(), 3, "승인한 호스트를 다시 물었다");
    // 다른 호스트는 따로 묻는다
    let other = format!("other@127.0.0.1:{}", rig.server.port);
    let _ = done(&rig.router, "env:test", json!([{ "kind": "ssh", "sshHost": other }])).await;
    assert_eq!(rig.confirms(), 4);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn 모르는_호스트_키는_거부하고_지문과_ssh_안내를_주며_known_hosts_에_쓰지_않는다() {
    let rig = Rig::new(Cfg::Wildcard, Sftp::Off, false).await;
    let host = rig.direct();
    assert!(!rig.home.known_hosts.exists());
    for (ch, args) in [
        ("env:test", json!([{ "kind": "ssh", "sshHost": host }])),
        ("env:listDir", json!([{ "sshHost": host, "path": "" }])),
        ("repos:scan", json!([{ "kind": "ssh", "sshHost": host, "path": "/home/tester/x" }])),
    ] {
        let v = done(&rig.router, ch, args).await;
        assert_eq!((v["ok"].clone(), v["code"].clone()), (json!(false), json!("ERROR")), "{ch} {v}");
        let msg = v["error"].as_str().unwrap();
        assert!(msg.contains("알 수 없는 호스트입니다"), "{ch}: {msg}");
        assert!(msg.contains("SHA256:"), "{ch}: 지문이 없다 {msg}");
        assert!(msg.contains(&format!("확인 명령: ssh -p {} tester@127.0.0.1", rig.server.port)), "{ch}: {msg}");
    }
    assert!(!rig.home.known_hosts.exists(), "known_hosts 에 썼다(TOFU)");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn 원격_폴더_목록은_sftp_로_읽고_경로가_문자열이_아니면_홈을_연다() {
    let rig = Rig::new(Cfg::Alias, Sftp::Tree(work_tree()), true).await;
    let v = done(&rig.router, "env:listDir", json!([{ "sshHost": "test", "path": "~/work" }])).await;
    assert_eq!(
        v,
        json!({ "ok": true, "data": {
            "path": "/home/tester/work", "parent": "/home/tester", "isRepo": false, "truncated": false,
            "dirs": [{ "name": "app", "isRepo": true }, { "name": "lib", "isRepo": false }]
        } })
    );
    let home = done(&rig.router, "env:listDir", json!([{ "sshHost": "test", "path": 5 }])).await;
    assert_eq!(home["data"]["path"], "/home/tester");
    assert_eq!(home["data"]["dirs"], json!([{ "name": "work", "isRepo": false }]));
    // 같은 호스트의 연속 요청은 풀의 연결 하나를 나눠 쓴다(쓰는 동안 잡혀 있다)
    assert!(rig.server.connections() <= 2);
    assert_eq!(rig.confirms(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sftp_를_막은_서버는_목록은_오류이고_찾기는_경로를_그대로_받는다() {
    let rig = Rig::new(Cfg::Alias, Sftp::Off, true).await;
    let l = done(&rig.router, "env:listDir", json!([{ "sshHost": "test", "path": "" }])).await;
    assert_eq!(l["ok"], false);
    assert!(l["error"].as_str().unwrap().starts_with("이 서버는 SFTP 를 쓸 수 없어 폴더를 둘러볼 수 없습니다. 경로를 직접 입력하세요"), "{l}");
    let s = done(&rig.router, "repos:scan", json!([{ "kind": "ssh", "sshHost": "test", "path": "  /home/tester/work  " }])).await;
    assert_eq!(s, json!({ "ok": true, "data": { "path": "/home/tester/work", "isRepo": false, "repos": [], "truncated": false } }));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ssh_찾기는_저장소_안이면_is_repo_이고_아니면_원격_목록의_저장소를_절대_경로로_돌려준다() {
    let rig = Rig::new(Cfg::Alias, Sftp::Tree(work_tree()), true).await;
    // 원격 git 이 저장소로 보는 경로(서버는 이 PC 의 git 을 돌린다)
    let inside = rig.repo.path().to_string_lossy().into_owned();
    let v = done(&rig.router, "repos:scan", json!([{ "kind": "ssh", "sshHost": "test", "path": inside }])).await;
    assert_eq!(v, json!({ "ok": true, "data": { "path": inside, "isRepo": true, "repos": [], "truncated": false } }));
    // 저장소가 아닌 경로는 SFTP 목록에서 `.git` 이 있는 하위 폴더를 찾는다(로컬에 없는 절대 경로라 git 은 저장소 아님)
    let v = done(&rig.router, "repos:scan", json!([{ "kind": "ssh", "sshHost": "test", "path": "/home/tester/work" }])).await;
    assert_eq!(
        v,
        json!({ "ok": true, "data": {
            "path": "/home/tester/work", "isRepo": false, "truncated": false,
            "repos": [{ "name": "app", "path": "/home/tester/work/app" }]
        } })
    );
    assert_eq!(rig.confirms(), 0);
}

// ── 소스 검사 ──────────────────────────────────────────────────────────────────

fn prod_code(rel: &str) -> String {
    let text = read_src(rel);
    text.split("#[cfg(test)]").next().unwrap().lines().filter(|l| !l.trim_start().starts_with("//")).collect::<Vec<_>>().join("\n")
}

#[test]
fn 환경_모듈은_프로세스를_띄우지_않고_known_hosts_에_쓰지_않고_해석은_설치_확인_함수_하나다() {
    let code = prod_code("src/env_host.rs");
    for banned in ["Command", "std::process", "tokio::process", "learn_known_hosts", "known_hosts::", "request_subsystem", "crate::external", "AppHandle", "Sidecar", "tauri_plugin"] {
        assert!(!code.contains(banned), "env_host.rs 가 {banned} 를 쓴다");
    }
    // 저장·실행으로 이어지는 해석은 설치 확인이 붙은 함수만 부른다(`resolve_env(` 직접 호출 금지)
    assert!(!code.contains("resolve_env("), "resolve_env 를 직접 부른다");
    assert_eq!(code.matches("resolve_env_checked_with(").count(), 1);
    // 파일 시스템 접근은 로컬 찾기(scan_local_repos)와 `~/.ssh/config` 읽기(ssh_hosts_for_home) 뿐이다
    assert_eq!(code.matches("scan_local_repos(").count(), 1);
    assert_eq!(code.matches("load_ssh_config(").count(), 1);
    // 고른 폴더(로컬)의 링크 확인은 실행기를 만들고 `git -C`(probe)를 돌리기 전이다. external 은 쓰지 않고 공용 `fsguard` 를 쓴다
    assert_eq!(code.matches("fsguard::check_local_root(").count(), 1);
    assert!(code.find("fsguard::check_local_root(").unwrap() < code.find("(host.exec_for)(env.kind").unwrap());
    // 확인 창은 한 곳에서만 부르고 승인은 응답이 참일 때만 기억한다
    assert_eq!(code.matches("confirm(&arg)").count(), 1);
    assert!(code.find("confirm(&arg)").unwrap() < code.find(".insert(host.to_string())").unwrap());
}

#[test]
fn 앱은_운영_포트를_주입하고_wsl_목록은_레지스트리_관문의_함수다() {
    let lib = prod_code("src/lib.rs");
    assert_eq!(lib.matches("router.set_env(").count(), 1);
    // 운영 WSL 목록 포트는 `list_distros`(레지스트리 관문 뒤에서만 wsl.exe)이고 `wsl.exe` 를 직접 부르지 않는다
    assert!(lib.contains("git::exec::list_distros()"));
    assert!(!lib.contains("wsl.exe"));
    // 확인 창은 취소가 기본 버튼인 `dialogs::message_box`(응답 0 = 연결)다
    assert!(lib.contains("dialogs::message_box(&app, arg)") && lib.contains("== Some(0)"));
    // 풀 주입 뒤·라우터 등록 전이다
    let pool = lib.find("router.set_ssh_pool(").unwrap();
    let env = lib.find("router.set_env(").unwrap();
    assert!(pool < env && env < lib.find("app.manage(router)").unwrap());
    let router = read_src("src/router.rs");
    assert!(router.contains("pub fn set_env(&self, host: EnvHost)"));
    // 라우터의 환경 가로채기는 git 채널 처리(`RUST_CHANNELS`)보다 앞이다
    assert!(router.find("ENV_CHANNELS.contains(&channel)").unwrap() < router.find("if !RUST_CHANNELS.contains(&channel)").unwrap());
}
