import type { DiffHunk, DiffLine, FileDiff } from '@shared/types'

/** 기본 diff 줄 수 상한 */
export const DEFAULT_MAX_DIFF_LINES = 3000
/** 상한 이후 줄 수를 세는 한계. 넘으면 totalLines 는 하한값이 된다 */
export const DEFAULT_COUNT_LIMIT = 200_000

export interface DiffParseOpts {
  /** 객체로 만들 diff 줄 수 상한(기본 3000) */
  maxLines?: number
  /** 파싱하는 입력 바이트 상한. 넘으면 나머지는 세기만 한다(기본 무제한) */
  maxBytes?: number
  /** 상한 이후 줄 수를 세는 한계(기본 20만). 넘으면 더 읽지 않는다 */
  countLimit?: number
}

const HUNK_RE = /^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@ ?(.*)$/

/** 사람이 읽을 만한 헤더 줄(index·---·+++ 는 제외) */
const META_PREFIXES = [
  'new file mode ',
  'deleted file mode ',
  'old mode ',
  'new mode ',
  'similarity index ',
  'dissimilarity index ',
  'rename from ',
  'rename to ',
  'copy from ',
  'copy to '
]

/** 상한 이후의 줄을 객체 없이 바이트로만 센다. '@' 로 시작하는 줄은 hunk 헤더라 diff 줄에서 뺀다 */
class LineCounter {
  lines = 0
  headers = 0
  private atStart = true

  constructor(private readonly limit: number) {}

  get exhausted(): boolean {
    return this.lines >= this.limit
  }

  feed(buf: Buffer): void {
    let i = 0
    while (i < buf.length && !this.exhausted) {
      if (this.atStart) {
        this.atStart = false
        if (buf[i] === 0x40) this.headers++
        else this.lines++
      }
      const nl = buf.indexOf(10, i)
      if (nl === -1) return
      this.atStart = true
      i = nl + 1
    }
  }
}

/**
 * 파일 하나의 unified diff(`git diff-tree -p` 출력)를 청크 단위로 받아 FileDiff 로 만든다.
 * - 첫 @@ 이전의 헤더(diff --git/index/---/+++)는 건너뛰고, 그 뒤의 "--"/"++" 로 시작하는 줄은 내용으로 본다.
 * - maxLines 에 도달하면 이후는 줄 수만 센다(truncated). 센 값이 countLimit 를 넘으면 totalLines 는 하한값이다.
 * - push 가 false 를 돌려주면 더 읽을 필요가 없다(호출자는 git 프로세스를 끊어도 된다).
 */
export class DiffParser {
  private readonly maxLines: number
  private readonly maxBytes: number
  private readonly counter: LineCounter
  private pending: Buffer[] = []
  private pendingLen = 0
  private bytes = 0
  private counting = false
  private overflow = false
  private ended = false
  private seenDiffHeader = false

  private binary = false
  private readonly meta: string[] = []
  private readonly hunks: DiffHunk[] = []
  private hunk: DiffHunk | null = null
  private inHunks = false
  private o = 0
  private n = 0
  private parsedLines = 0

  constructor(
    private readonly path: string,
    private readonly oldPath?: string,
    opts: DiffParseOpts = {}
  ) {
    this.maxLines = opts.maxLines ?? DEFAULT_MAX_DIFF_LINES
    this.maxBytes = opts.maxBytes ?? Infinity
    this.counter = new LineCounter(opts.countLimit ?? DEFAULT_COUNT_LIMIT)
  }

  /** @returns 더 읽어야 하면 true */
  push(chunk: Buffer): boolean {
    if (this.counting) {
      this.counter.feed(chunk)
      return !this.counter.exhausted
    }
    this.bytes += chunk.length
    let start = 0
    while (!this.counting) {
      const nl = chunk.indexOf(10, start)
      if (nl === -1) break
      const line = this.takeLine(chunk, start, nl)
      start = nl + 1
      this.feedLine(line)
      if (this.parsedLines >= this.maxLines && !this.counting) this.startCounting(chunk.subarray(start))
    }
    if (this.counting) return !this.counter.exhausted
    if (start < chunk.length) {
      const tail = chunk.subarray(start)
      this.pending.push(tail)
      this.pendingLen += tail.length
    }
    // 입력이 너무 크면(줄 하나가 거대한 경우 포함) 여기까지만 담는다
    if (this.bytes > this.maxBytes || this.pendingLen > this.maxBytes) {
      this.overflow = true
      this.startCounting(Buffer.alloc(0), this.pendingLen > 0)
    }
    return !this.counter.exhausted
  }

