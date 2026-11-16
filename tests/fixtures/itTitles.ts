// 테스트 파일의 `describe` / `it` / `it.each` 제목을 소스 텍스트에서 뽑는다(테스트를 실제로 돌리지 않고, `%s` 같은 템플릿은 그대로 둔다).
// 이식 대응표(tests/golden/fetch/test-map.json)가 원본 테스트(tests/fetch.test.ts)를 빠짐없이 덮는지 강제하는 데 쓴다.

const STRING = String.raw`(['"\`])((?:\\.|(?!\1)[^\\\n])*)\1`

function unquote(quote: string, body: string): string {
  // 제목에 쓰인 이스케이프(\\, \', \")만 있다 — 소스의 문자열 리터럴을 그대로 평가하지 않고 필요한 것만 푼다
  return body.replace(/\\(.)/g, (_m, c: string) => (c === 'n' ? '\n' : c)).replace(new RegExp(`\\\\${quote}`, 'g'), quote)
}

/** `<describe 제목> > <it 제목>` 목록(소스 순서, 중복 포함) */
export function itKeys(source: string): string[] {
  const keys: string[] = []
  let describeTitle = ''
  const token = new RegExp(String.raw`^describe\(\s*${STRING}|^\s*it(\.each)?\b`, 'gm')
  const each = new RegExp(String.raw`\)\(\s*${STRING}`, 'g')
  const plain = new RegExp(String.raw`\(\s*${STRING}`, 'y')
  for (let m = token.exec(source); m; m = token.exec(source)) {
    if (m[0].startsWith('describe')) {
      describeTitle = unquote(m[1]!, m[2]!)
      continue
    }
    let t: RegExpExecArray | null
    if (m[3]) {
      each.lastIndex = m.index + m[0].length
      t = each.exec(source)
    } else {
      plain.lastIndex = m.index + m[0].length
      t = plain.exec(source)
    }
    if (!t) throw new Error(`it 제목을 읽지 못했다: ${source.slice(m.index, m.index + 60)}`)
    keys.push(`${describeTitle} > ${unquote(t[1]!, t[2]!)}`)
  }
  return keys
}
