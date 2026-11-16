//! Atlassian Document Format(ADF, Jira Cloud 의 설명·댓글 JSON) → HTML(`src/main/jira/adf.ts`).
//! 모든 텍스트·속성은 이스케이프하고, 모르는 노드는 자식만 렌더한다. 링크는 http(s) 만 싣는다.
//! 결과는 renderer 가 `sanitizeHtml` 로 한 번 더 정화한 뒤에만 화면에 넣는다(이 변환기를 유일한 방어선으로 쓰지 않는다).
//!
//! TS 와 같게 옮겼다 — 허용 태그·속성은 TS 와 같고 더 넓히지 않는다. 다른 점(모두 더 좁거나 같은 쪽):
//! - 링크 주소는 [`crate::net::url_rules::parse_url`] 로 해석한다(IDN 호스트·`blob:` 를 TS 보다 엄격하게 거른다).
//! - 출력을 UTF-16 1,000,000 단위에서 자를 때 서로게이트 쌍이 쪼개지면 U+FFFD 다(TS 는 반쪽을 그대로 둔다).
//! - 자른 끝에 미완성 태그(`<strong`)·엔티티(`&am`)가 남으면 그 자리부터 버린다(TS 는 그대로 둔다) — Rust 출력은 TS 출력의 접두사다.
//! - TS 의 예외 대체 경로(평문 한 문단)는 옮기지 않았다 — JSON 값만으로는 TS 변환이 던지지 않는다(재귀 깊이는 50 으로 묶인다).
use crate::git::commit::utf16_len;
use crate::git::fetch::slice_utf16_lossy;
use crate::net::js_date::iso_string;
use crate::net::js_value::{arr_or_empty, field, str_or_empty, to_number};
use crate::net::json::Json;
use crate::net::url_rules::parse_url;

const MAX_DEPTH: usize = 50;
const MAX_OUTPUT: usize = 1_000_000;

fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

/// http(s) 절대 주소만 통과시킨다(`u.href`)
fn safe_href(raw: Option<&Json>) -> Option<String> {
    let Some(Json::Str(t)) = raw else { return None };
    let u = parse_url(t)?;
    matches!(u.scheme(), "http" | "https").then(|| u.as_str().to_string())
}

struct Budget {
    size: usize,
}

fn render_children(n: &Json, depth: usize, b: &mut Budget) -> String {
    let mut out = String::new();
    if let Some(Json::Arr(content)) = field(Some(n), "content") {
        for c in content {
            if b.size > MAX_OUTPUT {
                break;
            }
            out.push_str(&render(c, depth + 1, b));
        }
    }
    out
}

fn attr<'a>(m: &'a Json, key: &str) -> Option<&'a Json> {
    field(field(Some(m), "attrs"), key)
}

