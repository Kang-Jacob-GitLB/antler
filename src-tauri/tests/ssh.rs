//! SSH 실행기(`src/git/exec/ssh`, Phase 5 PR 2) 통합 테스트. TS 의 `tests/sshExecutor.test.ts` 이식 + 세션 정지·취소·슬롯·재연결·ProxyJump 2홉·거부 경로.
//!
//! **127.0.0.1 의 프로세스 안 `russh::server` 만 쓴다**(외부 접속·사용자의 `~/.ssh`·ssh-agent 를 읽거나 쓰지 않는다 — 실행기에는 임시 홈과 `AgentSetting::Disabled` 를 준다).
//! 서버는 `ssh/server.rs`, 임시 홈·저장소는 `ssh/common.rs`, 세션 정지 스파이크는 `ssh/spike.rs`. 원격 git 은 `LocalExecutor` 로 돌려 격리·트리 종료가 같다.
//! 격리: 이 바이너리가 시작될 때(`isolate`) git 설정 격리 환경을 프로세스에 넣는다(`tests/exec.rs` 와 같다).
//! 시간 의존은 `wait_for`(조건 + 상한)로 기다린다. 무거운 시험(4MB 이상 전송·git 다수)은 `heavy()` 로 직렬화한다.
//! 실제 sshd 를 쓰는 시험은 `#[ignore]` + `ANTLER_SSH_TEST_HOST`.

use std::path::Path;
use std::time::{Duration, Instant};

use antler_lib::git::exec::ssh::config::{list_hosts, load_ssh_config, Ctx, RealFs};
use antler_lib::git::exec::ssh::quote::build_remote_command;
use antler_lib::git::exec::ssh::{SshExecutor, SshOptions, SHELL_PROBE};
use antler_lib::git::exec::{ExecError, RunOpts, RunResult, GIT_ENV};
use russh::keys::{Algorithm, EcdsaCurve, PrivateKey};
use tokio_util::sync::CancellationToken;

#[path = "ssh/common.rs"]
mod common;
#[path = "ssh/server.rs"]
mod server;
#[path = "ssh/spike.rs"]
mod spike;

use common::{known_hosts_line, make_repo, openssh_no_comment, wait_for, Home, TempDir};
use server::{new_key, unquote, ExecMode, Opts, Shell, TestServer};

/// 테스트 프로세스 환경 격리(`tests/exec.rs` 의 `isolate` 와 같다). 서버 쪽 git 과 저장소 준비용 git 이 사용자의 전역 설정을 읽지 않게 한다
// unsafe: main 이전 코드는 Rust 런타임 초기화 전에 돈다 — 여기서는 환경 변수만 넣고 다른 것에 기대지 않는다
#[ctor::ctor(unsafe)]
fn isolate() {
    let global = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("gitconfig");
    std::env::set_var("GIT_CONFIG_GLOBAL", global);
    std::env::set_var("GIT_CONFIG_NOSYSTEM", "1");
    std::env::set_var("GCM_INTERACTIVE", "never");
}

const BIG: usize = 4 * 1024 * 1024;

/// 바이너리당 하나: 클라이언트 키와 큰 blob 이 든 저장소
struct Shared {
    client: PrivateKey,
    repo: TempDir,
}

async fn shared() -> &'static Shared {
    static S: tokio::sync::OnceCell<Shared> = tokio::sync::OnceCell::const_new();
    S.get_or_init(|| async { Shared { client: new_key(), repo: make_repo(BIG) } }).await
}

/// 무거운 시험은 한 번에 하나(git 프로세스·4MB 이상 전송이 겹치면 느린 러너에서 시간 상한을 넘는다)
async fn heavy() -> tokio::sync::MutexGuard<'static, ()> {
    static H: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    H.lock().await
}

/// 시험 하나의 환경: 이 시험 전용 서버(연결 수를 세려고)·임시 홈·설정(`Host test`)·known_hosts
struct Env {
    server: TestServer,
    home: Home,
    client: PrivateKey,
    repo: String,
}

impl Env {
    async fn new() -> Env {
        Env::with(|_| {}).await
    }

    async fn with(tweak: impl FnOnce(&mut Opts)) -> Env {
        let s = shared().await;
        let repo = s.repo.path().to_string_lossy().into_owned();
        let mut opts = Opts::new(vec![s.client.public_key().clone()]);
        tweak(&mut opts);
        let server = TestServer::start(opts).await;
        let home = Home::new(&s.client);
        home.write_config(&Env::host_block("test", server.port, &home, ""));
        home.write_known_hosts(&known_hosts_line(server.port, &server.host_key));
        Env { server, home, client: s.client.clone(), repo }
    }

    fn host_block(alias: &str, port: u16, home: &Home, extra: &str) -> String {
        format!(
            "Host {alias}\n  HostName 127.0.0.1\n  Port {port}\n  User tester\n  IdentityFile {}\n  IdentitiesOnly yes\n{extra}",
            home.key_for_config()
        )
    }

    fn ex(&self) -> SshExecutor {
        self.home.executor("test")
    }
}

async fn run(ex: &SshExecutor, repo: &str, args: &[&str]) -> Result<RunResult, ExecError> {
    ex.run(repo, args, &RunOpts::default()).await
}

fn message(r: Result<RunResult, ExecError>) -> String {
    match r {
        Ok(o) => panic!("실패해야 하는데 성공했다: code {}", o.code),
        Err(e) => e.to_string(),
    }
}

// ── TS sshExecutor.test.ts 이식: run ───────────────────────────────────────────

#[tokio::test]
async fn run_rev_parse_한글_커밋_메시지_연결은_재사용된다() {
    let e = Env::new().await;
    let ex = e.ex();
    let r = run(&ex, &e.repo, &["rev-parse", "--is-inside-work-tree"]).await.unwrap();
    assert_eq!(r.code, 0);
    assert_eq!(String::from_utf8_lossy(&r.stdout).trim(), "true");
    let log = run(&ex, &e.repo, &["log", "-1", "--format=%s"]).await.unwrap();
    assert_eq!(String::from_utf8_lossy(&log.stdout).trim(), "첫 커밋: 한글 메시지");
    let rs = futures_util::future::join_all((0..3).map(|_| run(&ex, &e.repo, &["rev-parse", "HEAD"]))).await;
    assert!(rs.iter().all(|r| r.as_ref().is_ok_and(|o| o.code == 0)));
    assert_eq!(e.server.connections(), 1, "연결은 하나를 재사용한다");
    ex.dispose();
}

