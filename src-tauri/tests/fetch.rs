//! fetch 실행(`antler_lib::git::fetch::run`) 통합 테스트 — 실제 git 을 띄운다. TS `tests/fetch.test.ts` 의 git 실행 시나리오를 이름 단위로 옮겼다.
//! 어느 TS 테스트가 어느 Rust 테스트가 됐는지(또는 왜 옮기지 않았는지)는 `tests/golden/fetch/test-map.json` 이고, vitest(`tests/goldenFetch.test.ts`)가
//! 원본의 모든 `it` 이 표에 있는지, `대응표가_가리키는_rust_테스트가_있다` 가 표가 가리키는 Rust 테스트가 있는지 강제한다.
//! 옮기지 않은 것은 FetchService 의 상태 기계(진행 합치기·확인→재검사·승인 저장·백오프)로 PR C 의 몫이다 — 여기서는 검사(`inspect_config`)와 실행(`run_fetch`)만 본다.
//!
//! 격리: 이 바이너리가 시작될 때(`isolate`) `GIT_CONFIG_GLOBAL`(커밋된 `tests/gitconfig`)·`GIT_CONFIG_NOSYSTEM` 을 넣고, 프록시·askpass·ssh 환경변수를 비운다
//! (127.0.0.1 요청이 프록시로 나가거나 askpass 대조가 흐려지지 않게). **전역 git 설정 파일을 고치지 않는다**(커밋된 파일이고 테스트가 병렬이다) —
//! 전역 값이 필요한 시나리오(전역 core.sshCommand)는 가짜 실행기의 `--global` 응답으로 준다. WSL 의 git 은 이 격리를 받지 않고 배포판의 `~/.gitconfig` 를 읽으므로
//! `/tmp` 저장소와 `credential_helpers=false` 로만 돌린다. 네트워크는 127.0.0.1 만 쓴다.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use antler_lib::git::exec::{ExecError, FetchEnv, GitExecutor, LocalExecutor, RunOpts, RunResult, WslExecutor};
use antler_lib::git::fetch::guard::{
    classify_failure, hooks_path_for, hooks_path_with, inspect_entries, version_at_least, ConfigInspection, FetchFailure, FILE_TRANSPORT_MIN_GIT, SSH_COMMAND_DEFAULT,
};
use antler_lib::git::fetch::run::{
    fetch_args, fetch_config_for, fetch_env_for, inspect_config, process_ssh_env, run_fetch, run_fetch_with, ssh_env, FetchRunOpts, GitRunner, GitVersions,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// ctor 가 넣는 사용자 환경의 ssh 명령(대소문자를 일부러 섞어 Windows 의 대소문자 무시 상속을 본다 — 위험 11)
const PARENT_SSH: &str = "ssh -o AntlerParentProbe=1";

/// 테스트 프로세스 환경 격리. ctor 로 main 전에 한 번 돌아 테스트 스레드와 경합하지 않는다(set_var 는 다른 스레드가 환경을 읽는 중이면 위험하다).
// unsafe: main 이전 코드는 Rust 런타임 초기화 전에 돈다 — 여기서는 환경 변수만 넣고 다른 것에 기대지 않는다
#[ctor::ctor(unsafe)]
fn isolate() {
    let global = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("gitconfig");
    std::env::set_var("GIT_CONFIG_GLOBAL", global);
    std::env::set_var("GIT_CONFIG_NOSYSTEM", "1");
    std::env::set_var("GCM_INTERACTIVE", "never");
    for k in [
        "HTTP_PROXY", "http_proxy", "HTTPS_PROXY", "https_proxy", "ALL_PROXY", "all_proxy", "GIT_ASKPASS", "SSH_ASKPASS", "GIT_SSH", "GIT_SSH_COMMAND", "GIT_ALLOW_PROTOCOL",
        "GIT_TERMINAL_PROMPT",
    ] {
        std::env::remove_var(k);
    }
    std::env::set_var("NO_PROXY", "127.0.0.1,localhost");
    // 위험 11: 사용자 환경에 소문자·혼합 대소문자 이름이 있어도 Windows 에서는 같은 변수다(Node 의 process.env 도 그렇다). 다른 곳은 정확한 이름만
    std::env::set_var(if cfg!(windows) { "Git_Ssh_Command" } else { "GIT_SSH_COMMAND" }, PARENT_SSH);
    // 우리가 실행기에 주는 값(GIT_PAGER=cat)을 부모의 같은 이름(대소문자만 다름)이 이기면 안 된다
    std::env::set_var(if cfg!(windows) { "Git_Pager" } else { "ANTLER_PAGER_PROBE" }, "antler-evil-pager");
    // 대소문자 구분 여부 확인용(위험 11). Windows 에서만 `ANTLER_CASE_PROBE` 로도 보인다
    std::env::set_var("Antler_Case_Probe", "1");
}

// ---------------------------------------------------------------------------------------------------------------------
// 도구
// ---------------------------------------------------------------------------------------------------------------------

/// 테스트 준비·대조용 날 프로세스. **실행기를 거치지 않는 것**이 목적이다 — 방어 없이 돌려 보는 대조군, 저장소 만들기.
/// 제품 코드는 이 길을 쓰지 않는다(clippy.toml 의 disallowed-methods).
#[allow(clippy::disallowed_methods)]
fn raw(program: &str) -> std::process::Command {
    std::process::Command::new(program)
}

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        static N: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!("antler-fetch-{tag}-{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
        fs::create_dir_all(&dir).unwrap();
        TempDir(dir)
    }
    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        // 읽기 전용 .git 파일 때문에 실패할 수 있다 — 임시 폴더라 남아도 무해하다
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn posix(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

fn file_url(p: &Path) -> String {
    let s = posix(p);
    if s.starts_with('/') {
        format!("file://{s}")
    } else {
        format!("file:///{s}")
    }
}

/// 설정을 직접 읽는 날 git(저장소 준비·검증용). 전역 설정은 격리돼 있다
fn raw_git(dir: &Path, args: &[&str]) -> String {
    let out = raw("git").arg("-C").arg(dir).args(["-c", "user.name=t", "-c", "user.email=t@t"]).args(args).output().expect("git 실행");
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn local() -> GitExecutor {
    GitExecutor::Local(LocalExecutor::default())
}

/// bare remote(file://) + 시드 작업 복제본 + 테스트 대상 복제본
struct Case {
    t: TempDir,
    work: PathBuf,
    repo: PathBuf,
    marker: PathBuf,
}

impl Case {
    fn new() -> Self {
        let t = TempDir::new("case");
        let dir = t.path().to_path_buf();
        let (bare, work, repo) = (dir.join("remote.git"), dir.join("work"), dir.join("repo"));
        assert!(raw("git").args(["init", "-q", "--bare", "-b", "main"]).arg(&bare).status().unwrap().success());
        assert!(raw("git").args(["clone", "-q", &file_url(&bare)]).arg(&work).output().unwrap().status.success());
        raw_git(&work, &["checkout", "-q", "-b", "main"]);
        raw_git(&work, &["commit", "-q", "--allow-empty", "-m", "first"]);
        raw_git(&work, &["push", "-q", "origin", "main"]);
        assert!(raw("git").args(["clone", "-q", &file_url(&bare)]).arg(&repo).output().unwrap().status.success());
        let marker = dir.join("PWNED");
        Case { t, work, repo, marker }
    }

    fn push_commit(&self, msg: &str) -> String {
        raw_git(&self.work, &["commit", "-q", "--allow-empty", "-m", msg]);
        raw_git(&self.work, &["push", "-q", "origin", "main"]);
        raw_git(&self.work, &["rev-parse", "HEAD"])
    }

    fn repo_str(&self) -> String {
        self.repo.to_string_lossy().into_owned()
    }

    fn git(&self, args: &[&str]) -> String {
        raw_git(&self.repo, args)
    }

    /// 훅 차단용 앱 소유 빈 디렉터리(드라이브 문자 경로)
    fn no_hooks(&self) -> String {
        let d = self.t.path().join("no-hooks");
        fs::create_dir_all(&d).unwrap();
        d.to_string_lossy().into_owned()
    }

    /// 실행되면 마커 파일을 만드는 무해한 페이로드(git 이 셸로 실행하는 설정 값)
    fn payload(&self) -> String {
        format!("echo x > \"{}\"", posix(&self.marker))
    }

    fn marker_exists(&self) -> bool {
        self.marker.exists()
    }

    async fn inspect(&self) -> ConfigInspection {
        inspect_config(&local(), &self.repo_str(), None).await.expect("설정 검사")
    }

    async fn fetch(&self, allow_file: bool, credential_helpers: bool) -> Result<(), ExecError> {
        let ins = self.inspect().await;
        let nh = self.no_hooks();
        run_fetch(&local(), &self.repo_str(), &ins, &FetchRunOpts { cancel: None, no_hooks_dir: Some(&nh), allow_file, credential_helpers }).await
    }
}

fn fetch_env_default(allow_file: bool) -> FetchEnv {
    FetchEnv { allow_file, ssh_command: SSH_COMMAND_DEFAULT.to_string() }
}

fn fetch_opts(allow_file: bool) -> RunOpts {
    RunOpts { fetch_env: Some(fetch_env_default(allow_file)), ..RunOpts::default() }
}

fn dangerous_keys(ins: &ConfigInspection) -> Vec<String> {
    ins.dangerous.iter().map(|d| d.key.clone()).collect()
}

fn forbidden_keys(ins: &ConfigInspection) -> Vec<String> {
    ins.forbidden.iter().map(|f| f.key.clone()).collect()
}

/// 가짜 git: 인자를 기록하고, `f` 가 답하면 그 결과를 주고 아니면 실제 git 으로 넘긴다(옛 git·실패 흉내 / 인자 검사용).
/// 프로세스를 띄우는 길은 늘지 않는다 — 실제 실행은 [`GitExecutor`] 가 한다
struct Fake<F> {
    kind: &'static str,
    inner: GitExecutor,
    calls: Mutex<Vec<(Vec<String>, RunOpts)>>,
    f: F,
}

impl<F: Fn(&[String]) -> Option<RunResult> + Sync> Fake<F> {
    fn new(kind: &'static str, f: F) -> Self {
        Fake { kind, inner: local(), calls: Mutex::new(Vec::new()), f }
    }

    fn args(&self) -> Vec<Vec<String>> {
        self.calls.lock().unwrap().iter().map(|(a, _)| a.clone()).collect()
    }
}

impl<F: Fn(&[String]) -> Option<RunResult> + Sync> GitRunner for Fake<F> {
    fn kind(&self) -> &'static str {
        self.kind
    }

    fn run(&self, repo_path: &str, args: &[String], opts: &RunOpts) -> impl Future<Output = Result<RunResult, ExecError>> + Send {
        self.calls.lock().unwrap().push((args.to_vec(), opts.clone()));
        let hit = (self.f)(args);
        let inner = &self.inner;
        async move {
            match hit {
                Some(r) => Ok(r),
                None => inner.run(repo_path, args, opts).await,
            }
        }
    }
}

fn res(code: i32, stdout: &str, stderr: &str) -> RunResult {
    RunResult { stdout: stdout.as_bytes().to_vec(), stderr: stderr.to_string(), code }
}

fn has(args: &[String], a: &str) -> bool {
    args.iter().any(|x| x == a)
}

/// `--show-scope` 를 넣으면 옛 git 처럼 실패한다
fn old_git_answer(args: &[String]) -> Option<RunResult> {
    has(args, "--show-scope").then(|| res(129, "", "error: unknown option `show-scope'\n"))
}

/// 옛 git 인데 `--worktree` 읽기도 주어진 stderr 로 실패한다
fn worktree_fails(stderr: &'static str) -> impl Fn(&[String]) -> Option<RunResult> + Sync {
    move |a: &[String]| old_git_answer(a).or_else(|| has(a, "--worktree").then(|| res(128, "", stderr)))
}

// ---------------------------------------------------------------------------------------------------------------------
// 루프백 가짜 서버(127.0.0.1 만)
// ---------------------------------------------------------------------------------------------------------------------

/// 받은 요청의 (경로, Authorization 헤더)
type Requests = Arc<Mutex<Vec<(String, Option<String>)>>>;

struct Srv {
    port: u16,
    seen: Requests,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Srv {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Srv {
    fn count(&self) -> usize {
        self.seen.lock().unwrap().len()
    }

    /// 통합 인증(NTLM·Negotiate)으로 보이는 Authorization 헤더
    fn integrated(&self) -> Vec<String> {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .filter_map(|(_, a)| a.clone())
            .filter(|a| {
                let l = a.to_ascii_lowercase();
                l.starts_with("ntlm") || l.starts_with("negotiate")
            })
            .collect()
    }

    fn paths(&self) -> Vec<String> {
        self.seen.lock().unwrap().iter().map(|(p, _)| p.clone()).collect()
    }
}

/// 모든 요청에 `status` 와 `headers` 로 답하는 서버. 요청마다 연결을 닫는다
async fn serve(status: &'static str, headers: Vec<&'static str>) -> Srv {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("루프백 바인드");
    let port = listener.local_addr().unwrap().port();
    let seen: Requests = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&seen);
    let task = tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else { return };
            let (log, headers) = (Arc::clone(&log), headers.clone());
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut chunk = [0u8; 4096];
                while !buf.windows(4).any(|w| w == b"\r\n\r\n") && buf.len() < 64 * 1024 {
                    match sock.read(&mut chunk).await {
                        Ok(0) | Err(_) => break,
                        Ok(n) => buf.extend_from_slice(&chunk[..n]),
                    }
                }
                let head = String::from_utf8_lossy(&buf).into_owned();
                let mut lines = head.split("\r\n");
                let path = lines.next().unwrap_or("").split(' ').nth(1).unwrap_or("").to_string();
                let auth = lines.find_map(|l| l.split_once(':').filter(|(k, _)| k.eq_ignore_ascii_case("authorization")).map(|(_, v)| v.trim().to_string()));
                log.lock().unwrap().push((path, auth));
                let mut out = format!("HTTP/1.1 {status}\r\n");
                for h in &headers {
                    out.push_str(h);
                    out.push_str("\r\n");
                }
                out.push_str("Content-Length: 0\r\nConnection: close\r\n\r\n");
                let _ = sock.write_all(out.as_bytes()).await;
                let _ = sock.shutdown().await;
            });
        }
    });
    Srv { port, seen, task }
}

