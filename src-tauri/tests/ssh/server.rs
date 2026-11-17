//! SSH 테스트 서버 하네스 — **프로세스 안 `russh::server`(127.0.0.1)** 만 쓴다. 실제 원격 호스트·사용자의 `~/.ssh`·ssh-agent 는 건드리지 않는다
//! (TS 의 `tests/sshHarness.ts` 이식). 서버는 전용 스레드의 자체 런타임에서 돈다 — `#[tokio::test]` 런타임에 띄우면 그 테스트가 끝날 때 서버가 죽는다.
//!
//! 원격 셸 흉내: `env K=V… git -C <repo> <전역 인자> <args…>`(`build_remote_command` 의 결과)를 [`unquote`] 로 풀어 **실행기 `LocalExecutor`** 로 돌린다
//! (git 설정 격리·트리 종료가 같다). 풀린 `env` 값이 `GIT_ENV` 와 같은지는 [`TestServer::exec_log`] 로 시험이 확인한다.
//! 가짜 명령 `antler-fake flood <바이트> [패킷 크기]`(출력만 쏟는다)·`antler-fake hold`(취소될 때까지 산다)는 git 이 필요 없는 흐름 제어 시험용이다.

#![allow(dead_code)]

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use antler_lib::git::exec::ssh::SHELL_PROBE;
use antler_lib::git::exec::{ExecError, LocalExecutor, RunOpts};
use russh::keys::{Algorithm, PrivateKey, PublicKey};
use russh::server::{self, Auth, Msg, Server as _, Session, VerifiedPublicKey};
use russh::{Channel, ChannelId, Sig};
use tokio::io::AsyncReadExt;
use tokio::net::{TcpListener, TcpStream};
use tokio_util::sync::CancellationToken;

/// 서버 전용 런타임(프로세스 수명)
fn runtime() -> &'static tokio::runtime::Runtime {
    static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread().worker_threads(2).thread_name("ssh-test-server").enable_all().build().expect("서버 런타임")
    })
}

/// POSIX 셸 인용 해제(작은따옴표·큰따옴표·백슬래시). 원격 셸이 하는 일을 흉내 내 argv 를 복원한다
pub fn unquote(cmd: &str) -> Result<Vec<String>, String> {
    let chars: Vec<char> = cmd.chars().collect();
    let mut args = Vec::new();
    let mut cur = String::new();
    let mut has = false;
    let mut i = 0;
    while i < chars.len() {
        let ch = chars[i];
        match ch {
            '\'' => {
                let Some(end) = chars[i + 1..].iter().position(|c| *c == '\'') else {
                    return Err("닫히지 않은 작은따옴표".into());
                };
                cur.extend(&chars[i + 1..i + 1 + end]);
                i += end + 1;
                has = true;
            }
            '"' => {
                i += 1;
                while i < chars.len() && chars[i] != '"' {
                    if chars[i] == '\\' && i + 1 < chars.len() && "\"\\$`".contains(chars[i + 1]) {
                        i += 1;
                    }
                    cur.push(chars[i]);
                    i += 1;
                }
                has = true;
            }
            '\\' => {
                i += 1;
                if let Some(c) = chars.get(i) {
                    cur.push(*c);
                }
                has = true;
            }
            ' ' | '\t' | '\n' => {
                if has {
                    args.push(std::mem::take(&mut cur));
                }
                has = false;
            }
            c => {
                cur.push(c);
                has = true;
            }
        }
        i += 1;
    }
    if has {
        args.push(cur);
    }
    Ok(args)
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Shell {
    Posix,
    /// Windows cmd 처럼 작은따옴표를 문자 그대로 돌려준다(클라이언트의 셸 확인이 거부해야 한다)
    Cmd,
}

/// 프로브가 아닌 exec 요청에 서버가 어떻게 답하는가
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ExecMode {
    Normal,
    /// 요청을 거부한다(channel_failure). 채널은 클라이언트가 닫을 때까지 서버에 남는다
    Reject,
}

pub struct Opts {
    pub authorized: Vec<PublicKey>,
    pub host_key: Option<PrivateKey>,
    pub shell: Shell,
    /// 서버가 채널마다 내주는 윈도우(바이트). 작으면 흐름 제어가 빨리 걸린다
    pub window: u32,
    /// 인증이 끝나면 서버가 클라이언트에게 채널 7종(agent·session·direct-tcpip·direct-streamlocal·forwarded-tcpip·forwarded-streamlocal·x11)을 연다.
    /// 결과는 [`TestServer::server_opens`] 로 본다(클라이언트가 거부해야 한다)
    pub open_server_channels: bool,
    pub exec: ExecMode,
}

impl Opts {
    pub fn new(authorized: Vec<PublicKey>) -> Self {
        Opts { authorized, host_key: None, shell: Shell::Posix, window: 2 * 1024 * 1024, open_server_channels: false, exec: ExecMode::Normal }
    }
}

pub fn new_key() -> PrivateKey {
    PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).expect("키 생성")
}

