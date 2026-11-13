//! 실행기(`src/git/exec`) 통합 테스트 — 실제 git 을 띄운다. TS 의 `tests/executor.test.ts`·`hostileRepo.test.ts`·
//! `wslExecutor.test.ts` 이식과 TS 골든(`tests/golden/exec/env.json`) 패리티.
//!
//! 격리: 이 테스트 바이너리가 시작될 때(`isolate`, 어떤 테스트보다 먼저) `GIT_CONFIG_GLOBAL`(커밋된 `src-tauri/tests/gitconfig`)·
//! `GIT_CONFIG_NOSYSTEM`·`GCM_INTERACTIVE` 를 프로세스 환경에 넣어 사용자의 실제 git 설정을 읽지 않는다(`격리` 테스트가 확인한다).
//! `.cargo/config.toml` 의 `[env]` 로 하지 않는 이유: 그 설정은 `cargo run`·`tauri dev` 에도 걸려 개발 중인 앱의 git 이
//! 사용자 전역 설정(WSL 경로의 safe.directory, 자격 증명 도우미)을 못 보게 된다.
//! 수동 테스트(`--ignored`): 트리 kill 은 `ANTLER_BIG_REPO=<큰 저장소>` 가 있으면 거기서, 없으면 이 저장소에서 돈다.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use antler_lib::git::exec::{
    build_git_args, wsl_argv, ExecError, GitExecutor, GitStream, LocalExecutor, RunOpts, WslExecutor, GIT_ENV, GIT_GLOBAL_ARGS,
    WSL_SPAWN_ENV,
};
use antler_lib::git::parsers::commit::COMMIT_FORMAT_ARGS;
use antler_lib::git::parsers::log::LOG_FORMAT_ARGS;
use antler_lib::git::parsers::refs::REFS_FORMAT;
use antler_lib::git::parsers::stash::STASH_ARGS;
use antler_lib::git::parsers::wip_status::WIP_STATUS_ARGS;
use antler_lib::git::parsers::worktree::WORKTREE_ARGS;
use serde_json::Value;
use tokio::io::AsyncReadExt;
use tokio_util::sync::CancellationToken;

/// 테스트 프로세스 환경 격리. 실행기는 부모 환경을 상속하므로 여기서 넣은 값이 모든 git 에 간다.
/// ctor 로 main 전에 한 번 돌아 테스트 스레드와 경합하지 않는다(set_var 는 다른 스레드가 환경을 읽는 중이면 위험하다).
// unsafe: main 이전 코드는 Rust 런타임 초기화 전에 돈다 — 여기서는 환경 변수만 넣고 다른 것에 기대지 않는다
#[ctor::ctor(unsafe)]
fn isolate() {
    let global = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("gitconfig");
    std::env::set_var("GIT_CONFIG_GLOBAL", global);
    std::env::set_var("GIT_CONFIG_NOSYSTEM", "1");
    std::env::set_var("GCM_INTERACTIVE", "never");
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().expect("src-tauri 의 상위").to_path_buf()
}

fn root_str() -> String {
    repo_root().to_string_lossy().into_owned()
}

/// 테스트 준비·대조용 날 프로세스. **실행기를 거치지 않는 것**이 목적이다 — 방어 없이 돌려 보는 대조군, 저장소 만들기,
/// 프로세스 목록 조회. 제품 코드는 이 길을 쓰지 않는다(clippy.toml 의 disallowed-methods).
#[allow(clippy::disallowed_methods)]
fn raw(program: &str) -> std::process::Command {
    std::process::Command::new(program)
}

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        static N: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!("antler-exec-{tag}-{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
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

/// 설정을 직접 읽는 날 git(저장소 준비용). 전역 설정은 격리돼 있다
fn raw_git(dir: &Path, args: &[&str]) -> String {
    let out = raw("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@t"])
        .args(args)
        .output()
        .expect("git 실행");
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn init_repo(dir: &Path) {
    let out = raw("git").args(["init", "-q", "-b", "main"]).arg(dir).output().expect("git init");
    assert!(out.status.success());
}

fn local() -> LocalExecutor {
    LocalExecutor::default()
}

fn opts() -> RunOpts {
    RunOpts::default()
}

/// 스트림을 EOF 까지 읽고 종료 코드를 확인한다(TS 의 `for await` 소비)
async fn read_all(mut st: GitStream) -> Result<Vec<u8>, ExecError> {
    let mut v = Vec::new();
    if st.read_to_end(&mut v).await.is_err() {
        return Err(st.finish().await.expect_err("읽기 오류 뒤 finish 는 실패한다"));
    }
    st.finish().await?;
    Ok(v)
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).trim().to_string()
}

// ---------------------------------------------------------------------------------------------------------------------
// 골든 패리티: tests/golden/exec/env.json (TS 가 만든다)
// ---------------------------------------------------------------------------------------------------------------------

