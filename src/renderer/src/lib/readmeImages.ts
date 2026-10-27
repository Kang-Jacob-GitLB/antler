// README 의 저장소 이미지를 git:blob 으로 받아 채운다(api 를 쓰므로 순수 함수 모음인 readme.ts 와 나눈다)
import type { CodeBlobResult } from '@shared/types'
import { api, newRequestId } from './api'

// 같은 그림을 다시 받지 않게 작은 캐시를 둔다. 개수와 data URL 문자열 길이 합(바이트 상한)을 모두 제한한다
const IMAGE_CACHE_MAX = 40
const IMAGE_CACHE_MAX_BYTES = 32 * 1024 * 1024
const imageCache = new Map<string, string | null>()
let imageCacheBytes = 0

/** 코드 탐색기를 닫을 때 캐시를 비운다 */
export function clearRepoImageCache(): void {
  imageCache.clear()
  imageCacheBytes = 0
}

function evictOldest(): void {
  const oldest = imageCache.keys().next().value
  if (oldest === undefined) return
  imageCacheBytes -= imageCache.get(oldest)?.length ?? 0
  imageCache.delete(oldest)
}

function cacheImage(key: string, url: string | null): void {
  const size = url?.length ?? 0
  if (imageCache.has(key)) {
    // 동시에 같은 그림을 받은 경우: 합계가 겹쳐 세어지지 않게 먼저 뺀다
    imageCacheBytes -= imageCache.get(key)?.length ?? 0
    imageCache.delete(key)
  }
  if (size > IMAGE_CACHE_MAX_BYTES) return // 상한보다 큰 단일 항목은 캐시하지 않는다
  while (imageCache.size > 0 && (imageCache.size >= IMAGE_CACHE_MAX || imageCacheBytes + size > IMAGE_CACHE_MAX_BYTES)) evictOldest()
  imageCache.set(key, url)
  imageCacheBytes += size
}

async function fetchImage(repoId: string, rev: string, path: string, ids: string[]): Promise<string | null> {
  const key = `${repoId}\0${rev}\0${path}`
  if (imageCache.has(key)) return imageCache.get(key) ?? null
  const id = newRequestId('mdimg')
  ids.push(id)
  let url: string | null = null
  try {
    const res: CodeBlobResult = await api.invoke('git:blob', repoId, { rev, path }, id)
    url = res.kind === 'image' && res.dataUrl ? res.dataUrl : null
  } catch {
    return null // 취소·일시 오류는 캐시하지 않는다
  }
  cacheImage(key, url)
  return url
}

const IMAGE_CONCURRENCY = 4

/** `img[data-repo-src]` 를 저장소에서 읽어 채운다. 이미지가 아니거나 없으면 깨진 그림 대신 alt 글자를 보인다 */
export async function loadRepoImages(root: ParentNode, repoId: string, rev: string, ids: string[], alive: () => boolean = () => true): Promise<void> {
  const imgs = [...root.querySelectorAll<HTMLImageElement>('img[data-repo-src]')]
  let next = 0
  const worker = async (): Promise<void> => {
    while (alive() && next < imgs.length) {
      const img = imgs[next++]!
      const path = img.getAttribute('data-repo-src') ?? ''
      const url = path ? await fetchImage(repoId, rev, path, ids) : null
      if (!alive()) return
      if (url) img.setAttribute('src', url)
      else {
        const span = img.ownerDocument.createElement('span')
        span.className = 'md-img-missing'
        span.textContent = img.getAttribute('alt') || path.split('/').pop() || ''
        img.replaceWith(span)
      }
    }
  }
  await Promise.all(Array.from({ length: Math.min(IMAGE_CONCURRENCY, imgs.length) }, worker))
}
