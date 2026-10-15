import { resolve } from 'node:path'
import { defineConfig } from 'electron-vite'
import type { Plugin } from 'vite'
import react from '@vitejs/plugin-react'

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
  main: { resolve: { alias: shared } },
  preload: { resolve: { alias: shared } },
  renderer: {
    resolve: {
      alias: {
        ...shared,
        '@renderer': resolve('src/renderer/src'),
        // 폰트는 npm 패키지의 woff2 를 그대로 번들한다(오프라인 앱 + CSP font-src 'self')
        '@font-pretendard': resolve('node_modules/pretendard/dist/web/variable/woff2'),
        '@font-d2coding': resolve('node_modules/d2coding/fonts')
      }
    },
    plugins: [react(), cspPlugin()],
    worker: { format: 'es' },
    // 파싱 시간을 줄여 콜드 스타트를 앞당긴다
    build: { minify: true }
  }
})
