//! GitHub·Jira 순수 로직의 골든 패리티 테스트. `tests/golden/net/*.json`(TS 가 기준: `UPDATE_GOLDEN=1 npx vitest run tests/goldenNet.test.ts`)을
//! 모두 읽어 Rust 이식(`antler_lib::{net, github, jira}`)이 같은 결과를 내는지 본다. **건너뛰는 골든이 없다** — 처리기 없는 파일이 있으면 실패한다.
//! 안전 방향 확인: URL 판정 격자 전체에서 "Rust 허용 ⇒ TS 허용"을 따로 센다(`url_격자_판정이_ts_와_같다_허용_범위는_ts_안이다`).
//! 이 파일은 네트워크를 쓰지 않는다. 알려진 차이는 각각 `차이_…` 이름의 테스트가 단언한다.

use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use antler_lib::github::device_rules::{interpret_start, DeviceError, PollState, PollStep, EXPIRED_MESSAGE};
use antler_lib::github::gh_env::gh_env;
use antler_lib::github::validate as gh;
use antler_lib::jira::validate::{self as jira, SiteOptions};
use antler_lib::jira::{auth_header, JiraKind};
use antler_lib::net::http_error::{
    github_body_message, github_http_error, has_next_link, jira_body_message, jira_http_error, parse_http_date, parse_rate, parse_retry_after, RateHeaders,
};
use antler_lib::net::json::{self, js_number, js_number_string, Json, MAX_DEPTH};
use antler_lib::net::redact::{redact_token, usable_token, SecretString};
use antler_lib::net::url_rules::{self, image_hop, in_site, is_public_avatar_host, parse_url, resolve_location, ImageHop};
use antler_lib::net::HAND_WRITTEN;
use serde_json::{json, Value};

fn golden_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().expect("src-tauri 의 상위").join("tests").join("golden").join("net")
}

fn golden(name: &str) -> Value {
    let path = golden_dir().join(name);
    let text = fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{name}: {e}"))
}

fn arr<'a>(v: &'a Value, what: &str) -> &'a Vec<Value> {
    let a = v.as_array().unwrap_or_else(|| panic!("{what} 가 배열이 아니다"));
    assert!(!a.is_empty(), "{what} 가 비어 있다(골든을 건너뛴 것과 같다)");
    a
}

fn s(v: &Value) -> &str {
    v.as_str().unwrap_or_else(|| panic!("문자열이 아니다: {v}"))
}

fn opt_s(v: &Value) -> Option<&str> {
    v.as_str()
}

/// 숫자를 전부 f64 로 맞춘다(`5` 와 `5.0` 은 같은 값이다)
fn norm(v: &Value) -> Value {
    match v {
        Value::Number(n) => json!(n.as_f64().expect("f64")),
        Value::Array(a) => Value::Array(a.iter().map(norm).collect()),
        Value::Object(o) => Value::Object(o.iter().map(|(k, x)| (k.clone(), norm(x))).collect()),
        other => other.clone(),
    }
}

/// JSON 에 담을 수 없는 숫자는 문자열로(골든과 같은 약속)
fn enc(n: f64) -> Value {
    if n.is_nan() {
        json!("NaN")
    } else if n == f64::INFINITY {
        json!("Infinity")
    } else if n == f64::NEG_INFINITY {
        json!("-Infinity")
    } else if n == 0.0 && n.is_sign_negative() {
        json!("-0")
    } else {
        json!(n)
    }
}

fn dec(v: &Value) -> f64 {
    match v {
        Value::Number(n) => n.as_f64().expect("f64"),
        Value::String(t) => match t.as_str() {
            "NaN" => f64::NAN,
            "Infinity" => f64::INFINITY,
            "-Infinity" => f64::NEG_INFINITY,
            "-0" => -0.0,
            other => panic!("숫자 표기가 아니다: {other}"),
        },
        other => panic!("숫자가 아니다: {other}"),
    }
}

fn same_num(a: f64, b: f64) -> bool {
    (a.is_nan() && b.is_nan()) || a == b
}

/// 어긋난 항목을 모았다가 한 번에 보고한다(앞 몇 개만)
#[derive(Default)]
struct Diffs {
    checked: usize,
    bad: Vec<String>,
    /// Rust 는 허용, TS 는 거부 — 보안 방향의 어긋남
    unsound: Vec<String>,
    /// TS 는 받지만 Rust 는 거부한 입력(알려진 차이 — 호출자가 목록과 대조한다)
    stricter: BTreeSet<String>,
    /// 알려진 차이 목록의 주소 — 둘 다 거부할 때 오류 문구가 달라도(Rust 는 호스트 검사에서 먼저 거부한다) 넘어간다
    known: BTreeSet<String>,
}

impl Diffs {
    fn eq(&mut self, what: impl FnOnce() -> String, got: &Value, expected: &Value) {
        self.checked += 1;
        if norm(got) != norm(expected) {
            self.bad.push(format!("{} → Rust {got}, TS {expected}", what()));
        }
    }

    /// `eq` 와 같되, Rust 가 거부하고 TS 만 받은 어긋남은 오류가 아니라 `stricter` 에 모은다
    fn eq_s(&mut self, raw: &str, what: impl FnOnce() -> String, got: &Value, expected: &Value) {
        let rejects = |v: &Value| v.is_null() || v.get("e").is_some() || v == "throw";
        self.checked += 1;
        if norm(got) != norm(expected) {
            if rejects(got) && rejects(expected) && self.known.contains(raw) {
                return;
            }
            if rejects(got) && !rejects(expected) {
                self.stricter.insert(raw.to_string());
            } else {
                self.bad.push(format!("{} → Rust {got}, TS {expected}", what()));
            }
        }
    }

    fn unsound(&mut self, what: impl FnOnce() -> String) {
        self.unsound.push(what());
    }

