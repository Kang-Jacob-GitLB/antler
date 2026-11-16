//! fetch 가드 판정 로직의 골든 패리티 테스트. `tests/golden/fetch/*.json`(TS 가 기준: `UPDATE_GOLDEN=1 npx vitest run tests/goldenFetch.test.ts`)을
//! 모두 읽어 Rust 이식(`antler_lib::git::fetch`)이 같은 결과를 내는지 본다. **건너뛰는 골든이 없다** — 처리기 없는 파일이 있으면 실패한다.
//! 안전 방향 확인: "Rust 가 허용하는데 TS 가 거부하는" 경우가 하나라도 있으면 실패한다(판정 그리드 전체, `허용_판정_그리드`).
//! 위험 목록(이식 우회 위험)의 항목은 `위험N_…` 이름의 테스트가 따로 대응한다.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use antler_lib::git::fetch::confirm_text::{display_units, format_item_units, is_special, quote_units, MAX_CONFIRM_ITEMS};
use antler_lib::git::fetch::guard::*;
use antler_lib::git::fetch::lowercase_table::{CASED_RANGES, CASE_IGNORABLE_RANGES, LOWERCASE_MAP};
use antler_lib::git::fetch::special_table::{SPECIAL_RANGES, UNICODE_VERSION};
use antler_lib::git::fetch::{js_to_lowercase, js_trim, slice_utf16_lossy};
use serde_json::{json, Value};

fn golden_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().expect("src-tauri 의 상위").join("tests").join("golden").join("fetch")
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

fn units(v: &Value) -> Vec<u16> {
    v.as_array().expect("코드 단위 배열").iter().map(|n| u16::try_from(n.as_u64().expect("정수")).expect("u16")).collect()
}

fn bit(v: &Value) -> bool {
    match s(v) {
        "1" => true,
        "0" => false,
        other => panic!("1/0 이 아니다: {other}"),
    }
}

/// 처리하는 골든 파일. 목록에 없는 파일이 디렉터리에 있으면 `골든_파일마다_처리기가_있다` 가 실패한다
const HANDLED: [&str; 8] = ["lists.json", "allowed.json", "urls.json", "config.json", "inspect.json", "confirm.json", "misc.json", "fuzz.json"];

/// 어긋난 항목을 모았다가 한 번에 보고한다(앞 몇 개만)
#[derive(Default)]
struct Diffs {
    checked: usize,
    bad: Vec<String>,
    /// Rust 는 허용, TS 는 거부 — 보안 방향의 어긋남
    unsound: Vec<String>,
}

impl Diffs {
    fn check(&mut self, what: impl FnOnce() -> String, got: bool, expected: bool) {
        self.checked += 1;
        if got != expected {
            let msg = format!("{} → Rust {got}, TS {expected}", what());
            if got {
                self.unsound.push(msg.clone());
            }
            self.bad.push(msg);
        }
    }

    fn finish(self, name: &str) {
        assert!(self.checked > 0, "{name}: 확인한 항목이 0 개다(골든을 건너뜀)");
        assert!(self.unsound.is_empty(), "{name}: Rust 허용 ⇒ TS 허용 위반 {}건, 예: {:?}", self.unsound.len(), &self.unsound[..self.unsound.len().min(5)]);
        assert!(self.bad.is_empty(), "{name}: TS 와 다른 판정 {}건 / {}, 예: {:?}", self.bad.len(), self.checked, &self.bad[..self.bad.len().min(5)]);
    }
}

#[test]
fn 골든_파일마다_처리기가_있다() {
    let mut found: Vec<String> = fs::read_dir(golden_dir())
        .expect("tests/golden/fetch")
        .map(|e| e.expect("항목").file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".json"))
        .collect();
    found.sort();
    let mut handled: Vec<String> = HANDLED.iter().map(ToString::to_string).collect();
    handled.sort();
    assert_eq!(found, handled, "처리기가 없는 골든이 있거나 골든이 없다");
}

// ── 목록 ──────────────────────────────────────────────────────────────────

#[test]
fn 허용_목록_소스와_값_표가_ts_와_글자_그대로_같다() {
    let g = golden("lists.json");
    assert_eq!(s(&g["runtime"]["unicode"]), UNICODE_VERSION, "special_table.rs 를 만든 Unicode 판과 골든이 다르다 — 둘 다 다시 만든다");
    let keys = arr(&g["allowedKeys"], "allowedKeys");
    assert_eq!(keys.len(), ALLOWED_KEY_SOURCES.len(), "ALLOWED_KEYS 개수");
    for (k, src) in keys.iter().zip(ALLOWED_KEY_SOURCES) {
        assert_eq!(s(&k["source"]), src);
        assert_eq!(s(&k["flags"]), "", "플래그가 있는 정규식은 매처가 모른다: {src}");
    }
    let mut values = serde_json::Map::new();
    for (k, vs) in ALLOWED_VALUES {
        values.insert((*k).to_string(), json!(vs));
    }
    assert_eq!(g["allowedValues"], Value::Object(values));
    for (name, src) in [("remoteUrlKey", REMOTE_URL_KEY_SOURCE), ("submoduleUrlKey", SUBMODULE_URL_KEY_SOURCE)] {
        assert_eq!(s(&g[name]["source"]), src, "{name}");
        assert_eq!(s(&g[name]["flags"]), "", "{name}");
    }
    // 손으로 옮긴 정규식: 소스가 바뀌면 구현을 다시 봐야 한다
    let hw = &g["handWritten"];
    assert_eq!(s(&hw["httpUserinfo"]["source"]), HTTP_USERINFO_SOURCE);
    assert_eq!(s(&hw["httpUserinfo"]["flags"]), HTTP_USERINFO_FLAGS);
    assert_eq!(s(&hw["ntNamespace"]["source"]), NT_NAMESPACE_SOURCE);
    assert_eq!(s(&hw["ntNamespace"]["flags"]), "");
    assert_eq!(s(&hw["worktreeConfigAbsent"]["source"]), WORKTREE_CONFIG_ABSENT_SOURCE);
    assert_eq!(s(&hw["worktreeConfigAbsent"]["flags"]), "");
}

