//! `src/main/git/parsers/diff.ts` — 파일 하나의 unified diff 를 청크 단위로 받아 `FileDiff` 로 만든다.
//! 상한(줄·바이트·세기 한계)과 잘림 표시는 TS 와 같은 값·같은 순서로 동작한다.

use serde::Serialize;

use super::{js_number, js_trim, JsNumber};

/// 기본 diff 줄 수 상한
pub const DEFAULT_MAX_DIFF_LINES: usize = 3000;
/// 상한 이후 줄 수를 세는 한계. 넘으면 `total_lines` 는 하한값이 된다
pub const DEFAULT_COUNT_LIMIT: usize = 200_000;
/// 상한 이후 줄을 세며 읽는 바이트 한계(64MB). 넘으면 더 읽지 않는다
pub const DEFAULT_COUNT_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone, Default)]
pub struct DiffParseOpts {
    /// 객체로 만들 diff 줄 수 상한(기본 3000)
    pub max_lines: Option<usize>,
    /// 파싱하는 입력 바이트 상한. 넘으면 나머지는 세기만 한다(기본 무제한)
    pub max_bytes: Option<usize>,
    /// 상한 이후 줄을 세는 한계(기본 20만)
    pub count_limit: Option<usize>,
    /// 상한 이후 줄을 세며 읽는 바이트 한계(기본 64MB)
    pub count_bytes: Option<usize>,
}