fn golden() -> Value {
    let path = repo_root().join("tests/golden/exec/env.json");
    serde_json::from_str(&fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))).expect("골든 JSON")
}

fn strs(v: &Value) -> Vec<String> {
    v.as_array().expect("배열").iter().map(|x| x.as_str().expect("문자열").to_string()).collect()
}

fn pairs(v: &Value) -> Vec<(String, String)> {
    v.as_array()
        .expect("배열")
        .iter()
        .map(|p| (p[0].as_str().expect("키").to_string(), p[1].as_str().expect("값").to_string()))
        .collect()
}

fn own_env() -> Vec<(String, String)> {
    GIT_ENV.iter().map(|(k, v)| ((*k).to_string(), (*v).to_string())).collect()
}

#[test]
fn 골든_전역_인자와_환경이_ts_와_같다() {
    let g = golden();
    assert_eq!(strs(&g["globalArgs"]), GIT_GLOBAL_ARGS, "GIT_GLOBAL_ARGS 가 TS 와 다르다");
    // 순서까지 같아야 한다(WSL argv 의 env K=V 순서)
    assert_eq!(pairs(&g["env"]), own_env(), "GIT_ENV 가 TS 와 다르다");
}

#[test]
fn 골든_로컬_실행기_argv_와_환경() {
    let g = golden();
    assert_eq!(g["local"]["command"], "git");
    assert_eq!(strs(&g["local"]["argv"]), build_git_args("/r", &["log"]));
    // TS 로컬 실행기가 spawn 에 준 값 = 우리 상수
    assert_eq!(pairs(&g["local"]["env"]), own_env());
}

