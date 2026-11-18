//! russh 연결 수명 관리(`src/main/executors/ssh/connection.ts`): 지연 연결·재사용·재연결, ProxyJump 체인, 채널 동시성 제한, 유휴 종료, 원격 셸 확인.
//! Windows OpenSSH 에는 ControlMaster 가 없으므로 프로세스 안에서 연결 하나를 직접 유지한다.
//!
//! 방어(TS 의 `connection.ts` 가 하던 것 — 빠뜨리면 PR 1 의 순수 로직이 막은 것이 연결에서 새 나간다):
//! - **홉마다 `unsupported` 거부**: 대상과 모든 ProxyJump 홉을 `resolve_host` 로 해석해 하나라도 `unsupported`(ProxyCommand·Match·비 ASCII·포트 범위·RevokedHostKeys)면 연결 전에 거부한다.
//! - **홉마다 known_hosts 판정**(`verify_host_key_text`, TOFU·쓰기 없음). 알려진 키 종류로 서버 호스트 키 알고리즘을 제한한다. `HostKeyAlias` 가 있으면 별칭·포트 22 로 조회한다.
//! - **원격 셸 확인**([`SHELL_PROBE`]): 접속 직후 로그인 셸이 POSIX 인용을 해석하는지 본다. 확인 전에는 사용자 값이 든 명령을 보내지 않는다.
//! - 인증은 agent·키 파일뿐이다(`auth.rs`). 비밀번호·keyboard-interactive·agent 전달·X11·포트 포워딩은 쓰지 않는다(clippy.toml).
//!
//! **채널은 태스크가 소유한다.** Rust 에서는 future 를 버리는 것이 취소이고 `Drop` 은 동기라 원격에 KILL/close 를 보낼 수 없다. 그래서 exec 채널을 연 순간부터는
//! 스폰된 태스크가 채널·슬롯(세마포어 허가)을 쥐고, 호출자가 버려지면(취소 토큰) 태스크가 KILL → close → 닫힘 확인까지 하고 **그 뒤에** 슬롯을 돌려준다.

use std::borrow::Cow;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use russh::client::{self, ChannelOpenHandle, Handle, Msg, Session};
use russh::keys::{Algorithm, EcdsaCurve, HashAlg, PublicKeyOrCertificate};
use russh::{Channel, ChannelMsg, ChannelOpenFailure, ChannelReadHalf, ChannelWriteHalf, Preferred, Sig};
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, oneshot, OwnedSemaphorePermit, Semaphore};
use tokio_util::sync::CancellationToken;

use super::auth::{self, AgentSetting};
use super::config::{load_ssh_config, resolve_host, Ctx, RealFs, ResolvedHost};
use super::known_hosts::{known_host_key_algorithms, read_known_hosts, verify_host_key_text};
use super::sftp::{self, SftpError};
use super::stream::{End, Event, SshStream, EVENT_COST, STREAM_AHEAD_LIMIT};
use crate::git::exec::common::{to_strings, RunOpts, RunResult, DEFAULT_MAX_BUFFER, MAX_STDERR};
use crate::git::exec::error::ExecError;
use crate::git::parsers::js_trim;
use crate::repos::scan::RemoteDirListing;

/// 원격 로그인 셸 확인용 고정 명령. 원격 명령은 POSIX 작은따옴표로 인용하는데, 로그인 셸이 cmd.exe·PowerShell 이면 작은따옴표가 인용이 아니라서
/// 경로 안의 `&`·`|` 등이 명령으로 해석될 수 있다. 사용자 값이 든 명령을 보내기 전에 연결마다 한 번 확인한다:
/// sh/bash/zsh/fish 는 인접한 두 따옴표 문자열을 이어 `antler` 를 출력하고, cmd 는 따옴표를 그대로, PowerShell 은 `an'tler` 를 출력한다.
pub const SHELL_PROBE: &str = "echo 'an''tler'";
const SHELL_PROBE_EXPECT: &str = "antler";
const CONNECTION_LOST: &str = "SSH 연결이 끊겨 원격 git 이 끝나기 전에 종료되었습니다";
const DISPOSED: &str = "SSH 연결이 이미 종료되었습니다";
/// 채널을 열고 exec 요청을 보내는 데 주는 시간(응답이 없는 서버에서 영원히 기다리지 않는다)
const OPEN_TIMEOUT: Duration = Duration::from_secs(15);
/// 취소·종료 때 KILL·close 요청을 보내는 데 주는 시간
const KILL_TIMEOUT: Duration = Duration::from_secs(2);
/// KILL 뒤 서버가 채널을 닫기를 기다리는 시간(그 뒤에야 슬롯을 돌려준다)
const CLOSE_WAIT: Duration = Duration::from_secs(3);
/// SFTP 다리(채널 ↔ 클라이언트 스트림)의 한 번 복사 크기와 스트림 완충 크기
const SFTP_CHUNK: usize = 32 * 1024;
const SFTP_PIPE: usize = 64 * 1024;
/// 연결 실패를 이 시간 동안 다음 호출과 공유한다(잠금을 얻은 직후 이보다 최근 실패가 있으면 다시 맺지 않고 그 오류로 돌려준다)
const FAILURE_SHARE: Duration = Duration::from_secs(5);
/// 한 연결에서 sshd `MaxSessions`(기본 10)보다 안쪽으로 유지할 동시 채널 수
pub const DEFAULT_MAX_CHANNELS: usize = 6;

/// 시간 제한. 기본값은 TS 와 같고, 시험은 짧은 값을 주입한다(시계를 멈추는 `start_paused` 는 실제 소켓과 맞지 않는다)
#[derive(Debug, Clone, Copy)]
pub struct Timeouts {
    /// 호스트 하나(TCP·키 교환·인증)를 여는 제한
    pub ready: Duration,
    /// keepalive 간격(응답이 3번 연속 없으면 끊는다)
    pub keepalive: Duration,
    /// 채널이 하나도 없는 채로 이만큼 지나면 연결을 닫는다
    pub idle: Duration,
    /// 원격 셸 확인 제한
    pub probe: Duration,
    /// 원격 폴더 목록(SFTP) 한 번의 전체 제한(SFTP 하위 시스템이 시작된 뒤부터 결과가 날 때까지 — 채널 열기·하위 시스템 응답은 15초 제한이 따로 있다)
    pub sftp: Duration,
}

impl Default for Timeouts {
    fn default() -> Self {
        Timeouts {
            ready: Duration::from_secs(15),
            keepalive: Duration::from_secs(15),
            idle: Duration::from_secs(5 * 60),
            probe: Duration::from_secs(10),
            sftp: Duration::from_secs(30),
        }
    }
}

/// 환경변수 조회(주입 가능)
pub type EnvFn = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// 연결 옵션. 홈·환경변수·agent 는 전부 주입한다(시험이 사용자의 `~/.ssh`·agent 를 건드리지 않게)
#[derive(Clone)]
pub struct SshOptions {
    /// `~/.ssh/config`
    pub config_path: String,
    /// 기본 `~/.ssh/known_hosts`(`UserKnownHostsFile` 이 있으면 그쪽)
    pub known_hosts_path: String,
    /// `~` 와 기본 키(`~/.ssh/id_*`) 위치의 기준
    pub home_dir: String,
    pub agent: AgentSetting,
    /// 설정 해석(`IdentityAgent` 의 `$VAR`)과 agent 기본값이 읽는 환경변수
    pub env: EnvFn,
    /// `%u`·기본 User
    pub local_user: String,
    pub timeouts: Timeouts,
    pub max_channels: usize,
}

impl SshOptions {
    /// 홈 기준 기본값(`<홈>/.ssh/config`·`<홈>/.ssh/known_hosts`, 프로세스 환경변수, 로컬 사용자)
    pub fn for_home(home_dir: &str) -> Self {
        let ssh = std::path::Path::new(home_dir).join(".ssh");
        SshOptions {
            config_path: ssh.join("config").to_string_lossy().into_owned(),
            known_hosts_path: ssh.join("known_hosts").to_string_lossy().into_owned(),
            home_dir: home_dir.to_string(),
            agent: AgentSetting::UseConfig,
            env: Arc::new(super::config::process_env),
            local_user: super::config::local_user_name(),
            timeouts: Timeouts::default(),
            max_channels: DEFAULT_MAX_CHANNELS,
        }
    }
}

/// 이 파일의 오류는 모두 Antler 와 원격 호스트 사이의 전송 오류다(연결·호스트 키·인증·셸 확인·채널·종료) — `ExecError::Ssh` 로 구분한다(fetch 가 문구 대신 종류로 분류한다)
fn other(message: impl Into<String>) -> ExecError {
    ExecError::Ssh { message: message.into() }
}

