import { describe, expect, it } from 'vitest'
import { SECTIONS, slugFromRepositoryUrl } from '../src/main/github/launchpad'
import { pickGithubRemotes } from '../src/main/github/remoteUrl'

describe('Launchpad 검색어', () => {
  it('섹션마다 사용자 기준 검색어 하나다(repo: 한정자 없음, 열린 항목·보관 제외)', () => {
    expect(SECTIONS.map((s) => [s.section, s.q])).toEqual([
      ['authored', 'is:pr is:open author:@me archived:false'],
      ['review', 'is:pr is:open review-requested:@me archived:false'],
      ['issues', 'is:issue is:open assignee:@me archived:false']
    ])
    for (const s of SECTIONS) {
      expect(s.q).not.toContain('repo:')
      expect(s.q.length).toBeLessThanOrEqual(256)
    }
  })
})

describe('slugFromRepositoryUrl', () => {
  it('검색 결과의 repository_url 에서 owner/repo 를 뽑는다', () => {
    expect(slugFromRepositoryUrl('https://api.github.com/repos/octo/hello')).toEqual({ owner: 'octo', repo: 'hello' })
  })
  it('다른 형태는 거부한다', () => {
    expect(slugFromRepositoryUrl(undefined)).toBeNull()
    expect(slugFromRepositoryUrl('https://evil.example/repos/a/b')).toBeNull()
    expect(slugFromRepositoryUrl('https://api.github.com/repos/a/b/c')).toBeNull()
  })
  it('잘못된 퍼센트 인코딩은 예외 없이 null', () => {
    expect(slugFromRepositoryUrl('https://api.github.com/repos/a/%E0%A4%A')).toBeNull()
  })
})

describe('pickGithubRemotes', () => {
  it('origin·upstream 등 GitHub remote 를 모두 돌려주고 GitHub 가 아닌 것은 뺀다', () => {
    const out = pickGithubRemotes([
      { name: 'origin', url: 'https://github.com/me/fork.git' },
      { name: 'upstream', url: 'git@github.com:Org/Project.git' },
      { name: 'gitlab', url: 'https://gitlab.com/x/y.git' }
    ])
    expect(out).toEqual([
      { owner: 'me', repo: 'fork', remote: 'origin' },
      { owner: 'Org', repo: 'Project', remote: 'upstream' }
    ])
  })
  it('같은 저장소(대소문자 무시)는 하나만 둔다', () => {
    expect(pickGithubRemotes([{ name: 'origin', url: 'https://github.com/A/B' }, { name: 'mirror', url: 'https://github.com/a/b.git' }])).toHaveLength(1)
  })
})
