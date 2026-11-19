// 워크플로(.github/workflows/*.yml) 정적 검사용 작은 YAML 해석기. js-yaml 은 electron-builder 의 전이 의존이라 Electron 을 지운 뒤에는 없고,
// 새 npm 패키지는 들이지 않는다 — 그래서 이 저장소의 워크플로가 쓰는 부분집합만 푼다:
//   블록 매핑·블록 시퀀스·한 줄 `[a, b]` 흐름 시퀀스·평문/작은따옴표/큰따옴표 문자열·`|`·`|-`·`>` 블록 스칼라·주석(`# …`)·true/false/null/정수.
// 앵커·태그·여러 문서·여러 줄 평문·중괄호 흐름 매핑은 지원하지 않고 만나면 던진다(조용히 틀리게 읽지 않는다).
// 같은 파일을 js-yaml 과 대조해 이 부분집합에서 결과가 같음을 확인했다(PR 7 작성 때 한 번, docs/tauri.md).

export type Yaml = null | boolean | number | string | Yaml[] | { [k: string]: Yaml }

interface Line {
  indent: number
  text: string
  /** 원본 줄(블록 스칼라용) */
  raw: string
}

/** 따옴표 밖의 ` #` 부터 주석이다 */
function stripComment(s: string): string {
  let q = ''
  for (let i = 0; i < s.length; i++) {
    const c = s[i]!
    if (q) {
      if (c === q) q = ''
      else if (q === '"' && c === '\\') i++
    } else if (c === '"' || c === "'") {
      // 값 한가운데의 아포스트로피(예: don't)는 따옴표가 아니다 — 토큰 시작에서만 연다
      if (i === 0 || /[\s[,:]/.test(s[i - 1]!)) q = c
    } else if (c === '#' && (i === 0 || /\s/.test(s[i - 1]!))) return s.slice(0, i)
  }
  return s
}

function scalar(src: string): Yaml {
  const s = src.trim()
  if (s === '' || s === '~' || s === 'null') return null
  if (s === 'true') return true
  if (s === 'false') return false
  if (/^-?(0|[1-9]\d*)$/.test(s)) return Number(s)
  if (s.startsWith('"')) {
    if (!s.endsWith('"') || s.length < 2) throw new Error(`닫히지 않은 큰따옴표: ${s}`)
    return JSON.parse(s) as string
  }
  if (s.startsWith("'")) {
    if (!s.endsWith("'") || s.length < 2) throw new Error(`닫히지 않은 작은따옴표: ${s}`)
    return s.slice(1, -1).replace(/''/g, "'")
  }
  if (s.startsWith('[')) {
    if (!s.endsWith(']')) throw new Error(`여러 줄 흐름 시퀀스는 지원하지 않는다: ${s}`)
    const inner = s.slice(1, -1).trim()
    return inner === '' ? [] : inner.split(',').map((x) => scalar(x))
  }
  if (s.startsWith('{') || s.startsWith('&') || s.startsWith('*') || s.startsWith('!')) throw new Error(`지원하지 않는 YAML: ${s}`)
  return s
}

export function parseYaml(text: string): Yaml {
  const src = text.replace(/^﻿/, '').replace(/\r\n/g, '\n').split('\n')
  const lines: Line[] = src.map((raw) => ({ indent: raw.length - raw.trimStart().length, text: raw.trim(), raw }))
  let pos = 0

  const skipBlank = (): void => {
    while (pos < lines.length && (lines[pos]!.text === '' || lines[pos]!.text.startsWith('#'))) pos++
  }

  /** `key: value` 한 줄을 (키, 값 문자열)로 가른다. 따옴표 안의 `: ` 는 키 구분이 아니다 */
  function splitKey(t: string): [string, string] | null {
    let q = ''
    for (let i = 0; i < t.length; i++) {
      const c = t[i]!
      if (q) {
        if (c === q) q = ''
        else if (q === '"' && c === '\\') i++
      } else if (c === '"' || c === "'") {
        if (i === 0) q = c
      } else if (c === ':' && (i === t.length - 1 || t[i + 1] === ' ')) {
        const k = t.slice(0, i).trim()
        return [typeof scalar(k) === 'string' ? (scalar(k) as string) : String(scalar(k)), t.slice(i + 1)]
      }
    }
    return null
  }

  function blockScalar(parentIndent: number, header: string): string {
    const style = header[0]!
    const chomp = header.includes('-') ? 'strip' : header.includes('+') ? 'keep' : 'clip'
    const body: string[] = []
    let indent = -1
    while (pos < lines.length) {
      const l = lines[pos]!
      if (l.text !== '' && l.indent <= parentIndent) break
      if (indent < 0 && l.text !== '') indent = l.indent
      body.push(l.text === '' ? '' : l.raw.slice(Math.min(indent, l.indent)))
      pos++
    }
    while (body.length && body[body.length - 1] === '') body.pop()
    const joined = style === '>' ? body.join(' ') : body.join('\n')
    return chomp === 'strip' ? joined : `${joined}\n`
  }

  function valueAfterKey(rest: string, parentIndent: number): Yaml {
    const v = stripComment(rest).trim()
    if (/^[|>][+-]?\d*$/.test(v)) return blockScalar(parentIndent, v)
    if (v === '') {
      skipBlank()
      if (pos < lines.length) {
        const n = lines[pos]!
        // 시퀀스는 부모 키와 같은 들여쓰기에도 올 수 있다
        if (n.indent > parentIndent || (n.indent === parentIndent && n.text.startsWith('- '))) return node(n.indent)
        if (n.indent === parentIndent && n.text === '-') return node(n.indent)
      }
      return null
    }
    return scalar(v)
  }

  function mapping(indent: number, first?: { key: string; rest: string; keyIndent: number }): { [k: string]: Yaml } {
    const out: { [k: string]: Yaml } = {}
    if (first) out[first.key] = valueAfterKey(first.rest, first.keyIndent)
    for (;;) {
      skipBlank()
      if (pos >= lines.length) break
      const l = lines[pos]!
      if (l.indent < indent) break
      if (l.indent > indent) throw new Error(`들여쓰기가 맞지 않다(줄 ${pos + 1}): ${l.text}`)
      if (l.text.startsWith('- ') || l.text === '-') break
      const kv = splitKey(stripComment(l.text))
      if (!kv) throw new Error(`매핑 줄이 아니다(줄 ${pos + 1}): ${l.text}`)
      pos++
      out[kv[0]] = valueAfterKey(kv[1], indent)
    }
    return out
  }

  function sequence(indent: number): Yaml[] {
    const out: Yaml[] = []
    for (;;) {
      skipBlank()
      if (pos >= lines.length) break
      const l = lines[pos]!
      if (l.indent !== indent || !(l.text.startsWith('- ') || l.text === '-')) break
      const after = l.text === '-' ? '' : stripComment(l.text.slice(2)).trim()
      const itemIndent = indent + (l.text === '-' ? 1 : l.text.length - l.text.slice(2).trimStart().length)
      pos++
      if (after === '') {
        skipBlank()
        out.push(pos < lines.length && lines[pos]!.indent > indent ? node(lines[pos]!.indent) : null)
        continue
      }
      const kv = splitKey(after)
      if (kv && !after.startsWith('[') && !after.startsWith('"') && !after.startsWith("'")) {
        out.push(mapping(itemIndent, { key: kv[0], rest: kv[1], keyIndent: itemIndent }))
      } else {
        out.push(scalar(after))
      }
    }
    return out
  }

  function node(indent: number): Yaml {
    skipBlank()
    const l = lines[pos]
    if (!l) return null
    if (l.text.startsWith('- ') || l.text === '-') return sequence(l.indent)
    return mapping(indent)
  }

  skipBlank()
  const result = node(lines[pos]?.indent ?? 0)
  skipBlank()
  if (pos < lines.length) throw new Error(`해석하지 못한 줄(${pos + 1}): ${lines[pos]!.text}`)
  return result
}
