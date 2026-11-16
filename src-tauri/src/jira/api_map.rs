//! Jira REST 응답 → 공유 타입(IPC 값) 변환(`src/main/jira/api.ts` 의 순수 부분). Cloud(API v3, ADF)와 Data Center/Server(API v2, 서버 렌더 HTML)의 차이는 여기서 흡수한다.
//! 요청·페이징·재시도·동시성은 뒤 PR 의 서비스 계층 몫이고, 여기는 "응답(들) → 출력" 순수 함수만 둔다.
//!
//! TS 가 `str()`·`obj()` 로 거르는 곳은 같은 분기고, 던지는 곳(`getMyId` 의 검증·`toISOString` 의 범위·잘못된 페이지 표지)은 [`MapError`] 다.
//! 알려진 차이: `Date.parse` 가 받는 ISO 8601 이외의 꼴(시간대 없는 시각 등)은 받지 않는다([`crate::net::js_date`]) —
//! dev-status 의 시각은 `''`, 워크로그 `started` 는 범위 밖으로 본다.
use std::collections::HashSet;

use super::adf::adf_to_html;
use super::validate::{is_user_id, MAX_PAGES, PAGE_SIZE};
use super::JiraKind;
use crate::git::commit::utf16_len;
use crate::git::fetch::{js_to_lowercase, js_trim, slice_utf16_lossy};
use crate::git::scope::is_js_space;
use crate::net::js_date::{iso_string, parse_date_ms};
use crate::net::js_value::{arr_or_empty, encode_uri_component, field, num, obj, s, str_or_empty, truthy, MapError, MapResult};
use crate::net::json::{js_number, Json};

const COMMENT_COUNT: usize = 5;
/// `jira:avatar` 가 받는 주소 길이 상한과 같다
pub const AVATAR_URL_MAX: usize = 2048;
/// 후보 id 상한 — 요청 URL(fields 쿼리) 길이를 묶는다
pub const MAX_DATE_FIELD_CANDIDATES: usize = 8;
const START_NAMES: [&str; 4] = ["start date", "시작 날짜", "시작일", "target start"];
const END_NAMES: [&str; 4] = ["end date", "종료일", "종료 날짜", "target end"];
const DEV_MAX: usize = 30;
const WORKLOG_ISSUE_MAX: usize = 300;
const WORKLOG_MAX: usize = 20_000;
const COMMENT_MAX: usize = 500;
const DAY_MS: f64 = 86_400_000.0;
/// ConfigStore team.name 상한과 맞춘다
const USER_NAME_MAX: usize = 200;

fn st(v: Option<&Json>) -> &str {
    str_or_empty(v)
}

fn fs(v: Option<&Json>, key: &str) -> String {
    st(field(v, key)).to_string()
}

/// 사용자 객체의 프로필 사진 주소(`avatarUrls` 의 48x48, 없으면 더 작은 것). http(s) 가 아니거나 너무 길면 빈 문자열
pub fn avatar_of(user: Option<&Json>) -> String {
    let urls = field(user, "avatarUrls");
    let pick = |k: &str| st(field(urls, k));
    let url = [pick("48x48"), pick("32x32"), pick("24x24")].into_iter().find(|u| !u.is_empty()).unwrap_or("");
    // /^https?:///i — 대소문자 무시는 ASCII 만(비 ASCII 문자는 ASCII 로 접히지 않는다)
    let starts_ci = |p: &str| url.as_bytes().get(..p.len()).is_some_and(|h| h.eq_ignore_ascii_case(p.as_bytes()));
    if utf16_len(url) <= AVATAR_URL_MAX && (starts_ci("http://") || starts_ci("https://")) {
        url.to_string()
    } else {
        String::new()
    }
}

/// 사진 주소 필드: 있을 때만 싣는다
fn with_avatar(o: &mut Vec<(&str, Json)>, key: &'static str, user: Option<&Json>) {
    let url = avatar_of(user);
    if !url.is_empty() {
        o.push((key, s(&url)));
    }
}

/// 날짜 문자열의 앞 `YYYY-MM-DD` 만. 형식이 다르면 빈 문자열
fn date_only(v: Option<&Json>) -> String {
    let t = st(v).as_bytes();
    let ok = t.len() >= 10 && t[..10].iter().enumerate().all(|(i, c)| if i == 4 || i == 7 { *c == b'-' } else { c.is_ascii_digit() });
    if ok {
        String::from_utf8_lossy(&t[..10]).into_owned()
    } else {
        String::new()
    }
}