/// 표시를 차례로 감싼다(뒤의 표시가 바깥). 문자열을 매번 다시 만들면 표시가 많을 때 제곱 시간이라 여는 태그·닫는 태그를 모았다가 한 번에 잇는다
fn apply_marks(html: String, marks: Option<&Json>) -> String {
    let (mut opens, mut closes): (Vec<String>, Vec<&'static str>) = (Vec::new(), Vec::new());
    for m in arr_or_empty(marks) {
        if !matches!(m, Json::Obj(_) | Json::Arr(_)) {
            continue;
        }
        let (open, close): (String, &'static str) = match str_or_empty(field(Some(m), "type")) {
            "strong" => ("<strong>".into(), "</strong>"),
            "em" => ("<em>".into(), "</em>"),
            "code" => ("<code>".into(), "</code>"),
            "strike" => ("<s>".into(), "</s>"),
            "underline" => ("<u>".into(), "</u>"),
            "subsup" => {
                if matches!(attr(m, "type"), Some(Json::Str(t)) if t == "sub") {
                    ("<sub>".into(), "</sub>")
                } else {
                    ("<sup>".into(), "</sup>")
                }
            }
            "link" => match safe_href(attr(m, "href")) {
                Some(href) => (format!("<a href=\"{}\">", esc(&href)), "</a>"),
                None => continue,
            },
            _ => continue,
        };
        opens.push(open);
        closes.push(close);
    }
    if opens.is_empty() {
        return html;
    }
    let mut out = String::with_capacity(html.len() + opens.iter().map(String::len).sum::<usize>() + closes.iter().map(|c| c.len()).sum::<usize>());
    for o in opens.iter().rev() {
        out.push_str(o);
    }
    out.push_str(&html);
    for c in &closes {
        out.push_str(c);
    }
    out
}

/// `Number.isInteger(x)` 이면 그 값
fn integer(x: f64) -> Option<i64> {
    (x.is_finite() && x.fract() == 0.0 && x.abs() < 9e15).then_some(x as i64)
}

fn render(n: &Json, depth: usize, b: &mut Budget) -> String {
    // `!n || typeof n !== 'object' || depth > MAX_DEPTH` — 배열은 객체지만 `type`·`content` 가 없어 빈 문자열이 된다
    if !matches!(n, Json::Obj(_)) || depth > MAX_DEPTH {
        return String::new();
    }
    let out = match str_or_empty(field(Some(n), "type")) {
        "text" => apply_marks(esc(str_or_empty(field(Some(n), "text"))), field(Some(n), "marks")),
        "hardBreak" => "<br>".to_string(),
        "paragraph" => format!("<p>{}</p>", render_children(n, depth, b)),
        "heading" => {
            let lv = integer(to_number(attr(n, "level"))).filter(|l| (1..=6).contains(l)).unwrap_or(3);
            format!("<h{lv}>{}</h{lv}>", render_children(n, depth, b))
        }
        "bulletList" | "taskList" => format!("<ul>{}</ul>", render_children(n, depth, b)),
        "orderedList" => {
            let start = to_number(attr(n, "order"));
            let at = integer(start).filter(|s| *s > 1 && *s < 1_000_000).map_or(String::new(), |s| format!(" start=\"{s}\""));
            format!("<ol{at}>{}</ol>", render_children(n, depth, b))
        }
        "listItem" => format!("<li>{}</li>", render_children(n, depth, b)),
        "taskItem" => {
            let done = matches!(attr(n, "state"), Some(Json::Str(t)) if t == "DONE");
            format!("<li>{} {}</li>", if done { '☑' } else { '☐' }, render_children(n, depth, b))
        }
        "blockquote" | "panel" => format!("<blockquote>{}</blockquote>", render_children(n, depth, b)),
        "codeBlock" => {
            let text: String = arr_or_empty(field(Some(n), "content")).iter().map(|c| str_or_empty(field(Some(c), "text"))).collect();
            format!("<pre><code>{}</code></pre>", esc(&text))
        }
        "rule" => "<hr>".to_string(),
        "table" => format!("<table><tbody>{}</tbody></table>", render_children(n, depth, b)),
        "tableRow" => format!("<tr>{}</tr>", render_children(n, depth, b)),
        "tableHeader" => format!("<th>{}</th>", render_children(n, depth, b)),
        "tableCell" => format!("<td>{}</td>", render_children(n, depth, b)),
        "expand" | "nestedExpand" => {
            let title = str_or_empty(attr(n, "title"));
            let summary = if title.is_empty() { String::new() } else { format!("<summary>{}</summary>", esc(title)) };
            format!("<details>{summary}{}</details>", render_children(n, depth, b))
        }
        "mention" => {
            let t = str_or_empty(attr(n, "text"));
            format!("<strong>{}</strong>", esc(if t.is_empty() { "@user" } else { t }))
        }
        "emoji" => {
            let t = str_or_empty(attr(n, "text"));
            esc(if t.is_empty() { str_or_empty(attr(n, "shortName")) } else { t })
        }
        "status" => format!("<mark>{}</mark>", esc(str_or_empty(attr(n, "text")))),
        "date" => {
            // 범위를 벗어난 값은 Invalid Date 라 TS 는 빈 문자열이다
            iso_string(to_number(attr(n, "timestamp"))).map_or(String::new(), |iso| esc(&iso.chars().take(10).collect::<String>()))
        }
        "inlineCard" | "blockCard" => safe_href(attr(n, "url")).map_or(String::new(), |h| format!("<a href=\"{}\">{}</a>", esc(&h), esc(&h))),
        // 첨부 이미지는 인증이 필요하고 CSP 상 불러올 수도 없다 — 자리만 표시한다
        "media" => "<em>[첨부]</em> ".to_string(),
        _ => render_children(n, depth, b),
    };
    b.size += utf16_len(&out);
    out
}

/// 상한으로 자른 끝에 남은 미완성 태그(`<strong`)와 미완성 엔티티(`&am`)를 버린다.
/// 본문 텍스트의 `<`·`&` 는 모두 이스케이프되므로 끝의 `<` 뒤에 `>` 가 없으면 잘린 태그고, `&` 뒤에 `;` 가 없으면 잘린 엔티티다
fn drop_incomplete_tail(mut html: String) -> String {
    if let Some(i) = html.rfind('<') {
        if !html[i..].contains('>') {
            html.truncate(i);
        }
    }
    if let Some(i) = html.rfind('&') {
        if !html[i..].contains(';') {
            html.truncate(i);
        }
    }
    html
}

/// ADF 문서(또는 노드)를 HTML 로. 해석할 수 없는 입력은 빈 문자열
pub fn adf_to_html(doc: &Json) -> String {
    if !matches!(doc, Json::Obj(_) | Json::Arr(_)) {
        return String::new();
    }
    let html = render(doc, 0, &mut Budget { size: 0 });
    if utf16_len(&html) > MAX_OUTPUT {
        drop_incomplete_tail(slice_utf16_lossy(&html, MAX_OUTPUT))
    } else {
        html
    }
}
