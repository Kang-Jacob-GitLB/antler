//! GitHub REST 응답 → 공유 타입(IPC 값) 변환(`src/main/github/api.ts` 의 순수 부분). 날짜는 epoch seconds.
//! 입력은 이미 해석한 응답 [`Json`], 출력은 TS 와 같은 키 순서의 [`Json`] 이다. 페이징·보강 요청·동시성은 뒤 PR 의 서비스 계층 몫이다.
//!
//! TS 가 던질 입력(`null` 항목, 배열이 아닌 `labels`)은 [`MapError`] 다. 타입이 틀려도 TS 가 던지지 않는 필드(`title: 5`)는 그대로 흘린다.
//! 알려진 차이는 날짜 하나다: `Date.parse` 가 받는 ISO 8601 이외의 꼴(시간대 없는 시각·`2024-1-1`)과 문자열이 아닌 값은 0 이다([`crate::net::js_date`]).
use std::collections::HashMap;

use crate::net::js_date::to_epoch;
use crate::net::js_value::{field, field_strict, items, js_length, must_arr, nn, num, obj, push_some, s, truthy, MapError, MapResult};
use crate::net::json::Json;

const FILE_STATUSES: [&str; 7] = ["added", "removed", "modified", "renamed", "copied", "changed", "unchanged"];
const COMMITS_MAX: usize = 250;

fn str_eq(v: Option<&Json>, want: &str) -> bool {
    matches!(v, Some(Json::Str(t)) if t == want)
}

/// `mapUser`: 삭제된 계정은 user 가 null 이다
pub fn map_user(u: Option<&Json>) -> Json {
    obj(vec![("login", nn(field(u, "login"), s("ghost"))), ("avatarUrl", nn(field(u, "avatar_url"), s("")))])
}

/// `mapLabel`: 문자열이면 이름만, 객체면 name·color(앞의 `#` 제거). 색이 문자열이 아니면 TS 는 `.replace` 에서 던진다
pub fn map_label(l: &Json) -> MapResult<Json> {
    match l {
        Json::Str(name) => Ok(obj(vec![("name", s(name)), ("color", s(""))])),
        Json::Null => Err(MapError),
        other => {
            let color = match nn(field(Some(other), "color"), s("")) {
                Json::Str(c) => c.strip_prefix('#').unwrap_or(&c).to_string(),
                _ => return Err(MapError),
            };
            Ok(obj(vec![("name", nn(field(Some(other), "name"), s(""))), ("color", s(&color))]))
        }
    }
}

fn labels(v: Option<&Json>) -> MapResult<Json> {
    Ok(Json::Arr(items(v)?.iter().map(map_label).collect::<MapResult<_>>()?))
}

fn users(v: Option<&Json>) -> MapResult<Json> {
    Ok(Json::Arr(items(v)?.iter().map(|u| map_user(Some(u))).collect()))
}

fn pull_state(state: Option<&Json>, merged_at: Option<&Json>) -> &'static str {
    if truthy(merged_at) {
        "merged"
    } else if str_eq(state, "closed") {
        "closed"
    } else {
        "open"
    }
}

fn non_null(p: &Json) -> MapResult<()> {
    if matches!(p, Json::Null) {
        Err(MapError)
    } else {
        Ok(())
    }
}

