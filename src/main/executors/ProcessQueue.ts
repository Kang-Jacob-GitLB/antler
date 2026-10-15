import { AbortError } from './GitExecutor'

interface Waiter {
  start: () => void
  cancel: () => void
}

/**
 * 동시 실행 수 제한 + 동일 키 요청 dedupe.
 * 환경마다 하나씩 두어 git 프로세스(또는 SSH 채널)가 폭주하지 않게 한다.
 */
export class ProcessQueue {
  private active = 0
  private readonly waiting: Waiter[] = []
  private readonly inflight = new Map<string, Promise<unknown>>()

  constructor(private readonly limit: number) {}

  /**
   * key 가 같은 요청이 진행 중이면 그 Promise 를 공유한다.
   * 공유 요청에는 signal 을 걸지 않는다 — 한쪽 취소가 다른 쪽을 죽이면 안 되기 때문이다.
   */
  run<T>(task: () => Promise<T>, opts: { key?: string; signal?: AbortSignal } = {}): Promise<T> {
    const { key, signal } = opts
    if (key) {
      const existing = this.inflight.get(key)
      if (existing) return existing as Promise<T>
    }
    const p = this.schedule(task, key ? undefined : signal)
    if (key) {
      this.inflight.set(key, p)
      const clear = (): void => {
        if (this.inflight.get(key) === p) this.inflight.delete(key)
      }
      p.then(clear, clear)
    }
    return p
  }

  get stats(): { active: number; waiting: number } {
    return { active: this.active, waiting: this.waiting.length }
  }

  private schedule<T>(task: () => Promise<T>, signal?: AbortSignal): Promise<T> {
    return new Promise<T>((resolve, reject) => {
      if (signal?.aborted) return reject(new AbortError())
      const exec = (): void => {
        signal?.removeEventListener('abort', onAbort)
        this.active++
        // task 가 동기로 throw 해도 슬롯을 돌려받도록 Promise 체인 안에서 호출한다
        Promise.resolve()
          .then(task)
          .then(resolve, reject)
          .finally(() => {
            this.active--
            this.next()
          })
      }
      const waiter: Waiter = {
        start: exec,
        cancel: () => reject(new AbortError())
      }
      const onAbort = (): void => {
        const i = this.waiting.indexOf(waiter)
        if (i >= 0) {
          this.waiting.splice(i, 1)
          waiter.cancel()
        }
      }
      signal?.addEventListener('abort', onAbort, { once: true })
      if (this.active < this.limit) exec()
      else this.waiting.push(waiter)
    })
  }

  private next(): void {
    if (this.active >= this.limit) return
    this.waiting.shift()?.start()
  }
}
