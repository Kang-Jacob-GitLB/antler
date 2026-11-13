import { resolve } from 'node:path'
import { defineConfig } from 'vite'

// Tauri 개발 전용 Node 사이드카 번들. src/main 코드를 electron 없이 돌리므로 `electron` 을 shim 으로 바꾼다.
export default defineConfig({
  resolve: {
    alias: {
      '@shared': resolve('src/shared'),
      electron: resolve('src/sidecar/electronShim.ts')
    }
  },
  build: {
    ssr: 'src/sidecar/index.ts',
    outDir: 'out-tauri/sidecar',
    emptyOutDir: true,
    target: 'node22',
    rollupOptions: {
      external: ['ssh2'],
      output: { format: 'cjs', entryFileNames: 'index.cjs' }
    }
  }
})