    fn finish(self, name: &str) {
        assert!(self.checked > 0, "{name}: 확인한 항목이 0 개다(골든을 건너뜀)");
        assert!(self.unsound.is_empty(), "{name}: Rust 허용 ⇒ TS 허용 위반 {}건, 예: {:?}", self.unsound.len(), &self.unsound[..self.unsound.len().min(5)]);
        assert!(self.bad.is_empty(), "{name}: TS 와 다른 결과 {}건 / {}, 예: {:?}", self.bad.len(), self.checked, &self.bad[..self.bad.len().min(30)]);
    }
}

fn res<T: Into<Value>>(r: Result<T, String>) -> Value {
    match r {
        Ok(v) => json!({ "ok": v.into() }),
        Err(e) => json!({ "err": e }),
    }
}

const HANDLED: [&str; 5] = ["validate.json", "urls.json", "http.json", "device.json", "env.json"];

#[test]
fn 골든_파일마다_처리기가_있다() {
    let mut found: Vec<String> = fs::read_dir(golden_dir())
        .expect("tests/golden/net")
        .map(|e| e.expect("항목").file_name().to_string_lossy().into_owned())
        // test-map.json 은 TS 테스트 → Rust 테스트 이식 대응표(손글씨)라 골든이 아니다
        .filter(|n| n.ends_with(".json") && n != "test-map.json")
        .collect();
    found.sort();
    let mut handled: Vec<String> = HANDLED.iter().map(ToString::to_string).collect();
    handled.sort();
    assert_eq!(found, handled, "처리기가 없는 골든이 있거나 골든이 없다");
}

// ── 검증(validate) ────────────────────────────────────────────────────────

fn opts(loop_: &Value) -> SiteOptions {
    SiteOptions { allow_insecure_loopback: loop_.as_bool().expect("loop") }
}

fn check_cases(g: &Value, key: &str, f: impl Fn(&Value, &Value) -> Value) {
    let mut d = Diffs::default();
    for c in arr(&g[key], key) {
        d.eq(|| format!("{key}({})", c["in"]), &f(&c["in"], c), &c["out"]);
    }
    d.finish(key);
}

#[test]
fn 손으로_옮긴_정규식_소스가_골든과_같다() {
    let g = golden("validate.json");
    let list = arr(&g["handWritten"], "handWritten");
    assert_eq!(list.len(), HAND_WRITTEN.len(), "손으로 옮긴 정규식 개수");
    for (e, (name, snippet)) in list.iter().zip(HAND_WRITTEN) {
        assert_eq!(s(&e["name"]), *name);
        assert_eq!(s(&e["snippet"]), *snippet, "{name}: TS 정규식이 바뀌었다 — 구현(손 판정)을 다시 본다");
    }
    let c = &g["consts"];
    assert_eq!(c["PAGE_SIZE"], json!(jira::PAGE_SIZE));
    assert_eq!(c["MAX_PAGES"], json!(jira::MAX_PAGES));
    assert_eq!(c["MAX_JQL_LEN"], json!(jira::MAX_JQL_LEN));
    assert_eq!(c["MAX_WORKLOG_USERS"], json!(jira::MAX_WORKLOG_USERS));
    assert_eq!(c["MAX_WORKLOG_DAYS"], json!(jira::MAX_WORKLOG_DAYS));
}

#[test]
fn validate_github_가_ts_와_같다() {
    let g = golden("validate.json");
    check_cases(&g, "ghParseNumber", |i, _| res(gh::parse_number(i)));
    check_cases(&g, "ghParseId", |i, _| res(gh::parse_id(i, "저장소")));
    check_cases(&g, "ghParseListQuery", |i, _| {
        res(gh::parse_list_query(i).map(|q| {
            let mut o = json!({ "state": q.state, "page": q.page });
            if let Some(l) = q.label {
                o["label"] = json!(l);
            }
            if let Some(a) = q.assignee {
                o["assignee"] = json!(a);
            }
            o
        }))
    });
    check_cases(&g, "ghParsePat", |i, _| res(gh::parse_pat(i).map(|t| t.expose().to_string())));
    check_cases(&g, "ghParseAvatarAuthors", |i, _| res(gh::parse_avatar_authors(i).map(|v| v.into_iter().map(|a| json!({ "email": a.email, "sha": a.sha })).collect::<Vec<_>>())));
    check_cases(&g, "ghParseRunId", |i, _| res(gh::parse_run_id(i)));
    check_cases(&g, "ghParseSha", |i, _| res(gh::parse_sha(i)));
    check_cases(&g, "ghParseCheckShas", |i, _| res(gh::parse_check_shas(i)));
    check_cases(&g, "ghParsePage", |i, _| res(gh::parse_page(i)));
    check_cases(&g, "ghAllowedExternalUrl", |i, _| json!({ "ok": gh::allowed_external_url(i) }));
}

#[test]
fn validate_jira_가_ts_와_같다() {
    let g = golden("validate.json");
    check_cases(&g, "jiraParseSiteUrl", |i, c| res(jira::parse_site_url(i, opts(&c["loop"]))));
    check_cases(&g, "jiraParseConnectInput", |i, c| {
        res(jira::parse_connect_input(i, opts(&c["loop"])).map(|x| {
            json!({ "kind": if x.kind == JiraKind::Cloud { "cloud" } else { "server" }, "siteUrl": x.site_url, "email": x.email, "token": x.token.expose() })
        }))
    });
    check_cases(&g, "jiraParseIssueKey", |i, _| res(jira::parse_issue_key(i)));
    check_cases(&g, "jiraParseTransitionId", |i, _| res(jira::parse_transition_id(i)));
    check_cases(&g, "jiraParseSearchQuery", |i, _| {
        res(jira::parse_search_query(i).map(|q| {
            let mut o = json!({ "jql": q.jql, "page": q.page });
            if let Some(c) = q.cursor {
                o["cursor"] = json!(c);
            }
            o
        }))
    });
    check_cases(&g, "jiraParseWorklogQuery", |i, _| {
        res(jira::parse_worklog_query(i).map(|q| {
            let mut o = json!({ "from": q.from, "to": q.to, "users": q.users });
            if q.include_me {
                o["includeMe"] = json!(true);
            }
            o
        }))
    });
    check_cases(&g, "jiraJqlQuote", |i, _| res(jira::jql_quote(s(i))));
    check_cases(&g, "jiraParseUserSearch", |i, _| res(jira::parse_user_search(i)));
    // allowedJiraUrl 은 {ok} 로 싸지 않고 값(문자열·null)을 그대로 적었다
    let mut d = Diffs::default();
    for c in arr(&g["jiraAllowedJiraUrl"], "jiraAllowedJiraUrl") {
        let got = jira::allowed_jira_url(&c["in"], opt_s(&c["site"]), opts(&c["loop"]));
        let expected = c["out"].as_str();
        if got.is_some() && expected.is_none() {
            d.unsound(|| format!("allowedJiraUrl({}, {}) → Rust {got:?}, TS null", c["in"], c["site"]));
        }
        d.eq(|| format!("allowedJiraUrl({}, {}, {})", c["in"], c["site"], c["loop"]), &json!(got), &c["out"]);
    }
    d.finish("jiraAllowedJiraUrl");
}

