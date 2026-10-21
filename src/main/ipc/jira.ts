import { shell } from 'electron'
import type { JiraService } from '../jira/service'
import type { IpcHandle } from './github'

/** jira:* 채널 등록. 인자 검증은 JiraService 가 하고, 자격 증명은 어떤 응답에도 실리지 않는다(jira:auth 계열은 JiraAuthState 만 돌려준다) */
export function registerJiraIpc(handle: IpcHandle, jira: JiraService): void {
  handle('jira:auth', () => jira.auth())
  handle('jira:connect', (_e, input) => jira.connect(input))
  handle('jira:disconnect', () => jira.disconnect())
  handle('jira:issue', (_e, key, requestId) => jira.issue(key, typeof requestId === 'string' ? requestId : undefined))
  handle('jira:transitions', (_e, key, requestId) => jira.transitions(key, typeof requestId === 'string' ? requestId : undefined))
  handle('jira:transition', (_e, key, transitionId) => jira.transition(key, transitionId))
  handle('jira:comments', (_e, key, requestId) => jira.comments(key, typeof requestId === 'string' ? requestId : undefined))
  handle('jira:dev', (_e, key, requestId) => jira.devInfo(key, typeof requestId === 'string' ? requestId : undefined))
  handle('jira:search', (_e, query, requestId) => jira.search(query, typeof requestId === 'string' ? requestId : undefined))
  handle('jira:worklogs', (_e, query, requestId) => jira.worklogs(query, typeof requestId === 'string' ? requestId : undefined))
  handle('jira:userSearch', (_e, text, requestId) => jira.userSearch(text, typeof requestId === 'string' ? requestId : undefined))
  handle('jira:openExternal', async (_e, url) => {
    const href = jira.externalUrl(url)
    if (!href) throw new Error('연결한 Jira 사이트의 주소만 열 수 있습니다')
    await shell.openExternal(href)
  })
}
