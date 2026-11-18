//! 업데이트 판정 로직(`src/update/logic.rs`, Phase 6c PR 1)의 시험: 골든 패리티(`tests/golden/update/*.json` — TS 가 기준: `UPDATE_GOLDEN=1 pnpm exec vitest run tests/goldenUpdate.test.ts`),
//! TS 에 짝이 없는 매니페스트 검사의 단언, 소스 검사. **네트워크·파일·프로세스를 쓰지 않는다** — 릴리스 노트는 가짜 getter 로 돈다.
//! 안전 방향 확인: URL 판정 격자에서 "Rust 허용 ⇒ TS 허용"을 따로 센다. TS 테스트 → Rust 테스트 대응표는 `tests/golden/update/test-map.json`(vitest 가 누락을 강제한다).

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use antler_lib::update::logic::{
    self, check_manifest, compare_semver, fetch_release_notes, is_allowed_download_url, is_offerable, is_release_asset_url, is_safe_file_name, parse_dismiss, parse_release_response, parse_semver,
    prerelease_manifest_url, release_notes_url, sanitize_error, Dismiss, DismissAction, UpdatePolicy, HAND_WRITTEN, MAX_MANIFEST_BYTES, MAX_NOTES_CHARS, MAX_SIGNATURE_LEN,
};
use serde_json::{json, Value};

const BASE: &str = "https://github.com/Kang-Jacob-GitLB/antler/releases/download";
const SIG: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IHNpZ25hdHVyZQpSVVE9PQ==";

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().expect("src-tauri 의 상위").to_path_buf()
}

