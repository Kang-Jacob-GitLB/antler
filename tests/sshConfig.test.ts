import { mkdirSync, mkdtempSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { describe, expect, it } from 'vitest'
import { listHosts, parseHostSpec, parseSshConfig, resolveHost } from '../src/main/executors/ssh/config'

const HOME = join(tmpdir(), 'fake-home')
const parse = (t: string) => parseSshConfig(t, { homeDir: HOME })
const opt = { localUser: 'me' }

describe('ssh config 파서', () => {
  it('Host 복수 패턴·와일드카드·부정', () => {
    const c = parse(`
Host web-* !web-old
  User deploy
Host a b
  Port 2200
`)
    expect(resolveHost(c, 'web-1', opt).user).toBe('deploy')
    expect(resolveHost(c, 'web-old', opt).user).toBe('me')
    expect(resolveHost(c, 'b', opt).port).toBe(2200)
    expect(resolveHost(c, 'a', opt).port).toBe(2200)
    expect(resolveHost(c, 'c', opt).port).toBe(22)
  })

  it('? 와일드카드와 대소문자 무시', () => {
    const c = parse('HOST node?\n  USER=u1\n')
    expect(resolveHost(c, 'node7', opt).user).toBe('u1')
    expect(resolveHost(c, 'node77', opt).user).toBe('me')
  })

  it('먼저 얻은 값이 이긴다(블록 순서 기준)', () => {
    const c = parse(`
Host dev
  User first
  HostName dev.example.com
Host *
  User second
  Port 2222
  HostName ignored
`)
    const r = resolveHost(c, 'dev', opt)
    expect(r.user).toBe('first')
    expect(r.hostName).toBe('dev.example.com')
    expect(r.port).toBe(2222)
  })

  it('전역(Host 이전) 옵션도 적용된다', () => {
    const c = parse('User g\nHost x\n  Port 1\n')
    expect(resolveHost(c, 'x', opt).user).toBe('g')
  })

  it('IdentityFile 은 일치하는 모든 블록에서 누적되고 ~ 와 토큰이 확장된다', () => {
    const c = parse(`
Host srv
  HostName 10.0.0.5
  User bob
  IdentityFile ~/.ssh/a
  IdentityFile "%d/keys/%h-%r-%u-%p"
Host *
  IdentityFile ~/.ssh/z
  IdentitiesOnly yes
`)
    const r = resolveHost(c, 'srv', opt)
    expect(r.identityFiles).toEqual([
      join(HOME, '.ssh', 'a'),
      join(HOME, 'keys', '10.0.0.5-bob-me-22'),
      join(HOME, '.ssh', 'z')
    ])
    expect(r.identitiesOnly).toBe(true)
  })

  it('HostName 의 %h 토큰', () => {
    const c = parse('Host app\n  HostName %h.internal.example.com\n')
    expect(resolveHost(c, 'app', opt).hostName).toBe('app.internal.example.com')
  })

  it('ProxyJump 체인과 user@host:port 형태, none', () => {
    const c = parse(`
Host target
  ProxyJump j1, ops@j2:2201,[::1]:22
Host direct
  ProxyJump none
Host *
  ProxyJump ignored
`)
    const r = resolveHost(c, 'target', opt)
    expect(r.proxyJump.map((h) => [h.user, h.host, h.port])).toEqual([
      [undefined, 'j1', undefined],
      ['ops', 'j2', 2201],
      [undefined, '::1', 22]
    ])
    expect(resolveHost(c, 'direct', opt).proxyJump).toEqual([])
  })

  it('IdentityAgent none / SSH_AUTH_SOCK / 경로', () => {
    const c = parse(`
Host a
  IdentityAgent none
Host b
  IdentityAgent SSH_AUTH_SOCK
Host c
  IdentityAgent ~/.agent.sock
`)
    expect(resolveHost(c, 'a', opt).identityAgent).toBeNull()
    expect(resolveHost(c, 'b', opt).identityAgent).toBeUndefined()
    expect(resolveHost(c, 'c', opt).identityAgent).toBe(join(HOME, '.agent.sock'))
  })

  it('따옴표 값·주석·행 끝 주석', () => {
    const c = parse(`
# 주석
Host "my host"   # 끝 주석
  IdentityFile "C:\\Users\\me\\my key"
  Port 2022 # 포트
`)
    const r = resolveHost(c, 'my host', opt)
    expect(r.port).toBe(2022)
    expect(r.identityFiles).toEqual(['C:\\Users\\me\\my key'])
  })

  it('Match 블록은 적용하지 않고 Match all 만 전역으로 취급한다', () => {
    const c = parse(`
Match host foo
  User matched
Host foo
  Port 1
Match all
  User all
`)
    const r = resolveHost(c, 'foo', opt)
    expect(r.user).toBe('all')
    expect(r.port).toBe(1)
  })

  it('user@host:port 직접 입력은 설정의 user/port 보다 우선하되 host 설정은 조회한다', () => {
    const c = parse('Host box\n  HostName 1.2.3.4\n  User cfg\n  Port 9\n  IdentityFile ~/k\n')
    const r = resolveHost(c, 'root@box:2200', opt)
    expect(r).toMatchObject({ hostName: '1.2.3.4', user: 'root', port: 2200 })
    expect(r.identityFiles).toHaveLength(1)
    expect(resolveHost(c, 'ann@raw.example.com', opt)).toMatchObject({ hostName: 'raw.example.com', user: 'ann', port: 22 })
    expect(parseHostSpec('[fe80::1]:2222')).toEqual({ user: undefined, host: 'fe80::1', port: 2222 })
    expect(parseHostSpec('fe80::1').host).toBe('fe80::1')
  })

  it('Include: ~/.ssh 기준 상대 경로와 글롭, 포함 파일의 Host 는 거기서 끝난다', () => {
    const home = mkdtempSync(join(tmpdir(), 'antler-inc-'))
    mkdirSync(join(home, '.ssh', 'conf.d'), { recursive: true })
    writeFileSync(join(home, '.ssh', 'conf.d', '10-a.conf'), 'Host inc-a\n  Port 1111\nHost inc-b\n  Port 2222\n')
    writeFileSync(join(home, '.ssh', 'extra'), 'Host inc-c\n  Port 3333\n')
    const c = parseSshConfig('Include conf.d/*.conf extra\nHost after\n  Port 4444\nHost *\n  User tail\n', {
      homeDir: home
    })
    expect(resolveHost(c, 'inc-a', opt).port).toBe(1111)
    expect(resolveHost(c, 'inc-b', opt).port).toBe(2222)
    expect(resolveHost(c, 'inc-c', opt).port).toBe(3333)
    expect(resolveHost(c, 'after', opt).port).toBe(4444)
    expect(resolveHost(c, 'inc-a', opt).user).toBe('tail')
    // 없는 Include 는 조용히 무시
    expect(() => parseSshConfig('Include nope/*\n', { homeDir: home })).not.toThrow()
  })

  it('listHosts: 와일드카드·부정 패턴 제외, 별칭 순서·중복 제거', () => {
    const c = parse(`
Host * !x
  User all
Host alpha beta
  HostName 10.1.1.1
  Port 2200
  ProxyJump bastion
Host alpha
  User dup
Host bastion
  HostName b.example.com
`)
    const list = listHosts(c, opt)
    expect(list.map((h) => h.alias)).toEqual(['alpha', 'beta', 'bastion'])
    expect(list[0]).toEqual({ alias: 'alpha', hostName: '10.1.1.1', user: 'all', port: 2200, proxyJump: 'bastion' })
    expect(list[2]!.proxyJump).toBeUndefined()
  })
})

describe('지원하지 않는 설정은 조용히 무시하지 않는다', () => {
  const r = (t: string, h: string) => resolveHost(parse(t), h, { localUser: 'me' })

  it('ProxyCommand 가 걸린 호스트는 unsupported', () => {
    expect(r('Host inner\n  ProxyCommand ssh -W %h:%p bastion\n', 'inner').unsupported).toMatch(/ProxyCommand/)
    expect(r('Host inner\n  ProxyCommand none\n', 'inner').unsupported).toBeUndefined()
  })

  it('이 호스트에 걸릴 수 있는 Match 블록만 unsupported', () => {
    const cfg = 'Match host *.corp exec "true"\n  IdentitiesOnly yes\nMatch user root\n  User admin\n'
    // *.corp 가 아닌 호스트: host 조건 블록은 무관, user 조건 블록은 판단할 수 없어 unsupported
    expect(r(cfg, 'box.home').unsupported).toMatch(/Match user root/)
    expect(r(cfg, 'box.home').unsupported).not.toMatch(/corp/)
    expect(r(cfg, 'db.corp').unsupported).toMatch(/\*\.corp/)
    // 다른 호스트만 겨냥한 Match 는 영향 없음
    expect(r('Match host other\n  User x\n', 'mine').unsupported).toBeUndefined()
    // 옵션 없는 Match 블록은 무관
    expect(r('Match exec "true"\n', 'mine').unsupported).toBeUndefined()
  })
})
