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
pub mod confirm_text;
pub mod guard;
pub mod pattern;
pub mod special_table;

use crate::git::scope::is_js_space;

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