#[test]
fn 골든_wsl_argv_와_wsl_exe_환경() {
    let g = golden();
    assert_eq!(g["wsl"]["command"], "wsl.exe");
    assert_eq!(strs(&g["wsl"]["argv"]), wsl_argv("Ubuntu", "/home/me/repo", &["log", "-z"]));
    let own: Vec<(&str, &str)> = WSL_SPAWN_ENV.to_vec();
    assert_eq!(own, [("WSL_UTF8", g["wsl"]["wslUtf8"].as_str().unwrap())]);
    let samples = g["samples"].as_array().unwrap();
    assert!(samples.len() >= 4);
    for s in samples {
        let (distro, repo, args) = (s["distro"].as_str().unwrap(), s["repoPath"].as_str().unwrap(), strs(&s["args"]));
        assert_eq!(strs(&s["buildGitArgs"]), build_git_args(repo, &args), "buildGitArgs {repo}");
        assert_eq!(strs(&s["wslArgv"]), wsl_argv(distro, repo, &args), "wslArgv {distro} {repo}");
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// 격리
// ---------------------------------------------------------------------------------------------------------------------

#[tokio::test]
async fn 격리_테스트용_전역_설정만_읽는다() {
    // 격리 없이 돌면(isolate 가 안 돌면) 사용자의 ~/.gitconfig 를 조용히 읽는 대신 여기서 실패한다
    let global = std::env::var("GIT_CONFIG_GLOBAL").expect("GIT_CONFIG_GLOBAL 이 설정돼야 한다(isolate)");
    assert!(global.replace('\\', "/").ends_with("src-tauri/tests/gitconfig"), "{global}");
    assert_eq!(std::env::var("GIT_CONFIG_NOSYSTEM").as_deref(), Ok("1"));
    assert_eq!(std::env::var("GCM_INTERACTIVE").as_deref(), Ok("never"));

    // 실행기가 띄운 git 이 실제로 그 파일만 본다: 값의 출처가 테스트용 파일이고, 사용자의 식별자·alias·safe.directory 가 안 보인다
    let r = local().run(&root_str(), &["config", "--show-origin", "--get-all", "user.name"], &opts()).await.unwrap();
    // show-origin 은 경로의 역슬래시를 `\\` 로 이스케이프한다
    let out = text(&r.stdout).replace("\\\\", "/").replace('\\', "/");
    assert!(out.contains("src-tauri/tests/gitconfig") && out.ends_with("Antler Test"), "{out}");
    let all = local().run(&root_str(), &["config", "--global", "--list"], &opts()).await.unwrap();
    assert_eq!(text(&all.stdout), "user.name=Antler Test\nuser.email=test@antler.invalid");
    // (`config --system` 은 GIT_CONFIG_NOSYSTEM 을 무시하고 파일을 직접 읽으므로 쓰지 않는다 — 실제로 읽힌 범위를 본다)
    let scopes = local().run(&root_str(), &["config", "--show-scope", "--list"], &opts()).await.unwrap();
    for line in text(&scopes.stdout).lines() {
        let scope = line.split_whitespace().next().unwrap_or("");
        assert!(matches!(scope, "global" | "local" | "worktree" | "command"), "시스템·사용자 범위 설정이 읽혔다: {line}");
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// executor.test.ts — LocalExecutor
// ---------------------------------------------------------------------------------------------------------------------

#[tokio::test]
async fn 저장소_안에서_git_을_실행한다() {
    let r = local().run(&root_str(), &["rev-parse", "--is-inside-work-tree"], &opts()).await.unwrap();
    assert_eq!(r.code, 0);
    assert_eq!(text(&r.stdout), "true");
}

#[tokio::test]
async fn 실패_코드를_그대로_돌려준다() {
    let r = local().run(&root_str(), &["rev-parse", "--verify", "refs/heads/__없는_브랜치__"], &opts()).await.unwrap();
    assert_ne!(r.code, 0);
}

#[tokio::test]
async fn stream_은_실패하면_finish_가_git_오류를_돌려준다() {
    let st = local().stream(&root_str(), &["log", "refs/heads/__없는_브랜치__"], &opts()).unwrap();
    let err = read_all(st).await.unwrap_err();
    let ExecError::Git { code, stderr, .. } = &err else { panic!("Git 오류여야 한다: {err:?}") };
    assert_ne!(*code, 0);
    assert!(stderr.contains("__없는_브랜치__"), "{stderr}");
    assert_eq!(err.envelope_code(), "GIT");
}

#[tokio::test]
async fn 이미_취소된_signal_이면_aborted() {
    let t = CancellationToken::new();
    t.cancel();
    let o = RunOpts::with_cancel(t);
    assert_eq!(local().run(&root_str(), &["status"], &o).await.unwrap_err(), ExecError::Aborted);
    assert!(matches!(local().stream(&root_str(), &["status"], &o), Err(ExecError::Aborted)));
}

#[tokio::test]
async fn stream_이_출력을_돌려주고_정상_종료한다() {
    let st = local().stream(&root_str(), &["rev-parse", "--is-inside-work-tree"], &opts()).unwrap();
    assert_eq!(text(&read_all(st).await.unwrap()), "true");
}

#[tokio::test]
async fn git_바이너리가_없으면_spawn_실패는_error_코드다() {
    let err = LocalExecutor::new("antler-없는-git").run(&root_str(), &["status"], &opts()).await.unwrap_err();
    assert!(matches!(err, ExecError::SpawnFailed { .. }), "{err:?}");
    assert_eq!(err.envelope_code(), "ERROR");
}

#[tokio::test]
async fn 한글_경로와_출력이_깨지지_않는다() {
    let t = TempDir::new("kr");
    let repo = t.path().join("내 저장소");
    init_repo(&repo);
    fs::write(repo.join("a b.txt"), "x").unwrap();
    raw_git(&repo, &["add", "."]);
    raw_git(&repo, &["commit", "-q", "-m", "첫 커밋: 한글 메시지"]);
    let repo = repo.to_string_lossy().into_owned();
    let r = local().run(&repo, &["log", "-z", "--format=%s"], &opts()).await.unwrap();
    assert_eq!(String::from_utf8_lossy(&r.stdout).split('\0').next().unwrap(), "첫 커밋: 한글 메시지");
    // core.quotepath=false: 한글 파일명이 따옴표·8진수로 안 나온다
    fs::write(Path::new(&repo).join("한글.txt"), "y").unwrap();
    let r = local().run(&repo, &["status", "--porcelain"], &opts()).await.unwrap();
    assert!(text(&r.stdout).contains("한글.txt"), "{}", text(&r.stdout));
}

#[tokio::test]
async fn stdout_상한을_넘으면_트리를_죽이고_git_오류() {
    let o = RunOpts { cancel: None, max_buffer: Some(1000) };
    let err = local().run(&root_str(), &["log", "-p", "--root", "--format=%H"], &o).await.unwrap_err();
    let ExecError::Git { message, code, .. } = err else { panic!("Git 오류여야 한다") };
    assert_eq!(code, -1);
    assert_eq!(message, "output exceeded 1000 bytes");
}

#[tokio::test]
async fn stderr_는_앞_4mb_만_담고_파이프는_끝까지_비운다() {
    // 셸 alias 로 5MB 를 stderr 에 쓴다. 파이프를 안 비우면 git 이 멈춰 이 테스트는 타임아웃된다
    let alias = "alias.noisy=!head -c 5000000 /dev/zero | tr '\\0' x >&2";
    let r = tokio::time::timeout(Duration::from_secs(60), local().run(&root_str(), &["-c", alias, "noisy"], &opts()))
        .await
        .expect("stderr 파이프가 비워지지 않아 멈췄다")
        .unwrap();
    assert_eq!(r.stderr.len(), antler_lib::git::exec::MAX_STDERR);
    assert!(r.stderr.bytes().all(|b| b == b'x'));
}

/// 출력 없이 오래 블록하는 명령: git 셸 alias 가 sleep 을 실행한다
const SLEEP_ARGS: [&str; 3] = ["-c", "alias.antlersleep=!sleep 30", "antlersleep"];

#[tokio::test]
async fn 실행_중_취소하면_바로_aborted_로_끝난다() {
    let t = CancellationToken::new();
    let o = RunOpts::with_cancel(t.clone());
    let canceller = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(700)).await;
        t.cancel();
        Instant::now()
    });
    let err = local().run(&root_str(), &SLEEP_ARGS, &o).await.unwrap_err();
    let done = Instant::now();
    let cancelled_at = canceller.await.unwrap();
    assert_eq!(err, ExecError::Aborted);
    // 취소 → 반환만 잰다(spawn 지연은 뺀다). sleep 30 이 끝나길 기다렸다면 30초 가까이 걸린다
    let latency = done.saturating_duration_since(cancelled_at);
    eprintln!("[cancel] 취소 -> Aborted 반환 {latency:?}");
    assert!(latency < Duration::from_secs(10), "취소 뒤 반환이 {latency:?} 걸렸다");
}

#[tokio::test]
async fn stream_취소는_읽기_오류와_aborted_가_된다() {
    let t = CancellationToken::new();
    let o = RunOpts::with_cancel(t.clone());
    let mut st = local().stream(&root_str(), &SLEEP_ARGS, &o).unwrap();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(500)).await;
        t.cancel();
    });
    let mut buf = [0u8; 16];
    let read = tokio::time::timeout(Duration::from_secs(15), st.read(&mut buf)).await.expect("취소가 읽기를 깨우지 못했다");
    assert!(read.is_err());
    assert_eq!(st.finish().await.unwrap_err(), ExecError::Aborted);
}