#[derive(Default)]
struct Shared {
    connections: AtomicUsize,
    /// 지금 열려 있는 연결 수(`new_client` +1, 핸들러가 버려질 때 -1 — 세션이 끝나면 핸들러가 버려진다)
    open: AtomicUsize,
    children: AtomicUsize,
    /// 가짜 명령이 지금까지 보낸 바이트(흐름 제어 시험용)
    flooded: AtomicUsize,
    /// 열려 있는 클라이언트 세션(dropClients 용)
    handles: Mutex<Vec<server::Handle>>,
    /// 받은 exec 명령(셸 확인 포함, 받은 순서)
    exec_log: Mutex<Vec<String>>,
    /// 받은 direct-tcpip 요청(`host:port`)
    tunnels: Mutex<Vec<String>>,
    /// 공개키 인증을 시도한 연결 수(연결당 한 번만 센다)
    auth_attempts: AtomicUsize,
    /// exec 를 거부했는데 아직 클라이언트가 닫지 않은 채널 수(서버가 먼저 닫는 채널은 핸들러가 불리지 않아 세지 않는다)
    lingering_channels: AtomicUsize,
    /// 서버가 연 채널의 결과(종류, 결과). `accepted` 이면 클라이언트가 받아들인 것이다
    server_opens: Mutex<Vec<(String, String)>>,
    /// true 면 세션 채널 열기 응답을 `open_gate` 가 풀어 줄 때까지 미룬다([`TestServer::hold_opens`])
    hold_opens: AtomicBool,
    open_gate: OpenGate,
    /// 응답을 미루고 있는 채널 열기 수
    held_opens: AtomicUsize,
    /// 열기 응답을 미룬 세션 채널 중 exec 를 받지 않았고 클라이언트도 닫지 않은 수
    idle_channels: AtomicUsize,
}

/// 0 허가로 시작하는 세마포어(`Shared` 의 `Default` 를 위해 감싼다)
struct OpenGate(tokio::sync::Semaphore);

impl Default for OpenGate {
    fn default() -> Self {
        OpenGate(tokio::sync::Semaphore::new(0))
    }
}

pub struct TestServer {
    pub port: u16,
    pub host_key: PublicKey,
    shared: Arc<Shared>,
    shutdown: server::RunningServerHandle,
}

impl TestServer {
    pub async fn start(opts: Opts) -> TestServer {
        runtime().spawn(Self::start_inner(opts)).await.expect("서버 시작")
    }

