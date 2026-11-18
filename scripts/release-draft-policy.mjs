#!/usr/bin/env node
// 초안 강제 이중화(.github/workflows/release.yml 의 preflight). 호출자가 draft 를 어떻게 넘기든 아래 중 하나면 초안으로 발행한다:
//   - 직전 정식 태그(vX.Y.Z)와 메이저가 다르다(메이저 첫 릴리스 — 수동 실행의 `--bump major` 강제 등으로도 공개로 나가지 않게)
//   - docs/release-notes/v<버전>.md(메이저 노트)가 있다
// 사용: node scripts/release-draft-policy.mjs --version <버전> (--last <vX.Y.Z|none> | --tags <쉼표로 이은 태그>) --notes <true|false>   → stdout `force=true|false`
import { pathToFileURL } from 'node:url'

const FORMAL = /^v(\d+)\.(\d+)\.(\d+)$/

/** 모든 태그 목록에서 `exclude`(이번 태그)를 뺀 가장 높은 정식 태그. 없으면 null */
export function lastFormalTag(tags, exclude) {
  const parsed = tags
    .filter((t) => t !== exclude)
    .map((t) => ({ t, m: FORMAL.exec(t) }))
    .filter((x) => x.m)
    .map((x) => ({ t: x.t, k: [Number(x.m[1]), Number(x.m[2]), Number(x.m[3])] }))
  parsed.sort((a, b) => b.k[0] - a.k[0] || b.k[1] - a.k[1] || b.k[2] - a.k[2])
  return parsed[0]?.t ?? null
}

/** @returns {{ force: boolean, reason: string }} */
export function mustBeDraft({ version, last, notesExist }) {
  if (notesExist) return { force: true, reason: `메이저 노트(docs/release-notes/v${version}.md)가 있다` }
  const v = /^(\d+)\./.exec(version ?? '')
  if (!v) return { force: true, reason: '버전을 읽지 못해 초안으로 둔다' }
  if (!last || last === 'none') return { force: true, reason: '직전 정식 태그가 없어 초안으로 둔다' }
  const l = FORMAL.exec(last)
  if (!l) return { force: true, reason: '직전 태그를 읽지 못해 초안으로 둔다' }
  if (Number(l[1]) !== Number(v[1])) return { force: true, reason: `메이저가 바뀐다(${last} → ${version})` }
  return { force: false, reason: '' }
}

function main() {
  const argv = process.argv.slice(2)
  const get = (n) => {
    const i = argv.indexOf(`--${n}`)
    return i >= 0 ? argv[i + 1] : undefined
  }
  const version = get('version')
  // --tags: 쉼표로 이은 태그 목록(이번 태그는 뺀다). --last 가 있으면 그것을 쓴다
  const last = get('last') ?? lastFormalTag((get('tags') ?? '').split(',').filter(Boolean), `v${version}`) ?? 'none'
  const { force, reason } = mustBeDraft({ version, last, notesExist: get('notes') === 'true' })
  console.log(`force=${force}`)
  console.log(`reason=${reason}`)
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) main()