/// 서버 호스트 키를 known_hosts 로 판정하는 클라이언트 핸들러. 거부 사유는 `verdict` 에 남겨 연결 오류 문구로 쓴다
struct HostKeyHandler {
    known_hosts: Arc<String>,
    name: String,
    port: u32,
    verdict: Arc<Mutex<Option<String>>>,
}

impl client::Handler for HostKeyHandler {
    type Error = russh::Error;

    async fn check_server_key(&mut self, key: &PublicKeyOrCertificate) -> Result<bool, Self::Error> {
        let set = |msg: String| *self.verdict.lock().unwrap_or_else(PoisonError::into_inner) = Some(msg);
        // 호스트 인증서(`*-cert-v01`)는 광고하지 않지만 서버가 내밀면 신뢰 기준이 없으므로 거부한다
        let PublicKeyOrCertificate::PublicKey { key, .. } = key else {
            set("호스트 인증서는 지원하지 않습니다(known_hosts 의 일반 호스트 키만 검증합니다)".to_string());
            return Ok(false);
        };
        let Ok(blob) = key.to_bytes() else {
            set("서버 호스트 키를 해석하지 못했습니다".to_string());
            return Ok(false);
        };
        match verify_host_key_text(&self.known_hosts, &self.name, self.port, &blob) {
            Ok(()) => Ok(true),
            Err(message) => {
                set(message);
                Ok(false)
            }
        }
    }

    // 서버가 여는 채널은 전부 거부한다(ssh2 처럼). russh 의 기본 구현은 accept 해서 채널 맵에 쌓이고, 포워딩·agent·X11 데이터가 들어올 길이 열린다.
    // 우리는 포워딩·agent 전달·X11 을 요청하지 않으므로 정상 서버는 이런 채널을 열 이유가 없다
    async fn server_channel_open_session(&mut self, _channel: Channel<Msg>, reply: ChannelOpenHandle, _session: &mut Session) -> Result<(), Self::Error> {
        reject_server_channel(reply).await
    }

    #[allow(clippy::too_many_arguments)]
    async fn server_channel_open_direct_tcpip(
        &mut self,
        _channel: Channel<Msg>,
        _host_to_connect: &str,
        _port_to_connect: u32,
        _originator_address: &str,
        _originator_port: u32,
        reply: ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        reject_server_channel(reply).await
    }

    async fn server_channel_open_direct_streamlocal(
        &mut self,
        _channel: Channel<Msg>,
        _socket_path: &str,
        reply: ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        reject_server_channel(reply).await
    }

    #[allow(clippy::too_many_arguments)]
    async fn server_channel_open_forwarded_tcpip(
        &mut self,
        _channel: Channel<Msg>,
        _connected_address: &str,
        _connected_port: u32,
        _originator_address: &str,
        _originator_port: u32,
        reply: ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        reject_server_channel(reply).await
    }

    async fn server_channel_open_forwarded_streamlocal(
        &mut self,
        _channel: Channel<Msg>,
        _socket_path: &str,
        reply: ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        reject_server_channel(reply).await
    }

    async fn server_channel_open_agent_forward(&mut self, _channel: Channel<Msg>, reply: ChannelOpenHandle, _session: &mut Session) -> Result<(), Self::Error> {
        reject_server_channel(reply).await
    }

    async fn server_channel_open_x11(
        &mut self,
        _channel: Channel<Msg>,
        _originator_address: &str,
        _originator_port: u32,
        reply: ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        reject_server_channel(reply).await
    }
}

/// 서버가 연 채널을 거부한다(`AdministrativelyProhibited`)
async fn reject_server_channel(reply: ChannelOpenHandle) -> Result<(), russh::Error> {
    reply.reject(ChannelOpenFailure::AdministrativelyProhibited).await;
    Ok(())
}

type ClientHandle = Handle<HostKeyHandler>;

/// 연결된 체인(점프 호스트들 + 대상). 점프 호스트의 세션을 버리면 그 위의 터널이 끊기므로 체인 전체를 쥔다
struct Live {
    chain: Vec<ClientHandle>,
}

impl Live {
    fn last(&self) -> &ClientHandle {
        self.chain.last().expect("체인은 비지 않는다")
    }

    fn is_closed(&self) -> bool {
        self.last().is_closed()
    }
}

struct Inner {
    host_spec: String,
    opts: SshOptions,
    permits: Arc<Semaphore>,
    live: Mutex<Option<Arc<Live>>>,
    /// 연결을 동시에 한 번만 맺는다(기다리던 호출은 맺어진 연결을 쓴다)
    connect_lock: tokio::sync::Mutex<()>,
    /// 가장 최근 연결 실패(시각, 문구). 잠금을 기다리던 호출이 같은 실패를 `FAILURE_SHARE` 동안 공유한다 — 거부하는 서버에 호출 수만큼 재연결·재인증하지 않는다
    last_failure: Mutex<Option<(Instant, String)>>,
    disposed: AtomicBool,
    /// 유휴 타이머 세대. 슬롯을 잡으면 올라가 이미 걸린 타이머를 무효로 만든다
    idle_gen: AtomicU64,
    /// 걸려 있는 유휴 타이머(새로 걸거나 슬롯을 잡으면 이전 것을 중단한다 — 호출마다 5분짜리 태스크가 쌓이지 않게)
    idle_timer: Mutex<Option<tokio::task::JoinHandle<()>>>,
    /// `stream` 이 소비자보다 먼저 읽어 둘 수 있는 바이트 상한(운영은 [`STREAM_AHEAD_LIMIT`], 시험은 줄인다)
    stream_ahead: AtomicUsize,
}

/// 호스트 하나의 SSH 연결(프로세스 안에서 재사용)
pub(super) struct Connection {
    inner: Arc<Inner>,
}

/// 채널 슬롯. 버려지면(그 뒤에) 유휴 타이머를 건다
struct Slot {
    permit: Option<OwnedSemaphorePermit>,
    inner: Arc<Inner>,
}

impl Drop for Slot {
    fn drop(&mut self) {
        drop(self.permit.take());
        self.inner.start_idle_if_unused();
    }
}

impl Inner {
    fn current(&self) -> Option<Arc<Live>> {
        let mut g = self.live.lock().unwrap_or_else(PoisonError::into_inner);
        match g.as_ref() {
            Some(l) if !l.is_closed() => Some(l.clone()),
            Some(_) => {
                // 끊긴 연결: 다음 호출이 다시 맺도록 비우고 점프 호스트 세션도 정리한다
                let dead = g.take();
                drop(g);
                if let Some(l) = dead {
                    disconnect_in_background(l);
                }
                None
            }
            None => None,
        }
    }

    fn teardown(&self) {
        let taken = self.live.lock().unwrap_or_else(PoisonError::into_inner).take();
        if let Some(l) = taken {
            disconnect_in_background(l);
        }
    }

    fn start_idle_if_unused(self: &Arc<Self>) {
        if self.disposed.load(Ordering::SeqCst) || self.permits.available_permits() != self.opts.max_channels {
            return;
        }
        let generation = self.idle_gen.fetch_add(1, Ordering::SeqCst) + 1;
        let weak = Arc::downgrade(self);
        let idle = self.opts.timeouts.idle;
        // 런타임 밖(Drop 이 런타임 종료 중에 불린 경우)이면 타이머를 걸지 않는다
        let Ok(rt) = tokio::runtime::Handle::try_current() else { return };
        let timer = rt.spawn(async move {
            tokio::time::sleep(idle).await;
            if let Some(inner) = weak.upgrade() {
                if inner.idle_gen.load(Ordering::SeqCst) == generation && inner.permits.available_permits() == inner.opts.max_channels {
                    inner.teardown();
                }
            }
        });
        self.set_idle_timer(Some(timer));
    }

    /// 걸려 있던 유휴 타이머를 중단하고 `new` 로 바꾼다
    fn set_idle_timer(&self, new: Option<tokio::task::JoinHandle<()>>) {
        let old = std::mem::replace(&mut *self.idle_timer.lock().unwrap_or_else(PoisonError::into_inner), new);
        if let Some(t) = old {
            t.abort();
        }
    }

    async fn acquire(self: &Arc<Self>) -> Result<Slot, ExecError> {
        self.idle_gen.fetch_add(1, Ordering::SeqCst);
        self.set_idle_timer(None);
        match self.permits.clone().acquire_owned().await {
            Ok(permit) => Ok(Slot { permit: Some(permit), inner: self.clone() }),
            Err(_) => Err(other(DISPOSED)),
        }
    }

