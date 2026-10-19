// 개발·화면 검증용 가짜 GitHub. ANTLER_GH_FAKE=<fixture.json> 이고 패키징되지 않은 빌드에서만 index.ts 가 켠다.
// 실제 네트워크는 쓰지 않고 fixture 에 적힌 응답만 돌려준다. fixture 는 읽을 때마다 다시 읽어 실행 중에도 바꿀 수 있다.
import { readFileSync } from 'node:fs'

interface Fixture {
  /** GraphQL statusCheckRollup.state (sha → 상태, null 이면 체크 없음). 없는 sha 는 GitHub 가 모르는 커밋(object 가 null) */
  rollup?: Record<string, string | null>
  /** sha → check-runs 응답의 check_runs */
  checkRuns?: Record<string, unknown[]>
  /** sha → legacy status 응답의 statuses */
  statuses?: Record<string, unknown[]>
  /** actions/runs 응답의 workflow_runs */
  runs?: unknown[]
  /** runId → jobs */
  jobs?: Record<string, unknown[]>
  /** true 면 GraphQL 이 한도 초과로 실패한다 */
  rateLimited?: boolean
}

const j = (body: unknown, status = 200, headers: Record<string, string> = {}): Response =>
  new Response(JSON.stringify(body), { status, headers: { 'content-type': 'application/json', ...headers } })

export function createDevFakeFetch(file: string): typeof fetch {
  const load = (): Fixture => JSON.parse(readFileSync(file, 'utf8')) as Fixture
  const impl = async (input: string | URL | Request, init?: RequestInit): Promise<Response> => {
    const fx = load()
    const url = new URL(String(input))
    const path = url.pathname
    if (url.origin !== 'https://api.github.com') return j({ message: 'Not Found' }, 404)
    if (path === '/user') return j({ login: 'fake-user', avatar_url: '' })
    if (path === '/graphql' && init?.method === 'POST') {
      if (fx.rateLimited) return j({ errors: [{ type: 'RATE_LIMITED', message: 'rate limited' }] })
      const body = JSON.parse(String(init.body)) as { variables?: Record<string, string> }
      const vars = body.variables ?? {}
      const repository: Record<string, unknown> = {}
      for (let i = 0; `o${i}` in vars; i++) {
        const sha = vars[`o${i}`]!
        const rollup = fx.rollup ?? {}
        if (!(sha in rollup)) repository[`c${i}`] = null
        else repository[`c${i}`] = { statusCheckRollup: rollup[sha] ? { state: rollup[sha] } : null }
      }
      return j({ data: { repository } })
    }
    const m = /^\/repos\/[^/]+\/[^/]+\/(.*)$/.exec(path)
    const rest = m?.[1] ?? ''
    let mm: RegExpExecArray | null
    if ((mm = /^commits\/([0-9a-f]+)\/check-runs$/.exec(rest))) return j({ check_runs: fx.checkRuns?.[mm[1]!] ?? [] })
    if ((mm = /^commits\/([0-9a-f]+)\/status$/.exec(rest))) return j({ statuses: fx.statuses?.[mm[1]!] ?? [] })
    if (rest === 'actions/runs') return j({ workflow_runs: fx.runs ?? [] })
    if ((mm = /^actions\/runs\/(\d+)\/jobs$/.exec(rest))) return j({ jobs: fx.jobs?.[mm[1]!] ?? [] })
    return j({ message: 'Not Found' }, 404)
  }
  return impl as typeof fetch
}
