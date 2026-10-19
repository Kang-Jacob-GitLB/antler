import { mkdtempSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterEach, beforeEach, describe, expect, it } from 'vitest'
import { CHECKS_BATCH, CheckStateCache, fetchCheckDetails, fetchCommitChecks, listJobs, listRuns, rollupState, TTL_DONE_MS, TTL_NONE_MS, TTL_PENDING_MS } from '../src/main/github/checks'
import { GitHubApiError, GitHubClient } from '../src/main/github/client'
import { GitHubService } from '../src/main/github/service'
import { TokenStore } from '../src/main/github/tokenStore'
import { parseCheckShas, parsePage, parseRunId, parseSha } from '../src/main/github/validate'
import { fakeFetch, json, type RecordedRequest } from './ghFake'

const slug = { owner: 'octo', repo: 'hello' }
const TOKEN = 'ghp_supersecrettoken'
const sha = (n: number): string => n.toString(16).padStart(40, '0')

/** GraphQL 요청에서 (변수 o{i} → sha) 를 읽어 sha 별 응답을 만든다 */
function graphqlResponder(rollups: Record<string, string | null | undefined>) {
  return (req: RecordedRequest): Response | undefined => {
    if (!req.url.endsWith('/graphql')) return undefined
    const vars = (JSON.parse(req.body!) as { variables: Record<string, string> }).variables
    const repository: Record<string, unknown> = {}
    for (let i = 0; `o${i}` in vars; i++) {
      const s = vars[`o${i}`]!
      const v = rollups[s]
      repository[`c${i}`] = v === undefined ? null : { statusCheckRollup: v ? { state: v } : null }
    }
    return json({ data: { repository } })
  }
}

describe('rollupState', () => {
  it('GraphQL StatusState 를 공유 상태로 옮긴다', () => {
    expect(rollupState('SUCCESS')).toBe('success')
    expect(rollupState('FAILURE')).toBe('failure')
    expect(rollupState('ERROR')).toBe('failure')
    expect(rollupState('PENDING')).toBe('pending')
    expect(rollupState('EXPECTED')).toBe('pending')
    expect(rollupState(undefined)).toBe('none')
    expect(rollupState(null)).toBe('none')
  })
})

describe('CheckStateCache', () => {
  it('이미 있는 키를 갱신해도 다른 항목을 밀어내지 않는다', () => {
    const c = new CheckStateCache()
    for (let i = 0; i < 5000; i++) c.set(`k${i}`, 'success')
    c.set('k4999', 'failure')
    expect(c.get('k0')).toBe('success')
    expect(c.get('k4999')).toBe('failure')
  })

  it('완료 상태는 오래, 진행 중은 짧게, 없음은 중간 길이로 보관한다', () => {
    let now = 1_000_000
    const c = new CheckStateCache(() => now)
    c.set('a', 'success')
    c.set('b', 'pending')
    c.set('c', 'none')
    now += TTL_PENDING_MS
    expect([c.get('a'), c.get('b'), c.get('c')]).toEqual(['success', undefined, 'none'])
    now += TTL_NONE_MS - TTL_PENDING_MS
    expect(c.get('c')).toBeUndefined()
    expect(c.get('a')).toBe('success')
    now += TTL_DONE_MS
    expect(c.get('a')).toBeUndefined()
  })
})

