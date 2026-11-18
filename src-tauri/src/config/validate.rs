//! `config:setUi` 패치의 검증기(`ConfigStore.ts` 22-102 의 1:1 이식). renderer 가 보낸 값이라 모양·크기·키를 모두 확인한다.
//!
//! 정규식은 외부 크레이트 없이 ASCII 규칙을 직접 쓴다(JS 의 `\d`·`[A-Z]` 는 ASCII 뿐이고 `$` 는 줄 끝 앞에서 맞지 않는다 — 전체 일치와 같다).
//! 길이는 JS `length`(UTF-16 코드 단위)다. 검사 **순서**가 어느 문구가 먼저 나오는지를 정하므로 [`check_set_ui`] 는 TS 의 순서를 그대로 따른다.
use std::collections::HashSet;

use crate::net::json::{is_integer, is_safe_integer, Json};

use super::model::{get, js_get, utf16_len};

const EDITORS: [&str; 4] = ["vscode", "cursor", "jetbrains", "custom"];
const MAX_LAUNCHPAD_PINS: usize = 200;
const MAX_LAUNCHPAD_SNOOZES: usize = 500;

/// `isStr(v, max)`
fn is_str(v: Option<&Json>, max: usize) -> bool {
    matches!(v, Some(Json::Str(s)) if utf16_len(s) <= max)
}

/// 정수형 숫자의 값(JS `Number.isInteger`)
fn int_of(v: &Json) -> Option<f64> {
    match v {
        Json::Num(n) if is_integer(*n) => Some(*n),
        _ => None,
    }
}

fn all_ascii(s: &str, f: impl Fn(u8) -> bool) -> bool {
    s.bytes().all(f)
}

fn is_upper(b: u8) -> bool {
    b.is_ascii_uppercase()
}

/// `/^[A-Z][A-Z0-9_]{1,19}$/` 에서 뒤쪽 `[A-Z0-9_]{min,max}` 부분
fn key_tail_ok(s: &str, min: usize, max: usize) -> bool {
    (min..=max).contains(&s.len()) && all_ascii(s, |b| is_upper(b) || b.is_ascii_digit() || b == b'_')
}

/// `/^[A-Z][A-Z0-9_]{1,19}$/`
fn is_project_key(s: &str) -> bool {
    s.as_bytes().first().is_some_and(|b| is_upper(*b)) && key_tail_ok(&s[1..], 1, 19)
}

/// `/^\d{1,9}$/`
fn is_issue_number(s: &str) -> bool {
    (1..=9).contains(&s.len()) && all_ascii(s, |b| b.is_ascii_digit())
}

/// `/^[a-z0-9._-]{1,100}$/`
fn is_slug(s: &str) -> bool {
    (1..=100).contains(&s.len()) && all_ascii(s, |b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'_' | b'-'))
}

/// Launchpad 항목 키(`@shared/launchpad` 의 `LAUNCHPAD_KEY_RE`): GitHub `owner/repo#번호`(소문자) 또는 Jira `jira:KEY-번호`
fn is_launchpad_key(k: &str) -> bool {
    if let Some(rest) = k.strip_prefix("jira:") {
        return is_jira_issue_key(rest);
    }
    let Some((slug, number)) = k.split_once('#') else { return false };
    let Some((owner, repo)) = slug.split_once('/') else { return false };
    is_slug(owner) && is_slug(repo) && is_issue_number(number)
}

/// `/^[A-Z][A-Z0-9_]{1,19}-\d{1,9}$/`
fn is_jira_issue_key(s: &str) -> bool {
    // 프로젝트 키에는 `-` 가 없으므로 첫 `-` 가 경계다
    let Some((project, number)) = s.split_once('-') else { return false };
    is_project_key(project) && is_issue_number(number)
}

/// 보일 저장소 그룹 목록의 모양 검사(`checkVisibleGroups`). 돌려주는 중복 제거 결과는 `setUi` 가 쓰지 않는다(TS 도 원래 값을 저장한다)
pub fn check_visible_groups(v: Option<&Json>) -> Result<Vec<String>, String> {
    let bad = |what: &str| Err(format!("잘못된 {what}입니다"));
    let Some(Json::Arr(items)) = v else { return bad("그룹 목록") };
    if items.len() > 50 {
        return bad("그룹 목록");
    }
    let mut out: Vec<String> = Vec::new();
    for g in items {
        // `!g.trim()` 은 JS trim — 공백 문자 집합이 Rust `trim` 과 다르다
        let Json::Str(s) = g else { return bad("그룹 이름") };
        if crate::git::fetch::js_trim(s).is_empty() || utf16_len(s) > 200 || s.contains('\0') {
            return bad("그룹 이름");
        }
        if !out.contains(s) {
            out.push(s.clone());
        }
    }
    Ok(out)
}

