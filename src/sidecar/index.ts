// Tauri 용 개발 전용 Node 사이드카. Rust 가 stdin 으로 넘기는 채널 호출을 처리한다. 프로토콜은 protocol.ts.
// Rust 가 설정을 소유하는 실행(env ANTLER_RUST_CONFIG=1 — Phase 6b PR 9 부터 기본)에서는 UpdateService 만 조립하고 `update:*` 7채널만 받는다(PR 10).
// 되돌리기(ANTLER_RUST_GIT=0)에서는 src/main 의 서비스 조립(legacy.ts)이 모든 채널을 처리한다 — 6c 에서 사이드카와 함께 사라진다.
import { createInterface } from 'node:readline'
import { inspect } from 'node:util'
import { join } from 'node:path'
import { UpdateService } from '../main/update/UpdateService'
import { bindHost, invokeHandler, resolveHost } from './electronShim'
import { rustOwnsConfig, sidecarAccepts } from './configOwns'
import { assembleLegacy } from './legacy'
import { registerUpdateIpc } from './updateIpc'
import { UNDEFINED_ARG, type FromSidecar, type ToSidecar } from './protocol'
import { IPC_CHANNELS } from '@shared/ipc'

// stdout 은 프로토콜 전용이다 — 서비스가 console.log/info 를 써도 프로토콜 줄이 깨지지 않게 stderr 로 돌린다
console.log = console.error
console.info = console.error
// Node 의 console.debug·dir 도 stdout 으로 쓴다 — 프로토콜 줄({t:'repos'} 등)을 위조할 수 있는 길이 되지 않게 함께 돌린다
console.debug = console.error
console.dir = (item, options) => void process.stderr.write(`${inspect(item, options)}\n`)

const userData = process.env['ANTLER_USER_DATA']
if (!userData) {
  console.error('ANTLER_USER_DATA 가 필요합니다')
  process.exit(1)
}

// 짝 없는 서로게이트(.slice 가 이모지 가운데를 자른 문자열 등)는 JSON.stringify 가 "\ud83d" 로 내보내는데,
// Rust serde_json 은 이를 거부해 그 응답이 버려지고 호출이 영원히 끝나지 않는다. U+FFFD 로 바꿔 보낸다
const LONE_SURROGATE = /[\uD800-\uDBFF](?![\uDC00-\uDFFF])|(?<![\uD800-\uDBFF])[\uDC00-\uDFFF]/g
const wellFormed = (_key: string, v: unknown): unknown => (typeof v === 'string' ? v.replace(LONE_SURROGATE, '�') : v)

const out = (msg: FromSidecar): void => {
  process.stdout.write(`${JSON.stringify(msg, wellFormed)}\n`)
}
bindHost(out)
const emit = (name: string, payload: unknown): void => out({ t: 'evt', name, payload })

// Rust 가 설정 파일을 소유한다(env ANTLER_RUST_CONFIG=1 — Rust 는 라우터가 켜져 있을 때만 준다). **한 실행에서 설정 파일을 쓰는 쪽은 하나다**:
// 소유하면 이 사이드카는 설정·저장소·fetch·감시·GitHub·Jira 서비스를 만들지 않고 파일도 열지 않으며, update:* 밖의 채널은 거부한다.
// 값이 없으면(ANTLER_RUST_GIT=0) legacy.ts 의 조립이 지금처럼 설정의 유일한 작성자다
const rustConfig = rustOwnsConfig(process.env['ANTLER_RUST_CONFIG'])

// 업데이트는 비활성(isPackaged=false, ANTLER_UPDATE_DEV 없음)
const updates = new UpdateService({
  currentVersion: '0.0.0-tauri-poc',
  platform: process.platform,
  arch: process.arch,
  env: {},
  isPackaged: false,
  getSkipped: () => undefined,
  setSkipped: () => {},
  send: (s) => emit('update:state', s),
  downloadsDir: () => join(userData, 'updates'),
  openPath: () => Promise.resolve('사이드카(PoC): openPath 미지원'),
  showItem: () => {}
})
const legacy = rustConfig ? null : assembleLegacy(userData, out, emit, updates)
if (rustConfig) registerUpdateIpc(updates)
updates.start()

// preload 의 허용 목록과 같은 검사. renderer 의 bridge 검사는 __TAURI__ 로 우회할 수 있으므로 여기서 다시 막는다
const allowedChannels = new Set<string>(IPC_CHANNELS)

/** bridge 의 encodeArgs 를 되돌린다 — 표식 객체(키가 정확히 그것 하나)만 undefined 로 바꾼다 */
function decodeArgs(args: unknown[]): unknown[] {
  return args.map((a) =>
    a !== null && typeof a === 'object' && !Array.isArray(a) && Object.keys(a).length === 1 && (a as Record<string, unknown>)[UNDEFINED_ARG] === true
      ? undefined
      : a
  )
}

async function onRequest(id: number, ch: string, rawArgs: unknown[]): Promise<void> {
  const args = decodeArgs(rawArgs)
  const accepted = allowedChannels.has(ch) && sidecarAccepts(ch, rustConfig)
  const pending = accepted ? invokeHandler(ch, args) : undefined
  // Rust 라우터가 이미 막는다(update:* 밖은 오지 않는다) — 와도(방어) 처리하지 않는다
  const env = pending
    ? await pending
    : { ok: false, error: allowedChannels.has(ch) ? `Rust 가 처리하는 채널입니다: ${ch}` : `알 수 없는 채널: ${ch}`, code: 'ERROR' }
  out({ t: 'res', id, env: env as Extract<FromSidecar, { t: 'res' }>['env'] })
}

function onLine(line: string): void {
  let msg: ToSidecar
  try {
    msg = JSON.parse(line) as ToSidecar
  } catch {
    console.error('JSON 이 아닌 입력 줄 무시')
    return
  }
  if (msg.t === 'req') {
    void onRequest(msg.id, msg.ch, Array.isArray(msg.args) ? msg.args : []).catch((err) =>
      out({ t: 'res', id: msg.id, env: { ok: false, error: err instanceof Error ? err.message : String(err), code: 'ERROR' } })
    )
  } else if (msg.t === 'focus') {
    // src/main/index.ts 의 win.on('focus'|'blur') 와 같다. Rust 가 설정을 소유하면 오지 않는다(받을 서비스가 없다)
    legacy?.setFocused(msg.focused)
  } else if (msg.t === 'hostres') {
    resolveHost(msg.id, msg.ok, msg.value)
  }
}

function shutdown(): void {
  updates.dispose()
  void (legacy ? legacy.dispose() : Promise.resolve()).finally(() => process.exit(0))
}

const rl = createInterface({ input: process.stdin })
rl.on('line', onLine)
// stdin 이 닫히면(Rust 종료) 정리하고 끝낸다
rl.on('close', shutdown)
out({ t: 'ready' })
