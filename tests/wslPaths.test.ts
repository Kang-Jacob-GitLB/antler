import { describe, expect, it } from 'vitest'
import { parseWslList, WslExecutor } from '../src/main/executors/WslExecutor'
import { isWslUnc, uncFromWsl, wslFromUnc } from '../src/main/executors/wslPaths'

describe('parseWslList', () => {
  const utf16 = (s: string, bom = true): Buffer =>
    Buffer.concat([bom ? Buffer.from([0xff, 0xfe]) : Buffer.alloc(0), Buffer.from(s, 'utf16le')])

  it('BOM/CRLF 가 섞인 UTF-16LE 를 디코드한다', () => {
    expect(parseWslList(utf16('Ubuntu-24.04\r\nDebian\r\n'))).toEqual(['Ubuntu-24.04', 'Debian'])
  })

  it('BOM 이 없어도 동작한다', () => {
    expect(parseWslList(utf16('Ubuntu\r\n', false))).toEqual(['Ubuntu'])
  })

  it('docker-desktop 계열과 빈 줄을 버린다', () => {
    const buf = utf16('docker-desktop\r\n\r\nUbuntu\r\ndocker-desktop-data\r\n  \r\n')
    expect(parseWslList(buf)).toEqual(['Ubuntu'])
  })

  it('빈 출력은 빈 배열', () => {
    expect(parseWslList(Buffer.alloc(0))).toEqual([])
  })

  it('공백이 든 이름을 보존한다', () => {
    expect(parseWslList(utf16('My Distro\r\n'))).toEqual(['My Distro'])
  })

  it('WSL_UTF8=1 일 때의 UTF-8 출력도 받아들인다', () => {
    expect(parseWslList(Buffer.from('Ubuntu-24.04\r\nDebian\r\n', 'utf8'))).toEqual(['Ubuntu-24.04', 'Debian'])
  })
})

describe.runIf(process.platform === 'win32')('WslExecutor 실패 메시지', () => {
  it('없는 배포판이면 wsl.exe 의 안내를 읽을 수 있는 오류로 돌려준다', async () => {
    const ex = new WslExecutor('antler-no-such-distro')
    const r = await ex.run('.', ['--version'])
    expect(r.code).not.toBe(0)
    expect(r.stderr).toMatch(/^WSL/)
    expect(r.stderr).not.toMatch(/\0/)
    await expect(async () => {
      for await (const _ of ex.stream('.', ['--version'])) void _
    }).rejects.toThrow(/^WSL/)
  }, 30_000)
})

describe('wslFromUnc', () => {
  it.each([
    ['\\\\wsl.localhost\\Ubuntu-24.04\\home\\me\\repo', 'Ubuntu-24.04', '/home/me/repo'],
    ['\\\\wsl$\\Ubuntu\\home\\me\\repo', 'Ubuntu', '/home/me/repo'],
    ['//wsl.localhost/Ubuntu/home/me/repo', 'Ubuntu', '/home/me/repo'],
    ['//wsl$/Ubuntu/home/me/repo', 'Ubuntu', '/home/me/repo'],
    ['\\\\WSL.LOCALHOST\\Ubuntu\\home\\me\\repo\\', 'Ubuntu', '/home/me/repo'],
    ['\\\\wsl.localhost\\Ubuntu\\home\\me\\repo\\\\', 'Ubuntu', '/home/me/repo'],
    ['\\\\wsl.localhost\\Ubuntu\\home\\me\\내 저장소', 'Ubuntu', '/home/me/내 저장소'],
    ['\\\\wsl.localhost\\Ubuntu', 'Ubuntu', '/'],
    ['\\\\wsl.localhost\\Ubuntu\\', 'Ubuntu', '/']
  ])('%s', (input, distro, path) => {
    expect(wslFromUnc(input)).toEqual({ distro, path })
  })

  it.each(['C:\\Users\\me', '/home/me/repo', '\\\\server\\share\\x', '\\\\wsl.localhost', '\\\\wsl.localhost\\', '\\\\wslx\\a\\b'])(
    'WSL UNC 가 아니면 null: %s',
    (input) => {
      expect(wslFromUnc(input)).toBeNull()
      expect(isWslUnc(input)).toBe(false)
    }
  )

  it('isWslUnc', () => {
    expect(isWslUnc('\\\\wsl$\\Ubuntu\\home')).toBe(true)
  })
})

describe('uncFromWsl', () => {
  it('리눅스 경로를 UNC 로 만든다', () => {
    expect(uncFromWsl('Ubuntu', '/home/me/repo')).toBe('\\\\wsl.localhost\\Ubuntu\\home\\me\\repo')
    expect(uncFromWsl('Ubuntu', '/')).toBe('\\\\wsl.localhost\\Ubuntu')
  })

  it('왕복 변환', () => {
    const p = '/home/me/내 저장소'
    expect(wslFromUnc(uncFromWsl('Ubuntu-24.04', p))).toEqual({ distro: 'Ubuntu-24.04', path: p })
  })
})
