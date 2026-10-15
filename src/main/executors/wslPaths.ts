// WSL UNC 경로(\\wsl.localhost\<distro>\... / \\wsl$\<distro>\...) 와 리눅스 경로 사이의 변환.

const UNC_RE = /^[\\/]{2}(?:wsl\.localhost|wsl\$)(?:[\\/]+([^\\/]+))?(.*)$/i

/** `\\wsl.localhost\X\...` 또는 `\\wsl$\X\...` 형태인지(슬래시 방향 무관) */
export function isWslUnc(p: string): boolean {
  return wslFromUnc(p) !== null
}

/** WSL UNC 경로를 { distro, 리눅스 경로 } 로 분해한다. 배포판 이름이 없으면 null. 루트는 '/'. */
export function wslFromUnc(p: string): { distro: string; path: string } | null {
  const m = UNC_RE.exec(p.trim())
  if (!m || !m[1]) return null
  const segments = (m[2] ?? '').split(/[\\/]+/).filter((s) => s.length > 0)
  return { distro: m[1], path: '/' + segments.join('/') }
}

/** 리눅스 경로를 `\\wsl.localhost\<distro>\...` 로 만든다. */
export function uncFromWsl(distro: string, linuxPath: string): string {
  const segments = linuxPath.split('/').filter((s) => s.length > 0)
  return ['\\\\wsl.localhost', distro, ...segments].join('\\')
}
