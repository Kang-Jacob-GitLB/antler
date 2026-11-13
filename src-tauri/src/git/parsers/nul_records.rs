//! `src/main/git/parsers/nulRecords.ts` — `-z` 출력용 NUL 구분 레코드 분리기.
//! 바이트 단위로 NUL 을 찾고, 각 레코드는 UTF-8 로 디코드한다(깨진 바이트는 U+FFFD, Node 의 `toString('utf8')` 과 같다).

#[derive(Debug, Default)]
pub struct NulRecordSplitter {
    rest: Vec<u8>,
}

impl NulRecordSplitter {
    pub fn new() -> Self {
        Self::default()
    }

    /// 완성된 레코드들을 돌려준다(빈 레코드 포함). 마지막 미완성 조각은 다음 push 까지 보관한다
    pub fn push(&mut self, chunk: &[u8]) -> Vec<String> {
        let mut buf = std::mem::take(&mut self.rest);
        buf.extend_from_slice(chunk);
        let mut out = Vec::new();
        let mut start = 0;
        while let Some(i) = buf[start..].iter().position(|&b| b == 0) {
            out.push(String::from_utf8_lossy(&buf[start..start + i]).into_owned());
            start += i + 1;
        }
        self.rest = buf[start..].to_vec();
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
    fn invalid_utf8_is_replaced() {
        assert_eq!(split_nul(&[b'a', 0xff, b'b', 0]), ["a\u{fffd}b"]);
    }
}
