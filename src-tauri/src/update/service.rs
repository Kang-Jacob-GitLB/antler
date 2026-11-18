//! 앱 업데이트 상태 기계(`src/main/update/UpdateService.ts` 의 auto 갈래 이식, Phase 6c PR 4). 네트워크·서명·설치는 모른다 — [`Backend`] 가 하고, 이 모듈은 상태 순서·정책·입력 검증만 가진다
//! (TS 의 `AutoBackend` 와 같은 granularity: 확인 → 내려받기(진행·취소) → 설치). 그래서 `tests/fixtures/update.scenarios.json` 을 TS `UpdateService`(가짜 백엔드)와 이 서비스(가짜 백엔드)에 똑같이 먹여
//! **상태가 나간 순서**를 골든으로 비교할 수 있다(`tests/updateScenarios.rs`).
//!
//! TS 와 같은 점: `update:check` 는 항상 수동 확인(`check(true)`)이고 자동 확인(시작 10초 뒤·6시간마다)의 실패는 조용하다, 받아 둔 업데이트가 있으면 다시 확인하지 않는다,
//! 건너뛴 버전은 이 서비스만 쓴다(`Deps::set_skipped`), dismiss 는 지금 알리는 버전만 받는다.
//! 다른 점: ① 수동 모드(`manual-dmg`·`manual-file`)는 없다(Windows NSIS 만 — 모드는 항상 `auto`). ② 설치(`Backend::install`)는 프로세스를 끝내는 동기 호출이라 블로킹 스레드에서 돈다.
//! ③ 릴리스 노트의 동시 요청 합치기는 같은 키의 진행 중 future 를 공유한다(TS 는 같은 `Promise`).
//!
//! `Deps::send` 는 서비스 잠금 안에서 불린다 — 상태가 바뀐 순서대로 나가게 하려는 것이므로 **서비스를 다시 부르지 않는다**(이벤트를 내보내기만 한다).

use std::collections::HashMap;
use std::future::Future;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use futures_util::future::{BoxFuture, FutureExt, Shared};
use serde::Serialize;
use serde_json::{json, Value};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use super::logic::{is_offerable, parse_dismiss, parse_semver, release_notes_url, sanitize_error, strip_v, DismissAction, ReleaseNotes, UpdatePolicy};

/// 시작 뒤 첫 자동 확인까지(TS `FIRST_CHECK_MS`)와 이후 주기(`INTERVAL_MS`)
pub const FIRST_CHECK: Duration = Duration::from_secs(10);
pub const CHECK_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);
/// 릴리스 노트 재사용 시간(성공한 결과만)
const NOTES_TTL: Duration = Duration::from_secs(10 * 60);
/// 개발 장치가 받는 상태 이름(TS `FAKE`)
const FAKE_KINDS: [&str; 4] = ["available", "downloading", "downloaded", "error"];

const MODE: &str = "auto";
const RETRY_AFTER_CHECK: &str = "다시 확인한 뒤 시도하세요";
const NOT_NOTIFIED: &str = "현재 알림 중인 버전이 아닙니다";
const BAD_VERSION: &str = "잘못된 버전입니다";
const INSTALL_FAILED: &str = "설치를 시작하지 못했습니다";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Status {
    Idle,
    Checking,
    Available,
    Downloading,
    Downloaded,
    Error,
    NotAvailable,
}

/// `@shared/types` 의 `UpdateState`(`fileName` 은 수동 모드 전용이라 없다)
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateState {
    pub status: Status,
    pub mode: &'static str,
    pub current_version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// 확인·내려받기·설치를 실제로 하는 쪽(TS `AutoBackend` + 릴리스 노트). 오류 문구는 사용자에게 보여 줘도 되는 고정 문구여야 한다(요청 주소·경로가 섞이지 않게 — 서비스가 한 번 더 `sanitize_error` 를 건다)
pub trait Backend: Send + Sync + 'static {
    /// 새 버전(앞의 `v` 를 뗀 문자열), 없으면 `None`. 받아 둔 설치본이 있으면 버린다(새 확인이 이긴다)
    fn check(&self) -> impl Future<Output = Result<Option<String>, String>> + Send;
    /// `version`(마지막 확인이 돌려준 버전)의 설치본을 내려받아 **서명을 검증하고** 보관한다. 진행은 0~100(알 수 없으면 부르지 않는다). 취소하면 `Err` 로 끝낸다
    fn download(&self, version: &str, progress: &(dyn Fn(f64) + Sync), cancel: &CancellationToken) -> impl Future<Output = Result<(), String>> + Send;
    /// 보관한 설치본을 설치한다. 성공하면 프로세스가 끝난다(돌아오지 않는다)
    fn install(&self) -> Result<(), String>;
    /// `version`(앞의 `v` 를 뗀 값)의 릴리스 노트
    fn release_notes(&self, version: &str) -> impl Future<Output = Result<ReleaseNotes, String>> + Send;
}