    async fn live(&self) -> Result<Arc<Live>, ExecError> {
        if self.disposed.load(Ordering::SeqCst) {
            return Err(other(DISPOSED));
        }
        if let Some(l) = self.current() {
            return Ok(l);
        }
        let _guard = self.connect_lock.lock().await;
        if let Some(l) = self.current() {
            return Ok(l);
        }
        if self.disposed.load(Ordering::SeqCst) {
            return Err(other(DISPOSED));
        }
        if let Some((at, message)) = &*self.last_failure.lock().unwrap_or_else(PoisonError::into_inner) {
            if at.elapsed() < FAILURE_SHARE {
                return Err(other(message.clone()));
            }
        }
        let live = match self.do_connect().await {
            Ok(l) => Arc::new(l),
            Err(e) => {
                *self.last_failure.lock().unwrap_or_else(PoisonError::into_inner) = Some((Instant::now(), e.to_string()));
                return Err(e);
            }
        };
        *self.last_failure.lock().unwrap_or_else(PoisonError::into_inner) = None;
        if self.disposed.load(Ordering::SeqCst) {
            disconnect_in_background(live);
            return Err(other(DISPOSED));
        }
        *self.live.lock().unwrap_or_else(PoisonError::into_inner) = Some(live.clone());
        Ok(live)
    }

    /// 설정을 읽어 대상과 모든 홉을 해석한다(동기 — `Ctx` 가 `Sync` 가 아니라 `await` 를 건너 쥐지 않게 따로 뺐다)
    fn resolve_nodes(&self) -> (Vec<ResolvedHost>, usize) {
        // 연결할 때마다 설정을 다시 읽는다(사용자가 ~/.ssh/config 를 고친 뒤 재연결하면 반영)
        let config = load_ssh_config(&self.opts.config_path, &self.opts.home_dir, &RealFs);
        let ctx = Ctx { local_user: &self.opts.local_user, env: &*self.opts.env };
        let target = resolve_host(&config, &self.host_spec, &ctx);
        let hops: Vec<ResolvedHost> = target.proxy_jump.iter().map(|h| resolve_host(&config, &h.spec, &ctx)).collect();
        let hop_count = hops.len();
        (hops.into_iter().chain(std::iter::once(target)).collect(), hop_count)
    }

    async fn do_connect(&self) -> Result<Live, ExecError> {
        let (nodes, hop_count) = self.resolve_nodes();
        for n in &nodes {
            if let Some(why) = &n.unsupported {
                return Err(other(format!(
                    "{}: 지원하지 않는 ssh 설정({why})이 걸려 있어 접속하지 않습니다 — ProxyJump 로 바꾸거나 해당 블록을 정리하세요",
                    n.alias
                )));
            }
        }
        // 점프 호스트 자신의 ProxyJump(중첩)는 따르지 않는다. TS 는 조용히 무시하지만 그러면 사용자가 의도한 경로가 아닌 곳으로 접속하게 되므로 거부한다
        if let Some(n) = nodes[..hop_count].iter().find(|n| !n.proxy_jump.is_empty()) {
            return Err(other(format!("{}: 점프 호스트의 ProxyJump(중첩)는 지원하지 않습니다 — 대상의 ProxyJump 에 홉을 나열하세요", n.alias)));
        }
        let mut chain: Vec<ClientHandle> = Vec::new();
        let result = self.connect_chain(&nodes, hop_count, &mut chain).await;
        match result {
            Ok(()) => Ok(Live { chain }),
            Err(e) => {
                if !chain.is_empty() {
                    disconnect_in_background(Arc::new(Live { chain }));
                }
                Err(e)
            }
        }
    }

    async fn connect_chain(&self, nodes: &[ResolvedHost], hop_count: usize, chain: &mut Vec<ClientHandle>) -> Result<(), ExecError> {
        for (i, node) in nodes.iter().enumerate() {
            let prefix = if i < hop_count { format!("[점프 호스트 {}] ", node.alias) } else { String::new() };
            let handle = match i {
                0 => self.open_hop(node, None, &prefix).await?,
                _ => {
                    let from = &nodes[i - 1];
                    let via = chain.last().expect("앞 홉이 있다");
                    self.open_hop(node, Some((via, from)), &prefix).await?
                }
            };
            chain.push(handle);
        }
        let last = chain.last().expect("대상이 있다");
        check_posix_shell(last, self.opts.timeouts.probe).await
    }

    /// 호스트 하나를 연다(TCP 또는 앞 홉의 direct-tcpip 위에서 키 교환·호스트 키 검증·인증). 전체가 `ready` 시간 안이어야 한다
    async fn open_hop(&self, node: &ResolvedHost, via: Option<(&ClientHandle, &ResolvedHost)>, prefix: &str) -> Result<ClientHandle, ExecError> {
        let t = self.opts.timeouts;
        let where_ = format!("{}:{}", node.host_name, node.port);
        let work = self.open_hop_inner(node, via, prefix, &where_);
        match tokio::time::timeout(t.ready, work).await {
            Ok(r) => r,
            Err(_) => Err(other(format!("{prefix}연결 시간이 초과되었습니다 ({where_}, {}초).", t.ready.as_secs()))),
        }
    }

    async fn open_hop_inner(&self, node: &ResolvedHost, via: Option<(&ClientHandle, &ResolvedHost)>, prefix: &str, where_: &str) -> Result<ClientHandle, ExecError> {
        let Ok(port) = u16::try_from(node.port) else {
            return Err(other(format!("{prefix}포트가 1~65535 범위 밖입니다 ({where_})")));
        };
        let mut plan = auth::build_plan(node, &self.opts.home_dir, &self.opts.agent, &*self.opts.env);
        if plan.is_empty() {
            let tail = format!("ssh-agent 를 실행해 키를 등록하거나 암호 없는 개인키를 IdentityFile 로 지정하세요. {}", plan.notes.join(" "));
            return Err(other(format!("{prefix}사용할 수 있는 인증 수단이 없습니다 ({}@{}). {}", node.user, node.host_name, tail.trim())));
        }
        // UserKnownHostsFile·HostKeyAlias 를 따른다(지정이 없으면 기본 known_hosts 와 HostName).
        // OpenSSH 는 HostKeyAlias 를 쓸 때 [이름]:포트 가 아니라 별칭 그대로 조회한다(포트를 붙이지 않는다)
        let files: Vec<&str> =
            if node.known_hosts_files.is_empty() { vec![self.opts.known_hosts_path.as_str()] } else { node.known_hosts_files.iter().map(String::as_str).collect() };
        let known_hosts = Arc::new(read_known_hosts(&files));
        let name = node.host_key_alias.clone().unwrap_or_else(|| node.host_name.clone());
        let kh_port = if node.host_key_alias.is_some() { 22 } else { node.port };
        let algorithms = host_key_algorithms(&known_host_key_algorithms(&known_hosts, &name, kh_port));

        let verdict = Arc::new(Mutex::new(None));
        let handler = HostKeyHandler { known_hosts, name, port: kh_port, verdict: verdict.clone() };
        let preferred = build_preferred(algorithms);
        let config = Arc::new(client::Config {
            keepalive_interval: Some(self.opts.timeouts.keepalive),
            keepalive_max: 3,
            inactivity_timeout: None,
            nodelay: true,
            preferred,
            ..client::Config::default()
        });
        let fail = |e: russh::Error, plan_notes: &[String]| other(format!("{prefix}{}", describe_error(&e, node, where_, &verdict, plan_notes, self.opts.timeouts.ready)));

        // password·keyboard-interactive 는 의도적으로 쓰지 않는다(키/agent 인증만)
        let mut handle = match via {
            None => {
                let tcp = tcp_connect(&node.host_name, port, prefix, where_).await?;
                // russh 의 연결 함수는 이 파일에서만 부른다(clippy.toml)
                #[allow(clippy::disallowed_methods)]
                let r = client::connect_stream(config, tcp, handler).await;
                r.map_err(|e| fail(e, &plan.notes))?
            }
            Some((via_handle, from)) => {
                #[allow(clippy::disallowed_methods)]
                let tunnel = via_handle.channel_open_direct_tcpip(node.host_name.clone(), node.port, "127.0.0.1", 0).await;
                let channel = tunnel.map_err(|e| {
                    other(format!("점프 호스트 {} 에서 {}:{} 로 연결하지 못했습니다: {e}", from.alias, node.host_name, node.port))
                })?;
                #[allow(clippy::disallowed_methods)]
                let r = client::connect_stream(config, channel.into_stream(), handler).await;
                r.map_err(|e| fail(e, &plan.notes))?
            }
        };
        match auth::authenticate(&mut handle, &node.user, &mut plan).await {
            Ok(true) => Ok(handle),
            Ok(false) => {
                let notes = plan.notes.join(" ");
                Err(other(format!(
                    "{prefix}인증에 실패했습니다 ({}@{where_}). 비밀번호 인증은 지원하지 않습니다 — 서버에 공개키를 등록하고 ssh-agent 에 키를 올리거나 IdentityFile 을 지정하세요. {notes}",
                    node.user
                ).trim_end().to_string()))
            }
            Err(e) => Err(fail(e, &plan.notes)),
        }
    }
}

