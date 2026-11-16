//! GitHub REST 클라이언트(`src/main/github/client.ts` 이식, PR 3 — 앱에 연결하지 않는다). 토큰은 [`TokenProvider`] 로만 읽고 요청 헤더에만 싣는다(반환값·오류 문구·로그에 넣지 않는다).
//! 오류 문구는 마지막에 `redact_secrets` 를 거친다.
//!
//! - ETag 캐시(500개·32MB): 키는 `sha256(토큰)[..16]\0주소\0Accept` 라 계정이 바뀌어도 다른 사용자의 비공개 응답이 섞이지 않는다. 304 는 한도에 들어가지 않아 같은 요청을 `If-None-Match` 로 다시 보낸다.
//! - 비로그인(시간당 60회)은 방금 받은 응답을 3분 동안 네트워크 없이 쓴다. 로그인하면 이 단축은 쓰지 않는다.
//! - core 한도를 추적한다(search 등 다른 resource 는 덮어쓰지 않는다).
//!
//! TS 와 다른 점: 리다이렉트는 origin 이 정확히 `https://api.github.com` 인 주소로만, 5번까지만 따라간다(TS 는 `fetch` 기본 — 전부 따른다). 응답 본문은 32MB 까지만 받고
//! (TS 는 상한이 없다) 넘으면 `GH_HTTP`("GitHub 응답이 너무 큽니다"). 네트워크 오류 문구에 원인 문자열을 붙이지 않는다. 한도 해제 시각의 표시는 시간대 크레이트 없이는 ko-KR 로컬 시각을 만들 수 없어
//! 생성자 주입이다([`GitHubClient::with_format_time`] — 기본은 UTC `HH:MM`).
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, AUTHORIZATION, CONTENT_TYPE, ETAG, IF_NONE_MATCH, LINK, RETRY_AFTER};
use reqwest::{Method, Response};
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

use crate::git::lru::LruCache;
use crate::net::http::{classify, header, or_abort, read_capped, Failure, HttpClient, HttpInitError, NetError, Profile, ReadError, Redirects, Rewrite, REQUEST_TIMEOUT_SECS};
use crate::net::http_error::{github_http_error, has_next_link, parse_rate, RateHeaders, RateInfo};
use crate::net::js_value::truthy;
use crate::net::json::{self, Json};
use crate::net::redact::{github_secrets, SecretString};
use crate::net::url_rules::{github_request_url, GITHUB_API_BASE};

pub const API_BASE: &str = GITHUB_API_BASE;
pub const API_VERSION: &str = "2022-11-28";
const GRAPHQL_PATH: &str = "/graphql";
const ACCEPT_JSON: &str = "application/vnd.github+json";
/// `body_html`(GitHub 가 렌더링·정화한 본문)을 함께 받는다
const ACCEPT_HTML: &str = "application/vnd.github.html+json";
/// 응답 본문 상한(TS 에는 없다 — 알려진 차이)
pub const MAX_BODY_BYTES: usize = 32 * 1024 * 1024;
const CACHE_ENTRIES: usize = 500;
const CACHE_BYTES: u64 = 32 * 1024 * 1024;
/// 비로그인에서 방금 받은 응답을 네트워크 없이 그대로 쓰는 시간
pub const ANON_FRESH: Duration = Duration::from_secs(3 * 60);
/// 리다이렉트를 따라가는 최대 횟수
pub const MAX_REDIRECTS: usize = 5;
pub const GH_TOO_BIG: &str = "GitHub 응답이 너무 큽니다";

/// 저장된 토큰을 돌려주는 함수(없으면 `None`)
pub type TokenProvider = Arc<dyn Fn() -> Option<SecretString> + Send + Sync>;

/// 요청에 쓸 토큰(TS `GetOptions.token`)
#[derive(Clone, Debug, Default)]
pub enum TokenChoice {
    /// 저장된 토큰(`TokenProvider`)
    #[default]
    Stored,
    /// 익명(TS 의 `null`)
    Anonymous,
    /// 이 토큰으로(로그인 전 검증용)
    Explicit(SecretString),
}

#[derive(Clone, Copy, Default)]
pub struct GetOptions<'a> {
    /// 쿼리(값이 `None` 이면 건너뛴다)
    pub query: &'a [(&'a str, Option<&'a str>)],
    /// true 면 `application/vnd.github.html+json` 으로 받아 `body_html` 을 얻는다
    pub html: bool,
    pub cancel: Option<&'a CancellationToken>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GetResult {
    pub data: Json,
    /// `Link` 헤더에 `rel="next"` 가 있는가
    pub has_more: bool,
}

