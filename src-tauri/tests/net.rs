//! GitHub·Jira 순수 로직의 골든 패리티 테스트. `tests/golden/net/*.json`(TS 가 기준: `UPDATE_GOLDEN=1 npx vitest run tests/goldenNet.test.ts`)을
//! 모두 읽어 Rust 이식(`antler_lib::{net, github, jira}`)이 같은 결과를 내는지 본다. **건너뛰는 골든이 없다** — 처리기 없는 파일이 있으면 실패한다.
//! 안전 방향 확인: URL 판정 격자 전체에서 "Rust 허용 ⇒ TS 허용"을 따로 센다(`url_격자_판정이_ts_와_같다_허용_범위는_ts_안이다`).
//! 이 파일은 네트워크를 쓰지 않는다. 알려진 차이는 각각 `차이_…` 이름의 테스트가 단언한다.

use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use antler_lib::github::{accessible_repos, api_map, checks_map, launchpad_map};
use antler_lib::github::device_rules::{interpret_start, DeviceError, PollState, PollStep, EXPIRED_MESSAGE};
use antler_lib::github::gh_env::gh_env;
use antler_lib::github::validate as gh;
use antler_lib::jira::validate::{self as jira, SiteOptions};
use antler_lib::jira::{adf, api_map as jira_map, auth_header, JiraKind};
use antler_lib::net::http_error::{
    github_body_message, github_http_error, has_next_link, jira_body_message, jira_http_error, parse_http_date, parse_rate, parse_retry_after, RateHeaders,
};
use antler_lib::net::js_date::{iso_string, parse_date_ms, to_epoch};
use antler_lib::net::js_value::{field, must_arr, obj, stringify, truthy, MapError, MapResult};
use antler_lib::net::json::{self, js_number, js_number_string, Json, MAX_DEPTH};
use antler_lib::net::redact::{redact_token, usable_token, SecretString};
use antler_lib::net::url_rules::{self, in_site, is_public_avatar_host, parse_url, ImageStep, ImageWalk};
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

const HANDLED: [&str; 7] = ["validate.json", "urls.json", "http.json", "device.json", "env.json", "ghmap.json", "jiramap.json"];

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
        // 홉 규칙은 제품 코드의 상태 기계(`ImageWalk`)다 — 다음 PR 의 HTTP 코드가 쓰는 것과 같다
        let mut walk = ImageWalk::new(site, href);
        while let ImageStep::Fetch { url, with_auth } = walk.step() {
            calls.push(json!([url.to_string(), with_auth]));
            let (status, loc) = match script.get(url.as_str()) {
                Some(e) => (e[0].as_u64().expect("상태"), e[1].as_str().map(str::to_string)),
                None => (404, None),
            };
            if (300..400).contains(&status) {
                walk.on_redirect(loc.as_deref());
                continue;
            }
            ok = status == 200;
            break;
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

/// 중첩 상한(MAX_DEPTH=64): 상한 안은 TS 와 같고, 넘으면 Rust 는 JSON 오류다(TS 는 스택이 다할 때까지 받는다). 알려진 차이
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
    // 정확한 경계: 배열 65겹까지 받고 66겹부터 거부한다(최상위 값이 깊이 0 이라 상한 + 1 겹이 마지막이다)
    assert_eq!(MAX_DEPTH, 64);
    let nest = |n: usize| format!("{}{}", "[".repeat(n), "]".repeat(n));
    assert!(json::parse(&nest(64)).is_ok());
    assert!(json::parse(&nest(65)).is_ok());
    assert_eq!(json::parse(&nest(66)), Err(json::JsonError::TooDeep));
}

