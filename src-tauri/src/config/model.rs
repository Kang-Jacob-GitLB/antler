//! 설정 값의 모양: 순서 있는 JSON 객체 + JS 의미의 접근 헬퍼.
//!
//! 설정은 구조체가 아니라 [`Json`] 트리로 둔다. TS 가 파일에서 읽은 저장소·소스를 **검증 없이 그대로** 펼쳐(`{...r}`) 저장하므로
//! 알 수 없는 키와 키 순서가 파일 바이트에 남는다 — 구조체로 옮기면 그 둘을 잃는다. 최상위·`ui`·저장소·소스 모두 삽입 순서를 보존하는 `Json::Obj` 다.
//!
//! JS 에서 `{...a, k: v}` 는 `k` 가 이미 있으면 **제자리**에서 값만 바꾸고 없으면 끝에 붙인다([`set`]). 그리고 값이 `undefined` 인 키는
//! 객체에 남지만 `JSON.stringify` 가 건너뛴다 — `removeRepo` 가 `lastRepoId: undefined` 로 둔 자리가 그 예다. 이 자리를 [`UNDEFINED`] 로 나타낸다
//! (이후 `setUi({lastRepoId})` 가 끝이 아니라 옛 자리를 채운다). 파일에는 쓰지 않는다([`strip_undefined`]).
use crate::net::json::Json;

pub type Obj = Vec<(String, Json)>;

/// JS `undefined` 값이 든 객체 자리. JSON 해석은 NaN 을 만들 수 없으므로(`1e999` 는 Infinity) 다른 값과 겹치지 않는다
pub const UNDEFINED: Json = Json::Num(f64::NAN);

pub fn is_undefined(v: &Json) -> bool {
    matches!(v, Json::Num(n) if n.is_nan())
}

/// `o.key`(자리가 `undefined` 면 `None` — JS 와 같다)
pub fn get<'a>(o: &'a [(String, Json)], key: &str) -> Option<&'a Json> {
    o.iter().find(|(k, _)| k == key).map(|(_, v)| v).filter(|v| !is_undefined(v))
}

/// `key in o`(값이 `undefined` 여도 키가 있으면 참)
pub fn has_key(o: &[(String, Json)], key: &str) -> bool {
    o.iter().any(|(k, _)| k == key)
}

/// `{...o, [key]: value}` — 있으면 제자리, 없으면 끝
pub fn set(o: &mut Obj, key: &str, value: Json) {
    match o.iter_mut().find(|(k, _)| k == key) {
        Some(slot) => slot.1 = value,
        None => o.push((key.to_string(), value)),
    }
}

/// `delete o[key]`
pub fn remove(o: &mut Obj, key: &str) {
    o.retain(|(k, _)| k != key);
}

/// JS 객체 펼침 `{...v}` 의 열거 가능한 자체 속성(키·값). 객체는 그대로, 배열은 인덱스, 문자열은 글자(BMP 밖은 코드 단위 둘로 쪼개지는데 Rust 문자열은
/// 이를 담지 못해 한 글자로 둔다 — 설정이 이런 값을 가질 일은 손상 파일뿐이다), 그 밖(null·숫자·불리언)은 빈 객체
pub fn spread(v: Option<&Json>) -> Obj {
    match v {
        Some(Json::Obj(o)) => o.clone(),
        Some(Json::Arr(a)) => a.iter().enumerate().map(|(i, x)| (i.to_string(), x.clone())).collect(),
        Some(Json::Str(s)) => s.chars().enumerate().map(|(i, c)| (i.to_string(), Json::Str(c.to_string()))).collect(),
        _ => Vec::new(),
    }
}

/// `v[key]`. 객체는 키, 배열·문자열은 인덱스 꼴 키만(JS 의 `"abc"["x"]` 는 `undefined`)
pub fn js_get<'a>(v: &'a Json, key: &str) -> Option<&'a Json> {
    match v {
        Json::Obj(o) => get(o, key),
        Json::Arr(a) => key.parse::<usize>().ok().filter(|i| i.to_string() == key).and_then(|i| a.get(i)),
        _ => None,
    }
}

/// 문자열의 UTF-16 코드 단위 수(JS `length`)
pub fn utf16_len(s: &str) -> usize {
    s.encode_utf16().count()
}

