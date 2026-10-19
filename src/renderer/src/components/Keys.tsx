import { platformKey } from '../lib/shortcuts'
import './CommandPalette.css'

/** 키캡 표시. 맥은 ⌘⌥⇧ 기호를 붙여 쓰고, 그 외는 키마다 칸을 나눈다 */
export function Keys({ parts, mac }: { parts: string[]; mac: boolean }): React.JSX.Element {
  return (
    <span className="keys">
      {parts.map((p, i) => (
        <kbd key={i} className="keys__cap">
          {platformKey(p, mac)}
        </kbd>
      ))}
    </span>
  )
}
