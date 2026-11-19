import { resolve } from 'node:path'
import react from '@vitejs/plugin-react'
import { defineConfig, type Plugin } from 'vite'

// Tauri 프런트(renderer) 빌드 설정. `pnpm build`(= tauri.conf.json 의 beforeBuildCommand)와 `pnpm tauri:web`(개발 서버)이 쓴다.
// CSP 는 tauri.conf.json 의 app.security.csp 가 넣는다(HTML 메타 태그를 넣지 않는다).
const shared = { '@shared': resolve('src/shared') }

/** window.antler 를 만드는 어댑터를 기존 진입 스크립트(main.tsx)보다 먼저 실행하게 한다 */
function tauriBridgePlugin(): Plugin {
  return {
    name: 'antler-tauri-bridge',
    // pre: 빌드에서 vite 가 스크립트 태그를 번들 진입으로 모으기 전에 넣어야 한다
    transformIndexHtml: {
      order: 'pre',
      handler(html) {
        return html.replace('<script type="module" src="/src/main.tsx"></script>', '<script type="module" src="/tauri/bridge.ts"></script>\n    $&')
      }
    }
  }
}

export default defineConfig({
  root: resolve('src/renderer'),
  resolve: {
    alias: {
      ...shared,
      '@renderer': resolve('src/renderer/src'),
      // 폰트는 npm 패키지의 woff2 를 그대로 번들한다(오프라인 앱 + CSP font-src 'self')
      '@font-pretendard': resolve('node_modules/pretendard/dist/web/variable/woff2'),
      '@font-nanumcoding': resolve('node_modules/@fontsource/nanum-gothic-coding/files')
    }
  },
  plugins: [react(), tauriBridgePlugin()],
  worker: { format: 'es' },
  // 파싱 시간을 줄여 콜드 스타트를 앞당긴다
  build: { minify: true, outDir: resolve('out-tauri/renderer'), emptyOutDir: true },
  server: { port: 5174, strictPort: true }
})
