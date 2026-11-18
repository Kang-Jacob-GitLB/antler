//! 설정 저장소 코어(`src/config`)와 GitHub 링크 순수 함수(`src/git/github_link.rs`)의 골든 패리티 테스트(Phase 6b PR 1).
//! `tests/golden/config/*.json`(TS 가 기준: `UPDATE_GOLDEN=1 npx vitest run tests/goldenConfig.test.ts`)은 **실제 TS ConfigStore** 를 임시 파일로 돌린 결과다 —
//! 연산 결과·던진 문구·단계별 설정·**최종 파일 바이트**·알림 횟수·미러·fetch 설정. 여기서는 같은 입력(골든에 들어 있다)으로 Rust 를 돌려 같은지 본다.
//!
//! 결정적이게 하려고 저장소 id 는 순번(`id-1`…)을 주입하고, 손상 파일 백업 이름의 시각은 고정한다(`open_for_test`). github 캐시 기준 폴더는 가상 경로라
//! 골든에 실제 경로가 없다(임시 폴더·CI 의 8.3 짧은 이름이 끼어들 곳이 없다). 설정 파일은 이 테스트가 만든 임시 폴더에만 쓰고 실제 사용자 설정을 건드리지 않는다.
//! 객체 키 순서가 바이트를 정하므로 골든도 `serde_json::Value`(키 정렬)가 아니라 순서를 보존하는 `net::json::parse` 로 읽는다.
//!
//! 알려진 차이(골든에 넣지 않았다): 짝 없는 서로게이트는 JSON 해석에서 U+FFFD 가 된다(`net/json.rs`), `__proto__` 키는 Rust 에서 일반 키다,
//! 문자열을 `ui` 로 펼치는 손상 파일에서 BMP 밖 글자는 TS 가 코드 단위로 쪼갠다.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

use antler_lib::config::derive::{fetch_prefs_of, mirror_of};
use antler_lib::config::model::strip_undefined;
use antler_lib::config::ConfigStore;
use antler_lib::git::github_link::{
    cache_path_matches_link, github_cache_path, github_cache_slug, github_full_name, github_remote_url, is_github_cache_path, is_safe_branch, sanitize_github_link,
    validate_github_link, GITHUB_ORIGIN,
};
use antler_lib::net::js_value::stringify_pretty;
use antler_lib::net::json::{parse, Json};

const FIXED_NOW: u128 = 1_700_000_000_000;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().expect("src-tauri 의 상위").to_path_buf()
}

fn golden(name: &str) -> Json {
    let path = root().join("tests").join("golden").join("config").join(name);
    let text = fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    parse(&text).unwrap_or_else(|e| panic!("{name}: {e:?}"))
}

fn key<'a>(v: &'a Json, k: &str) -> &'a Json {
    v.get(k).unwrap_or_else(|| panic!("골든에 `{k}` 가 없다: {}", stringify_pretty(v)))
}

fn arr<'a>(v: &'a Json, what: &str) -> &'a [Json] {
    match v {
        Json::Arr(a) if !a.is_empty() => a,
        _ => panic!("{what} 가 비어 있거나 배열이 아니다(골든을 건너뛴 것과 같다)"),
    }
}

fn string<'a>(v: &'a Json, k: &str) -> &'a str {
    key(v, k).as_str().unwrap_or_else(|| panic!("`{k}` 가 문자열이 아니다"))
}

/// JSON 으로 오가는 값 비교(객체 키는 JS 열거 순서로 쓴다 — 골든도 같은 순서로 직렬화돼 있다)
fn pretty(v: &Json) -> String {
    stringify_pretty(&strip_undefined(v))
}

fn assert_same(actual: &Json, expected: &Json, what: &str) {
    assert_eq!(pretty(actual), pretty(expected), "{what}");
}

/// 테스트마다 따로 쓰는 임시 폴더(끝나면 지운다)
struct TempDir(PathBuf);

