import { execFile } from 'node:child_process'
import { afterAll, beforeAll, describe, expect, it } from 'vitest'
import { AbortError, GitError } from '../src/main/executors/GitExecutor'
import { listWslDistros, WslExecutor, wslArgv } from '../src/main/executors/WslExecutor'

describe('wslArgv', () => {
  it('--exec env ... git -C 형태로 만든다', () => {
    const argv = wslArgv('Ubuntu', '/home/me/내 repo', ['log', '-z'])
    expect(argv.slice(0, 6)).toEqual(['-d', 'Ubuntu', '--cd', '~', '--exec', 'env'])
    expect(argv).toContain('LC_ALL=C.UTF-8')
    expect(argv).toContain('GIT_LITERAL_PATHSPECS=1')
    const gitAt = argv.indexOf('git')
    expect(argv.slice(gitAt, gitAt + 3)).toEqual(['git', '-C', '/home/me/내 repo'])
    expect(argv.slice(-2)).toEqual(['log', '-z'])
  })
})

/** 셸 스크립트를 배포판 안에서 실행한다(테스트 셋업/검사용). 인자는 --exec 라 그대로 전달된다. */
function sh(distro: string, script: string): Promise<{ code: number; out: string }> {
  return new Promise((resolve) => {
    execFile(
      'wsl.exe',
      ['-d', distro, '--exec', 'sh', '-c', script],
      { encoding: 'buffer', windowsHide: true, maxBuffer: 64 * 1024 * 1024 },
      (err, stdout) => {
        const code = err ? ((err as { code?: number }).code ?? -1) : 0
        resolve({ code: typeof code === 'number' ? code : -1, out: stdout.toString('utf8').trim() })
      }
    )
  })
}

async function consume(it: AsyncIterable<Buffer>): Promise<Buffer[]> {
  const chunks: Buffer[] = []
  for await (const c of it) chunks.push(c)
  return chunks
}

let distro = ''
let skipReason = '' // 배포판 조회가 비동기라 beforeAll 에서 채운다

