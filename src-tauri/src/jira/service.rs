//! Jira 서비스(`src/main/jira/service.ts` + `api.ts` 의 호출부 이식, Phase 4 PR 5 — 앱에는 연결하지 않는다).
//! 연결 상태·세대(epoch)·내 id·날짜 필드·프로필 사진 캐시를 가지고, `jira:*` 채널마다 대응하는 메서드를 둔다.
//! 요청은 [`JiraClient`](PR 3), 응답 변환은 [`api_map`](PR 2), 입력 검증은 [`validate`](PR 1), 토큰 보관은 [`Vault`](PR 4)가 맡는다.
//!
//! - 토큰은 `SecretString` 이다. 반환값에는 토큰이 없다([`JiraAuthState`] 와 `Json` 결과뿐). 오류 문구는 마지막에 `redact_secrets` 를 거친다.
//! - 세대: 연결·해제로 `new_epoch` 가 불리면 진행 중인 요청이 모두 취소되고(`CancellationToken` 의 자식), 내 id·날짜 필드·사진 캐시가 비워진다.
//!   느린 작업이 끝났을 때 세대가 바뀌었으면 결과를 캐시에 넣지 않는다.
//! - `jira:transition` 은 `requestId` 로 취소하지 않는다(적용 여부를 알 수 없게 되므로). POST 는 재시도하지 않는다(클라이언트가 보장한다).
//! - 시계([`Clock`])·대기([`Sleeper`])는 트레이트로 주입한다. 시계는 날짜 필드 쿨다운·사진 캐시 유효 기간, 대기는 `/field` 대기 상한에 쓴다.
//!
//! TS 와 다른 점:
//! - 인자는 IPC 값(`serde_json::Value`)을 그대로 받아 이 모듈이 검증한다(TS 의 `unknown`).
//! - 결과는 키 순서를 지키는 [`Json`] 이다. 상태([`JiraAuthState`])만 `serde` 로 직렬화한다.
//! - `connect` 는 저장에 실패하면(너무 긴 값) 세대를 바꾸기 전에 오류를 돌려줘 기존 연결을 그대로 둔다(TS 는 세대를 먼저 바꾼다).
//! - `disconnect` 는 키체인 삭제가 실패하면 연결은 끊은 채로 그 오류를 돌려준다(TS 의 `clear` 는 실패하지 않는다).
//! - 워크로그는 이슈마다 받은 즉시 거르고 변환해 원본을 쌓지 않는다(TS 와 같은 의도). 20,000건 상한은 이슈 순서로 자른다(TS 는 응답 도착 순서).
//! - `search`·`getMyId`·날짜 계산의 `RangeError` 문구는 TS 가 던지는 문구와 같다(아래 상수).
use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use futures_util::future::{BoxFuture, FutureExt, Shared};
use futures_util::stream::{self, StreamExt};
use serde::Serialize;
use serde_json::Value;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

use super::api_map::{self, DateFieldIds, AVATAR_URL_MAX};
use super::client::{GetOptions, JiraClient, JiraCredential, JIRA_TOO_BIG};
use super::validate::{self, is_user_id, jql_quote, SiteOptions, MAX_PAGES, PAGE_SIZE};
use super::JiraKind;
use crate::git::commit::utf16_len;
use crate::net::http::{now_ms, HttpInitError, NetError};
use crate::net::js_value::{arr_or_empty, encode_uri_component, field, obj, s, str_or_empty};
use crate::net::js_date::parse_date_ms;
use crate::net::json::{js_number, js_number_string, Json};
use crate::net::redact::{jira_secrets, redact_secrets};
use crate::secret::Vault;

/// 날짜 필드 목록 조회가 실패했을 때 다시 시도하기까지의 간격(ms)
const DATE_FIELDS_COOLDOWN_MS: f64 = 5.0 * 60_000.0;
/// 이슈 조회가 날짜 필드 목록(/field)을 기다리는 최대 시간
const DATE_FIELDS_WAIT: Duration = Duration::from_millis(4000);
/// 프로필 사진 캐시: 항목 수 상한, 받은 사진 / 받지 못한 주소의 유효 기간(ms)
const AVATAR_CACHE_MAX: usize = 300;
const AVATAR_TTL_MS: f64 = 24.0 * 3_600_000.0;
const AVATAR_FAIL_TTL_MS: f64 = 10.0 * 60_000.0;
/// 사진을 동시에 받는 수
const AVATAR_CONCURRENCY: usize = 4;

const LIST_FIELDS: &str = "summary,status,issuetype,priority,assignee,updated,duedate";
const DETAIL_FIELDS: &str = "summary,status,issuetype,priority,assignee,updated,duedate,reporter,created,description";
const COMMENT_COUNT: usize = 5;

const WORKLOG_ISSUE_MAX: usize = 300;
const WORKLOG_MAX: usize = 20_000;
/// 워크로그를 내장해 받으므로 페이지가 크면 응답이 커진다
const WORKLOG_ISSUE_PAGE: usize = 25;
/// 이슈별 worklog 엔드포인트만 허용하는 더 큰 응답 상한
const WORKLOG_BODY_MAX_BYTES: usize = 20 * 1024 * 1024;
const WORKLOG_FETCH_CONCURRENCY: usize = 3;
/// 이슈 하나의 워크로그를 이어 받는 최대 횟수(진전이 없으면 그 전에 멈춘다)
const WORKLOG_FETCH_PAGES: usize = 20;
const DAY_MS: f64 = 86_400_000.0;

const NOT_FOUND_ISSUE: &str = "이슈를 찾을 수 없거나 볼 권한이 없습니다";
const NOT_FOUND_API: &str = "이 주소에서 Jira API 를 찾을 수 없습니다(사이트 주소와 Cloud/Server 종류를 확인하세요)";
const NOT_FOUND_SEARCH: &str = "검색 API 를 찾을 수 없습니다(Jira 버전과 사이트 주소를 확인하세요)";
const NOT_CONNECTED: &str = "Jira 에 연결되어 있지 않습니다";
/// `getMyId` 가 id 를 읽을 수 없을 때(TS 의 `Error`)
const BAD_ACCOUNT: &str = "Jira 계정 정보를 읽을 수 없습니다";
/// 서버가 보낸 페이지 표지·날짜가 이상할 때(TS 의 `Error('잘못된 페이지 표지입니다')`)
const BAD_CURSOR: &str = "잘못된 페이지 표지입니다";
/// TS `new Date(NaN).toISOString()` 이 던지는 `RangeError` 의 문구
const INVALID_TIME: &str = "Invalid time value";

