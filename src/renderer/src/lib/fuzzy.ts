// 명령 팔레트용 퍼지 매칭(순수 함수). 대소문자 무시, 글자 순서만 맞으면 일치하는 부분열 매칭.
// 글자 단위는 유니코드 코드 포인트(한글 음절 하나 = 한 글자)이고, indices 도 그 기준이다.

export interface FuzzyMatch {
  score: number
  /** 일치한 글자의 코드 포인트 위치(강조 표시용, 오름차순) */
  indices: number[]
}

const SEPARATORS = new Set([' ', '-', '_', '/', ':', '.', '(', ')', '·'])
const chars = (s: string): string[] => [...s.toLowerCase()]

/**
 * query 가 text 의 부분열이면 점수와 일치 위치를, 아니면 null.
 * 연속으로 붙어 있을수록·단어 앞글자일수록·앞쪽일수록 점수가 높다. 통째로 포함(부분 문자열)되면 항상 더 높다.
 */
export function fuzzyMatch(query: string, text: string): FuzzyMatch | null {
  const q = chars(query.trim())
  const s = chars(text)
  if (q.length === 0) return { score: 0, indices: [] }
  if (q.length > s.length) return null

  // 부분 문자열로 들어 있으면 그 위치 중 단어 시작이 있는 곳을 우선한다
  let best = -1
  for (let i = 0; i + q.length <= s.length; i++) {
    if (q.every((c, k) => s[i + k] === c)) {
      if (best < 0) best = i
      if (i === 0 || SEPARATORS.has(s[i - 1]!)) {
        best = i
        break
      }
    }
  }
  if (best >= 0) {
    const atBoundary = best === 0 || SEPARATORS.has(s[best - 1]!)
    const score = 1000 + (best === 0 ? 200 : 0) + (atBoundary ? 100 : 0) - best - (s.length - q.length) * 0.5
    return { score, indices: q.map((_, k) => best + k) }
  }

  // 부분열: 앞에서부터 가장 이른 위치에 욕심껏 맞춘다
  const indices: number[] = []
  let score = 0
  let prev = -2
  let from = 0
  for (const c of q) {
    let at = -1
    for (let i = from; i < s.length; i++) {
      if (s[i] === c) {
        at = i
        break
      }
    }
    if (at < 0) return null
    score += 10
    if (at === prev + 1) score += 15
    if (at === 0 || SEPARATORS.has(s[at - 1]!)) score += 10
    indices.push(at)
    prev = at
    from = at + 1
  }
  score -= indices[0]! + (indices[indices.length - 1]! - indices[0]! + 1 - q.length) * 0.5
  return { score, indices }
}

export interface Ranked<T> {
  item: T
  score: number
  /** title 에서 일치한 위치(키워드로만 일치했으면 빈 배열) */
  indices: number[]
}

/**
 * 항목을 질의로 걸러 점수순으로 정렬한다. 공백으로 나눈 낱말이 모두 title 이나 keywords 에 있어야 한다.
 * 질의가 비어 있으면 입력 순서 그대로 돌려준다. 점수가 같으면 입력 순서를 유지한다(안정 정렬).
 */
export function rankByQuery<T>(items: readonly T[], query: string, fields: (item: T) => { title: string; keywords?: string }): Ranked<T>[] {
  const tokens = query.trim().split(/\s+/).filter(Boolean)
  if (tokens.length === 0) return items.map((item) => ({ item, score: 0, indices: [] }))

  const out: (Ranked<T> & { order: number })[] = []
  items.forEach((item, order) => {
    const { title, keywords } = fields(item)
    let score = 0
    const hit = new Set<number>()
    for (const tok of tokens) {
      const inTitle = fuzzyMatch(tok, title)
      const inKeys = keywords ? fuzzyMatch(tok, keywords) : null
      if (!inTitle && !inKeys) return
      // 제목 일치가 키워드 일치보다 낫다
      if (inTitle && (!inKeys || inTitle.score + 200 >= inKeys.score)) {
        score += inTitle.score + 200
        inTitle.indices.forEach((i) => hit.add(i))
      } else score += inKeys!.score
    }
    out.push({ item, score, indices: [...hit].sort((a, b) => a - b), order })
  })
  out.sort((a, b) => b.score - a.score || a.order - b.order)
  return out.map(({ item, score, indices }) => ({ item, score, indices }))
}
