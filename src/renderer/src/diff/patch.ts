// GitHub PR 파일 API 의 patch 문자열(@@ 로 시작하는 unified diff 조각)을 hunk 로 바꾼다.
// main 의 DiffParser 와 같은 규칙(접두 문자, 줄 번호, "\ No newline")을 따르는 renderer 용 축약판이다.
import type { DiffHunk, DiffLine, FileDiff } from '@shared/types'

const HUNK_RE = /^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@ ?(.*)$/

export function parsePatch(patch: string): DiffHunk[] {
  const hunks: DiffHunk[] = []
  let hunk: DiffHunk | null = null
  let o = 0
  let n = 0
  for (const raw0 of patch.split('\n')) {
    const raw = raw0.endsWith('\r') ? raw0.slice(0, -1) : raw0
    const m = raw.startsWith('@@') ? HUNK_RE.exec(raw) : null
    if (m) {
      hunk = {
        oldStart: Number(m[1]),
        oldLines: m[2] === undefined ? 1 : Number(m[2]),
        newStart: Number(m[3]),
        newLines: m[4] === undefined ? 1 : Number(m[4]),
        header: m[5] ?? '',
        lines: []
      }
      o = hunk.oldStart
      n = hunk.newStart
      hunks.push(hunk)
      continue
    }
    if (!hunk || raw === '') continue
    const t = raw.charAt(0)
    const text = raw.slice(1)
    let line: DiffLine
    if (t === '+') line = { t: '+', text, n: n++ }
    else if (t === '-') line = { t: '-', text, o: o++ }
    else if (t === '\\') line = { t: '\\', text: text.trim() }
    else line = { t: ' ', text, o: o++, n: n++ }
    hunk.lines.push(line)
  }
  return hunks
}

/**
 * PR 파일 하나를 diff 뷰어가 받는 FileDiff 모양으로.
 * GitHub 는 바이너리뿐 아니라 너무 큰 텍스트 diff·내용 없는 이름 변경에도 patch 를 주지 않으므로,
 * patch 가 없다고 모두 바이너리로 보지 않는다 — 줄 수 변화가 있거나 이름 변경이면 "diff 없음" 안내로 둔다.
 */
export function fileDiffFromPatch(
  path: string,
  patch: string | undefined,
  oldPath?: string,
  stats?: { additions: number; deletions: number; status: string }
): FileDiff {
  const hunks = patch ? parsePatch(patch) : []
  const noPatchText = patch === undefined && !!stats && (stats.additions + stats.deletions > 0 || stats.status === 'renamed')
  return {
    path,
    ...(oldPath ? { oldPath } : {}),
    binary: patch === undefined && !noPatchText,
    hunks,
    truncated: false,
    totalLines: hunks.reduce((s, h) => s + h.lines.length, 0),
    meta: noPatchText ? [stats!.status === 'renamed' && stats!.additions + stats!.deletions === 0 ? 'rename-only' : 'too-large'] : []
  }
}
