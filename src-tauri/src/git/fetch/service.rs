//! fetch 상태 기계(`src/main/services/FetchService.ts` 의 이식). 앱에서는 라우터가 [`FetchControl`] 로 부른다(local·WSL 저장소, 그리고 Rust 가 풀 때는 ssh·github 저장소).
//! GitHub 원격 전용 저장소(github kind)는 **다른 갈래**다(`service/github.rs` — 허용 목록·확인 창 없이 정확 일치 검사, 큐 슬롯을 얻은 뒤부터 재는 60분/60초 제한, 선택 즉시 fetch, 로그인 변경으로 인증 정지 해제, 삭제 전 기다리는 forget).
//!
//! 이 앱이 저장소에 하는 유일한 쓰기라 흐름을 TS 와 같게 둔다:
//! - 저장소당 진행 중인 요청은 하나다 — 진행 중이면 같은 [`FetchFuture`] 를 돌려준다(두 번째 호출의 `manual` 은 무시된다. TS 와 같다).
//!   본체는 spawn 한 태스크라 기다리는 쪽이 사라져도 끝까지 간다(TS 의 Promise 와 같다). 패닉하면 `fetching=false` 로 되돌리고 fetch 하지 않는다.
//! - 환경(`source_key`)마다 전용 큐 슬롯이 하나다(TS `fetchQueue = new ProcessQueue(1)`).
//! - 60초 제한은 **요청 시점부터** 잰다 — 큐 대기와 확인 창이 떠 있는 시간을 포함한다(TS 와 같다. 확인 창은 취소되지 않고, 닫히고 나서 다음 git 호출이 취소를 본다).
//! - 검사 → (수동이면) 확인 → **실행 직전 재검사**(TOCTOU) → fetch. 재검사에서 위험 항목이 늘었으면 멈춘다.
//! - 실패 분류: 인증·허용 안 된 전송은 `auth_paused`(수동 새로고침 전까지 자동 중단), 네트워크는 지수 백오프, 수동 fetch 로 승인되지 않은 http(s) remote 의
//!   자동 인증 실패는 `needs_manual`. 오류 문구는 300 UTF-16 단위로 자른다(서로게이트가 쪼개지면 U+FFFD).
//!
//! **승인(`fetchApprovals`·`trustedConfigs`)은 main 의 설정 파일에만 있다.** 이 모듈은 그것을 쓰지 않고 [`FetchHost::approve`] 로 사이드카에 부탁할 뿐이며,
//! 부탁하는 곳은 정확히 두 군데다: 수동 fetch 성공 뒤(`ApprovalKind::Approval`)와 확인 창에서 "이 저장소에서 항상 허용" 을 받은 뒤(`ApprovalKind::Trusted`).
//! 읽는 쪽은 사이드카가 보낸 [`FetchPrefs`] 스냅샷과, 방금 부탁한 값을 되돌아오기 전까지 쥐는 메모리 오버레이다. 판정은 항상 **지문이 같은가**다("항목이 있는가"가 아니다).
//!
//! 이식 규칙: 운영 경로는 플랫폼·ssh 환경을 주입받지 않는다(`run_fetch`·`inspect_config` 만 부른다 — 주입 버전은 `*_for_test` 이고
//! `tests/fetch_service.rs` 가 이 파일의 소스에 그 이름이 없음을 강제한다). 외부 의존(이벤트·확인 창·승인·사이드카 알림)은 [`FetchHost`] 로 주입한다.

use std::collections::HashMap;
use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use futures_util::future::{BoxFuture, FutureExt, Shared};
use serde::Serialize;
use serde_json::Value;
use tokio::sync::OnceCell;
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

mod github;

use super::confirm_text::{fetch_confirm_plan, ConfirmBox, Decision};
use super::guard::{classify_failure, next_delay_ms, version_at_least, FetchFailure, GitVersion, FILE_TRANSPORT_MIN_GIT};
use super::run::{git_version, inspect_config, run_fetch, FetchRunOpts, GitRunner};
use super::slice_utf16_lossy;
use crate::git::exec::queue::ProcessQueue;
use crate::git::exec::ExecError;
use crate::git::log_service::NoteOp;
use crate::git::signature::refs_signature;
use crate::mirror::{RepoMirror, Resolved, Target};

/// 자동 fetch 기본 주기(분). TS `DEFAULT_AUTO_FETCH_MINUTES`. 기본값을 정하는 곳은 사이드카 한 곳이라(`fetchPrefs.autoFetchMinutes` 는 이미 해석된 값) 이 상수는 테스트·문서용이다
pub const DEFAULT_AUTO_FETCH_MINUTES: f64 = 5.0;
/// 저장소를 선택한 뒤 첫 자동 fetch 까지의 지연(선택 직후의 조회들이 먼저 끝나도록)
pub const FIRST_DELAY: Duration = Duration::from_secs(3);
/// fetch 한 번의 제한 시간(요청 시점부터)
pub const FETCH_TIMEOUT: Duration = Duration::from_secs(60);
/// GitHub 원격 전용 저장소의 첫 fetch(아직 ref 가 없다)는 이력 전체를 받으므로 넉넉히 둔다. 큐 슬롯을 얻은 뒤부터 잰다
pub const GITHUB_FIRST_FETCH_TIMEOUT: Duration = Duration::from_secs(60 * 60);
/// 저장소를 지우기 전에 진행 중인 fetch 가 끝나기를 기다리는 최대 시간(그 뒤에는 고아 캐시 정리가 안전망이다)
pub const FORGET_WAIT: Duration = Duration::from_secs(5);
/// "삭제 중" 표식의 안전장치 시간 — 레코드가 지워지지 않아도 이 시간이 지나면 풀린다(그 저장소가 영영 막히지 않게)
const REMOVING_TTL: Duration = Duration::from_secs(30);
const REMOVING_MESSAGE: &str = "저장소를 삭제하는 중입니다";
const MAX_ERROR_LEN: usize = 300;
/// 주기 계산이 JS 숫자처럼 무한이 될 때의 상한
const DELAY_CAP: Duration = Duration::from_secs(30 * 24 * 60 * 60);

