//! GitHub 서비스(`src/main/github/service.ts` 이식, Phase 4 PR 7 — 앱에는 연결하지 않는다). 로그인 상태·세대(epoch)·Device Flow 세션·한도를 가지고,
//! `gh:*` 채널(`gh:repo` 제외 — git 채널이라 라우터가 답한다)마다 대응하는 메서드를 둔다. 요청은 [`GitHubClient`](PR 3), 응답 변환은 [`api_map`](PR 2),
//! 입력 검증은 [`validate`](PR 1), 토큰 보관은 [`Vault`](PR 4), 저장소 정보는 [`RepoQuery`], gh CLI 같은 밖의 출처는 [`ExternalTokenProvider`] 포트다.
//!
//! - 토큰은 `SecretString` 이다. 반환값에는 토큰이 없다([`GhAuthState`]·[`GhDeviceCode`]와 `Json` 결과뿐 — 테스트가 키 목록과 직렬화 문자열을 단언한다).
//!   오류 문구는 마지막에 `redact_secrets` 를 거친다. 오류 코드는 `handlers.ts` 의 순서(ABORTED → `GH_*` → ERROR)다.
//! - 세대: 로그인·로그아웃·토큰 폐기로 `new_epoch` 가 불리면 그 세대의 `CancellationToken`(의 자식)으로 도는 조회가 모두 `ABORTED` 로 끝난다.
//!   조회는 시작할 때 세대 id 를 잡고, 저장소를 물은 뒤 이미 취소됐으면 요청을 시작하지 않는다. 느린 작업(저장된 토큰 검증)이 끝났을 때 세대가 바뀌었으면 결과를 쓰지 않는다.
//! - `connect`(로그인)·`disconnect`(로그아웃)·토큰 폐기는 tokio `Mutex` 하나로 직렬화한다(저장·세대 교체·상태 교체가 서로 섞이지 않게). 로그인 검증 도중 로그아웃하면 그 로그인은 `ABORTED` 다.
//! - 시계([`Clock`])·대기([`Sleeper`])는 `jira::service` 의 트레이트를 쓴다(Device Flow 의 대기·만료 시각, 7b 캐시 유효 기간).
//!
//! TS 와 다른 점:
//! - 인자는 IPC 값(`serde_json::Value`)을 그대로 받아 이 모듈이 검증한다(TS 의 `unknown`). 결과는 키 순서를 지키는 [`Json`]이다. 상태·Device Flow 코드만 `serde` 로 직렬화한다.
//! - 로그인은 저장(키체인)이 실패하면(너무 긴 값 등) 세대를 바꾸기 전에 오류를 낸다(TS 는 세대를 먼저 바꾼다). 로그아웃은 키체인 삭제가 실패해도 연결은 끊고 그 오류를 돌려준다(`async`).
//! - 저장된 토큰이 거부돼 버려질 때 키체인 삭제가 실패하면 오류를 올릴 곳이 없어 삼킨다(메모리에서는 이미 사라졌다).
//! - 로그인 검증은 로그아웃(`logout`)으로 취소된다(TS 는 취소되지 않고 로그아웃 뒤에 로그인이 되살아날 수 있다).
//! - 응답 해석 실패(TS 라면 `TypeError`)는 고정 문구("GitHub 응답 형식이 올바르지 않습니다")의 `ERROR` 다.
use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use futures_util::future::{BoxFuture, FutureExt, Shared};
use serde::Serialize;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use super::api_map;
use super::avatars::{AvatarResolver, Fallback, FALLBACK_SHAS};
use super::checks::{self, CheckStateCache};
use super::client::{GetOptions, GitHubClient, TokenChoice, TokenProvider};
use super::device_flow::{DeviceFlow, DeviceSession, GhDeviceCode};
use super::device_rules::MISSING_CLIENT_ID_MESSAGE;
use super::error::{GhError, Res};
use super::launchpad::{self, RESOLVE_TIMEOUT};
use super::ports::{ExternalTokenProvider, GhRepoRef, RepoQuery};
use super::queries::{self, Slug};
use super::validate;
use crate::git::fetch::js_trim;
use crate::jira::service::{Clock, SystemClock, TokioSleeper};
use crate::net::http::{HttpInitError, NetError};
use crate::net::js_value::{obj, template_string};
use crate::net::json::Json;
use crate::net::redact::{github_secrets, SecretString};
use crate::secret::{GithubMethod, Vault};

const NOT_GITHUB: &str = "이 저장소의 remote 가 GitHub(github.com) 저장소가 아닙니다";
const BAD_TOKEN: &str = "토큰이 올바르지 않거나 만료되었습니다";
const NO_FLOW: &str = "진행 중인 로그인이 없습니다. 다시 시작하세요";
const NO_CLI: &str = "이 환경에서는 GitHub CLI 로그인 가져오기를 쓸 수 없습니다";
const BAD_CLI_TOKEN: &str = "GitHub CLI 가 돌려준 토큰 형식이 올바르지 않습니다";
const CLI_HINT: &str = " — 터미널에서 `gh auth login` 으로 다시 로그인한 뒤 시도하세요";
/// 접근 가능한 저장소 목록 캐시 유효 시간
const ACCESSIBLE_TTL_MS: f64 = 5.0 * 60_000.0;

