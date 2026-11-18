// Rust 가 보낸 `githubBranch` 메시지를 ConfigStore 에 옮긴다. 프로토콜은 protocol.ts.
//
// GitHub 원격 전용 저장소의 fetch 를 Rust 가 맡아도 레코드(`github.defaultBranch`)를 쓰는 쪽은 아직 이 프로세스의 ConfigStore 다(설정 소유 전환 전).
// renderer 가 보낸 `req` 는 여기로 오지 않는다 — 메시지의 `t` 는 Rust 가 정하고 renderer 의 값은 `req` 의 `args` 안에만 들어간다. 그래도 사이드카는 모양을 다시 검사한다.
import { isSafeBranch } from '../main/git/githubCache'

/** 처리에 필요한 것만 — 테스트가 ConfigStore 전체를 만들지 않아도 된다 */
export interface GithubBranchTarget {
  repo(repoId: string): { sourceId: string } | undefined
  source(sourceId: string): { kind: string } | undefined
  setGithubDefaultBranch(repoId: string, branch: string): void
}

/** 존재하는 github 소스 저장소이고 쓸 수 있는 브랜치 이름일 때만 기록한다. 처리했으면 true(이미 같은 값이면 ConfigStore 가 아무것도 하지 않는다) */
export function handleGithubBranch(target: GithubBranchTarget, msg: { repoId?: unknown; branch?: unknown }): boolean {
  const { repoId, branch } = msg
  if (typeof repoId !== 'string' || !isSafeBranch(branch)) return false
  const repo = target.repo(repoId)
  if (!repo || target.source(repo.sourceId)?.kind !== 'github') return false
  target.setGithubDefaultBranch(repoId, branch)
  return true
}
