//! JS `JSON.parse` 와 같은 모양의 관대한 JSON 해석과, 응답 해석에 쓰는 JS 값 의미 헬퍼.
//!
//! `serde_json::Value` 를 쓰지 않는 이유: (1) 짝 없는 서로게이트 이스케이프(`"\ud800"`)를 거부한다 — JS 는 받는다,
//! (2) `preserve_order` 가 꺼져 있어 객체 키가 정렬된다 — JS `Object.values` 는 삽입 순서(정수형 키는 먼저 오름차순)다.
//!
//! 알려진 차이(Rust 가 더 엄격하거나 표현이 다른 곳):
//! - 짝 없는 서로게이트는 U+FFFD 로 바뀐다(Rust 문자열은 담지 못한다. JS 는 그대로 둔다).
//! - 중첩 깊이 [`MAX_DEPTH`] 초과는 오류다(JS 는 스택이 다할 때까지 받는다. Rust 의 재귀는 잡을 수 없는 abort 라 상한이 필요하다).
//! - 숫자는 f64 다(`1e999` 는 JS 처럼 `Infinity`).
use std::collections::HashMap;

use crate::git::fetch::js_trim;

/// 중첩 상한. 넘으면 [`JsonError::TooDeep`]. 재귀 한 겹이 스택을 1.4KB 안팎 쓰므로(512 겹이면 약 700KB) 작은 스택의 스레드에서도 안전한 값으로 잡았다.
/// API 응답의 실제 중첩은 한 자릿수다
pub const MAX_DEPTH: usize = 64;

#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Json>),
    /// 삽입 순서(중복 키는 처음 자리에 마지막 값). JS 열거 순서는 [`Json::entries_js_order`]
    Obj(Vec<(String, Json)>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JsonError {
    Syntax,
    TooDeep,
}

impl Json {
    /// 객체의 값(없거나 객체가 아니면 `None`). JS 의 `b.key` 와 달리 상속 속성은 없다
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Obj(v) => v.iter().find(|(k, _)| k == key).map(|(_, x)| x),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Json::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Json::Num(n) => Some(*n),
            _ => None,
        }
    }

    /// JS 객체의 `Object.entries` 순서: 배열 인덱스 꼴 키(정규 십진수, 0..2^32-2)가 오름차순으로 먼저, 나머지는 삽입 순서
    pub fn entries_js_order(&self) -> Vec<(&str, &Json)> {
        let Json::Obj(v) = self else { return Vec::new() };
        let mut idx: Vec<(u32, &str, &Json)> = v.iter().filter_map(|(k, x)| array_index(k).map(|i| (i, k.as_str(), x))).collect();
        idx.sort_by_key(|e| e.0);
        let mut out: Vec<(&str, &Json)> = idx.into_iter().map(|(_, k, x)| (k, x)).collect();
        out.extend(v.iter().filter(|(k, _)| array_index(k).is_none()).map(|(k, x)| (k.as_str(), x)));
        out
    }
}

/// JS 배열 인덱스 꼴의 키인가(`"0"`, `"17"`; `"01"`·`"-1"`·`"4294967295"` 는 아니다)
fn array_index(k: &str) -> Option<u32> {
    if k.is_empty() || k.len() > 10 || !k.bytes().all(|b| b.is_ascii_digit()) || (k.len() > 1 && k.starts_with('0')) {
        return None;
    }
    let n: u64 = k.parse().ok()?;
    (n <= 4_294_967_294).then_some(n as u32)
}

/// `JSON.parse(text)`. 앞뒤 공백은 JSON 공백(스페이스·탭·개행·CR)만 허용한다
pub fn parse(text: &str) -> Result<Json, JsonError> {
    let mut p = Parser { s: text.as_bytes(), i: 0 };
    p.ws();
    let v = p.value(0)?;
    p.ws();
    if p.i != p.s.len() {
        return Err(JsonError::Syntax);
    }
    Ok(v)
}

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
}

