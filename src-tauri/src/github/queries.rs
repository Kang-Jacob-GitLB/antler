//! GitHub REST 조회(`src/main/github/api.ts` 의 호출부 이식, Phase 4 PR 7). 요청·쪽 읽기·보강·상한은 여기서, 응답 변환은 [`super::api_map`](PR 2)이 맡는다.
//!
//! - 쪽을 읽는 대로 그 자리에서 변환해 쌓는다(원본 응답은 쪽이 끝나면 버려진다). 쪽 수·항목 수 상한은 TS 와 같다:
//!   리뷰 3쪽·파일 30쪽·댓글 10쪽·커밋 250개·빈 쪽 추가 5쪽·보강 동시 6개.
//! - 보강(`listPulls`·`getPull`)은 TS 의 `soft` 규칙을 따른다: 요청이나 변환이 실패하면 그 키만 빼고 목록·상세는 돌려주고, **취소만** 그대로 던진다.
//!   그 규칙은 [`soft_checks`]·[`reviews_page`] 에 있고 골든 구동기(`tests/net.rs` 의 `run_gh_case`)도 같은 함수를 쓴다.
//! - 취소·세대 교체는 `CancellationToken` 이다. 취소됐으면 새 요청을 시작하지 않는다.
use futures_util::future::join;
use futures_util::stream::{self, StreamExt};
use tokio_util::sync::CancellationToken;

use super::{accessible_repos, api_map};
use super::client::{GetOptions, GetResult, GitHubClient, TokenChoice};
use super::error::{GhError, Res};
use super::validate::GhListQuery;
use crate::jira::service::Clock;
use crate::net::js_value::{encode_uri_component, must_arr, num, obj, template_string, truthy};
use crate::net::json::Json;

const PAGE_SIZE: usize = 30;
const ENRICH_CONCURRENCY: usize = 6;
const FILES_MAX_PAGES: usize = 30;
const COMMITS_MAX: usize = 250;
const COMMENTS_MAX_PAGES: usize = 10;
const REVIEWS_MAX_PAGES: usize = 3;
/// 필터링 결과가 빈 쪽이면 이어서 읽는 추가 쪽 수의 상한
const EMPTY_PAGE_MAX_EXTRA: usize = 5;
const KEY_PULLS_MAX: usize = 20;

/// `owner/repo`. 사이드카가 준 값이 URL 경로(`..` 등)로 새지 않게 [`Slug::parse`] 로만 만든다(필드는 비공개)
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Slug {
    owner: String,
    repo: String,
}

impl Slug {
    /// `remoteUrl.ts` 의 `OWNER_RE`(`^[A-Za-z0-9][A-Za-z0-9_-]{0,38}$`)·`REPO_RE`(`^[A-Za-z0-9._-]{1,100}$`)와 같은 규칙. `repo` 가 `.`·`..` 이면 거부한다
    pub fn parse(owner: &str, repo: &str) -> Option<Self> {
        let owner_ok = (1..=39).contains(&owner.len())
            && owner.bytes().enumerate().all(|(i, b)| b.is_ascii_alphanumeric() || (i > 0 && matches!(b, b'_' | b'-')));
        let repo_ok = (1..=100).contains(&repo.len()) && repo.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-')) && repo != "." && repo != "..";
        (owner_ok && repo_ok).then(|| Self { owner: owner.to_string(), repo: repo.to_string() })
    }

    pub fn owner(&self) -> &str {
        &self.owner
    }

    pub fn repo(&self) -> &str {
        &self.repo
    }
}

fn repo_path(r: &Slug) -> String {
    format!("/repos/{}/{}", encode_uri_component(r.owner()), encode_uri_component(r.repo()))
}

pub(crate) async fn get(c: &GitHubClient, path: &str, query: &[(&str, Option<&str>)], html: bool, cancel: &CancellationToken) -> Res<GetResult> {
    Ok(c.get(path, &TokenChoice::Stored, GetOptions { query, html, cancel: Some(cancel) }).await?)
}

