//! 채널 라우터. renderer 의 `antler_invoke` 를 Rust 가 직접 처리할지 Node 사이드카로 넘길지 정한다.
//!
//! Rust 가 처리하는 것: [`RUST_CHANNELS`](`git:fetch` 포함 — fetch 가드·상태 기계는 `git::fetch`)에 있는 채널이고, 인자의 저장소 id 가 미러([`RepoMirror`])에 있고 kind 가 local·wsl 이고,
//! 환경변수 `ANTLER_RUST_GIT` 이 `0` 이 아닐 때. 그 밖에는 전부 사이드카가 지금까지처럼 처리한다
//! (미러에 없는 id 의 오류 문구도 사이드카가 만든다). `ANTLER_RUST_GIT=0` 은 즉시 되돌리는 스위치다.
//!
//! fetch 의 소유권: 스위치가 켜져 있으면 local·wsl 저장소의 fetch(수동·자동·포커스)는 Rust 의 서비스가 맡고, 앱은 사이드카를 env
//! `ANTLER_RUST_FETCH_KINDS=local,wsl` 로 띄워 사이드카의 FetchService 가 그 저장소를 fetch·예약하지 않게 한다([`sidecar_fetch_kinds`]).
//! 스위치가 꺼져 있으면 env 도 서비스도 없다 — 사이드카가 전부 맡는다(이중 fetch 없음).
//!
//! `jira:*`([`API_CHANNELS`])는 별개의 스위치다: `ANTLER_RUST_API` 가 `0` 이 아니고(`ANTLER_RUST_GIT=0` 이어도 꺼진다) 서비스가 주입돼 있으면 **미러와 무관하게**
//! Rust 가 처리한다([`api_host`]). 꺼져 있으면 앱은 서비스도 만들지 않고 키체인도 읽지 않으며 사이드카가 지금처럼 처리한다. 켜져 있으면 사이드카를
//! env `ANTLER_RUST_API=1` 로 띄워 jira 를 소유하지 않음을 알린다(라우터가 가로채므로 닿지 않는다 — 방어).
//!
//! 보안 경계(docs/security.md · docs/tauri.md "실행기"):
//! - 저장소 경로·kind·배포판은 **미러에서만** 가져온다. renderer 가 보낸 값은 저장소 id 와 검증된 값(해시·ref 범위·파일 경로·숫자)뿐이다.
//! - git 인자는 `git::commit`·`git::scope`(log)·`git::file_channels`(compare·fileDiff·fileHistory·blame) 가 서브커맨드·옵션 상수 + 검증된 값으로만 만든다.
//!   파일 경로는 항상 `--` 뒤(blame 의 크기 조회만 `<해시>:<경로>` 한 인자)다. 이 파일은 인자를 만들지 않는다.
//! - 취소: `requestId → CancellationToken` 레지스트리. cancel 이 등록보다 먼저 도착해도(별개 호출이라 순서가 보장되지 않는다)
//!   tombstone 이 기억했다가 등록 때 이미 취소된 토큰을 준다.
//!
//! 이 모듈은 `Sidecar`·`AppHandle` 에 기대지 않는다 — 통합 테스트(`tests/channels.rs`)가 그대로 돌릴 수 있다.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use crate::api_host::{self, JiraHost};
use crate::git::commit::{commit_detail, estimate_commit_bytes, HexHash};
use crate::git::exec::queue::{ProcessQueue, LOCAL_LIMIT, WSL_LIMIT};
use crate::git::exec::{ExecError, GitExecutor, LocalExecutor, WslExecutor};
use crate::git::fetch::service::{FetchControl, FetchPrefs};
use crate::git::file_channels::{
    compare, estimate_compare_bytes, estimate_diff_bytes, file_blame, file_diff, file_history, parse_compare, BlameRequest, DiffRequest, HistoryRequest,
};
use crate::git::log_service::{parse_query, LogService, NoteSink};
use crate::git::lru::LruCache;
use crate::mirror::{RepoMirror, Resolved, Target};

/// Rust 가 처리하는 채널. 전부 `src/shared/ipc.ts` 의 `IPC_CHANNELS` 에 있어야 한다
/// (`tests/channels.rs` 가 `tests/golden/exec/channels.json` 으로 강제한다)
pub const RUST_CHANNELS: &[&str] = &["git:commit", "git:log", "git:compare", "git:fileDiff", "git:fileHistory", "git:blame", "git:fetch"];

/// Rust 가 처리하는 `jira:*` 채널(`api_host`). `RUST_CHANNELS` 와 별개다 — 미러·저장소와 무관하고 `ANTLER_RUST_API` 스위치만 본다.
/// 전부 `src/shared/ipc.ts` 의 `IPC_CHANNELS` 에 있어야 하고 `IPC_CHANNELS` 의 `jira:` 채널과 정확히 같아야 한다(`tests/api_route.rs` 가 강제한다)
pub const API_CHANNELS: &[&str] = &[
    "jira:auth",
    "jira:connect",
    "jira:disconnect",
    "jira:issue",
    "jira:transitions",
    "jira:transition",
    "jira:comments",
    "jira:search",
    "jira:dev",
    "jira:worklogs",
    "jira:userSearch",
    "jira:avatar",
    "jira:openExternal",
];

/// Rust 가 fetch 를 맡는 소스 종류(사이드카 env `ANTLER_RUST_FETCH_KINDS` 의 값. `src/sidecar/fetchOwns.ts` 가 읽는다)
pub const RUST_FETCH_KINDS: &str = "local,wsl";

/// 사이드카를 띄울 때 넣을 `ANTLER_RUST_FETCH_KINDS` 값. 라우터가 꺼져 있으면(`ANTLER_RUST_GIT=0`) 없다 — 사이드카가 fetch 를 전부 맡는다
pub fn sidecar_fetch_kinds(router_enabled: bool) -> Option<&'static str> {
    router_enabled.then_some(RUST_FETCH_KINDS)
}