// ── 공개 타입 ─────────────────────────────────────────────────────────────

/// TS `FetchStatus`(`src/shared/types.ts`). renderer 로 가는 이벤트 모양 그대로 직렬화된다
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FetchStatus {
    pub repo_id: String,
    pub fetching: bool,
    /// 마지막으로 성공한 시각(epoch ms)
    pub last_ok_at: Option<u64>,
    pub error: Option<String>,
    pub blocked: Option<Blocked>,
    pub auth_paused: bool,
    pub needs_manual: bool,
    pub no_remote: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Blocked {
    pub keys: Vec<String>,
    /// 수동 새로고침에서 사용자가 허용할 수 있다
    pub confirmable: bool,
}

impl FetchStatus {
    fn idle(repo_id: &str) -> Self {
        FetchStatus { repo_id: repo_id.to_string(), fetching: false, last_ok_at: None, error: None, blocked: None, auth_paused: false, needs_manual: false, no_remote: false }
    }
}

/// 호스트로 나가는 이벤트
#[derive(Debug, Clone, PartialEq)]
pub enum HostEvent {
    /// `fetch:status`
    Status(FetchStatus),
    /// `repo:changed` — fetch 가 ref 를 바꿨다. **항상 [`FetchHost::note`] 다음에** 나간다
    RepoChanged(String),
}

/// 승인의 종류. 프로토콜의 `kind` 문자열이 이 둘뿐이다
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalKind {
    /// 수동 fetch 가 성공한 remote(이름+URL 지문) — 자동 fetch 가 자격 증명 도우미를 써도 된다
    Approval,
    /// 확인 창에서 "항상 허용" 한 위험 설정 지문
    Trusted,
}

impl ApprovalKind {
    /// 승인 종류의 이름(로그·시험용 — 설정에는 종류별 변경자가 따로 있다)
    pub fn as_str(self) -> &'static str {
        match self {
            ApprovalKind::Approval => "approval",
            ApprovalKind::Trusted => "trusted",
        }
    }
}

/// 서비스가 바깥에 기대는 것. 앱에서는 Tauri 이벤트·네이티브 확인 창·설정 쓰기(`ConfigStore`)로 구현한다(`fetch_host.rs`)
pub trait FetchHost: Send + Sync + 'static {
    fn emit(&self, event: HostEvent);

    /// 확인 창을 띄우고 눌린 버튼 번호를 돌려준다(창이 없거나 실패하면 None). 창이 닫힐 때까지 블로킹이면 구현이 `spawn_blocking` 으로 감싼다.
    /// **응답의 해석은 서비스가 [`super::confirm_text::ConfirmPlan::decide`] 로만 한다** — 구현이 허용 여부를 정하지 않는다
    fn confirm(&self, repo_id: &str, confirm: &ConfirmBox) -> impl Future<Output = Option<i64>> + Send;

    /// 승인을 설정 파일에 남긴다(Rust 가 설정의 유일한 작성자다 — 동기 쓰기). 서비스가 부르는 곳은 두 군데뿐이다(모듈 문서)
    fn approve(&self, kind: ApprovalKind, repo_id: &str, fingerprint: &str);

    /// 사이드카의 RepoService 에 알린다(`{t:"note"}`)
    fn note(&self, op: NoteOp, repo_id: &str);

    /// GitHub 원격 전용 저장소의 fetch 가 HEAD 를 맞추며 알아낸 기본 브랜치가 레코드의 값과 다르다 — 설정의 레코드에 기록한다(Rust 가 설정의 유일한 작성자다).
    /// 서비스가 부르는 곳은 github 갈래 한 군데뿐이고, 이름은 `is_safe_branch` 를 통과한 값이다
    fn github_branch(&self, _repo_id: &str, _branch: &str) {}

    /// 확인 창 제목에 쓸 저장소 이름(TS `config.repo(id)?.name ?? id`). 앱은 미러의 `name` 에서 꺼낸다. 없으면 id 를 쓴다
    fn repo_name(&self, _repo_id: &str) -> Option<String> {
        None
    }
}

/// 사이드카가 보내는 fetch 설정 스냅샷(`{t:"fetchPrefs", gen, autoFetchMinutes, approvals, trusted}`)
#[derive(Debug, Clone, PartialEq)]
pub struct FetchPrefs {
    pub gen: u64,
    /// 해석이 끝난 자동 fetch 주기(분). 0 이면 끈다
    pub auto_fetch_minutes: f64,
    /// 저장소 id → 수동 fetch 로 승인된 remote 지문
    pub approvals: HashMap<String, String>,
    /// 저장소 id → "항상 허용" 한 위험 설정 지문
    pub trusted: HashMap<String, String>,
}