    async fn start_inner(opts: Opts) -> TestServer {
        let host = opts.host_key.unwrap_or_else(new_key);
        let host_pub = host.public_key().clone();
        let config = server::Config {
            keys: vec![host],
            auth_rejection_time: Duration::from_millis(1),
            auth_rejection_time_initial: Some(Duration::ZERO),
            inactivity_timeout: None,
            window_size: opts.window,
            ..Default::default()
        };
        let shared = Arc::new(Shared::default());
        let socket = TcpListener::bind(("127.0.0.1", 0)).await.expect("bind");
        let port = socket.local_addr().expect("addr").port();
        let mut srv =
            Srv { shared: shared.clone(), authorized: Arc::new(opts.authorized), shell: opts.shell, open_server_channels: opts.open_server_channels, exec: opts.exec };
        let (tx, rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let running = srv.run_on_socket(Arc::new(config), &socket);
            let _ = tx.send(running.handle());
            let _ = running.await;
        });
        let shutdown = rx.await.expect("서버 핸들");
        TestServer { port, host_key: host_pub, shared, shutdown }
    }

    /// 지금까지 받은 SSH 연결 수(TCP 접속 기준)
    pub fn connections(&self) -> usize {
        self.shared.connections.load(Ordering::SeqCst)
    }

    /// 지금 열려 있는 연결 수 — 클라이언트가 연결을 **실제로** 닫았는지 본다(`is_connected` 는 클라이언트 쪽 장부일 뿐이다)
    pub fn open_connections(&self) -> usize {
        self.shared.open.load(Ordering::SeqCst)
    }

    /// 실행 중인 원격 자식(git·가짜 명령) 수
    pub fn active_children(&self) -> usize {
        self.shared.children.load(Ordering::SeqCst)
    }

    /// 가짜 명령 `flood` 가 지금까지 보낸 바이트
    pub fn flooded(&self) -> usize {
        self.shared.flooded.load(Ordering::SeqCst)
    }

    /// 서버가 받은 exec 명령들(셸 확인 포함)
    pub fn exec_log(&self) -> Vec<String> {
        self.shared.exec_log.lock().unwrap().clone()
    }

    /// 공개키 인증을 시도한 연결 수
    pub fn auth_attempts(&self) -> usize {
        self.shared.auth_attempts.load(Ordering::SeqCst)
    }

    /// exec 를 거부했는데 아직 클라이언트가 닫지 않은 채널 수(`ExecMode::Reject`)
    pub fn lingering_channels(&self) -> usize {
        self.shared.lingering_channels.load(Ordering::SeqCst)
    }

    /// 서버가 클라이언트에게 연 채널의 결과들(`open_server_channels`)
    pub fn server_opens(&self) -> Vec<(String, String)> {
        self.shared.server_opens.lock().unwrap().clone()
    }

    /// 이후의 세션 채널 열기 응답을 미룬다(`on`) / 풀고 미뤄 둔 것을 모두 응답한다(`!on`). 연결 직후의 셸 확인(프로브)도 미루므로 연결을 맺은 뒤에 켠다
    pub fn hold_opens(&self, on: bool) {
        self.shared.hold_opens.store(on, Ordering::SeqCst);
        if !on {
            self.shared.open_gate.0.add_permits(1 << 16);
        }
    }

    /// 응답을 미루고 있는 채널 열기 수
    pub fn held_opens(&self) -> usize {
        self.shared.held_opens.load(Ordering::SeqCst)
    }

    /// 열렸지만 exec 를 받지 않았고 클라이언트도 닫지 않은 세션 채널 수(응답을 미룬 뒤 늦게 열린 채널이 정리됐는지 본다)
    pub fn idle_channels(&self) -> usize {
        self.shared.idle_channels.load(Ordering::SeqCst)
    }

    /// 서버가 받은 direct-tcpip 목적지들
    pub fn tunnels(&self) -> Vec<String> {
        self.shared.tunnels.lock().unwrap().clone()
    }

    /// 열려 있는 모든 클라이언트 연결을 서버 쪽에서 끊는다
    pub async fn drop_clients(&self) {
        let handles: Vec<server::Handle> = std::mem::take(&mut *self.shared.handles.lock().unwrap());
        runtime()
            .spawn(async move {
                for h in handles {
                    let _ = h.disconnect(russh::Disconnect::ByApplication, "test".into(), "en".into()).await;
                }
            })
            .await
            .expect("끊기");
    }

    pub fn shutdown(&self) {
        self.shutdown.shutdown("test".into());
    }
}

