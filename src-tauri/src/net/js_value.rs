//! 응답 매퍼가 쓰는 JS 값 의미 헬퍼와 `JSON.stringify` 꼴 직렬화.
//!
//! 매퍼는 `&Json`(응답) → `Json`(공유 타입의 IPC 값)이다. TS 가 `??`·`||`·`=== true` 로 거르는 곳은 같은 분기로 옮기고,
//! TS 가 던지는 곳(`null.필드`·배열이 아닌 값의 `.map`)은 [`MapError`] 로 돌려준다. 타입이 틀린 값을 TS 가 그대로 흘리는 필드(`title: 5`)는 Rust 도 그대로 흘린다.
use super::json::{js_number, js_number_string, Json};

/// TS 라면 `TypeError` 가 났을 입력
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MapError;

pub type MapResult<T> = Result<T, MapError>;

/// 객체 만들기(키 순서가 그대로 출력 순서다)
pub fn obj(items: Vec<(&str, Json)>) -> Json {
    Json::Obj(items.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
}

pub fn s(v: &str) -> Json {
    Json::Str(v.to_string())
}

pub fn num(n: f64) -> Json {
    Json::Num(n)
}

/// 조건부 spread(`...(cond ? { k: v } : {})`)용: 값이 있을 때만 키를 더한다
pub fn push_some(o: &mut Vec<(&str, Json)>, key: &'static str, v: Option<Json>) {
    if let Some(v) = v {
        o.push((key, v));
    }
}

/// JS truthiness
pub fn truthy(v: Option<&Json>) -> bool {
    match v {
        None | Some(Json::Null) => false,
        Some(Json::Bool(b)) => *b,
        Some(Json::Num(n)) => *n != 0.0 && !n.is_nan(),
        Some(Json::Str(t)) => !t.is_empty(),
        Some(Json::Arr(_) | Json::Obj(_)) => true,
    }
}

/// `v ?? default`: 없거나 null 이면 기본값, 아니면 타입과 상관없이 그 값
pub fn nn(v: Option<&Json>, default: Json) -> Json {
    match v {
        None | Some(Json::Null) => default,
        Some(x) => x.clone(),
    }
}

/// `x.key`. 객체가 아니면 `undefined`. `x` 가 null 이면 TS 는 던진다 — 호출부가 [`field_strict`] 를 쓴다
pub fn field<'a>(v: Option<&'a Json>, key: &str) -> Option<&'a Json> {
    v.and_then(|x| x.get(key))
}

/// `x.key`(옵셔널 체이닝 없음): `x` 가 null(또는 없음)이면 TS 는 TypeError
pub fn field_strict<'a>(v: Option<&'a Json>, key: &str) -> MapResult<Option<&'a Json>> {
    match v {
        None | Some(Json::Null) => Err(MapError),
        Some(x) => Ok(x.get(key)),
    }
}

/// `(x ?? []).map(...)` 의 원소: 배열이면 원소, 없음·null 이면 빈 목록, 그 외(문자열·객체·숫자)는 TS 가 던진다
pub fn items(v: Option<&Json>) -> MapResult<&[Json]> {
    match v {
        None | Some(Json::Null) => Ok(&[]),
        Some(Json::Arr(a)) => Ok(a),
        Some(_) => Err(MapError),
    }
}

/// `v.map(...)`·`for (const x of v)` 의 원소: 배열이어야 한다(없음·null 포함 그 외는 TS 가 던진다)
pub fn must_arr(v: Option<&Json>) -> MapResult<&[Json]> {
    match v {
        Some(Json::Arr(a)) => Ok(a),
        _ => Err(MapError),
    }
}

/// 템플릿 문자열 보간 `${v}`(undefined → `undefined`, null → `null`)
pub fn template_string(v: Option<&Json>) -> String {
    match v {
        None => "undefined".into(),
        Some(Json::Null) => "null".into(),
        Some(x) => to_js_string(x),
    }
}