pub fn map_summary(issue: &Json) -> Json {
    let i = Some(issue);
    let f = field(i, "fields");
    let status = field(f, "status");
    let mut o: Vec<(&str, Json)> = vec![
        ("key", s(st(field(i, "key")))),
        ("summary", s(st(field(f, "summary")))),
        ("status", s(st(field(status, "name")))),
        ("statusCategory", s(st(field(field(status, "statusCategory"), "key")))),
        ("issueType", s(st(field(field(f, "issuetype"), "name")))),
        ("priority", s(st(field(field(f, "priority"), "name")))),
        ("assignee", s(st(field(field(f, "assignee"), "displayName")))),
    ];
    with_avatar(&mut o, "assigneeAvatar", field(f, "assignee"));
    o.push(("updated", s(st(field(f, "updated")))));
    o.push(("dueDate", s(&date_only(field(f, "duedate")))));
    obj(o)
}

/// 서버가 줄 수 있는 평문(위키 마크업)을 문단 HTML 로. 렌더 HTML 이 없을 때의 대체
pub fn plain_to_html(text: &str) -> String {
    if js_trim(text).is_empty() {
        return String::new();
    }
    let escaped = text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
    format!("<p>{}</p>", escaped.replace("\r\n", "<br>").replace('\n', "<br>"))
}

/// 본문 하나의 HTML. Cloud 는 ADF 를 변환하고, Server 는 expand 로 받은 서버 렌더 HTML(없으면 평문)
pub fn body_html(kind: JiraKind, raw: Option<&Json>, rendered: Option<&Json>) -> String {
    match kind {
        JiraKind::Cloud => match raw {
            Some(Json::Str(t)) => plain_to_html(t),
            Some(doc) => adf_to_html(doc),
            None => String::new(),
        },
        JiraKind::Server => match rendered {
            Some(Json::Str(t)) if !t.is_empty() => t.clone(),
            _ => plain_to_html(st(raw)),
        },
    }
}

/// `myself`: 표시 이름 → 사용자 이름 → 이메일
pub fn myself_name(me: &Json) -> String {
    [fs(Some(me), "displayName"), fs(Some(me), "name"), fs(Some(me), "emailAddress")].into_iter().find(|t| !t.is_empty()).unwrap_or_default()
}

// ── 날짜 필드 ─────────────────────────────────────────────────────────────

/// 시작·종료 날짜 커스텀 필드 후보 id(우선순위 순, 없으면 빈 목록)
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DateFieldIds {
    pub start: Vec<String>,
    pub end: Vec<String>,
}

impl DateFieldIds {
    pub fn to_json(&self) -> Json {
        let list = |v: &[String]| Json::Arr(v.iter().map(|t| s(t)).collect());
        obj(vec![("start", list(&self.start)), ("end", list(&self.end))])
    }
}

/// `^customfield_\d+$`
fn is_custom_field(id: &str) -> bool {
    id.strip_prefix("customfield_").is_some_and(|d| !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit()))
}

/// 필드 목록에서 이름으로 날짜 필드 후보를 고른다. 순서: 이름 우선순위 > 같은 이름 안에서 scope 없는 전역 필드 > date·datetime 형식 > 응답 순서
pub fn pick_date_fields(list: &Json) -> DateFieldIds {
    let found: Vec<&Json> = arr_or_empty(Some(list)).iter().filter(|f| is_custom_field(st(field(Some(f), "id")))).collect();
    let ty = |f: &Json| st(field(field(Some(f), "schema"), "type")).to_string();
    let pick = |names: &[&str]| -> Vec<String> {
        let mut hits: Vec<(usize, u8, u8, usize, String)> = found
            .iter()
            .enumerate()
            .filter_map(|(order, f)| {
                let name = js_to_lowercase(js_trim(st(field(Some(f), "name"))));
                let idx = names.iter().position(|n| *n == name)?;
                let scoped = u8::from(truthy(field(Some(f), "scope")));
                let typed = u8::from(!matches!(ty(f).as_str(), "date" | "datetime"));
                Some((idx, scoped, typed, order, st(field(Some(f), "id")).to_string()))
            })
            .collect();
        hits.sort_by_key(|h| (h.0, h.1, h.2, h.3));
        let mut seen = HashSet::new();
        hits.into_iter().map(|h| h.4).filter(|id| seen.insert(id.clone())).take(MAX_DATE_FIELD_CANDIDATES).collect()
    };
    DateFieldIds { start: pick(&START_NAMES), end: pick(&END_NAMES) }
}

