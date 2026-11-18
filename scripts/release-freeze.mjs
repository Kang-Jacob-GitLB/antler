#!/usr/bin/env node
// 야간 릴리스의 "공개 대기" 가드(.github/workflows/nightly-release.yml 의 plan 잡). 마지막 정식 태그의 릴리스가 **초안**이면 새 릴리스를 만들지 않는다.
//   이유: 메이저(v2.0.0)는 초안으로 발행해 사람이 첨부를 확인한 뒤 공개한다. 초안인 동안 태그는 이미 push 돼 있어, 그 사이 `fix:` 하나가 머지되면
//   다음 야간이 v2.0.1 을 **정식으로** 발행한다 — 그러면 v2.0.1 이 `releases/latest` 가 돼 사람의 확인도 이관 노트도 없이 이관이 시작된다.
// 닫힌 쪽(fail-closed)으로 판정한다: 초안·릴리스 없음·조회 오류·알 수 없는 응답은 모두 "발행하지 않음"이다. (태그로 조회하는 API 는 초안을 돌려주지 않아
//   초안은 보통 "없음"으로 보인다. 읽기 전용 토큰이 초안을 볼 수 있는지 확인하지 못했으므로 어느 쪽이든 막는다.)
// 사용: node scripts/release-freeze.mjs --last <vX.Y.Z|none> --rc <gh 종료 코드> --out <gh 출력(JSON 또는 오류 문구)>
//   stdout 마지막 줄들: `frozen=true|false` 와 `reason=<문구>` (GITHUB_OUTPUT 에 이어 붙이면 된다)
import { pathToFileURL } from 'node:url'

/**
 * @param {{ last: string|null|undefined, rc: number, out: string }} input
 * @returns {{ frozen: boolean, reason: string }}
 */
export function decideFreeze({ last, rc, out }) {
  // 기준 태그가 없으면(첫 릴리스 전) 막을 초안도 없다. next-version.mjs 는 태그가 없으면 어차피 실패한다
  if (!last || last === 'none') return { frozen: false, reason: '기준 태그 없음' }
  if (rc !== 0) return { frozen: true, reason: `${last} 의 릴리스를 확인하지 못해(없음 또는 조회 오류 — 초안 포함) 발행하지 않습니다` }
  let doc
  try {
    doc = JSON.parse(out)
  } catch {
    return { frozen: true, reason: `${last} 릴리스 조회 결과를 읽지 못해 발행하지 않습니다` }
  }
  if (doc === null || typeof doc !== 'object' || typeof doc.isDraft !== 'boolean') return { frozen: true, reason: `${last} 릴리스 조회 결과에 isDraft 가 없어 발행하지 않습니다` }
  if (doc.isDraft) return { frozen: true, reason: `${last} 릴리스가 초안입니다 — 사람이 공개하기 전에는 다음 릴리스를 만들지 않습니다` }
  return { frozen: false, reason: `${last} 릴리스는 공개돼 있습니다` }
}

function main() {
  const argv = process.argv.slice(2)
  const get = (name) => {
    const i = argv.indexOf(`--${name}`)
    return i >= 0 ? argv[i + 1] : undefined
  }
  const rcText = get('rc')
  const rc = rcText !== undefined && /^\d+$/.test(rcText) ? Number(rcText) : 1
  const { frozen, reason } = decideFreeze({ last: get('last'), rc, out: get('out') ?? '' })
  console.log(`frozen=${frozen}`)
  console.log(`reason=${reason}`)
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) main()