/// `Array.isArray(v) ? v : []`
pub fn arr_or_empty(v: Option<&Json>) -> &[Json] {
    match v {
        Some(Json::Arr(a)) => a,
        _ => &[],
    }
}

/// `typeof v === 'string' ? v : ''`
pub fn str_or_empty(v: Option<&Json>) -> &str {
    match v {
        Some(Json::Str(t)) => t,
        _ => "",
    }
}

/// `x?.length ?? 0`: 배열은 길이, 문자열은 UTF-16 길이, 그 외는 `undefined` → 0
pub fn js_length(v: Option<&Json>) -> f64 {
    match v {
        Some(Json::Arr(a)) => a.len() as f64,
        Some(Json::Str(t)) => t.encode_utf16().count() as f64,
        _ => 0.0,
    }
}

/// `String(v)` 에 해당하는 문자열(배열은 `join(',')`, 객체는 `[object Object]`)
fn to_js_string(v: &Json) -> String {
    match v {
        Json::Null => String::new(),
        Json::Bool(b) => b.to_string(),
        Json::Num(n) => js_number_string(*n),
        Json::Str(t) => t.clone(),
        Json::Arr(a) => a.iter().map(to_js_string).collect::<Vec<_>>().join(","),
        Json::Obj(_) => "[object Object]".into(),
    }
}

/// JS `Number(v)`
pub fn to_number(v: Option<&Json>) -> f64 {
    match v {
        None => f64::NAN,
        Some(Json::Null) => 0.0,
        Some(Json::Bool(b)) => f64::from(u8::from(*b)),
        Some(Json::Num(n)) => *n,
        Some(Json::Str(t)) => js_number(t),
        Some(x @ Json::Arr(_)) => js_number(&to_js_string(x)),
        Some(Json::Obj(_)) => f64::NAN,
    }
}

/// JS `encodeURIComponent`: `A-Za-z0-9 - _ . ! ~ * ' ( )` 만 남기고 UTF-8 바이트를 `%XX`(대문자)로
pub fn encode_uri_component(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'!' | b'~' | b'*' | b'\'' | b'(' | b')') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

// ── JSON.stringify ─────────────────────────────────────────────────────────

/// `JSON.stringify(v)` 와 같은 텍스트(공백 없음). 유한하지 않은 숫자는 `null`
pub fn stringify(v: &Json) -> String {
    let mut out = String::new();
    write_json(v, &mut out);
    out
}

fn write_json(v: &Json, out: &mut String) {
    match v {
        Json::Null => out.push_str("null"),
        Json::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Json::Num(n) => out.push_str(&if n.is_finite() { js_number_string(*n) } else { "null".into() }),
        Json::Str(t) => write_str(t, out),
        Json::Arr(a) => {
            out.push('[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_json(x, out);
            }
            out.push(']');
        }
        Json::Obj(o) => {
            out.push('{');
            for (i, (k, x)) in o.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_str(k, out);
                out.push(':');
                write_json(x, out);
            }
            out.push('}');
        }
    }
}

fn write_str(t: &str, out: &mut String) {
    out.push('"');
    for c in t.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c < ' ' => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::json::parse;

    #[test]
    fn 직렬화는_삽입_순서와_js_이스케이프를_따른다() {
        let j = parse(r#"{"b":1,"a":[null,true,"x\u0001\"\u007f"],"2":0.5,"1":-0}"#).unwrap();
        // 정수형 키 순서는 매퍼가 아니라 JS 객체의 열거 규칙이라 여기서는 삽입 순서 그대로다
        assert_eq!(stringify(&j), "{\"b\":1,\"a\":[null,true,\"x\\u0001\\\"\u{7f}\"],\"2\":0.5,\"1\":0}");
    }

    #[test]
    fn to_number_는_js_와_같다() {
        assert_eq!(to_number(Some(&parse("[2]").unwrap())), 2.0);
        assert_eq!(to_number(Some(&parse("null").unwrap())), 0.0);
        assert!(to_number(Some(&parse("{}").unwrap())).is_nan());
        assert!(to_number(None).is_nan());
    }
}
