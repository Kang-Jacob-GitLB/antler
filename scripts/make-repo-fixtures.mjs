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
  /** 이미 만든 커밋(mark)에 ref 를 붙인다(가벼운 태그·리모트 추적 ref) */
  reset(ref, from) {
    this.raw(`reset ${ref}\nfrom :${from}\n\n`)
  }
  /** 주석 태그 */
  tag(name, from, n, msg) {
    this.raw(`tag ${name}\nfrom :${from}\ntagger ${WHO} ${BASE + n * 3600} +0900\n`)
    this.data(msg)
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

// ---- branchy: git:log 시나리오용 — 병합·가지·태그·리모트 추적 ref 가 여럿이고 date-order 에서 가지가 섞인다 ----
{
  const s = new Stream()
  s.blob(1, 'x\n')
  const f = [['M', 'a.txt', 1]]
  // n(시각) 순서가 곧 --date-order 의 순서다. 같은 n 은 쓰지 않는다
  s.commit({ mark: 10, n: 1, msg: 'main 1: root\n', files: f })
  s.commit({ mark: 11, n: 2, from: 10, msg: 'main 2\n', files: [] })
  s.commit({ mark: 12, n: 3, from: 11, msg: 'main 3\n', files: [] })
  s.commit({ ref: 'refs/heads/feature/a', mark: 20, n: 4, from: 11, msg: 'feature/a 1\n', files: [] })
  s.commit({ mark: 13, n: 5, from: 12, msg: 'main 4\n', files: [] })
  s.commit({ ref: 'refs/heads/feature/a', mark: 21, n: 6, from: 20, msg: 'feature/a 2\n', files: [] })
  s.commit({ mark: 14, n: 7, from: 13, merge: 21, msg: "Merge branch 'feature/a'\n", files: [] })
  s.commit({ ref: 'refs/heads/dev', mark: 30, n: 8, from: 14, msg: 'dev 1\n', files: [] })
  s.commit({ ref: 'refs/heads/dev', mark: 31, n: 9, from: 30, msg: 'dev 2\n', files: [] })
  s.commit({ mark: 15, n: 10, from: 14, msg: 'main 5\n', files: [] })
  s.commit({ ref: 'refs/heads/release/1.0', mark: 40, n: 11, from: 13, msg: 'release 1\n', files: [] })
  s.commit({ mark: 16, n: 12, from: 15, msg: 'main 6\n', files: [] })
  s.commit({ ref: 'refs/heads/dev', mark: 32, n: 13, from: 31, msg: 'dev 3\n', files: [] })
  s.commit({ mark: 17, n: 14, from: 16, merge: 32, msg: "Merge branch 'dev'\n", files: [] })
  s.commit({ ref: 'refs/heads/release/1.0', mark: 41, n: 15, from: 40, msg: 'release 2\n', files: [] })
  s.commit({ mark: 18, n: 16, from: 17, msg: 'main 7\n', files: [] })
  s.commit({ mark: 19, n: 17, from: 18, msg: 'main 8\n', files: [] })
  s.commit({ mark: 50, n: 18, from: 19, msg: 'main 9\n', files: [] })
  s.commit({ mark: 51, n: 19, from: 50, msg: 'main 10\n', files: [] })
  s.commit({ mark: 52, n: 20, from: 51, msg: 'main 11\n', files: [] })
  // 어느 가지에도 합쳐지지 않은 가장 최신 가지 — 지우면 앞쪽 순서가 바뀐다(STALE 시나리오)
  s.commit({ ref: 'refs/heads/topic', mark: 60, n: 21, from: 52, msg: 'topic 1\n', files: [] })
  s.commit({ ref: 'refs/heads/topic', mark: 61, n: 22, from: 60, msg: 'topic 2\n', files: [] })
  s.commit({ ref: 'refs/heads/topic', mark: 62, n: 23, from: 61, msg: 'topic 3\n', files: [] })
  s.reset('refs/tags/v1.0', 13)
  s.tag('v2.0', 18, 16, 'release 2.0\n')
  s.reset('refs/remotes/origin/main', 15)
  s.reset('refs/remotes/origin/dev', 31)
  s.reset('refs/remotes/team/alice/work', 21)
  s.write('branchy.fi')
}

// ---- files: 파일 단위 채널(compare·fileDiff·fileHistory·blame)용 — 이름 변경 이력·삭제·바이너리·3000줄을 넘는 diff·병합·작성자가 둘·glob 문자·옵션처럼 보이는 파일 이름·blame 상한을 넘는 파일 ----
{
  const s = new Stream()
  const ALICE = 'Alice <alice@example.com>'
  const nums = (n, tag) => `${Array.from({ length: n }, (_, i) => `${tag} ${i + 1}`).join('\n')}\n`
  s.blob(1, lines(30, 'alpha'))
  s.blob(2, lines(25, 'old'))
  s.blob(3, Buffer.from([0, 1, 2, 3, 0, 255]))
  s.blob(4, '공백 경로\n')
  // 1: 루트 커밋
  s.commit({ mark: 10, n: 1, msg: 'init\n', files: [['M', 'a.txt', 1], ['M', 'dir/old.txt', 2], ['M', 'bin.dat', 3], ['M', 'sp ace/한글 파일.txt', 4], ['M', 'b.txt', 1]] })
  s.blob(5, lines(30, 'alpha').replace('alpha 3\n', 'ALICE 3\n').replace('alpha 28\n', 'ALICE 28\n'))
  // 2: 다른 작성자가 a.txt 의 두 줄을 고친다(blame 의 작성자 구분)
  s.commit({ mark: 20, n: 2, from: 10, who: ALICE, msg: 'fix: alice edits\n', files: [['M', 'a.txt', 5]] })
  s.blob(6, lines(25, 'old').replace('old 10\n', 'new 10\n'))
  // 3: dir/old.txt → dir/new.txt (이름 변경 + 한 줄 수정)
  s.commit({ mark: 30, n: 3, from: 20, msg: 'rename old to new\n', files: [['D', 'dir/old.txt'], ['M', 'dir/new.txt', 6]] })
  s.blob(7, nums(4000, 'row'))
  // 4: 바이너리 삭제 + 4000줄 파일 추가(diff 상한 3000줄 초과)
  s.commit({ mark: 40, n: 4, from: 30, msg: 'big file\n', files: [['D', 'bin.dat'], ['M', 'big.txt', 7]] })
  s.blob(8, nums(4000, 'ROW'))
  // 5: 4000줄을 전부 바꾼다(삭제 4000 + 추가 4000 = 8000줄)
  s.commit({ mark: 50, n: 5, from: 40, msg: 'rewrite big\n', files: [['M', 'big.txt', 8]] })
  s.blob(9, 'side\n')
  // 6: 2 에서 갈라진 가지
  s.commit({ ref: 'refs/heads/side', mark: 60, n: 6, from: 20, who: ALICE, msg: 'side branch\n', files: [['M', 'side.txt', 9]] })
  s.blob(10, 'glob\n')
  s.blob(11, 'opt\n')
  s.blob(12, lines(30, 'alpha').replace('alpha 15\n', 'BOB 15\n'))
  // 7: 병합(첫 부모 = 5) + a.txt 한 줄 수정 + 옵션·glob 문자처럼 보이는 이름의 파일
  s.commit({
    mark: 70,
    n: 7,
    from: 50,
    merge: 60,
    msg: "Merge branch 'side'\n",
    files: [['M', 'side.txt', 9], ['M', 'a.txt', 12], ['M', '[ab].txt', 10], ['M', '--output=pwned.txt', 11]]
  })
  // 8: blame 상한(512KiB)을 넘는 파일
  s.blob(13, `${Array.from({ length: 11000 }, (_, i) => `line ${String(i).padStart(5, '0')} ${'x'.repeat(36)}`).join('\n')}\n`)
  s.commit({ mark: 80, n: 8, from: 70, msg: 'large file\n', files: [['M', 'large.txt', 13]] })
  s.write('files.fi')
}

// ---- empty: 커밋이 하나도 없는 저장소(빈 스트림) ----
writeFileSync(join(OUT, 'empty.fi'), '')

console.log(`wrote ${OUT}`)
