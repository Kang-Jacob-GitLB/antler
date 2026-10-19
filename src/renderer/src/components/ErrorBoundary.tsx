import { Component, type ErrorInfo, type ReactNode } from 'react'
import { translate } from '../i18n'
import { useAppStore } from '../stores/appStore'
import './ErrorBoundary.css'

interface State {
  error: Error | null
}

/** 렌더 중 예외로 화면 전체가 하얘지지 않게 하는 최상위 경계: 오류 메시지와 다시 그리기 버튼을 보여 준다 */
export class ErrorBoundary extends Component<{ children: ReactNode }, State> {
  override state: State = { error: null }

  static getDerivedStateFromError(error: Error): State {
    return { error }
  }

  override componentDidCatch(error: Error, info: ErrorInfo): void {
    console.error('[ErrorBoundary]', error, info.componentStack)
  }

  override render(): ReactNode {
    const { error } = this.state
    if (!error) return this.props.children
    // 이 경계는 설정 로딩 전에도 뜰 수 있으므로 훅 대신 스토어를 직접 읽는다
    const locale = useAppStore.getState().config?.ui.locale ?? 'ko'
    return (
      <div className="error-boundary" role="alert">
        <h1 className="error-boundary__title">{translate(locale, 'error.boundaryTitle')}</h1>
        <pre className="error-boundary__msg selectable">{error.message}</pre>
        <button className="btn" onClick={() => this.setState({ error: null })}>
          {translate(locale, 'error.boundaryRetry')}
        </button>
      </div>
    )
  }
}