/// 소문자 16진수 64자(sha256 지문)인가
pub fn is_fingerprint(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn fingerprint_map(v: &Value) -> Option<HashMap<String, String>> {
    let obj = v.as_object()?;
    // 모양이 틀린 지문은 그 항목만 뺀다 — 없는 것과 같다(승인되지 않음 = 안전한 쪽)
    Some(obj.iter().filter_map(|(k, v)| v.as_str().filter(|s| is_fingerprint(s)).map(|s| (k.clone(), s.to_string()))).collect())
}

impl FetchPrefs {
    /// 사이드카 메시지를 읽는다. 모양이 틀리면(gen·주기가 숫자가 아니거나 음수·무한, 승인 맵이 객체가 아님) None — 적용하지 않는다
    pub fn from_message(msg: &Value) -> Option<Self> {
        let gen = msg.get("gen")?.as_u64()?;
        let minutes = msg.get("autoFetchMinutes")?.as_f64().filter(|m| m.is_finite() && *m >= 0.0)?;
        Some(FetchPrefs { gen, auto_fetch_minutes: minutes, approvals: fingerprint_map(msg.get("approvals")?)?, trusted: fingerprint_map(msg.get("trusted")?)? })
    }
}

/// 진행 중인 fetch 를 기다리는 핸들. 복제해 여럿이 기다릴 수 있고, [`Shared::ptr_eq`] 로 같은 요청인지 볼 수 있다
pub type FetchFuture = Shared<BoxFuture<'static, FetchStatus>>;

// ── 내부 상태 ─────────────────────────────────────────────────────────────

#[derive(Default)]
struct State {
    last_ok_at: Option<u64>,
    last_attempt: Option<Instant>,
    error: Option<String>,
    blocked: Option<Blocked>,
    auth_paused: bool,
    no_remote: bool,
    failures: u32,
    needs_manual: bool,
    fetching: bool,
    /// 이번 실행에서 사용자가 허용한 위험 설정의 지문("항상 허용"은 trusted 에 따로 남는다)
    accepted: Option<String>,
}

struct Running {
    future: FetchFuture,
    token: CancellationToken,
}

#[derive(Default)]
struct Core {
    states: HashMap<String, State>,
    inflight: HashMap<String, Running>,
    active: Option<String>,
    timer: Option<JoinHandle<()>>,
    /// 첫 fetchPrefs 를 받기 전에는 None — 자동 fetch 를 예약하지 않는다
    minutes: Option<f64>,
    last_gen: u64,
    approvals: HashMap<String, String>,
    trusted: HashMap<String, String>,
    /// 방금 사이드카에 부탁한 값. fetchPrefs 가 되돌려 주기 전까지(또는 저장소가 사라질 때까지) 쓴다
    overlay_approvals: HashMap<String, String>,
    overlay_trusted: HashMap<String, String>,
    /// "삭제 중"인 저장소(id → 만료 시각). `forget_and_wait` 가 달고, 새 fetch 를 거부한다. 레코드가 지워져 Rust 가 맡지 않게 되거나 [`REMOVING_TTL`] 이 지나면 풀린다
    removing: HashMap<String, Instant>,
}

impl Core {
    fn state(&mut self, repo_id: &str) -> &mut State {
        self.states.entry(repo_id.to_string()).or_default()
    }

    fn auto_enabled(&mut self, repo_id: &str) -> bool {
        let on = self.minutes.is_some_and(|m| m > 0.0);
        let s = self.state(repo_id);
        on && s.blocked.is_none() && !s.auth_paused && !s.no_remote
    }

    fn status(&mut self, repo_id: &str) -> FetchStatus {
        let s = self.state(repo_id);
        FetchStatus {
            repo_id: repo_id.to_string(),
            fetching: s.fetching,
            last_ok_at: s.last_ok_at,
            error: s.error.clone(),
            blocked: s.blocked.clone(),
            auth_paused: s.auth_paused,
            needs_manual: s.needs_manual,
            no_remote: s.no_remote,
        }
    }
}

/// 환경(source_key)마다 하나: 실행기·fetch 전용 큐(동시 1)·git 버전 캐시
struct Source<R> {
    runner: Arc<R>,
    queue: ProcessQueue<bool>,
    version: OnceCell<Option<GitVersion>>,
}

/// 한 번의 실행 중에 큐 작업이 남기고 실패 분류가 읽는 값(TS 의 `approvedForRun`·`hasHttp` 지역 변수)
struct RunFlags {
    approved_for_run: AtomicBool,
    has_http: AtomicBool,
    deadline: Deadline,
    /// github 갈래가 큐 슬롯을 얻은 뒤 거는 제한(초 — `run` 이 ref 유무로 정한다). 일반 저장소는 쓰지 않는다
    github_limit_secs: AtomicU64,
}

/// fetch 한 번의 제한 시간 타이머. 걸린 제한과 터졌는지를 실패 분류가 읽는다(TS 의 `timeoutMs`·`timedOut`).
/// 일반 저장소는 요청 때, github 는 큐 슬롯을 얻은 뒤에 건다 — 한 번만 걸고, 버려지면 타이머도 끝난다
struct Deadline {
    fired: Arc<AtomicBool>,
    secs: AtomicU64,
    timer: Mutex<Option<AbortOnDrop>>,
}

impl Deadline {
    fn new() -> Self {
        Deadline { fired: Arc::new(AtomicBool::new(false)), secs: AtomicU64::new(FETCH_TIMEOUT.as_secs()), timer: Mutex::new(None) }
    }

    fn arm(&self, token: &CancellationToken, limit: Duration) {
        self.secs.store(limit.as_secs(), Ordering::SeqCst);
        let (token, fired) = (token.clone(), self.fired.clone());
        *lock(&self.timer) = Some(AbortOnDrop(tokio::spawn(async move {
            tokio::time::sleep(limit).await;
            fired.store(true, Ordering::SeqCst);
            token.cancel();
        })));
    }

    fn timed_out(&self) -> bool {
        self.fired.load(Ordering::SeqCst)
    }
}

/// 버려지면 태스크를 끝낸다(패닉·취소로 빠져도 60초 타이머가 남지 않게)
struct AbortOnDrop(JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

type RunnerFactory<R> = Box<dyn Fn(&Target) -> Arc<R> + Send + Sync>;

struct Inner<H, R> {
    host: H,
    mirror: RepoMirror,
    make_runner: RunnerFactory<R>,
    /// 로컬 git 이 core.hooksPath 로 쓸 앱 소유의 빈 디렉터리(드라이브 문자가 있는 절대 경로). Windows 에서는 필수다
    no_hooks_dir: Option<String>,
    /// GitHub 원격 전용 저장소의 앱 캐시 기준 폴더 등(`with_github_cache` 가 정한다). 없으면 github 저장소는 "캐시 경로가 맞지 않음"으로 막힌다
    github: Option<github::GithubFetch>,
    sources: Mutex<HashMap<String, Arc<Source<R>>>>,
    core: Mutex<Core>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

fn delay_for(minutes: f64, failures: u32) -> Duration {
    Duration::try_from_secs_f64(next_delay_ms(minutes, failures) / 1000.0).unwrap_or(DELAY_CAP)
}

impl<H: FetchHost, R: GitRunner + Send + 'static> Inner<H, R> {
    fn with_state<T>(&self, repo_id: &str, f: impl FnOnce(&mut State) -> T) -> T {
        f(lock(&self.core).state(repo_id))
    }

    fn status(&self, repo_id: &str) -> FetchStatus {
        lock(&self.core).status(repo_id)
    }

    fn emit_status(&self, repo_id: &str) -> FetchStatus {
        let status = self.status(repo_id);
        self.host.emit(HostEvent::Status(status.clone()));
        status
    }

    /// 이 저장소의 환경(실행기·큐). 미러에 없거나 Rust 가 fetch 하지 않는 저장소(github, 꺼진 ssh)면 오류(TS `fetchContext` 의 `unknown repo` 와 같다)
    fn resolve(&self, repo_id: &str) -> Result<(Target, Arc<Source<R>>), ExecError> {
        let target = match self.mirror.resolve_fetch(repo_id) {
            Resolved::Rust(t) => t,
            Resolved::Sidecar => return Err(ExecError::Other { message: format!("unknown repo: {repo_id}") }),
            Resolved::Invalid(message) => return Err(ExecError::Other { message }),
        };
        let key = target.source_key();
        let mut sources = lock(&self.sources);
        // ssh: `resolve_fetch` 와 이 사이에 미러가 바뀌어 호스트가 빠졌으면 실행기를 만들지 않는다(낡은 대상이 지운 호스트로 풀에 다시 등록하지 않게).
        // `sources` 잠금 안에서 본다 — 정리(`sync_mirror`)도 이 잠금을 잡는다. 라우터의 `runtime` 과 같다
        if let Target::Ssh { host, .. } = &target {
            if !self.mirror.ssh_hosts().contains(host) {
                return Err(ExecError::Other { message: "저장소 목록이 바뀌었습니다. 다시 시도하세요".to_string() });
            }
        }
        // 풀이 닫은 실행기(미러에서 빠졌다가 다시 추가된 호스트)를 계속 쥐지 않는다 — 버리고 새로 받는다
        if sources.get(&key).is_some_and(|s| s.runner.is_closed()) {
            sources.remove(&key);
        }
        let source = sources
            .entry(key)
            .or_insert_with(|| Arc::new(Source { runner: (self.make_runner)(&target), queue: ProcessQueue::new(1), version: OnceCell::new() }))
            .clone();
        Ok((target, source))
    }

    /// 위험 설정 지문을 사용자가 허용했는가(이번 실행 동안, 또는 이 저장소에서 항상). **지문이 같을 때만**이다
    fn accepted(&self, repo_id: &str, fingerprint: &str) -> bool {
        let mut core = lock(&self.core);
        if core.state(repo_id).accepted.as_deref() == Some(fingerprint) {
            return true;
        }
        core.trusted.get(repo_id).is_some_and(|f| f == fingerprint) || core.overlay_trusted.get(repo_id).is_some_and(|f| f == fingerprint)
    }

    /// remote 지문이 수동 fetch 로 승인됐는가(같은 이름+URL 일 때만)
    fn remotes_approved(&self, repo_id: &str, fingerprint: &str) -> bool {
        let core = lock(&self.core);
        core.approvals.get(repo_id).is_some_and(|f| f == fingerprint) || core.overlay_approvals.get(repo_id).is_some_and(|f| f == fingerprint)
    }

    /// 승인을 사이드카에 부탁하고 되돌아오기 전까지 메모리에 쥔다. **서비스에서 승인을 쓰는 유일한 함수**다
    fn send_approval(&self, kind: ApprovalKind, repo_id: &str, fingerprint: &str) {
        {
            let mut core = lock(&self.core);
            let overlay = match kind {
                ApprovalKind::Approval => &mut core.overlay_approvals,
                ApprovalKind::Trusted => &mut core.overlay_trusted,
            };
            overlay.insert(repo_id.to_string(), fingerprint.to_string());
        }
        self.host.approve(kind, repo_id, fingerprint);
    }

    /// Rust fetch 서비스가 맡는 저장소인가 — **소스 종류(`rust_fetch_kinds`)로** 정한다(`RepoMirror::resolve_fetch`). 사이드카의 소유 판정과 같은 술어라 이중 fetch 가 없다
    fn is_rust_repo(&self, repo_id: &str) -> bool {
        matches!(self.mirror.resolve_fetch(repo_id), Resolved::Rust(_))
    }

    /// 미러에 없는(또는 Rust 로 풀리지 않는) ssh 호스트의 환경과 풀이 닫은 실행기의 환경을 버린다. 연결은 풀(라우터의 `sync_ssh_hosts`)이 닫는다 —
    /// 여기서는 닫힌 실행기를 계속 쥐지 않게 `sources` 에서만 뺀다. 저장소 상태가 없어도 거른다(호스트 단위다)
    fn sync_sources(&self) {
        let keep = self.mirror.ssh_hosts();
        let mut sources = lock(&self.sources);
        sources.retain(|key, s| !s.runner.is_closed() && key.strip_prefix("ssh:").is_none_or(|host| keep.contains(host)));
    }

    // ── 예약 ──────────────────────────────────────────────────────────────

    fn schedule(self: &Arc<Self>) {
        let mut core = lock(&self.core);
        if let Some(t) = core.timer.take() {
            t.abort();
        }
        let Some(id) = core.active.clone() else { return };
        if !core.auto_enabled(&id) {
            return;
        }
        let Some(minutes) = core.minutes else { return };
        let s = core.state(&id);
        let wait = match s.last_attempt {
            None => FIRST_DELAY,
            Some(at) => delay_for(minutes, s.failures).saturating_sub(Instant::now().saturating_duration_since(at)).max(FIRST_DELAY),
        };
        let this = self.clone();
        core.timer = Some(tokio::spawn(async move {
            tokio::time::sleep(wait).await;
            // 시작만 하면 된다 — 끝나는 쪽(run_guarded)이 다시 예약한다
            drop(this.fetch(&id, false));
        }));
    }

    // ── fetch ─────────────────────────────────────────────────────────────

    fn fetch(self: &Arc<Self>, repo_id: &str, manual: bool) -> FetchFuture {
        let mut core = lock(&self.core);
        if let Some(running) = core.inflight.get(repo_id) {
            return running.future.clone();
        }
        // 삭제 중인 저장소(`forget_and_wait`)는 새 fetch 를 시작하지 않는다 — 캐시 폴더를 지우기 전에 git 이 폴더를 다시 쥐지 않게
        if core.removing.get(repo_id).is_some_and(|until| *until > Instant::now()) {
            let status = FetchStatus { error: Some(REMOVING_MESSAGE.to_string()), ..FetchStatus::idle(repo_id) };
            return async move { status }.boxed().shared();
        }
        let token = CancellationToken::new();
        let id = repo_id.to_string();
        let task = tokio::spawn(Self::run_guarded(self.clone(), id.clone(), manual, token.clone()));
        let fallback = id.clone();
        let future: FetchFuture = async move { task.await.unwrap_or_else(|_| FetchStatus::idle(&fallback)) }.boxed().shared();
        core.inflight.insert(id, Running { future: future.clone(), token });
        future
    }

    /// 본체를 돌리고 TS 의 `.finally` 와 같이 정리한다. 본체가 패닉해도 `fetching` 은 풀린다(위험 5: 패닉 시 fetch 안 함)
    async fn run_guarded(self: Arc<Self>, repo_id: String, manual: bool, token: CancellationToken) -> FetchStatus {
        let status = match AssertUnwindSafe(self.run(&repo_id, manual, token)).catch_unwind().await {
            Ok(status) => status,
            Err(_) => {
                self.with_state(&repo_id, |s| {
                    s.fetching = false;
                    s.error = Some("내부 오류로 fetch 하지 않았습니다".into());
                });
                self.emit_status(&repo_id)
            }
        };
        let reschedule = {
            let mut core = lock(&self.core);
            core.inflight.remove(&repo_id);
            core.active.as_deref() == Some(repo_id.as_str())
        };
        if reschedule {
            self.schedule();
        }
        status
    }

    async fn run(self: &Arc<Self>, repo_id: &str, manual: bool, token: CancellationToken) -> FetchStatus {
        self.with_state(repo_id, |s| {
            s.last_attempt = Some(Instant::now());
            if manual {
                s.auth_paused = false;
                s.failures = 0;
            }
            s.fetching = true;
        });
        self.emit_status(repo_id);

        // 소스가 github 면(레코드가 어긋나 있어도) 일반 저장소 흐름으로 가지 않는다 — `fetch_task` 가 `Target::Github` 를 github 갈래로만 보낸다
        let resolved = self.resolve(repo_id);
        let github = matches!(&resolved, Ok((Target::Github { .. }, _)));
        let flags = Arc::new(RunFlags { approved_for_run: AtomicBool::new(true), has_http: AtomicBool::new(false), deadline: Deadline::new(), github_limit_secs: AtomicU64::new(FETCH_TIMEOUT.as_secs()) });
        // 일반 저장소는 요청 때부터 잰다. github 는 큐 슬롯을 얻은 뒤부터 잰다(첫 fetch 는 이력 전체를 받는 긴 작업이라 다른 fetch 를 기다린 시간이 제한을 갉아먹지 않게)
        if !github {
            flags.deadline.arm(&token, FETCH_TIMEOUT);
        }
        self.with_state(repo_id, |s| s.needs_manual = false);

        let result = match resolved {
            Ok((target, source)) => {
                if github {
                    flags.github_limit_secs.store(self.github_limit(repo_id, &target, &source, &token).await.as_secs(), Ordering::SeqCst);
                }
                let queue = source.queue.clone();
                let (this, id, tok, fl) = (self.clone(), repo_id.to_string(), token.clone(), flags.clone());
                queue.run(None, Some(token.clone()), move || this.fetch_task(id, target, source, manual, tok, fl)).await
            }
            Err(e) => Err(e),
        };

        match result {
            Ok(changed) => {
                self.with_state(repo_id, |s| {
                    s.error = None;
                    s.failures = 0;
                    s.auth_paused = false;
                });
                if changed {
                    // 사이드카가 기억한 시그니처를 먼저 버린다 — repo:changed 로 다시 계산하는 쪽이 낡은 값을 쓰지 않게
                    self.host.note(NoteOp::FetchChanged, repo_id);
                    self.host.emit(HostEvent::RepoChanged(repo_id.to_string()));
                }
            }
            Err(err) => self.record_failure(repo_id, &err, manual, github, &flags),
        }
        self.with_state(repo_id, |s| s.fetching = false);
        self.emit_status(repo_id)
    }

    /// `github`: GitHub 원격 전용 저장소의 fetch 다 — 분류가 "권한이 없는 저장소는 404 로 숨긴다"(401·403·404)를 인증 문제로 본다
    fn record_failure(&self, repo_id: &str, err: &ExecError, manual: bool, github: bool, flags: &RunFlags) {
        let timed_out = flags.deadline.timed_out();
        self.with_state(repo_id, |s| {
            if matches!(err, ExecError::Aborted) && !timed_out {
                s.error = None; // 취소는 오류가 아니다
                return;
            }
            let text = if timed_out { format!("시간 초과({}초)", flags.deadline.secs.load(Ordering::SeqCst)) } else { err.to_string() };
            let mut error = slice_utf16_lossy(&text, MAX_ERROR_LEN);
            // SSH 전송 오류(연결·호스트 키·인증·셸 확인 실패)는 문구가 아니라 종류로 network 다 — 재시도해도 소용없는 쪽이 아니라 백오프로 다시 본다.
            // 원격 git 이 낸 오류(`Git`)는 그대로 문구로 분류한다(원격이 자기 remote 에 인증하지 못한 것은 auth)
            let kind = if timed_out || matches!(err, ExecError::Aborted | ExecError::Ssh { .. }) { FetchFailure::Network } else { classify_failure(&err.to_string(), github) };
            if kind == FetchFailure::Protocol {
                error = slice_utf16_lossy(&format!("자동 fetch 는 로컬 경로(file) remote 를 쓰지 않습니다. 새로고침으로 직접 fetch 하세요. {error}"), MAX_ERROR_LEN);
            }
            if kind == FetchFailure::Auth && !manual && !flags.approved_for_run.load(Ordering::SeqCst) && flags.has_http.load(Ordering::SeqCst) {
                s.needs_manual = true;
            }
            if kind == FetchFailure::Network {
                s.failures += 1;
            } else {
                s.auth_paused = true;
            }
            s.error = Some(error);
        });
    }

    /// 큐 슬롯 안에서 도는 본체(TS `queue.run` 의 콜백). 바뀐 ref 가 있으면 true
    async fn fetch_task(self: Arc<Self>, repo_id: String, target: Target, source: Arc<Source<R>>, manual: bool, token: CancellationToken, flags: Arc<RunFlags>) -> Result<bool, ExecError> {
        // GitHub 원격 전용 저장소는 허용 목록·확인 창 없이 자기 갈래로만 간다(일반 저장소가 이 갈래를 타지도, 이 저장소가 아래 일반 갈래를 타지도 않는다)
        if matches!(target, Target::Github { .. }) {
            return self.github_task(repo_id, target, source, token, flags).await;
        }
        let (r, path) = (&*source.runner, target.path());
        let ins = inspect_config(r, path, Some(&token)).await?;
        let blocked = |keys: Vec<String>, confirmable: bool| Some(Blocked { keys, confirmable });
        let forbidden_keys = |ins: &super::guard::ConfigInspection| ins.forbidden.iter().map(|f| format!("{} ({})", f.key, f.reason)).collect::<Vec<_>>();
        let dangerous_keys = |ins: &super::guard::ConfigInspection| ins.dangerous.iter().map(|d| d.key.clone()).collect::<Vec<_>>();

        self.with_state(&repo_id, |s| {
            s.no_remote = !ins.has_remote;
            s.blocked = None;
        });
        if !ins.has_remote {
            return Ok(false);
        }
        if !ins.forbidden.is_empty() {
            // remote URL 은 허용 여부를 묻지 않고 거부한다
            self.with_state(&repo_id, |s| s.blocked = blocked(forbidden_keys(&ins), false));
            return Ok(false);
        }
        if !ins.dangerous.is_empty() && !self.accepted(&repo_id, &ins.fingerprint) {
            let decision = if manual {
                let name = self.host.repo_name(&repo_id).unwrap_or_else(|| repo_id.clone());
                let items: Vec<(&str, &str)> = ins.dangerous.iter().map(|d| (d.key.as_str(), d.value.as_str())).collect();
                let plan = fetch_confirm_plan(&name, &items);
                // 응답의 해석은 창 종류에 묶여 있다 — 차단 창("닫기" = 0)은 허용이 될 수 없다
                plan.decide(self.host.confirm(&repo_id, plan.confirm_box()).await)
            } else {
                Decision::Deny
            };
            if decision == Decision::Deny {
                self.with_state(&repo_id, |s| s.blocked = blocked(dangerous_keys(&ins), true));
                return Ok(false);
            }
            self.with_state(&repo_id, |s| s.accepted = Some(ins.fingerprint.clone()));
            // 확인 창이 떠 있는 동안 저장소를 지웠으면 남기지 않는다. 저장은 사이드카가 하고, 실패해도 이번 실행 동안의 허용으로 계속한다
            if decision == Decision::Always && self.is_rust_repo(&repo_id) {
                self.send_approval(ApprovalKind::Trusted, &repo_id, &ins.fingerprint);
            }
        }
        // 검사와 실행 사이에 설정이 바뀌었을 수 있다(TOCTOU) — 실행 직전에 다시 검사해 달라졌으면 멈춘다
        let again = inspect_config(r, path, Some(&token)).await?;
        if !again.forbidden.is_empty() {
            self.with_state(&repo_id, |s| s.blocked = blocked(forbidden_keys(&again), false));
            return Ok(false);
        }
        if !again.dangerous.is_empty() && !self.accepted(&repo_id, &again.fingerprint) {
            self.with_state(&repo_id, |s| s.blocked = blocked(dangerous_keys(&again), true));
            return Ok(false);
        }
        // file 전송은 수동 fetch 에서, 그것도 CVE-2024-32004 가 고쳐진 git 에서만 허용한다
        let allow_file = manual && version_at_least(*source.version.get_or_init(|| git_version(r)).await, FILE_TRANSPORT_MIN_GIT);
        let before = refs_signature(r, path, &token).await.ok();
        // 자동 fetch 는 수동 fetch 로 승인된 remote(이름+URL 지문이 같음)에서만 자격 증명 도우미를 쓴다
        let approved = self.remotes_approved(&repo_id, &again.remotes_fingerprint);
        flags.approved_for_run.store(approved, Ordering::SeqCst);
        flags.has_http.store(again.remotes.iter().any(|rm| is_http_url(&rm.url)), Ordering::SeqCst);
        let opts = FetchRunOpts { cancel: Some(token.clone()), no_hooks_dir: self.no_hooks_dir.as_deref(), allow_file, credential_helpers: manual || approved };
        run_fetch(r, path, &again, &opts).await?;
        if manual {
            self.send_approval(ApprovalKind::Approval, &repo_id, &again.remotes_fingerprint);
        }
        let after = refs_signature(r, path, &token).await.ok();
        self.with_state(&repo_id, |s| s.last_ok_at = Some(now_ms()));
        Ok(before != after)
    }
}

/// `/^https?:/i`
fn is_http_url(url: &str) -> bool {
    let lc = url.get(..6).map_or_else(|| url.to_ascii_lowercase(), str::to_ascii_lowercase);
    lc.starts_with("http:") || lc.starts_with("https:")
}

// ── 공개 서비스 ───────────────────────────────────────────────────────────

/// fetch 서비스. 복제는 같은 서비스를 가리킨다(Arc)
pub struct FetchService<H: FetchHost, R: GitRunner + Send + 'static> {
    inner: Arc<Inner<H, R>>,
}

impl<H: FetchHost, R: GitRunner + Send + 'static> Clone for FetchService<H, R> {
    fn clone(&self) -> Self {
        FetchService { inner: self.inner.clone() }
    }
}