// ── 시계·대기 ─────────────────────────────────────────────────────────────

/// 지금 시각(`Date.now()`). 테스트가 주입한다
pub trait Clock: Send + Sync {
    fn now_ms(&self) -> f64;
}

pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> f64 {
        now_ms()
    }
}

/// 기다림(`setTimeout`). 테스트가 주입한다
pub trait Sleeper: Send + Sync {
    fn sleep(&self, d: Duration) -> BoxFuture<'static, ()>;
}

pub struct TokioSleeper;

impl Sleeper for TokioSleeper {
    fn sleep(&self, d: Duration) -> BoxFuture<'static, ()> {
        tokio::time::sleep(d).boxed()
    }
}

// ── 오류 ──────────────────────────────────────────────────────────────────

/// 서비스가 돌려주는 오류. `code` 는 IPC 봉투의 `code`(`handlers.ts` 의 `handle`): 취소 `ABORTED`, Jira API 오류는 `JIRA_*`, 그 밖은 `ERROR`.
/// `message` 는 사용자에게 그대로 보여 줘도 되는 문장이며 비밀이 가려져 있다
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JiraError {
    pub code: &'static str,
    pub status: Option<u16>,
    pub message: String,
}

impl JiraError {
    /// TS 의 평범한 `Error`(봉투 코드 `ERROR`)
    pub fn plain(message: impl Into<String>) -> Self {
        Self { code: "ERROR", status: None, message: message.into() }
    }

    fn api(code: &'static str, message: impl Into<String>) -> Self {
        Self { code, status: None, message: message.into() }
    }

    fn aborted() -> Self {
        Self::api("ABORTED", "aborted")
    }

    /// 문구에서 비밀을 가린다(마지막 단계)
    #[must_use]
    fn sealed(mut self, secrets: &[String]) -> Self {
        let refs: Vec<&str> = secrets.iter().map(String::as_str).collect();
        self.message = redact_secrets(&self.message, &refs);
        self
    }
}

impl From<NetError> for JiraError {
    fn from(e: NetError) -> Self {
        match e {
            NetError::Api { code, status, message } => Self { code, status, message },
            NetError::InvalidPath => Self::plain("invalid api path"),
            NetError::Aborted => Self::aborted(),
        }
    }
}

impl std::fmt::Display for JiraError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for JiraError {}

type Res<T> = Result<T, JiraError>;

// ── 상태 값 ───────────────────────────────────────────────────────────────

/// `jira:auth` 등의 결과. 토큰 필드는 없다
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JiraAuthState {
    pub connected: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub site_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// Jira 토큰이 디스크(키체인)에 남지 않는 상태일 때만 true 로 싣는다
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub storage_unavailable: bool,
}

struct Conn {
    cred: JiraCredential,
    display_name: String,
}

#[derive(Clone)]
struct Epoch {
    id: u64,
    token: CancellationToken,
}

type SharedDates = Shared<BoxFuture<'static, DateFieldIds>>;
type SharedAvatar = Shared<BoxFuture<'static, Option<String>>>;

/// 프로필 사진 캐시: 주소 → (data URL 또는 null, 받은 시각). `order` 의 앞이 가장 오래 쓰지 않은 것
#[derive(Default)]
struct Avatars {
    map: HashMap<String, (Option<String>, f64)>,
    order: VecDeque<String>,
}

impl Avatars {
    fn touch(&mut self, url: &str) {
        if let Some(i) = self.order.iter().position(|u| u == url) {
            if let Some(u) = self.order.remove(i) {
                self.order.push_back(u);
            }
        }
    }

    /// 유효 기간 안의 항목(있으면 최근 사용으로 올린다)
    fn fresh(&mut self, url: &str, now: f64) -> Option<Option<String>> {
        let (data, t) = self.map.get(url)?;
        let ttl = if data.is_some() { AVATAR_TTL_MS } else { AVATAR_FAIL_TTL_MS };
        if now - *t >= ttl {
            return None;
        }
        let data = data.clone();
        self.touch(url);
        Some(data)
    }

    fn put(&mut self, url: &str, data: Option<String>, now: f64) {
        if self.map.insert(url.to_string(), (data, now)).is_some() {
            if let Some(i) = self.order.iter().position(|u| u == url) {
                self.order.remove(i);
            }
        }
        self.order.push_back(url.to_string());
        while self.map.len() > AVATAR_CACHE_MAX {
            match self.order.pop_front() {
                Some(old) => {
                    self.map.remove(&old);
                }
                None => break,
            }
        }
    }

    fn clear(&mut self) {
        self.map.clear();
        self.order.clear();
    }
}

struct Inner {
    conn: Option<Arc<Conn>>,
    epoch: Epoch,
    seq: u64,
    /// requestId → (등록 번호, 취소 토큰). 같은 id 가 다시 등록되면 덮어쓴다
    requests: HashMap<String, (u64, CancellationToken)>,
    /// /myself 로 알아낸 내 id(`{id, name}`). 연결이 바뀌거나 해제되면 비운다
    me: Option<Json>,
    /// /field 로 찾은 날짜 필드(진행 중이거나 성공한 것만). (등록 번호, 공유 결과)
    date_fields: Option<(u64, SharedDates)>,
    /// /field 가 실패한 뒤 이 시각(ms)까지는 다시 묻지 않는다
    date_fields_retry_at: f64,
    avatars: Avatars,
    avatar_inflight: HashMap<String, (u64, SharedAvatar)>,
}

impl Inner {
    fn next_seq(&mut self) -> u64 {
        self.seq += 1;
        self.seq
    }

    /// 연결이 바뀌거나 해제될 때: 진행 중인 요청을 모두 취소하고 연결 단위 캐시를 비운다
    fn new_epoch(&mut self) {
        self.epoch.token.cancel();
        self.epoch = Epoch { id: self.epoch.id + 1, token: CancellationToken::new() };
        self.me = None;
        self.date_fields = None;
        self.date_fields_retry_at = 0.0;
        self.avatars.clear();
        self.avatar_inflight.clear();
    }
}

