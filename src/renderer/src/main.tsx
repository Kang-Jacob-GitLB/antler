import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import { App } from './App'
import { ErrorBoundary } from './components/ErrorBoundary'
import { watchBackdrops } from './lib/titleBarSync'
import './styles/fonts.css'
import './styles/tokens.css'
import './styles/base.css'

// 툴바가 제목 표시줄을 겸하므로 CSS 가 플랫폼별로 창 컨트롤 자리를 비운다
document.documentElement.dataset['platform'] = window.antler.platform
// 모달 배경막이 뜨면 OS 창 컨트롤(–, ㅁ, ×)도 같이 어둡게
watchBackdrops()

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <ErrorBoundary>
      <App />
    </ErrorBoundary>
  </StrictMode>
)