fn golden(name: &str) -> Value {
    let path = root().join("tests").join("golden").join("update").join(name);
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

/// 숫자를 전부 f64 로 맞춘다(`5` 와 `5.0` 은 같은 값이다)
fn norm(v: &Value) -> Value {
    match v {
        Value::Number(n) => json!(n.as_f64().expect("f64")),
        Value::Array(a) => Value::Array(a.iter().map(norm).collect()),
        Value::Object(o) => Value::Object(o.iter().map(|(k, x)| (k.clone(), norm(x))).collect()),
        other => other.clone(),
    }
}

/// 어긋난 사례를 모아 한 번에 보고한다
#[derive(Default)]
struct Diffs {
    checked: usize,
    bad: Vec<String>,
}

impl Diffs {
    fn eq(&mut self, what: impl FnOnce() -> String, got: &Value, expected: &Value) {
        self.checked += 1;
        if norm(got) != norm(expected) {
            self.bad.push(format!("{} → Rust {got}, TS {expected}", what()));
        }
    }

    fn finish(self, what: &str) {
        assert!(self.checked > 0, "{what}: 검사한 사례가 없다");
        assert!(self.bad.is_empty(), "{what}: 골든 불일치 {}건 / {}건\n  {}", self.bad.len(), self.checked, self.bad.join("\n  "));
    }
}

const HANDLED: [&str; 7] = ["dismiss.json", "manifest.json", "notes.json", "policy.json", "sanitize.json", "semver.json", "url.json"];

#[test]
fn 골든_파일마다_처리기가_있다() {
    let mut found: Vec<String> = fs::read_dir(root().join("tests/golden/update"))
        .expect("tests/golden/update")
        .map(|e| e.expect("항목").file_name().to_string_lossy().into_owned())
        // test-map.json 은 TS 테스트 → Rust 테스트 이식 대응표(손글씨)라 골든이 아니다
        .filter(|n| n.ends_with(".json") && n != "test-map.json")
        .collect();
    found.sort();
    assert_eq!(found, HANDLED, "처리기가 없는 골든이 있거나 골든이 없다");
}

#[test]
fn 손으로_옮긴_정규식_소스가_골든과_같다() {
    let g = golden("semver.json");
    let list = arr(&g["handWritten"], "handWritten");
    assert_eq!(list.len(), HAND_WRITTEN.len(), "손으로 옮긴 정규식 개수");
    for (e, (name, snippet)) in list.iter().zip(HAND_WRITTEN) {
        assert_eq!(s(&e["name"]), *name);
        assert_eq!(s(&e["snippet"]), *snippet, "{name}: TS 정규식이 바뀌었다 — 구현(손 판정)을 다시 본다");
    }
}

// ── semver ────────────────────────────────────────────────────────────────────

#[test]
fn semver_파싱_골든이_ts_와_같다() {
    let g = golden("semver.json");
    let mut d = Diffs::default();
    for c in arr(&g["parse"], "parse") {
        let got = match parse_semver(s(&c["input"])) {
            Some(v) => json!({ "major": v.major, "minor": v.minor, "patch": v.patch, "pre": v.pre }),
            None => Value::Null,
        };
        d.eq(|| format!("parse_semver({:?})", c["input"]), &got, &c["out"]);
    }
    d.finish("semver.parse");
}

fn versions(g: &Value) -> Vec<String> {
    arr(&g["versions"], "versions").iter().map(|v| s(v).to_string()).collect()
}

#[test]
fn semver_비교_골든이_ts_와_같다() {
    let g = golden("semver.json");
    let vs = versions(&g);
    let rows = arr(&g["compare"], "compare");
    assert_eq!(rows.len(), vs.len());
    let mut d = Diffs::default();
    for (a, row) in vs.iter().zip(rows) {
        let cells: Vec<&str> = s(row).split(' ').collect();
        assert_eq!(cells.len(), vs.len());
        for (b, want) in vs.iter().zip(cells) {
            let got = match compare_semver(a, b) {
                Some(o) => (o as i8).to_string(),
                None => "n".to_string(),
            };
            d.eq(|| format!("compare_semver({a:?}, {b:?})"), &json!(got), &json!(want));
        }
    }
    d.finish("semver.compare");
}

#[test]
fn 알림_대상_골든이_ts_와_같다() {
    let g = golden("semver.json");
    let vs = versions(&g);
    let rows = arr(&g["offerable"], "offerable");
    let mut d = Diffs::default();
    for (current, row) in vs.iter().zip(rows) {
        let cells: Vec<char> = s(row).chars().collect();
        assert_eq!(cells.len(), vs.len());
        for (candidate, want) in vs.iter().zip(cells) {
            d.eq(|| format!("is_offerable({current:?}, {candidate:?})"), &json!(is_offerable(current, candidate)), &json!(want == '1'));
        }
    }
    d.finish("semver.offerable");
}

#[test]
fn 알림_대상은_다운그레이드와_정식_사용자의_프리릴리스를_거부한다() {
    assert!(is_offerable("0.1.0", "0.2.0"));
    assert!(!is_offerable("0.1.0", "0.1.0"));
    assert!(!is_offerable("0.2.0", "0.1.0"), "다운그레이드");
    assert!(!is_offerable("0.1.0", "0.2.0-rc.1"));
    assert!(is_offerable("0.2.0-rc.1", "0.2.0-rc.2"));
    assert!(is_offerable("0.2.0-rc.1", "0.2.0"));
    assert!(!is_offerable("0.2.0-rc.2", "0.2.0-rc.1"), "프리릴리스끼리의 다운그레이드");
    assert!(!is_offerable("0.2.0", "0.2.0-rc.9"), "정식에서 같은 번호의 프리릴리스로");
    assert!(!is_offerable("dev", "1.0.0"), "현재 버전을 읽을 수 없으면 제안하지 않는다");
    assert!(!is_offerable("1.0.0", "garbage"));
}

// ── URL ───────────────────────────────────────────────────────────────────────

/// 격자 한 번의 결과: 같은지 + "Rust 허용 ⇒ TS 허용" 위반(unsound)
fn url_grid(g: &Value, key: &str, f: fn(&str) -> bool) -> (Diffs, Vec<String>, BTreeSet<String>) {
    let mut d = Diffs::default();
    let mut unsound = Vec::new();
    let mut stricter = BTreeSet::new();
    for c in arr(&g[key], key) {
        let got = f(s(&c["input"]));
        let want = c["out"].as_bool().expect("bool");
        if got && !want {
            unsound.push(format!("{key}({:?})", c["input"]));
        }
        if !got && want {
            stricter.insert(s(&c["input"]).to_string());
        }
        d.eq(|| format!("{key}({:?})", c["input"]), &json!(got), &json!(want));
    }
    (d, unsound, stricter)
}

#[test]
fn url_격자_판정이_ts_와_같다_허용은_ts_안이다() {
    let g = golden("url.json");
    for (key, f) in [("allowed", is_allowed_download_url as fn(&str) -> bool), ("asset", is_release_asset_url as fn(&str) -> bool)] {
        let (d, unsound, stricter) = url_grid(&g, key, f);
        assert!(unsound.is_empty(), "Rust 만 허용(보안 방향의 어긋남): {unsound:?}");
        assert!(stricter.is_empty(), "{key}: TS 는 받지만 Rust 가 거부 — 알려진 차이로 기록하려면 목록을 만든다: {stricter:?}");
        d.finish(key);
    }
}

#[test]
fn 파일_이름_골든이_ts_와_같다() {
    let g = golden("url.json");
    let mut d = Diffs::default();
    for c in arr(&g["safeName"], "safeName") {
        d.eq(|| format!("is_safe_file_name({:?})", c["input"]), &json!(is_safe_file_name(s(&c["input"]))), &c["out"]);
    }
    d.finish("safeName");
}

#[test]
fn 릴리스_노트_주소_골든이_ts_와_같다() {
    let g = golden("url.json");
    let mut d = Diffs::default();
    for c in arr(&g["notesUrl"], "notesUrl") {
        d.eq(|| format!("release_notes_url({:?})", c["input"]), &json!(release_notes_url(s(&c["input"]))), &c["out"]);
    }
    d.finish("notesUrl");
}

#[test]
fn 허용_목록은_원본_단언대로_판정한다() {
    assert!(is_allowed_download_url("https://github.com/a/b"));
    assert!(is_allowed_download_url("https://objects.githubusercontent.com/x?y=1"));
    assert!(is_allowed_download_url("https://release-assets.githubusercontent.com/x"));
    assert!(!is_allowed_download_url("http://github.com/a"));
    assert!(!is_allowed_download_url("https://evil.com/a"));
    assert!(!is_allowed_download_url("https://github.com.evil.com/a"));
    assert!(!is_allowed_download_url("https://user:pw@github.com/a"));
    assert!(!is_allowed_download_url("https://github.com:8443/a"));
    assert!(!is_allowed_download_url("not a url"));
}

#[test]
fn 릴리스_자산은_이_저장소만_받는다() {
    assert!(is_release_asset_url(&format!("{BASE}/v0.2.0/a.dmg")));
    assert!(!is_release_asset_url("https://github.com/other/repo/releases/download/v1/a.dmg"));
}

#[test]
fn 경로_이탈을_거부한다() {
    for tail in ["/../../../../evil/repo/releases/download/v9.9.9/x.dmg", "/v1/%2e%2e/%2e%2e/x.dmg", "/v1/..%2f..%2fx.dmg", "/v1/a%5cb.dmg", "/v1/a\\b.dmg", "/v1/./a.dmg", "/v1/a.dmg?x=1"] {
        assert!(!is_release_asset_url(&format!("{BASE}{tail}")), "{tail}");
    }
}

// ── 정책·입력·오류 문구 ───────────────────────────────────────────────────────

#[test]
fn 정책_골든이_ts_와_같다() {
    let g = golden("policy.json");
    let mut d = Diffs::default();
    for sc in arr(&g["scenarios"], "scenarios") {
        let mut skipped: Option<String> = None;
        let mut p = UpdatePolicy::new();
        for st in arr(&sc["steps"], "steps") {
            match s(&st["op"]) {
                "snooze" => p.snooze(s(&st["version"])),
                "skip" => skipped = st["value"].as_str().map(str::to_string),
                "restart" => p = UpdatePolicy::new(),
                "notify" => {
                    let got = p.should_notify(s(&st["version"]), st["manual"].as_bool().expect("manual"), skipped.as_deref());
                    d.eq(|| format!("{} / {st}", sc["name"]), &json!(got), &st["out"]);
                }
                other => panic!("알 수 없는 단계: {other}"),
            }
        }
    }
    d.finish("policy");
}

#[test]
fn dismiss_골든이_ts_와_같다() {
    let g = golden("dismiss.json");
    let mut d = Diffs::default();
    for c in arr(&g["cases"], "cases") {
        let got = match parse_dismiss(&c["action"], &c["version"]) {
            Ok(Dismiss { action, version }) => json!({ "ok": { "action": if action == DismissAction::Skip { "skip" } else { "later" }, "version": version } }),
            Err(e) => json!({ "err": e }),
        };
        d.eq(|| format!("parse_dismiss({})", c["name"]), &got, &c["out"]);
    }
    d.finish("dismiss");
}

#[test]
fn 오류_문구_골든이_ts_와_같다() {
    let g = golden("sanitize.json");
    let mut d = Diffs::default();
    for c in arr(&g["cases"], "cases") {
        d.eq(|| format!("sanitize_error({:?})", c["input"]), &json!(sanitize_error(s(&c["input"]))), &c["out"]);
    }
    d.finish("sanitize");
}

#[test]
fn 오류_문구는_비밀과_경로를_가리고_길이를_제한한다() {
    let out = sanitize_error("fail https://objects.githubusercontent.com/x?token=SECRET ghp_abcdefghijklmnopqrstuvwxyz0123 C:\\Users\\me\\a.dmg");
    assert!(!out.contains("SECRET") && !out.contains("ghp_") && !out.contains("Users"), "{out}");
    assert!(out.encode_utf16().count() <= 201);
    assert_eq!(sanitize_error(&"a".repeat(300)).encode_utf16().count(), 201);
}

#[test]
fn 차이_200자_경계가_서로게이트_쌍을_가르면_반쪽은_u_fffd_다() {
    // JS 는 짝 없는 서로게이트를 남긴다(골든에는 담지 못한다 — JSON 파서가 거부한다). 쌍이 온전히 들어가면 그대로다
    assert_eq!(sanitize_error(&format!("{}😀", "a".repeat(199))), format!("{}\u{fffd}…", "a".repeat(199)));
    assert_eq!(sanitize_error(&format!("{}😀", "a".repeat(198))), format!("{}😀", "a".repeat(198)));
}

// ── 매니페스트 ────────────────────────────────────────────────────────────────

fn manifest(version: &str, platforms: &Value) -> String {
    json!({ "version": version, "notes": "hi", "pub_date": "2026-01-01T00:00:00Z", "platforms": platforms }).to_string()
}

fn nsis(version: &str) -> Value {
    json!({ "windows-x86_64-nsis": { "url": format!("{BASE}/v{version}/Antler-Setup-{version}-x64.exe"), "signature": SIG } })
}

#[test]
fn 매니페스트_골든이_기대와_ts_부품에_맞다() {
    let g = golden("manifest.json");
    let mut offered = 0;
    for c in arr(&g["cases"], "cases") {
        let name = s(&c["name"]);
        let r = check_manifest(s(&c["text"]), s(&c["current"]));
        let got = match &r {
            Ok(Some(_)) => "offer",
            Ok(None) => "none",
            Err(_) => "err",
        };
        assert_eq!(got, s(&c["expect"]), "{name}: {r:?}");
        if let Ok(Some(o)) = &r {
            offered += 1;
            // TS 의 부품 판정이 모두 허용해야 한다(Rust 가 더 느슨하면 안 된다)
            let parts = &c["parts"];
            assert!(parts.is_object(), "{name}: TS 부품이 없다");
            assert_eq!(parts["offerable"], json!(true), "{name}: TS isOfferable 이 거부한다");
            assert_eq!(parts["urlOk"], json!(true), "{name}: TS isReleaseAssetUrl 이 거부한다");
            assert!(is_release_asset_url(&o.url) && o.url.starts_with(&format!("{BASE}/v{}/", o.version)), "{name}");
        }
    }
    assert!(offered > 5, "허용 사례가 너무 적다");
}

#[test]
fn 매니페스트에서_새_버전의_주소와_서명을_찾는다() {
    let o = check_manifest(&manifest("v1.1.0", &nsis("1.1.0")), "1.0.0").expect("검사").expect("새 버전");
    assert_eq!(o.version, "1.1.0", "앞의 v 는 뗀다");
    assert_eq!(o.url, format!("{BASE}/v1.1.0/Antler-Setup-1.1.0-x64.exe"));
    assert_eq!(o.signature, SIG);
    assert_eq!(o.platform, "windows-x86_64-nsis");
    let generic = json!({ "windows-x86_64": nsis("1.1.0")["windows-x86_64-nsis"].clone() });
    assert_eq!(check_manifest(&manifest("1.1.0", &generic), "1.0.0").expect("검사").expect("새 버전").platform, "windows-x86_64");
}

#[test]
fn 같거나_낮은_버전은_제안하지_않는다() {
    assert_eq!(check_manifest(&manifest("1.1.0", &nsis("1.1.0")), "1.1.0"), Ok(None));
    assert_eq!(check_manifest(&manifest("1.1.0", &nsis("1.1.0")), "2.0.0"), Ok(None), "다운그레이드");
    // 제안하지 않는 버전은 플랫폼 항목을 보지 않는다 — 깨진 항목이 있어도 오류가 아니다
    assert_eq!(check_manifest(&manifest("1.1.0", &json!({})), "2.0.0"), Ok(None));
}

#[test]
fn 정식_사용자에게_프리릴리스는_숨긴다() {
    assert_eq!(check_manifest(&manifest("1.1.0-rc.1", &nsis("1.1.0-rc.1")), "1.0.0"), Ok(None));
    assert!(check_manifest(&manifest("1.1.0-rc.2", &nsis("1.1.0-rc.2")), "1.1.0-rc.1").expect("검사").is_some());
    assert_eq!(prerelease_manifest_url(&json!([{ "tag_name": "v1.1.0-rc.1", "prerelease": true }]).to_string(), "1.0.0"), Ok(None));
}

#[test]
fn 프리릴리스_사용자는_릴리스_목록에서_매니페스트를_찾는다() {
    let list = json!([
        { "tag_name": "v1.2.0-rc.3", "draft": true },
        { "tag_name": "v1.2.0-rc.2", "prerelease": true },
        { "tag_name": "v1.1.0" }
    ])
    .to_string();
    let want = Some(format!("{BASE}/v1.2.0-rc.2/latest.json"));
    assert_eq!(prerelease_manifest_url(&list, "1.2.0-rc.1"), Ok(want.clone()), "첫 비초안 릴리스의 태그");
    assert!(is_release_asset_url(want.as_deref().expect("url")), "매니페스트 주소도 릴리스 자산 형식이다");
    assert_eq!(prerelease_manifest_url("[]", "1.2.0-rc.1"), Ok(None));
    assert_eq!(prerelease_manifest_url(&json!([{ "draft": true, "tag_name": "v9.0.0" }]).to_string(), "1.2.0-rc.1"), Ok(None), "초안만 있다");
    for tag in [json!("1.2.0"), json!("vlatest"), json!(" v1.2.0"), json!("v1.2.0\n"), json!(7), Value::Null] {
        assert_eq!(prerelease_manifest_url(&json!([{ "tag_name": tag }]).to_string(), "1.2.0-rc.1"), Ok(None), "{tag}");
    }
    assert!(prerelease_manifest_url("{}", "1.2.0-rc.1").is_err(), "배열이 아니다");
    assert!(prerelease_manifest_url("nope", "1.2.0-rc.1").is_err());
}

#[test]
fn 다른_저장소_url_은_거부한다() {
    let other = json!({ "windows-x86_64-nsis": { "url": "https://github.com/evil/repo/releases/download/v1.1.0/a.exe", "signature": SIG } });
    assert!(check_manifest(&manifest("1.1.0", &other), "1.0.0").is_err());
    // 이 저장소여도 다른 릴리스의 설치본(태그 결속)·하위 경로는 거부한다
    for tail in ["/v1.0.9/a.exe", "/v9.9.9/a.exe", "/1.1.0/a.exe", "/v1.1.0/sub/a.exe", "/v1.1.0/", "/v1.1.0"] {
        let p = json!({ "windows-x86_64-nsis": { "url": format!("{BASE}{tail}"), "signature": SIG } });
        assert!(check_manifest(&manifest("1.1.0", &p), "1.0.0").is_err(), "{tail}");
    }
}

#[test]
fn 매니페스트_크기와_서명_길이의_경계() {
    let pad = json!({ "version": "1.1.0", "pad": "x".repeat(MAX_MANIFEST_BYTES) }).to_string();
    assert!(pad.len() > MAX_MANIFEST_BYTES);
    assert_eq!(check_manifest(&pad, "1.0.0"), Err("응답이 너무 큽니다".to_string()));
    let sig_entry = |n: usize| json!({ "windows-x86_64-nsis": { "url": nsis("1.1.0")["windows-x86_64-nsis"]["url"].clone(), "signature": "A".repeat(n) } });
    assert!(check_manifest(&manifest("1.1.0", &sig_entry(MAX_SIGNATURE_LEN)), "1.0.0").expect("검사").is_some());
    assert!(check_manifest(&manifest("1.1.0", &sig_entry(MAX_SIGNATURE_LEN + 1)), "1.0.0").is_err());
}

#[test]
fn 매니페스트_오류_문구에는_입력이_들어가지_않는다() {
    let evil = json!({ "windows-x86_64-nsis": { "url": "https://evil.example/ghp_abcdefghijklmnopqrstuvwxyz0123?token=SECRET", "signature": SIG } });
    let e = check_manifest(&manifest("1.1.0", &evil), "1.0.0").expect_err("거부");
    assert!(!e.contains("evil") && !e.contains("SECRET") && !e.contains("ghp_"), "{e}");
}

// ── 릴리스 노트 ───────────────────────────────────────────────────────────────

fn brief(v: &str) -> String {
    let n = v.encode_utf16().count();
    if n > 200 {
        format!("len:{n}:{}", antler_lib::git::fetch::slice_utf16_lossy(v, 20))
    } else {
        v.to_string()
    }
}

async fn notes_with(current: &str, routes: &Value) -> (Vec<String>, Result<logic::ReleaseNotes, String>) {
    let requests = std::cell::RefCell::new(Vec::<String>::new());
    let result = fetch_release_notes(current, |path| {
        requests.borrow_mut().push(path.clone());
        let r = &routes[&path];
        let out = if r.is_null() {
            Ok(None)
        } else {
            let status = r["status"].as_u64().unwrap_or(200) as u16;
            let body = match &r["body"] {
                Value::Null => String::new(),
                Value::String(t) => t.clone(),
                other => other.to_string(),
            };
            parse_release_response(status, &body)
        };
        async move { out }
    })
    .await;
    (requests.into_inner(), result)
}

#[tokio::test]
async fn 노트_골든이_ts_와_같다() {
    let g = golden("notes.json");
    let mut d = Diffs::default();
    for c in arr(&g["cases"], "cases") {
        let (requests, result) = notes_with(s(&c["current"]), &c["routes"]).await;
        let got = match result {
            Ok(n) => {
                let mut v = serde_json::to_value(&n).expect("직렬화");
                v["body"] = json!(brief(&n.body));
                if let Some(t) = &n.title {
                    v["title"] = json!(brief(t));
                }
                json!({ "ok": v })
            }
            Err(e) => json!({ "err": e }),
        };
        d.eq(|| format!("{} / 요청", c["name"]), &json!(requests), &c["requests"]);
        d.eq(|| format!("{}", c["name"]), &got, &c["out"]);
    }
    d.finish("notes");
}

#[tokio::test]
async fn 큰_노트_본문은_100000자에서_자른다() {
    let tag = json!({ "/releases/tags/v1.2.3": { "body": { "tag_name": "v1.2.3", "body": format!("{}y", "x".repeat(MAX_NOTES_CHARS)) } } });
    let n = notes_with("1.2.3", &tag).await.1.expect("노트");
    assert!(n.exact && n.truncated == Some(true));
    assert_eq!(n.body.len(), MAX_NOTES_CHARS);
    let exact = json!({ "/releases/tags/v1.2.3": { "body": { "tag_name": "v1.2.3", "body": "x".repeat(MAX_NOTES_CHARS) } } });
    let n = notes_with("1.2.3", &exact).await.1.expect("노트");
    assert_eq!((n.truncated, n.body.len()), (None, MAX_NOTES_CHARS));
    // 서로게이트 쌍(😀 = 2 단위)이 경계에서 갈리면 반쪽은 U+FFFD 다
    let emoji = json!({ "/releases/tags/v1.2.3": { "body": { "tag_name": "v1.2.3", "body": format!("{}😀", "x".repeat(MAX_NOTES_CHARS - 1)) } } });
    let n = notes_with("1.2.3", &emoji).await.1.expect("노트");
    assert!(n.truncated == Some(true) && n.body.ends_with('\u{fffd}'));
}

#[test]
fn 요청_한도는_읽기_쉬운_오류로() {
    for st in [403, 429] {
        assert_eq!(parse_release_response(st, ""), Err("GitHub 요청 한도에 걸렸습니다. 잠시 후 다시 시도하세요".to_string()));
    }
    assert_eq!(parse_release_response(404, "x"), Ok(None));
    assert_eq!(parse_release_response(500, ""), Err("릴리스 노트를 가져오지 못했습니다 (HTTP 500)".to_string()));
    assert!(parse_release_response(200, "").is_err());
    assert!(parse_release_response(200, "not json").is_err());
    assert_eq!(parse_release_response(200, "[]"), Ok(None));
}

// ── 소스 검사 ─────────────────────────────────────────────────────────────────

fn rust_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in fs::read_dir(dir).expect("폴더") {
        let p = e.expect("항목").path();
        if p.is_dir() {
            rust_sources(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

#[test]
fn 판정_로직은_네트워크_파일_프로세스를_쓰지_않는다() {
    let mut files = Vec::new();
    rust_sources(&root().join("src-tauri/src/update"), &mut files);
    assert!(files.len() >= 2);
    for f in files {
        // 주석(`//` 줄)은 빼고 본다
        let text: String = fs::read_to_string(&f).expect("소스").lines().filter(|l| !l.trim_start().starts_with("//")).collect::<Vec<_>>().join("\n");
        for bad in ["reqwest", "std::fs", "tokio::fs", "std::net", "tokio::net", "Command", "std::env", "SystemTime", "Instant::now", "tauri::", "net::http"] {
            assert!(!text.contains(bad), "{}: `{bad}` — 순수 로직에는 입출력이 없다", f.display());
        }
    }
}

#[test]
fn 판정_로직은_앱에_연결되지_않았다() {
    // PR 4(채널 연결)에서 이 시험을 바꾼다
    let mut files = Vec::new();
    rust_sources(&root().join("src-tauri/src"), &mut files);
    for f in files.into_iter().filter(|f| !f.starts_with(root().join("src-tauri/src/update"))) {
        let text = fs::read_to_string(&f).expect("소스");
        assert!(!text.contains("update::logic") && !text.contains("crate::update"), "{}: 아직 앱에 연결하지 않는다", f.display());
    }
}