// ── 보강 규칙 ─────────────────────────────────────────────────────────────

/// 리뷰 목록 한 쪽: 배열이어야 한다(TS `all.push(...res.data)` 는 배열이 아니면 던진다)
pub fn reviews_page(data: &Json) -> Option<&[Json]> {
    match data {
        Json::Arr(a) => Some(a),
        _ => None,
    }
}

/// `getPull`·`listPulls` 의 체크 보강: 헤드 sha 가 있을 때만 묻고, 변환이 던지면(TS `soft`) 없는 것으로 본다
pub fn soft_checks(summary: &Json, runs: &Json, status: &Json) -> Option<Vec<Json>> {
    truthy(summary.get("headSha")).then(|| api_map::check_runs_of(runs, status).ok()).flatten()
}

/// 보강 실패는 목록을 막지 않는다. 취소만 그대로 전파한다
fn soft<T>(r: Res<T>) -> Res<Option<T>> {
    match r {
        Ok(v) => Ok(Some(v)),
        Err(e) if e.is_aborted() => Err(e),
        Err(_) => Ok(None),
    }
}

/// PR 의 리뷰(최대 3쪽 × 100). 쪽마다 `hasMore` 로 이어 받는다
async fn fetch_reviews(c: &GitHubClient, r: &Slug, n: &str, cancel: &CancellationToken) -> Res<Vec<Json>> {
    let mut all: Vec<Json> = Vec::new();
    for page in 1..=REVIEWS_MAX_PAGES {
        let page_s = page.to_string();
        let res = get(c, &format!("{}/pulls/{n}/reviews", repo_path(r)), &[("per_page", Some("100")), ("page", Some(&page_s))], false, cancel).await?;
        all.extend_from_slice(reviews_page(&res.data).ok_or_else(GhError::bad_response)?);
        if !res.has_more {
            break;
        }
    }
    Ok(all)
}

/// check-run 과 legacy status 의 응답 원문 둘(변환은 [`soft_checks`])
async fn fetch_check_raw(c: &GitHubClient, r: &Slug, sha: &str, cancel: &CancellationToken) -> Res<(Json, Json)> {
    let base = format!("{}/commits/{}", repo_path(r), encode_uri_component(sha));
    let q = [("per_page", Some("100"))];
    let (runs, status) = join(get(c, &format!("{base}/check-runs"), &q, false, cancel), get(c, &format!("{base}/status"), &q, false, cancel)).await;
    Ok((runs?.data, status?.data))
}

/// 헤드 sha 의 체크 목록(실패는 `None`). sha 가 비면 묻지 않는다
async fn soft_check_list(c: &GitHubClient, r: &Slug, summary: &Json, cancel: &CancellationToken) -> Res<Option<Vec<Json>>> {
    let Some(sha) = summary.get("headSha").and_then(Json::as_str).filter(|t| !t.is_empty()) else { return Ok(None) };
    Ok(soft(fetch_check_raw(c, r, sha, cancel).await)?.and_then(|(runs, status)| soft_checks(summary, &runs, &status)))
}

// ── 목록 ──────────────────────────────────────────────────────────────────

pub(crate) fn list_json(items: Vec<Json>, has_more: bool, next_page: Option<usize>) -> Json {
    let mut o = vec![("items", Json::Arr(items)), ("hasMore", Json::Bool(has_more))];
    if let Some(p) = next_page {
        o.push(("nextPage", num(p as f64)));
    }
    obj(o)
}

