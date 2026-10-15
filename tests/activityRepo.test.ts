import { execFileSync } from 'node:child_process'
import { mkdtempSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import { afterAll, beforeAll, describe, expect, it } from 'vitest'
import { LocalExecutor } from '../src/main/executors/LocalExecutor'
import { commitActivity } from '../src/main/git/activity'
import { logArgs } from '../src/main/git/commands'
import { parseLog } from '../src/main/git/parsers/log'

describe('commitActivity (실제 저장소)', () => {
  let root: string
  let repo: string
  let empty: string
  const exec = new LocalExecutor()
  beforeAll(() => {
    root = mkdtempSync(join(tmpdir(), 'antler-activity-'))
    repo = join(root, 'repo')
    empty = join(root, 'empty')
    execFileSync('node', [resolve(__dirname, '../scripts/make-big-repo.mjs'), repo, '800'], { stdio: 'ignore' })
    execFileSync('git', ['init', '-q', empty])
  }, 60_000)
  afterAll(() => rmSync(root, { recursive: true, force: true }))

  it('구간 합과 범위가 목록(authorDate)과 일치한다', async () => {
    const all = parseLog((await exec.run(repo, logArgs(undefined, 0))).stdout)
    const a = await commitActivity(exec, repo, undefined)
    expect(a.total).toBe(all.length)
    expect(a.buckets.reduce((s, b) => s + b.count, 0)).toBe(all.length)
    const dates = all.map((c) => c.authorDate)
    expect(a.first).toBe(Math.min(...dates))
    expect(a.last).toBe(Math.max(...dates))
  })

  it('커밋 없는 저장소는 빈 결과', async () => {
    const a = await commitActivity(exec, empty, undefined)
    expect(a.total).toBe(0)
    expect(a.buckets).toEqual([])
  })

  it('큰 저장소(ANTLER_BIG_REPO)에서 소요 시간을 잰다', async () => {
    const big = process.env['ANTLER_BIG_REPO']
    if (!big) return
    const t = performance.now()
    const a = await commitActivity(exec, big, undefined)
    console.log(`commitActivity ${a.total} commits, ${a.buckets.length} buckets (${a.bucketSec}s): ${(performance.now() - t).toFixed(0)} ms`)
    expect(a.total).toBeGreaterThan(0)
  }, 60_000)
})
