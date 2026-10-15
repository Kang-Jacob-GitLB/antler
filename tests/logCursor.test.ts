import { execFileSync } from 'node:child_process'
import { mkdtempSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import { afterAll, beforeAll, describe, expect, it } from 'vitest'
import { LocalExecutor } from '../src/main/executors/LocalExecutor'
import { logArgs } from '../src/main/git/commands'
import { LogCursor } from '../src/main/git/logCursor'
import { parseLog } from '../src/main/git/parsers/log'

const COMMITS = 1200
let root: string
let repo: string
let empty: string
const exec = new LocalExecutor()

beforeAll(() => {
  root = mkdtempSync(join(tmpdir(), 'antler-cursor-'))
  repo = join(root, '큰 저장소')
  empty = join(root, 'empty')
  execFileSync('node', [resolve(__dirname, '../scripts/make-big-repo.mjs'), repo, String(COMMITS)], { stdio: 'ignore' })
  execFileSync('git', ['init', '-q', empty])
}, 60_000)

afterAll(() => rmSync(root, { recursive: true, force: true }))

const fullLog = async (dir: string) => parseLog((await exec.run(dir, logArgs(undefined, 0))).stdout)

describe('LogCursor', () => {
  it('페이지를 이어 붙이면 전체 log 와 같다', async () => {
    const all = await fullLog(repo)
    expect(all).toHaveLength(COMMITS)
    const cursor = new LogCursor(exec, repo, (s) => logArgs(undefined, s))
    const got = []
    let hasMore = true
    while (hasMore) {
      const page = await cursor.read(250)
      got.push(...page.commits)
      hasMore = page.hasMore
    }
    cursor.dispose()
    expect(got.map((c) => c.hash)).toEqual(all.map((c) => c.hash))
    expect(cursor.position).toBe(COMMITS)
  })

  it('skip 위치에서 시작한 커서는 같은 구간을 돌려준다', async () => {
    const all = await fullLog(repo)
    const cursor = new LogCursor(exec, repo, (s) => logArgs(undefined, s), 700)
    const page = await cursor.read(100)
    cursor.dispose()
    expect(page.commits.map((c) => c.hash)).toEqual(all.slice(700, 800).map((c) => c.hash))
    expect(page.hasMore).toBe(true)
  })

  it('동시에 read 해도 순서대로 겹치지 않게 나눠 준다', async () => {
    const cursor = new LogCursor(exec, repo, (s) => logArgs(undefined, s))
    const [a, b] = await Promise.all([cursor.read(100), cursor.read(100)])
    cursor.dispose()
    const hashes = new Set([...a.commits, ...b.commits].map((c) => c.hash))
    expect(hashes.size).toBe(200)
  })

  it('커밋 없는 저장소는 빈 페이지', async () => {
    const cursor = new LogCursor(exec, empty, (s) => logArgs(undefined, s))
    expect(await cursor.read(100)).toEqual({ commits: [], hasMore: false })
    cursor.dispose()
  })

  it('dispose 후 read 는 실패한다', async () => {
    const cursor = new LogCursor(exec, repo, (s) => logArgs(undefined, s))
    cursor.dispose()
    await expect(cursor.read(10)).rejects.toThrow()
  })
})

describe('logArgs', () => {
  it('ref 는 옵션으로 해석되지 않게 --end-of-options 뒤에 둔다', () => {
    const args = logArgs('--output=/tmp/x', 0)
    expect(args.indexOf('--end-of-options')).toBeLessThan(args.indexOf('--output=/tmp/x'))
  })
})