/// 협상 알고리즘. 기본값에 호스트 키 종류 제한만 얹는다. kex 에 `none` 이 없음은 시험이 고정한다
fn build_preferred(host_key_algorithms: Vec<Algorithm>) -> Preferred {
    let mut preferred = Preferred::default();
    if !host_key_algorithms.is_empty() {
        preferred.key = Cow::Owned(host_key_algorithms);
    }
    preferred
}

/// known_hosts 에 등록된 키 종류 이름 → russh 알고리즘. 알 수 없는 이름(`sk-…` 등)은 건너뛴다
fn host_key_algorithms(names: &[String]) -> Vec<Algorithm> {
    names
        .iter()
        .filter_map(|n| match n.as_str() {
            "ssh-ed25519" => Some(Algorithm::Ed25519),
            "ecdsa-sha2-nistp256" => Some(Algorithm::Ecdsa { curve: EcdsaCurve::NistP256 }),
            "ecdsa-sha2-nistp384" => Some(Algorithm::Ecdsa { curve: EcdsaCurve::NistP384 }),
            "ecdsa-sha2-nistp521" => Some(Algorithm::Ecdsa { curve: EcdsaCurve::NistP521 }),
            "rsa-sha2-512" => Some(Algorithm::Rsa { hash: Some(HashAlg::Sha512) }),
            "rsa-sha2-256" => Some(Algorithm::Rsa { hash: Some(HashAlg::Sha256) }),
            "ssh-rsa" => Some(Algorithm::Rsa { hash: None }),
            _ => None,
        })
        .collect()
}

async fn tcp_connect(host: &str, port: u16, prefix: &str, where_: &str) -> Result<TcpStream, ExecError> {
    let not_found = || other(format!("{prefix}호스트 이름을 찾을 수 없습니다 ({host})."));
    let addrs: Vec<std::net::SocketAddr> = tokio::net::lookup_host((host, port)).await.map_err(|_| not_found())?.collect();
    if addrs.is_empty() {
        return Err(not_found());
    }
    TcpStream::connect(&addrs[..]).await.map_err(|e| match e.kind() {
        std::io::ErrorKind::ConnectionRefused => other(format!("{prefix}연결이 거부되었습니다 ({where_}). sshd 가 떠 있는지, 포트가 맞는지 확인하세요.")),
        std::io::ErrorKind::TimedOut => other(format!("{prefix}연결 시간이 초과되었습니다 ({where_}).")),
        _ => other(format!("{prefix}SSH 연결에 실패했습니다 ({where_}): {e}")),
    })
}

/// 연결 오류 문구(앞의 `prefix` 는 호출자가 붙인다). 호스트 키 거부 사유가 있으면 그것이 우선이다
fn describe_error(e: &russh::Error, node: &ResolvedHost, where_: &str, verdict: &Mutex<Option<String>>, notes: &[String], ready: Duration) -> String {
    if let Some(v) = verdict.lock().unwrap_or_else(PoisonError::into_inner).clone() {
        return v;
    }
    match e {
        russh::Error::ConnectionTimeout => format!("연결 시간이 초과되었습니다 ({where_}, {}초).", ready.as_secs()),
        russh::Error::IO(io) if io.kind() == std::io::ErrorKind::ConnectionRefused => {
            format!("연결이 거부되었습니다 ({where_}). sshd 가 떠 있는지, 포트가 맞는지 확인하세요.")
        }
        russh::Error::Disconnect | russh::Error::HUP => format!("연결이 닫혔습니다 ({}:{})", node.host_name, node.port),
        other_err => format!("SSH 연결에 실패했습니다 ({where_}): {other_err} {}", notes.join(" ")).trim_end().to_string(),
    }
}

/// 체인을 닫는다(대상 → 점프 순). 동기 문맥(Drop)에서도 부를 수 있게 런타임에 맡긴다
fn disconnect_in_background(live: Arc<Live>) {
    let Ok(rt) = tokio::runtime::Handle::try_current() else { return };
    rt.spawn(async move {
        for h in live.chain.iter().rev() {
            let _ = tokio::time::timeout(KILL_TIMEOUT, h.disconnect(russh::Disconnect::ByApplication, "", "en")).await;
        }
    });
}

/// 원격 로그인 셸이 POSIX 계열인지 확인한다(`SHELL_PROBE` 참고). 아니면 사용자 값이 든 명령을 보내지 않는다
async fn check_posix_shell(handle: &ClientHandle, limit: Duration) -> Result<(), ExecError> {
    let probe = async {
        let mut ch = tokio::time::timeout(OPEN_TIMEOUT, handle.channel_open_session())
            .await
            .map_err(|_| other("원격 셸을 확인하지 못했습니다: 채널을 열지 못했습니다"))?
            .map_err(|e| other(format!("원격 셸을 확인하지 못했습니다: {e}")))?;
        // 원격 명령 실행은 이 파일(프로브·Job::run)에서만 요청한다(clippy.toml)
        #[allow(clippy::disallowed_methods)]
        let sent = ch.exec(true, SHELL_PROBE).await;
        sent.map_err(|e| other(format!("원격 셸을 확인하지 못했습니다: {e}")))?;
        let mut out: Vec<u8> = Vec::new();
        let (mut eof, mut exited) = (false, false);
        while let Some(msg) = ch.wait().await {
            match msg {
                ChannelMsg::Data { data } => {
                    if out.len() < 256 {
                        out.extend_from_slice(&data);
                    }
                }
                ChannelMsg::Failure => return Err(other("원격 셸을 확인하지 못했습니다: 서버가 exec 요청을 거부했습니다")),
                ChannelMsg::ExitStatus { .. } | ChannelMsg::ExitSignal { .. } => exited = true,
                ChannelMsg::Eof => eof = true,
                ChannelMsg::Close => break,
                _ => {}
            }
            if eof && exited {
                break;
            }
        }
        let _ = ch.close().await;
        Ok(out)
    };
    let out = match tokio::time::timeout(limit, probe).await {
        Ok(r) => r?,
        Err(_) => return Err(other("원격 셸 확인이 시간 안에 끝나지 않았습니다")),
    };
    if js_trim(&String::from_utf8_lossy(&out)) == SHELL_PROBE_EXPECT {
        Ok(())
    } else {
        Err(other("원격 로그인 셸이 POSIX 셸이 아닙니다(sh·bash·zsh·fish 만 지원합니다). Windows OpenSSH 서버라면 기본 셸을 바꿔야 합니다."))
    }
}

async fn cancelled(cancel: &Option<CancellationToken>) {
    match cancel {
        Some(t) => t.cancelled().await,
        None => std::future::pending().await,
    }
}

/// 원격 프로세스에 KILL 을 요청하고 채널을 닫는다(이미 닫힌 채널이면 실패해도 무해하다)
async fn kill_channel(ch: &Channel<Msg>) {
    let _ = tokio::time::timeout(KILL_TIMEOUT, async {
        let _ = ch.signal(Sig::KILL).await;
        let _ = ch.close().await;
    })
    .await;
}

/// KILL 뒤 서버가 채널을 닫을 때까지 남은 메시지를 버리며 기다린다(슬롯을 닫힘 뒤에 돌려주려고)
async fn drain_until_closed(ch: &mut Channel<Msg>) {
    let _ = tokio::time::timeout(CLOSE_WAIT, async { while ch.wait().await.is_some() {} }).await;
}