/// issues 엔드포인트처럼 항목을 걸러내는 목록 조회. 걸러낸 뒤 빈 쪽이고 `hasMore` 면 항목이 나올 때까지(또는 끝날 때까지, 추가 5쪽 상한) 다음 쪽을 이어 읽는다.
/// 추가 쪽을 읽었다면 호출자가 다음에 요청할 쪽을 `nextPage` 로 알린다
async fn fetch_filtered(
    c: &GitHubClient,
    path: &str,
    base: &[(&str, Option<&str>)],
    page: usize,
    pick: impl Fn(&Json) -> Result<Vec<Json>, crate::net::js_value::MapError>,
    cancel: &CancellationToken,
) -> Res<Json> {
    for (extra, cur) in (page..).enumerate() {
        let cur_s = cur.to_string();
        let mut query: Vec<(&str, Option<&str>)> = base.to_vec();
        query.push(("page", Some(&cur_s)));
        let res = get(c, path, &query, false, cancel).await?;
        let items = pick(&res.data)?;
        if !items.is_empty() || !res.has_more || extra >= EMPTY_PAGE_MAX_EXTRA {
            return Ok(list_json(items, res.has_more, (res.has_more && cur != page).then_some(cur + 1)));
        }
    }
    Err(GhError::bad_response())
}

/// PR 목록. 라벨·담당자 필터가 없으면 pulls 엔드포인트(updated 내림차순, 브랜치·sha 포함)를 쓰고, 있으면 issues 엔드포인트(labels·assignee 파라미터)에서 PR 만 고른다
/// (검색 API 의 쿼리 언어를 쓰지 않아 라벨 이름으로 다른 한정자를 끼워 넣을 수 없다). 필터된 쪽은 보강하지 않는다.
/// 로그인했다면 pulls 결과에만 리뷰·체크 요약을 동시 6개씩 보강한다
pub async fn list_pulls(c: &GitHubClient, r: &Slug, q: &GhListQuery, cancel: &CancellationToken) -> Res<Json> {
    let page = q.page as usize;
    if q.label.is_some() || q.assignee.is_some() {
        let base = [
            ("state", Some(q.state.as_str())),
            ("labels", q.label.as_deref()),
            ("assignee", q.assignee.as_deref()),
            ("per_page", Some("30")),
            ("sort", Some("updated")),
            ("direction", Some("desc")),
        ];
        return fetch_filtered(c, &format!("{}/issues", repo_path(r)), &base, page, api_map::filtered_pull_items, cancel).await;
    }
    let (page_s, size) = (page.to_string(), PAGE_SIZE.to_string());
    let res = get(c, &format!("{}/pulls", repo_path(r)), &[("state", Some(&q.state)), ("per_page", Some(&size)), ("page", Some(&page_s)), ("sort", Some("updated")), ("direction", Some("desc"))], false, cancel).await?;
    let raws = must_arr(Some(&res.data))?;
    let mut items = api_map::pulls_page(&res.data)?;
    if c.has_token() {
        enrich(c, r, raws, &mut items, cancel).await?;
    }
    Ok(list_json(items, res.has_more, None))
}

/// 항목마다 리뷰·체크를 동시 6개 이하로 받아 `review`·`checks` 키를 붙인다. 취소가 오면 남은 요청은 시작하지 않고 진행 중인 것은 버린다
async fn enrich(c: &GitHubClient, r: &Slug, raws: &[Json], items: &mut [Json], cancel: &CancellationToken) -> Res<()> {
    type Found = (usize, Option<Vec<Json>>, Option<Vec<Json>>);
    let summaries: Vec<Json> = items.to_vec();
    // 인덱스를 값으로 넘기고 참조는 클로저 안에서 만든다 — 참조 인자를 받는 클로저면 이 future 를 tauri command 에서 `Send` 로 증명하지 못한다
    // (서비스 future 가 `Send` 인지는 `tests/net.rs` 가 컴파일로 본다)
    let mut found: Vec<Found> = Vec::with_capacity(raws.len());
    let mut results = stream::iter((0..raws.len()).map(|i| {
        let (raw, summary) = (&raws[i], &summaries[i]);
        async move {
            let number = template_string(raw.get("number"));
            let (reviews, checks) = join(async { soft(fetch_reviews(c, r, &number, cancel).await) }, soft_check_list(c, r, summary, cancel)).await;
            Ok::<Found, GhError>((i, reviews?, checks?))
        }
    }))
    .buffer_unordered(ENRICH_CONCURRENCY);
    while let Some(f) = results.next().await {
        found.push(f?);
    }
    drop(results);
    found.sort_by_key(|f| f.0);
    for (i, reviews, checks) in &found {
        api_map::enrich_list_item(&raws[*i], &mut items[*i], reviews.as_deref(), checks.as_deref())?;
    }
    Ok(())
}

