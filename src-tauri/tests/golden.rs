//! 골든 패리티 테스트. `tests/golden/parsers/manifest.json` 의 항목 중 Rust 로 옮긴 파서의 것만 돌려,
//! 출력 JSON 이 TS 가 만든 골든과 같은지 본다(골든은 `UPDATE_GOLDEN=1 npx vitest run tests/golden.test.ts` 로 갱신).
//! 아직 옮기지 않은 파서의 항목은 건너뛰고 몇 개를 건너뛰었는지 출력한다(`--nocapture` 로 보인다).

use std::fs;
use std::path::{Path, PathBuf};

use antler_lib::git::parsers::log::parse_log;
use antler_lib::git::parsers::nul_records::split_nul;
use antler_lib::git::parsers::refs::{parse_refs, parse_remote_names};
use antler_lib::git::parsers::stash::parse_stashes;
use antler_lib::git::parsers::worktree::parse_worktrees;
use serde_json::Value;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().expect("src-tauri 의 상위").to_path_buf()
}

/// 파서 이름 → 실행. 구현하지 않은 이름은 None
fn run_parser(parser: &str, input: &[u8], args: &Value) -> Option<Value> {
    let to_value = |r: Result<Value, serde_json::Error>| r.expect("직렬화");
    Some(match parser {
        "log" => to_value(serde_json::to_value(parse_log(input))),
        "refs" => {
            let names = match args.get("remotes").and_then(Value::as_str) {
                Some(f) => parse_remote_names(&fs::read(repo_root().join("tests/fixtures").join(f)).expect("remotes fixture")),
                None => Vec::new(),
            };
            to_value(serde_json::to_value(parse_refs(input, &names)))
        }
        "stash" => to_value(serde_json::to_value(parse_stashes(input))),
        "worktree" => to_value(serde_json::to_value(parse_worktrees(input))),
        "nulRecords" => to_value(serde_json::to_value(split_nul(input))),
        _ => return None,
    })
}

/// 두 JSON 의 차이를 경로와 함께 모은다(최대 `limit` 개)
fn diff(path: &str, want: &Value, got: &Value, out: &mut Vec<String>, limit: usize) {
    if out.len() >= limit || want == got {
        return;
    }
    match (want, got) {
        (Value::Object(w), Value::Object(g)) => {
            for k in w.keys().chain(g.keys().filter(|k| !w.contains_key(*k))) {
                let p = format!("{path}.{k}");
                match (w.get(k), g.get(k)) {
                    (Some(a), Some(b)) => diff(&p, a, b, out, limit),
                    (Some(a), None) => out.push(format!("{p}: 골든에만 있음 {a}")),
                    (None, Some(b)) => out.push(format!("{p}: Rust 에만 있음 {b}")),
                    (None, None) => {}
                }
            }
        }
        (Value::Array(w), Value::Array(g)) => {
            if w.len() != g.len() {
                out.push(format!("{path}: 길이 골든 {} / Rust {}", w.len(), g.len()));
            }
            for (i, (a, b)) in w.iter().zip(g).enumerate() {
                diff(&format!("{path}[{i}]"), a, b, out, limit);
            }
        }
        _ => out.push(format!("{path}: 골든 {want} / Rust {got}")),
    }
}

#[test]
fn rust_parsers_match_ts_golden() {
    let root = repo_root();
    let dir = root.join("tests/golden/parsers");
    let manifest: Vec<Value> = serde_json::from_slice(&fs::read(dir.join("manifest.json")).expect("manifest.json")).expect("manifest 형식");
    assert!(!manifest.is_empty(), "manifest 가 비었다");

    let (mut checked, mut skipped, mut failures) = (0, Vec::<String>::new(), Vec::<String>::new());
    for e in &manifest {
        let field = |k: &str| e[k].as_str().unwrap_or_else(|| panic!("manifest 항목에 {k} 가 없다: {e}")).to_string();
        let (parser, fixture, golden) = (field("parser"), field("fixture"), field("golden"));
        let input = fs::read(root.join("tests/fixtures").join(&fixture)).unwrap_or_else(|err| panic!("fixture {fixture}: {err}"));
        let Some(got) = run_parser(&parser, &input, e.get("args").unwrap_or(&Value::Null)) else {
            skipped.push(parser);
            continue;
        };
        let want: Value = serde_json::from_slice(&fs::read(dir.join(&golden)).unwrap_or_else(|err| panic!("골든 {golden}: {err}"))).expect("골든 형식");
        checked += 1;
        let mut d = Vec::new();
        diff("$", &want, &got, &mut d, 8);
        if !d.is_empty() {
            failures.push(format!("{parser} × {fixture} ({golden})\n    {}", d.join("\n    ")));
        }
    }

    skipped.sort();
    skipped.dedup_by(|a, b| a == b);
    println!("골든 비교 {checked}개, 건너뜀 {}개(미이식 파서: {skipped:?})", manifest.len() - checked);
    assert!(failures.is_empty(), "골든 불일치 {}건\n  {}", failures.len(), failures.join("\n  "));
}
