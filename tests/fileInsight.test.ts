import { execFileSync } from 'node:child_process'
import { existsSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterAll, afterEach, beforeAll, describe, expect, it } from 'vitest'
import { parseBlame } from '../src/main/git/parsers/blame'
import { parseFileHistory } from '../src/main/git/parsers/fileHistory'
import { ConfigStore } from '../src/main/services/ConfigStore'
import { RepoService } from '../src/main/services/RepoService'

const H1 = 'a'.repeat(40)
const H2 = 'b'.repeat(40)

describe('parseBlame', () => {
  const porcelain = [
    `${H1} 1 1 2`,
    'author 홍길동',
    'author-mail <dev@example.com>',
    'author-time 1700000000',
    'author-tz +0900',
    'summary 첫 커밋',
    'boundary',
    'filename f.txt',
    '\talpha',
    `${H1} 2 2`,
    '\tbeta',
    `${H2} 3 3 1`,
    'author 김철수',
    'author-mail <kim@example.com>',
    'author-time 1700000100',
    'summary 수정',
    'previous ' + H1 + ' f.txt',
    'filename f.txt',
    '\t\tindented\r',
    `${H1} 3 4 1`,
    '\t'
  ].join('\n') + '\n'

  it('줄과 커밋 메타데이터를 읽고 같은 커밋의 연속 줄을 묶는다', () => {
    const r = parseBlame(Buffer.from(porcelain))
    expect(r.lines).toEqual(['alpha', 'beta', '\tindented\r', ''])
    expect(r.groups).toEqual([
      { hash: H1, start: 0, count: 2 },
      { hash: H2, start: 2, count: 1 },
      { hash: H1, start: 3, count: 1 }
    ])
    expect(r.commits[H1]).toMatchObject({ authorName: '홍길동', authorEmail: 'dev@example.com', authorDate: 1700000000, summary: '첫 커밋', boundary: true })
    expect(r.commits[H2]).toMatchObject({ authorName: '김철수', summary: '수정', boundary: false })
    expect(r.binary).toBe(false)
  })

  it('줄에 NUL 이 있으면 바이너리로 표시한다', () => {
    const r = parseBlame(Buffer.from(`${H1} 1 1 1\nauthor x\nsummary s\nfilename f\n\ta\0b\n`))
    expect(r.binary).toBe(true)
  })

  it('빈 출력은 빈 결과', () => {
    expect(parseBlame(Buffer.alloc(0))).toEqual({ commits: {}, groups: [], lines: [], binary: false })
  })
})

describe('parseFileHistory', () => {
  const rec = (hash: string, subject: string, status: string[]) =>
    `\x01${hash}\x1f${H2}\x1f홍길동\x1fdev@example.com\x1f1700000000\x1f${subject}\0\n${status.join('\0')}\0`

  it('상태·경로와 이름 변경(옛 경로)을 읽는다', () => {
    const buf = Buffer.from(rec(H1, '수정', ['M', 'g.txt']) + rec(H2, '이름 변경', ['R100', 'f.txt', 'g.txt']) + rec('c'.repeat(40), '추가', ['A', 'f.txt']))
    const r = parseFileHistory(buf, 'g.txt')
    expect(r.map((e) => [e.status, e.path, e.oldPath])).toEqual([
      ['M', 'g.txt', undefined],
      ['R', 'g.txt', 'f.txt'],
      ['A', 'f.txt', undefined]
    ])
    expect(r[0]).toMatchObject({ hash: H1, parents: [H2], authorName: '홍길동', authorEmail: 'dev@example.com', authorDate: 1700000000, subject: '수정' })
  })

  it('상태 줄이 없는 레코드는 요청 경로로 채운다', () => {
    const r = parseFileHistory(Buffer.from(`\x01${H1}\x1f\x1fa\x1fa@a\x1f1\x1fmsg\0`), 'x.txt')
    expect(r).toHaveLength(1)
    expect(r[0]).toMatchObject({ parents: [], path: 'x.txt', status: 'M' })
  })

  it('제목에 SOH 가 섞여도 해시 모양이 아니면 경계로 보지 않는다', () => {
    const r = parseFileHistory(Buffer.from(rec(H1, '앞\x01뒤', ['M', 'a.txt'])), 'a.txt')
    expect(r).toHaveLength(1)
    expect(r[0]!.subject).toBe('앞\x01뒤')
  })
})