struct CacheEntry {
    etag: String,
    data: Json,
    has_more: bool,
    /// 받은(또는 304 로 확인한) 시각
    at: Instant,
    /// 캐시 비용(추정 바이트)
    cost: u64,
}

type RateCallback = Box<dyn Fn(RateInfo) + Send + Sync>;

pub struct GitHubClient {
    http: HttpClient,
    token: TokenProvider,
    cache: Mutex<LruCache<Arc<CacheEntry>>>,
    rate: Mutex<Option<RateInfo>>,
    on_rate: Option<RateCallback>,
    format_time: Box<dyn Fn(f64) -> String + Send + Sync>,
    anon_fresh: Duration,
}

impl GitHubClient {
    pub fn new(token: TokenProvider) -> Result<Self, HttpInitError> {
        let http = HttpClient::production(Redirects::SameOrigin { origin: API_BASE.to_string(), max_hops: MAX_REDIRECTS })?;
        Ok(Self::with(http, token, ANON_FRESH))
    }

    /// 테스트 전용: `https://api.github.com` 으로 가는 요청을 `mock_base`(`http://127.0.0.1:포트`)로 보내고, 그 origin 으로의 리다이렉트만 따른다.
    /// 루프백 http 를 허용하고 제한 시간·비로그인 신선 시간을 줄일 수 있다. **운영 코드에서 부르지 않는다**
    #[doc(hidden)]
    pub fn new_for_test(token: TokenProvider, mock_base: &str, timeout: Duration, anon_fresh: Duration) -> Result<Self, HttpInitError> {
        let base = mock_base.trim_end_matches('/').to_string();
        let to = base.clone();
        let rewrite: Rewrite = Arc::new(move |url: &str| match url.strip_prefix(API_BASE) {
            Some(rest) => format!("{to}{rest}"),
            None => url.to_string(),
        });
        let http = HttpClient::build(Redirects::SameOrigin { origin: base, max_hops: MAX_REDIRECTS }, Profile::Test { timeout, rewrite: Some(rewrite) })?;
        Ok(Self::with(http, token, anon_fresh))
    }

    fn with(http: HttpClient, token: TokenProvider, anon_fresh: Duration) -> Self {
        Self {
            http,
            token,
            cache: Mutex::new(LruCache::new(CACHE_ENTRIES, CACHE_BYTES)),
            rate: Mutex::new(None),
            on_rate: None,
            format_time: Box::new(utc_hhmm),
            anon_fresh,
        }
    }

    /// core 한도가 바뀔 때마다 부른다
    #[must_use]
    pub fn with_on_rate(mut self, f: impl Fn(RateInfo) + Send + Sync + 'static) -> Self {
        self.on_rate = Some(Box::new(f));
        self
    }

    /// 한도가 풀리는 시각(epoch 초)의 표시(TS 는 ko-KR 로컬 시각)
    #[must_use]
    pub fn with_format_time(mut self, f: impl Fn(f64) -> String + Send + Sync + 'static) -> Self {
        self.format_time = Box::new(f);
        self
    }

    pub fn has_token(&self) -> bool {
        (self.token)().is_some()
    }

    pub fn clear_cache(&self) {
        *self.cache.lock().unwrap() = LruCache::new(CACHE_ENTRIES, CACHE_BYTES);
    }

    /// 마지막으로 본 core 한도(search 는 별도 한도라 섞지 않는다)
    pub fn rate(&self) -> Option<RateInfo> {
        *self.rate.lock().unwrap()
    }

    fn resolve_token(&self, choice: &TokenChoice) -> Option<SecretString> {
        match choice {
            TokenChoice::Stored => (self.token)(),
            TokenChoice::Anonymous => None,
            TokenChoice::Explicit(t) => Some(t.clone()),
        }
        .filter(|t| !t.is_empty())
    }

    /// REST GET. `path` 는 `/` 로 시작하는 API 경로(세그먼트는 호출자가 인코딩한다). ETag 캐시 키는 (토큰 해시 + 전체 주소 + Accept)
    pub async fn get(&self, path: &str, token: &TokenChoice, opts: GetOptions<'_>) -> Result<GetResult, NetError> {
        let token = self.resolve_token(token);
        let secrets = token.as_ref().map(github_secrets).unwrap_or_default();
        self.get_inner(path, token.as_ref(), opts).await.map_err(|e| e.sealed(&secrets))
    }