/// 후보 중 이 이슈에 값이 있는 첫 필드의 날짜
fn first_date(f: Option<&Json>, ids: &[String]) -> String {
    ids.iter().map(|id| date_only(field(f, id))).find(|d| !d.is_empty()).unwrap_or_default()
}

/// `getIssue`: 요약 + 주소(요청한 키 기준)·보고자·날짜·본문
pub fn issue_detail(kind: JiraKind, site_url: &str, key: &str, dates: &DateFieldIds, raw: &Json) -> Json {
    let r = Some(raw);
    let f = field(r, "fields");
    let Json::Obj(mut o) = map_summary(raw) else { unreachable!("map_summary 는 객체다") };
    o.push(("url".into(), s(&format!("{site_url}/browse/{}", encode_uri_component(key)))));
    o.push(("reporter".into(), s(st(field(field(f, "reporter"), "displayName")))));
    let mut extra: Vec<(&str, Json)> = Vec::new();
    with_avatar(&mut extra, "reporterAvatar", field(f, "reporter"));
    o.extend(extra.into_iter().map(|(k, v)| (k.to_string(), v)));
    o.push(("created".into(), s(st(field(f, "created")))));
    o.push(("startDate".into(), s(&first_date(f, &dates.start))));
    o.push(("endDate".into(), s(&first_date(f, &dates.end))));
    o.push(("descriptionHtml".into(), s(&body_html(kind, field(f, "description"), field(field(r, "renderedFields"), "description")))));
    Json::Obj(o)
}

/// 지금 실행 가능한 전환 목록(`id`·`name` 이 있는 것만)
pub fn transitions(raw: &Json) -> Vec<Json> {
    arr_or_empty(field(Some(raw), "transitions"))
        .iter()
        .filter_map(|x| {
            let t = Some(x);
            let to = field(t, "to");
            let (id, name) = (st(field(t, "id")), st(field(t, "name")));
            (!id.is_empty() && !name.is_empty()).then(|| {
                obj(vec![
                    ("id", s(id)),
                    ("name", s(name)),
                    ("to", obj(vec![("name", s(st(field(to, "name")))), ("category", s(st(field(field(to, "statusCategory"), "key"))))])),
                ])
            })
        })
        .collect()
}

// ── 댓글 ──────────────────────────────────────────────────────────────────

/// Data Center 는 첫 쪽의 total 이 5 를 넘으면 마지막 쪽(최신 N개)을 다시 받는다. 그때의 `startAt`(없으면 `None`)
pub fn server_comments_start_at(first: &Json) -> Option<f64> {
    let total = field(Some(first), "total").and_then(Json::as_f64).unwrap_or(0.0);
    (total > COMMENT_COUNT as f64).then(|| (total - COMMENT_COUNT as f64).max(0.0))
}

/// 댓글 응답 → 최신 5개. Cloud 는 `orderBy=-created` 로 이미 최신순이고, Data Center 는 받은 쪽을 뒤집는다
pub fn comments(kind: JiraKind, raw: &Json) -> Vec<Json> {
    let mut list: Vec<&Json> = arr_or_empty(field(Some(raw), "comments")).iter().collect();
    if kind == JiraKind::Server {
        list.reverse();
    }
    list.into_iter()
        .take(COMMENT_COUNT)
        .map(|x| {
            let cm = Some(x);
            let mut o: Vec<(&str, Json)> = vec![("author", s(st(field(field(cm, "author"), "displayName"))))];
            with_avatar(&mut o, "authorAvatar", field(cm, "author"));
            o.push(("created", s(st(field(cm, "created")))));
            o.push(("bodyHtml", s(&body_html(kind, field(cm, "body"), field(cm, "renderedBody")))));
            obj(o)
        })
        .collect()
}