/// 깊이 65 는 거부하고 64 는 받는다는 요청의 기준은 "객체·배열 안쪽 값의 깊이"다: 값 한 겹 더 안으로 들어갈 때마다 깊이가 1 늘고 상한은 `depth > 64` 에서 거부한다.
/// 여기서는 빈 배열이 마지막 값인 입력으로 경계를 못 박는다(`[`×n + `[]` + `]`×n 이면 가장 안쪽 빈 배열이 깊이 n)
#[test]
fn 깊이_64_는_받고_65_는_거부한다() {
    let inner_depth = |n: usize| format!("{}[]{}", "[".repeat(n), "]".repeat(n));
    assert!(json::parse(&inner_depth(64)).is_ok(), "깊이 64");
    assert_eq!(json::parse(&inner_depth(65)), Err(json::JsonError::TooDeep), "깊이 65");
    let obj_depth = |n: usize| format!("{}1{}", r#"{"a":"#.repeat(n), "}".repeat(n));
    assert!(json::parse(&obj_depth(64)).is_ok());
    assert_eq!(json::parse(&obj_depth(65)), Err(json::JsonError::TooDeep));
}

/// 작은 스택(256KB)의 스레드에서 상한을 넘는 깊은 입력을 해석해도 스택이 넘치지 않는다(넘치면 프로세스가 abort 한다)
#[test]
fn 작은_스택_스레드에서도_깊은_입력이_넘치지_않는다() {
    let h = std::thread::Builder::new()
        .stack_size(256 * 1024)
        .spawn(|| {
            let deep = |n: usize| format!("{}{}", "[".repeat(n), "]".repeat(n));
            assert_eq!(json::parse(&deep(100_000)), Err(json::JsonError::TooDeep));
            assert_eq!(json::parse(&format!("{}1{}", r#"{"a":"#.repeat(100_000), "}".repeat(100_000))), Err(json::JsonError::TooDeep));
            // 상한 안의 가장 깊은 입력은 받는다
            assert!(json::parse(&deep(MAX_DEPTH + 1)).is_ok());
        })
        .unwrap();
    h.join().expect("256KB 스택에서 해석이 실패했다");
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
        // 알려진 차이: Rust 는 interval 을 1..=60초, expires_in 을 0..=3600초로 묶는다(TS 는 그대로 쓴다). 기대값에 같은 상한을 적용해 비교한다
        let mut want = c["out"].clone();
        if let Some(ok) = want.get_mut("ok") {
            let exp = dec(&ok["expiresIn"]).clamp(0.0, 3600.0);
            ok["expiresIn"] = enc(exp);
            ok["interval"] = enc(dec(&ok["interval"]).clamp(1.0, 60.0));
            ok["expiresAt"] = enc(1000.0 + exp * 1000.0);
        }
        d.eq(|| format!("start({}, {}, {:.60})", c["clientId"], c["status"], c["text"]), &got, &want);
    }
    d.finish("Device Flow 시작");
}

#[test]
fn 차이_device_flow_간격과_수명은_상한이_있다() {
    let start = |extra: &str| {
        let body = format!(r#"{{"device_code":"d","user_code":"u","verification_uri":"v",{extra}}}"#);
        interpret_start("cid", 200, &body).map(|x| (x.interval, x.expires_in)).unwrap()
    };
    assert_eq!(start(r#""interval":1e999,"expires_in":1e999"#), (60.0, 3600.0), "Infinity 는 상한으로");
    assert_eq!(start(r#""interval":-1e999,"expires_in":-1e999"#), (1.0, 0.0), "-Infinity 는 하한으로");
    assert_eq!(start(r#""interval":61,"expires_in":3601"#), (60.0, 3600.0));
    assert_eq!(start(r#""interval":60,"expires_in":3600"#), (60.0, 3600.0));
    assert_eq!(start(r#""interval":0,"expires_in":-5"#), (1.0, 0.0));
    // slow_down: 유한하고 현재보다 큰 값만 쓰고 60초를 넘기지 않는다. 그 밖은 +5초(역시 60초 상한)
    for (body, want) in [
        (r#"{"error":"slow_down","interval":1e999}"#, 10.0),
        (r#"{"error":"slow_down","interval":-1e999}"#, 10.0),
        (r#"{"error":"slow_down","interval":1e300}"#, 60.0),
        (r#"{"error":"slow_down","interval":30}"#, 30.0),
        (r#"{"error":"slow_down","interval":3}"#, 10.0),
    ] {
        let mut st = PollState::new(5.0);
        assert!(matches!(st.on_response(200, body), PollStep::SlowDown));
        assert_eq!(st.interval, want, "{body}");
    }
    let mut st = PollState::new(58.0);
    st.on_response(200, r#"{"error":"slow_down"}"#);
    assert_eq!(st.interval, 60.0, "+5초도 상한을 넘지 않는다");
    // JSON 에는 NaN 이 없다(NaN 입력은 device_rules 의 단위 테스트가 본다)
    assert!(json::parse("NaN").is_err());
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

/// 타입이 어느 트레이트도 구현하지 않음을 컴파일 시점에 단언한다(`static_assertions::assert_not_impl_any!` 와 같은 모호성 기법 — 네트워크 없이 의존성을 늘리지 않으려고 직접 둔다).
/// 구현이 하나라도 있으면 `some_item` 의 후보가 둘이 되어 타입 추론이 모호하다는 컴파일 오류가 난다
macro_rules! assert_not_impl_any {
    ($x:ty: $($t:path),+ $(,)?) => {
        const _: fn() = || {
            trait AmbiguousIfImpl<A> {
                fn some_item() {}
            }
            impl<T: ?Sized> AmbiguousIfImpl<()> for T {}
            $({
                #[allow(dead_code)]
                struct Invalid;
                impl<T: ?Sized + $t> AmbiguousIfImpl<Invalid> for T {}
            })+
            let _ = <$x as AmbiguousIfImpl<_>>::some_item;
        };
    };
}

// 값이 문자열로 새는 길(포맷·직렬화·역직렬화)을 타입 수준에서 막는다. 참조는 `Serialize` 를 `&T: Serialize` 로 물려받으므로 `&'static SecretString` 도 확인한다
assert_not_impl_any!(SecretString: std::fmt::Display, serde::Serialize, serde::de::DeserializeOwned);
assert_not_impl_any!(&'static SecretString: std::fmt::Display, serde::Serialize);

/// `SecretString` 은 `Display`·`Serialize` 를 구현하지 않는다 — 위 타입 단언이 컴파일 시점에 보장하고, 아래는 소스 문자열 검사(보조)와 `Debug` 가 `***` 인지 본다
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

// ── 응답 매퍼(PR 2) ───────────────────────────────────────────────────────

/// 골든을 `Json`(키 순서 보존)으로 읽는다 — 매퍼 출력의 키 순서까지 비교하려는 것이다
fn golden_json(name: &str) -> Json {
    let path = golden_dir().join(name);
    let text = fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    json::parse(&text).unwrap_or_else(|e| panic!("{name}: {e:?}"))
}

fn jget<'a>(j: &'a Json, k: &str) -> &'a Json {
    j.get(k).unwrap_or_else(|| panic!("키 {k} 가 없다"))
}

fn jarr<'a>(j: &'a Json, what: &str) -> &'a [Json] {
    match j {
        Json::Arr(a) if !a.is_empty() => a,
        _ => panic!("{what} 가 비었거나 배열이 아니다(골든을 건너뛴 것과 같다)"),
    }
}

fn jarr_or_empty(j: &Json) -> &[Json] {
    match j {
        Json::Arr(a) => a,
        _ => &[],
    }
}

fn jstr(j: &Json) -> &str {
    j.as_str().unwrap_or_else(|| panic!("문자열이 아니다: {j:?}"))
}

fn input(c: &Json) -> Vec<Json> {
    jarr_or_empty(jget(c, "in")).iter().map(|t| json::parse(jstr(t)).unwrap_or_else(|e| panic!("입력 {}: {e:?}", jstr(t)))).collect()
}

fn st(v: &str) -> Json {
    Json::Str(v.to_string())
}

/// `getPull`·`listPulls` 의 체크 보강: 헤드 sha 가 있을 때만 묻고, 변환이 던지면(TS `soft`) 없는 것으로 본다
fn soft_checks(summary: &Json, runs: &Json, status: &Json) -> Option<Vec<Json>> {
    truthy(summary.get("headSha")).then(|| api_map::check_runs_of(runs, status).ok()).flatten()
}

fn sha_n(k: usize) -> String {
    format!("{k:040x}")
}

/// 원격 목록(`{저장소 ID: [{owner, repo}]}`) → 소문자 `owner/repo` → 저장소 ID(등록 순서)
fn by_key_of(remotes: &Json) -> HashMap<String, Vec<String>> {
    let mut by_key: HashMap<String, Vec<String>> = HashMap::new();
    for (id, list) in remotes.entries_js_order() {
        for slug in jarr_or_empty(list) {
            let key = launchpad_map::slug_key(jstr(jget(slug, "owner")), jstr(jget(slug, "repo")));
            by_key.entry(key).or_default().push(id.to_string());
        }
    }
    by_key
}

/// TS 구동기(tests/goldenMaps.ts)의 `fn` 마다 같은 조립을 Rust 매퍼로 한다
fn run_gh_case(f: &str, a: &[Json]) -> MapResult<Json> {
    let null = Json::Null;
    let one = |i: usize| a.get(i).unwrap_or(&null);
    let list = |v: MapResult<Vec<Json>>| v.map(Json::Arr);
    match f {
        "mapPull" => api_map::map_pull(one(0)),
        "mapSearchPull" => api_map::map_search_pull(one(0)),
        "mapIssue" => api_map::map_issue(one(0)),
        "getIssue" => api_map::issue_detail(one(0)),
        "mapUser" => Ok(api_map::map_user(Some(one(0)))),
        "aggregateReviews" => api_map::aggregate_reviews(must_arr(Some(one(0)))?, one(1).as_f64().expect("개수")).map(st),
        "checkRunState" => Ok(st(api_map::check_run_state(field(Some(one(0)), "status"), field(Some(one(0)), "conclusion")))),
        "commitStatusState" => Ok(st(api_map::commit_status_state(Some(one(0))))),
        "aggregateChecks" => {
            let states: Vec<&str> = must_arr(Some(one(0)))?.iter().map(jstr).collect();
            Ok(st(api_map::aggregate_checks(&states)))
        }
        "rollupState" => Ok(st(checks_map::rollup_state(Some(one(0))))),
        "rollupQuery" => Ok(st(&checks_map::rollup_query(one(0).as_f64().expect("n") as usize))),
        "getPull" => {
            let reviews = must_arr(Some(one(1)))?;
            let summary = api_map::map_pull(one(0))?;
            let runs = soft_checks(&summary, one(2), one(3));
            api_map::pull_detail(one(0), reviews, runs.as_deref())
        }
        "listPulls" => {
            let mut out = Vec::new();
            for raw in must_arr(Some(one(0)))? {
                let mut item = api_map::map_pull(raw)?;
                let reviews = match one(1) {
                    Json::Arr(r) => Some(r.as_slice()),
                    _ => None,
                };
                let checks = soft_checks(&item, one(2), one(3));
                api_map::enrich_list_item(raw, &mut item, reviews, checks.as_deref())?;
                out.push(item);
            }
            Ok(Json::Arr(out))
        }
        "listPullsAnon" => list(api_map::pulls_page(one(0))),
        "listPullsFiltered" => list(api_map::filtered_pull_items(one(0))),
        "listIssues" => list(api_map::issue_items(one(0))),
        "searchPullsByKey" => list(api_map::search_pulls(one(0))),
        "getPullFiles" => list(api_map::pull_files(one(0))),
        "getPullCommits" => list(api_map::pull_commits(one(0)).map(api_map::limit_commits)),
        "getComments" => list(api_map::comments_page(one(0))),
        "getFilters" => api_map::filters(one(0), one(1)),
        "fetchCheckDetails" => list(checks_map::check_details(one(0), one(1))),
        "listRuns" => list(checks_map::runs_page(one(0))),
        "listJobs" => list(checks_map::jobs(one(0))),
        "mapRun" => checks_map::map_run(one(0)),
        "fetchCommitChecks" => {
            let n = one(1).as_f64().expect("n") as usize;
            let states = checks_map::rollup_states(one(0), n)?;
            Ok(Json::Arr(states.map_or_else(Vec::new, |v| v.iter().enumerate().map(|(k, x)| Json::Arr(vec![st(&sha_n(k + 1)), st(x)])).collect())))
        }
        "mapAccessibleRepo" => Ok(accessible_repos::map_accessible_repo(one(0)).unwrap_or(Json::Null)),
        "accessibleRepos" => {
            let pages: Vec<&Json> = if a.len() == 1 && !matches!(a[0], Json::Arr(_)) { vec![&a[0]] } else { a.iter().collect() };
            let mut repos: Vec<Json> = pages.iter().flat_map(|p| accessible_repos::accessible_page(p)).collect();
            accessible_repos::sort_accessible(&mut repos);
            Ok(obj(vec![("repos", Json::Arr(repos)), ("truncated", Json::Bool(false))]))
        }
        "slugFromRepositoryUrl" => Ok(launchpad_map::slug_from_repository_url(Some(one(0))).map_or(Json::Null, |(o, r)| obj(vec![("owner", st(&o)), ("repo", st(&r))]))),
        "sections" => Ok(Json::Arr(launchpad_map::SECTIONS.iter().map(|d| obj(vec![("section", st(d.section)), ("kind", st(d.kind)), ("q", st(d.q))])).collect())),
        "fetchLaunchpad" => {
            let by_key = by_key_of(one(1));
            let hits = |k: &str| field(Some(one(0)), k).map_or(&[][..], |v| must_arr(Some(v)).expect("배열"));
            let sections: Vec<launchpad_map::SectionHits<'_>> = launchpad_map::SECTIONS
                .iter()
                .zip(["authored", "review", "issues"])
                .map(|(d, k)| launchpad_map::SectionHits { def: *d, items: hits(k) })
                .collect();
            launchpad_map::merge_launchpad(&sections, &by_key).map(Json::Arr)
        }
        "searchPage" => {
            let (items, incomplete) = launchpad_map::search_page(one(0))?;
            let by_key = HashMap::from([("octo/hello".to_string(), vec!["r".to_string()])]);
            let sections: Vec<launchpad_map::SectionHits<'_>> = launchpad_map::SECTIONS.iter().map(|d| launchpad_map::SectionHits { def: *d, items }).collect();
            let n = launchpad_map::merge_launchpad(&sections, &by_key)?.len();
            Ok(obj(vec![("n", Json::Num(n as f64)), ("incomplete", Json::Bool(incomplete))]))
        }
        other => panic!("처리기가 없는 fn: {other}"),
    }
}

#[test]
fn github_응답_매퍼가_ts_와_같다() {
    let g = golden_json("ghmap.json");
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let (mut ok, mut err) = (0, 0);
    let mut bad: Vec<String> = Vec::new();
    for c in jarr(jget(&g, "cases"), "cases") {
        let f = jstr(jget(c, "fn")).to_string();
        seen.insert(f.clone());
        let got = run_gh_case(&f, &input(c));
        match (c.get("err").is_some(), got) {
            (true, Err(_)) => err += 1,
            (false, Ok(v)) if &v == jget(c, "out") => ok += 1,
            (expect_err, got) => bad.push(format!(
                "{f}({:.200}) → Rust {}, TS {}",
                jarr_or_empty(jget(c, "in")).first().map_or("", jstr),
                got.map_or_else(|_| "오류".to_string(), |v| stringify(&v)),
                if expect_err { "오류".to_string() } else { stringify(jget(c, "out")) }
            )),
        }
    }
    assert!(bad.is_empty(), "TS 와 다른 결과 {}건, 예: {:#?}", bad.len(), &bad[..bad.len().min(12)]);
    assert!(ok > 150 && err > 30, "확인한 사례가 적다: 정상 {ok}, 오류 {err}");
    assert!(seen.len() >= 30, "fn 종류가 적다: {seen:?}");
}

/// 알려진 차이: 우리는 ISO 8601(시간대 있는 시각·날짜만)만 받는다 — TS 가 받는 그 외 꼴(소문자 구분자·공백·확장 연도…)은 0 이다
const DATE_STRICTER: &[&str] = &["2026-01-01 00:00:00Z", "2026-01-01t00:00:00z", "Mon, 01 Jan 2026 00:00:00 GMT"];

#[test]
fn 날짜_해석이_date_parse_와_같다_더_엄격한_것은_목록이다() {
    let g = golden_json("ghmap.json");
    let mut stricter: BTreeSet<String> = BTreeSet::new();
    let mut matched = 0;
    for c in jarr(jget(&g, "dates"), "dates") {
        let raw = jstr(jget(c, "in"));
        let got = parse_date_ms(raw);
        match (jget(c, "out"), got) {
            (Json::Num(t), Some(g)) if *t == g => matched += 1,
            (Json::Num(_), None) => {
                stricter.insert(raw.to_string());
            }
            // 시간대마다 값이 다른 입력은 Rust 가 받지 않는다(받으면 값이 머신마다 달라진다)
            (Json::Str(t), None) if t == "NaN" || t == "tz" => matched += 1,
            (want, got) => panic!("Date.parse({raw:?}) → Rust {got:?}, TS {want:?}"),
        }
    }
    let known: BTreeSet<String> = DATE_STRICTER.iter().map(ToString::to_string).collect();
    assert_eq!(stricter, known, "TS 만 받는 날짜 꼴의 목록이 달라졌다");
    assert!(matched > 120, "맞춘 날짜가 적다: {matched}");
}

#[test]
fn iso_문자열이_to_iso_string_과_같다() {
    let g = golden_json("ghmap.json");
    for c in jarr(jget(&g, "iso"), "iso") {
        let ms = match jget(c, "in") {
            Json::Num(n) => *n,
            Json::Str(t) if t == "-0" => -0.0,
            other => panic!("{other:?}"),
        };
        let got = iso_string(ms);
        match c.get("err") {
            Some(_) => assert_eq!(got, None, "{ms}"),
            None => assert_eq!(got.as_deref(), Some(jstr(jget(c, "out"))), "{ms}"),
        }
    }
}

/// 이상한 값이 어디에 들어와도 패닉하지 않는다(퍼즈성): 타입이 틀린 필드·null·깊은 중첩·거대 배열·제어 문자
#[test]
fn 매퍼는_이상한_입력에도_패닉하지_않는다() {
    let mut nest = Json::Str("\u{0}\u{1f}\u{7f}\u{feff}😀\u{fffd}".into());
    for _ in 0..json::MAX_DEPTH {
        nest = Json::Arr(vec![nest]);
    }
    let deep_obj = (0..json::MAX_DEPTH).fold(Json::Null, |acc, _| Json::Obj(vec![("x".into(), acc)]));
    let big: Json = Json::Arr((0..100_000).map(|i| Json::Num(f64::from(i))).collect());
    let big_objs: Json = Json::Arr((0..20_000).map(|_| Json::Obj(vec![("number".into(), Json::Num(1.0))])).collect());
    let weird = [
        Json::Null,
        Json::Bool(true),
        Json::Num(f64::INFINITY),
        Json::Num(-0.0),
        Json::Num(1e308),
        st(""),
        st("\u{0}\u{1}\u{1f}\u{7f}\"\\</script>"),
        st("2026-01-01T00:00:00Z\u{0}"),
        Json::Arr(vec![]),
        Json::Obj(vec![]),
        nest,
        deep_obj,
        big,
        big_objs,
    ];
    let keys = [
        "number", "title", "state", "draft", "merged_at", "user", "labels", "assignees", "head", "base", "updated_at", "created_at", "comments", "html_url", "pull_request", "requested_reviewers",
        "requested_teams", "submitted_at", "login", "avatar_url", "name", "color", "items", "check_runs", "statuses", "workflow_runs", "jobs", "conclusion", "status", "repository", "repository_url",
        "owner", "pushed_at", "filename", "commit", "author", "total_count", "incomplete_results",
    ];
    let mut calls = 0usize;
    for w in &weird {
        // 값 자체, 모든 키에 그 값을 넣은 객체, 그것을 한 번 더 감싼 배열
        let all = Json::Obj(keys.iter().map(|k| ((*k).to_string(), w.clone())).collect());
        let wrapped = Json::Arr(vec![all.clone(), w.clone()]);
        for v in [w, &all, &wrapped] {
            let _ = (
                api_map::map_pull(v),
                api_map::map_search_pull(v),
                api_map::map_issue(v),
                api_map::issue_detail(v),
                api_map::pulls_page(v),
                api_map::filtered_pull_items(v),
                api_map::issue_items(v),
                api_map::search_pulls(v),
                api_map::pull_files(v),
                api_map::pull_commits(v),
                api_map::comments_page(v),
                api_map::filters(v, v),
                api_map::check_runs_of(v, v),
                api_map::pull_detail(v, jarr_or_empty(v), None),
                api_map::aggregate_reviews(jarr_or_empty(v), 1.0),
                checks_map::check_details(v, v),
                checks_map::map_run(v),
                checks_map::runs_page(v),
                checks_map::jobs(v),
                checks_map::rollup_states(v, 3),
                accessible_repos::map_accessible_repo(v),
                launchpad_map::search_page(v).map(|(i, c)| (i.len(), c)),
                launchpad_map::slug_from_repository_url(Some(v)),
                to_epoch(Some(v)),
            );
            fuzz_jira(v);
            let mut repos = accessible_repos::accessible_page(v);
            accessible_repos::sort_accessible(&mut repos);
            let by_key = HashMap::from([("a/b".to_string(), vec!["id".to_string()])]);
            let sections = [launchpad_map::SectionHits { def: launchpad_map::SECTIONS[0], items: jarr_or_empty(v) }];
            let _ = launchpad_map::merge_launchpad(&sections, &by_key);
            calls += 1;
        }
    }
    assert!(calls >= 40);
}

/// Jira 매퍼에 같은 이상한 값을 먹인다(Cloud·Data Center 둘 다)
fn fuzz_jira(v: &Json) {
    let _ = (
        adf::adf_to_html(v),
        jira_map::map_summary(v),
        jira_map::avatar_of(Some(v)),
        jira_map::transitions(v),
        jira_map::dev_issue_id(v),
        jira_map::dev_instances(v, "pullrequest"),
        jira_map::dev_info(jarr_or_empty(v), jarr_or_empty(v)),
        jira_map::pick_date_fields(v),
        jira_map::comment_text(Some(v)),
        jira_map::myself_name(v),
        jira_map::server_comments_start_at(v),
    );
    let dates = jira_map::DateFieldIds { start: vec!["customfield_1".into()], end: vec!["customfield_2".into()] };
    for kind in [JiraKind::Cloud, JiraKind::Server] {
        let _ = (
            jira_map::issue_detail(kind, "https://x.example.net", "K-1", &dates, v),
            jira_map::comments(kind, v),
            jira_map::search_result(kind, v, Some("5"), 1),
            jira_map::search_result(kind, v, None, 8),
            jira_map::my_id(kind, v),
            jira_map::search_users(kind, v),
            jira_map::body_html(kind, Some(v), Some(v)),
        );
        let issue = jira_map::worklog_issue(v);
        let (issues, _) = jira_map::worklog_issues(&[v]);
        let users = vec!["a".to_string()];
        let q = jira_map::WorklogQuery { from: "2026-01-01", to: "2026-01-02", users: &users, include_me: true };
        let mut pairs: Vec<(&jira_map::WorklogIssue, Vec<&Json>)> = issues.iter().map(|i| (i, i.embedded.iter().collect())).collect();
        pairs.push((&issue, jira_map::dedupe_worklogs(jarr_or_empty(v))));
        let _ = jira_map::worklog_result(kind, &q, v, &pairs, false);
    }
}

/// 큰 입력도 선형에 가까운 시간에 끝난다(표시 20만 개·서로 다른 PR/커밋/사용자 10만 개). 제곱 시간이면 디버그 빌드에서도 이 상한을 넘는다
#[test]
fn 큰_입력은_제곱_시간이_아니다() {
    use std::time::{Duration, Instant};
    let timed = |label: &str, f: &dyn Fn()| {
        let t0 = Instant::now();
        f();
        let d = t0.elapsed();
        eprintln!("[큰 입력] {label}: {d:?}");
        assert!(d < Duration::from_secs(20), "{label} 이 {d:?} 걸렸다(제곱 시간 의심)");
    };
    let n = 100_000usize;
    let marks = Json::Arr((0..200_000).map(|i| obj(vec![("type", st(["strong", "em", "code", "strike", "underline"][i % 5]))])).collect());
    let big_text = obj(vec![("type", st("text")), ("text", st("x")), ("marks", marks)]);
    timed("ADF 표시 20만 개", &|| {
        let html = adf::adf_to_html(&adf_doc(vec![obj(vec![("type", st("paragraph")), ("content", Json::Arr(vec![big_text.clone()]))])]));
        assert!(html.starts_with("<p>"));
    });
    let pulls: Vec<Json> = (0..n).map(|i| obj(vec![("name", st(&format!("p{i}"))), ("url", st(&format!("https://x/{i}"))), ("lastUpdate", Json::Num(1_700_000_000_000.0 + i as f64))])).collect();
    let commits: Vec<Json> = (0..n).map(|i| obj(vec![("id", st(&format!("{:x}", 0x100_0000 + i))), ("message", st("m")), ("authorTimestamp", Json::Num(1_700_000_000_000.0 + i as f64))])).collect();
    let pull_detail = [obj(vec![("pullRequests", Json::Arr(pulls))])];
    let repo_detail = [obj(vec![("repositories", Json::Arr(vec![obj(vec![("name", st("r")), ("commits", Json::Arr(commits))])]))])];
    timed("dev-status PR·커밋 각 10만", &|| {
        let out = jira_map::dev_info(&pull_detail, &repo_detail).unwrap();
        assert_eq!(jarr_or_empty(jget(&out, "pulls")).len(), 30);
    });
    let reviews: Vec<Json> = (0..n)
        .map(|i| obj(vec![("user", obj(vec![("login", st(&format!("u{i}")))])), ("state", st(if i % 3 == 0 { "APPROVED" } else { "COMMENTED" })), ("submitted_at", st("2026-01-01T00:00:00Z"))]))
        .collect();
    timed("리뷰 10만(서로 다른 사용자)", &|| {
        assert_eq!(api_map::aggregate_reviews(&reviews, 0.0), Ok("approved"));
    });
    let issues: Vec<Json> = (0..n)
        .map(|i| obj(vec![("number", Json::Num(i as f64)), ("title", st("t")), ("repository_url", st("https://api.github.com/repos/o/r")), ("updated_at", st("2026-01-01T00:00:00Z"))]))
        .collect();
    timed("Launchpad 10만", &|| {
        let by_key = HashMap::from([("o/r".to_string(), vec!["id".to_string()])]);
        let sections = [launchpad_map::SectionHits { def: launchpad_map::SECTIONS[0], items: &issues }];
        assert_eq!(launchpad_map::merge_launchpad(&sections, &by_key).unwrap().len(), n);
    });
    let fields: Vec<Json> = (0..n).map(|i| obj(vec![("id", st(&format!("customfield_{i}"))), ("name", st("Start date"))])).collect();
    timed("날짜 필드 10만", &|| {
        assert_eq!(jira_map::pick_date_fields(&Json::Arr(fields.clone())).start.len(), 8);
    });
    let worklogs: Vec<Json> = (0..n).map(|i| obj(vec![("id", st(&i.to_string())), ("author", obj(vec![("accountId", st("a"))])), ("started", st("2026-01-01T00:00:00.000+0000"))])).collect();
    timed("워크로그 10만", &|| {
        let issue = jira_map::WorklogIssue { key: "K-1".into(), summary: "s".into(), issue_type: "t".into(), embedded: vec![], complete: true };
        let users = vec![];
        let q = jira_map::WorklogQuery { from: "2026-01-01", to: "2026-01-02", users: &users, include_me: false };
        let me = obj(vec![("id", st("a")), ("name", st("a"))]);
        let pairs = vec![(&issue, jira_map::dedupe_worklogs(&worklogs))];
        let out = jira_map::worklog_result(JiraKind::Cloud, &q, &me, &pairs, false).unwrap();
        // 2만 건 상한(WORKLOG_MAX)에서 자르고 truncated 로 알린다
        assert_eq!(jarr_or_empty(jget(&out, "worklogs")).len(), 20_000);
        assert_eq!(jget(&out, "truncated"), &Json::Bool(true));
    });
    let repos: Vec<Json> = (0..n).map(|i| obj(vec![("name", st("r")), ("owner", obj(vec![("login", st("o"))])), ("html_url", st("https://example.com/o/r")), ("pushed_at", st(&format!("2026-01-01T00:00:{:02}Z", i % 60)))])).collect();
    timed("접근 가능한 저장소 10만", &|| {
        let mut v = accessible_repos::accessible_page(&Json::Arr(repos.clone()));
        accessible_repos::sort_accessible(&mut v);
        assert_eq!(v.len(), n);
    });
}

#[test]
fn 날짜_문자열의_제어_문자와_거대_입력도_패닉하지_않는다() {
    for t in ["\u{0}", "2026\u{0}", "２０２６-01-01", &"9".repeat(100_000), &format!("2026-01-01T00:00:00.{}Z", "9".repeat(50_000)), "2026-01-01T00:00:00+\u{0}", "-", "+", "T", "2026-01-01T"] {
        let _ = parse_date_ms(t);
    }
    assert_eq!(parse_date_ms("2026-01-01T00:00:00.9999999999Z"), parse_date_ms("2026-01-01T00:00:00.999Z"));
}

#[test]
fn 차이_check_runs_가_문자열이면_rust_는_오류다() {
    // TS 는 `for (const run of "abc")` 가 글자마다 돌아 항목이 생긴다. 응답 모양이 아니라 Rust 는 오류로 본다
    let runs = json::parse(r#"{"check_runs":"abc"}"#).unwrap();
    let status = json::parse("{}").unwrap();
    assert!(api_map::check_runs_of(&runs, &status).is_err());
}

fn kind_of(j: &Json) -> JiraKind {
    match jstr(j) {
        "cloud" => JiraKind::Cloud,
        "server" => JiraKind::Server,
        other => panic!("종류: {other}"),
    }
}

const SITE: &str = "https://x.example.net";

/// `Json` 로 쓴 일반 문자열 값
fn jnum(j: &Json) -> f64 {
    j.as_f64().unwrap_or_else(|| panic!("숫자가 아니다: {j:?}"))
}

/// 입력 JSON 대신 코드로 만든 ADF(JSON 깊이 상한 때문에 텍스트로 쓸 수 없는 크기)
fn adf_text_para(text: &str) -> Json {
    obj(vec![("type", st("paragraph")), ("content", Json::Arr(vec![obj(vec![("type", st("text")), ("text", st(text))])]))])
}

fn adf_doc(content: Vec<Json>) -> Json {
    obj(vec![("type", st("doc")), ("content", Json::Arr(content))])
}

/// UTF-16 마지막 `n` 단위(짝 없는 서로게이트는 U+FFFD — `toWellFormed`)
fn tail16(s: &str, n: usize) -> String {
    let u: Vec<u16> = s.encode_utf16().collect();
    String::from_utf16_lossy(&u[u.len().saturating_sub(n)..])
}

fn head16(s: &str, n: usize) -> String {
    let u: Vec<u16> = s.encode_utf16().take(n).collect();
    String::from_utf16_lossy(&u)
}

fn html_stats(html: &str) -> Json {
    obj(vec![("len", Json::Num(html.encode_utf16().count() as f64)), ("head", st(&head16(html, 20))), ("tail", st(&tail16(html, 20)))])
}

fn dates_of(j: &Json) -> jira_map::DateFieldIds {
    let list = |k: &str| jarr_or_empty(jget(j, k)).iter().map(|t| jstr(t).to_string()).collect();
    jira_map::DateFieldIds { start: list("start"), end: list("end") }
}

fn run_jira_case(f: &str, a: &[Json]) -> MapResult<Json> {
    let null = Json::Null;
    let one = |i: usize| a.get(i).unwrap_or(&null);
    let list = |v: Vec<Json>| Ok(Json::Arr(v));
    match f {
        "adfToHtml" => Ok(st(&adf::adf_to_html(one(0)))),
        "adfDeep" => {
            let mut inner = adf_text_para("끝");
            for _ in 0..jnum(one(0)) as usize {
                inner = obj(vec![("type", st("blockquote")), ("content", Json::Arr(vec![inner]))]);
            }
            let html = adf::adf_to_html(&adf_doc(vec![inner]));
            Ok(obj(vec![("len", Json::Num(html.encode_utf16().count() as f64)), ("bq", Json::Num(html.matches("<blockquote>").count() as f64)), ("text", Json::Bool(html.contains('끝')))]))
        }
        "adfBig" => {
            let paras = (0..jnum(one(0)) as usize).map(|_| adf_text_para(&"a".repeat(jnum(one(1)) as usize))).collect();
            Ok(html_stats(&adf::adf_to_html(&adf_doc(paras))))
        }
        "adfBigEmoji" => Ok(html_stats(&adf::adf_to_html(&adf_doc(vec![adf_text_para(&"😀".repeat(jnum(one(0)) as usize))])))),
        "avatarOf" => Ok(st(&jira_map::avatar_of(Some(one(0))))),
        "mapSummary" => Ok(jira_map::map_summary(one(0))),
        "myself" => Ok(st(&jira_map::myself_name(one(1)))),
        "getMyId" => jira_map::my_id(kind_of(one(0)), one(1)),
        "searchUsers" => list(jira_map::search_users(kind_of(one(0)), one(1))),
        "getTransitions" => list(jira_map::transitions(one(1))),
        "getIssue" => Ok(jira_map::issue_detail(kind_of(one(0)), SITE, jstr(one(1)), &dates_of(one(2)), one(3))),
        "getComments" => {
            let kind = kind_of(one(0));
            let first = one(1);
            let last = match (kind, jira_map::server_comments_start_at(first)) {
                (JiraKind::Server, Some(_)) => a.get(2).unwrap_or(first),
                _ => first,
            };
            list(jira_map::comments(kind, last))
        }
        "getDevInfo" => {
            if jira_map::dev_issue_id(one(1)).is_none() {
                return Ok(jira_map::no_dev_info());
            }
            let details = |part: &str, data: &Json| -> Vec<Json> {
                let n = jira_map::dev_instances(one(2), part).len();
                (0..n).flat_map(|_| jarr_or_empty(field(Some(data), "detail").unwrap_or(&Json::Null)).to_vec()).collect()
            };
            jira_map::dev_info(&details("pullrequest", one(3)), &details("repository", one(4)))
        }
        "search" => {
            let cursor = match one(2) {
                Json::Str(t) => Some(t.as_str()),
                _ => None,
            };
            jira_map::search_result(kind_of(one(0)), one(1), cursor, jnum(one(3)) as u32)
        }
        "pickDateFields" => Ok(jira_map::pick_date_fields(one(0)).to_json()),
        "getWorklogs" => {
            let kind = kind_of(one(0));
            let q = one(1);
            let users: Vec<String> = jarr_or_empty(jget(q, "users")).iter().map(|t| jstr(t).to_string()).collect();
            let query = jira_map::WorklogQuery { from: jstr(jget(q, "from")), to: jstr(jget(q, "to")), users: &users, include_me: matches!(q.get("includeMe"), Some(Json::Bool(true))) };
            let (issues, truncated) = jira_map::worklog_issues(&[one(3)]);
            let fetched = one(4);
            let mut pairs: Vec<(&jira_map::WorklogIssue, Vec<&Json>)> = Vec::new();
            for issue in &issues {
                let entries: Vec<&Json> = if issue.complete {
                    issue.embedded.iter().collect()
                } else {
                    // 이슈별로 다시 받은 응답(없으면 TS 는 그 경로의 요청 오류를 던진다)
                    let hit = fetched.get(&issue.key).ok_or(MapError)?;
                    jira_map::dedupe_worklogs(jarr_or_empty(jget(hit, "worklogs")))
                };
                pairs.push((issue, entries));
            }
            jira_map::worklog_result(kind, &query, one(2), &pairs, truncated)
        }
        other => panic!("처리기가 없는 fn: {other}"),
    }
}

#[test]
fn jira_응답_매퍼가_ts_와_같다() {
    let g = golden_json("jiramap.json");
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let (mut ok, mut err) = (0, 0);
    let mut bad: Vec<String> = Vec::new();
    for c in jarr(jget(&g, "cases"), "cases") {
        let f = jstr(jget(c, "fn")).to_string();
        seen.insert(f.clone());
        let got = run_jira_case(&f, &input(c));
        match (c.get("err").is_some(), got) {
            (true, Err(_)) => err += 1,
            (false, Ok(v)) if &v == jget(c, "out") => ok += 1,
            (expect_err, got) => bad.push(format!(
                "{f}({:.300}) → Rust {:.600}, TS {:.600}",
                jarr_or_empty(jget(c, "in")).iter().map(|t| jstr(t).to_string()).collect::<Vec<_>>().join(" , "),
                got.map_or_else(|_| "오류".to_string(), |v| stringify(&v)),
                if expect_err { "오류".to_string() } else { stringify(jget(c, "out")) }
            )),
        }
    }
    assert!(bad.is_empty(), "TS 와 다른 결과 {}건, 예: {:#?}", bad.len(), &bad[..bad.len().min(8)]);
    assert!(ok > 150 && err > 10, "확인한 사례가 적다: 정상 {ok}, 오류 {err}");
    assert!(seen.len() >= 15, "fn 종류가 적다: {seen:?}");
}

/// 렌더러로 가는 HTML 의 안전성: 어떤 문자열 자리에 공격 문자열을 넣어도 태그는 고정 목록뿐이고 `href` 는 http(s) 뿐이며 이스케이프하지 않은 `<`·따옴표가 속성 안에 없다
#[test]
fn adf_출력은_허용_태그와_http_링크뿐이다() {
    let attack = ["<script>alert(1)</script>", "\"><img src=x onerror=alert(1)>", "' onmouseover='x", "javascript:alert(1)", "https://x.example/\"><script>", "&lt;&amp;", "\u{0}</p><iframe>"];
    let allowed = ["p", "br", "strong", "em", "code", "s", "u", "sub", "sup", "a", "h1", "h2", "h3", "h4", "h5", "h6", "ul", "ol", "li", "blockquote", "pre", "hr", "table", "tbody", "tr", "th", "td", "details", "summary", "mark"];
    for atk in attack {
        let a = st(atk);
        let doc = adf_doc(vec![
            obj(vec![("type", st("heading")), ("attrs", obj(vec![("level", a.clone())])), ("content", Json::Arr(vec![obj(vec![("type", st("text")), ("text", a.clone()), ("marks", Json::Arr(vec![obj(vec![("type", st("link")), ("attrs", obj(vec![("href", a.clone())]))]), obj(vec![("type", a.clone())])]))])]))]),
            obj(vec![("type", st("paragraph")), ("content", Json::Arr(vec![
                obj(vec![("type", st("mention")), ("attrs", obj(vec![("text", a.clone())]))]),
                obj(vec![("type", st("emoji")), ("attrs", obj(vec![("text", a.clone()), ("shortName", a.clone())]))]),
                obj(vec![("type", st("status")), ("attrs", obj(vec![("text", a.clone())]))]),
                obj(vec![("type", st("inlineCard")), ("attrs", obj(vec![("url", a.clone())]))]),
                obj(vec![("type", st("date")), ("attrs", obj(vec![("timestamp", a.clone())]))]),
            ]))]),
            obj(vec![("type", st("expand")), ("attrs", obj(vec![("title", a.clone())])), ("content", Json::Arr(vec![]))]),
            obj(vec![("type", st("codeBlock")), ("content", Json::Arr(vec![obj(vec![("text", a.clone())])]))]),
            obj(vec![("type", st("orderedList")), ("attrs", obj(vec![("order", a.clone())])), ("content", Json::Arr(vec![]))]),
        ]);
        let html = adf::adf_to_html(&doc);
        assert!(!html.contains("<script") && !html.contains("<img") && !html.contains("<iframe"), "{atk}: {html}");
        let mut rest = html.as_str();
        while let Some(i) = rest.find('<') {
            let tag = &rest[i + 1..];
            let end = tag.find('>').unwrap_or_else(|| panic!("닫히지 않은 태그: {html}"));
            let inner = &tag[..end];
            let name = inner.trim_start_matches('/').split([' ', '/']).next().unwrap_or("");
            assert!(allowed.contains(&name), "허용 밖 태그 {name}: {html}");
            if let Some(h) = inner.strip_prefix("a href=\"") {
                let v = h.strip_suffix('"').unwrap_or_else(|| panic!("href 가 닫히지 않았다: {inner}"));
                assert!(v.starts_with("http://") || v.starts_with("https://"), "http(s) 가 아닌 링크: {v}");
                assert!(!v.contains(['<', '>', '"', '\'']), "href 에 이스케이프하지 않은 문자: {v}");
            } else {
                assert!(!inner.contains(' ') || name == "ol", "속성이 있는 태그: {inner}");
            }
            rest = &tag[end + 1..];
        }
    }
}

#[test]
fn adf_깊이_상한_50_과_출력_상한은_코드로_만든_입력에서도_지킨다() {
    // JSON 텍스트로는 깊이 64 를 못 넘지만 코드로 만든 값은 넘을 수 있다 — 60 겹이어도 50 에서 멈춘다
    let mut inner = adf_text_para("끝");
    for _ in 0..60 {
        inner = obj(vec![("type", st("blockquote")), ("content", Json::Arr(vec![inner]))]);
    }
    let html = adf::adf_to_html(&adf_doc(vec![inner]));
    assert_eq!(html.matches("<blockquote>").count(), 50);
    assert!(!html.contains('끝'));
}

#[test]
fn 차이_시간대_없는_워크로그_시각은_범위_밖으로_본다() {
    let me = json::parse(r#"{"id":"a","name":"a"}"#).unwrap();
    let issue = json::parse(r#"{"key":"K-1","fields":{"summary":"s","issuetype":{"name":"t"},"worklog":{"worklogs":[{"id":"1","author":{"accountId":"a"},"started":"2026-01-11T00:00:00"},{"id":"2","author":{"accountId":"a"},"started":"2026-01-11T00:00:00Z"}]}}}"#).unwrap();
    let (issues, _) = jira_map::worklog_issues(&[&json::parse(&format!(r#"{{"issues":[{}]}}"#, stringify(&issue))).unwrap()]);
    let users: Vec<String> = vec![];
    let q = jira_map::WorklogQuery { from: "2026-01-10", to: "2026-01-12", users: &users, include_me: false };
    let pairs: Vec<(&jira_map::WorklogIssue, Vec<&Json>)> = issues.iter().map(|i| (i, i.embedded.iter().collect())).collect();
    let out = jira_map::worklog_result(JiraKind::Cloud, &q, &me, &pairs, false).unwrap();
    // TS 는 로컬 시간으로 풀어 첫 항목도 받는다(시간대마다 범위 경계가 달라질 뿐 이 날짜는 항상 안이다). Rust 는 UTC 표기가 있는 것만 받는다
    assert_eq!(jarr_or_empty(jget(&out, "worklogs")).len(), 1);
}

#[test]
fn 차이_dev_status_의_숫자_시각이_범위_밖이면_ts_처럼_오류다() {
    let d = json::parse(r#"{"pullRequests":[{"name":"p","url":"https://x/1","lastUpdate":1e300}]}"#).unwrap();
    assert!(jira_map::dev_info(&[d], &[]).is_err());
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
