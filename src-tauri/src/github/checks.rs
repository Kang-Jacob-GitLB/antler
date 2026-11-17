//! GitHub Actions(체크) 상태 조회(`src/main/github/checks.ts` 의 호출부 + `src/shared/checkTtl.ts` 이식, Phase 4 PR 7): 커밋 목록용 일괄 요약(GraphQL),
//! 커밋 상세의 체크 목록, 워크플로 실행·잡 목록. 모두 읽기 전용이다. 응답 변환은 [`super::checks_map`](PR 2)이 맡는다.
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, Mutex};

use futures_util::future::join;
use tokio_util::sync::CancellationToken;

use super::checks_map::{self, CHECKS_BATCH};
use super::client::GitHubClient;
use super::error::{GhError, Res};
use super::queries::{get, list_json, Slug};
use crate::git::fetch::js_to_lowercase;
use crate::jira::service::Clock;
use crate::net::js_value::{encode_uri_component, obj, s};
use crate::net::json::Json;

const RUNS_PER_PAGE: usize = 30;
const CACHE_MAX: usize = 5000;
/// 끝난 상태(성공·실패·중립)는 오래, 진행 중은 짧게, 체크 없음(아직 push 안 된 커밋 포함)은 중간
pub const TTL_DONE_MS: f64 = 30.0 * 60_000.0;
pub const TTL_PENDING_MS: f64 = 30_000.0;
pub const TTL_NONE_MS: f64 = 2.0 * 60_000.0;

pub fn ttl_of(state: &str) -> f64 {
    match state {
        "pending" => TTL_PENDING_MS,
        "none" => TTL_NONE_MS,
        _ => TTL_DONE_MS,
    }
}

