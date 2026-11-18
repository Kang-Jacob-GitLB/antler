// Rust 가 창을 만들 때 심는 OS 이름(src-tauri/src/platform.rs 의 init_script)을 읽는다. Node `process.platform` 과 같은 값이다.
export type NodePlatform = 'win32' | 'darwin' | 'linux'

const KNOWN: readonly string[] = ['win32', 'darwin', 'linux']

/** 주입된 값이 없거나 모르는 값이면 userAgent 로 추정한다(개발 서버를 일반 브라우저로 열었을 때) */
export function resolvePlatform(injected: unknown, userAgent: string): NodePlatform {
  if (typeof injected === 'string' && KNOWN.includes(injected)) return injected as NodePlatform
  if (/Windows/i.test(userAgent)) return 'win32'
  if (/Mac/i.test(userAgent)) return 'darwin'
  return 'linux'
}