async fn auth_server(challenge: &[&'static str]) -> Srv {
    serve("401 Unauthorized", challenge.to_vec()).await
}

// ---------------------------------------------------------------------------------------------------------------------
// 격리 · 대응표
// ---------------------------------------------------------------------------------------------------------------------

#[test]
fn 격리_전역_설정은_테스트용_파일만_읽는다() {
    let global = std::env::var("GIT_CONFIG_GLOBAL").expect("GIT_CONFIG_GLOBAL 이 설정돼야 한다(isolate)");
    assert!(global.replace('\\', "/").ends_with("src-tauri/tests/gitconfig"), "{global}");
    assert_eq!(std::env::var("GIT_CONFIG_NOSYSTEM").as_deref(), Ok("1"));
    assert_eq!(std::env::var("GCM_INTERACTIVE").as_deref(), Ok("never"));
    // git 이 실제로 그 파일만 본다: 테스트 식별자 말고는 항목이 없다(사용자·시스템 설정이 섞이면 credential.helper=manager 등이 보인다)
    let out = raw("git").args(["config", "--global", "--list", "--name-only"]).output().unwrap();
    let mut keys: Vec<String> = String::from_utf8_lossy(&out.stdout).lines().map(str::to_string).collect();
    keys.sort();
    assert_eq!(keys, ["user.email", "user.name"]);
}

#[test]
fn 대응표가_가리키는_rust_테스트가_있다() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let map: serde_json::Value = serde_json::from_str(&fs::read_to_string(root.join("tests/golden/fetch/test-map.json")).unwrap()).unwrap();
    let (mut ported, mut na) = (0, 0);
    for (title, v) in map.as_object().expect("대응표는 객체") {
        let v = v.as_str().unwrap();
        if let Some(why) = v.strip_prefix("N/A: ") {
            assert!(why.chars().count() > 8, "{title}: N/A 사유가 비어 있다");
            na += 1;
            continue;
        }
        let (file, rest) = v.split_once("::").unwrap_or_else(|| panic!("{title}: '<파일>.rs::<이름>' 이어야 한다 → {v}"));
        let name = rest.split_whitespace().next().unwrap();
        assert!(["fetch.rs", "fetch_guard.rs", "exec.rs"].contains(&file), "{title}: {file}");
        let src = fs::read_to_string(root.join("src-tauri/tests").join(file)).unwrap();
        assert!(src.contains(&format!("fn {name}(")), "{title}: {file} 에 fn {name} 이 없다");
        ported += 1;
    }
    eprintln!("[대응표] 이식 {ported}, N/A {na}");
    assert_eq!(ported + na, map.as_object().unwrap().len());
}

// ---------------------------------------------------------------------------------------------------------------------
// 안전한 저장소의 fetch
// ---------------------------------------------------------------------------------------------------------------------

#[tokio::test]
async fn 안전한_fetch_새_커밋이_올라오면_origin_main_이_갱신된다() {
    let c = Case::new();
    let sha = c.push_commit("second");
    assert_ne!(c.git(&["rev-parse", "origin/main"]), sha);
    c.fetch(true, false).await.expect("fetch");
    assert_eq!(c.git(&["rev-parse", "origin/main"]), sha);
    // --no-write-fetch-head
    assert!(!c.repo.join(".git").join("FETCH_HEAD").exists());
}

#[tokio::test]
async fn prune_원격에서_지운_브랜치의_추적_ref_를_정리한다() {
    let c = Case::new();
    raw_git(&c.work, &["push", "-q", "origin", "main:topic"]);
    c.fetch(true, false).await.unwrap();
    assert!(c.git(&["branch", "-r"]).contains("origin/topic"));
    raw_git(&c.work, &["push", "-q", "origin", "--delete", "topic"]);
    c.fetch(true, false).await.unwrap();
    assert!(!c.git(&["branch", "-r"]).contains("origin/topic"));
}

#[tokio::test]
async fn remote_가_없으면_대상이_아니다() {
    let t = TempDir::new("norem");
    assert!(raw("git").args(["init", "-q"]).arg(t.path()).status().unwrap().success());
    let repo = t.path().to_string_lossy().into_owned();
    let ins = inspect_config(&local(), &repo, None).await.unwrap();
    assert!(!ins.has_remote);
    assert!(ins.remote_names.is_empty());
    // 대상이 없으면 아무것도 하지 않는다
    let nh = t.path().join("no-hooks");
    fs::create_dir_all(&nh).unwrap();
    let nh = nh.to_string_lossy().into_owned();
    run_fetch(&local(), &repo, &ins, &FetchRunOpts { cancel: None, no_hooks_dir: Some(&nh), allow_file: false, credential_helpers: false }).await.unwrap();
}

#[tokio::test]
async fn 훅_reference_transaction_은_실행되지_않는다() {
    let c = Case::new();
    let hook = c.repo.join(".git").join("hooks").join("reference-transaction");
    fs::write(&hook, format!("#!/bin/sh\necho x > \"{}\"\n", posix(&c.marker))).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let sha = c.push_commit("second");
    c.fetch(true, false).await.unwrap();
    assert_eq!(c.git(&["rev-parse", "origin/main"]), sha);
    assert!(!c.marker_exists(), "훅이 실행됐다");
    // 대조: -c core.hooksPath 없이 직접 fetch 하면 훅이 실제로 실행된다(테스트가 의미 있는지 확인)
    c.git(&["update-ref", "-d", "refs/remotes/origin/main"]);
    c.git(&["fetch", "-q", "origin"]);
    assert!(c.marker_exists(), "대조군에서 훅이 실행되지 않았다 — 이 테스트는 아무것도 증명하지 못한다");
}

// ---------------------------------------------------------------------------------------------------------------------
// 악성 저장소 설정
// ---------------------------------------------------------------------------------------------------------------------