// ── URL ───────────────────────────────────────────────────────────────────

const SITE_A: &str = "https://a.atlassian.net";
const SITE_B: &str = "https://h.example.com/jira";

/// 격자에서 TS 는 받지만 Rust 는 거부하는 주소(더 엄격한 쪽의 알려진 차이): 잘못된 punycode, UTS 46 처리 방식이 갈리는 문자(ẞ)가 든 IDN 호스트
const KNOWN_STRICTER_URLS: [&str; 17] = [
    // blob: 은 받지 않는다(origin 이 안쪽 주소의 것). 비 ASCII 바이트의 퍼센트 이스케이프가 든 호스트(`%C3%A9`)도 받지 않는다
    "blob:https://a.atlassian.net/uuid",
    "blob:https://aӀb.example.com/uuid",
    "blob:https://h.example.com/jira/uuid",
    "BLOB:https://h.example.com/jira/x",
    "https://a%c3%a9b.example.com",
    "https://u%C3%A9@a.example.com",
    "https://xn--github-.com",
    "https://ẞ.example.com",
    "https://ẞ.ẞ",
    "https://ŉ.example.com",
    "https://ꞎ.example.com",
    "https://№.example.com",
    "https://①.example.com",
    "https://㎒.example.com",
    "https://ﬁ.example.com",
    "https://a\u{feff}.com",
    "https://hello\u{200b}world.com",
];

/// 판정 격자: 같은 주소 문자열에 URL 직렬화·사이트 주소·외부 링크·사이트 안·Jira 링크 판정을 모두 돌린다.
/// TS 가 거부하는데 Rust 가 허용하는 경우가 하나라도 있으면 실패한다(보안 방향). 어긋난 모든 칸을 모아 한 번에 보인다
#[test]
fn url_격자_판정이_ts_와_같다_허용_범위는_ts_안이다() {
    let g = golden("urls.json");
    let mut d = Diffs { known: KNOWN_STRICTER_URLS.iter().map(ToString::to_string).collect(), ..Diffs::default() };
    for row in arr(&g["grid"], "grid") {
        let r = s(&row["r"]);
        let raw = json!(r);
        // 직렬화
        let href = parse_url(r).map(|u| u.to_string());
        if href.is_some() && row["h"].is_null() {
            d.unsound(|| format!("URL {r:?} → Rust 파싱, TS 거부"));
        }
        d.eq_s(r, || format!("href({r:?})"), &json!(href), &row["h"]);
        // 사이트 주소(루프백 없음·있음)
        for (key, loop_) in [("s", false), ("l", true)] {
            let got = jira::parse_site_url(&raw, SiteOptions { allow_insecure_loopback: loop_ });
            let expected = &row[key];
            if got.is_ok() && expected.get("e").is_some() {
                d.unsound(|| format!("parseSiteUrl({r:?}, loop={loop_}) → Rust Ok, TS 거부"));
            }
            let got_v = match got {
                Ok(v) => json!(v),
                Err(e) => json!({ "e": e }),
            };
            d.eq_s(r, || format!("parseSiteUrl({r:?}, loop={loop_})"), &got_v, expected);
        }
        // 외부 링크
        let ext = gh::allowed_external_url(&raw);
        if ext.is_some() && row["x"].is_null() {
            d.unsound(|| format!("allowedExternalUrl({r:?}) → Rust {ext:?}, TS null"));
        }
        d.eq_s(r, || format!("allowedExternalUrl({r:?})"), &json!(ext), &row["x"]);
        // 사이트 안
        for (key, site) in [("a", SITE_A), ("b", SITE_B)] {
            let got = match parse_url(r) {
                None => json!("throw"),
                Some(u) => json!(in_site(&u, site)),
            };
            if got == json!(true) && row[key] != json!(true) {
                d.unsound(|| format!("inSite({r:?}, {site}) → Rust true, TS {}", row[key]));
            }
            d.eq_s(r, || format!("inSite({r:?}, {site})"), &got, &row[key]);
        }
        // Jira 링크
        let j = jira::allowed_jira_url(&raw, Some(SITE_B), SiteOptions::default());
        if j.is_some() && row["j"].is_null() {
            d.unsound(|| format!("allowedJiraUrl({r:?}, {SITE_B}) → Rust {j:?}, TS null"));
        }
        d.eq_s(r, || format!("allowedJiraUrl({r:?}, {SITE_B})"), &json!(j), &row["j"]);
        let k = jira::allowed_jira_url(&raw, Some("http://127.0.0.1:8080/jira"), SiteOptions { allow_insecure_loopback: true });
        if k.is_some() && row["k"].is_null() {
            d.unsound(|| format!("allowedJiraUrl({r:?}, loopback) → Rust {k:?}, TS null"));
        }
        d.eq_s(r, || format!("allowedJiraUrl({r:?}, loopback)"), &json!(k), &row["k"]);
    }
    // TS 만 받는 입력(알려진 차이): 잘못된 punycode 와 UTS 46 이 갈리는 문자의 IDN 호스트. 목록이 실제와 같아야 한다(낡은 항목·새 차이 모두 실패)
    let known: BTreeSet<String> = KNOWN_STRICTER_URLS.iter().map(ToString::to_string).collect();
    assert_eq!(std::mem::take(&mut d.stricter), known, "TS 만 받는 입력 목록이 달라졌다 — 새 차이면 원인을 보고 KNOWN_STRICTER_URLS 와 문서(docs/tauri.md)에 적는다");
    d.finish("url 격자");
}