#[derive(Clone)]
struct Srv {
    shared: Arc<Shared>,
    authorized: Arc<Vec<PublicKey>>,
    shell: Shell,
    open_server_channels: bool,
    exec: ExecMode,
}

impl server::Server for Srv {
    type Handler = Conn;
    fn new_client(&mut self, _peer: Option<SocketAddr>) -> Conn {
        self.shared.connections.fetch_add(1, Ordering::SeqCst);
        self.shared.open.fetch_add(1, Ordering::SeqCst);
        Conn {
            shared: self.shared.clone(),
            authorized: self.authorized.clone(),
            shell: self.shell,
            open_server_channels: self.open_server_channels,
            exec: self.exec,
            auth_counted: false,
            opened: HashSet::new(),
            gated: HashSet::new(),
            channels: HashMap::new(),
            tokens: HashMap::new(),
            conn: CancellationToken::new(),
        }
    }
}

struct Conn {
    shared: Arc<Shared>,
    authorized: Arc<Vec<PublicKey>>,
    shell: Shell,
    open_server_channels: bool,
    exec: ExecMode,
    auth_counted: bool,
    /// exec 를 거부한 채널 id(`lingering_channels` 장부)
    opened: HashSet<ChannelId>,
    /// 열기 응답을 미룬 채널 id(`idle_channels` 장부 — exec 를 받거나 닫히면 뺀다)
    gated: HashSet<ChannelId>,
    channels: HashMap<ChannelId, Channel<Msg>>,
    tokens: HashMap<ChannelId, CancellationToken>,
    /// 이 연결이 끝나면 취소된다(sshd 처럼 연결이 끊기면 세션 프로세스도 정리한다)
    conn: CancellationToken,
}

impl Drop for Conn {
    fn drop(&mut self) {
        self.conn.cancel();
        self.shared.open.fetch_sub(1, Ordering::SeqCst);
        self.shared.lingering_channels.fetch_sub(self.opened.len(), Ordering::SeqCst);
        self.shared.idle_channels.fetch_sub(self.gated.len(), Ordering::SeqCst);
    }
}

impl server::Handler for Conn {
    type Error = russh::Error;

    async fn auth_publickey(&mut self, _user: &str, key: VerifiedPublicKey<'_>) -> Result<Auth, Self::Error> {
        if !std::mem::replace(&mut self.auth_counted, true) {
            self.shared.auth_attempts.fetch_add(1, Ordering::SeqCst);
        }
        if self.authorized.iter().any(|k| k.key_data() == key.public_key.key_data()) {
            Ok(Auth::Accept)
        } else {
            Ok(Auth::reject())
        }
    }

    /// 인증 뒤 서버 쪽에서 채널을 연다(클라이언트가 거부하는지 보는 시험용). 열고 결과만 적는다 — 받아들여진 채널은 바로 버린다
    async fn auth_succeeded(&mut self, session: &mut Session) -> Result<(), Self::Error> {
        if !self.open_server_channels {
            return Ok(());
        }
        let (handle, shared) = (session.handle(), self.shared.clone());
        tokio::spawn(async move {
            let results = [
                ("auth-agent", handle.channel_open_agent().await),
                ("session", handle.channel_open_session().await),
                ("direct-tcpip", handle.channel_open_direct_tcpip("127.0.0.1", 1, "127.0.0.1", 2).await),
                ("direct-streamlocal", handle.channel_open_direct_streamlocal("/tmp/x").await),
                ("forwarded-tcpip", handle.channel_open_forwarded_tcpip("127.0.0.1", 1, "127.0.0.1", 2).await),
                ("forwarded-streamlocal", handle.channel_open_forwarded_streamlocal("/tmp/x").await),
                ("x11", handle.channel_open_x11("127.0.0.1", 3).await),
            ];
            for (kind, r) in results {
                let text = match r {
                    Ok(_) => "accepted".to_string(),
                    Err(russh::Error::ChannelOpenFailure(reason)) => format!("{reason:?}"),
                    Err(e) => format!("error {e:?}"),
                };
                shared.server_opens.lock().unwrap().push((kind.to_string(), text));
            }
        });
        Ok(())
    }

