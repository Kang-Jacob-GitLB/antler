// 루트 PID 의 자손 프로세스 트리 메모리를 이미지 이름별로 합산한다(Windows 전용, PowerShell Get-CimInstance).
// 같은 함수로 Electron·Tauri 를 재서 두 판의 수치를 같은 기준으로 비교한다.
import { execFileSync } from 'node:child_process'

/** 전체 프로세스 목록(PID·부모·이미지·커맨드라인·메모리 바이트) */
export function listProcesses() {
  const ps =
    '[Console]::OutputEncoding=[Text.Encoding]::UTF8;' +
    'Get-CimInstance Win32_Process | Select-Object ProcessId,ParentProcessId,Name,CommandLine,PrivatePageCount,WorkingSetSize | ConvertTo-Json -Compress'
  const out = execFileSync('powershell', ['-NoProfile', '-NonInteractive', '-Command', ps], { encoding: 'utf8', maxBuffer: 64 * 1024 * 1024 })
  const list = JSON.parse(out)
  return Array.isArray(list) ? list : [list]
}

/** userData 아래를 --user-data-dir 로 쓰는 msedgewebview2 만 고른다(다른 WebView2 앱·Edge 를 섞지 않는다) */
export function webviewProcsOf(all, userData) {
  const needle = userData.toLowerCase().replaceAll('/', '\\')
  return all.filter((p) => /^msedgewebview2\.exe$/i.test(p.Name) && (p.CommandLine ?? '').toLowerCase().replaceAll('/', '\\').includes(needle))
}

/** rootPid 의 자손(자기 자신 포함) */
function descendants(all, rootPid) {
  const kids = new Map()
  for (const p of all) {
    if (!kids.has(p.ParentProcessId)) kids.set(p.ParentProcessId, [])
    kids.get(p.ParentProcessId).push(p)
  }
  const byPid = new Map(all.map((p) => [p.ProcessId, p]))
  const out = []
  const seen = new Set()
  const walk = (pid) => {
    if (seen.has(pid)) return
    seen.add(pid)
    const self = byPid.get(pid)
    if (self) out.push(self)
    for (const k of kids.get(pid) ?? []) walk(k.ProcessId)
  }
  walk(rootPid)
  return out
}

const mb = (b) => Math.round(b / 1024 / 1024)

/**
 * 루트 PID 트리의 메모리를 그룹별로 낸다. userData 를 주면 그 userData 를 쓰는 msedgewebview2 도 합산한다
 * (트리 밖에 있을 때만 더해지며, 트리 안인지 여부는 webviewInTree 로 알린다).
 * 그룹: app(루트와 같은 이미지) / webview(msedgewebview2) / node / other. 값은 MB(WorkingSet, PrivatePageCount).
 */
export function measureTree({ rootPid, userData }) {
  const all = listProcesses()
  const tree = descendants(all, rootPid)
  // Electron 은 cmd.exe 래퍼가 루트일 수 있어(playwright 가 돌려주는 pid) 그 경우 첫 자식의 이미지를 앱으로 본다
  let rootName = tree.find((p) => p.ProcessId === rootPid)?.Name?.toLowerCase()
  if (rootName === 'cmd.exe') rootName = tree.find((p) => p.ParentProcessId === rootPid)?.Name?.toLowerCase() ?? rootName
  const inTree = new Set(tree.map((p) => p.ProcessId))
  let webviewInTree = false
  const procs = [...tree]
  if (userData) {
    const wv = webviewProcsOf(all, userData)
    webviewInTree = wv.length > 0 && wv.every((p) => inTree.has(p.ProcessId))
    for (const p of wv) if (!inTree.has(p.ProcessId)) procs.push(p)
  }
  const groups = { app: [], webview: [], node: [], other: [] }
  for (const p of procs) {
    const n = (p.Name ?? '').toLowerCase()
    const g = n === 'msedgewebview2.exe' ? 'webview' : n === 'node.exe' ? 'node' : n === rootName ? 'app' : 'other'
    groups[g].push(p)
  }
  const sum = (ps) => ({ count: ps.length, ws: mb(ps.reduce((s, p) => s + (p.WorkingSetSize ?? 0), 0)), priv: mb(ps.reduce((s, p) => s + (p.PrivatePageCount ?? 0), 0)) })
  const g = Object.fromEntries(Object.entries(groups).map(([k, v]) => [k, sum(v)]))
  const total = sum(procs)
  const noNode = sum(procs.filter((p) => (p.Name ?? '').toLowerCase() !== 'node.exe'))
  return { groups: g, total, noNode, webviewInTree, rootName }
}

/** 한 줄 요약(ws/priv MB) */
export function formatTree(t) {
  const f = (s) => `${s.ws}/${s.priv} MB (${s.count})`
  const gs = Object.entries(t.groups)
    .filter(([, s]) => s.count)
    .map(([k, s]) => `${k} ${f(s)}`)
    .join(', ')
  return `${gs} — total ${f(t.total)}, excluding node ${f(t.noNode)}${t.webviewInTree ? ' [webview2 in tree]' : ''}`
}