fn base_summary(p: &Json, state: &str, head: Option<(Json, Json, Json)>) -> MapResult<Vec<(&'static str, Json)>> {
    non_null(p)?;
    let mut o: Vec<(&'static str, Json)> = Vec::new();
    push_some(&mut o, "number", field(Some(p), "number").cloned());
    o.push(("title", nn(field(Some(p), "title"), s(""))));
    o.push(("state", s(state)));
    o.push(("draft", Json::Bool(matches!(field(Some(p), "draft"), Some(Json::Bool(true))))));
    o.push(("author", map_user(field(Some(p), "user"))));
    o.push(("labels", labels(field(Some(p), "labels"))?));
    o.push(("assignees", users(field(Some(p), "assignees"))?));
    let (head_ref, base_ref, head_sha) = head.unwrap_or((s(""), s(""), s("")));
    o.push(("headRef", head_ref));
    o.push(("baseRef", base_ref));
    o.push(("headSha", head_sha));
    o.push(("updatedAt", num(to_epoch(field(Some(p), "updated_at")))));
    o.push(("createdAt", num(to_epoch(field(Some(p), "created_at")))));
    o.push(("comments", nn(field(Some(p), "comments"), num(0.0))));
    o.push(("htmlUrl", nn(field(Some(p), "html_url"), s(""))));
    Ok(o)
}

/// `mapPull`: 목록용 PR 요약. pulls 목록 응답에는 comments 수가 없어 0 이다
pub fn map_pull(p: &Json) -> MapResult<Json> {
    non_null(p)?;
    let head = field(Some(p), "head");
    let head3 = (
        nn(field(head, "ref"), s("")),
        nn(field(field(Some(p), "base"), "ref"), s("")),
        nn(field(head, "sha"), s("")),
    );
    let state = pull_state(field(Some(p), "state"), field(Some(p), "merged_at"));
    Ok(obj(base_summary(p, state, Some(head3))?))
}

/// `mapSearchPull`: 검색 API(issue 모양) 결과의 최소 변환. 브랜치·sha 는 빈 문자열이다
pub fn map_search_pull(i: &Json) -> MapResult<Json> {
    non_null(i)?;
    let merged = field(field(Some(i), "pull_request"), "merged_at");
    let state = pull_state(field(Some(i), "state"), merged);
    Ok(obj(base_summary(i, state, None)?))
}

pub fn map_issue(i: &Json) -> MapResult<Json> {
    non_null(i)?;
    let mut o: Vec<(&str, Json)> = Vec::new();
    push_some(&mut o, "number", field(Some(i), "number").cloned());
    o.push(("title", nn(field(Some(i), "title"), s(""))));
    o.push(("state", s(if str_eq(field(Some(i), "state"), "closed") { "closed" } else { "open" })));
    o.push(("author", map_user(field(Some(i), "user"))));
    o.push(("labels", labels(field(Some(i), "labels"))?));
    o.push(("assignees", users(field(Some(i), "assignees"))?));
    o.push(("comments", nn(field(Some(i), "comments"), num(0.0))));
    o.push(("updatedAt", num(to_epoch(field(Some(i), "updated_at")))));
    o.push(("createdAt", num(to_epoch(field(Some(i), "created_at")))));
    o.push(("htmlUrl", nn(field(Some(i), "html_url"), s(""))));
    Ok(obj(o))
}

// ── 리뷰·체크 집계 ────────────────────────────────────────────────────────

/// `x?.toUpperCase()`: null·없음은 `None`, 문자열 외의 값은 TS 가 던진다
fn upper(v: Option<&Json>) -> MapResult<Option<String>> {
    match v {
        None | Some(Json::Null) => Ok(None),
        Some(Json::Str(t)) => Ok(Some(t.to_uppercase())),
        Some(_) => Err(MapError),
    }
}

/// `aggregateReviews`: 사용자별 마지막 판정(APPROVED·CHANGES_REQUESTED, DISMISSED 는 취소)으로 요약한다.
/// `requested_count` 는 `(requested_reviewers?.length ?? 0) + (requested_teams?.length ?? 0)` 다([`requested_count`])
pub fn aggregate_reviews(reviews: &[Json], requested_count: f64) -> MapResult<&'static str> {
    // 사용자(login 의 값 그대로 — 숫자 5 와 문자열 "5" 는 다른 키다)별 마지막 판정
    let mut verdict: HashMap<String, &'static str> = HashMap::new();
    let mut any_review = false;
    for r in reviews {
        non_null(r)?;
    }
    // 제출 시각순(안정 정렬 — V8 도 안정). 시각은 한 번만 해석한다
    let mut sorted: Vec<(f64, &Json)> = reviews.iter().map(|r| (to_epoch(field(Some(r), "submitted_at")), r)).collect();
    sorted.sort_by(|a, b| a.0.total_cmp(&b.0));
    for (_, r) in sorted {
        let Some(state) = upper(field(Some(r), "state"))? else { continue };
        if state.is_empty() || state == "PENDING" {
            continue;
        }
        any_review = true;
        let who = format!("{:?}", nn(field(field(Some(r), "user"), "login"), s("ghost")));
        match state.as_str() {
            "APPROVED" => {
                verdict.insert(who, "APPROVED");
            }
            "CHANGES_REQUESTED" => {
                verdict.insert(who, "CHANGES_REQUESTED");
            }
            "DISMISSED" => {
                verdict.remove(&who);
            }
            _ => {}
        }
    }
    Ok(if verdict.values().any(|v| *v == "CHANGES_REQUESTED") {
        "changes_requested"
    } else if verdict.values().any(|v| *v == "APPROVED") {
        "approved"
    } else if any_review {
        "commented"
    } else if requested_count > 0.0 {
        "pending"
    } else {
        "none"
    })
}