// ---------------------------------------------------------------------------------------------------------------------
// backpressure
// ---------------------------------------------------------------------------------------------------------------------

/// 출력이 파이프 버퍼보다 훨씬 큰 명령
const BIG_OUT: [&str; 4] = ["log", "-p", "--root", "--format=%H"];

#[tokio::test]
async fn 스트림을_읽지_않으면_git_이_멈추고_읽으면_끝난다() {
    let mut st = local().stream(&root_str(), &BIG_OUT, &opts()).unwrap();
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert!(st.is_running(), "읽지 않는데 git 이 끝났다 — 출력을 미리 읽어 두고 있다(backpressure 없음)");
    // 읽기 시작하면 끝까지 흘러 정상 종료한다
    let mut total = 0usize;
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = st.read(&mut buf).await.unwrap();
        if n == 0 {
            break;
        }
        total += n;
    }
    st.finish().await.unwrap();
    assert!(total > 256 * 1024, "테스트 저장소의 출력이 파이프 버퍼보다 작다({total} 바이트) — backpressure 를 검증하지 못한다");
}

// ---------------------------------------------------------------------------------------------------------------------
// hostileRepo.test.ts
// ---------------------------------------------------------------------------------------------------------------------

const MISSING: &str = "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef";

struct Hostile {
    _root: TempDir,
    repo: PathBuf,
    marker: PathBuf,
}

/// 설정에 심은 명령이 실행되면 마커 파일이 생긴다(git 이 셸로 실행하는 설정 값. 무해한 `echo` 만 쓴다)
fn hostile() -> Hostile {
    let root = TempDir::new("hostile");
    let repo = root.path().join("repo");
    let marker = root.path().join("PWNED");
    init_repo(&repo);
    raw_git(&repo, &["commit", "-q", "--allow-empty", "-m", "ok"]);
    let touch = format!("echo x > '{}'", marker.to_string_lossy().replace('\\', "/"));
    // partial clone(promisor) 저장소처럼 꾸미고, 없는 객체를 가리키는 ref 를 심는다 → 조회 시 lazy fetch 유도
    for (k, v) in [
        ("core.repositoryformatversion", "1"),
        ("extensions.partialClone", "origin"),
        ("remote.origin.promisor", "true"),
        ("remote.origin.url", "ssh://example.invalid/x"),
        ("core.sshCommand", touch.as_str()),
        ("remote.origin.uploadpack", touch.as_str()),
        ("core.fsmonitor", touch.as_str()),
    ] {
        raw_git(&repo, &["config", k, v]);
    }
    // update-ref 는 없는 객체를 거부하므로 loose ref 파일을 직접 쓴다
    fs::write(repo.join(".git/refs/heads/evil"), format!("{MISSING}\n")).unwrap();
    Hostile { _root: root, repo, marker }
}

/// 방어 없이(또는 일부 방어만으로) 날 git 을 돌려 마커가 생기는지 본다
fn probe(h: &Hostile, extra_env: &[(&str, &str)]) -> bool {
    let _ = fs::remove_file(&h.marker);
    let _ = raw("git")
        .arg("-C")
        .arg(&h.repo)
        .args(["for-each-ref", "--format=%(objecttype)"])
        .env("GIT_TERMINAL_PROMPT", "0")
        .envs(extra_env.iter().copied())
        .output();
    h.marker.exists()
}