impl<H: FetchHost, R: GitRunner + Send + 'static> FetchService<H, R> {
    /// `make_runner` 는 환경(source_key)마다 한 번 불린다. `no_hooks_dir` 는 만들어 둔다(못 만들면 fetch 때 검사에서 거부한다)
    pub fn new(host: H, mirror: RepoMirror, make_runner: impl Fn(&Target) -> Arc<R> + Send + Sync + 'static, no_hooks_dir: Option<String>) -> Self {
        if let Some(dir) = &no_hooks_dir {
            let _ = std::fs::create_dir_all(dir);
        }
        FetchService {
            inner: Arc::new(Inner { host, mirror, make_runner: Box::new(make_runner), no_hooks_dir, github: None, sources: Mutex::default(), core: Mutex::default() }),
        }
    }

    pub fn status(&self, repo_id: &str) -> FetchStatus {
        self.inner.status(repo_id)
    }

    /// fetch 를 시작한다(이미 진행 중이면 그 요청에 합류한다). 기다리지 않아도 끝까지 간다. 사용자가 새로고침을 눌렀으면 `manual` 이다 —
    /// 위험 설정 확인 창을 띄울 수 있고, 인증 실패·백오프 상태를 풀고 다시 시도한다.
    /// tokio 런타임 안에서 불러야 한다
    pub fn fetch(&self, repo_id: &str, manual: bool) -> FetchFuture {
        self.inner.fetch(repo_id, manual)
    }

    /// 진행 중인 fetch 를 끊는다
    pub fn cancel(&self, repo_id: &str) {
        if let Some(running) = lock(&self.inner.core).inflight.get(repo_id) {
            running.token.cancel();
        }
    }

    /// 자동 fetch 대상(활성 저장소)을 바꾼다
    pub fn set_active(&self, repo_id: Option<&str>) {
        {
            let mut core = lock(&self.inner.core);
            if core.active.as_deref() == repo_id {
                return;
            }
            core.active = repo_id.map(str::to_string);
        }
        self.inner.schedule();
        // 아직 한 번도 받지 못한 GitHub 원격 전용 저장소는 보여 줄 것이 없으므로 선택 즉시 받는다(`fetch_if_empty` 가 github 가 아니면 아무것도 하지 않는다)
        if let Some(id) = repo_id {
            let (this, id) = (self.inner.clone(), id.to_string());
            tokio::spawn(async move { this.fetch_if_empty(&id).await });
        }
    }

    /// 창이 다시 포커스를 얻었을 때: 마지막 시도가 주기보다 오래됐으면 바로 fetch
    pub fn on_focus(&self) {
        let id = {
            let mut core = lock(&self.inner.core);
            let Some(id) = core.active.clone() else { return };
            if !core.auto_enabled(&id) || core.inflight.contains_key(&id) {
                return;
            }
            let Some(minutes) = core.minutes else { return };
            let s = core.state(&id);
            // 아직 한 번도 시도하지 않았으면 schedule 의 첫 지연(FIRST_DELAY)에 맡긴다 — 시작 직후 포커스로 조회와 겹치지 않게
            let Some(at) = s.last_attempt else { return };
            if Instant::now().saturating_duration_since(at) < delay_for(minutes, s.failures) {
                return;
            }
            id
        };
        drop(self.inner.fetch(&id, false));
    }

    /// 저장소를 지웠다(미러에서 사라졌다): 진행 중인 fetch 를 끊고 상태·오버레이를 비운다
    pub fn forget(&self, repo_id: &str) {
        let was_active = {
            let mut core = lock(&self.inner.core);
            if let Some(running) = core.inflight.get(repo_id) {
                running.token.cancel();
            }
            core.states.remove(repo_id);
            core.overlay_approvals.remove(repo_id);
            core.overlay_trusted.remove(repo_id);
            core.active.as_deref() == Some(repo_id)
        };
        if was_active {
            self.set_active(None);
        }
    }

    /// 사이드카의 fetchPrefs 를 적용한다. 이미 받은 것 이하의 gen 이면 버리고 false.
    /// 오버레이는 사이드카가 같은 값을 돌려줬거나(반영됨) 다른 값으로 바뀌었거나(덮어씌워짐) 저장소가 사라졌으면 정리한다 —
    /// 값이 아직 없는 경우는 사이드카가 부탁을 처리하기 전의 스냅샷일 수 있어 남겨 둔다(실패하는 쪽은 안전한 쪽이다)
    pub fn apply_prefs(&self, prefs: FetchPrefs) -> bool {
        {
            let mut core = lock(&self.inner.core);
            if prefs.gen <= core.last_gen {
                return false;
            }
            core.last_gen = prefs.gen;
            core.minutes = Some(prefs.auto_fetch_minutes);
            let keep = |overlay: &mut HashMap<String, String>, snapshot: &HashMap<String, String>| {
                overlay.retain(|id, _| !snapshot.contains_key(id) && self.inner.is_rust_repo(id));
            };
            keep(&mut core.overlay_approvals, &prefs.approvals);
            keep(&mut core.overlay_trusted, &prefs.trusted);
            core.approvals = prefs.approvals;
            core.trusted = prefs.trusted;
        }
        self.inner.schedule();
        true
    }

    /// 미러가 바뀐 직후: 더 이상 Rust 가 맡지 않는(미러에서 사라졌거나 다른 환경으로 바뀐) 저장소의 상태를 비운다
    pub fn sync_mirror(&self) {
        let gone: Vec<String> = {
            let core = lock(&self.inner.core);
            let mut ids: Vec<&String> = core.states.keys().chain(core.inflight.keys()).chain(core.overlay_approvals.keys()).chain(core.overlay_trusted.keys()).collect();
            ids.extend(core.active.iter());
            ids.sort();
            ids.dedup();
            ids.into_iter().filter(|id| !self.inner.is_rust_repo(id)).cloned().collect()
        };
        for id in gone {
            self.forget(&id);
        }
        // 레코드가 지워져 더는 Rust 가 맡지 않는 저장소(또는 만료된 것)의 "삭제 중" 표식을 푼다
        {
            let mut core = lock(&self.inner.core);
            let now = Instant::now();
            core.removing.retain(|id, until| *until > now && self.inner.is_rust_repo(id));
        }
        self.inner.sync_sources();
    }

    /// 저장소를 지우기 전: "삭제 중" 표식을 달고(새 fetch 거부), 진행 중인 fetch 를 끊고 끝날 때까지 기다린다 — 캐시 폴더를 지우기 전에 git 이 손을 떼야 한다.
    /// 끊어도 끝나지 않는 fetch 가 삭제를 붙잡지 않도록 [`FORGET_WAIT`] 까지만 기다린다(TS `forgetAndWait`). 끝난 실행이 되살린 상태·이벤트는 지운다
    pub async fn forget_and_wait(&self, repo_id: &str) {
        let running = {
            let mut core = lock(&self.inner.core);
            core.removing.insert(repo_id.to_string(), Instant::now() + REMOVING_TTL);
            core.inflight.get(repo_id).map(|r| r.future.clone())
        };
        self.forget(repo_id);
        if let Some(running) = running {
            let _ = tokio::time::timeout(FORGET_WAIT, running).await;
        }
        lock(&self.inner.core).states.remove(repo_id);
    }

    /// GitHub 로그인 상태가 바뀌었다(로그인·로그아웃·토큰 만료): 인증 문제로 멈춰 있던 원격 전용 저장소의 자동 fetch 를 다시 연다.
    /// 저장소를 열고 있다면 아직 받은 것이 없는 경우에 바로 다시 시도한다. **진행 중인 fetch 는 건드리지 않는다**(TS 와 같다 — 토큰은 git 을 띄울 때마다 읽는다).
    /// tokio 런타임 안에서 불러야 한다
    pub fn resume_github_auth(&self) {
        self.inner.resume_github_auth();
    }

    pub fn dispose(&self) {
        let mut core = lock(&self.inner.core);
        if let Some(t) = core.timer.take() {
            t.abort();
        }
        core.active = None;
        for running in core.inflight.values() {
            running.token.cancel();
        }
    }
}

