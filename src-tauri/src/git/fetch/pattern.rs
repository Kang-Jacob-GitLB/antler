//! 허용 목록 정규식(`ALLOWED_KEYS`·`REMOTE_URL_KEY`·`SUBMODULE_URL_KEY`)을 해석하는 작은 매처. regex 크레이트를 쓰지 않는다.
//!
//! TS 정규식 소스 문자열을 그대로 받아, 아래 **닫힌 문법**만 안다(`tests/goldenFetch.test.ts` 의 `grammarError` 가 같은 문법을 검사한다 —
//! 문법 밖 토큰이 들어오면 TS·Rust 양쪽 테스트가 실패하고, 그 정규식은 새로 옮겨야 한다).
//! - `^` 로 시작하고 `$` 로 끝난다(양 끝에만, 플래그 없음)
//! - `\.`, `.+`, 리터럴 `[A-Za-z0-9_-]`
//! - `( 대안 | 대안 … )` 와 `(…)?`(`|` 가 든 그룹에는 `?` 를 붙이지 않는다). 그룹은 중첩하지 않는다
//!
//! `.` 는 JS 와 같이 `\n`·`\r`·U+2028·U+2029 를 맞추지 않는다. `$` 는 문자열의 맨 끝이다.
//! 판정은 위치 집합을 노드마다 전진시키는 방식이라 입력 길이에 선형이고 재귀 깊이가 일정하다(큰 입력에서 스택이 넘치지 않는다).
//! 위치는 입력의 **바이트 오프셋**(항상 글자 경계)이고 집합은 비트셋이라, 메모리는 입력 바이트 수의 1/8 짜리 집합 몇 개뿐이다
//! (악성 설정의 수천만 자 키에서도 글자 배열·위치 배열을 만들지 않는다).

use super::is_line_terminator;

#[derive(Debug, Clone, PartialEq)]
enum Node {
    /// 리터럴(문법상 ASCII 뿐이라 바이트 하나)
    Lit(u8),
    /// `.+`
    AnyPlus,
    /// `( … | … )` 또는 `( … )?`
    Group { alts: Vec<Vec<Node>>, optional: bool },
}

#[derive(Debug, Clone)]
pub struct Pattern {
    nodes: Vec<Node>,
    /// 맨 앞의 리터럴들 — 텍스트가 이것으로 시작하지 않으면 글자 단위 판정 없이 거른다(긴 입력을 13개 패턴에 일일이 돌리지 않으려는 것뿐, 결과는 같다)
    prefix: String,
}

impl Pattern {
    /// 문법에 맞으면 컴파일하고, 아니면 사유를 돌려준다
    pub fn compile(source: &str) -> Result<Pattern, String> {
        let body = source.strip_prefix('^').and_then(|s| s.strip_suffix('$')).ok_or("^…$ 가 아님")?;
        let chars: Vec<char> = body.chars().collect();
        let mut i = 0;
        let mut nodes = Vec::new();
        while i < chars.len() {
            if chars[i] == '(' {
                nodes.push(parse_group(&chars, &mut i)?);
            } else {
                nodes.push(parse_atom(&chars, &mut i)?);
            }
        }
        let prefix = nodes.iter().map_while(|n| if let Node::Lit(c) = n { Some(char::from(*c)) } else { None }).collect();
        Ok(Pattern { nodes, prefix })
    }

    /// 문자열 전체가 맞는가(`^…$`)
    pub fn is_match(&self, text: &str) -> bool {
        if !text.starts_with(self.prefix.as_str()) {
            return false;
        }
        let mut start = Positions::new(text.len() + 1);
        start.insert(0);
        let mut ctx = Ctx { text, line: None };
        step(&self.nodes, start, &mut ctx).contains(text.len())
    }
}

/// 리터럴·`\.`·`.+` 한 개. 그룹 안팎에서 같이 쓴다
fn parse_atom(chars: &[char], i: &mut usize) -> Result<Node, String> {
    let c = chars[*i];
    match c {
        '\\' => {
            if chars.get(*i + 1) != Some(&'.') {
                return Err(format!("이스케이프 \\{}", chars.get(*i + 1).copied().unwrap_or(' ')));
            }
            *i += 2;
            Ok(Node::Lit(b'.'))
        }
        '.' => {
            if chars.get(*i + 1) != Some(&'+') {
                return Err("`.+` 가 아닌 `.`".into());
            }
            *i += 2;
            Ok(Node::AnyPlus)
        }
        c if c.is_ascii_alphanumeric() || c == '_' || c == '-' => {
            *i += 1;
            Ok(Node::Lit(c as u8))
        }
        c => Err(format!("문법 밖 토큰 {c}")),
    }
}

