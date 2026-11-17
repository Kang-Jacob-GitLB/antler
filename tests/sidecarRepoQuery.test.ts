import { describe, expect, it, vi } from 'vitest'
import type { GhRepoRef } from '../src/shared/types'
import { REPOQ_OPS } from '../src/sidecar/protocol'
import { RepoQueryHandler, type RepoQueryTarget } from '../src/sidecar/repoQuery'

const REF: GhRepoRef = { owner: 'octo', repo: 'hello', remote: 'origin' }

function setup(over: Partial<RepoQueryTarget> = {}, config = { ui: { githubClientId: ' Iv1.abc ' as string | undefined }, repos: [{ id: 'r1' }, { id: 'r2' }] }) {
  const replies: { id: number; env: unknown }[] = []
  const repos: RepoQueryTarget = {
    ghRepo: vi.fn(() => Promise.resolve(REF)),
    ghRemotes: vi.fn(() => Promise.resolve([REF])),
    pushedShasByAuthor: vi.fn(() => Promise.resolve(['a'.repeat(40)])),
    invalidateGhRemotes: vi.fn(),
    ...over
  }
  const handler = new RepoQueryHandler(repos, { get: () => config }, (id, env) => replies.push({ id, env }))
  const settle = async (): Promise<void> => {
    for (let i = 0; i < 5; i++) await Promise.resolve()
  }
  return { repos, handler, replies, settle }
}