/// `ANTLER_RUST_API`(`0` 이면 끔)와 `ANTLER_RUST_GIT`(`0` 이면 함께 끔)으로 [`API_CHANNELS`] 를 Rust 가 처리할지 정한다.
/// 이 스위치는 미러·저장소와 무관하다 — 켜져 있으면(서비스가 만들어졌으면) 항상 Rust 가 처리한다
pub fn rust_api_enabled(api_var: Option<&str>, git_var: Option<&str>) -> bool {
    api_var != Some("0") && rust_git_enabled(git_var)
}

/// 주어진 tokio 런타임 안에서 `f` 를 부른다. 서비스의 예약·fetch 시작은 태스크를 만들어서 런타임 밖 스레드(사이드카 읽기 스레드·창 이벤트)에서는 패닉한다 —
/// 그 스레드들은 이 함수로 앱 런타임에 들어간 뒤 라우터를 부른다
pub fn in_runtime_of<T>(handle: &tokio::runtime::Handle, f: impl FnOnce() -> T) -> T {
    let _guard = handle.enter();
    f()
}

/// bridge 의 `encodeArgs` 가 인자 배열의 빈 자리(undefined)에 넣는 표식(`src/sidecar/protocol.ts` 의 `UNDEFINED_ARG`)
pub const UNDEFINED_ARG: &str = "__antlerUndefinedArg";

/// 커밋 상세 캐시(`RepoService.ts` 의 COMMIT_CACHE_ENTRIES·COMMIT_CACHE_BYTES)
const COMMIT_CACHE_ENTRIES: usize = 300;
const COMMIT_CACHE_BYTES: u64 = 24 * 1024 * 1024;
/// 파일 diff 캐시(`DIFF_CACHE_ENTRIES`·`DIFF_CACHE_BYTES`). 비교 결과 캐시는 커밋 상세와 같은 크기다
const DIFF_CACHE_ENTRIES: usize = 500;
const DIFF_CACHE_BYTES: u64 = 32 * 1024 * 1024;

/// 등록보다 먼저 도착한 cancel 을 기억하는 시간·개수
const TOMBSTONE_TTL: Duration = Duration::from_secs(10);
const TOMBSTONE_MAX: usize = 256;

/// 검증을 마친 파일 단위 채널 요청
enum FileJob {
    Compare(HexHash, HexHash),
    Diff(DiffRequest),
    History(HistoryRequest),
    Blame(BlameRequest),
}

fn to_value<T: serde::Serialize>(v: &T) -> Result<Value, ExecError> {
    serde_json::to_value(v).map_err(|e| ExecError::Other { message: e.to_string() })
}

pub enum Routed {
    /// Rust 가 처리했다 — IpcEnvelope(`{ok:true,data}` / `{ok:false,error,code}`)
    Done(Value),
    /// 사이드카로 보낸다(인자는 호출자가 가진 원본 그대로)
    Sidecar,
}

/// `ANTLER_RUST_GIT` 값으로 Rust 경로를 켤지 정한다. `0` 만 끈다
pub fn rust_git_enabled(var: Option<&str>) -> bool {
    var != Some("0")
}

fn ok_env(data: Value) -> Value {
    json!({ "ok": true, "data": data })
}

fn err_env(error: &str, code: &str) -> Value {
    json!({ "ok": false, "error": error, "code": code })
}

/// 인자 배열의 표식을 None(undefined)으로 되돌린다. 사이드카 `decodeArgs` 와 같다 — 키가 정확히 그것 하나이고 값이 `true` 인 객체만
pub fn decode_args(raw: &[Value]) -> Vec<Option<&Value>> {
    raw.iter()
        .map(|a| match a.as_object() {
            Some(o) if o.len() == 1 && o.get(UNDEFINED_ARG) == Some(&Value::Bool(true)) => None,
            _ => Some(a),
        })
        .collect()
}

// ── 취소 레지스트리 ────────────────────────────────────────

#[derive(Default)]
struct RegInner {
    live: HashMap<String, (u64, CancellationToken)>,
    tombs: VecDeque<(String, Instant)>,
    serial: u64,
}

/// `requestId → CancellationToken`. 같은 id 를 다시 등록하면 새 것이 이긴다(TS `withSignal` 의 `requests.set` 과 같다)
#[derive(Default)]
pub struct CancelRegistry {
    inner: Mutex<RegInner>,
}

/// 등록한 요청이 끝나면(버려지면) 레지스트리에서 자기 항목을 지운다
pub struct Registration {
    registry: Arc<CancelRegistry>,
    id: String,
    serial: u64,
    pub token: CancellationToken,
}

impl Drop for Registration {
    fn drop(&mut self) {
        let mut inner = self.registry.inner.lock().unwrap_or_else(PoisonError::into_inner);
        if inner.live.get(&self.id).is_some_and(|(s, _)| *s == self.serial) {
            inner.live.remove(&self.id);
        }
    }
}

impl RegInner {
    fn prune(&mut self, now: Instant) {
        while self.tombs.front().is_some_and(|(_, at)| now.saturating_duration_since(*at) > TOMBSTONE_TTL) {
            self.tombs.pop_front();
        }
    }
}

/// renderer 의 requestId(`newRequestId`: `prefix-<base36>-<base36>`, 수십 바이트) 상한.
/// 넘거나 비면 requestId 없음으로 취급한다 — tombstone 이 renderer 가 정한 큰 문자열을 붙잡아 두지 않게(보안 리뷰 L1)
const MAX_REQUEST_ID: usize = 128;

pub(crate) fn is_request_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= MAX_REQUEST_ID
}

impl CancelRegistry {
    /// 핸들러가 **첫 await 전에** 부른다. 이미 cancel 이 도착해 있었으면 취소된 토큰을 돌려준다
    pub fn register(self: &Arc<Self>, id: &str) -> Registration {
        self.register_at(id, Instant::now())
    }

    pub fn cancel(&self, id: &str) {
        self.cancel_at(id, Instant::now());
    }

    fn register_at(self: &Arc<Self>, id: &str, now: Instant) -> Registration {
        let token = CancellationToken::new();
        let mut inner = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        inner.prune(now);
        if let Some(pos) = inner.tombs.iter().position(|(t, _)| t == id) {
            inner.tombs.remove(pos);
            token.cancel();
        }
        inner.serial += 1;
        let serial = inner.serial;
        inner.live.insert(id.to_string(), (serial, token.clone()));
        Registration { registry: self.clone(), id: id.to_string(), serial, token }
    }