/// 채널을 열고 명령을 exec 한다. 채널 열기와 exec 요청을 두 단계로 나눈다 — exec 단계가 실패·시간 초과하면 이미 열린 채널을
/// KILL → close → 닫힘 확인으로 정리한 **뒤에** 오류를 돌려준다(슬롯은 호출자가 그 뒤에 돌려준다).
/// `stop`·`user` 가 취소되면 시작을 멈추고 `Aborted` 를 돌려준다 — 서버가 열기에 느리게 답해도 슬롯을 최대 `OPEN_TIMEOUT` 동안 쥐지 않는다.
/// 채널 열기 대기를 버릴 때는 future 를 그냥 버리지 않고, 뒤늦게 열린 채널을 닫는 정리 태스크로 넘긴다(서버에 채널이 남지 않게)
async fn open_exec(live: &Arc<Live>, command: &str, stop: &CancellationToken, user: &Option<CancellationToken>) -> Result<Channel<Msg>, ExecError> {
    let mut ch = open_session(live, stop, user, "원격 명령을 시작하지 못했습니다").await?;
    // 원격 명령 실행은 이 파일(프로브·open_exec)에서만 요청한다(clippy.toml)
    #[allow(clippy::disallowed_methods)]
    let sent = tokio::select! {
        biased;
        () = stop.cancelled() => None,
        () = cancelled(user) => None,
        r = tokio::time::timeout(OPEN_TIMEOUT, ch.exec(true, command)) => Some(r),
    };
    let start_error = match sent {
        None => {
            kill_channel(&ch).await;
            drain_until_closed(&mut ch).await;
            return Err(ExecError::Aborted);
        }
        Some(Ok(Ok(()))) => return Ok(ch),
        Some(Ok(Err(e))) => format!("{e}"),
        Some(Err(_)) => "시간 초과".to_string(),
    };
    kill_channel(&ch).await;
    drain_until_closed(&mut ch).await;
    Err(other(format!("원격 명령을 시작하지 못했습니다: {start_error}")))
}

/// 세션 채널을 연다(`open_exec` 와 SFTP 가 같이 쓴다). 열기는 `OPEN_TIMEOUT` 안에 끝나야 하고, `stop`·`user` 가 취소되면 `Aborted` 다.
/// 열기 대기를 버릴 때는 future 를 그냥 버리지 않고 뒤늦게 열린 채널을 닫는 정리 태스크로 넘긴다. `fail` 은 열기 실패 문구의 앞부분이다
async fn open_session(live: &Arc<Live>, stop: &CancellationToken, user: &Option<CancellationToken>, fail: &str) -> Result<Channel<Msg>, ExecError> {
    let mut opening = {
        let live = live.clone();
        tokio::spawn(async move { tokio::time::timeout(OPEN_TIMEOUT, live.last().channel_open_session()).await })
    };
    let joined = tokio::select! {
        biased;
        () = stop.cancelled() => None,
        () = cancelled(user) => None,
        r = &mut opening => Some(r),
    };
    let Some(joined) = joined else {
        // 열기는 `OPEN_TIMEOUT` 안에 끝나므로 정리 태스크도 그 안에 끝난다
        tokio::spawn(async move {
            if let Ok(Ok(Ok(mut ch))) = opening.await {
                kill_channel(&ch).await;
                drain_until_closed(&mut ch).await;
            }
        });
        return Err(ExecError::Aborted);
    };
    match joined {
        Ok(Ok(Ok(ch))) => Ok(ch),
        Ok(Ok(Err(e))) => Err(other(format!("{fail}: {e}"))),
        Ok(Err(_)) => Err(other(format!("{fail}: 시간 초과"))),
        Err(_) => Err(other("SSH 실행 작업이 중단되었습니다")),
    }
}

/// SFTP 하위 시스템을 시작한 채널. **SFTP 하위 시스템 요청은 이 함수 한 곳에서만 보낸다**(clippy.toml). 서버가 거부하거나 응답이 없으면
/// 채널을 정리한 뒤 `Unavailable`(이 서버는 SFTP 를 막았다)로 돌려준다 — 호출자가 경로 직접 입력 안내로 이어 간다
async fn open_sftp(live: &Arc<Live>, stop: &CancellationToken, user: &Option<CancellationToken>) -> Result<Channel<Msg>, SftpError> {
    let mut ch = open_session(live, stop, user, "SFTP 채널을 열지 못했습니다").await.map_err(|e| match e {
        ExecError::Aborted => SftpError::Aborted,
        e => SftpError::Failed(e.to_string()),
    })?;
    #[allow(clippy::disallowed_methods)]
    let sent = tokio::select! {
        biased;
        () = stop.cancelled() => None,
        () = cancelled(user) => None,
        r = tokio::time::timeout(OPEN_TIMEOUT, async {
            ch.request_subsystem(true, "sftp").await?;
            // 요청 응답(성공·실패)을 기다린다. 그 사이 다른 메시지(창 조정 등)는 건너뛴다
            loop {
                match ch.wait().await {
                    Some(ChannelMsg::Success) => return Ok::<bool, russh::Error>(true),
                    Some(ChannelMsg::Failure | ChannelMsg::Close) | None => return Ok(false),
                    Some(_) => {}
                }
            }
        }) => Some(r),
    };
    let why = match sent {
        Some(Ok(Ok(true))) => return Ok(ch),
        None => None,
        Some(Ok(Ok(false))) => Some("서버가 SFTP 요청을 거부했습니다".to_string()),
        Some(Ok(Err(e))) => Some(e.to_string()),
        Some(Err(_)) => Some("시간 초과".to_string()),
    };
    let (mut read, write) = ch.split();
    close_split(&mut read, &write).await;
    Err(why.map_or(SftpError::Aborted, SftpError::Unavailable))
}

/// 채널과 SFTP 클라이언트 쪽 스트림(`far`) 사이로 바이트를 옮긴다. **채널은 항상 비워진다** — 클라이언트가 읽지 않거나 이미 끝났어도 서버가 보낸 데이터를
/// 계속 받아야 같은 연결의 다른 채널(`run`·`stream`)이 멈추지 않는다(R1: russh 는 채널마다 유한한 큐로 넘겨 읽지 않는 채널이 세션 루프를 멈춘다).
/// 그래서 읽기(채널 → 스트림)와 쓰기(스트림 → 채널)를 따로 돌리고, 쓰기가 원격 윈도우에 막혀도 읽기는 멈추지 않는다.
/// 클라이언트가 먼저 끝나(오류·EOF) 스트림이 닫혀도 읽기는 `stop` 이나 채널 닫힘까지 계속 비운다(버린다). 끝나는 조건은 채널 닫힘·`stop` 취소뿐이다.
/// 채널을 닫고 닫힘을 확인하는 것은 호출자 몫이다
async fn sftp_bridge(read: &mut ChannelReadHalf, write: &ChannelWriteHalf<Msg>, far: DuplexStream, stop: &CancellationToken, gone: &CancellationToken) {
    let (mut from_stream, mut to_stream) = tokio::io::split(far);
    let inbound = async {
        let mut sink_open = true;
        loop {
            let msg = tokio::select! {
                biased;
                () = stop.cancelled() => return,
                m = read.wait() => m,
            };
            match msg {
                Some(ChannelMsg::Data { data }) if sink_open => {
                    let sent = tokio::select! {
                        biased;
                        () = stop.cancelled() => return,
                        r = to_stream.write_all(&data) => r,
                    };
                    // 클라이언트가 이미 끝났다 — 이후 데이터는 버리되 채널은 계속 비운다
                    sink_open = sent.is_ok();
                }
                Some(ChannelMsg::Eof | ChannelMsg::Close) | None => {
                    let _ = to_stream.shutdown().await;
                    return;
                }
                Some(_) => {}
            }
        }
    };
    let outbound = async {
        let mut buf = vec![0u8; SFTP_CHUNK];
        loop {
            let n = tokio::select! {
                biased;
                () = stop.cancelled() => return,
                n = from_stream.read(&mut buf) => n,
            };
            // 클라이언트 쪽이 끝났다(세션이 오류로 끝나 스트림이 버려졌다) — 기다리는 요청이 제 시간 제한까지 매달리지 않게 알린다
            let Ok(n @ 1..) = n else {
                gone.cancel();
                return;
            };
            let sent = tokio::select! {
                biased;
                () = stop.cancelled() => return,
                r = write.data(&buf[..n]) => r,
            };
            if sent.is_err() {
                return;
            }
        }
    };
    // 스트림 쪽이 먼저 끝나도(outbound 종료) 채널 읽기(inbound)는 계속 돈다 — 위 문서
    tokio::join!(inbound, outbound);
}

