import { EventEmitter } from 'node:events'
import { describe, expect, it } from 'vitest'
import { ExternalOpener, type OpenerDeps } from '../src/main/external/opener'
import { buildRevealPlan, clsidComponent, type Target } from '../src/main/external/plan'

const GUID = '{0AFACED1-E828-11D1-9187-B532F1E9575D}'
const root = 'C:\\work\\repo'
const t: Target = { kind: 'local', root }

describe('clsidComponent', () => {
  it('저장소 루트부터 대상까지 .{GUID} 로 끝나는 첫 구성 요소의 경로', () => {
    expect(clsidComponent(root, `${root}\\src\\lib`)).toBeNull()
    expect(clsidComponent(root, `${root}\\fs.${GUID}`)).toBe(`${root}\\fs.${GUID}`)
    expect(clsidComponent(root, `${root}\\a\\fs.${GUID}\\b\\c`)).toBe(`${root}\\a\\fs.${GUID}`)
    expect(clsidComponent(`C:\\work\\x.${GUID.toLowerCase()}`, `C:\\work\\x.${GUID.toLowerCase()}\\src`)).toBe(`C:\\work\\x.${GUID.toLowerCase()}`)
    // GUID 가 이름 끝이 아니거나 '.' 없이 붙은 이름은 대상이 아니다
    expect(clsidComponent(root, `${root}\\fs.${GUID}.txt`)).toBeNull()
    expect(clsidComponent(root, `${root}\\fs${GUID}`)).toBeNull()
    // 루트 위쪽 이름은 보지 않는다
    expect(clsidComponent(root, root)).toBeNull()
    expect(clsidComponent(`C:\\a.${GUID}\\repo`, `C:\\a.${GUID}\\repo\\src`)).toBeNull()
  })
})

describe('buildRevealPlan(Windows): 이름 확장 폴더', () => {
  it('폴더 바로 가기로 해석되는 이름은 열지 않고 그 폴더를 부모에서 보여 준다', () => {
    const bad = `${root}\\fs.${GUID}`
    expect(buildRevealPlan(t, { path: `${bad}\\x`, isDir: true, root }, 'win32')).toEqual({ type: 'showItem', path: bad })
    expect(buildRevealPlan(t, { path: bad, isDir: true, root }, 'win32')).toEqual({ type: 'showItem', path: bad })
    expect(buildRevealPlan(t, { path: `${root}\\src`, isDir: true, root }, 'win32')).toEqual({ type: 'openPath', path: `${root}\\src` })
  })
})

function fake(dirs: string[]) {
  const set = new Set(dirs)
  const spawned: { file: string; args: string[]; cwd?: string }[] = []
  const shown: { type: string; path: string }[] = []
  const stat = (p: string) => {
    if (!set.has(p)) throw new Error('ENOENT')
    return { isFile: () => false, isDirectory: () => true }
  }
  const deps: OpenerDeps = {
    platform: 'win32',
    env: {},
    fs: { realpathSync: (p) => (stat(p), p), statSync: stat, lstatSync: stat },
    spawn: (file, args, opts) => {
      spawned.push({ file, args, ...(opts.cwd ? { cwd: opts.cwd } : {}) })
      const ee = new EventEmitter() as EventEmitter & { unref(): void }
      ee.unref = () => {}
      setImmediate(() => ee.emit('spawn'))
      return ee as never
    },
    shell: {
      openPath: async (p) => {
        shown.push({ type: 'openPath', path: p })
        return ''
      },
      showItemInFolder: (p) => shown.push({ type: 'showItem', path: p })
    }
  }
  return { deps, spawned, shown }
}

describe('ExternalOpener(Windows): name.{CLSID} 폴더', () => {
  const bad = `${root}\\fs.${GUID}`
  const dirs = [root, `${root}\\src`, bad, `${bad}\\sub`]

  it('파일 관리자: openPath 대신 showItem', async () => {
    const f = fake(dirs)
    const o = new ExternalOpener(f.deps)
    await o.open(t, 'fileManager', 'src', {})
    await o.open(t, 'fileManager', `fs.${GUID}/sub`, {})
    expect(f.shown).toEqual([
      { type: 'openPath', path: `${root}\\src` },
      { type: 'showItem', path: bad }
    ])
  })

  it('터미널: 조건을 만족하는 가장 가까운 상위(저장소 루트)에서 연다', async () => {
    const f = fake(dirs)
    await new ExternalOpener(f.deps).open(t, 'terminal', `fs.${GUID}/sub`, {})
    expect(f.spawned).toHaveLength(1)
    expect(f.spawned[0]?.cwd ?? f.spawned[0]?.args.at(-1)).toBe(root)
  })

  it('저장소 루트 자체가 이름 확장 폴더이면 터미널을 거부한다', async () => {
    const r = `C:\\work\\x.${GUID}`
    const f = fake([r, `${r}\\src`])
    await expect(new ExternalOpener(f.deps).open({ kind: 'local', root: r }, 'terminal', 'src', {})).rejects.toThrow(/GUID/)
    expect(f.spawned).toEqual([])
  })
})