/// 사람이 읽을 만한 헤더 줄(index·---·+++ 는 제외)
const META_PREFIXES: [&str; 10] = [
    "new file mode ",
    "deleted file mode ",
    "old mode ",
    "new mode ",
    "similarity index ",
    "dissimilarity index ",
    "rename from ",
    "rename to ",
    "copy from ",
    "copy to ",
];

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DiffLine {
    /// ' ' · '+' · '-' · '\\'
    pub t: &'static str,
    pub text: String,
    /// 원래 파일 줄 번호(추가 줄은 없음)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub o: Option<JsNumber>,
    /// 새 파일 줄 번호(삭제 줄은 없음)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub n: Option<JsNumber>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffHunk {
    pub old_start: JsNumber,
    pub old_lines: JsNumber,
    pub new_start: JsNumber,
    pub new_lines: JsNumber,
    /// @@ 뒤의 함수 문맥 등
    pub header: String,
    pub lines: Vec<DiffLine>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileDiff {
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old_path: Option<String>,
    pub binary: bool,
    pub hunks: Vec<DiffHunk>,
    /// 크기 상한으로 일부만 담았는지
    pub truncated: bool,
    /// 전체 diff 줄 수(잘렸을 때 안내용)
    pub total_lines: usize,
    /// 세기 한계에 걸려 `total_lines` 가 하한값인지("N+ 줄"). 아니면 키가 없다
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_is_lower_bound: Option<bool>,
    /// 모드 변경 등 내용 외 변경 안내("new file mode 100755" 등)
    pub meta: Vec<String>,
}

/// 상한 이후의 줄을 객체 없이 바이트로만 센다. '@' 로 시작하는 줄은 hunk 헤더라 diff 줄에서 뺀다
struct LineCounter {
    lines: usize,
    headers: usize,
    bytes: usize,
    at_start: bool,
    limit: usize,
    byte_limit: usize,
}

impl LineCounter {
    fn new(limit: usize, byte_limit: usize) -> Self {
        Self { lines: 0, headers: 0, bytes: 0, at_start: true, limit, byte_limit }
    }

    /// 줄 수나 읽은 바이트가 한계에 닿았다 — 이후 줄 수는 하한값이다
    fn exhausted(&self) -> bool {
        self.lines >= self.limit || self.bytes >= self.byte_limit
    }

    fn feed(&mut self, buf: &[u8]) {
        self.bytes = self.bytes.saturating_add(buf.len());
        let mut i = 0;
        while i < buf.len() && !self.exhausted() {
            if self.at_start {
                self.at_start = false;
                if buf[i] == 0x40 {
                    self.headers += 1;
                } else {
                    self.lines += 1;
                }
            }
            let Some(nl) = buf[i..].iter().position(|&b| b == b'\n') else { return };
            self.at_start = true;
            i += nl + 1;
        }
    }
}

/// ASCII 숫자로 시작하는 부분(JS 정규식 `\d+`)과 나머지
fn digits(s: &str) -> Option<(&str, &str)> {
    let end = s.bytes().position(|b| !b.is_ascii_digit()).unwrap_or(s.len());
    (end > 0).then(|| s.split_at(end))
}

/// `-12,3` 의 `12,3` 부분: 숫자 + 선택적 `,숫자`
fn range(s: &str) -> Option<(&str, Option<&str>, &str)> {
    let (a, rest) = digits(s)?;
    match rest.strip_prefix(',') {
        Some(r) => {
            let (b, rest) = digits(r)?;
            Some((a, Some(b), rest))
        }
        None => Some((a, None, rest)),
    }
}

/// 정규식 `^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@ ?(.*)$` — `.` 은 줄 끝 문자(\r· · )에 걸리지 않는다
fn parse_hunk_header(s: &str) -> Option<DiffHunk> {
    let s = s.strip_prefix("@@ -")?;
    let (old_start, old_lines, s) = range(s)?;
    let s = s.strip_prefix(" +")?;
    let (new_start, new_lines, s) = range(s)?;
    let s = s.strip_prefix(" @@")?;
    let header = s.strip_prefix(' ').unwrap_or(s);
    if header.contains(['\r', '\n', '\u{2028}', '\u{2029}']) {
        return None;
    }
    Some(DiffHunk {
        old_start: js_number(old_start),
        old_lines: old_lines.map_or(JsNumber(1.0), js_number),
        new_start: js_number(new_start),
        new_lines: new_lines.map_or(JsNumber(1.0), js_number),
        header: header.to_string(),
        lines: Vec::new(),
    })
}

/// 파일 하나의 unified diff(`git diff-tree -p` 출력)를 청크 단위로 받아 `FileDiff` 로 만든다.
/// - 첫 @@ 이전의 헤더(diff --git/index/---/+++)는 건너뛰고, 그 뒤의 "--"/"++" 로 시작하는 줄은 내용으로 본다.
/// - `max_lines` 에 도달하면 이후는 줄 수만 센다(truncated). 센 값이 `count_limit` 를 넘으면 `total_lines` 는 하한값이다.
/// - `push` 가 false 를 돌려주면 더 읽을 필요가 없다(호출자는 git 프로세스를 끊어도 된다).
pub struct DiffParser {
    path: String,
    old_path: Option<String>,
    max_lines: usize,
    max_bytes: usize,
    counter: LineCounter,
    pending: Vec<u8>,
    bytes: usize,
    counting: bool,

    binary: bool,
    meta: Vec<String>,
    hunks: Vec<DiffHunk>,
    /// 지금 줄을 담고 있는 hunk 가 있는지(있으면 `hunks` 의 마지막)
    in_hunk: bool,
    in_hunks: bool,
    o: f64,
    n: f64,
    parsed_lines: usize,
}

impl DiffParser {
    pub fn new(path: &str, old_path: Option<&str>, opts: &DiffParseOpts) -> Self {
        Self {
            path: path.to_string(),
            old_path: old_path.map(str::to_string),
            max_lines: opts.max_lines.unwrap_or(DEFAULT_MAX_DIFF_LINES),
            max_bytes: opts.max_bytes.unwrap_or(usize::MAX),
            counter: LineCounter::new(opts.count_limit.unwrap_or(DEFAULT_COUNT_LIMIT), opts.count_bytes.unwrap_or(DEFAULT_COUNT_BYTES)),
            pending: Vec::new(),
            bytes: 0,
            counting: false,
            binary: false,
            meta: Vec::new(),
            hunks: Vec::new(),
            in_hunk: false,
            in_hunks: false,
            o: 0.0,
            n: 0.0,
            parsed_lines: 0,
        }
    }

    /// 더 읽어야 하면 true
    pub fn push(&mut self, chunk: &[u8]) -> bool {
        if self.counting {
            self.counter.feed(chunk);
            return !self.counter.exhausted();
        }
        self.bytes = self.bytes.saturating_add(chunk.len());
        let mut start = 0;
        while !self.counting {
            let Some(p) = chunk[start..].iter().position(|&b| b == b'\n') else { break };
            let nl = start + p;
            let line = self.take_line(chunk, start, nl);
            start = nl + 1;
            self.feed_line(&line);
            if self.parsed_lines >= self.max_lines && !self.counting {
                self.start_counting(&chunk[start..], false);
            }
        }
        if self.counting {
            return !self.counter.exhausted();
        }
        if start < chunk.len() {
            self.pending.extend_from_slice(&chunk[start..]);
        }
        // 입력이 너무 크면(줄 하나가 거대한 경우 포함) 여기까지만 담는다
        if self.bytes > self.max_bytes || self.pending.len() > self.max_bytes {
            let partial = !self.pending.is_empty();
            self.start_counting(&[], partial);
        }
        !self.counter.exhausted()
    }

    pub fn finish(mut self) -> FileDiff {
        if !self.counting && !self.pending.is_empty() {
            // 개행 없이 끝난 마지막 줄
            let line = String::from_utf8_lossy(&std::mem::take(&mut self.pending)).into_owned();
            self.feed_line(&line);
        }
        let counted = self.counter.lines;
        // 상한에 걸렸어도 실제로 남은 줄이 없으면(입력이 상한 직후 끝남) 잘린 것이 아니다
        let truncated = counted > 0 || self.counter.headers > 0;
        FileDiff {
            path: self.path,
            old_path: self.old_path,
            binary: self.binary,
            hunks: self.hunks,
            truncated,
            total_lines: self.parsed_lines + counted,
            total_is_lower_bound: (truncated && self.counter.exhausted()).then_some(true),
            meta: self.meta,
        }
    }

    /// 개행 앞까지를 한 줄로 꺼낸다. 이전 청크에서 이어진 조각이 있으면 합친다
    fn take_line(&mut self, chunk: &[u8], start: usize, nl: usize) -> String {
        if self.pending.is_empty() {
            return String::from_utf8_lossy(&chunk[start..nl]).into_owned();
        }
        self.pending.extend_from_slice(&chunk[start..nl]);
        let s = String::from_utf8_lossy(&self.pending).into_owned();
        self.pending.clear();
        s
    }

    /// 객체 만들기를 멈추고 남은 입력은 세기만 한다.
    /// `partial_line`: 개행 없이 끝난 조각이 남아 있으면(입력 상한 초과) 그것을 한 줄로 센다.
    fn start_counting(&mut self, rest: &[u8], partial_line: bool) {
        self.counting = true;
        self.pending.clear();
        if partial_line {
            self.counter.feed(b"x");
        }
        self.counter.feed(rest);
    }

    fn feed_line(&mut self, raw: &str) {
        if !self.in_hunks {
            if raw.starts_with("@@") {
                self.in_hunks = true;
            } else {
                self.feed_header(raw);
                return;
            }
        }
        if raw.starts_with("@@") {
            // 함수 문맥 줄 끝의 CR(CRLF 파일)이 헤더의 줄 끝 문자 검사에 걸리지 않게 먼저 뗀다
            let Some(hunk) = parse_hunk_header(raw.strip_suffix('\r').unwrap_or(raw)) else { return };
            self.o = hunk.old_start.0;
            self.n = hunk.new_start.0;
            self.hunks.push(hunk);
            self.in_hunk = true;
            return;
        }
        // 내용 줄은 항상 접두 문자(' ', '+', '-', '\\')가 있으므로 "diff " 로 시작하면 다음 섹션이다.
        // 같은 경로의 종류 변경(파일↔심볼릭 링크·서브모듈)은 git 이 삭제 + 생성 두 섹션으로 나눠 내보내므로
        // 끝내지 않고 새 섹션의 헤더부터 다시 읽는다(경로 필터로 요청한 파일만 나온다)
        if raw.starts_with("diff ") {
            self.in_hunks = false;
            self.in_hunk = false;
            return;
        }
        if !self.in_hunk {
            return;
        }
        // 첫 글자는 접두. 비어 있는 줄은 접두 없는 문맥 줄로 본다
        let first = raw.chars().next();
        let rest = &raw[first.map_or(0, char::len_utf8)..];
        // CRLF 파일은 줄 끝의 \r 만 떼고 나머지는 그대로 둔다
        let text = if raw.ends_with('\r') { rest.strip_suffix('\r').unwrap_or(rest) } else { rest };
        let line = match first {
            Some('+') => {
                let n = self.n;
                self.n += 1.0;
                DiffLine { t: "+", text: text.to_string(), o: None, n: Some(JsNumber(n)) }
            }
            Some('-') => {
                let o = self.o;
                self.o += 1.0;
                DiffLine { t: "-", text: text.to_string(), o: Some(JsNumber(o)), n: None }
            }
            Some('\\') => DiffLine { t: "\\", text: js_trim(text).to_string(), o: None, n: None },
            _ => {
                let (o, n) = (self.o, self.n);
                self.o += 1.0;
                self.n += 1.0;
                DiffLine { t: " ", text: text.to_string(), o: Some(JsNumber(o)), n: Some(JsNumber(n)) }
            }
        };
        if let Some(h) = self.hunks.last_mut() {
            h.lines.push(line);
        }
        self.parsed_lines += 1;
    }

    fn feed_header(&mut self, raw: &str) {
        if raw.starts_with("diff ") {
            return;
        }
        if raw.starts_with("Binary files ") || raw.starts_with("GIT binary patch") {
            self.binary = true;
            return;
        }
        if META_PREFIXES.iter().any(|p| raw.starts_with(p)) {
            self.meta.push(raw.strip_suffix('\r').unwrap_or(raw).to_string());
        }
    }
}

/// 이미 메모리에 있는 diff 텍스트를 한 번에 파싱한다
pub fn parse_diff(buf: &[u8], path: &str, old_path: Option<&str>, opts: &DiffParseOpts) -> FileDiff {
    let mut p = DiffParser::new(path, old_path, opts);
    p.push(buf);
    p.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> FileDiff {
        parse_diff(s.as_bytes(), "x", None, &DiffParseOpts::default())
    }

    #[test]
    fn hunk_header_counts_default_to_one() {
        let d = parse("@@ -5 +7 @@\n-a\n+b\n");
        assert_eq!((d.hunks[0].old_lines.0, d.hunks[0].new_lines.0), (1.0, 1.0));
        assert_eq!(d.total_lines, 2);
    }

    #[test]
    fn header_with_line_terminator_is_not_a_hunk() {
        assert!(parse_hunk_header("@@ -1 +1 @@ a\rb").is_none());
        assert!(parse_hunk_header("@@ -1 +1 @@ a\u{2028}b").is_none());
        assert!(parse_hunk_header("@@ -1,x +1 @@").is_none());
        assert_eq!(parse_hunk_header("@@ -1 +1 @@  y").map(|h| h.header), Some(" y".to_string()));
    }

    #[test]
    fn chunk_boundaries_do_not_matter() {
        let text = "diff --git a/f b/f\n@@ -1,2 +1,2 @@ 한글\n 문맥\n-삭제\n+추가 🙂\n";
        let whole = parse(text);
        for size in 1..text.len() {
            let mut p = DiffParser::new("x", None, &DiffParseOpts::default());
            for c in text.as_bytes().chunks(size) {
                p.push(c);
            }
            assert_eq!(p.finish(), whole, "chunk={size}");
        }
    }
}