fn parse_group(chars: &[char], i: &mut usize) -> Result<Node, String> {
    *i += 1; // (
    if chars.get(*i) == Some(&'?') {
        return Err("(? 확장 그룹".into());
    }
    let mut alts: Vec<Vec<Node>> = vec![Vec::new()];
    loop {
        match chars.get(*i) {
            None => return Err("닫히지 않은 그룹".into()),
            Some('(') => return Err("중첩 그룹".into()),
            Some(')') => {
                *i += 1;
                break;
            }
            Some('|') => {
                *i += 1;
                alts.push(Vec::new());
            }
            Some(_) => {
                let atom = parse_atom(chars, i)?;
                alts.last_mut().expect("대안이 하나는 있다").push(atom);
            }
        }
    }
    if alts.iter().any(Vec::is_empty) {
        return Err("빈 대안".into());
    }
    let optional = chars.get(*i) == Some(&'?');
    if optional {
        if alts.len() > 1 {
            return Err("|가 든 그룹의 ?".into());
        }
        *i += 1;
    }
    Ok(Node::Group { alts, optional })
}

/// 바이트 오프셋(0..=len)의 집합. 비트 하나가 위치 하나다
#[derive(Clone)]
struct Positions {
    words: Vec<u64>,
}

impl Positions {
    fn new(bits: usize) -> Self {
        Self { words: vec![0; bits.div_ceil(64)] }
    }

    /// 같은 크기의 빈 집합
    fn empty_like(&self) -> Self {
        Self { words: vec![0; self.words.len()] }
    }

    fn insert(&mut self, i: usize) {
        self.words[i / 64] |= 1 << (i % 64);
    }

    fn contains(&self, i: usize) -> bool {
        self.words.get(i / 64).is_some_and(|w| w >> (i % 64) & 1 == 1)
    }

    fn is_empty(&self) -> bool {
        self.words.iter().all(|&w| w == 0)
    }

    fn union_with(&mut self, other: &Positions) {
        for (a, b) in self.words.iter_mut().zip(&other.words) {
            *a |= b;
        }
    }

    /// 들어 있는 위치를 오름차순으로
    fn ones(&self) -> impl Iterator<Item = usize> + '_ {
        self.words.iter().enumerate().flat_map(|(w, &word)| {
            let mut word = word;
            std::iter::from_fn(move || {
                if word == 0 {
                    return None;
                }
                let bit = word.trailing_zeros() as usize;
                word &= word - 1;
                Some(w * 64 + bit)
            })
        })
    }
}

struct Ctx<'a> {
    text: &'a str,
    /// 마지막으로 찾은 줄 끝: `(from, end)` — from 이상 end 이하의 위치에서 시작하는 `.+` 는 end 를 넘지 못한다. 위치가 오름차순으로 오는 동안 다시 찾지 않는다
    line: Option<(usize, usize)>,
}

impl Ctx<'_> {
    /// p(글자 경계) 이후 첫 줄 끝 문자의 위치(없으면 입력 길이). `.+` 가 p 에서 시작해 닿을 수 있는 가장 먼 위치다
    fn line_end(&mut self, p: usize) -> usize {
        if let Some((from, end)) = self.line {
            if (from..=end).contains(&p) {
                return end;
            }
        }
        let end = self.text[p..].find(is_line_terminator).map_or(self.text.len(), |i| p + i);
        self.line = Some((p, end));
        end
    }
}