impl Parser<'_> {
    fn ws(&mut self) {
        while matches!(self.s.get(self.i), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.i += 1;
        }
    }

    fn eat(&mut self, lit: &str) -> Result<(), JsonError> {
        if self.s[self.i..].starts_with(lit.as_bytes()) {
            self.i += lit.len();
            Ok(())
        } else {
            Err(JsonError::Syntax)
        }
    }

    fn value(&mut self, depth: usize) -> Result<Json, JsonError> {
        if depth > MAX_DEPTH {
            return Err(JsonError::TooDeep);
        }
        match self.s.get(self.i).copied() {
            Some(b'n') => self.eat("null").map(|()| Json::Null),
            Some(b't') => self.eat("true").map(|()| Json::Bool(true)),
            Some(b'f') => self.eat("false").map(|()| Json::Bool(false)),
            Some(b'"') => self.string().map(Json::Str),
            Some(b'[') => {
                self.i += 1;
                let mut items = Vec::new();
                self.ws();
                if self.s.get(self.i) == Some(&b']') {
                    self.i += 1;
                    return Ok(Json::Arr(items));
                }
                loop {
                    self.ws();
                    items.push(self.value(depth + 1)?);
                    self.ws();
                    match self.s.get(self.i) {
                        Some(b',') => self.i += 1,
                        Some(b']') => {
                            self.i += 1;
                            return Ok(Json::Arr(items));
                        }
                        _ => return Err(JsonError::Syntax),
                    }
                }
            }
            Some(b'{') => {
                self.i += 1;
                let mut items: Vec<(String, Json)> = Vec::new();
                let mut pos: HashMap<String, usize> = HashMap::new();
                self.ws();
                if self.s.get(self.i) == Some(&b'}') {
                    self.i += 1;
                    return Ok(Json::Obj(items));
                }
                loop {
                    self.ws();
                    if self.s.get(self.i) != Some(&b'"') {
                        return Err(JsonError::Syntax);
                    }
                    let k = self.string()?;
                    self.ws();
                    if self.s.get(self.i) != Some(&b':') {
                        return Err(JsonError::Syntax);
                    }
                    self.i += 1;
                    self.ws();
                    let v = self.value(depth + 1)?;
                    match pos.get(&k) {
                        Some(&at) => items[at].1 = v,
                        None => {
                            pos.insert(k.clone(), items.len());
                            items.push((k, v));
                        }
                    }
                    self.ws();
                    match self.s.get(self.i) {
                        Some(b',') => self.i += 1,
                        Some(b'}') => {
                            self.i += 1;
                            return Ok(Json::Obj(items));
                        }
                        _ => return Err(JsonError::Syntax),
                    }
                }
            }
            Some(b'-' | b'0'..=b'9') => self.number().map(Json::Num),
            _ => Err(JsonError::Syntax),
        }
    }

    fn number(&mut self) -> Result<f64, JsonError> {
        let start = self.i;
        if self.s.get(self.i) == Some(&b'-') {
            self.i += 1;
        }
        match self.s.get(self.i) {
            Some(b'0') => self.i += 1,
            Some(b'1'..=b'9') => self.digits(),
            _ => return Err(JsonError::Syntax),
        }
        if self.s.get(self.i) == Some(&b'.') {
            self.i += 1;
            if !matches!(self.s.get(self.i), Some(b'0'..=b'9')) {
                return Err(JsonError::Syntax);
            }
            self.digits();
        }
        if matches!(self.s.get(self.i), Some(b'e' | b'E')) {
            self.i += 1;
            if matches!(self.s.get(self.i), Some(b'+' | b'-')) {
                self.i += 1;
            }
            if !matches!(self.s.get(self.i), Some(b'0'..=b'9')) {
                return Err(JsonError::Syntax);
            }
            self.digits();
        }
        std::str::from_utf8(&self.s[start..self.i]).ok().and_then(|t| t.parse::<f64>().ok()).ok_or(JsonError::Syntax)
    }

    fn digits(&mut self) {
        while matches!(self.s.get(self.i), Some(b'0'..=b'9')) {
            self.i += 1;
        }
    }

    fn hex4(&mut self) -> Result<u32, JsonError> {
        let h = self.s.get(self.i..self.i + 4).ok_or(JsonError::Syntax)?;
        let mut n = 0u32;
        for &b in h {
            n = n * 16 + (b as char).to_digit(16).ok_or(JsonError::Syntax)?;
        }
        self.i += 4;
        Ok(n)
    }

    /// 여는 따옴표 위치에서 시작한다. 짝 없는 서로게이트 이스케이프는 U+FFFD
    fn string(&mut self) -> Result<String, JsonError> {
        self.i += 1;
        let mut out = String::new();
        loop {
            let run = self.i;
            while let Some(&b) = self.s.get(self.i) {
                if b == b'"' || b == b'\\' || b < 0x20 {
                    break;
                }
                self.i += 1;
            }
            // 입력이 &str 이고 끊은 곳은 ASCII 경계라 항상 유효한 UTF-8 이다
            out.push_str(std::str::from_utf8(&self.s[run..self.i]).map_err(|_| JsonError::Syntax)?);
            match self.s.get(self.i).copied() {
                Some(b'"') => {
                    self.i += 1;
                    return Ok(out);
                }
                Some(b'\\') => {
                    self.i += 1;
                    let e = self.s.get(self.i).copied().ok_or(JsonError::Syntax)?;
                    self.i += 1;
                    match e {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{c}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let hi = self.hex4()?;
                            if (0xd800..0xdc00).contains(&hi) {
                                // 바로 뒤의 \uDC00..DFFF 와 쌍이 되면 합친다. 아니면 짝 없는 것 — 뒤 이스케이프는 소비하지 않는다
                                let save = self.i;
                                if self.s[self.i..].starts_with(b"\\u") {
                                    self.i += 2;
                                    if let Ok(lo) = self.hex4() {
                                        if (0xdc00..0xe000).contains(&lo) {
                                            out.push(char::from_u32(0x10000 + ((hi - 0xd800) << 10) + (lo - 0xdc00)).unwrap_or('\u{fffd}'));
                                            continue;
                                        }
                                    }
                                }
                                self.i = save;
                                out.push('\u{fffd}');
                            } else if (0xdc00..0xe000).contains(&hi) {
                                out.push('\u{fffd}');
                            } else {
                                out.push(char::from_u32(hi).unwrap_or('\u{fffd}'));
                            }
                        }
                        _ => return Err(JsonError::Syntax),
                    }
                }
                _ => return Err(JsonError::Syntax),
            }
        }
    }
}