pub fn check_editor(v: Option<&Json>) -> Result<String, String> {
    match v {
        Some(Json::Str(s)) if EDITORS.contains(&s.as_str()) => Ok(s.clone()),
        _ => Err("잘못된 에디터입니다".to_string()),
    }
}

/// `Array.isArray(v)` 일 때의 항목
fn arr(v: &Json) -> Option<&[Json]> {
    match v {
        Json::Arr(a) => Some(a),
        _ => None,
    }
}

/// `typeof v === 'object' && v !== null && !Array.isArray(v)` 일 때의 항목(JS 열거 순서)
fn plain_obj(v: &Json) -> Option<Vec<(&str, &Json)>> {
    matches!(v, Json::Obj(_)).then(|| v.entries_js_order())
}

/// Jira 설정(프로젝트 키·저장 필터·최근 이슈·하루 표시 시각·보기)
fn check_jira_prefs(patch: &[(String, Json)]) -> Result<(), String> {
    let bad = || Err("잘못된 Jira 설정입니다".to_string());
    if let Some(keys) = get(patch, "jiraProjectKeys") {
        let ok = arr(keys).is_some_and(|k| k.len() <= 50 && k.iter().all(|x| matches!(x, Json::Str(s) if is_project_key(s))));
        if !ok {
            return bad();
        }
    }
    if let Some(layout) = get(patch, "jiraLayout") {
        if !matches!(layout, Json::Str(s) if s == "list" || s == "board") {
            return bad();
        }
    }
    if let Some(hidden) = get(patch, "jiraHiddenCategories") {
        let ok = arr(hidden).is_some_and(|h| {
            h.iter().all(|c| matches!(c, Json::Str(s) if s == "new" || s == "indeterminate" || s == "done"))
                && h.iter().filter_map(Json::as_str).collect::<HashSet<_>>().len() == h.len()
        });
        if !ok {
            return bad();
        }
    }
    // 워크로그 하루 표시 시각: 한쪽만 바꿔도 저장된 다른 쪽과 어긋나지 않게 호출부(setUi)가 합친 값을 다시 확인한다
    let ds = get(patch, "worklogDayStart");
    let de = get(patch, "worklogDayEnd");
    if let Some(ds) = ds {
        if !int_of(ds).is_some_and(|n| (0.0..=23.0).contains(&n)) {
            return bad();
        }
    }
    if let Some(de) = de {
        if !int_of(de).is_some_and(|n| (1.0..=24.0).contains(&n)) {
            return bad();
        }
    }
    if let (Some(ds), Some(de)) = (ds.and_then(int_of), de.and_then(int_of)) {
        if ds >= de {
            return bad();
        }
    }
    if let Some(wm) = get(patch, "worklogMode") {
        if !matches!(wm, Json::Str(s) if s == "day" || s == "week" || s == "month") {
            return bad();
        }
    }
    let Some(sites) = get(patch, "jiraSites") else { return Ok(()) };
    // `!sites || typeof sites !== 'object' || Array.isArray(sites) || Object.keys(sites).length > 20`
    let Some(entries) = plain_obj(sites).filter(|e| e.len() <= 20) else { return bad() };
    for (site, d) in entries {
        // `!d || typeof d !== 'object'` — 배열도 객체라 통과한다(아래 필드는 모두 없다)
        if utf16_len(site) > 300 || !matches!(d, Json::Obj(_) | Json::Arr(_)) {
            return bad();
        }
        if let Some(filters) = js_get(d, "filters") {
            let ok = arr(filters).is_some_and(|f| {
                f.len() <= 50 && f.iter().all(|x| is_str(js_get(x, "name"), 100) && is_str(js_get(x, "jql"), 2000))
            });
            if !ok {
                return bad();
            }
        }
        if let Some(team) = js_get(d, "team") {
            let ok = arr(team).is_some_and(|t| {
                t.len() <= 30
                    && t.iter().all(|u| matches!(js_get(u, "id"), Some(Json::Str(id)) if is_team_id(id)) && is_str(js_get(u, "name"), 200))
            });
            if !ok {
                return bad();
            }
        }
        if let Some(recent) = js_get(d, "recent") {
            let ok = arr(recent).is_some_and(|r| {
                r.len() <= 30 && r.iter().all(|x| matches!(js_get(x, "key"), Some(Json::Str(k)) if is_jira_issue_key(k)) && is_str(js_get(x, "summary"), 500))
            });
            if !ok {
                return bad();
            }
        }
    }
    Ok(())
}

