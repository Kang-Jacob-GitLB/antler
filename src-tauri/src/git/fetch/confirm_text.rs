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
        // 짝 없는 서로게이트는 특수 문자라 위에서 걸린다. 표가 어긋나도 패닉하지 않고 드러내는 쪽으로 간다
        None => out.push_str(&format!("\\u{cp:04x}")),
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

/// fetch 확인 창의 옵션(TS `fetchConfirmOptions` 의 `options`: Electron 의 MessageBoxOptions 중 쓰는 것만)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfirmBox {
    pub buttons: Vec<String>,
    pub default_id: usize,
    pub cancel_id: usize,
    pub title: String,
    pub message: String,
    pub detail: String,
}

impl ConfirmBox {
    /// 사이드카 shim(`electronShim.ts` 의 `showMessageBox`)이 Rust 에 넘기는 `MessageBoxArg`: 본문은 message 와 detail 을 빈 줄로 잇되 빈 것은 뺀다.
    /// 네이티브 대화상자는 기본 버튼 지정을 지원하지 않아 `defaultId` 는 가지 않는다(취소가 마지막 자리·Esc 가 된다)
    pub fn to_message_box_arg(&self) -> serde_json::Value {
        let message: Vec<&str> = [self.message.as_str(), self.detail.as_str()].into_iter().filter(|s| !s.is_empty()).collect();
        serde_json::json!({
            "kind": "warning",
            "title": self.title,
            "message": message.join("\n\n"),
            "buttons": self.buttons,
            "cancelId": self.cancel_id,
        })
    }
}

const CONFIRM_FOOTER: &str = "직접 만든 설정이거나 신뢰하는 저장소일 때만 허용하세요. \"이번만\"은 이번 실행 동안, \"항상\"은 앱을 다시 켜도 적용되며, 어느 쪽이든 위 키·값이 바뀌면 다시 묻습니다. 값이 가리키는 스크립트·파일의 내용이 바뀌는 것은 확인하지 않습니다.";

/// TS `fetchConfirmOptions`: 알 수 없는 저장소 설정이 있을 때 수동 fetch 전에 띄우는 확인 창. 앞 값은 `tooMany`(전부 보여 줄 수 없어 허용을 묻지 않고 닫기만 있는 차단 창).
/// 기본 선택은 어느 쪽이든 취소(닫기)다. 저장소가 정한 문자열(이름·키·값)은 모두 [`display`] 로 드러낸다
pub fn fetch_confirm_box(name: &str, items: &[(&str, &str)]) -> (bool, ConfirmBox) {
    let title = format!("{} 저장소의 설정에 fetch 때 외부 프로그램을 실행하거나 연결을 가로챌 수 있는 항목이 있습니다.", display(name));
    let shown = |list: &[(&str, &str)]| list.iter().map(|(k, v)| format_item(k, v)).collect::<Vec<_>>().join("\n");
    if items.len() > MAX_CONFIRM_ITEMS {
        let detail = format!(
            "항목이 {}개로 너무 많아 모두 확인할 수 없어 fetch 하지 않습니다. 저장소의 .git/config 를 직접 확인하세요.\n\n{}\n… 외 {}개",
            items.len(),
            shown(&items[..MAX_CONFIRM_ITEMS]),
            items.len() - MAX_CONFIRM_ITEMS
        );
        return (true, ConfirmBox { buttons: vec!["닫기".into()], default_id: 0, cancel_id: 0, title: "fetch 차단".into(), message: title, detail });
    }
    let detail = format!("{}\n\n{CONFIRM_FOOTER}", shown(items));
    (
        false,
        ConfirmBox {
            buttons: vec!["이번만 fetch".into(), "이 저장소에서 항상 허용".into(), "취소".into()],
            default_id: 2,
            cancel_id: 2,
            title: "fetch 확인".into(),
            message: title,
            detail,
        },
    )
}
