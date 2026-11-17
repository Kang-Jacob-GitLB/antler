//! 커밋 작성자 → GitHub 계정(프로필 사진) 해석(`src/shared/avatar.ts` 의 순수 규칙 + `src/main/github/avatars.ts` 의 `AvatarResolver` 이식, Phase 4 PR 7).
//! noreply 이메일은 네트워크 없이 이메일에서 바로 읽고, 그 밖의 작성자는 그 작성자의 커밋 sha 하나로 GitHub 가 연결해 둔 계정을 조회한다 — 이메일은 GitHub 로 보내지 않는다.
//! 결과는 메모리 + 파일 캐시(`avatar-cache.json`)에 둔다.
//!
//! - 조회는 `tokio::spawn` 으로 돌고 `Shared` 로 같은 작성자의 동시 요청이 나눠 쓴다. 호출자가 떠나도 끝나고(세마포어 슬롯을 물고 멈추지 않는다) 호출 토큰이 끊기면 새 요청을 시작하지 않는다.
//! - 동시 3개(`Semaphore`), 호출 한도 여유 10 미만이면 멈춘다, 이번 실행의 조회 상한(익명 40·로그인 400), 한 요청에서 새로 조회하는 작성자는 60명까지.
//!
//! TS 와 다른 점: 파일 캐시의 키는 이메일의 **sha256**(TS 는 sha1)이다 — 이메일은 해시로만 저장된다. 그래서 Electron 판 캐시 파일은 읽지 않는다(키가 다르다).
//! TS 는 조회마다 `AbortSignal.any` 로 세대 신호를 합치지만 여기서는 호출 토큰이 이미 세대 토큰의 자식이다.
use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use futures_util::future::{join_all, BoxFuture, FutureExt, Shared};
use sha2::{Digest, Sha256};
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

use super::client::{GetOptions, GitHubClient, TokenChoice};
use super::queries::Slug;
use super::validate::AvatarQuery;
use crate::git::commit::utf16_len;
use crate::git::fetch::{js_to_lowercase, js_trim};
use crate::git::scope::is_js_space;
use crate::jira::service::Clock;
use crate::net::http::NetError;
use crate::net::js_value::{encode_uri_component, num, obj, s};
use crate::net::json::Json;
use crate::net::url_rules::{has_credentials, js_origin, parse_url};

pub const AVATAR_ORIGIN: &str = "https://avatars.githubusercontent.com";
pub const AVATAR_CACHE_FILE: &str = "avatar-cache.json";
const DAY_MS: f64 = 86_400_000.0;
/// 계정이 확인된 항목 / "GitHub 계정과 연결되지 않음" 항목의 유효 기간
const POSITIVE_TTL_MS: f64 = 30.0 * DAY_MS;
const NEGATIVE_TTL_MS: f64 = 7.0 * DAY_MS;
/// 파일 캐시 최대 항목 수(넘으면 오래된 것부터 버린다)
const MAX_ENTRIES: usize = 5000;
/// 남은 호출이 이보다 적으면 한도가 풀릴 때까지 해석을 멈춘다(PR·이슈 화면이 쓸 몫을 남긴다)
const MIN_REMAINING: f64 = 10.0;
const CONCURRENCY: usize = 3;
/// 앱을 켠 동안 보낼 수 있는 조회 수: 익명은 시간당 60회뿐이라 작게, 로그인하면 넉넉히
const SESSION_CAP_ANON: u32 = 40;
const SESSION_CAP_AUTH: u32 = 400;
/// 한 번의 요청에서 새로 조회할 작성자 수
const PER_CALL_CAP: usize = 60;
/// 미푸시 커밋 등 GitHub 가 모르는 sha(404/422) / 네트워크 실패 후 같은 작성자를 다시 묻기까지의 간격
const UNKNOWN_SHA_RETRY_MS: f64 = 10.0 * 60_000.0;
const NETWORK_RETRY_MS: f64 = 60_000.0;
const SAVE_DELAY: Duration = Duration::from_millis(2000);
/// 비로그인에서 성공 없이 404 가 이만큼 쌓이면 비공개 저장소로 본다
const ANON_NOT_FOUND_LIMIT: u32 = 3;
/// 고른 커밋이 404(push 안 됨)일 때 같은 작성자의 push 된 커밋으로 더 시도하는 횟수
pub const FALLBACK_SHAS: usize = 2;
const SIZE_BUCKETS: [u32; 6] = [32, 48, 64, 96, 128, 192];
const NOREPLY_SUFFIX: &str = "@users.noreply.github.com";