// ── 개발 정보(dev-status) ─────────────────────────────────────────────────

fn is_ascii_digits(t: &str, min: usize, max: usize) -> bool {
    (min..=max).contains(&t.len()) && t.bytes().all(|b| b.is_ascii_digit())
}

/// 이슈 숫자 id(`^\d{1,18}$`). 아니면 `None`(개발 정보 없음)
pub fn dev_issue_id(issue: &Json) -> Option<String> {
    let id = st(field(Some(issue), "id"));
    is_ascii_digits(id, 1, 18).then(|| id.to_string())
}

/// 요약에서 연동 종류(`byInstanceType` 의 키, 최대 4개). `summary_raw` 는 요약 API 의 응답 본문이다
pub fn dev_instances(summary_raw: &Json, part: &str) -> Vec<String> {
    // `Object.keys(...)`: 객체는 JS 열거 순서, 배열은 인덱스 문자열
    let keys: Vec<String> = match field(field(field(Some(summary_raw), "summary"), part), "byInstanceType") {
        Some(Json::Arr(a)) => (0..a.len()).map(|i| i.to_string()).collect(),
        Some(b) => b.entries_js_order().into_iter().map(|(k, _)| k.to_string()).collect(),
        None => Vec::new(),
    };
    keys.into_iter()
        .filter(|k| (1..=40).contains(&k.len()) && k.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-')))
        .take(4)
        .collect()
}

fn https_url(v: Option<&Json>) -> String {
    match v {
        Some(Json::Str(t)) if t.strip_prefix("https://").is_some_and(|rest| !rest.is_empty() && !rest.chars().any(is_js_space)) => t.clone(),
        _ => String::new(),
    }
}

/// dev-status 의 시각은 ISO 문자열 또는 epoch ms 로 온다. 범위 밖 숫자는 TS 가 `RangeError` 를 던진다
fn iso_time(v: Option<&Json>) -> MapResult<String> {
    Ok(match v {
        Some(Json::Num(n)) if n.is_finite() => iso_string(*n).ok_or(MapError)?,
        Some(Json::Str(t)) if parse_date_ms(t).is_some() => t.clone(),
        _ => String::new(),
    })
}

fn pull_state(v: Option<&Json>) -> &'static str {
    match st(v).to_uppercase().as_str() {
        "OPEN" => "open",
        "MERGED" => "merged",
        _ => "closed",
    }
}

/// 삽입 순서를 지키는 맵: 같은 키는 처음 자리에 마지막 값(JS `Map.set`)
#[derive(Default)]
struct OrderedMap {
    index: std::collections::HashMap<String, usize>,
    list: Vec<(String, Json)>,
}

impl OrderedMap {
    fn set(&mut self, key: String, v: Json) {
        match self.index.get(&key) {
            Some(&i) => self.list[i].1 = v,
            None => {
                self.index.insert(key.clone(), self.list.len());
                self.list.push((key, v));
            }
        }
    }
}

fn by_time_desc(list: &mut [(String, Json)], at: &str) {
    let t = |j: &Json| j.get(at).and_then(Json::as_str).and_then(parse_date_ms).unwrap_or(0.0);
    list.sort_by(|a, b| t(&b.1).total_cmp(&t(&a.1)));
}

