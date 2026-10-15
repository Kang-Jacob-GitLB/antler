#!/usr/bin/env node
// git log 옵션 조합별 소요 시간 측정. 사용: node scripts/bench-log.mjs <저장소>
import { execFileSync } from 'node:child_process'

const repo = process.argv[2]
const FORMAT = '--format=%H%x1f%P%x1f%an%x1f%ae%x1f%at%x1f%ct%x1f%D%x1f%s'
const REVS = ['--branches', '--remotes', '--tags', 'HEAD']
const cases = [
  ['date-order skip0', ['--date-order', '--max-count=501']],
  ['date-order skip5000', ['--date-order', '--skip=5000', '--max-count=501']],
  ['topo-order skip0', ['--topo-order', '--max-count=501']],
  ['default skip0', ['--max-count=501']],
  ['date-order skip0 no-decorate', ['--date-order', '--max-count=501'], true],
  ['date-order all 100k', ['--date-order']]
]
for (const [name, extra, noDeco] of cases) {
  const args = ['-C', repo, '-c', 'core.quotepath=false', 'log', '-z', noDeco ? FORMAT.replace('%x1f%D', '%x1f') : FORMAT]
  if (!noDeco) args.push('--decorate=full')
  const t = performance.now()
  const out = execFileSync('git', [...args, ...extra, ...REVS, '--'], { maxBuffer: 1 << 30, env: { ...process.env, GIT_OPTIONAL_LOCKS: '0' } })
  console.log(`${name.padEnd(30)} ${(performance.now() - t).toFixed(0).padStart(6)} ms  ${(out.length / 1024).toFixed(0)} KB`)
}
