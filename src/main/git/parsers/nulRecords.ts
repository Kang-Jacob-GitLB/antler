/**
 * `-z` 출력용 NUL 구분 레코드 분리기. 청크 경계에 걸친 레코드와 UTF-8 다중 바이트 문자를 안전하게 처리하려고
 * 문자열이 아니라 바이트 단위로 NUL 을 찾는다.
 */
export class NulRecordSplitter {
  private rest: Buffer = Buffer.alloc(0)

  /** 완성된 레코드들을 돌려준다. 마지막 미완성 조각은 다음 push 까지 보관한다. */
  push(chunk: Buffer): string[] {
    const buf = this.rest.length ? Buffer.concat([this.rest, chunk]) : chunk
    const out: string[] = []
    let start = 0
    for (let i = buf.indexOf(0, start); i !== -1; i = buf.indexOf(0, start)) {
      out.push(buf.toString('utf8', start, i))
      start = i + 1
    }
    this.rest = start < buf.length ? Buffer.from(buf.subarray(start)) : Buffer.alloc(0)
    return out
  }

  /** 스트림 종료 시 남은 조각(종결 NUL 없이 끝난 레코드)을 돌려준다. */
  end(): string[] {
    const tail = this.rest.length ? [this.rest.toString('utf8')] : []
    this.rest = Buffer.alloc(0)
    return tail.filter((s) => s.length > 0)
  }
}

export function splitNul(buf: Buffer): string[] {
  const s = new NulRecordSplitter()
  return [...s.push(buf), ...s.end()]
}