struct State {
    inner: Mutex<Inner>,
    vault: Arc<Vault>,
    client: JiraClient,
    site: SiteOptions,
    clock: Arc<dyn Clock>,
    sleeper: Arc<dyn Sleeper>,
    date_fields_wait: Duration,
    avatar_slots: Semaphore,
}

impl State {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// 진행 중인 `requestId` 등록. 끝나면(드롭) 내가 등록한 것일 때만 지운다
struct RequestGuard {
    st: Arc<State>,
    id: String,
    seq: u64,
}

impl Drop for RequestGuard {
    fn drop(&mut self) {
        let mut g = self.st.lock();
        if g.requests.get(&self.id).is_some_and(|(n, _)| *n == self.seq) {
            g.requests.remove(&self.id);
        }
    }
}

#[derive(Clone)]
pub struct JiraService {
    st: Arc<State>,
}

impl JiraService {
    /// 운영 서비스: 보관소에서 저장된 연결을 복원한다
    pub fn new(vault: Arc<Vault>) -> Result<Self, HttpInitError> {
        Ok(Self::with_parts(vault, JiraClient::new()?, SiteOptions::default(), Arc::new(SystemClock), Arc::new(TokioSleeper), DATE_FIELDS_WAIT))
    }

    /// 부품을 직접 받는 생성자. 테스트가 루프백용 클라이언트·사이트 옵션·가짜 시계를 넣는다(운영은 [`JiraService::new`])
    pub fn with_parts(vault: Arc<Vault>, client: JiraClient, site: SiteOptions, clock: Arc<dyn Clock>, sleeper: Arc<dyn Sleeper>, date_fields_wait: Duration) -> Self {
        let conn = vault.load_jira_with(site).map(|st| Arc::new(Conn { cred: JiraCredential { kind: st.input.kind, site_url: st.input.site_url, email: st.input.email, token: st.input.token }, display_name: st.display_name }));
        let inner = Inner {
            conn,
            epoch: Epoch { id: 0, token: CancellationToken::new() },
            seq: 0,
            requests: HashMap::new(),
            me: None,
            date_fields: None,
            date_fields_retry_at: 0.0,
            avatars: Avatars::default(),
            avatar_inflight: HashMap::new(),
        };
        Self { st: Arc::new(State { inner: Mutex::new(inner), vault, client, site, clock, sleeper, date_fields_wait, avatar_slots: Semaphore::new(AVATAR_CONCURRENCY) }) }
    }

    /// 날짜 필드 조회를 다시 시도할 수 있게 되는 시각(ms). 쿨다운 시험용
    #[cfg(feature = "test-hooks")]
    #[doc(hidden)]
    pub fn date_fields_retry_at(&self) -> f64 {
        self.st.lock().date_fields_retry_at
    }

    // ── 연결 ────────────────────────────────────────────

    /// `jira:auth`
    pub fn auth(&self) -> JiraAuthState {
        let conn = self.st.lock().conn.clone();
        let storage_unavailable = self.st.vault.jira_storage_unavailable();
        match conn {
            None => JiraAuthState { connected: false, kind: None, site_url: None, email: None, display_name: None, storage_unavailable },
            Some(c) => JiraAuthState {
                connected: true,
                kind: Some(match c.cred.kind {
                    JiraKind::Cloud => "cloud",
                    JiraKind::Server => "server",
                }),
                site_url: Some(c.cred.site_url.clone()),
                email: Some(c.cred.email.clone()).filter(|e| !e.is_empty()),
                display_name: Some(c.display_name.clone()).filter(|n| !n.is_empty()),
                storage_unavailable,
            },
        }
    }

    /// `jira:connect`: 입력한 자격 증명으로 /myself 가 되는지 확인한 뒤 저장하고 연결한다
    pub async fn connect(&self, input: &Value) -> Res<JiraAuthState> {
        let p = validate::parse_connect_input(input, self.st.site).map_err(JiraError::plain)?;
        let cred = JiraCredential { kind: p.kind, site_url: p.site_url.clone(), email: p.email.clone(), token: p.token.clone() };
        let secrets = jira_secrets(&cred.email, &cred.token);
        let seal = |e: JiraError| e.sealed(&secrets);
        let none = CancellationToken::new();
        let api = Api { client: &self.st.client, cred: &cred, cancel: &none };
        let display_name = match api.get(&format!("{}/myself", api_base(cred.kind)), &[], Some(NOT_FOUND_API)).await {
            Ok(me) => api_map::myself_name(&me),
            Err(e) if e.code == "JIRA_AUTH" => return Err(seal(JiraError::plain("이메일·토큰이 올바르지 않거나 만료되었습니다"))),
            Err(e) => return Err(seal(e)),
        };
        // 저장하지 못하면(너무 긴 값 등) 세대를 바꾸기 전에 알린다 — 기존 연결은 그대로다
        self.st.vault.save_jira(&p, &display_name).map_err(|e| JiraError::plain(e.to_string()))?;
        {
            let mut g = self.st.lock();
            g.new_epoch();
            g.conn = Some(Arc::new(Conn { cred, display_name }));
        }
        Ok(self.auth())
    }

    /// `jira:disconnect`. 연결은 항상 끊는다. 키체인 삭제가 실패하면 그 오류를 돌려준다(토큰이 키체인에 남아 있을 수 있다)
    pub fn disconnect(&self) -> Res<JiraAuthState> {
        {
            let mut g = self.st.lock();
            g.requests.clear();
            g.new_epoch();
            g.conn = None;
        }
        self.st.vault.clear_jira().map_err(|e| JiraError::plain(format!("연결은 끊었지만 안전 저장소에서 토큰을 지우지 못했습니다: {e}")))?;
        Ok(self.auth())
    }

    // ── 조회 ────────────────────────────────────────────

