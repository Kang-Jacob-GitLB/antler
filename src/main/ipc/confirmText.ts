// 확인 창에 저장소가 정한 문자열(설정 키·값·이름)을 보여 줄 때의 안전한 표시. 저장소는 적대적일 수 있다 —
// 줄바꿈·제어 문자·방향 제어 문자·닮은꼴 따옴표·빈 칸처럼 보이는 문자로 "허용해도 안전한" 것처럼 보이는 문구를 꾸미지 못하게 한다.

/** 한 번에 보여 줄 수 있는 항목 수. 넘으면 사용자가 전부 볼 수 없으므로 허용 버튼을 주지 않는다 */
export const MAX_CONFIRM_ITEMS = 12
const MAX_SHOWN = 80

/**
 * 그대로 보여 주지 않고 \uXXXX 로 드러내는 문자. 눈에 안 보이거나, 보이더라도 다른 글자·구분자로 오인될 수 있는 것들이다.
 *  - Cc·Cf·Zl·Zp·Cs   제어(C0·DEL·C1), 서식(소프트 하이픈·ALM·폭 없는 문자·방향 제어·BOM·태그), 줄/문단 구분자, 짝 없는 서로게이트
 *  - Default_Ignorable_Code_Point   화면에 그려지지 않는 문자 전반(한글 채움 문자 U+115F/1160/3164/FFA0, 변이 선택자, U+034F 등)
 *  - Quotation_Mark   “ ” ＂ « » 등 닮은꼴 따옴표 — 값 안에서 `" = "` 경계를 흉내 내지 못하게(ASCII " 는 JSON 이 \" 로, ' 는 구분자가 아니라 그대로 둔다)
 *  - Zs(U+0020 제외)  NBSP·전각 공백 등 공백처럼 보이는 문자
 *  - M   결합 문자(U+0336 겹침 줄 등으로 앞 글자를 가리거나 바꿔 보이게 함)
 *  - U+2800(점자 빈칸) · U+180E(옛 몽골어 모음 구분자, 공백으로 쓰였음)
 */
const SPECIAL =
  /[\p{Cc}\p{Cf}\p{Zl}\p{Zp}\p{Cs}\p{Default_Ignorable_Code_Point}\p{M}⠀᠎ᅟᅠㅤﾠ]|(?! )\p{Zs}|(?!["'])\p{Quotation_Mark}/u

function escapeChar(ch: string): string {
  if (SPECIAL.test(ch)) {
    const cp = ch.codePointAt(0)!
    return cp > 0xffff ? `\\u{${cp.toString(16)}}` : `\\u${cp.toString(16).padStart(4, '0')}`
  }
  // 나머지는 JSON 규칙(", \, \n 등)대로. 따옴표를 빼고 앞뒤에 직접 붙인다
  return JSON.stringify(ch).slice(1, -1)
}

/** 따옴표로 감싸 보여 준다. 문자 단위로 검사하므로 \uXXXX 로 바뀐 문자는 구분자·공백으로 읽힐 수 없다 */
export function quoteForDisplay(s: string): string {
  let out = ''
  for (const ch of s) out += escapeChar(ch)
  return `"${out}"`
}

/** 긴 문자열은 앞…뒤만 남기고, 줄였다는 사실과 전체 길이를 따옴표 밖에 덧붙인다(가운데에 숨긴 내용이 있음을 알린다) */
export function display(s: string, max = MAX_SHOWN): string {
  if (s.length <= max) return quoteForDisplay(s)
  const head = Math.ceil(max * 0.6)
  const tail = max - head
  return `${quoteForDisplay(`${s.slice(0, head)}…${s.slice(s.length - tail)}`)} (길이 ${s.length})`
}

export function formatItem(key: string, value: string): string {
  return `${display(key)} = ${display(value)}`
}
