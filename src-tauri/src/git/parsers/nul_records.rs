//! `src/main/git/parsers/nulRecords.ts` — `-z` 출력용 NUL 구분 레코드 분리기.
//! 바이트 단위로 NUL 을 찾고, 각 레코드는 UTF-8 로 디코드한다(깨진 바이트는 U+FFFD, Node 의 `toString('utf8')` 과 같다).

/// 스트리밍 소비자(`git:log` 커서)가 레코드 하나에 허용하는 최대 바이트. TS 에는 상한이 없다 — NUL 이 없는 출력이 끝없이 이어지면
/// 미완성 조각이 메모리를 다 먹으므로 Rust 쪽에만 둔다. 한 커밋의 메시지·메타 전체가 이보다 클 수는 거의 없다(실제 최대는 수십 KB).
pub const MAX_STREAM_RECORD: usize = 16 * 1024 * 1024;

/// 레코드 하나가 상한을 넘었다
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecordTooLong;

#[derive(Debug, Default)]
pub struct NulRecordSplitter {
    rest: Vec<u8>,
    /// 레코드 하나의 상한. `None` 이면 제한 없음([`NulRecordSplitter::push`] 의 동작)
    limit: Option<usize>,
}

impl NulRecordSplitter {
    pub fn new() -> Self {
        Self::default()
    }

    /// 레코드 하나가 `max` 바이트를 넘으면 [`NulRecordSplitter::push_checked`] 가 오류를 돌려준다
    pub fn with_limit(max: usize) -> Self {
        NulRecordSplitter { rest: Vec::new(), limit: Some(max) }
    }

    /// [`NulRecordSplitter::push`] 와 같되, 완성된 레코드나 아직 NUL 을 못 만난 미완성 조각이 상한을 넘으면 `RecordTooLong`.
    /// 오류 뒤에는 이 분리기를 더 쓰지 않는다
    pub fn push_checked(&mut self, chunk: &[u8]) -> Result<Vec<String>, RecordTooLong> {
        let max = self.limit.unwrap_or(usize::MAX);
        // 상한을 넘는 조각을 합치기 전에 거른다(합친 뒤에는 이미 메모리를 썼다)
        let mut start = 0;
        let mut pending = self.rest.len();
        for (i, &b) in chunk.iter().enumerate() {
            if b == 0 {
                if pending + (i - start) > max {
                    return Err(RecordTooLong);
                }
                pending = 0;
                start = i + 1;
            }
        }
        if pending + (chunk.len() - start) > max {
            return Err(RecordTooLong);
        }
        Ok(self.push(chunk))
    }

    /// 완성된 레코드들을 돌려준다(빈 레코드 포함). 마지막 미완성 조각은 다음 push 까지 보관한다
    ///
    /// 새 청크만 훑는다 — 미완성 조각을 청크마다 처음부터 다시 훑고 복사하면 긴 레코드에서 시간이 길이의 제곱으로 는다.
    pub fn push(&mut self, chunk: &[u8]) -> Vec<String> {
        let Some(first) = chunk.iter().position(|&b| b == 0) else {
            self.rest.extend_from_slice(chunk);
            return Vec::new();
        };
        let mut out = Vec::new();
        // 첫 레코드 = 보관 조각 + 청크 앞부분
        let mut head = std::mem::take(&mut self.rest);
        head.extend_from_slice(&chunk[..first]);
        out.push(String::from_utf8_lossy(&head).into_owned());
        let mut start = first + 1;
        while let Some(i) = chunk[start..].iter().position(|&b| b == 0) {
            out.push(String::from_utf8_lossy(&chunk[start..start + i]).into_owned());
            start += i + 1;
        }
        self.rest.extend_from_slice(&chunk[start..]);
        out
    }

    /// 스트림 종료 시 남은 조각(종결 NUL 없이 끝난 레코드)을 돌려준다. 비어 있으면 없다
    pub fn end(&mut self) -> Vec<String> {
        let rest = std::mem::take(&mut self.rest);
        if rest.is_empty() {
            Vec::new()
        } else {
            vec![String::from_utf8_lossy(&rest).into_owned()]
        }
    }
}

pub fn split_nul(buf: &[u8]) -> Vec<String> {
    let mut s = NulRecordSplitter::new();
    let mut out = s.push(buf);
    out.extend(s.end());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_and_trailing_nul() {
        assert!(split_nul(b"").is_empty());
        assert_eq!(split_nul(b"a\0"), ["a"]);
        assert_eq!(split_nul(b"a\0b"), ["a", "b"]);
        assert_eq!(split_nul(b"a\0\0b\0"), ["a", "", "b"]);
        assert_eq!(split_nul(b"\0"), [""]);
    }

    #[test]
    fn chunk_boundaries_do_not_matter() {
        let data = "한글\0경로 공백\0끝".as_bytes();
        for cut in 0..=data.len() {
            let mut s = NulRecordSplitter::new();
            let mut out = s.push(&data[..cut]);
            out.extend(s.push(&data[cut..]));
            out.extend(s.end());
            assert_eq!(out, ["한글", "경로 공백", "끝"], "cut={cut}");
        }
    }

    #[test]
    fn 레코드_상한은_완성된_레코드와_미완성_조각_모두에_걸린다() {
        // 상한 이하는 통과하고 push 와 결과가 같다(경계: 정확히 상한)
        let mut s = NulRecordSplitter::with_limit(4);
        assert_eq!(s.push_checked(b"abcd\0ab").unwrap(), ["abcd"]);
        assert_eq!(s.push_checked(b"cd\0").unwrap(), ["abcd"]);
        // 완성된 레코드가 상한을 넘는다
        assert_eq!(NulRecordSplitter::with_limit(4).push_checked(b"abcde\0"), Err(RecordTooLong));
        // NUL 이 안 나오는 미완성 조각이 청크에 걸쳐 상한을 넘는다
        let mut s = NulRecordSplitter::with_limit(4);
        assert!(s.push_checked(b"abc").unwrap().is_empty());
        assert_eq!(s.push_checked(b"de"), Err(RecordTooLong));
        // 상한이 없으면(new) push_checked 도 제한하지 않는다
        assert_eq!(NulRecordSplitter::new().push_checked(&vec![b'x'; 1 << 20]).unwrap().len(), 0);
    }

    #[test]
    fn 청크_경계가_어디든_상한_판정은_같다() {
        let data = b"abcd\0abcd\0ab";
        for cut in 0..=data.len() {
            let mut s = NulRecordSplitter::with_limit(4);
            let mut out = s.push_checked(&data[..cut]).unwrap();
            out.extend(s.push_checked(&data[cut..]).unwrap());
            out.extend(s.end());
            assert_eq!(out, ["abcd", "abcd", "ab"], "cut={cut}");
        }
    }

    #[test]
    fn 긴_미완성_조각을_여러_청크로_받아도_선형이다() {
        // 4MB 조각을 1바이트씩 넘기면 매번 처음부터 다시 훑던 구현은 수조 번 비교한다 — 지금은 즉시 끝나야 한다
        let mut s = NulRecordSplitter::new();
        let t = std::time::Instant::now();
        for _ in 0..(4 << 20) {
            assert!(s.push(b"x").is_empty());
        }
        assert_eq!(s.push(b"\0").len(), 1);
        assert!(t.elapsed() < std::time::Duration::from_secs(10), "{:?}", t.elapsed());
    }

    #[test]
    fn invalid_utf8_is_replaced() {
        assert_eq!(split_nul(&[b'a', 0xff, b'b', 0]), ["a\u{fffd}b"]);
    }
}