describe('fetchCommitChecks(GraphQL 일괄)', () => {
  it('여러 커밋을 한 번의 요청으로 묻고 성공·실패·진행 중·없음·미push 를 구분한다', async () => {
    const rollups = { [sha(1)]: 'SUCCESS', [sha(2)]: 'FAILURE', [sha(3)]: 'PENDING', [sha(4)]: null }
    const { fetch, calls } = fakeFetch(graphqlResponder(rollups))
    const client = new GitHubClient(fetch, () => TOKEN)
    const out = await fetchCommitChecks(client, slug, [1, 2, 3, 4, 5].map(sha), new CheckStateCache())
    expect(calls).toHaveLength(1)
    expect(calls[0]!.method).toBe('POST')
    expect(out).toEqual({ [sha(1)]: 'success', [sha(2)]: 'failure', [sha(3)]: 'pending', [sha(4)]: 'none', [sha(5)]: 'none' })
  })

  it('sha 는 쿼리 문자열이 아니라 변수로 보낸다', async () => {
    const { fetch, calls } = fakeFetch(graphqlResponder({}))
    await fetchCommitChecks(new GitHubClient(fetch, () => TOKEN), slug, [sha(7)], new CheckStateCache())
    const body = JSON.parse(calls[0]!.body!) as { query: string; variables: Record<string, string> }
    expect(body.query).not.toContain(sha(7))
    expect(body.variables).toMatchObject({ owner: 'octo', name: 'hello', o0: sha(7) })
  })

  it(`${CHECKS_BATCH}개씩 나눠 묻고, 잘못된 sha·중복은 버린다`, async () => {
    const { fetch, calls } = fakeFetch(graphqlResponder({}))
    const shas = Array.from({ length: CHECKS_BATCH + 5 }, (_, i) => sha(i + 1))
    const out = await fetchCommitChecks(new GitHubClient(fetch, () => TOKEN), slug, [...shas, sha(1), 'xyz', '; drop'], new CheckStateCache())
    expect(calls).toHaveLength(2)
    expect(Object.keys(out)).toHaveLength(CHECKS_BATCH + 5)
  })

  it('캐시된 커밋은 다시 묻지 않고, 진행 중은 TTL 이 지나면 다시 묻는다', async () => {
    let now = 5_000_000
    const rollups: Record<string, string> = { [sha(1)]: 'SUCCESS', [sha(2)]: 'PENDING' }
    const { fetch, calls } = fakeFetch(graphqlResponder(rollups))
    const client = new GitHubClient(fetch, () => TOKEN)
    const cache = new CheckStateCache(() => now)
    await fetchCommitChecks(client, slug, [sha(1), sha(2)], cache)
    expect(calls).toHaveLength(1)
    await fetchCommitChecks(client, slug, [sha(1), sha(2)], cache)
    expect(calls).toHaveLength(1)
    // 진행 중이던 커밋이 끝났다
    rollups[sha(2)] = 'SUCCESS'
    now += TTL_PENDING_MS + 1
    const out = await fetchCommitChecks(client, slug, [sha(1), sha(2)], cache)
    expect(calls).toHaveLength(2)
    expect(JSON.parse(calls[1]!.body!).variables).toMatchObject({ o0: sha(2) })
    expect(JSON.parse(calls[1]!.body!).variables).not.toHaveProperty('o1')
    expect(out).toEqual({ [sha(1)]: 'success', [sha(2)]: 'success' })
  })

  it('레이트 리밋(GraphQL RATE_LIMITED·403)은 GH_RATE_LIMIT 로 던진다', async () => {
    const limited = fakeFetch(() => json({ errors: [{ type: 'RATE_LIMITED', message: 'x' }] }))
    await expect(fetchCommitChecks(new GitHubClient(limited.fetch, () => TOKEN), slug, [sha(1)], new CheckStateCache())).rejects.toMatchObject({ code: 'GH_RATE_LIMIT' })
    const forbidden = fakeFetch(() => json({ message: 'rate' }, { status: 403, headers: { 'x-ratelimit-remaining': '0', 'x-ratelimit-limit': '5000', 'x-ratelimit-reset': '1900000000' } }))
    await expect(fetchCommitChecks(new GitHubClient(forbidden.fetch, () => TOKEN), slug, [sha(1)], new CheckStateCache())).rejects.toMatchObject({ code: 'GH_RATE_LIMIT' })
  })

  it('토큰이 없으면 요청하지 않고 GH_AUTH 로 거절한다', async () => {
    const { fetch, calls } = fakeFetch(() => json({}))
    await expect(fetchCommitChecks(new GitHubClient(fetch, () => null), slug, [sha(1)], new CheckStateCache())).rejects.toBeInstanceOf(GitHubApiError)
    expect(calls).toHaveLength(0)
  })

  it('저장소를 못 보면(repository: null) 결과에서 빠지고 캐시하지 않는다', async () => {
    const { fetch } = fakeFetch(() => json({ data: { repository: null } }))
    const cache = new CheckStateCache()
    expect(await fetchCommitChecks(new GitHubClient(fetch, () => TOKEN), slug, [sha(1)], cache)).toEqual({})
    expect(cache.get(`octo/hello@${sha(1)}`)).toBeUndefined()
  })
})

