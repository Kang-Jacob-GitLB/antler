//! 설정 파일 내용의 해석과 정화(`ConfigStore.ts` 104-131·303-355 의 이식).
//!
//! TS 는 `JSON.parse` 결과를 느슨하게 다룬다 — 모양이 틀려도 **던지지 않는 경우**가 많고(`raw.repos` 가 문자열이면 글자를 훑어 모두 건너뛴다),
//! 던지는 경우(`null.repos`·`sources` 안의 `null`·반복할 수 없는 `repos`)만 "손상 파일"이 된다. 이 구분을 그대로 옮긴다 — 더 엄격하면 멀쩡한 설정을 버리고,
//! 더 느슨하면 TS 가 막던 값을 저장한다. 반환 `Err(Corrupt)` 는 TS 의 `catch`(손상 처리) 에 해당한다.
use std::collections::HashSet;

use crate::git::github_link::{github_cache_path, sanitize_github_link, GITHUB_SOURCE_ID};
use crate::net::js_value::{stringify, truthy};
use crate::net::json::{is_integer, is_safe_integer, parse, Json};

use super::model::{default_ui, defaults, get, has_key, is_undefined, js_get, remove, set, source_for, spread, Obj, LOCAL_SOURCE_ID, UNDEFINED};

/// 손상된 설정 파일(TS 의 `catch`) — 해석에 실패했거나 TS 라면 `TypeError` 를 던졌을 모양
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Corrupt;

/// 저장된 창 위치·크기의 모양 검사(`sanitizeWindowState`). 이상하면 `None`(기본 크기로 연다).
/// Tauri 는 창 상태를 window-state 플러그인이 맡아 쓰지 않지만, Electron 판과 같은 모양으로 왕복시킨다
pub fn sanitize_window_state(v: Option<&Json>) -> Option<Json> {
    // `!v || typeof v !== 'object'` — 배열은 객체지만 필드가 없어 아래에서 걸린다
    let v = v.filter(|x| matches!(x, Json::Obj(_) | Json::Arr(_)))?;
    let int = |key: &str| match js_get(v, key) {
        Some(Json::Num(n)) if is_safe_integer(*n) && n.abs() <= 100_000.0 => Some(*n),
        _ => None,
    };
    let (x, y, width, height) = (int("x")?, int("y")?, int("width")?, int("height")?);
    if width <= 0.0 || height <= 0.0 {
        return None;
    }
    let mut o: Obj = ["x", "y", "width", "height"].iter().zip([x, y, width, height]).map(|(k, n)| ((*k).to_string(), Json::Num(n))).collect();
    if matches!(js_get(v, "maximized"), Some(Json::Bool(true))) {
        o.push(("maximized".to_string(), Json::Bool(true)));
    }
    Some(Json::Obj(o))
}

/// 저장된 저장소 목록의 GitHub 링크를 검증한다. github 소스의 저장소인데 링크가 틀렸으면 fetch 할 대상도 URL 을 확인할 기준도
/// 없으므로 그 항목을 버리고, github 소스가 아닌 저장소에 붙은 링크는 필드만 버린다
fn sanitize_repos(repos: Option<&Json>, cache_dir: Option<&str>) -> Result<Vec<Json>, Corrupt> {
    // `for (const r of repos)` — 배열은 항목, 문자열은 글자(모두 비객체라 건너뛴다), null·없음은 `?? []`, 그 밖은 반복할 수 없어 던진다
    let items: &[Json] = match repos {
        None | Some(Json::Null | Json::Str(_)) => &[],
        Some(Json::Arr(a)) => a,
        Some(_) => return Err(Corrupt),
    };
    let mut out = Vec::new();
    let mut github_paths: HashSet<String> = HashSet::new();
    for r in items {
        // null·비객체 항목 하나가 설정 전체를 손상 처리하지 않게 그 항목만 건너뛴다
        let Json::Obj(o) = r else { continue };
        if matches!(get(o, "sourceId"), Some(Json::Str(s)) if s == GITHUB_SOURCE_ID) {
            let Some(link) = sanitize_github_link(get(o, "github")) else { continue };
            // 캐시 경로는 저장된 절대경로를 믿지 않고 이 PC 의 캐시 폴더와 링크로 다시 정한다(설정 파일을 다른 PC·사용자로 옮겨도 맞도록).
            // 대소문자만 다른 같은 저장소는 같은 경로가 되므로 처음 것만 남긴다
            let path = match cache_dir {
                Some(dir) => Json::Str(github_cache_path(dir, &link.owner, &link.repo)),
                None => get(o, "path").cloned().unwrap_or(UNDEFINED),
            };
            let key = if is_undefined(&path) { "undefined".to_string() } else { stringify(&path) };
            if !github_paths.insert(key) {
                continue;
            }
            let mut next = o.clone();
            set(&mut next, "path", path);
            set(&mut next, "github", link.to_json());
            out.push(Json::Obj(next));
        } else if has_key(o, "github") {
            let mut rest = o.clone();
            remove(&mut rest, "github");
            out.push(Json::Obj(rest));
        } else {
            out.push(r.clone());
        }
    }
    Ok(out)
}