#[tokio::test]
async fn run_실패_코드와_stderr_를_그대로_돌려준다() {
    let e = Env::new().await;
    let ex = e.ex();
    let r = run(&ex, &e.repo, &["rev-parse", "--verify", "refs/heads/없는_브랜치"]).await.unwrap();
    assert_ne!(r.code, 0);
    assert!(!r.stderr.is_empty());
    ex.dispose();
}

#[tokio::test]
async fn run_공백_따옴표가_든_인자도_그대로_전달된다() {
    let e = Env::new().await;
    let ex = e.ex();
    let r = run(&ex, &e.repo, &["log", "-1", "--format=it's $HOME `x` 한글 %s"]).await.unwrap();
    assert_eq!(String::from_utf8_lossy(&r.stdout).trim(), "it's $HOME `x` 한글 첫 커밋: 한글 메시지");
    ex.dispose();
}

#[tokio::test]
async fn run_max_buffer_초과는_git_오류이고_원격_자식을_정리한다() {
    let _h = heavy().await;
    let e = Env::new().await;
    let ex = e.ex();
    let opts = RunOpts { max_buffer: Some(1024), ..RunOpts::default() };
    let err = ex.run(&e.repo, &["cat-file", "blob", "HEAD:big.bin"], &opts).await.unwrap_err();
    assert!(matches!(&err, ExecError::Git { code: -1, message, .. } if message == "output exceeded 1024 bytes"), "{err:?}");
    wait_for("원격 자식 정리", || e.server.active_children() == 0).await;
    wait_for("슬롯 반환", || ex.free_slots() == 6).await;
    ex.dispose();
}

#[tokio::test]
async fn run_이미_취소된_토큰이면_aborted() {
    let e = Env::new().await;
    let ex = e.ex();
    let t = CancellationToken::new();
    t.cancel();
    let r = ex.run(&e.repo, &["rev-parse", "HEAD"], &RunOpts::with_cancel(t)).await;
    assert!(matches!(r, Err(ExecError::Aborted)));
    assert_eq!(e.server.connections(), 0, "취소된 호출은 연결도 맺지 않는다");
    ex.dispose();
}

#[tokio::test]
async fn run_큰_blob_4mb_를_온전히_받는다() {
    let _h = heavy().await;
    let e = Env::new().await;
    let ex = e.ex();
    let r = run(&ex, &e.repo, &["cat-file", "blob", "HEAD:big.bin"]).await.unwrap();
    assert_eq!(r.code, 0);
    assert_eq!(r.stdout.len(), BIG);
    ex.dispose();
}

#[tokio::test]
async fn 끊어진_연결은_다음_호출에서_지연_재연결한다() {
    let e = Env::new().await;
    let ex = e.ex();
    run(&ex, &e.repo, &["rev-parse", "HEAD"]).await.unwrap();
    let before = e.server.connections();
    e.server.drop_clients().await;
    wait_for("클라이언트가 끊김을 인지", || !ex.is_connected()).await;
    assert_eq!(run(&ex, &e.repo, &["rev-parse", "HEAD"]).await.unwrap().code, 0);
    assert_eq!(e.server.connections() - before, 1);
    ex.dispose();
}

#[tokio::test]
async fn 동시_채널이_한도를_넘어도_모두_처리한다() {
    let _h = heavy().await;
    let e = Env::new().await;
    let ex = e.ex();
    let rs = futures_util::future::join_all((0..20).map(|_| run(&ex, &e.repo, &["rev-parse", "HEAD"]))).await;
    assert!(rs.iter().all(|r| r.as_ref().is_ok_and(|o| o.code == 0)));
    wait_for("슬롯 반환", || ex.free_slots() == 6).await;
    assert_eq!(e.server.connections(), 1);
    ex.dispose();
}

#[tokio::test]
async fn 호출당_왕복_시간_로컬_ms() {
    let _h = heavy().await;
    let e = Env::new().await;
    let ex = e.ex();
    run(&ex, &e.repo, &["rev-parse", "HEAD"]).await.unwrap(); // 연결·핸드셰이크 제외
    let n = 20;
    let t0 = Instant::now();
    for _ in 0..n {
        run(&ex, &e.repo, &["rev-parse", "HEAD"]).await.unwrap();
    }
    let per = t0.elapsed().as_secs_f64() * 1000.0 / f64::from(n);
    println!("[ssh] exec 왕복 평균 {per:.1}ms (git 프로세스 기동 포함, 로컬 in-process 서버)");
    assert!(per < 2000.0, "{per}ms");
    ex.dispose();
}

#[tokio::test]
async fn user_host_port_직접_입력() {
    let e = Env::new().await;
    let home = Home::new(&e.client);
    home.write_config(&format!("Host *\n  IdentityFile {}\n", home.key_for_config()));
    home.write_known_hosts(&known_hosts_line(e.server.port, &e.server.host_key));
    let ex = home.executor(&format!("tester@127.0.0.1:{}", e.server.port));
    assert_eq!(run(&ex, &e.repo, &["rev-parse", "HEAD"]).await.unwrap().code, 0);
    ex.dispose();
}

#[tokio::test]
async fn config_의_구체_별칭을_돌려준다_없는_파일은_빈_배열() {
    let e = Env::new().await;
    let list = |path: &str| {
        let cfg = load_ssh_config(path, &e.home.home_str(), &RealFs);
        let ctx = Ctx { local_user: "tester", env: &|_| None };
        list_hosts(&cfg, &ctx)
    };
    let hosts = list(&e.home.config.to_string_lossy());
    assert_eq!(hosts.iter().map(|h| h.alias.as_str()).collect::<Vec<_>>(), ["test"]);
    assert_eq!((hosts[0].host_name.as_str(), hosts[0].user.as_str(), hosts[0].port), ("127.0.0.1", "tester", u32::from(e.server.port)));
    assert!(list(&format!("{}.none", e.home.config.to_string_lossy())).is_empty());
}

// ── 원격 명령·셸 확인 ──────────────────────────────────────────────────────────

