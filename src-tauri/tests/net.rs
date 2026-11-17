//! GitHub·Jira 순수 로직의 골든 패리티 테스트. `tests/golden/net/*.json`(TS 가 기준: `UPDATE_GOLDEN=1 npx vitest run tests/goldenNet.test.ts`)을
//! 모두 읽어 Rust 이식(`antler_lib::{net, github, jira}`)이 같은 결과를 내는지 본다. **건너뛰는 골든이 없다** — 처리기 없는 파일이 있으면 실패한다.
//! 안전 방향 확인: URL 판정 격자 전체에서 "Rust 허용 ⇒ TS 허용"을 따로 센다(`url_격자_판정이_ts_와_같다_허용_범위는_ts_안이다`).
//! 골든 테스트는 네트워크를 쓰지 않는다. 맨 아래 `mod live`(PR 3)만 127.0.0.1 의 목 서버로 HTTP 코어와 두 클라이언트를 시험한다(외부 접속 없음). 알려진 차이는 각각 `차이_…` 이름의 테스트가 단언한다.

use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use antler_lib::github::{accessible_repos, api_map, checks_map, launchpad_map};
use antler_lib::github::device_rules::{interpret_start, DeviceError, PollState, PollStep, EXPIRED_MESSAGE};
use antler_lib::github::gh_env::gh_env;
use antler_lib::github::queries::{reviews_page, soft_checks};
use antler_lib::github::validate as gh;
use antler_lib::jira::validate::{self as jira, SiteOptions};
use antler_lib::jira::{adf, api_map as jira_map, auth_header, JiraKind};
use antler_lib::net::http_error::{
    github_body_message, github_http_error, has_next_link, jira_body_message, jira_http_error, parse_http_date, parse_rate, parse_retry_after, RateHeaders,
};
use antler_lib::net::js_date::{iso_string, parse_date_ms, to_epoch};
use antler_lib::net::js_value::{field, must_arr, obj, stringify, MapError, MapResult};
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

fn site_options(allow_loopback: bool) -> SiteOptions {
    if allow_loopback {
        SiteOptions::insecure_loopback_for_test()
    } else {
        SiteOptions::default()
    }
}

fn opts(loop_: &Value) -> SiteOptions {
    site_options(loop_.as_bool().expect("loop"))
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
            let got = jira::parse_site_url(&raw, site_options(loop_));
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
        let k = jira::allowed_jira_url(&raw, Some("http://127.0.0.1:8080/jira"), SiteOptions::insecure_loopback_for_test());
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
        d.eq(|| format!("github bodyMessage({:.80})", c["t"]), &json!(github_body_message(s(&c["t"]), &[])), &c["out"]);
    }
    for c in arr(&g["bodyJira"], "bodyJira") {
        d.eq(|| format!("jira bodyMessage({:.80})", c["t"]), &json!(jira_body_message(s(&c["t"]), &[])), &c["out"]);
    }
    d.finish("오류 본문 메시지");
}

/// 비밀이 200 단위 경계에 걸쳐도 남지 않는다 — 가린 **뒤에** 자른다(자른 뒤에 가리면 잘린 접두가 그대로 남는다)
#[test]
fn 본문_메시지는_가린_뒤에_자른다() {
    use base64::engine::general_purpose::STANDARD;
    use base64::Engine;

    // 반복·접두 겹침이 없는 영숫자 토큰(결정적)
    fn token(len: usize) -> String {
        const A: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
        let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
        let body: String = (0..len)
            .map(|_| {
                x = x.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
                A[((x >> 33) % A.len() as u64) as usize] as char
            })
            .collect();
        format!("tok_{body}")
    }
    /// 비밀의 길이 8 이상 접두(8 단위부터 전체까지)가 문구에 남아 있으면 그 접두
    fn leaked<'a>(out: &str, secret: &'a str) -> Option<&'a str> {
        (8..=secret.len()).map(|n| &secret[..n]).find(|p| out.contains(p))
    }

    let long = token(300);
    let at_190 = token(60);
    let email = "someone@example.com";
    let basic = STANDARD.encode(format!("{email}:{}", token(220)));
    assert!(basic.len() > 200);
    let cases: Vec<(&str, String, String)> = vec![
        ("200 단위보다 긴 토큰", long.clone(), format!("실패: {long}")),
        ("190번째 단위 근처에서 시작하는 토큰", at_190.clone(), format!("{}{at_190} 끝", "x".repeat(190))),
        ("200자를 넘는 base64(email:token)", basic.clone(), format!("Basic {basic}")),
        ("경계 바로 앞에서 시작하는 base64", basic.clone(), format!("{}{basic}", "y".repeat(199))),
    ];
    for (name, secret, message) in cases {
        let secrets = [secret.as_str()];
        let gh_body = serde_json::json!({ "message": message }).to_string();
        let jira_body = serde_json::json!({ "errorMessages": [message] }).to_string();
        let jira_fields = serde_json::json!({ "errors": { "a": "앞", "b": message } }).to_string();

        let gh = github_body_message(&gh_body, &secrets);
        let ji = jira_body_message(&jira_body, &secrets);
        let jf = jira_body_message(&jira_fields, &secrets);
        let rh = RateHeaders { remaining: None, limit: None, reset: None, retry_after: None };
        let gh_err = github_http_error(403, &rh, &gh_body, true, &|_| String::new(), &secrets).message;
        let ji_err = jira_http_error(403, None, &jira_body, JiraKind::Cloud, None, 0.0, &secrets).message;
        for (what, out) in [("gh", &gh), ("jira", &ji), ("jira errors", &jf), ("gh 오류", &gh_err), ("jira 오류", &ji_err)] {
            assert_eq!(leaked(out, &secret), None, "{name} / {what}: 비밀의 접두가 남았다: {out}");
            // 199 단위 뒤에서 가려진 `***` 는 200 단위 상한에서 `*` 하나만 남을 수 있고, 필드 앞 문구가 붙는 `errors` 는 통째로 잘릴 수 있다
            assert!(out.contains('*') || what == "jira errors","{name} / {what}: 가린 흔적이 없다: {out}");
        }
    }
}

#[test]
fn jira_오류_분류가_ts_와_같다() {
    let g = golden("http.json");
    let mut d = Diffs::default();
    for c in arr(&g["jiraError"], "jiraError") {
        let kind = if s(&c["kind"]) == "cloud" { JiraKind::Cloud } else { JiraKind::Server };
        let e = jira_http_error(c["status"].as_u64().expect("상태") as u16, opt_s(&c["retry"]), s(&c["body"]), kind, opt_s(&c["notFound"]), 1_700_000_000_000.0, &[]);
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
        let e = github_http_error(c["status"].as_u64().expect("상태") as u16, &rh, s(&c["body"]), c["signed"].as_bool().expect("signed"), &|_| "{TIME}".to_string(), &[]);
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
            assert_eq!(json!(github_body_message(&text, &[])), c["gh"], "gh depth {depth}");
            assert_eq!(json!(jira_body_message(&text, &[])), c["jira"], "jira depth {depth}");
            within += 1;
        } else if depth > MAX_DEPTH + 2 {
            assert!(!rust_ok, "depth {depth}: 상한을 넘는데 받았다");
            assert_eq!(github_body_message(&text, &[]), "");
            assert_eq!(jira_body_message(&text, &[]), "");
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

/// 운영 코드의 끝(바이트 위치) — 들여쓰기 없는 `#[cfg(test)]` 바로 다음 줄이 `mod ` 인 곳(파일 끝의 시험 모듈)이다.
/// 메서드 앞의 `#[cfg(test)] fn ...` 같은 시험 전용 항목에서는 멈추지 않는다(그 뒤의 운영 코드가 검사에서 빠진다)
fn prod_end(text: &str) -> usize {
    let mut off = 0;
    let mut lines = text.split_inclusive('\n').peekable();
    while let Some(l) = lines.next() {
        if l.trim_end_matches(['\r', '\n']) == "#[cfg(test)]" && lines.peek().is_some_and(|n| n.starts_with("mod ")) {
            return off;
        }
        off += l.len();
    }
    text.len()
}

#[test]
fn 운영_코드_끝_검출기는_시험_전용_항목에서_멈추지_않는다() {
    // 들여쓰기 있는 `#[cfg(test)] fn` 뒤의 운영 코드도 운영 구간이다
    let src = "impl A {\n    #[cfg(test)]\n    fn count(&self) {}\n    fn prod(&self) { PROD }\n}\n#[cfg(test)]\nmod tests { TEST }\n";
    let body = &src[..prod_end(src)];
    assert!(body.contains("PROD") && !body.contains("TEST"));
    // 들여쓰기 없는 `#[cfg(test)]` 라도 다음 줄이 `mod ` 가 아니면 멈추지 않는다
    let src = "#[cfg(test)]\nfn only_test() {}\nfn prod() { PROD }\n#[cfg(test)]\r\nmod tests { TEST }\n";
    let body = &src[..prod_end(src)];
    assert!(body.contains("PROD") && !body.contains("TEST"));
    // 시험 모듈이 없으면 전부다
    assert_eq!(prod_end("fn a() {}\n"), 10);
}

/// `SecretString` 은 `Display`·`Serialize` 를 구현하지 않는다 — 위 타입 단언이 컴파일 시점에 보장하고, 아래는 소스 문자열 검사(보조)와 `Debug` 가 `***` 인지 본다
#[test]
fn secret_string_은_display_와_serialize_가_없다() {
    let src = fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src").join("net").join("redact.rs")).unwrap();
    // 비교 대상은 테스트 모듈 앞까지(테스트 안의 설명 문자열은 제외)
    let body = &src[..prod_end(&src)];
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
                let reviews = reviews_page(one(1));
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
    let weird = weird_values(100_000);
    // GitHub 매퍼가 읽는 키 + ADF·Jira 매퍼가 읽는 키(`src/jira/adf.rs`·`api_map.rs` 의 `field(…, "…")` 전부)
    let keys = [
        "number", "title", "state", "draft", "merged_at", "user", "labels", "assignees", "head", "base", "updated_at", "created_at", "comments", "html_url", "pull_request", "requested_reviewers",
        "requested_teams", "submitted_at", "login", "avatar_url", "name", "color", "items", "check_runs", "statuses", "workflow_runs", "jobs", "conclusion", "status", "repository", "repository_url",
        "owner", "pushed_at", "filename", "commit", "author", "total_count", "incomplete_results",
        // ADF
        "type", "content", "marks", "attrs", "text", "href", "url", "level", "order", "timestamp", "shortName",
        // Jira
        "fields", "issues", "worklogs", "worklog", "avatarUrls", "48x48", "32x32", "24x24", "pullRequests", "repositories", "commits", "transitions", "id", "key", "summary", "issuetype", "priority",
        "assignee", "reporter", "updated", "created", "duedate", "description", "renderedFields", "body", "renderedBody", "comment", "to", "statusCategory", "schema", "scope", "total", "isLast",
        "nextPageToken", "startAt", "byInstanceType", "detail", "repositoryName", "source", "branch", "lastUpdate", "message", "authorTimestamp", "started", "timeSpentSeconds", "accountId",
        "accountType", "displayName", "emailAddress", "pullrequest",
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

/// 이상한 값 모음. `big` 은 거대 배열의 길이(객체 배열은 그 1/5)
fn weird_values(big: usize) -> Vec<Json> {
    let mut nest = Json::Str("\u{0}\u{1f}\u{7f}\u{feff}😀\u{fffd}".into());
    for _ in 0..json::MAX_DEPTH {
        nest = Json::Arr(vec![nest]);
    }
    let deep_obj = (0..json::MAX_DEPTH).fold(Json::Null, |acc, _| Json::Obj(vec![("x".into(), acc)]));
    let big_arr = Json::Arr((0..big).map(|i| Json::Num(i as f64)).collect());
    let big_objs = Json::Arr((0..big / 5).map(|_| Json::Obj(vec![("number".into(), Json::Num(1.0))])).collect());
    vec![
        Json::Null,
        Json::Bool(true),
        Json::Bool(false),
        Json::Num(f64::INFINITY),
        Json::Num(f64::NAN),
        Json::Num(-0.0),
        Json::Num(1e308),
        Json::Num(-1.0),
        Json::Num(3.0),
        Json::Num(2.5),
        Json::Num(1e6),
        Json::Num(1e15),
        st(""),
        st("\u{0}\u{1}\u{1f}\u{7f}\"\\</script>"),
        st("2026-01-01T00:00:00Z\u{0}"),
        st("<script>alert(1)</script>"),
        st("javascript:alert(1)"),
        st("https://x.example/\"><script>"),
        st("7"),
        Json::Arr(vec![]),
        Json::Obj(vec![]),
        nest,
        deep_obj,
        big_arr,
        big_objs,
    ]
}

/// `t` 의 모든 자리(뿌리 포함)를 한 번에 하나씩 `w` 로 바꾼 사본들. 모양을 갖춘 문서의 각 자리에 이상한 값을 끼우는 데 쓴다
fn variants(t: &Json, w: &Json) -> Vec<Json> {
    let mut out = vec![w.clone()];
    match t {
        Json::Obj(kv) => {
            for (i, (_, child)) in kv.iter().enumerate() {
                for v in variants(child, w) {
                    let mut c = kv.clone();
                    c[i].1 = v;
                    out.push(Json::Obj(c));
                }
            }
        }
        Json::Arr(items) => {
            for (i, child) in items.iter().enumerate() {
                for v in variants(child, w) {
                    let mut c = items.clone();
                    c[i] = v;
                    out.push(Json::Arr(c));
                }
            }
        }
        _ => {}
    }
    out
}

const ADF_TAGS: [&str; 30] = [
    "p", "br", "strong", "em", "code", "s", "u", "sub", "sup", "a", "h1", "h2", "h3", "h4", "h5", "h6", "ul", "ol", "li", "blockquote", "pre", "hr", "table", "tbody", "tr", "th", "td", "details", "summary", "mark",
];

/// ADF 출력의 안전성 위반 첫 건(없으면 `None`): 날것의 `<` 는 허용 태그의 온전한 여는·닫는 태그여야 하고, `href` 는 http(s) 뿐이며, `ol` 말고는 속성이 없다
fn adf_unsafe(html: &str) -> Option<String> {
    let mut rest = html;
    while let Some(i) = rest.find('<') {
        let tag = &rest[i + 1..];
        let Some(end) = tag.find('>') else { return Some(format!("닫히지 않은 태그(날것의 `<`): …{}", tail16(rest, 40))) };
        let inner = &tag[..end];
        let name = inner.trim_start_matches('/').split([' ', '/']).next().unwrap_or("");
        if !ADF_TAGS.contains(&name) {
            return Some(format!("허용 밖 태그 {name:?}"));
        }
        if let Some(h) = inner.strip_prefix("a href=\"") {
            let Some(v) = h.strip_suffix('"') else { return Some(format!("href 가 닫히지 않았다: {inner}")) };
            if !(v.starts_with("http://") || v.starts_with("https://")) {
                return Some(format!("http(s) 가 아닌 링크: {v}"));
            }
            if v.contains(['<', '>', '"', '\'']) {
                return Some(format!("href 에 이스케이프하지 않은 문자: {v}"));
            }
        } else if name == "ol" && inner.contains(' ') {
            let ok = inner.strip_prefix("ol start=\"").and_then(|r| r.strip_suffix('"')).is_some_and(|d| !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit()));
            if !ok {
                return Some(format!("ol 의 속성이 start 숫자가 아니다: {inner}"));
            }
        } else if inner.contains(' ') {
            return Some(format!("속성이 있는 태그: {inner}"));
        }
        rest = &tag[end + 1..];
    }
    None
}

fn adf_node(ty: &str, attrs: Option<Vec<(&str, Json)>>, content: Option<Vec<Json>>) -> Json {
    let mut o = vec![("type", st(ty))];
    if let Some(a) = attrs {
        o.push(("attrs", obj(a)));
    }
    if let Some(c) = content {
        o.push(("content", Json::Arr(c)));
    }
    obj(o)
}

fn adf_text(text: &str, marks: Vec<Json>) -> Json {
    obj(vec![("type", st("text")), ("text", st(text)), ("marks", Json::Arr(marks))])
}

/// 모든 노드·표시·속성을 한 번씩 쓰는, 모양을 갖춘 ADF 문서
fn adf_template() -> Json {
    let mark = |ty: &str| obj(vec![("type", st(ty))]);
    let para = |c: Vec<Json>| adf_node("paragraph", None, Some(c));
    let plain = || adf_text("글", vec![]);
    adf_node("doc", None, Some(vec![
        para(vec![
            adf_text("굵게", vec![mark("strong"), mark("em"), mark("code"), mark("strike"), mark("underline")]),
            adf_text("위", vec![obj(vec![("type", st("subsup")), ("attrs", obj(vec![("type", st("sub"))]))])]),
            adf_text("링크", vec![obj(vec![("type", st("link")), ("attrs", obj(vec![("href", st("https://a.example/x"))]))])]),
            adf_node("hardBreak", None, None),
            adf_node("mention", Some(vec![("text", st("@kim"))]), None),
            adf_node("emoji", Some(vec![("text", st("😀")), ("shortName", st(":smile:"))]), None),
            adf_node("status", Some(vec![("text", st("진행"))]), None),
            adf_node("date", Some(vec![("timestamp", Json::Num(1_767_225_600_000.0))]), None),
            adf_node("inlineCard", Some(vec![("url", st("https://a.example/card"))]), None),
        ]),
        adf_node("heading", Some(vec![("level", Json::Num(2.0))]), Some(vec![plain()])),
        adf_node("bulletList", None, Some(vec![adf_node("listItem", None, Some(vec![para(vec![plain()])]))])),
        adf_node("orderedList", Some(vec![("order", Json::Num(3.0))]), Some(vec![adf_node("listItem", None, Some(vec![para(vec![plain()])]))])),
        adf_node("taskList", None, Some(vec![adf_node("taskItem", Some(vec![("state", st("DONE"))]), Some(vec![plain()]))])),
        adf_node("blockquote", None, Some(vec![para(vec![plain()])])),
        adf_node("panel", None, Some(vec![para(vec![plain()])])),
        adf_node("codeBlock", None, Some(vec![obj(vec![("type", st("text")), ("text", st("let a = 1 < 2;"))])])),
        adf_node("rule", None, None),
        adf_node("table", None, Some(vec![adf_node("tableRow", None, Some(vec![
            adf_node("tableHeader", None, Some(vec![para(vec![plain()])])),
            adf_node("tableCell", None, Some(vec![para(vec![plain()])])),
        ]))])),
        adf_node("expand", Some(vec![("title", st("제목"))]), Some(vec![para(vec![plain()])])),
        adf_node("nestedExpand", Some(vec![("title", st("안쪽"))]), Some(vec![para(vec![plain()])])),
        adf_node("media", None, None),
        adf_node("blockCard", Some(vec![("url", st("https://a.example/block"))]), None),
    ]))
}

#[test]
fn adf_는_모양을_갖춘_문서의_모든_자리에_이상한_값이_들어와도_패닉하지_않고_안전하다() {
    let t = adf_template();
    let base = adf::adf_to_html(&t);
    for want in ["<em><strong>", "<sub>", "<a href=\"https://a.example/x\">", "<br>", "<h2>", "<ol start=\"3\">", "<pre><code>let a = 1 &lt; 2;", "<table><tbody><tr><th>", "<details><summary>제목", "<mark>", "<hr>", "2026-01-01", "<blockquote>"] {
        assert!(base.contains(want), "기준 문서에 {want} 가 없다: {base}");
    }
    assert_eq!(adf_unsafe(&base), None);
    let mut n = 0usize;
    for w in weird_values(3_000) {
        for v in variants(&t, &w) {
            let html = adf::adf_to_html(&v);
            if let Some(why) = adf_unsafe(&html) {
                panic!("{why} (끼운 값 {:.80}) → {:.300}", stringify(&w), html);
            }
            n += 1;
        }
    }
    assert!(n > 2_000, "시험한 변형이 적다: {n}");
}

/// Jira 응답의 모양을 갖춘 견본(이름, 값). 중첩 자리마다 이상한 값을 끼워 매퍼에 먹인다
fn jira_templates() -> Vec<(&'static str, Json)> {
    let user = |id: &str| {
        obj(vec![
            ("accountId", st(id)),
            ("accountType", st("atlassian")),
            ("name", st(id)),
            ("key", st(id)),
            ("displayName", st("김")),
            ("emailAddress", st("a@b.example")),
            ("avatarUrls", obj(vec![("48x48", st("https://a.example/48")), ("32x32", st("https://a.example/32")), ("24x24", st("https://a.example/24"))])),
        ])
    };
    let adf_small = || adf_node("doc", None, Some(vec![adf_node("paragraph", None, Some(vec![adf_text("링크", vec![obj(vec![("type", st("link")), ("attrs", obj(vec![("href", st("https://a.example/x"))]))])])]))]));
    let worklog = obj(vec![
        ("id", st("w1")),
        ("author", user("a")),
        ("started", st("2026-01-11T00:00:00.000Z")),
        ("timeSpentSeconds", Json::Num(600.0)),
        ("comment", adf_small()),
    ]);
    let issue = obj(vec![
        ("id", st("10001")),
        ("key", st("K-1")),
        ("fields", obj(vec![
            ("summary", st("요약")),
            ("status", obj(vec![("name", st("진행")), ("statusCategory", obj(vec![("key", st("indeterminate"))]))])),
            ("issuetype", obj(vec![("name", st("Task"))])),
            ("priority", obj(vec![("name", st("High"))])),
            ("assignee", user("a")),
            ("reporter", user("b")),
            ("updated", st("2026-01-02T00:00:00.000Z")),
            ("created", st("2026-01-01T00:00:00.000Z")),
            ("duedate", st("2026-02-01")),
            ("customfield_1", st("2026-01-05")),
            ("customfield_2", st("2026-01-09")),
            ("description", adf_small()),
            ("worklog", obj(vec![("total", Json::Num(1.0)), ("worklogs", Json::Arr(vec![worklog.clone()]))])),
        ])),
        ("renderedFields", obj(vec![("description", st("<p>x</p>"))])),
    ]);
    let pull = obj(vec![("name", st("PR")), ("url", st("https://a.example/pr/1")), ("status", st("OPEN")), ("author", obj(vec![("name", st("김"))])), ("repositoryName", st("r")), ("source", obj(vec![("branch", st("b"))])), ("lastUpdate", st("2026-01-01T00:00:00Z"))]);
    let commit = obj(vec![("id", st("abcdef1234")), ("message", st("m\nbody")), ("url", st("https://a.example/c")), ("author", obj(vec![("name", st("김"))])), ("authorTimestamp", Json::Num(1_767_225_600_000.0))]);
    vec![
        ("issue", issue.clone()),
        ("page", obj(vec![("issues", Json::Arr(vec![issue])), ("nextPageToken", st("tok")), ("isLast", Json::Bool(false)), ("startAt", Json::Num(0.0)), ("total", Json::Num(30.0))])),
        ("comments", obj(vec![("total", Json::Num(9.0)), ("comments", Json::Arr(vec![obj(vec![("author", user("a")), ("created", st("2026-01-01T00:00:00.000Z")), ("body", adf_small()), ("renderedBody", st("<p>y</p>"))])]))])),
        ("transitions", obj(vec![("transitions", Json::Arr(vec![obj(vec![("id", st("11")), ("name", st("완료")), ("to", obj(vec![("name", st("Done")), ("statusCategory", obj(vec![("key", st("done"))]))]))])]))])),
        ("devpull", obj(vec![("pullRequests", Json::Arr(vec![pull]))])),
        ("devrepo", obj(vec![("repositories", Json::Arr(vec![obj(vec![("name", st("r")), ("commits", Json::Arr(vec![commit]))])]))])),
        ("devsummary", obj(vec![("summary", obj(vec![("pullrequest", obj(vec![("byInstanceType", obj(vec![("github", obj(vec![]))]))])), ("repository", obj(vec![("byInstanceType", obj(vec![("stash", obj(vec![]))]))]))]))])),
        ("fields", Json::Arr(vec![obj(vec![("id", st("customfield_1")), ("name", st("Start date")), ("schema", obj(vec![("type", st("date"))])), ("scope", obj(vec![]))])])),
        ("users", Json::Arr(vec![user("a"), user("b")])),
        ("worklog", worklog),
    ]
}

/// 견본 한 종류의 변형 하나를 그 종류가 쓰이는 매퍼에 먹이고, Cloud 본문 HTML(ADF 변환 결과)을 모아 돌려준다
fn jira_shaped_call(name: &str, v: &Json) -> Vec<String> {
    let dates = jira_map::DateFieldIds { start: vec!["customfield_1".into()], end: vec!["customfield_2".into()] };
    let mut htmls: Vec<String> = Vec::new();
    let mut body = |o: Option<&Json>, key: &str| {
        if let Some(t) = o.and_then(|j| j.get(key)).and_then(Json::as_str) {
            htmls.push(t.to_string());
        }
    };
    match name {
        "issue" => {
            let _ = (jira_map::map_summary(v), jira_map::worklog_issue(v), jira_map::dev_issue_id(v), jira_map::avatar_of(Some(v)), jira_map::comment_text(Some(v)));
            let out = jira_map::issue_detail(JiraKind::Cloud, SITE, "K-1", &dates, v);
            body(Some(&out), "descriptionHtml");
            let _ = jira_map::issue_detail(JiraKind::Server, SITE, "K-1", &dates, v);
        }
        "page" => {
            for kind in [JiraKind::Cloud, JiraKind::Server] {
                let _ = (jira_map::search_result(kind, v, Some("5"), 1), jira_map::search_result(kind, v, None, 8));
                let me = obj(vec![("id", st("a")), ("name", st("김"))]);
                let (issues, _) = jira_map::worklog_issues(&[v]);
                let users = vec!["a".to_string()];
                let q = jira_map::WorklogQuery { from: "2026-01-10", to: "2026-01-12", users: &users, include_me: true };
                let pairs: Vec<(&jira_map::WorklogIssue, Vec<&Json>)> = issues.iter().map(|i| (i, jira_map::dedupe_worklogs(&i.embedded))).collect();
                let _ = jira_map::worklog_result(kind, &q, &me, &pairs, false);
            }
        }
        "comments" => {
            for kind in [JiraKind::Cloud, JiraKind::Server] {
                let list = jira_map::comments(kind, v);
                if kind == JiraKind::Cloud {
                    for c in &list {
                        body(Some(c), "bodyHtml");
                    }
                }
            }
            let _ = jira_map::server_comments_start_at(v);
        }
        "transitions" => {
            let _ = jira_map::transitions(v);
        }
        "devpull" => {
            let _ = jira_map::dev_info(std::slice::from_ref(v), &[]);
        }
        "devrepo" => {
            let _ = jira_map::dev_info(&[], std::slice::from_ref(v));
        }
        "devsummary" => {
            let _ = (jira_map::dev_instances(v, "pullrequest"), jira_map::dev_instances(v, "repository"));
        }
        "fields" => {
            let _ = jira_map::pick_date_fields(v);
        }
        "users" => {
            for kind in [JiraKind::Cloud, JiraKind::Server] {
                let _ = (jira_map::search_users(kind, v), jira_map::my_id(kind, v));
            }
            let _ = jira_map::myself_name(v);
        }
        "worklog" => {
            let _ = (jira_map::comment_text(v.get("comment")), jira_map::dedupe_worklogs(jarr_or_empty(v)));
            htmls.push(adf::adf_to_html(v.get("comment").unwrap_or(&Json::Null)));
        }
        other => panic!("견본 이름: {other}"),
    }
    htmls
}

#[test]
fn jira_매퍼는_모양을_갖춘_응답의_모든_중첩_자리에_이상한_값이_들어와도_패닉하지_않고_안전하다() {
    let mut n = 0usize;
    let mut cloud_bodies = 0usize;
    for (name, t) in jira_templates() {
        // 기준 견본은 정상으로 처리돼야 한다(견본이 엉뚱한 모양이면 아래 시험이 허공을 친다)
        let base = jira_shaped_call(name, &t);
        for h in &base {
            assert_eq!(adf_unsafe(h), None, "{name} 기준 견본: {h}");
        }
        for w in weird_values(2_000) {
            for v in variants(&t, &w) {
                for html in jira_shaped_call(name, &v) {
                    if let Some(why) = adf_unsafe(&html) {
                        panic!("{name}: {why} (끼운 값 {:.80}) → {:.300}", stringify(&w), html);
                    }
                    cloud_bodies += 1;
                }
                n += 1;
            }
        }
    }
    assert!(n > 5_000, "시험한 변형이 적다: {n}");
    assert!(cloud_bodies > 500, "ADF 본문까지 닿은 변형이 적다: {cloud_bodies}");
}

/// 출력 상한(UTF-16 1,000,000 단위) 경계에서 잘라도 허용 태그 밖의 날것의 `<` 가 남지 않는다.
/// 상한 앞 `d` 단위 지점에서 뒤쪽 노드(표시·링크·엔티티가 될 글자·서로게이트 쌍·`<br>`)가 시작하게 맞춘다
#[test]
fn adf_출력_상한에서_잘려도_날것의_꺾쇠가_남지_않는다() {
    const MAX: usize = 1_000_000;
    let link = |t: &str| adf_text(t, vec![obj(vec![("type", st("link")), ("attrs", obj(vec![("href", st("https://a.example/long/path?q=1&r=2"))]))])]);
    let tails: Vec<(&str, Json)> = vec![
        ("굵게", adf_text("굵게", vec![obj(vec![("type", st("strong"))])])),
        ("겹친 표시", adf_text("겹침", ["strong", "em", "code", "strike", "underline"].iter().map(|t| obj(vec![("type", st(t))])).collect())),
        ("링크", link("링크")),
        ("꺾쇠 엔티티", adf_text("<<<<>>>>", vec![])),
        ("앰퍼샌드·따옴표", adf_text("&&\"\"''", vec![])),
        ("서로게이트", adf_text("😀😀😀", vec![])),
        ("hardBreak", adf_node("hardBreak", None, None)),
        ("한 글자", adf_text("x", vec![])),
    ];
    let (mut bad, mut cut, mut total) = (Vec::<String>::new(), 0usize, 0usize);
    let mut partial_entity = 0usize;
    for (label, tail) in &tails {
        for d in -6i64..=44 {
            let n = (MAX as i64 - 3 - d) as usize;
            let doc = adf_doc(vec![adf_node("paragraph", None, Some(vec![adf_text(&"a".repeat(n), vec![]), tail.clone(), tail.clone()]))]);
            let html = adf::adf_to_html(&doc);
            total += 1;
            if html.encode_utf16().count() == MAX {
                cut += 1;
            }
            if let Some(why) = adf_unsafe(&html) {
                bad.push(format!("{label} d={d}: {why}"));
            }
            // 참고: 엔티티가 중간에 잘린 경우(`&am` 처럼 끝나는 것) — 단언하지 않고 센다
            if let Some(i) = html.rfind('&') {
                if !html[i..].contains(';') {
                    partial_entity += 1;
                }
            }
        }
    }
    eprintln!("[잘림 경계] 시험 {total}, 상한에서 잘린 것 {cut}, 엔티티가 잘린 것 {partial_entity}, 날것의 `<` 등 위반 {}", bad.len());
    assert!(cut > 100, "상한까지 닿은 사례가 적다: {cut}");
    assert!(bad.is_empty(), "잘림 경계 위반 {}건, 예: {:#?}", bad.len(), &bad[..bad.len().min(10)]);
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

// ── 실제 소켓(루프백) 테스트: HTTP 코어와 두 클라이언트(PR 3) ─────────────────────────────────────
//
// 127.0.0.1 의 최소 HTTP/1.1 목 서버만 쓴다(외부 접속 없음). 모든 응답은 `Connection: close` 라 연결 수와 요청 수가 맞는다.
// 프록시 덫: 이 바이너리가 시작되기 전에(ctor) HTTP(S)_PROXY·ALL_PROXY 를 덫 리스너로 돌려 놓고, 소켓을 쓰는 테스트가 끝날 때마다 덫에 연결이 없는지 본다.
// `start_paused` 는 쓰지 않는다(시계가 멈춘 채 실제 소켓을 기다리면 제한 시간이 뜻대로 돌지 않는다).
mod live {
    use std::io::{Read, Write};
    use std::net::{SocketAddr, TcpListener, TcpStream};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use antler_lib::github::client::{GetOptions as GhGet, GitHubClient, TokenChoice, TokenProvider, GH_TOO_BIG, MAX_BODY_BYTES as GH_MAX_BODY};
    use antler_lib::jira::client::{GetOptions as JiraGet, JiraClient, JiraCredential, JIRA_TOO_BIG};
    use antler_lib::jira::JiraKind;
    use antler_lib::net::http::{HttpClient, NetError, Redirects};
    use antler_lib::net::js_value::stringify;
    use antler_lib::net::json;
    use antler_lib::net::redact::{git_basic_token, SecretString};
    use base64::engine::general_purpose::STANDARD;
    use base64::Engine;
    use tokio_util::sync::CancellationToken;

    // ── 프록시 덫 ───────────────────────────────────────────────────────────

    static TRAP: Mutex<Option<TcpListener>> = Mutex::new(None);
    static TRAP_SEEN: AtomicUsize = AtomicUsize::new(0);
    static TRAP_CONTROL: AtomicUsize = AtomicUsize::new(0);

    // unsafe: main 이전 코드다 — 소켓 하나를 열고 환경 변수만 넣는다(다른 스레드는 아직 없다)
    #[ctor::ctor(unsafe)]
    fn install_proxy_trap() {
        let l = TcpListener::bind("127.0.0.1:0").expect("프록시 덫");
        l.set_nonblocking(true).expect("프록시 덫");
        let url = format!("http://{}", l.local_addr().expect("프록시 덫"));
        for k in ["HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY", "http_proxy", "https_proxy", "all_proxy"] {
            std::env::set_var(k, &url);
        }
        // 루프백이 프록시 예외 목록에 들어 있으면 덫이 아무것도 못 잡는다
        for k in ["NO_PROXY", "no_proxy"] {
            std::env::remove_var(k);
        }
        *TRAP.lock().unwrap() = Some(l);
    }

    /// 덫에 들어온 연결 수에서 덫이 살아 있는지 확인하려고 일부러 건 연결 수를 뺀 값
    fn trap_unexpected() -> usize {
        let g = TRAP.lock().unwrap();
        let l = g.as_ref().expect("덫이 없다(ctor 가 돌지 않았다)");
        while l.accept().is_ok() {
            TRAP_SEEN.fetch_add(1, Ordering::SeqCst);
        }
        TRAP_SEEN.load(Ordering::SeqCst) - TRAP_CONTROL.load(Ordering::SeqCst)
    }

    /// 소켓을 쓰는 테스트의 첫 줄에 둔다 — 테스트가 끝날 때(목 서버보다 늦게 정리된다) 덫에 연결이 없어야 한다
    struct Guard;

    impl Drop for Guard {
        fn drop(&mut self) {
            if !std::thread::panicking() {
                assert_eq!(trap_unexpected(), 0, "프록시 덫에 연결이 왔다 — 클라이언트가 프록시 환경 변수를 따랐다");
            }
        }
    }

    // ── 목 서버 ─────────────────────────────────────────────────────────────

    #[derive(Clone, Debug)]
    pub struct Req {
        pub method: String,
        pub target: String,
        pub headers: Vec<(String, String)>,
        pub body: Vec<u8>,
    }

    impl Req {
        pub fn header(&self, name: &str) -> Option<&str> {
            self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
        }

        pub fn text(&self) -> String {
            String::from_utf8_lossy(&self.body).into_owned()
        }
    }

    enum Mode {
        Normal,
        /// `Transfer-Encoding: chunked` 로 끝없이 보낸다(클라이언트가 끊을 때까지)
        Chunked,
        /// `Content-Length` 를 `n` 으로 적고 본문은 가진 만큼만 보낸 뒤 끊는다
        Claim(usize),
        /// 헤더와 본문 앞부분을 보낸 뒤 이 시간 동안 멈춘다
        Stall(Duration),
    }

    pub struct Resp {
        status: u16,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
        delay: Duration,
        mode: Mode,
    }

    impl Resp {
        pub fn new(status: u16) -> Self {
            Self { status, headers: Vec::new(), body: Vec::new(), delay: Duration::ZERO, mode: Mode::Normal }
        }

        pub fn header(mut self, k: &str, v: &str) -> Self {
            self.headers.push((k.to_string(), v.to_string()));
            self
        }

        pub fn body(mut self, b: impl AsRef<[u8]>) -> Self {
            self.body = b.as_ref().to_vec();
            self
        }

        pub fn json(self, text: &str) -> Self {
            self.header("Content-Type", "application/json").body(text)
        }

        pub fn delay(mut self, d: Duration) -> Self {
            self.delay = d;
            self
        }

        pub fn chunked_forever(mut self) -> Self {
            self.mode = Mode::Chunked;
            self
        }

        pub fn claim_len(mut self, n: usize) -> Self {
            self.mode = Mode::Claim(n);
            self
        }

        pub fn stall(mut self, d: Duration) -> Self {
            self.mode = Mode::Stall(d);
            self
        }
    }

    fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
        hay.windows(needle.len()).position(|w| w == needle)
    }

    fn read_req(s: &mut TcpStream) -> Option<Req> {
        s.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
        let mut buf = Vec::new();
        let mut tmp = [0u8; 4096];
        let head_end = loop {
            if let Some(p) = find(&buf, b"\r\n\r\n") {
                break p + 4;
            }
            let n = s.read(&mut tmp).ok()?;
            if n == 0 {
                return None;
            }
            buf.extend_from_slice(&tmp[..n]);
        };
        let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
        let mut lines = head.split("\r\n");
        let mut first = lines.next()?.split(' ');
        let (method, target) = (first.next()?.to_string(), first.next()?.to_string());
        let headers: Vec<(String, String)> = lines.filter_map(|l| l.split_once(':')).map(|(k, v)| (k.trim().to_string(), v.trim().to_string())).collect();
        let want: usize = headers.iter().find(|(k, _)| k.eq_ignore_ascii_case("content-length")).and_then(|(_, v)| v.parse().ok()).unwrap_or(0);
        while buf.len() < head_end + want {
            let n = s.read(&mut tmp).ok()?;
            if n == 0 {
                return None;
            }
            buf.extend_from_slice(&tmp[..n]);
        }
        Some(Req { method, target, headers, body: buf[head_end..head_end + want].to_vec() })
    }

    fn write_resp(s: &mut TcpStream, r: &Resp) {
        std::thread::sleep(r.delay);
        let mut head = format!("HTTP/1.1 {} X\r\n", r.status);
        for (k, v) in &r.headers {
            head.push_str(&format!("{k}: {v}\r\n"));
        }
        head.push_str("Connection: close\r\n");
        match r.mode {
            Mode::Normal => {
                head.push_str(&format!("Content-Length: {}\r\n\r\n", r.body.len()));
                let _ = s.write_all(head.as_bytes()).and_then(|()| s.write_all(&r.body));
            }
            Mode::Claim(n) => {
                head.push_str(&format!("Content-Length: {n}\r\n\r\n"));
                let _ = s.write_all(head.as_bytes()).and_then(|()| s.write_all(&r.body));
            }
            Mode::Stall(d) => {
                head.push_str(&format!("Content-Length: {}\r\n\r\n", r.body.len() + 1024));
                let _ = s.write_all(head.as_bytes()).and_then(|()| s.write_all(&r.body)).and_then(|()| s.flush());
                std::thread::sleep(d);
            }
            Mode::Chunked => {
                head.push_str("Transfer-Encoding: chunked\r\n\r\n");
                if s.write_all(head.as_bytes()).is_err() {
                    return;
                }
                let chunk = [b'x'; 64 * 1024];
                // 안전장치: 클라이언트가 끊지 않아도 256MB 에서 멈춘다
                for _ in 0..4096 {
                    if s.write_all(b"10000\r\n").and_then(|()| s.write_all(&chunk)).and_then(|()| s.write_all(b"\r\n")).is_err() {
                        return;
                    }
                }
            }
        }
    }

    pub struct Mock {
        pub addr: SocketAddr,
        reqs: Arc<Mutex<Vec<Req>>>,
        conns: Arc<AtomicUsize>,
        stop: Arc<AtomicBool>,
    }

    impl Mock {
        /// `handler(n, 요청)` 의 `n` 은 0 부터 세는 요청 순번이다
        pub fn start(handler: impl Fn(usize, &Req) -> Resp + Send + Sync + 'static) -> Mock {
            let listener = TcpListener::bind("127.0.0.1:0").expect("목 서버");
            let addr = listener.local_addr().expect("목 서버");
            let (reqs, conns, stop) = (Arc::new(Mutex::new(Vec::new())), Arc::new(AtomicUsize::new(0)), Arc::new(AtomicBool::new(false)));
            let handler = Arc::new(handler);
            let (r2, c2, s2) = (reqs.clone(), conns.clone(), stop.clone());
            std::thread::spawn(move || {
                for stream in listener.incoming() {
                    if s2.load(Ordering::SeqCst) {
                        return;
                    }
                    let Ok(mut stream) = stream else { continue };
                    c2.fetch_add(1, Ordering::SeqCst);
                    let (reqs, handler) = (r2.clone(), handler.clone());
                    std::thread::spawn(move || {
                        let Some(req) = read_req(&mut stream) else { return };
                        let n = {
                            let mut g = reqs.lock().unwrap();
                            g.push(req.clone());
                            g.len() - 1
                        };
                        let resp = handler(n, &req);
                        write_resp(&mut stream, &resp);
                    });
                }
            });
            Mock { addr, reqs, conns, stop }
        }

        pub fn base(&self) -> String {
            format!("http://{}", self.addr)
        }

        pub fn requests(&self) -> Vec<Req> {
            self.reqs.lock().unwrap().clone()
        }

        pub fn connections(&self) -> usize {
            self.conns.load(Ordering::SeqCst)
        }
    }

    impl Drop for Mock {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::SeqCst);
            let _ = TcpStream::connect(self.addr);
        }
    }

    /// 받은 바이트를 기록하고 평문 HTTP 로 답한 뒤 끊는 리스너(TLS 가 아닌 서버)
    struct Raw {
        addr: SocketAddr,
        received: Arc<Mutex<Vec<u8>>>,
    }

    impl Raw {
        fn start() -> Raw {
            let l = TcpListener::bind("127.0.0.1:0").expect("raw");
            let addr = l.local_addr().expect("raw");
            let received = Arc::new(Mutex::new(Vec::new()));
            let r2 = received.clone();
            std::thread::spawn(move || {
                for s in l.incoming() {
                    let Ok(mut s) = s else { continue };
                    let _ = s.set_read_timeout(Some(Duration::from_millis(800)));
                    let mut buf = [0u8; 8192];
                    if let Ok(n) = s.read(&mut buf) {
                        r2.lock().unwrap().extend_from_slice(&buf[..n]);
                    }
                    let _ = s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}");
                }
            });
            Raw { addr, received }
        }
    }

    /// 자체 서명 인증서(tests/fixtures/selfsigned-loopback.pfx — 테스트 전용 키, tests/fixtures/README.md)로 TLS 를 하는 루프백 서버.
    /// 핸드셰이크를 끝낸 연결이 보낸 바이트를 기록하고 200 으로 답한다(클라이언트가 인증서를 거부하면 읽을 바이트가 없다)
    struct TlsMock {
        addr: SocketAddr,
        received: Arc<Mutex<Vec<u8>>>,
    }

    impl TlsMock {
        fn start() -> TlsMock {
            let identity = native_tls::Identity::from_pkcs12(include_bytes!("fixtures/selfsigned-loopback.pfx"), "antler-test").expect("테스트 PFX");
            let acceptor = Arc::new(native_tls::TlsAcceptor::new(identity).expect("TLS 수락기"));
            let l = TcpListener::bind("127.0.0.1:0").expect("tls 목");
            let addr = l.local_addr().expect("tls 목");
            let received = Arc::new(Mutex::new(Vec::new()));
            let r2 = received.clone();
            std::thread::spawn(move || {
                for s in l.incoming() {
                    let Ok(s) = s else { continue };
                    let (acceptor, r3) = (acceptor.clone(), r2.clone());
                    std::thread::spawn(move || {
                        let _ = s.set_read_timeout(Some(Duration::from_secs(2)));
                        let Ok(mut tls) = acceptor.accept(s) else { return };
                        let mut buf = [0u8; 8192];
                        if let Ok(n) = tls.read(&mut buf) {
                            r3.lock().unwrap().extend_from_slice(&buf[..n]);
                        }
                        let _ = tls.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}");
                        let _ = tls.shutdown();
                    });
                }
            });
            TlsMock { addr, received }
        }
    }

    /// 아무도 듣지 않는 루프백 주소
    fn closed_addr() -> SocketAddr {
        let l = TcpListener::bind("127.0.0.1:0").expect("빈 포트");
        l.local_addr().expect("빈 포트")
    }

    // ── 클라이언트·값 도우미 ────────────────────────────────────────────────

    const JTOKEN: &str = "jtok_SECRET_0123456789";
    const GTOKEN: &str = "ghp_SECRET_abcdef0123456789";
    const EMAIL: &str = "me@example.com";

    fn b64(s: &str) -> String {
        STANDARD.encode(s)
    }

    fn jira(timeout_ms: u64) -> JiraClient {
        JiraClient::new_for_test(Duration::from_millis(timeout_ms), None, Duration::from_millis(1)).unwrap()
    }

    /// 공개 사진 호스트(https)로 가는 요청을 목 서버의 `/pub/<호스트>/…` 로 돌린다 — 판정은 원래 주소로 하고 연결만 루프백으로 간다
    fn jira_with_public_hosts(base: &str) -> JiraClient {
        let base = base.to_string();
        let rewrite: antler_lib::net::http::Rewrite = Arc::new(move |url: &str| {
            for host in ["secure.gravatar.com", "i0.wp.com", "i1.wp.com", "example.invalid"] {
                for scheme in ["https", "http"] {
                    if let Some(rest) = url.strip_prefix(&format!("{scheme}://{host}")) {
                        return format!("{base}/pub/{host}{rest}");
                    }
                }
            }
            url.to_string()
        });
        JiraClient::new_for_test(Duration::from_secs(5), Some(rewrite), Duration::from_millis(1)).unwrap()
    }

    fn cred(site: &str) -> JiraCredential {
        JiraCredential { kind: JiraKind::Cloud, site_url: site.to_string(), email: EMAIL.to_string(), token: SecretString::new(JTOKEN) }
    }

    fn gh(base: &str, token: Option<&str>, timeout_ms: u64, anon_fresh_ms: u64) -> GitHubClient {
        let t = token.map(SecretString::new);
        let provider: TokenProvider = Arc::new(move || t.clone());
        GitHubClient::new_for_test(provider, base, Duration::from_millis(timeout_ms), Duration::from_millis(anon_fresh_ms)).unwrap().with_format_time(|e| format!("T{e}"))
    }

    fn api(e: &NetError) -> (&'static str, Option<u16>, &str) {
        match e {
            NetError::Api { code, status, message } => (code, *status, message.as_str()),
            other => panic!("Api 오류가 아니다: {other:?}"),
        }
    }

    fn err<T: std::fmt::Debug>(r: Result<T, NetError>) -> NetError {
        r.expect_err("오류여야 한다")
    }

    const PNG: [u8; 8] = [137, 80, 78, 71, 13, 10, 26, 10];

    // ── Jira ────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn jira_요청에_정확한_헤더가_실리고_컨텍스트_경로가_유지된다() {
        let _g = Guard;
        let mock = Mock::start(|_, _| Resp::new(200).json(r#"{"ok":true}"#));
        let c = cred(&format!("{}/jira", mock.base()));
        let out = jira(5000).get(&c, "/rest/api/2/myself", JiraGet { query: &[("a", Some("x y")), ("skip", None)], ..Default::default() }).await.unwrap();
        assert_eq!(stringify(&out), r#"{"ok":true}"#);
        let r = &mock.requests()[0];
        assert_eq!((r.method.as_str(), r.target.as_str()), ("GET", "/jira/rest/api/2/myself?a=x+y"));
        let want = format!("Basic {}", b64(&format!("{EMAIL}:{JTOKEN}")));
        assert_eq!(r.header("authorization"), Some(want.as_str()));
        assert_eq!(r.header("accept"), Some("application/json"));
        assert_eq!(r.header("user-agent"), Some("Antler"));
        // GET 에는 쓰기용 헤더를 싣지 않는다
        assert_eq!((r.header("content-type"), r.header("x-atlassian-token")), (None, None));

        // Data Center 는 PAT Bearer
        let server = JiraCredential { kind: JiraKind::Server, ..c };
        jira(5000).get(&server, "/rest/api/2/myself", JiraGet::default()).await.unwrap();
        let want = format!("Bearer {JTOKEN}");
        assert_eq!(mock.requests()[1].header("authorization"), Some(want.as_str()));
    }

    #[tokio::test]
    async fn jira_리다이렉트는_따라가지_않고_대상에는_연결하지_않는다() {
        let _g = Guard;
        let target = Mock::start(|_, _| Resp::new(200).json("{}"));
        let to = format!("{}/stolen", target.base());
        let site = Mock::start(move |_, req| if req.target.ends_with("/same") { Resp::new(301).header("Location", "/jira/login") } else { Resp::new(302).header("Location", &to) });
        let c = cred(&site.base());
        // 다른 호스트(포트)
        let e = err(jira(5000).get(&c, "/rest/api/2/other", JiraGet::default()).await);
        assert_eq!((api(&e).0, api(&e).1), ("JIRA_REDIRECT", Some(302)));
        assert_eq!(target.connections(), 0);
        // 같은 호스트 안의 리다이렉트도
        let e = err(jira(5000).get(&c, "/rest/api/2/same", JiraGet::default()).await);
        assert_eq!(api(&e).0, "JIRA_REDIRECT");
        assert_eq!(site.requests().len(), 2, "리다이렉트를 따라가 요청이 늘면 안 된다");
        // POST 도 같다
        let e = err(jira(5000).post(&c, "/rest/api/2/other", &json::parse("{}").unwrap(), JiraGet::default()).await);
        assert_eq!(api(&e).0, "JIRA_REDIRECT");
        assert_eq!(target.connections(), 0);
    }

    #[tokio::test]
    async fn jira_http_오류는_분류되고_메시지에_토큰이_없다() {
        let _g = Guard;
        let mock = Mock::start(|_, req| {
            let status: u16 = req.target.rsplit('/').next().unwrap().parse().unwrap();
            Resp::new(status).header("Retry-After", "120").json(r#"{"errorMessages":["boom"]}"#)
        });
        let c = cred(&mock.base());
        for (status, code) in [(401u16, "JIRA_AUTH"), (403, "JIRA_FORBIDDEN"), (404, "JIRA_NOT_FOUND"), (429, "JIRA_RATE_LIMIT"), (500, "JIRA_HTTP")] {
            let e = err(jira(5000).get(&c, &format!("/rest/api/2/{status}"), JiraGet::default()).await);
            let (got, st, msg) = api(&e);
            assert_eq!((got, st), (code, Some(status)), "{status}");
            for secret in [JTOKEN.to_string(), b64(&format!("{EMAIL}:{JTOKEN}"))] {
                assert!(!msg.contains(&secret) && !format!("{e:?}").contains(&secret), "{status}: 비밀이 문구에 있다");
            }
        }
        assert_eq!(mock.requests().len(), 5, "Retry-After 가 길면 429 도 다시 보내지 않는다");
    }

    #[tokio::test]
    async fn jira_오류_문구는_종류와_호출자가_정한다() {
        let _g = Guard;
        let mock = Mock::start(|_, req| Resp::new(if req.target.contains("nf") { 404 } else { 401 }).json("{}"));
        let cloud = cred(&mock.base());
        let e = err(jira(5000).get(&cloud, "/x", JiraGet::default()).await);
        assert!(api(&e).2.contains("이메일·API 토큰"));
        let server = JiraCredential { kind: JiraKind::Server, ..cloud.clone() };
        let e = err(jira(5000).get(&server, "/x", JiraGet::default()).await);
        assert!(api(&e).2.contains("Personal Access Token"));
        let e = err(jira(5000).get(&cloud, "/nf", JiraGet { not_found: Some("이슈가 없습니다"), ..Default::default() }).await);
        assert_eq!((api(&e).0, api(&e).2), ("JIRA_NOT_FOUND", "이슈가 없습니다"));
        let e = err(jira(5000).get(&cloud, "/nf", JiraGet::default()).await);
        assert_eq!(api(&e).2, "요청한 항목을 찾을 수 없거나 볼 권한이 없습니다");
    }

    #[tokio::test]
    async fn jira_연결_실패_메시지에도_토큰이_없다() {
        let _g = Guard;
        let c = cred(&format!("http://{}", closed_addr()));
        let e = err(jira(10_000).get(&c, "/rest/api/2/myself", JiraGet::default()).await);
        assert_eq!((api(&e).0, api(&e).1, api(&e).2), ("JIRA_NETWORK", None, "Jira 에 연결할 수 없습니다"));
        assert!(!format!("{e:?}").contains(JTOKEN));
    }

    #[tokio::test]
    async fn jira_본문이_json_이_아니면_해석_오류다() {
        let _g = Guard;
        let mock = Mock::start(|_, _| Resp::new(200).body("<html>"));
        let e = err(jira(5000).get(&cred(&mock.base()), "/x", JiraGet::default()).await);
        assert_eq!((api(&e).0, api(&e).1, api(&e).2), ("JIRA_HTTP", Some(200), "Jira 응답을 해석할 수 없습니다"));
    }

    #[tokio::test]
    async fn jira_취소하면_aborted_다() {
        let _g = Guard;
        // 이미 취소된 신호면 요청을 보내지 않는다
        let mock = Mock::start(|_, _| Resp::new(200).json("{}").delay(Duration::from_secs(3)));
        let c = cred(&mock.base());
        let done = CancellationToken::new();
        done.cancel();
        assert_eq!(err(jira(5000).get(&c, "/x", JiraGet { cancel: Some(&done), ..Default::default() }).await), NetError::Aborted);
        assert_eq!(mock.connections(), 0);
        // 요청 중 취소
        let cancel = CancellationToken::new();
        let c2 = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(150)).await;
            c2.cancel();
        });
        let t = Instant::now();
        assert_eq!(err(jira(10_000).get(&c, "/x", JiraGet { cancel: Some(&cancel), ..Default::default() }).await), NetError::Aborted);
        assert!(t.elapsed() < Duration::from_secs(2));
    }

    #[tokio::test]
    async fn jira_경로는_사이트_밖을_가리킬_수_없다() {
        let _g = Guard;
        let mock = Mock::start(|_, _| Resp::new(200).json("{}"));
        let c = cred(&format!("{}/jira", mock.base()));
        for path in ["//evil.example/x", "rest/api/2/myself", ""] {
            assert_eq!(err(jira(5000).get(&c, path, JiraGet::default()).await), NetError::InvalidPath, "{path:?}");
            assert_eq!(err(jira(5000).post(&c, path, &json::parse("{}").unwrap(), JiraGet::default()).await), NetError::InvalidPath, "{path:?}");
        }
        assert_eq!(mock.connections(), 0);
    }

    #[tokio::test]
    async fn jira_post_는_쓰기용_헤더를_싣고_빈_본문은_none_이다() {
        let _g = Guard;
        let mock = Mock::start(|_, req| if req.target.ends_with("/transitions") { Resp::new(204) } else { Resp::new(200).json(r#"{"id":"1"}"#) });
        let c = cred(&format!("{}/jira", mock.base()));
        let body = json::parse(r#"{"transition":{"id":"31"}}"#).unwrap();
        let out = jira(5000).post(&c, "/rest/api/2/issue/A-1/transitions", &body, JiraGet::default()).await.unwrap();
        assert_eq!(out, None);
        let r = &mock.requests()[0];
        assert_eq!((r.method.as_str(), r.target.as_str()), ("POST", "/jira/rest/api/2/issue/A-1/transitions"));
        assert_eq!(r.header("content-type"), Some("application/json"));
        assert_eq!(r.header("x-atlassian-token"), Some("no-check"));
        assert_eq!(r.text(), r#"{"transition":{"id":"31"}}"#);
        let out = jira(5000).post(&c, "/rest/api/2/issue", &body, JiraGet::default()).await.unwrap();
        assert_eq!(out.as_ref().map(stringify).as_deref(), Some(r#"{"id":"1"}"#));
    }

    #[tokio::test]
    async fn jira_post_는_429_를_다시_보내지_않는다() {
        let _g = Guard;
        let mock = Mock::start(|_, _| Resp::new(429).header("Retry-After", "0").json("{}"));
        let c = cred(&mock.base());
        let e = err(jira(5000).post(&c, "/rest/api/2/issue/A-1/transitions", &json::parse("{}").unwrap(), JiraGet::default()).await);
        assert_eq!(api(&e).0, "JIRA_RATE_LIMIT");
        assert_eq!(mock.requests().len(), 1);
    }

    #[tokio::test]
    async fn jira_429_는_짧으면_한_번_다시_시도하고_길면_기다리지_않는다() {
        let _g = Guard;
        // 짧으면 다시 시도
        let mock = Mock::start(|n, _| if n == 0 { Resp::new(429).header("Retry-After", "0").json("{}") } else { Resp::new(200).json(r#"{"ok":1}"#) });
        let out = jira(5000).get(&cred(&mock.base()), "/x", JiraGet::default()).await.unwrap();
        assert_eq!(stringify(&out), r#"{"ok":1}"#);
        assert_eq!(mock.requests().len(), 2);
        // 길면 기다리지 않고 남은 시간을 알린다
        let mock = Mock::start(|_, _| Resp::new(429).header("Retry-After", "120").json("{}"));
        let e = err(jira(5000).get(&cred(&mock.base()), "/x", JiraGet::default()).await);
        assert_eq!((api(&e).0, api(&e).2), ("JIRA_RATE_LIMIT", "Jira 요청이 너무 잦습니다. 120초 후에 다시 시도하세요"));
        assert_eq!(mock.requests().len(), 1);
        // 다시 시도해도 429 면 거기서 멈춘다
        let mock = Mock::start(|_, _| Resp::new(429).header("Retry-After", "0").json("{}"));
        let e = err(jira(5000).get(&cred(&mock.base()), "/x", JiraGet::default()).await);
        assert_eq!(api(&e).0, "JIRA_RATE_LIMIT");
        assert_eq!(mock.requests().len(), 2);
        // 재시도 대기 중에도 취소가 먹는다(운영 값 1초 단위가 아니라 긴 대기로 시험)
        let mock = Mock::start(|_, _| Resp::new(429).header("Retry-After", "2").json("{}"));
        let slow = JiraClient::new_for_test(Duration::from_secs(5), None, Duration::from_secs(30)).unwrap();
        let cancel = CancellationToken::new();
        let c2 = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(300)).await;
            c2.cancel();
        });
        let t = Instant::now();
        let e = err(slow.get(&cred(&mock.base()), "/x", JiraGet { cancel: Some(&cancel), ..Default::default() }).await);
        assert_eq!(e, NetError::Aborted);
        assert!(t.elapsed() < Duration::from_secs(5));
    }

    #[tokio::test]
    async fn jira_본문_상한은_chunked_와_content_length_와_엔드포인트별_값을_지킨다() {
        let _g = Guard;
        let c = |m: &Mock| cred(&m.base());
        // chunked 로 끝없이 보내도 상한(5MB)에서 읽기를 중단한다
        let mock = Mock::start(|_, _| Resp::new(200).header("Content-Type", "application/json").chunked_forever());
        let t = Instant::now();
        let e = err(jira(10_000).get(&c(&mock), "/x", JiraGet::default()).await);
        assert_eq!((api(&e).0, api(&e).1, api(&e).2), ("JIRA_HTTP", Some(200), JIRA_TOO_BIG));
        assert!(t.elapsed() < Duration::from_secs(8));
        // Content-Length 가 상한을 넘으면 본문을 읽지 않는다(본문은 오지도 않는다)
        let mock = Mock::start(|_, _| Resp::new(200).claim_len(6 * 1024 * 1024));
        let e = err(jira(10_000).get(&c(&mock), "/x", JiraGet::default()).await);
        assert_eq!(api(&e).2, JIRA_TOO_BIG);
        // 엔드포인트별 상한
        let mock = Mock::start(|_, _| Resp::new(200).json(r#"{"a":"0123456789012345"}"#));
        let e = err(jira(5000).get(&c(&mock), "/x", JiraGet { max_bytes: Some(10), ..Default::default() }).await);
        assert_eq!(api(&e).2, JIRA_TOO_BIG);
        assert!(jira(5000).get(&c(&mock), "/x", JiraGet { max_bytes: Some(100), ..Default::default() }).await.is_ok());
    }

    #[tokio::test]
    async fn jira_429_재시도_경로에서도_거대한_본문은_상한을_지킨다() {
        let _g = Guard;
        let mock = Mock::start(|n, _| if n == 0 { Resp::new(429).header("Retry-After", "0").chunked_forever() } else { Resp::new(200).json(r#"{"ok":1}"#) });
        let t = Instant::now();
        let out = jira(10_000).get(&cred(&mock.base()), "/x", JiraGet::default()).await.unwrap();
        assert_eq!(stringify(&out), r#"{"ok":1}"#);
        assert_eq!(mock.requests().len(), 2);
        assert!(t.elapsed() < Duration::from_secs(8));
    }

    #[tokio::test]
    async fn jira_본문을_받는_중_시간이_초과되면_timeout_이다() {
        let _g = Guard;
        let mock = Mock::start(|_, _| Resp::new(200).header("Content-Type", "application/json").body("{\"a\":").stall(Duration::from_secs(5)));
        let t = Instant::now();
        let e = err(jira(500).get(&cred(&mock.base()), "/x", JiraGet::default()).await);
        assert_eq!((api(&e).0, api(&e).2), ("JIRA_TIMEOUT", "Jira 응답이 15초 안에 오지 않았습니다"));
        assert!(t.elapsed() < Duration::from_secs(4));
        // 헤더가 오기 전의 지연도 같다
        let mock = Mock::start(|_, _| Resp::new(200).json("{}").delay(Duration::from_secs(3)));
        let e = err(jira(400).get(&cred(&mock.base()), "/x", JiraGet::default()).await);
        assert_eq!(api(&e).0, "JIRA_TIMEOUT");
    }

    // ── Jira 프로필 사진 ────────────────────────────────────────────────────

    #[tokio::test]
    async fn jira_사이트_안_사진은_인증을_붙여_받는다() {
        let _g = Guard;
        let mock = Mock::start(|_, _| Resp::new(200).header("Content-Type", "image/png; charset=x").body(PNG));
        let c = cred(&format!("{}/jira", mock.base()));
        let img = jira(5000).image(&c, &format!("{}/jira/secure/useravatar?ownerId=1", mock.base()), None).await.unwrap().expect("사진");
        assert_eq!((img.content_type.as_str(), img.bytes.as_slice()), ("image/png", PNG.as_slice()));
        let r = &mock.requests()[0];
        assert_eq!(r.target, "/jira/secure/useravatar?ownerId=1");
        let want = format!("Basic {}", b64(&format!("{EMAIL}:{JTOKEN}")));
        assert_eq!(r.header("authorization"), Some(want.as_str()));
        assert_eq!(r.header("accept"), Some("image/*"));
    }

    #[tokio::test]
    async fn jira_사이트_밖_사진에는_요청하지_않는다() {
        let _g = Guard;
        let other = Mock::start(|_, _| Resp::new(200).header("Content-Type", "image/png").body(PNG));
        let mock = Mock::start(|_, _| Resp::new(200).header("Content-Type", "image/png").body(PNG));
        let c = cred(&format!("{}/jira", mock.base()));
        let hrefs = [
            format!("{}/other/a.png", mock.base()),
            format!("{}/jira/../a.png", other.base()),
            format!("{}/jira/a.png", other.base()),
            "http://secure.gravatar.com/a.png".to_string(),
            "https://user:pw@secure.gravatar.com/a.png".to_string(),
            "https://secure.gravatar.com:8443/a.png".to_string(),
            format!("https://{}/a.png", closed_addr()),
            format!("{}/jira/%2e%2e/a.png", mock.base()),
            "not a url".to_string(),
        ];
        let client = jira_with_public_hosts(&mock.base());
        for href in &hrefs {
            assert_eq!(client.image(&c, href, None).await.unwrap(), None, "{href}");
        }
        assert!(mock.requests().is_empty(), "{:?}", mock.requests());
        assert_eq!(other.connections(), 0);
    }

    #[tokio::test]
    async fn jira_사진은_이미지가_아니거나_너무_크거나_리다이렉트면_null_이다() {
        let _g = Guard;
        let target = Mock::start(|_, _| Resp::new(200).header("Content-Type", "image/png").body(PNG));
        let to = format!("{}/stolen.png", target.base());
        let mock = Mock::start(move |_, req| match req.target.rsplit('/').next().unwrap() {
            "svg" => Resp::new(200).header("Content-Type", "image/svg+xml").body("<svg/>"),
            "big" => Resp::new(200).header("Content-Type", "image/png").claim_len(300 * 1024),
            "chunked" => Resp::new(200).header("Content-Type", "image/png").chunked_forever(),
            "empty" => Resp::new(200).header("Content-Type", "image/png"),
            "missing" => Resp::new(404).header("Content-Type", "image/png").body(PNG),
            "redirect" => Resp::new(302).header("Location", &to),
            _ => Resp::new(200).header("Content-Type", "image/gif").body("GIF89a"),
        });
        let c = cred(&mock.base());
        let client = jira(5000);
        for name in ["svg", "big", "chunked", "empty", "missing", "redirect"] {
            assert_eq!(client.image(&c, &format!("{}/avatar/{name}", mock.base()), None).await.unwrap(), None, "{name}");
        }
        assert_eq!(target.connections(), 0, "사이트 안의 리다이렉트는 따라가지 않는다");
        assert!(client.image(&c, &format!("{}/avatar/gif", mock.base()), None).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn jira_공개_사진_호스트는_인증_없이_받고_그_호스트들_사이의_리다이렉트만_따라간다() {
        let _g = Guard;
        let site = Mock::start(|_, req| match req.target.as_str() {
            "/pub/secure.gravatar.com/avatar/a" => Resp::new(302).header("Location", "https://i0.wp.com/avatar/b"),
            "/pub/i0.wp.com/avatar/b" => Resp::new(200).header("Content-Type", "image/webp").body("RIFFxxxxWEBP"),
            "/pub/secure.gravatar.com/to-bad" => Resp::new(302).header("Location", "https://example.invalid/x"),
            "/pub/secure.gravatar.com/to-http" => Resp::new(302).header("Location", "http://i1.wp.com/x"),
            "/pub/secure.gravatar.com/loop" => Resp::new(302).header("Location", "https://secure.gravatar.com/loop"),
            _ => Resp::new(404),
        });
        let c = cred(&format!("{}/jira", site.base()));
        let client = jira_with_public_hosts(&site.base());
        let img = client.image(&c, "https://secure.gravatar.com/avatar/a", None).await.unwrap().expect("사진");
        assert_eq!((img.content_type.as_str(), img.bytes.len()), ("image/webp", 12));
        let reqs = site.requests();
        assert_eq!(reqs.len(), 2);
        assert!(reqs.iter().all(|r| r.header("authorization").is_none()), "공개 호스트에는 인증을 붙이지 않는다");
        // 공개 호스트가 아닌 곳·http 로의 리다이렉트는 따라가지 않는다
        for name in ["to-bad", "to-http"] {
            let before = site.requests().len();
            assert_eq!(client.image(&c, &format!("https://secure.gravatar.com/{name}"), None).await.unwrap(), None, "{name}");
            assert_eq!(site.requests().len() - before, 1, "{name}");
        }
        // 리다이렉트는 3번까지(첫 요청 포함 4번 요청)
        let before = site.requests().len();
        assert_eq!(client.image(&c, "https://secure.gravatar.com/loop", None).await.unwrap(), None);
        assert_eq!(site.requests().len() - before, 4);
    }

    #[tokio::test]
    async fn jira_공개_호스트가_사이트로_리다이렉트해도_인증을_붙이지_않는다() {
        let _g = Guard;
        let site = Mock::start(|_, req| {
            if req.target.starts_with("/pub/") {
                Resp::new(302).header("Location", &format!("http://{}/jira/secure/useravatar", req.header("host").unwrap()))
            } else {
                Resp::new(200).header("Content-Type", "image/png").body(PNG)
            }
        });
        let c = cred(&format!("{}/jira", site.base()));
        let client = jira_with_public_hosts(&site.base());
        assert_eq!(client.image(&c, "https://secure.gravatar.com/avatar/a", None).await.unwrap(), None);
        let reqs = site.requests();
        assert_eq!(reqs.len(), 1, "사이트로 돌아오는 리다이렉트는 따라가지 않는다");
        assert!(reqs[0].header("authorization").is_none());
    }

    #[tokio::test]
    async fn jira_사진_요청도_취소할_수_있다() {
        let _g = Guard;
        let mock = Mock::start(|_, _| Resp::new(200).header("Content-Type", "image/png").body(PNG).delay(Duration::from_secs(3)));
        let cancel = CancellationToken::new();
        let c2 = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(150)).await;
            c2.cancel();
        });
        let e = err(jira(10_000).image(&cred(&mock.base()), &format!("{}/a.png", mock.base()), Some(&cancel)).await);
        assert_eq!(e, NetError::Aborted);
    }

    // ── GitHub ──────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn gh_고정_헤더를_보내고_토큰이_있으면_bearer_를_붙인다() {
        let _g = Guard;
        let mock = Mock::start(|_, _| Resp::new(200).json(r#"{"a":1}"#));
        let client = gh(&mock.base(), Some(GTOKEN), 5000, 0);
        let out = client.get("/repos/o/r", &TokenChoice::Stored, GhGet { query: &[("per_page", Some("5")), ("skip", None)], ..Default::default() }).await.unwrap();
        assert_eq!((stringify(&out.data).as_str(), out.has_more), (r#"{"a":1}"#, false));
        let r = &mock.requests()[0];
        assert_eq!((r.method.as_str(), r.target.as_str()), ("GET", "/repos/o/r?per_page=5"));
        assert_eq!(r.header("accept"), Some("application/vnd.github+json"));
        assert_eq!(r.header("x-github-api-version"), Some("2022-11-28"));
        assert_eq!(r.header("user-agent"), Some("Antler"));
        let want = format!("Bearer {GTOKEN}");
        assert_eq!(r.header("authorization"), Some(want.as_str()));
        // 익명이면 Authorization 이 없다
        client.get("/repos/o/r", &TokenChoice::Anonymous, GhGet::default()).await.unwrap();
        assert_eq!(mock.requests()[1].header("authorization"), None);
        // 명시한 토큰은 저장된 토큰 대신 쓴다
        client.get("/repos/o/r", &TokenChoice::Explicit(SecretString::new("other-token")), GhGet::default()).await.unwrap();
        assert_eq!(mock.requests()[2].header("authorization"), Some("Bearer other-token"));
    }

    #[tokio::test]
    async fn gh_html_옵션이면_html_accept_를_쓴다() {
        let _g = Guard;
        let mock = Mock::start(|_, _| Resp::new(200).json("{}"));
        gh(&mock.base(), None, 5000, 0).get("/repos/o/r/issues/1", &TokenChoice::Stored, GhGet { html: true, ..Default::default() }).await.unwrap();
        assert_eq!(mock.requests()[0].header("accept"), Some("application/vnd.github.html+json"));
    }

    #[tokio::test]
    async fn gh_api_밖으로_나가는_경로는_거부한다() {
        let _g = Guard;
        let mock = Mock::start(|_, _| Resp::new(200).json("{}"));
        let client = gh(&mock.base(), Some(GTOKEN), 5000, 0);
        for path in ["evil.example/x", "", "@evil.example/x"] {
            assert_eq!(err(client.get(path, &TokenChoice::Stored, GhGet::default()).await), NetError::InvalidPath, "{path:?}");
        }
        assert_eq!(mock.connections(), 0);
        // 문자열 결합이라 호스트를 바꿀 수 없다 — `//host/x` 는 API 호스트의 경로일 뿐이다
        client.get("//evil.example/x", &TokenChoice::Stored, GhGet::default()).await.unwrap();
        assert_eq!(mock.requests()[0].target, "//evil.example/x");
    }

    #[tokio::test]
    async fn gh_304_면_캐시된_본문을_돌려주고_if_none_match_를_보낸다() {
        let _g = Guard;
        let mock = Mock::start(|n, _| {
            if n == 0 {
                Resp::new(200).header("ETag", "\"abc\"").header("Link", "<https://api.github.com/x?page=2>; rel=\"next\"").json(r#"{"a":1}"#)
            } else {
                Resp::new(304)
            }
        });
        let client = gh(&mock.base(), Some(GTOKEN), 5000, 0);
        let first = client.get("/x", &TokenChoice::Stored, GhGet::default()).await.unwrap();
        assert!(first.has_more);
        let again = client.get("/x", &TokenChoice::Stored, GhGet::default()).await.unwrap();
        assert_eq!(again, first);
        assert_eq!(mock.requests().len(), 2);
        // 304 로 확인한 항목은 계속 쓸 수 있다
        assert_eq!(client.get("/x", &TokenChoice::Stored, GhGet::default()).await.unwrap(), first);
        assert_eq!(mock.requests().len(), 3);
        assert!(mock.requests()[1..].iter().all(|r| r.header("if-none-match") == Some("\"abc\"")));
    }

    #[tokio::test]
    async fn gh_토큰_url_accept_가_다르면_캐시를_공유하지_않고_clear_후에는_조건부_요청이_없다() {
        let _g = Guard;
        let mock = Mock::start(|_, _| Resp::new(200).header("ETag", "\"e\"").json("{}"));
        let client = gh(&mock.base(), Some(GTOKEN), 5000, 0);
        let a = TokenChoice::Explicit(SecretString::new("tok-A"));
        let b = TokenChoice::Explicit(SecretString::new("tok-B"));
        client.get("/x", &a, GhGet::default()).await.unwrap();
        client.get("/x", &b, GhGet::default()).await.unwrap(); // 다른 토큰
        client.get("/x", &a, GhGet::default()).await.unwrap(); // 같은 토큰
        client.get("/x", &a, GhGet { html: true, ..Default::default() }).await.unwrap(); // 다른 Accept
        client.get("/x", &a, GhGet { query: &[("q", Some("1"))], ..Default::default() }).await.unwrap(); // 다른 URL
        let inm: Vec<bool> = mock.requests().iter().map(|r| r.header("if-none-match").is_some()).collect();
        assert_eq!(inm, [false, false, true, false, false]);
        client.clear_cache();
        client.get("/x", &a, GhGet::default()).await.unwrap();
        assert!(mock.requests()[5].header("if-none-match").is_none());
    }

    #[tokio::test]
    async fn gh_etag_캐시는_500개를_넘으면_오래된_것부터_버린다() {
        let _g = Guard;
        let mock = Mock::start(|_, _| Resp::new(200).header("ETag", "\"e\"").json("{}"));
        let client = gh(&mock.base(), Some(GTOKEN), 5000, 0);
        for i in 0..501 {
            client.get(&format!("/p/{i}"), &TokenChoice::Stored, GhGet::default()).await.unwrap();
        }
        client.get("/p/0", &TokenChoice::Stored, GhGet::default()).await.unwrap();
        client.get("/p/500", &TokenChoice::Stored, GhGet::default()).await.unwrap();
        let reqs = mock.requests();
        assert!(reqs[501].header("if-none-match").is_none(), "가장 오래된 항목은 밀려났어야 한다");
        assert!(reqs[502].header("if-none-match").is_some());
    }

    #[tokio::test]
    async fn gh_비로그인은_3분_캐시를_쓰고_로그인하면_항상_조건부_요청이다() {
        let _g = Guard;
        let mock = Mock::start(|_, req| if req.header("if-none-match").is_some() { Resp::new(304) } else { Resp::new(200).header("ETag", "\"e\"").json(r#"{"a":1}"#) });
        let anon = gh(&mock.base(), None, 5000, 180_000);
        let first = anon.get("/x", &TokenChoice::Stored, GhGet::default()).await.unwrap();
        assert_eq!(anon.get("/x", &TokenChoice::Stored, GhGet::default()).await.unwrap(), first);
        assert_eq!(mock.requests().len(), 1, "신선한 응답은 네트워크 없이 쓴다");
        // 신선 시간이 지나면(0) 조건부 요청
        let stale = gh(&mock.base(), None, 5000, 0);
        stale.get("/y", &TokenChoice::Stored, GhGet::default()).await.unwrap();
        stale.get("/y", &TokenChoice::Stored, GhGet::default()).await.unwrap();
        assert_eq!(mock.requests().len(), 3);
        assert!(mock.requests()[2].header("if-none-match").is_some());
        // 로그인하면 신선 시간 안이어도 요청한다
        let signed = gh(&mock.base(), Some(GTOKEN), 5000, 180_000);
        signed.get("/z", &TokenChoice::Stored, GhGet::default()).await.unwrap();
        signed.get("/z", &TokenChoice::Stored, GhGet::default()).await.unwrap();
        assert_eq!(mock.requests().len(), 5);
    }

    #[tokio::test]
    async fn gh_남은_횟수를_기록하고_search_는_core_표시를_덮어쓰지_않는다() {
        let _g = Guard;
        let mock = Mock::start(|_, req| {
            let (res, rem) = if req.target.starts_with("/search") { ("search", "3") } else { ("core", "42") };
            Resp::new(200).header("x-ratelimit-resource", res).header("x-ratelimit-remaining", rem).header("x-ratelimit-limit", "5000").header("x-ratelimit-reset", "1700000000").json("{}")
        });
        let seen = Arc::new(Mutex::new(Vec::new()));
        let s2 = seen.clone();
        let client = gh(&mock.base(), Some(GTOKEN), 5000, 0).with_on_rate(move |r| s2.lock().unwrap().push(r.remaining));
        assert!(client.rate().is_none());
        client.get("/repos/o/r", &TokenChoice::Stored, GhGet::default()).await.unwrap();
        client.get("/search/issues", &TokenChoice::Stored, GhGet::default()).await.unwrap();
        let r = client.rate().expect("core 한도");
        assert_eq!((r.remaining, r.limit, r.reset), (42.0, 5000.0, 1_700_000_000.0));
        assert_eq!(*seen.lock().unwrap(), [42.0]);
    }

    #[tokio::test]
    async fn gh_http_오류는_분류되고_한도_문구는_시각을_담는다() {
        let _g = Guard;
        let mock = Mock::start(|_, req| match req.target.as_str() {
            "/limit" => Resp::new(403).header("x-ratelimit-remaining", "0").header("x-ratelimit-limit", "60").header("x-ratelimit-reset", "1700000000").json("{}"),
            "/retry" => Resp::new(429).header("Retry-After", "7").json("{}"),
            "/forbidden" => Resp::new(403).json(r#"{"message":"nope"}"#),
            "/missing" => Resp::new(404).json("{}"),
            "/auth" => Resp::new(401).json("{}"),
            _ => Resp::new(500).json(r#"{"message":"boom"}"#),
        });
        let client = gh(&mock.base(), Some(GTOKEN), 5000, 0);
        let anon = TokenChoice::Anonymous;
        let e = err(client.get("/limit", &anon, GhGet::default()).await);
        assert_eq!((api(&e).0, api(&e).1), ("GH_RATE_LIMIT", Some(403)));
        assert_eq!(api(&e).2, "GitHub API 호출 한도를 초과했습니다. T1700000000 이후에 다시 시도하세요. 로그인하면 한도가 크게 늘어납니다.");
        let e = err(client.get("/limit", &TokenChoice::Stored, GhGet::default()).await);
        assert_eq!(api(&e).2, "GitHub API 호출 한도를 초과했습니다. T1700000000 이후에 다시 시도하세요.");
        let e = err(client.get("/retry", &anon, GhGet::default()).await);
        assert_eq!((api(&e).0, api(&e).2), ("GH_RATE_LIMIT", "GitHub 요청이 너무 잦습니다. 7초 후에 다시 시도하세요"));
        let e = err(client.get("/forbidden", &anon, GhGet::default()).await);
        assert_eq!((api(&e).0, api(&e).2), ("GH_FORBIDDEN", "GitHub 접근이 거부되었습니다: nope"));
        let e = err(client.get("/missing", &anon, GhGet::default()).await);
        assert_eq!((api(&e).0, api(&e).1), ("GH_NOT_FOUND", Some(404)));
        let e = err(client.get("/auth", &anon, GhGet::default()).await);
        assert_eq!((api(&e).0, api(&e).1), ("GH_AUTH", Some(401)));
        let e = err(client.get("/other", &anon, GhGet::default()).await);
        assert_eq!((api(&e).0, api(&e).2), ("GH_HTTP", "GitHub API 오류(500): boom"));
        // 304 가 캐시 없이 오면 오류다
        let m304 = Mock::start(|_, _| Resp::new(304));
        let e = err(gh(&m304.base(), None, 5000, 0).get("/x", &TokenChoice::Stored, GhGet::default()).await);
        assert_eq!((api(&e).0, api(&e).1), ("GH_HTTP", Some(304)));
        // JSON 이 아니면 해석 오류
        let bad = Mock::start(|_, _| Resp::new(200).body("<html>"));
        let e = err(gh(&bad.base(), None, 5000, 0).get("/x", &TokenChoice::Stored, GhGet::default()).await);
        assert_eq!((api(&e).0, api(&e).2), ("GH_HTTP", "GitHub 응답을 해석할 수 없습니다"));
    }

    #[tokio::test]
    async fn gh_네트워크_오류_메시지에_토큰이_실리지_않는다() {
        let _g = Guard;
        let client = gh(&format!("http://{}", closed_addr()), Some(GTOKEN), 10_000, 0);
        let e = err(client.get("/x", &TokenChoice::Stored, GhGet::default()).await);
        assert_eq!((api(&e).0, api(&e).1, api(&e).2), ("GH_NETWORK", None, "GitHub 에 연결할 수 없습니다"));
        assert!(!format!("{e:?}").contains(GTOKEN));
    }

    #[tokio::test]
    async fn gh_본문을_읽다가_시간이_초과되면_timeout_이고_취소하면_aborted_다() {
        let _g = Guard;
        let mock = Mock::start(|_, _| Resp::new(200).header("Content-Type", "application/json").body("{\"a\":").stall(Duration::from_secs(5)));
        let e = err(gh(&mock.base(), None, 500, 0).get("/x", &TokenChoice::Stored, GhGet::default()).await);
        assert_eq!((api(&e).0, api(&e).2), ("GH_TIMEOUT", "GitHub 응답이 15초 안에 오지 않았습니다"));
        let slow = Mock::start(|_, _| Resp::new(200).json("{}").delay(Duration::from_secs(3)));
        let client = gh(&slow.base(), None, 10_000, 0);
        let cancel = CancellationToken::new();
        let c2 = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(150)).await;
            c2.cancel();
        });
        assert_eq!(err(client.get("/x", &TokenChoice::Stored, GhGet { cancel: Some(&cancel), ..Default::default() }).await), NetError::Aborted);
        let done = CancellationToken::new();
        done.cancel();
        let before = slow.connections();
        assert_eq!(err(client.get("/y", &TokenChoice::Stored, GhGet { cancel: Some(&done), ..Default::default() }).await), NetError::Aborted);
        assert_eq!(slow.connections(), before);
    }

    #[tokio::test]
    async fn gh_link_헤더의_rel_next_가_has_more_다() {
        let _g = Guard;
        let mock = Mock::start(|_, req| {
            if req.target == "/more" {
                Resp::new(200).header("Link", "<https://api.github.com/x?page=1>; rel=\"prev\", <https://api.github.com/x?page=3>; rel=\"next\"").json("[]")
            } else {
                Resp::new(200).header("Link", "<https://api.github.com/x?page=1>; rel=\"prev\"").json("[]")
            }
        });
        let client = gh(&mock.base(), None, 5000, 0);
        assert!(client.get("/more", &TokenChoice::Stored, GhGet::default()).await.unwrap().has_more);
        assert!(!client.get("/last", &TokenChoice::Stored, GhGet::default()).await.unwrap().has_more);
    }

    #[tokio::test]
    async fn gh_같은_origin_리다이렉트는_5번까지_따라간다() {
        let _g = Guard;
        // /h0 → /h1 → … : `/hN` 은 N 번째 리다이렉트의 목적지, `/end` 는 마지막 목적지
        let mock = Mock::start(|_, req| {
            let n: usize = req.target.trim_start_matches("/h").parse().unwrap_or(0);
            if req.target == "/end" {
                Resp::new(200).json(r#"{"done":true}"#)
            } else if n < 5 {
                Resp::new(301).header("Location", &format!("/h{}", n + 1))
            } else {
                Resp::new(301).header("Location", "/end")
            }
        });
        let client = gh(&mock.base(), Some(GTOKEN), 5000, 0);
        // /h5 → /end 까지 리다이렉트 6번이 필요한 사슬이다: /h0→h1→h2→h3→h4→h5 가 5번, h5→end 가 6번째
        let e = err(client.get("/h0", &TokenChoice::Stored, GhGet::default()).await);
        assert_eq!((api(&e).0, api(&e).1), ("GH_HTTP", Some(301)));
        assert_eq!(mock.requests().len(), 6, "5번 따라가고 6번째에서 멈춘다");
        // 5번이면 끝까지 간다
        let ok = client.get("/h1", &TokenChoice::Stored, GhGet::default()).await.unwrap();
        assert_eq!(stringify(&ok.data), r#"{"done":true}"#);
        // 같은 origin 이라 Authorization 이 유지된다
        let want = format!("Bearer {GTOKEN}");
        assert!(mock.requests().iter().all(|r| r.header("authorization") == Some(want.as_str())));
    }

    #[tokio::test]
    async fn gh_다른_포트나_호스트로의_리다이렉트는_따라가지_않는다() {
        let _g = Guard;
        let other = Mock::start(|_, _| Resp::new(200).json("{}"));
        let to_port = format!("{}/stolen", other.base());
        let mock_port = Arc::new(Mutex::new(0u16));
        let mp = mock_port.clone();
        let mock = Mock::start(move |_, req| match req.target.as_str() {
            "/port" => Resp::new(301).header("Location", &to_port),
            "/host" => Resp::new(301).header("Location", &format!("http://localhost:{}/stolen", mp.lock().unwrap())),
            _ => Resp::new(301).header("Location", "https://example.invalid/stolen"),
        });
        *mock_port.lock().unwrap() = mock.addr.port();
        let client = gh(&mock.base(), Some(GTOKEN), 5000, 0);
        for path in ["/port", "/host", "/scheme"] {
            let e = err(client.get(path, &TokenChoice::Stored, GhGet::default()).await);
            assert_eq!((api(&e).0, api(&e).1), ("GH_HTTP", Some(301)), "{path}");
        }
        assert_eq!(other.connections(), 0);
        assert!(mock.requests().iter().all(|r| r.target != "/stolen"), "다른 호스트·포트의 주소를 요청했다: {:?}", mock.requests());
        assert_eq!(mock.requests().len(), 3);
    }

    #[tokio::test]
    async fn gh_본문_상한_32mb_를_넘으면_오류다() {
        let _g = Guard;
        assert_eq!(GH_MAX_BODY, 32 * 1024 * 1024);
        let mock = Mock::start(|_, req| if req.target == "/claim" { Resp::new(200).claim_len(GH_MAX_BODY + 1) } else { Resp::new(200).header("Content-Type", "application/json").chunked_forever() });
        let client = gh(&mock.base(), None, 20_000, 0);
        for path in ["/claim", "/chunked"] {
            let e = err(client.get(path, &TokenChoice::Stored, GhGet::default()).await);
            assert_eq!((api(&e).0, api(&e).1, api(&e).2), ("GH_HTTP", Some(200), GH_TOO_BIG), "{path}");
        }
    }

    #[tokio::test]
    async fn gh_graphql_은_로그인이_있어야_하고_부분_오류는_data_를_돌려준다() {
        let _g = Guard;
        let mock = Mock::start(|_, req| match req.text() {
            t if t.contains("limited") => Resp::new(200).json(r#"{"errors":[{"type":"RATE_LIMITED"}],"data":null}"#),
            t if t.contains("nodata") => Resp::new(200).json(r#"{"data":null}"#),
            t if t.contains("partial") => Resp::new(200).json(r#"{"data":{"a":1},"errors":[{"type":"X"}]}"#),
            _ => Resp::new(200).json(r#"{"data":{"viewer":{"login":"me"}}}"#),
        });
        let none = gh(&mock.base(), None, 5000, 0);
        let e = err(none.graphql("{ viewer { login } }", &[], None).await);
        assert_eq!((api(&e).0, api(&e).1), ("GH_AUTH", Some(401)));
        assert_eq!(mock.connections(), 0);
        let client = gh(&mock.base(), Some(GTOKEN), 5000, 0);
        let out = client.graphql("{ viewer { login } }", &[("owner", "o")], None).await.unwrap();
        assert_eq!(stringify(&out), r#"{"viewer":{"login":"me"}}"#);
        let r = &mock.requests()[0];
        assert_eq!((r.method.as_str(), r.target.as_str()), ("POST", "/graphql"));
        assert_eq!(r.header("content-type"), Some("application/json"));
        let want = format!("Bearer {GTOKEN}");
        assert_eq!(r.header("authorization"), Some(want.as_str()));
        let sent: serde_json::Value = serde_json::from_str(&r.text()).unwrap();
        assert_eq!((sent["query"].as_str(), sent["variables"]["owner"].as_str()), (Some("{ viewer { login } }"), Some("o")));
        let e = err(client.graphql("limited", &[], None).await);
        assert_eq!(api(&e).0, "GH_RATE_LIMIT");
        let e = err(client.graphql("nodata", &[], None).await);
        assert_eq!((api(&e).0, api(&e).2), ("GH_HTTP", "GitHub GraphQL 조회에 실패했습니다"));
        assert_eq!(stringify(&client.graphql("partial", &[], None).await.unwrap()), r#"{"a":1}"#);
    }

    // ── 보안 ────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn 오류_봉투에는_토큰과_base64_형태가_없다() {
        let _g = Guard;
        // 서버가 받은 Authorization 과 비밀의 base64 를 본문 메시지에 되돌려 줘도 새지 않는다
        let jira_secrets = [JTOKEN.to_string(), b64(&format!("{EMAIL}:{JTOKEN}"))];
        let gh_secrets = [GTOKEN.to_string(), git_basic_token(GTOKEN)];
        let jm = Mock::start(|_, req| {
            let auth = req.header("authorization").unwrap_or("").to_string();
            Resp::new(403).json(&format!(r#"{{"errorMessages":["denied {auth} {JTOKEN} {}"]}}"#, b64(&format!("{EMAIL}:{JTOKEN}"))))
        });
        let e = err(jira(5000).get(&cred(&jm.base()), "/x", JiraGet::default()).await);
        let e2 = err(jira(5000).post(&cred(&jm.base()), "/x", &json::parse("{}").unwrap(), JiraGet::default()).await);
        for e in [e, e2] {
            assert_eq!(api(&e).0, "JIRA_FORBIDDEN");
            let text = format!("{e:?} {e}");
            assert!(text.contains("***"), "{text}");
            for s in &jira_secrets {
                assert!(!text.contains(s.as_str()), "Jira 오류에 비밀이 있다: {text}");
            }
        }
        let gm = Mock::start(|_, req| {
            let auth = req.header("authorization").unwrap_or("").to_string();
            Resp::new(403).json(&format!(r#"{{"message":"denied {auth} {GTOKEN} {}"}}"#, git_basic_token(GTOKEN)))
        });
        let client = gh(&gm.base(), Some(GTOKEN), 5000, 0);
        let e = err(client.get("/x", &TokenChoice::Stored, GhGet::default()).await);
        let e2 = err(client.graphql("q", &[], None).await);
        for e in [e, e2] {
            let text = format!("{e:?} {e}");
            assert!(text.contains("***"), "{text}");
            for s in &gh_secrets {
                assert!(!text.contains(s.as_str()), "GitHub 오류에 비밀이 있다: {text}");
            }
        }
        // 연결·시간 초과·리다이렉트·상한 초과 오류도 같다
        let closed = format!("http://{}", closed_addr());
        let e = err(jira(10_000).get(&cred(&closed), "/x", JiraGet::default()).await);
        assert!(jira_secrets.iter().all(|s| !format!("{e:?}").contains(s.as_str())));
    }

    #[tokio::test]
    async fn 운영_클라이언트는_http_루프백에_연결하지_않는다() {
        let _g = Guard;
        let mock = Mock::start(|_, _| Resp::new(200).json("{}"));
        let client = JiraClient::new().unwrap();
        let e = err(client.get(&cred(&mock.base()), "/rest/api/2/myself", JiraGet::default()).await);
        assert_eq!(api(&e).0, "JIRA_NETWORK");
        // 사진은 오류 대신 None 이다
        assert_eq!(client.image(&cred(&mock.base()), &format!("{}/a.png", mock.base()), None).await.unwrap(), None);
        assert_eq!(mock.connections(), 0, "https_only 가 http 연결 자체를 막아야 한다");
        assert!(HttpClient::production(Redirects::None).is_ok());
    }

    /// 인증서 검증이 실제로 걸린다: 자체 서명 인증서 서버에서 운영 클라이언트는 핸드셰이크에서 실패하고, 서버는 `Authorization` 을 받지 못한다
    #[tokio::test]
    async fn 운영_클라이언트는_자체_서명_인증서_서버와_핸드셰이크에서_실패하고_인증_정보가_새지_않는다() {
        let _g = Guard;
        let tls = TlsMock::start();

        // 대조: 검증을 끈 클라이언트(native-tls 직접 — 이 파일은 테스트라 clippy 의 reqwest 규칙과 소스 검사 대상이 아니다)는 서버와 요청을 주고받는다.
        // 이게 없으면 "서버가 아무것도 못 받았다"가 서버 고장과 구별되지 않는다
        {
            let connector = native_tls::TlsConnector::builder().danger_accept_invalid_certs(true).danger_accept_invalid_hostnames(true).build().expect("대조 클라이언트");
            let tcp = TcpStream::connect(tls.addr).expect("대조 연결");
            let mut s = connector.connect("127.0.0.1", tcp).expect("검증을 끈 클라이언트는 핸드셰이크가 된다");
            s.write_all(b"GET /control HTTP/1.1\r\nHost: x\r\nAuthorization: CONTROL\r\n\r\n").expect("대조 요청");
            let t = Instant::now();
            while !String::from_utf8_lossy(&tls.received.lock().unwrap()).contains("CONTROL") {
                assert!(t.elapsed() < Duration::from_secs(5), "대조 요청이 서버에 닿지 않았다 — 목 서버가 고장이다");
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        tls.received.lock().unwrap().clear();

        // 운영 클라이언트(JiraClient): 연결 실패 계열 오류
        let c = cred(&format!("https://{}", tls.addr));
        let e = err(JiraClient::new().unwrap().get(&c, "/rest/api/2/myself", JiraGet::default()).await);
        assert_eq!((api(&e).0, api(&e).1), ("JIRA_NETWORK", None));
        assert!(!format!("{e:?}").contains(JTOKEN));

        // 운영 HTTP 코어를 직접: Authorization 을 실어도 핸드셰이크에서 끊긴다
        let direct = HttpClient::production(Redirects::None).unwrap();
        let sent = direct.request(reqwest::Method::GET, &format!("https://{}/x", tls.addr)).header("Authorization", format!("Bearer {JTOKEN}")).send().await;
        let failure = sent.expect_err("자체 서명 인증서는 거부돼야 한다");
        assert_eq!(antler_lib::net::http::classify(&failure), antler_lib::net::http::Failure::Connect);

        // 서버 쪽 처리가 끝날 시간을 준 뒤 확인한다. Windows(SChannel)는 서버 쪽 핸드셰이크가 끝난 뒤 클라이언트가 인증서를 검증하므로
        // 서버의 `completed` 는 늘 수 있다 — 검증은 요청을 보내기 전에 끝나야 하므로 서버가 받은 바이트가 비어 있는 것이 기준이다
        std::thread::sleep(Duration::from_millis(500));
        let got = String::from_utf8_lossy(&tls.received.lock().unwrap()).to_lowercase();
        assert!(got.is_empty(), "서버가 운영 클라이언트에게서 바이트를 받았다: {got:?}");
        for secret in [JTOKEN.to_lowercase(), "authorization".to_string(), b64(&format!("{EMAIL}:{JTOKEN}")).to_lowercase()] {
            assert!(!got.contains(&secret), "서버가 받은 바이트에 {secret} 가 있다");
        }
    }

    #[tokio::test]
    async fn 운영_클라이언트는_tls_가_아닌_서버와_핸드셰이크에서_실패하고_인증_정보가_새지_않는다() {
        let _g = Guard;
        let raw = Raw::start();
        let c = cred(&format!("https://{}", raw.addr));
        let e = err(JiraClient::new().unwrap().get(&c, "/rest/api/2/myself", JiraGet::default()).await);
        assert_eq!(api(&e).0, "JIRA_NETWORK");
        assert!(!format!("{e:?}").contains(JTOKEN));
        let got = raw.received.lock().unwrap().clone();
        let text = String::from_utf8_lossy(&got).into_owned();
        assert!(!got.starts_with(b"GET "), "평문 요청이 나갔다: {text:?}");
        for secret in [JTOKEN.to_string(), b64(&format!("{EMAIL}:{JTOKEN}")), "Authorization".to_string(), "authorization".to_string()] {
            assert!(!text.contains(&secret), "서버가 받은 바이트에 {secret} 가 있다");
        }
    }

    #[tokio::test]
    async fn 프록시_덫은_살아_있다() {
        // 덫이 실제로 연결을 세는지 확인한다(다른 테스트의 "연결 0" 이 우연이 아니도록). 덫을 잠근 채 연결하고 세고 대조 값도 함께 올린다
        let g = TRAP.lock().unwrap();
        let l = g.as_ref().expect("덫");
        let addr = l.local_addr().unwrap();
        let _conn = TcpStream::connect(addr).expect("덫에 연결");
        let t = Instant::now();
        loop {
            if l.accept().is_ok() {
                TRAP_SEEN.fetch_add(1, Ordering::SeqCst);
                TRAP_CONTROL.fetch_add(1, Ordering::SeqCst);
                break;
            }
            assert!(t.elapsed() < Duration::from_secs(5), "덫이 연결을 받지 못했다");
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(std::env::var("HTTPS_PROXY").ok(), Some(format!("http://{addr}")));
    }

    // ── 소스 검사: 운영 코드는 테스트 전용 주입과 클라이언트 생성을 쓰지 않는다 ─────────────────────

    fn rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
        for e in std::fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                rs_files(&p, out);
            } else if p.extension().is_some_and(|x| x == "rs") {
                out.push(p);
            }
        }
    }

    /// 주석이 아닌 줄에서 규칙을 어긴 것. `rel` 은 `src/` 기준 경로(`/` 구분)
    fn source_violations(rel: &str, text: &str) -> Vec<String> {
        let mut out = Vec::new();
        for (i, line) in text.lines().enumerate() {
            let t = line.trim_start();
            if t.starts_with("//") {
                continue;
            }
            // 네트워크 계층의 `*_for_test`(net·github·jira 안의 것과 어디서든 `new_for_test`·`insecure_loopback_for_test`)는 정의(`fn`)만 있어야 한다 — 호출은 테스트(tests/)에만 있다.
            // (git/fetch 의 `*_for_test` 는 플랫폼 인자를 받는 순수 함수로 운영 코드도 부른다 — 이 검사의 대상이 아니다)
            let in_net_area = ["net/", "github/", "jira/"].iter().any(|d| rel.starts_with(d));
            let hook = t.contains("new_for_test") || t.contains("insecure_loopback_for_test") || (in_net_area && t.contains("_for_test"));
            let test_hook = hook && !(t.starts_with("pub fn ") || t.starts_with("fn "));
            // 루프백 http 허용 플래그는 검증 함수가 읽는 jira/validate.rs 에만 있다
            let loopback = t.contains("insecure_loopback") && rel != "jira/validate.rs";
            // reqwest 클라이언트는 net/http.rs 에서만 만든다(clippy 의 disallowed-methods 와 이중 방어).
            // `Client` 라는 이름 자체를 쓰지 못하게 한다 — `HttpClient`·`JiraClient` 는 다른 이름이다
            // (문자열 리터럴 안의 글자는 보지 않는다: `"Client ID"` 같은 문구가 있다)
            let code: String = t.split('"').step_by(2).collect::<Vec<_>>().join(" ");
            let names_reqwest_client = code.split(|c: char| !(c.is_alphanumeric() || c == '_')).any(|w| w == "Client" || w == "ClientBuilder");
            let client = rel != "net/http.rs" && (names_reqwest_client || t.contains("reqwest::get"));
            // TLS 검증·프록시·이름 해석을 바꾸는 호출은 net/http.rs 도 예외 없이 쓰지 않는다(운영 설정은 `no_proxy()` 와 기본 TLS 검증뿐이다)
            // (`.resolve(` 는 reqwest 의 `resolve(도메인, 주소)` 처럼 인자가 둘 이상인 것만 본다 — 인자 하나짜리는 앱 안의 다른 `resolve` 다)
            let resolves_name = t.find(".resolve(").is_some_and(|i| t[i..].split(')').next().is_some_and(|args| args.contains(',')));
            let weakens = resolves_name || ["danger_accept", "tls_danger", ".proxy(", ".resolve_to_addrs("].iter().any(|p| t.contains(p));
            if test_hook || loopback || client || weakens {
                out.push(format!("{rel}:{}: {t}", i + 1));
            }
        }
        out
    }

    #[test]
    fn 운영_코드에는_테스트_전용_주입_호출과_클라이언트_생성이_없다() {
        // 검사기 자체 확인: 어긴 예를 잡고, 정의·주석은 통과시킨다
        assert_eq!(source_violations("x.rs", "let c = JiraClient::new_for_test(a, b, c);").len(), 1);
        assert_eq!(source_violations("x.rs", "let o = SiteOptions::insecure_loopback_for_test();").len(), 1);
        assert_eq!(source_violations("x.rs", "let o = SiteOptions { allow_insecure_loopback: true };").len(), 1);
        assert_eq!(source_violations("x.rs", "let c = reqwest::Client::new();").len(), 1);
        assert_eq!(source_violations("x.rs", "let c = Client::builder();").len(), 1);
        assert!(source_violations("x.rs", "    pub fn new_for_test(a: u8) -> Self {").is_empty());
        assert!(source_violations("x.rs", "    // JiraClient::new_for_test 로 만든다").is_empty());
        assert!(source_violations("jira/validate.rs", "if opts.allow_insecure_loopback {").is_empty());
        assert_eq!(source_violations("x.rs", "use reqwest::{Client, Method};").len(), 1);
        assert!(source_violations("x.rs", "let c = JiraClient::new().unwrap(); let h = HttpClient::production(r);").is_empty());
        assert!(source_violations("net/http.rs", "let client = Client::builder()").is_empty());
        assert!(source_violations("net/http.rs", "    .no_proxy()").is_empty());
        assert!(source_violations("router.rs", "match self.mirror.resolve(repo_id) {").is_empty());
        for bad in ["b.danger_accept_invalid_certs(true)", "b.tls_danger_accept_invalid_hostnames(true)", "b.proxy(p)", "b.resolve(h, a)"] {
            assert_eq!(source_violations("net/http.rs", bad).len(), 1, "net/http.rs 도 예외가 아니다: {bad}");
            assert_eq!(source_violations("x.rs", bad).len(), 1, "{bad}");
        }

        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        rs_files(&src, &mut files);
        assert!(files.len() > 40, "src 를 찾지 못했다: {}", files.len());
        let mut bad = Vec::new();
        for f in &files {
            let rel = f.strip_prefix(&src).unwrap().to_string_lossy().replace('\\', "/");
            bad.extend(source_violations(&rel, &std::fs::read_to_string(f).unwrap()));
        }
        assert!(bad.is_empty(), "운영 코드가 테스트 전용 주입을 쓰거나 클라이언트를 직접 만든다:\n{}", bad.join("\n"));
    }

    const HOOK_CFG: &str = r#"#[cfg(feature = "test-hooks")]"#;

    /// 테스트 훅의 정의(`Rewrite`·`Profile::Test` 변형·net/github/jira 의 `*_for_test`·`insecure_loopback_for_test`)와, 그 위 속성·문서 줄에
    /// `#[cfg(feature = "test-hooks")]` 가 없는 것. 반환은 (정의 수, 속성이 빠진 정의들)
    fn unguarded_hooks(rel: &str, text: &str) -> (usize, Vec<String>) {
        let in_net_area = ["net/", "github/", "jira/"].iter().any(|d| rel.starts_with(d));
        let lines: Vec<&str> = text.lines().map(str::trim_start).collect();
        let (mut defs, mut bad) = (0, Vec::new());
        for (i, t) in lines.iter().enumerate() {
            let is_def = t.starts_with("pub type Rewrite") || t.starts_with("Test {") || (in_net_area && t.contains("fn ") && t.contains("_for_test("));
            if !is_def || t.starts_with("//") {
                continue;
            }
            defs += 1;
            let guarded = lines[..i].iter().rev().take_while(|l| l.starts_with("#[") || l.starts_with("//")).any(|l| *l == HOOK_CFG);
            if !guarded {
                bad.push(format!("{rel}:{}: {t}", i + 1));
            }
        }
        (defs, bad)
    }

    /// feature 없이 빌드하면 훅이 존재하지 않는다 — 컴파일 실패 예제 대신, 정의마다 cfg 속성이 붙어 있는지를 소스에서 검사한다
    /// (feature 없는 `cargo clippy --all-targets` 가 훅 없이 lib 가 컴파일되는 것을 따로 확인한다)
    #[test]
    fn 테스트_훅_정의는_모두_test_hooks_feature_뒤에_있다() {
        assert_eq!(unguarded_hooks("net/http.rs", "pub type Rewrite = X;").1.len(), 1);
        assert!(unguarded_hooks("net/http.rs", "#[cfg(feature = \"test-hooks\")]\n#[doc(hidden)]\npub fn new_for_test(a: u8) {").1.is_empty());
        assert!(unguarded_hooks("net/http.rs", "#[cfg(feature = \"test-hooks\")]\n/// 문서\n#[doc(hidden)]\npub fn new_for_test(a: u8) {").1.is_empty());
        assert_eq!(unguarded_hooks("jira/client.rs", "#[doc(hidden)]\npub fn new_for_test(a: u8) {").1.len(), 1);
        assert_eq!(unguarded_hooks("net/http.rs", "enum P {\n    Production,\n    Test { a: u8 },\n}").1.len(), 1);
        // 앞 항목의 속성이 다음 정의로 번지지 않는다
        assert_eq!(unguarded_hooks("net/http.rs", "#[cfg(feature = \"test-hooks\")]\nfn a() {}\nfn b_for_test() {}").1.len(), 1);

        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        rs_files(&src, &mut files);
        let (mut defs, mut bad) = (0, Vec::new());
        for f in &files {
            let rel = f.strip_prefix(&src).unwrap().to_string_lossy().replace('\\', "/");
            let (d, b) = unguarded_hooks(&rel, &std::fs::read_to_string(f).unwrap());
            defs += d;
            bad.extend(b);
        }
        // Rewrite·Profile::Test·HttpClient/GitHubClient/JiraClient 의 new_for_test·insecure_loopback_for_test — 정의를 못 찾았다면 검사기가 눈먼 것이다
        assert!(defs >= 6, "훅 정의를 {defs}개만 찾았다");
        assert!(bad.is_empty(), "test-hooks feature 뒤에 있지 않은 테스트 훅:\n{}", bad.join("\n"));
    }

    // ── Jira 서비스(PR 5) ───────────────────────────────────────────────────
    //
    // 루프백 목 서버(127.0.0.1)와 메모리 보관소만 쓴다. `start_paused` 는 쓰지 않는다 — /field 대기 상한은 짧은 실제 시간(`wait_ms`)으로 건다.
    // 시계(`Clock`)를 주입하는 시험(쿨다운)은 소켓 없이 닫힌 포트(연결 거부)로 만든다.
    mod service {
        use std::sync::atomic::AtomicU64;

        use antler_lib::jira::api_map as jira_map;
        use antler_lib::jira::service::{Clock, JiraError, JiraService, SystemClock, TokioSleeper};
        use antler_lib::jira::validate::{parse_connect_input, SiteOptions};
        use antler_lib::net::js_date::parse_date_ms;
        use antler_lib::net::json::Json;
        use antler_lib::secret::memory::MemoryStore;
        use antler_lib::secret::{GithubMethod, SecretError, SecretStore, Vault};
        use serde_json::{json, Value};
        use zeroize::Zeroizing;

        use super::*;

        const SECRET: &str = "tok_SECRET_0123456789";
        const PNG: [u8; 16] = [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52];

        // ── 도우미 ──────────────────────────────────────────────────────────

        pub(super) struct FakeClock(AtomicU64);

        impl FakeClock {
            pub(super) fn at(ms: u64) -> Arc<Self> {
                Arc::new(Self(AtomicU64::new(ms)))
            }

            pub(super) fn set(&self, ms: u64) {
                self.0.store(ms, Ordering::SeqCst);
            }
        }

        impl Clock for FakeClock {
            fn now_ms(&self) -> f64 {
                self.0.load(Ordering::SeqCst) as f64
            }
        }

        pub(super) fn new_vault() -> Arc<Vault> {
            Arc::new(Vault::new(Arc::new(MemoryStore::standing_in_for_keychain())))
        }

        fn service_on(vault: Arc<Vault>, clock: Arc<dyn Clock>, wait_ms: u64) -> JiraService {
            let client = JiraClient::new_for_test(Duration::from_secs(5), None, Duration::from_millis(1)).unwrap();
            JiraService::with_parts(vault, client, SiteOptions::insecure_loopback_for_test(), clock, Arc::new(TokioSleeper), Duration::from_millis(wait_ms))
        }

        fn service() -> JiraService {
            service_on(new_vault(), Arc::new(SystemClock), 4000)
        }

        fn service_wait(wait_ms: u64) -> JiraService {
            service_on(new_vault(), Arc::new(SystemClock), wait_ms)
        }

        fn j(j: &Json) -> Value {
            serde_json::from_str(&stringify(j)).unwrap()
        }

        fn ok(v: Value) -> Resp {
            Resp::new(200).json(&v.to_string())
        }

        fn status(code: u16, v: Value) -> Resp {
            Resp::new(code).json(&v.to_string())
        }

        fn path_of(r: &Req) -> &str {
            r.target.split('?').next().unwrap_or("")
        }

        fn qp(r: &Req, k: &str) -> Option<String> {
            url::Url::parse(&format!("http://x{}", r.target)).unwrap().query_pairs().find(|(n, _)| n == k).map(|(_, v)| v.into_owned())
        }

        fn cloud_in(base: &str) -> Value {
            json!({"kind": "cloud", "siteUrl": base, "email": "a@b.c", "token": SECRET})
        }

        fn server_in(base: &str) -> Value {
            json!({"kind": "server", "siteUrl": base, "token": SECRET})
        }

        async fn connect_cloud(svc: &JiraService, m: &Mock) {
            svc.connect(&cloud_in(&m.base())).await.unwrap();
        }

        async fn connect_server(svc: &JiraService, m: &Mock) {
            svc.connect(&server_in(&m.base())).await.unwrap();
        }

        fn key(k: &str) -> Value {
            json!(k)
        }

        fn failure<T: std::fmt::Debug>(r: Result<T, JiraError>) -> JiraError {
            r.expect_err("오류여야 한다")
        }

        /// 조건이 참이 될 때까지 기다린다(최대 `WAIT_LIMIT`)
        pub(super) async fn wait_for(f: impl Fn() -> bool) {
            let t = Instant::now();
            while !f() {
                assert!(t.elapsed() < WAIT_LIMIT, "조건이 되지 않았다");
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }

        /// 목 응답을 테스트가 직접 푸는 문. 처리기가 `wait` 로 응답을 붙들고, 테스트가 `open` 으로 놓아 준다.
        /// 시간이 아니라 순서로 "응답이 오기 전/후"를 만든다. 열리지 않아도 처리기 스레드가 영원히 남지 않게 `GATE_LIMIT` 뒤에는 풀린다
        #[derive(Clone, Default)]
        pub(super) struct Gate(Arc<(Mutex<bool>, std::sync::Condvar)>);

        const GATE_LIMIT: Duration = Duration::from_secs(30);
        /// 테스트가 기다리는 최대 시간(조건이 되지 않아 걸리는 경우에만 쓰인다. 정상이면 곧바로 끝난다)
        pub(super) const WAIT_LIMIT: Duration = Duration::from_secs(10);

        impl Gate {
            pub(super) fn wait(&self) {
                let (lock, cv) = &*self.0;
                let g = lock.lock().unwrap();
                let _ = cv.wait_timeout_while(g, GATE_LIMIT, |open| !*open).unwrap();
            }

            pub(super) fn open(&self) {
                let (lock, cv) = &*self.0;
                *lock.lock().unwrap() = true;
                cv.notify_all();
            }
        }

        /// 문이 닫힌 채로 끝나야 하는 비동기 작업을 기다린다(상한 안에 안 끝나면 실패)
        pub(super) async fn within<T>(f: impl std::future::Future<Output = T>) -> T {
            tokio::time::timeout(WAIT_LIMIT, f).await.expect("상한 안에 끝나지 않았다")
        }

        pub(super) fn count(m: &Mock, f: impl Fn(&Req) -> bool) -> usize {
            m.requests().iter().filter(|r| f(r)).count()
        }

        const ADF: &str = r#"{"type":"doc","content":[{"type":"paragraph","content":[{"type":"text","text":"설명 <b>본문</b>","marks":[{"type":"strong"}]}]},{"type":"paragraph","content":[{"type":"text","text":"x","marks":[{"type":"link","attrs":{"href":"javascript:alert(1)"}}]}]}]}"#;

        fn jira_handler(_n: usize, r: &Req) -> Resp {
            let path = path_of(r);
            if path.ends_with("/myself") {
                return ok(json!({"displayName": "홍길동"}));
            }
            if path.ends_with("/issue/ABC-1") {
                let adf: Value = serde_json::from_str(ADF).unwrap();
                return ok(json!({
                    "key": "ABC-1",
                    "fields": {"summary": "로그인 오류", "status": {"name": "진행 중", "statusCategory": {"key": "indeterminate"}}, "issuetype": {"name": "Bug"}, "priority": {"name": "High"},
                        "assignee": {"displayName": "홍길동"}, "reporter": {"displayName": "김철수"}, "created": "2026-01-01T00:00:00.000+0000", "updated": "2026-01-02T00:00:00.000+0000", "description": adf},
                    "renderedFields": {"description": "<p>서버 렌더</p>"}
                }));
            }
            if path.ends_with("/comment") {
                let adf: Value = serde_json::from_str(ADF).unwrap();
                return ok(json!({"comments": [{"author": {"displayName": "이영희"}, "created": "2026-01-03", "body": adf, "renderedBody": "<p>댓글 렌더</p>"}]}));
            }
            if path.ends_with("/search/jql") {
                return ok(json!({"issues": [{"key": "ABC-2", "fields": {"summary": "s"}}], "nextPageToken": "tok2"}));
            }
            if path.ends_with("/search") {
                return ok(json!({"issues": [{"key": "ABC-3", "fields": {"summary": "s"}}], "startAt": 0, "total": 99}));
            }
            status(404, json!({"errorMessages": ["nf"]}))
        }

        // ── 연결·복원·해제 ──────────────────────────────────────────────────

        #[tokio::test]
        async fn 서비스는_연결하면_myself_로_확인하고_상태_응답_어디에도_토큰이_없다() {
            let _g = Guard;
            let m = Mock::start(jira_handler);
            let svc = service();
            let state = svc.connect(&json!({"kind": "cloud", "siteUrl": format!("{}/", m.base()), "email": "me@example.com", "token": format!(" {SECRET} ")})).await.unwrap();
            let text = serde_json::to_string(&state).unwrap();
            // 상태에는 토큰 필드가 없다: 키 목록과 직렬화 문자열 둘 다 본다
            let keys: Vec<String> = serde_json::from_str::<Value>(&text).unwrap().as_object().unwrap().keys().cloned().collect();
            let mut want = vec!["connected", "kind", "siteUrl", "email", "displayName"];
            want.sort_unstable();
            let mut got = keys.clone();
            got.sort_unstable();
            assert_eq!(got, want, "{text}");
            assert!(!text.to_lowercase().contains("token") && !text.contains(SECRET));
            assert_eq!(serde_json::from_str::<Value>(&text).unwrap(), json!({"connected": true, "kind": "cloud", "siteUrl": m.base(), "email": "me@example.com", "displayName": "홍길동"}));
            assert_eq!(m.requests()[0].target, "/rest/api/3/myself");
            assert_eq!(m.requests()[0].header("authorization"), Some(format!("Basic {}", b64(&format!("me@example.com:{SECRET}")).as_str()).as_str()));

            let detail = svc.issue(&key("ABC-1"), None).await.unwrap();
            let comments = svc.comments(&key("ABC-1"), None).await.unwrap();
            let found = svc.search(&json!({"jql": "project = ABC"}), None).await.unwrap();
            let all = format!("{}{}{}{:?}", stringify(&detail), stringify(&comments), stringify(&found), svc.auth());
            assert!(!all.contains(SECRET));
        }

        #[tokio::test]
        async fn 서비스는_다시_시작해도_저장된_연결을_복원한다() {
            let _g = Guard;
            let m = Mock::start(jira_handler);
            let vault = new_vault();
            connect_server(&service_on(vault.clone(), Arc::new(SystemClock), 4000), &m).await;
            let again = service_on(vault.clone(), Arc::new(SystemClock), 4000);
            let a = again.auth();
            assert_eq!((a.connected, a.kind, a.site_url.as_deref()), (true, Some("server"), Some(m.base().as_str())));
            again.issue(&key("ABC-1"), None).await.unwrap();
            assert_eq!(m.requests().last().unwrap().header("authorization"), Some(format!("Bearer {SECRET}").as_str()));
            assert!(!again.disconnect().await.unwrap().connected);
            assert!(vault.load_jira_with(SiteOptions::insecure_loopback_for_test()).is_none());
            assert!(!service_on(vault, Arc::new(SystemClock), 4000).auth().connected);
        }

        #[tokio::test]
        async fn 서비스는_401_이면_저장하지_않고_토큰_없는_메시지로_거부한다() {
            let _g = Guard;
            let m = Mock::start(|_, _| status(401, json!({})));
            let vault = new_vault();
            let svc = service_on(vault.clone(), Arc::new(SystemClock), 4000);
            let e = failure(svc.connect(&cloud_in(&m.base())).await);
            assert!(!e.message.contains(SECRET), "{e}");
            assert_eq!((e.code, e.message.as_str()), ("ERROR", "이메일·토큰이 올바르지 않거나 만료되었습니다"));
            assert!(!svc.auth().connected);
            assert!(vault.load_jira_with(SiteOptions::insecure_loopback_for_test()).is_none());
        }

        #[tokio::test]
        async fn 서비스는_연결_전에는_조회할_수_없고_열_수_있는_주소는_연결한_사이트의_것뿐이다() {
            let _g = Guard;
            let svc = service();
            let e = failure(svc.issue(&key("ABC-1"), None).await);
            assert_eq!((e.code, e.message.as_str()), ("JIRA_AUTH", "Jira 에 연결되어 있지 않습니다"));
            assert_eq!(svc.external_url(&json!("https://x.atlassian.net/browse/ABC-1")), None);
            let m = Mock::start(jira_handler);
            connect_server(&svc, &m).await;
            let own = format!("{}/browse/ABC-1", m.base());
            assert_eq!(svc.external_url(&json!(own)), Some(own));
            let alt = format!("http://127.0.0.1:{}/browse/ABC-1", m.addr.port().wrapping_add(1));
            assert_eq!(svc.external_url(&json!(alt)), None);
            assert_eq!(svc.external_url(&json!("https://evil.example/browse/ABC-1")), None);
        }

        #[tokio::test]
        async fn 서비스_상태는_저장소가_못_남기면_jira_만_storage_unavailable_이다() {
            let _g = Guard;
            struct NoWrite(Arc<MemoryStore>);
            impl SecretStore for NoWrite {
                fn get(&self, a: &str) -> Result<Option<Zeroizing<Vec<u8>>>, SecretError> {
                    self.0.get(a)
                }
                fn set(&self, _: &str, _: &[u8]) -> Result<(), SecretError> {
                    Err(SecretError::Backend)
                }
                fn delete(&self, a: &str) -> Result<(), SecretError> {
                    self.0.delete(a)
                }
                fn persistent(&self) -> bool {
                    true
                }
            }
            let m = Mock::start(jira_handler);
            let vault = Arc::new(Vault::new(Arc::new(NoWrite(Arc::new(MemoryStore::standing_in_for_keychain())))));
            let svc = service_on(vault.clone(), Arc::new(SystemClock), 4000);
            assert!(!svc.auth().storage_unavailable);
            let state = svc.connect(&server_in(&m.base())).await.unwrap();
            assert!(state.connected && state.storage_unavailable);
            assert_eq!(serde_json::to_value(&state).unwrap()["storageUnavailable"], json!(true));
            // GitHub 계정은 따로다 — Jira 의 쓰기 실패가 GitHub 상태를 바꾸지 않는다
            assert!(!vault.github_storage_unavailable());
            assert!(vault.save_github(&SecretString::new("ghp_x"), GithubMethod::Pat).is_ok());
            assert!(vault.jira_storage_unavailable());
        }

        #[tokio::test]
        async fn 서비스는_키체인_삭제가_실패하면_연결은_끊고_오류를_돌려준다() {
            let _g = Guard;
            struct NoDelete(Arc<MemoryStore>);
            impl SecretStore for NoDelete {
                fn get(&self, a: &str) -> Result<Option<Zeroizing<Vec<u8>>>, SecretError> {
                    self.0.get(a)
                }
                fn set(&self, a: &str, v: &[u8]) -> Result<(), SecretError> {
                    self.0.set(a, v)
                }
                fn delete(&self, _: &str) -> Result<(), SecretError> {
                    Err(SecretError::Backend)
                }
                fn persistent(&self) -> bool {
                    true
                }
            }
            let m = Mock::start(jira_handler);
            let svc = service_on(Arc::new(Vault::new(Arc::new(NoDelete(Arc::new(MemoryStore::standing_in_for_keychain()))))), Arc::new(SystemClock), 4000);
            connect_server(&svc, &m).await;
            let e = failure(svc.disconnect().await);
            assert_eq!(e.code, "ERROR");
            assert!(e.message.contains("토큰을 지우지 못했습니다") && !e.message.contains(SECRET), "{e}");
            assert!(!svc.auth().connected, "연결은 끊겨 있어야 한다");
        }

        #[tokio::test]
        async fn 서비스_오류_문구는_마지막에_비밀을_가린다() {
            let _g = Guard;
            let m = Mock::start(|_, r| if path_of(r).ends_with("/myself") { ok(json!({"displayName": "x"})) } else { status(400, json!({"errorMessages": [format!("토큰 {SECRET} 가 틀립니다")]})) });
            let svc = service();
            connect_server(&svc, &m).await;
            let e = failure(svc.transition(&key("ABC-1"), &json!("31")).await);
            assert_eq!((e.code, e.status), ("JIRA_HTTP", Some(400)));
            assert!(!e.message.contains(SECRET) && e.message.contains("***"), "{e}");
        }

        // ── 조회 ────────────────────────────────────────────────────────────

        #[tokio::test]
        async fn 서비스_cloud_이슈는_adf_를_html_로_바꾸고_이스케이프하며_위험한_링크는_싣지_않는다() {
            let _g = Guard;
            let m = Mock::start(jira_handler);
            let svc = service();
            connect_cloud(&svc, &m).await;
            let d = j(&svc.issue(&key("ABC-1"), None).await.unwrap());
            assert_eq!(
                (d["key"].clone(), d["summary"].clone(), d["status"].clone(), d["statusCategory"].clone(), d["issueType"].clone(), d["priority"].clone(), d["assignee"].clone(), d["reporter"].clone(), d["url"].clone()),
                (json!("ABC-1"), json!("로그인 오류"), json!("진행 중"), json!("indeterminate"), json!("Bug"), json!("High"), json!("홍길동"), json!("김철수"), json!(format!("{}/browse/ABC-1", m.base())))
            );
            let html = d["descriptionHtml"].as_str().unwrap();
            assert!(html.contains("<strong>설명 &lt;b&gt;본문&lt;/b&gt;</strong>"), "{html}");
            assert!(!html.contains("javascript:") && !html.contains("<b>"));
            assert_eq!(j(&svc.comments(&key("ABC-1"), None).await.unwrap())[0]["author"], json!("이영희"));
            let r = m.requests().into_iter().find(|r| r.target.contains("/issue/ABC-1?")).unwrap();
            assert_eq!(qp(&r, "expand"), None, "Cloud 는 ADF 를 직접 변환하므로 서버 렌더 HTML 을 요청하지 않는다");
        }

        #[tokio::test]
        async fn 서비스_server_이슈는_서버가_렌더한_html_을_쓴다() {
            let _g = Guard;
            let m = Mock::start(jira_handler);
            let svc = service();
            connect_server(&svc, &m).await;
            assert_eq!(j(&svc.issue(&key("ABC-1"), None).await.unwrap())["descriptionHtml"], json!("<p>서버 렌더</p>"));
            assert_eq!(j(&svc.comments(&key("ABC-1"), None).await.unwrap())[0]["bodyHtml"], json!("<p>댓글 렌더</p>"));
            let r = m.requests().into_iter().find(|r| r.target.starts_with("/rest/api/2/issue/ABC-1?")).expect("v2 이슈 요청");
            assert_eq!(qp(&r, "expand").as_deref(), Some("renderedFields"));
        }

        #[tokio::test]
        async fn 서비스_검색은_cloud_는_토큰_server_는_start_at_이고_페이지_상한에서_멈춘다() {
            let _g = Guard;
            let m = Mock::start(jira_handler);
            let cloud = service();
            connect_cloud(&cloud, &m).await;
            let r = j(&cloud.search(&json!({"jql": "x = 1"}), None).await.unwrap());
            assert_eq!(r["nextCursor"], json!("tok2"));
            let last = j(&cloud.search(&json!({"jql": "x = 1", "cursor": "tok2", "page": 8}), None).await.unwrap());
            assert!(last.get("nextCursor").is_none());
            assert_eq!(qp(m.requests().last().unwrap(), "nextPageToken").as_deref(), Some("tok2"));
            assert!(failure(cloud.search(&json!({"jql": "x = 1", "page": 9}), None).await).message.contains("잘못된 페이지"));

            let dc = service();
            connect_server(&dc, &m).await;
            let r = j(&dc.search(&json!({"jql": "x = 1"}), None).await.unwrap());
            assert_eq!((r["items"][0]["key"].clone(), r["nextCursor"].clone()), (json!("ABC-3"), json!("1")));
            dc.search(&json!({"jql": "x = 1", "cursor": "25", "page": 2}), None).await.unwrap();
            let last = m.requests().last().unwrap().clone();
            assert_eq!((qp(&last, "startAt").as_deref(), qp(&last, "maxResults").as_deref()), (Some("25"), Some("25")));
            // TS 와 같은 문구이고, 요청을 보내기 전에 거부한다
            let before = m.requests().len();
            let e = failure(dc.search(&json!({"jql": "x = 1", "cursor": "100000", "page": 2}), None).await);
            assert_eq!((e.code, e.message.as_str()), ("ERROR", "잘못된 페이지 표지입니다"));
            assert_eq!(m.requests().len(), before);
        }

        #[tokio::test]
        async fn 서비스_개발_정보는_이슈_id_에서_연동_종류_상세로_가고_나쁜_주소와_sha_는_버린다() {
            let _g = Guard;
            let m = Mock::start(|_, r| {
                let path = path_of(r);
                if path.ends_with("/myself") {
                    return ok(json!({"displayName": "x"}));
                }
                if path.ends_with("/issue/ABC-1") {
                    return ok(json!({"id": "10001", "key": "ABC-1", "fields": {}}));
                }
                if path.ends_with("/dev-status/latest/issue/summary") {
                    return ok(json!({"summary": {"pullrequest": {"byInstanceType": {"GitHub": {}}}, "repository": {"byInstanceType": {"GitHub": {}, "bad type!": {}}}}}));
                }
                if path.ends_with("/dev-status/latest/issue/detail") {
                    if qp(r, "dataType").as_deref() == Some("pullrequest") {
                        return ok(json!({"detail": [{"pullRequests": [
                            {"name": "로그인 수정", "url": "https://github.com/o/r/pull/7", "status": "MERGED", "author": {"name": "홍길동"}, "repositoryName": "o/r", "source": {"branch": "fix/ABC-1"}, "lastUpdate": "2026-01-02T00:00:00.000+0000"},
                            {"name": "나쁜 주소", "url": "javascript:alert(1)", "status": "OPEN", "repositoryName": "o/r"}]}]}));
                    }
                    return ok(json!({"detail": [{"repositories": [{"name": "o/r", "commits": [
                        {"id": "abcdef1234567", "message": "ABC-1 로그인 수정\n\n본문", "url": "https://github.com/o/r/commit/abcdef1234567", "author": {"name": "홍길동"}, "authorTimestamp": 1767225600000u64},
                        {"id": "not-a-sha", "message": "x"}]}]}]}));
                }
                status(404, json!({}))
            });
            let svc = service();
            connect_cloud(&svc, &m).await;
            let dev = j(&svc.dev_info(&key("ABC-1"), None).await.unwrap());
            assert_eq!(dev["available"], json!(true));
            assert_eq!(
                dev["pulls"],
                json!([
                    {"title": "로그인 수정", "url": "https://github.com/o/r/pull/7", "state": "merged", "author": "홍길동", "repo": "o/r", "branch": "fix/ABC-1", "updated": "2026-01-02T00:00:00.000+0000"},
                    {"title": "나쁜 주소", "url": "", "state": "open", "author": "", "repo": "o/r", "branch": "", "updated": ""}
                ])
            );
            assert_eq!(
                dev["commits"],
                json!([{"sha": "abcdef1234567", "message": "ABC-1 로그인 수정", "url": "https://github.com/o/r/commit/abcdef1234567", "author": "홍길동", "repo": "o/r", "date": "2026-01-01T00:00:00.000Z"}])
            );
            assert!(m.requests().iter().all(|r| !r.target.contains("bad")), "이상한 연동 종류 이름은 요청하지 않는다");
            assert!(!dev.to_string().contains(SECRET));
        }

        #[tokio::test]
        async fn 서비스_개발_정보는_연동_api_가_없으면_오류가_아니라_available_false() {
            let _g = Guard;
            let none = json!({"available": false, "pulls": [], "commits": []});
            let m = Mock::start(jira_handler);
            let svc = service();
            connect_server(&svc, &m).await;
            // 이슈 응답에 id 가 없으면 요약까지 가지 않는다
            assert_eq!(j(&svc.dev_info(&key("ABC-1"), None).await.unwrap()), none);
            let m2 = Mock::start(|_, r| if r.target.contains("/myself") { ok(json!({"displayName": "x"})) } else if r.target.contains("/issue/ABC-1") { ok(json!({"id": "5"})) } else { status(404, json!({})) });
            let svc2 = service();
            connect_server(&svc2, &m2).await;
            assert_eq!(j(&svc2.dev_info(&key("ABC-1"), None).await.unwrap()), none);
            assert!(m2.requests().iter().any(|r| r.target.starts_with("/rest/dev-status/latest/issue/summary?issueId=5")));
        }

        #[tokio::test]
        async fn 서비스는_요청_id_로_진행_중인_조회를_취소할_수_있다() {
            let _g = Guard;
            let gate = Gate::default();
            let g2 = gate.clone();
            let m = Mock::start(move |_, r| if path_of(r).ends_with("/myself") { ok(json!({"displayName": "x"})) } else {
                g2.wait();
                ok(json!({}))
            });
            let svc = service();
            connect_server(&svc, &m).await;
            let s2 = svc.clone();
            let task = tokio::spawn(async move { s2.issue(&key("ABC-1"), Some("req-1")).await });
            // 응답이 문에 막힌 요청이 서버에 도착한 뒤 취소한다
            wait_for(|| count(&m, |r| !path_of(r).ends_with("/myself")) >= 1).await;
            svc.cancel("req-1");
            // 문이 닫힌 채로 끝난다 — 응답을 기다리지 않고 취소로 돌아온다
            let e = failure(within(task).await.unwrap());
            assert_eq!((e.code, e.message.as_str()), ("ABORTED", "aborted"));
            gate.open();
            // 없는 id 는 무시한다
            svc.cancel("nope");
        }

        #[tokio::test]
        async fn 서비스는_연결이_바뀌면_진행_중인_조회를_버린다() {
            let _g = Guard;
            let m = Mock::start(|_, r| if path_of(r).ends_with("/myself") { ok(json!({"displayName": "x"})) } else { ok(json!({"key": "ABC-1", "fields": {}})).delay(Duration::from_secs(2)) });
            let svc = service();
            connect_server(&svc, &m).await;
            let (s2, base) = (svc.clone(), m.base());
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(100)).await;
                s2.connect(&server_in(&base)).await.unwrap();
            });
            let e = failure(svc.issue(&key("ABC-1"), None).await);
            assert_eq!(e.code, "ABORTED");
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn 서비스_날짜_필드는_조회가_세대를_잡은_뒤_연결이_바뀌면_옛_자격_증명으로_작업을_시작하거나_캐시하지_않는다() {
            let _g = Guard;
            let old = Mock::start(date_handler(Dates { fields: standard_fields(), issue_fields: json!({"customfield_10015": "2026-10-01"}), ..Default::default() }));
            let new_fields = json!([{"id": "customfield_20001", "name": "Start date", "schema": {"type": "date"}}]);
            let new = Mock::start(date_handler(Dates { fields: new_fields, issue_fields: json!({"customfield_20001": "2026-11-01"}), ..Default::default() }));
            let svc = service_wait(60_000);
            connect_cloud(&svc, &old).await;
            // data() 가 세대를 잡은 뒤 날짜 필드 공유 작업을 잠그기 전에 연결을 바꾼다(두 잠금 사이의 경합)
            let fired = Arc::new(AtomicBool::new(false));
            let (hook_svc, new_base, rt) = (svc.clone(), new.base(), tokio::runtime::Handle::current());
            svc.set_before_date_fields_for_test(move || {
                if !fired.swap(true, Ordering::SeqCst) {
                    tokio::task::block_in_place(|| rt.block_on(hook_svc.connect(&cloud_in(&new_base)))).unwrap();
                }
            });
            let s2 = svc.clone();
            let stale = tokio::spawn(async move { s2.issue(&key("ABC-1"), None).await }).await.unwrap();
            assert_eq!(failure(stale).code, "ABORTED");
            // 새 연결의 조회는 새 사이트의 날짜 필드를 쓴다 — 옛 사이트로 시작한 작업이 새 세대에 캐시돼 있으면 날짜가 비거나 옛 id 를 쓴다
            assert_eq!(j(&svc.issue(&key("ABC-1"), None).await.unwrap())["startDate"], json!("2026-11-01"));
            assert_eq!((field_calls(&old), field_calls(&new)), (0, 1));
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
        async fn 서비스_겹친_connect_와_disconnect_뒤에도_키체인과_메모리의_연결이_같다() {
            let _g = Guard;
            let site = SiteOptions::insecure_loopback_for_test();
            let (a, b) = (Mock::start(|_, _| ok(json!({"displayName": "a"}))), Mock::start(|_, _| ok(json!({"displayName": "b"}))));
            for second_is_disconnect in [false, true] {
                let vault = new_vault();
                let svc = service_on(vault.clone(), Arc::new(SystemClock), 4000);
                // 첫 connect 가 저장한 직후(연결 교체 전)에 멈추고, 두 번째 호출이 교체 잠금 앞까지 온 것을 확인하고서야 놓아 준다.
                // 직렬화되어 있지 않으면 두 번째가 그 사이에 끝까지 실행되어 키체인과 메모리가 어긋난다
                let (arrived, paused) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicBool::new(false)));
                let (arrived2, paused2) = (arrived.clone(), paused.clone());
                svc.set_connect_stage_for_test(move |stage| match stage {
                    "before_lock" => {
                        arrived2.fetch_add(1, Ordering::SeqCst);
                    }
                    "after_save" if !paused2.swap(true, Ordering::SeqCst) => {
                        let t = Instant::now();
                        while arrived2.load(Ordering::SeqCst) < 2 && t.elapsed() < WAIT_LIMIT {
                            std::thread::sleep(Duration::from_millis(5));
                        }
                    }
                    _ => {}
                });
                let (s1, s2, ca, cb) = (svc.clone(), svc.clone(), server_in(&a.base()), server_in(&b.base()));
                let first = tokio::spawn(async move { s1.connect(&ca).await });
                wait_for(|| paused.load(Ordering::SeqCst)).await;
                let second = tokio::spawn(async move {
                    if second_is_disconnect { s2.disconnect().await } else { s2.connect(&cb).await }
                });
                first.await.unwrap().unwrap();
                second.await.unwrap().unwrap();
                let mem = svc.auth();
                let disk = vault.load_jira_with(site).map(|st| st.input.site_url);
                assert_eq!(mem.site_url, disk, "disconnect={second_is_disconnect}: 메모리와 키체인의 연결이 다르다");
                // 나중에 시작한 호출이 이긴다
                assert_eq!(mem.site_url, (!second_is_disconnect).then(|| b.base()));
            }
        }

        #[tokio::test]
        async fn 서비스는_거부된_입력으로_jira_에_요청을_보내지_않는다() {
            let _g = Guard;
            let m = Mock::start(|_, _| ok(json!({"accountId": "a", "displayName": "x"})));
            let svc = service();
            connect_cloud(&svc, &m).await;
            let before = m.requests().len();
            let ok_range = json!({"from": "2026-10-19", "to": "2026-10-20"});
            let bad_users = json!({"from": ok_range["from"], "to": ok_range["to"], "users": ["a\" OR 1=1"]});
            assert_eq!(failure(svc.worklogs(&bad_users, None).await).code, "ERROR");
            assert_eq!(failure(svc.user_search(&json!("a\u{1}"), None).await).code, "ERROR");
            assert_eq!(failure(svc.transition(&key("abc-1"), &json!("31")).await).code, "ERROR");
            assert_eq!(m.requests().len(), before);
        }

        #[tokio::test]
        async fn 서비스_검색과_연결_확인_404_는_각각의_문구이다() {
            let _g = Guard;
            let m = Mock::start(|_, r| if r.target.contains("/myself") { ok(json!({"displayName": "x"})) } else { status(404, json!({})) });
            let svc = service();
            connect_server(&svc, &m).await;
            let e = failure(svc.search(&json!({"jql": "x = 1"}), None).await);
            assert_eq!((e.code, e.message.contains("검색 API")), ("JIRA_NOT_FOUND", true));
            assert!(failure(svc.issue(&key("ABC-1"), None).await).message.contains("이슈를 찾을 수 없"));
            let m2 = Mock::start(|_, _| status(404, json!({})));
            assert!(failure(service().connect(&server_in(&m2.base())).await).message.contains("사이트 주소"));
        }

        #[tokio::test]
        async fn 서비스_댓글은_dc_가_total_로_마지막_페이지를_받아_뒤집고_cloud_는_order_by_를_쓴다() {
            let _g = Guard;
            let all: Vec<Value> = (1..=12).map(|i| json!({"author": {"displayName": format!("u{i}")}, "created": format!("c{i}"), "renderedBody": format!("<p>{i}</p>")})).collect();
            let m = Mock::start(move |_, r| {
                if path_of(r).ends_with("/myself") {
                    return ok(json!({"displayName": "x"}));
                }
                let start: usize = qp(r, "startAt").and_then(|s| s.parse().ok()).unwrap_or(0);
                let max: usize = qp(r, "maxResults").and_then(|s| s.parse().ok()).unwrap_or(50);
                ok(json!({"comments": all.iter().skip(start).take(max).collect::<Vec<_>>(), "total": all.len(), "startAt": start}))
            });
            let svc = service();
            connect_server(&svc, &m).await;
            let list = j(&svc.comments(&key("ABC-1"), None).await.unwrap());
            let names: Vec<&str> = list.as_array().unwrap().iter().map(|c| c["author"].as_str().unwrap()).collect();
            assert_eq!(names, ["u12", "u11", "u10", "u9", "u8"]);
            let calls: Vec<Req> = m.requests().into_iter().filter(|r| r.target.contains("/comment")).collect();
            assert_eq!(calls.iter().map(|r| qp(r, "startAt").unwrap()).collect::<Vec<_>>(), ["0", "7"]);
            assert!(calls.iter().all(|r| !r.target.contains("orderBy")));

            let m2 = Mock::start(|_, r| if r.target.contains("/myself") { ok(json!({"displayName": "x"})) } else { ok(json!({"comments": []})) });
            let cloud = service();
            connect_cloud(&cloud, &m2).await;
            cloud.comments(&key("ABC-1"), None).await.unwrap();
            let r = m2.requests().into_iter().find(|r| r.target.contains("/comment")).unwrap();
            assert_eq!((qp(&r, "orderBy").as_deref(), qp(&r, "expand")), (Some("-created"), None));
        }

        #[tokio::test]
        async fn 서비스_이슈_주소는_요청한_키_기준이다() {
            let _g = Guard;
            let m = Mock::start(|_, r| if r.target.contains("/myself") { ok(json!({"displayName": "x"})) } else { ok(json!({"key": "NEW-9", "fields": {"summary": "s"}})) });
            let svc = service();
            connect_server(&svc, &m).await;
            assert_eq!(j(&svc.issue(&key("OLD-1"), None).await.unwrap())["url"], json!(format!("{}/browse/OLD-1", m.base())));
        }

        // ── 날짜 필드 ───────────────────────────────────────────────────────

        #[derive(Clone, Default)]
        struct Dates {
            fields: Value,
            field_status: u16,
            /// 있으면 /field 응답을 이 문이 열릴 때까지 붙든다
            field_gate: Option<Gate>,
            field_huge: bool,
            issue_fields: Value,
            by_key: Value,
        }

        fn date_handler(o: Dates) -> impl Fn(usize, &Req) -> Resp + Send + Sync + 'static {
            move |_, r| {
                let path = path_of(r);
                if path.ends_with("/myself") {
                    return ok(json!({"displayName": "x"}));
                }
                if path.ends_with("/field") {
                    if let Some(g) = &o.field_gate {
                        g.wait();
                    }
                    if o.field_huge {
                        return ok(json!([{"id": "customfield_1", "name": "Start date", "pad": "x".repeat(6 * 1024 * 1024)}]));
                    }
                    return if o.field_status != 0 && o.field_status != 200 { status(o.field_status, json!({"errorMessages": ["x"]})) } else { ok(if o.fields.is_null() { json!([]) } else { o.fields.clone() }) };
                }
                if let Some(k) = path.rsplit('/').next().filter(|k| path.contains("/issue/") && k.starts_with("ABC-")) {
                    let mut f = json!({"summary": "s"});
                    let extra = if o.by_key.get(k).is_some() { &o.by_key[k] } else { &o.issue_fields };
                    if let Some(e) = extra.as_object() {
                        f.as_object_mut().unwrap().extend(e.clone());
                    }
                    return ok(json!({"key": k, "fields": f}));
                }
                status(404, json!({}))
            }
        }

        fn field_calls(m: &Mock) -> usize {
            count(m, |r| path_of(r).ends_with("/field"))
        }

        fn fields_param(m: &Mock) -> Vec<String> {
            let r = m.requests().into_iter().rev().find(|r| r.target.contains("/issue/ABC-1?")).expect("이슈 요청");
            qp(&r, "fields").unwrap().split(',').map(str::to_string).collect()
        }

        fn standard_fields() -> Value {
            json!([
                {"id": "customfield_10015", "name": "Start date", "schema": {"type": "date"}},
                {"id": "customfield_10016", "name": "End date", "schema": {"type": "date"}},
                {"id": "duedate", "name": "Due date"}
            ])
        }

        #[tokio::test]
        async fn 서비스_날짜_필드가_대기_상한을_넘긴_채로_재연결해도_새_연결에서_다시_부른다() {
            let _g = Guard;
            let gate = Gate::default();
            let m = Mock::start(date_handler(Dates { fields: standard_fields(), field_gate: Some(gate.clone()), issue_fields: json!({"customfield_10015": "2026-10-01"}), ..Default::default() }));
            let svc = service_wait(10);
            connect_cloud(&svc, &m).await;
            // /field 응답을 붙들어 둔 채라 10ms 상한을 넘겨 날짜 없이 돌아온다
            assert_eq!(j(&svc.issue(&key("ABC-1"), None).await.unwrap())["startDate"], json!(""));
            wait_for(|| field_calls(&m) == 1).await;
            // 응답을 놓아 첫 연결의 공유 작업이 끝나 캐시될 때까지 기다린 뒤 재연결한다
            gate.open();
            start_date_eventually(&svc, "2026-10-01").await;
            connect_cloud(&svc, &m).await;
            start_date_eventually(&svc, "2026-10-01").await;
            assert_eq!(field_calls(&m), 2);
        }

        /// 공유 /field 작업이 끝나 캐시될 때까지(날짜가 나올 때까지) 이슈 조회를 되풀이한다
        async fn start_date_eventually(svc: &JiraService, want: &str) {
            let t = Instant::now();
            loop {
                if j(&svc.issue(&key("ABC-1"), None).await.unwrap())["startDate"] == json!(want) {
                    return;
                }
                assert!(t.elapsed() < WAIT_LIMIT, "날짜가 나오지 않았다");
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }

        #[tokio::test]
        async fn 서비스_날짜_필드는_id_를_찾아_fields_에_싣고_값은_날짜_부분만_돌려준다() {
            let _g = Guard;
            let m = Mock::start(date_handler(Dates {
                fields: standard_fields(),
                issue_fields: json!({"duedate": "2026-10-22", "customfield_10015": "2026-10-01T09:00:00.000+0900", "customfield_10016": "2026/10/30"}),
                ..Default::default()
            }));
            let svc = service();
            connect_cloud(&svc, &m).await;
            let d = j(&svc.issue(&key("ABC-1"), None).await.unwrap());
            let f = fields_param(&m);
            assert!(["duedate", "customfield_10015", "customfield_10016"].iter().all(|x| f.iter().any(|y| y == x)), "{f:?}");
            assert!(m.requests().iter().any(|r| r.target == "/rest/api/3/field"));
            assert_eq!((d["dueDate"].clone(), d["startDate"].clone(), d["endDate"].clone()), (json!("2026-10-22"), json!("2026-10-01"), json!("")));
        }

        #[tokio::test]
        async fn 서비스_날짜_필드가_404_403_이어도_이슈_조회는_성공하고_해당_날짜만_빈다() {
            let _g = Guard;
            for st in [404u16, 403] {
                let m = Mock::start(date_handler(Dates { field_status: st, issue_fields: json!({"duedate": "2026-10-22", "customfield_10015": "2026-10-01"}), ..Default::default() }));
                let svc = service();
                connect_cloud(&svc, &m).await;
                let d = j(&svc.issue(&key("ABC-1"), None).await.unwrap());
                assert_eq!((d["key"].clone(), d["dueDate"].clone(), d["startDate"].clone(), d["endDate"].clone()), (json!("ABC-1"), json!("2026-10-22"), json!(""), json!("")));
                assert_eq!(fields_param(&m), ["summary", "status", "issuetype", "priority", "assignee", "updated", "duedate", "reporter", "created", "description"]);
            }
        }

        #[tokio::test]
        async fn 서비스_날짜_필드는_연결_동안_한_번만_부르고_재연결하면_다시_부른다() {
            let _g = Guard;
            let m = Mock::start(date_handler(Dates { fields: standard_fields(), ..Default::default() }));
            // 이슈가 /field 를 끝까지 기다리게 해 부하와 무관하게 호출 수를 센다
            let svc = service_wait(60_000);
            connect_cloud(&svc, &m).await;
            let k = key("ABC-1");
            let (a, b) = tokio::join!(svc.issue(&k, None), svc.issue(&k, None)); // 동시 호출은 같은 작업
            a.unwrap();
            b.unwrap();
            svc.issue(&key("ABC-1"), None).await.unwrap();
            assert_eq!(field_calls(&m), 1);
            connect_cloud(&svc, &m).await;
            svc.issue(&key("ABC-1"), None).await.unwrap();
            assert_eq!(field_calls(&m), 2);
        }

        #[tokio::test]
        async fn 서비스_날짜_필드_실패는_캐시하지_않지만_쿨다운_동안은_다시_묻지_않고_재연결하면_다시_시도한다() {
            let _g = Guard;
            let m = Mock::start(date_handler(Dates { field_status: 404, ..Default::default() }));
            let svc = service_wait(60_000);
            connect_cloud(&svc, &m).await;
            svc.issue(&key("ABC-1"), None).await.unwrap();
            svc.issue(&key("ABC-1"), None).await.unwrap();
            assert_eq!(field_calls(&m), 1);
            connect_cloud(&svc, &m).await;
            svc.issue(&key("ABC-1"), None).await.unwrap();
            assert_eq!(field_calls(&m), 2);
        }

        #[tokio::test]
        async fn 서비스_날짜_필드_쿨다운은_주입한_시계로_5분이고_소켓이_필요_없다() {
            // 닫힌 포트(연결 거부)라 모든 요청이 곧바로 실패한다. 시계는 가짜라 5분을 기다리지 않는다
            let _g = Guard;
            let closed = format!("http://{}", closed_addr());
            let vault = new_vault();
            vault.save_jira(&parse_connect_input(&server_in(&closed), SiteOptions::insecure_loopback_for_test()).unwrap(), "x").unwrap();
            let clock = FakeClock::at(1_000);
            let svc = service_on(vault, clock.clone(), 60_000);
            assert!(svc.auth().connected, "저장된 연결을 복원한다");
            let ask = || async {
                let e = failure(svc.issue(&key("ABC-1"), None).await);
                assert_eq!(e.code, "JIRA_NETWORK");
                // 공유 /field 작업이 끝나 쿨다운이 기록될 때까지 기다린다
            };
            ask().await;
            wait_for(|| svc.date_fields_retry_at() > 0.0).await;
            assert_eq!(svc.date_fields_retry_at(), 301_000.0);
            clock.set(2_000);
            ask().await;
            assert_eq!(svc.date_fields_retry_at(), 301_000.0, "쿨다운 안에서는 다시 묻지 않는다");
            clock.set(301_001);
            ask().await;
            wait_for(|| svc.date_fields_retry_at() == 601_001.0).await;
        }

        #[tokio::test]
        async fn 서비스_날짜_필드는_server_에서_api_v2_를_쓴다() {
            let _g = Guard;
            let m = Mock::start(date_handler(Dates { fields: standard_fields(), issue_fields: json!({"customfield_10016": "2026-11-02"}), ..Default::default() }));
            let svc = service_wait(60_000);
            connect_server(&svc, &m).await;
            assert_eq!(j(&svc.issue(&key("ABC-1"), None).await.unwrap())["endDate"], json!("2026-11-02"));
            assert!(m.requests().iter().any(|r| r.target == "/rest/api/2/field"));
        }

        #[tokio::test]
        async fn 서비스_날짜_필드는_이상한_id_를_버리고_요청에_싣지_않는다() {
            let _g = Guard;
            let m = Mock::start(date_handler(Dates {
                fields: json!([
                    {"id": "customfield_1; drop", "name": "Start date", "schema": {"type": "date"}},
                    {"id": "summary", "name": "End date", "schema": {"type": "date"}},
                    {"id": "customfield_20", "name": "Target start"}
                ]),
                ..Default::default()
            }));
            let svc = service_wait(60_000);
            connect_cloud(&svc, &m).await;
            svc.issue(&key("ABC-1"), None).await.unwrap();
            let f = fields_param(&m);
            assert!(f.contains(&"customfield_20".to_string()));
            assert!(!f.iter().any(|x| x.contains("drop") || x.contains(';')));
            assert_eq!(f.iter().filter(|x| *x == "summary").count(), 1);
        }

        #[tokio::test]
        async fn 서비스_날짜_필드는_동명_필드가_여럿이면_date_형식을_먼저_두고_이름은_별칭도_받는다() {
            let _g = Guard;
            let m = Mock::start(date_handler(Dates {
                fields: json!([
                    {"id": "customfield_1", "name": "start date", "schema": {"type": "string"}},
                    {"id": "customfield_2", "name": "Start Date", "schema": {"type": "datetime"}},
                    {"id": "customfield_3", "name": "종료일", "schema": {"type": "string"}},
                    {"id": "customfield_4", "name": "종료 날짜"}
                ]),
                ..Default::default()
            }));
            let svc = service_wait(60_000);
            connect_cloud(&svc, &m).await;
            svc.issue(&key("ABC-1"), None).await.unwrap();
            let custom: Vec<String> = fields_param(&m).into_iter().filter(|x| x.starts_with("customfield_")).collect();
            assert_eq!(custom, ["customfield_2", "customfield_1", "customfield_3", "customfield_4"]);
        }

        #[tokio::test]
        async fn 서비스_날짜_필드는_scope_가_다른_동명_필드_중_이슈에_값이_있는_쪽이_나온다() {
            let _g = Guard;
            let m = Mock::start(date_handler(Dates {
                fields: json!([
                    {"id": "customfield_11", "name": "End date", "schema": {"type": "date"}, "scope": {"type": "PROJECT", "project": {"id": "1"}}},
                    {"id": "customfield_12", "name": "End date", "schema": {"type": "date"}, "scope": {"type": "PROJECT", "project": {"id": "2"}}}
                ]),
                by_key: json!({"ABC-1": {"customfield_11": "2026-12-01", "customfield_12": null}, "ABC-2": {"customfield_11": null, "customfield_12": "2026-12-02"}}),
                ..Default::default()
            }));
            let svc = service_wait(60_000);
            connect_cloud(&svc, &m).await;
            assert_eq!(j(&svc.issue(&key("ABC-1"), None).await.unwrap())["endDate"], json!("2026-12-01"));
            assert_eq!(j(&svc.issue(&key("ABC-2"), None).await.unwrap())["endDate"], json!("2026-12-02"));
        }

        #[tokio::test]
        async fn 서비스_날짜_필드는_start_date_가_target_start_보다_우선이고_없으면_다음_후보로_넘어간다() {
            let _g = Guard;
            let m = Mock::start(date_handler(Dates {
                fields: json!([
                    {"id": "customfield_21", "name": "Target start", "schema": {"type": "date"}},
                    {"id": "customfield_22", "name": "Start date", "schema": {"type": "date"}}
                ]),
                by_key: json!({"ABC-1": {"customfield_21": "2026-01-01", "customfield_22": "2026-02-02"}, "ABC-2": {"customfield_21": "2026-03-03"}}),
                ..Default::default()
            }));
            let svc = service_wait(60_000);
            connect_cloud(&svc, &m).await;
            assert_eq!(j(&svc.issue(&key("ABC-1"), None).await.unwrap())["startDate"], json!("2026-02-02"));
            assert_eq!(j(&svc.issue(&key("ABC-2"), None).await.unwrap())["startDate"], json!("2026-03-03"));
        }

        #[test]
        fn 서비스_날짜_필드는_같은_이름_안에서_전역_필드가_scoped_보다_먼저고_후보는_8개까지이다() {
            let list = json::parse(
                r#"[{"id":"customfield_31","name":"Start date","schema":{"type":"date"},"scope":{"type":"PROJECT"}},{"id":"customfield_32","name":"Start date","schema":{"type":"date"}}]"#,
            )
            .unwrap();
            assert_eq!(jira_map::pick_date_fields(&list).start, ["customfield_32", "customfield_31"]);
            let many: Vec<Value> = (0..12).map(|i| json!({"id": format!("customfield_{}", 100 + i), "name": "Start date", "schema": {"type": "date"}})).collect();
            let picked = jira_map::pick_date_fields(&json::parse(&Value::Array(many.clone()).to_string()).unwrap()).start;
            assert_eq!(picked, many.iter().take(8).map(|f| f["id"].as_str().unwrap().to_string()).collect::<Vec<_>>());
            assert_eq!(jira_map::MAX_DATE_FIELD_CANDIDATES, 8);
        }

        #[tokio::test]
        async fn 서비스_날짜_필드_후보는_요청에_최대_8개만_싣는다() {
            let _g = Guard;
            let many: Vec<Value> = (0..12).map(|i| json!({"id": format!("customfield_{}", 100 + i), "name": "Start date", "schema": {"type": "date"}})).collect();
            let m = Mock::start(date_handler(Dates { fields: Value::Array(many), ..Default::default() }));
            let svc = service_wait(60_000);
            connect_cloud(&svc, &m).await;
            svc.issue(&key("ABC-1"), None).await.unwrap();
            assert_eq!(fields_param(&m).iter().filter(|x| x.starts_with("customfield_")).count(), 8);
        }

        #[tokio::test]
        async fn 서비스_날짜_필드가_늦으면_이슈는_날짜_없이_상한_안에_돌아오고_끝난_뒤_조회에는_날짜가_나온다() {
            let _g = Guard;
            let gate = Gate::default();
            let m = Mock::start(date_handler(Dates { fields: standard_fields(), field_gate: Some(gate.clone()), issue_fields: json!({"customfield_10015": "2026-10-01"}), ..Default::default() }));
            let svc = service_wait(50);
            connect_cloud(&svc, &m).await;
            // /field 응답이 아직 오지 않은 채(문이 닫혀 있다) 대기 상한(50ms)에서 날짜 없이 돌아온다
            assert_eq!(j(&within(svc.issue(&key("ABC-1"), None)).await.unwrap())["startDate"], json!(""));
            gate.open();
            start_date_eventually(&svc, "2026-10-01").await; // 공유 /field 요청이 끝나 캐시될 때까지
            assert_eq!(field_calls(&m), 1);
        }

        #[tokio::test]
        async fn 서비스_날짜_필드_대기_중_개별_취소는_즉시_거절되고_다른_조회는_날짜를_받는다() {
            let _g = Guard;
            let gate = Gate::default();
            let m = Mock::start(date_handler(Dates { fields: standard_fields(), field_gate: Some(gate.clone()), issue_fields: json!({"customfield_10015": "2026-10-01"}), ..Default::default() }));
            let svc = service_wait(60_000);
            connect_cloud(&svc, &m).await;
            let (sa, sb) = (svc.clone(), svc.clone());
            let a = tokio::spawn(async move { sa.issue(&key("ABC-1"), Some("req-a")).await });
            wait_for(|| field_calls(&m) == 1).await; // a 가 공유 /field 를 시작했고 응답은 문에 막혀 있다
            let b = tokio::spawn(async move { sb.issue(&key("ABC-1"), Some("req-b")).await });
            svc.cancel("req-a");
            // 문이 닫힌 채로 a 가 곧바로 거절된다(응답을 기다리지 않는다)
            assert_eq!(failure(within(a).await.unwrap()).code, "ABORTED");
            gate.open();
            assert_eq!(j(&within(b).await.unwrap().unwrap())["startDate"], json!("2026-10-01"));
            assert_eq!(field_calls(&m), 1);
        }

        #[tokio::test]
        async fn 서비스_날짜_필드_응답이_너무_크면_연결_동안_다시_묻지_않는다() {
            let _g = Guard;
            let m = Mock::start(date_handler(Dates { field_huge: true, ..Default::default() }));
            let svc = service_wait(60_000);
            connect_cloud(&svc, &m).await;
            assert_eq!(j(&svc.issue(&key("ABC-1"), None).await.unwrap())["startDate"], json!(""));
            svc.issue(&key("ABC-1"), None).await.unwrap();
            assert_eq!(field_calls(&m), 1);
            assert!(!fields_param(&m).contains(&"customfield_1".to_string()));
        }

        // ── 상태 전환 ───────────────────────────────────────────────────────

        fn transitions_handler(_n: usize, r: &Req) -> Resp {
            if r.target.ends_with("/myself") {
                return ok(json!({"displayName": "홍길동"}));
            }
            if path_of(r).ends_with("/issue/ABC-1/transitions") {
                if r.method == "POST" {
                    return Resp::new(204);
                }
                return ok(json!({"transitions": [
                    {"id": "11", "name": "진행 시작", "to": {"name": "진행 중", "statusCategory": {"key": "indeterminate"}}},
                    {"id": "31", "name": "완료", "to": {"name": "완료됨", "statusCategory": {"key": "done"}}},
                    {"name": "id 없음", "to": {"name": "x"}}
                ]}));
            }
            status(404, json!({}))
        }

        #[tokio::test]
        async fn 서비스_전환_목록을_매핑하고_전환을_post_한다() {
            let _g = Guard;
            for (cloud, base) in [(true, "/rest/api/3"), (false, "/rest/api/2")] {
                let m = Mock::start(transitions_handler);
                let svc = service();
                if cloud { connect_cloud(&svc, &m).await } else { connect_server(&svc, &m).await }
                assert_eq!(
                    j(&svc.transitions(&key("ABC-1"), None).await.unwrap()),
                    json!([{"id": "11", "name": "진행 시작", "to": {"name": "진행 중", "category": "indeterminate"}}, {"id": "31", "name": "완료", "to": {"name": "완료됨", "category": "done"}}])
                );
                svc.transition(&key("ABC-1"), &json!("31")).await.unwrap();
                let post = m.requests().into_iter().find(|r| r.method == "POST").unwrap();
                assert_eq!(post.target, format!("{base}/issue/ABC-1/transitions"));
                assert_eq!(serde_json::from_slice::<Value>(&post.body).unwrap(), json!({"transition": {"id": "31"}}));
                assert_eq!((post.header("x-atlassian-token"), post.header("content-type")), (Some("no-check"), Some("application/json")));
            }
        }

        #[tokio::test]
        async fn 서비스_전환은_잘못된_키_전환_id_를_요청_없이_거부한다() {
            let _g = Guard;
            let m = Mock::start(transitions_handler);
            let svc = service();
            connect_server(&svc, &m).await;
            let before = m.requests().len();
            for (k, id) in [(json!("abc-1"), json!("31")), (json!("ABC-1"), json!("../31")), (json!("ABC-1"), json!("")), (json!("ABC-1"), json!("12345678901")), (json!("ABC-1"), json!(31))] {
                assert_eq!(failure(svc.transition(&k, &id).await).code, "ERROR", "{k} {id}");
            }
            assert_eq!(failure(svc.transitions(&json!("x/../y"), None).await).code, "ERROR");
            assert_eq!(m.requests().len(), before);
        }

        #[tokio::test]
        async fn 서비스_전환은_400_이면_오류로_알리고_필드별_오류의_제어문자는_뺀다() {
            let _g = Guard;
            // 본문은 글자 그대로 보낸다(serde_json 의 객체는 키를 정렬하지만 Jira 응답의 키 순서가 메시지 순서다)
            let bodies = [r#"{"errorMessages":["전환할 수 없습니다"]}"#, r#"{"errorMessages":[],"errors":{"resolution":"해결 사유는\n필수입니다","assignee":"담당자가 필요합니다"}}"#];
            let mut messages = Vec::new();
            for body in bodies {
                let m = Mock::start(move |n, r| if r.method == "POST" { Resp::new(400).json(body) } else { transitions_handler(n, r) });
                let svc = service();
                connect_server(&svc, &m).await;
                messages.push(failure(svc.transition(&key("ABC-1"), &json!("31")).await).message);
            }
            assert!(messages[0].contains("전환할 수 없습니다"), "{}", messages[0]);
            assert_eq!(messages[1], "Jira API 오류(400): 해결 사유는 필수입니다 / 담당자가 필요합니다");
        }

        #[tokio::test]
        async fn 서비스_전환은_요청_id_로_취소되지_않고_429_도_다시_보내지_않는다() {
            let _g = Guard;
            // 취소 시도: transition 은 requestId 를 받지 않는다. 같은 id 로 cancel 을 불러도 끝까지 간다
            let m = Mock::start(|n, r| if r.method == "POST" { Resp::new(204).delay(Duration::from_millis(300)) } else { transitions_handler(n, r) });
            let svc = service();
            connect_server(&svc, &m).await;
            let s2 = svc.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(80)).await;
                s2.cancel("tr-1");
            });
            svc.transition(&key("ABC-1"), &json!("31")).await.unwrap();

            // 429(Retry-After 0 이어도) POST 는 한 번만 보낸다
            let m2 = Mock::start(|n, r| if r.method == "POST" { Resp::new(429).header("Retry-After", "0").json("{}") } else { transitions_handler(n, r) });
            let svc2 = service();
            connect_server(&svc2, &m2).await;
            let e = failure(svc2.transition(&key("ABC-1"), &json!("31")).await);
            assert_eq!(e.code, "JIRA_RATE_LIMIT");
            assert_eq!(count(&m2, |r| r.method == "POST"), 1);
        }

        // ── 워크로그·사용자 검색 ────────────────────────────────────────────

        #[derive(Clone)]
        struct Wl {
            id: String,
            author: String,
            started: String,
            author_key: Option<String>,
            comment: Option<Value>,
        }

        fn wl(id: &str, author: &str, started: &str) -> Wl {
            Wl { id: id.into(), author: author.into(), started: started.into(), author_key: None, comment: None }
        }

        #[derive(Clone)]
        struct Fi {
            key: String,
            worklogs: Vec<Wl>,
            embed: Option<usize>,
            status: Option<u16>,
            huge: bool,
            /// 있으면 이 이슈의 worklog 응답을 문이 열릴 때까지 붙든다
            gate: Option<Gate>,
        }

        fn fi(key: &str, worklogs: Vec<Wl>) -> Fi {
            Fi { key: key.into(), worklogs, embed: None, status: None, huge: false, gate: None }
        }

        #[derive(Clone)]
        struct Site {
            cloud: bool,
            issues: Vec<Fi>,
            bad_users: Vec<String>,
            users: Value,
        }

        fn site(cloud: bool, issues: Vec<Fi>) -> Site {
            Site { cloud, issues, bad_users: vec![], users: json!([]) }
        }

        fn to_worklog(cloud: bool, w: &Wl) -> Value {
            let author = if cloud { json!({"accountId": w.author, "displayName": format!("표시-{}", w.author)}) } else { json!({"name": w.author, "key": w.author_key.clone().unwrap_or(w.author.clone()), "displayName": format!("표시-{}", w.author)}) };
            let mut o = json!({"id": w.id, "author": author, "started": w.started, "timeSpentSeconds": 3600});
            if let Some(c) = &w.comment {
                o["comment"] = c.clone();
            }
            o
        }

        fn worklog_handler(site: Site) -> impl Fn(usize, &Req) -> Resp + Send + Sync + 'static {
            move |_, r| {
                let (cloud, path) = (site.cloud, path_of(r));
                if path.ends_with("/myself") {
                    return ok(if cloud { json!({"accountId": "acc-me", "displayName": "나"}) } else { json!({"name": "me", "key": "JIRAUSER1", "displayName": "나"}) });
                }
                if path.ends_with("/user/search") {
                    return ok(site.users.clone());
                }
                if let Some(rest) = path.split("/issue/").nth(1).and_then(|p| p.strip_suffix("/worklog")) {
                    let Some(issue) = site.issues.iter().find(|i| i.key == rest) else { return status(404, json!({})) };
                    if let Some(g) = &issue.gate {
                        g.wait();
                    }
                    if let Some(st) = issue.status {
                        return status(st, json!({"errorMessages": ["x"]}));
                    }
                    if issue.huge {
                        return ok(json!({"worklogs": [], "pad": "x".repeat(21 * 1024 * 1024)}));
                    }
                    if cloud {
                        let num = |k: &str, d: f64| qp(r, k).and_then(|v| v.parse::<f64>().ok()).unwrap_or(d);
                        let (after, before) = (num("startedAfter", f64::NEG_INFINITY), num("startedBefore", f64::INFINITY));
                        let list: Vec<&Wl> = issue.worklogs.iter().filter(|w| parse_date_ms(&w.started).is_some_and(|t| t >= after && t < before)).collect();
                        let (start, max) = (num("startAt", 0.0) as usize, num("maxResults", 5000.0) as usize);
                        return ok(json!({"startAt": start, "maxResults": max, "total": list.len(), "worklogs": list.iter().skip(start).take(max).map(|w| to_worklog(true, w)).collect::<Vec<_>>()}));
                    }
                    return ok(json!({"worklogs": issue.worklogs.iter().map(|w| to_worklog(false, w)).collect::<Vec<_>>()}));
                }
                if path.ends_with("/search/jql") || path.ends_with("/search") {
                    let jql = qp(r, "jql").unwrap_or_default();
                    if site.bad_users.iter().any(|b| jql.contains(&format!("\"{b}\""))) {
                        return status(400, json!({"errorMessages": ["사용자가 없습니다"]}));
                    }
                    let max: usize = qp(r, "maxResults").and_then(|v| v.parse().ok()).unwrap_or(50);
                    let start: usize = qp(r, if cloud { "nextPageToken" } else { "startAt" }).and_then(|v| v.parse().ok()).unwrap_or(0);
                    let page: Vec<&Fi> = site.issues.iter().skip(start).take(max).collect();
                    let issues: Vec<Value> = page
                        .iter()
                        .map(|i| {
                            let embed = i.embed.unwrap_or(i.worklogs.len());
                            json!({"key": i.key, "fields": {"summary": format!("요약 {}", i.key), "issuetype": {"name": "Task"},
                                "worklog": {"startAt": 0, "maxResults": embed, "total": i.worklogs.len(), "worklogs": i.worklogs.iter().take(embed).map(|w| to_worklog(cloud, w)).collect::<Vec<_>>()}}})
                        })
                        .collect();
                    let end = start + page.len();
                    return ok(if cloud {
                        if end < site.issues.len() { json!({"issues": issues, "nextPageToken": end.to_string()}) } else { json!({"issues": issues, "isLast": true}) }
                    } else {
                        json!({"issues": issues, "startAt": start, "total": site.issues.len()})
                    });
                }
                status(404, json!({"errorMessages": ["nf"]}))
            }
        }

        async fn connected(s: Site) -> (JiraService, Mock) {
            let cloud = s.cloud;
            let m = Mock::start(worklog_handler(s));
            let svc = service();
            if cloud { connect_cloud(&svc, &m).await } else { connect_server(&svc, &m).await }
            (svc, m)
        }

        fn q() -> Value {
            json!({"from": "2026-10-19", "to": "2026-10-20", "users": []})
        }

        fn q_with(users: &[&str], include_me: Option<bool>) -> Value {
            let mut v = q();
            v["users"] = json!(users);
            if let Some(b) = include_me {
                v["includeMe"] = json!(b);
            }
            v
        }

        fn ids(r: &Json) -> Vec<String> {
            j(r)["worklogs"].as_array().unwrap().iter().map(|w| w["id"].as_str().unwrap().to_string()).collect()
        }

        fn jql_of(r: &Req) -> String {
            qp(r, "jql").unwrap_or_default()
        }

        fn worklog_reqs(m: &Mock) -> Vec<Req> {
            m.requests().into_iter().filter(|r| path_of(r).contains("/issue/") && path_of(r).ends_with("/worklog")).collect()
        }

        const T: &str = "2026-10-19T09:00:00.000+0900";

        #[tokio::test]
        async fn 서비스_워크로그_cloud_는_내장_worklog_로_충분하면_다시_받지_않고_나만_범위_안만_남긴다() {
            let _g = Guard;
            let mut first = wl("1", "acc-me", T);
            first.comment = Some(json!({"type": "doc", "version": 1, "content": [{"type": "paragraph", "content": [{"type": "text", "text": "작업"}]}, {"type": "paragraph", "content": [{"type": "text", "text": "둘째"}]}]}));
            let (svc, m) = connected(site(true, vec![fi("ABC-1", vec![first, wl("2", "acc-other", T), wl("3", "acc-me", "2026-10-10T09:00:00.000+0900"), wl("4", "acc-me", "2026-10-21T23:00:00.000+0900")])])).await;
            let r = svc.worklogs(&q(), None).await.unwrap();
            let v = j(&r);
            assert_eq!(v["me"], json!({"id": "acc-me", "name": "나"}));
            assert_eq!(v["truncated"], json!(false));
            assert_eq!(ids(&r), ["1", "4"]); // 4 는 로컬 날짜 필터가 화면 몫인 여유분
            assert_eq!(
                v["worklogs"][0],
                json!({"id": "1", "issueKey": "ABC-1", "issueSummary": "요약 ABC-1", "issueType": "Task", "authorId": "acc-me", "author": "표시-acc-me", "started": T, "seconds": 3600, "comment": "작업 둘째"})
            );
            let search = m.requests().into_iter().find(|x| x.target.starts_with("/rest/api/3/search/jql")).unwrap();
            assert_eq!(jql_of(&search), "worklogAuthor = currentUser() AND worklogDate >= \"2026-10-17\" AND worklogDate < \"2026-10-22\" ORDER BY key ASC");
            assert_eq!((qp(&search, "fields").as_deref(), qp(&search, "maxResults").as_deref()), (Some("summary,issuetype,worklog"), Some("25")));
            assert!(worklog_reqs(&m).is_empty());
            assert!(m.requests().iter().all(|x| x.method == "GET"));
        }

        #[tokio::test]
        async fn 서비스_워크로그_cloud_는_내장_worklog_가_total_보다_적으면_그_이슈만_범위를_주어_다시_받는다() {
            let _g = Guard;
            let t = "2026-10-20T10:00:00.000+0900";
            let mk = |n: usize, p: &str| (0..n).map(|i| wl(&format!("{p}{i}"), "acc-x", t)).collect::<Vec<_>>();
            let mut i1 = fi("ABC-1", mk(3, "w"));
            i1.embed = Some(1);
            let mut far = vec![wl("far", "acc-x", "2026-01-01T10:00:00.000+0900")];
            far.extend(mk(2, "nw"));
            let mut i3 = fi("ABC-3", far);
            i3.embed = Some(0);
            let (svc, m) = connected(site(true, vec![i1, fi("ABC-2", vec![wl("z", "acc-x", t)]), i3])).await;
            let r = svc.worklogs(&q_with(&["acc-x", "acc-y"], None), None).await.unwrap();
            let mut got = ids(&r);
            got.sort();
            assert_eq!(got, ["nw0", "nw1", "w0", "w1", "w2", "z"].iter().map(|s| s.to_string()).collect::<Vec<_>>());
            let reqs = worklog_reqs(&m);
            let mut paths: Vec<&str> = reqs.iter().map(path_of).collect();
            paths.sort_unstable();
            assert_eq!(paths, ["/rest/api/3/issue/ABC-1/worklog", "/rest/api/3/issue/ABC-3/worklog"]);
            assert_eq!(qp(&reqs[0], "startedAfter").unwrap().parse::<f64>().unwrap(), parse_date_ms("2026-10-17T00:00:00Z").unwrap());
            assert_eq!(qp(&reqs[0], "startedBefore").unwrap().parse::<f64>().unwrap(), parse_date_ms("2026-10-22T14:00:00Z").unwrap());
            let search = m.requests().into_iter().find(|x| x.target.starts_with("/rest/api/3/search/jql")).unwrap();
            assert!(jql_of(&search).contains("worklogAuthor in (\"acc-x\",\"acc-y\")"));
        }

        #[tokio::test]
        async fn 서비스_워크로그_server_는_name_과_key_둘_다로_작성자를_맞추고_전체를_클라이언트에서_거른다() {
            let _g = Guard;
            let mut bob = wl("2", "bob", T);
            bob.author_key = Some("JIRAUSER9".into());
            let mut first = wl("1", "alice", T);
            first.comment = Some(json!("평문 설명"));
            let mut i = fi("DC-1", vec![first, bob, wl("3", "carol", T), wl("4", "alice", "2026-09-01T09:00:00.000+0900")]);
            i.embed = Some(0);
            let (svc, m) = connected(site(false, vec![i])).await;
            let r = svc.worklogs(&q_with(&["alice", "JIRAUSER9"], None), None).await.unwrap();
            let v = j(&r);
            assert_eq!(v["me"], json!({"id": "me", "name": "나"}));
            let rows: Vec<Value> = v["worklogs"].as_array().unwrap().iter().map(|w| json!([w["id"], w["authorId"], w["comment"]])).collect();
            assert_eq!(rows, [json!(["1", "alice", "평문 설명"]), json!(["2", "bob", ""])]);
            assert_eq!(worklog_reqs(&m).iter().map(|r| r.target.clone()).collect::<Vec<_>>(), ["/rest/api/2/issue/DC-1/worklog"]);
            let search = m.requests().into_iter().find(|x| x.target.starts_with("/rest/api/2/search")).unwrap();
            assert!(jql_of(&search).contains("worklogAuthor in (\"alice\",\"JIRAUSER9\")"));
        }

        #[tokio::test]
        async fn 서비스_워크로그는_사용자를_비우면_server_도_me_id_로_거른다() {
            let _g = Guard;
            let (svc, _m) = connected(site(false, vec![fi("DC-1", vec![wl("1", "me", T), wl("2", "alice", T)])])).await;
            assert_eq!(ids(&svc.worklogs(&q(), None).await.unwrap()), ["1"]);
        }

        #[tokio::test]
        async fn 서비스_워크로그의_내_id_는_연결_동안_캐시하고_재연결하면_다시_읽는다() {
            let _g = Guard;
            let (svc, m) = connected(site(true, vec![])).await;
            let myself = |m: &Mock| count(m, |x| x.target.ends_with("/myself"));
            let base = myself(&m);
            svc.worklogs(&q(), None).await.unwrap();
            svc.worklogs(&q(), None).await.unwrap();
            assert_eq!(myself(&m), base + 1);
            connect_cloud(&svc, &m).await;
            let after = myself(&m);
            svc.worklogs(&q(), None).await.unwrap();
            assert_eq!(myself(&m), after + 1);
        }

        #[tokio::test]
        async fn 서비스_워크로그는_이슈가_300건을_넘으면_300건에서_자르고_truncated_로_알린다() {
            let _g = Guard;
            for cloud in [true, false] {
                let author = if cloud { "acc-me" } else { "me" };
                let issues: Vec<Fi> = (0..350).map(|i| fi(&format!("ABC-{}", i + 1), vec![wl(&format!("w{i}"), author, T)])).collect();
                let (svc, _m) = connected(site(cloud, issues)).await;
                let r = svc.worklogs(&q(), None).await.unwrap();
                assert_eq!(ids(&r).len(), 300);
                assert_eq!(j(&r)["truncated"], json!(true));
            }
        }

        #[tokio::test]
        async fn 서비스_워크로그_include_me_는_팀원_jql_과_필터에_내_id_를_더하고_팀원이_없으면_무시한다() {
            let _g = Guard;
            let (svc, m) = connected(site(true, vec![fi("ABC-1", vec![wl("1", "acc-me", T), wl("2", "acc-x", T), wl("3", "acc-z", T)])])).await;
            let r = svc.worklogs(&q_with(&["acc-x"], Some(true)), None).await.unwrap();
            assert_eq!(ids(&r), ["1", "2"]);
            let searches = || m.requests().into_iter().filter(|x| x.target.starts_with("/rest/api/3/search/jql")).collect::<Vec<_>>();
            assert!(jql_of(&searches()[0]).contains("worklogAuthor in (\"acc-x\",\"acc-me\")"));
            let none = svc.worklogs(&q_with(&[], Some(true)), None).await.unwrap();
            assert_eq!(ids(&none), ["1"]);
            assert!(jql_of(searches().last().unwrap()).contains("worklogAuthor = currentUser()"));
        }

        #[tokio::test]
        async fn 서비스_워크로그_include_me_는_내_id_가_jql_에_못_들어가면_current_user_와_or_로_묶는다() {
            let _g = Guard;
            let inner = worklog_handler(site(false, vec![fi("DC-1", vec![wl("1", "kim min gyu", T), wl("2", "alice", T)])]));
            let m = Mock::start(move |n, r| if r.target.contains("/myself") { ok(json!({"name": "kim min gyu", "displayName": "나"})) } else { inner(n, r) });
            let svc = service();
            connect_server(&svc, &m).await;
            let r = svc.worklogs(&q_with(&["alice"], Some(true)), None).await.unwrap();
            assert_eq!(ids(&r), ["1", "2"]);
            let search = m.requests().into_iter().find(|x| x.target.starts_with("/rest/api/2/search")).unwrap();
            assert!(jql_of(&search).contains("(worklogAuthor = currentUser() OR worklogAuthor in (\"alice\")) AND"));
            assert_eq!(ids(&svc.worklogs(&q(), None).await.unwrap()), ["1"]);
        }

        #[tokio::test]
        async fn 서비스_워크로그는_검색_페이지가_겹쳐도_이슈를_키로_한_번만_센다() {
            let _g = Guard;
            for cloud in [true, false] {
                let author = if cloud { "acc-me" } else { "me" };
                let a = fi("ABC-1", vec![wl("1", author, T)]);
                let mut issues = vec![a.clone()];
                issues.extend((0..24).map(|i| fi(&format!("F-{i}"), vec![])));
                issues.push(a);
                let (svc, _m) = connected(site(cloud, issues)).await;
                assert_eq!(ids(&svc.worklogs(&q(), None).await.unwrap()), ["1"]);
            }
        }

        fn slow(n: usize) -> Vec<Fi> {
            (0..n)
                .map(|i| {
                    let mut f = fi(&format!("ABC-{}", i + 1), vec![wl(&format!("w{i}"), "acc-me", T)]);
                    f.embed = Some(0);
                    f
                })
                .collect()
        }

        #[tokio::test]
        async fn 서비스_워크로그는_이슈_worklog_가_404_403_이면_그_이슈만_건너뛰고_truncated() {
            let _g = Guard;
            let mut issues = slow(3);
            issues[0].status = Some(404);
            issues[2].status = Some(403);
            let (svc, _m) = connected(site(true, issues)).await;
            let r = svc.worklogs(&q(), None).await.unwrap();
            assert_eq!(ids(&r), ["w1"]);
            assert_eq!(j(&r)["truncated"], json!(true));
        }

        #[tokio::test]
        async fn 서비스_워크로그는_이슈_worklog_응답이_20mb_를_넘으면_그_이슈만_건너뛰고_truncated() {
            let _g = Guard;
            let mut issues = slow(2);
            issues[1].huge = true;
            let (svc, _m) = connected(site(true, issues)).await;
            let r = svc.worklogs(&q(), None).await.unwrap();
            assert_eq!(ids(&r), ["w0"]);
            assert_eq!(j(&r)["truncated"], json!(true));
        }

        #[tokio::test]
        async fn 서비스_워크로그는_이슈_worklog_가_인증_오류면_전체_실패하고_남은_이슈는_요청하지_않는다() {
            let _g = Guard;
            let gate = Gate::default();
            let mut issues = slow(10);
            for i in &mut issues {
                i.gate = Some(gate.clone());
            }
            issues[0].status = Some(401);
            issues[0].gate = None;
            let (svc, m) = connected(site(true, issues)).await;
            // 나머지 이슈의 응답은 문에 막혀 있다 — 401 이 확정되면 그것을 기다리지 않고 바로 실패한다
            assert_eq!(failure(within(svc.worklogs(&q(), None)).await).code, "JIRA_AUTH");
            // 돌아온 시점에 요청 스트림은 이미 버려졌다 — 동시 3개를 넘는 요청은 시작되지 않았다
            assert!(worklog_reqs(&m).len() <= 3, "{}", worklog_reqs(&m).len());
            gate.open();
        }

        #[tokio::test]
        async fn 서비스_워크로그는_받는_도중_2만건_상한에서_멈추고_넘는_것은_담지_않고_truncated() {
            let _g = Guard;
            // 이슈 둘이 각각 10,005건(내장은 비워 이슈별로 다시 받게 한다) — 맞는 항목이 2만 10건이다
            let issues: Vec<Fi> = (0..2)
                .map(|n| {
                    let mut f = fi(&format!("ABC-{}", n + 1), (0..10_005).map(|i| wl(&format!("w{n}-{i}"), "acc-me", T)).collect());
                    f.embed = Some(0);
                    f
                })
                .collect();
            let (svc, m) = connected(site(true, issues)).await;
            let r = svc.worklogs(&q(), None).await.unwrap();
            let v = j(&r);
            assert_eq!((v["worklogs"].as_array().unwrap().len(), v["truncated"].clone()), (20_000, json!(true)));
            // 받는 것은 끝까지 받는다(TS 와 같다) — 페이지는 이슈마다 1000건씩 11번
            assert_eq!(worklog_reqs(&m).len(), 22);
        }

        #[tokio::test]
        async fn 서비스_워크로그는_없는_팀원_400_을_한_명씩_확인해_빼고_invalid_users_로_알린다() {
            let _g = Guard;
            let mut s = site(true, vec![fi("ABC-1", vec![wl("1", "acc-x", T), wl("2", "ghost", T)])]);
            s.bad_users = vec!["ghost".into()];
            let (svc, m) = connected(s).await;
            let r = svc.worklogs(&q_with(&["acc-x", "ghost"], None), None).await.unwrap();
            assert_eq!(j(&r)["invalidUsers"], json!(["ghost"]));
            assert_eq!(ids(&r), ["1"]);
            let probes: Vec<String> = m.requests().iter().filter(|x| ["worklogAuthor = \"ghost\"", "worklogAuthor = \"acc-x\""].contains(&jql_of(x).as_str())).map(|x| qp(x, "maxResults").unwrap()).collect();
            assert_eq!(probes, ["1", "1"]);
            // 남은 사용자가 없으면 빈 결과(includeMe 면 나만 조회)
            let empty = j(&svc.worklogs(&q_with(&["ghost"], None), None).await.unwrap());
            assert_eq!((empty["worklogs"].clone(), empty["truncated"].clone(), empty["invalidUsers"].clone()), (json!([]), json!(false), json!(["ghost"])));
            let me = j(&svc.worklogs(&q_with(&["ghost"], Some(true)), None).await.unwrap());
            assert_eq!(me["invalidUsers"], json!(["ghost"]));
        }

        #[tokio::test]
        async fn 서비스_사용자_검색은_표시_이름을_200자로_자른다() {
            let _g = Guard;
            let mut s = site(false, vec![]);
            s.users = json!([{"name": "alice", "displayName": "가".repeat(300)}]);
            let (svc, _m) = connected(s).await;
            assert_eq!(j(&svc.user_search(&json!("al"), None).await.unwrap())[0]["name"].as_str().unwrap().chars().count(), 200);
        }

        #[tokio::test]
        async fn 서비스_워크로그는_요청_id_로_취소하면_aborted_이다() {
            let _g = Guard;
            let m = Mock::start(|_, r| if r.target.contains("/myself") { ok(json!({"accountId": "acc-me", "displayName": "x"})) } else { ok(json!({})).delay(Duration::from_secs(2)) });
            let svc = service();
            connect_cloud(&svc, &m).await;
            let s2 = svc.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(50)).await;
                s2.cancel("wl-1");
            });
            assert_eq!(failure(svc.worklogs(&q(), Some("wl-1")).await).code, "ABORTED");
        }

        #[tokio::test]
        async fn 서비스_워크로그는_내_id_를_읽을_수_없으면_ts_와_같은_문구로_거부한다() {
            let _g = Guard;
            let m = Mock::start(|_, _| ok(json!({"displayName": "x"})));
            let svc = service();
            connect_cloud(&svc, &m).await;
            let e = failure(svc.worklogs(&q(), None).await);
            assert_eq!((e.code, e.message.as_str()), ("ERROR", "Jira 계정 정보를 읽을 수 없습니다"));
        }

        #[tokio::test]
        async fn 서비스_사용자_검색은_cloud_는_사람_계정만_server_는_name_이_id() {
            let _g = Guard;
            let mut s = site(true, vec![]);
            s.users = json!([
                {"accountId": "712020:aaaa-bbbb", "accountType": "atlassian", "displayName": "홍길동"},
                {"accountId": "app1", "accountType": "app", "displayName": "봇"},
                {"accountId": "cust1", "accountType": "customer", "displayName": "고객"}
            ]);
            let (svc, m) = connected(s).await;
            assert_eq!(j(&svc.user_search(&json!(" 홍 "), None).await.unwrap()), json!([{"id": "712020:aaaa-bbbb", "name": "홍길동"}]));
            let r = m.requests().into_iter().find(|x| x.target.starts_with("/rest/api/3/user/search")).unwrap();
            assert_eq!((qp(&r, "query").as_deref(), qp(&r, "maxResults").as_deref()), (Some("홍"), Some("20")));
            let mut s2 = site(false, vec![]);
            s2.users = json!([{"name": "alice", "key": "JIRAUSER2", "displayName": "Alice"}]);
            let (svc2, m2) = connected(s2).await;
            assert_eq!(j(&svc2.user_search(&json!("al"), None).await.unwrap()), json!([{"id": "alice", "name": "Alice"}]));
            assert!(m2.requests().iter().any(|x| x.target.starts_with("/rest/api/2/user/search?username=al&maxResults=20")));
        }

        // ── 프로필 사진 ─────────────────────────────────────────────────────

        fn image_handler(_n: usize, r: &Req) -> Resp {
            let path = path_of(r);
            if path.ends_with("/myself") {
                return ok(json!({"displayName": "홍길동", "name": "hong"}));
            }
            if path.ends_with("/secure/useravatar") {
                return Resp::new(200).header("Content-Type", "image/png").body(PNG);
            }
            status(404, json!({"errorMessages": ["nf"]}))
        }

        #[tokio::test]
        async fn 서비스_사진은_data_url_로_돌려주고_같은_주소는_다시_받지_않으며_연결을_끊으면_캐시를_비운다() {
            let _g = Guard;
            let m = Mock::start(image_handler);
            let svc = service();
            connect_server(&svc, &m).await;
            let url = json!(format!("{}/secure/useravatar?avatarId=7", m.base()));
            let (a, b) = tokio::join!(svc.avatar(&url), svc.avatar(&url));
            let want = format!("data:image/png;base64,{}", STANDARD.encode(PNG));
            assert_eq!((a.as_deref(), b.as_deref()), (Some(want.as_str()), Some(want.as_str())));
            assert_eq!(svc.avatar(&url).await.as_deref(), Some(want.as_str()));
            let hits = || count(&m, |r| r.target.starts_with("/secure/useravatar"));
            assert_eq!(hits(), 1);
            assert!(!svc.disconnect().await.unwrap().connected);
            assert_eq!(svc.avatar(&url).await, None);
            connect_server(&svc, &m).await;
            assert_eq!(svc.avatar(&url).await.as_deref(), Some(want.as_str()));
            assert_eq!(hits(), 2);
        }

        #[tokio::test]
        async fn 서비스_사진은_받을_수_없으면_짧게_캐시하고_주입한_시계로_만료된다() {
            let _g = Guard;
            let m = Mock::start(image_handler);
            let clock = FakeClock::at(1_000);
            let svc = service_on(new_vault(), clock.clone(), 4000);
            connect_server(&svc, &m).await;
            let url = json!(format!("{}/missing", m.base()));
            let hits = || count(&m, |r| r.target.starts_with("/missing"));
            assert_eq!(svc.avatar(&url).await, None);
            assert_eq!(svc.avatar(&url).await, None);
            assert_eq!(hits(), 1, "받지 못한 주소도 10분은 다시 묻지 않는다");
            clock.set(1_000 + 10 * 60_000 + 1);
            assert_eq!(svc.avatar(&url).await, None);
            assert_eq!(hits(), 2);
        }

        #[tokio::test]
        async fn 서비스_사진은_문자열이_아니거나_너무_긴_주소는_요청하지_않고_null() {
            let _g = Guard;
            let m = Mock::start(image_handler);
            let svc = service();
            connect_server(&svc, &m).await;
            let before = m.requests().len();
            assert_eq!(svc.avatar(&json!(42)).await, None);
            assert_eq!(svc.avatar(&json!(format!("{}/secure/useravatar?x={}", m.base(), "a".repeat(3000)))).await, None);
            assert_eq!(m.requests().len(), before);
        }
    }

    // ── GitHub 서비스(PR 7) ─────────────────────────────────────────────────
    //
    // 루프백 목 서버(127.0.0.1)와 메모리 보관소만 쓴다. `start_paused` 는 쓰지 않는다. "응답이 오기 전/후"는 시간이 아니라 `Gate`·`wait_for` 로 만든다.
    // 사이드카가 쥔 정보(`RepoQuery`)는 가짜(`FakeRepos`)로 채운다 — 실제 구현은 PR 9 다.
    mod github_service {
        use std::collections::BTreeMap;

        use antler_lib::github::device_flow::{DeviceFlow, DeviceSession};
        use antler_lib::github::device_rules::MISSING_CLIENT_ID_MESSAGE;
        use antler_lib::github::ports::{ExternalTokenError, ExternalTokenProvider, GhRepoRef, RepoQuery};
        use antler_lib::github::queries::{self, Slug};
        use antler_lib::github::avatars::{self as av, Avatar, AvatarResolver, Fallback};
        use antler_lib::github::checks::{self, CheckStateCache, TTL_DONE_MS, TTL_NONE_MS, TTL_PENDING_MS};
        use antler_lib::github::service::{GitHubService, ServiceOptions};
        use antler_lib::github::validate::{AvatarQuery, GhListQuery};
        use antler_lib::net::js_date::parse_date_ms;
        use antler_lib::jira::service::{Clock, Sleeper};
        use antler_lib::secret::memory::MemoryStore;
        use antler_lib::secret::{GithubMethod, Vault};
        use futures_util::future::{BoxFuture, FutureExt};
        use serde_json::{json, Value};

        use super::service::{count, new_vault, wait_for, within, FakeClock, Gate, WAIT_LIMIT};
        use super::*;

        // ── 도우미 ──────────────────────────────────────────────────────────

        fn ok(v: Value) -> Resp {
            Resp::new(200).json(&v.to_string())
        }

        fn status(code: u16, v: Value) -> Resp {
            Resp::new(code).json(&v.to_string())
        }

        fn path_of(r: &Req) -> &str {
            r.target.split('?').next().unwrap_or("")
        }

        fn qmap(r: &Req) -> BTreeMap<String, String> {
            url::Url::parse(&format!("http://x{}", r.target)).unwrap().query_pairs().map(|(k, v)| (k.into_owned(), v.into_owned())).collect()
        }

        fn jv(j: &json::Json) -> Value {
            serde_json::from_str(&stringify(j)).unwrap()
        }

        fn rate(r: Resp, remaining: u32) -> Resp {
            r.header("x-ratelimit-remaining", &remaining.to_string()).header("x-ratelimit-limit", "5000").header("x-ratelimit-reset", "1900000000")
        }

        fn user_ok() -> Resp {
            ok(json!({"login": "me", "avatar_url": "https://a/me.png"}))
        }

        const NEXT: &str = "<https://api.github.com/x?page=2>; rel=\"next\"";

        /// Launchpad 가 저장소의 remote 를 읽는 결과: 목록 / 읽기 실패 / 응답 없음(취소될 때까지)
        #[derive(Clone)]
        enum Remotes {
            List(Vec<GhRepoRef>),
            Fail,
            Hang,
        }

        /// 사이드카가 쥔 저장소 정보의 가짜. `hold` 를 켜면 `gh_repo` 가 `gate` 가 열릴 때까지 멈춘다(세대 교체와 겹치는 경합을 만들 때)
        #[derive(Clone)]
        struct FakeRepos {
            repo: Arc<Mutex<Option<GhRepoRef>>>,
            client_id: Arc<Mutex<Option<String>>>,
            hold: Arc<AtomicBool>,
            entered: Arc<AtomicUsize>,
            gate: Arc<tokio::sync::Notify>,
            /// 등록된 저장소 ID → 그 저장소의 remote 읽기 결과(등록 순서)
            remotes: Arc<Mutex<Vec<(String, Remotes)>>>,
            /// 같은 작성자의 push 된 커밋 sha(대체 조회용)와 물어본 이메일
            pushed: Arc<Mutex<Vec<String>>>,
            asked: Arc<Mutex<Vec<String>>>,
            invalidated: Arc<AtomicUsize>,
            /// `hold_client_id` 를 켜면 `github_client_id` 가 `client_id_gate` 가 열릴 때까지 멈춘다(로그인 계열의 첫 await 와 로그아웃이 겹치는 경합)
            hold_client_id: Arc<AtomicBool>,
            client_id_entered: Arc<AtomicUsize>,
            client_id_gate: Arc<tokio::sync::Notify>,
        }

        impl FakeRepos {
            fn new() -> Self {
                let repo = GhRepoRef { owner: "octo".into(), repo: "hello".into(), remote: "origin".into() };
                Self {
                    repo: Arc::new(Mutex::new(Some(repo))),
                    client_id: Arc::new(Mutex::new(None)),
                    hold: Arc::new(AtomicBool::new(false)),
                    entered: Arc::new(AtomicUsize::new(0)),
                    gate: Arc::new(tokio::sync::Notify::new()),
                    remotes: Arc::new(Mutex::new(Vec::new())),
                    pushed: Arc::new(Mutex::new(Vec::new())),
                    asked: Arc::new(Mutex::new(Vec::new())),
                    invalidated: Arc::new(AtomicUsize::new(0)),
                    hold_client_id: Arc::new(AtomicBool::new(false)),
                    client_id_entered: Arc::new(AtomicUsize::new(0)),
                    client_id_gate: Arc::new(tokio::sync::Notify::new()),
                }
            }

            fn with_remotes<S: AsRef<str>>(self, list: Vec<(S, Remotes)>) -> Self {
                *self.remotes.lock().unwrap() = list.into_iter().map(|(id, r)| (id.as_ref().to_string(), r)).collect();
                self
            }

            fn no_repo(self) -> Self {
                *self.repo.lock().unwrap() = None;
                self
            }

            fn with_client_id(self, id: &str) -> Self {
                *self.client_id.lock().unwrap() = Some(id.to_string());
                self
            }
        }

        impl RepoQuery for FakeRepos {
            fn gh_repo<'a>(&'a self, _repo_id: &'a str) -> BoxFuture<'a, Result<Option<GhRepoRef>, String>> {
                async move {
                    self.entered.fetch_add(1, Ordering::SeqCst);
                    if self.hold.load(Ordering::SeqCst) {
                        self.gate.notified().await;
                    }
                    Ok(self.repo.lock().unwrap().clone())
                }
                .boxed()
            }

            fn gh_remotes<'a>(&'a self, repo_id: &'a str, cancel: CancellationToken) -> BoxFuture<'a, Result<Vec<GhRepoRef>, String>> {
                async move {
                    let spec = self.remotes.lock().unwrap().iter().find(|(id, _)| id == repo_id).map(|(_, r)| r.clone());
                    match spec {
                        Some(Remotes::List(l)) => Ok(l),
                        Some(Remotes::Hang) => {
                            cancel.cancelled().await;
                            Err("aborted".to_string())
                        }
                        Some(Remotes::Fail) | None => Err("unreachable".to_string()),
                    }
                }
                .boxed()
            }

            fn pushed_shas_by_author<'a>(&'a self, _repo_id: &'a str, email: &'a str, _limit: usize, _cancel: CancellationToken) -> BoxFuture<'a, Result<Vec<String>, String>> {
                async move {
                    self.asked.lock().unwrap().push(email.to_string());
                    Ok(self.pushed.lock().unwrap().clone())
                }
                .boxed()
            }

            fn invalidate_gh_remotes(&self) {
                self.invalidated.fetch_add(1, Ordering::SeqCst);
            }

            fn github_client_id(&self) -> BoxFuture<'_, Option<String>> {
                async move {
                    self.client_id_entered.fetch_add(1, Ordering::SeqCst);
                    if self.hold_client_id.load(Ordering::SeqCst) {
                        self.client_id_gate.notified().await;
                    }
                    self.client_id.lock().unwrap().clone()
                }
                .boxed()
            }

            fn repo_ids(&self) -> BoxFuture<'_, Vec<String>> {
                async move { self.remotes.lock().unwrap().iter().map(|(id, _)| id.clone()).collect() }.boxed()
            }
        }

        /// 기다림을 기록한다. `clock` 이 있으면 그만큼 시계를 앞으로 돌려 기다리지 않고 시간이 흐른 것처럼 만든다. `pace_ms` 는 실제로 쉬는 시간(요청이 쏟아지지 않게)
        struct StepSleeper {
            waits: Mutex<Vec<u64>>,
            clock: Option<Arc<FakeClock>>,
            pace_ms: u64,
        }

        impl StepSleeper {
            fn new(clock: Option<Arc<FakeClock>>, pace_ms: u64) -> Arc<Self> {
                Arc::new(Self { waits: Mutex::new(Vec::new()), clock, pace_ms })
            }

            fn waits(&self) -> Vec<u64> {
                self.waits.lock().unwrap().clone()
            }
        }

        impl Sleeper for StepSleeper {
            fn sleep(&self, d: Duration) -> BoxFuture<'static, ()> {
                let ms = d.as_millis() as u64;
                self.waits.lock().unwrap().push(ms);
                if let Some(c) = &self.clock {
                    c.set(c.now_ms() as u64 + ms);
                }
                let pace = Duration::from_millis(self.pace_ms);
                async move {
                    if !pace.is_zero() {
                        tokio::time::sleep(pace).await;
                    }
                }
                .boxed()
            }
        }

        /// 대기에 들어가는 순간 주어진 토큰을 취소하고 곧바로 돌아오는 대기
        struct CancelOnSleep(CancellationToken);

        impl Sleeper for CancelOnSleep {
            fn sleep(&self, _d: Duration) -> BoxFuture<'static, ()> {
                self.0.cancel();
                async {}.boxed()
            }
        }

        const CLOCK0: u64 = 1_000_000;

        struct Env {
            svc: GitHubService,
            vault: Arc<Vault>,
            repos: FakeRepos,
        }

        fn flow_on(base: &str, sleeper: Arc<dyn Sleeper>, clock: Arc<FakeClock>) -> DeviceFlow {
            DeviceFlow::new_for_test(base, Duration::from_secs(5), sleeper, clock).unwrap()
        }

        fn env_on(base: &str, vault: Arc<Vault>, repos: FakeRepos, pace_ms: u64) -> Env {
            env_opts(base, vault, repos, pace_ms, ServiceOptions::default())
        }

        fn env_opts(base: &str, vault: Arc<Vault>, repos: FakeRepos, pace_ms: u64, opts: ServiceOptions) -> Env {
            env_full(base, vault, repos, pace_ms, opts, None)
        }

        fn env_full(base: &str, vault: Arc<Vault>, repos: FakeRepos, pace_ms: u64, opts: ServiceOptions, cli: Option<Arc<dyn ExternalTokenProvider>>) -> Env {
            let clock = FakeClock::at(CLOCK0);
            let sleeper = StepSleeper::new(None, pace_ms);
            let flow = flow_on(base, sleeper.clone(), clock.clone());
            let base = base.to_string();
            let svc = GitHubService::with_parts(
                vault.clone(),
                Arc::new(repos.clone()),
                cli,
                move |p| GitHubClient::new_for_test(p, &base, Duration::from_secs(5), Duration::ZERO),
                flow,
                clock,
                opts,
            )
            .unwrap();
            Env { svc, vault, repos }
        }

        fn env(m: &Mock) -> Env {
            env_on(&m.base(), new_vault(), FakeRepos::new(), 0)
        }

        fn signed_in_env(m: &Mock, token: &str, method: GithubMethod) -> Env {
            let vault = new_vault();
            vault.save_github(&SecretString::new(token), method).unwrap();
            env_on(&m.base(), vault, FakeRepos::new(), 0)
        }

        fn failure<T: std::fmt::Debug>(r: Result<T, antler_lib::github::error::GhError>) -> antler_lib::github::error::GhError {
            r.expect_err("오류여야 한다")
        }

        fn slug() -> Slug {
            Slug::parse("octo", "hello").unwrap()
        }

        fn query(state: &str, label: Option<&str>, assignee: Option<&str>, page: u32) -> GhListQuery {
            GhListQuery { state: state.into(), label: label.map(Into::into), assignee: assignee.map(Into::into), page }
        }

        fn state_json(s: &antler_lib::github::service::GhAuthState) -> Value {
            serde_json::to_value(s).unwrap()
        }

        // ── 인증(ghService) ─────────────────────────────────────────────────

        #[tokio::test]
        async fn gh_서비스_pat_로그인은_user_로_검증하고_저장하며_토큰은_상태에_없다() {
            let _g = Guard;
            let m = Mock::start(|_, r| {
                if r.header("authorization") == Some("Bearer ghp_good") {
                    rate(user_ok(), 4999)
                } else {
                    status(401, json!({"message": "Bad credentials"}))
                }
            });
            let e = env(&m);
            let changed = Arc::new(AtomicUsize::new(0));
            let c2 = changed.clone();
            e.svc.set_on_auth_changed(move || {
                c2.fetch_add(1, Ordering::SeqCst);
            });

            let bad = failure(e.svc.login_pat(&json!("ghp_bad")).await);
            assert!(bad.message.contains("올바르지 않"), "{bad}");
            assert!(e.vault.load_github().is_none());
            assert!(!e.svc.auth().await.signed_in);

            let state = e.svc.login_pat(&json!("  ghp_good  ")).await.unwrap();
            let text = serde_json::to_string(&state).unwrap();
            // 상태에는 토큰 필드가 없다: 값과 키 목록, 직렬화 문자열을 모두 본다
            assert_eq!(
                serde_json::from_str::<Value>(&text).unwrap(),
                json!({"signedIn": true, "user": {"login": "me", "avatarUrl": "https://a/me.png"}, "method": "pat", "rateRemaining": 4999, "rateLimit": 5000, "rateReset": 1900000000u64})
            );
            let mut keys: Vec<String> = state_json(&state).as_object().unwrap().keys().cloned().collect();
            keys.sort_unstable();
            assert_eq!(keys, ["method", "rateLimit", "rateRemaining", "rateReset", "signedIn", "user"]);
            assert!(!text.contains("ghp_good") && !text.to_lowercase().contains("token"));
            let stored = e.vault.load_github().unwrap();
            assert_eq!((stored.token.expose(), stored.method), ("ghp_good", GithubMethod::Pat));
            assert_eq!(m.requests().last().unwrap().target, "/user");

            let out = e.svc.logout().await.unwrap();
            assert_eq!(state_json(&out), json!({"signedIn": false}));
            assert!(e.vault.load_github().is_none());
            assert_eq!(changed.load(Ordering::SeqCst), 2, "로그인·로그아웃마다 알린다");
        }

        #[tokio::test]
        async fn gh_서비스는_저장된_토큰을_첫_auth_에서_한_번만_검증한다() {
            let _g = Guard;
            let m = Mock::start(|_, _| user_ok());
            let e = signed_in_env(&m, "ghp_saved", GithubMethod::Device);
            assert_eq!(m.requests().len(), 0);
            let (a, b) = tokio::join!(e.svc.auth(), e.svc.auth());
            e.svc.auth().await;
            assert_eq!(m.requests().len(), 1);
            assert_eq!((a.signed_in, a.method, a.user.as_ref().map(|u| u.login.as_str())), (true, Some("device"), Some("me")));
            assert_eq!(a, b);
        }

        #[tokio::test]
        async fn gh_서비스는_저장된_토큰이_거부되면_버리고_로그아웃_상태로_보인다() {
            let _g = Guard;
            let m = Mock::start(|_, _| status(401, json!({"message": "Bad credentials"})));
            let e = signed_in_env(&m, "ghp_old", GithubMethod::Pat);
            assert!(!e.svc.auth().await.signed_in);
            assert!(e.vault.load_github().is_none());
        }

        #[tokio::test]
        async fn gh_서비스는_네트워크_오류로_검증하지_못하면_로그인_상태를_유지한다() {
            let _g = Guard;
            let vault = new_vault();
            vault.save_github(&SecretString::new("ghp_saved"), GithubMethod::Pat).unwrap();
            let e = env_on(&format!("http://{}", closed_addr()), vault, FakeRepos::new(), 0);
            let s = e.svc.auth().await;
            assert_eq!(state_json(&s), json!({"signedIn": true, "method": "pat"}));
            assert!(e.vault.load_github().is_some());
        }

        #[tokio::test]
        async fn gh_서비스는_저장소가_못_남기는_환경이면_storage_unavailable_을_알린다() {
            let _g = Guard;
            let m = Mock::start(|_, _| user_ok());
            let e = env_on(&m.base(), Arc::new(Vault::new(Arc::new(MemoryStore::new()))), FakeRepos::new(), 0);
            assert!(e.svc.login_pat(&json!("ghp_x")).await.unwrap().storage_unavailable);
        }

        #[tokio::test]
        async fn gh_서비스는_client_id_가_없으면_device_flow_시작을_거부한다() {
            let _g = Guard;
            let m = Mock::start(|_, _| ok(json!({})));
            let e = env(&m);
            assert!(failure(e.svc.device_start().await).message.contains("Client ID"));
            assert!(failure(e.svc.device_poll(&json!("r1")).await).message.contains("진행 중인 로그인이 없습니다"));
            assert_eq!(m.requests().len(), 0);
        }

        fn device_handler(_: usize, r: &Req) -> Resp {
            match path_of(r) {
                "/login/device/code" => ok(json!({"device_code": "dc", "user_code": "U-1", "verification_uri": "https://github.com/login/device", "expires_in": 900, "interval": 5})),
                "/login/oauth/access_token" => ok(json!({"access_token": "gho_tok"})),
                "/user" => user_ok(),
                _ => status(404, json!({})),
            }
        }

        #[tokio::test]
        async fn gh_서비스는_device_flow_전체를_돌려_로그인한다() {
            let _g = Guard;
            let m = Mock::start(device_handler);
            let e = env_on(&m.base(), new_vault(), FakeRepos::new().with_client_id(" cid "), 0);
            let code = e.svc.device_start().await.unwrap();
            let text = serde_json::to_string(&code).unwrap();
            assert_eq!(serde_json::from_str::<Value>(&text).unwrap(), json!({"userCode": "U-1", "verificationUri": "https://github.com/login/device", "expiresIn": 900, "interval": 5}));
            // device_code 는 renderer 용 값에 없다
            assert!(!text.contains("dc") && !text.contains("device_code"), "{text}");
            let body: Value = serde_json::from_str(&m.requests()[0].text()).unwrap();
            assert_eq!(body, json!({"client_id": "cid", "scope": "repo read:org"}));

            let state = e.svc.device_poll(&json!("req-1")).await.unwrap();
            assert_eq!((state.signed_in, state.method, state.user.as_ref().map(|u| u.login.as_str())), (true, Some("device"), Some("me")));
            assert!(!serde_json::to_string(&state).unwrap().contains("gho_tok"));
            let stored = e.vault.load_github().unwrap();
            assert_eq!((stored.token.expose(), stored.method), ("gho_tok", GithubMethod::Device));
            // 끝난 세션은 다시 쓸 수 없다
            assert!(failure(e.svc.device_poll(&json!("req-2")).await).message.contains("진행 중인 로그인이 없습니다"));
        }

        #[tokio::test]
        async fn gh_서비스의_device_poll_은_cancel_request_id_로_취소된다() {
            let _g = Guard;
            let m = Mock::start(|_, r| if path_of(r) == "/login/device/code" { device_handler(0, r) } else { ok(json!({"error": "authorization_pending"})) });
            let e = env_on(&m.base(), new_vault(), FakeRepos::new().with_client_id("cid"), 5);
            e.svc.device_start().await.unwrap();
            let svc = e.svc.clone();
            let poll = tokio::spawn(async move { svc.device_poll(&json!("req-2")).await });
            // 폴링이 실제로 요청을 보내기 시작한 뒤에 취소한다(등록 전에 취소하는 경합이 없다)
            wait_for(|| count(&m, |r| path_of(r) == "/login/oauth/access_token") >= 1).await;
            e.svc.cancel("req-2");
            let err = failure(within(poll).await.unwrap());
            assert_eq!(err.code, "ABORTED");
        }

        #[tokio::test]
        async fn gh_서비스는_github_저장소가_아니면_조회를_거부한다() {
            let _g = Guard;
            let m = Mock::start(|_, _| ok(json!([])));
            let e = env_on(&m.base(), new_vault(), FakeRepos::new().no_repo(), 0);
            let err = failure(e.svc.pulls(&json!("r"), &json!({"state": "open"})).await);
            assert!(err.message.contains("GitHub"), "{err}");
            assert_eq!(m.requests().len(), 0);
        }

        #[tokio::test]
        async fn gh_서비스의_저장소_조회는_owner_repo_를_경로에_쓴다() {
            let _g = Guard;
            let m = Mock::start(|_, _| ok(json!([])));
            let e = env(&m);
            e.svc.issues(&json!("r"), &json!({"state": "open"})).await.unwrap();
            assert_eq!(path_of(&m.requests()[0]), "/repos/octo/hello/issues");
        }

        fn bad_credentials() -> Resp {
            status(401, json!({"message": "Bad credentials"}))
        }

        /// 밖의 토큰 출처(gh CLI)의 가짜: 쓸 수 있는지와 돌려줄 토큰(또는 실패 문구)
        struct FakeCli {
            available: bool,
            token: Result<String, String>,
        }

        impl ExternalTokenProvider for FakeCli {
            fn available(&self) -> BoxFuture<'_, bool> {
                async move { self.available }.boxed()
            }

            fn token(&self) -> BoxFuture<'_, Result<SecretString, ExternalTokenError>> {
                async move { self.token.clone().map(SecretString::new).map_err(|message| ExternalTokenError { message }) }.boxed()
            }
        }

        fn cli_env(m: &Mock, cli: Option<FakeCli>) -> Env {
            env_full(&m.base(), new_vault(), FakeRepos::new(), 0, ServiceOptions::default(), cli.map(|c| Arc::new(c) as Arc<dyn ExternalTokenProvider>))
        }

        #[tokio::test]
        async fn gh_cli_토큰_가져오기는_검증해서_로그인하고_gh_cli_방식으로_저장하며_토큰은_상태에_없다() {
            let _g = Guard;
            let m = Mock::start(|_, r| if r.header("authorization") == Some("Bearer ghp_fromcli") { user_ok() } else { bad_credentials() });
            let e = cli_env(&m, Some(FakeCli { available: true, token: Ok(" ghp_fromcli ".into()) }));
            // 로그인 전 상태에도 cliAvailable 이 실린다
            assert_eq!(state_json(&e.svc.auth().await), json!({"signedIn": false, "cliAvailable": true}));
            let state = e.svc.import_cli_token().await.unwrap();
            let text = serde_json::to_string(&state).unwrap();
            assert_eq!(serde_json::from_str::<Value>(&text).unwrap(), json!({"signedIn": true, "user": {"login": "me", "avatarUrl": "https://a/me.png"}, "method": "gh-cli", "cliAvailable": true}));
            assert!(!text.contains("ghp_fromcli"));
            let stored = e.vault.load_github().unwrap();
            assert_eq!((stored.token.expose(), stored.method), ("ghp_fromcli", GithubMethod::GhCli));
        }

        #[tokio::test]
        async fn gh_cli_토큰_가져오기는_출처가_없거나_실패하거나_형식이_틀리면_요청_없이_거절한다() {
            let _g = Guard;
            let m = Mock::start(|_, _| user_ok());
            let none = cli_env(&m, None);
            assert!(failure(none.svc.import_cli_token().await).message.contains("GitHub CLI 로그인 가져오기를 쓸 수 없습니다"));
            assert_eq!(state_json(&none.svc.auth().await), json!({"signedIn": false}));
            let broken = cli_env(&m, Some(FakeCli { available: false, token: Err("gh 에 로그인되어 있지 않습니다".into()) }));
            assert_eq!(failure(broken.svc.import_cli_token().await).message, "gh 에 로그인되어 있지 않습니다");
            let bad_format = cli_env(&m, Some(FakeCli { available: true, token: Ok("has space".into()) }));
            let err = failure(bad_format.svc.import_cli_token().await);
            assert!(err.message.contains("토큰 형식이 올바르지 않습니다") && !err.message.contains("has space"), "{err}");
            assert_eq!(m.requests().len(), 0);
        }

        #[tokio::test]
        async fn gh_cli_토큰이_github_에서_거부되면_gh_auth_login_안내를_덧붙이고_저장하지_않는다() {
            let _g = Guard;
            let m = Mock::start(|_, _| bad_credentials());
            let e = cli_env(&m, Some(FakeCli { available: true, token: Ok("ghp_stale".into()) }));
            let err = failure(e.svc.import_cli_token().await);
            assert!(err.message.contains("올바르지 않거나 만료") && err.message.contains("gh auth login"), "{err}");
            assert!(!err.message.contains("ghp_stale"));
            assert!(e.vault.load_github().is_none());
            assert!(!e.svc.auth().await.signed_in);
        }

        #[tokio::test]
        async fn gh_낡은_세대의_저장된_토큰_검증이_거부돼도_새_로그인은_유지된다() {
            let _g = Guard;
            let gate = Gate::default();
            let g2 = gate.clone();
            let m = Mock::start(move |_, r| {
                if r.header("authorization") == Some("Bearer ghp_a") {
                    g2.wait();
                    bad_credentials()
                } else {
                    user_ok()
                }
            });
            let e = signed_in_env(&m, "ghp_a", GithubMethod::Pat);
            let svc = e.svc.clone();
            // 저장된 토큰 A 의 /user 검증이 응답을 기다리는 동안 토큰 B 로 로그인한다
            let first = tokio::spawn(async move { svc.auth().await });
            wait_for(|| !m.requests().is_empty()).await;
            e.svc.login_pat(&json!("ghp_b")).await.unwrap();
            gate.open();
            let state = within(first).await.unwrap();
            assert!(state.signed_in);
            assert_eq!(e.svc.git_token().unwrap().expose(), "ghp_b");
            assert_eq!(e.vault.load_github().unwrap().token.expose(), "ghp_b");
            assert!(e.svc.auth().await.signed_in);
        }

        #[tokio::test]
        async fn gh_서비스는_데이터_호출이_401_이고_user_도_401_이면_토큰을_버리고_원래_오류를_돌려준다() {
            let _g = Guard;
            let m = Mock::start(|n, r| if path_of(r) == "/user" && n == 0 { user_ok() } else { bad_credentials() });
            let e = signed_in_env(&m, "ghp_revoked", GithubMethod::Pat);
            assert!(e.svc.auth().await.signed_in);
            let err = failure(e.svc.pulls(&json!("r"), &json!({"state": "open"})).await);
            assert_eq!(err.code, "GH_AUTH");
            assert_eq!(count(&m, |r| path_of(r) == "/user"), 2);
            assert!(!e.svc.auth().await.signed_in);
            assert!(e.vault.load_github().is_none());
        }

        #[tokio::test]
        async fn gh_서비스는_user_는_통과하면_저장소_권한_문제로_보고_토큰을_유지한다() {
            let _g = Guard;
            let m = Mock::start(|_, r| if path_of(r) == "/user" { user_ok() } else { bad_credentials() });
            let e = signed_in_env(&m, "ghp_ok", GithubMethod::Pat);
            assert_eq!(failure(e.svc.issue(&json!("r"), &json!(1)).await).code, "GH_AUTH");
            assert!(e.svc.auth().await.signed_in);
            assert!(e.vault.load_github().is_some());
        }

        #[tokio::test]
        async fn gh_서비스는_토큰이_없을_때의_401_은_재검증하지_않는다() {
            let _g = Guard;
            let m = Mock::start(|_, _| bad_credentials());
            let e = env(&m);
            assert_eq!(failure(e.svc.issue(&json!("r"), &json!(1)).await).code, "GH_AUTH");
            assert_eq!(m.requests().len(), 1);
        }

        #[tokio::test]
        async fn gh_서비스는_로그아웃하면_진행_중이던_목록_보강_요청이_멈춘다() {
            let _g = Guard;
            let gate = Gate::default();
            let g2 = gate.clone();
            let enrich = Arc::new(AtomicUsize::new(0));
            let e2 = enrich.clone();
            let m = Mock::start(move |_, r| {
                if path_of(r).ends_with("/pulls") {
                    return ok(Value::Array((0..12).map(|i| json!({"number": i + 1, "head": {"sha": format!("s{i}")}})).collect()));
                }
                e2.fetch_add(1, Ordering::SeqCst);
                g2.wait();
                ok(json!([]))
            });
            let e = signed_in_env(&m, "ghp_x", GithubMethod::Pat);
            let svc = e.svc.clone();
            let list = tokio::spawn(async move { svc.pulls(&json!("r"), &json!({"state": "open"})).await });
            // 보강 요청이 나가 응답을 기다리는 동안 로그아웃한다
            wait_for(|| enrich.load(Ordering::SeqCst) >= 1).await;
            e.svc.logout().await.unwrap();
            gate.open();
            let err = failure(within(list).await.unwrap());
            assert_eq!(err.code, "ABORTED");
            // 목록이 끝난 뒤에는 이어 도는 작업이 없으므로 요청 수가 더 늘지 않는다. 로그아웃 시점에 이미 나간 동시 요청(최대 6 PR × 3)뿐이다
            let after = enrich.load(Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(50)).await;
            assert_eq!(enrich.load(Ordering::SeqCst), after);
            assert!(after <= 18, "{after}");
        }

        // ── 경합·세대(리뷰가 짚은 것) ───────────────────────────────────────

        #[tokio::test]
        async fn gh_로그인_검증_도중_로그아웃하면_그_로그인은_취소되고_되살아나지_않는다() {
            let _g = Guard;
            let gate = Gate::default();
            let g2 = gate.clone();
            let m = Mock::start(move |_, _| {
                g2.wait();
                user_ok()
            });
            let e = env(&m);
            let svc = e.svc.clone();
            let login = tokio::spawn(async move { svc.login_pat(&json!("ghp_late")).await });
            wait_for(|| !m.requests().is_empty()).await;
            assert!(!e.svc.logout().await.unwrap().signed_in);
            gate.open();
            assert_eq!(failure(within(login).await.unwrap()).code, "ABORTED");
            assert!(!e.svc.auth().await.signed_in);
            assert!(e.vault.load_github().is_none());
            assert!(e.svc.git_token().is_none());
        }

        #[tokio::test]
        async fn gh_device_poll_은_client_id_를_기다리는_사이_로그아웃하면_로그인하지_않는다() {
            let _g = Guard;
            let m = Mock::start(device_handler);
            let e = env_on(&m.base(), new_vault(), FakeRepos::new().with_client_id("cid"), 0);
            e.svc.device_start().await.unwrap();
            e.repos.hold_client_id.store(true, Ordering::SeqCst);
            let entered = e.repos.client_id_entered.load(Ordering::SeqCst);
            let svc = e.svc.clone();
            let poll = tokio::spawn(async move { svc.device_poll(&json!("p1")).await });
            wait_for(|| e.repos.client_id_entered.load(Ordering::SeqCst) > entered).await;
            e.svc.logout().await.unwrap();
            e.repos.hold_client_id.store(false, Ordering::SeqCst);
            e.repos.client_id_gate.notify_one();
            assert_eq!(failure(within(poll).await.unwrap()).code, "ABORTED");
            assert!(e.vault.load_github().is_none());
            assert!(e.svc.git_token().is_none());
            assert!(!e.svc.auth().await.signed_in);
            assert_eq!(count(&m, |r| path_of(r) == "/login/oauth/access_token" || path_of(r) == "/user"), 0);
        }

        /// 토큰을 `gate` 가 열릴 때까지 붙잡는 gh CLI 가짜
        struct GatedCli {
            entered: Arc<AtomicUsize>,
            gate: Arc<tokio::sync::Notify>,
        }

        impl ExternalTokenProvider for GatedCli {
            fn available(&self) -> BoxFuture<'_, bool> {
                async { true }.boxed()
            }

            fn token(&self) -> BoxFuture<'_, Result<SecretString, ExternalTokenError>> {
                async move {
                    self.entered.fetch_add(1, Ordering::SeqCst);
                    self.gate.notified().await;
                    Ok(SecretString::new("ghp_fromcli"))
                }
                .boxed()
            }
        }

        #[tokio::test]
        async fn gh_cli_토큰_가져오기는_토큰을_기다리는_사이_로그아웃하면_로그인하지_않는다() {
            let _g = Guard;
            let m = Mock::start(|_, _| user_ok());
            let (entered, gate) = (Arc::new(AtomicUsize::new(0)), Arc::new(tokio::sync::Notify::new()));
            let cli: Arc<dyn ExternalTokenProvider> = Arc::new(GatedCli { entered: entered.clone(), gate: gate.clone() });
            let e = env_full(&m.base(), new_vault(), FakeRepos::new(), 0, ServiceOptions::default(), Some(cli));
            let svc = e.svc.clone();
            let import = tokio::spawn(async move { svc.import_cli_token().await });
            wait_for(|| entered.load(Ordering::SeqCst) >= 1).await;
            e.svc.logout().await.unwrap();
            gate.notify_one();
            assert_eq!(failure(within(import).await.unwrap()).code, "ABORTED");
            assert!(e.vault.load_github().is_none());
            assert!(e.svc.git_token().is_none());
            assert!(!e.svc.auth().await.signed_in);
        }

        #[tokio::test]
        async fn gh_device_poll_은_같은_request_id_로_다른_요청이_등록돼도_로그아웃이_멈춘다() {
            let _g = Guard;
            let m = Mock::start(|_, r| if path_of(r) == "/login/device/code" { device_handler(0, r) } else { ok(json!({"error": "authorization_pending"})) });
            let e = env_on(&m.base(), new_vault(), FakeRepos::new().with_client_id("cid"), 5);
            e.svc.device_start().await.unwrap();
            let svc = e.svc.clone();
            let poll = tokio::spawn(async move { svc.device_poll(&json!("x")).await });
            wait_for(|| count(&m, |r| path_of(r) == "/login/oauth/access_token") >= 1).await;
            // 같은 id 의 다른 채널 호출이 맵의 항목을 덮어쓰고 끝나면서 지운다
            e.svc.launchpad(&json!("x"), &json!(false)).await.unwrap();
            e.svc.logout().await.unwrap();
            assert_eq!(failure(within(poll).await.unwrap()).code, "ABORTED");
        }

        #[tokio::test]
        async fn gh_저장소_이름이_slug_규칙에_어긋나면_요청_없이_거부한다() {
            let _g = Guard;
            let m = Mock::start(|_, _| ok(json!([])));
            let e = signed_in_env(&m, "ghp_tok", GithubMethod::Pat);
            let sha = "a".repeat(40);
            for (owner, repo) in [("x", ".."), ("..", "user"), (".", "a"), ("a", "."), ("-a", "r"), ("a/b", "r"), ("a", "r/s"), ("", "r"), ("a", ""), ("a_", "r%2f"), ("o".repeat(40).as_str(), "r"), ("o", "r".repeat(101).as_str())] {
                *e.repos.repo.lock().unwrap() = Some(GhRepoRef { owner: owner.into(), repo: repo.into(), remote: "origin".into() });
                let id = json!("r");
                let errs = [
                    e.svc.issues(&id, &json!({"state": "open"})).await.map(|_| ()),
                    e.svc.pulls_for_key(&id, &json!("ABC-1")).await.map(|_| ()),
                    e.svc.commit_checks(&id, &json!([sha])).await.map(|_| ()),
                    e.svc.runs(&id, &json!(1)).await.map(|_| ()),
                ];
                for r in errs {
                    let err = failure(r);
                    assert!(err.message.contains("GitHub"), "{owner}/{repo}: {err}");
                }
                assert_eq!(m.requests().len(), 0, "{owner}/{repo}");
            }
            // 규칙에 맞는 값은 그대로 쓴다
            *e.repos.repo.lock().unwrap() = Some(GhRepoRef { owner: "a-b_1".into(), repo: "r.s_t-1".into(), remote: "origin".into() });
            e.svc.issues(&json!("r"), &json!({"state": "open"})).await.unwrap();
            assert_eq!(path_of(&m.requests()[0]), "/repos/a-b_1/r.s_t-1/issues");
            assert!(Slug::parse("o", "r").is_some() && Slug::parse("o".repeat(39).as_str(), "r".repeat(100).as_str()).is_some());
        }

        #[tokio::test]
        async fn gh_일괄_체크는_응답을_기다리는_사이_캐시가_비워지면_그_결과를_넣지_않는다() {
            let _g = Guard;
            let gate = Gate::default();
            let g2 = gate.clone();
            let inner = graphql_handler(rollups(&[(1, Some("SUCCESS"))]));
            let m = Mock::start(move |n, r| {
                g2.wait();
                inner(n, r)
            });
            let c = gh(&m.base(), Some(TOKEN), 5000, 0);
            let cache = cache(FakeClock::at(CLOCK0));
            let shas = [sha(1)];
            // 취소되지 않는 토큰: 응답이 온 뒤 캐시에 넣기 직전의 경합만 본다
            let cancel = CancellationToken::new();
            let sl = slug();
            let (first, _) = tokio::join!(checks::fetch_commit_checks(&c, &sl, &shas, &cache, &cancel), async {
                wait_for(|| !m.requests().is_empty()).await;
                cache.clear();
                gate.open();
            });
            assert_eq!(jv(&first.unwrap()), json!({sha(1): "success"}));
            // 낡은 결과가 캐시에 없으니 다시 묻는다
            checks::fetch_commit_checks(&c, &slug(), &shas, &cache, &cancel).await.unwrap();
            assert_eq!(m.requests().len(), 2);
        }

        #[tokio::test]
        async fn gh_동시에_로그인해도_키체인과_메모리의_토큰이_같다() {
            let _g = Guard;
            let m = Mock::start(|_, _| user_ok());
            let e = env(&m);
            let (ta, tb) = (json!("ghp_aaa"), json!("ghp_bbb"));
            let (a, b) = tokio::join!(e.svc.login_pat(&ta), e.svc.login_pat(&tb));
            a.unwrap();
            b.unwrap();
            let held = e.svc.git_token().unwrap();
            assert_eq!(e.vault.load_github().unwrap().token.expose(), held.expose());
        }

        #[tokio::test]
        async fn gh_조회는_저장소를_묻는_동안_세대가_바뀌면_요청을_시작하지_않는다() {
            let _g = Guard;
            let m = Mock::start(|_, _| ok(json!([])));
            let e = env(&m);
            e.repos.hold.store(true, Ordering::SeqCst);
            let svc = e.svc.clone();
            let list = tokio::spawn(async move { svc.issues(&json!("r"), &json!({"state": "open"})).await });
            wait_for(|| e.repos.entered.load(Ordering::SeqCst) >= 1).await;
            e.svc.logout().await.unwrap();
            e.repos.gate.notify_one();
            assert_eq!(failure(within(list).await.unwrap()).code, "ABORTED");
            assert_eq!(m.requests().len(), 0);
        }

        #[tokio::test]
        async fn gh_오류_문구에는_서버가_되돌려_준_토큰이_없다() {
            let _g = Guard;
            let m = Mock::start(|_, r| status(500, json!({"message": format!("boom {}", r.header("authorization").unwrap_or(""))})));
            let e = signed_in_env(&m, "ghp_leaky_token_value", GithubMethod::Pat);
            let err = failure(e.svc.issue(&json!("r"), &json!(1)).await);
            assert!(!err.message.contains("ghp_leaky_token_value"), "{err}");
            assert_eq!(err.code, "GH_HTTP");
        }

        fn assert_send<T: Send>(_: &T) {}

        #[tokio::test]
        async fn gh_서비스의_조회_future_는_send_이다() {
            let _g = Guard;
            let m = Mock::start(|_, _| ok(json!([])));
            let e = env(&m);
            let (id, q, n, rid) = (json!("r"), json!({"state": "open"}), json!(1), json!("req"));
            assert_send(&e.svc.pulls(&id, &q));
            assert_send(&e.svc.pull(&id, &n));
            assert_send(&e.svc.issues(&id, &q));
            assert_send(&e.svc.filters(&id));
            assert_send(&e.svc.auth());
            assert_send(&e.svc.login_pat(&rid));
            assert_send(&e.svc.device_start());
            assert_send(&e.svc.device_poll(&rid));
            assert_send(&e.svc.logout());
            let (shas, one_sha, authors) = (json!([sha(1)]), json!(sha(1)), json!([{"email": "a@x.io", "sha": sha(1)}]));
            assert_send(&e.svc.pull_files(&id, &n));
            assert_send(&e.svc.pull_commits(&id, &n));
            assert_send(&e.svc.comments(&id, &n));
            assert_send(&e.svc.pulls_for_key(&id, &json!("ABC-1")));
            assert_send(&e.svc.commit_checks(&id, &shas));
            assert_send(&e.svc.commit_check_runs(&id, &one_sha));
            assert_send(&e.svc.runs(&id, &n));
            assert_send(&e.svc.run_jobs(&id, &n));
            assert_send(&e.svc.avatars(&id, &rid, &authors));
            assert_send(&e.svc.launchpad(&rid, &Value::Null));
            assert_send(&e.svc.accessible_repos(&rid, &Value::Null));
            assert_send(&e.svc.import_cli_token());
        }

        // ── Device Flow(ghDevice) ───────────────────────────────────────────

        fn session_at(clock: &FakeClock, interval: f64, expires_in_ms: f64) -> DeviceSession {
            DeviceSession { device_code: SecretString::new("dc"), interval, expires_at_ms: clock.now_ms() + expires_in_ms }
        }

        fn flow_with(m: &Mock, advance: bool, pace_ms: u64) -> (DeviceFlow, Arc<StepSleeper>, Arc<FakeClock>) {
            let clock = FakeClock::at(0);
            let sleeper = StepSleeper::new(advance.then(|| clock.clone()), pace_ms);
            (flow_on(&m.base(), sleeper.clone(), clock.clone()), sleeper, clock)
        }

        #[tokio::test]
        async fn gh_device_start_는_device_code_로_client_id_와_scope_를_보낸다() {
            let _g = Guard;
            let m = Mock::start(|_, _| ok(json!({"device_code": "dc", "user_code": "ABCD-1234", "verification_uri": "https://github.com/login/device", "expires_in": 900, "interval": 5})));
            let (flow, _, _) = flow_with(&m, false, 0);
            let (code, session) = flow.start("cid", &CancellationToken::new()).await.unwrap();
            let r = &m.requests()[0];
            assert_eq!((r.method.as_str(), r.target.as_str(), r.header("accept"), r.header("user-agent")), ("POST", "/login/device/code", Some("application/json"), Some("Antler")));
            assert_eq!(serde_json::from_str::<Value>(&r.text()).unwrap(), json!({"client_id": "cid", "scope": "repo read:org"}));
            let text = serde_json::to_string(&code).unwrap();
            assert_eq!(serde_json::from_str::<Value>(&text).unwrap(), json!({"userCode": "ABCD-1234", "verificationUri": "https://github.com/login/device", "expiresIn": 900, "interval": 5}));
            assert!(!text.contains("dc\""), "device_code 는 renderer 용 결과에 없다: {text}");
            assert_eq!(session.device_code.expose(), "dc");
        }

        #[tokio::test]
        async fn gh_device_start_는_client_id_가_없으면_안내_문구로_실패하고_요청하지_않는다() {
            let _g = Guard;
            let m = Mock::start(|_, _| ok(json!({})));
            let (flow, _, _) = flow_with(&m, false, 0);
            assert_eq!(failure(flow.start("", &CancellationToken::new()).await).message, MISSING_CLIENT_ID_MESSAGE);
            assert!(MISSING_CLIENT_ID_MESSAGE.contains("Device Flow") && MISSING_CLIENT_ID_MESSAGE.contains("PAT"));
            assert_eq!(m.requests().len(), 0);
        }

        #[tokio::test]
        async fn gh_device_start_는_device_flow_가_꺼진_앱을_안내한다() {
            let _g = Guard;
            let m = Mock::start(|_, _| ok(json!({"error": "device_flow_disabled"})));
            let (flow, _, _) = flow_with(&m, false, 0);
            assert!(failure(flow.start("cid", &CancellationToken::new()).await).message.contains("Enable Device Flow"));
        }

        #[tokio::test]
        async fn gh_device_poll_은_pending_slow_down_성공_순으로_간격을_늘린다() {
            let _g = Guard;
            let steps = [json!({"error": "authorization_pending"}), json!({"error": "slow_down"}), json!({"error": "authorization_pending"}), json!({"access_token": "gho_abc"})];
            let m = Mock::start(move |n, _| ok(steps[n].clone()));
            let (flow, sleeper, clock) = flow_with(&m, false, 0);
            let token = flow.poll("cid", &session_at(&clock, 5.0, 900_000.0), &CancellationToken::new()).await.unwrap();
            assert_eq!(token.expose(), "gho_abc");
            assert_eq!(sleeper.waits(), [5000, 5000, 10_000, 10_000]);
            assert_eq!(m.requests()[0].target, "/login/oauth/access_token");
            assert_eq!(serde_json::from_str::<Value>(&m.requests()[0].text()).unwrap(), json!({"client_id": "cid", "device_code": "dc", "grant_type": "urn:ietf:params:oauth:grant-type:device_code"}));
        }

        #[tokio::test]
        async fn gh_device_poll_은_만료_거부_기타_오류를_한국어_문구로_끝낸다() {
            let _g = Guard;
            for (body, want) in [(json!({"error": "expired_token"}), "만료"), (json!({"error": "access_denied"}), "거부"), (json!({"error": "incorrect_client_credentials", "error_description": "bad client"}), "bad client")] {
                let m = Mock::start(move |_, _| ok(body.clone()));
                let (flow, _, clock) = flow_with(&m, false, 0);
                let err = failure(flow.poll("cid", &session_at(&clock, 5.0, 900_000.0), &CancellationToken::new()).await);
                assert!(err.message.contains(want), "{err}");
                assert_eq!(err.code, "ERROR");
            }
        }

        #[tokio::test]
        async fn gh_device_poll_은_세션_만료_시각이_지나면_요청_없이_만료로_끝낸다() {
            let _g = Guard;
            let m = Mock::start(|_, _| ok(json!({"error": "authorization_pending"})));
            let (flow, _, clock) = flow_with(&m, true, 0);
            let err = failure(flow.poll("cid", &session_at(&clock, 5.0, 12_000.0), &CancellationToken::new()).await);
            assert!(err.message.contains("만료"), "{err}");
            assert_eq!(m.requests().len(), 2);
        }

        #[tokio::test]
        async fn gh_device_poll_은_일시_오류를_재시도하고_간격에_backoff_를_더한다() {
            let _g = Guard;
            let m = Mock::start(|n, _| match n {
                // 본문 도중에 끊긴 응답(네트워크 오류), 502, JSON 이 아닌 본문, 정상 pending, 성공
                0 => Resp::new(200).claim_len(64),
                1 => status(502, json!({"message": "bad gateway"})),
                2 => Resp::new(200).body("<html>oops</html>"),
                3 => ok(json!({"error": "authorization_pending"})),
                _ => ok(json!({"access_token": "gho_ok"})),
            });
            let (flow, sleeper, clock) = flow_with(&m, false, 0);
            let token = flow.poll("cid", &session_at(&clock, 5.0, 900_000.0), &CancellationToken::new()).await.unwrap();
            assert_eq!(token.expose(), "gho_ok");
            assert_eq!(m.requests().len(), 5);
            // 연속 실패마다 +2초, 정상 응답이 오면 초기화
            assert_eq!(sleeper.waits(), [5000, 7000, 9000, 11_000, 5000]);
        }

        #[tokio::test]
        async fn gh_device_poll_은_일시_오류만_이어지면_세션_만료로_끝난다() {
            let _g = Guard;
            let m = Mock::start(|_, _| status(503, json!({})));
            let (flow, _, clock) = flow_with(&m, true, 0);
            let err = failure(flow.poll("cid", &session_at(&clock, 5.0, 30_000.0), &CancellationToken::new()).await);
            assert!(err.message.contains("만료"), "{err}");
            assert!(m.requests().len() > 1);
        }

        #[tokio::test]
        async fn gh_device_poll_은_기다리는_중_취소하면_aborted_이고_요청하지_않는다() {
            let _g = Guard;
            let m = Mock::start(|_, _| ok(json!({"error": "authorization_pending"})));
            let cancel = CancellationToken::new();
            let clock = FakeClock::at(0);
            let flow = flow_on(&m.base(), Arc::new(CancelOnSleep(cancel.clone())), clock.clone());
            let err = failure(flow.poll("cid", &session_at(&clock, 5.0, 900_000.0), &cancel).await);
            assert_eq!(err.code, "ABORTED");
            assert_eq!(m.requests().len(), 0);
        }

        #[tokio::test]
        async fn gh_device_poll_은_요청_중_취소해도_aborted_이다() {
            let _g = Guard;
            let gate = Gate::default();
            let g2 = gate.clone();
            let m = Mock::start(move |_, _| {
                g2.wait();
                ok(json!({"error": "authorization_pending"}))
            });
            let (flow, _, clock) = flow_with(&m, false, 0);
            let cancel = CancellationToken::new();
            let (c2, s) = (cancel.clone(), session_at(&clock, 5.0, 900_000.0));
            let poll = tokio::spawn(async move { flow.poll("cid", &s, &c2).await });
            wait_for(|| !m.requests().is_empty()).await;
            cancel.cancel();
            assert_eq!(failure(within(poll).await.unwrap()).code, "ABORTED");
            gate.open();
        }

        #[tokio::test]
        async fn gh_device_flow_는_리다이렉트를_따르지_않고_대상에_연결하지_않는다() {
            let _g = Guard;
            let target = Mock::start(|_, _| ok(json!({"access_token": "stolen"})));
            let to = format!("{}/stolen", target.base());
            let m = Mock::start(move |_, _| Resp::new(302).header("Location", &to));
            let (flow, _, clock) = flow_with(&m, true, 0);
            let err = failure(flow.poll("cid", &session_at(&clock, 5.0, 12_000.0), &CancellationToken::new()).await);
            assert!(err.message.contains("만료"), "3xx 는 해석할 수 없는 응답이라 일시 오류다: {err}");
            assert_eq!(target.connections(), 0);
        }

        // ── 조회(ghApi) ─────────────────────────────────────────────────────

        fn user(login: &str) -> Value {
            json!({"login": login, "avatar_url": format!("https://a/{login}.png")})
        }

        fn merged(mut base: Value, extra: Value) -> Value {
            if let (Some(o), Some(e)) = (base.as_object_mut(), extra.as_object()) {
                for (k, v) in e {
                    o.insert(k.clone(), v.clone());
                }
            }
            base
        }

        fn pull(n: u64, extra: Value) -> Value {
            merged(
                json!({
                    "number": n, "title": format!("PR {n}"), "state": "open", "draft": false, "merged_at": null, "user": user("alice"),
                    "labels": [{"name": "bug", "color": "ff0000"}], "assignees": [user("bob")], "requested_reviewers": [],
                    "head": {"ref": format!("feat-{n}"), "sha": format!("sha{n}")}, "base": {"ref": "main"},
                    "updated_at": "2026-01-02T00:00:00Z", "created_at": "2026-01-01T00:00:00Z", "html_url": format!("https://github.com/octo/hello/pull/{n}")
                }),
                extra,
            )
        }

        fn issue(n: u64, extra: Value) -> Value {
            merged(
                json!({"number": n, "title": format!("I{n}"), "state": "open", "user": user("a"), "labels": [], "assignees": [], "comments": 0, "updated_at": "2026-01-01T00:00:00Z", "created_at": "2026-01-01T00:00:00Z", "html_url": "u"}),
                extra,
            )
        }

        #[tokio::test]
        async fn gh_list_pulls_비로그인은_pulls_만_부르고_merged_상태를_해석한다() {
            let _g = Guard;
            let body = json!([pull(1, json!({})), pull(2, json!({"state": "closed", "merged_at": "2026-01-03T00:00:00Z"})), pull(3, json!({"state": "closed"})), pull(4, json!({"draft": true, "user": null}))]);
            let m = Mock::start(move |_, _| ok(body.clone()).header("link", NEXT));
            let c = gh(&m.base(), None, 5000, 0);
            let res = jv(&queries::list_pulls(&c, &slug(), &query("all", None, None, 2), &CancellationToken::new()).await.unwrap());
            assert_eq!(m.requests().len(), 1);
            assert_eq!(path_of(&m.requests()[0]), "/repos/octo/hello/pulls");
            let want: BTreeMap<String, String> = [("state", "all"), ("per_page", "30"), ("page", "2"), ("sort", "updated"), ("direction", "desc")].iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
            assert_eq!(qmap(&m.requests()[0]), want);
            assert_eq!(res["hasMore"], json!(true));
            let items = res["items"].as_array().unwrap();
            assert_eq!(items.iter().map(|p| p["state"].as_str().unwrap()).collect::<Vec<_>>(), ["open", "merged", "closed", "open"]);
            assert_eq!((items[3]["draft"].clone(), items[3]["author"]["login"].clone()), (json!(true), json!("ghost")));
            let first = &items[0];
            assert_eq!(
                (first["number"].clone(), first["headRef"].clone(), first["baseRef"].clone(), first["headSha"].clone(), first["updatedAt"].clone(), first["createdAt"].clone()),
                (json!(1), json!("feat-1"), json!("main"), json!("sha1"), json!(1767312000), json!(1767225600))
            );
            assert_eq!(first["labels"], json!([{"name": "bug", "color": "ff0000"}]));
            assert_eq!(first["assignees"], json!([{"login": "bob", "avatarUrl": "https://a/bob.png"}]));
            assert!(first.get("review").is_none() && first.get("checks").is_none());
        }

        #[tokio::test]
        async fn gh_list_pulls_로그인은_리뷰_체크를_동시_6개_이하로_보강한다() {
            let _g = Guard;
            let (active, peak) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
            let (a, p) = (active.clone(), peak.clone());
            let m = Mock::start(move |_, r| {
                let path = path_of(r);
                if path.ends_with("/pulls") {
                    return ok(Value::Array((1..=12).map(|i| pull(i, if i == 1 { json!({"requested_reviewers": [user("r")]}) } else { json!({}) })).collect()));
                }
                let now = a.fetch_add(1, Ordering::SeqCst) + 1;
                p.fetch_max(now, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(2));
                a.fetch_sub(1, Ordering::SeqCst);
                if let Some(rest) = path.strip_suffix("/reviews") {
                    let approved = rest.ends_with("/pulls/2");
                    return ok(if approved { json!([{"user": user("x"), "state": "APPROVED", "submitted_at": "2026-01-01T00:00:00Z"}]) } else { json!([]) });
                }
                if path.ends_with("/check-runs") {
                    return ok(json!({"check_runs": if path.contains("/commits/sha3/") { json!([{"name": "ci", "status": "completed", "conclusion": "failure"}]) } else { json!([]) }}));
                }
                ok(json!({"state": "pending", "total_count": 0, "statuses": []}))
            });
            let c = gh(&m.base(), Some("tok"), 5000, 0);
            let res = jv(&queries::list_pulls(&c, &slug(), &query("open", None, None, 1), &CancellationToken::new()).await.unwrap());
            let items = res["items"].as_array().unwrap();
            assert_eq!((items[0]["review"].clone(), items[0]["checks"].clone()), (json!("pending"), json!("none")));
            assert_eq!(items[1]["review"], json!("approved"));
            assert_eq!(items[2]["checks"], json!("failure"));
            // PR 당 reviews + check-runs + status
            assert_eq!(m.requests().len(), 1 + 12 * 3);
            assert!(peak.load(Ordering::SeqCst) <= 6 * 3, "동시 {}개", peak.load(Ordering::SeqCst));
            assert!(m.requests().iter().all(|r| r.header("authorization") == Some("Bearer tok")));
        }

        #[tokio::test]
        async fn gh_list_pulls_는_보강이_실패해도_목록은_돌려준다() {
            let _g = Guard;
            let m = Mock::start(|_, r| if path_of(r).ends_with("/pulls") { ok(json!([pull(1, json!({}))])) } else { status(500, json!({"message": "boom"})) });
            let c = gh(&m.base(), Some("tok"), 5000, 0);
            let res = jv(&queries::list_pulls(&c, &slug(), &query("open", None, None, 1), &CancellationToken::new()).await.unwrap());
            assert_eq!(res["items"].as_array().unwrap().len(), 1);
            assert!(res["items"][0].get("review").is_none());
        }

        #[tokio::test]
        async fn gh_list_pulls_는_라벨_담당자_필터면_issues_엔드포인트에서_pr_만_남기고_보강하지_않는다() {
            let _g = Guard;
            let pr = json!({"number": 7, "title": "S", "state": "closed", "draft": false, "user": user("a"), "labels": [{"name": "bug", "color": "f00"}], "assignees": [], "comments": 3, "updated_at": "2026-01-02T00:00:00Z", "created_at": "2026-01-01T00:00:00Z", "html_url": "u", "pull_request": {"merged_at": "2026-01-02T00:00:00Z"}});
            let plain = json!({"number": 8, "title": "그냥 이슈", "state": "closed", "user": user("a"), "labels": [], "assignees": [], "comments": 0, "updated_at": "2026-01-02T00:00:00Z", "created_at": "2026-01-01T00:00:00Z", "html_url": "u"});
            let body = json!([pr, plain]);
            let m = Mock::start(move |_, _| ok(body.clone()));
            let c = gh(&m.base(), Some("tok"), 5000, 0);
            let res = jv(&queries::list_pulls(&c, &slug(), &query("closed", Some("good \"first\""), Some("none"), 3), &CancellationToken::new()).await.unwrap());
            assert_eq!(m.requests().len(), 1);
            assert_eq!(path_of(&m.requests()[0]), "/repos/octo/hello/issues");
            let q = qmap(&m.requests()[0]);
            assert_eq!(
                (q["state"].as_str(), q["labels"].as_str(), q["assignee"].as_str(), q["per_page"].as_str(), q["page"].as_str(), q["sort"].as_str(), q["direction"].as_str()),
                ("closed", "good \"first\"", "none", "30", "3", "updated", "desc")
            );
            let items = res["items"].as_array().unwrap();
            assert_eq!(items.iter().map(|i| i["number"].clone()).collect::<Vec<_>>(), [json!(7)]);
            assert_eq!((items[0]["state"].clone(), items[0]["comments"].clone(), items[0]["headSha"].clone()), (json!("merged"), json!(3), json!("")));
        }

        #[tokio::test]
        async fn gh_list_pulls_의_필터된_목록은_pr_이_없는_쪽을_건너뛰고_끝나면_next_page_가_없다() {
            let _g = Guard;
            let m = Mock::start(|_, r| {
                let page: u64 = qmap(r)["page"].parse().unwrap();
                let raw = merged(issue(page, json!({})), if page == 3 { json!({"pull_request": {}}) } else { json!({}) });
                let resp = ok(json!([raw]));
                if page == 3 { resp } else { resp.header("link", "<x>; rel=\"next\"") }
            });
            let c = gh(&m.base(), None, 5000, 0);
            let res = jv(&queries::list_pulls(&c, &slug(), &query("open", Some("bug"), None, 1), &CancellationToken::new()).await.unwrap());
            assert_eq!(m.requests().iter().map(|r| qmap(r)["page"].clone()).collect::<Vec<_>>(), ["1", "2", "3"]);
            assert_eq!(res["items"].as_array().unwrap().iter().map(|p| p["number"].clone()).collect::<Vec<_>>(), [json!(3)]);
            assert_eq!(res["hasMore"], json!(false));
            assert!(res.get("nextPage").is_none());
        }

        #[tokio::test]
        async fn gh_list_pulls_의_라벨_이름은_검색_쿼리_언어로_해석되지_않는다() {
            let _g = Guard;
            let m = Mock::start(|_, _| ok(json!([])));
            let c = gh(&m.base(), None, 5000, 0);
            queries::list_pulls(&c, &slug(), &query("all", Some("x\" repo:other/repo"), None, 1), &CancellationToken::new()).await.unwrap();
            let r = &m.requests()[0];
            assert_eq!(path_of(r), "/repos/octo/hello/issues");
            assert_eq!(qmap(r)["labels"], "x\" repo:other/repo");
            assert!(!m.requests().iter().any(|r| path_of(r) == "/search/issues"));
        }

        #[tokio::test]
        async fn gh_get_pull_은_상세_리뷰_체크를_합친다() {
            let _g = Guard;
            let m = Mock::start(|_, r| {
                let p = path_of(r);
                if p.ends_with("/pulls/5") {
                    return ok(pull(5, json!({"body_html": "<p>hi</p>", "mergeable": true, "additions": 3, "deletions": 1, "changed_files": 2, "comments": 4, "requested_reviewers": [user("rev")]})));
                }
                if p.ends_with("/reviews") {
                    return ok(json!([{"user": user("x"), "state": "CHANGES_REQUESTED", "submitted_at": "2026-01-02T00:00:00Z"}, {"user": user("y"), "state": "PENDING"}]));
                }
                if p.ends_with("/check-runs") {
                    return ok(json!({"check_runs": [{"name": "ci", "status": "completed", "conclusion": "success", "html_url": "https://c/1"}]}));
                }
                if p.ends_with("/status") {
                    return ok(json!({"statuses": [{"context": "lint", "state": "pending", "target_url": null}]}));
                }
                status(404, json!({}))
            });
            let c = gh(&m.base(), None, 5000, 0);
            let d = jv(&queries::get_pull(&c, &slug(), 5, &CancellationToken::new()).await.unwrap());
            assert_eq!(m.requests()[0].header("accept"), Some("application/vnd.github.html+json"));
            for (k, v) in [("bodyHtml", json!("<p>hi</p>")), ("mergeable", json!(true)), ("additions", json!(3)), ("deletions", json!(1)), ("changedFiles", json!(2)), ("comments", json!(4)), ("review", json!("changes_requested")), ("checks", json!("pending"))] {
                assert_eq!(d[k], v, "{k}");
            }
            assert_eq!(d["reviews"], json!([{"author": {"login": "x", "avatarUrl": "https://a/x.png"}, "state": "changes_requested", "submittedAt": 1767312000}]));
            assert_eq!(d["checkRuns"], json!([{"name": "ci", "state": "success", "url": "https://c/1"}, {"name": "lint", "state": "pending"}]));
            assert_eq!(d["requestedReviewers"], json!([{"login": "rev", "avatarUrl": "https://a/rev.png"}]));
        }

        #[tokio::test]
        async fn gh_get_pull_의_리뷰_목록은_dismissed_를_보이고_집계는_판정을_취소한다() {
            let _g = Guard;
            let m = Mock::start(|_, r| {
                let p = path_of(r);
                if p.ends_with("/pulls/6") {
                    return ok(pull(6, json!({})));
                }
                if p.ends_with("/reviews") {
                    return ok(json!([{"user": user("x"), "state": "CHANGES_REQUESTED", "submitted_at": "2026-01-01T00:00:00Z"}, {"user": user("x"), "state": "DISMISSED", "submitted_at": "2026-01-02T00:00:00Z"}]));
                }
                ok(json!({"check_runs": [], "statuses": []}))
            });
            let c = gh(&m.base(), None, 5000, 0);
            let d = jv(&queries::get_pull(&c, &slug(), 6, &CancellationToken::new()).await.unwrap());
            assert_eq!(d["reviews"].as_array().unwrap().iter().map(|r| r["state"].clone()).collect::<Vec<_>>(), [json!("changes_requested"), json!("dismissed")]);
            assert_eq!(d["review"], json!("commented"));
        }

        #[tokio::test]
        async fn gh_get_pull_files_는_link_를_따라_이어_받는다() {
            let _g = Guard;
            let m = Mock::start(|_, r| {
                let page: u64 = qmap(r)["page"].parse().unwrap();
                let mut files = vec![json!({"filename": format!("f{page}.ts"), "status": "modified", "additions": 1, "deletions": 2, "patch": "@@"})];
                if page == 2 {
                    files.push(json!({"filename": "new.ts", "previous_filename": "old.ts", "status": "renamed", "additions": 0, "deletions": 0}));
                }
                let resp = ok(Value::Array(files));
                if page < 2 { resp.header("link", "<x>; rel=\"next\"") } else { resp }
            });
            let c = gh(&m.base(), None, 5000, 0);
            let files = jv(&queries::get_pull_files(&c, &slug(), 9, &CancellationToken::new()).await.unwrap());
            assert_eq!(m.requests().iter().map(|r| qmap(r)["page"].clone()).collect::<Vec<_>>(), ["1", "2"]);
            assert_eq!(qmap(&m.requests()[0])["per_page"], "100");
            let paths: Vec<&str> = files.as_array().unwrap().iter().map(|f| f["path"].as_str().unwrap()).collect();
            assert_eq!(paths, ["f1.ts", "f2.ts", "new.ts"]);
            assert_eq!(files[2], json!({"path": "new.ts", "oldPath": "old.ts", "status": "renamed", "additions": 0, "deletions": 0}));
            assert_eq!(files[0]["patch"], json!("@@"));
        }

        #[tokio::test]
        async fn gh_get_pull_files_는_30쪽에서_멈춘다() {
            let _g = Guard;
            let m = Mock::start(|_, _| ok(json!([])).header("link", "<x>; rel=\"next\""));
            let c = gh(&m.base(), None, 5000, 0);
            queries::get_pull_files(&c, &slug(), 9, &CancellationToken::new()).await.unwrap();
            assert_eq!(m.requests().len(), 30);
        }

        #[tokio::test]
        async fn gh_get_pull_commits_는_250개까지_받는다() {
            let _g = Guard;
            let m = Mock::start(|_, r| {
                let page: u64 = qmap(r)["page"].parse().unwrap();
                let commits: Vec<Value> = (0..100).map(|i| json!({"sha": format!("c{page}-{i}"), "commit": {"message": "m", "author": {"name": "N", "date": "2026-01-01T00:00:00Z"}}, "author": if i == 0 { user("al") } else { Value::Null }})).collect();
                ok(Value::Array(commits)).header("link", "<x>; rel=\"next\"")
            });
            let c = gh(&m.base(), None, 5000, 0);
            let commits = jv(&queries::get_pull_commits(&c, &slug(), 9, &CancellationToken::new()).await.unwrap());
            assert_eq!(m.requests().len(), 3);
            assert_eq!(commits.as_array().unwrap().len(), 250);
            assert_eq!(commits[0], json!({"sha": "c1-0", "message": "m", "authorName": "N", "author": {"login": "al", "avatarUrl": "https://a/al.png"}, "date": 1767225600}));
            assert!(commits[1].get("author").is_none());
        }

        #[tokio::test]
        async fn gh_get_comments_는_html_accept_로_받는다() {
            let _g = Guard;
            let m = Mock::start(|_, _| ok(json!([{"user": user("c"), "body_html": "<p>x</p>", "created_at": "2026-01-01T00:00:00Z", "html_url": "u"}])));
            let c = gh(&m.base(), None, 5000, 0);
            let res = jv(&queries::get_comments(&c, &slug(), 3, &CancellationToken::new()).await.unwrap());
            assert_eq!(path_of(&m.requests()[0]), "/repos/octo/hello/issues/3/comments");
            assert_eq!(m.requests()[0].header("accept"), Some("application/vnd.github.html+json"));
            assert_eq!(res, json!([{"author": {"login": "c", "avatarUrl": "https://a/c.png"}, "bodyHtml": "<p>x</p>", "createdAt": 1767225600, "htmlUrl": "u"}]));
        }

        #[tokio::test]
        async fn gh_get_filters_는_라벨과_담당_가능_사용자를_함께_받는다() {
            let _g = Guard;
            let m = Mock::start(|_, r| if path_of(r).ends_with("/labels") { ok(json!([{"name": "bug", "color": "f00"}])) } else { ok(json!([user("bob")])) });
            let c = gh(&m.base(), None, 5000, 0);
            let res = jv(&queries::get_filters(&c, &slug(), &CancellationToken::new()).await.unwrap());
            let mut paths: Vec<String> = m.requests().iter().map(|r| path_of(r).to_string()).collect();
            paths.sort();
            assert_eq!(paths, ["/repos/octo/hello/assignees", "/repos/octo/hello/labels"]);
            assert_eq!(res, json!({"labels": [{"name": "bug", "color": "f00"}], "assignees": [{"login": "bob", "avatarUrl": "https://a/bob.png"}]}));
        }

        #[tokio::test]
        async fn gh_list_issues_는_pr_을_거르고_has_more_는_link_로_판단한다() {
            let _g = Guard;
            let body = json!([issue(1, json!({})), issue(2, json!({"pull_request": {"url": "x"}})), issue(3, json!({"state": "closed"}))]);
            let m = Mock::start(move |_, _| ok(body.clone()).header("link", "<x>; rel=\"next\""));
            let c = gh(&m.base(), None, 5000, 0);
            let res = jv(&queries::list_issues(&c, &slug(), &query("all", Some("bug"), Some("none"), 2), &CancellationToken::new()).await.unwrap());
            let want: BTreeMap<String, String> = [("state", "all"), ("labels", "bug"), ("assignee", "none"), ("per_page", "30"), ("page", "2"), ("sort", "updated"), ("direction", "desc")].iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
            assert_eq!(qmap(&m.requests()[0]), want);
            assert_eq!(res["items"].as_array().unwrap().iter().map(|i| i["number"].clone()).collect::<Vec<_>>(), [json!(1), json!(3)]);
            assert_eq!(res["items"][1]["state"], json!("closed"));
            // 30개 미만이어도 Link 가 있으면 더 있다
            assert_eq!(res["hasMore"], json!(true));
        }

        #[tokio::test]
        async fn gh_list_issues_는_전부_pr_인_쪽도_link_가_없으면_끝이다() {
            let _g = Guard;
            let m = Mock::start(|_, _| ok(json!([issue(1, json!({"pull_request": {}}))])));
            let c = gh(&m.base(), None, 5000, 0);
            let res = jv(&queries::list_issues(&c, &slug(), &query("open", None, None, 1), &CancellationToken::new()).await.unwrap());
            assert_eq!(res, json!({"items": [], "hasMore": false}));
        }

        #[tokio::test]
        async fn gh_list_issues_는_pr_만_있는_쪽이_비면_항목이_나올_때까지_읽고_next_page_를_알린다() {
            let _g = Guard;
            let m = Mock::start(|_, r| {
                let page: u64 = qmap(r)["page"].parse().unwrap();
                let body = if page < 4 { json!([issue(page, json!({"pull_request": {}}))]) } else { json!([issue(9, json!({}))]) };
                ok(body).header("link", "<x>; rel=\"next\"")
            });
            let c = gh(&m.base(), None, 5000, 0);
            let res = jv(&queries::list_issues(&c, &slug(), &query("open", None, None, 2), &CancellationToken::new()).await.unwrap());
            assert_eq!(m.requests().iter().map(|r| qmap(r)["page"].clone()).collect::<Vec<_>>(), ["2", "3", "4"]);
            assert_eq!(res["items"].as_array().unwrap().iter().map(|i| i["number"].clone()).collect::<Vec<_>>(), [json!(9)]);
            assert_eq!((res["hasMore"].clone(), res["nextPage"].clone()), (json!(true), json!(5)));
        }

        #[tokio::test]
        async fn gh_list_issues_는_계속_비어_있어도_추가_5쪽에서_멈춘다() {
            let _g = Guard;
            let m = Mock::start(|_, _| ok(json!([issue(1, json!({"pull_request": {}}))])).header("link", "<x>; rel=\"next\""));
            let c = gh(&m.base(), None, 5000, 0);
            let res = jv(&queries::list_issues(&c, &slug(), &query("open", None, None, 1), &CancellationToken::new()).await.unwrap());
            assert_eq!(m.requests().len(), 6);
            assert_eq!(res, json!({"items": [], "hasMore": true, "nextPage": 7}));
        }

        #[tokio::test]
        async fn gh_get_issue_는_html_본문과_closed_at_을_돌려준다() {
            let _g = Guard;
            let m = Mock::start(|_, _| ok(issue(4, json!({"state": "closed", "body_html": "<p>b</p>", "closed_at": "2026-01-02T00:00:00Z"}))));
            let c = gh(&m.base(), None, 5000, 0);
            let d = jv(&queries::get_issue(&c, &slug(), 4, &CancellationToken::new()).await.unwrap());
            assert_eq!(m.requests()[0].header("accept"), Some("application/vnd.github.html+json"));
            assert_eq!((d["number"].clone(), d["bodyHtml"].clone(), d["closedAt"].clone(), d["state"].clone()), (json!(4), json!("<p>b</p>"), json!(1767312000), json!("closed")));
        }

        // ── 아바타(avatars) ─────────────────────────────────────────────────

        fn sha_of(c: char) -> String {
            c.to_string().repeat(40)
        }

        fn rate_headers(r: Resp, remaining: u32, reset: u64) -> Resp {
            r.header("x-ratelimit-remaining", &remaining.to_string()).header("x-ratelimit-limit", "60").header("x-ratelimit-reset", &reset.to_string())
        }

        fn commit(sha: &str, author: Value) -> Resp {
            rate_headers(ok(json!([{"sha": sha, "author": author}])), 50, 1_900_000_000)
        }

        fn sha_query(r: &Req) -> String {
            qmap(r).get("sha").cloned().unwrap_or_default()
        }

        fn query_of(email: &str, sha: &str) -> AvatarQuery {
            AvatarQuery { email: email.into(), sha: sha.into() }
        }

        fn resolver(base: &str, token: Option<&str>, clock: Arc<FakeClock>, file: Option<std::path::PathBuf>) -> AvatarResolver {
            AvatarResolver::new(Arc::new(gh(base, token, 5000, 0)), file, clock)
        }

        /// 시험용 임시 폴더(끝나면 지운다)
        struct TempDir(std::path::PathBuf);

        impl TempDir {
            fn new(tag: &str) -> Self {
                static N: AtomicUsize = AtomicUsize::new(0);
                let p = std::env::temp_dir().join(format!("antler-{tag}-{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
                std::fs::create_dir_all(&p).unwrap();
                Self(p)
            }
        }

        impl Drop for TempDir {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }

        fn avatar(login: &str, url: &str) -> Option<Avatar> {
            Some(Avatar { login: login.into(), url: url.into() })
        }

        #[test]
        fn gh_parse_noreply_는_id_와_로그인을_읽는다() {
            let n = av::parse_noreply("12345+octocat@users.noreply.github.com").unwrap();
            assert_eq!((n.login.as_str(), n.id), ("octocat", Some(12345)));
        }

        #[test]
        fn gh_parse_noreply_는_옛_형식을_id_없이_읽는다() {
            let n = av::parse_noreply("octocat@users.noreply.github.com").unwrap();
            assert_eq!((n.login.as_str(), n.id), ("octocat", None));
        }

        #[test]
        fn gh_parse_noreply_는_대소문자_공백을_허용하고_봇은_id_가_있어야_한다() {
            let n = av::parse_noreply(" 7+Foo-Bar@Users.Noreply.GitHub.com ").unwrap();
            assert_eq!((n.login.as_str(), n.id), ("Foo-Bar", Some(7)));
            let b = av::parse_noreply("49699333+dependabot[bot]@users.noreply.github.com").unwrap();
            assert_eq!((b.login.as_str(), b.id), ("dependabot[bot]", Some(49_699_333)));
            assert_eq!(av::parse_noreply("dependabot[bot]@users.noreply.github.com"), None);
        }

        #[test]
        fn gh_parse_noreply_는_다른_도메인과_잘못된_로그인을_거부한다() {
            for bad in ["me@example.com", "me@users.noreply.github.com.evil.com", "evil@x@users.noreply.github.com", "-bad@users.noreply.github.com", "a/b@users.noreply.github.com", "@users.noreply.github.com", "5+me+x@users.noreply.github.com", "1234567890123+me@users.noreply.github.com"] {
                assert_eq!(av::parse_noreply(bad), None, "{bad}");
            }
        }

        #[test]
        fn gh_아바타_기본_주소는_id_가_있으면_u_id_없으면_로그인이다() {
            assert_eq!(av::avatar_base_url("octocat", Some(5)), "https://avatars.githubusercontent.com/u/5");
            assert_eq!(av::avatar_base_url("octocat", None), "https://avatars.githubusercontent.com/octocat");
        }

        #[test]
        fn gh_api_avatar_url_은_호스트를_고정하고_쿼리를_버린다() {
            let u9 = Some("https://avatars.githubusercontent.com/u/9?v=4");
            assert_eq!(av::normalize_avatar_url(u9, "x", Some(9)).as_deref(), Some("https://avatars.githubusercontent.com/u/9"));
            assert_eq!(av::normalize_avatar_url(u9, "x", None).as_deref(), Some("https://avatars.githubusercontent.com/u/9"));
            assert_eq!(av::normalize_avatar_url(Some("https://evil.example/u/9"), "x", Some(9)), None);
            assert_eq!(av::normalize_avatar_url(Some("http://avatars.githubusercontent.com/u/9"), "x", Some(9)), None);
            assert_eq!(av::normalize_avatar_url(Some("https://user:pw@avatars.githubusercontent.com/u/9"), "x", Some(9)), None);
            assert_eq!(av::normalize_avatar_url(Some(&format!("https://avatars.githubusercontent.com/u/{}", "9".repeat(300))), "x", None), None);
            assert_eq!(av::normalize_avatar_url(None, "x", None), None);
        }

        #[test]
        fn gh_아바타_요청_크기는_몇_단계로_묶인다() {
            assert_eq!((av::avatar_px(18.0, 1.0), av::avatar_px(18.0, 2.0), av::avatar_px(32.0, 2.0), av::avatar_px(500.0, 1.0)), (32, 48, 64, 192));
            assert_eq!(av::avatar_src("https://avatars.githubusercontent.com/u/1", 48), "https://avatars.githubusercontent.com/u/1?s=48");
            assert_eq!(av::avatar_src("https://x/y?a=1", 48), "https://x/y?a=1&s=48");
        }

        #[tokio::test]
        async fn gh_아바타는_noreply_를_네트워크_없이_풀고_github_저장소가_아니어도_동작한다() {
            let _g = Guard;
            let m = Mock::start(|_, _| status(404, json!({})));
            let r = resolver(&m.base(), None, FakeClock::at(CLOCK0), None);
            let out = r.lookup_authors(None, &[query_of("12+me@users.noreply.github.com", &sha_of('a')), query_of("x@y.io", &sha_of('b'))], &CancellationToken::new(), None).await;
            assert_eq!(out, vec![("12+me@users.noreply.github.com".to_string(), avatar("me", "https://avatars.githubusercontent.com/u/12"))]);
            assert_eq!(m.requests().len(), 0);
        }

        #[tokio::test]
        async fn gh_아바타는_sha_로_login_을_찾고_author_null_은_null_이며_이메일은_보내지_않는다() {
            let _g = Guard;
            let (a, b) = (sha_of('a'), sha_of('b'));
            let (a2, b2) = (a.clone(), b.clone());
            let m = Mock::start(move |_, r| {
                let sha = sha_query(r);
                if sha == a2 {
                    commit(&a2, json!({"login": "octocat", "id": 583231, "avatar_url": "https://avatars.githubusercontent.com/u/583231?v=4"}))
                } else if sha == b2 {
                    commit(&b2, Value::Null)
                } else {
                    status(404, json!({}))
                }
            });
            let r = resolver(&m.base(), None, FakeClock::at(CLOCK0), None);
            let out = r.lookup_authors(Some(&slug()), &[query_of("octo@corp.io", &a), query_of("ghost@corp.io", &b)], &CancellationToken::new(), None).await;
            assert_eq!(out, vec![("octo@corp.io".to_string(), avatar("octocat", "https://avatars.githubusercontent.com/u/583231")), ("ghost@corp.io".to_string(), None)]);
            assert_eq!(m.requests().len(), 2);
            for c in m.requests() {
                assert_eq!(path_of(&c), "/repos/octo/hello/commits");
                assert_eq!(qmap(&c)["per_page"], "1");
                assert!(!c.target.contains("corp.io") && !c.target.contains("%40"), "{}", c.target);
            }
        }

        #[tokio::test]
        async fn gh_아바타는_캐시_적중이면_다시_부르지_않고_파일_캐시는_재시작_뒤에도_쓴다_이메일은_해시로만_저장한다() {
            let _g = Guard;
            let dir = TempDir::new("avatar");
            let file = dir.0.join("avatar-cache.json");
            let a = sha_of('a');
            let a2 = a.clone();
            let m = Mock::start(move |_, _| commit(&a2, json!({"login": "octocat", "id": 1, "avatar_url": "https://avatars.githubusercontent.com/u/1?v=4"})));
            let cancel = CancellationToken::new();
            let r1 = resolver(&m.base(), None, FakeClock::at(CLOCK0), Some(file.clone()));
            r1.lookup_authors(Some(&slug()), &[query_of("octo@corp.io", &a)], &cancel, None).await;
            r1.lookup_authors(Some(&slug()), &[query_of("OCTO@corp.io", &a)], &cancel, None).await;
            assert_eq!(m.requests().len(), 1);
            r1.flush();
            let text = std::fs::read_to_string(&file).unwrap();
            assert!(!text.contains("corp.io"), "{text}");
            // 키는 이메일(소문자)의 sha256 16진수 64자다
            assert!(serde_json::from_str::<Value>(&text).unwrap()["e"].as_object().unwrap().keys().all(|k| k.len() == 64));

            let again = Mock::start(|_, _| status(404, json!({})));
            let r2 = resolver(&again.base(), None, FakeClock::at(CLOCK0), Some(file));
            let out = r2.lookup_authors(Some(&slug()), &[query_of("octo@corp.io", &a)], &cancel, None).await;
            assert_eq!(out[0].1.as_ref().map(|v| v.login.as_str()), Some("octocat"));
            assert_eq!(again.requests().len(), 0);
        }

        #[tokio::test]
        async fn gh_아바타는_부정은_7일_긍정은_30일_뒤_다시_조회한다() {
            let _g = Guard;
            let (a, b) = (sha_of('a'), sha_of('b'));
            let (a2, b2) = (a.clone(), b.clone());
            let m = Mock::start(move |_, r| if sha_query(r) == a2 { commit(&a2, Value::Null) } else { commit(&b2, json!({"login": "me", "id": 2, "avatar_url": ""})) });
            let t0: u64 = 1_000_000_000_000;
            let clock = FakeClock::at(t0);
            let r = resolver(&m.base(), None, clock.clone(), None);
            let authors = [query_of("n@x.io", &a), query_of("p@x.io", &b)];
            let cancel = CancellationToken::new();
            let day = 86_400_000;
            r.lookup_authors(Some(&slug()), &authors, &cancel, None).await;
            assert_eq!(m.requests().len(), 2);
            clock.set(t0 + 6 * day);
            r.lookup_authors(Some(&slug()), &authors, &cancel, None).await;
            assert_eq!(m.requests().len(), 2);
            clock.set(t0 + 8 * day);
            r.lookup_authors(Some(&slug()), &authors, &cancel, None).await;
            assert_eq!(m.requests().len(), 3);
            clock.set(t0 + 33 * day);
            r.lookup_authors(Some(&slug()), &authors, &cancel, None).await;
            assert_eq!(m.requests().len(), 5);
        }

        #[tokio::test]
        async fn gh_아바타는_미푸시_커밋_404_를_기록하지_않고_결과에서_빼며_잠시_뒤_다시_묻는다() {
            let _g = Guard;
            let m = Mock::start(|_, _| rate_headers(status(404, json!({"message": "Not Found"})), 50, 1_900_000_000));
            let t0: u64 = 1_000_000_000_000;
            let clock = FakeClock::at(t0);
            let r = resolver(&m.base(), None, clock.clone(), None);
            let a = [query_of("x@corp.io", &sha_of('a'))];
            let cancel = CancellationToken::new();
            assert_eq!(r.lookup_authors(Some(&slug()), &a, &cancel, None).await, vec![]);
            r.lookup_authors(Some(&slug()), &a, &cancel, None).await;
            assert_eq!(m.requests().len(), 1);
            clock.set(t0 + 11 * 60_000);
            r.lookup_authors(Some(&slug()), &a, &cancel, None).await;
            assert_eq!(m.requests().len(), 2);
        }

        #[tokio::test]
        async fn gh_아바타는_고른_커밋이_404_면_같은_작성자의_push_된_커밋으로_다시_묻는다() {
            let _g = Guard;
            let (a, b, c) = (sha_of('a'), sha_of('b'), sha_of('c'));
            let c2 = c.clone();
            let m = Mock::start(move |_, r| {
                if sha_query(r) == c2 {
                    commit(&c2, json!({"login": "dev", "id": 7, "avatar_url": "https://avatars.githubusercontent.com/u/7?v=4"}))
                } else {
                    rate_headers(status(404, json!({"message": "Not Found"})), 50, 1_900_000_000)
                }
            });
            let asked = Arc::new(Mutex::new(Vec::<String>::new()));
            let (asked2, list) = (asked.clone(), vec![a.clone(), b.clone(), c.clone()]);
            // 원래 sha(a)는 빼고 앞에서 2개(b, c)까지 쓴다
            let fallback: Fallback = Arc::new(move |email: String| {
                asked2.lock().unwrap().push(email);
                let list = list.clone();
                async move { list }.boxed()
            });
            let r = resolver(&m.base(), None, FakeClock::at(CLOCK0), None);
            let out = r.lookup_authors(Some(&slug()), &[query_of("dev@corp.io", &a)], &CancellationToken::new(), Some(fallback)).await;
            assert_eq!(out, vec![("dev@corp.io".to_string(), avatar("dev", "https://avatars.githubusercontent.com/u/7"))]);
            assert_eq!(*asked.lock().unwrap(), ["dev@corp.io"]);
            assert_eq!(m.requests().iter().map(sha_query).collect::<Vec<_>>(), [a, b, c]);
            // 이메일은 로컬 대체 조회에만 쓰이고 GitHub 요청에는 실리지 않는다
            assert!(m.requests().iter().all(|r| !r.target.contains("corp.io")));
        }

        #[tokio::test]
        async fn gh_아바타는_404_가_아닌_결과면_대체_조회를_하지_않고_대체_후보도_2개까지만_쓴다() {
            let _g = Guard;
            let (a, b) = (sha_of('a'), sha_of('b'));
            let a2 = a.clone();
            let m = Mock::start(move |_, r| {
                if sha_query(r) == a2 {
                    commit(&a2, Value::Null)
                } else {
                    rate_headers(status(404, json!({"message": "Not Found"})), 50, 1_900_000_000)
                }
            });
            let calls = Arc::new(AtomicUsize::new(0));
            let c2 = calls.clone();
            let fallback: Fallback = Arc::new(move |_email: String| {
                c2.fetch_add(1, Ordering::SeqCst);
                async { vec![sha_of('d'), sha_of('e'), sha_of('f')] }.boxed()
            });
            let r = resolver(&m.base(), None, FakeClock::at(CLOCK0), None);
            let out = r.lookup_authors(Some(&slug()), &[query_of("none@corp.io", &a), query_of("gone@corp.io", &b)], &CancellationToken::new(), Some(fallback)).await;
            assert_eq!(out, vec![("none@corp.io".to_string(), None)]);
            // none: 1회(대체 없음) · gone: 원래 1회 + 대체 2회
            assert_eq!(calls.load(Ordering::SeqCst), 1);
            assert_eq!(count(&m, |r| path_of(r) == "/repos/octo/hello/commits"), 4);
        }

        #[tokio::test]
        async fn gh_아바타는_남은_호출이_10_미만이면_멈추고_reset_이후_재개한다() {
            let _g = Guard;
            let reset: u64 = 1_900_000_000;
            let m = Mock::start(move |n, r| rate_headers(ok(json!([{"sha": sha_query(r), "author": {"login": format!("u{n}"), "id": n + 1, "avatar_url": ""}}])), 9, reset));
            let clock = FakeClock::at(reset * 1000 - 3_600_000);
            let r = resolver(&m.base(), None, clock.clone(), None);
            let cancel = CancellationToken::new();
            let first = r.lookup_authors(Some(&slug()), &[query_of("1@x.io", &sha_of('a')), query_of("2@x.io", &sha_of('b')), query_of("3@x.io", &sha_of('c'))], &cancel, None).await;
            // 동시성 3 이라 첫 묶음은 모두 나갈 수 있다. 중요한 것은 이후 새 요청이 한도 때문에 막힌다는 점
            let n = m.requests().len();
            assert!(n > 0);
            let again = [query_of("9@x.io", &sha_of('d'))];
            assert_eq!(r.lookup_authors(Some(&slug()), &again, &cancel, None).await, vec![]);
            assert_eq!(m.requests().len(), n);
            assert_eq!(first.len(), n);
            clock.set((reset + 1) * 1000);
            r.lookup_authors(Some(&slug()), &again, &cancel, None).await;
            assert_eq!(m.requests().len(), n + 1);
        }

        #[tokio::test]
        async fn gh_아바타는_한도_초과_응답이면_일시_중지한다() {
            let _g = Guard;
            let m = Mock::start(|_, _| rate_headers(status(403, json!({"message": "rate limited"})), 0, 1_900_000_000));
            let r = resolver(&m.base(), None, FakeClock::at(CLOCK0), None);
            let cancel = CancellationToken::new();
            assert_eq!(r.lookup_authors(Some(&slug()), &[query_of("1@x.io", &sha_of('a'))], &cancel, None).await, vec![]);
            r.lookup_authors(Some(&slug()), &[query_of("2@x.io", &sha_of('b'))], &cancel, None).await;
            assert_eq!(m.requests().len(), 1);
            let block = r.blocked_for(Some(&slug())).unwrap();
            assert_eq!((block.reason, block.signed_in), ("rate-limited", false));
        }

        #[tokio::test]
        async fn gh_아바타는_동시_호출을_3개로_제한하고_같은_작성자의_동시_요청은_합친다() {
            let _g = Guard;
            let (active, peak) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
            let (a, p) = (active.clone(), peak.clone());
            let m = Mock::start(move |_, r| {
                let now = a.fetch_add(1, Ordering::SeqCst) + 1;
                p.fetch_max(now, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(15));
                a.fetch_sub(1, Ordering::SeqCst);
                let sha = sha_query(r);
                commit(&sha, json!({"login": "u", "id": 1, "avatar_url": ""}))
            });
            let r = resolver(&m.base(), None, FakeClock::at(CLOCK0), None);
            let authors: Vec<AvatarQuery> = (0..8).map(|i| query_of(&format!("u{i}@x.io"), &i.to_string().repeat(40))).collect();
            let cancel = CancellationToken::new();
            let sl = slug();
            tokio::join!(r.lookup_authors(Some(&sl), &authors, &cancel, None), r.lookup_authors(Some(&sl), &authors, &cancel, None));
            assert!(peak.load(Ordering::SeqCst) <= 3, "동시 {}개", peak.load(Ordering::SeqCst));
            assert_eq!(m.requests().len(), 8);
        }

        #[tokio::test]
        async fn gh_아바타는_취소되면_더_호출하지_않는다() {
            let _g = Guard;
            let m = Mock::start(|_, _| status(404, json!({})));
            let r = resolver(&m.base(), None, FakeClock::at(CLOCK0), None);
            let cancel = CancellationToken::new();
            cancel.cancel();
            assert_eq!(r.lookup_authors(Some(&slug()), &[query_of("a@x.io", &sha_of('a'))], &cancel, None).await, vec![]);
            assert_eq!(m.requests().len(), 0);
        }

        #[tokio::test]
        async fn gh_서비스_아바타는_remote_가_있으면_sha_로_조회하고_토큰과_이메일이_새지_않는다() {
            let _g = Guard;
            let a = sha_of('a');
            let a2 = a.clone();
            let m = Mock::start(move |_, _| ok(json!([{"sha": a2, "author": {"login": "octocat", "id": 1, "avatar_url": "https://avatars.githubusercontent.com/u/1?v=4"}}])));
            let e = env(&m);
            let res = e.svc.avatars(&json!("r1"), &json!("req-1"), &json!([{"email": "o@corp.io", "sha": a}])).await.unwrap();
            assert_eq!(jv(&res), json!({"github": true, "avatars": {"o@corp.io": {"login": "octocat", "url": "https://avatars.githubusercontent.com/u/1"}}}));
            assert_eq!(m.requests().len(), 1);
            assert!(!m.requests()[0].target.contains("corp.io"));
        }

        #[tokio::test]
        async fn gh_서비스_아바타는_github_저장소가_아니면_github_false_이고_noreply_는_풀린다() {
            let _g = Guard;
            let m = Mock::start(|_, _| status(404, json!({})));
            let e = env_on(&m.base(), new_vault(), FakeRepos::new().no_repo(), 0);
            let res = e.svc.avatars(&json!("r1"), &json!("req-2"), &json!([{"email": "3+me@users.noreply.github.com", "sha": sha_of('a')}, {"email": "o@corp.io", "sha": sha_of('b')}])).await.unwrap();
            let v = jv(&res);
            assert_eq!(v["github"], json!(false));
            assert_eq!(v["avatars"].as_object().unwrap().keys().collect::<Vec<_>>(), ["3+me@users.noreply.github.com"]);
            assert_eq!(m.requests().len(), 0);
        }

        // ── 접근 가능한 저장소(ghAccessibleRepos) ───────────────────────────

        fn repo_raw(owner: &str, name: &str, pushed: &str, extra: Value) -> Value {
            merged(
                json!({
                    "name": name, "full_name": format!("{owner}/{name}"), "owner": {"login": owner, "type": "User"}, "private": false, "fork": false, "archived": false,
                    "default_branch": "main", "description": null, "pushed_at": pushed, "html_url": format!("https://github.com/{owner}/{name}")
                }),
                extra,
            )
        }

        fn page_of(r: &Req) -> u64 {
            qmap(r).get("page").and_then(|p| p.parse().ok()).unwrap_or(1)
        }

        const NEXT_REPOS: &str = "<https://api.github.com/user/repos?page=2>; rel=\"next\"";

        #[tokio::test]
        async fn gh_접근_가능한_저장소는_응답을_매핑하고_pushed_at_내림차순으로_정렬하며_이상한_항목은_건너뛴다() {
            let _g = Guard;
            let body = json!([
                repo_raw("me", "old", "2026-01-01T00:00:00Z", json!({"description": "옛것"})),
                null,
                {"name": "noowner", "html_url": "https://github.com/x/noowner"},
                repo_raw("acme", "new", "2026-03-01T00:00:00Z", json!({"private": true, "fork": true, "archived": true, "default_branch": "trunk", "owner": {"login": "acme", "type": "Organization"}}))
            ]);
            let m = Mock::start(move |_, _| ok(body.clone()));
            let e = signed_in_env(&m, "ghp_tok", GithubMethod::Pat);
            let res = jv(&e.svc.accessible_repos(&json!("r1"), &json!(false)).await.unwrap());
            assert_eq!((res["needsLogin"].clone(), res["truncated"].clone()), (json!(false), json!(false)));
            assert_eq!(res["repos"].as_array().unwrap().iter().map(|r| r["fullName"].clone()).collect::<Vec<_>>(), [json!("acme/new"), json!("me/old")]);
            let pushed = parse_date_ms("2026-03-01T00:00:00Z").unwrap();
            assert_eq!(
                res["repos"][0],
                json!({"owner": "acme", "repo": "new", "fullName": "acme/new", "ownerType": "Organization", "private": true, "fork": true, "archived": true, "defaultBranch": "trunk", "description": null, "pushedAt": pushed as u64, "htmlUrl": "https://github.com/acme/new"})
            );
            assert_eq!((res["repos"][1]["ownerType"].clone(), res["repos"][1]["description"].clone()), (json!("User"), json!("옛것")));
            let q = &m.requests()[0];
            assert_eq!(path_of(q), "/user/repos");
            assert_eq!((qmap(q)["affiliation"].as_str(), qmap(q)["per_page"].as_str()), ("owner,collaborator,organization_member", "100"));
            assert_eq!(q.header("authorization"), Some("Bearer ghp_tok"));
        }

        #[tokio::test]
        async fn gh_접근_가능한_저장소는_link_next_가_있으면_다음_쪽까지_읽어_합친다() {
            let _g = Guard;
            let m = Mock::start(|_, r| if page_of(r) == 1 { ok(json!([repo_raw("me", "a", "2026-01-01T00:00:00Z", json!({}))])).header("link", NEXT_REPOS) } else { ok(json!([repo_raw("me", "b", "2026-02-01T00:00:00Z", json!({}))])) });
            let e = signed_in_env(&m, "ghp_tok", GithubMethod::Pat);
            let res = jv(&e.svc.accessible_repos(&json!("r1"), &json!(false)).await.unwrap());
            assert_eq!(m.requests().len(), 2);
            assert_eq!(res["repos"].as_array().unwrap().iter().map(|r| r["repo"].clone()).collect::<Vec<_>>(), [json!("b"), json!("a")]);
            assert_eq!(res["truncated"], json!(false));
        }

        #[tokio::test]
        async fn gh_접근_가능한_저장소는_쪽_수_상한에_걸리면_거기까지만_돌려주고_truncated_로_알린다() {
            let _g = Guard;
            let m = Mock::start(|_, r| ok(json!([repo_raw("me", &format!("r{}", page_of(r)), "2026-01-01T00:00:00Z", json!({}))])).header("link", NEXT_REPOS));
            let e = signed_in_env(&m, "ghp_tok", GithubMethod::Pat);
            let res = jv(&e.svc.accessible_repos(&json!("r1"), &json!(false)).await.unwrap());
            assert_eq!(m.requests().len(), 20);
            assert_eq!(res["repos"].as_array().unwrap().len(), 20);
            assert_eq!(res["truncated"], json!(true));
        }

        #[tokio::test]
        async fn gh_접근_가능한_저장소는_로그인하지_않았으면_조회_없이_needs_login() {
            let _g = Guard;
            let m = Mock::start(|_, _| ok(json!([])));
            let e = env(&m);
            let res = jv(&e.svc.accessible_repos(&json!("r1"), &json!(false)).await.unwrap());
            assert_eq!((res["repos"].clone(), res["needsLogin"].clone(), res["truncated"].clone()), (json!([]), json!(true), json!(false)));
            assert_eq!(m.requests().len(), 0);
        }

        #[tokio::test]
        async fn gh_접근_가능한_저장소는_5분_캐시를_쓰고_fresh_면_다시_읽는다() {
            let _g = Guard;
            let m = Mock::start(|_, _| ok(json!([repo_raw("me", "a", "2026-01-01T00:00:00Z", json!({}))])));
            let e = signed_in_env(&m, "ghp_tok", GithubMethod::Pat);
            e.svc.accessible_repos(&json!("r1"), &json!(false)).await.unwrap();
            e.svc.accessible_repos(&json!("r2"), &json!(false)).await.unwrap();
            assert_eq!(m.requests().len(), 1);
            e.svc.accessible_repos(&json!("r3"), &json!(true)).await.unwrap();
            assert_eq!(m.requests().len(), 2);
        }

        #[tokio::test]
        async fn gh_접근_가능한_저장소는_로그아웃하면_캐시를_버린다() {
            let _g = Guard;
            let m = Mock::start(|_, _| ok(json!([repo_raw("me", "a", "2026-01-01T00:00:00Z", json!({}))])));
            let e = signed_in_env(&m, "ghp_tok", GithubMethod::Pat);
            e.svc.accessible_repos(&json!("r1"), &json!(false)).await.unwrap();
            e.svc.logout().await.unwrap();
            let res = jv(&e.svc.accessible_repos(&json!("r2"), &json!(false)).await.unwrap());
            assert_eq!(res["needsLogin"], json!(true));
            assert_eq!(m.requests().len(), 1);
        }

        #[tokio::test]
        async fn gh_접근_가능한_저장소는_요청을_취소하면_aborted_로_끝나고_캐시에_남기지_않는다() {
            let _g = Guard;
            let gate = Gate::default();
            let g2 = gate.clone();
            let m = Mock::start(move |_, r| {
                if page_of(r) == 1 {
                    g2.wait();
                    ok(json!([repo_raw("me", "a", "2026-01-01T00:00:00Z", json!({}))])).header("link", NEXT_REPOS)
                } else {
                    ok(json!([]))
                }
            });
            let e = signed_in_env(&m, "ghp_tok", GithubMethod::Pat);
            let svc = e.svc.clone();
            let first = tokio::spawn(async move { svc.accessible_repos(&json!("r1"), &json!(false)).await });
            wait_for(|| !m.requests().is_empty()).await;
            e.svc.cancel("r1");
            assert_eq!(failure(within(first).await.unwrap()).code, "ABORTED");
            gate.open();
            let res = jv(&e.svc.accessible_repos(&json!("r2"), &json!(false)).await.unwrap());
            assert_eq!(res["repos"].as_array().unwrap().len(), 1);
            // 취소된 호출의 결과가 캐시에 남았다면 두 번째 호출은 네트워크를 타지 않는다
            assert!(m.requests().len() >= 2);
        }

        #[tokio::test]
        async fn gh_접근_가능한_저장소는_401_이면_user_로_다시_확인해_거부된_토큰을_버린다() {
            let _g = Guard;
            let m = Mock::start(|_, _| bad_credentials());
            let e = signed_in_env(&m, "ghp_tok", GithubMethod::Pat);
            assert_eq!(failure(e.svc.accessible_repos(&json!("r1"), &json!(false)).await).code, "GH_AUTH");
            assert!(e.svc.git_token().is_none());
        }

        #[tokio::test]
        async fn gh_git_token_은_로그인_토큰을_돌려주고_로그인하지_않았으면_none_이다() {
            let _g = Guard;
            let m = Mock::start(|_, _| ok(json!([])));
            assert_eq!(signed_in_env(&m, "ghp_tok", GithubMethod::Pat).svc.git_token().unwrap().expose(), "ghp_tok");
            assert!(env(&m).svc.git_token().is_none());
        }

        // ── 체크(ghChecks) ──────────────────────────────────────────────────

        const TOKEN: &str = "ghp_supersecrettoken";

        fn sha(n: u64) -> String {
            format!("{n:040x}")
        }

        /// GraphQL 요청에서 (변수 o{i} → sha) 를 읽어 sha 별 응답을 만든다
        fn graphql_handler(rollups: Arc<Mutex<BTreeMap<String, Option<&'static str>>>>) -> impl Fn(usize, &Req) -> Resp + Send + Sync + 'static {
            move |_, r| {
                if !path_of(r).ends_with("/graphql") {
                    return status(404, json!({}));
                }
                let body: Value = serde_json::from_str(&r.text()).unwrap();
                let vars = body["variables"].as_object().unwrap();
                let mut repository = serde_json::Map::new();
                let mut i = 0;
                while let Some(s) = vars.get(&format!("o{i}")).and_then(Value::as_str) {
                    let v = match rollups.lock().unwrap().get(s) {
                        None => Value::Null,
                        Some(state) => json!({"statusCheckRollup": state.map(|st| json!({"state": st}))}),
                    };
                    repository.insert(format!("c{i}"), v);
                    i += 1;
                }
                ok(json!({"data": {"repository": repository}}))
            }
        }

        fn rollups(list: &[(u64, Option<&'static str>)]) -> Arc<Mutex<BTreeMap<String, Option<&'static str>>>> {
            Arc::new(Mutex::new(list.iter().map(|(n, s)| (sha(*n), *s)).collect()))
        }

        fn cache(clock: Arc<FakeClock>) -> CheckStateCache {
            CheckStateCache::new(clock)
        }

        #[test]
        fn gh_체크_캐시는_이미_있는_키를_갱신해도_다른_항목을_밀어내지_않는다() {
            let c = cache(FakeClock::at(CLOCK0));
            for i in 0..5000 {
                c.set(&format!("k{i}"), "success");
            }
            c.set("k4999", "failure");
            assert_eq!((c.get("k0"), c.get("k4999")), (Some("success"), Some("failure")));
            // 새 키가 들어오면 가장 오래된 것부터 밀려난다
            c.set("k5000", "success");
            assert_eq!((c.get("k0"), c.get("k1")), (None, Some("success")));
        }

        #[test]
        fn gh_체크_캐시는_완료_상태는_오래_진행_중은_짧게_없음은_중간_길이로_보관한다() {
            let clock = FakeClock::at(1_000_000);
            let c = cache(clock.clone());
            c.set("a", "success");
            c.set("b", "pending");
            c.set("c", "none");
            clock.set(1_000_000 + TTL_PENDING_MS as u64);
            assert_eq!((c.get("a"), c.get("b"), c.get("c")), (Some("success"), None, Some("none")));
            clock.set(1_000_000 + TTL_NONE_MS as u64);
            assert_eq!((c.get("c"), c.get("a")), (None, Some("success")));
            clock.set(1_000_000 + TTL_NONE_MS as u64 + TTL_DONE_MS as u64);
            assert_eq!(c.get("a"), None);
        }

        #[tokio::test]
        async fn gh_일괄_체크는_여러_커밋을_한_번의_요청으로_묻고_성공_실패_진행_중_없음_미push_를_구분한다() {
            let _g = Guard;
            let m = Mock::start(graphql_handler(rollups(&[(1, Some("SUCCESS")), (2, Some("FAILURE")), (3, Some("PENDING")), (4, None)])));
            let c = gh(&m.base(), Some(TOKEN), 5000, 0);
            let shas: Vec<String> = (1..=5).map(sha).collect();
            let out = jv(&checks::fetch_commit_checks(&c, &slug(), &shas, &cache(FakeClock::at(CLOCK0)), &CancellationToken::new()).await.unwrap());
            assert_eq!(m.requests().len(), 1);
            assert_eq!(m.requests()[0].method, "POST");
            assert_eq!(out, json!({sha(1): "success", sha(2): "failure", sha(3): "pending", sha(4): "none", sha(5): "none"}));
        }

        #[tokio::test]
        async fn gh_일괄_체크는_sha_를_쿼리_문자열이_아니라_변수로_보낸다() {
            let _g = Guard;
            let m = Mock::start(graphql_handler(rollups(&[])));
            let c = gh(&m.base(), Some(TOKEN), 5000, 0);
            checks::fetch_commit_checks(&c, &slug(), &[sha(7)], &cache(FakeClock::at(CLOCK0)), &CancellationToken::new()).await.unwrap();
            let body: Value = serde_json::from_str(&m.requests()[0].text()).unwrap();
            assert!(!body["query"].as_str().unwrap().contains(&sha(7)));
            assert_eq!((body["variables"]["owner"].clone(), body["variables"]["name"].clone(), body["variables"]["o0"].clone()), (json!("octo"), json!("hello"), json!(sha(7))));
        }

        #[tokio::test]
        async fn gh_일괄_체크는_50개씩_나눠_묻고_잘못된_sha_와_중복은_버린다() {
            let _g = Guard;
            let m = Mock::start(graphql_handler(rollups(&[])));
            let c = gh(&m.base(), Some(TOKEN), 5000, 0);
            let mut shas: Vec<String> = (1..=55).map(sha).collect();
            shas.extend([sha(1), "xyz".to_string(), "; drop".to_string()]);
            let out = jv(&checks::fetch_commit_checks(&c, &slug(), &shas, &cache(FakeClock::at(CLOCK0)), &CancellationToken::new()).await.unwrap());
            assert_eq!(m.requests().len(), 2);
            assert_eq!(out.as_object().unwrap().len(), 55);
        }

        #[tokio::test]
        async fn gh_일괄_체크는_캐시된_커밋은_다시_묻지_않고_진행_중은_ttl_이_지나면_다시_묻는다() {
            let _g = Guard;
            let state = rollups(&[(1, Some("SUCCESS")), (2, Some("PENDING"))]);
            let m = Mock::start(graphql_handler(state.clone()));
            let c = gh(&m.base(), Some(TOKEN), 5000, 0);
            let t0: u64 = 5_000_000;
            let clock = FakeClock::at(t0);
            let cache = cache(clock.clone());
            let shas = [sha(1), sha(2)];
            let cancel = CancellationToken::new();
            checks::fetch_commit_checks(&c, &slug(), &shas, &cache, &cancel).await.unwrap();
            assert_eq!(m.requests().len(), 1);
            checks::fetch_commit_checks(&c, &slug(), &shas, &cache, &cancel).await.unwrap();
            assert_eq!(m.requests().len(), 1);
            // 진행 중이던 커밋이 끝났다
            state.lock().unwrap().insert(sha(2), Some("SUCCESS"));
            clock.set(t0 + TTL_PENDING_MS as u64 + 1);
            let out = jv(&checks::fetch_commit_checks(&c, &slug(), &shas, &cache, &cancel).await.unwrap());
            assert_eq!(m.requests().len(), 2);
            let vars: Value = serde_json::from_str::<Value>(&m.requests()[1].text()).unwrap()["variables"].clone();
            assert_eq!((vars["o0"].clone(), vars.get("o1").is_none()), (json!(sha(2)), true));
            assert_eq!(out, json!({sha(1): "success", sha(2): "success"}));
        }

        #[tokio::test]
        async fn gh_일괄_체크는_레이트_리밋을_gh_rate_limit_로_던진다() {
            let _g = Guard;
            let limited = Mock::start(|_, _| ok(json!({"errors": [{"type": "RATE_LIMITED", "message": "x"}]})));
            let c = gh(&limited.base(), Some(TOKEN), 5000, 0);
            let err = failure(checks::fetch_commit_checks(&c, &slug(), &[sha(1)], &cache(FakeClock::at(CLOCK0)), &CancellationToken::new()).await);
            assert_eq!(err.code, "GH_RATE_LIMIT");
            let forbidden = Mock::start(|_, _| rate(status(403, json!({"message": "rate"})), 0));
            let c = gh(&forbidden.base(), Some(TOKEN), 5000, 0);
            let err = failure(checks::fetch_commit_checks(&c, &slug(), &[sha(1)], &cache(FakeClock::at(CLOCK0)), &CancellationToken::new()).await);
            assert_eq!(err.code, "GH_RATE_LIMIT");
        }

        #[tokio::test]
        async fn gh_일괄_체크는_토큰이_없으면_요청하지_않고_gh_auth_로_거절한다() {
            let _g = Guard;
            let m = Mock::start(|_, _| ok(json!({})));
            let c = gh(&m.base(), None, 5000, 0);
            let err = failure(checks::fetch_commit_checks(&c, &slug(), &[sha(1)], &cache(FakeClock::at(CLOCK0)), &CancellationToken::new()).await);
            assert_eq!(err.code, "GH_AUTH");
            assert_eq!(m.requests().len(), 0);
        }

        #[tokio::test]
        async fn gh_일괄_체크는_저장소를_못_보면_결과에서_빠지고_캐시하지_않는다() {
            let _g = Guard;
            let m = Mock::start(|_, _| ok(json!({"data": {"repository": null}})));
            let c = gh(&m.base(), Some(TOKEN), 5000, 0);
            let cache = cache(FakeClock::at(CLOCK0));
            let out = jv(&checks::fetch_commit_checks(&c, &slug(), &[sha(1)], &cache, &CancellationToken::new()).await.unwrap());
            assert_eq!(out, json!({}));
            assert_eq!(cache.get(&format!("octo/hello@{}", sha(1))), None);
        }

        #[tokio::test]
        async fn gh_커밋_상세는_check_run_과_legacy_status_를_합치고_소요_시간_결론_링크를_채운다() {
            let _g = Guard;
            let m = Mock::start(|_, r| {
                if path_of(r).ends_with("/check-runs") {
                    ok(json!({"check_runs": [
                        {"name": "build", "status": "completed", "conclusion": "success", "html_url": "https://github.com/octo/hello/runs/1", "started_at": "2026-01-01T00:00:00Z", "completed_at": "2026-01-01T00:01:05Z"},
                        {"name": "lint", "status": "in_progress", "conclusion": null, "started_at": "2026-01-01T00:00:00Z"},
                        {"name": "e2e", "status": "completed", "conclusion": "failure"}
                    ]}))
                } else {
                    ok(json!({"statuses": [{"context": "ci/legacy", "state": "success", "target_url": "https://ci.example/1"}]}))
                }
            });
            let c = gh(&m.base(), Some(TOKEN), 5000, 0);
            let out = jv(&checks::fetch_check_details(&c, &slug(), &sha(9).to_uppercase(), &CancellationToken::new()).await.unwrap());
            let mut targets: Vec<String> = m.requests().iter().map(|r| r.target.clone()).collect();
            targets.sort();
            assert_eq!(targets, [format!("/repos/octo/hello/commits/{}/check-runs?per_page=100", sha(9)), format!("/repos/octo/hello/commits/{}/status?per_page=100", sha(9))]);
            assert_eq!(
                out,
                json!([
                    {"name": "build", "state": "success", "conclusion": "success", "durationSec": 65, "url": "https://github.com/octo/hello/runs/1"},
                    {"name": "lint", "state": "pending"},
                    {"name": "e2e", "state": "failure", "conclusion": "failure"},
                    {"name": "ci/legacy", "state": "success", "url": "https://ci.example/1"}
                ])
            );
            assert!(failure(checks::fetch_check_details(&c, &slug(), "../x", &CancellationToken::new()).await).message.contains("커밋 해시"));
        }

        #[tokio::test]
        async fn gh_워크플로_실행과_잡_목록을_변환한다_비로그인도_요청_가능() {
            let _g = Guard;
            let m = Mock::start(|_, r| {
                if path_of(r).contains("/actions/runs/77/jobs") {
                    ok(json!({"jobs": [
                        {"id": 1, "name": "test", "status": "completed", "conclusion": "cancelled", "started_at": "2026-01-01T00:00:00Z", "completed_at": "2026-01-01T01:02:03Z", "html_url": "https://github.com/o/r/job/1"},
                        {"id": 2, "name": "deploy", "status": "queued", "conclusion": null}
                    ]}))
                } else {
                    ok(json!({"workflow_runs": [{"id": 77, "name": "CI", "display_title": "fix: x", "head_branch": "main", "head_sha": sha(1), "event": "push", "status": "completed", "conclusion": "success", "run_number": 12, "created_at": "2026-01-01T00:00:00Z", "updated_at": "2026-01-01T00:05:00Z", "html_url": "https://github.com/o/r/actions/runs/77"}]})).header("link", NEXT)
                }
            });
            let c = gh(&m.base(), None, 5000, 0);
            let cancel = CancellationToken::new();
            let runs = jv(&checks::list_runs(&c, &slug(), 1, &cancel).await.unwrap());
            assert_eq!(m.requests()[0].header("authorization"), None);
            assert_eq!(runs["hasMore"], json!(true));
            let first = &runs["items"][0];
            assert_eq!(
                (first["id"].clone(), first["name"].clone(), first["title"].clone(), first["branch"].clone(), first["event"].clone(), first["state"].clone(), first["number"].clone(), first["conclusion"].clone()),
                (json!(77), json!("CI"), json!("fix: x"), json!("main"), json!("push"), json!("success"), json!(12), json!("success"))
            );
            let jobs = jv(&checks::list_jobs(&c, &slug(), 77, &cancel).await.unwrap());
            assert_eq!(
                jobs,
                json!([
                    {"id": 1, "name": "test", "state": "failure", "status": "completed", "conclusion": "cancelled", "durationSec": 3723, "htmlUrl": "https://github.com/o/r/job/1"},
                    {"id": 2, "name": "deploy", "state": "pending", "status": "queued", "htmlUrl": ""}
                ])
            );
        }

        #[tokio::test]
        async fn gh_서비스_체크는_로그인하지_않았으면_요청_없이_available_false() {
            let _g = Guard;
            let m = Mock::start(|_, _| ok(json!({})));
            let e = env(&m);
            let res = e.svc.commit_checks(&json!("r1"), &json!([sha(1)])).await.unwrap();
            assert_eq!(jv(&res), json!({"available": false, "states": {}}));
            assert_eq!(m.requests().len(), 0);
        }

        #[tokio::test]
        async fn gh_서비스_체크는_같은_커밋을_다시_물으면_캐시를_쓰고_결과_오류_어디에도_토큰이_없다() {
            let _g = Guard;
            let m = Mock::start(graphql_handler(rollups(&[(1, Some("SUCCESS"))])));
            let e = signed_in_env(&m, TOKEN, GithubMethod::Pat);
            let (id, shas) = (json!("r1"), json!([sha(1)]));
            let a = e.svc.commit_checks(&id, &shas).await.unwrap();
            let b = e.svc.commit_checks(&id, &shas).await.unwrap();
            assert_eq!(jv(&a), json!({"available": true, "states": {sha(1): "success"}}));
            assert_eq!(a, b);
            assert_eq!(m.requests().len(), 1);
            assert_eq!(m.requests()[0].header("authorization"), Some(format!("Bearer {TOKEN}").as_str()));
            assert!(!format!("{}{}", stringify(&a), stringify(&b)).contains(TOKEN));
            assert!(!m.requests()[0].text().contains(TOKEN));
            // 로그아웃하면 캐시도 비워져 다시 묻는 대신 available=false
            e.svc.logout().await.unwrap();
            assert_eq!(jv(&e.svc.commit_checks(&id, &shas).await.unwrap())["available"], json!(false));
        }

        #[tokio::test]
        async fn gh_서비스_체크의_레이트_리밋_오류_문구에_토큰이_실리지_않는다() {
            let _g = Guard;
            let m = Mock::start(|_, r| rate(status(403, json!({"message": format!("limit for {}", r.header("authorization").unwrap_or(""))})), 0));
            let e = signed_in_env(&m, TOKEN, GithubMethod::Pat);
            let err = failure(e.svc.commit_checks(&json!("r1"), &json!([sha(1)])).await);
            assert!(err.code.starts_with("GH_"), "{err}");
            assert!(!err.message.contains(TOKEN), "{err}");
        }

        // ── Launchpad(launchpadService) ─────────────────────────────────────

        fn remote(owner: &str, repo: &str, name: &str) -> GhRepoRef {
            GhRepoRef { owner: owner.into(), repo: repo.into(), remote: name.into() }
        }

        fn lp_issue(n: u64, repo: &str, over: Value) -> Value {
            merged(
                json!({
                    "number": n, "title": format!("item {n}"), "state": "open", "user": {"login": "me", "avatar_url": "https://avatars.githubusercontent.com/u/1"},
                    "labels": [{"name": "bug", "color": "ff0000"}], "assignees": [], "comments": 2, "updated_at": format!("2026-01-0{n}T00:00:00Z"), "created_at": "2026-01-01T00:00:00Z",
                    "html_url": format!("https://github.com/{repo}/pull/{n}"), "repository_url": format!("https://api.github.com/repos/{repo}")
                }),
                over,
            )
        }

        fn q_of(r: &Req) -> String {
            qmap(r).get("q").cloned().unwrap_or_default()
        }

        fn section_of(r: &Req) -> &'static str {
            let q = q_of(r);
            if q.contains("author:@me") {
                "authored"
            } else if q.contains("review-requested:@me") {
                "review"
            } else {
                "issues"
            }
        }

        fn lp_env<S: AsRef<str>>(m: &Mock, remotes: Vec<(S, Remotes)>, signed_in: bool) -> Env {
            let vault = new_vault();
            if signed_in {
                vault.save_github(&SecretString::new("ghp_tok"), GithubMethod::Pat).unwrap();
            }
            let opts = ServiceOptions { remote_timeout: Duration::from_millis(300), ..ServiceOptions::default() };
            env_opts(&m.base(), vault, FakeRepos::new().with_remotes(remotes), 0, opts)
        }

        fn lp_remotes() -> Vec<(&'static str, Remotes)> {
            vec![
                ("r1", Remotes::List(vec![remote("acme", "app", "origin")])),
                ("r2", Remotes::List(vec![remote("acme", "lib", "origin")])),
                ("r3", Remotes::List(vec![])),
                // 같은 GitHub 저장소를 대소문자만 다르게 등록한 클론
                ("r4", Remotes::List(vec![remote("Acme", "App", "origin")])),
                // 포크: origin 은 포크, upstream 이 원본
                ("r5", Remotes::List(vec![remote("me", "lib-fork", "origin"), remote("acme", "lib", "upstream")])),
            ]
        }

        #[tokio::test]
        async fn gh_launchpad_는_로그인하지_않았으면_호출_없이_needs_login_으로_알린다() {
            let _g = Guard;
            let m = Mock::start(|_, _| ok(json!({"items": []})));
            let e = lp_env(&m, lp_remotes(), false);
            let res = jv(&e.svc.launchpad(&json!("req1"), &Value::Null).await.unwrap());
            assert_eq!((res["needsLogin"].clone(), res["items"].clone(), res["errors"].clone()), (json!(true), json!([]), json!([])));
            assert_eq!(m.requests().len(), 0);
        }

        #[tokio::test]
        async fn gh_launchpad_는_사용자_기준_검색을_섹션당_한_번만_보내고_등록된_저장소의_remote_와_맞춰_거른다() {
            let _g = Guard;
            let m = Mock::start(|_, r| {
                assert_eq!(path_of(r), "/search/issues");
                assert_eq!(r.header("authorization"), Some("Bearer ghp_tok"));
                assert_eq!(qmap(r)["per_page"], "100");
                match section_of(r) {
                    "authored" => ok(json!({"items": [lp_issue(1, "acme/app", json!({"draft": true, "pull_request": {"merged_at": null}})), lp_issue(5, "stranger/other", json!({"pull_request": {}}))]})),
                    "review" => ok(json!({"items": [lp_issue(2, "acme/lib", json!({"pull_request": {}}))]})),
                    _ => ok(json!({"items": [lp_issue(3, "ACME/App", json!({})), lp_issue(4, "unknown/thing", json!({}))]})),
                }
            });
            let e = lp_env(&m, lp_remotes(), true);
            let res = e.svc.launchpad(&json!("req1"), &Value::Null).await.unwrap();
            assert_eq!(m.requests().len(), 3);
            let mut qs: Vec<String> = m.requests().iter().map(q_of).collect();
            qs.sort();
            assert_eq!(qs, ["is:issue is:open assignee:@me archived:false", "is:pr is:open author:@me archived:false", "is:pr is:open review-requested:@me archived:false"]);
            assert!(qs.iter().all(|q| !q.contains("repo:")));
            let v = jv(&res);
            assert_eq!((v["needsLogin"].clone(), v["incomplete"].clone(), v["skipped"].clone(), v["unresolved"].clone(), v["errors"].clone()), (json!(false), json!(false), json!(1), json!(0), json!([])));
            let items = v["items"].as_array().unwrap();
            assert_eq!(items.len(), 3);
            let by = |section: &str| items.iter().find(|i| i["section"] == json!(section)).unwrap().clone();
            // 대소문자만 다른 두 등록(r1·r4)은 같은 저장소로 묶인다
            let authored = by("authored");
            assert_eq!(
                (authored["kind"].clone(), authored["repoIds"].clone(), authored["owner"].clone(), authored["repo"].clone(), authored["number"].clone(), authored["draft"].clone(), authored["state"].clone()),
                (json!("pull"), json!(["r1", "r4"]), json!("acme"), json!("app"), json!(1), json!(true), json!("open"))
            );
            // upstream remote 로 맞는다(포크 r5 와 원본 r2 모두 등록 저장소)
            let review = by("review");
            assert_eq!((review["kind"].clone(), review["repoIds"].clone(), review["number"].clone()), (json!("pull"), json!(["r2", "r5"]), json!(2)));
            let issues = by("issues");
            assert_eq!((issues["kind"].clone(), issues["repoIds"].clone(), issues["number"].clone(), issues["draft"].clone()), (json!("issue"), json!(["r1", "r4"]), json!(3), json!(false)));
            assert!(!stringify(&res).contains("ghp_tok"));
        }

        #[tokio::test]
        async fn gh_launchpad_는_저장소가_아무리_많아도_검색_수는_그대로이고_repo_묶음이_없다() {
            let _g = Guard;
            let m = Mock::start(|_, r| if q_of(r).contains("repo:") { status(422, json!({"message": "Validation Failed"})) } else { ok(json!({"items": [lp_issue(1, "org/repo-with-a-long-name-7", json!({"pull_request": {}}))]})) });
            let e = lp_env(&m, (0..400).map(|i| (format!("x{i}"), Remotes::List(vec![remote("org", &format!("repo-with-a-long-name-{i}"), "origin")]))).collect(), true);
            let res = jv(&e.svc.launchpad(&json!("req1"), &Value::Null).await.unwrap());
            assert_eq!(m.requests().len(), 3);
            assert_eq!(res["errors"], json!([]));
            let items = res["items"].as_array().unwrap();
            assert_eq!(items.len(), 3);
            assert!(items.iter().all(|i| i["repoIds"] == json!(["x7"])));
        }

        #[tokio::test]
        async fn gh_launchpad_는_검색_한도에_걸린_섹션만_errors_로_남기고_나머지는_돌려준다() {
            let _g = Guard;
            let m = Mock::start(|_, r| match section_of(r) {
                "review" => status(403, json!({"message": "rate limit"})).header("x-ratelimit-remaining", "0").header("x-ratelimit-limit", "30").header("x-ratelimit-reset", "1900000000").header("x-ratelimit-resource", "search"),
                "authored" => ok(json!({"items": [lp_issue(1, "acme/app", json!({"pull_request": {}}))]})),
                _ => ok(json!({"items": []})),
            });
            let e = lp_env(&m, lp_remotes(), true);
            let res = jv(&e.svc.launchpad(&json!("req1"), &Value::Null).await.unwrap());
            assert_eq!(res["items"].as_array().unwrap().iter().map(|i| i["number"].clone()).collect::<Vec<_>>(), [json!(1)]);
            assert_eq!(res["errors"].as_array().unwrap().len(), 1);
            assert!(res["errors"][0].as_str().unwrap().contains("한도"), "{res}");
            assert_eq!(res["incomplete"], json!(true));
        }

        #[tokio::test]
        async fn gh_launchpad_는_쪽_상한에_닿으면_incomplete_로_알리고_같은_항목은_한_번만_담는다() {
            let _g = Guard;
            let m = Mock::start(|_, r| {
                if section_of(r) != "authored" {
                    return ok(json!({"items": []}));
                }
                ok(json!({"items": [lp_issue(1, "acme/app", json!({"pull_request": {}}))]})).header("link", "<https://api.github.com/search/issues?page=9>; rel=\"next\"")
            });
            let e = lp_env(&m, lp_remotes(), true);
            let res = jv(&e.svc.launchpad(&json!("req1"), &Value::Null).await.unwrap());
            assert_eq!(count(&m, |r| section_of(r) == "authored"), 5);
            assert_eq!(res["incomplete"], json!(true));
            assert_eq!(res["items"].as_array().unwrap().iter().filter(|i| i["section"] == json!("authored")).count(), 1);
        }

        #[tokio::test]
        async fn gh_launchpad_는_incomplete_results_와_1000건_초과를_incomplete_로_알린다() {
            let _g = Guard;
            let m = Mock::start(|_, _| ok(json!({"items": [], "incomplete_results": true})));
            let e = lp_env(&m, lp_remotes(), true);
            assert_eq!(jv(&e.svc.launchpad(&json!("req1"), &Value::Null).await.unwrap())["incomplete"], json!(true));
            let m2 = Mock::start(|_, _| ok(json!({"items": [], "total_count": 1500})));
            let e2 = lp_env(&m2, lp_remotes(), true);
            assert_eq!(jv(&e2.svc.launchpad(&json!("req1"), &Value::Null).await.unwrap())["incomplete"], json!(true));
        }

        #[tokio::test]
        async fn gh_launchpad_는_remote_를_읽지_못한_저장소는_건너뛰고_unresolved_로_알린다() {
            let _g = Guard;
            let m = Mock::start(|_, r| ok(json!({"items": if section_of(r) == "authored" { json!([lp_issue(1, "acme/app", json!({"pull_request": {}}))]) } else { json!([]) }})));
            let e = lp_env(&m, vec![("r1", Remotes::List(vec![remote("acme", "app", "origin")])), ("bad", Remotes::Fail), ("slow", Remotes::Hang)], true);
            let t0 = Instant::now();
            let res = jv(&e.svc.launchpad(&json!("req1"), &Value::Null).await.unwrap());
            // 응답 없는 저장소 하나가 제한 시간(시험에서는 300ms) 이상 붙잡지 않는다
            assert!(t0.elapsed() < WAIT_LIMIT, "{:?}", t0.elapsed());
            assert_eq!((res["unresolved"].clone(), res["skipped"].clone()), (json!(2), json!(0)));
            assert_eq!(res["items"].as_array().unwrap().iter().map(|i| i["number"].clone()).collect::<Vec<_>>(), [json!(1)]);
        }

        #[tokio::test]
        async fn gh_launchpad_는_저장소가_하나도_github_가_아니어도_검색은_한_번씩_하고_결과는_비어_있다() {
            let _g = Guard;
            let m = Mock::start(|_, _| ok(json!({"items": [lp_issue(1, "acme/app", json!({"pull_request": {}}))]})));
            let e = lp_env(&m, vec![("r3", Remotes::List(vec![]))], true);
            let res = jv(&e.svc.launchpad(&json!("req1"), &Value::Null).await.unwrap());
            assert_eq!(m.requests().len(), 3);
            assert_eq!((res["items"].clone(), res["skipped"].clone(), res["incomplete"].clone()), (json!([]), json!(1), json!(false)));
        }

        #[tokio::test]
        async fn gh_launchpad_는_401_이면_토큰을_다시_확인하고_오류를_돌려준다() {
            let _g = Guard;
            let m = Mock::start(|_, _| bad_credentials());
            let e = lp_env(&m, lp_remotes(), true);
            let err = failure(e.svc.launchpad(&json!("req1"), &Value::Null).await);
            assert!(err.message.contains("인증"), "{err}");
            assert!(!e.svc.auth().await.signed_in);
        }

        #[tokio::test]
        async fn gh_launchpad_는_fresh_면_remote_목록_캐시를_먼저_버린다() {
            let _g = Guard;
            let m = Mock::start(|_, _| ok(json!({"items": []})));
            let e = lp_env(&m, lp_remotes(), false);
            e.svc.launchpad(&json!("req1"), &json!(true)).await.unwrap();
            assert_eq!(e.repos.invalidated.load(Ordering::SeqCst), 1);
            e.svc.launchpad(&json!("req2"), &json!(false)).await.unwrap();
            assert_eq!(e.repos.invalidated.load(Ordering::SeqCst), 1);
        }
    }
}

// ── 토큰 보관(PR 4): 메모리 구현만으로 돈다(실제 자격 증명 관리자는 src/secret/keyring.rs 의 #[ignore] 수동 시험) ─────────────────

mod secret_store {
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    use antler_lib::jira::validate::{parse_connect_input, ConnectInput, SiteOptions};
    use antler_lib::jira::JiraKind;
    use antler_lib::net::redact::SecretString;
    use antler_lib::secret::memory::MemoryStore;
    use antler_lib::secret::{open_store, service_name, GithubMethod, SecretError, SecretStore, Vault, GITHUB_ACCOUNT, JIRA_ACCOUNT, MAX_BLOB};
    use serde_json::{json, Value};
    use zeroize::Zeroizing;

    /// 키체인처럼 남는 척하는 메모리 저장소와, 그 위의 보관소
    fn vault() -> (Arc<MemoryStore>, Vault) {
        let store = Arc::new(MemoryStore::standing_in_for_keychain());
        (store.clone(), Vault::new(store))
    }

    fn jira_input(token: &str) -> ConnectInput {
        parse_connect_input(&json!({"kind": "cloud", "siteUrl": "https://acme.atlassian.net", "email": "dev@acme.io", "token": token}), SiteOptions::default()).unwrap()
    }

    fn raw(store: &MemoryStore, account: &str) -> Option<String> {
        store.get(account).unwrap().map(|v| String::from_utf8(v.to_vec()).unwrap())
    }

    fn has(store: &MemoryStore, account: &str) -> bool {
        store.get(account).unwrap().is_some()
    }

    /// 쓰기·삭제가 항상 실패하는 저장소(읽기는 비어 있다)
    struct Failing;
    impl SecretStore for Failing {
        fn get(&self, _: &str) -> Result<Option<Zeroizing<Vec<u8>>>, SecretError> {
            Ok(None)
        }
        fn set(&self, _: &str, _: &[u8]) -> Result<(), SecretError> {
            Err(SecretError::Backend)
        }
        fn delete(&self, _: &str) -> Result<(), SecretError> {
            Err(SecretError::Backend)
        }
        fn persistent(&self) -> bool {
            true
        }
    }

    /// 읽기만 항상 실패하는 저장소(쓰기·삭제는 안쪽 저장소로 간다)
    struct ReadFails(Arc<MemoryStore>);
    impl SecretStore for ReadFails {
        fn get(&self, _: &str) -> Result<Option<Zeroizing<Vec<u8>>>, SecretError> {
            Err(SecretError::Backend)
        }
        fn set(&self, a: &str, v: &[u8]) -> Result<(), SecretError> {
            self.0.set(a, v)
        }
        fn delete(&self, a: &str) -> Result<(), SecretError> {
            self.0.delete(a)
        }
        fn persistent(&self) -> bool {
            true
        }
    }

    #[test]
    fn github_는_저장하고_다시_읽는다_메타_파일_없이_한_덩어리다() {
        let (store, v) = vault();
        assert!(v.load_github().is_none());
        assert_eq!(v.save_github(&SecretString::new("ghp_abc"), GithubMethod::Device), Ok(true));
        assert!(!v.github_storage_unavailable());
        assert_eq!(raw(&store, GITHUB_ACCOUNT).unwrap(), r#"{"method":"device","token":"ghp_abc"}"#);
        let got = v.load_github().unwrap();
        assert_eq!((got.token.expose(), got.method), ("ghp_abc", GithubMethod::Device));
        for m in [GithubMethod::Pat, GithubMethod::GhCli] {
            v.save_github(&SecretString::new("t"), m).unwrap();
            assert_eq!(v.load_github().unwrap().method, m);
        }
        v.clear_github().unwrap();
        assert!(v.load_github().is_none() && !has(&store, GITHUB_ACCOUNT));
    }

    #[test]
    fn jira_는_다섯_필드를_한_덩어리로_저장하고_다시_검증해_읽는다() {
        let (store, v) = vault();
        assert_eq!(v.save_jira(&jira_input("jtok"), "홍길동"), Ok(true));
        let stored: Value = serde_json::from_str(&raw(&store, JIRA_ACCOUNT).unwrap()).unwrap();
        assert_eq!(stored, json!({"kind": "cloud", "siteUrl": "https://acme.atlassian.net", "email": "dev@acme.io", "token": "jtok", "displayName": "홍길동"}));
        let got = v.load_jira().unwrap();
        assert_eq!(
            (got.input.kind, got.input.site_url.as_str(), got.input.email.as_str(), got.input.token.expose(), got.display_name.as_str()),
            (JiraKind::Cloud, "https://acme.atlassian.net", "dev@acme.io", "jtok", "홍길동")
        );
        // Data Center 는 이메일이 없다
        let dc = parse_connect_input(&json!({"kind": "server", "siteUrl": "https://jira.acme.io/jira/", "token": "pat"}), SiteOptions::default()).unwrap();
        v.save_jira(&dc, "").unwrap();
        let got = v.load_jira().unwrap();
        assert_eq!((got.input.kind, got.input.site_url.as_str(), got.input.email.as_str(), got.display_name.as_str()), (JiraKind::Server, "https://jira.acme.io/jira", "", ""));
        v.clear_jira().unwrap();
        assert!(v.load_jira().is_none() && !has(&store, JIRA_ACCOUNT));
    }

    #[test]
    fn 깨진_값은_지우고_none_을_돌려준다() {
        let (store, v) = vault();
        let bad_github = [
            "not json",
            "{}",
            r#"{"method":"pat"}"#,
            r#"{"method":"oauth","token":"t"}"#,
            r#"{"method":"pat","token":"a b"}"#,
            r#"{"method":"pat","token":""}"#,
            r#"{"method":"pat","token":5}"#,
            "[]",
            "null",
        ];
        for b in bad_github {
            store.set(GITHUB_ACCOUNT, b.as_bytes()).unwrap();
            assert!(v.load_github().is_none(), "{b}");
            assert!(!has(&store, GITHUB_ACCOUNT), "깨진 항목을 지웠어야 한다: {b}");
        }
        store.set(GITHUB_ACCOUNT, &[0xff, 0xfe, 0x00]).unwrap();
        assert!(v.load_github().is_none() && !has(&store, GITHUB_ACCOUNT));

        let bad_jira = [
            "not json",
            r#"{"kind":"cloud"}"#,
            r#"{"kind":"other","siteUrl":"https://a.atlassian.net","token":"t"}"#,
            // 검증 실패: http, 자격 증명이 든 주소, 이메일 없는 cloud, 공백이 든 토큰
            r#"{"kind":"server","siteUrl":"http://jira.acme.io","token":"t"}"#,
            r#"{"kind":"server","siteUrl":"https://u:p@jira.acme.io","token":"t"}"#,
            r#"{"kind":"cloud","siteUrl":"https://a.atlassian.net","token":"t"}"#,
            r#"{"kind":"server","siteUrl":"https://jira.acme.io","token":"a b"}"#,
        ];
        for b in bad_jira {
            store.set(JIRA_ACCOUNT, b.as_bytes()).unwrap();
            assert!(v.load_jira().is_none(), "{b}");
            assert!(!has(&store, JIRA_ACCOUNT), "깨진 항목을 지웠어야 한다: {b}");
        }
        // 한쪽이 깨져도 다른 쪽은 그대로다
        v.save_github(&SecretString::new("ghp"), GithubMethod::Pat).unwrap();
        store.set(JIRA_ACCOUNT, b"x").unwrap();
        assert!(v.load_jira().is_none() && v.load_github().is_some());
    }

    #[test]
    fn 남지_않는_저장소는_메모리에만_두고_storage_unavailable_이다() {
        let store = Arc::new(MemoryStore::new());
        let v = Vault::new(store.clone());
        assert!(v.github_storage_unavailable() && v.jira_storage_unavailable());
        assert_eq!(v.save_github(&SecretString::new("ghp_mem"), GithubMethod::Pat), Ok(false));
        assert_eq!(v.save_jira(&jira_input("j"), "n"), Ok(false));
        assert!(!has(&store, GITHUB_ACCOUNT) && !has(&store, JIRA_ACCOUNT), "저장소에는 쓰지 않는다");
        assert_eq!(v.load_github().unwrap().token.expose(), "ghp_mem");
        assert_eq!(v.load_jira().unwrap().display_name, "n");
        v.clear_github().unwrap();
        v.clear_jira().unwrap();
        assert!(v.load_github().is_none() && v.load_jira().is_none());
    }

    #[test]
    fn 쓰기에_실패하면_메모리에_두고_성공할_때까지_storage_unavailable_이다() {
        let v = Vault::new(Arc::new(Failing));
        assert!(!v.github_storage_unavailable() && !v.jira_storage_unavailable());
        assert_eq!(v.save_github(&SecretString::new("ghp_x"), GithubMethod::Device), Ok(false));
        // 실패는 그 계정만 표시한다
        assert!(v.github_storage_unavailable() && !v.jira_storage_unavailable());
        assert_eq!(v.load_github().unwrap().token.expose(), "ghp_x");
        assert_eq!(v.save_jira(&jira_input("jt"), "이름"), Ok(false));
        assert!(v.jira_storage_unavailable());
        assert_eq!(v.load_jira().unwrap().input.token.expose(), "jt");
        // 지우기가 저장소에서 실패해도 메모리 사본은 비운다
        assert_eq!(v.clear_github(), Err(SecretError::Backend));
        assert!(v.load_github().is_none());

        // 성공하는 저장소에서는 키체인으로 가고 메모리 사본은 없다
        let (store, v) = vault();
        v.save_github(&SecretString::new("a"), GithubMethod::Pat).unwrap();
        assert!(has(&store, GITHUB_ACCOUNT) && !v.github_storage_unavailable());
    }

    /// 한쪽 계정의 쓰기 실패가 다른 쪽의 상태를 바꾸지 않고, 그 계정이 다시 성공하면 풀린다
    #[test]
    fn 저장_실패_표시는_계정별이다() {
        struct FailOnly(Arc<MemoryStore>, &'static str);
        impl SecretStore for FailOnly {
            fn get(&self, a: &str) -> Result<Option<Zeroizing<Vec<u8>>>, SecretError> {
                self.0.get(a)
            }
            fn set(&self, a: &str, v: &[u8]) -> Result<(), SecretError> {
                if a == self.1 {
                    Err(SecretError::Backend)
                } else {
                    self.0.set(a, v)
                }
            }
            fn delete(&self, a: &str) -> Result<(), SecretError> {
                self.0.delete(a)
            }
            fn persistent(&self) -> bool {
                true
            }
        }
        let v = Vault::new(Arc::new(FailOnly(Arc::new(MemoryStore::standing_in_for_keychain()), JIRA_ACCOUNT)));
        assert_eq!(v.save_github(&SecretString::new("ghp_ok"), GithubMethod::Pat), Ok(true));
        assert_eq!(v.save_jira(&jira_input("jt"), "n"), Ok(false));
        assert!(v.jira_storage_unavailable() && !v.github_storage_unavailable());
        // GitHub 쓰기가 성공해도 Jira 표시는 풀리지 않는다
        assert_eq!(v.save_github(&SecretString::new("ghp_ok2"), GithubMethod::Pat), Ok(true));
        assert!(v.jira_storage_unavailable());
    }

    #[test]
    fn 쓸_수_없는_github_토큰은_저장하지_않고_오류다() {
        let (store, v) = vault();
        v.save_github(&SecretString::new("ghp_keep"), GithubMethod::Pat).unwrap();
        for bad in ["", "a b", "a\tb", " ", "a\0b", &"a".repeat(4097)] {
            assert_eq!(v.save_github(&SecretString::new(bad), GithubMethod::Pat), Err(SecretError::InvalidToken), "{bad:?}");
        }
        // 기존 값은 그대로다
        assert_eq!(v.load_github().unwrap().token.expose(), "ghp_keep");
        assert!(raw(&store, GITHUB_ACCOUNT).unwrap().contains("ghp_keep"));
        assert!(!SecretError::InvalidToken.to_string().contains("ghp"));
    }

    #[test]
    fn 읽기_실패는_없음으로_보고_항목을_지우지_않는다() {
        let inner = Arc::new(MemoryStore::standing_in_for_keychain());
        inner.set(GITHUB_ACCOUNT, br#"{"method":"pat","token":"t"}"#).unwrap();
        let v = Vault::new(Arc::new(ReadFails(inner.clone())));
        assert!(v.load_github().is_none());
        assert!(has(&inner, GITHUB_ACCOUNT), "접근 실패는 깨진 값이 아니다");
    }

    #[test]
    fn 한도를_넘는_덩어리는_저장하지_않는다_표시_이름은_200자로_자른다() {
        let (store, v) = vault();
        // 표시 이름: 200자(한글 3바이트 → 600바이트)로 잘리고 저장된다
        let long: String = "가".repeat(500);
        assert_eq!(v.save_jira(&jira_input("jt"), &long), Ok(true));
        assert_eq!(v.load_jira().unwrap().display_name.chars().count(), 200);
        let bytes = raw(&store, JIRA_ACCOUNT).unwrap().len();
        assert!(bytes <= MAX_BLOB, "{bytes}");

        // 그래도 넘으면(사이트 주소가 길다) 명확한 오류이고 기존 값은 건드리지 않는다
        let big = ConnectInput { site_url: format!("https://acme.atlassian.net/{}", "a".repeat(1900)), ..jira_input(&"t".repeat(1000)) };
        let before = raw(&store, JIRA_ACCOUNT);
        assert_eq!(v.save_jira(&big, &long), Err(SecretError::TooLarge));
        assert_eq!(raw(&store, JIRA_ACCOUNT), before);
        assert!(SecretError::TooLarge.to_string().contains("너무"));
        // 경계: 정확히 MAX_BLOB 바이트는 저장되고 1바이트 넘으면 거부한다
        let overhead = r#"{"method":"pat","token":""}"#.len();
        assert_eq!(v.save_github(&SecretString::new("a".repeat(MAX_BLOB - overhead)), GithubMethod::Pat), Ok(true));
        assert_eq!(raw(&store, GITHUB_ACCOUNT).unwrap().len(), MAX_BLOB);
        assert_eq!(v.save_github(&SecretString::new("a".repeat(MAX_BLOB - overhead + 1)), GithubMethod::Pat), Err(SecretError::TooLarge));
        // 메모리 저장소에서도 같은 한도다(플랫폼에 따라 동작이 갈리지 않는다)
        assert_eq!(Vault::new(Arc::new(MemoryStore::new())).save_github(&SecretString::new("a".repeat(MAX_BLOB)), GithubMethod::Pat), Err(SecretError::TooLarge));
    }

    #[test]
    fn 바이트와_secret_string_을_다루고_비밀은_포맷에_나오지_않는다() {
        let s = MemoryStore::standing_in_for_keychain();
        s.set_secret_string("k", &SecretString::new("ghp_secret")).unwrap();
        assert_eq!(s.get("k").unwrap().unwrap().as_slice(), b"ghp_secret");
        assert_eq!(s.get_secret_string("k").unwrap().unwrap().expose(), "ghp_secret");
        assert!(s.get_secret_string("none").unwrap().is_none());
        s.set("bin", &[0xff, 0xfe]).unwrap();
        assert!(s.get_secret_string("bin").unwrap().is_none(), "UTF-8 이 아니면 깨진 값");
        s.delete("k").unwrap();
        s.delete("k").unwrap();
        assert!(s.get("k").unwrap().is_none());

        let (_, v) = vault();
        v.save_github(&SecretString::new("ghp_secret"), GithubMethod::Pat).unwrap();
        v.save_jira(&jira_input("jira_secret"), "n").unwrap();
        let (g, j) = (v.load_github().unwrap(), v.load_jira().unwrap());
        for text in [format!("{g:?}"), format!("{j:?}"), format!("{:?}", g.token), SecretError::Backend.to_string(), format!("{:?}", SecretError::TooLarge)] {
            assert!(!text.contains("ghp_secret") && !text.contains("jira_secret"), "{text}");
        }
        assert_eq!(format!("{:?}", g.token), "***");
    }

    #[test]
    fn 저장소_선택과_서비스_이름() {
        // ANTLER_USER_DATA 가 있으면(비어 있어도 — lib.rs 와 같다) 메모리다. 키체인을 여는 경로(None)는 여기서 부르지 않는다 — 실제 항목을 건드릴 수 있다
        assert!(!open_store(Some(std::ffi::OsStr::new("C:/tmp/ud"))).persistent());
        assert!(!open_store(Some(std::ffi::OsStr::new(""))).persistent());
        // 이 시험 빌드는 디버그라 개발용 이름이다. 운영 이름(com.antler.app)과 겹치지 않는다
        assert_eq!(service_name(), if cfg!(debug_assertions) { "com.antler.app.dev" } else { "com.antler.app" });
        #[cfg(not(windows))]
        assert!(!open_store(None).persistent());
    }

    // ── 소스 검사 ────────────────────────────────────────────────────────

    fn rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
        for e in std::fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                rs_files(&p, out);
            } else if p.extension().is_some_and(|x| x == "rs") {
                out.push(p);
            }
        }
    }

    /// 주석 줄과 파일 끝의 시험 모듈(`#[cfg(test)]` + `mod `)을 뺀 코드 줄
    fn code_lines(text: &str) -> Vec<(usize, &str)> {
        let mut out = Vec::new();
        for (i, line) in text[..super::prod_end(text)].lines().enumerate() {
            let t = line.trim_start();
            if !t.starts_with("//") {
                out.push((i + 1, t));
            }
        }
        out
    }

    fn words(t: &str) -> Vec<&str> {
        t.split(|c: char| !(c.is_alphanumeric() || c == '_')).filter(|w| !w.is_empty()).collect()
    }

    /// `rel`(src/ 기준) 파일의 위반: secret 밖에서 키링 크레이트를 쓰거나, secret 안에서 출력·로그를 쓴다
    fn violations(rel: &str, text: &str) -> Vec<String> {
        let in_secret = rel.starts_with("secret/");
        let mut out = Vec::new();
        for (n, t) in code_lines(text) {
            let w = words(t);
            let uses_keyring = w.iter().any(|x| *x == "keyring_core" || *x == "windows_native_keyring_store") || t.contains("keyring-core") || t.contains("windows-native-keyring-store");
            let prints = in_secret
                && (["println", "eprintln", "print", "eprint", "dbg"].iter().any(|m| w.contains(m) && t.contains(&format!("{m}!")))
                    || w.iter().any(|x| matches!(*x, "log" | "tracing" | "slog" | "env_logger")));
            if (uses_keyring && !in_secret) || prints {
                out.push(format!("{rel}:{n}: {t}"));
            }
        }
        out
    }

    #[test]
    fn 소스_검사기가_위반을_알아본다() {
        assert_eq!(violations("net/x.rs", "use keyring_core::Entry;").len(), 1);
        assert_eq!(violations("net/x.rs", "let s = windows_native_keyring_store::Store::new();").len(), 1);
        assert!(violations("secret/keyring.rs", "use keyring_core::Entry;").is_empty());
        assert!(violations("net/x.rs", "// keyring_core 는 secret 에서만").is_empty());
        assert_eq!(violations("secret/mod.rs", "println!(\"{}\", x);").len(), 1);
        assert_eq!(violations("secret/mod.rs", "    eprintln!(\"x\");").len(), 1);
        assert_eq!(violations("secret/mod.rs", "dbg!(x);").len(), 1);
        assert_eq!(violations("secret/mod.rs", "log::info!(\"x\");").len(), 1);
        assert_eq!(violations("secret/mod.rs", "use tracing::debug;").len(), 1);
        assert!(violations("net/x.rs", "println!(\"ok\");").is_empty());
        assert!(violations("secret/mod.rs", "let login = 1; let blog = 2;").is_empty());
        // 파일 끝의 시험 모듈은 보지 않는다
        assert!(violations("secret/keyring.rs", "fn a() {}\n#[cfg(test)]\nmod t { fn b() { eprintln!(\"x\"); } }").is_empty());
        // 메서드 앞의 `#[cfg(test)]`(시험 전용 항목) 뒤의 운영 코드는 본다
        assert_eq!(violations("secret/mod.rs", "impl A {\n    #[cfg(test)]\n    fn c(&self) {}\n    fn p(&self) { eprintln!(\"x\"); }\n}\n").len(), 1);
    }

    #[test]
    fn 키링은_secret_모듈에서만_쓰고_secret_모듈에는_출력과_로그가_없다() {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        rs_files(&src, &mut files);
        let (mut secret_files, mut bad) = (0, Vec::new());
        for f in &files {
            let rel = f.strip_prefix(&src).unwrap().to_string_lossy().replace('\\', "/");
            secret_files += usize::from(rel.starts_with("secret/"));
            bad.extend(violations(&rel, &std::fs::read_to_string(f).unwrap()));
        }
        assert!(secret_files >= 3, "secret 모듈 파일을 {secret_files}개만 찾았다");
        assert!(bad.is_empty(), "위반:\n{}", bad.join("\n"));
        // Cargo.toml: 두 크레이트는 windows 대상 의존성으로만 있다
        let toml = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml")).unwrap();
        let (before, after) = toml.split_once("[target.'cfg(windows)'.dependencies]").expect("windows 의존성 절");
        let windows_part = after.split("\n[").next().unwrap();
        assert!(windows_part.contains("keyring-core") && windows_part.contains("windows-native-keyring-store"));
        assert!(!before.contains("keyring-core") && !before.contains("windows-native-keyring-store"));
    }

    #[test]
    fn secret_string_은_display_와_serialize_가_없고_버릴_때_덮는다() {
        let text = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/net/redact.rs")).unwrap();
        let code: String = code_lines(&text).iter().map(|(_, t)| *t).collect::<Vec<_>>().join("\n");
        assert!(!code.contains("Display for SecretString") && !code.contains("Serialize for SecretString") && !code.contains("derive(Serialize"));
        assert!(code.contains("impl Drop for SecretString"));
    }
}