    /// 연결을 확인하고 요청 하나를 실행한다. `request_id` 가 있으면 [`JiraService::cancel`] 로 취소할 수 있고, 연결이 바뀌면 항상 취소된다.
    /// 오류 문구에서는 마지막에 비밀을 가린다
    async fn data<T, F, Fut>(&self, request_id: Option<&str>, f: F) -> Res<T>
    where
        F: FnOnce(Arc<Conn>, CancellationToken) -> Fut,
        Fut: Future<Output = Res<T>>,
    {
        let (conn, token, _guard) = {
            let mut g = self.st.lock();
            let Some(conn) = g.conn.clone() else { return Err(JiraError::api("JIRA_AUTH", NOT_CONNECTED)) };
            let token = g.epoch.token.child_token();
            let guard = request_id.map(|id| {
                let seq = g.next_seq();
                g.requests.insert(id.to_string(), (seq, token.clone()));
                RequestGuard { st: self.st.clone(), id: id.to_string(), seq }
            });
            (conn, token, guard)
        };
        let secrets = jira_secrets(&conn.cred.email, &conn.cred.token);
        f(conn, token).await.map_err(|e| e.sealed(&secrets))
    }

    /// `jira:issue`
    pub async fn issue(&self, key: &Value, request_id: Option<&str>) -> Res<Json> {
        let k = validate::parse_issue_key(key).map_err(JiraError::plain)?;
        self.data(request_id, |c, tok| async move {
            let dates = self.date_field_ids(&c.cred, &tok).await?;
            let api = self.api(&c.cred, &tok);
            let fields = std::iter::once(DETAIL_FIELDS).chain(dates.start.iter().map(String::as_str)).chain(dates.end.iter().map(String::as_str)).collect::<Vec<_>>().join(",");
            let expand = (c.cred.kind == JiraKind::Server).then_some("renderedFields");
            let raw = api.get(&format!("{}/issue/{}", api_base(c.cred.kind), encode_uri_component(&k)), &[("fields", Some(&fields)), ("expand", expand)], Some(NOT_FOUND_ISSUE)).await?;
            Ok(api_map::issue_detail(c.cred.kind, &c.cred.site_url, &k, &dates, &raw))
        })
        .await
    }

    /// 시작·종료 날짜 필드 id. 연결 단위로 캐시하고 동시 호출은 같은 작업을 기다린다.
    /// 실패해도 던지지 않는다(그 날짜만 생략) — 캐시하지 않고 쿨다운 동안은 다시 묻지 않는다.
    /// 조회 작업은 개별 요청이 아니라 연결 세대의 신호로 돈다: 한 조회의 취소가 다른 조회에 번지지 않게
    fn shared_date_fields(&self, cred: &JiraCredential) -> SharedDates {
        let mut g = self.st.lock();
        if let Some((_, p)) = &g.date_fields {
            return p.clone();
        }
        if self.st.clock.now_ms() < g.date_fields_retry_at {
            return futures_util::future::ready(DateFieldIds::default()).boxed().shared();
        }
        let (epoch, seq) = (g.epoch.clone(), g.next_seq());
        let (st, cred) = (self.st.clone(), cred.clone());
        let handle = tokio::spawn(async move { run_date_fields(st, cred, epoch, seq).await });
        let shared = async move { handle.await.unwrap_or_default() }.boxed().shared();
        g.date_fields = Some((seq, shared.clone()));
        shared
    }

    /// 공유 /field 작업을 이 조회에서 기다린다. 대기는 상한까지만 — 넘으면 날짜 없이 진행하고 공유 작업은 계속 돌아 끝나면 캐시된다.
    /// 이 조회가 취소되면 기다리지 않고 `ABORTED`
    async fn date_field_ids(&self, cred: &JiraCredential, cancel: &CancellationToken) -> Res<DateFieldIds> {
        let shared = self.shared_date_fields(cred);
        tokio::select! {
            biased;
            () = cancel.cancelled() => Err(JiraError::aborted()),
            v = shared => Ok(v),
            () = self.st.sleeper.sleep(self.st.date_fields_wait) => Ok(DateFieldIds::default()),
        }
    }

