import { resolve } from 'node:path'
import { describe, expect, it } from 'vitest'
import { AbortError, GitError } from '../src/main/executors/GitExecutor'
import { LocalExecutor } from '../src/main/executors/LocalExecutor'
import { ProcessQueue } from '../src/main/executors/ProcessQueue'

const repoRoot = resolve(__dirname, '..')

describe('LocalExecutor', () => {
  const exec = new LocalExecutor()

  it('저장소 안에서 git 을 실행한다', async () => {
    const r = await exec.run(repoRoot, ['rev-parse', '--is-inside-work-tree'])
    expect(r.code).toBe(0)
    expect(r.stdout.toString().trim()).toBe('true')
  })

  it('실패 코드를 그대로 돌려준다', async () => {
    const r = await exec.run(repoRoot, ['rev-parse', '--verify', 'refs/heads/__없는_브랜치__'])
    expect(r.code).not.toBe(0)
  })

  it('stream 은 실패 시 GitError 를 던진다', async () => {
    const consume = async () => {
      for await (const _ of exec.stream(repoRoot, ['log', 'refs/heads/__없는_브랜치__'])) void _
    }
    await expect(consume()).rejects.toBeInstanceOf(GitError)
  })

  it('이미 취소된 signal 이면 AbortError', async () => {
    const ac = new AbortController()
    ac.abort()
    await expect(exec.run(repoRoot, ['status'], { signal: ac.signal })).rejects.toBeInstanceOf(AbortError)
  })
})

describe('ProcessQueue', () => {
  const defer = () => {
    let resolve!: (v: number) => void
    const promise = new Promise<number>((r) => (resolve = r))
    return { promise, resolve }
  }

  it('동시 실행 수를 제한한다', async () => {
    const q = new ProcessQueue(2)
    const ds = [defer(), defer(), defer()]
    const ps = ds.map((d) => q.run(() => d.promise))
    expect(q.stats).toEqual({ active: 2, waiting: 1 })
    ds[0]!.resolve(1)
    await ps[0]
    await Promise.resolve()
    expect(q.stats).toEqual({ active: 2, waiting: 0 })
    ds[1]!.resolve(2)
    ds[2]!.resolve(3)
    expect(await Promise.all(ps)).toEqual([1, 2, 3])
  })

  it('같은 key 요청은 하나로 합친다', async () => {
    const q = new ProcessQueue(4)
    let calls = 0
    const d = defer()
    const task = () => {
      calls++
      return d.promise
    }
    const a = q.run(task, { key: 'k' })
    const b = q.run(task, { key: 'k' })
    expect(a).toBe(b)
    d.resolve(7)
    expect(await b).toBe(7)
    expect(calls).toBe(1)
  })

  it('대기 중인 요청을 취소하면 실행하지 않는다', async () => {
    const q = new ProcessQueue(1)
    const d = defer()
    const first = q.run(() => d.promise)
    const ac = new AbortController()
    let ran = false
    const second = q.run(
      async () => {
        ran = true
        return 0
      },
      { signal: ac.signal }
    )
    ac.abort()
    await expect(second).rejects.toBeInstanceOf(AbortError)
    d.resolve(1)
    await first
    expect(ran).toBe(false)
  })
})
