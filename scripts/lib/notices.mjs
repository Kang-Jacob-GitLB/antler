// 서드파티 라이선스 고지를 모으는 순수 함수(gen-notices.mjs·gen-tauri-notices.mjs 가 쓴다).
// 파일 시스템은 읽기만 하고, 외부 도구(cargo-about 등)는 쓰지 않는다.
import { readFileSync, readdirSync } from 'node:fs'
import { createHash } from 'node:crypto'
import { createRequire } from 'node:module'
import { dirname, join } from 'node:path'

const RULE = '='.repeat(72)
const LICENSE_FILE = /^(licen[cs]e|copying|unlicen[cs]e|notice)([._-].*)?$/i

/** 패키지 디렉터리에서 라이선스 파일(LICENSE·LICENSE-MIT·LICENSE_APACHE-2.0·COPYING·NOTICE …)을 이름순으로 모두 읽는다 */
export function readLicenseFiles(dir) {
  return readdirSync(dir, { withFileTypes: true })
    .filter((e) => e.isFile() && LICENSE_FILE.test(e.name))
    .map((e) => e.name)
    .sort()
    .map((name) => ({ name, text: readFileSync(join(dir, name), 'utf8').replace(/\r\n/g, '\n').trim() }))
    .filter((f) => f.text)
}

/** `package.json` 이 가리키는 패키지의 고지 한 절(라이선스 파일이 여럿이면 이어 붙인다) */
export function npmSection(pkg, dir) {
  const files = readLicenseFiles(dir)
  if (!files.length) throw new Error(`${pkg.name}: 라이선스 파일 없음`)
  const body = files.length === 1 ? files[0].text : files.map((f) => `--- ${f.name} ---\n${f.text}`).join('\n\n')
  return `${RULE}\n${pkg.name}@${pkg.version} — ${pkg.license}\n${RULE}\n\n${body}\n`
}

/**
 * `cargo metadata` 결과에서 실제로 배포 바이너리에 들어가는 정상(normal) 의존 크레이트만 고른다.
 * 루트(앱 자신)와 경로 의존(source 없음)은 뺀다. build·dev 의존은 따라가지 않는다(proc-macro 는 정상 의존으로 잡히므로 남는다 — 과포함 쪽이 안전하다).
 */
export function selectNormalCrates(meta) {
  const nodes = new Map(meta.resolve.nodes.map((n) => [n.id, n]))
  const byId = new Map(meta.packages.map((p) => [p.id, p]))
  const seen = new Set()
  const stack = [meta.resolve.root ?? meta.workspace_members[0]]
  while (stack.length) {
    const id = stack.pop()
    if (seen.has(id)) continue
    seen.add(id)
    for (const d of nodes.get(id)?.deps ?? []) {
      if (d.dep_kinds.some((k) => k.kind === null)) stack.push(d.pkg)
    }
  }
  const root = meta.resolve.root ?? meta.workspace_members[0]
  return [...seen]
    .filter((id) => id !== root)
    .map((id) => byId.get(id))
    .filter((p) => p && p.source)
    .sort((a, b) => (a.name === b.name ? a.version.localeCompare(b.version) : a.name < b.name ? -1 : 1))
}

/** 크레이트 한 개의 라이선스 파일. `license_file` 필드가 있으면 그것도 포함한다 */
function crateFiles(pkg) {
  const dir = dirname(pkg.manifest_path)
  const files = readLicenseFiles(dir)
  if (pkg.license_file && !files.some((f) => f.name === pkg.license_file)) {
    try {
      const text = readFileSync(join(dir, pkg.license_file), 'utf8').replace(/\r\n/g, '\n').trim()
      if (text) files.push({ name: pkg.license_file, text })
    } catch {
      // 없으면 파일 없음으로 취급한다
    }
  }
  return files
}

/**
 * Rust 크레이트의 고지: 목록(이름·버전·license 필드)과, 같은 본문은 한 번만 싣고 어느 크레이트에 적용되는지 적은 본문 모음.
 * 라이선스 파일이 없거나 license 필드가 비어도 실패하지 않고 그 사실을 목록에 적는다(`missing` 으로도 돌려준다).
 */
export function rustNotices(crates) {
  const index = []
  const missing = []
  const groups = new Map() // 본문 해시 → { files: [{name, text}], crates: [name@version] }
  for (const p of crates) {
    const id = `${p.name}@${p.version}`
    const files = crateFiles(p)
    const license = p.license ?? (p.license_file ? `(license_file: ${p.license_file})` : '(license 필드 없음)')
    index.push(`${id} — ${license}${files.length ? '' : ' [라이선스 파일 없음]'}`)
    if (!files.length) {
      missing.push(id)
      continue
    }
    const key = createHash('sha256').update(files.map((f) => f.text).join('\0')).digest('hex')
    const g = groups.get(key) ?? { files, crates: [] }
    g.crates.push(id)
    groups.set(key, g)
  }
  const bodies = [...groups.values()].map((g) => {
    const text = g.files.length === 1 ? g.files[0].text : g.files.map((f) => `--- ${f.name} ---\n${f.text}`).join('\n\n')
    return `${RULE}\n다음 크레이트에 적용(${g.crates.length}):\n${g.crates.join(', ')}\n${RULE}\n\n${text}\n`
  })
  const text =
    `${RULE}\nRust 크레이트 (${crates.length}개)\n${RULE}\n\n${index.join('\n')}\n\n` + bodies.join('\n')
  return { text, count: crates.length, missing, uniqueTexts: groups.size }
}

/** renderer 번들에 들어가는 npm 라이브러리(간접 의존은 그것을 쓰는 패키지 기준으로 푼다) */
export const BUNDLED = ['react', 'react-dom', 'scheduler', 'zustand', '@tanstack/react-virtual', '@tanstack/virtual-core', 'highlight.js', 'marked', '@tauri-apps/api']

export const NOTICE_HEADER =
  'Antler 에 포함된 서드파티 소프트웨어의 라이선스 고지입니다.\n' +
  '폰트 라이선스는 같은 폴더의 Pretendard-LICENSE.txt, NanumGothicCoding-LICENSE.txt 를 보세요.\n\n'

function pkgDir(root, name) {
  const req = createRequire(join(root, 'package.json'))
  const bases = [root, dirname(req.resolve('react-dom/package.json')), dirname(req.resolve('@tanstack/react-virtual/package.json'))]
  for (const b of bases) {
    try {
      return dirname(createRequire(join(b, 'package.json')).resolve(`${name}/package.json`))
    } catch {
      // 다음 기준으로
    }
  }
  throw new Error(`패키지를 찾을 수 없음: ${name}`)
}

export function npmNotices(root, names = BUNDLED) {
  return names.map((name) => {
    const dir = pkgDir(root, name)
    return npmSection(JSON.parse(readFileSync(join(dir, 'package.json'), 'utf8')), dir)
  })
}