#[tokio::test]
async fn 원격_명령은_셸_확인_뒤에_build_remote_command_결과만_간다() {
    let e = Env::new().await;
    let ex = e.ex();
    run(&ex, &e.repo, &["rev-parse", "HEAD"]).await.unwrap();
    let log = e.server.exec_log();
    assert_eq!(log.len(), 2, "{log:?}");
    assert_eq!(log[0], SHELL_PROBE, "사용자 값이 든 명령보다 셸 확인이 먼저다");
    assert_eq!(log[1], build_remote_command(&e.repo, &["rev-parse", "HEAD"], None).unwrap());
    // 풀린 환경은 GIT_ENV 와 같다
    let argv = unquote(&log[1]).unwrap();
    assert_eq!(argv[0], "env");
    let mut got: Vec<(String, String)> =
        argv[1..].iter().take_while(|a| a.as_str() != "git").map(|a| a.split_once('=').map(|(k, v)| (k.to_string(), v.to_string())).unwrap()).collect();
    let mut want: Vec<(String, String)> = GIT_ENV.iter().map(|(k, v)| ((*k).to_string(), (*v).to_string())).collect();
    got.sort();
    want.sort();
    assert_eq!(got, want);
    ex.dispose();
}

#[tokio::test]
async fn 원격_로그인_셸이_posix_가_아니면_사용자_값이_든_명령을_보내기_전에_거부한다() {
    let e = Env::with(|o| o.shell = Shell::Cmd).await;
    let ex = e.ex();
    let m = message(run(&ex, &e.repo, &["rev-parse", "HEAD"]).await);
    assert!(m.contains("POSIX 셸이 아닙니다"), "{m}");
    assert_eq!(e.server.exec_log(), [SHELL_PROBE], "셸 확인 말고는 아무 명령도 보내지 않았다");
    wait_for("거부한 접속의 서버 쪽 연결 종료", || e.server.open_connections() == 0).await;
    ex.dispose();
}

// ── 거부 경로 ──────────────────────────────────────────────────────────────────

#[tokio::test]
async fn known_hosts_의_키와_다른_호스트_키는_거부한다_중간자_의심() {
    let e = Env::new().await;
    e.home.write_known_hosts(&known_hosts_line(e.server.port, new_key().public_key()));
    let ex = e.ex();
    let m = message(run(&ex, &e.repo, &["rev-parse", "HEAD"]).await);
    assert!(m.contains("중간자"), "{m}");
    assert!(!m.contains("PRIVATE"));
    wait_for("거부한 접속의 서버 쪽 연결 종료", || e.server.open_connections() == 0).await;
    ex.dispose();
}

#[tokio::test]
async fn 알_수_없는_호스트는_거부하고_ssh_cli_로_수락하라고_안내한다_자동_등록_없음() {
    let e = Env::new().await;
    e.home.write_known_hosts("");
    let ex = e.ex();
    let m = message(run(&ex, &e.repo, &["rev-parse", "HEAD"]).await);
    assert!(m.contains("알 수 없는 호스트") && m.contains("ssh CLI"), "{m}");
    assert!(!m.contains("PRIVATE"));
    assert_eq!(std::fs::read_to_string(&e.home.known_hosts).unwrap(), "", "known_hosts 에 쓰지 않는다");
    assert_eq!(e.server.exec_log().len(), 0);
    ex.dispose();
}

#[tokio::test]
async fn 폐기된_호스트_키는_거부한다() {
    let e = Env::new().await;
    let line = format!("@revoked [127.0.0.1]:{} {}\n", e.server.port, openssh_no_comment(&e.server.host_key));
    e.home.write_known_hosts(&(known_hosts_line(e.server.port, &e.server.host_key) + &line));
    let ex = e.ex();
    let m = message(run(&ex, &e.repo, &["rev-parse", "HEAD"]).await);
    assert!(m.contains("폐기"), "{m}");
    ex.dispose();
}

#[tokio::test]
async fn 서버가_모르는_클라이언트_키는_인증_오류() {
    let e = Env::new().await;
    let home = Home::new(&new_key());
    home.write_config(&Env::host_block("test", e.server.port, &home, ""));
    home.write_known_hosts(&known_hosts_line(e.server.port, &e.server.host_key));
    let ex = home.executor("test");
    let m = message(run(&ex, &e.repo, &["rev-parse", "HEAD"]).await);
    assert!(m.contains("인증에 실패"), "{m}");
    assert!(!m.contains("PRIVATE"));
    wait_for("실패한 접속의 서버 쪽 연결 종료", || e.server.open_connections() == 0).await;
    ex.dispose();
}

#[tokio::test]
async fn 인증을_거부하는_서버에_동시_호출이_몰려도_연결과_인증은_한_번이고_실패는_공유된다() {
    let e = Env::new().await;
    let home = Home::new(&new_key());
    home.write_config(&Env::host_block("test", e.server.port, &home, ""));
    home.write_known_hosts(&known_hosts_line(e.server.port, &e.server.host_key));
    let ex = home.executor("test");
    let rs = futures_util::future::join_all((0..6).map(|_| run(&ex, &e.repo, &["rev-parse", "HEAD"]))).await;
    let messages: Vec<String> = rs.into_iter().map(message).collect();
    assert!(messages[0].contains("인증에 실패"), "{}", messages[0]);
    assert!(messages.iter().all(|m| *m == messages[0]), "대기하던 호출은 같은 실패를 받는다: {messages:?}");
    assert_eq!(e.server.connections(), 1, "호출 수만큼 다시 맺지 않는다");
    assert_eq!(e.server.auth_attempts(), 1, "호출 수만큼 다시 인증하지 않는다");
    wait_for("슬롯 반환", || ex.free_slots() == 6).await;
    ex.dispose();
}

#[tokio::test]
async fn 인증_수단이_전혀_없으면_키_없음_agent_없음_즉시_안내한다() {
    let e = Env::new().await;
    let home = Home::new(&e.client);
    home.write_config(&format!("Host test\n  HostName 127.0.0.1\n  Port {}\n  IdentityFile {}/없는키\n", e.server.port, home.home_str().replace('\\', "/")));
    let ex = home.executor("test");
    let m = message(run(&ex, &e.repo, &["rev-parse", "HEAD"]).await);
    assert!(m.contains("인증 수단이 없습니다"), "{m}");
    assert_eq!(e.server.connections(), 0, "수단이 없으면 접속하지 않는다");
    ex.dispose();
}

