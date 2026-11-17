// Rust 가 보낸 `ghToken` 메시지(GitHub 토큰의 메모리 사본)를 받는 곳. 프로토콜은 protocol.ts.
//
// Rust 가 GitHub 를 처리할 때(env ANTLER_RUST_API=1)만 쓴다 — 사이드카에 남는 원격 전용 저장소의 fetch 인증(`RepoService` 의 githubToken)과 오류 문구의 토큰 가림이 읽는다.
// 토큰은 이 객체 안에만 있다: 로그·오류·env 에 싣지 않고, 직렬화·검사(콘솔 출력·util.inspect·JSON)에도 드러나지 않는다.
import { inspect } from 'node:util'

/** 실제 토큰은 수백 바이트다. 이보다 길면 받지 않는다(Rust 도 같은 상한으로 보낸다 — src-tauri/src/repoq.rs 의 MAX_TOKEN_BYTES) */
export const MAX_GH_TOKEN_LENGTH = 4096

/** 공백·제어 문자·비 ASCII 가 없는 토큰만. 헤더·명령 인자에 그대로 들어가므로 모양이 틀린 값은 버린다 */
const TOKEN_SHAPE = /^[\x21-\x7e]+$/

export type GhTokenResult = 'set' | 'cleared' | 'unchanged' | 'rejected'

export class GhTokenMirror {
  #token: string | null = null

  get(): string | null {
    return this.#token
  }

  /** 메시지의 token 값(문자열 또는 null)을 적용한다. 모양이 틀리면 **기존 값을 건드리지 않는다** */
  apply(value: unknown): GhTokenResult {
    if (value === null) {
      if (this.#token === null) return 'unchanged'
      this.#token = null
      return 'cleared'
    }
    if (typeof value !== 'string' || value.length === 0 || value.length > MAX_GH_TOKEN_LENGTH || !TOKEN_SHAPE.test(value)) return 'rejected'
    if (value === this.#token) return 'unchanged'
    this.#token = value
    return 'set'
  }

  toJSON(): string {
    return '[GhTokenMirror]'
  }

  [inspect.custom](): string {
    return '[GhTokenMirror]'
  }
}

/**
 * `ghToken` 메시지 처리. 바뀌었으면(`set`·`cleared`) `onChanged` 를 부른다(원격 전용 저장소의 인증 실패 정지를 푼다 — TS 의 `onAuthChanged`).
 * 거부한 값은 어디에도 적지 않는다(로그에는 고정 문구만)
 */
export function handleGhToken(mirror: GhTokenMirror, msg: { token?: unknown }, onChanged: () => void): GhTokenResult {
  const result = mirror.apply(msg.token)
  if (result === 'rejected') console.error('ghToken 메시지를 적용하지 않았다(모양이 틀렸다)')
  else if (result !== 'unchanged') onChanged()
  return result
}
