//! 확인 창에 저장소가 정한 문자열(설정 키·값·이름)을 보여 줄 때의 안전한 표시. `src/main/ipc/confirmText.ts` 의 이식이다.
//! 저장소는 적대적일 수 있다 — 줄바꿈·제어 문자·방향 제어 문자·닮은꼴 따옴표·빈 칸처럼 보이는 문자로 "허용해도 안전한" 것처럼 보이는
//! 문구를 꾸미지 못하게, 눈에 안 보이거나 다른 글자·구분자로 오인될 수 있는 문자를 `\uXXXX` 로 드러낸다.
//!
//! JS 문자열은 UTF-16 이라 `display` 의 자르기가 서로게이트 쌍을 쪼갤 수 있고, 그 반쪽은 짝 없는 서로게이트(`\ud83d` 로 드러난다)가 된다.
//! Rust `str` 은 그런 값을 담지 못하므로 안쪽은 UTF-16 코드 단위(`&[u16]`)로 처리하고, `&str` 진입점은 `encode_utf16` 으로 넘긴다.
//!
//! 무엇이 "특수"인지(SPECIAL)는 TS 의 유니코드 속성 정규식(`\p{Cc}\p{Cf}…`)이 정한다. 속성 표를 Rust 에 들이지 않고, TS 가 0..=0x10FFFF 를 훑어 낸
//! 일치 범위를 `special_table.rs` 로 만들어 쓴다(`scripts/gen-fetch-special.mjs`). 표는 Node 의 Unicode 판에 묶인다 — 판이 바뀌면 골든과 함께 다시 만든다.

use super::special_table::SPECIAL_RANGES;

/// 한 번에 보여 줄 수 있는 항목 수. 넘으면 사용자가 전부 볼 수 없으므로 허용 버튼을 주지 않는다
pub const MAX_CONFIRM_ITEMS: usize = 12;
const MAX_SHOWN: usize = 80;

/// 코드 포인트가 `\uXXXX` 로 드러낼 문자인가(짝 없는 서로게이트 포함)
pub fn is_special(cp: u32) -> bool {
    SPECIAL_RANGES.binary_search_by(|&(lo, hi)| if cp < lo { std::cmp::Ordering::Greater } else if cp > hi { std::cmp::Ordering::Less } else { std::cmp::Ordering::Equal }).is_ok()
}

/// JS 문자열 이터레이터(`for (const ch of s)`)와 같은 분해: 짝이 맞는 서로게이트는 한 코드 포인트, 짝 없는 것은 그 값 그대로
fn code_points(units: &[u16]) -> Vec<u32> {
    let mut out = Vec::with_capacity(units.len());
    let mut i = 0;
    while i < units.len() {
        let u = u32::from(units[i]);
        if (0xd800..0xdc00).contains(&u) {
            if let Some(&next) = units.get(i + 1) {
                let lo = u32::from(next);
                if (0xdc00..0xe000).contains(&lo) {
                    out.push(0x10000 + ((u - 0xd800) << 10) + (lo - 0xdc00));
                    i += 2;
                    continue;
                }
            }
        }
        out.push(u);
        i += 1;
    }
    out
}

fn escape_char(cp: u32, out: &mut String) {
    if is_special(cp) {
        // 0xffff 이하는 4자리로 채우고, 그 위는 `\u{…}`(JS 의 toString(16) 과 같은 소문자)
        if cp > 0xffff {
            out.push_str(&format!("\\u{{{cp:x}}}"));
        } else {
            out.push_str(&format!("\\u{cp:04x}"));
        }
        return;
    }
    // 나머지는 JSON 규칙. 제어 문자는 모두 특수라서 여기에는 `"` 와 `\` 만 걸린다
    match char::from_u32(cp) {
        Some('"') => out.push_str("\\\""),
        Some('\\') => out.push_str("\\\\"),
        Some(c) => out.push(c),
        None => unreachable!("짝 없는 서로게이트는 특수 문자다"),
    }
}

/// 따옴표로 감싸 보여 준다(UTF-16 코드 단위 입력)
pub fn quote_units(units: &[u16]) -> String {
    let mut out = String::from("\"");
    for cp in code_points(units) {
        escape_char(cp, &mut out);
    }
    out.push('"');
    out
}

/// 따옴표로 감싸 보여 준다. 문자 단위로 검사하므로 `\uXXXX` 로 바뀐 문자는 구분자·공백으로 읽힐 수 없다
pub fn quote_for_display(s: &str) -> String {
    quote_units(&s.encode_utf16().collect::<Vec<u16>>())
}

/// 긴 문자열은 앞…뒤만 남기고, 줄였다는 사실과 전체 길이(UTF-16 단위)를 따옴표 밖에 덧붙인다
pub fn display_units(units: &[u16], max: usize) -> String {
    if units.len() <= max {
        return quote_units(units);
    }
    let head = (max as f64 * 0.6).ceil() as usize;
    let tail = max - head;
    let mut shown: Vec<u16> = Vec::with_capacity(max + 1);
    shown.extend_from_slice(&units[..head]);
    shown.push(0x2026); // …
    shown.extend_from_slice(&units[units.len() - tail..]);
    format!("{} (길이 {})", quote_units(&shown), units.len())
}

pub fn display(s: &str) -> String {
    display_units(&s.encode_utf16().collect::<Vec<u16>>(), MAX_SHOWN)
}

/// `display` 의 최대 길이를 지정하는 형태(TS `display(s, max)`)
pub fn display_max(s: &str, max: usize) -> String {
    display_units(&s.encode_utf16().collect::<Vec<u16>>(), max)
}

pub fn format_item_units(key: &[u16], value: &[u16]) -> String {
    format!("{} = {}", display_units(key, MAX_SHOWN), display_units(value, MAX_SHOWN))
}

pub fn format_item(key: &str, value: &str) -> String {
    format!("{} = {}", display(key), display(value))
}