#[tokio::test]
async fn 암호화된_키는_건너뛰고_사유를_안내한다() {
    let e = Env::new().await;
    let home = Home::new(&e.client);
    // 암호가 걸린 키(passphrase 는 저장·질문하지 않는다)
    let locked = new_key().encrypt(&mut rand::rng(), "pw").unwrap();
    std::fs::write(&home.key_path, locked.to_openssh(russh::keys::ssh_key::LineEnding::LF).unwrap().as_bytes()).unwrap();
    home.write_config(&Env::host_block("test", e.server.port, &home, ""));
    home.write_known_hosts(&known_hosts_line(e.server.port, &e.server.host_key));
    let ex = home.executor("test");
    let m = message(run(&ex, &e.repo, &["rev-parse", "HEAD"]).await);
    assert!(m.contains("암호화된 키라 건너뜀") && m.contains("id_test") && m.contains("ssh-add"), "{m}");
    assert_eq!(e.server.connections(), 0);
    ex.dispose();
}

#[tokio::test]
async fn 연결_거부는_명확한_메시지() {
    let e = Env::new().await;
    // 아무도 듣지 않는 포트: 잡았다가 놓는다
    let port = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap().local_addr().unwrap().port();
    let home = Home::new(&e.client);
    home.write_config(&Env::host_block("test", port, &home, ""));
    let ex = home.executor("test");
    let m = message(run(&ex, &e.repo, &["rev-parse", "HEAD"]).await);
    assert!(m.contains("연결이 거부"), "{m}");
    ex.dispose();
}

#[tokio::test]
async fn 아무_말도_없는_서버는_접속_제한_시간_안에_끊는다() {
    let e = Env::new().await;
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    // 받기만 하고 배너를 보내지 않는다
    let held = tokio::spawn(async move {
        let mut socks = Vec::new();
        while let Ok((s, _)) = listener.accept().await {
            socks.push(s);
        }
    });
    let home = Home::new(&e.client);
    home.write_config(&Env::host_block("test", port, &home, ""));
    let ex = home.executor_with("test", |o| o.timeouts.ready = Duration::from_millis(300));
    let m = message(run(&ex, &e.repo, &["rev-parse", "HEAD"]).await);
    assert!(m.contains("연결 시간이 초과"), "{m}");
    ex.dispose();
    held.abort();
}

#[tokio::test]
async fn dispose_뒤에는_사용할_수_없다() {
    let e = Env::new().await;
    let ex = e.ex();
    ex.dispose();
    let m = message(run(&ex, &e.repo, &["rev-parse", "HEAD"]).await);
    assert!(m.contains("종료"), "{m}");
    assert_eq!(e.server.connections(), 0);
}

#[tokio::test]
async fn dispose_는_서버_쪽_연결도_닫는다() {
    let e = Env::new().await;
    let ex = e.ex();
    run(&ex, &e.repo, &["rev-parse", "HEAD"]).await.unwrap();
    assert_eq!(e.server.open_connections(), 1);
    ex.dispose();
    wait_for("서버 쪽 연결 종료", || e.server.open_connections() == 0).await;
}

#[tokio::test]
async fn dispose_는_슬롯을_기다리던_호출과_진행_중인_호출을_깨운다() {
    let e = Env::new().await;
    let ex = e.home.executor_with("test", |o| o.max_channels = 1);
    let (a, b) = (ex.clone(), ex.clone());
    let first = tokio::spawn(async move { a.run("antler-fake:hold", &["status"], &RunOpts::default()).await });
    wait_for("원격 자식 시작", || e.server.active_children() == 1).await;
    let second = tokio::spawn(async move { b.run("antler-fake:hold", &["status"], &RunOpts::default()).await });
    // 슬롯이 하나라 두 번째는 첫째가 끝나기 전에 슬롯을 얻지 못한다(서버에 두 번째 자식이 생기지 않는다)
    ex.dispose();
    let m2 = message(second.await.unwrap());
    assert!(m2.contains("종료"), "{m2}");
    let m1 = message(first.await.unwrap());
    assert!(m1.contains("연결이 끊겨"), "{m1}");
    assert_eq!(e.server.exec_log().len(), 2, "셸 확인과 첫 호출뿐 — 두 번째 명령은 서버에 가지 않았다");
    wait_for("원격 자식 정리", || e.server.active_children() == 0).await;
}

#[tokio::test]
async fn 비_ascii_호스트_이름은_연결_전에_거부한다() {
    let e = Env::new().await;
    let home = Home::new(&e.client);
    home.write_config(&format!("Host test\n  HostName 호스트.example\n  Port {}\n  IdentityFile {}\n", e.server.port, home.key_for_config()));
    let ex = home.executor("test");
    let m = message(run(&ex, &e.repo, &["rev-parse", "HEAD"]).await);
    assert!(m.contains("지원하지 않는 ssh 설정") && m.contains("비 ASCII"), "{m}");
    assert_eq!(e.server.connections(), 0);
    ex.dispose();
}

#[tokio::test]
async fn 비_ascii_별칭_스펙은_연결_전에_거부한다() {
    let e = Env::new().await;
    let ex = e.home.executor("테스트");
    let m = message(run(&ex, &e.repo, &["rev-parse", "HEAD"]).await);
    assert!(m.contains("지원하지 않는 ssh 설정") && m.contains("비 ASCII"), "{m}");
    assert_eq!(e.server.connections(), 0);
    ex.dispose();
}

#[tokio::test]
async fn 호스트_키_별칭의_비_ascii_도_거부한다() {
    let e = Env::new().await;
    let home = Home::new(&e.client);
    home.write_config(&Env::host_block("test", e.server.port, &home, "  HostKeyAlias 별칭\n"));
    let ex = home.executor("test");
    let m = message(run(&ex, &e.repo, &["rev-parse", "HEAD"]).await);
    assert!(m.contains("비 ASCII"), "{m}");
    assert_eq!(e.server.connections(), 0);
    ex.dispose();
}

#[tokio::test]
async fn 대상과_홉의_포트_범위_밖은_연결_전에_거부한다() {
    let e = Env::new().await;
    for spec in ["tester@127.0.0.1:0", "tester@127.0.0.1:65536"] {
        let ex = e.home.executor(spec);
        let m = message(run(&ex, &e.repo, &["rev-parse", "HEAD"]).await);
        assert!(m.contains("포트가 1~65535 범위 밖"), "{spec}: {m}");
        ex.dispose();
    }
    let home = Home::new(&e.client);
    home.write_config(&Env::host_block("test", e.server.port, &home, "  ProxyJump 127.0.0.1:70000\n"));
    let ex = home.executor("test");
    let m = message(run(&ex, &e.repo, &["rev-parse", "HEAD"]).await);
    assert!(m.contains("포트가 1~65535 범위 밖"), "{m}");
    assert_eq!(e.server.connections(), 0);
    ex.dispose();
}