#[test]
fn 악성_방어가_없으면_실행된다_재현_확인() {
    // 이게 거짓이면 아래 "마커 없음" 검사가 공허하다(페이로드가 이 환경에서 동작하지 않는 것)
    assert!(probe(&hostile(), &[]), "방어가 없는데도 마커가 안 생겼다 — 페이로드가 동작하지 않는다");
}

#[test]
fn allow_protocol_none_만으로_막힌다() {
    // 옛 git(WSL 2.43 등)은 GIT_NO_LAZY_FETCH 를 모르므로 GIT_ALLOW_PROTOCOL 만으로도 막혀야 한다
    assert!(!probe(&hostile(), &[("GIT_ALLOW_PROTOCOL", "none")]));
}

#[test]
fn no_lazy_fetch_만으로_막힌다() {
    assert!(!probe(&hostile(), &[("GIT_NO_LAZY_FETCH", "1")]));
}

#[tokio::test]
async fn 악성_설정_명령이_ref_조회_log_상세_wip_중_어느_것에서도_실행되지_않는다() {
    let h = hostile();
    let repo = h.repo.to_string_lossy().into_owned();
    let e = local();
    let o = opts();
    // refs·refsSignature
    let refs_format = format!("--format={REFS_FORMAT}");
    for args in [
        // probe() 의 대조군과 같은 argv: 방어 없이는 마커가 생기는 명령이다
        vec!["for-each-ref", "--format=%(objecttype)"],
        vec!["for-each-ref", refs_format.as_str(), "refs/heads", "refs/remotes", "refs/tags"],
        vec!["for-each-ref", "--format=%(objectname) %(refname)"],
        vec!["symbolic-ref", "--quiet", "HEAD"],
        vec!["rev-parse", "--verify", "--quiet", "HEAD"],
        vec!["symbolic-ref", "--quiet", "refs/remotes/origin/HEAD"],
        vec!["remote"],
    ] {
        let _ = e.run(&repo, &args, &o).await;
    }
    let _ = e.run(&repo, &WORKTREE_ARGS, &o).await;
    let _ = e.run(&repo, &STASH_ARGS, &o).await;
    // logArgs(undefined, 0)
    let log: Vec<&str> = ["log"].into_iter().chain(LOG_FORMAT_ARGS).chain(["--date-order", "--"]).collect();
    let _ = e.run(&repo, &log, &o).await;
    // commitDetail(MISSING)
    let detail: Vec<&str> = ["log", "-1"].into_iter().chain(COMMIT_FORMAT_ARGS).chain([MISSING, "--"]).collect();
    let _ = e.run(&repo, &detail, &o).await;
    // compareCommits(MISSING, MISSING): changedFiles 의 두 모드
    for mode in ["--name-status", "--numstat"] {
        let _ = e.run(&repo, &["diff-tree", "-r", "-z", "-M", "--no-commit-id", "--no-ext-diff", mode, MISSING, MISSING, "--"], &o).await;
    }
    let cat = e.run(&repo, &["cat-file", "-t", MISSING], &o).await.unwrap();
    assert_ne!(cat.code, 0, "없는 객체 조회가 성공했다");
    // 작업 트리 조회(WIP)도 같은 설정을 읽는다: filterOverrides + status
    let _ = e.run(&repo, &["config", "-z", "--get-regexp", "^filter\\..+\\.(clean|smudge|process)$"], &o).await;
    if let Ok(st) = e.stream(&repo, &WIP_STATUS_ARGS, &o) {
        let _ = read_all(st).await;
    }
    assert!(!h.marker.exists(), "악성 저장소 설정의 명령이 실행됐다");
}

#[tokio::test]
async fn refs_replace_로_위조한_이력을_보여_주지_않는다() {
    let root = TempDir::new("replace");
    let dir = root.path().join("replace");
    init_repo(&dir);
    raw_git(&dir, &["commit", "-q", "--allow-empty", "-m", "fake history"]);
    let fake = raw_git(&dir, &["rev-parse", "HEAD"]);
    raw_git(&dir, &["commit", "-q", "--allow-empty", "-m", "real history"]);
    let real = raw_git(&dir, &["rev-parse", "HEAD"]);
    raw_git(&dir, &["replace", &real, &fake]); // real 을 가리키는 모든 조회가 fake 로 보이게 된다
    // 대조: 보호 없이는 위조된 제목이 보인다
    assert_eq!(raw_git(&dir, &["log", "-1", "--format=%s", &real]), "fake history");
    // 실행기(GIT_ENV 에 GIT_NO_REPLACE_OBJECTS=1)는 실제 이력을 보여 준다
    let r = local().run(&dir.to_string_lossy(), &["log", "-1", "--format=%s", &real], &opts()).await.unwrap();
    assert_eq!(text(&r.stdout), "real history");
}

