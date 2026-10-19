// Git LFS 표시용 순수 함수. LFS 바이너리나 저장소 설정의 명령은 실행하지 않고 git 오브젝트·속성 텍스트만 해석한다.

/** LFS 포인터 파일 첫 줄 */
export const LFS_POINTER_VERSION = 'https://git-lfs.github.com/spec/v1'
/** 포인터 파일은 100바이트 안팎이다. 이보다 길면 포인터가 아니다 */
const POINTER_MAX_CHARS = 1024

export interface LfsPointer {
  /** 해시 알고리즘(현재 sha256 만 허용) */
  algo: 'sha256'
  /** 소문자 hex 64자 */
  oid: string
  /** 실제 객체 크기(바이트) */
  size: number
}

/**
 * LFS 포인터 텍스트를 읽는다. 형식: `version <url>` 로 시작하고 `key value` 줄이 이어진다.
 * 형식이 조금이라도 어긋나면(필수 키 누락·oid 형식·크기가 정수가 아님·너무 긴 텍스트) null 이다.
 */
export function parseLfsPointer(text: string): LfsPointer | null {
  if (text.length > POINTER_MAX_CHARS) return null
  const lines = text.split(/\r?\n/)
  if (lines[lines.length - 1] === '') lines.pop()
  if (lines.length < 3 || lines[0] !== `version ${LFS_POINTER_VERSION}`) return null
  let oid: string | undefined
  let size: number | undefined
  for (const line of lines.slice(1)) {
    const sp = line.indexOf(' ')
    if (sp <= 0) return null
    const key = line.slice(0, sp)
    const value = line.slice(sp + 1)
    if (key === 'oid') {
      const m = /^sha256:([0-9a-f]{64})$/.exec(value)
      if (!m || oid !== undefined) return null
      oid = m[1]!
    } else if (key === 'size') {
      if (!/^\d{1,15}$/.test(value) || size !== undefined) return null
      size = Number(value)
    }
  }
  return oid !== undefined && size !== undefined ? { algo: 'sha256', oid, size } : null
}

/** 줄 목록(diff 의 한쪽 면)에서 포인터를 찾는다. 포인터가 아니면 null */
export function pointerFromLines(lines: string[]): LfsPointer | null {
  return lines.length >= 3 && lines.length <= 12 ? parseLfsPointer(lines.join('\n')) : null
}

/**
 * `git check-attr -z filter -- <paths>` 출력(`path\0attr\0value\0` 반복)에서 filter=lfs 인 경로를 모은다.
 * 경로에 개행이 있어도 NUL 구분이라 안전하다.
 */
export function parseCheckAttrLfs(out: string): string[] {
  const parts = out.split('\0')
  const paths: string[] = []
  for (let i = 0; i + 2 < parts.length; i += 3) {
    if (parts[i + 1] === 'filter' && parts[i + 2] === 'lfs') paths.push(parts[i]!)
  }
  return paths
}

/** .gitattributes 텍스트가 LFS 를 쓰는지(`filter=lfs` 속성이 있는 규칙). 주석 줄은 무시한다 */
export function gitattributesUsesLfs(text: string): boolean {
  for (const raw of text.split(/\r?\n/)) {
    const line = raw.trim()
    if (!line || line.startsWith('#')) continue
    if (/(^|\s)filter=lfs(\s|$)/.test(line)) return true
  }
  return false
}