/// URL 의 호스트를 유니코드로 되돌린다(`xn--` 레이블 해독)
fn unicode_host(u: &url::Url) -> String {
    u.host_str()
        .unwrap_or_default()
        .split('.')
        .map(|l| match l.strip_prefix("xn--") {
            Some(p) => url_rules::punycode_decode(p).map_or_else(|| l.to_string(), |cs| cs.into_iter().collect()),
            None => l.to_string(),
        })
        .collect::<Vec<_>>()
        .join(".")
}

/// IDN 호스트에서 Rust 가 받는 범위(`IDNA_SAFE_RANGES`)의 모든 코드 포인트를 TS(`new URL`, Node 의 Unicode 판)와 대조한다:
/// Rust 가 받으면 TS 도 받고 같은 호스트로 풀려야 한다(새 Unicode 판의 문자를 Rust 만 받는 일이 없다). TS 만 받는 문자는 알려진 차이(더 엄격)로 센다
#[test]
fn idna_허용_범위의_모든_문자가_ts_와_같은_호스트로_풀린다() {
    let g = golden("urls.json");
    let idna = &g["idna"];
    let ranges: Vec<(u32, u32)> = arr(&idna["ranges"], "ranges").iter().map(|r| (r[0].as_u64().unwrap() as u32, r[1].as_u64().unwrap() as u32)).collect();
    assert_eq!(ranges, url_rules::IDNA_SAFE_RANGES, "골든(IDNA_SWEEP_RANGES)과 구현(IDNA_SAFE_RANGES)의 범위가 다르다");
    let accepted: Vec<(u32, u32)> = arr(&idna["accepted"], "accepted").iter().map(|r| (r[0].as_u64().unwrap() as u32, r[1].as_u64().unwrap() as u32)).collect();
    let ts_accepts = |cp: u32| accepted.binary_search_by(|&(lo, hi)| if cp < lo { std::cmp::Ordering::Greater } else if cp > hi { std::cmp::Ordering::Less } else { std::cmp::Ordering::Equal }).is_ok();
    let mapped: HashMap<u32, &str> = idna["mapped"].as_array().unwrap().iter().map(|e| (e[0].as_u64().unwrap() as u32, s(&e[1]))).collect();
    let (mut both, mut stricter_cps) = (0usize, Vec::new());
    let (mut unsound, mut bad) = (Vec::new(), Vec::new());
    for &(lo, hi) in &ranges {
        for cp in lo..=hi {
            let Some(c) = char::from_u32(cp) else { continue };
            let rust = parse_url(&format!("https://a{c}b.example.com/")).map(|u| unicode_host(&u));
            match (rust, ts_accepts(cp)) {
                (Some(h), true) => {
                    let expected = mapped.get(&cp).map_or_else(|| format!("a{c}b.example.com"), |m| (*m).to_string());
                    if h != expected {
                        bad.push(format!("U+{cp:04X} → Rust {h}, TS {expected}"));
                    }
                    both += 1;
                }
                (Some(h), false) => unsound.push(format!("U+{cp:04X} → Rust {h}, TS 거부")),
                (None, true) => stricter_cps.push(cp),
                (None, false) => {}
            }
        }
    }
    eprintln!("[idna] 둘 다 받음 {both}자, TS 만 받음 {}자 {:X?}", stricter_cps.len(), stricter_cps);
    assert!(unsound.is_empty(), "Rust 허용 ⇒ TS 허용 위반 {}건, 예: {:?}", unsound.len(), &unsound[..unsound.len().min(40)]);
    assert!(bad.is_empty(), "같은 호스트가 아닌 {}건, 예: {:?}", bad.len(), &bad[..bad.len().min(5)]);
    assert!(both > 40_000, "대조한 문자가 너무 적다: {both}");
    // 알려진 차이(더 엄격): 범위 안인데 idna 가 거부하는 문자(분수·ŉ·ẞ 등) — 목록이 실제와 같아야 한다
    assert_eq!(stricter_cps, [0xBC, 0xBD, 0xBE, 0x149, 0x23A, 0x23E, 0x374, 0x61D, 0x1E9A, 0x1E9E, 0xFF5F, 0xFF60, 0xFFE6, 0xFFE8, 0xFFE9, 0xFFEA, 0xFFEB, 0xFFEC, 0xFFED, 0xFFEE], "TS 만 받는 문자 목록이 달라졌다");
    // 제외 목록은 실제로 TS 가 거부하는 문자만 담는다(낡은 항목이 남지 않게)
    for &cp in url_rules::IDNA_SAFE_EXCLUDED {
        assert!(!ts_accepts(cp), "U+{cp:04X}: TS 가 받는 문자를 제외했다 — 제외 목록에서 뺀다");
    }
}