/// SFTP 작업 하나(채널 열기 → 폴더 목록 → 정리). **슬롯은 이 함수가 끝난 뒤(채널 닫힘 확인 뒤)** 호출자가 돌려준다
async fn sftp_job(live: Arc<Live>, path: String, limit: Duration, token: CancellationToken, user: Option<CancellationToken>) -> Result<RemoteDirListing, SftpError> {
    let ch = open_sftp(&live, &token, &user).await?;
    let (mut read, write) = ch.split();
    let (near, far) = tokio::io::duplex(SFTP_PIPE);
    let stop = token.child_token();
    let gone = CancellationToken::new();
    let bridge = sftp_bridge(&mut read, &write, far, &stop, &gone);
    let work = async {
        let r = tokio::select! {
            biased;
            () = token.cancelled() => Err(SftpError::Aborted),
            () = cancelled(&user) => Err(SftpError::Aborted),
            r = tokio::time::timeout(limit, sftp::list_dirs(near, &path)) => r.unwrap_or_else(|_| Err(SftpError::Failed(format!("원격 폴더 목록이 {}초 안에 끝나지 않았습니다", limit.as_secs())))),
            // 결과보다 뒤에 둔다 — 정상 종료로 세션이 닫히는 순간의 EOF 는 결과가 이긴다
            () = gone.cancelled() => Err(SftpError::Failed("서버가 SFTP 응답을 끝냈거나 올바르지 않은 응답을 보내 목록을 읽지 못했습니다".to_string())),
        };
        // 결과가 났다 — 다리를 멈춘다(`near` 는 위 future 와 함께 버려졌다)
        stop.cancel();
        r
    };
    let ((), result) = tokio::join!(bridge, work);
    close_split(&mut read, &write).await;
    result
}

/// 나눈 채널을 닫는다(SFTP 용). close 를 보내는 동안 채널을 계속 비운다 — 서버가 계속 보내는 중이면 채널 큐가 차서 세션 루프가 멈추고, 그러면 close 요청도 처리되지 못한다(R1).
/// **닫힘 확인을 기다리지 않는다**: russh 는 우리가 close 를 보낸 뒤에 오는 서버의 close 응답을 무시해(채널 장부에서 이미 지웠다) 읽기 쪽이 끝나지 않으므로 기다려도 `CLOSE_WAIT` 만 채운다.
/// close 요청은 이 연결의 이후 요청보다 먼저 서버로 나가지만 **서버의 채널 해제는 비동기**다 — 이후 채널 열기가 서버의 해제보다 먼저 닿을 수 있어, 서버의 채널 수 상한(`MaxSessions`)을
/// 넘지 않으려면 여유 슬롯(풀의 슬롯 수가 그 상한보다 작다)에 의존한다. close 를 보낸 뒤 서버가 보내는 데이터는 russh 가 큐에 넣지 않고 버린다
async fn close_split(read: &mut ChannelReadHalf, write: &ChannelWriteHalf<Msg>) {
    let close = tokio::time::timeout(KILL_TIMEOUT, write.close());
    tokio::pin!(close);
    loop {
        tokio::select! {
            _ = &mut close => return,
            m = read.wait() => {
                if m.is_none() {
                    return;
                }
            }
        }
    }
}

enum Outcome {
    /// 원격이 끝났다. 종료 코드를 받지 못했으면 `None`(연결이 끊긴 것)
    Done(Option<i32>),
    /// stdout 이 상한을 넘었다
    TooBig,
    /// 서버가 exec 요청을 거부했다
    Rejected,
    Aborted,
}

struct Job {
    live: Arc<Live>,
    command: String,
    max_stdout: usize,
    git_args: Vec<String>,
}

impl Job {
    /// 채널을 열어 명령을 실행하고 끝까지 읽는다. **슬롯은 이 함수가 끝난 뒤(닫힘 확인 뒤)** 호출자가 돌려준다
    async fn run(self, token: CancellationToken) -> Result<RunResult, ExecError> {
        let mut ch = open_exec(&self.live, &self.command, &token, &None).await?;
        let mut stdout: Vec<u8> = Vec::new();
        let mut stderr: Vec<u8> = Vec::new();
        let outcome = self.read_all(&mut ch, &token, &mut stdout, &mut stderr).await;
        match outcome {
            Outcome::Done(code) => {
                let _ = tokio::time::timeout(KILL_TIMEOUT, ch.close()).await;
                match code {
                    None => Err(other(CONNECTION_LOST)),
                    Some(code) => Ok(RunResult { stdout, stderr: String::from_utf8_lossy(&stderr).into_owned(), code }),
                }
            }
            Outcome::TooBig => {
                kill_channel(&ch).await;
                drain_until_closed(&mut ch).await;
                Err(ExecError::Git { message: format!("output exceeded {} bytes", self.max_stdout), code: -1, stderr: String::new(), args: self.git_args.clone() })
            }
            Outcome::Rejected => {
                let _ = tokio::time::timeout(KILL_TIMEOUT, ch.close()).await;
                drain_until_closed(&mut ch).await;
                Err(other("원격 명령을 시작하지 못했습니다: 서버가 exec 요청을 거부했습니다"))
            }
            Outcome::Aborted => {
                kill_channel(&ch).await;
                drain_until_closed(&mut ch).await;
                Err(ExecError::Aborted)
            }
        }
    }

    /// 채널이 닫히거나(Close) EOF·종료 코드를 모두 받을 때까지 읽는다. **EOF·종료 코드만 보고 끊지 않는다** — 데이터는 항상 그 앞에 온다
    async fn read_all(&self, ch: &mut Channel<Msg>, token: &CancellationToken, stdout: &mut Vec<u8>, stderr: &mut Vec<u8>) -> Outcome {
        let mut code: Option<i32> = None;
        let mut eof = false;
        loop {
            let msg = tokio::select! {
                biased;
                _ = token.cancelled() => return Outcome::Aborted,
                m = ch.wait() => m,
            };
            let Some(msg) = msg else { return Outcome::Done(code) };
            match msg {
                ChannelMsg::Data { data } => {
                    if stdout.len() + data.len() > self.max_stdout {
                        return Outcome::TooBig;
                    }
                    stdout.extend_from_slice(&data);
                }
                ChannelMsg::ExtendedData { data, ext: 1 } => {
                    let room = MAX_STDERR.saturating_sub(stderr.len());
                    stderr.extend_from_slice(&data[..data.len().min(room)]);
                }
                ChannelMsg::ExitStatus { exit_status } => code = Some(i32::try_from(exit_status).unwrap_or(-1)),
                // 시그널로 죽었으면 -1(TS 와 같다)
                ChannelMsg::ExitSignal { .. } => code = Some(-1),
                ChannelMsg::Eof => eof = true,
                ChannelMsg::Failure => return Outcome::Rejected,
                ChannelMsg::Close => return Outcome::Done(code),
                _ => {}
            }
            if eof && code.is_some() {
                return Outcome::Done(code);
            }
        }
    }
}

/// `stream` 펌프가 소유하는 것들
struct Pump {
    live: Arc<Live>,
    command: String,
    /// 채널이 닫힌 것을 확인한 뒤에 돌려준다
    slot: Slot,
    tx: mpsc::UnboundedSender<Event>,
    /// 큐에 쌓였지만 소비자가 아직 꺼내지 않은 바이트
    queued: Arc<AtomicUsize>,
    cap: usize,
    /// 스트림이 버려졌다(Drop)
    stop: CancellationToken,
    /// 호출자가 준 취소 토큰
    user: Option<CancellationToken>,
    started: oneshot::Sender<Result<(), ExecError>>,
}

/// 펌프가 읽기를 끝낸 이유
enum PumpExit {
    /// 원격이 끝났다(종료 알림을 보냈다)
    Ended,
    /// 서버가 exec 요청을 거부했다(알림을 보냈다)
    Rejected,
    /// 중지(소비자 이탈·취소)나 선읽기 상한 초과 — 원격을 죽여야 한다
    Kill,
}

/// 데이터 프레임 하나의 처리
#[derive(Debug, PartialEq, Eq)]
enum Admit {
    /// 빈 프레임 — 버린다(큐에 노드만 쌓는 서버를 막는다)
    Skip,
    /// 상한을 넘는다
    Overflow,
    /// 큐에 넣는다(과금했다)
    Queue,
}

/// 프레임을 큐에 넣을지 정하고 넣으면 과금한다. 비용은 페이로드 + 메시지당 고정 [`EVENT_COST`] 이며 소비자가 같은 값을 뺀다
fn admit(queued: &AtomicUsize, cap: usize, len: usize) -> Admit {
    if len == 0 {
        return Admit::Skip;
    }
    let cost = len + EVENT_COST;
    if queued.load(Ordering::SeqCst) + cost > cap {
        return Admit::Overflow;
    }
    queued.fetch_add(cost, Ordering::SeqCst);
    Admit::Queue
}