    fn api<'a>(&'a self, cred: &'a JiraCredential, cancel: &'a CancellationToken) -> Api<'a> {
        Api { client: &self.st.client, cred, cancel }
    }

    /// `jira:transitions`: 이슈에서 실행할 수 있는 상태 전환 목록
    pub async fn transitions(&self, key: &Value, request_id: Option<&str>) -> Res<Json> {
        let k = validate::parse_issue_key(key).map_err(JiraError::plain)?;
        self.data(request_id, |c, tok| async move {
            let raw = self.api(&c.cred, &tok).get(&format!("{}/issue/{}/transitions", api_base(c.cred.kind), encode_uri_component(&k)), &[], Some(NOT_FOUND_ISSUE)).await?;
            Ok(Json::Arr(api_map::transitions(&raw)))
        })
        .await
    }

    /// `jira:transition`: 상태 전환 실행(쓰기). `requestId` 로 취소하지 않는다 — 도중에 끊으면 적용 여부를 알 수 없게 되므로. 연결이 바뀔 때만 중단된다.
    /// 재시도하지 않는다
    pub async fn transition(&self, key: &Value, transition_id: &Value) -> Res<()> {
        let k = validate::parse_issue_key(key).map_err(JiraError::plain)?;
        let id = validate::parse_transition_id(transition_id).map_err(JiraError::plain)?;
        self.data(None, |c, tok| async move {
            let body = obj(vec![("transition", obj(vec![("id", s(&id))]))]);
            let opts = GetOptions { not_found: Some(NOT_FOUND_ISSUE), cancel: Some(&tok), ..Default::default() };
            self.st.client.post(&c.cred, &format!("{}/issue/{}/transitions", api_base(c.cred.kind), encode_uri_component(&k)), &body, opts).await?;
            Ok(())
        })
        .await
    }

    /// `jira:comments`: 최신 5개
    pub async fn comments(&self, key: &Value, request_id: Option<&str>) -> Res<Json> {
        let k = validate::parse_issue_key(key).map_err(JiraError::plain)?;
        self.data(request_id, |c, tok| async move {
            let api = self.api(&c.cred, &tok);
            let path = format!("{}/issue/{}/comment", api_base(c.cred.kind), encode_uri_component(&k));
            let count = COMMENT_COUNT.to_string();
            let raw = if c.cred.kind == JiraKind::Cloud {
                // Cloud 는 orderBy 로 최신순을 직접 받는다
                api.get(&path, &[("orderBy", Some("-created")), ("maxResults", Some(&count))], Some(NOT_FOUND_ISSUE)).await?
            } else {
                // Data Center 는 정렬 인자가 문서화되어 있지 않다 — 첫 페이지의 total 로 마지막 페이지(최신 N개)를 받아 뒤집는다
                let (path, count) = (&path, &count);
                let get = |start: String| async move {
                    let query = [("maxResults", Some(count.as_str())), ("expand", Some("renderedBody")), ("startAt", Some(start.as_str()))];
                    api.get(path, &query, Some(NOT_FOUND_ISSUE)).await
                };
                let first = get("0".to_string()).await?;
                match api_map::server_comments_start_at(&first) {
                    Some(start) => get(js_number_string(start)).await?,
                    None => first,
                }
            };
            Ok(Json::Arr(api_map::comments(c.cred.kind, &raw)))
        })
        .await
    }

    /// `jira:dev`: 개발 정보(Jira 의 GitHub 연동이 모은 PR·커밋). 연동이 없으면 `available:false`
    pub async fn dev_info(&self, key: &Value, request_id: Option<&str>) -> Res<Json> {
        let k = validate::parse_issue_key(key).map_err(JiraError::plain)?;
        self.data(request_id, |c, tok| async move {
            let api = self.api(&c.cred, &tok);
            let issue = api.get(&format!("{}/issue/{}", api_base(c.cred.kind), encode_uri_component(&k)), &[("fields", Some("issuetype"))], Some(NOT_FOUND_ISSUE)).await?;
            let Some(id) = api_map::dev_issue_id(&issue) else { return Ok(api_map::no_dev_info()) };
            // 연동이 없는 사이트는 404·403 을 줄 수 있다 — 오류가 아니라 '정보 없음'으로 본다(취소만 그대로 던진다)
            let soft = |path: &'static str, query: Vec<(&'static str, String)>| async move {
                let refs: Vec<(&str, Option<&str>)> = query.iter().map(|(k, v)| (*k, Some(v.as_str()))).collect();
                match api.get(path, &refs, None).await {
                    Ok(raw) => Ok(Some(raw)),
                    Err(e) if e.code == "ABORTED" => Err(e),
                    Err(_) => Ok(None),
                }
            };
            let Some(summary) = soft("/rest/dev-status/latest/issue/summary", vec![("issueId", id.clone())]).await? else { return Ok(api_map::no_dev_info()) };
            let details = |part: &'static str| {
                let (soft, id, summary) = (&soft, &id, &summary);
                async move {
                    let mut out: Vec<Json> = Vec::new();
                    for application_type in api_map::dev_instances(summary, part) {
                        let data_type = if part == "pullrequest" { "pullrequest" } else { "repository" };
                        let q = vec![("issueId", id.clone()), ("applicationType", application_type), ("dataType", data_type.to_string())];
                        if let Some(raw) = soft("/rest/dev-status/latest/issue/detail", q).await? {
                            out.extend(arr_or_empty(field(Some(&raw), "detail")).iter().cloned());
                        }
                    }
                    Ok::<_, JiraError>(out)
                }
            };
            let pulls = details("pullrequest").await?;
            let repos = details("repository").await?;
            api_map::dev_info(&pulls, &repos).map_err(|_| JiraError::plain(INVALID_TIME))
        })
        .await
    }

    /// `jira:search`: JQL 검색. 한 번에 `PAGE_SIZE` 건, 이어 받기는 `MAX_PAGES` 페이지까지.
    /// Cloud 는 /search/jql 의 nextPageToken, Data Center 는 /search 의 startAt 을 표지로 쓴다
    pub async fn search(&self, query: &Value, request_id: Option<&str>) -> Res<Json> {
        let q = validate::parse_search_query(query).map_err(JiraError::plain)?;
        self.data(request_id, |c, tok| async move {
            let api = self.api(&c.cred, &tok);
            let size = PAGE_SIZE.to_string();
            let raw = if c.cred.kind == JiraKind::Cloud {
                api.get("/rest/api/3/search/jql", &[("jql", Some(&q.jql)), ("fields", Some(LIST_FIELDS)), ("maxResults", Some(&size)), ("nextPageToken", q.cursor.as_deref())], Some(NOT_FOUND_SEARCH)).await?
            } else {
                let start_at = q.cursor.as_deref().map_or(0.0, js_number);
                if !(start_at.fract() == 0.0 && start_at >= 0.0 && start_at <= f64::from(MAX_PAGES * PAGE_SIZE)) {
                    return Err(JiraError::plain(BAD_CURSOR));
                }
                api.get("/rest/api/2/search", &[("jql", Some(&q.jql)), ("fields", Some(LIST_FIELDS)), ("maxResults", Some(&size)), ("startAt", Some(&js_number_string(start_at)))], Some(NOT_FOUND_SEARCH)).await?
            };
            api_map::search_result(c.cred.kind, &raw, q.cursor.as_deref(), q.page).map_err(|_| JiraError::plain(BAD_CURSOR))
        })
        .await
    }

    /// `jira:worklogs`: 범위 안의 워크로그. users 가 비면 나만
    pub async fn worklogs(&self, query: &Value, request_id: Option<&str>) -> Res<Json> {
        let q = validate::parse_worklog_query(query).map_err(JiraError::plain)?;
        self.data(request_id, |c, tok| async move {
            let api = self.api(&c.cred, &tok);
            let (epoch, cached) = {
                let g = self.st.lock();
                (g.epoch.id, g.me.clone())
            };
            let me = match cached {
                Some(me) => me,
                None => {
                    let me = get_my_id(&api).await?;
                    let mut g = self.st.lock();
                    if g.epoch.id == epoch {
                        g.me = Some(me.clone());
                    }
                    me
                }
            };
            get_worklogs(&api, &q, &me).await
        })
        .await
    }

    /// `jira:userSearch`: 팀원 추가용 사용자 검색
    pub async fn user_search(&self, text: &Value, request_id: Option<&str>) -> Res<Json> {
        let t = validate::parse_user_search(text).map_err(JiraError::plain)?;
        self.data(request_id, |c, tok| async move {
            let api = self.api(&c.cred, &tok);
            let raw = match c.cred.kind {
                JiraKind::Cloud => api.get("/rest/api/3/user/search", &[("query", Some(&t)), ("maxResults", Some("20"))], None).await?,
                JiraKind::Server => api.get("/rest/api/2/user/search", &[("username", Some(&t)), ("maxResults", Some("20"))], None).await?,
            };
            Ok(Json::Arr(api_map::search_users(c.cred.kind, &raw)))
        })
        .await
    }

    /// `jira:avatar`: 프로필 사진(이슈·댓글의 avatarUrls 주소)을 data URL 로. 받을 수 없으면 `None` — 사진은 보조 기능이라 오류를 내지 않는다.
    /// 받을 수 있는 주소는 `JiraClient::image` 가 정한다
    pub async fn avatar(&self, raw: &Value) -> Option<String> {
        let url = raw.as_str().filter(|u| !u.is_empty() && utf16_len(u) <= AVATAR_URL_MAX)?;
        let pending = {
            let mut g = self.st.lock();
            let conn = g.conn.clone()?;
            if let Some(hit) = g.avatars.fresh(url, self.st.clock.now_ms()) {
                return hit;
            }
            if let Some((_, p)) = g.avatar_inflight.get(url) {
                p.clone()
            } else {
                let (epoch, seq) = (g.epoch.clone(), g.next_seq());
                let (st, url) = (self.st.clone(), url.to_string());
                let task_url = url.clone();
                let handle = tokio::spawn(async move { run_avatar(st, conn, epoch, seq, task_url).await });
                let shared = async move { handle.await.unwrap_or(None) }.boxed().shared();
                g.avatar_inflight.insert(url, (seq, shared.clone()));
                shared
            }
        };
        pending.await
    }

    /// `jira:openExternal` 이 쓸 검증된 주소(연결한 사이트의 것만). 아니면 `None`
    pub fn external_url(&self, raw: &Value) -> Option<String> {
        let conn = self.st.lock().conn.clone();
        validate::allowed_jira_url(raw, conn.as_deref().map(|c| c.cred.site_url.as_str()), self.st.site)
    }

    // ── 요청 취소 ───────────────────────────────────────

    /// `req:cancel` 에서 호출 — 다른 서비스와 requestId 공간을 공유하므로 없는 id 는 무시한다
    pub fn cancel(&self, request_id: &str) {
        if let Some((_, t)) = self.st.lock().requests.remove(request_id) {
            t.cancel();
        }
    }

    pub fn dispose(&self) {
        let mut g = self.st.lock();
        g.epoch.token.cancel();
        g.requests.clear();
    }
}

