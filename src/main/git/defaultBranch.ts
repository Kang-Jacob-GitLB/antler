// 저장소의 기본 브랜치 판정(읽기 전용). 순서:
//   1) refs/remotes/origin/HEAD 가 가리키는 리모트 추적 브랜치(예: origin/main) — 로컬 main 이 체크아웃돼 있거나 앞서 있어도
//      원격 tip 을 쓴다(단순함을 위해. "기본 브랜치"는 원격이 정한 것이다)
//   2) origin/HEAD 가 없으면 로컬 main, 그다음 master
//   3) 그 밖에는 없음
import type { Branch, DefaultBranch, RemoteGroup } from '@shared/types'

/**
 * @param originHead `git symbolic-ref -q refs/remotes/origin/HEAD` 의 출력(없으면 null). 예: refs/remotes/origin/main
 */
export function pickDefaultBranch(originHead: string | null, local: Branch[], remotes: RemoteGroup[]): DefaultBranch | undefined {
  const target = originHead?.trim()
  if (target?.startsWith('refs/remotes/')) {
    for (const g of remotes) {
      const b = g.branches.find((x) => x.fullName === target)
      if (b) return { name: b.name, hash: b.hash }
    }
  }
  for (const name of ['main', 'master']) {
    const b = local.find((x) => x.name === name)
    if (b) return { name: b.name, hash: b.hash }
  }
  return undefined
}