/// `raw.sources?.length ? raw.sources : base.sources` 와 이어지는 `.filter(...)`. 던지는 입력은 `Err`
fn sanitize_sources(raw: Option<&Json>, base: &[Json]) -> Result<Vec<Json>, Corrupt> {
    let list: &[Json] = match raw {
        Some(Json::Arr(a)) if !a.is_empty() => a,
        // 길이가 참인 문자열·객체는 `.filter` 가 없어 던진다
        Some(Json::Str(s)) if !s.is_empty() => return Err(Corrupt),
        Some(Json::Obj(o)) if truthy(get(o, "length")) => return Err(Corrupt),
        _ => base,
    };
    let mut out = Vec::new();
    for s in list {
        // `s.id` — null 이면 던진다. 객체가 아닌 값은 `id`·`kind` 가 없어 그대로 남는다
        if matches!(s, Json::Null) {
            return Err(Corrupt);
        }
        let is_github = |key: &str| matches!(js_get(s, key), Some(Json::Str(v)) if v == GITHUB_SOURCE_ID);
        if !is_github("id") && !is_github("kind") {
            out.push(s.clone());
        }
    }
    Ok(out)
}

/// 설정 파일 내용을 정화된 설정으로 만든다. `Err(Corrupt)` 는 손상(해석 실패 또는 TS 라면 던졌을 모양)이다
pub fn parse_config(text: &str, cache_dir: Option<&str>) -> Result<Obj, Corrupt> {
    let raw = parse(text).map_err(|_| Corrupt)?;
    // `raw.repos` 가 null 에서는 던진다. 숫자·문자열·배열 등은 속성이 없어 빈 객체처럼 읽힌다
    if matches!(raw, Json::Null) {
        return Err(Corrupt);
    }
    let base = defaults();
    let base_sources: Vec<Json> = match get(&base, "sources") {
        Some(Json::Arr(a)) => a.clone(),
        _ => Vec::new(),
    };
    let repos = sanitize_repos(js_get(&raw, "repos"), cache_dir)?;
    // github 소스는 정해진 모양 하나뿐이고, 저장소가 있을 때만 존재한다(파일에서 바뀐 값은 믿지 않고 다시 만든다)
    let has_github = repos.iter().any(|r| matches!(js_get(r, "sourceId"), Some(Json::Str(s)) if s == GITHUB_SOURCE_ID));
    let mut sources = sanitize_sources(js_get(&raw, "sources"), &base_sources)?;
    if has_github {
        sources.push(source_for("github", None).map_err(|_| Corrupt)?);
    }
    if !sources.iter().any(|s| matches!(js_get(s, "id"), Some(Json::Str(v)) if v == LOCAL_SOURCE_ID)) {
        sources.insert(0, base_sources[0].clone());
    }
    let mut ui = default_ui();
    for (k, v) in spread(js_get(&raw, "ui")) {
        set(&mut ui, &k, v);
    }
    // 예전 버전이 남긴 프로필 표시는 버린다(다음 저장 때 파일에서도 사라진다. raw.profiles 도 옮기지 않는다)
    remove(&mut ui, "activeProfileId");
    // 손상된 하루 표시 시각은 두 키 모두 버려 기본값으로 돌린다(그대로 두면 이후 setUi 가 계속 실패한다)
    let (ds, de) = (get(&ui, "worklogDayStart").cloned(), get(&ui, "worklogDayEnd").cloned());
    if ds.is_some() || de.is_some() {
        let int = |v: &Option<Json>| match v {
            Some(Json::Num(n)) if is_integer(*n) => Some(*n),
            _ => None,
        };
        let ok = matches!((int(&ds), int(&de)), (Some(s), Some(e)) if s >= 0.0 && e <= 24.0 && s < e);
        if !ok {
            remove(&mut ui, "worklogDayStart");
            remove(&mut ui, "worklogDayEnd");
        }
    }
    // 알 수 없는 워크로그 보기 값은 버린다(기본 월 보기)
    if let Some(wm) = get(&ui, "worklogMode") {
        if !matches!(wm, Json::Str(s) if s == "day" || s == "week" || s == "month") {
            remove(&mut ui, "worklogMode");
        }
    }
    let mut config: Obj = vec![
        ("version".to_string(), Json::Num(1.0)),
        ("sources".to_string(), Json::Arr(sources)),
        ("repos".to_string(), Json::Arr(repos)),
        ("ui".to_string(), Json::Obj(ui)),
    ];
    for key in ["fetchApprovals", "trustedConfigs"] {
        let v = js_get(&raw, key);
        if truthy(v) {
            config.push((key.to_string(), v.cloned().unwrap_or(Json::Null)));
        }
    }
    if let Some(w) = sanitize_window_state(js_get(&raw, "window")) {
        config.push(("window".to_string(), w));
    }
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 손상과_비손상_모양을_ts_처럼_가른다() {
        for text in ["null", "{", "", r#"{"repos":5}"#, r#"{"repos":{}}"#, r#"{"sources":[null]}"#, r#"{"sources":"x"}"#, "\u{feff}{}"] {
            assert!(parse_config(text, None).is_err(), "{text:?}");
        }
        for text in ["5", r#""s""#, "[]", "{}", r#"{"repos":"abc"}"#, r#"{"repos":[null,1,[]]}"#, r#"{"sources":[1,"a"]}"#, r#"{"sources":5}"#] {
            assert!(parse_config(text, None).is_ok(), "{text:?}");
        }
    }

    #[test]
    fn 창_상태는_정수_좌표와_양수_크기만_받는다() {
        let w = |s: &str| sanitize_window_state(Some(&parse(s).unwrap()));
        assert_eq!(w(r#"{"x":-8,"y":10,"width":1200,"height":800,"maximized":false}"#), Some(parse(r#"{"x":-8,"y":10,"width":1200,"height":800}"#).unwrap()));
        assert!(w(r#"{"x":0,"y":0,"width":0,"height":800}"#).is_none());
        assert!(w(r#"{"x":0.5,"y":0,"width":1,"height":1}"#).is_none());
        assert!(w(r#"{"x":"1","y":0,"width":1,"height":1}"#).is_none());
        assert!(sanitize_window_state(None).is_none());
    }
}