// ── 허용 판정 ─────────────────────────────────────────────────────────────

#[test]
fn 허용_판정_그리드가_ts_와_같고_rust_허용은_ts_허용이다() {
    let g = golden("allowed.json");
    let ax = &g["axes"];
    let prefixes: Vec<&str> = arr(&ax["prefixes"], "prefixes").iter().map(s).collect();
    let middles: Vec<String> = arr(&ax["middles"], "middles")
        .iter()
        .map(|m| match m.get("text") {
            Some(t) => s(t).to_string(),
            None => s(&m["repeat"]).repeat(m["count"].as_u64().expect("count") as usize),
        })
        .collect();
    let suffixes: Vec<&str> = arr(&ax["suffixes"], "suffixes").iter().map(s).collect();
    let mut d = Diffs::default();
    for p in &prefixes {
        for (mi, m) in middles.iter().enumerate() {
            for form in 0..2 {
                let verdict = s(&g["verdicts"][*p][mi][form]).as_bytes();
                assert_eq!(verdict.len(), suffixes.len());
                for (si, suf) in suffixes.iter().enumerate() {
                    let key = format!("{p}{m}{}{suf}", if form == 1 { "." } else { "" });
                    let got = is_allowed_config(&key, Some("x"));
                    d.check(|| format!("{:?}", key.chars().take(60).collect::<String>()), got, verdict[si] == b'1');
                }
            }
        }
    }
    d.finish("그리드");
}

#[test]
fn 허용_판정_개별_표와_값_근접_오답_refspec() {
    let g = golden("allowed.json");
    for name in ["cases", "valueCases", "refspec"] {
        let mut d = Diffs::default();
        for c in arr(&g[name], name) {
            let (key, value, expected) = (s(&c[0]), opt_s(&c[1]), bit(&c[2]));
            d.check(|| format!("{key:?} = {value:?}"), is_allowed_config(key, value), expected);
        }
        d.finish(name);
    }
}

// ── URL ───────────────────────────────────────────────────────────────────

#[test]
fn remote_url_판정_사유가_ts_와_같다() {
    let g = golden("urls.json");
    let mut bad = Vec::new();
    for c in arr(&g["cases"], "cases") {
        let (url, expected) = (s(&c[0]), opt_s(&c[1]));
        let got = unsafe_remote_url(url);
        if got.as_deref() != expected {
            bad.push(format!("{url:?}: Rust {got:?}, TS {expected:?}"));
        }
    }
    assert!(bad.is_empty(), "{}건, 예: {:?}", bad.len(), &bad[..bad.len().min(5)]);
}

// ── 설정 파싱·검사 ────────────────────────────────────────────────────────

#[test]
fn 설정_목록_파싱과_키_소문자화가_ts_와_같다() {
    let g = golden("config.json");
    for c in arr(&g["list"], "list") {
        assert_eq!(serde_json::to_value(parse_config_list(s(&c["out"]))).expect("직렬화"), c["entries"], "{:?}", c["out"]);
    }
    for c in arr(&g["scoped"], "scoped") {
        assert_eq!(serde_json::to_value(parse_scoped_config_list(s(&c["out"]), s(&c["scope"]))).expect("직렬화"), c["entries"], "{:?}", c["out"]);
    }
    // 코드 포인트 하나씩의 소문자화: std 가 아니라 Node 표(생성 파일)만 쓰므로 예외 없이 **완전히** 같아야 한다
    let map: HashMap<u32, &str> = arr(&g["lowercaseMap"], "lowercaseMap").iter().map(|e| (e[0].as_u64().expect("cp") as u32, s(&e[1]))).collect();
    let mut bad = Vec::new();
    for cp in (0..=0x10ffffu32).filter(|cp| !(0xd800..=0xdfff).contains(cp)) {
        let ch = char::from_u32(cp).expect("서로게이트 제외");
        let want = map.get(&cp).map_or_else(|| ch.to_string(), |l| (*l).to_string());
        let got = js_to_lowercase(&ch.to_string());
        if got != want {
            bad.push(format!("U+{cp:X}: Rust {got:?}, TS {want:?}"));
        }
    }
    assert!(bad.is_empty(), "소문자화가 Node 와 다른 코드 포인트 {}개: {:?}", bad.len(), &bad[..bad.len().min(10)]);
    // 생성 표가 골든과 같다(골든을 다시 만들었으면 node scripts/gen-fetch-special.mjs)
    let table: Vec<(u32, &str)> = LOWERCASE_MAP.to_vec();
    let golden_map: Vec<(u32, &str)> = arr(&g["lowercaseMap"], "lowercaseMap").iter().map(|e| (e[0].as_u64().expect("cp") as u32, s(&e[1]))).collect();
    assert_eq!(table, golden_map, "lowercase_table.rs 가 골든과 다르다 — node scripts/gen-fetch-special.mjs");
    let ranges = |v: &Value| -> Vec<(u32, u32)> { arr(v, "범위").iter().map(|r| (r[0].as_u64().expect("lo") as u32, r[1].as_u64().expect("hi") as u32)).collect() };
    assert_eq!(ranges(&g["cased"]), CASED_RANGES, "cased 범위");
    assert_eq!(ranges(&g["caseIgnorable"]), CASE_IGNORABLE_RANGES, "caseIgnorable 범위");
    for c in arr(&g["lowercase"], "lowercase") {
        let entries = parse_config_list(&format!("x\0{}\n\0", s(&c["input"])));
        assert_eq!(entries[0].key, s(&c["key"]), "{:?}", c["input"]);
        assert_eq!(js_to_lowercase(s(&c["input"])), s(&c["key"]), "{:?}", c["input"]);
    }
}