impl TempDir {
    fn new() -> TempDir {
        static N: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!("antler-config-test-{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("임시 폴더");
        TempDir(dir)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn from_hex(s: &str) -> Vec<u8> {
    (0..s.len() / 2).map(|i| u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).expect("16진수")).collect()
}

fn opt_str(v: &Json, k: &str) -> Option<String> {
    v.get(k).and_then(Json::as_str).map(str::to_string)
}

/// 골든의 연산 하나를 실행한다. 반환: (결과, 설정을 돌려주는 연산인가)
fn apply(store: &ConfigStore, op: &Json) -> (Result<Json, String>, bool) {
    let name = string(op, "op");
    let void = |r: Result<(), String>| r.map(|()| Json::Null);
    match name {
        "setUi" => (store.set_ui(key(op, "patch")), true),
        "setEditorExePath" => (store.set_editor_exe_path(string(op, "path")), true),
        "setSkippedUpdateVersion" => (store.set_skipped_update_version(string(op, "version")), true),
        "addRepo" => (store.add_repo(key(op, "input"), op.get("source")), false),
        "updateRepo" => (store.update_repo(string(op, "id"), key(op, "patch")), true),
        "removeRepo" => (store.remove_repo(string(op, "id")), true),
        "setGithubDefaultBranch" => (void(store.set_github_default_branch(string(op, "id"), string(op, "branch"))), false),
        "setTrustedConfig" => (void(store.set_trusted_config(string(op, "id"), string(op, "fingerprint"))), false),
        "setFetchApproval" => (void(store.set_fetch_approval(string(op, "id"), op.get("fingerprint").and_then(Json::as_str))), false),
        other => panic!("모르는 연산 {other}"),
    }
}

fn mirror_json(config: &Json) -> Json {
    let s = |v: &str| Json::Str(v.to_string());
    Json::Arr(
        mirror_of(config)
            .into_iter()
            .map(|e| {
                let mut o = vec![("id".to_string(), s(&e.repo.id)), ("kind".to_string(), s(&e.repo.kind)), ("path".to_string(), s(&e.repo.path))];
                if let Some(n) = &e.name {
                    o.push(("name".to_string(), s(n)));
                }
                if let Some(d) = &e.repo.distro {
                    o.push(("distro".to_string(), s(d)));
                }
                if let Some(h) = &e.repo.ssh_host {
                    o.push(("sshHost".to_string(), s(h)));
                }
                Json::Obj(o)
            })
            .collect(),
    )
}

fn fetch_prefs_json(config: &Json) -> Json {
    let p = fetch_prefs_of(config);
    let map = |v: Vec<(String, String)>| Json::Obj(v.into_iter().map(|(k, x)| (k, Json::Str(x))).collect());
    Json::Obj(vec![
        ("autoFetchMinutes".to_string(), Json::Num(p.auto_fetch_minutes)),
        ("approvals".to_string(), map(p.approvals)),
        ("trusted".to_string(), map(p.trusted)),
    ])
}

fn open(file: &Path, cache_dir: Option<&str>) -> (ConfigStore, Arc<AtomicUsize>) {
    let n = Arc::new(AtomicU64::new(0));
    let store = ConfigStore::open_for_test(file, cache_dir.map(str::to_string), move || format!("id-{}", n.fetch_add(1, Ordering::SeqCst) + 1), || FIXED_NOW);
    let notified = Arc::new(AtomicUsize::new(0));
    let counter = notified.clone();
    store.set_on_change(Box::new(move |_, _| {
        counter.fetch_add(1, Ordering::SeqCst);
    }));
    (store, notified)
}

#[test]
fn store_골든이_ts_와_같다() {
    let g = golden("store.json");
    assert_eq!(key(&g, "now").as_f64(), Some(FIXED_NOW as f64), "골든의 고정 시각이 시험과 다르다");
    let cache = string(&g, "cacheDir");
    let cases = arr(key(&g, "cases"), "cases");
    assert!(cases.len() > 200, "사례가 너무 적다: {}", cases.len());
    for c in cases {
        let name = string(c, "name");
        let input = key(c, "input");
        let tmp = TempDir::new();
        let file = tmp.0.join("antler.config.json");
        if key(input, "fileIsDir") == &Json::Bool(true) {
            fs::create_dir(&file).expect("폴더");
        } else if let Some(hex) = opt_str(input, "fileHex") {
            fs::write(&file, from_hex(&hex)).expect("파일");
        } else if let Some(text) = opt_str(input, "file") {
            fs::write(&file, text).expect("파일");
        }
        let cache_dir = (key(input, "cacheDir") == &Json::Bool(true)).then_some(cache);
        let (store, notified) = open(&file, cache_dir);
        assert_eq!(Json::Bool(store.load_failed()), *key(c, "loadFailed"), "{name}: loadFailed");
        assert_same(&store.get(), key(c, "initial"), &format!("{name}: 로드한 설정"));
        let mut expected_config = key(c, "initial").clone();
        let ops: &[Json] = match key(input, "ops") {
            Json::Arr(a) => a,
            _ => panic!("ops"),
        };
        let steps: &[Json] = match key(c, "steps") {
            Json::Arr(a) => a,
            _ => panic!("steps"),
        };
        assert_eq!(ops.len(), steps.len(), "{name}: 연산 수와 단계 수");
        for (i, (op, step)) in ops.iter().zip(steps).enumerate() {
            let what = format!("{name} / 단계 {i} ({})", string(op, "op"));
            let (result, returns_config) = apply(&store, op);
            if let Some(c) = step.get("config") {
                expected_config = c.clone();
            }
            match (&result, step.get("error")) {
                (Err(e), Some(Json::Str(want))) => assert_eq!(e, want, "{what}: 오류 문구"),
                (Ok(out), None) => {
                    if returns_config {
                        assert_eq!(key(step, "result").as_str(), Some("=config"), "{what}");
                        assert_same(out, &expected_config, &format!("{what}: 반환한 설정"));
                    } else {
                        assert_same(out, key(step, "result"), &format!("{what}: 결과"));
                    }
                }
                (r, e) => panic!("{what}: 결과가 다르다 — Rust {r:?} / TS {e:?}"),
            }
            assert_same(&store.get(), &expected_config, &format!("{what}: 설정"));
        }
        assert_eq!(notified.load(Ordering::SeqCst) as f64, key(c, "notified").as_f64().expect("notified"), "{name}: 알림 횟수");
        // 최종 파일 바이트
        let text = fs::read_to_string(&file).ok().filter(|_| file.is_file());
        match (key(c, "file"), text) {
            (Json::Null, None) => {}
            (Json::Str(want), Some(got)) => assert_eq!(&got, want, "{name}: 최종 파일 바이트"),
            (want, got) => panic!("{name}: 파일 유무가 다르다 — TS {want:?} / Rust {got:?}"),
        }
        let mut files: Vec<String> = fs::read_dir(&tmp.0).expect("폴더").map(|e| e.expect("항목").file_name().to_string_lossy().into_owned()).collect();
        files.sort();
        let want_files: Vec<String> = match key(c, "files") {
            Json::Arr(a) => a.iter().map(|f| f.as_str().expect("파일 이름").to_string()).collect(),
            _ => panic!("files"),
        };
        assert_eq!(files, want_files, "{name}: 남은 파일(손상 백업·임시 파일)");
        let fin = store.get();
        assert_same(&mirror_json(&fin), key(c, "mirror"), &format!("{name}: 미러"));
        assert_same(&fetch_prefs_json(&fin), key(c, "fetchPrefs"), &format!("{name}: fetch 설정"));
    }
}

#[test]
fn link_골든이_ts_와_같다() {
    let g = golden("link.json");
    for c in arr(key(&g, "validate"), "validate") {
        let input = key(c, "input");
        let want = key(c, "result");
        match (validate_github_link(Some(input)), want.get("ok"), want.get("error")) {
            (Ok(link), Some(ok), None) => assert_same(&link.to_json(), ok, &format!("validate {}", pretty(input))),
            (Err(e), None, Some(Json::Str(msg))) => assert_eq!(&e, msg, "validate {}", pretty(input)),
            (r, ..) => panic!("validate {}: {r:?} / TS {}", pretty(input), pretty(want)),
        }
    }
    for c in arr(key(&g, "sanitize"), "sanitize") {
        let input = key(c, "input");
        let got = sanitize_github_link(Some(input)).map_or(Json::Null, |l| l.to_json());
        assert_same(&got, key(c, "result"), &format!("sanitize {}", pretty(input)));
    }
    for c in arr(key(&g, "branches"), "branches") {
        let input = key(c, "input");
        let safe = matches!(input, Json::Str(s) if is_safe_branch(s));
        assert_eq!(Json::Bool(safe), *key(c, "safe"), "isSafeBranch {}", pretty(input));
    }
    for c in arr(key(&g, "slugs"), "slugs") {
        let (base, path) = (string(c, "base"), string(c, "path"));
        let slug = github_cache_slug(base, path).map_or(Json::Null, |(o, r)| Json::Obj(vec![("owner".into(), Json::Str(o)), ("repo".into(), Json::Str(r))]));
        assert_same(&slug, key(c, "slug"), &format!("slug {base:?} {path:?}"));
        assert_eq!(Json::Bool(is_github_cache_path(base, path)), *key(c, "isCachePath"), "isGithubCachePath {base:?} {path:?}");
    }
    for c in arr(key(&g, "matches"), "matches") {
        let (path, link) = (string(c, "path"), key(c, "link"));
        let got = cache_path_matches_link("C:\\golden\\cache", path, string(link, "owner"), string(link, "repo"));
        assert_eq!(Json::Bool(got), *key(c, "matches"), "cachePathMatchesLink {path:?}");
    }
    for c in arr(key(&g, "paths"), "paths") {
        let (base, link) = (string(c, "base"), key(c, "link"));
        assert_eq!(github_cache_path(base, string(link, "owner"), string(link, "repo")), string(c, "path"), "githubCachePath {base:?}");
    }
    for c in arr(key(&g, "urls"), "urls") {
        let link = key(c, "link");
        let (owner, repo) = (string(link, "owner"), string(link, "repo"));
        let origin = opt_str(c, "origin").unwrap_or_else(|| GITHUB_ORIGIN.to_string());
        assert_eq!(github_remote_url(owner, repo, &origin), string(c, "url"));
        let full = github_full_name(&validate_github_link(Some(link)).expect("링크"));
        assert_eq!(full, string(c, "fullName"));
    }
}

// ── 알림·쓰기 실패·동시성(골든에 없는 Rust 고유 동작) ─────────────────────────────────────────

#[test]
fn on_change_기본은_아무_일도_하지_않는다() {
    let tmp = TempDir::new();
    let file = tmp.0.join("antler.config.json");
    // 리스너를 달지 않은 스토어도 쓰기가 그대로 된다
    let store = ConfigStore::open_for_test(&file, None, || "id".to_string(), || FIXED_NOW);
    store.set_ui(&parse(r#"{"diffMode":"split"}"#).unwrap()).expect("쓰기");
    let saved = parse(&fs::read_to_string(&file).expect("파일")).expect("JSON");
    assert_eq!(saved.get("ui").and_then(|u| u.get("diffMode")).and_then(Json::as_str), Some("split"));
    assert_eq!(store.get().get("ui").and_then(|u| u.get("diffMode")).and_then(Json::as_str), Some("split"));
}

#[test]
fn on_change_는_쓰기가_끝난_뒤_새_설정으로_불린다() {
    let tmp = TempDir::new();
    let file = tmp.0.join("antler.config.json");
    let store = Arc::new(ConfigStore::open_for_test(&file, None, || "r1".to_string(), || FIXED_NOW));
    let seen: Arc<std::sync::Mutex<Vec<(u64, usize, usize)>>> = Arc::default();
    let (s2, f2, seen2) = (store.clone(), file.clone(), seen.clone());
    store.set_on_change(Box::new(move |gen, cfg| {
        // 불린 시점에 디스크와 메모리가 이미 새 상태다(잠금을 놓은 뒤라 같은 스토어를 읽어도 막히지 않는다)
        let on_disk = parse(&fs::read_to_string(&f2).expect("파일")).expect("JSON");
        let n = |v: &Json| match v.get("repos") {
            Some(Json::Arr(a)) => a.len(),
            _ => usize::MAX,
        };
        assert_eq!(n(&on_disk), n(cfg));
        seen2.lock().unwrap().push((gen, n(cfg), n(&s2.get())));
    }));
    let repo = store.add_repo(&parse(r#"{"sourceId":"local","path":"/tmp/x","name":"x"}"#).unwrap(), None).expect("추가");
    store.set_ui(&parse(r#"{"diffMode":"split"}"#).unwrap()).expect("ui");
    store.remove_repo(repo.get("id").and_then(Json::as_str).expect("id")).expect("삭제");
    // 같은 소스·경로를 다시 추가하는 것처럼 쓰지 않는 변경은 알리지 않는다
    store.set_trusted_config("zz", "fp").expect("쓰기");
    store.set_trusted_config("zz", "fp").expect("같은 값");
    assert_eq!(*seen.lock().unwrap(), vec![(1, 1, 1), (2, 1, 1), (3, 0, 0), (4, 0, 0)]);
}

#[test]
fn on_change_가_패닉해도_저장된_변경은_실패하지_않는다() {
    let tmp = TempDir::new();
    let store = ConfigStore::open_for_test(tmp.0.join("antler.config.json"), None, || "id".to_string(), || FIXED_NOW);
    store.set_on_change(Box::new(|_, _| panic!("boom")));
    let out = store.set_ui(&parse(r#"{"diffMode":"split"}"#).unwrap()).expect("알림 실패는 변경 실패가 아니다");
    assert_eq!(out.get("ui").and_then(|u| u.get("diffMode")).and_then(Json::as_str), Some("split"));
    assert_eq!(store.get().get("ui").and_then(|u| u.get("diffMode")).and_then(Json::as_str), Some("split"));
}

#[test]
fn 쓰기가_실패하면_메모리를_바꾸지_않고_알리지_않는다() {
    let tmp = TempDir::new();
    // 설정 파일의 상위가 일반 파일이라 폴더를 만들 수 없다
    let blocker = tmp.0.join("blocker");
    fs::write(&blocker, "x").expect("파일");
    let store = ConfigStore::open_for_test(blocker.join("antler.config.json"), None, || "id".to_string(), || FIXED_NOW);
    let notified = Arc::new(AtomicUsize::new(0));
    let n = notified.clone();
    store.set_on_change(Box::new(move |_, _| {
        n.fetch_add(1, Ordering::SeqCst);
    }));
    let before = pretty(&store.get());
    assert!(store.set_ui(&parse(r#"{"diffMode":"split"}"#).unwrap()).is_err());
    assert!(store.add_repo(&parse(r#"{"sourceId":"local","path":"x","name":"x"}"#).unwrap(), None).is_err());
    assert_eq!(pretty(&store.get()), before, "디스크에 쓰지 못했으면 메모리도 그대로다");
    assert_eq!(notified.load(Ordering::SeqCst), 0);
}

#[test]
fn 동시에_변경해도_하나도_잃지_않고_파일은_늘_온전하다() {
    let tmp = TempDir::new();
    let file = tmp.0.join("antler.config.json");
    let ids = Arc::new(AtomicU64::new(0));
    let i2 = ids.clone();
    let store = Arc::new(ConfigStore::open_for_test(&file, None, move || format!("id-{}", i2.fetch_add(1, Ordering::SeqCst)), || FIXED_NOW));
    let gens = Arc::new(std::sync::Mutex::new(Vec::new()));
    let g2 = gens.clone();
    store.set_on_change(Box::new(move |gen, _| g2.lock().unwrap().push(gen)));
    let handles: Vec<_> = (0..8)
        .map(|t| {
            let store = store.clone();
            std::thread::spawn(move || {
                for i in 0..5 {
                    store.add_repo(&parse(&format!(r#"{{"sourceId":"local","path":"C:\\{t}-{i}","name":"n"}}"#)).unwrap(), None).expect("추가");
                }
            })
        })
        .collect();
    for h in handles {
        h.join().expect("스레드");
    }
    let n = |v: &Json| match v.get("repos") {
        Some(Json::Arr(a)) => a.len(),
        _ => 0,
    };
    assert_eq!(n(&store.get()), 40);
    assert_eq!(n(&parse(&fs::read_to_string(&file).expect("파일")).expect("JSON")), 40, "마지막 쓰기가 모든 변경을 담는다");
    let mut g = gens.lock().unwrap().clone();
    g.sort_unstable();
    assert_eq!(g, (1..=40).collect::<Vec<u64>>(), "세대 번호는 변경마다 하나씩 겹치지 않게 올라간다");
    assert!(!tmp.0.join("antler.config.json.tmp").exists());
}

#[test]
fn 운영_생성자는_uuid_v4_id_를_만든다() {
    let tmp = TempDir::new();
    let store = ConfigStore::open(tmp.0.join("antler.config.json"), None);
    let a = store.add_repo(&parse(r#"{"sourceId":"local","path":"a","name":"a"}"#).unwrap(), None).expect("추가");
    let b = store.add_repo(&parse(r#"{"sourceId":"local","path":"b","name":"b"}"#).unwrap(), None).expect("추가");
    let (ia, ib) = (a.get("id").and_then(Json::as_str).expect("id"), b.get("id").and_then(Json::as_str).expect("id"));
    assert_ne!(ia, ib);
    for id in [ia, ib] {
        // 8-4-4-4-12 소문자 16진수, 버전 4·변형 8~b
        let parts: Vec<&str> = id.split('-').collect();
        assert_eq!(parts.iter().map(|p| p.len()).collect::<Vec<_>>(), [8, 4, 4, 4, 12], "{id}");
        assert!(id.bytes().all(|b| b == b'-' || b.is_ascii_digit() || (b'a'..=b'f').contains(&b)), "{id}");
        assert!(parts[2].starts_with('4') && matches!(parts[3].as_bytes()[0], b'8' | b'9' | b'a' | b'b'), "{id}");
    }
}

#[test]
fn 쓴_파일을_다시_열면_같은_설정이다() {
    let tmp = TempDir::new();
    let file = tmp.0.join("antler.config.json");
    let (store, _) = open(&file, Some("C:\\golden\\cache"));
    store.set_ui(&parse(r#"{"theme":"light","groupColors":{"b":1,"2024":2}}"#).unwrap()).expect("ui");
    store.add_repo(&parse(r#"{"sourceId":"local","path":"C:\\a","name":"a","extra":[1,{"k":null}]}"#).unwrap(), None).expect("추가");
    let bytes = fs::read(&file).expect("파일");
    let (again, _) = open(&file, Some("C:\\golden\\cache"));
    assert_same(&again.get(), &store.get(), "재로드");
    // 다시 쓴 파일도 같은 바이트다(왕복이 안정적이다)
    again.set_ui(&parse("{}").unwrap()).expect("쓰기");
    assert_eq!(fs::read(&file).expect("파일"), bytes);
}
