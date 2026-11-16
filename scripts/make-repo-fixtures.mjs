#!/usr/bin/env node
// 채널 패리티 테스트용 저장소 fixture 를 `git fast-import` 스트림(.fi)으로 만든다. 날짜·작성자·내용이 고정이라 해시가 결정적이다.
// TS 쪽 vitest(tests/goldenChannels.test.ts)와 Rust 통합 테스트(src-tauri/tests/channels.rs)가 같은 .fi 를 임시 디렉터리에 import 한다.
// 사용: node scripts/make-repo-fixtures.mjs  →  tests/fixtures/repos/*.fi  (바뀌면 해시가 바뀌므로 골든도 다시 만든다)
import { mkdirSync, writeFileSync } from 'node:fs'
import { join, resolve } from 'node:path'

const OUT = resolve(import.meta.dirname, '..', 'tests', 'fixtures', 'repos')
mkdirSync(OUT, { recursive: true })

const WHO = '홍길동 <dev@example.com>'
const BASE = 1_700_000_000

/** fast-import 스트림 조각을 모은다. data 의 길이는 바이트 수다 */
class Stream {
  parts = []
  raw(s) {
    this.parts.push(Buffer.from(s, 'utf8'))
  }
  data(body) {
    const b = Buffer.isBuffer(body) ? body : Buffer.from(body, 'utf8')
    this.parts.push(Buffer.from(`data ${b.length}\n`), b, Buffer.from('\n'))
  }
  blob(mark, body) {
    this.raw(`blob\nmark :${mark}\n`)
    this.data(body)
  }
  /** files: [['M', 경로, mark] | ['D', 경로]] (경로는 따옴표가 필요 없는 문자만) */
  commit({ ref = 'refs/heads/main', mark, n, msg, from, merge, files }) {
    const when = `${BASE + n * 3600} +0900`
    this.raw(`commit ${ref}\nmark :${mark}\nauthor ${WHO} ${when}\ncommitter ${WHO} ${when}\n`)
    this.data(msg)
    if (from) this.raw(`from :${from}\n`)
    if (merge) this.raw(`merge :${merge}\n`)
    for (const [op, path, m] of files) this.raw(op === 'D' ? `D ${path}\n` : `M 100644 :${m} ${path}\n`)
    this.raw('\n')
  }
  write(name) {
    writeFileSync(join(OUT, name), Buffer.concat(this.parts))
  }
}

const lines = (n, tag = 'line') => `${Array.from({ length: n }, (_, i) => `${tag} ${i + 1}`).join('\n')}\n`

// ---- basic: 루트·일반 변경(수정·추가·삭제·이름 변경·바이너리·한글 경로)·병합·빈 커밋 ----
{
  const s = new Stream()
  s.blob(1, '# 샘플\n')
  s.blob(2, lines(40))
  s.blob(3, '안녕\n')
  s.blob(4, Buffer.from([0, 1, 2, 3, 0, 255]))
  s.blob(5, lines(20, 'rename me'))
  // 1: 루트 커밋(본문 있음)
  s.commit({
    mark: 10,
    n: 1,
    msg: 'init\n\n본문 첫 줄\n본문 둘째 줄',
    files: [['M', 'README.md', 1], ['M', 'a.txt', 2], ['M', 'docs/한글 문서 이름.md', 3], ['M', 'img.bin', 4], ['M', 'old name.ts', 5]]
  })
  s.blob(6, lines(40).replace('line 2\n', 'LINE 2\n').replace('line 38\n', 'LINE 38\n'))
  s.blob(7, 'brand new\n')
  s.blob(8, lines(20, 'rename me').replace('rename me 5\n', 'changed\n'))
  s.blob(9, Buffer.from([0, 1, 2, 9, 0, 255, 7]))
  s.blob(11, '안녕\n하세요\n')
  // 2: 수정·추가·삭제·이름 변경(유사도 < 100)·바이너리 수정·한글 경로 수정
  s.commit({
    mark: 20,
    n: 2,
    from: 10,
    msg: 'feat: 여러 변경\n',
    files: [['M', 'a.txt', 6], ['M', 'new.txt', 7], ['D', 'README.md'], ['D', 'old name.ts'], ['M', 'src/new name.ts', 8], ['M', 'img.bin', 9], ['M', 'docs/한글 문서 이름.md', 11]]
  })
  s.blob(12, 'side\n')
  // 3: 1 에서 갈라진 가지
  s.commit({ ref: 'refs/heads/feature', mark: 30, n: 3, from: 10, msg: 'feature: side 파일\n', files: [['M', 'side.txt', 12]] })
  s.blob(13, 'merged\n')
  // 4: 병합(첫 부모 = 2) — 첫 부모 대비 변경만 나온다
  s.commit({ mark: 40, n: 4, from: 20, merge: 30, msg: "Merge branch 'feature'\n", files: [['M', 'side.txt', 12], ['M', 'merged.txt', 13]] })
  // 5: 빈 커밋(변경 파일 없음)
  s.commit({ mark: 50, n: 5, from: 40, msg: 'chore: 빈 커밋\n', files: [] })
  // 6: 제목만 있고 본문이 공백뿐
  s.commit({ mark: 60, n: 6, from: 50, msg: 'subject only\n\n   \n', files: [] })
  s.write('basic.fi')
}

// ---- many: 변경 파일이 상한(5000)을 넘는 루트 커밋 — truncated 와 전체 기준 합계 ----
{
  const s = new Stream()
  s.blob(1, 'x\n')
  const files = []
  for (let i = 0; i < 5001; i++) files.push(['M', `f/${String(i).padStart(4, '0')}.txt`, 1])
  s.commit({ mark: 10, n: 1, msg: 'many files\n', files })
  s.write('many.fi')
}

console.log(`wrote ${OUT}`)
