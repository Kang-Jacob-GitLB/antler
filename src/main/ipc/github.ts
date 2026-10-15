import { shell, type IpcMainInvokeEvent } from 'electron'
import type { IpcArgs, IpcChannel, IpcResult } from '@shared/ipc'
import type { GitHubService } from '../github/service'
import { allowedExternalUrl, parseAvatarAuthors, parseId, parseListQuery, parseNumber, parsePat } from '../github/validate'
import type { RepoService } from '../services/RepoService'

/** handlers.ts 의 handle() 과 같은 모양 — 결과를 IpcEnvelope 로 감싸 등록하는 함수를 주입받는다 */
export type IpcHandle = <C extends IpcChannel>(
  channel: C,
  fn: (e: IpcMainInvokeEvent, ...args: IpcArgs<C>) => Promise<IpcResult<C>> | IpcResult<C>
) => void

/**
 * gh:* 채널 등록. 모든 인자는 여기서 검증하고, 토큰은 GitHubService 안에만 있어 어떤 응답에도 실리지 않는다
 * (gh:auth 계열은 GhAuthState 만 돌려준다).
 */
export function registerGithubIpc(handle: IpcHandle, gh: GitHubService, repos: RepoService): void {
  handle('gh:repo', (_e, repoId) => repos.ghRepo(parseId(repoId, '저장소 ID')))
  handle('gh:auth', () => gh.auth())
  handle('gh:loginPat', (_e, token) => gh.loginPat(parsePat(token)))
  handle('gh:deviceStart', () => gh.deviceStart())
  handle('gh:devicePoll', (_e, requestId) => gh.devicePoll(parseId(requestId, '요청 ID')))
  handle('gh:logout', () => gh.logout())
  handle('gh:pulls', (_e, repoId, query) => gh.pulls(parseId(repoId, '저장소 ID'), parseListQuery(query)))
  handle('gh:pull', (_e, repoId, n) => gh.pull(parseId(repoId, '저장소 ID'), parseNumber(n)))
  handle('gh:pullFiles', (_e, repoId, n) => gh.pullFiles(parseId(repoId, '저장소 ID'), parseNumber(n)))
  handle('gh:pullCommits', (_e, repoId, n) => gh.pullCommits(parseId(repoId, '저장소 ID'), parseNumber(n)))
  handle('gh:issues', (_e, repoId, query) => gh.issues(parseId(repoId, '저장소 ID'), parseListQuery(query)))
  handle('gh:issue', (_e, repoId, n) => gh.issue(parseId(repoId, '저장소 ID'), parseNumber(n)))
  handle('gh:comments', (_e, repoId, n) => gh.comments(parseId(repoId, '저장소 ID'), parseNumber(n)))
  handle('gh:filters', (_e, repoId) => gh.filters(parseId(repoId, '저장소 ID')))
  handle('gh:avatars', (_e, repoId, requestId, authors) =>
    gh.avatars(parseId(repoId, '저장소 ID'), parseId(requestId, '요청 ID'), parseAvatarAuthors(authors))
  )
  handle('gh:openExternal', async (_e, url) => {
    const href = allowedExternalUrl(url)
    if (!href) throw new Error('GitHub(https://github.com/) 주소만 열 수 있습니다')
    await shell.openExternal(href)
  })
}
