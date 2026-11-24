//! 보기 범위(Solo / Hide) 검증과 `git log` 인자 만들기. `src/shared/logScope.ts` 와 `src/main/git/commands.ts` 의
//! `isValidScopeRef`·`isRemoteAll`·`scopeArgs`·`logArgs`·`scopeKey` 의 이식이다.
//!
//! 보안(인자 주입 경계): ref 이름은 renderer 가 보낸 값이다. 실행기가 인자를 전역 옵션 바로 뒤에 붙이므로 옵션으로 읽히는 값이
//! 들어가면 `-c alias.x=!cmd`·`--output=<경로>` 가 된다. 그래서
//! - 항목은 `refs/(heads|tags|remotes)/…` 형식·40/64자 소문자 해시·`refs/remotes/<이름>/*` 만 통과한다(그 밖의 문자열은 argv 를 만들 방법이 없다).
//! - 통과한 값도 옵션 자리에 오지 않는다: solo 의 rev 는 `refs/` 나 16진수로 시작해 `-` 로 시작할 수 없고, glob·exclude 는 `--glob=`·`--exclude=`
//!   접두가 붙은 한 인자로만 들어간다. `--end-of-options` 는 쓰지 않는다 — git 2.24 미만(예: Ubuntu 18.04 의 2.17)의 `log` 가
//!   `unrecognized argument` 로 거부해 SSH 저장소의 solo 보기가 실패했다.
//!
//! 검증은 TS 정규식과 같은 결과여야 한다. 정규식 크레이트를 쓰지 않고 손으로 검사한다 — JS `\s`(U+FEFF 포함, U+0085 제외)는
//! Rust `char::is_whitespace`(U+0085 포함, U+FEFF 제외)와 달라서 [`is_js_space`] 로 나열하고, 길이는 UTF-16 코드 단위로 센다.

use serde_json::{Map, Value};

use crate::git::parsers::log::LOG_FORMAT_ARGS;

/// 보기 범위 한 목록의 항목 수 상한(`SCOPE_MAX_ITEMS`)
pub const SCOPE_MAX_ITEMS: usize = 200;
/// ref 항목 길이 상한(UTF-16 코드 단위)
const MAX_REF_LEN: usize = 1024;

/// JS 정규식 `\s`: 공백·줄바꿈 + `     -          　 ﻿`
pub(crate) fn is_js_space(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n' | '\u{b}' | '\u{c}' | '\r' | ' ' | '\u{a0}' | '\u{1680}' | '\u{2000}'..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}' | '\u{feff}'
    )
}

/// `[^\s\x00-\x1f\x7f~^:?*[\\]` — ref 이름에 쓸 수 없는 문자를 뺀 나머지
fn is_name_char(c: char) -> bool {
    !is_js_space(c) && !matches!(c, '\u{0}'..='\u{1f}' | '\u{7f}' | '~' | '^' | ':' | '?' | '*' | '[' | '\\')
}