#[tokio::test]
async fn proxy_command_와_revoked_host_keys_는_연결_전에_거부한다() {
    let e = Env::new().await;
    for (extra, what) in [("  ProxyCommand nc %h %p\n", "ProxyCommand"), ("  RevokedHostKeys /x/revoked\n", "RevokedHostKeys")] {
        let home = Home::new(&e.client);
        home.write_config(&Env::host_block("test", e.server.port, &home, extra));
        let ex = home.executor("test");
        let m = message(run(&ex, &e.repo, &["rev-parse", "HEAD"]).await);
        assert!(m.contains("지원하지 않는 ssh 설정") && m.contains(what), "{m}");
        ex.dispose();
    }
    assert_eq!(e.server.connections(), 0);
}

#[tokio::test]
async fn 점프_호스트의_unsupported_설정도_거부한다() {
    let e = Env::new().await;
    let jump = TestServer::start(Opts::new(vec![e.client.public_key().clone()])).await;
    let home = Home::new(&e.client);
    home.write_config(&format!(
        "{}{}",
        Env::host_block("jump", jump.port, &home, "  ProxyCommand nc %h %p\n"),
        Env::host_block("target", e.server.port, &home, "  ProxyJump jump\n")
    ));
    let ex = home.executor("target");
    let m = message(run(&ex, &e.repo, &["rev-parse", "HEAD"]).await);
    assert!(m.starts_with("jump: 지원하지 않는 ssh 설정") && m.contains("ProxyCommand"), "{m}");
    assert_eq!(jump.connections() + e.server.connections(), 0);
    ex.dispose();
}

