// 테스트 실행 한 번에 임시 홈 디렉터리 하나를 만들고(워커들이 setup.ts 에서 이 안에 전역 git 설정 파일을 둔다) 끝나면 지운다.
import { mkdtempSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

export default function setup(): () => void {
  const dir = mkdtempSync(join(tmpdir(), 'antler-test-home-'))
  process.env['ANTLER_TEST_HOME'] = dir
  return () => rmSync(dir, { recursive: true, force: true })
}