/// 건너뛴 버전을 설정에 쓰는 포트(검증 실패는 `Err` — `dismiss` 가 그대로 돌려준다)
pub type SetSkipped = Arc<dyn Fn(&str) -> Result<(), String> + Send + Sync>;

pub struct Deps {
    pub current_version: String,
    /// 확인·내려받기를 하는가. 운영은 번들러가 실행 파일에 심은 표식이 NSIS 일 때만이다(`host::bundle_is_nsis`) — 아니면 확인 0회(수동 확인은 `not-available`)
    pub enabled: bool,
    /// 개발 장치(`ANTLER_UPDATE_FAKE`): 받는 상태 이름. 호스트가 개발·cdp 빌드에서만 채운다
    pub fake: Option<String>,
    /// 설정에 저장된 건너뛴 버전(호출 때마다 읽는다)
    pub skipped: Arc<dyn Fn() -> Option<String> + Send + Sync>,
    /// 건너뛴 버전 저장(설정 쓰기는 호스트의 이 한 곳)
    pub set_skipped: SetSkipped,
    /// 상태가 바뀔 때마다(잠금 안에서) 부른다
    pub send: Arc<dyn Fn(&UpdateState) + Send + Sync>,
}

type NotesFuture = Shared<BoxFuture<'static, Result<ReleaseNotes, String>>>;

struct St {
    state: UpdateState,
    policy: UpdatePolicy,
    busy: bool,
    /// 마지막 확인이 돌려준 새 버전(내려받기가 이 버전만 받는다)
    pending_version: Option<String>,
    /// 진행 중인 내려받기의 취소 신호
    cancel: Option<CancellationToken>,
    timers: Vec<tokio::task::JoinHandle<()>>,
    notes: HashMap<String, (Instant, NotesFuture)>,
}

struct Core<B: Backend> {
    backend: Arc<B>,
    d: Deps,
    enabled: bool,
    fake: Option<String>,
    st: Mutex<St>,
}

pub struct UpdateService<B: Backend>(Arc<Core<B>>);

impl<B: Backend> Clone for UpdateService<B> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

/// 확인·내려받기가 끝나거나 중단돼도(future 를 버려도) `busy` 가 남지 않게 한다(TS `finally`)
struct BusyGuard<'a, B: Backend>(&'a Core<B>);

impl<B: Backend> Drop for BusyGuard<'_, B> {
    fn drop(&mut self) {
        self.0.lock().busy = false;
    }
}

impl<B: Backend> Core<B> {
    fn lock(&self) -> MutexGuard<'_, St> {
        self.st.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn base(&self, status: Status) -> UpdateState {
        UpdateState { status, mode: MODE, current_version: self.d.current_version.clone(), version: None, notes_url: None, progress: None, error: None }
    }

    fn with_version(&self, status: Status, version: &str) -> UpdateState {
        UpdateState { version: Some(version.to_string()), notes_url: Some(release_notes_url(version)), ..self.base(status) }
    }

    fn set(&self, st: &mut St, s: UpdateState) -> UpdateState {
        st.state = s.clone();
        (self.d.send)(&s);
        s
    }

    fn fake_state(&self, kind: &str) -> UpdateState {
        let version = "0.2.0";
        match kind {
            "available" => self.with_version(Status::Available, version),
            "downloading" => UpdateState { progress: Some(42), ..self.with_version(Status::Downloading, version) },
            "downloaded" => self.with_version(Status::Downloaded, version),
            _ => UpdateState { error: Some("GitHub 요청 한도에 걸렸습니다. 잠시 후 다시 시도하세요".to_string()), ..self.with_version(Status::Error, version) },
        }
    }
}

impl<B: Backend> UpdateService<B> {
    pub fn new(backend: Arc<B>, d: Deps) -> Self {
        let enabled = d.enabled;
        let fake = d.fake.clone().filter(|f| FAKE_KINDS.contains(&f.as_str()));
        let core = Core {
            backend,
            enabled,
            fake,
            st: Mutex::new(St {
                state: UpdateState { status: Status::Idle, mode: MODE, current_version: d.current_version.clone(), version: None, notes_url: None, progress: None, error: None },
                policy: UpdatePolicy::new(),
                busy: false,
                pending_version: None,
                cancel: None,
                timers: Vec::new(),
                notes: HashMap::new(),
            }),
            d,
        };
        if let Some(kind) = &core.fake {
            let s = core.fake_state(kind);
            core.st.lock().unwrap_or_else(PoisonError::into_inner).state = s;
        }
        Self(Arc::new(core))
    }

