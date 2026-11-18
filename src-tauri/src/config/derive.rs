//! 설정에서 파생하는 값: Rust 라우터 미러(`src/sidecar/mirror.ts` 의 `mirrorOf`)와 fetch 설정(`src/sidecar/fetchPrefs.ts` 의 `fetchPrefsOf`).
//! 지금은 사이드카가 이 값을 `repos`·`fetchPrefs` 줄로 보낸다. Rust 가 설정을 소유하면(PR 9) 같은 함수로 미러·fetch 서비스에 직접 넘긴다.
use crate::mirror::MirrorRepo;
use crate::net::json::Json;

use super::model::js_get;

/// 미러 한 줄: 라우터가 쓰는 값과 사용자가 붙인 이름(확인 창 제목에만 쓴다)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MirrorEntry {
    pub repo: MirrorRepo,
    pub name: Option<String>,
    /// github 소스 저장소의 owner/repo 링크(기본 브랜치는 싣지 않는다 — 바뀌어도 미러가 바뀌지 않게). github 소스가 아니면 없다
    pub github: Option<(String, String)>,
}

fn same(a: Option<&Json>, b: Option<&Json>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(Json::Str(x)), Some(Json::Str(y))) => x == y,
        (Some(Json::Num(x)), Some(Json::Num(y))) => x == y,
        (Some(Json::Bool(x)), Some(Json::Bool(y))) => x == y,
        (Some(Json::Null), Some(Json::Null)) => true,
        _ => false,
    }
}

fn str_of(v: Option<&Json>) -> Option<String> {
    v.and_then(Json::as_str).map(str::to_string)
}

/// 설정의 저장소를 미러 모양으로 바꾼다(wsl 은 distro, ssh 는 sshHost 를 함께 담는다). 소스를 못 찾은 저장소는 라우터가 쓸 수 없으므로 뺀다.
/// 라우터가 받는 쪽(`RepoMirror::apply_message`)이 문자열이 아닌 id·kind·path 의 항목을 버리므로 여기서 먼저 뺀다
pub fn mirror_of(config: &Json) -> Vec<MirrorEntry> {
    let (Some(Json::Arr(repos)), Some(Json::Arr(sources))) = (js_get(config, "repos"), js_get(config, "sources")) else { return Vec::new() };
    let mut out = Vec::new();
    for r in repos {
        let Some(s) = sources.iter().find(|x| same(js_get(x, "id"), js_get(r, "sourceId"))) else { continue };
        let (Some(id), Some(kind), Some(path)) = (str_of(js_get(r, "id")), str_of(js_get(s, "kind")), str_of(js_get(r, "path"))) else { continue };
        let distro = if kind == "wsl" { str_of(js_get(s, "distro")).filter(|d| !d.is_empty()) } else { None };
        let ssh_host = if kind == "ssh" { str_of(js_get(s, "sshHost")).filter(|d| !d.is_empty()) } else { None };
        let github = if kind == "github" {
            js_get(r, "github").and_then(|g| Some((str_of(js_get(g, "owner"))?, str_of(js_get(g, "repo"))?)))
        } else {
            None
        };
        out.push(MirrorEntry { repo: MirrorRepo { id, kind, path, distro, ssh_host }, name: str_of(js_get(r, "name")), github });
    }
    out
}

/// 자동 fetch 주기 기본값(분) — `FetchService.DEFAULT_AUTO_FETCH_MINUTES`
pub const DEFAULT_AUTO_FETCH_MINUTES: f64 = 5.0;

/// fetch 서비스가 쓸 설정 값(지문의 모양 검사는 받는 쪽 `FetchPrefs` 가 한다)
#[derive(Debug, Clone, PartialEq)]
pub struct FetchPrefsOf {
    /// 이미 기본값(5분)을 적용한 값이며, 음수·무한·NaN 은 0(끔)이다 — 기본값을 정하는 곳은 여기 한 곳이다
    pub auto_fetch_minutes: f64,
    /// 저장소 id 순(UTF-16 코드 단위 순)으로, 이 저장소 목록 안의 id 만(저장소가 사라지면 그 승인도 함께 사라진다)
    pub approvals: Vec<(String, String)>,
    pub trusted: Vec<(String, String)>,
}

fn only_known(map: Option<&Json>, config: &Json) -> Vec<(String, String)> {
    let Some(Json::Arr(repos)) = js_get(config, "repos") else { return Vec::new() };
    let mut ids: Vec<&str> = repos.iter().filter_map(|r| js_get(r, "id").and_then(Json::as_str)).collect();
    ids.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
    ids.dedup();
    let Some(map) = map else { return Vec::new() };
    ids.into_iter().filter_map(|id| js_get(map, id).and_then(Json::as_str).map(|v| (id.to_string(), v.to_string()))).collect()
}

pub fn fetch_prefs_of(config: &Json) -> FetchPrefsOf {
    let ui = js_get(config, "ui");
    // `config.ui.autoFetchMinutes ?? 5`
    let m = match ui.and_then(|u| js_get(u, "autoFetchMinutes")) {
        None | Some(Json::Null) => DEFAULT_AUTO_FETCH_MINUTES,
        Some(Json::Num(n)) => *n,
        // 숫자가 아닌 값은 `Number.isFinite` 가 거짓이라 0
        Some(_) => 0.0,
    };
    FetchPrefsOf {
        auto_fetch_minutes: if m.is_finite() && m > 0.0 { m } else { 0.0 },
        approvals: only_known(js_get(config, "fetchApprovals"), config),
        trusted: only_known(js_get(config, "trustedConfigs"), config),
    }
}