/// 라우터가 서비스를 다루는 입구. 서비스는 호스트·실행기 타입에 제네릭이라(`FetchHost::confirm` 이 `impl Future` 라 dyn 이 안 된다)
/// 라우터(`State<Router>`)가 제네릭이 되지 않게 이 객체 안전한 포트로만 부른다.
///
/// **tokio 런타임 안에서 불러야 하는 것**: `fetch`·`set_active`·`on_focus`·`apply_prefs`(예약이 태스크를 만든다). 사이드카 읽기 스레드·창 이벤트 스레드에서 부를 때는
/// 앱이 런타임에 들어간 뒤 부른다(`router::in_runtime_of`)
pub trait FetchControl: Send + Sync + 'static {
    fn fetch(&self, repo_id: &str, manual: bool) -> FetchFuture;
    fn set_active(&self, repo_id: Option<&str>);
    fn on_focus(&self);
    fn apply_prefs(&self, prefs: FetchPrefs) -> bool;
    fn sync_mirror(&self);
    fn dispose(&self);

    /// 저장소를 지우기 전에 fetch 를 멈추고 끝나기를 기다린다(`repos:remove` 가로채기가 부른다). 서비스가 없는 시험용 가짜는 기본(즉시 끝)을 쓴다
    fn forget_and_wait(&self, _repo_id: &str) -> BoxFuture<'static, ()> {
        async {}.boxed()
    }

    /// GitHub 로그인 상태가 바뀌었다 — 인증 문제로 멈춘 github 저장소를 다시 연다. 런타임 안에서 불러야 한다
    fn resume_github_auth(&self) {}
}