    pub fn get(&self) -> UpdateState {
        self.0.lock().state.clone()
    }

    /// 시작 10초 뒤 첫 자동 확인, 이후 6시간마다. 사용할 수 없는 빌드·개발 장치에서는 아무것도 하지 않는다. `rt` 는 타이머가 도는 런타임이다
    pub fn start(&self, rt: &tokio::runtime::Handle) {
        let c = &self.0;
        if !c.enabled || c.fake.is_some() {
            return;
        }
        let (first, every) = (self.clone(), self.clone());
        let t1 = rt.spawn(async move {
            tokio::time::sleep(FIRST_CHECK).await;
            first.check(false).await;
        });
        let t2 = rt.spawn(async move {
            let mut tick = tokio::time::interval_at(Instant::now() + CHECK_INTERVAL, CHECK_INTERVAL);
            loop {
                tick.tick().await;
                every.check(false).await;
            }
        });
        c.lock().timers.extend([t1, t2]);
    }

    /// 타이머를 멈추고 진행 중인 내려받기를 취소한다
    pub fn dispose(&self) {
        let mut st = self.0.lock();
        for t in st.timers.drain(..) {
            t.abort();
        }
        if let Some(c) = &st.cancel {
            c.cancel();
        }
    }

    pub async fn check(&self, manual: bool) -> UpdateState {
        let c = &*self.0;
        if c.fake.is_some() {
            return c.lock().state.clone();
        }
        {
            let mut st = c.lock();
            if st.busy || st.state.status == Status::Downloading {
                return st.state.clone();
            }
            if !c.enabled {
                // 개발 빌드 등: 확인하지 않는다(수동 확인이면 최신으로 안내)
                return if manual { c.set(&mut st, c.base(Status::NotAvailable)) } else { st.state.clone() };
            }
            // 이미 받아 둔 업데이트가 있으면 다시 확인하지 않는다
            if st.state.status == Status::Downloaded {
                return st.state.clone();
            }
            st.busy = true;
            if manual {
                c.set(&mut st, c.base(Status::Checking));
            }
        }
        let _busy = BusyGuard(c);
        let result = c.backend.check().await;
        let mut st = c.lock();
        match result {
            Ok(version) => {
                st.pending_version.clone_from(&version);
                match version.filter(|v| is_offerable(&c.d.current_version, v)) {
                    None => {
                        st.pending_version = None;
                        c.set(&mut st, c.base(if manual { Status::NotAvailable } else { Status::Idle }))
                    }
                    Some(v) => {
                        let skipped = (c.d.skipped)();
                        if st.policy.should_notify(&v, manual, skipped.as_deref()) {
                            c.set(&mut st, c.with_version(Status::Available, &v))
                        } else {
                            c.set(&mut st, c.base(Status::Idle))
                        }
                    }
                }
            }
            // 자동 확인의 실패(오프라인 등)는 조용히 넘기고, 수동 확인만 알린다
            Err(e) => {
                let s = if manual { UpdateState { error: Some(sanitize_error(&e)), ..c.base(Status::Error) } } else { c.base(Status::Idle) };
                c.set(&mut st, s)
            }
        }
    }

    pub async fn download(&self) -> UpdateState {
        let c = &*self.0;
        if c.fake.is_some() {
            return c.lock().state.clone();
        }
        let (version, token, pending) = {
            let mut st = c.lock();
            let Some(version) = st.state.version.clone() else { return st.state.clone() };
            if st.busy || !matches!(st.state.status, Status::Available | Status::Error) {
                return st.state.clone();
            }
            st.busy = true;
            let token = CancellationToken::new();
            st.cancel = Some(token.clone());
            c.set(&mut st, UpdateState { progress: Some(0), ..c.with_version(Status::Downloading, &version) });
            (version, token, st.pending_version.clone())
        };
        let _busy = BusyGuard(c);
        let last = AtomicI64::new(-1);
        let progress = |p: f64| {
            let pct = if p.is_nan() { 0 } else { p.round().clamp(0.0, 100.0) as i64 };
            if pct == last.load(Ordering::SeqCst) {
                return;
            }
            let mut st = c.lock();
            if st.state.status != Status::Downloading {
                return;
            }
            last.store(pct, Ordering::SeqCst);
            c.set(&mut st, UpdateState { progress: Some(pct as u32), ..c.with_version(Status::Downloading, &version) });
        };
        let outcome = if pending.as_deref() == Some(version.as_str()) { c.backend.download(&version, &progress, &token).await } else { Err(RETRY_AFTER_CHECK.to_string()) };
        let mut st = c.lock();
        st.cancel = None;
        match outcome {
            Ok(()) => c.set(&mut st, c.with_version(Status::Downloaded, &version)),
            Err(e) => {
                // `cancel()` 가 이미 상태를 되돌렸다 — 취소로 끝난 실패는 오류로 알리지 않는다
                if token.is_cancelled() || st.state.status != Status::Downloading {
                    if st.state.status == Status::Downloading {
                        c.set(&mut st, c.with_version(Status::Available, &version))
                    } else {
                        st.state.clone()
                    }
                } else {
                    c.set(&mut st, UpdateState { error: Some(sanitize_error(&e)), ..c.with_version(Status::Error, &version) })
                }
            }
        }
    }