  finish(): FileDiff {
    if (!this.counting && this.pendingLen > 0) {
      // 개행 없이 끝난 마지막 줄
      const line = Buffer.concat(this.pending).toString('utf8')
      this.pending = []
      this.pendingLen = 0
      this.feedLine(line)
    }
    const counted = this.counter.lines
    const truncated = this.overflow || counted > 0 || this.counter.headers > 0
    return {
      path: this.path,
      ...(this.oldPath !== undefined ? { oldPath: this.oldPath } : {}),
      binary: this.binary,
      hunks: this.hunks,
      truncated,
      totalLines: this.parsedLines + counted,
      meta: this.meta
    }
  }

  /** 개행 앞까지를 한 줄로 꺼낸다. 이전 청크에서 이어진 조각이 있으면 합친다 */
  private takeLine(chunk: Buffer, start: number, nl: number): string {
    if (this.pendingLen === 0) return chunk.toString('utf8', start, nl)
    this.pending.push(chunk.subarray(start, nl))
    const s = Buffer.concat(this.pending).toString('utf8')
    this.pending = []
    this.pendingLen = 0
    return s
  }

  /**
   * 객체 만들기를 멈추고 남은 입력은 세기만 한다.
   * partialLine: 개행 없이 끝난 조각이 남아 있으면(입력 상한 초과) 그것을 한 줄로 센다.
   */
  private startCounting(rest: Buffer, partialLine = false): void {
    this.counting = true
    this.pending = []
    this.pendingLen = 0
    if (partialLine) this.counter.feed(Buffer.from('x'))
    this.counter.feed(rest)
  }

  private feedLine(raw: string): void {
    if (this.ended) return
    if (!this.inHunks) {
      if (raw.startsWith('@@')) {
        this.inHunks = true
      } else {
        this.feedHeader(raw)
        return
      }
    }
    if (raw.startsWith('@@')) {
      // 함수 문맥 줄 끝의 CR(CRLF 파일)이 정규식 '.' 에 걸리지 않게 먼저 뗀다
      const m = HUNK_RE.exec(raw.replace(/\r$/, ''))
      if (!m) return
      this.hunk = {
        oldStart: Number(m[1]),
        oldLines: m[2] === undefined ? 1 : Number(m[2]),
        newStart: Number(m[3]),
        newLines: m[4] === undefined ? 1 : Number(m[4]),
        header: m[5] ?? '',
        lines: []
      }
      this.o = this.hunk.oldStart
      this.n = this.hunk.newStart
      this.hunks.push(this.hunk)
      return
    }
    // 내용 줄은 항상 접두 문자(' ', '+', '-', '\\')가 있으므로 "diff " 로 시작하면 다음 파일 섹션이다
    if (raw.startsWith('diff ')) {
      this.ended = true
      return
    }
    const hunk = this.hunk
    if (!hunk) return
    const t = raw.charAt(0)
    // CRLF 파일은 줄 끝의 \r 만 떼고 나머지는 그대로 둔다
    const text = raw.endsWith('\r') ? raw.slice(1, -1) : raw.slice(1)
    let line: DiffLine
    if (t === '+') line = { t: '+', text, n: this.n++ }
    else if (t === '-') line = { t: '-', text, o: this.o++ }
    else if (t === '\\') line = { t: '\\', text: text.trim() }
    else line = { t: ' ', text, o: this.o++, n: this.n++ }
    hunk.lines.push(line)
    this.parsedLines++
  }

  private feedHeader(raw: string): void {
    if (raw.startsWith('diff ')) {
      // 두 번째 파일 섹션이 나오면 무시한다(경로 필터로 한 파일만 요청하므로 정상 흐름이 아니다)
      if (this.seenDiffHeader) this.ended = true
      this.seenDiffHeader = true
      return
    }
    if (raw.startsWith('Binary files ') || raw.startsWith('GIT binary patch')) {
      this.binary = true
      return
    }
    if (META_PREFIXES.some((p) => raw.startsWith(p))) this.meta.push(raw.replace(/\r$/, ''))
  }
}

/** 이미 메모리에 있는 diff 텍스트(Buffer)를 한 번에 파싱한다 */
export function parseDiff(buf: Buffer, path: string, oldPath?: string, opts?: DiffParseOpts): FileDiff {
  const p = new DiffParser(path, oldPath, opts)
  p.push(buf)
  return p.finish()
}
