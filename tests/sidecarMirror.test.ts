import { mkdtempSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { describe, expect, it } from 'vitest'
import { ConfigStore } from '../src/main/services/ConfigStore'
import { MirrorSender, mirrorOf } from '../src/sidecar/mirror'
import type { FromSidecar } from '../src/sidecar/protocol'

const tmp = (): string => join(mkdtempSync(join(tmpdir(), 'antler-mirror-')), 'antler.config.json')

describe('사이드카 저장소 미러', () => {
  it('저장소마다 id·kind·path(+wsl 의 distro)만 담고 이름·그룹 같은 것은 담지 않는다', () => {
    const c = new ConfigStore(tmp())
    const local = c.addRepo({ sourceId: 'local', path: 'C:\\work\\a', name: 'a', group: 'g' })
    const wsl = c.addRepo({ sourceId: 'wsl:Ubuntu', path: '/home/me/b', name: 'b' }, { id: 'wsl:Ubuntu', kind: 'wsl', label: 'WSL: Ubuntu', distro: 'Ubuntu' })
    const ssh = c.addRepo({ sourceId: 'ssh:h', path: '/srv/c', name: 'c' }, { id: 'ssh:h', kind: 'ssh', label: 'SSH: h', sshHost: 'h' })
    expect(mirrorOf(c.get())).toEqual([
      { id: local.id, kind: 'local', path: 'C:\\work\\a' },
      { id: wsl.id, kind: 'wsl', path: '/home/me/b', distro: 'Ubuntu' },
      { id: ssh.id, kind: 'ssh', path: '/srv/c' }
    ])
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
    expect(sent.at(-1)).toEqual({ t: 'repos', gen: 2, repos: [{ id: r.id, kind: 'local', path: '/tmp/x' }] })
    // 이름만 바뀌어도 미러(id·kind·path)는 같다
    c.updateRepo(r.id, { name: 'y' })
    expect(sent).toHaveLength(2)
    c.removeRepo(r.id)
    expect(sent.at(-1)).toEqual({ t: 'repos', gen: 3, repos: [] })
  })
})