#[test]
fn 요청_url_조립이_ts_와_같다() {
    let g = golden("urls.json");
    let query = |q: &Value| -> Vec<(String, Option<String>)> {
        q.as_array()
            .expect("쿼리")
            .iter()
            .map(|p| {
                let k = s(&p[0]).to_string();
                let v = match &p[1] {
                    Value::Null => None,
                    Value::String(t) => Some(t.clone()),
                    Value::Number(n) => Some(js_number_string(n.as_f64().expect("f64"))),
                    other => panic!("쿼리 값: {other}"),
                };
                (k, v)
            })
            .collect()
    };
    let mut d = Diffs::default();
    for c in arr(&g["requests"], "requests") {
        let q = query(&c["q"]);
        let refs: Vec<(&str, Option<&str>)> = q.iter().map(|(k, v)| (k.as_str(), v.as_deref())).collect();
        let got = url_rules::github_request_url(s(&c["path"]), &refs);
        if got.is_some() && c["url"].is_null() {
            d.unsound(|| format!("github {} → Rust {got:?}, TS 거부", c["path"]));
        }
        d.eq(|| format!("github {} {}", c["path"], c["q"]), &json!(got), &c["url"]);
    }
    for c in arr(&g["jiraRequests"], "jiraRequests") {
        let q = query(&c["q"]);
        let refs: Vec<(&str, Option<&str>)> = q.iter().map(|(k, v)| (k.as_str(), v.as_deref())).collect();
        let got = url_rules::jira_request_url(s(&c["site"]), s(&c["path"]), &refs);
        if got.is_some() && c["url"].is_null() {
            d.unsound(|| format!("jira {} {} → Rust {got:?}, TS 거부", c["site"], c["path"]));
        }
        d.eq(|| format!("jira {} {} {}", c["site"], c["path"], c["q"]), &json!(got), &c["url"]);
    }
    d.finish("요청 url");
}

/// `JiraClient.image` 의 홉 루프를 같은 스크립트로 돌려 요청한 주소·인증 유무·결과를 비교한다
#[test]
fn 프로필_사진_홉_판정이_ts_와_같다() {
    let g = golden("urls.json");
    let mut d = Diffs::default();
    for c in arr(&g["images"], "images") {
        let (site, href, script) = (s(&c["site"]), s(&c["href"]), &c["script"]);
        let mut calls: Vec<Value> = Vec::new();
        let mut ok = false;
        if let Some(mut url) = parse_url(href) {
            for hop in 0..=3 {
                let own = match image_hop(&url, site, hop) {
                    ImageHop::Reject => break,
                    ImageHop::Own => true,
                    ImageHop::Public => false,
                };
                calls.push(json!([url.to_string(), own]));
                let (status, loc) = match script.get(url.as_str()) {
                    Some(e) => (e[0].as_u64().expect("상태"), e[1].as_str().map(str::to_string)),
                    None => (404, None),
                };
                if (300..400).contains(&status) {
                    // 사이트 안의 리다이렉트는 대개 로그인 화면이다 — 따라가지 않는다
                    match loc {
                        Some(l) if !own && !l.is_empty() => match resolve_location(&url, &l) {
                            Some(next) => {
                                url = next;
                                continue;
                            }
                            None => break,
                        },
                        _ => break,
                    }
                }
                ok = status == 200;
                break;
            }
        }
        // 인증이 붙는 요청(own)은 사이트 안으로 정한 첫 홉뿐이다
        d.eq(|| format!("image({site}, {href})"), &json!({ "calls": calls, "ok": ok }), &json!({ "calls": c["calls"], "ok": c["ok"] }));
    }
    d.finish("프로필 사진");
}

#[test]
fn 공개_사진_호스트_판정이_정규식과_같다() {
    let g = golden("urls.json");
    let mut d = Diffs::default();
    for c in arr(&g["avatarHosts"], "avatarHosts") {
        let got = is_public_avatar_host(s(&c["h"]));
        if got && c["ok"] != json!(true) {
            d.unsound(|| format!("{} → Rust 허용, TS 거부", c["h"]));
        }
        d.eq(|| format!("avatarHost({})", c["h"]), &json!(got), &c["ok"]);
    }
    d.finish("공개 사진 호스트");
}

// ── 응답 해석 ─────────────────────────────────────────────────────────────

#[test]
fn 숫자_해석이_js_number_와_같다() {
    let g = golden("http.json");
    let mut bad = Vec::new();
    for c in arr(&g["number"], "number") {
        let (got, expected) = (js_number(s(&c["s"])), dec(&c["out"]));
        if !same_num(got, expected) {
            bad.push(format!("Number({:?}) → Rust {got}, TS {expected}", c["s"]));
        }
    }
    for c in arr(&g["numberString"], "numberString") {
        let got = js_number_string(dec(&c["n"]));
        if got != s(&c["s"]) {
            bad.push(format!("String({}) → Rust {got}, TS {}", c["n"], c["s"]));
        }
    }
    assert!(bad.is_empty(), "JS 숫자 의미가 다른 {}건, 예: {:?}", bad.len(), &bad[..bad.len().min(5)]);
}

/// 숫자(초)는 TS 와 같고, 날짜는 세 형식(IMF-fixdate·RFC 850·asctime)만 받는다. Rust 가 값을 내면 TS 와 같아야 하고(null 이던 것을 받으면 안 된다),
/// TS 만 값을 내는 입력(ISO 8601·초 60·없는 날짜 등)은 Rust 가 null 이다 — 알려진 차이(더 엄격)
#[test]
fn retry_after_가_ts_와_같다_날짜는_더_엄격하다() {
    let g = golden("http.json");
    let now = 1_700_000_000_000.0;
    let mut d = Diffs::default();
    let (mut stricter, mut tz) = (0, 0);
    for c in arr(&g["retryAfter"], "retryAfter") {
        let v = opt_s(&c["v"]);
        let got = parse_retry_after(v, now);
        let numeric = v.is_some_and(|t| !t.is_empty() && js_number(t).is_finite() && js_number(t) >= 0.0);
        if c["out"] == json!("tz") {
            // 시간대에 따라 값이 달라지는 형식(시간대 표기 없음)은 골든에 값이 없다 — 숫자 분기가 아니어야 한다
            assert!(!numeric, "{v:?}: 숫자인데 시간대 의존으로 적혔다");
            tz += 1;
            continue;
        }
        let expected = if c["out"].is_null() { None } else { Some(dec(&c["out"])) };
        match (got, expected) {
            (Some(a), Some(b)) if same_num(a, b) => {}
            (None, None) => {}
            (None, Some(_)) if !numeric => stricter += 1,
            (g_, e_) => d.bad.push(format!("parseRetryAfter({v:?}) → Rust {g_:?}, TS {e_:?}")),
        }
        d.checked += 1;
    }
    eprintln!("[retry-after] TS 만 받는 날짜 {stricter}건(더 엄격), 시간대 의존 {tz}건(값 비교 제외)");
    assert!(stricter > 0 && tz > 0);
    d.finish("parseRetryAfter");
}