#[test]
fn 유니코드_판_차이_std_소문자화만_아는_대문자는_ts_처럼_그대로_둔다() {
    // Node(Unicode 16.0)는 U+A7CE→A7CF 를 모른다 — std 의 to_lowercase 를 쓰면 `remote."꟎".fetch` 가 Rust 에서만 허용된다
    assert_eq!(js_to_lowercase("\u{a7ce}"), "\u{a7ce}");
    assert_eq!(js_to_lowercase("\u{16ea0}\u{1df40}"), "\u{16ea0}\u{1df40}");
    let out = "local\0remote.\u{a7ce}.fetch\n+refs/heads/*:refs/remotes/\u{a7cf}/*\0";
    let ins = inspect_entries(&parse_config_list(out));
    assert_eq!(ins.dangerous.len(), 1, "알 수 없는 설정이어야 한다: {:?}", ins.dangerous);
    // 같은 이름을 쓰는 refspec 은 TS 도 허용한다
    let same = inspect_entries(&parse_config_list("local\0remote.\u{a7ce}.fetch\n+refs/heads/*:refs/remotes/\u{a7ce}/*\0"));
    assert!(same.dangerous.is_empty());
    // 최종 시그마: 단어 끝만 ς
    assert_eq!(js_to_lowercase("ΑΣ"), "ας");
    assert_eq!(js_to_lowercase("ΑΣ ΑΣ"), "ας ας");
    assert_eq!(js_to_lowercase("ΣΑ"), "σα");
    assert_eq!(js_to_lowercase("Σ"), "σ");
    assert_eq!(js_to_lowercase("Α.Σ"), "α.ς");
}

/// 가짜 git 응답으로 TS `inspectConfig` 가 한 일을 순수 함수로 다시 한다(실행기는 뒤 단계). 설정을 읽은 순서(calls)까지 맞아야 한다
fn replay(sc: &Value) -> Value {
    let out_of = |r: &Value| (r["code"].as_i64().expect("code") as i32, r["stdout"].as_str().unwrap_or("").to_string(), r["stderr"].as_str().unwrap_or("").to_string());
    let not_found = || (128, String::new(), "fatal: not found".to_string());
    let mut calls: Vec<&str> = vec!["show-scope"];
    let (code, stdout, stderr) = out_of(&sc["show"]);
    let entries = match show_scope_outcome(code, &stderr) {
        ShowScopeOutcome::Parsed => parse_config_list(&stdout),
        ShowScopeOutcome::Error(message) => return json!({ "calls": calls, "error": message }),
        ShowScopeOutcome::Fallback => {
            let mut walk = ScopeWalk::start();
            loop {
                let scope = walk.scope();
                calls.push(scope);
                let (code, stdout, stderr) = sc["byScope"].get(scope).map_or_else(not_found, out_of);
                match walk.feed(code, &stdout, &stderr) {
                    Err(message) => return json!({ "calls": calls, "error": message }),
                    Ok(WalkStep::Read(next)) => walk = next,
                    Ok(WalkStep::Done(entries)) => break entries,
                }
            }
        }
    };
    let mut ins = inspect_entries(&entries);
    let (code, stdout, stderr) = out_of(&sc["list"]);
    let list = if code == 0 { GitOut::Ok(&stdout) } else { GitOut::Failed(&stderr) };
    if let Some(names) = apply_remote_list(&mut ins, list) {
        for name in names {
            let (code, stdout, stderr) = sc["urls"].get(&name).map_or_else(not_found, out_of);
            apply_remote_urls(&mut ins, &name, if code == 0 { GitOut::Ok(&stdout) } else { GitOut::Failed(&stderr) });
        }
        finish_remote_urls(&mut ins);
    }
    json!({ "calls": calls, "inspection": ins })
}

#[test]
fn 검사_결과와_지문이_ts_와_같다() {
    let g = golden("inspect.json");
    for sc in arr(&g["scenarios"], "scenarios") {
        let got = replay(sc);
        let label = s(&sc["label"]);
        assert_eq!(got["calls"], sc["calls"], "{label}: 설정을 읽은 순서");
        assert_eq!(got.get("error"), sc.get("error"), "{label}");
        assert_eq!(got.get("inspection"), sc.get("inspection"), "{label}");
    }
    for c in arr(&g["entries"], "entries") {
        assert_eq!(serde_json::to_value(inspect_entries(&parse_config_list(s(&c["out"])))).expect("직렬화"), c["inspection"], "{:?}", c["out"]);
    }
    for c in arr(&g["fingerprints"], "fingerprints") {
        let dangerous: Vec<Dangerous> = arr_or_empty(&c["dangerous"])
            .iter()
            .map(|d| Dangerous { key: s(&d["key"]).into(), value: s(&d["value"]).into(), raw: opt_s(&d["raw"]).map(str::to_string) })
            .collect();
        assert_eq!(fingerprint_of(&dangerous), s(&c["sha"]), "{:?}", c["dangerous"]);
    }
}