// ---------------------------------------------------------------------------------------------------------------------
// 트리 kill(수동): cargo test --test exec -- --ignored --nocapture
// ---------------------------------------------------------------------------------------------------------------------

/// pid → 부모 pid 스냅샷(Windows)
#[cfg(windows)]
fn process_snapshot() -> Vec<(u32, u32)> {
    let out = raw("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", "Get-CimInstance Win32_Process | ForEach-Object { \"$($_.ProcessId),$($_.ParentProcessId)\" }"])
        .output()
        .expect("powershell 실행");
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| {
            let (a, b) = l.trim().split_once(',')?;
            Some((a.parse().ok()?, b.parse().ok()?))
        })
        .collect()
}

/// root 와 그 자손 pid
#[cfg(windows)]
fn tree_of(root: u32, snap: &[(u32, u32)]) -> Vec<u32> {
    let mut tree = vec![root];
    let mut i = 0;
    while i < tree.len() {
        let parent = tree[i];
        for &(pid, ppid) in snap {
            if ppid == parent && !tree.contains(&pid) {
                tree.push(pid);
            }
        }
        i += 1;
    }
    tree
}

fn big_repo() -> String {
    match std::env::var("ANTLER_BIG_REPO") {
        Ok(p) if !p.is_empty() => p,
        _ => root_str(),
    }
}

#[cfg(windows)]
async fn assert_tree_dies(tree: &[u32]) {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let snap = process_snapshot();
        let alive: Vec<u32> = tree.iter().copied().filter(|p| snap.iter().any(|(pid, _)| pid == p)).collect();
        if alive.is_empty() {
            return;
        }
        assert!(Instant::now() < deadline, "git 트리가 남았다: {alive:?}");
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
}

/// 스트림을 열고 조금 읽다 버리면(Drop) git.exe 자손이 남지 않는다
#[cfg(windows)]
#[tokio::test]
#[ignore = "수동: 큰 저장소(ANTLER_BIG_REPO)에서 git log 스트림을 열고 조금 읽다 버린다. powershell 로 프로세스 트리를 본다"]
async fn 트리_kill_스트림을_버리면_git_자손이_남지_않는다() {
    let mut st = local().stream(&big_repo(), &BIG_OUT, &opts()).unwrap();
    let mut buf = vec![0u8; 64 * 1024];
    assert!(st.read(&mut buf).await.unwrap() > 0);
    let pid = st.pid().expect("pid");
    let tree = tree_of(pid, &process_snapshot());
    eprintln!("[tree-kill/drop] repo={} root pid={pid} tree={tree:?}", big_repo());
    assert!(st.is_running());
    drop(st);
    assert_tree_dies(&tree).await;
    eprintln!("[tree-kill/drop] 트리 {}개 프로세스가 모두 종료됨", tree.len());
}

/// 취소 신호로 끊어도 마찬가지다
#[cfg(windows)]
#[tokio::test]
#[ignore = "수동: 위와 같고 취소 신호로 끊는다"]
async fn 트리_kill_취소하면_git_자손이_남지_않는다() {
    let t = CancellationToken::new();
    let mut st = local().stream(&big_repo(), &BIG_OUT, &RunOpts::with_cancel(t.clone())).unwrap();
    let mut buf = vec![0u8; 64 * 1024];
    assert!(st.read(&mut buf).await.unwrap() > 0);
    let pid = st.pid().expect("pid");
    let tree = tree_of(pid, &process_snapshot());
    eprintln!("[tree-kill/cancel] repo={} root pid={pid} tree={tree:?}", big_repo());
    t.cancel();
    assert!(st.read(&mut buf).await.is_err());
    assert_eq!(st.finish().await.unwrap_err(), ExecError::Aborted);
    assert_tree_dies(&tree).await;
    eprintln!("[tree-kill/cancel] 트리 {}개 프로세스가 모두 종료됨", tree.len());
}

/// 출력 없이 블록하는 손자(셸·sleep)까지 죽는다. 파이프가 닫혀서 죽는 경우(SIGPIPE 류)를 배제하려고 아무것도 쓰지 않는 명령을 쓴다
#[cfg(windows)]
#[tokio::test]
#[ignore = "수동: 출력 없이 sleep 하는 git alias 의 프로세스 트리(git 래퍼·git.exe·sh·sleep)가 취소로 모두 사라지는지"]
async fn 트리_kill_출력_없는_손자까지_죽는다() {
    let t = CancellationToken::new();
    let mut st = local().stream(&root_str(), &SLEEP_ARGS, &RunOpts::with_cancel(t.clone())).unwrap();
    let pid = st.pid().expect("pid");
    let mut tree = vec![pid];
    for _ in 0..40 {
        tokio::time::sleep(Duration::from_millis(250)).await;
        tree = tree_of(pid, &process_snapshot());
        if tree.len() >= 4 {
            break;
        }
    }
    eprintln!("[tree-kill/sleep] root pid={pid} tree={tree:?}");
    assert!(tree.len() >= 3, "sleep 까지 뜨지 않았다: {tree:?}");
    t.cancel();
    let mut buf = [0u8; 8];
    assert!(st.read(&mut buf).await.is_err());
    // finish 가 파이프를 닫기 전에 상태를 본다 — 파이프 닫힘이 아니라 트리 종료로 사라져야 한다
    assert_tree_dies(&tree).await;
    assert_eq!(st.finish().await.unwrap_err(), ExecError::Aborted);
    eprintln!("[tree-kill/sleep] 트리 {}개 프로세스가 모두 종료됨", tree.len());
}