// ── IPC 인자(serde_json::Value) 해석 ──────────────────────────────────────
// renderer 가 보낸 인자는 JSON 을 거쳐 오므로 JS 의 `undefined` 는 키가 없는 것이고, 최상위 `undefined` 는 `null` 과 같게 다룬다(검증 함수가 둘을 가르지 않는다).

/// JS `typeof v === 'number'` 이고 유한할 때의 값
pub fn arg_number(v: &serde_json::Value) -> Option<f64> {
    v.as_f64()
}

/// JS `Number.isInteger`
pub fn is_integer(n: f64) -> bool {
    n.is_finite() && n.fract() == 0.0
}

/// JS `Number.isSafeInteger`
pub fn is_safe_integer(n: f64) -> bool {
    is_integer(n) && n.abs() <= 9_007_199_254_740_991.0
}

/// `CONTROL_RE` = `/[\u0000-\u001f\u007f]/`
pub fn has_control(s: &str) -> bool {
    s.chars().any(|c| c <= '\u{1f}' || c == '\u{7f}')
}

/// `x.key` — 객체일 때만(배열·원시값은 `undefined`)
pub fn field<'a>(v: &'a serde_json::Value, key: &str) -> Option<&'a serde_json::Value> {
    v.as_object().and_then(|o| o.get(key))
}

