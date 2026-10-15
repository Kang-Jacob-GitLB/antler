// OpenSSH 클라이언트 설정(~/.ssh/config) 파서. 읽기 전용이며 접속 대상 해석에 필요한 키워드만 다룬다.
//
// 지원: Host(복수 패턴·와일드카드 * ? ·부정 !), HostName(%h), User, Port, IdentityFile(복수·~·%d %u %h %r %p %n),
//       ProxyJump(쉼표 체인·user@host:port·none), IdentitiesOnly, IdentityAgent(none/SSH_AUTH_SOCK/경로),
//       Include(~/.ssh 기준 상대 경로, 마지막 경로 요소의 * ? 글롭만 best-effort).
// 의미론: OpenSSH 와 같이 "먼저 얻은 값이 이긴다"(IdentityFile 만 모든 일치 블록에서 누적).
// 미지원: Match — `Match all` 만 전역으로 취급하고 그 외 조건(host/user/exec …)의 Match 블록은 통째로 적용하지 않는다.
//         ProxyCommand, 호스트별 ProxyJump 의 재귀 적용(점프 호스트 자신의 ProxyJump 는 무시)도 다루지 않는다.
import { readdirSync, readFileSync, statSync } from 'node:fs'
import { homedir, userInfo } from 'node:os'
import { basename, dirname, isAbsolute, join, normalize } from 'node:path'
import type { SshHostInfo } from '@shared/types'

export interface ParseOptions {
  /** ~ 와 Include 상대 경로의 기준(기본 os.homedir()). 테스트에서는 임시 디렉터리를 준다. */
  homeDir?: string
}

interface ConfigOption {
  /** 소문자 키워드 */
  key: string
  args: string[]
}

interface ConfigBlock {
  /** 전역(Host 이전) 또는 Match all 이면 null — 항상 적용 */
  patterns: string[] | null
  /** 적용 불가 Match 블록 */
  skipped: boolean
  /** skipped Match 블록의 조건(소문자 키워드·값 쌍) — 대상 호스트에 해당할 수 있는지 판단용 */
  matchArgs?: string[]
  options: ConfigOption[]
}

export interface SshConfig {
  homeDir: string
  blocks: ConfigBlock[]
}

export interface JumpHop {
  /** user@host:port 원형(재해석에 쓴다) */
  spec: string
  user?: string
  host: string
  port?: number
}

export interface ResolvedHost {
  /** 입력으로 받은 별칭/호스트(설정 조회 키) */
  alias: string
  hostName: string
  user: string
  port: number
  identityFiles: string[]
  proxyJump: JumpHop[]
  identitiesOnly: boolean
  /** undefined: 기본(SSH_AUTH_SOCK 등) · null: 사용 안 함 · string: 소켓/파이프 경로 */
  identityAgent: string | null | undefined
  /** 지원하지 않아 무시할 수 없는 설정이 있으면 그 설명(ProxyCommand, 이 호스트에 해당할 수 있는 Match 블록) */
  unsupported?: string
}

const MAX_INCLUDE_DEPTH = 16

/** 따옴표·공백·행 중간 주석(# 로 시작하는 토큰)을 처리해 인자로 쪼갠다. 백슬래시는 Windows 경로 때문에 그대로 둔다. */
function splitArgs(rest: string): string[] {
  const args: string[] = []
  let cur = ''
  let has = false
  let quoted = false
  for (let i = 0; i < rest.length; i++) {
    const ch = rest[i]!
    if (quoted) {
      if (ch === '\\' && rest[i + 1] === '"') {
        cur += '"'
        i++
      } else if (ch === '"') quoted = false
      else cur += ch
    } else if (ch === '"') {
      quoted = true
      has = true
    } else if (ch === ' ' || ch === '\t') {
      if (has) args.push(cur)
      cur = ''
      has = false
    } else if (ch === '#' && !has) {
      break
    } else {
      cur += ch
      has = true
    }
  }
  if (has) args.push(cur)
  return args
}

function globToRegExp(pattern: string): RegExp {
  let re = ''
  for (const ch of pattern) {
    if (ch === '*') re += '.*'
    else if (ch === '?') re += '.'
    else re += ch.replace(/[.+^${}()|[\]\\]/g, '\\$&')
  }
  return new RegExp(`^${re}$`, 'i')
}

export function hasWildcard(s: string): boolean {
  return /[*?]/.test(s)
}

