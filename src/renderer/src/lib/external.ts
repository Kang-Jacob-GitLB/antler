// 외부 도구로 열기(파일 관리자·에디터·터미널)의 renderer 쪽 진입점. 실행은 모두 main 이 한다 — 여기서는 저장소 id·동작·상대 경로만 보낸다.
import type { ExternalAction, GithubRepoLink, Repo, SourceKind } from '@shared/types'
import { useAppStore } from '../stores/appStore'
import { api, errorMessage } from './api'
import { githubUrl } from './ghRepoList'

/** SSH 저장소는 원격 에디터(VS Code·Cursor)로만 열 수 있다. 파일 관리자·터미널은 지원하지 않는다 */
export function externalSupported(kind: SourceKind | undefined, action: ExternalAction): boolean {
  // GitHub 원격 전용 저장소는 작업 폴더가 없다(캐시 폴더를 열어 봐야 bare 저장소뿐) — 대신 GitHub 에서 연다
  if (kind === 'github') return false
  return kind !== 'ssh' || action === 'editor'
}

/** 활성 저장소가 아닌 임의 저장소의 GitHub 링크(원격 전용 저장소만) */
export function githubLinkOf(repoId: string | null): GithubRepoLink | undefined {
  return useAppStore.getState().config?.repos.find((r) => r.id === repoId)?.github
}

/** 원격 전용 저장소를 GitHub 웹에서 연다 */
export async function openOnGithub(repoId: string): Promise<void> {
  const link = githubLinkOf(repoId)
  if (!link) return
  try {
    await api.invoke('gh:openExternal', githubUrl(link))
  } catch (err) {
    useAppStore.setState({ error: errorMessage(err) })
  }
}

/** 저장소 위치 표기: 원격 전용은 앱 캐시 경로 대신 GitHub 주소 */
export function repoLocation(repo: Repo): string {
  return repo.github ? githubUrl(repo.github) : repo.path
}

/** path 는 저장소 루트 기준 '/' 경로(없으면 저장소 폴더) */
export async function openExternal(repoId: string, action: ExternalAction, path?: string): Promise<void> {
  try {
    await api.invoke('external:open', repoId, { action, ...(path ? { path } : {}) })
  } catch (err) {
    useAppStore.setState({ error: errorMessage(err) })
  }
}

/** 저장소의 환경 종류(메뉴 항목 활성 여부 판단용) */
export function sourceKindOf(repoId: string | null): SourceKind | undefined {
  const cfg = useAppStore.getState().config
  const repo = cfg?.repos.find((r) => r.id === repoId)
  return cfg?.sources.find((s) => s.id === repo?.sourceId)?.kind
}
