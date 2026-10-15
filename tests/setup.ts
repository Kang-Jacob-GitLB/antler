// 모든 테스트 파일보다 먼저 실행된다. 테스트가 띄우는 git·gh(헬퍼의 execFileSync·executor 모두 process.env 를 상속)이
// 사용자의 실제 설정(~/.gitconfig, 시스템 설정, gh 로그인)·토큰을 읽거나 쓰지 못하게 한다.
// 사고: Git Credential Manager(전역 credential.helper=manager)가 루프백 테스트 서버마다 사용자의
// ~/.gitconfig 에 `[credential "http://127.0.0.1:<port>"]` 항목을 써 넣었다.
import { randomUUID } from 'node:crypto'
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

// globalSetup 이 만든 디렉터리(실행이 끝나면 통째로 지운다). 없으면(setup 만 단독으로 쓰는 경우) 자기 것을 만들고 종료 때 지운다
let home = process.env['ANTLER_TEST_HOME']
if (!home) {
  const own = mkdtempSync(join(tmpdir(), 'antler-test-home-'))
  home = own
  process.on('exit', () => rmSync(own, { recursive: true, force: true }))
}
mkdirSync(home, { recursive: true })
// 워커마다 따로 둔다 — 전역 설정에 쓰는 테스트가 다른 워커와 섞이지 않게
const worker = randomUUID()
const globalConfig = join(home, `gitconfig-${worker}`)
// 커밋을 만드는 테스트가 사용자의 전역 식별자에 기대지 않도록 둔다
writeFileSync(globalConfig, '[user]\n\tname = Antler Test\n\temail = test@antler.invalid\n')

process.env['GIT_CONFIG_GLOBAL'] = globalConfig
process.env['GIT_CONFIG_NOSYSTEM'] = '1'
process.env['GCM_INTERACTIVE'] = 'never'
process.env['GH_CONFIG_DIR'] = join(home, `gh-${worker}`)
delete process.env['GH_TOKEN']
delete process.env['GITHUB_TOKEN']
