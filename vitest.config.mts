import { resolve } from 'node:path'
import { defineConfig } from 'vitest/config'

export default defineConfig({
  resolve: { alias: { '@shared': resolve('src/shared') } },
  test: {
    include: ['tests/**/*.test.ts'],
    environment: 'node',
    // 테스트의 git 이 사용자 설정(~/.gitconfig·시스템 설정)을 건드리지 않게 환경을 격리한다
    globalSetup: ['tests/globalSetup.ts'],
    setupFiles: ['tests/setup.ts'],
    // 실제 git 을 여러 번 띄우는 테스트가 많아, 전체를 병렬로 돌릴 때 기본 5초로는 부하에 흔들린다
    testTimeout: 20_000,
    hookTimeout: 30_000
  }
})