#[tokio::test]
async fn 악성_설정은_알_수_없는_설정으로_걸리고_아무것도_실행하지_않는다() {
    for name in ["core.sshCommand", "credential.helper", "remote.origin.uploadpack"] {
        let c = Case::new();
        let payload = c.payload();
        let value = if name == "credential.helper" { format!("!{payload}") } else { payload };
        c.git(&["config", name, &value]);
        let before = c.git(&["rev-parse", "origin/main"]);
        c.push_commit("second");
        let ins = c.inspect().await;
        assert!(dangerous_keys(&ins).contains(&name.to_lowercase()), "{name}: {:?}", dangerous_keys(&ins));
        assert!(!c.marker_exists(), "{name}: 검사가 설정을 실행했다");
        assert_eq!(c.git(&["rev-parse", "origin/main"]), before);
    }
}

#[tokio::test]
async fn include_로_끌어온_파일의_위험_설정도_잡는다() {
    let c = Case::new();
    let inc = c.t.path().join("inc.gitconfig");
    fs::write(&inc, format!("[core]\n\tsshCommand = {}\n", c.payload().replace('\\', "\\\\").replace('"', "\\\""))).unwrap();
    c.git(&["config", "include.path", &posix(&inc)]);
    let ins = c.inspect().await;
    assert!(dangerous_keys(&ins).contains(&"core.sshcommand".to_string()), "{:?}", dangerous_keys(&ins));
    assert!(!c.marker_exists());
}

#[tokio::test]
async fn 위험한_remote_url_은_거부_목록에_오른다() {
    for (label, url) in [("ext::", None), ("옵션처럼 보이는 URL", Some("-oProxyCommand=echo".to_string()))] {
        let c = Case::new();
        let url = url.unwrap_or_else(|| format!("ext::sh -c \"{}\"", c.payload().replace('"', "\\\"")));
        c.git(&["config", "remote.origin.url", &url]);
        let ins = c.inspect().await;
        assert!(forbidden_keys(&ins).iter().any(|k| k.contains("remote.origin.url")), "{label}: {:?}", ins.forbidden);
        assert!(!c.marker_exists(), "{label}");
    }
}

#[tokio::test]
async fn 검사를_우회해_직접_호출해도_ext_전송은_막힌다() {
    let c = Case::new();
    c.git(&["config", "remote.origin.url", &format!("ext::sh -c \"touch {}\"", posix(&c.marker))]);
    let r = local().run(&c.repo_str(), &["fetch", "origin"], &fetch_opts(true)).await.unwrap();
    assert_ne!(r.code, 0);
    assert!(!c.marker_exists());
}

// ---------------------------------------------------------------------------------------------------------------------
// 훅 디렉터리(Windows 의 /dev/null 문제)
// ---------------------------------------------------------------------------------------------------------------------

#[test]
fn 훅_디렉터리_로컬은_앱_소유_디렉터리_원격은_dev_null() {
    let t = TempDir::new("hooks");
    let dir = t.path().to_string_lossy().into_owned();
    assert_eq!(hooks_path_for(true, Some(&dir), cfg!(windows)).unwrap().as_str(), posix(t.path()));
    assert_eq!(hooks_path_for(false, Some(&dir), cfg!(windows)).unwrap().as_str(), "/dev/null");
    assert_eq!(hooks_path_for(false, None, cfg!(windows)).unwrap().as_str(), "/dev/null");
    assert_eq!(hooks_path_for(true, None, false).unwrap().as_str(), "/dev/null");
    assert!(hooks_path_for(true, None, true).unwrap_err().contains("훅 차단"));
    // 비어 있지 않거나 없으면 거부
    fs::write(t.path().join("reference-transaction"), "").unwrap();
    assert!(hooks_path_for(true, Some(&dir), cfg!(windows)).unwrap_err().contains("비어 있지 않아"));
    assert!(hooks_path_for(true, Some(&format!("{dir}/missing")), cfg!(windows)).unwrap_err().contains("없거나"));
}

#[tokio::test]
async fn 앱_소유_훅_디렉터리를_쓰면_훅_경로가_그_안으로_풀린다() {
    let c = Case::new();
    let nh = c.no_hooks();
    let hook_path = |cfg: Vec<String>| {
        let repo = c.repo_str();
        async move {
            let mut args = cfg;
            args.extend(["rev-parse", "--git-path", "hooks/reference-transaction"].map(String::from));
            let r = local().run(&repo, &args, &RunOpts::default()).await.unwrap();
            assert_eq!(r.code, 0, "{}", r.stderr);
            String::from_utf8_lossy(&r.stdout).trim().replace('\\', "/")
        }
    };
    let cfg = fetch_config_for("local", Some(&nh), cfg!(windows), false).unwrap();
    let resolved = hook_path(cfg).await;
    assert!(resolved.to_lowercase().starts_with(&nh.replace('\\', "/").to_lowercase()), "{resolved}");
    assert!(!resolved.contains("dev/null"));
    if cfg!(windows) {
        // 대조: 옛 방식. git.exe 는 /dev/null 을 그대로 받아 저장소 드라이브 기준 경로로 푼다 — 여기에 훅을 심을 수 있다
        assert!(hook_path(vec!["-c".into(), "core.hooksPath=/dev/null".into()]).await.contains("dev/null"));
    }
}

#[tokio::test]
async fn 훅_디렉터리가_비어_있지_않으면_fetch_를_거부한다() {
    let c = Case::new();
    let nh = c.no_hooks();
    fs::write(Path::new(&nh).join("reference-transaction"), "").unwrap();
    let sha = c.push_commit("second");
    let ins = c.inspect().await;
    let err = run_fetch(&local(), &c.repo_str(), &ins, &FetchRunOpts { cancel: None, no_hooks_dir: Some(&nh), allow_file: true, credential_helpers: false }).await.unwrap_err();
    assert!(err.to_string().contains("훅 차단"), "{err}");
    assert_ne!(c.git(&["rev-parse", "origin/main"]), sha, "거부했는데 fetch 가 실행됐다");
}