/// 서비스 생성 옵션
#[derive(Clone, Debug)]
pub struct ServiceOptions {
    /// 작성자 아바타 캐시 파일 경로(없으면 메모리에만 둔다)
    pub avatar_cache_file: Option<PathBuf>,
    /// Launchpad 가 저장소 하나의 remote 를 읽는 제한 시간
    pub remote_timeout: Duration,
}

impl Default for ServiceOptions {
    fn default() -> Self {
        Self { avatar_cache_file: None, remote_timeout: RESOLVE_TIMEOUT }
    }
}

// ── 상태 값 ───────────────────────────────────────────────────────────────

/// `GhUser`
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GhUser {
    pub login: String,
    pub avatar_url: String,
}

/// `gh:auth` 등의 결과. 토큰 필드는 없다
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GhAuthState {
    pub signed_in: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<GhUser>,
    /// `pat` | `device` | `gh-cli`
    #[serde(skip_serializing_if = "Option::is_none")]
    pub method: Option<&'static str>,
    /// 남은 API 호출 수(마지막 응답 기준)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rate_remaining: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rate_limit: Option<u64>,
    /// epoch seconds
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rate_reset: Option<u64>,
    /// 토큰이 디스크(키체인)에 남지 않는 상태일 때만 true 로 싣는다
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub storage_unavailable: bool,
    /// gh CLI 로그인 가져오기를 쓸 수 있는지(아직 모르면 생략)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cli_available: Option<bool>,
}

fn gh_user(data: &Json) -> GhUser {
    let u = api_map::map_user(Some(data));
    GhUser { login: template_string(u.get("login")), avatar_url: template_string(u.get("avatarUrl")) }
}

#[derive(Clone)]
struct Epoch {
    id: u64,
    token: CancellationToken,
}

type SharedUnit = Shared<BoxFuture<'static, ()>>;
type AuthChanged = Arc<dyn Fn() + Send + Sync>;

struct Inner {
    method: Option<GithubMethod>,
    user: Option<GhUser>,
    epoch: Epoch,
    seq: u64,
    /// requestId → (등록 번호, 취소 토큰). 같은 id 가 다시 등록되면 덮어쓴다
    requests: HashMap<String, (u64, CancellationToken)>,
    /// 저장된 토큰의 `/user` 검증(첫 `gh:auth` 에서 한 번, 동시 호출은 합친다). (등록 번호, 공유 결과)
    validating: Option<(u64, SharedUnit)>,
    /// 진행 중인 Device Flow 세션. (등록 번호, 세션)
    device: Option<(u64, DeviceSession)>,
    cli_available: Option<bool>,
    /// 접근 가능한 저장소 목록 캐시(결과, 받은 시각 ms). 계정이 바뀌면 보이는 범위가 달라지므로 로그인·로그아웃 때 비운다
    accessible: Option<(Json, f64)>,
    /// 로그아웃만 올리는 세대. 로그인 계열(login·device_poll·import_cli_token)은 진입 직후(첫 await 전)에 잡고, 저장 직전(change 잠금 안)에 같은지 본다
    /// (토큰 폐기·다른 로그인으로 오르는 `epoch` 와 달리, 로그인 도중 로그아웃이 있었는지만 가린다)
    logout_gen: u64,
    /// 로그인 계열이 자식 토큰으로 쓰는 취소 신호. 로그아웃이 취소하고 새것으로 바꾼다(requestId 맵과 무관하게 모든 진행 중 로그인을 멈춘다)
    logout_token: CancellationToken,
}

impl Inner {
    fn next_seq(&mut self) -> u64 {
        self.seq += 1;
        self.seq
    }

    /// 토큰이 바뀌거나 로그아웃할 때: 이 세대의 조회를 모두 취소하고 새 세대를 연다
    fn new_epoch(&mut self) {
        self.epoch.token.cancel();
        self.epoch = Epoch { id: self.epoch.id + 1, token: CancellationToken::new() };
        self.validating = None;
    }
}

struct State {
    inner: Mutex<Inner>,
    /// 지금 로그인한 토큰. 클라이언트의 `TokenProvider` 가 읽는다. 쓰는 곳은 모두 `inner` 잠금 안이다(잠금 순서: inner → token)
    token: Arc<Mutex<Option<SecretString>>>,
    vault: Arc<Vault>,
    client: Arc<GitHubClient>,
    flow: DeviceFlow,
    repos: Arc<dyn RepoQuery>,
    cli: Option<Arc<dyn ExternalTokenProvider>>,
    clock: Arc<dyn Clock>,
    avatars: AvatarResolver,
    /// 커밋 체크 요약 캐시(계정이 바뀌면 보이는 범위가 달라지므로 로그인·로그아웃 때 비운다)
    checks: CheckStateCache,
    remote_timeout: Duration,
    /// 로그인·로그아웃·토큰 폐기를 한 번에 하나만 실행한다(저장·세대 교체·상태 교체가 다른 호출과 섞이지 않게)
    change: tokio::sync::Mutex<()>,
    on_auth_changed: Mutex<Option<AuthChanged>>,
}