describe.skipIf(process.platform !== 'win32')('WslExecutor (integration)', () => {
  let ready = false
  let dir = ''
  const exec = (): WslExecutor => new WslExecutor(distro)

  beforeAll(async () => {
    const list = await listWslDistros()
    if (list.length === 0) {
      skipReason = '사용 가능한 WSL 배포판이 없음'
      return
    }
    distro = list[0]!
    const has = await sh(distro, 'command -v git')
    if (has.code !== 0) {
      skipReason = `${distro} 에 git 이 설치돼 있지 않음`
      return
    }
    const script = [
      'set -e',
      'd=$(mktemp -d /tmp/antler-wsl-test.XXXXXX)',
      'cd "$d"',
      'git init -q -b main .',
      'echo a > "a b.txt"',
      'git add .',
      'git -c user.name=t -c user.email=t@t commit -q -m "첫 커밋: 한글 메시지"',
      'echo "$d"'
    ].join('\n')
    const r = await sh(distro, script)
    if (r.code !== 0 || !r.out.startsWith('/tmp/antler-wsl-test.')) {
      skipReason = `테스트 저장소 생성 실패: ${r.out}`
      return
    }
    dir = r.out
    ready = true
  }, 60_000)

  afterAll(async () => {
    if (dir.startsWith('/tmp/antler-wsl-test.')) await sh(distro, `rm -rf '${dir}'`)
  })

  const t = (name: string, fn: () => Promise<void>, timeout = 30_000): void => {
    it(name, async (ctx) => {
      if (!ready) ctx.skip(skipReason)
      await fn()
    }, timeout)
  }

  t('rev-parse --show-toplevel 이 리눅스 경로를 돌려준다', async () => {
    const r = await exec().run(dir, ['rev-parse', '--show-toplevel'])
    expect(r.code).toBe(0)
    expect(r.stdout.toString().trim()).toBe(dir)
  })

  t('홈 기준 상대 경로도 절대 리눅스 경로로 풀린다', async () => {
    const name = `antler-wsl-rel.${Date.now()}`
    const mk = await sh(distro, `mkdir -p "$HOME/${name}" && git init -q "$HOME/${name}" && cd "$HOME" && pwd`)
    try {
      expect(mk.code).toBe(0)
      const r = await exec().run(name, ['rev-parse', '--show-toplevel'])
      expect(r.code).toBe(0)
      expect(r.stdout.toString().trim()).toBe(`${mk.out}/${name}`)
    } finally {
      await sh(distro, `rm -rf "$HOME/${name}"`)
    }
  })

  t('GIT_ENV 가 리눅스 쪽에 전달된다(LC_ALL)', async () => {
    // alias 로 셸을 빌려 환경변수를 확인한다(--exec 에는 셸이 없다)
    const r = await exec().run(dir, ['-c', 'alias.e=!printf %s "$LC_ALL:$GIT_LITERAL_PATHSPECS"', 'e'])
    expect(r.stdout.toString()).toBe('C.UTF-8:1')
  })

  t('log -z 의 한글이 깨지지 않는다', async () => {
    const r = await exec().run(dir, ['log', '-z', '--format=%s'])
    expect(r.stdout.toString('utf8').split('\0')[0]).toBe('첫 커밋: 한글 메시지')
  })

  t('공백이 든 파일명을 그대로 돌려준다', async () => {
    const r = await exec().run(dir, ['ls-files', '-z'])
    expect(r.stdout.toString('utf8').split('\0')[0]).toBe('a b.txt')
  })

  t('stream 이 청크를 돌려준다', async () => {
    const chunks = await consume(exec().stream(dir, ['log', '--format=%H']))
    expect(Buffer.concat(chunks).toString().trim()).toMatch(/^[0-9a-f]{40}$/)
  })

  t('없는 ref 는 run 은 비정상 코드, stream 은 GitError', async () => {
    const r = await exec().run(dir, ['rev-parse', '--verify', 'refs/heads/__없는__'])
    expect(r.code).not.toBe(0)
    await expect(consume(exec().stream(dir, ['log', 'refs/heads/__없는__']))).rejects.toBeInstanceOf(GitError)
  })

  t('이미 취소된 signal 이면 AbortError', async () => {
    const ac = new AbortController()
    ac.abort()
    await expect(exec().run(dir, ['status'], { signal: ac.signal })).rejects.toBeInstanceOf(AbortError)
  })

  t('호출당 오버헤드 측정(정보용)', async () => {
    const times: number[] = []
    for (let i = 0; i < 10; i++) {
      const s = performance.now()
      await exec().run(dir, ['rev-parse', 'HEAD'])
      times.push(performance.now() - s)
    }
    times.sort((a, b) => a - b)
    console.log(`wsl.exe rev-parse HEAD median=${times[5]!.toFixed(0)}ms min=${times[0]!.toFixed(0)}ms max=${times[9]!.toFixed(0)}ms`)
  })

  t('취소하면 리눅스 git 프로세스도 종료된다', async () => {
    // 출력 없이 오래 블록하는 명령: git 셸 alias 가 sleep 을 실행한다(SIGPIPE 로 우연히 죽는 경우를 배제).
    // (전송 프로토콜은 GIT_ALLOW_PROTOCOL=none 으로 막혀 있어 ext:: 원격은 쓸 수 없다)
    const id = String(100000 + Math.floor(Math.random() * 899999))
    const marker = `sleep ${id}`
    // pgrep -f 가 검사용 sh -c 자신의 명령줄에 걸리지 않도록 첫 글자를 대괄호로 감싼다
    const pat = `sleep [${id[0]}]${id.slice(1)}`
    const ac = new AbortController()
    const p = exec().run(dir, ['-c', `alias.antlersleep=!${marker}`, 'antlersleep'], { signal: ac.signal })
    p.catch(() => {})
    // 리눅스 쪽에서 sleep 이 실제로 뜰 때까지 기다린다
    let alive = false
    for (let i = 0; i < 40 && !alive; i++) {
      await new Promise((r) => setTimeout(r, 250))
      alive = (await sh(distro, `pgrep -f '${pat}' >/dev/null && echo y`)).out === 'y'
    }
    expect(alive).toBe(true)
    ac.abort()
    await expect(p).rejects.toBeInstanceOf(AbortError)
    let gone = false
    for (let i = 0; i < 20 && !gone; i++) {
      await new Promise((r) => setTimeout(r, 250))
      gone = (await sh(distro, `pgrep -f '${pat}' >/dev/null || echo n`)).out === 'n'
    }
    // 못 죽으면 정리하고 실패시킨다
    if (!gone) await sh(distro, `pkill -f '${pat}'`)
    expect(gone).toBe(true)
  }, 60_000)
})
