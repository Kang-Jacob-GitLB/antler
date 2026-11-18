import { mkdtempSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { describe, expect, it } from 'vitest'
import { ConfigStore } from '../src/main/services/ConfigStore'
import { MirrorSender, mirrorOf } from '../src/sidecar/mirror'
import type { FromSidecar } from '../src/sidecar/protocol'

const tmp = (): string => join(mkdtempSync(join(tmpdir(), 'antler-mirror-')), 'antler.config.json')

describe('사이드카 저장소 미러', () => {
  it('저장소마다 id·kind·path·name(+wsl 의 distro, ssh 의 sshHost)만 담고 그룹 같은 것은 담지 않는다', () => {
    const c = new ConfigStore(tmp())
    const local = c.addRepo({ sourceId: 'local', path: 'C:\\work\\a', name: 'a', group: 'g' })
    const wsl = c.addRepo({ sourceId: 'wsl:Ubuntu', path: '/home/me/b', name: 'b' }, { id: 'wsl:Ubuntu', kind: 'wsl', label: 'WSL: Ubuntu', distro: 'Ubuntu' })
    const ssh = c.addRepo({ sourceId: 'ssh:h', path: '/srv/c', name: 'c' }, { id: 'ssh:h', kind: 'ssh', label: 'SSH: h', sshHost: 'h' })
    expect(mirrorOf(c.get())).toEqual([
      { id: local.id, kind: 'local', path: 'C:\\work\\a', name: 'a' },
      { id: wsl.id, kind: 'wsl', path: '/home/me/b', name: 'b', distro: 'Ubuntu' },
      { id: ssh.id, kind: 'ssh', path: '/srv/c', name: 'c', sshHost: 'h' }
    ])
  })

  it('ssh 호스트가 바뀌면 미러도 바뀌고 다른 종류의 소스에는 sshHost 가 실리지 않는다', () => {
    const sent: Extract<FromSidecar, { t: 'repos' }>[] = []
    const c = new ConfigStore(tmp())
    const m = new MirrorSender((msg) => sent.push(msg))
    const r = c.addRepo({ sourceId: 'ssh:a', path: '/srv/x', name: 'x' }, { id: 'ssh:a', kind: 'ssh', label: 'SSH: a', sshHost: 'a' })
    m.push(c.get())
    expect(sent.at(-1)?.repos).toEqual([{ id: r.id, kind: 'ssh', path: '/srv/x', name: 'x', sshHost: 'a' }])
    // 소스를 못 찾은 저장소는 뺀다(호스트도 없다)
    const cfg = c.get()
    cfg.sources = cfg.sources.filter((s) => s.id !== 'ssh:a')
    expect(mirrorOf(cfg)).toEqual([])
  })

  it('github 소스의 저장소는 owner/repo 링크(기본 브랜치 없이)를 담고 다른 소스에는 실리지 않는다', () => {
    const c = new ConfigStore(tmp())
    const link = { owner: 'octocat', repo: 'hello', defaultBranch: 'main' }
    const g = c.addRepo({ sourceId: 'github', path: 'C:\\cache\\octocat\\hello.git', name: 'hello', github: link }, { id: 'github', kind: 'github', label: 'GitHub' })
    const local = c.addRepo({ sourceId: 'local', path: 'C:\\work\\a', name: 'a' })
    expect(mirrorOf(c.get())).toEqual([
      { id: g.id, kind: 'github', path: 'C:\\cache\\octocat\\hello.git', name: 'hello', github: { owner: 'octocat', repo: 'hello' } },
      { id: local.id, kind: 'local', path: 'C:\\work\\a', name: 'a' }
    ])
    // 기본 브랜치가 바뀌어도 미러는 바뀌지 않는다(Rust 는 브랜치를 쓰지 않는다)
    const sent: Extract<FromSidecar, { t: 'repos' }>[] = []
    const m = new MirrorSender((msg) => sent.push(msg))
    m.push(c.get())
    c.setGithubDefaultBranch(g.id, 'develop')
    m.push(c.get())
    expect(sent).toHaveLength(1)
  })

  it('저장소 목록이 바뀔 때만 보내고 gen 은 1 부터 오른다', () => {
    const sent: Extract<FromSidecar, { t: 'repos' }>[] = []
    const c = new ConfigStore(tmp())
    const m = new MirrorSender((msg) => sent.push(msg))
    c.onChange = (cfg) => m.push(cfg)
    m.push(c.get())
    expect(sent).toEqual([{ t: 'repos', gen: 1, repos: [] }])
    // 저장소와 무관한 쓰기(UI 설정)는 보내지 않는다
    c.setUi({ diffMode: 'split' })
    expect(sent).toHaveLength(1)
    const r = c.addRepo({ sourceId: 'local', path: '/tmp/x', name: 'x' })
    expect(sent.at(-1)).toEqual({ t: 'repos', gen: 2, repos: [{ id: r.id, kind: 'local', path: '/tmp/x', name: 'x' }] })
    // 이름이 바뀌면 미러도 바뀐다(Rust 의 확인 창 제목이 낡지 않게)
    c.updateRepo(r.id, { name: 'y' })
    expect(sent.at(-1)).toEqual({ t: 'repos', gen: 3, repos: [{ id: r.id, kind: 'local', path: '/tmp/x', name: 'y' }] })
    // 그룹처럼 미러에 없는 값만 바뀌면 보내지 않는다
    c.updateRepo(r.id, { group: 'g2' })
    expect(sent).toHaveLength(3)
    c.removeRepo(r.id)
    expect(sent.at(-1)).toEqual({ t: 'repos', gen: 4, repos: [] })
  })
})