/// 리뷰 요청 수: `(requested_reviewers?.length ?? 0) + (requested_teams?.length ?? 0)`
pub fn requested_count(raw: &Json) -> f64 {
    js_length(field(Some(raw), "requested_reviewers")) + js_length(field(Some(raw), "requested_teams"))
}

pub fn map_review_state(state: Option<&Json>) -> MapResult<&'static str> {
    Ok(match upper(state)?.as_deref() {
        Some("APPROVED") => "approved",
        Some("CHANGES_REQUESTED") => "changes_requested",
        Some("COMMENTED") => "commented",
        Some("PENDING") => "pending",
        Some("DISMISSED") => "dismissed",
        _ => "none",
    })
}

/// `checkRunState`: 완료되지 않았으면 pending, 완료된 것은 결론으로 — success · neutral/skipped/stale → neutral · action_required → pending · 그 외 failure
pub fn check_run_state(status: Option<&Json>, conclusion: Option<&Json>) -> &'static str {
    if !str_eq(status, "completed") {
        return "pending";
    }
    match conclusion {
        Some(Json::Str(c)) => match c.as_str() {
            "success" => "success",
            "neutral" | "skipped" | "stale" => "neutral",
            "action_required" => "pending",
            _ => "failure",
        },
        _ => "failure",
    }
}

/// `commitStatusState`: legacy status 한 건
pub fn commit_status_state(state: Option<&Json>) -> &'static str {
    if str_eq(state, "success") {
        "success"
    } else if str_eq(state, "pending") {
        "pending"
    } else {
        "failure"
    }
}

/// `aggregateChecks`: failure > pending > (전부 neutral 이면 neutral) > success, 비면 none
pub fn aggregate_checks(states: &[&str]) -> &'static str {
    if states.is_empty() {
        "none"
    } else if states.contains(&"failure") {
        "failure"
    } else if states.contains(&"pending") {
        "pending"
    } else if states.iter().all(|x| *x == "neutral") {
        "neutral"
    } else {
        "success"
    }
}

// ── 조회 결과 변환 ────────────────────────────────────────────────────────

fn state_of(item: &Json) -> &str {
    item.get("state").and_then(Json::as_str).unwrap_or("none")
}

/// 개별 체크(check-run + legacy status)의 `{name, state, url?}`. `check_runs`·`statuses` 는 각 응답 본문이다(`fetchChecks`)
pub fn check_runs_of(runs_data: &Json, status_data: &Json) -> MapResult<Vec<Json>> {
    let mut out = Vec::new();
    for run in items(field_strict(Some(runs_data), "check_runs")?)? {
        non_null(run)?;
        let mut o = vec![("name", nn(field(Some(run), "name"), s(""))), ("state", s(check_run_state(field(Some(run), "status"), field(Some(run), "conclusion"))))];
        push_some(&mut o, "url", truthy(field(Some(run), "html_url")).then(|| field(Some(run), "html_url").cloned()).flatten());
        out.push(obj(o));
    }
    for st in items(field_strict(Some(status_data), "statuses")?)? {
        non_null(st)?;
        let mut o = vec![("name", nn(field(Some(st), "context"), s(""))), ("state", s(commit_status_state(field(Some(st), "state"))))];
        push_some(&mut o, "url", truthy(field(Some(st), "target_url")).then(|| field(Some(st), "target_url").cloned()).flatten());
        out.push(obj(o));
    }
    Ok(out)
}

/// 체크 목록의 집계 상태
pub fn aggregate_check_list(checks: &[Json]) -> &'static str {
    let states: Vec<&str> = checks.iter().map(state_of).collect();
    aggregate_checks(&states)
}