/// `^(?:[0-9a-f]{40}|[0-9a-f]{64})$` — 소문자만
pub fn is_hash(s: &str) -> bool {
    (s.len() == 40 || s.len() == 64) && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// ref 이름에 쓸 수 없는 모양: `..`, `@{`, 빈 구성 요소(`//`), 끝 `/`, `.lock`, `.` 로 시작하는 구성 요소
fn bad_shape(s: &str) -> bool {
    s.contains("..") || s.contains("@{") || s.contains("//") || s.ends_with('/') || s.ends_with(".lock") || s.contains("/.")
}

/// `^refs\/(heads|tags|remotes)\/<이름 문자>+$`
fn is_scope_ref_shape(s: &str) -> bool {
    let Some(rest) = s.strip_prefix("refs/") else { return false };
    let Some(name) = ["heads/", "tags/", "remotes/"].iter().find_map(|p| rest.strip_prefix(p)) else { return false };
    !name.is_empty() && name.chars().all(is_name_char)
}

/// 리모트 전체 패턴(`refs/remotes/<이름>/*`)인지. 이름에 `/` 가 있어도 된다
pub fn is_remote_all(id: &str) -> bool {
    let Some(rest) = id.strip_prefix("refs/remotes/") else { return false };
    let Some(name) = rest.strip_suffix("/*") else { return false };
    !name.is_empty() && name.chars().all(is_name_char) && !bad_shape(&id[..id.len() - 2])
}

/// `LogScope` 항목으로 쓸 수 있는 형식인지: 전체 ref 이름 · 40/64자 해시 · 리모트 전체 패턴
pub fn is_valid_scope_ref(item: &Value) -> bool {
    let Value::String(s) = item else { return false };
    if s.encode_utf16().count() > MAX_REF_LEN {
        return false;
    }
    is_hash(s) || is_remote_all(s) || (is_scope_ref_shape(s) && !bad_shape(s))
}

/// 검증을 마친 보기 범위. 목록은 중복을 걷어 처음 나온 순서를 지켰다
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Scope {
    pub solo: Vec<String>,
    pub hide: Vec<String>,
    pub include_head: Option<bool>,
}

fn scope_list(obj: &Map<String, Value>, key: &str) -> Result<Vec<String>, String> {
    let bad = || format!("invalid {key}");
    let Some(list) = obj.get(key) else { return Ok(Vec::new()) };
    let Value::Array(items) = list else { return Err(bad()) };
    if items.len() > SCOPE_MAX_ITEMS {
        return Err(bad());
    }
    let mut out: Vec<String> = Vec::with_capacity(items.len());
    for item in items {
        if !is_valid_scope_ref(item) {
            return Err(bad());
        }
        let s = item.as_str().expect("is_valid_scope_ref 가 문자열을 보장한다");
        if !out.iter().any(|o| o == s) {
            out.push(s.to_string());
        }
    }
    Ok(out)
}

/// renderer 가 보낸 `LogQuery.scope` 를 검증한다. `None`(키 없음·undefined)은 기본 보기다.
/// TS 와 같게 `null`·문자열·숫자는 `invalid scope`, 배열은 JS 에서 object 라 `solo`·`hide` 가 없는 범위로 통과한다.
pub fn parse_scope(v: Option<&Value>) -> Result<Option<Scope>, String> {
    let invalid = || "invalid scope".to_string();
    let obj = match v {
        None => return Ok(None),
        Some(Value::Array(_)) => return Ok(Some(Scope::default())),
        Some(Value::Object(o)) => o,
        Some(_) => return Err(invalid()),
    };
    let solo = scope_list(obj, "solo")?;
    let hide = scope_list(obj, "hide")?;
    // 중복을 걷은 뒤의 항목 수로 센다(TS 와 같다)
    if solo.len() + hide.len() > SCOPE_MAX_ITEMS {
        return Err(invalid());
    }
    let include_head = match obj.get("includeHead") {
        None => None,
        Some(Value::Bool(b)) => Some(*b),
        Some(_) => return Err(invalid()),
    };
    Ok(Some(Scope { solo, hide, include_head }))
}

fn rev_args() -> Vec<String> {
    ["--ignore-missing", "--branches", "--remotes", "--tags", "HEAD"].map(String::from).to_vec()
}

/// 범위를 `git log` 의 시작점 인자로 바꾼다(`scopeArgs`). 형식은 [`parse_scope`] 가 이미 검증했다.
/// - 범위가 없거나 비었으면 기본 보기(모든 브랜치·리모트·태그 + HEAD)
/// - solo: 리모트 전체는 `--glob=`, 나머지는 rev 그대로(검증상 `-` 로 시작하지 않는다). 시작점이 하나도 없으면 빈 목록
/// - hide: 종류별 `--exclude=` 가 바로 다음 `--branches`·`--remotes`·`--tags` 한 번에만 적용된다. 해시는 숨길 수 없다
pub fn scope_args(scope: Option<&Scope>) -> Result<Vec<String>, String> {
    let Some(scope) = scope else { return Ok(rev_args()) };
    if !scope.solo.is_empty() {
        let (globs, revs): (Vec<&String>, Vec<&String>) = scope.solo.iter().partition(|s| s.ends_with("/*"));
        let mut v = vec!["--ignore-missing".to_string()];
        v.extend(globs.iter().map(|g| format!("--glob={g}")));
        debug_assert!(revs.iter().all(|r| !r.starts_with('-')), "solo rev 는 옵션 모양일 수 없다");
        v.extend(revs.iter().map(|r| (*r).clone()));
        return Ok(v);
    }
    if scope.hide.is_empty() {
        return Ok(rev_args());
    }
    if scope.hide.iter().any(|h| is_hash(h)) {
        return Err("invalid hide".to_string());
    }
    let group = |prefix: &str| -> Vec<String> {
        scope.hide.iter().filter_map(|h| h.strip_prefix(prefix)).map(|n| format!("--exclude={n}")).collect()
    };
    // 리모트의 심볼릭 HEAD(origin/HEAD → origin/main)는 --remotes 로 따로 들어와 숨긴 브랜치를 다시 보이게 한다
    let remote_branch_hidden = scope.hide.iter().any(|h| h.starts_with("refs/remotes/") && !is_remote_all(h));
    let mut v = vec!["--ignore-missing".to_string()];
    v.extend(group("refs/heads/"));
    v.push("--branches".into());
    v.extend(group("refs/remotes/"));
    if remote_branch_hidden {
        v.push("--exclude=*/HEAD".into());
    }
    v.push("--remotes".into());
    v.extend(group("refs/tags/"));
    v.push("--tags".into());
    if scope.include_head != Some(false) {
        v.push("HEAD".into());
    }
    Ok(v)
}

/// 커밋 목록용 `git log` 인자(`logArgs`). `--date-order` 는 자식이 항상 부모보다 먼저 나오도록 보장한다.
/// 서브커맨드·옵션은 상수이고 renderer 값은 [`scope_args`] 의 정해진 위치로만 들어간다
pub fn log_args(scope: Option<&Scope>, skip: u64) -> Result<Vec<String>, String> {
    let mut v: Vec<String> = vec!["log".into()];
    v.extend(LOG_FORMAT_ARGS.iter().map(|s| (*s).to_string()));
    v.push("--date-order".into());
    if skip > 0 {
        v.push(format!("--skip={skip}"));
    }
    v.extend(scope_args(scope)?);
    v.push("--".into());
    Ok(v)
}

/// 범위의 정규 키(커서·앵커 키). 같은 범위면 항목 순서와 무관하게 같고, 비어 있으면 빈 문자열이다.
/// 프로세스 안에서만 쓰이므로 TS `scopeKey` 와 문자열이 같을 필요는 없고 같은 범위가 같은 키이면 된다
pub fn scope_key(scope: Option<&Scope>) -> String {
    let Some(scope) = scope else { return String::new() };
    if scope.solo.is_empty() && scope.hide.is_empty() {
        return String::new();
    }
    let sorted = |list: &[String]| {
        let mut l = list.to_vec();
        l.sort();
        l.join("\0")
    };
    // solo 가 있으면 hide 는 적용되지 않는다
    if !scope.solo.is_empty() {
        return format!("solo\0{}", sorted(&scope.solo));
    }
    format!("hide\0{}\0{}", if scope.include_head == Some(false) { "nohead" } else { "head" }, sorted(&scope.hide))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn valid(s: &str) -> bool {
        is_valid_scope_ref(&json!(s))
    }

    #[test]
    fn 허용_형식() {
        for ok in [
            "refs/heads/main",
            "refs/heads/feature/a-b_c.d",
            "refs/tags/v1.0.0",
            "refs/remotes/origin/main",
            "refs/remotes/origin/*",
            "refs/remotes/team/alice/*",
            "refs/heads/한글-브랜치",
            "refs/heads/a]b",
            &"a".repeat(40),
            &"0123456789abcdef".repeat(4),
        ] {
            assert!(valid(ok), "{ok:?}");
        }
    }

    #[test]
    fn 거부_형식() {
        for bad in [
            "",
            "main",
            "HEAD",
            "-c",
            "--output=x",
            "-cx=y",
            "refs/heads/",
            "refs/heads",
            "refs/other/x",
            "refs/heads/--output=x\n",
            "refs/heads/a b",
            "refs/heads/a\tb",
            "refs/heads/a\nb",
            "refs/heads/a\n",
            "refs/heads/a\0b",
            "refs/heads/a\x7fb",
            "refs/heads/a~1",
            "refs/heads/a^",
            "refs/heads/a:b",
            "refs/heads/a?b",
            "refs/heads/a*b",
            "refs/heads/a[b",
            "refs/heads/a\\b",
            "refs/heads/a..b",
            "refs/heads/a@{b",
            "refs/heads/a//b",
            "refs/heads/a/",
            "refs/heads/a.lock",
            "refs/heads/a/.b",
            "refs/heads/.a",
            "refs/remotes//*",
            "refs/remotes/*",
            "refs/remotes/a b/*",
            "refs/remotes/a/../b/*",
            "refs/remotes/a//b/*",
            "refs/remotes/a.lock/*",
            "refs/remotes/.x/*",
            "refs/heads/*",
            "refs/tags/*",
            "refs/remotes/a/*/",
            &"A".repeat(40),
            &"a".repeat(39),
            &"a".repeat(41),
            &"a".repeat(63),
            &format!("{}\n", "a".repeat(40)),
            // JS `\s` 에 있는 공백: BOM·NBSP·전각 공백·줄 구분자
            "refs/heads/\u{feff}x",
            "refs/heads/x\u{feff}",
            "refs/heads/a\u{a0}b",
            "refs/heads/a\u{3000}b",
            "refs/heads/a\u{2028}b",
            "refs/heads/a\u{2003}b",
            "refs/remotes/o\u{feff}/*",
        ] {
            assert!(!valid(bad), "{bad:?}");
        }
        // 문자열이 아닌 항목
        for v in [json!(null), json!(5), json!(["refs/heads/a"]), json!({ "a": 1 }), json!(true)] {
            assert!(!is_valid_scope_ref(&v), "{v}");
        }
    }

    #[test]
    fn js_공백과_rust_공백이_다른_두_문자() {
        // U+0085(NEL)는 Rust 에서는 공백이지만 JS `\s` 가 아니라 TS 는 허용한다. U+FEFF 는 반대다
        assert!(valid("refs/heads/a\u{85}b"));
        assert!(!valid("refs/heads/a\u{feff}b"));
    }

    #[test]
    fn 길이는_utf16_코드_단위로_센다() {
        let name = |n: usize, ch: &str| format!("refs/heads/{}", ch.repeat(n));
        let base = "refs/heads/".len();
        assert!(valid(&name(1024 - base, "a")));
        assert!(!valid(&name(1025 - base, "a")));
        // 이모지는 char 1개 = UTF-16 2단위: 1013 단위(=506개+a 1개)까지, 한 글자 더 넣으면 넘는다
        let room = 1024 - base;
        assert!(valid(&format!("{}{}", name(room / 2, "😀"), if room % 2 == 1 { "a" } else { "" })));
        assert!(!valid(&name(room / 2 + 1, "😀")));
        // 코드 포인트 수로는 1024 이하인데 UTF-16 으로는 넘는 경우
        assert!(!valid(&name(600, "😀")));
    }

    #[test]
    fn 범위_형식_검사() {
        assert_eq!(parse_scope(None), Ok(None));
        assert_eq!(parse_scope(Some(&json!({}))), Ok(Some(Scope::default())));
        // 배열은 JS 에서 object — solo·hide 가 없는 범위(기본 보기)
        assert_eq!(parse_scope(Some(&json!([]))), Ok(Some(Scope::default())));
        for bad in [json!(null), json!("x"), json!(5), json!(true)] {
            assert_eq!(parse_scope(Some(&bad)), Err("invalid scope".into()), "{bad}");
        }
        assert_eq!(parse_scope(Some(&json!({ "solo": null }))), Err("invalid solo".into()));
        assert_eq!(parse_scope(Some(&json!({ "solo": "refs/heads/a" }))), Err("invalid solo".into()));
        assert_eq!(parse_scope(Some(&json!({ "hide": {} }))), Err("invalid hide".into()));
        assert_eq!(parse_scope(Some(&json!({ "solo": ["-c"] }))), Err("invalid solo".into()));
        assert_eq!(parse_scope(Some(&json!({ "hide": ["--output=x"] }))), Err("invalid hide".into()));
        assert_eq!(parse_scope(Some(&json!({ "includeHead": null }))), Err("invalid scope".into()));
        assert_eq!(parse_scope(Some(&json!({ "includeHead": "no" }))), Err("invalid scope".into()));
        // 항목 수: 한 목록 200 까지, 중복은 걷은 뒤에 센다
        let many = |n: usize| (0..n).map(|i| format!("refs/heads/b{i}")).collect::<Vec<_>>();
        assert!(parse_scope(Some(&json!({ "solo": many(200) }))).is_ok());
        assert_eq!(parse_scope(Some(&json!({ "solo": many(201) }))), Err("invalid solo".into()));
        assert_eq!(parse_scope(Some(&json!({ "solo": many(120), "hide": (0..120).map(|i| format!("refs/tags/t{i}")).collect::<Vec<_>>() }))), Err("invalid scope".into()));
        let dup = parse_scope(Some(&json!({ "solo": ["refs/heads/b", "refs/heads/a", "refs/heads/b"] }))).unwrap().unwrap();
        assert_eq!(dup.solo, ["refs/heads/b", "refs/heads/a"]);
    }

    fn args(scope: Value) -> Vec<String> {
        let s = parse_scope(Some(&scope)).unwrap();
        scope_args(s.as_ref()).unwrap()
    }

    #[test]
    fn 기본_보기와_solo_인자() {
        assert_eq!(scope_args(None).unwrap(), ["--ignore-missing", "--branches", "--remotes", "--tags", "HEAD"]);
        assert_eq!(args(json!({ "hide": [] })), args(json!({ "includeHead": false })));
        assert_eq!(args(json!({ "solo": ["refs/heads/a", "refs/remotes/origin/*", "refs/tags/v"] })), [
            "--ignore-missing",
            "--glob=refs/remotes/origin/*",
            "refs/heads/a",
            "refs/tags/v"
        ]);
        // 옵션처럼 보이는 이름도 refs/ 로 시작하는 한 인자라 옵션이 아니다
        let a = args(json!({ "solo": ["refs/heads/--output=x"] }));
        assert_eq!(a, ["--ignore-missing", "refs/heads/--output=x"]);
        // solo 가 있으면 hide 는 무시한다(해시여도 거부하지 않는다)
        assert_eq!(args(json!({ "solo": ["refs/heads/a"], "hide": ["a".repeat(40)] })), ["--ignore-missing", "refs/heads/a"]);
    }

    #[test]
    fn hide_인자() {
        assert_eq!(
            args(json!({ "hide": ["refs/heads/a", "refs/tags/t", "refs/remotes/origin/main", "refs/remotes/up/*"] })),
            [
                "--ignore-missing",
                "--exclude=a",
                "--branches",
                "--exclude=origin/main",
                "--exclude=up/*",
                "--exclude=*/HEAD",
                "--remotes",
                "--exclude=t",
                "--tags",
                "HEAD"
            ]
        );
        // 리모트 전체만 숨기면 */HEAD 제외는 붙지 않는다
        assert_eq!(args(json!({ "hide": ["refs/remotes/up/*"] })), ["--ignore-missing", "--branches", "--exclude=up/*", "--remotes", "--tags", "HEAD"]);
        assert!(!args(json!({ "hide": ["refs/heads/a"], "includeHead": false })).contains(&"HEAD".to_string()));
        let s = parse_scope(Some(&json!({ "hide": ["a".repeat(40)] }))).unwrap();
        assert_eq!(scope_args(s.as_ref()), Err("invalid hide".into()));
    }

    #[test]
    fn log_args_는_서브커맨드와_옵션이_앞에_온다() {
        let a = log_args(None, 0).unwrap();
        assert_eq!(a[0], "log");
        assert_eq!(&a[1..4], LOG_FORMAT_ARGS);
        assert_eq!(a[4], "--date-order");
        assert_eq!(a.last().unwrap(), "--");
        assert!(!a.iter().any(|x| x.starts_with("--skip")));
        let b = log_args(None, 250).unwrap();
        assert_eq!(b[5], "--skip=250");
    }

    #[test]
    fn 키는_순서와_중복에_무관하다() {
        let key = |v: Value| scope_key(parse_scope(Some(&v)).unwrap().as_ref());
        assert_eq!(scope_key(None), "");
        assert_eq!(key(json!({})), "");
        assert_eq!(key(json!({ "includeHead": false })), "");
        assert_eq!(key(json!({ "solo": ["refs/heads/b", "refs/heads/a"] })), key(json!({ "solo": ["refs/heads/a", "refs/heads/b", "refs/heads/a"] })));
        assert_ne!(key(json!({ "solo": ["refs/heads/a"] })), key(json!({ "hide": ["refs/heads/a"] })));
        assert_ne!(key(json!({ "hide": ["refs/heads/a"] })), key(json!({ "hide": ["refs/heads/a"], "includeHead": false })));
        // solo 가 있으면 hide·includeHead 는 키에 영향이 없다
        assert_eq!(key(json!({ "solo": ["refs/heads/a"], "hide": ["refs/tags/t"], "includeHead": false })), key(json!({ "solo": ["refs/heads/a"] })));
    }
}