    pub fn cancel(&self) {
        let c = &*self.0;
        let mut st = c.lock();
        if c.fake.is_some() || st.state.status != Status::Downloading {
            return;
        }
        // 상태는 즉시 되돌리고(`download()` 의 마무리가 이미 바뀐 상태를 보고 넘어간다), 진행 중인 전송을 끊는다
        let version = st.state.version.clone();
        let s = match &version {
            Some(v) => c.with_version(Status::Available, v),
            None => c.base(Status::Available),
        };
        c.set(&mut st, s);
        if let Some(t) = &st.cancel {
            t.cancel();
        }
    }

    pub async fn install(&self) {
        let c = &*self.0;
        {
            let st = c.lock();
            if c.fake.is_some() || st.state.status != Status::Downloaded {
                return;
            }
        }
        // 설치는 프로세스를 끝내는 동기 호출이다(설치 프로그램을 띄우고 `exit(0)`) — 런타임 스레드를 잡지 않게 따로 돈다
        let backend = c.backend.clone();
        let result = tokio::task::spawn_blocking(move || backend.install()).await;
        let error = match result {
            Ok(Ok(())) => return,
            Ok(Err(e)) => sanitize_error(&e),
            Err(_) => INSTALL_FAILED.to_string(),
        };
        // 설치를 시작하지 못했다 — 받아 둔 상태에서 멈춰 있지 않게 오류로 알린다
        let mut st = c.lock();
        let version = st.state.version.clone();
        let s = UpdateState { version, error: Some(error), ..c.base(Status::Error) };
        c.set(&mut st, s);
    }

    pub fn dismiss(&self, action: &Value, version: &Value) -> Result<UpdateState, String> {
        let c = &*self.0;
        let p = parse_dismiss(action, version)?;
        let mut st = c.lock();
        if c.fake.is_some() {
            return Ok(c.set(&mut st, c.base(Status::Idle)));
        }
        // 지금 알리고 있는 버전에 대해서만 받는다(renderer 가 임의 버전으로 알림을 영구히 끄지 못하게)
        if !matches!(st.state.status, Status::Available | Status::Error) || st.state.version.as_deref() != Some(p.version.as_str()) {
            return Err(NOT_NOTIFIED.to_string());
        }
        match p.action {
            DismissAction::Skip => (c.d.set_skipped)(&p.version)?,
            DismissAction::Later => st.policy.snooze(&p.version),
        }
        // 내려받는 중이거나 받아 둔 업데이트는 그대로 두고, 알림 단계에서만 숨긴다
        Ok(if matches!(st.state.status, Status::Available | Status::Error | Status::NotAvailable) { c.set(&mut st, c.base(Status::Idle)) } else { st.state.clone() })
    }

    /// 그 버전(생략하면 현재 버전)의 릴리스 노트. 성공한 결과는 버전별로 10분 동안 재사용하고, 실패하면 다음 요청 때 다시 가져온다
    pub fn release_notes(&self, version: Option<&Value>) -> BoxFuture<'static, Result<ReleaseNotes, String>> {
        let c = &*self.0;
        if let Some(v) = version {
            if !v.as_str().is_some_and(|s| parse_semver(s).is_some()) {
                return std::future::ready(Err(BAD_VERSION.to_string())).boxed();
            }
        }
        let key = strip_v(version.and_then(Value::as_str).unwrap_or(&c.d.current_version)).to_string();
        let mut st = c.lock();
        if let Some((at, fut)) = st.notes.get(&key) {
            if at.elapsed() < NOTES_TTL {
                return fut.clone().boxed();
            }
        }
        let (backend, k) = (self.0.backend.clone(), key.clone());
        let fut: NotesFuture = async move { backend.release_notes(&k).await }.boxed().shared();
        st.notes.insert(key.clone(), (Instant::now(), fut.clone()));
        let (core, watched) = (self.0.clone(), fut.clone());
        drop(st);
        // 실패한 결과는 남기지 않는다(같은 요청이 아직 그 항목일 때만 지운다)
        async move {
            let out = watched.await;
            if out.is_err() {
                let mut st = core.lock();
                if st.notes.get(&key).is_some_and(|(_, f)| f.ptr_eq(&fut)) {
                    st.notes.remove(&key);
                }
            }
            out
        }
        .boxed()
    }
}