/// 목록 보강(`listPulls`): 항목 끝에 `review`·`checks` 를 이 순서로 붙인다. 리뷰·체크 조회가 실패했거나(`None`) 헤드 sha 가 없어 묻지 않은 쪽은 키를 더하지 않는다
pub fn enrich_list_item(raw: &Json, item: &mut Json, reviews: Option<&[Json]>, checks: Option<&[Json]>) -> MapResult<()> {
    let review = reviews.map(|r| aggregate_reviews(r, requested_count(raw))).transpose()?;
    if let Json::Obj(o) = item {
        if let Some(r) = review {
            o.push(("review".into(), s(r)));
        }
        if let Some(c) = checks {
            o.push(("checks".into(), s(aggregate_check_list(c))));
        }
    }
    Ok(())
}

/// `getPull`: 요약 + 리뷰·체크·본문. `check_runs` 가 `None` 이면(헤드 sha 없음·체크 조회 실패) `checks` 키가 없고 `checkRuns` 는 빈 배열이다
pub fn pull_detail(raw: &Json, reviews: &[Json], check_runs: Option<&[Json]>) -> MapResult<Json> {
    let Json::Obj(mut o) = map_pull(raw)? else { return Err(MapError) };
    let requested = items(field(Some(raw), "requested_reviewers"))?;
    let mut detail = Vec::new();
    for x in reviews {
        non_null(x)?;
        if truthy(field(Some(x), "submitted_at")) && upper(field(Some(x), "state"))?.as_deref() != Some("PENDING") {
            detail.push(obj(vec![
                ("author", map_user(field(Some(x), "user"))),
                ("state", s(map_review_state(field(Some(x), "state"))?)),
                ("submittedAt", num(to_epoch(field(Some(x), "submitted_at")))),
            ]));
        }
    }
    let review = aggregate_reviews(reviews, js_length(field(Some(raw), "requested_reviewers")) + js_length(field(Some(raw), "requested_teams")))?;
    o.push(("review".into(), s(review)));
    if let Some(runs) = check_runs {
        o.push(("checks".into(), s(aggregate_check_list(runs))));
    }
    o.push(("bodyHtml".into(), nn(field(Some(raw), "body_html"), s(""))));
    o.push(("mergeable".into(), nn(field(Some(raw), "mergeable"), Json::Null)));
    o.push(("additions".into(), nn(field(Some(raw), "additions"), num(0.0))));
    o.push(("deletions".into(), nn(field(Some(raw), "deletions"), num(0.0))));
    o.push(("changedFiles".into(), nn(field(Some(raw), "changed_files"), num(0.0))));
    o.push(("reviews".into(), Json::Arr(detail)));
    o.push(("checkRuns".into(), Json::Arr(check_runs.map_or_else(Vec::new, <[Json]>::to_vec))));
    o.push(("requestedReviewers".into(), Json::Arr(requested.iter().map(|u| map_user(Some(u))).collect())));
    Ok(Json::Obj(o))
}

/// 변경 파일 한 쪽(`getPullFiles`)
pub fn pull_files(data: &Json) -> MapResult<Vec<Json>> {
    let mut out = Vec::new();
    for f in must_arr(Some(data))? {
        non_null(f)?;
        let status = field(Some(f), "status").and_then(Json::as_str).and_then(|t| FILE_STATUSES.iter().find(|k| **k == t)).copied().unwrap_or("changed");
        let mut o: Vec<(&str, Json)> = Vec::new();
        push_some(&mut o, "path", field(Some(f), "filename").cloned());
        push_some(&mut o, "oldPath", truthy(field(Some(f), "previous_filename")).then(|| field(Some(f), "previous_filename").cloned()).flatten());
        o.push(("status", s(status)));
        o.push(("additions", nn(field(Some(f), "additions"), num(0.0))));
        o.push(("deletions", nn(field(Some(f), "deletions"), num(0.0))));
        push_some(&mut o, "patch", field(Some(f), "patch").cloned());
        out.push(obj(o));
    }
    Ok(out)
}

