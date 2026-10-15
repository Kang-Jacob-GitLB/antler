/**
 * 항목 수와 총 비용(바이트 추정치)을 함께 제한하는 LRU 캐시. Map 의 삽입 순서를 최근 사용 순으로 쓴다.
 * 값은 불변으로 취급한다(커밋 해시 기준 캐시용).
 */
export class LruCache<K, V> {
  private readonly map = new Map<K, { value: V; cost: number }>()
  private total = 0

  constructor(
    private readonly maxEntries: number,
    private readonly maxCost = Infinity
  ) {}

  get size(): number {
    return this.map.size
  }

  get cost(): number {
    return this.total
  }

  get(key: K): V | undefined {
    const e = this.map.get(key)
    if (!e) return undefined
    // 최근 사용으로 옮긴다
    this.map.delete(key)
    this.map.set(key, e)
    return e.value
  }

  has(key: K): boolean {
    return this.map.has(key)
  }

  /** cost 가 maxCost 를 혼자 넘는 항목은 저장하지 않는다(다른 항목을 다 밀어내지 않도록) */
  set(key: K, value: V, cost = 1): void {
    this.delete(key)
    if (cost > this.maxCost) return
    this.map.set(key, { value, cost })
    this.total += cost
    while (this.map.size > this.maxEntries || this.total > this.maxCost) {
      const oldest = this.map.keys().next()
      if (oldest.done) break
      this.delete(oldest.value)
    }
  }

  delete(key: K): boolean {
    const e = this.map.get(key)
    if (!e) return false
    this.map.delete(key)
    this.total -= e.cost
    return true
  }

  clear(): void {
    this.map.clear()
    this.total = 0
  }
}
