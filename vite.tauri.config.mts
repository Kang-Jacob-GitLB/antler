import { resolve } from 'node:path'
import { defineConfig, type Plugin } from 'vite'
import electronViteConfig from './electron.vite.config.mjs'

// electron.vite.config.mts 의 renderer 설정(alias·worker·build)을 그대로 쓰고, Tauri 에서 달라지는 것만 덮는다.
// CSP 는 tauri.conf.json 의 app.security.csp 가 넣으므로 antler-csp 플러그인은 뺀다.
const base = (electronViteConfig as { renderer: { resolve?: object; plugins?: Plugin[]; worker?: object; build?: object } }).renderer

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
  resolve: base.resolve ?? {},
  plugins: [...(base.plugins ?? []).flat().filter((p) => (p as Plugin).name !== 'antler-csp'), tauriBridgePlugin()],
  worker: base.worker ?? {},
  build: { ...base.build, outDir: resolve('out-tauri/renderer'), emptyOutDir: true },
  server: { port: 5174, strictPort: true }
})