/// JS 에서 `v && typeof v === 'object'` 인가(배열도 객체다 — 필드는 전부 `undefined`)
pub fn is_objectish(v: &serde_json::Value) -> bool {
    v.is_object() || v.is_array()
}

// ── JS 값 의미 ────────────────────────────────────────────────────────────

/// JS `Number(문자열)`(StringToNumber). 해석할 수 없으면 NaN. 빈 문자열·공백만은 0, `0x`·`0o`·`0b` 는 부호 없이만, `Infinity` 는 부호 가능
pub fn js_number(s: &str) -> f64 {
    let t = js_trim(s);
    if t.is_empty() {
        return 0.0;
    }
    let b = t.as_bytes();
    if b.len() > 2 && b[0] == b'0' {
        let radix = match b[1] {
            b'x' | b'X' => 16,
            b'o' | b'O' => 8,
            b'b' | b'B' => 2,
            _ => 0,
        };
        if radix != 0 {
            let mut acc = 0f64;
            for &c in &b[2..] {
                match (c as char).to_digit(radix) {
                    Some(d) => acc = acc * f64::from(radix) + f64::from(d),
                    None => return f64::NAN,
                }
            }
            return acc;
        }
    }
    let (neg, body) = match b[0] {
        b'+' => (false, &t[1..]),
        b'-' => (true, &t[1..]),
        _ => (false, t),
    };
    let n = if body == "Infinity" {
        f64::INFINITY
    } else if is_decimal_literal(body) {
        body.parse::<f64>().unwrap_or(f64::NAN)
    } else {
        return f64::NAN;
    };
    if neg {
        -n
    } else {
        n
    }
}

/// `StrUnsignedDecimalLiteral` 중 무한대가 아닌 것: `1` `1.` `1.5` `.5` `1e5` `1.5E-3`
fn is_decimal_literal(s: &str) -> bool {
    let b = s.as_bytes();
    let mut i = 0;
    let int = b[i..].iter().take_while(|c| c.is_ascii_digit()).count();
    i += int;
    let mut frac = 0;
    if b.get(i) == Some(&b'.') {
        i += 1;
        frac = b[i..].iter().take_while(|c| c.is_ascii_digit()).count();
        i += frac;
    }
    if int + frac == 0 {
        return false;
    }
    if matches!(b.get(i), Some(b'e' | b'E')) {
        i += 1;
        if matches!(b.get(i), Some(b'+' | b'-')) {
            i += 1;
        }
        let exp = b[i..].iter().take_while(|c| c.is_ascii_digit()).count();
        if exp == 0 {
            return false;
        }
        i += exp;
    }
    i == b.len()
}

/// 헤더 값 `Number(h.get(name))`: 헤더가 없으면(`null`) 0
pub fn js_number_opt(v: Option<&str>) -> f64 {
    v.map_or(0.0, js_number)
}

/// JS `String(숫자)` / 템플릿 보간. 정수는 소수점 없이, 1e21 이상은 `1e+21` 꼴
pub fn js_number_string(n: f64) -> String {
    if n.is_nan() {
        return "NaN".into();
    }
    if n.is_infinite() {
        return if n > 0.0 { "Infinity" } else { "-Infinity" }.into();
    }
    if n == 0.0 {
        return "0".into();
    }
    let a = n.abs();
    if (1e-6..1e21).contains(&a) {
        return format!("{n}");
    }
    // 매우 크거나 작은 수: 가수 + e±지수
    let s = format!("{n:e}");
    match s.split_once('e') {
        Some((m, e)) if !e.starts_with('-') => format!("{m}e+{e}"),
        _ => s,
    }
}