describe('커밋 상세·Actions 목록 변환', () => {
  it('check-run 과 legacy status 를 합치고 소요 시간·결론·링크를 채운다', async () => {
    const { fetch, calls } = fakeFetch((req) => {
      if (req.url.includes('/check-runs')) {
        return json({
          check_runs: [
            { name: 'build', status: 'completed', conclusion: 'success', html_url: 'https://github.com/octo/hello/runs/1', started_at: '2026-01-01T00:00:00Z', completed_at: '2026-01-01T00:01:05Z' },
            { name: 'lint', status: 'in_progress', conclusion: null, started_at: '2026-01-01T00:00:00Z' },
            { name: 'e2e', status: 'completed', conclusion: 'failure' }
          ]
        })
      }
      return json({ statuses: [{ context: 'ci/legacy', state: 'success', target_url: 'https://ci.example/1' }] })
    })
    const out = await fetchCheckDetails(new GitHubClient(fetch, () => TOKEN), slug, sha(9).toUpperCase())
    expect(calls.map((c) => c.url).sort()).toEqual([
      `https://api.github.com/repos/octo/hello/commits/${sha(9)}/check-runs?per_page=100`,
      `https://api.github.com/repos/octo/hello/commits/${sha(9)}/status?per_page=100`
    ])
    expect(out).toEqual([
      { name: 'build', state: 'success', conclusion: 'success', durationSec: 65, url: 'https://github.com/octo/hello/runs/1' },
      { name: 'lint', state: 'pending' },
      { name: 'e2e', state: 'failure', conclusion: 'failure' },
      { name: 'ci/legacy', state: 'success', url: 'https://ci.example/1' }
    ])
    await expect(fetchCheckDetails(new GitHubClient(fetch, () => TOKEN), slug, '../x')).rejects.toThrow('커밋 해시')
  })

  it('워크플로 실행·잡 목록을 변환한다(비로그인도 요청 가능)', async () => {
    const { fetch, calls } = fakeFetch((req) => {
      if (req.url.includes('/actions/runs/77/jobs')) {
        return json({ jobs: [{ id: 1, name: 'test', status: 'completed', conclusion: 'cancelled', started_at: '2026-01-01T00:00:00Z', completed_at: '2026-01-01T01:02:03Z', html_url: 'https://github.com/o/r/job/1' }, { id: 2, name: 'deploy', status: 'queued', conclusion: null }] })
      }
      return json(
        { workflow_runs: [{ id: 77, name: 'CI', display_title: 'fix: x', head_branch: 'main', head_sha: sha(1), event: 'push', status: 'completed', conclusion: 'success', run_number: 12, created_at: '2026-01-01T00:00:00Z', updated_at: '2026-01-01T00:05:00Z', html_url: 'https://github.com/o/r/actions/runs/77' }] },
        { headers: { link: '<https://api.github.com/x?page=2>; rel="next"' } }
      )
    })
    const client = new GitHubClient(fetch, () => null)
    const runs = await listRuns(client, slug, 1)
    expect(calls[0]!.headers['authorization']).toBeUndefined()
    expect(runs.hasMore).toBe(true)
    expect(runs.items[0]).toMatchObject({ id: 77, name: 'CI', title: 'fix: x', branch: 'main', event: 'push', state: 'success', number: 12, conclusion: 'success' })
    const jobs = await listJobs(client, slug, 77)
    expect(jobs).toEqual([
      { id: 1, name: 'test', state: 'failure', status: 'completed', conclusion: 'cancelled', durationSec: 3723, htmlUrl: 'https://github.com/o/r/job/1' },
      { id: 2, name: 'deploy', state: 'pending', status: 'queued', htmlUrl: '' }
    ])
  })
})