    async fn channel_open_session(&mut self, channel: Channel<Msg>, reply: server::ChannelOpenHandle, session: &mut Session) -> Result<(), Self::Error> {
        self.shared.handles.lock().unwrap().push(session.handle());
        let id = channel.id();
        self.channels.insert(id, channel);
        if self.shared.hold_opens.load(Ordering::SeqCst) {
            self.gated.insert(id);
            self.shared.idle_channels.fetch_add(1, Ordering::SeqCst);
            // 응답만 미룬다(세션 루프는 계속 돈다 — 다른 채널·정리 요청이 막히지 않게 별도 태스크로)
            let shared = self.shared.clone();
            shared.held_opens.fetch_add(1, Ordering::SeqCst);
            tokio::spawn(async move {
                if let Ok(p) = shared.open_gate.0.acquire().await {
                    p.forget();
                }
                shared.held_opens.fetch_sub(1, Ordering::SeqCst);
                reply.accept().await;
            });
            return Ok(());
        }
        reply.accept().await;
        Ok(())
    }

    async fn exec_request(&mut self, id: ChannelId, data: &[u8], session: &mut Session) -> Result<(), Self::Error> {
        if self.exec == ExecMode::Reject && data != SHELL_PROBE.as_bytes() {
            session.channel_failure(id)?;
            // 거부한 채널은 클라이언트가 닫을 때(channel_close)까지 남는다 — 그 수를 센다
            if self.opened.insert(id) {
                self.shared.lingering_channels.fetch_add(1, Ordering::SeqCst);
            }
            return Ok(());
        }
        let Some(channel) = self.channels.remove(&id) else {
            session.channel_failure(id)?;
            return Ok(());
        };
        if self.gated.remove(&id) {
            self.shared.idle_channels.fetch_sub(1, Ordering::SeqCst);
        }
        session.channel_success(id)?;
        let command = String::from_utf8_lossy(data).into_owned();
        self.shared.exec_log.lock().unwrap().push(command.clone());
        let token = self.conn.child_token();
        self.tokens.insert(id, token.clone());
        let (shared, shell) = (self.shared.clone(), self.shell);
        tokio::spawn(async move {
            serve_exec(shared, shell, channel, command, token).await;
        });
        Ok(())
    }

    async fn signal(&mut self, id: ChannelId, _signal: Sig, _session: &mut Session) -> Result<(), Self::Error> {
        if let Some(t) = self.tokens.get(&id) {
            t.cancel();
        }
        Ok(())
    }

    async fn channel_close(&mut self, id: ChannelId, _session: &mut Session) -> Result<(), Self::Error> {
        if let Some(t) = self.tokens.remove(&id) {
            t.cancel();
        }
        self.channels.remove(&id);
        if self.gated.remove(&id) {
            self.shared.idle_channels.fetch_sub(1, Ordering::SeqCst);
        }
        if self.opened.remove(&id) {
            self.shared.lingering_channels.fetch_sub(1, Ordering::SeqCst);
        }
        Ok(())
    }

