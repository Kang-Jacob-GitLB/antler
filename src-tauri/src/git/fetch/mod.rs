//! fetch 가드의 판정 로직(`src/main/git/fetch.ts`·`src/main/ipc/confirmText.ts`·`FetchService.ts` 의 순수 함수) 이식.
//! git 을 실행하지 않고 앱에도 아직 연결되지 않았다 — Phase 3 의 첫 단계로, 허용 목록·URL 판정·설정 파싱·확인 창 표시·오류 분류가
//! TS 와 글자 그대로 같은 결과를 내는지만 골든(`tests/golden/fetch/*.json`)으로 고정한다. 실행(run.rs)·상태 기계는 뒤 단계다.
//!
//! 이식 규칙(어기면 보안 판정이 TS 와 달라진다):
//! - **정규식 크레이트를 쓰지 않는다.** 허용 목록(`ALLOWED_KEYS`)은 TS 정규식 소스를 그대로 두고 닫힌 문법만 해석하는 매처([`pattern`])로 판정한다.
//!   나머지 정규식은 의미를 손으로 옮겼고, 어디서 달라지는지는 각 함수에 적었다.
//! - JS 의 `trim`·`\s` 는 U+FEFF 를 공백으로 보고 U+0085 는 아니다(Rust `char::is_whitespace` 는 반대) — [`js_trim`].
//! - JS `.` 는 `\n`·`\r`·U+2028·U+2029 를 맞추지 않는다. `$` 는 문자열의 맨 끝만이다(끝 `\n` 앞이 아니다).
//! - `i` 플래그(유니코드 아닌 정규식)는 ASCII 만 접는다: `ſ`(U+017F)·`K`(U+212A)는 `s`·`k` 와 같지 않다 → `to_ascii_lowercase` 로 비교한다.
//! - 길이·자르기는 UTF-16 코드 단위다(JS `length`·`slice`). 바이트·`char` 로 자르면 이모지 경계에서 TS 와 달라지거나 패닉한다.
//! - **Unicode 판은 Node 에 고정한다.** 소문자화([`js_to_lowercase`])와 확인 창의 SPECIAL 은 std 의 표가 아니라 Node 에서 뽑은 생성 표
//!   (`lowercase_table.rs`·`special_table.rs`)만 쓴다. std 의 `to_lowercase` 는 Node 가 모르는 대소문자 쌍(U+A7CE→A7CF 등)을 바꿔, 설정 키가 TS 는 "알 수 없는 설정"인데 Rust 는 허용으로 갈린다.
pub mod confirm_text;
pub mod guard;
pub mod lowercase_table;
pub mod pattern;
pub mod run;
pub mod special_table;

use lowercase_table::{CASED_RANGES, CASE_IGNORABLE_RANGES, LOWERCASE_MAP};

use crate::git::scope::is_js_space;

fn in_ranges(cp: u32, ranges: &[(u32, u32)]) -> bool {
    ranges
        .binary_search_by(|&(lo, hi)| if cp < lo { std::cmp::Ordering::Greater } else if cp > hi { std::cmp::Ordering::Less } else { std::cmp::Ordering::Equal })
        .is_ok()
}

/// Σ(U+03A3)가 단어 끝의 시그마(ς 로 소문자화)인가: 앞에 Cased 글자(사이에 Case_Ignorable 만 있어도 된다)가 있고, 뒤에는 그런 글자가 없다
fn is_final_sigma(s: &str, at: usize) -> bool {
    let cased = |c: char| in_ranges(c as u32, CASED_RANGES);
    let ignorable = |c: char| in_ranges(c as u32, CASE_IGNORABLE_RANGES);
    let before = s[..at].chars().rev().find(|&c| !ignorable(c)).is_some_and(cased);
    before && !s[at + '\u{3a3}'.len_utf8()..].chars().find(|&c| !ignorable(c)).is_some_and(cased)
}

/// JS `String.prototype.toLowerCase`: Node 의 변환표(생성 파일)만 쓴다. 문맥 규칙은 최종 시그마뿐이다(언어별 규칙은 JS 도 쓰지 않는다)
pub fn js_to_lowercase(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for (i, c) in s.char_indices() {
        if c.is_ascii() {
            out.push(c.to_ascii_lowercase());
        } else if c == '\u{3a3}' && is_final_sigma(s, i) {
            out.push('\u{3c2}');
        } else {
            match LOWERCASE_MAP.binary_search_by_key(&(c as u32), |e| e.0) {
                Ok(k) => out.push_str(LOWERCASE_MAP[k].1),
                Err(_) => out.push(c),
            }
        }
    }
    out
}

/// JS `String.prototype.trim`: 공백·줄 구분자·U+FEFF 를 양끝에서 지운다(U+0085 는 지우지 않는다)
pub fn js_trim(s: &str) -> &str {
    s.trim_matches(is_js_space)
}

/// JS `.`(dotAll 아님)가 맞추지 않는 줄 끝 문자
pub fn is_line_terminator(c: char) -> bool {
    matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}')
}

/// JS `s.slice(0, end)` 를 UTF-16 코드 단위로 한 결과. 서로게이트 쌍이 잘리면 반쪽을 U+FFFD 로 바꾼다
/// (Rust 문자열은 짝 없는 서로게이트를 담지 못한다 — JS 의 `toWellFormed()` 와 같은 결과다)
pub fn slice_utf16_lossy(s: &str, end: usize) -> String {
    let units: Vec<u16> = s.encode_utf16().collect();
    String::from_utf16_lossy(&units[..end.min(units.len())])
}