describe('RepoService.fileHistory / blame (실제 저장소)', { timeout: 60_000 }, () => {
  let root: string
  let repo: string
  const h: Record<string, string> = {}
  const env = () => ({ ...process.env, GIT_AUTHOR_NAME: '홍길동', GIT_AUTHOR_EMAIL: 'a@x.io', GIT_COMMITTER_NAME: '홍길동', GIT_COMMITTER_EMAIL: 'a@x.io' })
  const git = (...args: string[]) => execFileSync('git', ['-c', 'core.autocrlf=false', ...args], { cwd: repo, env: env() }).toString('utf8').trim()
  const commit = (name: string, msg: string) => {
    git('add', '-A')
    git('commit', '-q', '-m', msg)
    h[name] = git('rev-parse', 'HEAD')
  }

  beforeAll(() => {
    root = mkdtempSync(join(tmpdir(), 'antler-fi-'))
    repo = join(root, 'repo')
    mkdirSync(repo)
    git('init', '-q', '-b', 'main')
    writeFileSync(join(repo, 'f.txt'), 'one\ntwo\nthree\n')
    writeFileSync(join(repo, 'other.txt'), 'x\n')
    commit('c1', '추가')
    git('mv', 'f.txt', 'g.txt')
    commit('c2', '이름 변경')
    writeFileSync(join(repo, 'g.txt'), 'one\nTWO\nthree\n')
    commit('c3', '둘째 줄 수정')
    writeFileSync(join(repo, 'other.txt'), 'y\n')
    commit('c4', '다른 파일')
    writeFileSync(join(repo, 'g.txt'), 'one\nTWO\nthree\nfour\n')
    commit('c5', '넷째 줄 추가')
    writeFileSync(join(repo, 'bin.dat'), Buffer.from([0, 1, 2, 0, 255]))
    commit('c6', '바이너리')
    writeFileSync(join(repo, 'big.txt'), 'x'.repeat(600 * 1024) + '\n')
    commit('c7', '큰 파일')
  })
  afterAll(() => rmSync(root, { recursive: true, force: true }))

  let svc: RepoService | null = null
  afterEach(async () => {
    await svc?.dispose()
    svc = null
  })
  function setup(): { svc: RepoService; repoId: string } {
    const config = new ConfigStore(join(mkdtempSync(join(root, 'cfg-')), 'antler.config.json'))
    const r = config.addRepo({ sourceId: 'local', path: repo, name: 'repo' })
    svc = new RepoService(config)
    return { svc, repoId: r.id }
  }

  it('이름 변경을 따라가며 커밋마다 그 시점의 경로를 돌려준다', async () => {
    const { svc, repoId } = setup()
    const page = await svc.fileHistory(repoId, { path: 'g.txt', skip: 0, count: 50 })
    expect(page.hasMore).toBe(false)
    expect(page.entries.map((e) => [e.hash, e.status, e.path, e.oldPath])).toEqual([
      [h.c5, 'M', 'g.txt', undefined],
      [h.c3, 'M', 'g.txt', undefined],
      [h.c2, 'R', 'g.txt', 'f.txt'],
      [h.c1, 'A', 'f.txt', undefined]
    ])
    expect(page.entries[0]).toMatchObject({ authorName: '홍길동', subject: '넷째 줄 추가' })
  })

  it('페이지 단위로 읽고 hasMore 를 알린다', async () => {
    const { svc, repoId } = setup()
    const p1 = await svc.fileHistory(repoId, { path: 'g.txt', skip: 0, count: 3 })
    expect(p1.entries).toHaveLength(3)
    expect(p1.hasMore).toBe(true)
    const p2 = await svc.fileHistory(repoId, { path: 'g.txt', skip: 3, count: 3 })
    expect(p2.entries.map((e) => e.hash)).toEqual([h.c1])
    expect(p2.hasMore).toBe(false)
  })

  it('rev 를 주면 그 커밋부터 거슬러 올라간다', async () => {
    const { svc, repoId } = setup()
    const page = await svc.fileHistory(repoId, { path: 'g.txt', rev: h.c3!, skip: 0, count: 10 })
    expect(page.entries.map((e) => e.hash)).toEqual([h.c3, h.c2, h.c1])
  })

  it('rev·경로를 검증하고 옵션처럼 보이는 경로도 경로로만 쓴다', async () => {
    const { svc, repoId } = setup()
    await expect(svc.fileHistory(repoId, { path: 'g.txt', rev: '--all', skip: 0, count: 5 })).rejects.toThrow(/invalid/)
    await expect(svc.fileHistory(repoId, { path: '', skip: 0, count: 5 })).rejects.toThrow(/invalid/)
    const page = await svc.fileHistory(repoId, { path: '--output=pwned.txt', skip: 0, count: 5 })
    expect(page.entries).toEqual([])
    expect(existsSync(join(repo, 'pwned.txt'))).toBe(false)
  })

  it('요청을 취소할 수 있다', async () => {
    const { svc, repoId } = setup()
    const p = svc.fileHistory(repoId, { path: 'g.txt', skip: 0, count: 5 }, 'req-fh')
    svc.cancel('req-fh')
    await expect(p).rejects.toThrow()
  })

  it('blame: 줄마다 마지막으로 바꾼 커밋과 작성자를 돌려준다', async () => {
    const { svc, repoId } = setup()
    const b = await svc.blame(repoId, { path: 'g.txt' })
    expect(b.rev).toBe(h.c7)
    expect(b.lines).toEqual(['one', 'TWO', 'three', 'four'])
    // 'one' 은 이름 변경 전 최초 커밋, 'three' 도 마찬가지
    expect(b.groups.map((g) => [g.hash, g.start, g.count])).toEqual([
      [h.c1, 0, 1],
      [h.c3, 1, 1],
      [h.c1, 2, 1],
      [h.c5, 3, 1]
    ])
    expect(b.commits[h.c3!]).toMatchObject({ authorName: '홍길동', authorEmail: 'a@x.io', summary: '둘째 줄 수정' })
    expect(b.commits[h.c1!]!.boundary).toBe(true)
  })

  it('blame: 지정한 커밋 시점의 파일을 본다', async () => {
    const { svc, repoId } = setup()
    const b = await svc.blame(repoId, { path: 'g.txt', rev: h.c2! })
    expect(b.rev).toBe(h.c2)
    expect(b.lines).toEqual(['one', 'two', 'three'])
    expect(b.groups).toEqual([{ hash: h.c1, start: 0, count: 3 }])
  })

  it('blame: 바이너리·너무 큰 파일은 줄 없이 표시만 한다', async () => {
    const { svc, repoId } = setup()
    const bin = await svc.blame(repoId, { path: 'bin.dat' })
    expect(bin).toMatchObject({ binary: true, tooLarge: false, lines: [], groups: [] })
    const big = await svc.blame(repoId, { path: 'big.txt' })
    expect(big).toMatchObject({ binary: false, tooLarge: true, lines: [] })
    expect(big.size).toBeGreaterThan(512 * 1024)
  })

  it('blame: 그 시점에 없는 파일은 missing 으로, 잘못된 rev 는 거부한다', async () => {
    const { svc, repoId } = setup()
    expect(await svc.blame(repoId, { path: 'nope.txt' })).toMatchObject({ missing: true, lines: [] })
    expect(await svc.blame(repoId, { path: 'g.txt', rev: h.c1! })).toMatchObject({ missing: true })
    await expect(svc.blame(repoId, { path: 'g.txt', rev: 'HEAD' })).rejects.toThrow(/invalid/)
  })

  it('히스토리: HEAD 기준이면 해석한 HEAD 해시를 돌려주고, 끝을 넘긴 페이지는 비어 hasMore=false', async () => {
    const { svc, repoId } = setup()
    const p = await svc.fileHistory(repoId, { path: 'g.txt', skip: 0, count: 2 })
    expect(p.rev).toBe(h.c7)
    const beyond = await svc.fileHistory(repoId, { path: 'g.txt', skip: 10, count: 2 })
    expect(beyond).toMatchObject({ entries: [], hasMore: false })
    const capped = await svc.fileHistory(repoId, { path: 'g.txt', skip: 1_000_000, count: 2 })
    expect(capped).toEqual({ entries: [], hasMore: false })
  })

  it('blame: 저장소 설정의 textconv 명령을 실행하지 않는다(드라이버 이름에 = 가 있어도)', async () => {
    for (const driver of ['evil', 'a=b']) {
      const marker = join(root, `textconv-ran-${driver.replace('=', '_')}`)
      writeFileSync(join(repo, '.git', 'info', 'attributes'), `*.txt diff=${driver}
`)
      git('config', `diff.${driver}.textconv`, `sh -c "echo x > '${marker.replace(/\\/g, '/')}'; cat" --`)
      try {
        const { svc, repoId } = setup()
        const b = await svc.blame(repoId, { path: 'g.txt' })
        expect(b.lines).toEqual(['one', 'TWO', 'three', 'four'])
        expect(existsSync(marker)).toBe(false)
      } finally {
        git('config', '--unset', `diff.${driver}.textconv`)
        rmSync(join(repo, '.git', 'info', 'attributes'), { force: true })
      }
    }
  })

  it('blame: 전역 blame.ignoreRevsFile 이 없는 파일을 가리켜도 동작한다', async () => {
    const prev = process.env['GIT_CONFIG_GLOBAL']
    const cfg = join(root, 'global-ignore-revs')
    const missing = join(root, 'does-not-exist').replace(/\\/g, '/')
    writeFileSync(cfg, `[user]
	name = t
	email = t@t
[blame]
	ignoreRevsFile = ${missing}
`)
    process.env['GIT_CONFIG_GLOBAL'] = cfg
    try {
      const { svc, repoId } = setup()
      const b = await svc.blame(repoId, { path: 'g.txt' })
      expect(b.lines).toEqual(['one', 'TWO', 'three', 'four'])
    } finally {
      if (prev === undefined) delete process.env['GIT_CONFIG_GLOBAL']
      else process.env['GIT_CONFIG_GLOBAL'] = prev
    }
  })
})