    async fn get_inner(&self, path: &str, token: Option<&SecretString>, opts: GetOptions<'_>) -> Result<GetResult, NetError> {
        let url = github_request_url(path, opts.query).ok_or(NetError::InvalidPath)?;
        let accept = if opts.html { ACCEPT_HTML } else { ACCEPT_JSON };
        let key = format!("{}\0{url}\0{accept}", token.map_or_else(|| "anon".to_string(), |t| token_hash(t.expose())));
        let cached = self.cache.lock().unwrap().get(&key);
        if let Some(c) = &cached {
            if token.is_none() && c.at.elapsed() < self.anon_fresh {
                return Ok(GetResult { data: c.data.clone(), has_more: c.has_more });
            }
        }

        let mut req = self.http.request(Method::GET, &url).header(ACCEPT, accept).header("X-GitHub-Api-Version", API_VERSION);
        if let Some(t) = token {
            req = req.header(AUTHORIZATION, bearer(t)?);
        }
        if let Some(c) = &cached {
            if let Ok(v) = HeaderValue::from_str(&c.etag) {
                req = req.header(IF_NONE_MATCH, v);
            }
        }
        let mut res = send(req, opts.cancel).await?;
        let status = res.status().as_u16();
        self.track_rate(res.headers());

        if status == 304 {
            let Some(c) = cached else { return Err(NetError::api("GH_HTTP", Some(304), "GitHub 가 예상치 못한 304 응답을 보냈습니다")) };
            let fresh = Arc::new(CacheEntry { etag: c.etag.clone(), data: c.data.clone(), has_more: c.has_more, at: Instant::now(), cost: c.cost });
            self.cache.lock().unwrap().set(&key, fresh, c.cost);
            return Ok(GetResult { data: c.data.clone(), has_more: c.has_more });
        }
        let bytes = read_body(&mut res, opts.cancel, status).await?;
        let text = String::from_utf8_lossy(&bytes);
        if !res.status().is_success() {
            return Err(self.http_error(&res, &text, token.is_some()));
        }
        let data = json::parse(&text).map_err(|_| NetError::api("GH_HTTP", Some(status), "GitHub 응답을 해석할 수 없습니다"))?;
        let has_more = has_next_link(header(res.headers(), LINK.as_str()));
        if let Some(etag) = header(res.headers(), ETAG.as_str()).filter(|e| !e.is_empty()) {
            let cost = text.len() as u64 * 2;
            let entry = Arc::new(CacheEntry { etag: etag.to_string(), data: data.clone(), has_more, at: Instant::now(), cost });
            self.cache.lock().unwrap().set(&key, entry, cost);
        }
        Ok(GetResult { data, has_more })
    }

    /// GraphQL 조회(읽기 전용 query 만 보낸다). 비로그인 요청은 받지 않으므로 토큰이 없으면 `GH_AUTH` 로 거절한다. ETag 캐시가 없어 호출자가 결과를 캐시한다.
    /// 부분 오류(errors 와 data 가 함께 오는 경우)는 data 를 그대로 돌려준다
    pub async fn graphql(&self, query: &str, variables: &[(&str, &str)], cancel: Option<&CancellationToken>) -> Result<Json, NetError> {
        let token = self.resolve_token(&TokenChoice::Stored);
        let secrets = token.as_ref().map(github_secrets).unwrap_or_default();
        self.graphql_inner(token.as_ref(), query, variables, cancel).await.map_err(|e| e.sealed(&secrets))
    }