impl State {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn token(&self) -> Option<SecretString> {
        self.token.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn set_token(&self, t: Option<SecretString>) {
        *self.token.lock().unwrap_or_else(|e| e.into_inner()) = t;
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
pub struct GitHubService {
    st: Arc<State>,
}

impl GitHubService {
    /// 운영 서비스: 보관소에서 저장된 토큰을 복원한다. 한도 해제 시각은 로컬 시간대로 표시한다([`local_time`](super::local_time))
    pub fn new(vault: Arc<Vault>, repos: Arc<dyn RepoQuery>, cli: Option<Arc<dyn ExternalTokenProvider>>, opts: ServiceOptions) -> Result<Self, HttpInitError> {
        let clock: Arc<dyn Clock> = Arc::new(SystemClock);
        let flow = DeviceFlow::new(Arc::new(TokioSleeper), clock.clone())?;
        Self::with_parts(vault, repos, cli, |p| GitHubClient::new(p).map(|c| c.with_format_time(super::local_time::format_epoch_sec)), flow, clock, opts)
    }

    /// 부품을 직접 받는 생성자. `make_client` 는 서비스가 쥔 토큰을 읽는 `TokenProvider` 를 받아 클라이언트를 만든다.
    /// 테스트가 루프백용 클라이언트·Device Flow·가짜 시계를 넣는다(운영은 [`GitHubService::new`])
    pub fn with_parts(
        vault: Arc<Vault>,
        repos: Arc<dyn RepoQuery>,
        cli: Option<Arc<dyn ExternalTokenProvider>>,
        make_client: impl FnOnce(TokenProvider) -> Result<GitHubClient, HttpInitError>,
        flow: DeviceFlow,
        clock: Arc<dyn Clock>,
        opts: ServiceOptions,
    ) -> Result<Self, HttpInitError> {
        let stored = vault.load_github();
        let token: Arc<Mutex<Option<SecretString>>> = Arc::new(Mutex::new(stored.as_ref().map(|s| s.token.clone())));
        let cell = token.clone();
        let provider: TokenProvider = Arc::new(move || cell.lock().unwrap_or_else(|e| e.into_inner()).clone());
        let client = Arc::new(make_client(provider)?);
        let inner = Inner {
            method: stored.as_ref().map(|s| s.method),
            user: None,
            epoch: Epoch { id: 0, token: CancellationToken::new() },
            seq: 0,
            requests: HashMap::new(),
            validating: None,
            device: None,
            cli_available: None,
            accessible: None,
            logout_gen: 0,
            logout_token: CancellationToken::new(),
        };
        let avatars = AvatarResolver::new(client.clone(), opts.avatar_cache_file, clock.clone());
        let checks = CheckStateCache::new(clock.clone());
        let st = State {
            inner: Mutex::new(inner),
            token,
            vault,
            client,
            flow,
            repos,
            cli,
            clock,
            avatars,
            checks,
            remote_timeout: opts.remote_timeout,
            change: tokio::sync::Mutex::new(()),
            on_auth_changed: Mutex::new(None),
        };
        Ok(Self { st: Arc::new(st) })
    }

    /// 로그인·로그아웃·토큰 폐기로 토큰이 바뀌었을 때 부를 함수를 정한다(원격 전용 저장소의 인증 실패 정지를 풀 때 쓴다). 잠금 밖에서 불린다
    pub fn set_on_auth_changed(&self, f: impl Fn() + Send + Sync + 'static) {
        *self.st.on_auth_changed.lock().unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(f));
    }

    fn notify_auth_changed(&self) {
        let f = self.st.on_auth_changed.lock().unwrap_or_else(|e| e.into_inner()).clone();
        if let Some(f) = f {
            f();
        }
    }

    // ── 인증 ────────────────────────────────────────────

    fn state_of(&self, g: &Inner) -> GhAuthState {
        let signed_in = self.st.token().is_some();
        let rate = self.st.client.rate();
        GhAuthState {
            signed_in,
            user: g.user.clone().filter(|_| signed_in),
            method: g.method.map(GithubMethod::as_str).filter(|_| signed_in),
            rate_remaining: rate.map(|r| r.remaining as u64),
            rate_limit: rate.map(|r| r.limit as u64),
            rate_reset: rate.map(|r| r.reset as u64),
            storage_unavailable: self.st.vault.github_storage_unavailable(),
            cli_available: g.cli_available,
        }
    }

    fn state(&self) -> GhAuthState {
        let g = self.st.lock();
        self.state_of(&g)
    }

    /// `gh:auth`: 현재 로그인 상태. 저장된 토큰이 있으면 첫 호출에서 `GET /user` 로 검증하고 사용자 정보를 캐시한다
    pub async fn auth(&self) -> GhAuthState {
        if let Some(cli) = self.st.cli.clone() {
            let unknown = self.st.lock().cli_available.is_none();
            if unknown {
                let available = cli.available().await;
                self.st.lock().cli_available = Some(available);
            }
        }
        let pending = {
            let g = self.st.lock();
            (self.st.token().is_some() && g.user.is_none()).then_some(g.epoch.id)
        };
        if let Some(gen) = pending {
            if let Some(p) = self.validate(gen) {
                p.await;
            }
        }
        self.state()
    }

    /// 저장된 토큰의 검증을 시작하거나 진행 중인 것을 돌려준다. 세대가 이미 바뀌었으면(`gen` 이 낡았거나 취소됐으면) 시작하지 않는다 — 세대 교체와 같은 잠금 안에서 본다.
    /// 작업은 `tokio::spawn` 으로 돌아 기다리는 호출이 모두 떠나도 끝난다
    fn validate(&self, gen: u64) -> Option<SharedUnit> {
        let mut g = self.st.lock();
        if g.epoch.id != gen || g.epoch.token.is_cancelled() || self.st.token().is_none() {
            return None;
        }
        if let Some((_, p)) = &g.validating {
            return Some(p.clone());
        }
        let (epoch, seq) = (g.epoch.clone(), g.next_seq());
        let svc = self.clone();
        let handle = tokio::spawn(async move { svc.run_validate(epoch, seq).await });
        let shared = async move {
            let _ = handle.await;
        }
        .boxed()
        .shared();
        g.validating = Some((seq, shared.clone()));
        Some(shared)
    }

    /// `GET /user` 로 검증한다. 거부(401)면 토큰을 버려 로그인 화면으로 돌린다. 네트워크 문제 등은 로그인 상태를 유지하고 다음 호출에서 다시 확인한다
    async fn run_validate(&self, epoch: Epoch, seq: u64) {
        let opts = GetOptions { cancel: Some(&epoch.token), ..Default::default() };
        match self.st.client.get("/user", &TokenChoice::Stored, opts).await {
            Ok(res) => {
                let mut g = self.st.lock();
                if g.epoch.id == epoch.id {
                    g.user = Some(gh_user(&res.data));
                }
            }
            Err(NetError::Api { code: "GH_AUTH", .. }) => {
                let _ = self.drop_token(epoch.id).await;
            }
            Err(_) => {}
        }
        let mut g = self.st.lock();
        if g.validating.as_ref().is_some_and(|(n, _)| *n == seq) {
            g.validating = None;
        }
    }

    /// 이 토큰으로 `GET /user` 가 되는지 확인하고 사용자 정보를 돌려준다(저장 전 검증)
    async fn verify(&self, token: &SecretString, cancel: &CancellationToken) -> Res<GhUser> {
        let opts = GetOptions { cancel: Some(cancel), ..Default::default() };
        match self.st.client.get("/user", &TokenChoice::Explicit(token.clone()), opts).await {
            Ok(res) => Ok(gh_user(&res.data)),
            Err(NetError::Api { code: "GH_AUTH", .. }) => Err(GhError::plain(BAD_TOKEN)),
            Err(e) => Err(e.into()),
        }
    }

    /// 요청 등록(취소할 수 있게). 같은 id 가 다시 등록되면 덮어쓴다
    fn register(&self, id: &str, token: &CancellationToken) -> RequestGuard {
        let mut g = self.st.lock();
        let seq = g.next_seq();
        g.requests.insert(id.to_string(), (seq, token.clone()));
        RequestGuard { st: self.st.clone(), id: id.to_string(), seq }
    }

    /// 사용자가 취소할 수 없는 로그인 검증도 로그아웃으로는 취소되게 내부 id 로 등록한다(제어 문자가 있어 IPC 의 requestId 와 겹치지 않는다)
    fn login_id(&self) -> String {
        let seq = self.st.lock().next_seq();
        format!("\u{0}login#{seq}")
    }

    /// 로그인 계열의 시작: 로그아웃 세대와 그 자식 취소 토큰을 잡는다. **첫 await 전에** 불러야 그 사이의 로그아웃을 놓치지 않는다
    fn login_scope(&self) -> (u64, CancellationToken) {
        let g = self.st.lock();
        (g.logout_gen, g.logout_token.child_token())
    }

    /// 토큰으로 로그인한다: 검증 → 저장 → 세대 교체. 저장에 실패하면 상태를 바꾸지 않는다. `logout_gen` 은 로그인을 시작할 때 잡은 [`Inner::logout_gen`]
    async fn sign_in(&self, token: SecretString, method: GithubMethod, user: GhUser, logout_gen: u64, cancel: &CancellationToken) -> Res<GhAuthState> {
        let state = {
            let _change = self.st.change.lock().await;
            // 검증하는 동안(또는 그 전에) 로그아웃했으면 되살리지 않는다
            if cancel.is_cancelled() || self.st.lock().logout_gen != logout_gen {
                return Err(GhError::aborted());
            }
            self.st.vault.save_github(&token, method).map_err(|e| GhError::plain(e.to_string()))?;
            let mut g = self.st.lock();
            g.new_epoch();
            self.st.set_token(Some(token));
            g.method = Some(method);
            g.user = Some(user);
            // 한도·"로그인 필요" 표시는 비로그인 기준이었으니 비우고 다시 묻게 한다
            self.reset_caches(&mut g);
            self.state_of(&g)
        };
        self.notify_auth_changed();
        Ok(state)
    }

    /// 계정이 바뀌면 보이는 범위가 달라지는 캐시를 비운다
    fn reset_caches(&self, g: &mut Inner) {
        self.st.avatars.reset_session();
        self.st.checks.clear();
        g.accessible = None;
    }

    /// 토큰을 버린다. `gen` 이 있으면 그 세대일 때만(그 사이 다른 로그인·로그아웃이 있었으면 건드리지 않는다). 키체인 삭제 실패는 돌려준다
    async fn drop_token(&self, gen: u64) -> Result<(), crate::secret::SecretError> {
        let r = {
            let _change = self.st.change.lock().await;
            {
                let mut g = self.st.lock();
                if g.epoch.id != gen {
                    return Ok(());
                }
                self.clear_login(&mut g);
            }
            self.st.vault.clear_github()
        };
        self.notify_auth_changed();
        r
    }

    /// 로그인 상태를 비운다(`inner` 잠금 안에서)
    fn clear_login(&self, g: &mut Inner) {
        g.new_epoch();
        self.st.set_token(None);
        g.method = None;
        g.user = None;
        self.st.client.clear_rate();
        self.st.client.clear_cache();
        self.reset_caches(g);
    }

    /// 현재 로그인 토큰(없으면 `None`). 원격 전용 저장소(앱 캐시의 bare 저장소)를 github.com 에서 fetch 할 때 git 에 헤더로만 넘기는 용도다. renderer·로그로 내보내지 않는다
    pub fn git_token(&self) -> Option<SecretString> {
        self.st.token()
    }

    /// `gh:loginPat`
    pub async fn login_pat(&self, raw: &Value) -> Res<GhAuthState> {
        let token = validate::parse_pat(raw).map_err(GhError::plain)?;
        let secrets = github_secrets(&token);
        self.login(token, GithubMethod::Pat).await.map_err(|e| e.sealed(&secrets))
    }

    async fn login(&self, token: SecretString, method: GithubMethod) -> Res<GhAuthState> {
        let (logout_gen, cancel) = self.login_scope();
        let _guard = self.register(&self.login_id(), &cancel);
        let user = self.verify(&token, &cancel).await?;
        self.sign_in(token, method, user, logout_gen, &cancel).await
    }

    /// `gh:importCliToken`: 밖의 출처(GitHub CLI)의 로그인 토큰을 가져와 로그인한다. 토큰은 이 함수 안에서만 쓰이고 반환값·오류·로그에 실리지 않는다
    pub async fn import_cli_token(&self) -> Res<GhAuthState> {
        let Some(cli) = self.st.cli.clone() else { return Err(GhError::plain(NO_CLI)) };
        // 외부 토큰을 기다리는 동안의 로그아웃도 막도록 취소·세대를 먼저 잡고 등록한다
        let (logout_gen, cancel) = self.login_scope();
        let _guard = self.register(&self.login_id(), &cancel);
        let raw = cli.token().await.map_err(|e| GhError::plain(e.message))?;
        let token = validate::parse_pat_str(raw.expose()).map_err(|_| GhError::plain(BAD_CLI_TOKEN))?;
        drop(raw);
        let secrets = github_secrets(&token);
        let result = async {
            match self.verify(&token, &cancel).await {
                // 검증이 거부한 토큰의 안내는 gh 쪽 안내를 덧붙인다(GitHub API 오류·취소는 그대로)
                Err(e) if e.code == "ERROR" => Err(GhError::plain(format!("{}{CLI_HINT}", e.message))),
                Err(e) => Err(e),
                Ok(user) => self.sign_in(token.clone(), GithubMethod::GhCli, user, logout_gen, &cancel).await,
            }
        }
        .await;
        result.map_err(|e| e.sealed(&secrets))
    }

    async fn client_id(&self) -> Res<String> {
        let id = self.st.repos.github_client_id().await.map(|s| js_trim(&s).to_string()).filter(|s| !s.is_empty());
        id.ok_or_else(|| GhError::plain(MISSING_CLIENT_ID_MESSAGE))
    }

    /// `gh:deviceStart`: Device Flow 시작. 이전에 진행 중이던 흐름은 버린다
    pub async fn device_start(&self) -> Res<GhDeviceCode> {
        let client_id = self.client_id().await?;
        let cancel = self.st.lock().epoch.token.child_token();
        let (code, session) = self.st.flow.start(&client_id, &cancel).await?;
        let mut g = self.st.lock();
        let seq = g.next_seq();
        g.device = Some((seq, session));
        Ok(code)
    }

    /// `gh:devicePoll`: 사용자가 승인할 때까지 폴링하고 로그인한다. 취소(`ABORTED`)면 세션을 남겨 다시 폴링할 수 있다
    pub async fn device_poll(&self, request_id: &Value) -> Res<GhAuthState> {
        let id = validate::parse_id(request_id, "요청 ID").map_err(GhError::plain)?;
        // 세션을 복사한 뒤 client_id 를 기다리는 동안의 로그아웃도 막도록, 세션·취소·세대를 같은 잠금 안에서 잡고 첫 await 전에 등록한다
        let (seq, session, logout_gen, cancel) = {
            let g = self.st.lock();
            let (seq, session) = g.device.clone().ok_or_else(|| GhError::plain(NO_FLOW))?;
            (seq, session, g.logout_gen, g.logout_token.child_token())
        };
        let _guard = self.register(&id, &cancel);
        let client_id = self.client_id().await?;
        let secrets = zeroize::Zeroizing::new(vec![session.device_code.expose().to_string()]);
        let result = async {
            let token = match self.st.flow.poll(&client_id, &session, &cancel).await {
                Ok(t) => t,
                Err(e) => {
                    // 만료·거부·실패한 세션은 다시 쓸 수 없다
                    if !e.is_aborted() {
                        self.finish_device(seq);
                    }
                    return Err(e);
                }
            };
            self.finish_device(seq);
            // 서버가 준 access_token 도 PAT 와 같은 모양 검사를 통과한 것만 쓴다(공백·제어 문자가 든 값이 보관소로 가지 않게). 오류에는 토큰이 섞이지 않는 고정 문구뿐이다
            let token = validate::parse_pat_str(token.expose()).map_err(GhError::plain)?;
            let token_secrets = github_secrets(&token);
            let user = self.verify(&token, &cancel).await.map_err(|e| e.sealed(&token_secrets))?;
            self.sign_in(token, GithubMethod::Device, user, logout_gen, &cancel).await
        }
        .await;
        result.map_err(|e| e.sealed(&secrets))
    }

    fn finish_device(&self, seq: u64) {
        let mut g = self.st.lock();
        if g.device.as_ref().is_some_and(|(n, _)| *n == seq) {
            g.device = None;
        }
    }

    /// `gh:logout`. 연결은 항상 끊는다. 키체인 삭제가 실패하면 그 오류를 돌려준다(토큰이 키체인에 남아 있을 수 있다).
    /// 진행 중인 로그인의 저장·교체가 끝난 뒤에 실행한다
    pub async fn logout(&self) -> Res<GhAuthState> {
        let (state, cleared) = {
            let _change = self.st.change.lock().await;
            let state = {
                let mut g = self.st.lock();
                g.device = None;
                // 로그인 계열은 requestId 맵에서 밀려났어도 이 토큰의 자식이라 모두 멈춘다
                g.logout_gen += 1;
                g.logout_token.cancel();
                g.logout_token = CancellationToken::new();
                for (_, t) in g.requests.values() {
                    t.cancel();
                }
                g.requests.clear();
                self.clear_login(&mut g);
                self.state_of(&g)
            };
            (state, self.st.vault.clear_github())
        };
        self.notify_auth_changed();
        cleared.map_err(|e| GhError::plain(format!("로그아웃했지만 안전 저장소에서 토큰을 지우지 못했습니다: {e}")))?;
        Ok(state)
    }

    // ── 조회 ────────────────────────────────────────────

    async fn slug(&self, repo_id: &str) -> Res<Slug> {
        let r: Option<GhRepoRef> = self.st.repos.gh_repo(repo_id).await.map_err(GhError::plain)?;
        // 저장소 정보가 준 값은 믿지 않는다 — `..` 같은 값이 URL 경로로 새지 않게 형식을 다시 확인한다(TS 의 `parseGithubRemote` 와 같은 규칙)
        r.and_then(|r| Slug::parse(&r.owner, &r.repo)).ok_or_else(|| GhError::plain(NOT_GITHUB))
    }

    /// 데이터 호출 공통 처리: 현재 세대의 신호를 싣고, 토큰이 있는데 401(`GH_AUTH`)이면 `GET /user` 로 한 번 다시 확인해 그것도 거부되면 토큰을 버린다
    /// (세션 중 폐기된 토큰). 원래 오류는 그대로 돌려준다. 저장소를 묻는 동안 세대가 바뀌었으면 요청을 시작하지 않는다
    async fn data<T, F, Fut>(&self, repo_id: &str, f: F) -> Res<T>
    where
        F: FnOnce(Slug, CancellationToken) -> Fut,
        Fut: Future<Output = Res<T>>,
    {
        let (gen, cancel) = {
            let g = self.st.lock();
            (g.epoch.id, g.epoch.token.child_token())
        };
        let token = self.st.token();
        let secrets = token.as_ref().map(github_secrets).unwrap_or_default();
        let result = async {
            let slug = self.slug(repo_id).await?;
            if cancel.is_cancelled() {
                return Err(GhError::aborted());
            }
            f(slug, cancel).await
        }
        .await;
        if let Err(e) = &result {
            if token.is_some() && e.code == "GH_AUTH" {
                if let Some(p) = self.validate(gen) {
                    p.await;
                }
            }
        }
        result.map_err(|e| e.sealed(&secrets))
    }

    fn repo_id(v: &Value) -> Res<String> {
        validate::parse_id(v, "저장소 ID").map_err(GhError::plain)
    }

    /// `gh:pulls`
    pub async fn pulls(&self, repo_id: &Value, query: &Value) -> Res<Json> {
        let (id, q) = (Self::repo_id(repo_id)?, validate::parse_list_query(query).map_err(GhError::plain)?);
        self.data(&id, |slug, cancel| async move { queries::list_pulls(&self.st.client, &slug, &q, &cancel).await }).await
    }

    /// `gh:pullsForKey`: Jira 이슈 키를 언급한 PR
    pub async fn pulls_for_key(&self, repo_id: &Value, key: &Value) -> Res<Json> {
        let (id, k) = (Self::repo_id(repo_id)?, crate::jira::validate::parse_issue_key(key).map_err(GhError::plain)?);
        self.data(&id, |slug, cancel| async move { queries::search_pulls_by_key(&self.st.client, &slug, &k, &cancel).await }).await
    }

    /// `gh:pull`
    pub async fn pull(&self, repo_id: &Value, n: &Value) -> Res<Json> {
        let (id, n) = (Self::repo_id(repo_id)?, validate::parse_number(n).map_err(GhError::plain)?);
        self.data(&id, |slug, cancel| async move { queries::get_pull(&self.st.client, &slug, n, &cancel).await }).await
    }

    /// `gh:pullFiles`
    pub async fn pull_files(&self, repo_id: &Value, n: &Value) -> Res<Json> {
        let (id, n) = (Self::repo_id(repo_id)?, validate::parse_number(n).map_err(GhError::plain)?);
        self.data(&id, |slug, cancel| async move { queries::get_pull_files(&self.st.client, &slug, n, &cancel).await }).await
    }

    /// `gh:pullCommits`
    pub async fn pull_commits(&self, repo_id: &Value, n: &Value) -> Res<Json> {
        let (id, n) = (Self::repo_id(repo_id)?, validate::parse_number(n).map_err(GhError::plain)?);
        self.data(&id, |slug, cancel| async move { queries::get_pull_commits(&self.st.client, &slug, n, &cancel).await }).await
    }

    /// `gh:issues`
    pub async fn issues(&self, repo_id: &Value, query: &Value) -> Res<Json> {
        let (id, q) = (Self::repo_id(repo_id)?, validate::parse_list_query(query).map_err(GhError::plain)?);
        self.data(&id, |slug, cancel| async move { queries::list_issues(&self.st.client, &slug, &q, &cancel).await }).await
    }

    /// `gh:issue`
    pub async fn issue(&self, repo_id: &Value, n: &Value) -> Res<Json> {
        let (id, n) = (Self::repo_id(repo_id)?, validate::parse_number(n).map_err(GhError::plain)?);
        self.data(&id, |slug, cancel| async move { queries::get_issue(&self.st.client, &slug, n, &cancel).await }).await
    }

    /// `gh:comments`
    pub async fn comments(&self, repo_id: &Value, n: &Value) -> Res<Json> {
        let (id, n) = (Self::repo_id(repo_id)?, validate::parse_number(n).map_err(GhError::plain)?);
        self.data(&id, |slug, cancel| async move { queries::get_comments(&self.st.client, &slug, n, &cancel).await }).await
    }

    /// `gh:filters`
    pub async fn filters(&self, repo_id: &Value) -> Res<Json> {
        let id = Self::repo_id(repo_id)?;
        self.data(&id, |slug, cancel| async move { queries::get_filters(&self.st.client, &slug, &cancel).await }).await
    }

    /// `gh:commitChecks`: 커밋 목록의 체크 요약(GraphQL 일괄). 로그인하지 않았으면 조회하지 않고 `available:false` 로 알린다
    /// (GraphQL 은 비로그인을 받지 않고, 비로그인 REST 한도(시간당 60)로 커밋마다 묻는 것은 감당할 수 없다)
    pub async fn commit_checks(&self, repo_id: &Value, shas: &Value) -> Res<Json> {
        let (id, shas) = (Self::repo_id(repo_id)?, validate::parse_check_shas(shas).map_err(GhError::plain)?);
        if self.st.token().is_none() {
            return Ok(checks::unavailable());
        }
        self.data(&id, |slug, cancel| async move {
            let states = checks::fetch_commit_checks(&self.st.client, &slug, &shas, &self.st.checks, &cancel).await?;
            Ok(obj(vec![("available", Json::Bool(true)), ("states", states)]))
        })
        .await
    }

    /// `gh:commitCheckRuns`
    pub async fn commit_check_runs(&self, repo_id: &Value, sha: &Value) -> Res<Json> {
        let (id, sha) = (Self::repo_id(repo_id)?, validate::parse_sha(sha).map_err(GhError::plain)?);
        self.data(&id, |slug, cancel| async move { checks::fetch_check_details(&self.st.client, &slug, &sha, &cancel).await }).await
    }

    /// `gh:runs`
    pub async fn runs(&self, repo_id: &Value, page: &Value) -> Res<Json> {
        let (id, page) = (Self::repo_id(repo_id)?, validate::parse_page(page).map_err(GhError::plain)?);
        self.data(&id, |slug, cancel| async move { checks::list_runs(&self.st.client, &slug, page, &cancel).await }).await
    }

    /// `gh:runJobs`
    pub async fn run_jobs(&self, repo_id: &Value, run_id: &Value) -> Res<Json> {
        let (id, run) = (Self::repo_id(repo_id)?, validate::parse_run_id(run_id).map_err(GhError::plain)?);
        self.data(&id, |slug, cancel| async move { checks::list_jobs(&self.st.client, &slug, run, &cancel).await }).await
    }

    /// `gh:avatars`: 작성자 아바타 해석. GitHub 저장소가 아니어도 noreply 이메일은 풀린다(그 경우 `github:false`).
    /// 사진은 보조 기능이라 어떤 실패(한도·네트워크·인증)도 던지지 않고 "지금은 모름"으로 돌려준다
    pub async fn avatars(&self, repo_id: &Value, request_id: &Value, authors: &Value) -> Res<Json> {
        let id = Self::repo_id(repo_id)?;
        let rid = validate::parse_id(request_id, "요청 ID").map_err(GhError::plain)?;
        let authors = validate::parse_avatar_authors(authors).map_err(GhError::plain)?;
        let cancel = self.st.lock().epoch.token.child_token();
        let _guard = self.register(&rid, &cancel);
        let slug: Option<Slug> = self.st.repos.gh_repo(&id).await.ok().flatten().and_then(|r| Slug::parse(&r.owner, &r.repo));
        // 고른 커밋이 push 안 된 것이라 404 면, 같은 작성자의 push 된 커밋으로 다시 묻는다
        let fallback: Fallback = {
            let (repos, id, cancel) = (self.st.repos.clone(), id, cancel.clone());
            Arc::new(move |email: String| {
                let (repos, id, cancel) = (repos.clone(), id.clone(), cancel.clone());
                async move { repos.pushed_shas_by_author(&id, &email, FALLBACK_SHAS + 1, cancel).await.unwrap_or_default() }.boxed()
            })
        };
        let found = self.st.avatars.lookup_authors(slug.as_ref(), &authors, &cancel, Some(fallback)).await;
        // 풀리지 않은 작성자가 남아 있을 때만 막힌 이유를 알린다(전부 풀렸으면 알릴 것이 없다)
        let unresolved = authors.iter().any(|a| !found.iter().any(|(e, _)| *e == a.email));
        let blocked = if unresolved && slug.is_some() { self.st.avatars.blocked_for(slug.as_ref()) } else { None };
        let mut o = vec![("github", Json::Bool(slug.is_some())), ("avatars", Json::Obj(found.into_iter().map(|(e, v)| (e, v.map_or(Json::Null, |a| a.to_json()))).collect()))];
        if let Some(b) = blocked {
            o.push(("blocked", b.to_json()));
        }
        Ok(obj(o))
    }

    /// `gh:launchpad`: 내 PR·리뷰 요청·할당 이슈. 로그인하지 않았으면 조회하지 않고 `needsLogin` 으로 알린다. 검색은 사용자 기준으로 보내고, 등록된 모든 저장소의 GitHub remote 와 맞춰 거른다.
    /// `fresh` 가 `true` 면 remote 목록 캐시를 먼저 버린다(`gh:launchpad` 핸들러의 `invalidateGhRemotes`)
    pub async fn launchpad(&self, request_id: &Value, fresh: &Value) -> Res<Json> {
        if *fresh == Value::Bool(true) {
            self.st.repos.invalidate_gh_remotes();
        }
        let rid = validate::parse_id(request_id, "요청 ID").map_err(GhError::plain)?;
        let (gen, cancel) = {
            let g = self.st.lock();
            (g.epoch.id, g.epoch.token.child_token())
        };
        let _guard = self.register(&rid, &cancel);
        let Some(token) = self.st.token() else { return Ok(launchpad::login_required(self.st.clock.now_ms())) };
        let ids = self.st.repos.repo_ids().await;
        let result = launchpad::fetch_launchpad(&self.st.client, &*self.st.repos, &ids, self.st.remote_timeout, &*self.st.clock, &cancel).await;
        self.revalidate_on_auth_error(&result, gen).await;
        result.map_err(|e| e.sealed(&github_secrets(&token)))
    }

    /// `gh:accessibleRepos`: 로그인한 사용자가 접근할 수 있는 저장소(본인·협업·소속 org). 로그인하지 않았으면 조회하지 않고 `needsLogin` 으로 알린다.
    /// 결과는 5분 메모리 캐시에 둔다(`fresh` 가 `true` 면 무시하고 다시 읽는다)
    pub async fn accessible_repos(&self, request_id: &Value, fresh: &Value) -> Res<Json> {
        let rid = validate::parse_id(request_id, "요청 ID").map_err(GhError::plain)?;
        let fresh = *fresh == Value::Bool(true);
        let (gen, cancel) = {
            let g = self.st.lock();
            (g.epoch.id, g.epoch.token.child_token())
        };
        let _guard = self.register(&rid, &cancel);
        let Some(token) = self.st.token() else { return Ok(queries::accessible_login_required(self.st.clock.now_ms())) };
        if !fresh {
            let hit = self.st.lock().accessible.clone();
            if let Some((result, _)) = hit.filter(|(_, at)| self.st.clock.now_ms() - at < ACCESSIBLE_TTL_MS) {
                return Ok(result);
            }
        }
        let result = queries::fetch_accessible_repos(&self.st.client, &*self.st.clock, &cancel).await;
        if let Ok(r) = &result {
            // 읽는 동안 계정이 바뀌었으면 캐시에 넣지 않는다
            let mut g = self.st.lock();
            if g.epoch.id == gen {
                g.accessible = Some((r.clone(), self.st.clock.now_ms()));
            }
        }
        self.revalidate_on_auth_error(&result, gen).await;
        result.map_err(|e| e.sealed(&github_secrets(&token)))
    }

    /// 조회가 401(`GH_AUTH`)로 끝났으면 `GET /user` 로 한 번 다시 확인한다(그것도 거부되면 토큰을 버린다)
    async fn revalidate_on_auth_error<T>(&self, result: &Res<T>, gen: u64) {
        if result.as_ref().err().is_some_and(|e| e.code == "GH_AUTH") {
            if let Some(p) = self.validate(gen) {
                p.await;
            }
        }
    }

    /// `gh:openExternal` 이 쓸 검증된 주소(`https://github.com` 의 것만). 아니면 `None`
    pub fn external_url(&self, raw: &Value) -> Option<String> {
        validate::allowed_external_url(raw)
    }

    // ── 요청 취소 ───────────────────────────────────────

    /// `req:cancel` 에서 호출 — 다른 서비스와 requestId 공간을 공유하므로 없는 id 는 무시한다
    pub fn cancel(&self, request_id: &str) {
        if let Some((_, t)) = self.st.lock().requests.remove(request_id) {
            t.cancel();
        }
    }

    pub fn dispose(&self) {
        self.st.avatars.flush();
        let mut g = self.st.lock();
        g.epoch.token.cancel();
        for (_, t) in g.requests.values() {
            t.cancel();
        }
        g.requests.clear();
    }
}
