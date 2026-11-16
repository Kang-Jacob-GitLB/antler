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

use super::is_line_terminator;

#[derive(Debug, Clone, PartialEq)]
enum Node {
    Lit(char),
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
        let prefix = nodes.iter().map_while(|n| if let Node::Lit(c) = n { Some(*c) } else { None }).collect();
        Ok(Pattern { nodes, prefix })
    }

    /// 문자열 전체가 맞는가(`^…$`)
    pub fn is_match(&self, text: &str) -> bool {
        if !text.starts_with(self.prefix.as_str()) {
            return false;
        }
        let t: Vec<char> = text.chars().collect();
        let mut ctx = Ctx { t: &t, next_end: None };
        step(&self.nodes, vec![0], &mut ctx).binary_search(&t.len()).is_ok()
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
            Ok(Node::Lit('.'))
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
            Ok(Node::Lit(c))
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

struct Ctx<'a> {
    t: &'a [char],
    /// `next_end[p]`: p 이후 첫 줄 끝 문자의 위치(없으면 글자 수). `.+` 가 p 에서 시작해 닿을 수 있는 가장 먼 위치다. 처음 필요할 때 계산한다
    next_end: Option<Vec<usize>>,
}

impl Ctx<'_> {
    fn next_end(&mut self) -> &[usize] {
        let t = self.t;
        self.next_end.get_or_insert_with(|| {
            let mut v = vec![t.len(); t.len() + 1];
            for p in (0..t.len()).rev() {
                v[p] = if is_line_terminator(t[p]) { p } else { v[p + 1] };
            }
            v
        })
    }
}

/// `reach`: 앞 노드들이 텍스트 앞 p 글자를 소비하고 끝날 수 있는 p 의 오름차순 집합. 노드열을 적용한 뒤의 집합을 돌려준다
fn step(nodes: &[Node], mut reach: Vec<usize>, ctx: &mut Ctx<'_>) -> Vec<usize> {
    for node in nodes {
        if reach.is_empty() {
            break;
        }
        reach = match node {
            Node::Lit(c) => reach.iter().filter(|&&p| ctx.t.get(p) == Some(c)).map(|p| p + 1).collect(),
            Node::AnyPlus => {
                // 한 글자 이상, 모두 줄 끝 문자가 아니어야 한다: p 에서 p+1..=next_end[p] 까지
                let ends = ctx.next_end();
                let mut next = Vec::new();
                let mut done = 0; // 이미 넣은 가장 큰 위치
                for &p in &reach {
                    for j in (p + 1).max(done + 1)..=ends[p] {
                        next.push(j);
                    }
                    done = done.max(ends[p]);
                }
                next
            }
            Node::Group { alts, optional } => {
                let mut next: Vec<usize> = if *optional { reach.clone() } else { Vec::new() };
                for alt in alts {
                    next.extend(step(alt, reach.clone(), ctx));
                }
                next.sort_unstable();
                next.dedup();
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
}