    async fn graphql_inner(&self, token: Option<&SecretString>, query: &str, variables: &[(&str, &str)], cancel: Option<&CancellationToken>) -> Result<Json, NetError> {
        let Some(token) = token else { return Err(NetError::api("GH_AUTH", Some(401), "GraphQL 조회에는 GitHub 로그인이 필요합니다")) };
        let vars: serde_json::Map<String, serde_json::Value> = variables.iter().map(|(k, v)| ((*k).to_string(), serde_json::Value::String((*v).to_string()))).collect();
        let body = serde_json::json!({ "query": query, "variables": vars }).to_string();
        let req = self
            .http
            .request(Method::POST, &format!("{API_BASE}{GRAPHQL_PATH}"))
            .header(ACCEPT, ACCEPT_JSON)
            .header(CONTENT_TYPE, "application/json")
            .header(AUTHORIZATION, bearer(token)?)
            .body(body);
        let mut res = send(req, cancel).await?;
        let status = res.status().as_u16();
        let bytes = read_body(&mut res, cancel, status).await?;
        let text = String::from_utf8_lossy(&bytes);
        if !res.status().is_success() {
            return Err(self.http_error(&res, &text, true));
        }
        let body = json::parse(&text).map_err(|_| NetError::api("GH_HTTP", Some(status), "GitHub 응답을 해석할 수 없습니다"))?;
        let rate_limited = matches!(body.get("errors"), Some(Json::Arr(errs)) if errs.iter().any(|e| e.get("type").and_then(Json::as_str) == Some("RATE_LIMITED")));
        if rate_limited {
            return Err(NetError::api("GH_RATE_LIMIT", Some(status), "GitHub API 호출 한도를 초과했습니다. 잠시 후 다시 시도하세요"));
        }
        match body.get("data") {
            data @ Some(_) if truthy(data) => Ok(data.cloned().unwrap_or(Json::Null)),
            _ => Err(NetError::api("GH_HTTP", Some(status), "GitHub GraphQL 조회에 실패했습니다")),
        }
    }

    fn track_rate(&self, h: &HeaderMap) {
        // search 등 다른 resource 의 한도는 core 표시를 덮어쓰지 않는다
        if header(h, "x-ratelimit-resource").is_some_and(|r| !r.is_empty() && r != "core") {
            return;
        }
        let Some(rate) = parse_rate(&rate_headers(h)) else { return };
        *self.rate.lock().unwrap() = Some(rate);
        if let Some(f) = &self.on_rate {
            f(rate);
        }
    }

    fn http_error(&self, res: &Response, text: &str, signed_in: bool) -> NetError {
        let e = github_http_error(res.status().as_u16(), &rate_headers(res.headers()), text, signed_in, &*self.format_time);
        NetError::api(e.code, Some(e.status), e.message)
    }
}

fn rate_headers(h: &HeaderMap) -> RateHeaders<'_> {
    RateHeaders {
        remaining: header(h, "x-ratelimit-remaining"),
        limit: header(h, "x-ratelimit-limit"),
        reset: header(h, "x-ratelimit-reset"),
        retry_after: header(h, RETRY_AFTER.as_str()),
    }
}

/// `sha256(토큰)` 의 앞 16자(hex)
fn token_hash(token: &str) -> String {
    Sha256::digest(token.as_bytes()).iter().take(8).map(|b| format!("{b:02x}")).collect()
}

fn bearer(token: &SecretString) -> Result<HeaderValue, NetError> {
    let mut v = HeaderValue::from_str(&format!("Bearer {}", token.expose())).map_err(|_| connect_failed())?;
    v.set_sensitive(true);
    Ok(v)
}

fn utc_hhmm(epoch_sec: f64) -> String {
    let day_sec = (epoch_sec as i64).rem_euclid(86_400);
    format!("{:02}:{:02}(UTC)", day_sec / 3600, day_sec % 3600 / 60)
}

fn timed_out() -> NetError {
    NetError::api("GH_TIMEOUT", None, format!("GitHub 응답이 {REQUEST_TIMEOUT_SECS}초 안에 오지 않았습니다"))
}

fn connect_failed() -> NetError {
    NetError::api("GH_NETWORK", None, "GitHub 에 연결할 수 없습니다")
}

async fn send(req: reqwest::RequestBuilder, cancel: Option<&CancellationToken>) -> Result<Response, NetError> {
    match or_abort(cancel, req.send()).await? {
        Ok(r) => Ok(r),
        Err(e) => Err(match classify(&e) {
            Failure::Timeout => timed_out(),
            _ => connect_failed(),
        }),
    }
}

async fn read_body(res: &mut Response, cancel: Option<&CancellationToken>, status: u16) -> Result<Vec<u8>, NetError> {
    match or_abort(cancel, read_capped(res, MAX_BODY_BYTES)).await? {
        Ok(bytes) => Ok(bytes),
        Err(ReadError::TooBig) => Err(NetError::api("GH_HTTP", Some(status), GH_TOO_BIG)),
        Err(ReadError::Failed(Failure::Timeout)) => Err(timed_out()),
        Err(ReadError::Failed(_)) => Err(NetError::api("GH_NETWORK", Some(status), "GitHub 응답을 읽는 중 연결이 끊겼습니다")),
    }
}