#[tokio::test]
async fn 점프_호스트_자신의_proxy_jump_중첩은_연결_전에_거부한다() {
    let e = Env::new().await;
    let jump = TestServer::start(Opts::new(vec![e.client.public_key().clone()])).await;
    let home = Home::new(&e.client);
    home.write_config(&format!(
        "{}{}",
        Env::host_block("jump", jump.port, &home, "  ProxyJump bastion
"),
        Env::host_block("target", e.server.port, &home, "  ProxyJump jump
")
    ));
    let ex = home.executor("target");
    let m = message(run(&ex, &e.repo, &["rev-parse", "HEAD"]).await);
    assert!(m.starts_with("jump: 점프 호스트의 ProxyJump(중첩)는 지원하지 않습니다"), "{m}");
    assert_eq!(jump.connections() + e.server.connections(), 0, "무시하고 접속하지 않는다");
    ex.dispose();
}

// ── known_hosts 조회 이름 ──────────────────────────────────────────────────────

#[tokio::test]
async fn host_key_alias_가_있으면_별칭과_포트_22_로_조회한다() {
    let e = Env::new().await;
    let home = Home::new(&e.client);
    home.write_config(&Env::host_block("test", e.server.port, &home, "  HostKeyAlias alias-host\n"));
    let key = openssh_no_comment(&e.server.host_key);
    // 실제 포트 항목만 있으면 별칭 조회에는 쓰이지 않는다
    home.write_known_hosts(&known_hosts_line(e.server.port, &e.server.host_key));
    let ex = home.executor("test");
    let m = message(run(&ex, &e.repo, &["rev-parse", "HEAD"]).await);
    assert!(m.contains("알 수 없는 호스트") && m.contains("alias-host"), "{m}");
    ex.dispose();
    // 별칭 항목(포트 없음)이면 통과한다
    home.write_known_hosts(&format!("alias-host {key}\n"));
    let ex = home.executor("test");
    assert_eq!(run(&ex, &e.repo, &["rev-parse", "HEAD"]).await.unwrap().code, 0);
    ex.dispose();
}

#[tokio::test]
async fn user_known_hosts_file_을_따른다() {
    let e = Env::new().await;
    let home = Home::new(&e.client);
    let custom = home.dir.path().join("custom_known_hosts");
    std::fs::write(&custom, known_hosts_line(e.server.port, &e.server.host_key)).unwrap();
    home.write_config(&Env::host_block("test", e.server.port, &home, &format!("  UserKnownHostsFile {}\n", custom.to_string_lossy().replace('\\', "/"))));
    home.write_known_hosts(""); // 기본 파일에는 없다
    let ex = home.executor("test");
    assert_eq!(run(&ex, &e.repo, &["rev-parse", "HEAD"]).await.unwrap().code, 0);
    ex.dispose();
}

#[tokio::test]
async fn 호스트_키_알고리즘은_등록된_종류로_제한한다() {
    let e = Env::new().await;
    // known_hosts 에는 이 호스트가 ecdsa 키로만 등록돼 있는데 서버는 ed25519 만 낸다 — 다른 종류를 받아들이지 않고 협상에서 실패한다
    let ecdsa = PrivateKey::random(&mut rand::rng(), Algorithm::Ecdsa { curve: EcdsaCurve::NistP256 }).unwrap();
    e.home.write_known_hosts(&known_hosts_line(e.server.port, ecdsa.public_key()));
    let ex = e.ex();
    let m = message(run(&ex, &e.repo, &["rev-parse", "HEAD"]).await);
    assert!(!m.contains("알 수 없는 호스트") && !m.contains("중간자"), "다른 종류의 키를 내밀게 두면 안 된다(그러면 '알 수 없는 호스트' 로 보인다): {m}");
    assert_eq!(e.server.exec_log().len(), 0);
    ex.dispose();
}

// ── ProxyJump ──────────────────────────────────────────────────────────────────

#[tokio::test]
async fn 점프_호스트를_거쳐_대상에_접속한다_각_호스트_키를_따로_검증() {
    let e = Env::new().await;
    let jump = TestServer::start(Opts::new(vec![e.client.public_key().clone()])).await;
    let home = Home::new(&e.client);
    home.write_config(&format!(
        "{}{}",
        Env::host_block("jump", jump.port, &home, ""),
        Env::host_block("target", e.server.port, &home, "  ProxyJump jump\n")
    ));
    home.write_known_hosts(&(known_hosts_line(jump.port, &jump.host_key) + &known_hosts_line(e.server.port, &e.server.host_key)));
    let ex = home.executor("target");
    let r = run(&ex, &e.repo, &["log", "-1", "--format=%s"]).await.unwrap();
    assert_eq!(String::from_utf8_lossy(&r.stdout).trim(), "첫 커밋: 한글 메시지");
    assert_eq!(jump.connections(), 1);
    assert_eq!(jump.tunnels(), [format!("127.0.0.1:{}", e.server.port)]);
    let blob = run(&ex, &e.repo, &["cat-file", "blob", "HEAD:a.txt"]).await.unwrap();
    assert_eq!(blob.stdout.len(), 6);
    ex.dispose();
}

#[tokio::test]
async fn 점프_호스트_키가_틀리면_점프_단계에서_거부한다() {
    let e = Env::new().await;
    let jump = TestServer::start(Opts::new(vec![e.client.public_key().clone()])).await;
    let home = Home::new(&e.client);
    home.write_config(&format!(
        "{}{}",
        Env::host_block("jump", jump.port, &home, ""),
        Env::host_block("target", e.server.port, &home, "  ProxyJump jump\n")
    ));
    home.write_known_hosts(&(known_hosts_line(jump.port, new_key().public_key()) + &known_hosts_line(e.server.port, &e.server.host_key)));
    let ex = home.executor("target");
    let m = message(run(&ex, &e.repo, &["rev-parse", "HEAD"]).await);
    assert!(m.contains("점프 호스트 jump") && m.contains("중간자"), "{m}");
    assert_eq!(e.server.connections(), 0, "대상에는 접속하지 않는다");
    ex.dispose();
}

#[tokio::test]
async fn 점프_호스트_둘을_거쳐_대상에_접속한다() {
    let e = Env::new().await;
    let k = e.client.public_key().clone();
    let (j1, j2) = (TestServer::start(Opts::new(vec![k.clone()])).await, TestServer::start(Opts::new(vec![k])).await);
    let home = Home::new(&e.client);
    home.write_config(&format!(
        "{}{}{}",
        Env::host_block("j1", j1.port, &home, ""),
        Env::host_block("j2", j2.port, &home, ""),
        Env::host_block("target", e.server.port, &home, "  ProxyJump j1,j2\n")
    ));
    home.write_known_hosts(
        &(known_hosts_line(j1.port, &j1.host_key) + &known_hosts_line(j2.port, &j2.host_key) + &known_hosts_line(e.server.port, &e.server.host_key)),
    );
    let ex = home.executor("target");
    let r = run(&ex, &e.repo, &["rev-parse", "--is-inside-work-tree"]).await.unwrap();
    assert_eq!(String::from_utf8_lossy(&r.stdout).trim(), "true");
    assert_eq!((j1.connections(), j2.connections(), e.server.connections()), (1, 1, 1));
    assert_eq!(j1.tunnels(), [format!("127.0.0.1:{}", j2.port)], "첫 점프는 둘째 점프로만 터널을 낸다");
    assert_eq!(j2.tunnels(), [format!("127.0.0.1:{}", e.server.port)]);
    assert_eq!(j1.exec_log().len() + j2.exec_log().len(), 0, "점프 호스트에는 명령을 보내지 않는다");
    ex.dispose();
}

#[tokio::test]
async fn 점프_체인의_둘째_홉_키가_틀리면_그_홉에서_거부한다() {
    let e = Env::new().await;
    let k = e.client.public_key().clone();
    let (j1, j2) = (TestServer::start(Opts::new(vec![k.clone()])).await, TestServer::start(Opts::new(vec![k])).await);
    let home = Home::new(&e.client);
    home.write_config(&format!(
        "{}{}{}",
        Env::host_block("j1", j1.port, &home, ""),
        Env::host_block("j2", j2.port, &home, ""),
        Env::host_block("target", e.server.port, &home, "  ProxyJump j1,j2\n")
    ));
    home.write_known_hosts(&(known_hosts_line(j1.port, &j1.host_key) + &known_hosts_line(j2.port, new_key().public_key()) + &known_hosts_line(e.server.port, &e.server.host_key)));
    let ex = home.executor("target");
    let m = message(run(&ex, &e.repo, &["rev-parse", "HEAD"]).await);
    assert!(m.contains("점프 호스트 j2") && m.contains("중간자"), "{m}");
    assert_eq!(e.server.connections(), 0);
    // 실패한 체인의 이미 맺은 홉(j1)과 거부한 홉(j2)이 서버 쪽에서도 닫힌다
    wait_for("점프 호스트의 서버 쪽 연결 종료", || j1.open_connections() == 0 && j2.open_connections() == 0).await;
    ex.dispose();
}

// ── 취소·슬롯·유휴·세션 정지 ───────────────────────────────────────────────────

#[tokio::test]
async fn run_취소_토큰은_원격_자식을_정리하고_슬롯을_돌려준다() {
    let e = Env::new().await;
    let ex = e.ex();
    let t = CancellationToken::new();
    let (ex2, t2) = (ex.clone(), t.clone());
    let task = tokio::spawn(async move { ex2.run("antler-fake:hold", &["status"], &RunOpts::with_cancel(t2)).await });
    wait_for("원격 자식 시작", || e.server.active_children() == 1).await;
    t.cancel();
    assert!(matches!(task.await.unwrap(), Err(ExecError::Aborted)));
    wait_for("원격 자식 정리", || e.server.active_children() == 0).await;
    wait_for("슬롯 반환", || ex.free_slots() == 6).await;
    assert_eq!(run(&ex, &e.repo, &["rev-parse", "HEAD"]).await.unwrap().code, 0, "같은 연결로 계속 쓸 수 있다");
    assert_eq!(e.server.connections(), 1);
    ex.dispose();
}

#[tokio::test]
async fn run_future_를_버려도_원격_자식이_정리되고_슬롯이_돌아온다() {
    let e = Env::new().await;
    let ex = e.ex();
    let ex2 = ex.clone();
    let task = tokio::spawn(async move { ex2.run("antler-fake:hold", &["status"], &RunOpts::default()).await });
    wait_for("원격 자식 시작", || e.server.active_children() == 1).await;
    task.abort(); // ProcessQueue 가 대기자 없는 요청을 버리는 것과 같다
    let _ = task.await;
    wait_for("원격 자식 정리", || e.server.active_children() == 0).await;
    wait_for("슬롯 반환", || ex.free_slots() == 6).await;
    ex.dispose();
}

#[tokio::test]
async fn 취소와_버림을_슬롯_수보다_많이_반복해도_슬롯이_새지_않는다() {
    let e = Env::new().await;
    let ex = e.ex();
    for i in 0..10 {
        let t = CancellationToken::new();
        let (ex2, t2) = (ex.clone(), t.clone());
        let task = tokio::spawn(async move { ex2.run("antler-fake:hold", &["status"], &RunOpts::with_cancel(t2)).await });
        wait_for("원격 자식 시작", || e.server.active_children() == 1).await;
        if i % 2 == 1 {
            t.cancel();
            let _ = task.await;
        } else {
            task.abort();
            let _ = task.await;
        }
        wait_for("원격 자식 정리", || e.server.active_children() == 0).await;
    }
    wait_for("슬롯 반환", || ex.free_slots() == 6).await;
    let r = tokio::time::timeout(Duration::from_secs(8), run(&ex, &e.repo, &["rev-parse", "HEAD"])).await.expect("슬롯이 새어 요청이 멈췄다");
    assert_eq!(r.unwrap().code, 0);
    assert_eq!(e.server.connections(), 1);
    ex.dispose();
}

#[tokio::test]
async fn 출력_상한을_넘으면_원격_자식을_죽이고_슬롯을_돌려준다() {
    let _h = heavy().await;
    let e = Env::new().await;
    let ex = e.ex();
    let opts = RunOpts { max_buffer: Some(1024 * 1024), ..RunOpts::default() };
    let err = ex.run("antler-fake:flood:16777216:4096", &["status"], &opts).await.unwrap_err();
    assert!(matches!(&err, ExecError::Git { code: -1, message, .. } if message == "output exceeded 1048576 bytes"), "{err:?}");
    wait_for("원격 자식 정리", || e.server.active_children() == 0).await;
    wait_for("슬롯 반환", || ex.free_slots() == 6).await;
    ex.dispose();
}

#[tokio::test]
async fn 종료_코드_없이_연결이_끊기면_연결이_끊겼다는_오류() {
    let e = Env::new().await;
    let ex = e.ex();
    let ex2 = ex.clone();
    let task = tokio::spawn(async move { ex2.run("antler-fake:hold", &["status"], &RunOpts::default()).await });
    wait_for("원격 자식 시작", || e.server.active_children() == 1).await;
    e.server.drop_clients().await;
    let m = message(task.await.unwrap());
    assert!(m.contains("SSH 연결이 끊겨"), "{m}");
    ex.dispose();
}

#[tokio::test]
async fn 유휴_시간이_지나면_연결을_닫고_다음_호출에서_다시_맺는다() {
    let e = Env::new().await;
    let ex = e.home.executor_with("test", |o| o.timeouts.idle = Duration::from_millis(200));
    run(&ex, &e.repo, &["rev-parse", "HEAD"]).await.unwrap();
    wait_for("유휴 종료", || !ex.is_connected()).await;
    wait_for("서버 쪽 연결 종료", || e.server.open_connections() == 0).await;
    assert_eq!(run(&ex, &e.repo, &["rev-parse", "HEAD"]).await.unwrap().code, 0);
    assert_eq!(e.server.connections(), 2);
    ex.dispose();
}

#[tokio::test]
async fn 세션_정지_없음_큰_출력과_작은_호출이_같은_연결에서_함께_끝난다() {
    let _h = heavy().await;
    let e = Env::new().await;
    let ex = e.ex();
    // 작은 패킷(512B, 8192개 — 채널 큐 100개보다 훨씬 많다)으로 4MB 를 쏟는 호출 옆에서 짧은 호출들이 끝난다(읽는 쪽이 항상 비운다)
    let (big, ex2) = (e.repo.clone(), ex.clone());
    let flood = tokio::spawn(async move { ex2.run("antler-fake:flood:4194304:512", &["status"], &RunOpts::default()).await });
    let smalls = tokio::time::timeout(
        Duration::from_secs(60),
        futures_util::future::join_all((0..5).map(|_| run(&ex, &big, &["rev-parse", "HEAD"]))),
    )
    .await
    .expect("작은 호출이 큰 출력에 막혔다");
    assert!(smalls.iter().all(|r| r.as_ref().is_ok_and(|o| o.code == 0)));
    let r = tokio::time::timeout(Duration::from_secs(60), flood).await.expect("큰 출력이 끝나지 않았다").unwrap().unwrap();
    assert_eq!(r.stdout.len(), BIG);
    ex.dispose();
}

#[tokio::test]
async fn 세션_정지_스파이크_읽지_않는_채널_옆의_채널은_멈추고_펌프가_비우면_끝난다() {
    let _h = heavy().await;
    let (unread_ok, pumped_ok) = spike::measure().await;
    println!("[spike] 읽지 않는 A 옆의 B 완료={unread_ok}, 펌프가 비우는 A 옆의 B 완료={pumped_ok}");
    // 읽지 않으면 russh 의 세션 루프가 멈춘다(실측 — docs/tauri.md). 이 단언이 깨지면 russh 가 고친 것이니 펌프 설계를 다시 본다
    assert!(!unread_ok, "russh 가 읽지 않는 채널에도 세션을 멈추지 않는다 — 문서의 R1 절을 갱신한다");
    assert!(pumped_ok, "펌프가 비우는데도 B 가 끝나지 않았다");
}

// ── 소스 검사(clippy 의 allow 로 우회되지 않게) ────────────────────────────────

fn rust_sources(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let p = entry.path();
        if p.is_dir() {
            rust_sources(&p, out);
        } else if p.extension().is_some_and(|e| e == "rs") {
            out.push(p);
        }
    }
}

#[test]
fn russh_의_연결_인증_포워딩_api_는_허용된_파일에서만_쓴다() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rust_sources(&src, &mut files);
    // (이름, 허용되는 파일 이름들) — 빈 목록이면 어디에도 쓰지 않는다
    let rules: [(&str, &[&str]); 15] = [
        ("connect_stream", &["connection.rs"]),
        ("client::connect(", &[]),
        ("channel_open_direct_tcpip", &["connection.rs"]),
        ("authenticate_password", &[]),
        ("authenticate_keyboard_interactive", &[]),
        ("learn_known_hosts", &[]),
        ("check_known_hosts", &[]),
        ("tcpip_forward", &[]),
        ("streamlocal_forward", &[]),
        ("agent_forward", &[]),
        ("request_x11", &[]),
        ("channel_open_x11", &[]),
        ("channel_open_direct_streamlocal", &[]),
        ("gssapi", &[]),
        ("send_global_request", &[]),
    ];
    for f in &files {
        let name = f.file_name().unwrap().to_string_lossy().into_owned();
        let text = std::fs::read_to_string(f).unwrap();
        for (api, allowed) in rules {
            // 주석·문서에서 이름을 언급하는 것은 막지 않는다(코드 줄만 본다)
            let used = text.lines().filter(|l| !l.trim_start().starts_with("//")).any(|l| mentions(l, api));
            assert!(!used || allowed.contains(&name.as_str()), "{} 에서 {api} 를 쓴다 — 허용된 파일: {allowed:?}", f.display());
        }
        assert!(!imports_russh_connect(&text), "{} 가 russh 의 connect 를 이름으로 들여온다(`use russh::client::connect;` 후 `connect(` 로 clippy 규칙·위 검사를 피할 수 있다)", f.display());
    }
}

