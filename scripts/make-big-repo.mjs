#!/usr/bin/env node
// 성능 검증용 대형 저장소를 git fast-import 로 빠르게 만든다.
// 사용: node scripts/make-big-repo.mjs <대상 디렉터리> [커밋 수=100000]
// main 위에 주기적으로 feature 브랜치를 갈라 merge 하고, 일부는 열린 채 남겨 레인이 여러 개 생기게 한다.
import { execFileSync, spawn } from 'node:child_process'
import { existsSync, mkdirSync } from 'node:fs'
import { resolve } from 'node:path'

const dir = resolve(process.argv[2] ?? '')
const total = Number(process.argv[3] ?? 100_000)
if (!process.argv[2]) {
  console.error('usage: node scripts/make-big-repo.mjs <dir> [commits]')
  process.exit(1)
}
if (existsSync(resolve(dir, '.git'))) {
  console.error(`already a repo: ${dir}`)
  process.exit(1)
}
mkdirSync(dir, { recursive: true })
execFileSync('git', ['init', '-q', '-b', 'main', dir])

const authors = [
  ['홍길동', 'hong@example.com'],
  ['Alice Kim', 'alice@example.com'],
  ['Bob Lee', 'bob@example.com'],
  ['이서연', 'seoyeon@example.com'],
  ['Carol Park', 'carol@example.com']
]
const verbs = ['feat', 'fix', 'refactor', 'docs', 'chore', 'perf', 'test']
const nouns = ['그래프 레인', 'parser', 'diff 뷰어', 'sidebar', 'ssh 연결', '설정 저장', 'virtual list', '테마 토큰']

const fi = spawn('git', ['-C', dir, 'fast-import', '--quiet'], { stdio: ['pipe', 'inherit', 'inherit'] })
const out = []
let size = 0
const flush = async () => {
  if (!out.length) return
  const chunk = out.join('')
  out.length = 0
  size = 0
  if (!fi.stdin.write(chunk)) await new Promise((r) => fi.stdin.once('drain', r))
}
const emit = async (s) => {
  out.push(s)
  size += s.length
  if (size > 1 << 20) await flush()
}
const data = (s) => `data ${Buffer.byteLength(s)}\n${s}\n`

let mark = 0
let ts = 1_600_000_000
const commit = async (ref, msg, from, merge) => {
  mark++
  ts += 37 + (mark % 400)
  const [name, email] = authors[mark % authors.length]
  let s = `commit ${ref}\nmark :${mark}\nauthor ${name} <${email}> ${ts} +0900\ncommitter ${name} <${email}> ${ts} +0900\n`
  s += data(msg)
  if (from) s += `from :${from}\n`
  for (const m of merge ?? []) s += `merge :${m}\n`
  s += `M 644 inline f${mark % 64}.txt\n${data(`${mark}\n`)}`
  await emit(s)
  return mark
}
const msg = (i) => `${verbs[i % verbs.length]}: ${nouns[(i * 7) % nouns.length]} 개선 #${i}`

let main = await commit('refs/heads/main', '초기 커밋')
const open = [] // 열린 채 남겨 둔 브랜치
let featureNo = 0
while (mark < total) {
  if (mark % 40 === 0) {
    // feature 브랜치: 3~8 커밋 후 merge (가끔 열린 채로 둔다)
    const n = 3 + (mark % 6)
    const name = `refs/heads/feature/f-${++featureNo}`
    let tip = await commit(name, msg(mark), main)
    for (let i = 1; i < n && mark < total; i++) {
      // 브랜치 진행 중에도 main 이 앞서 나가 레인이 나란히 달리게 한다
      if (i % 2 === 0) main = await commit('refs/heads/main', msg(mark), main)
      tip = await commit(name, msg(mark), tip)
    }
    if (featureNo % 10 === 0) open.push(name)
    else main = await commit('refs/heads/main', `Merge branch 'feature/f-${featureNo}'`, main, [tip])
  } else {
    main = await commit('refs/heads/main', msg(mark), main)
  }
}
await emit(`reset refs/tags/v1.0.0\nfrom :${Math.floor(main / 2)}\n\n`)
await flush()
fi.stdin.end()
await new Promise((r, j) => fi.on('close', (c) => (c === 0 ? r() : j(new Error(`fast-import exit ${c}`)))))
execFileSync('git', ['-C', dir, 'reset', '-q', '--hard', 'main'])
execFileSync('git', ['-C', dir, 'commit-graph', 'write', '--reachable'])
console.log(`${dir}: ${mark} commits, ${featureNo} feature branches (${open.length} open)`)