/// 채널을 소유해 **소비자와 무관하게 즉시 비우는** 태스크. 채널을 읽는 쪽이 느리면 russh 의 세션 루프가 멈추므로(docs/tauri.md R1)
/// 소비자는 이 태스크가 채운 큐에서 가져간다. 큐가 상한을 넘으면 원격을 죽이고 소비자에게 `Overflow` 를 알린다
async fn pump(p: Pump) {
    let Pump { live, command, slot, tx, queued, cap, stop, user, started } = p;
    let mut ch = match open_exec(&live, &command, &stop, &user).await {
        Ok(ch) => ch,
        Err(e) => {
            let _ = started.send(Err(e));
            drop(slot);
            return;
        }
    };
    let _ = started.send(Ok(()));
    let mut stderr: Vec<u8> = Vec::new();
    let mut code: Option<i32> = None;
    let mut eof = false;
    let exit = loop {
        let msg = tokio::select! {
            biased;
            () = stop.cancelled() => break PumpExit::Kill,
            () = cancelled(&user) => break PumpExit::Kill,
            m = ch.wait() => m,
        };
        let Some(msg) = msg else {
            let _ = tx.send(Event::End(End { code, stderr, rejected: false }));
            break PumpExit::Ended;
        };
        match msg {
            ChannelMsg::Data { data } => match admit(&queued, cap, data.len()) {
                Admit::Skip => {}
                Admit::Overflow => {
                    let _ = tx.send(Event::Overflow);
                    break PumpExit::Kill;
                }
                Admit::Queue => {
                    if tx.send(Event::Data(data.to_vec())).is_err() {
                        break PumpExit::Kill;
                    }
                }
            },
            ChannelMsg::ExtendedData { data, ext: 1 } => {
                let room = MAX_STDERR.saturating_sub(stderr.len());
                stderr.extend_from_slice(&data[..data.len().min(room)]);
            }
            ChannelMsg::ExitStatus { exit_status } => code = Some(i32::try_from(exit_status).unwrap_or(-1)),
            // 시그널로 죽었으면 -1(TS 와 같다)
            ChannelMsg::ExitSignal { .. } => code = Some(-1),
            ChannelMsg::Eof => eof = true,
            ChannelMsg::Failure => {
                let _ = tx.send(Event::End(End { code: None, stderr: Vec::new(), rejected: true }));
                break PumpExit::Rejected;
            }
            ChannelMsg::Close => {
                let _ = tx.send(Event::End(End { code, stderr, rejected: false }));
                break PumpExit::Ended;
            }
            _ => {}
        }
        if eof && code.is_some() {
            let _ = tx.send(Event::End(End { code, stderr, rejected: false }));
            break PumpExit::Ended;
        }
    };
    match exit {
        PumpExit::Ended => {
            let _ = tokio::time::timeout(KILL_TIMEOUT, ch.close()).await;
        }
        PumpExit::Rejected => {
            let _ = tokio::time::timeout(KILL_TIMEOUT, ch.close()).await;
            drain_until_closed(&mut ch).await;
        }
        PumpExit::Kill => {
            kill_channel(&ch).await;
            drain_until_closed(&mut ch).await;
        }
    }
    // 슬롯은 채널이 닫힌 뒤에 돌려준다(서버 `MaxSessions` 를 넘지 않게)
    drop(slot);
}

impl Connection {
    pub(super) fn new(host_spec: &str, opts: SshOptions) -> Self {
        let max = opts.max_channels.max(1);
        let opts = SshOptions { max_channels: max, ..opts };
        Connection {
            inner: Arc::new(Inner {
                host_spec: host_spec.to_string(),
                permits: Arc::new(Semaphore::new(max)),
                opts,
                live: Mutex::new(None),
                connect_lock: tokio::sync::Mutex::new(()),
                last_failure: Mutex::new(None),
                disposed: AtomicBool::new(false),
                idle_gen: AtomicU64::new(0),
                idle_timer: Mutex::new(None),
                stream_ahead: AtomicUsize::new(STREAM_AHEAD_LIMIT),
            }),
        }
    }

    /// 지금 살아 있는 연결이 있는가(관측용)
    pub(super) fn is_connected(&self) -> bool {
        self.inner.current().is_some()
    }

    /// 정리(`dispose`)된 연결인가 — 닫은 연결은 영구히 거부한다
    pub(super) fn is_disposed(&self) -> bool {
        self.inner.disposed.load(Ordering::SeqCst)
    }

    /// 비어 있는 채널 슬롯 수(관측용 — 슬롯 누수 시험)
    pub(super) fn free_slots(&self) -> usize {
        self.inner.permits.available_permits()
    }

    /// 원격 명령 하나를 실행해 출력을 모은다. 연결은 지연해서 맺고, 동시 채널은 세마포어로 제한한다.
    /// 호출자가 취소(`opts.cancel`)하거나 future 를 버리면 원격 프로세스에 KILL 을 보내고 슬롯은 채널이 닫힌 뒤에 돌려준다.
    pub(super) async fn run(&self, command: String, git_args: &[String], opts: &RunOpts) -> Result<RunResult, ExecError> {
        if opts.cancel.as_ref().is_some_and(CancellationToken::is_cancelled) {
            return Err(ExecError::Aborted);
        }
        let inner = &self.inner;
        let Some(slot) = (tokio::select! {
            biased;
            _ = cancelled(&opts.cancel) => None,
            s = inner.acquire() => Some(s),
        }) else {
            return Err(ExecError::Aborted);
        };
        let slot = slot?;
        // 슬롯을 얻은 뒤 연결 전 사이에 이미 취소됐으면 연결(최대 15초)을 기다리지 않는다
        let live = tokio::select! {
            biased;
            _ = cancelled(&opts.cancel) => return Err(ExecError::Aborted),
            l = inner.live() => l?,
        };
        let job = Job { live, command, max_stdout: opts.max_buffer.unwrap_or(DEFAULT_MAX_BUFFER), git_args: to_strings(git_args) };
        let token = CancellationToken::new();
        // 이 future 가 어떤 경로로든 끝나거나 버려지면 태스크에 취소를 알린다(이미 끝났다면 무해하다)
        let _guard = token.clone().drop_guard();
        let (tx, rx) = oneshot::channel();
        tokio::spawn(async move {
            let r = job.run(token).await;
            let _ = tx.send(r);
            drop(slot);
        });
        tokio::select! {
            biased;
            _ = cancelled(&opts.cancel) => Err(ExecError::Aborted),
            r = rx => r.unwrap_or_else(|_| Err(other("SSH 실행 작업이 중단되었습니다"))),
        }
    }

    /// 원격 명령 하나를 실행해 stdout 을 스트림으로 준다(`run` 과 같은 슬롯·연결 규칙). 채널·슬롯은 펌프 태스크가 쥐고 소비자와 무관하게 비운다 —
    /// 스트림을 버리거나 취소하면 원격에 KILL 을 보내고 슬롯은 채널이 닫힌 뒤에 돌려준다. 시작(exec 요청)까지만 기다리고 돌아온다
    pub(super) async fn stream(&self, command: String, git_args: &[String], opts: &RunOpts) -> Result<SshStream, ExecError> {
        if opts.cancel.as_ref().is_some_and(CancellationToken::is_cancelled) {
            return Err(ExecError::Aborted);
        }
        let inner = &self.inner;
        let Some(slot) = (tokio::select! {
            biased;
            _ = cancelled(&opts.cancel) => None,
            s = inner.acquire() => Some(s),
        }) else {
            return Err(ExecError::Aborted);
        };
        let slot = slot?;
        let live = tokio::select! {
            biased;
            _ = cancelled(&opts.cancel) => return Err(ExecError::Aborted),
            l = inner.live() => l?,
        };
        let stop = CancellationToken::new();
        // 시작 전에 돌아가는 모든 경로에서 버려져 펌프를 멈춘다(성공하면 스트림이 넘겨받는다)
        let guard = stop.clone().drop_guard();
        let (tx, rx) = mpsc::unbounded_channel();
        let (started_tx, started_rx) = oneshot::channel();
        let queued = Arc::new(AtomicUsize::new(0));
        tokio::spawn(pump(Pump {
            live,
            command,
            slot,
            tx,
            queued: queued.clone(),
            cap: inner.stream_ahead.load(Ordering::SeqCst),
            stop,
            user: opts.cancel.clone(),
            started: started_tx,
        }));
        let started = tokio::select! {
            biased;
            _ = cancelled(&opts.cancel) => return Err(ExecError::Aborted),
            r = started_rx => r,
        };
        match started {
            Ok(Ok(())) => Ok(SshStream::new(rx, queued, opts.cancel.clone(), to_strings(git_args), guard)),
            Ok(Err(e)) => Err(e),
            Err(_) => Err(other("SSH 실행 작업이 중단되었습니다")),
        }
    }