/// PR 커밋 한 쪽(`getPullCommits`). 상한(250개)은 이어 붙인 뒤 [`limit_commits`] 가 자른다
pub fn pull_commits(data: &Json) -> MapResult<Vec<Json>> {
    let mut out = Vec::new();
    for k in must_arr(Some(data))? {
        non_null(k)?;
        let commit = field(Some(k), "commit");
        let author = field(Some(k), "author");
        let mut o: Vec<(&str, Json)> = Vec::new();
        push_some(&mut o, "sha", field(Some(k), "sha").cloned());
        o.push(("message", nn(field(commit, "message"), s(""))));
        let name = nn(field(field(commit, "author"), "name"), nn(field(author, "login"), s("")));
        o.push(("authorName", name));
        if truthy(author) {
            o.push(("author", map_user(author)));
        }
        let date = nn(field(field(commit, "author"), "date"), Json::Null);
        let date = if matches!(date, Json::Null) { field(field(commit, "committer"), "date").cloned().unwrap_or(Json::Null) } else { date };
        o.push(("date", num(to_epoch(Some(&date)))));
        out.push(obj(o));
    }
    Ok(out)
}

pub fn limit_commits(mut all: Vec<Json>) -> Vec<Json> {
    all.truncate(COMMITS_MAX);
    all
}

/// 이슈 목록 한 쪽: PR 이 섞여 있으면 버린다(`listIssues`의 pick)
pub fn issue_items(data: &Json) -> MapResult<Vec<Json>> {
    let mut out = Vec::new();
    for i in must_arr(Some(data))? {
        non_null(i)?;
        if !truthy(field(Some(i), "pull_request")) {
            out.push(map_issue(i)?);
        }
    }
    Ok(out)
}

/// 라벨·담당자 필터가 있을 때의 PR 목록 한 쪽: issues 엔드포인트에서 `pull_request` 가 있는 항목만(`listPulls` 의 pick)
pub fn filtered_pull_items(data: &Json) -> MapResult<Vec<Json>> {
    let mut out = Vec::new();
    for i in must_arr(Some(data))? {
        non_null(i)?;
        if truthy(field(Some(i), "pull_request")) {
            out.push(map_search_pull(i)?);
        }
    }
    Ok(out)
}

/// 키로 찾은 PR 검색 결과(`searchPullsByKey`)
pub fn search_pulls(data: &Json) -> MapResult<Vec<Json>> {
    non_null(data)?;
    let mut out = Vec::new();
    for i in items(field(Some(data), "items"))? {
        non_null(i)?;
        if truthy(field(Some(i), "pull_request")) {
            out.push(map_search_pull(i)?);
        }
    }
    Ok(out)
}

pub fn pulls_page(data: &Json) -> MapResult<Vec<Json>> {
    must_arr(Some(data))?.iter().map(map_pull).collect()
}

/// `getIssue`: 요약 + html 본문 + closedAt(0 이 아닐 때만)
pub fn issue_detail(raw: &Json) -> MapResult<Json> {
    let Json::Obj(mut o) = map_issue(raw)? else { return Err(MapError) };
    let closed = to_epoch(field(Some(raw), "closed_at"));
    o.push(("bodyHtml".into(), nn(field(Some(raw), "body_html"), s(""))));
    if closed != 0.0 {
        o.push(("closedAt".into(), num(closed)));
    }
    Ok(Json::Obj(o))
}

/// 댓글 한 쪽(`getComments`)
pub fn comments_page(data: &Json) -> MapResult<Vec<Json>> {
    let mut out = Vec::new();
    for k in must_arr(Some(data))? {
        non_null(k)?;
        out.push(obj(vec![
            ("author", map_user(field(Some(k), "user"))),
            ("bodyHtml", nn(field(Some(k), "body_html"), s(""))),
            ("createdAt", num(to_epoch(field(Some(k), "created_at")))),
            ("htmlUrl", nn(field(Some(k), "html_url"), s(""))),
        ]));
    }
    Ok(out)
}

/// 필터용 라벨·담당 가능 사용자(`getFilters`)
pub fn filters(labels_data: &Json, assignees_data: &Json) -> MapResult<Json> {
    let l = must_arr(Some(labels_data))?.iter().map(map_label).collect::<MapResult<Vec<_>>>()?;
    let a = must_arr(Some(assignees_data))?.iter().map(|u| map_user(Some(u))).collect();
    Ok(obj(vec![("labels", Json::Arr(l)), ("assignees", Json::Arr(a))]))
}
