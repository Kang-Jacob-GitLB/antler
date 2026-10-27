import { useEffect, useMemo, useRef } from 'react'
import { api } from '../lib/api'
import { findAnchor, highlightCodeBlocks, parseRepoUrl, renderMarkdownDoc } from '../lib/readme'
import { loadRepoImages } from '../lib/readmeImages'
import { useCodeStore } from '../stores/codeStore'
import './MarkdownBody.css'

/**
 * 저장소의 마크다운 문서를 GitHub 처럼 보여 준다(상대 이미지·링크는 저장소 경로로 푼다).
 * 링크: 저장소 안 경로는 앱 안에서 열고, #앵커는 문서 안에서 스크롤하고, 그 밖의 https 는 main 을 거쳐 기본 브라우저로 연다.
 */
export function MarkdownDoc({ source, dir, repoId, rev }: { source: string; dir: string; repoId: string; rev: string }): React.JSX.Element {
  const ref = useRef<HTMLDivElement>(null)
  const html = useMemo(() => renderMarkdownDoc(source, dir), [source, dir])
  const pendingAnchor = useCodeStore((s) => s.anchor)

  // HTML 을 넣은 뒤: 코드블록 강조, 저장소 이미지 채우기
  useEffect(() => {
    const root = ref.current
    if (!root) return
    let alive = true
    const ids: string[] = []
    const stillAlive = (): boolean => alive
    void highlightCodeBlocks(root, stillAlive)
    void loadRepoImages(root, repoId, rev, ids, stillAlive)
    return () => {
      alive = false
      for (const id of ids) void api.invoke('req:cancel', id)
    }
  }, [html, repoId, rev])

  // 다른 문서의 #앵커로 열었으면 문서를 넣은 뒤 그 제목으로 스크롤한다
  useEffect(() => {
    if (!pendingAnchor) return
    const root = ref.current
    if (root) findAnchor(root, pendingAnchor)?.scrollIntoView({ block: 'start' })
    useCodeStore.getState().setAnchor(null)
  }, [html, pendingAnchor])

  const activate = (e: React.SyntheticEvent): boolean => {
    const root = ref.current
    const target = e.target as HTMLElement
    const anchor = target.closest('a.anchor[data-anchor]')
    if (anchor && root) {
      e.preventDefault()
      findAnchor(root, anchor.getAttribute('data-anchor') ?? '')?.scrollIntoView({ block: 'start' })
      return true
    }
    const link = target.closest('[data-href]')
    if (!link) return false
    e.preventDefault()
    const href = link.getAttribute('data-href') ?? ''
    const repo = parseRepoUrl(href)
    if (!repo) {
      void api.invoke('code:openLink', href).catch(() => {})
      return true
    }
    // 같은 문서 안 #앵커(상대 기준이 이 문서의 디렉터리라 그 경로 + 해시로 풀린다)
    if (repo.hash && repo.path === dir && root) {
      findAnchor(root, repo.hash)?.scrollIntoView({ block: 'start' })
      return true
    }
    void useCodeStore.getState().goto(repo.path, repo.hash || undefined)
    return true
  }

  return (
    <div
      ref={ref}
      className="markdown-body selectable"
      onClick={(e) => void activate(e)}
      onKeyDown={(e) => {
        if (e.key === 'Enter' && (e.target as HTMLElement).closest('[data-href]')) activate(e)
      }}
      dangerouslySetInnerHTML={{ __html: html }}
    />
  )
}