/// Jira 이슈 키(호출부에서 ABC-123 형식으로 검증)를 제목·본문에 담은 PR(최근 수정순 최대 20개).
/// 검색 API 쿼리 언어를 쓰므로 저장소는 `Slug`(형식 확인을 마친 값)로 `repo:` 에 고정하고, 키는 따옴표로 감싸 한 낱말로만 찾는다
pub async fn search_pulls_by_key(c: &GitHubClient, r: &Slug, key: &str, cancel: &CancellationToken) -> Res<Json> {
    let (q, size) = (format!("repo:{}/{} is:pr \"{key}\"", r.owner(), r.repo()), KEY_PULLS_MAX.to_string());
    let res = get(c, "/search/issues", &[("q", Some(&q)), ("sort", Some("updated")), ("order", Some("desc")), ("per_page", Some(&size))], false, cancel).await?;
    Ok(Json::Arr(api_map::search_pulls(&res.data)?))
}

pub async fn get_pull(c: &GitHubClient, r: &Slug, n: u32, cancel: &CancellationToken) -> Res<Json> {
    let pull = get(c, &format!("{}/pulls/{n}", repo_path(r)), &[], true, cancel).await?;
    let raw = pull.data;
    let summary = api_map::map_pull(&raw)?;
    let number = n.to_string();
    let (reviews, runs) = join(fetch_reviews(c, r, &number, cancel), soft_check_list(c, r, &summary, cancel)).await;
    let (reviews, runs) = (reviews?, runs?);
    Ok(api_map::pull_detail(&raw, &reviews, runs.as_deref())?)
}

/// 변경 파일(최대 30쪽 × 100 = 3000개, GitHub 가 주는 상한과 같다)
pub async fn get_pull_files(c: &GitHubClient, r: &Slug, n: u32, cancel: &CancellationToken) -> Res<Json> {
    let mut out: Vec<Json> = Vec::new();
    for page in 1..=FILES_MAX_PAGES {
        let page_s = page.to_string();
        let res = get(c, &format!("{}/pulls/{n}/files", repo_path(r)), &[("per_page", Some("100")), ("page", Some(&page_s))], false, cancel).await?;
        out.extend(api_map::pull_files(&res.data)?);
        if !res.has_more {
            break;
        }
    }
    Ok(Json::Arr(out))
}

/// PR 커밋(최대 250개 — GitHub 가 이 API 로 주는 상한)
pub async fn get_pull_commits(c: &GitHubClient, r: &Slug, n: u32, cancel: &CancellationToken) -> Res<Json> {
    let mut out: Vec<Json> = Vec::new();
    let mut page = 1usize;
    while out.len() < COMMITS_MAX {
        let page_s = page.to_string();
        let res = get(c, &format!("{}/pulls/{n}/commits", repo_path(r)), &[("per_page", Some("100")), ("page", Some(&page_s))], false, cancel).await?;
        out.extend(api_map::pull_commits(&res.data)?);
        if !res.has_more {
            break;
        }
        page += 1;
    }
    Ok(Json::Arr(api_map::limit_commits(out)))
}