#[test]
fn 차이_asctime_은_utc_로_푼다_ts_는_로컬_시간이다() {
    // 2030-01-01T00:00:00Z
    assert_eq!(parse_http_date("Tue Jan  1 00:00:00 2030"), Some(1_893_456_000_000.0));
    assert_eq!(parse_http_date("Tue Jan 01 00:00:00 2030"), Some(1_893_456_000_000.0));
    assert_eq!(parse_http_date("Tue, 01 Jan 2030 00:00:00 GMT"), Some(1_893_456_000_000.0));
    assert_eq!(parse_http_date("Tuesday, 01-Jan-30 00:00:00 GMT"), Some(1_893_456_000_000.0));
    // 받지 않는 형식
    for v in ["2030-01-01T00:00:00Z", "Tue, 01 Jan 2030 23:59:60 GMT", "Mon, 31 Apr 2024 12:00:00 GMT", "Tue, 01 Jan 2030 00:00:00 UTC", "2030/01/01"] {
        assert_eq!(parse_http_date(v), None, "{v}");
    }
}

#[test]
fn link_next_판정이_ts_와_같다() {
    let g = golden("http.json");
    let mut d = Diffs::default();
    for c in arr(&g["linkNext"], "linkNext") {
        let got = has_next_link(opt_s(&c["v"]));
        if got && c["out"] != json!(true) {
            d.unsound(|| format!("hasNextLink({}) → Rust true, TS false", c["v"]));
        }
        d.eq(|| format!("hasNextLink({})", c["v"]), &json!(got), &c["out"]);
    }
    d.finish("hasNextLink");
}

#[test]
fn rate_헤더_해석이_ts_와_같다() {
    let g = golden("http.json");
    let mut d = Diffs::default();
    for c in arr(&g["rate"], "rate") {
        let h = RateHeaders { remaining: opt_s(&c["remaining"]), limit: opt_s(&c["limit"]), reset: opt_s(&c["reset"]), retry_after: None };
        let got = parse_rate(&h).map_or(Value::Null, |r| json!({ "remaining": enc(r.remaining), "limit": enc(r.limit), "reset": enc(r.reset) }));
        d.eq(|| format!("parseRate({}, {}, {})", c["remaining"], c["limit"], c["reset"]), &got, &c["out"]);
    }
    d.finish("parseRate");
}

#[test]
fn 오류_본문_메시지가_ts_와_같다() {
    let g = golden("http.json");
    let mut d = Diffs::default();
    for c in arr(&g["bodyGh"], "bodyGh") {
        d.eq(|| format!("github bodyMessage({:.80})", c["t"]), &json!(github_body_message(s(&c["t"]))), &c["out"]);
    }
    for c in arr(&g["bodyJira"], "bodyJira") {
        d.eq(|| format!("jira bodyMessage({:.80})", c["t"]), &json!(jira_body_message(s(&c["t"]))), &c["out"]);
    }
    d.finish("오류 본문 메시지");
}

#[test]
fn jira_오류_분류가_ts_와_같다() {
    let g = golden("http.json");
    let mut d = Diffs::default();
    for c in arr(&g["jiraError"], "jiraError") {
        let kind = if s(&c["kind"]) == "cloud" { JiraKind::Cloud } else { JiraKind::Server };
        let e = jira_http_error(c["status"].as_u64().expect("상태") as u16, opt_s(&c["retry"]), s(&c["body"]), kind, opt_s(&c["notFound"]), 1_700_000_000_000.0);
        d.eq(|| format!("jira httpError({}, {}, {:.60})", c["status"], c["retry"], c["body"]), &json!({ "code": e.code, "status": e.status, "message": e.message }), &c["out"]);
    }
    d.finish("jira httpError");
}

#[test]
fn github_오류_분류가_ts_와_같다() {
    let g = golden("http.json");
    let mut d = Diffs::default();
    for c in arr(&g["ghError"], "ghError") {
        let h = &c["h"];
        let rh = RateHeaders { remaining: opt_s(&h["remaining"]), limit: opt_s(&h["limit"]), reset: opt_s(&h["reset"]), retry_after: opt_s(&h["retry"]) };
        let e = github_http_error(c["status"].as_u64().expect("상태") as u16, &rh, s(&c["body"]), c["signed"].as_bool().expect("signed"), &|_| "{TIME}".to_string());
        d.eq(|| format!("github httpError({}, {h}, {:.60})", c["status"], c["body"]), &json!({ "code": e.code, "status": e.status, "message": e.message }), &c["out"]);
    }
    d.finish("github httpError");
}

#[test]
fn 인증_헤더가_ts_와_같다() {
    let g = golden("http.json");
    let mut d = Diffs::default();
    for c in arr(&g["auth"], "auth") {
        let kind = if s(&c["kind"]) == "cloud" { JiraKind::Cloud } else { JiraKind::Server };
        let got = auth_header(kind, s(&c["email"]), &SecretString::new(s(&c["token"])));
        d.eq(|| format!("authHeader({}, {})", c["kind"], c["email"]), &json!(got.expose()), &c["out"]);
    }
    d.finish("authHeader");
}

// ── JSON ──────────────────────────────────────────────────────────────────

fn tree(j: &Json) -> Value {
    match j {
        Json::Null => json!(["null"]),
        Json::Bool(b) => json!(["bool", b]),
        Json::Num(n) => json!(["num", enc(*n)]),
        Json::Str(t) => json!(["str", t]),
        Json::Arr(a) => json!(["arr", a.iter().map(tree).collect::<Vec<_>>()]),
        Json::Obj(_) => json!(["obj", j.entries_js_order().into_iter().map(|(k, v)| json!([k, tree(v)])).collect::<Vec<_>>()]),
    }
}