// ---------------------------------------------------------------------------------------------------------------------
// wslExecutor.test.ts — 배포판이 없으면(CI 러너 포함) 건너뛴다
// ---------------------------------------------------------------------------------------------------------------------

/// 시간 제한이 있는 날 프로세스 실행(wsl.exe 가 멈춰도 테스트가 매달리지 않게)
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

/// 배포판 안에서 셸 스크립트를 실행한다(준비·검사용). 인자는 --exec 라 그대로 전달된다
fn wsl_sh(distro: &str, script: &str) -> (bool, String) {
    let mut c = raw("wsl.exe");
    // 배포판 안의 사용자 git 설정(전역 hooks·commit.gpgsign 등)이 테스트 준비 커밋에 끼지 않게 격리한다
    let script = format!("export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1 GIT_AUTHOR_NAME=t GIT_AUTHOR_EMAIL=t@t GIT_COMMITTER_NAME=t GIT_COMMITTER_EMAIL=t@t; {script}");
    c.args(["-d", distro, "--exec", "sh", "-c", &script]).env("WSL_UTF8", "1");
    match output_with_timeout(c, Duration::from_secs(60)) {
        Some(o) => (o.status.success(), String::from_utf8_lossy(&o.stdout).trim().to_string()),
        None => (false, "시간 초과".to_string()),
    }
}

/// 첫 사용자 배포판 이름. `wsl.exe -l -q` 출력은 UTF-16LE 일 수 있어 NUL·BOM·CR 을 걸러 읽는다(docker-desktop 은 제외)
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

fn skip(why: &str) {
    eprintln!("[wsl] 건너뜀: {why}");
}

#[cfg(not(windows))]
#[test]
fn wsl_통합_windows_가_아니면_건너뛴다() {
    skip("Windows 가 아님");
}