/// 줄에 `api` 가 식별자 경계에서 시작하는가(핸들러 콜백 `server_channel_open_x11` 같은 이름은 `channel_open_x11` 호출로 보지 않는다)
fn mentions(line: &str, api: &str) -> bool {
    line.match_indices(api).any(|(at, _)| !line[..at].chars().next_back().is_some_and(|c| c.is_alphanumeric() || c == '_'))
}

#[test]
fn 소스_검사의_이름_일치는_식별자_경계에서_시작한다() {
    assert!(mentions("h.channel_open_x11(a)", "channel_open_x11"));
    assert!(mentions("Session::channel_open_x11", "channel_open_x11"));
    assert!(!mentions("async fn server_channel_open_x11(", "channel_open_x11"));
    assert!(mentions("x.agent_forward(true)", "agent_forward"));
    assert!(!mentions("fn server_channel_open_agent_forward(", "agent_forward"));
}

/// `use russh::…;` 문이 `connect` 를 이름으로 들여오거나(`{connect, …}`·`as`) `client` 를 통째로(`client::*`)·다른 이름으로 들여오는가
fn imports_russh_connect(text: &str) -> bool {
    let code: String = text.lines().filter(|l| !l.trim_start().starts_with("//")).collect::<Vec<_>>().join("
");
    code.match_indices("use russh").any(|(at, _)| {
        let stmt = &code[at..code[at..].find(';').map_or(code.len(), |e| at + e)];
        let tokens: Vec<&str> = stmt.split(|c: char| !(c.is_alphanumeric() || c == '_')).collect();
        tokens.contains(&"connect") || stmt.contains("client::*") || stmt.contains("client as")
    })
}

#[test]
fn russh_connect_들여오기_검사는_흔한_우회_형태를_잡는다() {
    for bad in [
        "use russh::client::connect;",
        "use russh::client::{self, connect, Handle};",
        "use russh::client::{
    Handle,
    connect,
};",
        "use russh::client::connect as c;",
        "use russh::client::*;",
        "use russh::client as ssh;",
    ] {
        assert!(imports_russh_connect(bad), "{bad}");
    }
    for good in ["use russh::client::{self, Handle, Msg};", "use russh::client::connect_stream;", "// use russh::client::connect;
use russh::Channel;", "let r = tcp_connect(a);"] {
        // connect_stream 은 별도 규칙(허용 파일 제한)이 본다
        assert!(!imports_russh_connect(good), "{good}");
    }
}