fn arr_or_empty(v: &Value) -> Vec<Value> {
    v.as_array().cloned().unwrap_or_default()
}

// ── 확인 창 표시 ──────────────────────────────────────────────────────────

#[test]
fn 확인_창_표시가_ts_와_같고_특수_문자_표가_골든과_같다() {
    let g = golden("confirm.json");
    assert_eq!(g["maxConfirmItems"].as_u64(), Some(MAX_CONFIRM_ITEMS as u64));
    for c in arr(&g["quote"], "quote") {
        assert_eq!(quote_units(&units(&c["units"])), s(&c["quote"]), "{:?}", c["units"]);
    }
    for c in arr(&g["display"], "display") {
        let max = c.get("max").map_or(80, |m| m.as_u64().expect("max") as usize);
        assert_eq!(display_units(&units(&c["units"]), max), s(&c["display"]), "{:?} max {max}", c["units"]);
    }
    for c in arr(&g["formatItem"], "formatItem") {
        assert_eq!(format_item_units(&units(&c["key"]), &units(&c["value"])), s(&c["formatted"]));
    }
    // 표: 골든(TS 가 0..=0x10FFFF 를 훑은 결과)과 같고, 경계 앞뒤 판정이 맞는다
    let ranges: Vec<(u32, u32)> = arr(&g["special"], "special").iter().map(|r| (r[0].as_u64().expect("lo") as u32, r[1].as_u64().expect("hi") as u32)).collect();
    assert_eq!(ranges, SPECIAL_RANGES, "special_table.rs 가 골든과 다르다 — node scripts/gen-fetch-special.mjs");
    for b in arr(&g["boundaries"], "boundaries") {
        assert_eq!(is_special(b[0].as_u64().expect("cp") as u32), bit(&b[1]), "U+{:X}", b[0].as_u64().unwrap());
    }
}

// ── 그 밖(버전·오류 분류·인자·환경·stderr 판정) ───────────────────────────

fn version_of(v: &Value) -> Option<GitVersion> {
    v.as_array().map(|a| [a[0].as_f64().expect("수"), a[1].as_f64().expect("수"), a[2].as_f64().expect("수")])
}

