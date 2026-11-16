import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { describe, expect, it } from 'vitest'
import { handleNote, type NoteTarget } from '../src/sidecar/note'
import { NOTE_OPS } from '../src/sidecar/protocol'

function recorder(): { target: NoteTarget; calls: [string, string][] } {
  const calls: [string, string][] = []
  return { target: { noteLog: (op, repoId) => void calls.push([op, repoId]) }, calls }
}

describe('사이드카 note 메시지', () => {
  it('허용 목록의 op 와 문자열 repoId 만 RepoService 로 넘긴다', () => {
    const { target, calls } = recorder()
    expect(handleNote(target, { op: 'logFirstPage', repoId: 'a' })).toBe(true)
    expect(handleNote(target, { op: 'logStale', repoId: 'b' })).toBe(true)
    expect(calls).toEqual([
      ['logFirstPage', 'a'],
      ['logStale', 'b']
    ])
  })

  it('fetchChanged(Rust 의 fetch 가 ref 를 바꿨다)도 허용 목록의 op 다', () => {
    const { target, calls } = recorder()
    expect(NOTE_OPS).toContain('fetchChanged')
    expect(handleNote(target, { op: 'fetchChanged', repoId: 'a' })).toBe(true)
    expect(calls).toEqual([['fetchChanged', 'a']])
  })

  it('모르는 op·문자열이 아닌 repoId·빈 메시지는 버린다', () => {
    const { target, calls } = recorder()
    for (const m of [
      { op: 'invalidateEverything', repoId: 'a' },
      { op: 'constructor', repoId: 'a' },
      { op: 'toString', repoId: 'a' },
      { op: 5, repoId: 'a' },
      { op: 'logStale', repoId: 5 },
      { op: 'logStale', repoId: null },
      { op: 'logStale' },
      { repoId: 'a' },
      {}
    ]) {
      expect(handleNote(target, m), JSON.stringify(m)).toBe(false)
    }
    expect(calls).toEqual([])
  })

  it('Rust 가 보내는 op 문자열이 프로토콜의 목록과 같다', () => {
    const src = readFileSync(join(__dirname, '..', 'src-tauri', 'src', 'git', 'log_service.rs'), 'utf8')
    const sent = [...src.matchAll(/=> "((?:log[A-Za-z]+)|fetchChanged)"/g)].map((m) => m[1])
    expect(sent.sort()).toEqual([...NOTE_OPS].sort())
  })

  it('renderer 인자는 req 의 args 안에만 들어가고 t 는 Rust 가 정한다', () => {
    // 사이드카 쪽 분기(index.ts)는 최상위 t 로만 메시지 종류를 고르고, Rust 가 req 를 만드는 곳은 args 를 값으로만 넣는다
    const rust = readFileSync(join(__dirname, '..', 'src-tauri', 'src', 'sidecar.rs'), 'utf8')
    expect(rust).toContain('json!({ "t": "req", "id": id, "ch": channel, "args": args })')
    expect(rust).toContain('json!({ "t": "note", "op": op.as_str(), "repoId": repo_id })')
  })
})