    /// ProxyJump 용 direct-tcpip: 대상 포트로 그대로 이어 준다
    async fn channel_open_direct_tcpip(
        &mut self,
        channel: Channel<Msg>,
        host: &str,
        port: u32,
        _originator: &str,
        _originator_port: u32,
        reply: server::ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.shared.tunnels.lock().unwrap().push(format!("{host}:{port}"));
        let target = (host.to_string(), port as u16);
        let token = self.conn.child_token();
        tokio::spawn(async move {
            let Ok(mut tcp) = TcpStream::connect(target).await else {
                reply.reject(russh::ChannelOpenFailure::ConnectFailed).await;
                return;
            };
            reply.accept().await;
            let mut stream = channel.into_stream();
            tokio::select! {
                _ = token.cancelled() => {}
                _ = tokio::io::copy_bidirectional(&mut stream, &mut tcp) => {}
            }
        });
        Ok(())
    }
}

/// 자식 수를 세는 가드(Drop 에서 줄인다)
struct ChildCount(Arc<Shared>);

impl ChildCount {
    fn new(shared: &Arc<Shared>) -> Self {
        shared.children.fetch_add(1, Ordering::SeqCst);
        ChildCount(shared.clone())
    }
}

impl Drop for ChildCount {
    fn drop(&mut self) {
        self.0.children.fetch_sub(1, Ordering::SeqCst);
    }
}

async fn finish(channel: &Channel<Msg>, code: Option<u32>) {
    if let Some(c) = code {
        let _ = channel.exit_status(c).await;
    }
    let _ = channel.eof().await;
    let _ = channel.close().await;
}

async fn serve_exec(shared: Arc<Shared>, shell: Shell, channel: Channel<Msg>, command: String, token: CancellationToken) {
    if command == SHELL_PROBE {
        let out: &[u8] = if shell == Shell::Cmd { b"'an''tler'\r\n" } else { b"antler\n" };
        let _ = channel.data(out).await;
        finish(&channel, Some(0)).await;
        return;
    }
    let Ok(argv) = unquote(&command) else {
        finish(&channel, Some(2)).await;
        return;
    };
    match argv.first().map(String::as_str) {
        Some("antler-fake") => fake(&shared, &channel, &argv[1..], token).await,
        Some("env") => git(&shared, &channel, &argv[1..], token).await,
        _ => {
            let _ = channel.extended_data(1, &b"command not found\n"[..]).await;
            finish(&channel, Some(127)).await;
        }
    }
}

async fn fake(shared: &Arc<Shared>, channel: &Channel<Msg>, argv: &[String], token: CancellationToken) {
    let _count = ChildCount::new(shared);
    match argv.first().map(String::as_str) {
        Some("flood") => {
            let total: usize = argv.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
            let chunk_len: usize = argv.get(2).and_then(|s| s.parse().ok()).unwrap_or(32 * 1024);
            let chunk = vec![b'x'; chunk_len.max(1)];
            let mut sent = 0;
            while sent < total {
                let n = chunk.len().min(total - sent);
                tokio::select! {
                    _ = token.cancelled() => { finish(channel, None).await; return; }
                    r = channel.data(&chunk[..n]) => { if r.is_err() { return; } shared.flooded.fetch_add(n, Ordering::SeqCst); }
                }
                sent += n;
            }
            finish(channel, Some(0)).await;
        }
        Some("hold") => {
            token.cancelled().await;
            finish(channel, None).await;
        }
        _ => finish(channel, Some(127)).await,
    }
}

/// `env K=V… git -C <repo> <전역 인자 8개> <args…>` 를 실행기로 돌린다
/// 원격 명령의 `env K=V …`(`env` 뒤 `git` 앞)에서 fetch 환경을 읽는다. `GIT_SSH_COMMAND` 가 없으면 fetch 가 아니다
fn fetch_env_of(env_args: &[String]) -> Option<antler_lib::git::exec::FetchEnv> {
    let value = |name: &str| env_args.iter().skip(1).find_map(|a| a.strip_prefix(name).and_then(|r| r.strip_prefix('=')).map(str::to_string));
    let ssh_command = value("GIT_SSH_COMMAND")?;
    let allow_file = value("GIT_ALLOW_PROTOCOL").is_some_and(|p| p.split(':').any(|x| x == "file"));
    Some(antler_lib::git::exec::FetchEnv { allow_file, ssh_command })
}