function expandIncludeGlob(pattern: string): string[] {
  if (!/[*?[]/.test(pattern)) return [pattern]
  const dir = dirname(pattern)
  if (/[*?[]/.test(dir)) return [] // 디렉터리 부분의 글롭은 지원하지 않는다
  const re = globToRegExp(basename(pattern))
  try {
    return readdirSync(dir)
      .filter((n) => re.test(n))
      .sort()
      .map((n) => join(dir, n))
  } catch {
    return []
  }
}

function expandTilde(p: string, homeDir: string): string {
  if (p === '~') return homeDir
  if (p.startsWith('~/') || p.startsWith('~\\')) return join(homeDir, p.slice(2))
  return p
}

interface ParseState {
  cur: ConfigBlock
}

function parseInto(text: string, cfg: SshConfig, state: ParseState, depth: number): void {
  for (const raw of text.replace(/^﻿/, '').split(/\r?\n/)) {
    const line = raw.trim()
    if (!line || line.startsWith('#')) continue
    const m = /^([A-Za-z0-9]+)(?:\s*=\s*|\s+)(.*)$/.exec(line)
    if (!m) continue
    const key = m[1]!.toLowerCase()
    const args = splitArgs(m[2]!)
    if (args.length === 0) continue

    if (key === 'host') {
      state.cur = { patterns: args, skipped: false, options: [] }
      cfg.blocks.push(state.cur)
    } else if (key === 'match') {
      const all = args.length === 1 && args[0]!.toLowerCase() === 'all'
      state.cur = { patterns: null, skipped: !all, options: [], ...(all ? {} : { matchArgs: args }) }
      cfg.blocks.push(state.cur)
    } else if (key === 'include') {
      if (depth >= MAX_INCLUDE_DEPTH) continue
      for (const arg of args) {
        const p = expandTilde(arg, cfg.homeDir)
        const abs = isAbsolute(p) ? p : join(cfg.homeDir, '.ssh', p)
        for (const file of expandIncludeGlob(abs)) {
          let content: string
          try {
            if (!statSync(file).isFile()) continue
            content = readFileSync(file, 'utf8')
          } catch {
            continue
          }
          // 포함된 파일 안의 Host/Match 는 그 파일 안에서만 유효하다(끝나면 부모 상태로 복귀)
          parseInto(content, cfg, { cur: state.cur }, depth + 1)
        }
      }
    } else {
      state.cur.options.push({ key, args })
    }
  }
}

export function parseSshConfig(text: string, opts: ParseOptions = {}): SshConfig {
  const global: ConfigBlock = { patterns: null, skipped: false, options: [] }
  const cfg: SshConfig = { homeDir: opts.homeDir ?? homedir(), blocks: [global] }
  parseInto(text, cfg, { cur: global }, 0)
  return cfg
}

/** 설정 파일을 읽어 파싱한다. 파일이 없으면 빈 설정. */
export function loadSshConfig(configPath: string, opts: ParseOptions = {}): SshConfig {
  let text = ''
  try {
    text = readFileSync(configPath, 'utf8')
  } catch {
    // 설정 파일이 없어도 user@host 직접 입력은 동작해야 한다
  }
  return parseSshConfig(text, opts)
}

function hostNameFor(first: Map<string, string[]>, alias: string): string {
  const raw = first.get('hostname')?.[0]
  return raw ? expandTokens(raw, { h: alias }) : alias
}

/**
 * 해석하지 않은 Match 블록이 이 호스트에 걸릴 수 있는지 보수적으로 판단한다.
 * host/originalhost 조건이 있고 그 패턴이 별칭·HostName 어느 쪽과도 맞지 않으면 무관하다고 본다.
 * 그 밖의 조건(exec, user, localuser, canonical …)만 있으면 판단할 수 없으므로 걸릴 수 있다고 본다.
 */
function matchMayApply(args: string[], names: string[]): boolean {
  for (let i = 0; i + 1 < args.length; i += 2) {
    const crit = args[i]!.toLowerCase().replace(/^!/, '')
    if (crit !== 'host' && crit !== 'originalhost') continue
    const negated = args[i]!.startsWith('!')
    const patterns = args[i + 1]!.split(',')
    const hit = names.some((n) =>
      patterns.some((p) => (p.startsWith('!') ? false : globToRegExp(p).test(n)))
    )
    if (negated ? hit : !hit) return false
  }
  return true
}

function blockApplies(block: ConfigBlock, host: string): boolean {
  if (block.skipped) return false
  if (block.patterns === null) return true
  let positive = false
  for (const p of block.patterns) {
    if (p.startsWith('!')) {
      if (globToRegExp(p.slice(1)).test(host)) return false
    } else if (globToRegExp(p).test(host)) {
      positive = true
    }
  }
  return positive
}

/** `[user@]host[:port]` (host 는 [::1] 형태의 대괄호 IPv6 허용). 그 외 모양이면 통째로 host. */
export function parseHostSpec(spec: string): { user?: string; host: string; port?: number } {
  let rest = spec.trim().replace(/^ssh:\/\//i, '').replace(/\/+$/, '')
  let user: string | undefined
  const at = rest.lastIndexOf('@')
  if (at > 0) {
    user = rest.slice(0, at)
    rest = rest.slice(at + 1)
  }
  let port: number | undefined
  const br = /^\[([^\]]+)\](?::(\d+))?$/.exec(rest)
  if (br) {
    rest = br[1]!
    if (br[2]) port = Number(br[2])
  } else {
    const pm = /^([^:]+):(\d+)$/.exec(rest) // 콜론이 하나일 때만 포트(여러 개면 IPv6 리터럴)
    if (pm) {
      rest = pm[1]!
      port = Number(pm[2])
    }
  }
  return { user, host: rest, port }
}

function parseProxyJump(value: string): JumpHop[] {
  if (value.toLowerCase() === 'none') return []
  return value
    .split(',')
    .map((s) => s.trim())
    .filter(Boolean)
    .map((spec) => ({ spec, ...parseHostSpec(spec) }))
}

function validPort(s: string | undefined): number | undefined {
  if (!s || !/^\d+$/.test(s)) return undefined
  const n = Number(s)
  return n >= 1 && n <= 65535 ? n : undefined
}

function expandTokens(s: string, t: Record<string, string>): string {
  return s.replace(/%(.)/g, (all, c: string) => (c === '%' ? '%' : (t[c] ?? all)))
}

function expandEnv(s: string): string {
  return s.replace(/\$\{(\w+)\}|\$(\w+)/g, (_, a?: string, b?: string) => process.env[a ?? b ?? ''] ?? '')
}

/**
 * 별칭 또는 `[user@]host[:port]` 를 해석한다. spec 의 user/port 는 설정 파일 값보다 우선한다(ssh CLI 와 동일).
 * 설정 조회는 spec 의 host 부분을 별칭으로 삼는다.
 */
export function resolveHost(config: SshConfig, spec: string, opts: { localUser?: string } = {}): ResolvedHost {
  const given = parseHostSpec(spec)
  const alias = given.host
  const localUser = opts.localUser ?? safeUserName()
  const first = new Map<string, string[]>()
  const identityFiles: string[] = []
  for (const block of config.blocks) {
    if (!blockApplies(block, alias)) continue
    for (const { key, args } of block.options) {
      if (key === 'identityfile') identityFiles.push(args[0]!)
      else if (!first.has(key)) first.set(key, args)
    }
  }

  const hostNameRaw = first.get('hostname')?.[0]
  const hostName = hostNameRaw ? expandTokens(hostNameRaw, { h: alias }) : alias
  const user = given.user ?? first.get('user')?.[0] ?? localUser
  const port = given.port ?? validPort(first.get('port')?.[0]) ?? 22

  const tokens = { d: config.homeDir, u: localUser, h: hostName, r: user, p: String(port), n: alias }
  const files = identityFiles
    .filter((f) => f.toLowerCase() !== 'none')
    .map((f) => normalize(expandTilde(expandTokens(f, tokens), config.homeDir)))

  // 조용히 무시하면 위험한 설정: ProxyCommand 를 무시하면 bastion 을 건너뛰고 내부 이름으로 직접 접속하고,
  // 이 호스트에 걸릴 수 있는 Match 블록(IdentitiesOnly·User 등)을 무시하면 의도와 다른 신원으로 접속한다
  const problems: string[] = []
  const pc = first.get('proxycommand')?.[0]
  if (pc && pc.toLowerCase() !== 'none') problems.push('ProxyCommand')
  for (const block of config.blocks) {
    if (block.skipped && block.options.length && matchMayApply(block.matchArgs ?? [], [alias, hostNameFor(first, alias)])) {
      problems.push(`Match ${(block.matchArgs ?? []).join(' ')}`)
    }
  }

  const pj = first.get('proxyjump')
  const agentRaw = first.get('identityagent')?.[0]
  let identityAgent: string | null | undefined
  if (agentRaw !== undefined) {
    if (agentRaw.toLowerCase() === 'none') identityAgent = null
    else if (agentRaw === 'SSH_AUTH_SOCK') identityAgent = undefined
    else identityAgent = expandTilde(expandEnv(expandTokens(agentRaw, tokens)), config.homeDir)
  }

  return {
    alias,
    hostName,
    user,
    port,
    identityFiles: files,
    proxyJump: pj ? parseProxyJump(pj.join(' ')) : [],
    identitiesOnly: first.get('identitiesonly')?.[0]?.toLowerCase() === 'yes',
    identityAgent,
    ...(problems.length ? { unsupported: problems.join(', ') } : {})
  }
}

function safeUserName(): string {
  try {
    return userInfo().username
  } catch {
    return process.env.USERNAME ?? process.env.USER ?? 'user'
  }
}

/** UI 에 보여줄 목록. 와일드카드·부정 패턴은 제외하고 구체적인 별칭만 낸다(등장 순서, 중복 제거). */
export function listHosts(config: SshConfig, opts: { localUser?: string } = {}): SshHostInfo[] {
  const seen = new Set<string>()
  const out: SshHostInfo[] = []
  for (const block of config.blocks) {
    if (block.skipped || block.patterns === null) continue
    for (const p of block.patterns) {
      if (p.startsWith('!') || hasWildcard(p) || seen.has(p)) continue
      seen.add(p)
      const r = resolveHost(config, p, opts)
      out.push({
        alias: p,
        hostName: r.hostName,
        user: r.user,
        port: r.port,
        ...(r.proxyJump.length ? { proxyJump: r.proxyJump.map((h) => h.spec).join(',') } : {})
      })
    }
  }
  return out
}