impl<H: FetchHost, R: GitRunner + Send + 'static> FetchControl for FetchService<H, R> {
    fn fetch(&self, repo_id: &str, manual: bool) -> FetchFuture {
        FetchService::fetch(self, repo_id, manual)
    }

    fn set_active(&self, repo_id: Option<&str>) {
        FetchService::set_active(self, repo_id);
    }

    fn on_focus(&self) {
        FetchService::on_focus(self);
    }

    fn apply_prefs(&self, prefs: FetchPrefs) -> bool {
        FetchService::apply_prefs(self, prefs)
    }

    fn sync_mirror(&self) {
        FetchService::sync_mirror(self);
    }

    fn dispose(&self) {
        FetchService::dispose(self);
    }

    fn forget_and_wait(&self, repo_id: &str) -> BoxFuture<'static, ()> {
        let (this, id) = (self.clone(), repo_id.to_string());
        async move { FetchService::forget_and_wait(&this, &id).await }.boxed()
    }

    fn resume_github_auth(&self) {
        FetchService::resume_github_auth(self);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 지문_모양은_소문자_16진수_64자다() {
        assert!(is_fingerprint(&"a".repeat(64)));
        assert!(is_fingerprint(&format!("{}{}", "0123456789".repeat(6), "abcd")));
        for bad in [String::new(), "a".repeat(63), "a".repeat(65), "A".repeat(64), "g".repeat(64), format!("{}\n", "a".repeat(63)), "é".repeat(32)] {
            assert!(!is_fingerprint(&bad), "{bad:?}");
        }
    }

    #[test]
    fn http_url_판정은_ascii_대소문자를_무시한다() {
        for u in ["http://x", "https://x", "HTTP://x", "HtTpS://x", "https:x"] {
            assert!(is_http_url(u), "{u}");
        }
        for u in ["ssh://x", "file:///x", "httpx://x", "", "h", "ｈttp://x", "ſ://x", "éhttp://x"] {
            assert!(!is_http_url(u), "{u}");
        }
    }

    #[test]
    fn 기본_주기와_상수는_ts_와_같다() {
        assert_eq!(FIRST_DELAY, Duration::from_millis(3_000));
        assert_eq!(FETCH_TIMEOUT, Duration::from_millis(60_000));
        assert!((DEFAULT_AUTO_FETCH_MINUTES - 5.0).abs() < f64::EPSILON);
    }
}