    fn cancel_at(&self, id: &str, now: Instant) {
        let mut inner = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        inner.prune(now);
        if let Some((_, token)) = inner.live.get(id) {
            token.cancel();
            return;
        }
        // 아직 등록 전이다 — 곧 도착할 요청을 위해 기억한다(같은 id 가 이미 있으면 새로 적지 않는다)
        if inner.tombs.iter().any(|(t, _)| t == id) {
            return;
        }
        inner.tombs.push_back((id.to_string(), now));
        while inner.tombs.len() > TOMBSTONE_MAX {
            inner.tombs.pop_front();
        }
    }

    #[cfg(test)]
    fn counts(&self) -> (usize, usize) {
        let inner = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        (inner.live.len(), inner.tombs.len())
    }
}

// ── 라우터 ────────────────────────────────────────────

/// 소스(local 하나, wsl 은 배포판마다 하나)마다 실행기와 동시성 큐를 하나씩 둔다(TS `RepoService.runtime` 과 같다).
/// 큐의 결과 타입은 직렬화된 JSON 이라 이후 채널들이 같은 큐(같은 6/4 상한)를 나눠 쓸 수 있다
struct SourceRuntime {
    exec: Arc<GitExecutor>,
    queue: ProcessQueue<Arc<Value>>,
}

pub struct Router {
    enabled: bool,
    log: bool,
    mirror: RepoMirror,
    cancels: Arc<CancelRegistry>,
    sources: Mutex<HashMap<String, Arc<SourceRuntime>>>,
    commit_cache: Arc<Mutex<LruCache<Arc<Value>>>>,
    compare_cache: Arc<Mutex<LruCache<Arc<Value>>>>,
    diff_cache: Arc<Mutex<LruCache<Arc<Value>>>>,
    logs: LogService,
    rust_handled: AtomicU64,
    /// fetch 서비스(앱이 켜져 있을 때만 주입한다). 없으면 fetch 채널은 오류다
    fetch: Mutex<Option<Arc<dyn FetchControl>>>,
    /// 서비스가 주입되기 전에 도착한 fetchPrefs(마지막 것만). 주입될 때 적용한다
    pending_prefs: Mutex<Option<FetchPrefs>>,
    /// `ANTLER_RUST_API` 스위치(`ANTLER_RUST_GIT=0` 이어도 꺼진다). 꺼져 있으면 `jira:*` 는 서비스 없이 사이드카로 간다
    api_enabled: bool,
    /// Jira 서비스(앱이 스위치가 켜져 있을 때만 주입한다). 없으면 `jira:*` 는 사이드카가 처리한다
    jira: Mutex<Option<JiraHost>>,
}

impl Router {
    pub fn new(enabled: bool, log: bool, mirror: RepoMirror) -> Self {
        Router {
            enabled,
            api_enabled: enabled,
            jira: Mutex::default(),
            log,
            mirror: mirror.clone(),
            cancels: Arc::default(),
            sources: Mutex::default(),
            commit_cache: Arc::new(Mutex::new(LruCache::new(COMMIT_CACHE_ENTRIES, COMMIT_CACHE_BYTES))),
            compare_cache: Arc::new(Mutex::new(LruCache::new(COMMIT_CACHE_ENTRIES, COMMIT_CACHE_BYTES))),
            diff_cache: Arc::new(Mutex::new(LruCache::new(DIFF_CACHE_ENTRIES, DIFF_CACHE_BYTES))),
            logs: LogService::new(mirror),
            rust_handled: AtomicU64::new(0),
            fetch: Mutex::default(),
            pending_prefs: Mutex::default(),
        }
    }

    /// 스위치가 켜져 있는가(`ANTLER_RUST_GIT` 이 0 이 아니다)
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// `jira:*` 스위치를 정한다(만든 직후에만). 기본은 git 스위치와 같다
    pub fn with_api(mut self, enabled: bool) -> Self {
        self.api_enabled = enabled;
        self
    }

    /// `ANTLER_RUST_API` 스위치가 켜져 있는가(서비스가 만들어졌는지와는 별개 — 만들기 전에 이 값으로 정한다)
    pub fn api_enabled(&self) -> bool {
        self.api_enabled
    }

    /// Rust 가 `jira:*` 를 처리하는가: 스위치가 켜져 있고 서비스가 주입됐다. 사이드카를 `ANTLER_RUST_API=1` 로 띄울지는 이 값이 정한다
    pub fn api_active(&self) -> bool {
        self.api_enabled && self.jira_port().is_some()
    }