// ---------------------------------------------------------------------------------------------------------------------
// Windows 통합 인증(NTLM/Negotiate) 유출
// ---------------------------------------------------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ntlm_공격자_http_remote_에_통합_인증_헤더를_보내지_않는다() {
    let srv = auth_server(&["WWW-Authenticate: NTLM", "WWW-Authenticate: Negotiate"]).await;
    let c = Case::new();
    c.git(&["config", "remote.origin.url", &format!("http://127.0.0.1:{}/x.git", srv.port)]);
    let r = c.fetch(false, false).await;
    assert!(r.is_err(), "401 만 주는 서버에서 fetch 가 성공할 수 없다");
    assert!(srv.count() > 0, "서버에 요청이 오지 않았다 — 테스트가 의미 없다");
    assert_eq!(srv.integrated(), Vec::<String>::new());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn userinfo_가_있는_url_은_위험_항목이고_값은_가려진다() {
    for base in ["http://user@127.0.0.1", "http://user:@127.0.0.1"] {
        let srv = auth_server(&["WWW-Authenticate: NTLM", "WWW-Authenticate: Negotiate"]).await;
        let c = Case::new();
        c.git(&["config", "remote.origin.url", &format!("{base}:{}/x.git", srv.port)]);
        let ins = c.inspect().await;
        assert_eq!(dangerous_keys(&ins), ["remote.origin.url"], "{base}");
        assert!(ins.dangerous[0].value.contains("<userinfo>@"), "{}", ins.dangerous[0].value);
        assert!(!ins.dangerous[0].value.contains("//user"), "{}", ins.dangerous[0].value);
        assert_eq!(srv.count(), 0, "{base}: 검사가 요청을 보냈다");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn 자격_증명_도우미는_credential_helpers_일_때만_부른다() {
    let srv = auth_server(&["WWW-Authenticate: Basic realm=\"x\""]).await;
    let c = Case::new();
    c.git(&["config", "remote.origin.url", &format!("http://127.0.0.1:{}/x.git", srv.port)]);
    c.git(&["config", "credential.helper", &format!("!{}", c.payload())]);
    // 자동 fetch(승인 전): 저장소가 정한 도우미도 부르지 않는다(-c credential.helper= 가 목록을 비운다)
    assert!(c.fetch(false, false).await.is_err());
    assert!(srv.count() > 0);
    assert!(!c.marker_exists(), "도우미가 불렸다");
    // 수동 fetch 로 승인된 remote: 도우미를 쓴다
    assert!(c.fetch(false, true).await.is_err());
    assert!(c.marker_exists(), "credential_helpers=true 인데 도우미가 불리지 않았다 — 위 검증이 의미 없다");
}

// ---------------------------------------------------------------------------------------------------------------------
// file 전송과 ssh 명령
// ---------------------------------------------------------------------------------------------------------------------

#[tokio::test]
async fn file_전송은_수동에서_git_2_45_1_이상일_때만_가능하다() {
    let c = Case::new();
    let sha = c.push_commit("second");
    // 자동 fetch: file 은 GIT_ALLOW_PROTOCOL 에서 빠진다
    let err = c.fetch(false, false).await.unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("not allowed") || msg.contains("file"), "{msg}");
    assert_eq!(classify_failure(&msg, false), FetchFailure::Protocol, "재시도해도 소용없는 오류로 분류돼야 한다");
    assert_ne!(c.git(&["rev-parse", "origin/main"]), sha);
    // 수동 fetch: 고쳐진 git(2.45.1+)에서만 file 을 허용한다 — 호출자가 이 판정으로 allow_file 을 정한다
    let versions = GitVersions::default();
    let v = versions.get(&local()).await;
    if version_at_least(v, FILE_TRANSPORT_MIN_GIT) {
        c.fetch(true, false).await.unwrap();
        assert_eq!(c.git(&["rev-parse", "origin/main"]), sha);
    } else {
        eprintln!("[건너뜀] git {v:?} 는 CVE-2024-32004 수정 전이라 file 전송을 허용하지 않는다");
    }
}

#[tokio::test]
async fn git_버전_캐시는_실행기_키별로_읽고_실패도_기억한다() {
    let versions = GitVersions::default();
    let a = versions.get(&local()).await;
    assert!(a.is_some_and(|v| v[0] >= 2.0), "{a:?}");
    assert_eq!(versions.get(&local()).await, a);
    let missing = GitExecutor::Local(LocalExecutor::new("antler-없는-git-실행-파일"));
    assert_ne!(missing.cache_key(), local().cache_key());
    assert_eq!(versions.get(&missing).await, None);
    assert_eq!(GitExecutor::Wsl(WslExecutor::new("Ubuntu")).cache_key(), "wsl:Ubuntu");
}

#[tokio::test]
async fn ssh_명령을_항상_명시하므로_저장소의_core_sshcommand_는_적용되지_않는다() {
    let c = Case::new();
    c.git(&["config", "core.sshCommand", &c.payload()]);
    c.git(&["config", "remote.origin.url", "ssh://127.0.0.1:1/x.git"]);
    let r = local().run(&c.repo_str(), &["fetch", "origin"], &fetch_opts(false)).await.unwrap();
    assert_ne!(r.code, 0);
    assert!(!c.marker_exists());
    // 대조: 환경변수 없이는 저장소 설정이 실행된다(부모의 GIT_SSH_COMMAND 는 설정을 이기므로 뺀다)
    let _ = raw("git").arg("-C").arg(&c.repo).args(["fetch", "origin"]).env("GIT_ALLOW_PROTOCOL", "ssh").env_remove("GIT_SSH_COMMAND").output().unwrap();
    assert!(c.marker_exists(), "대조군에서 core.sshCommand 가 실행되지 않았다 — 이 테스트는 아무것도 증명하지 못한다");
}

// ---------------------------------------------------------------------------------------------------------------------
// 허용 목록: 현실적인 저장소 설정
// ---------------------------------------------------------------------------------------------------------------------

#[tokio::test]
async fn 허용_목록_clone_직후의_기본_설정은_통과한다() {
    let c = Case::new();
    assert_eq!(dangerous_keys(&c.inspect().await), Vec::<String>::new());
}

#[tokio::test]
async fn 허용_목록_현실적인_설정은_통과하고_fetch_도_된다() {
    let c = Case::new();
    for (k, v) in [
        ("lfs.repositoryformatversion", "0"),
        ("filter.lfs.clean", "git-lfs clean -- %f"),
        ("filter.lfs.smudge", "git-lfs smudge -- %f"),
        ("filter.lfs.process", "git-lfs filter-process"),
        ("filter.lfs.required", "true"),
        ("branch.main.vscode-merge-base", "origin/main"),
        ("branch.main.github-pr-owner-number", "owner#repo#12"),
        ("core.hooksPath", ".husky"),
        ("core.fsmonitor", "true"),
        ("core.untrackedcache", "true"),
        ("core.excludesFile", ".gitignore-extra"),
        ("user.name", "Me"),
        ("user.email", "me@example.com"),
        ("pull.rebase", "true"),
        ("fetch.prune", "true"),
        ("remote.origin.promisor", "true"),
        ("remote.origin.partialclonefilter", "blob:none"),
        ("extensions.partialclone", "origin"),
        ("submodule.lib.url", "https://example.com/lib.git"),
        ("submodule.lib.active", "true"),
    ] {
        c.git(&["config", k, v]);
    }
    assert_eq!(dangerous_keys(&c.inspect().await), Vec::<String>::new());
    // 검사를 통과해 fetch 까지 된다(file remote 라 수동 fetch 의 allow_file)
    c.fetch(true, false).await.expect("fetch");
}

#[tokio::test]
async fn 허용_목록_lfs_기본값이_아닌_설정과_명령형_fsmonitor_는_걸린다() {
    let c = Case::new();
    c.git(&["config", "filter.lfs.process", &c.payload()]);
    c.git(&["config", "core.fsmonitor", &c.payload()]);
    let mut keys = dangerous_keys(&c.inspect().await);
    keys.sort();
    assert_eq!(keys, ["core.fsmonitor", "filter.lfs.process"]);
    assert!(!c.marker_exists());
}

// ---------------------------------------------------------------------------------------------------------------------
// 번들 URI · trace2
// ---------------------------------------------------------------------------------------------------------------------

#[tokio::test]
async fn 번들_uri_로컬_경로는_위험_항목이고_받지_않는다() {
    let c = Case::new();
    let bundle = c.t.path().join("b.bundle");
    raw_git(&c.work, &["commit", "-q", "--allow-empty", "-m", "in-bundle"]);
    raw_git(&c.work, &["bundle", "create", &posix(&bundle), "main"]);
    c.git(&["config", "fetch.bundleURI", &posix(&bundle)]);
    let ins = c.inspect().await;
    assert!(dangerous_keys(&ins).contains(&"fetch.bundleuri".to_string()), "{:?}", dangerous_keys(&ins));
    assert_eq!(c.git(&["for-each-ref", "refs/bundles"]), "");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn 번들_uri_가드를_우회해_실행해도_번들에_접근하지_않는다() {
    let host = serve("404 Not Found", vec![]).await;
    let c = Case::new();
    c.push_commit("second");
    let bundle = c.t.path().join("b.bundle");
    raw_git(&c.work, &["bundle", "create", &posix(&bundle), "main"]);
    for target in [format!("http://127.0.0.1:{}/b.bundle", host.port), posix(&bundle)] {
        c.git(&["config", "fetch.bundleURI", &target]);
        let ins = c.inspect().await;
        // 가드는 이것을 걸러야 한다 — 일부러 무시하고 실행한다
        assert!(dangerous_keys(&ins).contains(&"fetch.bundleuri".to_string()));
        let nh = c.no_hooks();
        run_fetch(&local(), &c.repo_str(), &ins, &FetchRunOpts { cancel: None, no_hooks_dir: Some(&nh), allow_file: true, credential_helpers: false }).await.expect("fetch");
        assert_eq!(host.paths(), Vec::<String>::new(), "번들 호스트에 접근했다: {target}");
        assert_eq!(c.git(&["for-each-ref", "refs/bundles"]), "");
    }
    // 대조: 덮어쓰기 없이 git 을 직접 부르면 번들 호스트에 접근하는지 기록한다
    c.git(&["config", "fetch.bundleURI", &format!("http://127.0.0.1:{}/b.bundle", host.port)]);
    let _ = raw("git").arg("-C").arg(&c.repo).args(["fetch", "origin"]).env("GIT_ALLOW_PROTOCOL", "file:http").output().unwrap();
    eprintln!("[대조] fetch.bundleURI 덮어쓰기 없이: 번들 호스트 요청 {}개", host.count());
}

#[tokio::test]
async fn trace2_는_알_수_없는_설정이다() {
    let c = Case::new();
    c.git(&["config", "trace2.eventTarget", &posix(&c.marker)]);
    c.git(&["config", "trace2.normalTarget", &posix(&c.marker)]);
    let keys = dangerous_keys(&c.inspect().await);
    assert!(keys.contains(&"trace2.eventtarget".to_string()) && keys.contains(&"trace2.normaltarget".to_string()), "{keys:?}");
    assert!(!c.marker_exists());
}

#[tokio::test]
async fn http_ssh_설정도_알_수_없는_설정으로_걸린다() {
    for (k, v) in [("http.schannelCheckRevoke", "false"), ("http.followRedirects", "true"), ("ssh.variant", "simple")] {
        let c = Case::new();
        c.git(&["config", k, v]);
        assert_eq!(dangerous_keys(&c.inspect().await), [k.to_lowercase()], "{k}");
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// fetch 는 로컬 ref 를 쓰지 않는다(설정이 목적지를 정해도)
// ---------------------------------------------------------------------------------------------------------------------

/// 로컬 ref 스냅샷(refs/heads·tags·stash·replace 와 HEAD)
fn snapshot(repo: &Path) -> BTreeMap<String, String> {
    let mut m = BTreeMap::new();
    let out = raw_git(repo, &["for-each-ref", "--format=%(refname) %(objectname)", "refs/heads", "refs/tags", "refs/stash", "refs/replace"]);
    for line in out.lines().filter(|l| !l.is_empty()) {
        let (name, sha) = line.split_once(' ').unwrap();
        m.insert(name.to_string(), sha.to_string());
    }
    m.insert("HEAD".into(), format!("{} {}", raw_git(repo, &["rev-parse", "HEAD"]), raw_git(repo, &["symbolic-ref", "-q", "HEAD"])));
    m
}

/// 이전에 있던 로컬 ref 는 하나도 바뀌거나 사라지지 않았고, 새로 생긴 것은 (원격에서 따라온 새) 태그뿐이다
fn expect_local_refs_kept(label: &str, before: &BTreeMap<String, String>, after: &BTreeMap<String, String>) {
    for (name, v) in before {
        assert_eq!(after.get(name), Some(v), "{label}: {name}");
    }
    for name in after.keys().filter(|n| !before.contains_key(*n)) {
        assert!(name.starts_with("refs/tags/"), "{label}: 새 ref {name}");
    }
}

struct Prepared {
    c: Case,
    sha: String,
}

fn prepare() -> Prepared {
    let c = Case::new();
    let sha = c.git(&["rev-parse", "HEAD"]);
    c.git(&["branch", "feature"]);
    c.git(&["tag", "localtag"]);
    c.git(&["update-ref", "refs/stash", &sha]);
    c.push_commit("second");
    raw_git(&c.work, &["push", "-q", "origin", "main:feature"]);
    raw_git(&c.work, &["tag", "v2"]);
    raw_git(&c.work, &["push", "-q", "origin", "v2"]);
    Prepared { c, sha }
}

#[tokio::test]
async fn 로컬_ref_보호_시나리오별_설정이_목적지를_정해도_로컬_ref_는_그대로다() {
    type Setup = fn(&Case, &str);
    fn add(c: &Case, spec: &str) {
        c.git(&["config", "--add", "remote.origin.fetch", spec]);
    }
    let scenarios: [(&str, Setup); 8] = [
        ("로컬 브랜치 덮어쓰기", |c, _| add(c, "+refs/heads/feature:refs/heads/feature")),
        ("refs/stash 덮어쓰기", |c, _| add(c, "+refs/heads/main:refs/stash")),
        ("모든 로컬 브랜치 prune", |c, _| add(c, "+refs/heads/none/*:refs/heads/*")),
        ("로컬 태그 prune(prunetags)", |c, _| {
            c.git(&["config", "fetch.prunetags", "true"]);
            c.git(&["config", "remote.origin.prunetags", "true"]);
            c.git(&["config", "remote.origin.prune", "true"]);
            add(c, "+refs/tags/*:refs/tags/*");
        }),
        ("core.bare=true 로 체크아웃 브랜치 이동", |c, _| {
            c.git(&["config", "core.bare", "true"]);
            add(c, "+refs/heads/main:refs/heads/main");
        }),
        ("refs/replace 생성", |c, sha| add(c, &format!("+refs/heads/main:refs/replace/{sha}"))),
        ("refs/heads/HEAD 생성", |c, _| add(c, "+refs/heads/main:refs/heads/HEAD")),
        ("HEAD 덮어쓰기", |c, _| add(c, "+refs/heads/main:HEAD")),
    ];
    for (label, setup) in scenarios {
        let Prepared { c, sha } = prepare();
        setup(&c, &sha);
        let before = snapshot(&c.repo);
        // 자동 fetch 는 막힌다: 허용 목록 밖의 설정으로 걸린다
        let ins = c.inspect().await;
        assert!(!ins.dangerous.is_empty(), "{label}: 알 수 없는 설정으로 걸리지 않았다");
        expect_local_refs_kept(label, &before, &snapshot(&c.repo));
        // 사용자가 허용한 수동 fetch(여기서는 검사 결과를 무시하고 실행): 원격 추적 ref 만 갱신된다
        c.fetch(true, false).await.unwrap_or_else(|e| panic!("{label}: {e}"));
        assert_ne!(c.git(&["rev-parse", "origin/main"]), sha, "{label}");
        assert_ne!(c.git(&["rev-parse", "origin/feature"]), sha, "{label}");
        expect_local_refs_kept(label, &before, &snapshot(&c.repo));
        // 한 번 더(설정이 아니라 명령 자체가 목적지를 고정한다)
        let third = c.push_commit("third");
        c.fetch(true, false).await.unwrap_or_else(|e| panic!("{label}: {e}"));
        assert_eq!(c.git(&["rev-parse", "origin/main"]), third, "{label}");
        expect_local_refs_kept(label, &before, &snapshot(&c.repo));
    }
}

#[tokio::test]
async fn 로컬_ref_보호_정상_설정은_추적_ref_갱신_prune_새_태그만_따라온다() {
    let Prepared { c, .. } = prepare();
    raw_git(&c.work, &["push", "-q", "origin", "main:gone"]);
    c.fetch(true, false).await.unwrap();
    assert!(c.git(&["branch", "-r"]).contains("origin/gone"));
    raw_git(&c.work, &["push", "-q", "origin", "--delete", "gone"]);
    let before = snapshot(&c.repo);
    c.fetch(true, false).await.unwrap();
    let branches = c.git(&["branch", "-r"]);
    assert!(!branches.contains("origin/gone"));
    assert!(branches.contains("origin/feature"));
    let tags = c.git(&["tag"]);
    assert!(tags.contains("v2") && tags.contains("localtag"), "{tags}");
    expect_local_refs_kept("정상", &before, &snapshot(&c.repo));
}

#[tokio::test]
async fn 로컬_ref_보호_원격에서_지운_태그는_지우지_않는다() {
    let Prepared { c, .. } = prepare();
    c.fetch(true, false).await.unwrap();
    assert!(c.git(&["tag"]).contains("v2"));
    raw_git(&c.work, &["push", "-q", "origin", "--delete", "v2"]);
    c.git(&["config", "remote.origin.prunetags", "true"]);
    c.fetch(true, false).await.unwrap();
    assert!(c.git(&["tag"]).contains("v2"));
}

// ---------------------------------------------------------------------------------------------------------------------
// UNC·NT 네임스페이스 remote
// ---------------------------------------------------------------------------------------------------------------------

#[tokio::test]
async fn unc_remote_설정은_확인_없이_거부한다() {
    let c = Case::new();
    c.git(&["config", "remote.origin.url", "//127.0.0.1@1/DavWWWRoot/x.git"]);
    let ins = c.inspect().await;
    assert_eq!(forbidden_keys(&ins).first().map(String::as_str), Some("remote.origin.url"), "{:?}", ins.forbidden);
    assert!(ins.forbidden[0].reason.contains("UNC"), "{}", ins.forbidden[0].reason);
}

#[tokio::test]
async fn nt_네임스페이스_remote_설정도_확인_없이_거부한다() {
    let c = Case::new();
    c.git(&["config", "remote.origin.url", "\\??\\UNC\\127.0.0.1\\share\\x.git"]);
    let ins = c.inspect().await;
    assert_eq!(forbidden_keys(&ins).first().map(String::as_str), Some("remote.origin.url"), "{:?}", ins.forbidden);
    assert!(ins.forbidden[0].reason.contains("NT 네임스페이스"), "{}", ins.forbidden[0].reason);
}

#[tokio::test]
async fn submodule_url_도_같은_검사를_받는다() {
    let c = Case::new();
    c.git(&["config", "submodule.lib.url", "\\\\host\\share\\lib.git"]);
    assert!(forbidden_keys(&c.inspect().await).contains(&"submodule.lib.url".to_string()));
}

// ---------------------------------------------------------------------------------------------------------------------
// 옛 형식 remote
// ---------------------------------------------------------------------------------------------------------------------

fn init_repo(tag: &str) -> TempDir {
    let t = TempDir::new(tag);
    assert!(raw("git").args(["init", "-q"]).arg(t.path()).status().unwrap().success());
    t
}

#[tokio::test]
async fn 옛_형식_remotes_파일의_url_도_검사한다() {
    // git 은 설정에 이름이 있는(remote.<n>.* 키가 하나라도 있는) remote 의 URL 을 옛 파일에서도 읽는다 — 설정 키 목록만으로는 못 본다
    let t = init_repo("legacy");
    let dir = t.path();
    raw_git(dir, &["config", "remote.legacy.fetch", "+refs/heads/*:refs/remotes/legacy/*"]);
    let marker = dir.join("PWNED");
    fs::create_dir_all(dir.join(".git").join("remotes")).unwrap();
    fs::write(dir.join(".git").join("remotes").join("legacy"), format!("URL: ext::sh -c \"touch {}\"\nPull: refs/heads/*:refs/remotes/legacy/*\n", posix(&marker))).unwrap();
    let ins = inspect_config(&local(), &dir.to_string_lossy(), None).await.unwrap();
    assert!(ins.has_remote);
    assert!(forbidden_keys(&ins).contains(&"remote.legacy.url".to_string()), "{:?}", ins.forbidden);
    assert!(!marker.exists());
}

#[tokio::test]
async fn 옛_형식_branches_파일의_url_도_검사한다() {
    let t = init_repo("legacy");
    let dir = t.path();
    raw_git(dir, &["config", "remote.old.fetch", "+refs/heads/*:refs/remotes/old/*"]);
    fs::create_dir_all(dir.join(".git").join("branches")).unwrap();
    fs::write(dir.join(".git").join("branches").join("old"), "fd::17\n").unwrap();
    let ins = inspect_config(&local(), &dir.to_string_lossy(), None).await.unwrap();
    assert!(forbidden_keys(&ins).contains(&"remote.old.url".to_string()), "{:?}", ins.forbidden);
}

#[tokio::test]
async fn 설정에_이름이_없는_옛_파일_remote_는_대상이_아니다() {
    let t = init_repo("legacy");
    let dir = t.path();
    fs::create_dir_all(dir.join(".git").join("remotes")).unwrap();
    fs::write(dir.join(".git").join("remotes").join("only"), "URL: https://example.invalid/x.git\n").unwrap();
    let ins = inspect_config(&local(), &dir.to_string_lossy(), None).await.unwrap();
    assert!(!ins.has_remote);
}

// ---------------------------------------------------------------------------------------------------------------------
// --show-scope 를 모르는 옛 git(2.26 미만)
// ---------------------------------------------------------------------------------------------------------------------

#[tokio::test]
async fn 옛_git_범위마다_따로_읽어_같은_검사를_한다() {
    let c = Case::new();
    c.git(&["config", "core.sshCommand", &c.payload()]);
    c.git(&["config", "core.excludesFile", ".gitignore-extra"]);
    // 전역 설정 파일은 커밋된 것이라 고치지 않는다 — `--global` 읽기에 전역 core.sshCommand 가 있다고 답한다
    let old = Fake::new("local", |a: &[String]| {
        old_git_answer(a).or_else(|| has(a, "--global").then(|| res(0, "core.sshcommand\nssh -i mykey\0", "")))
    });
    let ins = inspect_config(&old, &c.repo_str(), None).await.unwrap();
    assert!(old.args().iter().any(|a| has(a, "--local")), "{:?}", old.args());
    assert_eq!(dangerous_keys(&ins), ["core.sshcommand"]);
    assert_eq!(ins.user_ssh_command.as_deref(), Some("ssh -i mykey"));
    assert!(ins.has_remote);
    assert_eq!(ins.remote_names, ["origin"]);
    assert!(!c.marker_exists());
}

#[tokio::test]
async fn 옛_git_worktree_config_가_켜져_있으면_config_worktree_도_읽는다() {
    let c = Case::new();
    c.git(&["config", "extensions.worktreeConfig", "true"]);
    c.git(&["config", "--worktree", "trace2.eventTarget", "/tmp/x"]);
    let old = Fake::new("local", old_git_answer);
    let ins = inspect_config(&old, &c.repo_str(), None).await.unwrap();
    assert!(old.args().iter().any(|a| has(a, "--worktree")));
    assert_eq!(dangerous_keys(&ins), ["trace2.eventtarget"]);
}

#[tokio::test]
async fn 옛_git_config_worktree_가_없거나_모르면_비어_있는_것으로_보고_다른_실패는_거부한다() {
    let c = Case::new();
    c.git(&["config", "extensions.worktreeConfig", "true"]);
    // 실제 git 의 "파일 없음" 메시지(config.worktree 를 만들지 않은 저장소)
    let real = Fake::new("local", old_git_answer);
    assert!(inspect_config(&real, &c.repo_str(), None).await.unwrap().dangerous.is_empty());
    let unknown = Fake::new("local", worktree_fails("error: unknown option `worktree'"));
    assert!(inspect_config(&unknown, &c.repo_str(), None).await.unwrap().dangerous.is_empty());
    let bad = Fake::new("local", worktree_fails("fatal: bad config line 1 in file .git/config.worktree"));
    let err = inspect_config(&bad, &c.repo_str(), None).await.unwrap_err();
    assert!(err.to_string().contains("검사하지 못해"), "{err}");
}

#[tokio::test]
async fn 옛_git_include_로_끌어온_저장소_설정도_잡는다() {
    let c = Case::new();
    let inc = c.t.path().join("extra.cfg");
    fs::write(&inc, "[credential]\n\thelper = !evil\n").unwrap();
    c.git(&["config", "include.path", &posix(&inc)]);
    let old = Fake::new("local", old_git_answer);
    let ins = inspect_config(&old, &c.repo_str(), None).await.unwrap();
    assert!(dangerous_keys(&ins).contains(&"credential.helper".to_string()), "{:?}", dangerous_keys(&ins));
}

#[tokio::test]
async fn 옛_git_local_설정을_읽지_못하면_fetch_하지_않는다() {
    let t = TempDir::new("notrepo");
    let old = Fake::new("local", old_git_answer);
    let err = inspect_config(&old, &t.path().to_string_lossy(), None).await.unwrap_err();
    assert!(err.to_string().contains("저장소 설정을 검사하지 못해"), "{err}");
}

#[tokio::test]
async fn 옛_git_show_scope_와_무관한_실패는_그대로_실패한다() {
    let failing = Fake::new("local", |_: &[String]| Some(res(128, "", "fatal: bad config line 3")));
    let err = inspect_config(&failing, ".", None).await.unwrap_err();
    assert!(err.to_string().contains("bad config line"), "{err}");
    assert_eq!(failing.args().len(), 1, "범위별 읽기로 넘어가면 안 된다");
}

#[tokio::test]
async fn 옛_git_은_no_write_fetch_head_없이_다시_한다() {
    let mut ins = inspect_entries(&[]);
    ins.remote_names = vec!["o".into(), "p".into()];
    // 모르는 옵션이면 그 옵션을 빼고 한 번 더. 두 번째 remote 도 같은 순서로 한다
    let old = Fake::new("wsl", |a: &[String]| {
        has(a, "fetch").then(|| if has(a, "--no-write-fetch-head") { res(129, "", "error: unknown option `no-write-fetch-head'\n") } else { res(0, "", "") })
    });
    run_fetch_with(&old, ".", &ins, &FetchRunOpts { cancel: None, no_hooks_dir: None, allow_file: false, credential_helpers: false }, &HashMap::new(), true).await.unwrap();
    let calls = old.args();
    assert_eq!(calls.len(), 4, "{calls:?}");
    for (i, with_flag) in [true, false, true, false].into_iter().enumerate() {
        assert_eq!(has(&calls[i], "--no-write-fetch-head"), with_flag, "호출 {i}");
    }
    // 다른 이유로 실패하면 다시 하지 않고, 첫 오류를 돌려주되 나머지 remote 도 계속한다
    let failing = Fake::new("wsl", |a: &[String]| has(a, "fetch").then(|| res(128, "", "fatal: 서버가 거부했다\n")));
    let err = run_fetch_with(&failing, ".", &ins, &FetchRunOpts { cancel: None, no_hooks_dir: None, allow_file: false, credential_helpers: false }, &HashMap::new(), true)
        .await
        .unwrap_err();
    assert_eq!(failing.args().len(), 2, "remote 마다 한 번, 재시도 없음");
    assert_eq!(err.to_string(), "fatal: 서버가 거부했다");
}

// ---------------------------------------------------------------------------------------------------------------------
// 위험 목록(이식 우회) — 6~11
// ---------------------------------------------------------------------------------------------------------------------

#[tokio::test]
async fn 위험6_argv_순서_c_는_모두_fetch_앞_refmap_존재_c_키_중복_없음_이름은_구분자_뒤() {
    let mut ins = inspect_entries(&[]);
    let names = ["origin", "up stream", "-x", "--upload-pack=evil", "a:b", "한글"];
    ins.remote_names = names.iter().map(ToString::to_string).collect();
    for (kind, helpers) in [("wsl", false), ("wsl", true)] {
        let ok = Fake::new(kind, |_: &[String]| Some(res(0, "", "")));
        run_fetch_with(&ok, ".", &ins, &FetchRunOpts { cancel: None, no_hooks_dir: None, allow_file: false, credential_helpers: helpers }, &HashMap::new(), true).await.unwrap();
        let calls = ok.args();
        assert_eq!(calls.len(), names.len());
        for (name, a) in names.iter().zip(&calls) {
            let fetch_at = a.iter().position(|x| x == "fetch").expect("fetch");
            // 모든 -c 는 fetch 앞(fetch 뒤의 -c 는 git 이 fetch 의 옵션으로 읽는다)
            for (i, x) in a.iter().enumerate() {
                if x == "-c" {
                    assert!(i < fetch_at, "{name}: -c 가 fetch 뒤에 있다: {a:?}");
                }
            }
            assert!(a.contains(&"--refmap=".to_string()), "{name}: --refmap= 누락");
            // 같은 -c 키 중복 없음(뒤의 것이 앞의 것을 가리면 방어가 풀린다)
            let keys: Vec<&str> = a.windows(2).filter(|w| w[0] == "-c").map(|w| w[1].split('=').next().unwrap()).collect();
            let mut uniq = keys.clone();
            uniq.sort_unstable();
            uniq.dedup();
            assert_eq!(uniq.len(), keys.len(), "{name}: -c 키 중복 {keys:?}");
            // remote 이름은 `--` 뒤와 refspec 안에만 나온다
            let dd = a.iter().position(|x| x == "--").expect("--");
            assert!(dd > fetch_at && a.iter().position(|x| x == "--refmap=").unwrap() < dd);
            assert_eq!(&a[dd + 1..], [name.to_string(), format!("+refs/heads/*:refs/remotes/{name}/*")], "{name}");
            assert_eq!(a.iter().filter(|x| x.contains(*name)).count(), 2, "{name}: 이름은 정확히 두 곳(이름·refspec)에만");
        }
        // fetch 환경은 닫힌 타입으로만 간다
        for (_, o) in ok.calls.lock().unwrap().iter() {
            assert_eq!(o.fetch_env.as_ref().map(|f| f.allow_file), Some(false));
        }
    }
    // 같은 결과를 순수 함수로도: 첫 시도(--no-write-fetch-head)와 재시도의 차이는 그 옵션 하나뿐이다
    let cfg = fetch_config_for("wsl", None, true, false).unwrap();
    let (first, retry) = (fetch_args(&cfg, "o", false), fetch_args(&cfg, "o", true));
    assert_eq!(first.len(), retry.len() + 1);
    assert!(first.contains(&"--no-write-fetch-head".to_string()) && !retry.contains(&"--no-write-fetch-head".to_string()));
}

#[tokio::test]
async fn 위험7_wsl_에_windows_no_hooks_경로가_가지_않는다() {
    // 호출자가 로컬용 드라이브 문자 경로를 줘도 WSL 은 그것을 쓰지 않는다(훅 경로는 실행기 종류로 run.rs 가 만든다)
    let cfg = fetch_config_for("wsl", Some("C:/Users/me/no-hooks"), true, false).unwrap();
    assert!(cfg.contains(&"core.hooksPath=/dev/null".to_string()), "{cfg:?}");
    assert!(!cfg.iter().any(|a| a.contains("C:") || a.contains("no-hooks")), "{cfg:?}");
    // 실제 호출 인자로도(디렉터리가 없어도 WSL 은 검사하지 않는다)
    let mut ins = inspect_entries(&[]);
    ins.remote_names = vec!["origin".into()];
    let wsl = Fake::new("wsl", |_: &[String]| Some(res(0, "", "")));
    run_fetch_with(&wsl, ".", &ins, &FetchRunOpts { cancel: None, no_hooks_dir: Some("C:/없는/디렉터리"), allow_file: false, credential_helpers: false }, &HashMap::new(), true)
        .await
        .unwrap();
    let a = &wsl.args()[0];
    assert!(a.contains(&"core.hooksPath=/dev/null".to_string()) && !a.iter().any(|x| x.contains("C:")), "{a:?}");
    // 로컬은 반대다: Windows 에서 디렉터리가 없으면 /dev/null 로 물러서지 않고 거부한다
    let local_fake = Fake::new("local", |_: &[String]| Some(res(0, "", "")));
    let err = run_fetch_with(&local_fake, ".", &ins, &FetchRunOpts { cancel: None, no_hooks_dir: None, allow_file: false, credential_helpers: false }, &HashMap::new(), true)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("훅 차단"), "{err}");
    assert!(local_fake.args().is_empty(), "거부했는데 git 이 실행됐다");
}

/// 주석을 뺀 소스(설명에 나오는 단어에 걸리지 않게)
fn code_only(path: &str) -> String {
    let src = fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(path)).unwrap();
    src.lines().filter(|l| !l.trim_start().starts_with("//")).collect::<Vec<_>>().join("\n")
}

#[test]
fn 위험8_canonicalize_를_쓰지_않고_unc_nt_모양_훅_경로를_거부한다() {
    for f in ["src/git/fetch/run.rs", "src/git/fetch/guard.rs"] {
        assert!(!code_only(f).contains("canonicalize"), "{f}: 경로를 canonicalize 하면 Windows 에서 \\\\?\\ 접두가 붙어 UNC·NT 모양이 된다");
    }
    // 디렉터리 검사를 통과한다고 가정해도 UNC·NT 모양은 거부한다
    for bad in ["\\\\?\\C:\\Users\\me\\no-hooks", "//?/C:/Users/me/no-hooks", "\\\\host\\share\\no-hooks", "//host/share/no-hooks", "\\??\\C:\\x", "/??/C:/x", "\\\\.\\C:\\x", "\\\\?\\UNC\\h\\s\\x"] {
        assert!(hooks_path_with(true, Some(bad), true, |_| true).is_err(), "{bad}");
    }
    for ok in ["C:\\Users\\me\\no-hooks", "C:/Users/me/no-hooks", "D:\\a b\\한글", "/tmp/no-hooks"] {
        assert!(hooks_path_with(true, Some(ok), true, |_| true).is_ok(), "{ok}");
    }
    // 실제로 Windows 의 canonicalize 는 \\?\ 를 붙인다 — 그 결과를 그대로 넘기면 거부된다(canonicalize 를 쓰면 안 되는 이유)
    if cfg!(windows) {
        let t = TempDir::new("canon");
        let canon = fs::canonicalize(t.path()).unwrap();
        assert!(canon.to_string_lossy().starts_with("\\\\?\\"), "{canon:?}");
        assert!(hooks_path_for(true, Some(&canon.to_string_lossy()), true).is_err());
    }
}

/// `git credential fill` 을 도는 alias. 사용자 이름을 askpass 로 묻게 되고, askpass 가 안 되면 프롬프트 금지로 실패한다
const ASKPASS_ALIAS: &str = "alias.antlerprobe=!printf 'protocol=https\\nhost=x.invalid\\n\\n' | git credential fill";

/// 실행기가 git 에 준 환경 중 방어에 쓰이는 네 개를 `KEY=VALUE` 줄로 내는 alias. 값이 없으면 줄이 없고 빈 값이면 `KEY=` 라 둘이 구분된다.
/// 이름은 대소문자 무시로 찾는다: Windows 에서는 부모가 `Git_Pager` 로 갖고 있으면 자식 환경의 이름이 그 철자로 남는다(값은 우리 것이다)
const ENV_ALIAS: &str = "alias.antlerenv=!env | grep -iE '^(GIT_SSH_COMMAND|GIT_ALLOW_PROTOCOL|GIT_ASKPASS|GIT_PAGER)='";

/// [`ENV_ALIAS`] 의 출력을 (대문자 이름 → 값)으로
fn parse_env(out: &[u8]) -> BTreeMap<String, String> {
    String::from_utf8_lossy(out).lines().filter_map(|l| l.split_once('=').map(|(k, v)| (k.to_ascii_uppercase(), v.trim_end_matches('\r').to_string()))).collect()
}

fn env_map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs.iter().map(|(k, v)| ((*k).to_string(), (*v).to_string())).collect()
}

#[tokio::test]
async fn 위험9_git_askpass_빈_값이_로컬_git_에_빈_값으로_도착한다() {
    let c = Case::new();
    let repo = c.repo_str();
    // ① 환경: 빈 값이 unset 이 아니라 빈 값으로 자식(alias 셸)에 간다 — git.exe 자신의 getenv 가 아니라 git 이 자식에게 넘긴 환경을 본다
    let seen = |fetch: bool| {
        let (repo, o) = (repo.clone(), if fetch { fetch_opts(false) } else { RunOpts::default() });
        async move {
            let r = local().run(&repo, &["-c", ENV_ALIAS, "antlerenv"], &o).await.unwrap();
            assert_eq!(r.code, 0, "{}", r.stderr);
            parse_env(&r.stdout)
        }
    };
    assert_eq!(
        seen(true).await,
        env_map(&[("GIT_SSH_COMMAND", SSH_COMMAND_DEFAULT), ("GIT_ALLOW_PROTOCOL", "https:http:ssh:git"), ("GIT_ASKPASS", ""), ("GIT_PAGER", "cat")]),
        "GIT_ASKPASS 가 빈 값으로 도착해야 한다(줄이 없으면 unset 이다)"
    );
    // 대조: fetch 환경이 없으면 GIT_ASKPASS 는 unset 이고 부모의 GIT_SSH_COMMAND 가 그대로다
    assert_eq!(seen(false).await, env_map(&[("GIT_SSH_COMMAND", PARENT_SSH), ("GIT_ALLOW_PROTOCOL", "none"), ("GIT_PAGER", "cat")]));
    // ② 효과: 저장소 설정의 core.askpass 가 실행되지 않는다(GIT_ASKPASS 가 비어 있으면 git 은 core.askpass 로 물러서지 않는다)
    let marker = c.t.path().join("ASKED");
    // git 은 askpass 를 셸 없이 프로그램으로 실행한다 — 마커를 만드는 스크립트 파일을 준다(Git for Windows 는 #! 를 해석한다)
    let script = c.t.path().join("askpass.sh");
    fs::write(&script, format!("#!/bin/sh\necho x > \"{}\"\necho user\n", posix(&marker))).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let askpass = format!("core.askpass={}", posix(&script));
    let probe = |fetch: bool| {
        let (repo, askpass, o) = (repo.clone(), askpass.clone(), if fetch { fetch_opts(false) } else { RunOpts::default() });
        async move { local().run(&repo, &["-c", &askpass, "-c", ASKPASS_ALIAS, "antlerprobe"], &o).await.unwrap() }
    };
    let r = probe(true).await;
    assert_ne!(r.code, 0);
    assert!(r.stderr.contains("terminal prompts disabled"), "프롬프트까지 가지 못했다(검증이 의미 없다): {}", r.stderr);
    assert!(!marker.exists(), "GIT_ASKPASS='' 인데 core.askpass 가 실행됐다");
    // 대조: fetch 환경이 없으면 core.askpass 가 실행된다 — 위 검증이 의미 있다는 증거
    let ctl = probe(false).await;
    assert!(marker.exists(), "대조군에서 askpass 가 실행되지 않았다 — 이 테스트는 아무것도 증명하지 못한다: code {} stderr {:?} stdout {:?}", ctl.code, ctl.stderr, String::from_utf8_lossy(&ctl.stdout));
}

#[test]
fn 위험10_ssh_명령은_사용자_설정_다음_local_일_때만_환경에서_읽는다() {
    let ins = inspect_entries(&[]);
    let win_env: HashMap<String, String> = [("GIT_SSH_COMMAND", "C:\\Win\\ssh.exe -F x"), ("GIT_SSH", "C:\\Win\\plink.exe")].map(|(k, v)| (k.to_string(), v.to_string())).into();
    // Windows 의 GIT_SSH_COMMAND·GIT_SSH 는 WSL 로 새지 않는다(Windows 경로를 리눅스 git 이 실행하려 한다)
    for allow in [false, true] {
        assert_eq!(fetch_env_for(&ins, "wsl", &win_env, true, allow).ssh_command, SSH_COMMAND_DEFAULT);
    }
    assert_eq!(fetch_env_for(&ins, "local", &win_env, true, false).ssh_command, "C:\\Win\\ssh.exe -F x");
    let only_ssh: HashMap<String, String> = [("GIT_SSH".to_string(), "C:\\Win\\plink.exe".to_string())].into();
    assert_eq!(fetch_env_for(&ins, "local", &only_ssh, true, false).ssh_command, "'C:/Win/plink.exe'");
    assert_eq!(fetch_env_for(&ins, "local", &HashMap::new(), true, false).ssh_command, SSH_COMMAND_DEFAULT);
    // 사용자(시스템·전역) core.sshCommand 가 환경보다 앞서고 WSL 에도 적용된다. 저장소 안의 것은 어디에도 없다
    let user = inspect_entries(&antler_lib::git::fetch::guard::parse_config_list("global\0core.sshcommand\nssh -F u\0local\0core.sshcommand\nevil\0"));
    assert_eq!(fetch_env_for(&user, "local", &win_env, true, false).ssh_command, "ssh -F u");
    assert_eq!(fetch_env_for(&user, "wsl", &win_env, true, false).ssh_command, "ssh -F u");
    assert!(user.dangerous.iter().any(|d| d.key == "core.sshcommand"));
}

#[tokio::test]
async fn 위험11_env_상속은_대소문자를_구분하지_않는_플랫폼에서도_우리_값이_이긴다() {
    // ① 조회: Windows 의 환경변수 이름은 대소문자를 구분하지 않는다(Node 의 process.env 도 그렇다). std::env::var 가 그렇게 찾고
    //    std::env::vars() 로 맵을 채우면 `Git_Ssh_Command` 를 놓친다 — ssh_env 는 이름을 정해 찾는다
    assert_eq!(std::env::var("ANTLER_CASE_PROBE").is_ok(), cfg!(windows));
    let env = process_ssh_env();
    assert_eq!(env.get("GIT_SSH_COMMAND").map(String::as_str), Some(PARENT_SSH), "부모의 `{}` 를 찾지 못했다", if cfg!(windows) { "Git_Ssh_Command" } else { "GIT_SSH_COMMAND" });
    let ins = inspect_entries(&[]);
    assert_eq!(fetch_env_for(&ins, "local", &env, cfg!(windows), false).ssh_command, PARENT_SSH);
    assert_eq!(fetch_env_for(&ins, "wsl", &env, cfg!(windows), false).ssh_command, SSH_COMMAND_DEFAULT);
    let src = code_only("src/git/fetch/run.rs");
    assert!(src.contains("std::env::var(") && !src.contains("env::vars"), "run.rs 는 이름을 정해 std::env::var 로 읽어야 한다");
    // 이름 조회 함수는 주어진 것만 읽는다
    assert!(ssh_env(|_| None).is_empty());
    // ② 상속: 부모에 `Git_Pager` 가 있어도 자식 git 의 GIT_PAGER 는 실행기가 정한 cat 이다(한 이름으로 합쳐져 우리 값이 이긴다)
    let c = Case::new();
    let r = local().run(&c.repo_str(), &["-c", ENV_ALIAS, "antlerenv"], &RunOpts::default()).await.unwrap();
    assert_eq!(parse_env(&r.stdout).get("GIT_PAGER").map(String::as_str), Some("cat"));
    // ③ fetch 가 정한 GIT_SSH_COMMAND 가 부모의 같은 이름(대소문자만 다름)을 이긴다
    let fe = FetchEnv { allow_file: false, ssh_command: "ssh -o OurValue=1".into() };
    let r = local().run(&c.repo_str(), &["-c", ENV_ALIAS, "antlerenv"], &RunOpts { fetch_env: Some(fe), ..RunOpts::default() }).await.unwrap();
    assert_eq!(parse_env(&r.stdout).get("GIT_SSH_COMMAND").map(String::as_str), Some("ssh -o OurValue=1"));
}

// ---------------------------------------------------------------------------------------------------------------------
// WSL 통합 — 배포판이 없으면(CI 러너 포함) 건너뛴다
// ---------------------------------------------------------------------------------------------------------------------

#[cfg(not(windows))]
#[test]
fn wsl_통합_windows_가_아니면_건너뛴다() {
    eprintln!("[wsl] 건너뜀: Windows 가 아님");
}

#[cfg(windows)]
mod wsl {
    use super::*;
    use std::time::{Duration, Instant};

    fn output_with_timeout(mut cmd: std::process::Command, limit: Duration) -> Option<std::process::Output> {
        use std::process::Stdio;
        let mut child = cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().ok()?;
        let deadline = Instant::now() + limit;
        loop {
            match child.try_wait().ok()? {
                Some(_) => return child.wait_with_output().ok(),
                None if Instant::now() >= deadline => {
                    let _ = child.kill();
                    return None;
                }
                None => std::thread::sleep(Duration::from_millis(50)),
            }
        }
    }

    /// 배포판 안에서 셸 스크립트를 실행한다(준비·검사용). 배포판의 사용자 git 설정이 준비 커밋에 끼지 않게 격리한다
    pub fn wsl_sh(distro: &str, script: &str) -> (bool, String) {
        let mut c = raw("wsl.exe");
        let script = format!("export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1 GIT_AUTHOR_NAME=t GIT_AUTHOR_EMAIL=t@t GIT_COMMITTER_NAME=t GIT_COMMITTER_EMAIL=t@t; {script}");
        c.args(["-d", distro, "--exec", "sh", "-c", &script]).env("WSL_UTF8", "1");
        match output_with_timeout(c, Duration::from_secs(60)) {
            Some(o) => (o.status.success(), String::from_utf8_lossy(&o.stdout).trim().to_string()),
            None => (false, "시간 초과".to_string()),
        }
    }

    fn first_distro() -> Option<String> {
        let mut c = raw("wsl.exe");
        c.args(["-l", "-q"]).env("WSL_UTF8", "1");
        let o = output_with_timeout(c, Duration::from_secs(8))?;
        if !o.status.success() {
            return None;
        }
        let s: String = String::from_utf8_lossy(&o.stdout).chars().filter(|&c| c != '\0' && c != '\r' && c != '\u{FEFF}').collect();
        s.lines().map(str::trim).find(|l| !l.is_empty() && !l.starts_with("docker-desktop")).map(str::to_string)
    }

    pub struct Fixture {
        pub distro: String,
        /// `/tmp/antler-wsl-fetch.XXXXXX`: remote.git · work · repo
        pub dir: String,
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            if self.dir.starts_with("/tmp/antler-wsl-fetch.") {
                wsl_sh(&self.distro, &format!("rm -rf '{}'", self.dir));
            }
        }
    }

    impl Fixture {
        pub fn new() -> Option<Fixture> {
            let Some(distro) = first_distro() else {
                eprintln!("[wsl] 건너뜀: 사용 가능한 WSL 배포판이 없음");
                return None;
            };
            if !wsl_sh(&distro, "command -v git").0 {
                eprintln!("[wsl] 건너뜀: {distro} 에 git 이 없음");
                return None;
            }
            let script = [
                "set -e",
                "d=$(mktemp -d /tmp/antler-wsl-fetch.XXXXXX)",
                "cd \"$d\"",
                "git init -q --bare -b main remote.git",
                "git clone -q remote.git work 2>/dev/null",
                "cd work",
                "git checkout -q -b main",
                "git commit -q --allow-empty -m first",
                "git push -q origin main",
                "cd ..",
                "git clone -q remote.git repo",
                "echo \"$d\"",
            ]
            .join("\n");
            let (ok, dir) = wsl_sh(&distro, &script);
            if !ok || !dir.starts_with("/tmp/antler-wsl-fetch.") {
                eprintln!("[wsl] 건너뜀: 테스트 저장소 생성 실패: {dir}");
                return None;
            }
            eprintln!("[wsl] 배포판 {distro}, 저장소 {dir}");
            Some(Fixture { distro, dir })
        }

        pub fn repo(&self) -> String {
            format!("{}/repo", self.dir)
        }

        pub fn sh(&self, script: &str) -> (bool, String) {
            wsl_sh(&self.distro, script)
        }

        pub fn exec(&self) -> GitExecutor {
            GitExecutor::Wsl(WslExecutor::new(&self.distro))
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn wsl_통합_fetch_는_dev_null_훅_경로로_훅을_막고_ref_를_갱신한다() {
        let Some(f) = Fixture::new() else { return };
        let (repo, exec) = (f.repo(), f.exec());
        let ins = inspect_config(&exec, &repo, None).await.expect("WSL 설정 검사");
        assert!(ins.has_remote && ins.dangerous.is_empty(), "{:?}", ins.dangerous);
        assert_eq!(ins.remote_names, ["origin"]);

        let hook = format!("{}/.git/hooks/reference-transaction", repo);
        let marker = format!("{}/PWNED", f.dir);
        let (ok, _) = f.sh(&format!("printf '#!/bin/sh\\necho x > \"{marker}\"\\n' > '{hook}' && chmod +x '{hook}'"));
        assert!(ok);
        let (_, sha) = f.sh(&format!("cd '{}/work' && git commit -q --allow-empty -m second && git push -q origin main && git rev-parse HEAD", f.dir));
        // 로컬용 no-hooks 경로(존재하지 않는 Windows 경로)를 줘도 WSL 은 /dev/null 을 쓴다 — 거부하지도, 그 경로를 쓰지도 않는다
        let opts = FetchRunOpts { cancel: None, no_hooks_dir: Some("C:/Users/없는/no-hooks"), allow_file: true, credential_helpers: false };
        run_fetch(&exec, &repo, &ins, &opts).await.expect("WSL fetch");
        assert_eq!(f.sh(&format!("git -C '{repo}' rev-parse origin/main")).1, sha);
        assert!(f.sh(&format!("test -e '{marker}' && echo y || echo n")).1 == "n", "WSL 의 훅이 실행됐다");
        // 대조: -c core.hooksPath 없이 직접 fetch 하면 훅이 실행된다
        f.sh(&format!("git -C '{repo}' update-ref -d refs/remotes/origin/main && git -C '{repo}' fetch -q origin"));
        assert_eq!(f.sh(&format!("test -e '{marker}' && echo y || echo n")).1, "y", "대조군에서 훅이 실행되지 않았다 — 위 검증이 의미 없다");
        // 훅 경로가 실제로 /dev/null 로 풀린다
        let mut args = fetch_config_for("wsl", None, true, false).unwrap();
        args.extend(["rev-parse", "--git-path", "hooks/reference-transaction"].map(String::from));
        let r = exec.run(&repo, &args, &RunOpts::default()).await.unwrap();
        assert!(String::from_utf8_lossy(&r.stdout).trim().starts_with("/dev/null"), "{:?}", String::from_utf8_lossy(&r.stdout));
        // 같은 환경 상속: WSL 의 git 은 Windows 의 GIT_SSH_COMMAND(부모 환경)를 보지 않는다
        let ins = inspect_entries(&[]);
        assert_eq!(fetch_env_for(&ins, "wsl", &process_ssh_env(), true, false).ssh_command, SSH_COMMAND_DEFAULT);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn 위험9_git_askpass_빈_값이_wsl_git_에_빈_값으로_도착한다() {
        let Some(f) = Fixture::new() else { return };
        let (repo, exec) = (f.repo(), f.exec());
        // ① 환경: unset 이 아니라 빈 값으로 간다
        let r = exec.run(&repo, &["-c", ENV_ALIAS, "antlerenv"], &fetch_opts(false)).await.unwrap();
        assert_eq!(r.code, 0, "{}", r.stderr);
        assert_eq!(
            parse_env(&r.stdout),
            env_map(&[("GIT_SSH_COMMAND", SSH_COMMAND_DEFAULT), ("GIT_ALLOW_PROTOCOL", "https:http:ssh:git"), ("GIT_ASKPASS", ""), ("GIT_PAGER", "cat")]),
            "WSL: GIT_ASKPASS 가 빈 값으로 도착해야 한다(줄이 없으면 unset 이다)"
        );
        // ② 효과: core.askpass 가 실행되지 않는다
        let marker = format!("{}/ASKED", f.dir);
        // git 은 askpass 를 셸 없이 프로그램으로 실행한다 — 마커를 만드는 실행 파일을 준다
        let script = format!("{}/askpass.sh", f.dir);
        let (ok, _) = f.sh(&format!("printf '#!/bin/sh\\necho x > \"{marker}\"\\necho user\\n' > '{script}' && chmod +x '{script}'"));
        assert!(ok);
        let askpass = format!("core.askpass={script}");
        let r = exec.run(&repo, &["-c", &askpass, "-c", ASKPASS_ALIAS, "antlerprobe"], &fetch_opts(false)).await.unwrap();
        assert_ne!(r.code, 0);
        assert!(r.stderr.contains("terminal prompts disabled"), "프롬프트까지 가지 못했다: {}", r.stderr);
        assert_eq!(f.sh(&format!("test -e '{marker}' && echo y || echo n")).1, "n", "WSL: GIT_ASKPASS='' 인데 core.askpass 가 실행됐다");
        // 대조: fetch 환경 없이는 실행된다
        let _ = exec.run(&repo, &["-c", &askpass, "-c", ASKPASS_ALIAS, "antlerprobe"], &RunOpts::default()).await.unwrap();
        assert_eq!(f.sh(&format!("test -e '{marker}' && echo y || echo n")).1, "y", "대조군에서 askpass 가 실행되지 않았다 — 이 테스트는 아무것도 증명하지 못한다");
    }
}
