//! Launchpad(`src/main/github/launchpad.ts` 의 조회부 이식, Phase 4 PR 7): 내 "PR · 리뷰 요청 · 할당 이슈" 를 GitHub 검색 API 로 모은다.
//! 검색은 저장소를 한정하지 않고 사용자 기준으로 섹션마다 한 번씩만 보낸다(`repo:` 묶음을 쓰면 접근할 수 없는 저장소 하나가 검색 전체를 422 로 만들고 저장소 수만큼 검색어가 늘어난다).
//! 결과는 앱에 등록된 저장소의 GitHub remote 와 맞춰 거른다. 응답 변환은 [`super::launchpad_map`](PR 2)이 맡는다.
//!
//! - 쪽 수 상한은 섹션당 5쪽(× 100)이다. 검색 결과는 읽는 대로 그 자리에서 항목으로 바꿔 쌓는다(원본 응답은 쪽이 끝나면 버려진다).
//!   등록된 저장소에 속한 항목만 최종에 남으므로, remote 읽기가 끝난 뒤 걸러낸다.
//! - 저장소 remote 읽기는 동시 8개, 저장소마다 제한 시간(운영 3초) 안에 끝내야 하고 실패하면 `unresolved` 로 센다. 취소는 그대로 전파한다.
//! - 일부 검색이 GitHub 오류로 실패하면 이유를 `errors` 에 담고 읽은 데까지를 돌려준다. 로그인 만료(`GH_AUTH`)와 취소는 그대로 던진다.
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::time::Duration;

use futures_util::future::{try_join, try_join_all};
use futures_util::stream::{self, StreamExt};
use tokio_util::sync::CancellationToken;

use super::client::GitHubClient;
use super::error::{GhError, Res};
use super::launchpad_map::{launchpad_item, search_page, slug_from_repository_url, slug_key, SectionDef, MAX_PAGES_PER_SECTION, SECTIONS};
use super::ports::RepoQuery;
use super::queries::get;
use crate::jira::service::Clock;
use crate::net::js_value::{field, num, obj, s, template_string, MapError};
use crate::net::json::Json;

const PER_PAGE: usize = 100;
/// 저장소 하나의 remote 를 읽는 제한 시간(운영). 꺼진 SSH 호스트가 전체를 붙잡지 않게 한다
pub const RESOLVE_TIMEOUT: Duration = Duration::from_secs(3);
const RESOLVE_CONCURRENCY: usize = 8;

/// 검색 결과 한 건을 줄인 것: 저장소 키·중복 제거 키·항목(앱 저장소 ID 는 비어 있다 — 걸러낸 뒤 채운다)
struct Hit {
    slug_key: String,
    dedup: String,
    item: Result<Json, MapError>,
}

struct Section {
    hits: Vec<Hit>,
    incomplete: bool,
}

struct Resolved {
    /// 소문자 `owner/repo` → 그 저장소를 remote 로 가진 앱 저장소 ID(등록 순서)
    by_key: HashMap<String, Vec<String>>,
    /// GitHub remote 가 없는 저장소 수
    skipped: usize,
    /// 시간 초과·오류로 remote 를 읽지 못한 저장소 수
    unresolved: usize,
}

/// 저장소별 제한 시간 안에 병렬로 remote 를 읽는다. 실패한 저장소는 건너뛴다(취소는 그대로 던진다)
async fn resolve_remotes(repos: &dyn RepoQuery, ids: &[String], timeout: Duration, cancel: &CancellationToken) -> Res<Resolved> {
    let mut slugs: Vec<Option<Vec<(String, String)>>> = vec![None; ids.len()];
    let mut reads = stream::iter((0..ids.len()).map(|i| {
        let id = &ids[i];
        async move {
            if cancel.is_cancelled() {
                return Err(GhError::aborted());
            }
            let child = cancel.child_token();
            let read = tokio::time::timeout(timeout, repos.gh_remotes(id, child.clone())).await;
            // 시간 초과로 버려진 읽기가 계속 돌지 않게 신호를 끊는다
            child.cancel();
            if cancel.is_cancelled() {
                return Err(GhError::aborted());
            }
            Ok((i, read.ok().and_then(Result::ok).map(|list| list.into_iter().map(|r| (r.owner, r.repo)).collect::<Vec<_>>())))
        }
    }))
    .buffer_unordered(RESOLVE_CONCURRENCY);
    while let Some(r) = reads.next().await {
        let (i, v) = r?;
        slugs[i] = v;
    }
    drop(reads);
    let mut by_key: HashMap<String, Vec<String>> = HashMap::new();
    let (mut skipped, mut unresolved) = (0, 0);
    for (i, list) in slugs.into_iter().enumerate() {
        match list {
            None => unresolved += 1,
            Some(l) if l.is_empty() => skipped += 1,
            Some(l) => {
                for (owner, repo) in l {
                    by_key.entry(slug_key(&owner, &repo)).or_default().push(ids[i].clone());
                }
            }
        }
    }
    Ok(Resolved { by_key, skipped, unresolved })
}

