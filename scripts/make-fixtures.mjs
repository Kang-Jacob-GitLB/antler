#!/usr/bin/env node
// 파서 테스트용 git 출력 fixture 를 실제 git 으로 생성한다.
// 한글 경로·공백·이름 변경·merge 커밋·루트 커밋·태그·리모트 브랜치 케이스를 포함한다.
// 사용: node scripts/make-fixtures.mjs  →  tests/fixtures/*.bin
import { execFileSync } from 'node:child_process'
import { mkdtempSync, mkdirSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'

const OUT = resolve(import.meta.dirname, '..', 'tests', 'fixtures')
const repo = mkdtempSync(join(tmpdir(), 'antler-fixture-'))

const env = {
  ...process.env,
  GIT_AUTHOR_NAME: '홍길동',
  GIT_AUTHOR_EMAIL: 'dev@example.com',
  GIT_COMMITTER_NAME: '홍길동',
  GIT_COMMITTER_EMAIL: 'dev@example.com',
  GIT_CONFIG_NOSYSTEM: '1',
  GIT_CONFIG_GLOBAL: join(repo, '.no-global')
}
let tick = 1_700_000_000
const git = (...args) => {
  tick += 60
  const date = `${tick} +0900`
  return execFileSync('git', ['-c', 'core.quotepath=false', ...args], {
    cwd: repo,
    env: { ...env, GIT_AUTHOR_DATE: date, GIT_COMMITTER_DATE: date }
  })
}
const write = (rel, body) => {
  const p = join(repo, rel)
  mkdirSync(join(p, '..'), { recursive: true })
  writeFileSync(p, body)
}

try {
  git('init', '-q', '-b', 'main')
  write('README.md', '# 샘플\n')
  git('add', '.')
  git('commit', '-q', '-m', '초기 커밋')
  git('tag', 'v0.1.0')

  write('문서/설계 노트.md', '한글 경로와 공백\n')
  write('src/app.ts', 'export const a = 1\n')
  git('add', '.')
  git('commit', '-q', '-m', 'feat: 한글 경로 파일 추가')

  git('checkout', '-q', '-b', 'feature/이름 변경'.replace(' ', '-'))
  git('mv', 'src/app.ts', 'src/main app.ts')
  write('src/main app.ts', 'export const a = 1\nexport const b = 2\n')
  git('add', '.')
  git('commit', '-q', '-m', 'refactor: 파일 이름 변경 (rename)')
  write('bin/logo.bin', Buffer.from([0, 1, 2, 3, 255, 0, 10]))
  git('add', '.')
  git('commit', '-q', '-m', 'chore: 바이너리 추가')

  git('checkout', '-q', 'main')
  write('README.md', '# 샘플\n\n메인 쪽 변경\n')
  git('commit', '-q', '-am', 'docs: README 수정')
  git('merge', '-q', '--no-ff', 'feature/이름-변경', '-m', "Merge branch 'feature/이름-변경'")
  git('tag', '-a', 'v0.2.0', '-m', '릴리스 0.2.0')
  git('update-ref', 'refs/remotes/origin/main', 'HEAD~1')
  git('symbolic-ref', 'refs/remotes/origin/HEAD', 'refs/remotes/origin/main')
  git('rm', '-q', 'README.md')
  git('commit', '-q', '-m', 'chore: README 삭제\n\n본문 줄 1\n본문 줄 2')

  mkdirSync(OUT, { recursive: true })
  const dump = (name, ...args) => writeFileSync(join(OUT, name), git(...args))
  dump('log.bin', 'log', '-z', '--format=%H%x1f%P%x1f%an%x1f%ae%x1f%at%x1f%ct%x1f%D%x1f%s', '--decorate=full', '--date-order', '--branches', '--remotes', '--tags', 'HEAD')
  dump('show-ref.bin', 'for-each-ref', '--format=%(refname)%00%(objectname)%00%(*objectname)%00%(HEAD)', 'refs/heads', 'refs/remotes', 'refs/tags')
  dump('name-status-merge.bin', 'show', '-z', '--format=', '--name-status', '-M', 'HEAD~1', '--first-parent')
  dump('name-status-rename.bin', 'show', '-z', '--format=', '--name-status', '-M', 'feature/이름-변경~1')
  dump('numstat-rename.bin', 'show', '-z', '--format=', '--numstat', '-M', 'feature/이름-변경~1')
  dump('numstat-binary.bin', 'show', '-z', '--format=', '--numstat', '-M', 'feature/이름-변경')
  dump('name-status-delete.bin', 'show', '-z', '--format=', '--name-status', '-M', 'HEAD')
  console.log(`fixtures written to ${OUT}`)
} finally {
  rmSync(repo, { recursive: true, force: true })
}