// ── 연결 단위 작업(백그라운드) ────────────────────────────────────────────

/// /field 를 받아 날짜 필드를 고른다. 실패는 캐시하지 않고(응답이 너무 큰 경우만 이 연결 동안 빈 결과를 캐시한다) 쿨다운을 건다. 세대가 바뀌었으면 상태를 건드리지 않는다
async fn run_date_fields(st: Arc<State>, cred: JiraCredential, epoch: Epoch, seq: u64) -> DateFieldIds {
    let api = Api { client: &st.client, cred: &cred, cancel: &epoch.token };
    match api.get(&format!("{}/field", api_base(cred.kind)), &[], Some("필드 목록을 찾을 수 없습니다")).await {
        Ok(list) => api_map::pick_date_fields(&list),
        Err(e) => {
            let mut g = st.lock();
            if g.epoch.id == epoch.id {
                // 응답이 너무 크면 다시 물어도 같으므로 이 연결 동안 빈 결과를 캐시한다. 그 밖의 실패는 쿨다운 뒤 재시도
                let too_big = e.code == "JIRA_HTTP" && e.message == JIRA_TOO_BIG;
                if !too_big {
                    if g.date_fields.as_ref().is_some_and(|(n, _)| *n == seq) {
                        g.date_fields = None;
                    }
                    g.date_fields_retry_at = st.clock.now_ms() + DATE_FIELDS_COOLDOWN_MS;
                }
            }
            DateFieldIds::default()
        }
    }
}

/// 사진 한 장을 받아 캐시한다(동시 `AVATAR_CONCURRENCY` 장). 받지 못하면 `None` 을 캐시한다
async fn run_avatar(st: Arc<State>, conn: Arc<Conn>, epoch: Epoch, seq: u64, url: String) -> Option<String> {
    let data = {
        let _slot = st.avatar_slots.acquire().await.ok();
        if epoch.token.is_cancelled() {
            None
        } else {
            let img = st.client.image(&conn.cred, &url, Some(&epoch.token)).await.ok().flatten();
            img.map(|i| format!("data:{};base64,{}", i.content_type, STANDARD.encode(&i.bytes)))
        }
    };
    let mut g = st.lock();
    if g.epoch.id == epoch.id && !epoch.token.is_cancelled() {
        let now = st.clock.now_ms();
        g.avatars.put(&url, data.clone(), now);
    }
    if g.avatar_inflight.get(&url).is_some_and(|(n, _)| *n == seq) {
        g.avatar_inflight.remove(&url);
    }
    data
}

// ── API 호출(`api.ts`) ────────────────────────────────────────────────────

fn api_base(kind: JiraKind) -> &'static str {
    match kind {
        JiraKind::Cloud => "/rest/api/3",
        JiraKind::Server => "/rest/api/2",
    }
}

/// 클라이언트 + 자격 증명 + 취소 신호. 호출마다 같은 셋을 넘기지 않으려는 묶음이다
#[derive(Clone, Copy)]
struct Api<'a> {
    client: &'a JiraClient,
    cred: &'a JiraCredential,
    cancel: &'a CancellationToken,
}

impl Api<'_> {
    async fn get(&self, path: &str, query: &[(&str, Option<&str>)], not_found: Option<&str>) -> Res<Json> {
        self.get_max(path, query, not_found, None).await
    }

    async fn get_max(&self, path: &str, query: &[(&str, Option<&str>)], not_found: Option<&str>, max_bytes: Option<usize>) -> Res<Json> {
        let opts = GetOptions { query, not_found, max_bytes, cancel: Some(self.cancel) };
        Ok(self.client.get(self.cred, path, opts).await?)
    }
}