    /// 원격 폴더 목록(SFTP). `run` 과 같은 연결·채널 슬롯 하나를 쓴다(그 연결에서 셸 확인을 이미 마쳤다). 경로는 연결 전에 검사한다.
    /// 호출자가 취소하거나 future 를 버리면 채널을 닫고(닫힘 확인 뒤) 슬롯을 돌려준다. 전체 시간 제한은 `Timeouts::sftp`
    pub(super) async fn list_dirs(&self, path: &str, cancel: Option<&CancellationToken>) -> Result<RemoteDirListing, SftpError> {
        sftp::to_sftp_path(path)?;
        let cancel = cancel.cloned();
        if cancel.as_ref().is_some_and(CancellationToken::is_cancelled) {
            return Err(SftpError::Aborted);
        }
        let inner = &self.inner;
        let Some(slot) = (tokio::select! {
            biased;
            _ = cancelled(&cancel) => None,
            s = inner.acquire() => Some(s),
        }) else {
            return Err(SftpError::Aborted);
        };
        let slot = slot.map_err(|e| SftpError::Failed(e.to_string()))?;
        let live = tokio::select! {
            biased;
            _ = cancelled(&cancel) => return Err(SftpError::Aborted),
            l = inner.live() => l.map_err(|e| SftpError::Failed(e.to_string()))?,
        };
        let token = CancellationToken::new();
        // 이 future 가 어떤 경로로든 끝나거나 버려지면 태스크에 취소를 알린다(이미 끝났다면 무해하다)
        let _guard = token.clone().drop_guard();
        let (tx, rx) = oneshot::channel();
        let (path, limit, user) = (path.to_string(), inner.opts.timeouts.sftp, cancel.clone());
        tokio::spawn(async move {
            let r = sftp_job(live, path, limit, token, user).await;
            let _ = tx.send(r);
            drop(slot);
        });
        tokio::select! {
            biased;
            _ = cancelled(&cancel) => Err(SftpError::Aborted),
            r = rx => r.unwrap_or_else(|_| Err(SftpError::Failed("SSH 실행 작업이 중단되었습니다".to_string()))),
        }
    }

    /// 선읽기 상한을 바꾼다(시험 전용 — 16MB 를 실제로 보내지 않으려고)
    #[cfg(feature = "test-hooks")]
    pub(super) fn set_stream_ahead_limit_for_test(&self, bytes: usize) {
        self.inner.stream_ahead.store(bytes, Ordering::SeqCst);
    }

    /// 연결을 닫고 이후 호출을 거부한다. 슬롯을 기다리던 호출도 깨어나 "이미 종료" 로 실패한다
    pub(super) fn dispose(&self) {
        self.inner.disposed.store(true, Ordering::SeqCst);
        self.inner.idle_gen.fetch_add(1, Ordering::SeqCst);
        self.inner.set_idle_timer(None);
        self.inner.permits.close();
        self.inner.teardown();
    }
}

#[cfg(test)]
mod tests {
    use russh::client::Handler;
    use russh::keys::PrivateKey;

    use super::*;

    fn ed25519() -> russh::keys::PublicKey {
        PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).unwrap().public_key().clone()
    }

    fn line(host: &str, key: &russh::keys::PublicKey) -> String {
        format!("{host} {}\n", key.to_openssh().unwrap().split_whitespace().take(2).collect::<Vec<_>>().join(" "))
    }

    fn handler(known_hosts: String, name: &str) -> (HostKeyHandler, Arc<Mutex<Option<String>>>) {
        let verdict = Arc::new(Mutex::new(None));
        (HostKeyHandler { known_hosts: Arc::new(known_hosts), name: name.to_string(), port: 22, verdict: verdict.clone() }, verdict)
    }

    async fn check(h: &mut HostKeyHandler, key: &russh::keys::PublicKey) -> bool {
        h.check_server_key(&PublicKeyOrCertificate::PublicKey { key: key.clone(), hash_alg: None }).await.unwrap()
    }

    #[test]
    fn 협상_목록에_none_이_없다_호스트_키_제한은_기본값_위에_얹는다() {
        // kex·cipher·mac 에 `none` 이 끼면 협상이 무결성·기밀성 없는 연결로 내려갈 수 있다. russh 를 올릴 때 이 단언이 깨지면 목록을 명시로 고정한다
        let p = build_preferred(vec![]);
        assert!(!p.kex.is_empty() && !p.cipher.is_empty() && !p.mac.is_empty());
        assert!(!p.kex.iter().any(|n| n.as_ref() == "none"), "kex: {:?}", p.kex);
        assert!(!p.cipher.iter().any(|n| n.as_ref() == "none"), "cipher: {:?}", p.cipher);
        assert!(!p.mac.iter().any(|n| n.as_ref() == "none"), "mac: {:?}", p.mac);
        // 호스트 키 종류를 주면 그것만 남고, 비어 있으면 기본값을 그대로 쓴다
        assert_eq!(build_preferred(vec![Algorithm::Ed25519]).key.as_ref(), &[Algorithm::Ed25519]);
        assert_eq!(build_preferred(vec![]).key, Preferred::default().key);
    }

    #[tokio::test]
    async fn 호스트_키_콜백은_known_hosts_와_같을_때만_수락한다() {
        let key = ed25519();
        let (mut h, verdict) = handler(line("example.com", &key), "example.com");
        assert!(check(&mut h, &key).await);
        assert!(verdict.lock().unwrap().is_none());
        // 다른 키: 거부하고 사유를 남긴다
        assert!(!check(&mut h, &ed25519()).await);
        assert!(verdict.lock().unwrap().as_deref().is_some_and(|m| m.contains("중간자")));
    }

    #[tokio::test]
    async fn 호스트_키_콜백은_비_ascii_호스트를_판정하지_않고_거부한다() {
        // check_host_key 의 Unsupported 가 콜백의 거부로 이어진다(설정 관문을 거치지 않고 직접 부른다)
        let key = ed25519();
        let (mut h, verdict) = handler(line("example.com", &key), "호스트.example");
        assert!(!check(&mut h, &key).await);
        assert!(verdict.lock().unwrap().as_deref().is_some_and(|m| m.contains("비 ASCII")));
    }

    #[tokio::test]
    async fn 호스트_키_콜백은_폐기된_키와_모르는_호스트를_거부한다() {
        let key = ed25519();
        let (mut h, verdict) = handler(format!("@revoked example.com {}\n", line("", &key).trim_start()), "example.com");
        assert!(!check(&mut h, &key).await);
        assert!(verdict.lock().unwrap().as_deref().is_some_and(|m| m.contains("폐기")));
        let (mut h, verdict) = handler(String::new(), "example.com");
        assert!(!check(&mut h, &key).await);
        assert!(verdict.lock().unwrap().as_deref().is_some_and(|m| m.contains("알 수 없는 호스트")));
    }

    // russh 서버 API 는 빈 CHANNEL_DATA 를 보내지 못한다(하네스로 만들 수 없다) — 악성 서버가 보낼 수 있는 프레임을 직접 먹여 본다
    #[test]
    fn 빈_데이터_프레임은_큐에_쌓이지_않고_상한에도_걸리지_않는다() {
        let queued = AtomicUsize::new(0);
        for _ in 0..100_000 {
            assert_eq!(admit(&queued, 1024, 0), Admit::Skip);
        }
        assert_eq!(queued.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn 작은_프레임은_메시지마다_고정_비용으로_상한에_걸린다() {
        let (queued, cap) = (AtomicUsize::new(0), 64 * EVENT_COST);
        let mut accepted = 0;
        // 페이로드만 세면 1바이트 프레임 64개(64바이트)가 들어가지만, 메시지당 비용을 더하면 훨씬 일찍 걸린다
        while admit(&queued, cap, 1) == Admit::Queue {
            accepted += 1;
            assert!(accepted <= 64, "상한에 걸리지 않는다");
        }
        assert_eq!(accepted, cap / (1 + EVENT_COST));
        assert_eq!(queued.load(Ordering::SeqCst), accepted * (1 + EVENT_COST));
        // 걸린 프레임은 과금하지 않는다
        assert_eq!(admit(&queued, cap, 1), Admit::Overflow);
        assert_eq!(queued.load(Ordering::SeqCst), accepted * (1 + EVENT_COST));
    }
}