#[test]
fn json_해석이_ts_와_같다() {
    let g = golden("http.json");
    let mut d = Diffs::default();
    for c in arr(&g["json"], "json") {
        let got = json::parse(s(&c["t"])).map_or(Value::Null, |j| tree(&j));
        d.eq(|| format!("JSON.parse({:.80})", c["t"]), &got, &c["out"]);
    }
    d.finish("JSON.parse");
}

/// 중첩 상한(MAX_DEPTH=512): 상한 안은 TS 와 같고, 넘으면 Rust 는 JSON 오류다(TS 는 스택이 다할 때까지 받는다). 알려진 차이
#[test]
fn 차이_깊은_중첩은_rust_가_더_엄격하다() {
    let g = golden("http.json");
    let (mut within, mut beyond_known) = (0, 0);
    for c in arr(&g["nest"], "nest") {
        let text = format!("{}{}{}{}{}", s(&c["pre"]), s(&c["open"]).repeat(c["depth"].as_u64().unwrap() as usize), s(&c["inner"]), s(&c["close"]).repeat(c["depth"].as_u64().unwrap() as usize), s(&c["post"]));
        let depth = c["depth"].as_u64().unwrap() as usize;
        let rust_ok = json::parse(&text).is_ok();
        // 배열은 depth+0 겹이 최상위 값이다. 객체({"a":…})와 접두가 붙은 경우는 한 겹씩 더 깊다 — 정확한 경계는 아래 단위 단언이 본다
        if depth <= MAX_DEPTH - 2 {
            assert_eq!(if rust_ok { "ok" } else { "err" }, s(&c["json"]), "depth {depth} {:.40}", text);
            assert_eq!(json!(github_body_message(&text)), c["gh"], "gh depth {depth}");
            assert_eq!(json!(jira_body_message(&text)), c["jira"], "jira depth {depth}");
            within += 1;
        } else if depth > MAX_DEPTH + 2 {
            assert!(!rust_ok, "depth {depth}: 상한을 넘는데 받았다");
            assert_eq!(github_body_message(&text), "");
            assert_eq!(jira_body_message(&text), "");
            if s(&c["json"]) == "ok" {
                beyond_known += 1;
            }
        }
    }
    eprintln!("[json 깊이] 상한 안 {within}건 일치, TS 만 받는 깊은 입력 {beyond_known}건(더 엄격)");
    assert!(within > 0 && beyond_known > 0);
    // 정확한 경계: 배열 513겹까지 받고 514겹부터 거부한다
    let nest = |n: usize| format!("{}{}", "[".repeat(n), "]".repeat(n));
    assert!(json::parse(&nest(MAX_DEPTH + 1)).is_ok());
    assert_eq!(json::parse(&nest(MAX_DEPTH + 2)), Err(json::JsonError::TooDeep));
}