/// 이슈 목록. issues 엔드포인트는 PR 도 섞어 돌려주므로 `pull_request` 가 있는 항목은 버린다.
/// 그래서 한 쪽이 30개보다 짧을 수 있으니 `hasMore` 는 항목 수가 아니라 Link 헤더로 판단하고, 빈 쪽이면 항목이 나올 때까지 다음 쪽을 이어 읽는다
pub async fn list_issues(c: &GitHubClient, r: &Slug, q: &GhListQuery, cancel: &CancellationToken) -> Res<Json> {
    let base = [("state", Some(q.state.as_str())), ("labels", q.label.as_deref()), ("assignee", q.assignee.as_deref()), ("per_page", Some("30")), ("sort", Some("updated")), ("direction", Some("desc"))];
    fetch_filtered(c, &format!("{}/issues", repo_path(r)), &base, q.page as usize, api_map::issue_items, cancel).await
}

pub async fn get_issue(c: &GitHubClient, r: &Slug, n: u32, cancel: &CancellationToken) -> Res<Json> {
    let res = get(c, &format!("{}/issues/{n}", repo_path(r)), &[], true, cancel).await?;
    Ok(api_map::issue_detail(&res.data)?)
}

/// PR·이슈 공용 댓글(이슈 댓글 API). 최대 1000개
pub async fn get_comments(c: &GitHubClient, r: &Slug, n: u32, cancel: &CancellationToken) -> Res<Json> {
    let mut out: Vec<Json> = Vec::new();
    for page in 1..=COMMENTS_MAX_PAGES {
        let page_s = page.to_string();
        let res = get(c, &format!("{}/issues/{n}/comments", repo_path(r)), &[("per_page", Some("100")), ("page", Some(&page_s))], true, cancel).await?;
        out.extend(api_map::comments_page(&res.data)?);
        if !res.has_more {
            break;
        }
    }
    Ok(Json::Arr(out))
}

/// 필터용 라벨·담당 가능 사용자(각 최대 100개)
pub async fn get_filters(c: &GitHubClient, r: &Slug, cancel: &CancellationToken) -> Res<Json> {
    let q = [("per_page", Some("100"))];
    let (labels, assignees) = join(get(c, &format!("{}/labels", repo_path(r)), &q, false, cancel), get(c, &format!("{}/assignees", repo_path(r)), &q, false, cancel)).await;
    Ok(api_map::filters(&labels?.data, &assignees?.data)?)
}

/// 로그인한 사용자가 접근할 수 있는 저장소(본인·협업·소속 org)를 쪽별로 읽는다(최대 20쪽 × 100). 읽는 대로 변환해 쌓고, 상한을 넘으면 거기까지만 돌려주고 `truncated` 로 알린다.
/// `pushedAt` 내림차순. 취소는 `ABORTED`
pub async fn fetch_accessible_repos(c: &GitHubClient, clock: &dyn Clock, cancel: &CancellationToken) -> Res<Json> {
    let mut repos: Vec<Json> = Vec::new();
    let mut truncated = false;
    for page in 1.. {
        if cancel.is_cancelled() {
            return Err(GhError::aborted());
        }
        let page_s = page.to_string();
        let query = [("affiliation", Some("owner,collaborator,organization_member")), ("sort", Some("pushed")), ("per_page", Some("100")), ("page", Some(page_s.as_str()))];
        let res = get(c, "/user/repos", &query, false, cancel).await?;
        repos.extend(accessible_repos::accessible_page(&res.data));
        if !res.has_more {
            break;
        }
        if page >= accessible_repos::MAX_ACCESSIBLE_PAGES {
            truncated = true;
            break;
        }
    }
    accessible_repos::sort_accessible(&mut repos);
    Ok(obj(vec![("repos", Json::Arr(repos)), ("needsLogin", Json::Bool(false)), ("truncated", Json::Bool(truncated)), ("fetchedAt", num(clock.now_ms()))]))
}

/// 로그인하지 않은 상태의 결과
pub fn accessible_login_required(now_ms: f64) -> Json {
    obj(vec![("repos", Json::Arr(Vec::new())), ("needsLogin", Json::Bool(true)), ("truncated", Json::Bool(false)), ("fetchedAt", num(now_ms))])
}
