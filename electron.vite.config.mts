import { execFileSync } from 'node:child_process'
import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { defineConfig } from 'electron-vite'
import type { Plugin } from 'vite'
import react from '@vitejs/plugin-react'

// 앱 버전은 빌드 때 주입한다(개발 실행에서 app.getVersion() 은 Electron 버전을 돌려준다).
// 저장소의 package.json 은 0.0.0 으로 고정하고 릴리스 CI 가 태그 버전을 넣는다. 0.0.0 그대로인 로컬 빌드는
// 가장 가까운 태그로 "<태그>-dev" 를 쓴다(태그가 없으면 0.0.0-dev).
function appVersionOf(): string {
  const v = (JSON.parse(readFileSync(resolve('package.json'), 'utf8')) as { version: string }).version
  if (v !== '0.0.0') return v
  try {
    const tag = execFileSync('git', ['describe', '--tags', '--abbrev=0', '--match', 'v[0-9]*'], { encoding: 'utf8', stdio: ['ignore', 'pipe', 'ignore'] }).trim()
    if (/^v\d+\.\d+\.\d+/.test(tag)) return `${tag.slice(1)}-dev`
  } catch {
    // git 이 없거나 태그가 없는 복사본
  }
  return '0.0.0-dev'
}
const appVersion = appVersionOf()

const shared = { '@shared': resolve('src/shared') }

// 프로덕션 빌드에만 CSP 메타 태그를 넣는다. 개발 모드는 Vite HMR·React Refresh 인라인 스크립트가 필요하다.
const CSP = [
  "default-src 'self'",
  "script-src 'self'",
  "style-src 'self' 'unsafe-inline'",
  // 아바타·PR 본문 이미지만 허용(github.com/user-attachments 는 private-user-images 로 넘어가고, 이모지는 githubassets). 다른 외부 이미지는 막는다
  "img-src 'self' data: https://avatars.githubusercontent.com https://camo.githubusercontent.com https://user-images.githubusercontent.com https://private-user-images.githubusercontent.com https://github.com https://github.githubassets.com",
  "font-src 'self' data:",
  "worker-src 'self' blob:",
  "connect-src 'self'",
  "object-src 'none'",
  "base-uri 'none'",
  "form-action 'none'"
].join('; ')

function cspPlugin(): Plugin {
  return {
    name: 'antler-csp',
    apply: 'build',
    transformIndexHtml(html) {
      return html.replace(
        '<head>',
        `<head>\n    <meta http-equiv="Content-Security-Policy" content="${CSP}" />`
      )
    }
  }
}

export default defineConfig({
  main: { resolve: { alias: shared }, define: { __APP_VERSION__: JSON.stringify(appVersion) } },
  preload: { resolve: { alias: shared } },
  renderer: {
    resolve: {
      alias: {
        ...shared,
        '@renderer': resolve('src/renderer/src'),
        // 폰트는 npm 패키지의 woff2 를 그대로 번들한다(오프라인 앱 + CSP font-src 'self')
        '@font-pretendard': resolve('node_modules/pretendard/dist/web/variable/woff2'),
        '@font-nanumcoding': resolve('node_modules/@fontsource/nanum-gothic-coding/files')
      }
    },
    plugins: [react(), cspPlugin()],
    worker: { format: 'es' },
    // 파싱 시간을 줄여 콜드 스타트를 앞당긴다
    build: { minify: true }
  }
})
