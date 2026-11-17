import { readFileSync } from 'node:fs'
import { describe, expect, it } from 'vitest'
import { itKeys } from './fixtures/itTitles'

// Phase 6a(나머지 git 채널 12개 + RepoWatcher 의 Rust 이식)의 TS 테스트 → Rust 테스트 이식 대응표(tests/golden/channels/test-map.json) 검사.
// 순수 로직의 골든은 tests/golden.test.ts 가 맡고(파서 manifest), 이 파일은 "대상 TS 테스트의 모든 it 이 대응표에 있고, 값이 거짓말이 아닌가"만 본다.
// 값은 세 가지다: `<파일>.rs::<fn>`(그 파일에 fn 이 있다) · `N/A: PR n[a|b]`(뒤 PR 에서 옮긴다 — 마지막 PR 에서 0개여야 한다) · `N/A: <사유>`.
// 같은 구조의 SSH 대응표 검사(goldenSsh.test.ts)와 달리 PR 번호에 `3a`·`7b` 같은 갈래가 있다.

const MAP = new URL('./golden/channels/test-map.json', import.meta.url)

/** 대응 대상 TS 테스트 파일 */
const TARGET_TESTS = [
  'codeBrowse',
  'activity',
  'activityRepo',
  'lfsGitflow',
  'refs',
  'refsDetails',
  'search',
  'wip',
  'wipFilter',
  'wipFilterNonUtf8',
  'hostileRepo',
  'submodule',
  'defaultBranch',
  'repoWatcher',
  'repoService'
]

/** repoService.test.ts 는 6a 에 걸린 describe 만 대상이다(`RepoService.log` 는 Phase 2 에서 옮긴 git:log 의 시험) */
const REPO_SERVICE_DESCRIBES = ['RepoService 첫 페이지 문', 'RepoService.addRepo', 'RepoService.scanRepos', 'ConfigStore']

/** 대응표 값의 `<파일>::<이름>` 이 가리키는 Rust 소스 */
const RUST_SOURCES: Record<string, string> = {
  'golden.rs': '../src-tauri/tests/golden.rs',
  'exec.rs': '../src-tauri/tests/exec.rs',
  'channels.rs': '../src-tauri/tests/channels.rs',
  'refs.rs': '../src-tauri/src/git/parsers/refs.rs',
  'submodule.rs': '../src-tauri/src/git/parsers/submodule.rs',
  'activity.rs': '../src-tauri/src/git/activity.rs',
  'code_browse.rs': '../src-tauri/src/git/code_browse.rs',
  'lfs.rs': '../src-tauri/src/git/lfs.rs',
  'link_guard.rs': '../src-tauri/src/git/link_guard.rs',
  'refs_service.rs': '../src-tauri/src/git/refs_service.rs',
  'repo_state.rs': '../src-tauri/src/git/repo_state.rs',
  'wip.rs': '../src-tauri/src/git/wip.rs'
}

describe('Phase 6a 이식 대응표', () => {
  it('대상 테스트의 모든 it 이 대응표에 있고, 값의 모양이 맞고, 가리키는 Rust 테스트가 있다', () => {
    const map = JSON.parse(readFileSync(MAP, 'utf8')) as Record<string, string>
    const keys: string[] = []
    for (const f of TARGET_TESTS) {
      const seen = new Map<string, number>()
      for (const k of itKeys(readFileSync(new URL(`./${f}.test.ts`, import.meta.url), 'utf8'))) {
        const n = (seen.get(k) ?? 0) + 1
        seen.set(k, n)
        // 같은 파일 안에서 제목이 겹치면 두 번째부터 ` #n` 을 붙인다(원본 제목은 고치지 않는다)
        if (f === 'repoService' && !REPO_SERVICE_DESCRIBES.some((d) => k.startsWith(d))) continue
        keys.push(`${f} > ${k}${n > 1 ? ` #${n}` : ''}`)
      }
    }
    expect(new Set(keys).size).toBe(keys.length)
    expect(keys.filter((k) => !(k in map)), '대응표에 없는 it (대상 테스트에 it 을 더했으면 test-map.json 에도 적는다)').toEqual([])
    expect(Object.keys(map).filter((k) => !keys.includes(k)), '원본에 없는 대응표 항목').toEqual([])
    const sources = new Map<string, string>()
    for (const [title, value] of Object.entries(map)) {
      if (/^N\/A: PR [2-8][ab]?$/.test(value)) continue
      if (value.startsWith('N/A: ')) {
        expect(value.startsWith('N/A: PR '), `${title}: 'N/A: PR n' 은 n 이 2~8 이고 뒤에 a·b 만 붙을 수 있다 → ${value}`).toBe(false)
        expect(value.length, `${title}: N/A 사유가 비어 있다`).toBeGreaterThan(8)
        continue
      }
      const m = /^([a-z0-9_]+\.rs)::(\S+)$/.exec(value)
      expect(m, `${title}: 'N/A: PR n'·'N/A: 사유' 또는 '<파일>.rs::<테스트 이름>' 이어야 한다 → ${value}`).not.toBeNull()
      const rel = RUST_SOURCES[m![1]!]
      expect(rel, `${title}: 알 수 없는 Rust 파일 ${m![1]}`).toBeDefined()
      if (!sources.has(rel!)) sources.set(rel!, readFileSync(new URL(rel!, import.meta.url), 'utf8'))
      expect(new RegExp(String.raw`\bfn ${m![2]!}\(`).test(sources.get(rel!)!), `${title}: ${m![1]} 에 fn ${m![2]} 이 없다`).toBe(true)
    }
  })
})
