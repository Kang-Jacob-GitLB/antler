//! `src/main/git/parsers/*.ts` 의 Rust 이식. 출력 JSON 모양과 경계 동작은 TS 가 기준이다
//! (`tests/golden/parsers` 의 골든을 `src-tauri/tests/golden.rs` 가 비교한다).

use serde::{Serialize, Serializer};

pub mod blame;
pub mod changes;
pub mod commit;
pub mod diff;
pub mod file_history;
pub mod log;
pub mod nul_records;
pub mod refs;
pub mod stash;
pub mod submodule;
pub mod wip_status;
pub mod worktree;

/// JS `Number(str)` 의 결과. JSON 으로는 `JSON.stringify` 처럼 NaN·Infinity 가 null, 정수는 소수점 없이 나간다.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct JsNumber(pub f64);

impl Serialize for JsNumber {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let v = self.0;
        if !v.is_finite() {
            s.serialize_none()
        } else if v.fract() == 0.0 && v.abs() < 9_007_199_254_740_992.0 {
            s.serialize_i64(v as i64)
        } else {
            s.serialize_f64(v)
        }
    }
}

/// JS 공백(Unicode 공백 + BOM). Rust 의 `char::is_whitespace` 에 없는 BOM 만 더한다
fn is_js_space(c: char) -> bool {
    c.is_whitespace() || c == '\u{feff}'
}

/// JS `String.prototype.trim` 과 같은 공백 기준으로 앞뒤를 자른다
pub fn js_trim(s: &str) -> &str {
    s.trim_matches(is_js_space)
}

/// JS `Number(s) || 0`: NaN(과 0)은 0 이 된다. Infinity 는 그대로라 JSON 으로는 null
pub fn js_number_or_zero(s: &str) -> JsNumber {
    let n = js_number(s);
    if n.0.is_nan() || n.0 == 0.0 {
        JsNumber(0.0)
    } else {
        n
    }
}

/// 0x·0o·0b 접두 정수. 접두가 없으면 None, 접두 뒤가 비었거나 자릿수가 틀리면 NaN
fn prefixed_int(t: &str, prefixes: [&str; 2], radix: u32) -> Option<f64> {
    let rest = prefixes.iter().find_map(|p| t.strip_prefix(p))?;
    if rest.is_empty() || !rest.chars().all(|c| c.is_digit(radix)) {
        return Some(f64::NAN);
    }
    Some(rest.chars().fold(0.0, |acc, c| acc * f64::from(radix) + f64::from(c.to_digit(radix).unwrap_or(0))))
}

/// JS `Number(s)`: 앞뒤 공백 제거, 빈 문자열은 0, 0x·0o·0b 접두, `Infinity`, 십진 리터럴, 그 밖은 NaN
pub fn js_number(s: &str) -> JsNumber {
    let t = js_trim(s);
    if t.is_empty() {
        return JsNumber(0.0);
    }
    if let Some(v) = prefixed_int(t, ["0x", "0X"], 16)
        .or_else(|| prefixed_int(t, ["0o", "0O"], 8))
        .or_else(|| prefixed_int(t, ["0b", "0B"], 2))
    {
        return JsNumber(v);
    }
    let (sign, body) = match t.as_bytes()[0] {
        b'+' => (1.0, &t[1..]),
        b'-' => (-1.0, &t[1..]),
        _ => (1.0, t),
    };
    if body == "Infinity" {
        return JsNumber(sign * f64::INFINITY);
    }
    if is_decimal_literal(body) {
        if let Ok(v) = body.parse::<f64>() {
            return JsNumber(sign * v);
        }
    }
    JsNumber(f64::NAN)
}

/// `digits [. digits?] | . digits` + 선택적 지수. Rust 의 `parse::<f64>` 가 받는 inf·nan 같은 철자를 걸러 낸다
fn is_decimal_literal(s: &str) -> bool {
    let (mant, exp) = match s.find(['e', 'E']) {
        Some(i) => (&s[..i], Some(&s[i + 1..])),
        None => (s, None),
    };
    let digits = |x: &str| x.bytes().all(|b| b.is_ascii_digit());
    let mant_ok = match mant.split_once('.') {
        Some((a, b)) => digits(a) && digits(b) && !(a.is_empty() && b.is_empty()),
        None => !mant.is_empty() && digits(mant),
    };
    let exp_ok = exp.is_none_or(|e| {
        let e = e.strip_prefix(['+', '-']).unwrap_or(e);
        !e.is_empty() && digits(e)
    });
    mant_ok && exp_ok
}

#[cfg(test)]
mod tests {
    use super::*;

    fn num(s: &str) -> String {
        serde_json::to_string(&js_number(s)).unwrap()
    }

    #[test]
    fn js_number_matches_js() {
        assert_eq!(num(""), "0");
        assert_eq!(num("  12 "), "12");
        assert_eq!(num("1700000000"), "1700000000");
        assert_eq!(num("-5"), "-5");
        assert_eq!(num("0x10"), "16");
        assert_eq!(num("1e3"), "1000");
        assert_eq!(num(".5"), "0.5");
        assert_eq!(num("5."), "5");
        assert_eq!(num("abc"), "null");
        assert_eq!(num("inf"), "null");
        assert_eq!(num("nan"), "null");
        assert_eq!(num("Infinity"), "null");
        assert_eq!(num("1_0"), "null");
        assert_eq!(num("."), "null");
        assert_eq!(num("1e"), "null");
        assert_eq!(num("-0"), "0");
    }
}