/// 개발 정보: `pull_details`·`repo_details` 는 연동 종류마다 받은 `detail` 배열의 항목을 이어 붙인 것이다.
/// PR 은 url(없으면 `저장소#제목`)로, 커밋은 sha 로 중복을 없애고, 최근순 최대 30개
pub fn dev_info(pull_details: &[Json], repo_details: &[Json]) -> MapResult<Json> {
    let mut pulls = OrderedMap::default();
    for d in pull_details {
        for x in arr_or_empty(field(Some(d), "pullRequests")) {
            let p = Some(x);
            let url = https_url(field(p, "url"));
            let (title, repo) = (fs(p, "name"), fs(p, "repositoryName"));
            let pull = obj(vec![
                ("title", s(&title)),
                ("url", s(&url)),
                ("state", s(pull_state(field(p, "status")))),
                ("author", s(st(field(field(p, "author"), "name")))),
                ("repo", s(&repo)),
                ("branch", s(st(field(field(p, "source"), "branch")))),
                ("updated", s(&iso_time(field(p, "lastUpdate"))?)),
            ]);
            let key = if url.is_empty() { format!("{repo}#{title}") } else { url };
            pulls.set(key, pull);
        }
    }
    let mut commits = OrderedMap::default();
    for d in repo_details {
        for r in arr_or_empty(field(Some(d), "repositories")) {
            let repo = Some(r);
            for x in arr_or_empty(field(repo, "commits")) {
                let cm = Some(x);
                let sha = st(field(cm, "id"));
                if !((7..=64).contains(&sha.len()) && sha.bytes().all(|b| b.is_ascii_hexdigit())) {
                    continue;
                }
                let first_line = st(field(cm, "message")).split('\n').next().unwrap_or("");
                let first_line = first_line.strip_suffix('\r').unwrap_or(first_line);
                let commit = obj(vec![
                    ("sha", s(sha)),
                    ("message", s(js_trim(first_line))),
                    ("url", s(&https_url(field(cm, "url")))),
                    ("author", s(st(field(field(cm, "author"), "name")))),
                    ("repo", s(st(field(repo, "name")))),
                    ("date", s(&iso_time(field(cm, "authorTimestamp"))?)),
                ]);
                commits.set(sha.to_string(), commit);
            }
        }
    }
    by_time_desc(&mut pulls.list, "updated");
    by_time_desc(&mut commits.list, "date");
    Ok(obj(vec![
        ("available", Json::Bool(true)),
        ("pulls", Json::Arr(pulls.list.into_iter().take(DEV_MAX).map(|e| e.1).collect())),
        ("commits", Json::Arr(commits.list.into_iter().take(DEV_MAX).map(|e| e.1).collect())),
    ]))
}

/// 개발 정보가 없을 때의 값
pub fn no_dev_info() -> Json {
    obj(vec![("available", Json::Bool(false)), ("pulls", Json::Arr(vec![])), ("commits", Json::Arr(vec![]))])
}

// ── 검색 ──────────────────────────────────────────────────────────────────

/// `search` 의 응답 → `{items, nextCursor?}`. Cloud 는 `nextPageToken`, Data Center 는 `startAt` 이 표지다.
/// `cursor` 는 Data Center 의 이전 표지(`Number(cursor)`)이고 정수·0..=`MAX_PAGES*PAGE_SIZE` 가 아니면 TS 는 던진다
pub fn search_result(kind: JiraKind, raw: &Json, cursor: Option<&str>, page: u32) -> MapResult<Json> {
    let r = Some(raw);
    let items: Vec<Json> = arr_or_empty(field(r, "issues")).iter().map(map_summary).collect();
    let max = MAX_PAGES;
    let mut o: Vec<(&str, Json)> = Vec::new();
    match kind {
        JiraKind::Cloud => {
            let next = st(field(r, "nextPageToken")).to_string();
            let cont = !next.is_empty() && !matches!(field(r, "isLast"), Some(Json::Bool(true))) && page < max;
            o.push(("items", Json::Arr(items)));
            if cont {
                o.push(("nextCursor", s(&next)));
            }
        }
        JiraKind::Server => {
            let start_at = match cursor {
                Some(c) if !c.is_empty() => js_number(c),
                _ => 0.0,
            };
            if !(start_at.fract() == 0.0 && start_at >= 0.0 && start_at <= f64::from(max * PAGE_SIZE)) {
                return Err(MapError);
            }
            let end = start_at + items.len() as f64;
            let total = field(r, "total").and_then(Json::as_f64).unwrap_or(end);
            let cont = !items.is_empty() && end < total && page < max;
            o.push(("items", Json::Arr(items)));
            if cont {
                o.push(("nextCursor", s(&crate::net::json::js_number_string(end))));
            }
        }
    }
    Ok(obj(o))
}

// ── 사용자 ────────────────────────────────────────────────────────────────

fn is_control(c: char) -> bool {
    c <= '\u{1f}' || c == '\u{7f}'
}

/// `getMyId`: Cloud 는 accountId, Data Center 는 사용자 이름(name). 길이·제어 문자만 본다
pub fn my_id(kind: JiraKind, me: &Json) -> MapResult<Json> {
    let m = Some(me);
    let id = match kind {
        JiraKind::Cloud => st(field(m, "accountId")),
        JiraKind::Server => st(field(m, "name")),
    };
    let len = utf16_len(id);
    if !(1..=255).contains(&len) || id.chars().any(is_control) {
        return Err(MapError);
    }
    Ok(user_json(id, st(field(m, "displayName"))))
}