/// `reach`: 앞 노드들이 텍스트 앞부분을 소비하고 끝날 수 있는 위치의 집합. 노드열을 적용한 뒤의 집합을 돌려준다.
/// 위치는 언제나 글자 경계다(리터럴은 ASCII 한 바이트씩 전진하고 `.+` 는 경계에만 놓는다)
fn step(nodes: &[Node], mut reach: Positions, ctx: &mut Ctx<'_>) -> Positions {
    for node in nodes {
        if reach.is_empty() {
            break;
        }
        reach = match node {
            Node::Lit(c) => {
                let bytes = ctx.text.as_bytes();
                let mut next = reach.empty_like();
                for p in reach.ones() {
                    if bytes.get(p) == Some(c) {
                        next.insert(p + 1);
                    }
                }
                next
            }
            Node::AnyPlus => {
                // 한 글자 이상, 모두 줄 끝 문자가 아니어야 한다: p 에서 p 다음 글자 경계부터 line_end(p) 까지
                let mut next = reach.empty_like();
                let mut done = 0; // 이미 넣은 가장 큰 위치
                for p in reach.ones() {
                    let end = ctx.line_end(p);
                    for j in (p + 1).max(done + 1)..=end {
                        if ctx.text.is_char_boundary(j) {
                            next.insert(j);
                        }
                    }
                    done = done.max(end);
                }
                next
            }
            Node::Group { alts, optional } => {
                let mut next = if *optional { reach.clone() } else { reach.empty_like() };
                for alt in alts {
                    next.union_with(&step(alt, reach.clone(), ctx));
                }
                next
            }
        };
    }
    reach
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(src: &str, text: &str) -> bool {
        Pattern::compile(src).unwrap_or_else(|e| panic!("{src}: {e}")).is_match(text)
    }

    #[test]
    fn 문법_밖_토큰은_컴파일되지_않는다() {
        for bad in [
            r"^a\d$", "^a[bc]$", "^a*$", "^a.$", "^a.*$", "^a|b$", "^(a|b$", "^a)$", "^(?:a)$", "^((a))$", "^(a|b)?$", "^(|a)$", r"^\w$", "^a{2}$", "^a b$", "a$", "^a", "^é$",
        ] {
            assert!(Pattern::compile(bad).is_err(), "{bad}");
        }
        for good in [r"^a\.b$", r"^a\..+\.(b|c-d)$", r"^a\.(.+\.)?(b|c)$", "^(a)?$"] {
            assert!(Pattern::compile(good).is_ok(), "{good}");
        }
    }

    #[test]
    fn 점은_줄_끝_문자를_맞추지_않고_달러는_맨_끝만_맞춘다() {
        let re = r"^remote\..+\.(url|pushurl)$";
        assert!(m(re, "remote.a.url"));
        assert!(m(re, "remote.a.b.pushurl"));
        assert!(!m(re, "remote..url"));
        assert!(!m(re, "remote.a.url\n"));
        for lt in ['\n', '\r', '\u{2028}', '\u{2029}'] {
            assert!(!m(re, &format!("remote.a{lt}b.url")), "{lt:?}");
        }
        assert!(m(re, "remote.\u{feff}.url"));
        assert!(m(re, "remote.😀.url"));
    }

    #[test]
    fn 선택_그룹과_대안() {
        let re = r"^submodule\.(.+\.)?(url|active)$";
        assert!(m(re, "submodule.url"));
        assert!(m(re, "submodule.active"));
        assert!(m(re, "submodule.a.url"));
        assert!(m(re, "submodule.a.b.active"));
        assert!(!m(re, "submodule..url"));
        assert!(!m(re, "submodule.a.urlx"));
    }

    #[test]
    fn 큰_입력에서도_스택이_넘치지_않는다() {
        let big = "x".repeat(2_000_000);
        assert!(m(r"^remote\..+\.(url|pushurl)$", &format!("remote.{big}.url")));
        assert!(!m(r"^remote\..+\.(url|pushurl)$", &format!("remote.{big}.urlx")));
    }

    #[test]
    fn 여러_바이트_글자와_줄_끝_문자가_섞인_큰_입력도_글자_경계로_맞춘다() {
        // 3바이트 글자(한)·4바이트 글자(😀)·U+2028(3바이트 줄 끝 문자)이 섞여도 위치가 글자 경계에만 놓인다
        let re = r"^remote\..+\.(url|pushurl)$";
        let mixed = "한😀a".repeat(500_000);
        assert!(m(re, &format!("remote.{mixed}.url")));
        assert!(m(re, &format!("remote.{mixed}.pushurl")));
        assert!(!m(re, &format!("remote.{mixed}\u{2028}{mixed}.url")));
        assert!(!m(re, &format!("remote.{mixed}.urlx")));
        assert!(m(r"^remote\.(.+\.)?(url|active)$", "remote.😀.url"));
        assert!(!m(r"^remote\.(.+\.)?(url|active)$", "remote.\u{2029}.url"));
    }
}