describe('IPC 인자 검증', () => {
  it('sha·목록·페이지를 검증한다', () => {
    expect(parseSha(sha(3).toUpperCase())).toBe(sha(3))
    expect(() => parseSha('abc')).toThrow()
    expect(() => parseSha(5)).toThrow()
    expect(parseCheckShas([sha(1), sha(1), sha(2)])).toEqual([sha(1), sha(2)])
    expect(() => parseCheckShas(Array.from({ length: 101 }, (_, i) => sha(i + 1)))).toThrow()
    expect(() => parseCheckShas('x')).toThrow()
    expect(parsePage(2)).toBe(2)
    expect(() => parsePage(0)).toThrow()
    expect(() => parsePage(1.5)).toThrow()
  })

  it('실행·잡 ID 는 10자리 이상의 실제 크기도 받고, 0·음수·소수·안전하지 않은 정수는 거부한다', () => {
    expect(parseRunId(12345678901)).toBe(12345678901)
    expect(parseRunId(Number.MAX_SAFE_INTEGER)).toBe(Number.MAX_SAFE_INTEGER)
    for (const bad of [0, -1, 1.5, Number.MAX_SAFE_INTEGER + 1, '12', null, NaN]) expect(() => parseRunId(bad)).toThrow()
  })
})

describe('GitHubService 체크 조회', () => {
  let dir: string
  beforeEach(() => {
    dir = mkdtempSync(join(tmpdir(), 'antler-gh-checks-'))
  })
  afterEach(() => rmSync(dir, { recursive: true, force: true }))

  const repos = { ghRepo: async () => ({ owner: 'octo', repo: 'hello', remote: 'origin' }) }
  const config = { get: () => ({ ui: {} }) }
  // 암호화를 못 쓰는 환경 — 토큰은 메모리에만 둔다
  const memSafe = { isEncryptionAvailable: () => false, encryptString: () => Buffer.alloc(0), decryptString: () => '' }

  it('로그인하지 않았으면 요청 없이 available=false', async () => {
    const { fetch, calls } = fakeFetch(() => json({}))
    const svc = new GitHubService({ config, repos, store: new TokenStore(dir, memSafe), fetchImpl: fetch })
    expect(await svc.commitChecks('r1', [sha(1)])).toEqual({ available: false, states: {} })
    expect(calls).toHaveLength(0)
  })

  it('같은 커밋을 다시 물으면 캐시를 쓰고, 결과·에러 어디에도 토큰이 없다', async () => {
    const store = new TokenStore(dir, memSafe)
    store.save(TOKEN, 'pat')
    const { fetch, calls } = fakeFetch(graphqlResponder({ [sha(1)]: 'SUCCESS' }))
    const svc = new GitHubService({ config, repos, store, fetchImpl: fetch })
    const a = await svc.commitChecks('r1', [sha(1)])
    const b = await svc.commitChecks('r1', [sha(1)])
    expect(a).toEqual({ available: true, states: { [sha(1)]: 'success' } })
    expect(b).toEqual(a)
    expect(calls).toHaveLength(1)
    expect(calls[0]!.headers['authorization']).toBe(`Bearer ${TOKEN}`)
    expect(JSON.stringify([a, b])).not.toContain(TOKEN)
    expect(calls[0]!.body).not.toContain(TOKEN)

    // 로그아웃하면 캐시도 비워져 다시 묻는 대신 available=false
    svc.logout()
    expect((await svc.commitChecks('r1', [sha(1)])).available).toBe(false)
  })

  it('레이트 리밋 에러 메시지에 토큰이 실리지 않는다', async () => {
    const store = new TokenStore(dir, memSafe)
    store.save(TOKEN, 'pat')
    const { fetch } = fakeFetch(() => json({ message: `limit for ${'x'}` }, { status: 403, headers: { 'x-ratelimit-remaining': '0', 'x-ratelimit-limit': '5000', 'x-ratelimit-reset': '1900000000' } }))
    const svc = new GitHubService({ config, repos, store, fetchImpl: fetch })
    const err = await svc.commitChecks('r1', [sha(1)]).catch((e: Error) => e)
    expect(err).toBeInstanceOf(GitHubApiError)
    expect((err as Error).message).not.toContain(TOKEN)
  })
})
