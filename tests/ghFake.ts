// GitHub 테스트용 가짜 fetch. 실제 네트워크는 쓰지 않고 요청을 기록한다.
export interface RecordedRequest {
  url: string
  method: string
  headers: Record<string, string>
  body: string | undefined
}

export type Responder = (req: RecordedRequest, n: number) => Response | Promise<Response | undefined> | undefined

export function json(body: unknown, init: { status?: number; headers?: Record<string, string> } = {}): Response {
  return new Response(JSON.stringify(body), {
    status: init.status ?? 200,
    headers: { 'content-type': 'application/json', ...init.headers }
  })
}

export function fakeFetch(responder: Responder): { fetch: typeof fetch; calls: RecordedRequest[] } {
  const calls: RecordedRequest[] = []
  const impl = async (input: string | URL | Request, init?: RequestInit): Promise<Response> => {
    const headers: Record<string, string> = {}
    for (const [k, v] of Object.entries((init?.headers ?? {}) as Record<string, string>)) headers[k.toLowerCase()] = v
    const req: RecordedRequest = { url: String(input), method: init?.method ?? 'GET', headers, body: typeof init?.body === 'string' ? init.body : undefined }
    calls.push(req)
    if (init?.signal?.aborted) throw new DOMException('aborted', 'AbortError')
    const res = await responder(req, calls.length)
    return res ?? json({ message: 'Not Found' }, { status: 404 })
  }
  return { fetch: impl as typeof fetch, calls }
}
