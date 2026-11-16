//! 접근 가능한 GitHub 저장소 목록의 응답 변환(`src/main/github/accessibleRepos.ts` 의 순수 부분). 쪽 읽기·`truncated`·캐시는 서비스 계층(뒤 PR)의 몫이다.
use crate::net::js_date::parse_date_ms;
use crate::net::js_value::{field, num, obj, s};
use crate::net::json::Json;

/// 읽는 쪽 수 상한(저장소 2000개). 넘으면 truncated
pub const MAX_ACCESSIBLE_PAGES: usize = 20;

/// `str(v)`: 비어 있지 않은 문자열만
fn str_nonempty(v: Option<&Json>) -> Option<&str> {
    match v {
        Some(Json::Str(t)) if !t.is_empty() => Some(t),
        _ => None,
    }
}

fn opt_str(v: Option<&str>) -> Json {
    v.map_or(Json::Null, s)
}

fn is_true(v: Option<&Json>) -> bool {
    matches!(v, Some(Json::Bool(true)))
}

/// `mapAccessibleRepo`: 필수 필드가 없거나 형식이 이상하면 `None`(건너뛴다). `pushedAt` 은 ms 다
pub fn map_accessible_repo(raw: &Json) -> Option<Json> {
    if !matches!(raw, Json::Obj(_) | Json::Arr(_)) {
        return None;
    }
    let owner_obj = field(Some(raw), "owner");
    let owner = str_nonempty(field(owner_obj, "login"))?;
    let repo = str_nonempty(field(Some(raw), "name"))?;
    let html_url = str_nonempty(field(Some(raw), "html_url"))?;
    let pushed = match field(Some(raw), "pushed_at") {
        Some(Json::Str(t)) => parse_date_ms(t),
        _ => None,
    };
    let full_name = str_nonempty(field(Some(raw), "full_name")).map_or_else(|| format!("{owner}/{repo}"), str::to_string);
    Some(obj(vec![
        ("owner", s(owner)),
        ("repo", s(repo)),
        ("fullName", s(&full_name)),
        ("ownerType", s(if matches!(field(owner_obj, "type"), Some(Json::Str(t)) if t == "Organization") { "Organization" } else { "User" })),
        ("private", Json::Bool(is_true(field(Some(raw), "private")))),
        ("fork", Json::Bool(is_true(field(Some(raw), "fork")))),
        ("archived", Json::Bool(is_true(field(Some(raw), "archived")))),
        ("defaultBranch", s(str_nonempty(field(Some(raw), "default_branch")).unwrap_or("main"))),
        ("description", opt_str(str_nonempty(field(Some(raw), "description")))),
        ("pushedAt", num(pushed.unwrap_or(0.0))),
        ("htmlUrl", s(html_url)),
    ]))
}

/// 응답 한 쪽: 배열이 아니면 빈 목록, 항목마다 매핑해 건너뛸 것은 뺀다
pub fn accessible_page(data: &Json) -> Vec<Json> {
    match data {
        Json::Arr(a) => a.iter().filter_map(map_accessible_repo).collect(),
        _ => Vec::new(),
    }
}

/// `pushedAt` 내림차순(안정 정렬 — 같으면 읽은 순서)
pub fn sort_accessible(repos: &mut [Json]) {
    let at = |x: &Json| x.get("pushedAt").and_then(Json::as_f64).unwrap_or(0.0);
    repos.sort_by(|a, b| at(b).total_cmp(&at(a)));
}