fn user_json(id: &str, display: &str) -> Json {
    let name = if display.is_empty() { id } else { display };
    obj(vec![("id", s(id)), ("name", s(&slice_utf16_lossy(name, USER_NAME_MAX)))])
}

/// 사용자 검색(팀원 추가용) 응답. 최대 20명. Cloud 는 사람 계정(atlassian)만
pub fn search_users(kind: JiraKind, raw: &Json) -> Vec<Json> {
    let mut out = Vec::new();
    for x in arr_or_empty(Some(raw)) {
        let u = Some(x);
        if kind == JiraKind::Cloud && !matches!(field(u, "accountType"), Some(Json::Str(t)) if t == "atlassian") {
            continue;
        }
        let id = match kind {
            JiraKind::Cloud => st(field(u, "accountId")),
            JiraKind::Server => st(field(u, "name")),
        };
        if is_user_id(id) {
            out.push(user_json(id, st(field(u, "displayName"))));
        }
    }
    out.truncate(20);
    out
}

// ── 워크로그 ──────────────────────────────────────────────────────────────

/// 워크로그 comment 를 평문으로: Cloud 는 ADF 의 텍스트 노드를 이어 붙이고, Data Center 는 문자열 그대로
pub fn comment_text(v: Option<&Json>) -> String {
    if let Some(Json::Str(t)) = v {
        return slice_utf16_lossy(t, COMMENT_MAX);
    }
    fn walk(node: &Json, depth: usize, parts: &mut String, len: &mut usize) {
        if depth > 20 || *len > COMMENT_MAX || !matches!(node, Json::Obj(_) | Json::Arr(_)) {
            return;
        }
        let ty = st(field(Some(node), "type"));
        if let Some(Json::Str(t)) = field(Some(node), "text") {
            parts.push_str(t);
            *len += utf16_len(t);
        } else if ty == "hardBreak" {
            parts.push(' ');
        }
        for ch in arr_or_empty(field(Some(node), "content")) {
            walk(ch, depth + 1, parts, len);
        }
        if ty == "paragraph" {
            parts.push(' ');
        }
    }
    let (mut parts, mut len) = (String::new(), 0usize);
    if let Some(node) = v {
        walk(node, 0, &mut parts, &mut len);
    }
    slice_utf16_lossy(js_trim(&parts), COMMENT_MAX)
}

/// `'YYYY-MM-DD'` 에 일수를 더한다(UTC 달력). 날짜를 해석할 수 없으면 TS 는 `RangeError`
pub fn shift_day(ymd: &str, days: f64) -> MapResult<String> {
    let t = parse_date_ms(&format!("{ymd}T00:00:00Z")).ok_or(MapError)?;
    Ok(iso_string(t + days * DAY_MS).ok_or(MapError)?.chars().take(10).collect())
}

/// 워크로그를 찾은 이슈(내장 `fields.worklog` 포함)
#[derive(Clone, Debug)]
pub struct WorklogIssue {
    pub key: String,
    pub summary: String,
    pub issue_type: String,
    /// 내장 `fields.worklog.worklogs`
    pub embedded: Vec<Json>,
    /// 내장 worklog 가 전부인지. total 이 더 크면 이슈별로 다시 받아야 한다
    pub complete: bool,
}

pub fn worklog_issue(x: &Json) -> WorklogIssue {
    let i = Some(x);
    let f = field(i, "fields");
    let w = field(f, "worklog");
    let embedded = arr_or_empty(field(w, "worklogs")).to_vec();
    let total = field(w, "total").and_then(Json::as_f64).unwrap_or(embedded.len() as f64);
    WorklogIssue {
        key: fs(i, "key"),
        summary: fs(f, "summary"),
        issue_type: fs(field(f, "issuetype"), "name"),
        complete: total <= embedded.len() as f64,
        embedded,
    }
}