async fn git(shared: &Arc<Shared>, channel: &Channel<Msg>, argv: &[String], token: CancellationToken) {
    let mut i = 0;
    while i < argv.len() && argv[i] != "git" {
        i += 1;
    }
    let rest = &argv[(i + 1).min(argv.len())..];
    // -C <repo> 와 전역 옵션 8개를 걷어내면 실행기가 같은 것을 다시 붙인다
    if rest.len() < 10 || rest[0] != "-C" {
        let _ = channel.extended_data(1, &b"unexpected argv\n"[..]).await;
        finish(channel, Some(2)).await;
        return;
    }
    let (repo, args) = (&rest[1], &rest[10..]);
    // fetch(원격 명령의 `env` 에 GIT_SSH_COMMAND 가 실린 것): 실행기의 fetch 환경으로 돌린다 — `GIT_ALLOW_PROTOCOL`(file 허용 여부)·`GIT_SSH_COMMAND` 가 자식 git 에 닿는다.
    // 일반 env 를 그대로 자식에 넣지 않는다(제품 코드와 같은 닫힌 `FetchEnv` 로만 — TS 하네스는 env 를 그대로 적용한다)
    if let Some(fetch_env) = fetch_env_of(&argv[..i]) {
        let _count = ChildCount::new(shared);
        let opts = RunOpts { cancel: Some(token.clone()), fetch_env: Some(fetch_env), ..RunOpts::default() };
        match LocalExecutor::default().run(repo, args, &opts).await {
            Ok(out) => {
                if !out.stdout.is_empty() {
                    let _ = channel.data(&out.stdout[..]).await;
                }
                if !out.stderr.is_empty() {
                    let _ = channel.extended_data(1, out.stderr.as_bytes()).await;
                }
                finish(channel, Some(u32::try_from(out.code).unwrap_or(255))).await;
            }
            Err(_) => finish(channel, None).await,
        }
        return;
    }
    // 저장소 경로가 `antler-fake:<가짜 명령>[:인자…]` 면 git 대신 가짜 명령을 돌린다 — `run` 은 `build_remote_command` 의 결과만 보내므로
    // (임의 명령을 보낼 길이 없다) 오래 사는 원격 프로세스·큰 출력은 이 경로로 만든다
    if let Some(fake_cmd) = repo.strip_prefix("antler-fake:") {
        let parts: Vec<String> = fake_cmd.split(':').map(String::from).collect();
        fake(shared, channel, &parts, token).await;
        return;
    }
    let _count = ChildCount::new(shared);
    let mut stream = match LocalExecutor::default().stream(repo, args, &RunOpts::with_cancel(token.clone())) {
        Ok(s) => s,
        Err(_) => {
            finish(channel, Some(255)).await;
            return;
        }
    };
    let mut buf = vec![0u8; 32 * 1024];
    loop {
        let n = tokio::select! {
            _ = token.cancelled() => { drop(stream); finish(channel, None).await; return; }
            r = stream.read(&mut buf) => match r { Ok(n) => n, Err(_) => { drop(stream); finish(channel, None).await; return; } },
        };
        if n == 0 {
            break;
        }
        // 클라이언트가 읽지 않으면 여기서 윈도우가 막히고 git 도 멈춘다(원격 backpressure)
        let sent = tokio::select! {
            _ = token.cancelled() => { drop(stream); finish(channel, None).await; return; }
            r = channel.data(&buf[..n]) => r,
        };
        if sent.is_err() {
            return;
        }
    }
    match stream.finish().await {
        Ok(()) => finish(channel, Some(0)).await,
        Err(ExecError::Git { code, stderr, .. }) => {
            let _ = channel.extended_data(1, stderr.as_bytes()).await;
            finish(channel, Some(u32::try_from(code).unwrap_or(255))).await;
        }
        Err(_) => finish(channel, None).await,
    }
}
