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
// 링크드 작업 트리는 저장소 밖에 만든다(한글 + 공백 경로 포함)
const wtRoot = mkdtempSync(join(tmpdir(), 'antler-fixture-wt-'))

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
  dump('name-status-merge.bin', 'show', '-z', '--format=', '--name-status', '-M', 'HEAD~1', '--first-parent')
  dump('name-status-rename.bin', 'show', '-z', '--format=', '--name-status', '-M', 'feature/이름-변경~1')
  dump('numstat-rename.bin', 'show', '-z', '--format=', '--numstat', '-M', 'feature/이름-변경~1')
  dump('numstat-binary.bin', 'show', '-z', '--format=', '--numstat', '-M', 'feature/이름-변경')
  dump('name-status-delete.bin', 'show', '-z', '--format=', '--name-status', '-M', 'HEAD')

  // ---- 여기부터는 refs/worktree/stash fixture 용 추가 구성. log.bin 은 위에서 이미 덤프했다 ----
  git('tag', 'v0.10.0') // lightweight (자연 정렬 확인용: v0.2.0 < v0.10.0)
  git('tag', '-a', 'v0.9.0', '-m', '릴리스 0.9.0')

  // 리모트: 이름에 '/' 가 들어간 것 포함. fetch 없이 refs 만 직접 만든다
  git('remote', 'add', 'origin', 'https://example.com/origin.git')
  git('remote', 'add', 'team/upstream', 'https://example.com/team.git')
  git('update-ref', 'refs/remotes/team/upstream/main', 'HEAD')
  git('update-ref', 'refs/remotes/team/upstream/feature/x', 'HEAD~2')

  // 업스트림: main 은 ahead 2, behind-br 는 behind 만, diverged 는 ahead 1 + behind
  git('branch', '--set-upstream-to=origin/main', 'main')
  git('branch', 'behind-br', 'HEAD~3')
  git('branch', '--set-upstream-to=origin/main', 'behind-br')
  git('branch', 'diverged', 'HEAD~2')
  git('update-ref', 'refs/remotes/origin/diverged', 'HEAD~1')
  git('branch', '--set-upstream-to=origin/diverged', 'diverged')
  git('checkout', '-q', 'diverged')
  git('commit', '-q', '--allow-empty', '-m', 'chore: diverged 전용 커밋')
  git('checkout', '-q', 'main')
  // gone: 업스트림이 설정됐지만 리모트 추적 ref 가 없다
  git('branch', 'gone-br', 'HEAD~1')
  git('config', 'branch.gone-br.remote', 'origin')
  git('config', 'branch.gone-br.merge', 'refs/heads/gone-br')

  // 작업 트리: 한글 + 공백 경로, detached
  git('worktree', 'add', '-q', join(wtRoot, '작업 트리'), '-b', 'wt-branch')
  git('worktree', 'add', '-q', '--detach', join(wtRoot, 'detached-wt'), 'HEAD~1')

  // 스태시 2개(하나는 한글 메시지)
  write('src/main app.ts', 'export const a = 1\nexport const b = 3\n')
  git('stash', 'push', '-q', '-m', '임시 저장: 한글 메시지')
  write('src/main app.ts', 'export const a = 1\nexport const b = 4\n')
  git('stash', 'push', '-q', '-m', 'second stash')

  // 트리를 가리키는 태그(linux.git 의 v2.6.11-tree 같은 경우)와, 이름이 /HEAD 로 끝나는 실제 리모트 브랜치
  git('tag', 'tree-tag', 'HEAD^{tree}')
  git('update-ref', 'refs/remotes/origin/team/HEAD', 'HEAD')

  dump('for-each-ref.bin', 'for-each-ref', '--format=%(refname)%00%(objectname)%00%(*objectname)%00%(HEAD)%00%(upstream:short)%00%(upstream:track,nobracket)%00%(symref)%00%(objecttype)%00%(*objecttype)%00%(creatordate:unix)', 'refs/heads', 'refs/remotes', 'refs/tags')
  dump('remotes.txt', 'remote')
  dump('worktree.bin', 'worktree', 'list', '--porcelain', '-z')
  dump('stash.bin', 'stash', 'list', '-z', '--format=%H%x1f%gd%x1f%ct%x1f%gs')
  // M3: diff-tree 기반 변경 목록 fixture. 한글 + 공백 경로(문서/설계 노트.md)와 루트 커밋(--root)
  const koreanCommit = git('log', '--format=%H', '--grep=한글 경로 파일 추가', 'main').toString('utf8').trim()
  const rootCommit = git('rev-parse', 'v0.1.0').toString('utf8').trim()
  dump('dt-name-status-korean.bin', 'diff-tree', '-r', '-z', '-M', '--name-status', '--no-commit-id', rootCommit, koreanCommit)
  dump('dt-numstat-korean.bin', 'diff-tree', '-r', '-z', '-M', '--numstat', '--no-commit-id', rootCommit, koreanCommit)
  dump('dt-name-status-root.bin', 'diff-tree', '-r', '-z', '-M', '--name-status', '--no-commit-id', '--root', rootCommit)
  console.log(`fixtures written to ${OUT}`)
} finally {
  rmSync(repo, { recursive: true, force: true })
  rmSync(wtRoot, { recursive: true, force: true })
}
