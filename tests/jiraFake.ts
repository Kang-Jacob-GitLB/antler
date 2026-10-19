// Jira 테스트용 가짜 서버. 실제 Jira 에는 접속하지 않고, 루프백에 in-process HTTP 서버를 띄워 요청을 기록한다.
// (fetch 를 가짜로 바꾸면 리다이렉트 처리 같은 실제 동작이 검증되지 않으므로 진짜 소켓을 쓴다)
import { createServer, type IncomingMessage, type Server, type ServerResponse } from 'node:http'
import type { AddressInfo } from 'node:net'

export interface RecordedReq {
  method: string
  /** 경로 + 쿼리 */
  url: string
  headers: IncomingMessage['headers']
}

export type Handler = (req: RecordedReq, res: ServerResponse, n: number) => void

export interface FakeJira {
  /** http://127.0.0.1:<port> */
  origin: string
  /** 같은 서버를 localhost 이름으로 본 주소(다른 호스트 역할) */
  altOrigin: string
  requests: RecordedReq[]
  close(): Promise<void>
}

export async function startFakeJira(handler: Handler): Promise<FakeJira> {
  const requests: RecordedReq[] = []
  const server: Server = createServer((req, res) => {
    const rec: RecordedReq = { method: req.method ?? 'GET', url: req.url ?? '/', headers: req.headers }
    requests.push(rec)
    handler(rec, res, requests.length)
  })
  await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve))
  const port = (server.address() as AddressInfo).port
  return {
    origin: `http://127.0.0.1:${port}`,
    altOrigin: `http://localhost:${port}`,
    requests,
    close: () =>
      new Promise<void>((resolve) => {
        server.closeAllConnections()
        server.close(() => resolve())
      })
  }
}

export function sendJson(res: ServerResponse, body: unknown, status = 200, headers: Record<string, string> = {}): void {
  res.writeHead(status, { 'content-type': 'application/json', ...headers })
  res.end(JSON.stringify(body))
}