/// `/^[A-Za-z0-9._@:+-]{1,128}$/`
fn is_team_id(s: &str) -> bool {
    (1..=128).contains(&s.len()) && all_ascii(s, |b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'@' | b':' | b'+' | b'-'))
}

/// Launchpad 핀·스누즈
fn check_launchpad_prefs(patch: &[(String, Json)]) -> Result<(), String> {
    let bad = || Err("잘못된 Launchpad 설정입니다".to_string());
    if let Some(pins) = get(patch, "launchpadPins") {
        let ok = arr(pins).is_some_and(|p| p.len() <= MAX_LAUNCHPAD_PINS && p.iter().all(|k| matches!(k, Json::Str(s) if is_launchpad_key(s))));
        if !ok {
            return bad();
        }
    }
    let Some(snoozes) = get(patch, "launchpadSnoozes") else { return Ok(()) };
    let Some(entries) = plain_obj(snoozes) else { return bad() };
    // `Number.isSafeInteger(until) && until > 0`
    let ok = entries.len() <= MAX_LAUNCHPAD_SNOOZES
        && entries.iter().all(|(k, until)| is_launchpad_key(k) && matches!(until, Json::Num(n) if is_safe_integer(*n) && *n > 0.0));
    if ok {
        Ok(())
    } else {
        bad()
    }
}

/// 저장소 목록 순서·색(그룹 칩·환경 배지)
fn check_repo_list_prefs(patch: &[(String, Json)]) -> Result<(), String> {
    let bad = || Err("잘못된 저장소 목록 설정입니다".to_string());
    // 저장소·그룹 이름 한 개(`isName`)
    fn is_name(s: &str) -> bool {
        !s.is_empty() && utf16_len(s) <= 200 && !s.contains('\0')
    }
    let is_list = |v: &Json, max: usize| {
        arr(v).is_some_and(|a| {
            a.len() <= max && a.iter().all(|x| matches!(x, Json::Str(s) if is_name(s))) && a.iter().filter_map(Json::as_str).collect::<HashSet<_>>().len() == a.len()
        })
    };
    // `isMap(v, max)`: 객체이고(배열 아님) 키가 max 개 이하이며 모든 키가 이름이다
    fn map_of(v: &Json, max: usize) -> Option<Vec<(&str, &Json)>> {
        plain_obj(v).filter(|e| e.len() <= max && e.iter().all(|(k, _)| is_name(k)))
    }
    let is_hue = |v: &Json| matches!(v, Json::Num(n) if is_integer(*n) && (0.0..360.0).contains(n));
    for key in ["groupColors", "sourceColors"] {
        if let Some(colors) = get(patch, key) {
            if !map_of(colors, 500).is_some_and(|e| e.iter().all(|(_, v)| is_hue(v))) {
                return bad();
            }
        }
    }
    if let Some(v) = get(patch, "sourceOrder") {
        if !is_list(v, 500) {
            return bad();
        }
    }
    if let Some(v) = get(patch, "repoOrder") {
        if !is_list(v, 5000) {
            return bad();
        }
    }
    if let Some(v) = get(patch, "groupOrder") {
        if !map_of(v, 500).is_some_and(|e| e.iter().all(|(_, g)| is_list(g, 500))) {
            return bad();
        }
    }
    Ok(())
}

