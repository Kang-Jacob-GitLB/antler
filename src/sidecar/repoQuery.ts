// Rust 가 보낸 `repoq` 질의(GitHub 서비스가 저장소 정보를 묻는다)를 RepoService·설정으로 답한다. 프로토콜은 protocol.ts.
//
// 이 메시지는 Rust 만 만든다(renderer 의 값은 `req` 의 args 안에만 들어간다). 그래도 op 허용 목록·인자 모양·동시 질의 수를 다시 검사한다 —
// 알 수 없는 op 나 틀린 인자는 오류 봉투로 답하고(id 가 있을 때) 아무 일도 하지 않는다. 응답은 `res` 줄(`{t:'res',id,env}`)이다.
import type { GhRepoRef } from '@shared/types'
import type { IpcEnvelope } from '@shared/ipc'
import { REPOQ_OPS, type RepoqOp } from './protocol'

/** 처리에 필요한 것만 — 테스트가 RepoService 전체를 만들지 않아도 된다 */
export interface RepoQueryTarget {
  ghRepo(repoId: string): Promise<GhRepoRef | null>
  ghRemotes(repoId: string, signal?: AbortSignal): Promise<GhRepoRef[]>
  pushedShasByAuthor(repoId: string, email: string, limit: number, signal?: AbortSignal): Promise<string[]>
  invalidateGhRemotes(repoId?: string): void
}

export interface RepoQueryConfig {
  get(): { ui: { githubClientId?: string | undefined }; repos?: { id: string }[] }
}

/** 동시에 진행하는 질의의 상한(Launchpad 가 저장소마다 한 번씩 묻고 서비스가 8개로 제한한다) */
const MAX_INFLIGHT = 64
const MAX_ID_LENGTH = 200
const MAX_EMAIL_LENGTH = 320
const MAX_LIMIT = 100

type Reply = (id: number, env: IpcEnvelope<unknown>) => void

const fail = (error: string): IpcEnvelope<unknown> => ({ ok: false, error, code: 'ERROR' })

function repoIdOf(p: Record<string, unknown>): string {
  const id = p['repoId']
  if (typeof id !== 'string' || id.length === 0 || id.length > MAX_ID_LENGTH) throw new Error('잘못된 저장소 ID 입니다')
  return id
}

export class RepoQueryHandler {
  private readonly inflight = new Map<number, AbortController>()

  constructor(
    private readonly repos: RepoQueryTarget,
    private readonly config: RepoQueryConfig,
    private readonly reply: Reply
  ) {}

  /** 진행 중인 질의 수(테스트용) */
  get pending(): number {
    return this.inflight.size
  }

  /** 질의 하나를 시작한다. `id` 가 없으면 응답하지 않는 알림이다. 처리를 시작했으면 true */
  handle(msg: { id?: unknown; op?: unknown; p?: unknown }): boolean {
    const id = typeof msg.id === 'number' && Number.isSafeInteger(msg.id) && msg.id >= 0 ? msg.id : undefined
    const respond = (env: IpcEnvelope<unknown>): void => {
      if (id !== undefined) this.reply(id, env)
    }
    const op = msg.op
    if (typeof op !== 'string' || !(REPOQ_OPS as readonly string[]).includes(op)) {
      respond(fail('알 수 없는 질의입니다'))
      return false
    }
    const params = msg.p !== null && typeof msg.p === 'object' && !Array.isArray(msg.p) ? (msg.p as Record<string, unknown>) : {}
    if (id !== undefined && (this.inflight.has(id) || this.inflight.size >= MAX_INFLIGHT)) {
      respond(fail('진행 중인 질의가 너무 많습니다'))
      return false
    }
    const ac = new AbortController()
    if (id !== undefined) this.inflight.set(id, ac)
    void this.run(op as RepoqOp, params, ac.signal).then(
      (data) => {
        if (id !== undefined) this.inflight.delete(id)
        respond({ ok: true, data })
      },
      (err: unknown) => {
        if (id !== undefined) this.inflight.delete(id)
        respond(fail(err instanceof Error ? err.message : String(err)))
      }
    )
    return true
  }

  /** `repoqCancel`: 그 질의의 AbortSignal 을 끊는다(없는 id 는 무시). 응답은 취소 오류로 나가지만 Rust 는 이미 대기를 버렸다 */
  cancel(id: unknown): void {
    if (typeof id === 'number') this.inflight.get(id)?.abort()
  }

  private async run(op: RepoqOp, p: Record<string, unknown>, signal: AbortSignal): Promise<unknown> {
    switch (op) {
      case 'ghRepo':
        return this.repos.ghRepo(repoIdOf(p))
      case 'ghRemotes':
        return this.repos.ghRemotes(repoIdOf(p), signal)
      case 'pushedShas': {
        const email = p['email']
        const limit = p['limit']
        if (typeof email !== 'string' || email.length === 0 || email.length > MAX_EMAIL_LENGTH) throw new Error('잘못된 이메일입니다')
        if (typeof limit !== 'number' || !Number.isInteger(limit) || limit < 1 || limit > MAX_LIMIT) throw new Error('잘못된 개수입니다')
        return this.repos.pushedShasByAuthor(repoIdOf(p), email, limit, signal)
      }
      case 'invalidateGhRemotes':
        this.repos.invalidateGhRemotes()
        return null
      case 'githubClientId':
        // TS GitHubService.clientId() 와 같이 앞뒤 공백을 뗀다. 비었으면 없는 것이다
        return this.config.get().ui.githubClientId?.trim() || null
      case 'repoIds':
        return (this.config.get().repos ?? []).map((r) => r.id)
    }
  }
}
