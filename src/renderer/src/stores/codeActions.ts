// 코드 탐색기 진입점이 함께 쓰는 동작(명령 팔레트·단축키·툴바 버튼)
import { useAppStore } from './appStore'
import { useCodeStore } from './codeStore'

/** 열려 있으면 닫고, 아니면 활성 저장소를 HEAD 기준으로 연다 */
export function toggleCodeView(): void {
  const code = useCodeStore.getState()
  if (code.repoId) return code.close()
  const repoId = useAppStore.getState().activeRepoId
  if (repoId) code.open(repoId, { ref: 'HEAD', label: 'HEAD' })
}