#[tokio::test]
async fn 서버가_인증_뒤에_여는_채널은_종류를_가리지_않고_모두_거부한다() {
    let e = Env::with(|o| o.open_server_channels = true).await;
    let ex = e.ex();
    assert_eq!(run(&ex, &e.repo, &["rev-parse", "HEAD"]).await.unwrap().code, 0);
    wait_for("서버가 연 채널 7종의 결과", || e.server.server_opens().len() == 7).await;
    let opens = e.server.server_opens();
    let kinds: Vec<&str> = opens.iter().map(|(k, _)| k.as_str()).collect();
    for k in ["auth-agent", "session", "direct-tcpip", "direct-streamlocal", "forwarded-tcpip", "forwarded-streamlocal", "x11"] {
        assert!(kinds.contains(&k), "{k} 를 시험하지 않았다: {kinds:?}");
    }
    assert!(opens.iter().all(|(_, r)| r == "AdministrativelyProhibited"), "클라이언트가 받아들였다: {opens:?}");
    // 거부한 뒤에도 같은 연결이 정상으로 쓰인다
    assert_eq!(run(&ex, &e.repo, &["rev-parse", "HEAD"]).await.unwrap().code, 0);
    assert_eq!(e.server.connections(), 1);
    ex.dispose();
}

#[tokio::test]
async fn 서버가_exec_를_거부하면_채널을_닫고_닫힘을_확인한_뒤_슬롯을_돌려준다() {
    let e = Env::with(|o| o.exec = ExecMode::Reject).await;
    let ex = e.ex();
    for _ in 0..8 {
        let m = message(run(&ex, &e.repo, &["rev-parse", "HEAD"]).await);
        assert!(m.contains("거부"), "{m}");
    }
    wait_for("거부된 채널이 서버에서 닫힘", || e.server.lingering_channels() == 0).await;
    wait_for("슬롯 반환", || ex.free_slots() == 6).await;
    assert_eq!(e.server.connections(), 1);
    ex.dispose();
}

// ── 실제 sshd(수동) ────────────────────────────────────────────────────────────

/// 실제 sshd 에 접속한다 — 사용자의 `~/.ssh`·agent 를 **읽는다**(설정·known_hosts·키·agent). 자동 실행되지 않는다.
/// `ANTLER_SSH_TEST_HOST`(별칭 또는 user@host[:port])·`ANTLER_SSH_TEST_REPO`(원격 저장소 경로) 가 필요하다
#[tokio::test]
#[ignore = "실제 sshd 필요: ANTLER_SSH_TEST_HOST, ANTLER_SSH_TEST_REPO"]
async fn 실제_sshd_에서_rev_parse() {
    let host = std::env::var("ANTLER_SSH_TEST_HOST").expect("ANTLER_SSH_TEST_HOST");
    let repo = std::env::var("ANTLER_SSH_TEST_REPO").expect("ANTLER_SSH_TEST_REPO");
    let home = std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")).expect("홈");
    let ex = SshExecutor::new(&host, SshOptions::for_home(&home));
    let r = ex.run(&repo, &["rev-parse", "--is-inside-work-tree"], &RunOpts::default()).await.unwrap();
    assert_eq!(String::from_utf8_lossy(&r.stdout).trim(), "true");
    ex.dispose();
}
