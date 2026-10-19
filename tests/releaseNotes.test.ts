import { describe, expect, it } from 'vitest'
import { MAX_NOTES_CHARS, RELEASES_PAGE, fetchReleaseNotes } from '../src/main/update/releaseNotes'

/** 경로별 응답을 돌려주는 가짜 fetch. 요청한 URL 을 기록한다 */
function fakeFetch(routes: Record<string, { status?: number; json?: unknown }>): { fn: typeof fetch; seen: string[] } {
  const seen: string[] = []
  const fn = (async (url: string) => {
    seen.push(url)
    const path = url.replace('https://api.github.com/repos/Kang-Jacob-GitLB/antler', '')
    const r = routes[path]
    if (!r) return new Response('{}', { status: 404 })
    return new Response(JSON.stringify(r.json ?? {}), { status: r.status ?? 200 })
  }) as unknown as typeof fetch
  return { fn, seen }
}

describe('fetchReleaseNotes', () => {
  it('현재 버전 태그의 릴리스 본문을 돌려준다', async () => {
    const { fn, seen } = fakeFetch({
      '/releases/tags/v1.2.0': { json: { tag_name: 'v1.2.0', name: 'v1.2.0', body: '## 새 기능\n- 태그 최신순', published_at: '2026-10-19T00:00:00Z', html_url: 'https://evil.example/' } }
    })
    const n = await fetchReleaseNotes(fn, '1.2.0')
    expect(seen).toHaveLength(1)
    expect(n).toMatchObject({ version: '1.2.0', exact: true, body: '## 새 기능\n- 태그 최신순', title: 'v1.2.0', publishedAt: '2026-10-19T00:00:00Z' })
    // html_url 이 아니라 이 저장소의 릴리스 페이지로 만든다
    expect(n.url).toBe('https://github.com/Kang-Jacob-GitLB/antler/releases/tag/v1.2.0')
  })

  it('그 버전 릴리스가 없으면(개발 빌드) 최신 릴리스로 대신한다', async () => {
    const { fn, seen } = fakeFetch({ '/releases/latest': { json: { tag_name: 'v1.1.0', body: 'latest' } } })
    const n = await fetchReleaseNotes(fn, '1.2.0-dev')
    expect(seen.map((u) => u.split('/releases')[1])).toEqual(['/tags/v1.2.0-dev', '/latest'])
    expect(n).toMatchObject({ version: '1.1.0', exact: false, body: 'latest' })
  })

  it('릴리스가 하나도 없으면 빈 본문과 릴리스 목록 페이지', async () => {
    const { fn } = fakeFetch({})
    expect(await fetchReleaseNotes(fn, '1.0.0')).toEqual({ version: '1.0.0', exact: false, body: '', url: RELEASES_PAGE })
  })

  it('너무 긴 본문은 자르고 truncated', async () => {
    const { fn } = fakeFetch({ '/releases/tags/v1.0.0': { json: { tag_name: 'v1.0.0', body: 'x'.repeat(MAX_NOTES_CHARS + 10) } } })
    const n = await fetchReleaseNotes(fn, '1.0.0')
    expect(n.body).toHaveLength(MAX_NOTES_CHARS)
    expect(n.truncated).toBe(true)
  })

  it('요청 한도·서버 오류는 오류로 알린다', async () => {
    await expect(fetchReleaseNotes(fakeFetch({ '/releases/tags/v1.0.0': { status: 403 } }).fn, '1.0.0')).rejects.toThrow('요청 한도')
    await expect(fetchReleaseNotes(fakeFetch({ '/releases/tags/v1.0.0': { status: 500 } }).fn, '1.0.0')).rejects.toThrow('HTTP 500')
  })

  it('이상한 태그 이름은 링크로 쓰지 않는다', async () => {
    const { fn } = fakeFetch({ '/releases/tags/v1.0.0': { json: { tag_name: 'v1.0.0/../../x', body: 'b' } } })
    expect((await fetchReleaseNotes(fn, '1.0.0')).url).toBe(RELEASES_PAGE)
  })
})