/// 라우터가 쥐는 업데이트 서비스(백엔드 형식을 지운다 — `git::fetch::service::FetchControl` 과 같은 모양)
pub trait UpdateControl: Send + Sync + 'static {
    fn get(&self) -> UpdateState;
    fn check(&self, manual: bool) -> BoxFuture<'static, UpdateState>;
    fn download(&self) -> BoxFuture<'static, UpdateState>;
    fn cancel(&self);
    fn install(&self) -> BoxFuture<'static, ()>;
    fn dismiss(&self, action: &Value, version: &Value) -> Result<UpdateState, String>;
    fn release_notes(&self, version: Option<&Value>) -> BoxFuture<'static, Result<ReleaseNotes, String>>;
    fn dispose(&self);
}

impl<B: Backend> UpdateControl for UpdateService<B> {
    fn get(&self) -> UpdateState {
        UpdateService::get(self)
    }

    fn check(&self, manual: bool) -> BoxFuture<'static, UpdateState> {
        let me = self.clone();
        async move { me.check(manual).await }.boxed()
    }

    fn download(&self) -> BoxFuture<'static, UpdateState> {
        let me = self.clone();
        async move { me.download().await }.boxed()
    }

    fn cancel(&self) {
        UpdateService::cancel(self);
    }

    fn install(&self) -> BoxFuture<'static, ()> {
        let me = self.clone();
        async move { me.install().await }.boxed()
    }

    fn dismiss(&self, action: &Value, version: &Value) -> Result<UpdateState, String> {
        UpdateService::dismiss(self, action, version)
    }

    fn release_notes(&self, version: Option<&Value>) -> BoxFuture<'static, Result<ReleaseNotes, String>> {
        UpdateService::release_notes(self, version)
    }

    fn dispose(&self) {
        UpdateService::dispose(self);
    }
}

// ── IPC 채널 ─────────────────────────────────────────────────────────────

fn ok_void() -> Value {
    json!({ "ok": true })
}

fn ok_data(data: Value) -> Value {
    json!({ "ok": true, "data": data })
}

fn err(message: &str) -> Value {
    json!({ "ok": false, "error": message, "code": "ERROR" })
}

fn state_value(s: &UpdateState) -> Value {
    serde_json::to_value(s).unwrap_or(Value::Null)
}

/// `update:*` 채널 하나를 처리해 IpcEnvelope 를 돌려준다(`src/main/ipc/handlers.ts` 의 `handle` 과 같다: 오류는 모두 code `ERROR`, void 는 `data` 없는 `{ok:true}`).
/// `update:check` 는 항상 수동 확인이고, `update:get`·`check`·`download`·`cancel`·`install` 은 인자를 보지 않는다. `args` 는 표식을 되돌린 인자 배열(`None` = undefined)이다
pub async fn update_envelope(svc: &dyn UpdateControl, channel: &str, args: Vec<Option<Value>>) -> Value {
    let a0 = args.first().and_then(Option::as_ref);
    let a1 = args.get(1).and_then(Option::as_ref);
    match channel {
        "update:get" => ok_data(state_value(&svc.get())),
        "update:check" => ok_data(state_value(&svc.check(true).await)),
        "update:download" => ok_data(state_value(&svc.download().await)),
        "update:cancel" => {
            svc.cancel();
            ok_void()
        }
        "update:install" => {
            svc.install().await;
            ok_void()
        }
        "update:dismiss" => match svc.dismiss(a0.unwrap_or(&Value::Null), a1.unwrap_or(&Value::Null)) {
            Ok(s) => ok_data(state_value(&s)),
            Err(e) => err(&e),
        },
        "update:releaseNotes" => match svc.release_notes(a0).await {
            Ok(n) => ok_data(serde_json::to_value(&n).unwrap_or(Value::Null)),
            Err(e) => err(&e),
        },
        other => err(&format!("알 수 없는 채널: {other}")),
    }
}