/// `Json` 안의 [`UNDEFINED`] 객체 자리를 지운다(`JSON.stringify` 가 건너뛰는 것). 배열 안의 `undefined` 는 `null` 이지만 설정에는 생기지 않는다
pub fn strip_undefined(v: &Json) -> Json {
    match v {
        Json::Obj(o) => Json::Obj(o.iter().filter(|(_, x)| !is_undefined(x)).map(|(k, x)| (k.clone(), strip_undefined(x))).collect()),
        Json::Arr(a) => Json::Arr(a.iter().map(strip_undefined).collect()),
        other => other.clone(),
    }
}

/// 환경 하나를 나타내는 소스(`sourceFor`). id 가 결정적이라 같은 배포판·호스트는 같은 소스로 모인다
pub const LOCAL_SOURCE_ID: &str = "local";

/// `sourceFor(kind, target)` — `target` 은 wsl 배포판·ssh 호스트. 없으면 TS 와 같은 문구로 돌려준다
pub fn source_for(kind: &str, target: Option<&str>) -> Result<Json, String> {
    let o = |items: Vec<(&str, &str)>| Json::Obj(items.into_iter().map(|(k, v)| (k.to_string(), Json::Str(v.to_string()))).collect());
    match kind {
        "local" => Ok(o(vec![("id", LOCAL_SOURCE_ID), ("kind", "local"), ("label", "Local")])),
        // GitHub 원격 전용 저장소는 환경이 하나뿐이다(앱 캐시). 저장소가 하나라도 있을 때만 소스가 존재한다
        "github" => Ok(o(vec![("id", crate::git::github_link::GITHUB_SOURCE_ID), ("kind", "github"), ("label", "GitHub")])),
        _ => {
            // JS 의 `!target` — 빈 문자열도 없는 것이다
            let Some(target) = target.filter(|t| !t.is_empty()) else {
                return Err(format!("{kind} 소스에는 {} 이름이 필요합니다", if kind == "wsl" { "배포판" } else { "호스트" }));
            };
            if kind == "wsl" {
                Ok(o(vec![("id", &format!("wsl:{target}")), ("kind", "wsl"), ("label", &format!("WSL · {target}")), ("distro", target)]))
            } else {
                Ok(o(vec![("id", &format!("ssh:{target}")), ("kind", kind), ("label", &format!("SSH · {target}")), ("sshHost", target)]))
            }
        }
    }
}

/// 기본 설정(`defaults()`)의 `ui`
pub fn default_ui() -> Obj {
    vec![
        ("theme".to_string(), Json::Str("system".to_string())),
        ("locale".to_string(), Json::Str("ko".to_string())),
        ("paneWidths".to_string(), Json::Arr(vec![Json::Num(248.0), Json::Num(380.0)])),
    ]
}

/// 기본 설정
pub fn defaults() -> Obj {
    let local = source_for("local", None).unwrap_or(Json::Null);
    vec![
        ("version".to_string(), Json::Num(1.0)),
        ("sources".to_string(), Json::Arr(vec![local])),
        ("repos".to_string(), Json::Arr(Vec::new())),
        ("ui".to_string(), Json::Obj(default_ui())),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::json::parse;

    #[test]
    fn set_은_제자리에서_바꾸고_없으면_끝에_붙인다() {
        let mut o: Obj = vec![("a".into(), Json::Num(1.0)), ("b".into(), UNDEFINED)];
        assert_eq!(get(&o, "b"), None);
        assert!(has_key(&o, "b"));
        set(&mut o, "b", Json::Num(2.0));
        set(&mut o, "c", Json::Num(3.0));
        assert_eq!(o.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>(), ["a", "b", "c"]);
        assert_eq!(strip_undefined(&Json::Obj(vec![("x".into(), UNDEFINED), ("y".into(), Json::Null)])), parse(r#"{"y":null}"#).unwrap());
    }

    #[test]
    fn source_for_는_ts_와_같은_모양이다() {
        assert_eq!(source_for("wsl", Some("Ubuntu")).unwrap(), parse(r#"{"id":"wsl:Ubuntu","kind":"wsl","label":"WSL · Ubuntu","distro":"Ubuntu"}"#).unwrap());
        assert_eq!(source_for("ssh", None).unwrap_err(), "ssh 소스에는 호스트 이름이 필요합니다");
    }
}
