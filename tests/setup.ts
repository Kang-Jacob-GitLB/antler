// 테스트가 사용자의 실제 git/gh 설정·토큰을 읽거나 쓰지 않도록 환경을 격리한다.
import { mkdtempSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

const dir = mkdtempSync(join(tmpdir(), 'antler-test-env-'))
const gitConfig = join(dir, 'gitconfig')
// 커밋을 만드는 테스트가 전역 사용자 설정에 기대지 않도록 식별자를 둔다
writeFileSync(gitConfig, '[user]\n\tname = Antler Test\n\temail = test@antler.invalid\n')
process.env['GIT_CONFIG_GLOBAL'] = gitConfig
process.env['GIT_CONFIG_NOSYSTEM'] = '1'
process.env['GCM_INTERACTIVE'] = 'never'
process.env['GH_CONFIG_DIR'] = join(dir, 'gh')
delete process.env['GH_TOKEN']
delete process.env['GITHUB_TOKEN']