#[cfg(windows)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wsl_통합() {
    let Some(distro) = first_distro() else { return skip("사용 가능한 WSL 배포판이 없음") };
    if !wsl_sh(&distro, "command -v git").0 {
        return skip(&format!("{distro} 에 git 이 설치돼 있지 않음"));
    }
    let script = [
        "set -e",
        "d=$(mktemp -d /tmp/antler-wsl-test.XXXXXX)",
        "cd \"$d\"",
        "git init -q -b main .",
        "echo a > \"a b.txt\"",
        "git add .",
        "git -c user.name=t -c user.email=t@t commit -q -m \"첫 커밋: 한글 메시지\"",
        "echo \"$d\"",
    ]
    .join("\n");
    let (ok, dir) = wsl_sh(&distro, &script);
    if !ok || !dir.starts_with("/tmp/antler-wsl-test.") {
        return skip(&format!("테스트 저장소 생성 실패: {dir}"));
    }
    struct Cleanup(String, String);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            if self.1.starts_with("/tmp/antler-wsl-test.") {
                wsl_sh(&self.0, &format!("rm -rf '{}'", self.1));
            }
        }
    }
    let _cleanup = Cleanup(distro.clone(), dir.clone());
    eprintln!("[wsl] 배포판 {distro}, 저장소 {dir}");

    let wsl = WslExecutor::new(&distro);
    let o = opts();

    // rev-parse --show-toplevel 이 리눅스 경로를 돌려준다
    let r = wsl.run(&dir, &["rev-parse", "--show-toplevel"], &o).await.unwrap();
    assert_eq!((r.code, text(&r.stdout)), (0, dir.clone()));

    // 홈 기준 상대 경로도 절대 리눅스 경로로 풀린다
    let name = format!("antler-wsl-rel.{}", std::process::id());
    let (ok, home) = wsl_sh(&distro, &format!("mkdir -p \"$HOME/{name}\" && git init -q \"$HOME/{name}\" && cd \"$HOME\" && pwd"));
    assert!(ok);
    let r = wsl.run(&name, &["rev-parse", "--show-toplevel"], &o).await;
    wsl_sh(&distro, &format!("rm -rf \"$HOME/{name}\""));
    let r = r.unwrap();
    assert_eq!((r.code, text(&r.stdout)), (0, format!("{home}/{name}")));

    // GIT_ENV 가 리눅스 쪽에 전달된다(LC_ALL) — alias 로 셸을 빌려 환경변수를 확인한다(--exec 에는 셸이 없다)
    let r = wsl.run(&dir, &["-c", "alias.e=!printf %s \"$LC_ALL:$GIT_LITERAL_PATHSPECS\"", "e"], &o).await.unwrap();
    assert_eq!(String::from_utf8_lossy(&r.stdout), "C.UTF-8:1");

    // log -z 의 한글이 깨지지 않는다
    let r = wsl.run(&dir, &["log", "-z", "--format=%s"], &o).await.unwrap();
    assert_eq!(String::from_utf8_lossy(&r.stdout).split('\0').next().unwrap(), "첫 커밋: 한글 메시지");

    // 공백이 든 파일명을 그대로 돌려준다
    let r = wsl.run(&dir, &["ls-files", "-z"], &o).await.unwrap();
    assert_eq!(String::from_utf8_lossy(&r.stdout).split('\0').next().unwrap(), "a b.txt");

    // stream 이 출력을 돌려준다
    let out = read_all(wsl.stream(&dir, &["log", "--format=%H"], &o).unwrap()).await.unwrap();
    let hash = text(&out);
    assert!(hash.len() == 40 && hash.bytes().all(|b| b.is_ascii_hexdigit()), "{hash}");

    // 없는 ref 는 run 은 비정상 코드, stream 은 Git 오류
    let r = wsl.run(&dir, &["rev-parse", "--verify", "refs/heads/__없는__"], &o).await.unwrap();
    assert_ne!(r.code, 0);
    let err = read_all(wsl.stream(&dir, &["log", "refs/heads/__없는__"], &o).unwrap()).await.unwrap_err();
    assert!(matches!(err, ExecError::Git { .. }), "{err:?}");

    // 이미 취소된 signal 이면 Aborted
    let t = CancellationToken::new();
    t.cancel();
    assert_eq!(wsl.run(&dir, &["status"], &RunOpts::with_cancel(t)).await.unwrap_err(), ExecError::Aborted);

    // 없는 배포판: wsl.exe 자체 실패는 WSL 메시지가 담긴 Git 오류(code -1, stderr 에 같은 메시지)
    let bad = WslExecutor::new("antler-없는-배포판");
    let r = bad.run("/", &["status"], &o).await.unwrap();
    assert_eq!(r.code, -1);
    assert!(r.stdout.is_empty() && r.stderr.starts_with("WSL"), "{:?}", r.stderr);
    let err = read_all(bad.stream("/", &["status"], &o).unwrap()).await.unwrap_err();
    let ExecError::Git { message, code, stderr, .. } = err else { panic!("Git 오류여야 한다") };
    assert_eq!(code, -1);
    assert!(message.starts_with("WSL") && message == stderr, "{message:?}");

    // 취소하면 리눅스 git 프로세스도 종료된다. 출력 없이 오래 블록하는 명령: git 셸 alias 가 sleep 을 실행한다
    let id = (100_000 + std::process::id() % 899_999).to_string();
    // pgrep -f 가 검사용 sh -c 자신의 명령줄에 걸리지 않도록 첫 글자를 대괄호로 감싼다
    let pat = format!("sleep [{}]{}", &id[..1], &id[1..]);
    let t = CancellationToken::new();
    let alias = format!("alias.antlersleep=!sleep {id}");
    let task = {
        let (distro, dir, t) = (distro.clone(), dir.clone(), t.clone());
        tokio::spawn(async move { WslExecutor::new(distro).run(&dir, &["-c", &alias, "antlersleep"], &RunOpts::with_cancel(t)).await })
    };
    let mut alive = false;
    for _ in 0..40 {
        if alive {
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
        alive = wsl_sh(&distro, &format!("pgrep -f '{pat}' >/dev/null && echo y")).1 == "y";
    }
    assert!(alive, "리눅스 쪽 sleep 이 뜨지 않았다");
    t.cancel();
    assert_eq!(task.await.unwrap().unwrap_err(), ExecError::Aborted);
    let mut gone = false;
    for _ in 0..20 {
        if gone {
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
        gone = wsl_sh(&distro, &format!("pgrep -f '{pat}' >/dev/null || echo n")).1 == "n";
    }
    if !gone {
        wsl_sh(&distro, &format!("pkill -f '{pat}'"));
    }
    assert!(gone, "취소했는데 리눅스 git(sleep)이 남았다");
}

#[tokio::test]
async fn git_executor_enum_은_종류에_맞는_실행기로_보낸다() {
    let l = GitExecutor::Local(LocalExecutor::default());
    assert_eq!(l.kind(), "local");
    assert_eq!(text(&l.run(&root_str(), &["rev-parse", "--is-inside-work-tree"], &opts()).await.unwrap().stdout), "true");
    assert_eq!(GitExecutor::Wsl(WslExecutor::new("x")).kind(), "wsl");
}