#[test]
fn 버전_분류_백오프_인자_환경이_ts_와_같다() {
    let g = golden("misc.json");
    for c in arr(&g["classify"], "classify") {
        let got = if c["aborted"].as_bool() == Some(true) { FetchFailure::Network } else { classify_failure(s(&c["text"]), c["github"].as_bool().expect("github")) };
        let want = match s(&c["kind"]) {
            "auth" => FetchFailure::Auth,
            "protocol" => FetchFailure::Protocol,
            "network" => FetchFailure::Network,
            other => panic!("{other}"),
        };
        assert_eq!(got, want, "{:?}", c);
    }
    for c in arr(&g["delays"], "delays") {
        let want = c[2].as_f64().expect("ms");
        assert_eq!(next_delay_ms(c[0].as_f64().expect("분"), c[1].as_u64().expect("실패 수") as u32), want, "{c}");
    }
    for c in arr(&g["versions"], "versions") {
        assert_eq!(parse_git_version(s(&c["text"])), version_of(&c["version"]), "{:?}", c["text"]);
    }
    assert_eq!(Some(FILE_TRANSPORT_MIN_GIT), version_of(&g["fileTransportMinGit"]));
    for c in arr(&g["atLeast"], "atLeast") {
        let min = version_of(&c["min"]).expect("min");
        assert_eq!(version_at_least(version_of(&c["v"]), min), c["ok"].as_bool().expect("ok"), "{c}");
    }
    let mut built = 0;
    for c in arr(&g["fetchConfigArgs"], "fetchConfigArgs") {
        let follow_redirects = match opt_s(&c["opts"]["followRedirects"]) {
            None | Some("initial") => FollowRedirects::Initial,
            Some("false") => FollowRedirects::False,
            Some(other) => panic!("{other}"),
        };
        let opts = ConfigArgsOpts { credential_helpers: c["opts"]["credentialHelpers"].as_bool().unwrap_or(false), follow_redirects };
        let want: Vec<&str> = arr(&c["args"], "args").iter().map(s).collect();
        // hooksPath 는 hooks_path_for 로만 만든다: `/dev/null` 은 로컬이 아닌 원본, 나머지는 로컬의 앱 소유 디렉터리(검사는 주입). 빈 값은 만들 수 없어 건너뛴다
        let hooks = match s(&c["hooksPath"]) {
            "" => {
                assert!(hooks_path_with(true, Some(""), true, |_| true).is_err(), "빈 값은 Windows 에서 디렉터리가 없는 것과 같다");
                assert_eq!(hooks_path_with(true, Some(""), false, |_| true).expect("POSIX").as_str(), "/dev/null");
                continue;
            }
            "/dev/null" => hooks_path_for(false, None, true).expect("원격은 /dev/null"),
            dir => hooks_path_with(true, Some(dir), true, |_| true).expect("앱 소유 디렉터리"),
        };
        assert_eq!(hooks.as_str(), s(&c["hooksPath"]));
        assert_eq!(fetch_config_args(&hooks, opts), want, "{c}");
        built += 1;
    }
    assert!(built > 0);
    // 훅 경로: 상대 경로·비어 있지 않은 디렉터리·Windows 인데 디렉터리 없음은 거부, 역슬래시는 슬래시로
    for rel in ["nohooks", "./h", "../h", "C:h"] {
        assert!(hooks_path_with(true, Some(rel), true, |_| true).is_err(), "{rel}");
    }
    assert!(hooks_path_with(true, Some("C:/h"), true, |_| false).is_err());
    assert!(hooks_path_with(true, None, true, |_| true).is_err());
    assert_eq!(hooks_path_with(true, None, false, |_| true).expect("POSIX").as_str(), "/dev/null");
    assert_eq!(hooks_path_with(true, Some("C:\\Users\\me\\nohooks"), true, |_| true).expect("절대").as_str(), "C:/Users/me/nohooks");
    assert_eq!(hooks_path_with(false, Some("relative"), true, |_| false).expect("원격은 무시").as_str(), "/dev/null");
    // 실제 파일 시스템 검사: 없는 디렉터리·비어 있지 않은 디렉터리는 거부, 빈 디렉터리는 통과
    let dir = std::env::temp_dir().join(format!("antler-hooks-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    let path = dir.to_string_lossy().into_owned();
    assert!(hooks_path_for(true, Some(&path), true).is_err(), "없는 디렉터리");
    fs::create_dir_all(&dir).expect("임시 디렉터리");
    assert!(hooks_path_for(true, Some(&path), true).is_ok(), "빈 디렉터리");
    fs::write(dir.join("x"), "").expect("파일");
    assert!(hooks_path_for(true, Some(&path), true).is_err(), "비어 있지 않은 디렉터리");
    fs::remove_dir_all(&dir).expect("정리");
    let fetch_args: Vec<&str> = arr(&g["fetchArgs"], "fetchArgs").iter().map(s).collect();
    assert_eq!(fetch_args, FETCH_ARGS);
    for c in arr(&g["ssh"], "ssh") {
        let env: HashMap<String, String> = c["env"].as_object().expect("env").iter().map(|(k, v)| (k.clone(), s(v).to_string())).collect();
        let got = resolve_ssh_command(opt_s(&c["userSshCommand"]), s(&c["kind"]), &env, s(&c["platform"]) == "win32");
        assert_eq!(got, s(&c["result"]), "{c}");
    }
    for c in arr(&g["fetchEnv"], "fetchEnv") {
        let want: Vec<(String, String)> = arr(&c["env"], "env").iter().map(|p| (s(&p[0]).to_string(), s(&p[1]).to_string())).collect();
        let got: Vec<(String, String)> = fetch_env(s(&c["sshCommand"]), c["allowFile"].as_bool().expect("allowFile")).into_iter().map(|(k, v)| (k.to_string(), v)).collect();
        assert_eq!(got, want, "{c}");
        // GIT_ASKPASS 는 빈 값으로 남는다(없애면 git 이 상위 환경의 askpass 를 쓴다)
        assert!(got.contains(&("GIT_ASKPASS".to_string(), String::new())));
    }
}

#[test]
fn stderr_판정이_ts_와_같다() {
    let g = golden("misc.json");
    for c in arr(&g["unknownShowScope"], "unknownShowScope") {
        let stderr = s(&c["stderr"]);
        let got = match show_scope_outcome(129, stderr) {
            ShowScopeOutcome::Parsed => json!({ "outcome": "parsed" }),
            ShowScopeOutcome::Fallback => json!({ "outcome": "fallback" }),
            ShowScopeOutcome::Error(message) => json!({ "outcome": "error", "message": message }),
        };
        assert_eq!(got["outcome"], c["outcome"], "{stderr:?}");
        assert_eq!(got.get("message"), c.get("message"), "{stderr:?}");
    }
    assert_eq!(show_scope_outcome(0, "unknown option show-scope"), ShowScopeOutcome::Parsed);
    for c in arr(&g["unknownWriteFetchHead"], "unknownWriteFetchHead") {
        assert_eq!(unknown_write_fetch_head(s(&c["stderr"])), c["retries"].as_bool().expect("retries"), "{:?}", c["stderr"]);
    }
    for c in arr(&g["worktreeConfigAbsent"], "worktreeConfigAbsent") {
        let stderr = s(&c["stderr"]);
        assert_eq!(worktree_config_absent(stderr), c["regex"].as_bool().expect("regex"), "{stderr:?}");
        // 범위별 읽기에서 worktree 실패가 "없음"으로 취급되는가(던지지 않는가)
        let worktree = walk_to_worktree();
        assert_eq!(worktree.feed(1, "", stderr).is_ok(), c["absent"].as_bool().expect("absent"), "{stderr:?}");
    }
    for c in arr(&g["worktreeOn"], "worktreeOn") {
        let local = match opt_s(&c["value"]) {
            None => "extensions.worktreeconfig\0".to_string(),
            Some(v) => format!("extensions.worktreeconfig\n{v}\0"),
        };
        let mut walk = ScopeWalk::start();
        let mut asked = false;
        loop {
            asked |= walk.scope() == "worktree";
            let stdout = if walk.scope() == "local" { local.as_str() } else { "" };
            match walk.feed(0, stdout, "").expect("읽기 성공") {
                WalkStep::Read(next) => walk = next,
                WalkStep::Done(_) => break,
            }
        }
        assert_eq!(asked, c["asked"].as_bool().expect("asked"), "{:?}", c["value"]);
    }
}

/// system → global → local(worktreeconfig 켬)을 읽고 worktree 차례가 된 상태
fn walk_to_worktree() -> ScopeWalk {
    let mut walk = ScopeWalk::start();
    let mut order = Vec::new();
    while walk.scope() != "worktree" {
        order.push(walk.scope());
        let stdout = if walk.scope() == "local" { "extensions.worktreeconfig\ntrue\0" } else { "" };
        match walk.feed(0, stdout, "").expect("읽기 성공") {
            WalkStep::Read(next) => walk = next,
            WalkStep::Done(_) => panic!("worktree 차례가 오지 않았다"),
        }
    }
    assert_eq!(order, ["system", "global", "local"]);
    walk
}

#[test]
fn 범위별_읽기는_순서를_타입으로_강제하고_실패를_구분한다() {
    // system·global 은 실패해도 계속, local 실패는 중단, worktree 는 "없음"만 계속
    let mut walk = ScopeWalk::start();
    assert_eq!(walk.scope(), "system");
    for scope in ["system", "global"] {
        assert_eq!(walk.scope(), scope);
        walk = match walk.feed(1, "", "fatal: unable to read config file").expect("계속") {
            WalkStep::Read(next) => next,
            WalkStep::Done(_) => panic!("{scope}"),
        };
    }
    assert_eq!(walk.scope(), "local");
    assert!(walk.feed(1, "", "fatal: bad config").is_err());
    // worktreeconfig 가 꺼져 있으면 worktree 는 건너뛴다
    let mut walk = ScopeWalk::start();
    let entries = loop {
        let stdout = if walk.scope() == "local" { "a.b\nc\0" } else { "" };
        match walk.feed(0, stdout, "").expect("읽기 성공") {
            WalkStep::Read(next) => {
                assert_ne!(next.scope(), "worktree");
                walk = next;
            }
            WalkStep::Done(entries) => break entries,
        }
    };
    assert_eq!(entries.len(), 1);
    assert_eq!((entries[0].scope.as_str(), entries[0].key.as_str()), ("local", "a.b"));
}

// ── 위험 목록(이식 우회 위험)에 대응하는 테스트 ────────────────────────────

#[test]
fn 위험1_bom_앞의_unc_경로도_거부한다() {
    // JS trim 은 U+FEFF 를 지운다 → 그 뒤의 `\\host` 가 UNC 로 걸린다(Rust 의 trim 은 지우지 않는다)
    for url in ["\u{feff}\\\\host\\share\\x.git", "\u{feff}//host/share", " \u{feff}\\??\\UNC\\h\\s", "\u{2028}//h/s", "\u{a0}\\\\h\\s"] {
        assert!(unsafe_remote_url(url).is_some(), "{url:?}");
    }
    assert_eq!(js_trim("\u{feff}a\u{feff}"), "a");
    // U+0085 는 JS 도 지우지 않는다 → 앞이 `/` 가 아니므로 UNC 가 아니다
    assert_eq!(unsafe_remote_url("\u{85}//h/s"), None);
}

#[test]
fn 위험2_js_s_와_rust_공백의_차이가_refspec_판정에_새지_않는다() {
    // JS \s 는 U+FEFF 를 공백으로 보고 U+0085 는 공백으로 보지 않는다
    let ok = |name: &str| fetch_refspec_ok("remote.o.fetch", Some(&format!("+refs/heads/{name}:refs/remotes/o/{name}")));
    assert!(!ok("a\u{feff}b"));
    assert!(ok("a\u{85}b"));
    assert!(!ok("a\u{a0}b"));
    assert!(!ok("a b"));
    assert!(!ok("a\tb"));
    assert!(!ok("a\u{2028}b"));
}

#[test]
fn 위험3_점은_모든_문자가_아니다() {
    assert!(is_allowed_config("remote.origin.url", Some("x")));
    for lt in ["\n", "\r", "\u{2028}", "\u{2029}"] {
        assert!(!is_allowed_config(&format!("remote.a{lt}b.url"), Some("x")), "{lt:?}");
        assert!(!is_allowed_config(&format!("branch.a{lt}b.merge"), Some("x")), "{lt:?}");
        assert!(!is_allowed_config(&format!("remote.a{lt}b.fetch"), Some("refs/heads/*:refs/remotes/a/*")), "{lt:?}");
    }
    // `$` 는 맨 끝만: 끝 개행 앞에서 맞지 않는다
    assert!(!is_allowed_config("user.name\n", Some("x")));
    assert!(!is_allowed_config("remote.a.url\n", Some("x")));
}

#[test]
fn 위험4_i_플래그는_ascii_만_접는다() {
    // ſ(U+017F)·K(U+212A)는 s·k 로 접히지 않는다 — 접히면 "알 수 없는 오류"가 인증 오류로 바뀐다
    assert_eq!(classify_failure("acce\u{17f}s denied", false), FetchFailure::Network);
    assert_eq!(classify_failure("access denied", false), FetchFailure::Auth);
    assert_eq!(classify_failure("ACCESS DENIED", false), FetchFailure::Auth);
    assert_eq!(classify_failure("authentication failed \u{212a}", false), FetchFailure::Auth);
    assert_eq!(classify_failure("\u{212a}authentication failed", false), FetchFailure::Auth);
    assert_eq!(classify_failure("pub\u{212a}ey", false), FetchFailure::Network);
    assert!(!unknown_show_scope("unknown option show-\u{17f}cope"));
    assert!(!unknown_show_scope("un\u{212a}nown option show-scope"));
    assert!(unknown_show_scope("UNKNOWN OPTION SHOW-SCOPE"));
    assert_eq!(redact_userinfo("http\u{17f}://u@h/x"), None);
    assert_eq!(redact_userinfo("HTTPS://u@h/x").as_deref(), Some("HTTPS://<userinfo>@h/x"));
    // file: 스킴도 ASCII 만 접는다
    assert!(unsafe_remote_url("FILE://host/share").is_some());
    assert_eq!(unsafe_remote_url("\u{fb01}le://host/share"), None);
}

#[test]
fn 위험5_utf16_경계에서_자르기가_패닉하지_않고_ts_와_같다() {
    // 앞 48·뒤 32 단위의 경계에 서로게이트 쌍이 걸리는 모든 위치(골든이 일부를 TS 와 대조한다)
    for len in 81..140usize {
        for at in 0..len - 1 {
            let mut u: Vec<u16> = vec![u16::from(b'a'); len];
            u[at] = 0xd83d;
            u[at + 1] = 0xde00;
            let shown = display_units(&u, 80);
            assert!(shown.starts_with('"'), "{len} {at}");
            assert!(shown.ends_with(&format!("(길이 {len})")), "{len} {at}");
        }
    }
    // 반쪽만 남으면 \ud83d / \ude00 으로 드러난다
    let mut u: Vec<u16> = vec![u16::from(b'a'); 100];
    u[47] = 0xd83d;
    u[48] = 0xde00;
    assert!(display_units(&u, 80).contains("\\ud83d…"));
    // 서로게이트 쌍이 UTF-16 단위로 둘이라는 길이 표기
    let emoji: Vec<u16> = "😀".repeat(50).encode_utf16().collect();
    assert!(display_units(&emoji, 80).ends_with("(길이 100)"));
    // 오류 문구 자르기: 짝이 쪼개지면 반쪽이 U+FFFD
    assert_eq!(slice_utf16_lossy("a😀b", 2), "a\u{fffd}");
    assert_eq!(slice_utf16_lossy("a😀b", 3), "a😀");
    assert_eq!(slice_utf16_lossy("가나", 10), "가나");
    // 아주 큰 입력과 서로게이트 입력도 패닉 없이 끝난다
    let big = "😀x".repeat(200_000);
    let _ = unsafe_remote_url(&big);
    let _ = is_allowed_config(&format!("remote.{big}.url"), Some(&big));
    let _ = fetch_refspec_ok(&format!("remote.{big}.fetch"), Some(&format!("refs/heads/{big}:refs/remotes/{big}/{big}")));
    let _ = parse_config_list(&format!("local\0{big}\n{big}\0"));
    let _ = classify_failure(&big, true);
    let _ = redact_userinfo(&format!("https://{big}@h"));
    let _ = parse_git_version(&format!("git version {big}"));
    let _ = js_trim(&big);
}

#[test]
fn 위험14_nt_네임스페이스_경로는_거부한다() {
    for url in ["\\??\\UNC\\host\\share\\x.git", "  \\??\\UNC\\h\\s", "/??/UNC/h/s", "file:///??/UNC/host/share/x.git", "file:/??/UNC/h/s", "C:\\a\\??\\b"] {
        let reason = unsafe_remote_url(url).unwrap_or_else(|| panic!("{url:?} 는 거부돼야 한다"));
        assert!(reason.contains("NT 네임스페이스"), "{url:?}: {reason}");
    }
    assert!(has_nt_namespace("a\\??\\b"));
    assert!(!has_nt_namespace("a\\?b\\"));
    // 로컬 경로·file:///C:/ 는 그대로 둔다
    for ok in ["file:///C:/repos/x.git", "C:\\repos\\x.git", "/tmp/r.git"] {
        assert_eq!(unsafe_remote_url(ok), None, "{ok}");
    }
}

/// 퍼즈 코퍼스: TS 가 만든 입력 문자열(생성 규칙은 따라 하지 않는다)에 대한 판정이 같다. 알파벳에 Node 에 없고 Rust std 에는 있는 대문자가 들어 있다
#[test]
fn 퍼즈_코퍼스의_판정이_ts_와_같다() {
    let g = golden("fuzz.json");
    let (mut allowed, mut refspec, mut parsed) = (Diffs::default(), Diffs::default(), Diffs::default());
    let (mut allowed_true, mut refspec_true) = (0, 0);
    for c in arr(&g["config"], "config") {
        let (key, value) = (s(&c[0]), opt_s(&c[1]));
        allowed_true += usize::from(bit(&c[2]));
        refspec_true += usize::from(bit(&c[3]));
        allowed.check(|| format!("{key:?} = {value:?}"), is_allowed_config(key, value), bit(&c[2]));
        refspec.check(|| format!("{key:?} = {value:?}"), fetch_refspec_ok(key, value), bit(&c[3]));
        // 설정 출력으로 파싱(키 소문자화)한 뒤의 판정: 실제 검사가 가는 길
        let out = match value {
            None => format!("local\0{key}\0"),
            Some(v) => format!("local\0{key}\n{v}\0"),
        };
        let entries = parse_config_list(&out);
        parsed.check(|| format!("{out:?}"), is_allowed_config(&entries[0].key, entries[0].value.as_deref()), bit(&c[4]));
    }
    assert!(allowed_true >= 20 && refspec_true >= 20, "코퍼스가 허용 쪽을 충분히 건드리지 않는다: {allowed_true} {refspec_true}");
    allowed.finish("퍼즈 isAllowedConfig");
    refspec.finish("퍼즈 fetchRefspecOk");
    parsed.finish("퍼즈 파싱 뒤 isAllowedConfig");

    let mut bad = Vec::new();
    let urls = arr(&g["urls"], "urls");
    for c in urls {
        let (url, expected) = (s(&c[0]), opt_s(&c[1]));
        let got = unsafe_remote_url(url);
        if got.as_deref() != expected {
            bad.push(format!("{url:?}: Rust {got:?}, TS {expected:?}"));
        }
    }
    assert!(urls.iter().any(|c| c[1].is_null()) && urls.iter().any(|c| !c[1].is_null()), "코퍼스가 한쪽 판정뿐이다");
    assert!(bad.is_empty(), "unsafeRemoteUrl 이 다른 항목 {}건, 예: {:?}", bad.len(), &bad[..bad.len().min(5)]);

    let mut bad = Vec::new();
    for c in arr(&g["lower"], "lower") {
        let entries = parse_config_list(&format!("x\0{}\n\0", s(&c[0])));
        if entries[0].key != s(&c[1]) {
            bad.push(format!("{:?}: Rust {:?}, TS {:?}", c[0], entries[0].key, c[1]));
        }
    }
    assert!(bad.is_empty(), "키 소문자화가 다른 항목 {}건, 예: {:?}", bad.len(), &bad[..bad.len().min(5)]);
}

/// 악성 설정의 아주 긴 키·값에서도 메모리·시간이 입력에 선형이다(수천만 자 측정은 `ANTLER_BIG_PATTERN=<글자 수>` 로, 기본은 CI 용 수백만 자)
#[test]
fn 아주_긴_키에서도_매처가_빠르고_메모리를_아낀다() {
    let n: usize = std::env::var("ANTLER_BIG_PATTERN").ok().and_then(|v| v.parse().ok()).unwrap_or(3_000_000);
    let big = "한x".repeat(n / 2);
    let t = std::time::Instant::now();
    assert!(is_allowed_config(&format!("remote.{big}.url"), Some("x")));
    assert!(is_allowed_config(&format!("submodule.{big}.url"), Some("x")));
    assert!(!is_allowed_config(&format!("remote.{big}.urlx"), Some("x")));
    assert!(!is_allowed_config(&format!("branch.{big}\n.merge"), Some("x")));
    assert!(unsafe_remote_url(&format!("{big}\\??\\x")).is_some());
    let ms = t.elapsed().as_millis();
    println!("긴 키 {n} 글자: {ms} ms");
    assert!(ms < 20_000 * (n as u128 / 3_000_000).max(1), "{ms} ms (선형이어야 한다)");
}

/// 입력이 무엇이든 패닉하지 않는다(결정적 난수로 이모지·서로게이트 반쪽 대용 문자·줄 구분자·점 등을 섞은 문자열)
#[test]
fn 임의_입력에서_패닉하지_않는다() {
    let alphabet: Vec<&str> = vec![
        "a", "b", "x", ".", ":", "*", "+", "\n", "\r", "\u{2028}", "\u{2029}", "\u{feff}", "\u{85}", "😀", "İ", "ſ", "\u{212a}", "Σ", "\\", "/", "?", "@", "-", "\0", " ", "'", "#", "refs/heads/",
        "refs/remotes/", "remote.", ".url", ".fetch", "file:", "https://", "ext::", "git version ", "unknown option ", "show-scope", "config.worktree", "unable to read config file '",
    ];
    let mut seed: u64 = 0x9e37_79b9_7f4a_7c15;
    let mut next = || {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (seed >> 33) as usize
    };
    for _ in 0..4000 {
        let n = next() % 24;
        let text: String = (0..n).map(|_| alphabet[next() % alphabet.len()]).collect();
        let other: String = (0..n).map(|_| alphabet[next() % alphabet.len()]).collect();
        let _ = is_allowed_config(&text, Some(&other));
        let _ = is_allowed_config(&text, None);
        let _ = fetch_refspec_ok(&format!("remote.{text}.fetch"), Some(&other));
        let _ = unsafe_remote_url(&text);
        let _ = redact_userinfo(&text);
        let _ = parse_config_list(&format!("{text}\0{other}"));
        let _ = parse_scoped_config_list(&text, &other);
        let _ = inspect_entries(&parse_config_list(&format!("local\0{text}\n{other}\0")));
        let _ = classify_failure(&text, true);
        let _ = parse_git_version(&text);
        let _ = unknown_show_scope(&text);
        let _ = worktree_config_absent(&text);
        let units: Vec<u16> = text.encode_utf16().collect();
        let _ = display_units(&units, next() % 30);
        let _ = quote_units(&units[..units.len() / 2]);
        let mut ins = inspect_entries(&[]);
        let _ = apply_remote_list(&mut ins, GitOut::Failed(&text));
        apply_remote_urls(&mut ins, &text, GitOut::Ok(&other));
        finish_remote_urls(&mut ins);
    }
}