describe('사이드카 repoq(Rust 의 GitHub 서비스가 저장소 정보를 묻는다)', () => {
  it('ghRepo·ghRemotes·pushedShas 는 RepoService 에 묻고 res 봉투로 답한다', async () => {
    const { repos, handler, replies, settle } = setup()
    expect(handler.handle({ id: 1, op: 'ghRepo', p: { repoId: 'r1' } })).toBe(true)
    expect(handler.handle({ id: 2, op: 'ghRemotes', p: { repoId: 'r1' } })).toBe(true)
    expect(handler.handle({ id: 3, op: 'pushedShas', p: { repoId: 'r1', email: 'me@x.io', limit: 3 } })).toBe(true)
    await settle()
    expect(replies).toEqual([
      { id: 1, env: { ok: true, data: REF } },
      { id: 2, env: { ok: true, data: [REF] } },
      { id: 3, env: { ok: true, data: ['a'.repeat(40)] } }
    ])
    expect(repos.ghRepo).toHaveBeenCalledWith('r1')
    expect(repos.pushedShasByAuthor).toHaveBeenCalledWith('r1', 'me@x.io', 3, expect.any(AbortSignal))
    expect(handler.pending).toBe(0)
  })

  it('githubClientId 는 앞뒤 공백을 떼고 없으면 null, repoIds 는 설정의 저장소 ID', async () => {
    const a = setup()
    a.handler.handle({ id: 1, op: 'githubClientId', p: {} })
    a.handler.handle({ id: 2, op: 'repoIds', p: {} })
    await a.settle()
    expect(a.replies).toEqual([
      { id: 1, env: { ok: true, data: 'Iv1.abc' } },
      { id: 2, env: { ok: true, data: ['r1', 'r2'] } }
    ])
    const b = setup({}, { ui: { githubClientId: '  ' }, repos: undefined as never })
    b.handler.handle({ id: 1, op: 'githubClientId', p: {} })
    b.handler.handle({ id: 2, op: 'repoIds', p: {} })
    await b.settle()
    expect(b.replies).toEqual([
      { id: 1, env: { ok: true, data: null } },
      { id: 2, env: { ok: true, data: [] } }
    ])
  })

  it('invalidateGhRemotes 는 id 가 없으면 알림이라 답하지 않는다', async () => {
    const { repos, handler, replies, settle } = setup()
    handler.handle({ op: 'invalidateGhRemotes', p: {} })
    await settle()
    expect(repos.invalidateGhRemotes).toHaveBeenCalledTimes(1)
    expect(replies).toEqual([])
  })

  it('알 수 없는 op·틀린 인자는 아무 일도 하지 않고 오류로 답한다', async () => {
    const { repos, handler, replies, settle } = setup()
    expect(handler.handle({ id: 1, op: 'readFile', p: { repoId: 'r1' } })).toBe(false)
    expect(handler.handle({ id: 2, op: 5, p: {} })).toBe(false)
    expect(handler.handle({ id: 3, op: 'constructor', p: {} })).toBe(false)
    handler.handle({ id: 4, op: 'ghRepo', p: {} })
    handler.handle({ id: 5, op: 'ghRepo', p: { repoId: 5 } })
    handler.handle({ id: 6, op: 'ghRepo', p: { repoId: 'x'.repeat(201) } })
    handler.handle({ id: 7, op: 'pushedShas', p: { repoId: 'r1', email: 'a@b', limit: 0 } })
    handler.handle({ id: 8, op: 'pushedShas', p: { repoId: 'r1', email: 'a@b', limit: 101 } })
    handler.handle({ id: 9, op: 'pushedShas', p: { repoId: 'r1', email: '', limit: 3 } })
    handler.handle({ id: 10, op: 'ghRepo', p: null })
    await settle()
    expect(replies).toHaveLength(10)
    for (const r of replies) expect(r.env, String(r.id)).toMatchObject({ ok: false, code: 'ERROR' })
    expect(repos.ghRepo).not.toHaveBeenCalled()
    expect(repos.pushedShasByAuthor).not.toHaveBeenCalled()
    // 알림(id 없음)은 틀려도 답하지 않는다
    expect(handler.handle({ op: 'readFile' })).toBe(false)
    expect(replies).toHaveLength(10)
  })

  it('id 가 숫자가 아니면 답하지 않는다', async () => {
    const { handler, replies, settle } = setup()
    handler.handle({ id: '1', op: 'ghRepo', p: { repoId: 'r1' } })
    handler.handle({ id: -1, op: 'ghRepo', p: { repoId: 'r1' } })
    handler.handle({ id: 1.5, op: 'ghRepo', p: { repoId: 'r1' } })
    await settle()
    expect(replies).toEqual([])
  })

  it('RepoService 가 던지면 메시지만 오류 봉투로 답한다', async () => {
    const { handler, replies, settle } = setup({ ghRepo: () => Promise.reject(new Error('알 수 없는 저장소')) })
    handler.handle({ id: 1, op: 'ghRepo', p: { repoId: 'zz' } })
    await settle()
    expect(replies).toEqual([{ id: 1, env: { ok: false, error: '알 수 없는 저장소', code: 'ERROR' } }])
  })

  it('repoqCancel 은 그 질의의 신호를 끊는다(없는 id·숫자가 아닌 id 는 무시)', async () => {
    let signal: AbortSignal | undefined
    let release: (v: GhRepoRef[]) => void = () => undefined
    const { handler, replies, settle } = setup({
      ghRemotes: (_id, s) => {
        signal = s
        return new Promise((resolve) => {
          release = resolve
        })
      }
    })
    handler.handle({ id: 7, op: 'ghRemotes', p: { repoId: 'r1' } })
    expect(handler.pending).toBe(1)
    handler.cancel(8)
    handler.cancel('7')
    expect(signal?.aborted).toBe(false)
    handler.cancel(7)
    expect(signal?.aborted).toBe(true)
    release([])
    await settle()
    expect(handler.pending).toBe(0)
    expect(replies).toHaveLength(1)
  })

  it('동시 질의에 상한이 있고 같은 id 는 거부한다', async () => {
    const { handler, replies, settle } = setup({ ghRemotes: () => new Promise(() => undefined) })
    for (let i = 0; i < 64; i++) expect(handler.handle({ id: i, op: 'ghRemotes', p: { repoId: 'r1' } })).toBe(true)
    expect(handler.handle({ id: 100, op: 'ghRemotes', p: { repoId: 'r1' } })).toBe(false)
    expect(handler.handle({ id: 3, op: 'ghRemotes', p: { repoId: 'r1' } })).toBe(false)
    await settle()
    expect(replies.map((r) => r.id)).toEqual([100, 3])
  })

  it('허용 목록은 Rust 의 repoq::Op 과 같은 6개다', () => {
    expect([...REPOQ_OPS]).toEqual(['ghRepo', 'ghRemotes', 'pushedShas', 'invalidateGhRemotes', 'githubClientId', 'repoIds'])
  })
})