/// `ConfigStore.setUi` 의 검사 전부(병합 전). TS 와 같은 순서라 두 가지가 틀린 패치는 같은 문구로 거부된다
pub fn check_set_ui(patch: &[(String, Json)]) -> Result<(), String> {
    // renderer 가 보낸 값이다 — main 이 그대로 쓰는 값은 형식을 확인한다
    if let Some(cid) = get(patch, "githubClientId") {
        let ok = matches!(cid, Json::Str(s) if (1..=100).contains(&s.len()) && all_ascii(s, |b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-')));
        if !ok {
            return Err("잘못된 OAuth App client ID 입니다(영문·숫자·._- 100자 이내)".to_string());
        }
    }
    // 건너뛴 업데이트 버전은 main(UpdateService)만 쓴다 — renderer 가 알림을 영구히 끄지 못하게 막는다(`in` 검사라 값이 없어도 키가 있으면 거부)
    if super::model::has_key(patch, "skippedUpdateVersion") {
        return Err("이 설정은 직접 바꿀 수 없습니다".to_string());
    }
    if let Some(af) = get(patch, "autoFetchMinutes") {
        if !int_of(af).is_some_and(|n| (0.0..=1440.0).contains(&n)) {
            return Err("잘못된 자동 fetch 주기입니다(0~1440분)".to_string());
        }
    }
    // 에디터 실행 파일은 main 만 정한다 — renderer 가 임의 프로그램을 가리키지 못하게 한다
    if super::model::has_key(patch, "editorExePath") {
        return Err("이 설정은 직접 바꿀 수 없습니다".to_string());
    }
    if let Some(e) = get(patch, "editor") {
        check_editor(Some(e))?;
    }
    if let Some(g) = get(patch, "visibleGroups") {
        check_visible_groups(Some(g))?;
    }
    check_jira_prefs(patch)?;
    check_launchpad_prefs(patch)?;
    check_repo_list_prefs(patch)
}

/// 에디터 실행 파일 경로(`setEditorExePath`)의 모양: 비어 있지 않은 문자열, 4096 단위 이하, NUL 없음
pub fn check_editor_exe_path(s: &str) -> bool {
    !s.is_empty() && utf16_len(s) <= 4096 && !s.contains('\0')
}

/// `/^\d+\.\d+\.\d+(-[0-9A-Za-z.-]{1,40})?$/`
pub fn is_version(s: &str) -> bool {
    let (core, pre) = match s.split_once('-') {
        Some((c, p)) => (c, Some(p)),
        None => (s, None),
    };
    let parts: Vec<&str> = core.split('.').collect();
    let nums = parts.len() == 3 && parts.iter().all(|p| !p.is_empty() && all_ascii(p, |b| b.is_ascii_digit()));
    let pre_ok = pre.is_none_or(|p| (1..=40).contains(&p.len()) && all_ascii(p, |b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-')));
    nums && pre_ok
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::json::parse;

    fn patch(s: &str) -> Vec<(String, Json)> {
        match parse(s).unwrap() {
            Json::Obj(o) => o,
            _ => panic!("객체가 아니다"),
        }
    }

    #[test]
    fn check_visible_groups_는_모양과_크기를_확인하고_중복을_없앤다() {
        let v = |s: &str| check_visible_groups(Some(&parse(s).unwrap()));
        assert!(check_visible_groups(Some(&Json::Str("a".into()))).is_err());
        assert!(check_visible_groups(None).is_err());
        assert!(v(&format!(r#"["{}"]"#, "x".repeat(201))).is_err());
        assert!(v(&format!(r#"["{}"]"#, "x".repeat(200))).is_ok());
        let many = |n: usize| format!("[{}]", (0..n).map(|i| format!("\"g{i}\"")).collect::<Vec<_>>().join(","));
        assert!(v(&many(51)).is_err());
        assert!(v(&many(50)).is_ok());
        assert_eq!(v(r#"["a","a","b"]"#).unwrap(), ["a", "b"]);
    }

    #[test]
    fn check_editor_는_알려진_에디터만_받는다() {
        assert!(check_editor(Some(&Json::Str("notepad".into()))).is_err());
        assert!(check_editor(None).is_err());
        assert_eq!(check_editor(Some(&Json::Str("jetbrains".into()))).unwrap(), "jetbrains");
    }

    /// 골든에 넣기엔 큰 상한(수천 항목)의 경계
    #[test]
    fn 큰_상한의_경계를_지킨다() {
        let list = |n: usize| format!(r#"{{"repoOrder":[{}]}}"#, (0..n).map(|i| format!("\"r{i}\"")).collect::<Vec<_>>().join(","));
        assert!(check_set_ui(&patch(&list(5000))).is_ok());
        assert_eq!(check_set_ui(&patch(&list(5001))).unwrap_err(), "잘못된 저장소 목록 설정입니다");
    }

    #[test]
    fn 버전_문자열은_정규식과_같다() {
        for ok in ["1.2.3", "10.20.30-beta.1", "0.0.0-a-b"] {
            assert!(is_version(ok), "{ok}");
        }
        for bad in ["1.2", "1.2.3.4", "v1.2.3", "1.2.3-", "1..3", "1.2.3-é", "1.2.3\n", ""] {
            assert!(!is_version(bad), "{bad:?}");
        }
    }
}
