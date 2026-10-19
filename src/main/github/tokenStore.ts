// GitHub 토큰 저장소. 토큰은 Electron safeStorage(OS 키체인·DPAPI)로 암호화해 userData/github-token.bin 에 두고,
// 로그인 방식('pat'|'device')은 비밀이 아니므로 옆의 작은 JSON 에 따로 적는다.
// safeStorage 를 쓸 수 없는 환경(Linux 키링 없음 등)에서는 평문으로 디스크에 쓰지 않고 메모리에만 들고 있는다.
import { chmodSync, mkdirSync, readFileSync, renameSync, rmSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'

/** Electron safeStorage 중 쓰는 부분 — 테스트에서 가짜로 바꿔 끼운다 */
export interface SafeStorageLike {
  isEncryptionAvailable(): boolean
  encryptString(plain: string): Buffer
  decryptString(encrypted: Buffer): string
  /** Linux 전용(Electron): 'basic_text' 면 하드코딩 키라 사실상 평문이다 */
  getSelectedStorageBackend?(): string
}

export type TokenMethod = 'pat' | 'device' | 'gh-cli'

export interface StoredToken {
  token: string
  method: TokenMethod
}

export const TOKEN_FILE = 'github-token.bin'
export const META_FILE = 'github-token.json'

export class TokenStore {
  private readonly tokenPath: string
  private readonly metaPath: string
  /** 암호화 저장을 못 할 때만 쓰는 메모리 보관분 */
  private memory: StoredToken | null = null

  constructor(
    private readonly dir: string,
    private readonly safe: SafeStorageLike,
    /** 테스트에서 바꿀 수 있게 주입한다 */
    private readonly platform: NodeJS.Platform = process.platform,
    /** 같은 저장 방식을 쓰는 다른 자격 증명(Jira 등)이 파일 이름만 바꿔 쓴다 */
    files: { token: string; meta: string } = { token: TOKEN_FILE, meta: META_FILE }
  ) {
    this.tokenPath = join(dir, files.token)
    this.metaPath = join(dir, files.meta)
  }

  /** 지금 safeStorage 를 쓸 수 없어 토큰이 디스크에 남지 않는 상태인지(호출 시점에 평가) */
  get storageUnavailable(): boolean {
    if (!this.safe.isEncryptionAvailable()) return true
    // 키링 없는 Linux 에서 Chromium 은 하드코딩 키(basic_text)로 "암호화"한다 — 디스크에 두면 평문과 같으므로
    // 이때도 저장하지 않고 메모리에만 둔다(앱을 끄면 다시 로그인)
    return this.platform === 'linux' && this.safe.getSelectedStorageBackend?.() === 'basic_text'
  }

  /** 저장된(또는 메모리의) 토큰. 없거나 복호화할 수 없으면 null */
  load(): StoredToken | null {
    if (this.memory) return this.memory
    let encrypted: Buffer
    try {
      encrypted = readFileSync(this.tokenPath)
    } catch {
      return null
    }
    try {
      const token = this.safe.decryptString(encrypted)
      return token ? { token, method: this.readMethod() } : null
    } catch {
      // 키체인이 바뀌었거나 파일이 깨졌다. 암호화를 쓸 수 있는데도 못 읽으면 쓸모없는 파일이라 지운다
      if (!this.storageUnavailable) this.removeFiles()
      return null
    }
  }

  /** 토큰을 저장한다. 디스크에 남겼으면 true, 메모리에만 뒀으면 false */
  save(token: string, method: TokenMethod): boolean {
    if (this.storageUnavailable) {
      this.memory = { token, method }
      return false
    }
    this.memory = null
    mkdirSync(this.dir, { recursive: true })
    // 토큰을 먼저 쓰고 방식은 나중에 쓴다 — 중간에 끊겨도 토큰 없이 방식만 남는 일은 없다
    atomicWrite(this.tokenPath, this.safe.encryptString(token))
    atomicWrite(this.metaPath, JSON.stringify({ method }), 'utf8')
    return true
  }

  clear(): void {
    this.memory = null
    this.removeFiles()
  }

  private readMethod(): TokenMethod {
    try {
      const m = (JSON.parse(readFileSync(this.metaPath, 'utf8')) as { method?: unknown }).method
      return m === 'device' || m === 'gh-cli' ? m : 'pat'
    } catch {
      return 'pat'
    }
  }

  private removeFiles(): void {
    rmSync(this.tokenPath, { force: true })
    rmSync(this.metaPath, { force: true })
  }
}

/** 임시 파일에 쓴 뒤 rename — 쓰는 도중 끊겨도 기존 파일이 깨지지 않는다 */
function atomicWrite(file: string, data: Buffer | string, encoding?: BufferEncoding): void {
  const tmp = `${file}.${process.pid}.tmp`
  writeFileSync(tmp, data, encoding ? { encoding, mode: 0o600 } : { mode: 0o600 })
  try {
    chmodSync(tmp, 0o600)
  } catch {
    // Windows 등 chmod 가 의미 없는 환경
  }
  renameSync(tmp, file)
}