/// `getMyId`: /myself 의 내 id(`{id, name}`). Cloud 는 accountId, Data Center 는 사용자 이름(name)
async fn get_my_id(api: &Api<'_>) -> Res<Json> {
    let raw = api.get(&format!("{}/myself", api_base(api.cred.kind)), &[], Some(NOT_FOUND_API)).await?;
    api_map::my_id(api.cred.kind, &raw).map_err(|_| JiraError::plain(BAD_ACCOUNT))
}

fn js_range_error<T>(r: Result<T, crate::net::js_value::MapError>) -> Res<T> {
    r.map_err(|_| JiraError::plain(INVALID_TIME))
}

/// JQL 작성자 절. 내 id 가 JQL 에 들어갈 수 없는 모양이면 currentUser() 로 대신한다
fn author_clause(team: &[String], me_id: &str, include_me: bool) -> Res<String> {
    if team.is_empty() {
        return Ok("worklogAuthor = currentUser()".to_string());
    }
    let list = |ids: &[&str]| -> Res<String> {
        let quoted: Result<Vec<String>, String> = ids.iter().map(|i| jql_quote(i)).collect();
        Ok(format!("worklogAuthor in ({})", quoted.map_err(JiraError::plain)?.join(",")))
    };
    let team_refs: Vec<&str> = team.iter().map(String::as_str).collect();
    if !include_me {
        return list(&team_refs);
    }
    if is_user_id(me_id) {
        let mut all = team_refs;
        all.push(me_id);
        return list(&all);
    }
    Ok(format!("(worklogAuthor = currentUser() OR {})", list(&team_refs)?))
}

/// worklogAuthor·worklogDate JQL 로 이슈를 최대 `WORKLOG_ISSUE_MAX` 건 모은다(키로 중복 제거). 더 있으면 truncated
async fn find_worklog_issues(api: &Api<'_>, jql: &str) -> Res<(Vec<api_map::WorklogIssue>, bool)> {
    let mut issues: Vec<api_map::WorklogIssue> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let fields = "summary,issuetype,worklog";
    let size = WORKLOG_ISSUE_PAGE.to_string();
    let mut token: Option<String> = None;
    let mut start_at = 0usize;
    let finish = |mut issues: Vec<api_map::WorklogIssue>, truncated: bool| {
        let over = issues.len() > WORKLOG_ISSUE_MAX;
        issues.truncate(WORKLOG_ISSUE_MAX);
        (issues, truncated || over)
    };
    for _ in 0..=WORKLOG_ISSUE_MAX.div_ceil(WORKLOG_ISSUE_PAGE) {
        let (items, more): (Vec<Json>, bool) = if api.cred.kind == JiraKind::Cloud {
            let raw = api.get("/rest/api/3/search/jql", &[("jql", Some(jql)), ("fields", Some(fields)), ("maxResults", Some(&size)), ("nextPageToken", token.as_deref())], Some(NOT_FOUND_SEARCH)).await?;
            let next = str_or_empty(field(Some(&raw), "nextPageToken")).to_string();
            token = Some(next).filter(|t| !t.is_empty());
            let more = token.is_some() && !matches!(field(Some(&raw), "isLast"), Some(Json::Bool(true)));
            (arr_or_empty(field(Some(&raw), "issues")).to_vec(), more)
        } else {
            let at = start_at.to_string();
            let raw = api.get("/rest/api/2/search", &[("jql", Some(jql)), ("fields", Some(fields)), ("maxResults", Some(&size)), ("startAt", Some(&at))], Some(NOT_FOUND_SEARCH)).await?;
            let items = arr_or_empty(field(Some(&raw), "issues")).to_vec();
            start_at += items.len();
            let more = field(Some(&raw), "total").and_then(Json::as_f64).is_some_and(|t| (start_at as f64) < t);
            (items, more)
        };
        for it in &items {
            let issue = api_map::worklog_issue(it);
            // 페이지 경계에서 같은 이슈가 다시 오면(정렬이 흔들릴 때) 한 번만 쓴다
            if issue.key.is_empty() || !seen.insert(issue.key.clone()) {
                continue;
            }
            issues.push(issue);
        }
        if items.is_empty() || !more {
            return Ok(finish(issues, false));
        }
        if issues.len() >= WORKLOG_ISSUE_MAX {
            return Ok(finish(issues, true));
        }
    }
    Ok(finish(issues, true))
}

/// 이슈의 워크로그를 모두 받는다. Cloud 는 범위로 좁히고, startAt 페이징은 진전이 없으면 멈춘다(같은 id 는 한 번만)
async fn fetch_issue_worklogs(api: &Api<'_>, key: &str, lo: f64, hi: f64) -> Res<Vec<Json>> {
    let path = format!("{}/issue/{}/worklog", api_base(api.cred.kind), encode_uri_component(key));
    let mut seen = std::collections::HashSet::new();
    let mut out: Vec<Json> = Vec::new();
    let mut start_at = 0usize;
    for _ in 0..WORKLOG_FETCH_PAGES {
        // Data Center 는 쿼리 파라미터를 받지 않고 전부 돌려준다 — 거르는 것은 호출자의 몫
        let (at, after, before) = (start_at.to_string(), js_number_string(lo), js_number_string(hi));
        let cloud = [("startAt", Some(at.as_str())), ("maxResults", Some("1000")), ("startedAfter", Some(after.as_str())), ("startedBefore", Some(before.as_str()))];
        let query: &[(&str, Option<&str>)] = if api.cred.kind == JiraKind::Cloud { &cloud } else { &[] };
        let raw = api.get_max(&path, query, Some(NOT_FOUND_ISSUE), Some(WORKLOG_BODY_MAX_BYTES)).await?;
        let list = arr_or_empty(field(Some(&raw), "worklogs"));
        let mut fresh = 0usize;
        for w in list {
            let id = str_or_empty(field(Some(w), "id"));
            if !id.is_empty() && !seen.insert(id.to_string()) {
                continue;
            }
            out.push(w.clone());
            fresh += 1;
        }
        start_at += list.len();
        let total = field(Some(&raw), "total").and_then(Json::as_f64).unwrap_or(0.0);
        if fresh == 0 || (start_at as f64) >= total {
            break;
        }
    }
    Ok(out)
}