/// `^(?:[0-9a-f]{40}|[0-9a-f]{64})$`
fn is_sha(t: &str) -> bool {
    matches!(t.len(), 40 | 64) && t.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

struct CacheInner {
    /// 키 → (상태, 받은 시각, 순번)
    map: HashMap<String, (&'static str, f64, u64)>,
    /// 순번 → 키(가장 오래된 것부터 버린다)
    order: BTreeMap<u64, String>,
    next: u64,
}

/// (저장소, sha) → 체크 상태 캐시. 상태에 따라 수명이 다르다
pub struct CheckStateCache {
    inner: Mutex<CacheInner>,
    clock: Arc<dyn Clock>,
}

impl CheckStateCache {
    pub fn new(clock: Arc<dyn Clock>) -> Self {
        Self { inner: Mutex::new(CacheInner { map: HashMap::new(), order: BTreeMap::new(), next: 0 }), clock }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, CacheInner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn get(&self, key: &str) -> Option<&'static str> {
        let now = self.clock.now_ms();
        let mut g = self.lock();
        let (state, at, order) = *g.map.get(key)?;
        if now - at >= ttl_of(state) {
            g.map.remove(key);
            g.order.remove(&order);
            return None;
        }
        Some(state)
    }

    pub fn set(&self, key: &str, state: &'static str) {
        let now = self.clock.now_ms();
        let mut g = self.lock();
        // 이미 있는 키를 갱신할 때는 다른 항목을 밀어내지 않는다(맨 뒤로 옮기기만)
        if let Some((_, _, old)) = g.map.remove(key) {
            g.order.remove(&old);
        } else if g.map.len() >= CACHE_MAX {
            if let Some((_, oldest)) = g.order.pop_first() {
                g.map.remove(&oldest);
            }
        }
        let n = g.next;
        g.next += 1;
        g.map.insert(key.to_string(), (state, now, n));
        g.order.insert(n, key.to_string());
    }

    pub fn clear(&self) {
        let mut g = self.lock();
        g.map.clear();
        g.order.clear();
    }
}

/// 여러 커밋의 체크 요약을 한 번에 구한다. 캐시에 있으면 묻지 않고, 나머지는 `CHECKS_BATCH` 개씩 GraphQL 한 번으로 묻는다. 결과는 `{소문자 sha: 상태}`(캐시 적중 → 새로 안 것의 순서).
/// GitHub 가 모르는 커밋(아직 push 안 됨)·체크가 없는 커밋은 `none`. 저장소에 접근할 수 없으면 그 sha 들은 결과에서 빠진다.
/// GraphQL 은 읽는 대로 상태로 바꿔 쌓고 원본은 버린다
pub async fn fetch_commit_checks(c: &GitHubClient, r: &Slug, shas: &[String], cache: &CheckStateCache, cancel: &CancellationToken) -> Res<Json> {
    let mut out: Vec<(String, Json)> = Vec::new();
    let scope = js_to_lowercase(&format!("{}/{}", r.owner, r.repo));
    let mut missing: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for raw in shas {
        let raw = js_to_lowercase(raw);
        if !seen.insert(raw.clone()) || !is_sha(&raw) {
            continue;
        }
        match cache.get(&format!("{scope}@{raw}")) {
            Some(hit) => out.push((raw, s(hit))),
            None => missing.push(raw),
        }
    }
    for chunk in missing.chunks(CHECKS_BATCH) {
        let mut vars: Vec<(String, String)> = vec![("owner".into(), r.owner.clone()), ("name".into(), r.repo.clone())];
        vars.extend(chunk.iter().enumerate().map(|(k, sha)| (format!("o{k}"), sha.clone())));
        let refs: Vec<(&str, &str)> = vars.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        let data = c.graphql(&checks_map::rollup_query(chunk.len()), &refs, Some(cancel)).await?;
        let Some(states) = checks_map::rollup_states(&data, chunk.len())? else { continue };
        for (sha, state) in chunk.iter().zip(states) {
            cache.set(&format!("{scope}@{sha}"), state);
            out.push((sha.clone(), s(state)));
        }
    }
    Ok(Json::Obj(out))
}

/// 한 커밋의 체크 목록: check-run(소요 시간·결론 포함) + legacy status
pub async fn fetch_check_details(c: &GitHubClient, r: &Slug, sha: &str, cancel: &CancellationToken) -> Res<Json> {
    let sha = js_to_lowercase(sha);
    if !is_sha(&sha) {
        return Err(GhError::plain("잘못된 커밋 해시입니다"));
    }
    let base = format!("/repos/{}/{}/commits/{}", encode_uri_component(&r.owner), encode_uri_component(&r.repo), encode_uri_component(&sha));
    let q = [("per_page", Some("100"))];
    let (runs, status) = join(get(c, &format!("{base}/check-runs"), &q, false, cancel), get(c, &format!("{base}/status"), &q, false, cancel)).await;
    Ok(Json::Arr(checks_map::check_details(&runs?.data, &status?.data)?))
}

/// 워크플로 실행 한 쪽(30개)
pub async fn list_runs(c: &GitHubClient, r: &Slug, page: u32, cancel: &CancellationToken) -> Res<Json> {
    let (size, page_s) = (RUNS_PER_PAGE.to_string(), page.to_string());
    let path = format!("/repos/{}/{}/actions/runs", encode_uri_component(&r.owner), encode_uri_component(&r.repo));
    let res = get(c, &path, &[("per_page", Some(&size)), ("page", Some(&page_s))], false, cancel).await?;
    Ok(list_json(checks_map::runs_page(&res.data)?, res.has_more, None))
}

/// 워크플로 실행 하나의 잡(최대 100개)
pub async fn list_jobs(c: &GitHubClient, r: &Slug, run_id: u64, cancel: &CancellationToken) -> Res<Json> {
    let path = format!("/repos/{}/{}/actions/runs/{run_id}/jobs", encode_uri_component(&r.owner), encode_uri_component(&r.repo));
    let res = get(c, &path, &[("per_page", Some("100"))], false, cancel).await?;
    Ok(Json::Arr(checks_map::jobs(&res.data)?))
}

/// 로그인하지 않아 묻지 않았을 때의 결과(`available:false` — GraphQL 은 비로그인을 받지 않고, 비로그인 REST 한도로 커밋마다 묻는 것은 감당할 수 없다)
pub fn unavailable() -> Json {
    obj(vec![("available", Json::Bool(false)), ("states", Json::Obj(Vec::new()))])
}