    fn jira_port(&self) -> Option<JiraHost> {
        self.jira.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// Jira 서비스를 주입한다. 스위치가 꺼져 있으면 주입하지 않는다(서비스·키체인이 없어야 한다)
    pub fn set_jira(&self, host: JiraHost) {
        if self.api_enabled {
            *self.jira.lock().unwrap_or_else(PoisonError::into_inner) = Some(host);
        }
    }

    fn fetch_port(&self) -> Option<Arc<dyn FetchControl>> {
        self.fetch.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// fetch 서비스를 주입한다. 앞서 도착해 있던 fetchPrefs 가 있으면 적용한다. 스위치가 꺼져 있으면 주입하지 않는다(이중 fetch 방지)
    pub fn set_fetch(&self, service: Arc<dyn FetchControl>) {
        if !self.enabled {
            return;
        }
        *self.fetch.lock().unwrap_or_else(PoisonError::into_inner) = Some(service.clone());
        if let Some(prefs) = self.pending_prefs.lock().unwrap_or_else(PoisonError::into_inner).take() {
            service.apply_prefs(prefs);
        }
    }

    /// 사이드카의 fetchPrefs 를 서비스에 적용한다(서비스가 아직 없으면 마지막 것을 보관한다). 런타임 안에서 불러야 한다([`in_runtime_of`])
    pub fn apply_fetch_prefs(&self, prefs: FetchPrefs) {
        if !self.enabled {
            return;
        }
        match self.fetch_port() {
            Some(service) => {
                service.apply_prefs(prefs);
            }
            None => *self.pending_prefs.lock().unwrap_or_else(PoisonError::into_inner) = Some(prefs),
        }
    }

    /// 창이 포커스를 얻었다 — 활성 저장소의 자동 fetch 가 밀렸으면 바로 한다. 런타임 안에서 불러야 한다
    pub fn fetch_on_focus(&self) {
        if let Some(service) = self.fetch_port() {
            service.on_focus();
        }
    }

    /// 환경변수로 켜고 끈다. `ANTLER_RUST_GIT=0` 이면 모든 채널을 사이드카로 보낸다.
    /// `ANTLER_ROUTE_LOG=1` 이면 [`RUST_CHANNELS`] 채널이 어디로 갔는지 stderr 에 한 줄씩 남긴다(확인용 — 기본은 조용하다)
    pub fn from_env(mirror: RepoMirror) -> Self {
        let enabled = rust_git_enabled(std::env::var("ANTLER_RUST_GIT").ok().as_deref());
        let api = rust_api_enabled(std::env::var("ANTLER_RUST_API").ok().as_deref(), std::env::var("ANTLER_RUST_GIT").ok().as_deref());
        let log = std::env::var("ANTLER_ROUTE_LOG").as_deref() == Ok("1");
        Router::new(enabled, log, mirror).with_api(api)
    }

    /// 로그 커서의 유휴 시간·훑는 주기를 바꾼다(테스트용 — 프로세스 목록을 읽는 테스트가 느려도 커서가 먼저 닫히지 않게). 만든 직후에만 부른다
    pub fn with_log_timing(mut self, idle: Duration, sweep: Duration) -> Self {
        self.logs = LogService::with_timing(self.mirror.clone(), idle, sweep);
        self
    }

    /// `git:log` 가 사이드카에 알릴 일(첫 페이지가 나갔다·이력이 바뀌었다)을 받는 곳. 앱은 사이드카에 `note` 메시지로 보낸다
    pub fn set_note_sink(&self, sink: NoteSink) {
        self.logs.set_sink(sink);
    }

    /// 미러가 바뀐 직후 불린다 — 사라졌거나 대상이 바뀐 저장소의 로그 커서를 닫는다
    pub fn on_mirror_changed(&self) {
        self.logs.sync_mirror();
        // 사라졌거나 Rust 가 맡지 않게 된 저장소의 fetch 상태를 비운다
        if let Some(service) = self.fetch_port() {
            service.sync_mirror();
        }
    }

    /// 앱 종료: 열려 있는 로그 커서(git 프로세스)를 모두 닫는다
    pub fn shutdown(&self) {
        self.logs.close_all();
        // 진행 중인 fetch 를 끊는다(git 프로세스가 끝나기를 기다리지는 않는다 — 종료 때 Job 이 정리한다)
        if let Some(service) = self.fetch_port() {
            service.dispose();
        }
    }

    /// 열려 있는 로그 커서 수(테스트·확인용)
    pub fn log_cursor_count(&self) -> usize {
        self.logs.cursor_count()
    }

    /// 열려 있는 로그 커서의 git PID(테스트·확인용)
    pub fn log_cursor_pids(&self) -> Vec<u32> {
        self.logs.cursor_pids()
    }

    /// 지금까지 Rust 가 처리한 호출 수(테스트·확인용)
    pub fn rust_handled(&self) -> u64 {
        self.rust_handled.load(Ordering::Relaxed)
    }

    /// 채널 하나를 라우팅한다. `args` 는 renderer 가 보낸 인자 배열 그대로다(표식 포함)
    pub async fn route(&self, channel: &str, args: &Value) -> Routed {
        let Some(raw) = args.as_array() else { return Routed::Sidecar };
        if channel == "req:cancel" {
            // Rust 레지스트리를 먼저 취소하고, 요청이 어디서 돌든(사이드카 포함) 항상 사이드카에도 전달한다
            if let Some(Value::String(id)) = raw.first() {
                if is_request_id(id) {
                    self.cancels.cancel(id);
                }
                // Jira 서비스도 자기 요청 표(TS `JiraService.cancel` 과 같다)에서 같은 id 를 찾아 끊는다. 없는 id 는 무시한다
                // (레지스트리의 `is_request_id` 상한과 무관하다 — 서비스는 상한 없이 등록하므로 TS 와 같게 빈 id 가 아니면 부른다)
                if let Some(host) = self.jira_port() {
                    host.service.cancel(id);
                }
            }
            return Routed::Sidecar;
        }
        // jira:* — 스위치가 켜져 있고 서비스가 있으면 미러와 무관하게 Rust 가 처리한다. 아니면 사이드카가 지금까지처럼
        if API_CHANNELS.contains(&channel) {
            return self.jira_channel(channel, raw).await;
        }
        if channel == "repos:watch" {
            // 사이드카로 넘기기 전에 다른 저장소(또는 null 이면 전부)의 로그 커서를 닫는다 — 멈춘 git log 가 pack 파일 핸들을 쥐지 않게
            let active = match raw.first() {
                Some(Value::String(id)) => Some(id.as_str()),
                _ => None,
            };
            self.logs.close_others(active);
            // 자동 fetch 대상. Rust 가 맡는 저장소(미러에 있고 local·wsl)만이고, 아니면 비운다(사이드카 쪽 저장소로 옮겨 갔을 때 이전 예약이 남지 않게).
            // TS 의 `setActive(repoId && config.repo(repoId) ? repoId : null)` 에 해당한다
            if self.enabled {
                if let Some(service) = self.fetch_port() {
                    service.set_active(active.filter(|id| matches!(self.mirror.resolve(id), Resolved::Rust(_))));
                }
            }
            return Routed::Sidecar;
        }
        if !RUST_CHANNELS.contains(&channel) {
            return Routed::Sidecar;
        }
        if !self.enabled {
            self.note(channel, "sidecar(ANTLER_RUST_GIT=0)");
            return Routed::Sidecar;
        }
        let args = decode_args(raw);
        match channel {
            "git:commit" => self.git_commit(&args).await,
            "git:log" => self.git_log(&args).await,
            "git:compare" | "git:fileDiff" | "git:fileHistory" | "git:blame" => self.git_file_channel(channel, &args).await,
            "git:fetch" => self.git_fetch(&args).await,
            _ => Routed::Sidecar,
        }
    }

    /// `jira:*` 한 채널. 서비스가 없으면(스위치 꺼짐·만들지 못함) 사이드카다
    async fn jira_channel(&self, channel: &str, raw: &[Value]) -> Routed {
        let Some(host) = self.jira_port().filter(|_| self.api_enabled) else {
            self.note(channel, "sidecar(ANTLER_RUST_API=0)");
            return Routed::Sidecar;
        };
        self.note(channel, "rust");
        self.rust_handled.fetch_add(1, Ordering::Relaxed);
        let args: Vec<Option<Value>> = decode_args(raw).into_iter().map(|a| a.cloned()).collect();
        Routed::Done(api_host::jira_envelope(host, self.cancels.clone(), channel.to_string(), args).await)
    }

    fn note(&self, channel: &str, to: &str) {
        if self.log {
            eprintln!("[route] {channel} -> {to}");
        }
    }

    /// `git:commit(repoId, hash, requestId?)`
    async fn git_commit(&self, args: &[Option<&Value>]) -> Routed {
        // 저장소 id 가 문자열이 아니면 사이드카가 TS 와 같은 오류를 만든다
        let Some(Some(Value::String(repo_id))) = args.first() else { return Routed::Sidecar };
        let target = match self.mirror.resolve(repo_id) {
            Resolved::Rust(t) => t,
            Resolved::Sidecar => {
                self.note("git:commit", "sidecar");
                return Routed::Sidecar;
            }
            Resolved::Invalid(message) => return Routed::Done(err_env(&message, "ERROR")),
        };
        // TS 의 withSignal 과 같게 빈 문자열은 requestId 없음이다. 첫 await 전에 등록한다
        let reg = match args.get(2) {
            Some(Some(Value::String(id))) if is_request_id(id) => Some(self.cancels.register(id)),
            _ => None,
        };
        let token = reg.as_ref().map_or_else(CancellationToken::new, |r| r.token.clone());
        let hash = match args.get(1) {
            Some(Some(Value::String(s))) => Some(s.as_str()),
            _ => None,
        };
        self.note("git:commit", "rust");
        self.rust_handled.fetch_add(1, Ordering::Relaxed);
        let result = self.commit(repo_id, &target, hash, &token).await;
        drop(reg);
        Routed::Done(match result {
            Ok(detail) => ok_env((*detail).clone()),
            Err(e) => err_env(&e.to_string(), e.envelope_code()),
        })
    }

    /// `git:log(repoId, query, requestId?)`. query 는 renderer 가 보낸 값이라 숫자·scope 형식을 검증한 뒤에야 git 인자가 된다
    async fn git_log(&self, args: &[Option<&Value>]) -> Routed {
        let Some(Some(Value::String(repo_id))) = args.first() else { return Routed::Sidecar };
        let target = match self.mirror.resolve(repo_id) {
            Resolved::Rust(t) => t,
            Resolved::Sidecar => {
                self.note("git:log", "sidecar");
                return Routed::Sidecar;
            }
            Resolved::Invalid(message) => return Routed::Done(err_env(&message, "ERROR")),
        };
        // 검증 실패는 git 도 커서도 건드리지 않는다
        let query = match parse_query(args.get(1).copied().flatten()) {
            Ok(q) => q,
            Err(message) => return Routed::Done(err_env(&message, "ERROR")),
        };
        let reg = match args.get(2) {
            Some(Some(Value::String(id))) if is_request_id(id) => Some(self.cancels.register(id)),
            _ => None,
        };
        let token = reg.as_ref().map_or_else(CancellationToken::new, |r| r.token.clone());
        self.note("git:log", "rust");
        self.rust_handled.fetch_add(1, Ordering::Relaxed);
        let rt = self.runtime(&target);
        let result = self.logs.log(repo_id, &rt.exec, &target, &query, &token).await;
        drop(reg);
        Routed::Done(match result.and_then(|page| serde_json::to_value(&page).map_err(|e| crate::git::exec::ExecError::Other { message: e.to_string() }.into())) {
            Ok(page) => ok_env(page),
            Err(e) => err_env(&e.to_string(), e.envelope_code()),
        })
    }

    /// `git:fetch(repoId, manual)`. `manual` 은 정확히 `true` 일 때만 참이다(`handlers.ts` 의 `manual === true`) — renderer 가 주장하는 값이라 다른 모양은 모두 자동 fetch 다.
    /// 저장소의 경로·kind 는 미러에서만 얻고(서비스가 다시 미러로 푼다), 미러에 없거나 ssh·github 면 사이드카가 처리한다.
    /// local·wsl 이면 **절대 사이드카로 보내지 않는다** — 사이드카는 그 저장소를 맡지 않아(`ANTLER_RUST_FETCH_KINDS`) 보내면 오류가 나고, 보내서 둘이 fetch 하는 길도 없어야 한다
    async fn git_fetch(&self, args: &[Option<&Value>]) -> Routed {
        let Some(Some(Value::String(repo_id))) = args.first() else { return Routed::Sidecar };
        match self.mirror.resolve(repo_id) {
            Resolved::Rust(_) => {}
            Resolved::Sidecar => {
                self.note("git:fetch", "sidecar");
                return Routed::Sidecar;
            }
            Resolved::Invalid(message) => return Routed::Done(err_env(&message, "ERROR")),
        }
        let Some(service) = self.fetch_port() else {
            return Routed::Done(err_env("fetch 서비스가 준비되지 않았습니다", "ERROR"));
        };
        let manual = matches!(args.get(1), Some(Some(Value::Bool(true))));
        self.note("git:fetch", "rust");
        self.rust_handled.fetch_add(1, Ordering::Relaxed);
        let status = service.fetch(repo_id, manual).await;
        Routed::Done(match to_value(&status) {
            Ok(data) => ok_env(data),
            Err(e) => err_env(&e.to_string(), e.envelope_code()),
        })
    }

    /// 파일 단위 채널. 인자는 `git:compare(repoId, from, to, requestId?)`, `git:fileDiff|fileHistory|blame(repoId, req, requestId?)`.
    /// 검증이 먼저고(실패하면 git·레지스트리를 건드리지 않는다) 통과한 값만 `git::file_channels` 의 argv 빌더로 간다
    async fn git_file_channel(&self, channel: &str, args: &[Option<&Value>]) -> Routed {
        let Some(Some(Value::String(repo_id))) = args.first() else { return Routed::Sidecar };
        let target = match self.mirror.resolve(repo_id) {
            Resolved::Rust(t) => t,
            Resolved::Sidecar => {
                self.note(channel, "sidecar");
                return Routed::Sidecar;
            }
            Resolved::Invalid(message) => return Routed::Done(err_env(&message, "ERROR")),
        };
        let arg = |i: usize| args.get(i).copied().flatten();
        let job = match channel {
            "git:compare" => parse_compare(arg(1), arg(2)).map(|(from, to)| FileJob::Compare(from, to)),
            "git:fileDiff" => DiffRequest::parse(arg(1)).map(FileJob::Diff),
            "git:fileHistory" => HistoryRequest::parse(arg(1)).map(FileJob::History),
            _ => BlameRequest::parse(arg(1)).map(FileJob::Blame),
        };
        let job = match job {
            Ok(j) => j,
            Err(e) => return Routed::Done(err_env(&e.to_string(), e.envelope_code())),
        };
        let request_id = if channel == "git:compare" { 3 } else { 2 };
        let reg = match args.get(request_id) {
            Some(Some(Value::String(id))) if is_request_id(id) => Some(self.cancels.register(id)),
            _ => None,
        };
        let token = reg.as_ref().map_or_else(CancellationToken::new, |r| r.token.clone());
        self.note(channel, "rust");
        self.rust_handled.fetch_add(1, Ordering::Relaxed);
        let result = self.file_job(repo_id, &target, job, &token).await;
        drop(reg);
        Routed::Done(match result {
            // 캐시에 들어가지 않은(상한 초과) 큰 결과는 참조가 하나뿐이다 — 복사하지 않고 꺼낸다
            Ok(data) => ok_env(Arc::try_unwrap(data).unwrap_or_else(|a| (*a).clone())),
            Err(e) => err_env(&e.to_string(), e.envelope_code()),
        })
    }

    async fn file_job(&self, repo_id: &str, target: &Target, job: FileJob, token: &CancellationToken) -> Result<Arc<Value>, ExecError> {
        let rt = self.runtime(target);
        let (exec, path) = (rt.exec.clone(), target.path().to_string());
        match job {
            // 해시 쌍 기준이라 불변: 캐시 + 같은 키 요청 합치기
            FileJob::Compare(from, to) => {
                let key = format!("compare\0{repo_id}\0{}\0{}", from.lower(), to.lower());
                self.cached_shared(&rt, &self.compare_cache, key, token, move || async move {
                    let result = compare(&exec, &path, &from, &to).await?;
                    let cost = estimate_compare_bytes(&result);
                    Ok((to_value(&result)?, cost))
                })
                .await
            }
            // part/full 이 서로 다른 항목이다. 잘렸는지와 상관없이 캐시한다(TS 와 같다)
            FileJob::Diff(req) => {
                let key = req.key(repo_id);
                self.cached_shared(&rt, &self.diff_cache, key, token, move || async move {
                    let diff = file_diff(&exec, &path, &req).await?;
                    let cost = estimate_diff_bytes(&diff);
                    Ok((to_value(&diff)?, cost))
                })
                .await
            }
            // HEAD 가 움직일 수 있어 캐시하지 않고 합치지도 않는다. 취소는 이 요청의 프로세스에 바로 닿는다
            FileJob::History(req) => {
                let t = token.clone();
                rt.queue.run(None, Some(token.clone()), move || async move { file_history(&exec, &path, &req, &t).await.map(Arc::new) }).await
            }
            FileJob::Blame(req) => {
                let t = token.clone();
                rt.queue.run(None, Some(token.clone()), move || async move { file_blame(&exec, &path, &req, &t).await.map(Arc::new) }).await
            }
        }
    }

    /// 불변 결과의 캐시 → 같은 키 요청 합치기 → 큐 슬롯 안에서 `task`. `commit` 과 같은 방식이다:
    /// 요청 토큰은 실행기에 넘기지 않고(합쳐진 다른 호출자의 프로세스까지 죽이게 된다) 이 호출자는 기다리기만 그만둔다
    async fn cached_shared<F, Fut>(
        &self,
        rt: &SourceRuntime,
        cache: &Arc<Mutex<LruCache<Arc<Value>>>>,
        key: String,
        token: &CancellationToken,
        task: F,
    ) -> Result<Arc<Value>, ExecError>
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: std::future::Future<Output = Result<(Value, u64), ExecError>> + Send + 'static,
    {
        if let Some(hit) = cache.lock().unwrap_or_else(PoisonError::into_inner).get(&key) {
            return Ok(hit);
        }
        let (cache, cache_key) = (cache.clone(), key.clone());
        let run = rt.queue.run(Some(&key), None, move || async move {
            let (value, cost) = task().await?;
            let value = Arc::new(value);
            cache.lock().unwrap_or_else(PoisonError::into_inner).set(&cache_key, value.clone(), cost);
            Ok(value)
        });
        tokio::select! {
            biased;
            () = token.cancelled() => Err(ExecError::Aborted),
            r = run => r,
        }
    }

    fn runtime(&self, target: &Target) -> Arc<SourceRuntime> {
        let mut sources = self.sources.lock().unwrap_or_else(PoisonError::into_inner);
        sources
            .entry(target.source_key())
            .or_insert_with(|| {
                Arc::new(match target {
                    Target::Local { .. } => SourceRuntime { exec: Arc::new(GitExecutor::Local(LocalExecutor::default())), queue: ProcessQueue::new(LOCAL_LIMIT) },
                    Target::Wsl { distro, .. } => SourceRuntime { exec: Arc::new(GitExecutor::Wsl(WslExecutor::new(distro.clone()))), queue: ProcessQueue::new(WSL_LIMIT) },
                })
            })
            .clone()
    }

    /// 커밋 상세. TS `RepoService.commit` 과 같은 순서: 해시 기준 불변 캐시 → 같은 키 요청 합치기 → 큐 슬롯 안에서 git 세 번.
    /// 합친 요청의 취소는 기다리는 쪽이 모두 취소했을 때만 프로세스에 닿는다(기다리는 쪽이 모두 사라지면 큐가 프로세스를 정리한다)
    async fn commit(&self, repo_id: &str, target: &Target, hash: Option<&str>, token: &CancellationToken) -> Result<Arc<Value>, ExecError> {
        let hash = HexHash::parse(hash.unwrap_or(""))?;
        let key = format!("{repo_id}\0{}", hash.lower());
        if let Some(hit) = self.commit_cache.lock().unwrap_or_else(PoisonError::into_inner).get(&key) {
            return Ok(hit);
        }
        let rt = self.runtime(target);
        let (exec, path, cache, cache_key) = (rt.exec.clone(), target.path().to_string(), self.commit_cache.clone(), key.clone());
        let run = rt.queue.run(Some(&key), None, move || async move {
            let detail = commit_detail(&exec, &path, &hash).await?;
            let cost = estimate_commit_bytes(&detail);
            let value = Arc::new(serde_json::to_value(&detail).map_err(|e| ExecError::Other { message: e.to_string() })?);
            cache.lock().unwrap_or_else(PoisonError::into_inner).set(&cache_key, value.clone(), cost);
            Ok(value)
        });
        // 요청 토큰은 실행기에 넘기지 않는다 — 합쳐진 다른 호출자의 프로세스까지 죽이게 된다. 이 호출자는 기다리기만 그만둔다
        tokio::select! {
            biased;
            () = token.cancelled() => Err(ExecError::Aborted),
            r = run => r,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mirror::MirrorRepo;

    fn mirror_of(repos: Vec<MirrorRepo>) -> RepoMirror {
        let m = RepoMirror::default();
        m.apply(1, repos);
        m
    }

    fn repo(id: &str, kind: &str, path: &str) -> MirrorRepo {
        MirrorRepo { id: id.into(), kind: kind.into(), path: path.into(), distro: None }
    }

    fn is_sidecar(r: &Routed) -> bool {
        matches!(r, Routed::Sidecar)
    }

    fn envelope(r: Routed) -> Value {
        match r {
            Routed::Done(v) => v,
            Routed::Sidecar => panic!("사이드카로 갔다"),
        }
    }

    #[test]
    fn 스위치는_0_만_끈다() {
        assert!(rust_git_enabled(None));
        assert!(rust_git_enabled(Some("1")));
        assert!(rust_git_enabled(Some("")));
        assert!(rust_git_enabled(Some("false")));
        assert!(!rust_git_enabled(Some("0")));
    }

    #[test]
    fn 표식_객체만_undefined_로_되돌린다() {
        let raw = [
            json!("a"),
            json!({ UNDEFINED_ARG: true }),
            json!({ UNDEFINED_ARG: true, "x": 1 }),
            json!({ UNDEFINED_ARG: false }),
            json!(null),
            json!([{ UNDEFINED_ARG: true }]),
        ];
        let d = decode_args(&raw);
        assert_eq!(d[0], Some(&json!("a")));
        assert_eq!(d[1], None);
        assert_eq!(d[2], Some(&raw[2]));
        assert_eq!(d[3], Some(&raw[3]));
        assert_eq!(d[4], Some(&Value::Null));
        assert_eq!(d[5], Some(&raw[5]));
    }

    #[tokio::test]
    async fn 사이드카로_가는_경우() {
        let m = mirror_of(vec![repo("a", "local", "/nonexistent-antler"), repo("s", "ssh", "/srv"), repo("g", "github", "/cache")]);
        let r = Router::new(true, false, m.clone());
        // 채널이 등록되지 않았다
        assert!(is_sidecar(&r.route("git:refs", &json!(["a"])).await));
        // 미러에 없는 저장소·ssh·github
        for id in ["nope", "s", "g"] {
            assert!(is_sidecar(&r.route("git:commit", &json!([id, "abcd"])).await), "{id}");
        }
        // 저장소 id 가 문자열이 아니거나 없다
        for args in [json!([]), json!([null]), json!([5, "abcd"]), json!([{ UNDEFINED_ARG: true }, "abcd"]), json!({ "a": 1 }), json!("x")] {
            assert!(is_sidecar(&r.route("git:commit", &args).await), "{args}");
        }
        // 스위치를 끄면 미러에 있어도 사이드카
        let off = Router::new(false, false, m);
        assert!(is_sidecar(&off.route("git:commit", &json!(["a", "abcd"])).await));
        assert_eq!(off.rust_handled() + r.rust_handled(), 0);
    }

    #[tokio::test]
    async fn 미러의_경로가_검사에_걸리면_error_로_돌려준다() {
        let m = mirror_of(vec![repo("unc", "local", "\\\\evil\\share\\repo")]);
        let r = Router::new(true, false, m);
        let env = envelope(r.route("git:commit", &json!(["unc", "abcd1234"])).await);
        assert_eq!(env["ok"], false);
        assert_eq!(env["code"], "ERROR");
    }

    #[tokio::test]
    async fn 해시가_잘못되면_git_을_띄우기_전에_error() {
        // 저장소 경로가 없는 곳이다 — 검증이 먼저 걸러야 한다(git 을 띄웠다면 GIT 이나 다른 오류가 난다)
        let m = mirror_of(vec![repo("a", "local", "/nonexistent-antler")]);
        let r = Router::new(true, false, m);
        let hostile: Vec<Value> = vec![
            json!("-x"),
            json!("--output=pwned.txt"),
            json!("-calias.x=!echo"),
            json!("zzzz"),
            json!("abc"),
            json!("abcd\n"),
            json!(" abcd"),
            json!(123),
            json!(null),
            json!({ "x": 1 }),
            json!({ UNDEFINED_ARG: true }),
        ];
        for h in hostile {
            let env = envelope(r.route("git:commit", &json!(["a", h, "req-1"])).await);
            assert_eq!(env["ok"], false, "{h}");
            assert_eq!(env["code"], "ERROR", "{h}");
            assert_eq!(env["error"], "invalid commit hash", "{h}");
        }
        // 해시를 아예 안 보냈다
        assert_eq!(envelope(r.route("git:commit", &json!(["a"])).await)["code"], "ERROR");
    }

    #[tokio::test]
    async fn req_cancel_은_항상_사이드카에도_간다() {
        let r = Router::new(true, false, RepoMirror::default());
        assert!(is_sidecar(&r.route("req:cancel", &json!(["x"])).await));
        assert!(is_sidecar(&r.route("req:cancel", &json!([])).await));
        assert!(is_sidecar(&r.route("req:cancel", &json!([5])).await));
        // 스위치가 꺼져 있어도 마찬가지
        let off = Router::new(false, false, RepoMirror::default());
        assert!(is_sidecar(&off.route("req:cancel", &json!(["x"])).await));
    }

    #[tokio::test]
    async fn 등록_전에_온_cancel_은_등록한_요청을_취소한다() {
        let m = mirror_of(vec![repo("a", "local", "/nonexistent-antler")]);
        let r = Router::new(true, false, m);
        assert!(is_sidecar(&r.route("req:cancel", &json!(["early"])).await));
        // git 을 띄우지 않고 ABORTED — 경로가 없어서 띄웠다면 GIT/ERROR 가 난다
        let env = envelope(r.route("git:commit", &json!(["a", "abcd1234", "early"])).await);
        assert_eq!(env["code"], "ABORTED");
        // 쓴 tombstone 은 사라지고 등록도 끝나며 지워진다(이 테스트는 git 을 띄우지 않는다)
        assert_eq!(r.cancels.counts(), (0, 0));
    }

    #[tokio::test]
    async fn 등록_전에_온_cancel_은_로그_요청도_git_없이_취소하고_첫_페이지를_알린다() {
        let m = mirror_of(vec![repo("a", "local", "/nonexistent-antler")]);
        let r = Router::new(true, false, m);
        let notes: Arc<Mutex<Vec<(String, String)>>> = Arc::default();
        let sink = notes.clone();
        r.set_note_sink(Arc::new(move |op, id| sink.lock().unwrap().push((op.as_str().to_string(), id.to_string()))));
        assert!(is_sidecar(&r.route("req:cancel", &json!(["early"])).await));
        // skip 0 — 취소돼도 백그라운드 조회를 풀어 준다(TS 는 실패해도 openFirstPage). git·커서는 만들지 않는다
        let env = envelope(r.route("git:log", &json!(["a", { "skip": 0, "count": 10 }, "early"])).await);
        assert_eq!(env["code"], "ABORTED");
        assert_eq!(r.log_cursor_count(), 0);
        assert_eq!(*notes.lock().unwrap(), [("logFirstPage".to_string(), "a".to_string())]);
        // skip > 0 이면 알리지 않는다
        assert!(is_sidecar(&r.route("req:cancel", &json!(["early2"])).await));
        let env = envelope(r.route("git:log", &json!(["a", { "skip": 5, "count": 10 }, "early2"])).await);
        assert_eq!(env["code"], "ABORTED");
        assert_eq!(notes.lock().unwrap().len(), 1);
        // 검증에서 거부되면 알리지 않는다(TS 는 검증 오류를 첫 페이지 처리 전에 던진다)
        let env = envelope(r.route("git:log", &json!(["a", { "skip": 0, "count": "10" }])).await);
        assert_eq!(env["error"], "invalid query");
        assert_eq!(notes.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn log_채널도_미러에_없거나_스위치가_꺼지면_사이드카() {
        let m = mirror_of(vec![repo("a", "local", "/nonexistent-antler"), repo("s", "ssh", "/srv")]);
        let r = Router::new(true, false, m.clone());
        for id in ["nope", "s"] {
            assert!(is_sidecar(&r.route("git:log", &json!([id, { "skip": 0, "count": 1 }])).await), "{id}");
        }
        for args in [json!([]), json!([null]), json!([5, { "skip": 0, "count": 1 }])] {
            assert!(is_sidecar(&r.route("git:log", &args).await), "{args}");
        }
        let off = Router::new(false, false, m);
        assert!(is_sidecar(&off.route("git:log", &json!(["a", { "skip": 0, "count": 1 }])).await));
        assert_eq!(r.rust_handled() + off.rust_handled(), 0);
    }

    #[test]
    fn 레지스트리는_등록한_것만_취소하고_끝나면_지운다() {
        let reg: Arc<CancelRegistry> = Arc::default();
        let a = reg.register("a");
        assert!(!a.token.is_cancelled());
        reg.cancel("a");
        assert!(a.token.is_cancelled());
        assert_eq!(reg.counts(), (1, 0));
        drop(a);
        assert_eq!(reg.counts(), (0, 0));
        // 같은 id 를 다시 등록하면 새 것이 이기고, 먼저 등록한 것의 Drop 이 새 것을 지우지 않는다
        let first = reg.register("x");
        let second = reg.register("x");
        drop(first);
        reg.cancel("x");
        assert!(second.token.is_cancelled());
    }

    #[test]
    fn tombstone_은_10초_뒤_사라지고_256개를_넘지_못한다() {
        let reg: Arc<CancelRegistry> = Arc::default();
        let t0 = Instant::now();
        reg.cancel_at("old", t0);
        // 11초 뒤 등록하면 이미 만료돼 취소되지 않는다
        let r = reg.register_at("old", t0 + Duration::from_secs(11));
        assert!(!r.token.is_cancelled());
        // 10초 안이면 취소된다
        reg.cancel_at("fresh", t0);
        let r = reg.register_at("fresh", t0 + Duration::from_secs(9));
        assert!(r.token.is_cancelled());
        // 개수 상한: 가장 오래된 것부터 버린다
        for i in 0..(TOMBSTONE_MAX + 5) {
            reg.cancel_at(&format!("id{i}"), t0);
        }
        assert_eq!(reg.counts().1, TOMBSTONE_MAX);
        assert!(!reg.register_at("id0", t0).token.is_cancelled());
        assert!(reg.register_at(&format!("id{}", TOMBSTONE_MAX + 4), t0).token.is_cancelled());
    }
}