/// 그 이슈만 건너뛰어도 되는 오류: 이슈가 없거나(404) 볼 수 없거나(403) 워크로그 응답이 너무 큰 경우
fn skippable_issue_error(e: &JiraError) -> bool {
    matches!(e.code, "JIRA_NOT_FOUND" | "JIRA_FORBIDDEN") || (e.code == "JIRA_HTTP" && e.message == JIRA_TOO_BIG)
}

/// 검색이 400 일 때: 팀원을 한 명씩 가볍게 조회해 Jira 가 거부하는 id 를 골라낸다
async fn find_invalid_users(api: &Api<'_>, team: &[String]) -> Res<Vec<String>> {
    let path = if api.cred.kind == JiraKind::Cloud { "/rest/api/3/search/jql" } else { "/rest/api/2/search" };
    let mut bad = Vec::new();
    for id in team {
        let jql = format!("worklogAuthor = {}", jql_quote(id).map_err(JiraError::plain)?);
        match api.get(path, &[("jql", Some(&jql)), ("fields", Some("key")), ("maxResults", Some("1"))], None).await {
            Ok(_) => {}
            Err(e) if e.status == Some(400) => bad.push(id.clone()),
            Err(e) => return Err(e),
        }
    }
    Ok(bad)
}

/// 범위 안의 워크로그(GET 만). JQL 날짜는 사용자 프로필 시간대 기준일 수 있어 범위를 이틀씩 넓혀 이슈를 찾고,
/// started 가 [from-2일 00:00Z, to+2일 00:00Z+14h) 인 것만 남긴다(UTC+14 ~ UTC-12 를 덮는다) — 정확한 로컬 날짜 필터는 화면의 몫이다
async fn get_worklogs(api: &Api<'_>, q: &validate::WorklogQuery, me: &Json) -> Res<Json> {
    let kind = api.cred.kind;
    let me_id = str_or_empty(field(Some(me), "id")).to_string();
    let include_me = !q.users.is_empty() && q.include_me;
    let from_ms = js_range_error(parse_date_ms(&format!("{}T00:00:00Z", q.from)).ok_or(crate::net::js_value::MapError))?;
    let to_ms = js_range_error(parse_date_ms(&format!("{}T00:00:00Z", q.to)).ok_or(crate::net::js_value::MapError))?;
    let (lo, hi) = (from_ms - 2.0 * DAY_MS, to_ms + 2.0 * DAY_MS + 14.0 * 3_600_000.0);
    let (d_from, d_to) = (js_range_error(api_map::shift_day(&q.from, -2.0))?, js_range_error(api_map::shift_day(&q.to, 2.0))?);
    let dates = format!("worklogDate >= {} AND worklogDate < {} ORDER BY key ASC", jql_quote(&d_from).map_err(JiraError::plain)?, jql_quote(&d_to).map_err(JiraError::plain)?);

    let mut team = q.users.clone();
    let mut invalid: Vec<String> = Vec::new();
    let found = match find_worklog_issues(api, &format!("{} AND {dates}", author_clause(&team, &me_id, include_me)?)).await {
        Ok(f) => f,
        Err(e) => {
            // 400: 없는 사용자(이름 변경·삭제)가 섞였을 수 있다. 팀원이 있을 때만 하나씩 확인해 빼고 다시 조회한다
            if e.status != Some(400) || team.is_empty() {
                return Err(e);
            }
            invalid = find_invalid_users(api, &team).await?;
            if invalid.is_empty() {
                return Err(e);
            }
            team.retain(|u| !invalid.contains(u));
            if team.is_empty() && !include_me {
                return Ok(worklog_json(Vec::new(), me, false, &invalid));
            }
            find_worklog_issues(api, &format!("{} AND {dates}", author_clause(&team, &me_id, include_me)?)).await?
        }
    };
    let (issues, mut truncated) = found;

    // 작성자·범위 거르기는 api_map 이 한다. 원본을 쌓지 않으려고 이슈마다 받는 즉시 변환한다
    let wq = api_map::WorklogQuery { from: &q.from, to: &q.to, users: &team, include_me: q.include_me };
    let convert = |issue: &api_map::WorklogIssue, entries: Vec<&Json>| -> Res<Vec<Json>> {
        let r = js_range_error(api_map::worklog_result(kind, &wq, me, &[(issue, entries)], false))?;
        Ok(arr_or_empty(field(Some(&r), "worklogs")).to_vec())
    };
    let mut per_issue: Vec<Vec<Json>> = vec![Vec::new(); issues.len()];
    let mut todo: Vec<usize> = Vec::new();
    for (idx, issue) in issues.iter().enumerate() {
        if issue.complete {
            per_issue[idx] = convert(issue, issue.embedded.iter().collect())?;
        } else {
            todo.push(idx);
        }
    }
    // 내장 worklog 가 잘린 이슈만 이슈별로 다시 받는다(동시 3개). 전체 실패가 확정되는 첫 오류에서 멈춘다(남은 요청은 시작하지 않고 진행 중인 것은 버린다)
    let mut results = stream::iter(todo.iter().map(|&idx| {
        let key = issues[idx].key.as_str();
        async move { (idx, fetch_issue_worklogs(api, key, lo, hi).await) }
    }))
    .buffer_unordered(WORKLOG_FETCH_CONCURRENCY);
    while let Some((idx, r)) = results.next().await {
        match r {
            Ok(entries) => per_issue[idx] = convert(&issues[idx], entries.iter().collect())?,
            // 이 이슈만 건너뛰고 부분 결과로 돌려준다
            Err(e) if skippable_issue_error(&e) => truncated = true,
            Err(e) => return Err(e),
        }
    }
    drop(results);

    let mut worklogs: Vec<Json> = per_issue.into_iter().flatten().collect();
    if worklogs.len() > WORKLOG_MAX {
        worklogs.truncate(WORKLOG_MAX);
        truncated = true;
    }
    Ok(worklog_json(worklogs, me, truncated, &invalid))
}

fn worklog_json(worklogs: Vec<Json>, me: &Json, truncated: bool, invalid: &[String]) -> Json {
    let mut o = vec![("worklogs", Json::Arr(worklogs)), ("me", me.clone()), ("truncated", Json::Bool(truncated))];
    if !invalid.is_empty() {
        o.push(("invalidUsers", Json::Arr(invalid.iter().map(|u| s(u)).collect())));
    }
    obj(o)
}