// ── 순수 규칙(`src/shared/avatar.ts`) ──────────────────────────────────────

/// `LOGIN_RE`: `^[A-Za-z0-9](?:[A-Za-z0-9-]{0,38})(?:\[bot\])?$`
pub fn is_login(t: &str) -> bool {
    let body = t.strip_suffix("[bot]").unwrap_or(t);
    let b = body.as_bytes();
    (1..=39).contains(&b.len()) && b[0].is_ascii_alphanumeric() && b[1..].iter().all(|c| c.is_ascii_alphanumeric() || *c == b'-')
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Noreply {
    pub login: String,
    /// 사용자 숫자 ID(`<id>+<login>@…` 형식일 때만)
    pub id: Option<u64>,
}

/// `parseNoreply`: `<id>+<login>@users.noreply.github.com` · `<login>@users.noreply.github.com`(대소문자 무시). 로그인 형식이 아니거나 해석할 수 없으면 `None`.
/// `^(?:(\d{1,12})\+)?([^@+\s]{1,50})@users\.noreply\.github\.com$` 를 `i` 로
pub fn parse_noreply(email: &str) -> Option<Noreply> {
    let t = js_trim(email);
    let split = t.len().checked_sub(NOREPLY_SUFFIX.len())?;
    let (head, tail) = (t.get(..split)?, t.get(split..)?);
    if !tail.eq_ignore_ascii_case(NOREPLY_SUFFIX) {
        return None;
    }
    let (id_text, login) = match head.split_once('+') {
        Some((a, b)) => (Some(a), b),
        None => (None, head),
    };
    if id_text.is_some_and(|a| !(1..=12).contains(&a.len()) || !a.bytes().all(|c| c.is_ascii_digit())) {
        return None;
    }
    if !(1..=50).contains(&utf16_len(login)) || login.chars().any(|c| c == '@' || c == '+' || is_js_space(c)) || !is_login(login) {
        return None;
    }
    let id = id_text.and_then(|a| a.parse::<u64>().ok());
    // 봇(`name[bot]`)은 로그인으로는 이미지 주소를 만들 수 없어 ID 가 있어야 한다
    if id.is_none() && login.ends_with("[bot]") {
        return None;
    }
    Some(Noreply { login: login.to_string(), id: id.filter(|i| *i > 0) })
}

/// 크기(`s=`)를 붙이기 전의 아바타 기본 주소. ID 가 있으면 `/u/<id>`, 없으면 `/<login>`
pub fn avatar_base_url(login: &str, id: Option<u64>) -> String {
    match id {
        Some(i) => format!("{AVATAR_ORIGIN}/u/{i}"),
        None => format!("{AVATAR_ORIGIN}/{}", encode_uri_component(login)),
    }
}

/// GitHub API 의 `avatar_url` 을 기본 주소로 정리한다(쿼리 제거, 호스트 고정). 다른 호스트·형식이면 `None`. 숫자 ID 를 알면 그것으로 `/u/<id>` 를 만든다
pub fn normalize_avatar_url(url: Option<&str>, login: &str, id: Option<u64>) -> Option<String> {
    let url = url.filter(|u| utf16_len(u) <= 300)?;
    let u = parse_url(url)?;
    if js_origin(&u) != AVATAR_ORIGIN || has_credentials(&u) {
        return None;
    }
    let from_path = u.path().strip_prefix("/u/").filter(|d| (1..=12).contains(&d.len()) && d.bytes().all(|c| c.is_ascii_digit())).and_then(|d| d.parse::<u64>().ok());
    Some(match id.or(from_path) {
        Some(i) => avatar_base_url(login, Some(i)),
        None => format!("{AVATAR_ORIGIN}{}", u.path()),
    })
}

/// 요청 픽셀 크기를 몇 단계로 묶는다 — 같은 작성자의 이미지를 크기마다 따로 받지 않도록
pub fn avatar_px(css_px: f64, dpr: f64) -> u32 {
    let need = (css_px * dpr.max(1.0)).ceil();
    SIZE_BUCKETS.iter().copied().find(|b| f64::from(*b) >= need).unwrap_or(SIZE_BUCKETS[SIZE_BUCKETS.len() - 1])
}

/// 기본 주소 + 크기 → 이미지 src
pub fn avatar_src(base: &str, px: u32) -> String {
    format!("{base}{}s={px}", if base.contains('?') { '&' } else { '?' })
}

// ── 해석기 ────────────────────────────────────────────────────────────────

/// 해석된 계정
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Avatar {
    pub login: String,
    pub url: String,
}

impl Avatar {
    pub fn to_json(&self) -> Json {
        obj(vec![("login", s(&self.login)), ("url", s(&self.url))])
    }
}

/// 사진 조회가 막혀 있는 이유
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AvatarBlock {
    /// `rate-limited` | `needs-login` | `session-cap`
    pub reason: &'static str,
    /// epoch seconds (`rate-limited`)
    pub reset_at: Option<u64>,
    pub signed_in: bool,
}

impl AvatarBlock {
    pub fn to_json(&self) -> Json {
        let mut o = vec![("reason", s(self.reason))];
        if let Some(r) = self.reset_at {
            o.push(("resetAt", num(r as f64)));
        }
        o.push(("signedIn", Json::Bool(self.signed_in)));
        obj(o)
    }
}

/// 404 때 같은 작성자의 다른(push 된) 커밋 sha 를 찾아 주는 함수. 이메일은 GitHub 로 보내지 않고 로컬 git 에서만 쓴다
pub type Fallback = Arc<dyn Fn(String) -> BoxFuture<'static, Vec<String>> + Send + Sync>;

type SharedUnit = Shared<BoxFuture<'static, ()>>;

struct Entry {
    /// 해석된 계정(`None` = GitHub 계정과 연결되지 않음)
    v: Option<Avatar>,
    /// epoch ms
    t: f64,
}

#[derive(Default)]
struct Probe {
    not_found: u32,
    ok: u32,
    checking: bool,
    private: bool,
}

#[derive(Default)]
struct St {
    loaded: bool,
    entries: HashMap<String, Entry>,
    /// 메모리에만 두는 "나중에 다시" 표시(key → 다시 물어도 되는 시각)
    retry_at: HashMap<String, f64>,
    inflight: HashMap<String, SharedUnit>,
    paused_until: f64,
    /// `paused_until` 이 호출 한도 소진 때문인지(그렇다면 안내에 풀리는 시각을 보인다)
    paused_by_rate: bool,
    /// 비로그인 조회 결과를 저장소별로 센다: 404 만 쌓이고 성공이 없으면 비공개 저장소다(메모리에만 — 파일 캐시에 부정 결과로 남기지 않는다)
    anon_probe: HashMap<String, Probe>,
    /// 이번 실행에서 보낸 API 호출 수
    calls: u32,
    dirty: bool,
    save_scheduled: bool,
}

struct Core {
    st: Mutex<St>,
    client: Arc<GitHubClient>,
    cache_file: Option<PathBuf>,
    clock: Arc<dyn Clock>,
    slots: Semaphore,
}

#[derive(Clone)]
pub struct AvatarResolver {
    inner: Arc<Core>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Lookup {
    Resolved,
    UnknownSha,
    Other,
}

/// 이메일(앞뒤 공백 제거·소문자)의 sha256 16진수 — 파일 캐시에는 이것만 남는다
fn key_of(email: &str) -> String {
    Sha256::digest(js_to_lowercase(js_trim(email)).as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

fn is_key(k: &str) -> bool {
    k.len() == 64 && k.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn ttl_of(e: &Entry) -> f64 {
    if e.v.is_some() {
        POSITIVE_TTL_MS
    } else {
        NEGATIVE_TTL_MS
    }
}

fn slug_key(slug: &Slug) -> String {
    js_to_lowercase(&format!("{}/{}", slug.owner(), slug.repo()))
}

fn commits_path(slug: &Slug) -> String {
    format!("/repos/{}/{}/commits", encode_uri_component(slug.owner()), encode_uri_component(slug.repo()))
}

impl AvatarResolver {
    pub fn new(client: Arc<GitHubClient>, cache_file: Option<PathBuf>, clock: Arc<dyn Clock>) -> Self {
        Self { inner: Arc::new(Core { st: Mutex::new(St::default()), client, cache_file, clock, slots: Semaphore::new(CONCURRENCY) }) }
    }

    fn st(&self) -> MutexGuard<'_, St> {
        self.inner.st.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn now(&self) -> f64 {
        self.inner.clock.now_ms()
    }

    /// 이번 실행에서 보낸 API 호출 수(진단·시험용)
    pub fn calls(&self) -> u32 {
        self.st().calls
    }

    /// 작성자를 해석한다. 결과에 없는 이메일은 "지금은 모름"(한도·네트워크·GitHub 가 모르는 커밋)이라 나중에 다시 물어도 된다.
    /// `slug` 가 `None`(GitHub 저장소 아님)이면 noreply 이메일과 캐시만 쓴다. 결과의 순서는 TS 객체의 삽입 순서와 같다(noreply·캐시 적중 → 새로 안 것)
    pub async fn lookup_authors(&self, slug: Option<&Slug>, authors: &[AvatarQuery], cancel: &CancellationToken, fallback: Option<Fallback>) -> Vec<(String, Option<Avatar>)> {
        self.load();
        let signed_in = self.inner.client.has_token();
        let mut out: Vec<(String, Option<Avatar>)> = Vec::new();
        let mut pending: Vec<SharedUnit> = Vec::new();
        let mut started = 0usize;
        let now = self.now();
        for a in authors {
            if let Some(n) = parse_noreply(&a.email) {
                out.push((a.email.clone(), Some(Avatar { url: avatar_base_url(&n.login, n.id), login: n.login })));
                continue;
            }
            let key = key_of(&a.email);
            let step = {
                let g = self.st();
                if let Some(e) = g.entries.get(&key).filter(|e| now - e.t <= ttl_of(e)) {
                    Err(Some(e.v.clone()))
                } else if slug.is_none() || g.retry_at.get(&key).copied().unwrap_or(0.0) > now || Self::needs_login_locked(&g, slug, signed_in) {
                    Err(None)
                } else {
                    Ok(g.inflight.get(&key).cloned())
                }
            };
            match (step, slug) {
                (Err(Some(v)), _) => out.push((a.email.clone(), v)),
                (Err(None), _) | (Ok(None), None) => {}
                (Ok(Some(p)), _) => pending.push(p),
                (Ok(None), Some(slug)) if started < PER_CALL_CAP => {
                    started += 1;
                    pending.push(self.start_lookup(slug, a, key, cancel, fallback.clone()));
                }
                (Ok(None), Some(_)) => {}
            }
        }
        join_all(pending).await;

        // 이번 요청에서 새로 알게 된 것(다른 요청의 조회를 기다린 것 포함)을 모은다
        let after = self.now();
        for a in authors {
            if out.iter().any(|(e, _)| *e == a.email) {
                continue;
            }
            let g = self.st();
            if let Some(e) = g.entries.get(&key_of(&a.email)).filter(|e| after - e.t <= ttl_of(e)) {
                out.push((a.email.clone(), e.v.clone()));
            }
        }
        out
    }

    /// 같은 작성자의 조회가 이미 돌고 있으면 그것을, 아니면 새로 시작한다(시작 즉시 등록해 동시 요청이 중복 조회하지 않게 한다)
    fn start_lookup(&self, slug: &Slug, a: &AvatarQuery, key: String, cancel: &CancellationToken, fallback: Option<Fallback>) -> SharedUnit {
        let mut g = self.st();
        if let Some(p) = g.inflight.get(&key) {
            return p.clone();
        }
        let (this, slug, a, k, cancel) = (self.clone(), slug.clone(), a.clone(), key.clone(), cancel.clone());
        let handle = tokio::spawn(async move {
            this.limited(&cancel, async { this.lookup_with_fallback(&slug, &a, &k, &cancel, fallback.as_ref()).await }).await;
            this.st().inflight.remove(&k);
        });
        let shared = async move {
            let _ = handle.await;
        }
        .boxed()
        .shared();
        g.inflight.insert(key, shared.clone());
        shared
    }

    /// 모든 요청이 공유하는 동시성 제한(`CONCURRENCY`). 슬롯을 받은 뒤 취소·한도를 다시 확인하고 실행한다
    async fn limited(&self, cancel: &CancellationToken, f: impl Future<Output = ()>) {
        let permit = tokio::select! {
            biased;
            () = cancel.cancelled() => return,
            p = self.inner.slots.acquire() => p,
        };
        let Ok(_permit) = permit else { return };
        if cancel.is_cancelled() || !self.can_call() {
            return;
        }
        f.await;
    }

    /// 비로그인인데 이 저장소의 조회가 계속 404 — 비공개 저장소로 보고 로그인 전까지 더 묻지 않는다(404 도 한도를 쓴다)
    fn needs_login_locked(g: &St, slug: Option<&Slug>, signed_in: bool) -> bool {
        !signed_in && slug.is_some_and(|sl| g.anon_probe.get(&slug_key(sl)).is_some_and(|p| p.private))
    }

    /// 사진 조회가 막혀 있는 이유(막혀 있지 않으면 `None`). 화면이 안내를 띄우는 데 쓴다. 한도 소진 → 비공개 저장소(로그인 필요) → 이번 실행의 조회 상한 순으로 본다
    pub fn blocked_for(&self, slug: Option<&Slug>) -> Option<AvatarBlock> {
        let now = self.now();
        let signed_in = self.inner.client.has_token();
        let rate = self.inner.client.rate();
        let g = self.st();
        if now < g.paused_until && g.paused_by_rate {
            return Some(AvatarBlock { reason: "rate-limited", reset_at: Some((g.paused_until / 1000.0).ceil() as u64), signed_in });
        }
        if let Some(r) = rate.filter(|r| r.remaining < MIN_REMAINING && now < r.reset * 1000.0) {
            return Some(AvatarBlock { reason: "rate-limited", reset_at: Some(r.reset as u64), signed_in });
        }
        if Self::needs_login_locked(&g, slug, signed_in) {
            return Some(AvatarBlock { reason: "needs-login", reset_at: None, signed_in });
        }
        (g.calls >= if signed_in { SESSION_CAP_AUTH } else { SESSION_CAP_ANON }).then_some(AvatarBlock { reason: "session-cap", reset_at: None, signed_in })
    }

    /// 로그인·로그아웃으로 조건이 달라졌을 때: 메모리의 "나중에/로그인 필요/한도" 표시와 세션 호출 수를 비운다. 확인된 결과(`entries`)는 남긴다
    pub fn reset_session(&self) {
        let mut g = self.st();
        g.retry_at.clear();
        g.anon_probe.clear();
        g.paused_until = 0.0;
        g.paused_by_rate = false;
        g.calls = 0;
    }

    /// 호출해도 되는 상태인지: 한도 여유·일시 중지·세션 상한
    fn can_call(&self) -> bool {
        let now = self.now();
        let signed_in = self.inner.client.has_token();
        let rate = self.inner.client.rate();
        let mut g = self.st();
        if now < g.paused_until {
            return false;
        }
        if let Some(r) = rate.filter(|r| r.remaining < MIN_REMAINING && now < r.reset * 1000.0) {
            g.paused_until = r.reset * 1000.0;
            g.paused_by_rate = true;
            return false;
        }
        g.calls < if signed_in { SESSION_CAP_AUTH } else { SESSION_CAP_ANON }
    }

    /// 고른 커밋으로 묻고, GitHub 가 모르는 커밋(404 — 로컬 브랜치에만 있는 커밋)이면 같은 작성자의 push 된 커밋으로 `FALLBACK_SHAS` 번까지 다시 묻는다.
    /// 비공개 저장소를 비로그인으로 묻는 경우처럼 저장소 자체가 막혔으면 더 묻지 않는다
    async fn lookup_with_fallback(&self, slug: &Slug, a: &AvatarQuery, key: &str, cancel: &CancellationToken, fallback: Option<&Fallback>) {
        let first = js_to_lowercase(&a.sha);
        if self.lookup(slug, &first, key, cancel).await != Lookup::UnknownSha {
            return;
        }
        let Some(fallback) = fallback else { return };
        let candidates: Vec<String> = fallback(a.email.clone()).await.into_iter().filter(|s| *s != first).take(FALLBACK_SHAS).collect();
        for sha in candidates {
            if cancel.is_cancelled() || !self.can_call() {
                return;
            }
            let (signed_in, private) = (self.inner.client.has_token(), self.st().anon_probe.get(&slug_key(slug)).is_some_and(|p| p.private));
            if !signed_in && private {
                return;
            }
            if self.lookup(slug, &sha, key, cancel).await != Lookup::UnknownSha {
                return;
            }
        }
    }

    /// sha 한 건 조회: 그 커밋 한 개만 가져오는 목록 호출(`GET /commits?sha=<sha>&per_page=1`)로 author 를 읽는다.
    /// `GET /commits/{sha}` 는 변경 파일 목록까지 실어 응답이 커질 수 있어 이쪽을 쓴다
    async fn lookup(&self, slug: &Slug, sha: &str, key: &str, cancel: &CancellationToken) -> Lookup {
        let sk = slug_key(slug);
        let signed_in = self.inner.client.has_token();
        {
            // 같은 묶음에서 먼저 간 조회들이 이 저장소를 비공개로 판정했다면, 이미 대기열에 들어온 나머지도 보내지 않는다
            let mut g = self.st();
            if !signed_in && g.anon_probe.get(&sk).is_some_and(|p| p.private) {
                g.retry_at.insert(key.to_string(), self.now() + UNKNOWN_SHA_RETRY_MS);
                return Lookup::Other;
            }
            g.calls += 1;
        }
        let path = commits_path(slug);
        let query = [("sha", Some(sha)), ("per_page", Some("1"))];
        let result = self.inner.client.get(&path, &TokenChoice::Stored, GetOptions { query: &query, cancel: Some(cancel), ..Default::default() }).await;
        let res = match result {
            Ok(r) => r,
            Err(NetError::Aborted) => return Lookup::Other,
            Err(NetError::Api { code, status, .. }) => return self.after_failure(slug, code, status, key, cancel).await,
            Err(NetError::InvalidPath) => {
                self.st().retry_at.insert(key.to_string(), self.now() + NETWORK_RETRY_MS);
                return Lookup::Other;
            }
        };
        let first = match &res.data {
            Json::Arr(a) => a.first(),
            _ => None,
        };
        let Some(c) = first.filter(|c| c.get("sha").and_then(Json::as_str).is_some_and(|t| js_to_lowercase(t) == sha)) else {
            self.st().retry_at.insert(key.to_string(), self.now() + NETWORK_RETRY_MS);
            return Lookup::Other;
        };
        if !signed_in {
            self.st().anon_probe.entry(sk).or_default().ok += 1;
        }
        let who = match c.get("author") {
            None | Some(Json::Null | Json::Bool(false)) => None,
            Some(a) => a.get("login").and_then(Json::as_str).filter(|l| is_login(l)).map(|login| {
                let id = a.get("id").and_then(Json::as_f64).filter(|n| n.fract() == 0.0 && *n > 0.0 && *n < 9_007_199_254_740_992.0).map(|n| n as u64);
                let url = normalize_avatar_url(a.get("avatar_url").and_then(Json::as_str), login, id).unwrap_or_else(|| avatar_base_url(login, id));
                Avatar { login: login.to_string(), url }
            }),
        };
        self.store(key, who);
        Lookup::Resolved
    }

    /// 요청이 실패한 뒤의 분류: 한도 → 일시 중지, 인증 → 잠깐 중지, 404/422/409 → 다른 커밋으로 다시 시도할 수 있음, 그 밖 → 잠시 뒤 다시
    async fn after_failure(&self, slug: &Slug, code: &'static str, status: Option<u16>, key: &str, cancel: &CancellationToken) -> Lookup {
        let now = self.now();
        let reset = self.inner.client.rate().map(|r| r.reset).filter(|r| *r != 0.0);
        let unknown_sha = code == "GH_NOT_FOUND" || status == Some(422) || status == Some(409);
        {
            let mut g = self.st();
            match code {
                "GH_RATE_LIMIT" => {
                    g.paused_until = (now + NETWORK_RETRY_MS).max(reset.map_or(0.0, |r| r * 1000.0));
                    g.paused_by_rate = true;
                }
                "GH_AUTH" => {
                    g.paused_until = now + NETWORK_RETRY_MS;
                    g.paused_by_rate = false;
                }
                // 아직 push 하지 않은 커밋 등 — 다른 커밋으로 다시 시도할 수 있도록 기록은 남기지 않는다
                _ if unknown_sha => {
                    g.retry_at.insert(key.to_string(), now + UNKNOWN_SHA_RETRY_MS);
                }
                _ => {
                    g.retry_at.insert(key.to_string(), now + NETWORK_RETRY_MS);
                }
            }
        }
        if code == "GH_RATE_LIMIT" || code == "GH_AUTH" {
            return Lookup::Other;
        }
        if unknown_sha {
            if code == "GH_NOT_FOUND" {
                self.probe_not_found(slug, cancel).await;
            }
            return Lookup::UnknownSha;
        }
        Lookup::Other
    }

    /// 비로그인 404 를 센다. 404 만 쌓였다고 비공개로 단정하지 않는다(공개 저장소의 미푸시 커밋도 404) — 저장소 자체를 한 번 확인해 404 면 비공개다
    async fn probe_not_found(&self, slug: &Slug, cancel: &CancellationToken) {
        if self.inner.client.has_token() {
            return;
        }
        let sk = slug_key(slug);
        {
            let mut g = self.st();
            let p = g.anon_probe.entry(sk.clone()).or_default();
            p.not_found += 1;
            if !(p.ok == 0 && p.not_found >= ANON_NOT_FOUND_LIMIT && !p.checking && !p.private) {
                return;
            }
            p.checking = true;
            g.calls += 1;
        }
        let path = format!("/repos/{}/{}", encode_uri_component(slug.owner()), encode_uri_component(slug.repo()));
        let result = self.inner.client.get(&path, &TokenChoice::Stored, GetOptions { cancel: Some(cancel), ..Default::default() }).await;
        let mut g = self.st();
        let p = g.anon_probe.entry(sk).or_default();
        match result {
            Ok(_) => p.ok += 1,
            Err(NetError::Api { code: "GH_NOT_FOUND", .. }) => p.private = true,
            Err(_) => p.checking = false,
        }
    }

    fn store(&self, key: &str, v: Option<Avatar>) {
        let now = self.now();
        let mut g = self.st();
        g.entries.insert(key.to_string(), Entry { v, t: now });
        g.retry_at.remove(key);
        self.schedule_save(&mut g);
    }

    // ── 파일 캐시 ───────────────────────────────────────

    fn load(&self) {
        {
            let mut g = self.st();
            if g.loaded {
                return;
            }
            g.loaded = true;
        }
        let Some(file) = &self.inner.cache_file else { return };
        // 없거나 깨진 캐시는 비어 있는 것으로 본다
        let Ok(text) = std::fs::read_to_string(file) else { return };
        let Ok(raw) = serde_json::from_str::<serde_json::Value>(&text) else { return };
        if raw.get("v").and_then(serde_json::Value::as_u64) != Some(1) {
            return;
        }
        let Some(map) = raw.get("e").and_then(serde_json::Value::as_object) else { return };
        let mut g = self.st();
        for (key, e) in map {
            let Some(t) = e.get("t").and_then(serde_json::Value::as_f64).filter(|_| is_key(key)) else { continue };
            match e.get("l") {
                Some(serde_json::Value::Null) => {
                    g.entries.insert(key.clone(), Entry { v: None, t });
                }
                Some(serde_json::Value::String(login)) if is_login(login) => {
                    let id = e.get("i").and_then(serde_json::Value::as_u64).filter(|i| *i > 0 && *i < 9_007_199_254_740_992);
                    if let Some(url) = normalize_avatar_url(e.get("u").and_then(serde_json::Value::as_str), login, id) {
                        g.entries.insert(key.clone(), Entry { v: Some(Avatar { login: login.clone(), url }), t });
                    }
                }
                _ => {}
            }
        }
    }

    /// 저장을 2초 미룬다(한 번에 모아 쓴다). 런타임이 없는 곳에서는 미루지 않고 건너뛴다(`flush` 가 종료 때 쓴다)
    fn schedule_save(&self, g: &mut St) {
        if self.inner.cache_file.is_none() {
            return;
        }
        g.dirty = true;
        if g.save_scheduled || tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        g.save_scheduled = true;
        let this = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(SAVE_DELAY).await;
            this.flush();
        });
    }

    /// 대기 중인 캐시 쓰기를 바로 끝낸다(종료 시 호출)
    pub fn flush(&self) {
        let Some(file) = &self.inner.cache_file else { return };
        let now = self.now();
        let json = {
            let mut g = self.st();
            g.save_scheduled = false;
            if !g.dirty {
                return;
            }
            g.dirty = false;
            let mut live: Vec<(&String, &Entry)> = g.entries.iter().filter(|(_, e)| now - e.t <= ttl_of(e)).collect();
            live.sort_by(|a, b| b.1.t.total_cmp(&a.1.t));
            let mut e = serde_json::Map::new();
            for (key, v) in live.into_iter().take(MAX_ENTRIES) {
                let item = match &v.v {
                    None => serde_json::json!({"l": null, "t": v.t}),
                    Some(a) => {
                        let id = parse_url(&a.url).and_then(|u| u.path().strip_prefix("/u/").and_then(|d| d.parse::<u64>().ok()));
                        match id {
                            Some(i) => serde_json::json!({"l": a.login, "i": i, "u": a.url, "t": v.t}),
                            None => serde_json::json!({"l": a.login, "u": a.url, "t": v.t}),
                        }
                    }
                };
                e.insert(key.clone(), item);
            }
            serde_json::json!({"v": 1, "e": e}).to_string()
        };
        // 캐시는 부가 기능이라 쓰기 실패는 무시한다. 임시 파일에 쓴 뒤 rename — 쓰는 도중 끊겨도 기존 캐시가 깨지지 않는다
        if let Some(dir) = file.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let tmp = file.with_extension(format!("json.{}.tmp", std::process::id()));
        if std::fs::write(&tmp, json).is_ok() && std::fs::rename(&tmp, file).is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
    }
}
