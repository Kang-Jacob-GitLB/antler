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

/** fetch 확인 창의 옵션(Electron.MessageBoxOptions 중 쓰는 것만. 이 모듈은 electron 을 가져오지 않는다) */
export interface FetchConfirmOptions {
  type: 'warning'
  buttons: string[]
  defaultId: number
  cancelId: number
  title: string
  message: string
  detail: string
}

/**
 * 저장소 설정에 알 수 없는 항목이 있을 때 수동 fetch 전에 띄우는 확인 창의 내용. tooMany 면 전부 보여 줄 수 없어 허용을 묻지 않고
 * 닫기만 있는 차단 창이다. 기본 선택은 어느 쪽이든 취소(닫기)다.
 */
export function fetchConfirmOptions(name: string, items: { key: string; value: string }[]): { tooMany: boolean; options: FetchConfirmOptions } {
  // 저장소가 정한 문자열(이름·키·값)은 따옴표·이스케이프로 표시한다 — 문구를 꾸며 사용자를 속이지 못하게
  const title = `${display(name)} 저장소의 설정에 fetch 때 외부 프로그램을 실행하거나 연결을 가로챌 수 있는 항목이 있습니다.`
  if (items.length > MAX_CONFIRM_ITEMS) {
    // 전부 보여 줄 수 없으면 허용 여부를 물을 수 없다
    return {
      tooMany: true,
      options: {
        type: 'warning',
        buttons: ['닫기'],
        defaultId: 0,
        cancelId: 0,
        title: 'fetch 차단',
        message: title,
        detail: `항목이 ${items.length}개로 너무 많아 모두 확인할 수 없어 fetch 하지 않습니다. 저장소의 .git/config 를 직접 확인하세요.\n\n${items
          .slice(0, MAX_CONFIRM_ITEMS)
          .map((i) => formatItem(i.key, i.value))
          .join('\n')}\n… 외 ${items.length - MAX_CONFIRM_ITEMS}개`
      }
    }
  }
  return {
    tooMany: false,
    options: {
      type: 'warning',
      buttons: ['이번만 fetch', '이 저장소에서 항상 허용', '취소'],
      defaultId: 2,
      cancelId: 2,
      title: 'fetch 확인',
      message: title,
      detail: `${items.map((i) => formatItem(i.key, i.value)).join('\n')}\n\n직접 만든 설정이거나 신뢰하는 저장소일 때만 허용하세요. "이번만"은 이번 실행 동안, "항상"은 앱을 다시 켜도 적용되며, 어느 쪽이든 위 키·값이 바뀌면 다시 묻습니다. 값이 가리키는 스크립트·파일의 내용이 바뀌는 것은 확인하지 않습니다.`
    }
  }
}