/// 검색 결과(한 쪽)의 이슈들: 키가 없거나 이미 본 키는 건너뛴다. 합쳐서 [`WORKLOG_ISSUE_MAX`] 건까지
pub fn worklog_issues(pages: &[&Json]) -> (Vec<WorklogIssue>, bool) {
    let mut issues = Vec::new();
    let mut seen = HashSet::new();
    for p in pages {
        for it in arr_or_empty(field(Some(p), "issues")) {
            let issue = worklog_issue(it);
            if issue.key.is_empty() || !seen.insert(issue.key.clone()) {
                continue;
            }
            issues.push(issue);
        }
    }
    let truncated = issues.len() > WORKLOG_ISSUE_MAX;
    issues.truncate(WORKLOG_ISSUE_MAX);
    (issues, truncated)
}

/// 이슈별 워크로그 응답의 항목에서 같은 id 는 한 번만(`fetchIssueWorklogs`)
pub fn dedupe_worklogs(list: &[Json]) -> Vec<&Json> {
    let mut seen = HashSet::new();
    list.iter().filter(|w| {
        let id = st(field(Some(w), "id"));
        id.is_empty() || seen.insert(id.to_string())
    }).collect()
}

pub struct WorklogQuery<'a> {
    pub from: &'a str,
    pub to: &'a str,
    pub users: &'a [String],
    pub include_me: bool,
}

/// `getWorklogs` 의 조립: 이슈마다 받은 워크로그(`entries`: 내장 또는 이슈별로 다시 받은 것)를 작성자·범위로 거르고 바꿔 담는다.
/// 범위는 `[from-2일 00:00Z, to+2일 00:00Z+14h)` 다(UTC+14 ~ UTC-12 를 덮는다). `me` 는 `{id, name}` 객체다
pub fn worklog_result(kind: JiraKind, q: &WorklogQuery<'_>, me: &Json, issues: &[(&WorklogIssue, Vec<&Json>)], truncated_in: bool) -> MapResult<Json> {
    let include_me = !q.users.is_empty() && q.include_me;
    // JQL 날짜를 만들 때 TS 는 해석할 수 없는 날짜에서 던진다
    shift_day(q.from, -2.0)?;
    shift_day(q.to, 2.0)?;
    let lo = parse_date_ms(&format!("{}T00:00:00Z", q.from)).ok_or(MapError)? - 2.0 * DAY_MS;
    let hi = parse_date_ms(&format!("{}T00:00:00Z", q.to)).ok_or(MapError)? + 2.0 * DAY_MS + 14.0 * 3_600_000.0;
    let me_id = st(field(Some(me), "id")).to_string();
    let mut wanted: HashSet<String> = if q.users.is_empty() { HashSet::from([me_id.clone()]) } else { q.users.iter().cloned().collect() };
    if include_me {
        wanted.insert(me_id);
    }
    let mut truncated = truncated_in;
    let mut count = 0usize;
    let mut worklogs: Vec<Json> = Vec::new();
    for (issue, entries) in issues {
        for x in entries {
            let w = Some(*x);
            let a = field(w, "author");
            let author_id = match kind {
                JiraKind::Cloud => st(field(a, "accountId")),
                JiraKind::Server => st(field(a, "name")),
            };
            if author_id.is_empty() || !(wanted.contains(author_id) || (kind == JiraKind::Server && wanted.contains(st(field(a, "key"))))) {
                continue;
            }
            let started = st(field(w, "started"));
            let Some(t) = parse_date_ms(started) else { continue };
            if t < lo || t >= hi {
                continue;
            }
            if count >= WORKLOG_MAX {
                truncated = true;
                continue;
            }
            count += 1;
            let display = st(field(a, "displayName"));
            worklogs.push(obj(vec![
                ("id", s(st(field(w, "id")))),
                ("issueKey", s(&issue.key)),
                ("issueSummary", s(&issue.summary)),
                ("issueType", s(&issue.issue_type)),
                ("authorId", s(author_id)),
                ("author", s(if display.is_empty() { author_id } else { display })),
                ("started", s(started)),
                ("seconds", field(w, "timeSpentSeconds").and_then(Json::as_f64).map_or(num(0.0), num)),
                ("comment", s(&comment_text(field(w, "comment")))),
            ]));
        }
    }
    Ok(obj(vec![("worklogs", Json::Arr(worklogs)), ("me", me.clone()), ("truncated", Json::Bool(truncated))]))
}