/// 섹션 하나의 검색. GitHub 오류는 `errors` 에 담고(로그인 만료·취소는 던진다) 읽은 데까지를 돌려준다
async fn search_section(c: &GitHubClient, def: SectionDef, errors: &Mutex<Vec<String>>, cancel: &CancellationToken) -> Res<Section> {
    let mut out = Section { hits: Vec::new(), incomplete: false };
    for page in 1..=MAX_PAGES_PER_SECTION {
        let (size, page_s) = (PER_PAGE.to_string(), page.to_string());
        let query = [("q", Some(def.q)), ("sort", Some("updated")), ("order", Some("desc")), ("per_page", Some(size.as_str())), ("page", Some(page_s.as_str()))];
        let res = match get(c, "/search/issues", &query, false, cancel).await {
            Ok(r) => r,
            Err(e) if e.code.starts_with("GH_") && e.code != "GH_AUTH" => {
                let mut list = errors.lock().unwrap_or_else(|p| p.into_inner());
                if !list.contains(&e.message) {
                    list.push(e.message);
                }
                out.incomplete = true;
                return Ok(out);
            }
            Err(e) => return Err(e),
        };
        let (items, incomplete) = search_page(&res.data)?;
        out.incomplete |= incomplete;
        for raw in items {
            if matches!(raw, Json::Null) {
                return Err(GhError::bad_response());
            }
            let Some((owner, repo)) = slug_from_repository_url(field(Some(raw), "repository_url")) else { continue };
            let key = slug_key(&owner, &repo);
            let dedup = format!("{}:{key}#{}", def.section, template_string(field(Some(raw), "number")));
            out.hits.push(Hit { slug_key: key, dedup, item: launchpad_item(&def, raw, &[], &owner, &repo) });
        }
        if !res.has_more {
            return Ok(out);
        }
        if page == MAX_PAGES_PER_SECTION {
            out.incomplete = true;
        }
    }
    Ok(out)
}

/// 항목의 `repoIds` 를 채운다
fn with_repo_ids(mut item: Json, ids: &[String]) -> Json {
    if let Json::Obj(o) = &mut item {
        if let Some((_, v)) = o.iter_mut().find(|(k, _)| k == "repoIds") {
            *v = Json::Arr(ids.iter().map(|i| s(i)).collect());
        }
    }
    item
}

/// Launchpad 조회. 섹션 검색 3개와 저장소 remote 읽기를 동시에 시작하고, 둘이 끝나면 등록된 저장소에 속한 항목만 남긴다(섹션·저장소·번호가 같으면 먼저 온 것 하나, 최신순).
/// 일부 검색이 실패해도(한도 초과 등) 나머지는 돌려주고 이유를 `errors` 에 담는다. 로그인 만료(`GH_AUTH`)와 취소는 그대로 돌려준다
pub async fn fetch_launchpad(c: &GitHubClient, repos: &dyn RepoQuery, ids: &[String], timeout: Duration, clock: &dyn Clock, cancel: &CancellationToken) -> Res<Json> {
    let errors: Mutex<Vec<String>> = Mutex::new(Vec::new());
    let searches = try_join_all(SECTIONS.iter().map(|d| search_section(c, *d, &errors, cancel)));
    let (sections, resolved) = try_join(searches, resolve_remotes(repos, ids, timeout, cancel)).await?;
    let mut seen: HashSet<String> = HashSet::new();
    let mut items: Vec<Json> = Vec::new();
    let mut incomplete = false;
    for section in sections {
        incomplete |= section.incomplete;
        for hit in section.hits {
            let Some(repo_ids) = resolved.by_key.get(&hit.slug_key) else { continue };
            if !seen.insert(hit.dedup) {
                continue;
            }
            items.push(with_repo_ids(hit.item?, repo_ids));
        }
    }
    items.sort_by(|a, b| {
        let at = |x: &Json| x.get("updatedAt").and_then(Json::as_f64).unwrap_or(0.0);
        at(b).total_cmp(&at(a))
    });
    let errors: Vec<Json> = errors.into_inner().unwrap_or_else(|p| p.into_inner()).iter().map(|e| s(e)).collect();
    Ok(obj(vec![
        ("items", Json::Arr(items)),
        ("fetchedAt", num(clock.now_ms())),
        ("needsLogin", Json::Bool(false)),
        ("incomplete", Json::Bool(incomplete)),
        ("skipped", num(resolved.skipped as f64)),
        ("unresolved", num(resolved.unresolved as f64)),
        ("errors", Json::Arr(errors)),
    ]))
}

/// 로그인하지 않은 상태의 결과(`@me` 검색은 로그인이 있어야 한다)
pub fn login_required(now_ms: f64) -> Json {
    obj(vec![
        ("items", Json::Arr(Vec::new())),
        ("fetchedAt", num(now_ms)),
        ("needsLogin", Json::Bool(true)),
        ("incomplete", Json::Bool(false)),
        ("skipped", num(0.0)),
        ("unresolved", num(0.0)),
        ("errors", Json::Arr(Vec::new())),
    ])
}