#[test]
fn 차이_짝_없는_서로게이트는_u_fffd_로_받는다() {
    // JS 는 짝 없는 서로게이트를 그대로 두지만 Rust 문자열은 담지 못한다 — 파싱은 성공하고 U+FFFD 가 된다(serde_json 은 거부한다)
    assert!(serde_json::from_str::<Value>(r#""\ud83d""#).is_err());
    assert_eq!(json::parse(r#""\ud83d""#), Ok(Json::Str("\u{fffd}".into())));
    assert_eq!(json::parse(r#""\ude00x""#), Ok(Json::Str("\u{fffd}x".into())));
    assert_eq!(json::parse(r#""😀""#), Ok(Json::Str("😀".into())));
    assert_eq!(json::parse(r#""\ud83d😀""#), Ok(Json::Str("\u{fffd}😀".into())));
    // 중복 키는 처음 자리에 마지막 값, 정수형 키는 먼저(JS 열거 순서)
    let j = json::parse(r#"{"b":1,"10":2,"a":3,"b":4,"2":5}"#).unwrap();
    let keys: Vec<&str> = j.entries_js_order().into_iter().map(|(k, _)| k).collect();
    assert_eq!(keys, ["2", "10", "b", "a"]);
    assert_eq!(j.get("b"), Some(&Json::Num(4.0)));
}

// ── Device Flow ───────────────────────────────────────────────────────────

#[test]
fn device_flow_시작_응답_해석이_ts_와_같다() {
    let g = golden("device.json");
    let mut d = Diffs::default();
    for c in arr(&g["starts"], "starts") {
        let r = interpret_start(s(&c["clientId"]), c["status"].as_u64().unwrap() as u16, s(&c["text"]));
        let got = match r {
            Ok(x) => json!({ "ok": {
                "userCode": x.user_code, "verificationUri": x.verification_uri, "expiresIn": enc(x.expires_in), "interval": enc(x.interval),
                "deviceCode": x.device_code.expose(), "expiresAt": enc(1000.0 + x.expires_in * 1000.0)
            } }),
            Err(DeviceError::Message(m)) => json!({ "err": m }),
            Err(DeviceError::TypeError) => json!({ "typeError": true }),
        };
        d.eq(|| format!("start({}, {}, {:.60})", c["clientId"], c["status"], c["text"]), &got, &c["out"]);
    }
    d.finish("Device Flow 시작");
}

#[test]
fn device_flow_폴링이_ts_와_같다() {
    let g = golden("device.json");
    let mut d = Diffs::default();
    for c in arr(&g["polls"], "polls") {
        let expires_at = dec(&c["expiresIn"]) * 1000.0;
        let mut st = PollState::new(dec(&c["interval"]));
        let (mut t, mut sleeps) = (0.0_f64, Vec::new());
        let mut steps = c["steps"].as_array().unwrap().iter();
        let out = loop {
            let delay = st.delay_ms();
            sleeps.push(enc(delay));
            t += delay;
            if PollState::expired(t, expires_at) {
                break json!({ "err": EXPIRED_MESSAGE });
            }
            let step = steps.next().expect("단계가 모자란다");
            let r = if step.get("net").is_some() { st.on_transient() } else { st.on_response(step["status"].as_u64().unwrap() as u16, s(&step["text"])) };
            match r {
                PollStep::Pending | PollStep::SlowDown | PollStep::Transient => {}
                PollStep::Token(tok) => break json!({ "token": tok.expose() }),
                PollStep::Fail(DeviceError::Message(m)) => break json!({ "err": m }),
                PollStep::Fail(DeviceError::TypeError) => break json!({ "typeError": true }),
            }
        };
        d.eq(|| format!("poll({})", c["steps"]), &json!({ "sleeps": sleeps, "out": out }), &json!({ "sleeps": c["sleeps"], "out": c["out"] }));
    }
    d.finish("Device Flow 폴링");
}

#[test]
fn 차이_타입이_틀린_device_flow_필드는_없는_것으로_본다() {
    // TS 는 truthiness·형 변환으로 받아들이지만 Rust 는 없는 것으로 본다(GitHub 가 보내는 형식이 아니다)
    let mut st = PollState::new(5.0);
    assert!(matches!(st.on_response(200, r#"{"error":"slow_down","interval":"30"}"#), PollStep::SlowDown));
    assert_eq!(st.interval, 10.0, "문자열 interval 은 무시하고 +5초");
    assert!(matches!(PollState::new(5.0).on_response(200, r#"{"access_token":5}"#), PollStep::Fail(_)));
    let x = interpret_start("cid", 200, r#"{"device_code":"d","user_code":"u","verification_uri":"v","interval":"7","expires_in":"60"}"#).unwrap();
    assert_eq!((x.interval, x.expires_in), (5.0, 900.0));
    assert!(interpret_start("cid", 200, r#"{"device_code":5,"user_code":"u","verification_uri":"v"}"#).is_err());
}

// ── ghEnv · 토큰 가리기 ───────────────────────────────────────────────────

#[test]
fn gh_env_거르기가_ts_와_같다() {
    let g = golden("env.json");
    let mut d = Diffs::default();
    for c in arr(&g["env"], "env") {
        let input: Vec<(String, String)> = c["in"].as_array().unwrap().iter().map(|p| (s(&p[0]).to_string(), s(&p[1]).to_string())).collect();
        let got: Vec<Value> = gh_env(&input).into_iter().map(|(k, v)| json!([k, v])).collect();
        d.eq(|| format!("ghEnv({})", c["in"]), &json!(got), &c["out"]);
    }
    d.finish("ghEnv");
}

#[test]
fn 토큰_사용_가능_판정과_가리기가_ts_와_같다() {
    let g = golden("env.json");
    let mut d = Diffs::default();
    for c in arr(&g["usable"], "usable") {
        d.eq(|| format!("usableToken({:.30})", c["t"]), &json!(usable_token(s(&c["t"]))), &c["out"]);
    }
    for c in arr(&g["redact"], "redact") {
        d.eq(|| format!("redactToken({:.40}, {:.20})", c["text"], c["token"]), &json!(redact_token(s(&c["text"]), opt_s(&c["token"]))), &c["out"]);
    }
    d.finish("토큰 가리기");
}

/// `SecretString` 은 `Display`·`Serialize` 를 구현하지 않는다 — 소스에 그 구현이 없고(새로 더하면 실패), `Debug` 는 `***` 다
#[test]
fn secret_string_은_display_와_serialize_가_없다() {
    let src = fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src").join("net").join("redact.rs")).unwrap();
    // 비교 대상은 테스트 모듈 앞까지(테스트 안의 설명 문자열은 제외)
    let body = src.split("#[cfg(test)]").next().unwrap();
    for banned in ["Display for SecretString", "Serialize for SecretString", "Deserialize for SecretString", "derive(Serialize", "derive(Deserialize", "derive(Debug"] {
        assert!(!body.contains(banned), "SecretString 에 {banned} 를 더하면 안 된다");
    }
    assert!(body.contains("impl std::fmt::Debug for SecretString"));
    assert_eq!(format!("{:?}", SecretString::new("ghp_secret")), "***");
    // 값이 든 구조체의 Debug 에도 드러나지 않는다
    #[derive(Debug)]
    #[allow(dead_code)]
    struct Holder {
        token: SecretString,
    }
    assert_eq!(format!("{:?}", Holder { token: SecretString::new("ghp_secret") }), "Holder { token: *** }");
}

// ── 대응표 ────────────────────────────────────────────────────────────────

#[test]
fn 대응표가_가리키는_rust_테스트가_있다() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let map: Value = serde_json::from_str(&fs::read_to_string(root.join("tests/golden/net/test-map.json")).unwrap()).unwrap();
    let src = fs::read_to_string(root.join("src-tauri/tests/net.rs")).unwrap();
    let (mut ported, mut later, mut na) = (0, 0, 0);
    let mut targets: BTreeSet<String> = BTreeSet::new();
    let mut per_pr: HashMap<String, usize> = HashMap::new();
    for (title, v) in map.as_object().expect("대응표는 객체") {
        let v = v.as_str().unwrap();
        if let Some(why) = v.strip_prefix("N/A: ") {
            if let Some(pr) = why.strip_prefix("PR ") {
                assert!(pr.len() == 1 && ("2"..="9").contains(&pr), "{title}: 'N/A: PR 2'..'PR 9' 형식이어야 한다 → {v}");
                *per_pr.entry(pr.to_string()).or_default() += 1;
                later += 1;
            } else {
                assert!(why.chars().count() > 8, "{title}: N/A 사유가 비어 있다");
                na += 1;
            }
            continue;
        }
        let name = v.strip_prefix("net.rs::").unwrap_or_else(|| panic!("{title}: 'net.rs::<이름>' 이어야 한다 → {v}"));
        assert!(src.contains(&format!("fn {name}(")), "{title}: net.rs 에 fn {name} 이 없다");
        targets.insert(name.to_string());
        ported += 1;
    }
    let mut prs: Vec<_> = per_pr.into_iter().collect();
    prs.sort();
    eprintln!("[대응표] 이식 {ported}(테스트 {}개), 뒤 PR 몫 {later} {prs:?}, N/A {na}", targets.len());
    assert_eq!(ported + later + na, map.as_object().unwrap().len());
}
