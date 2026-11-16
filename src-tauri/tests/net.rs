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
            let client = rel != "net/http.rs" && (names_reqwest_client || t.contains("danger_accept") || t.contains("reqwest::get"));
            if test_hook || loopback || client {
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
}
