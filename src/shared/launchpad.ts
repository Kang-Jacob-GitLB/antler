// Launchpad 항목 키와 저장 한도. renderer 가 만들고 main(ConfigStore)이 검증하므로 한곳에 둔다.

/** GitHub 항목 `owner/repo#번호`(소문자로 정규화) 또는 Jira 항목 `jira:KEY` */
export const LAUNCHPAD_KEY_RE = /^(?:[a-z0-9._-]{1,100}\/[a-z0-9._-]{1,100}#\d{1,9}|jira:[A-Z][A-Z0-9_]{1,19}-\d{1,9})$/

export const MAX_LAUNCHPAD_PINS = 200
export const MAX_LAUNCHPAD_SNOOZES = 500

export function ghItemKey(owner: string, repo: string, number: number): string {
  return `${owner}/${repo}#${number}`.toLowerCase()
}

export const jiraItemKey = (key: string): string => `jira:${key}`
