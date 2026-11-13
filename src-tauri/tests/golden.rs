//! 골든 패리티 테스트. `tests/golden/parsers/manifest.json` 의 항목 중 Rust 로 옮긴 파서의 것만 돌려,
//! 출력 JSON 이 TS 가 만든 골든과 같은지 본다(골든은 `UPDATE_GOLDEN=1 npx vitest run tests/golden.test.ts` 로 갱신).
//! 아직 옮기지 않은 파서의 항목은 건너뛰고 몇 개를 건너뛰었는지 출력한다(`--nocapture` 로 보인다).

use std::fs;
use std::path::{Path, PathBuf};

use antler_lib::git::parsers::blame::parse_blame;
use antler_lib::git::parsers::changes::{parse_changes, parse_name_status, parse_numstat};
use antler_lib::git::parsers::commit::parse_commit_meta;
use antler_lib::git::parsers::diff::{DiffParseOpts, DiffParser};
use antler_lib::git::parsers::file_history::parse_file_history;
use antler_lib::git::parsers::log::parse_log;
use antler_lib::git::parsers::nul_records::split_nul;
use antler_lib::git::parsers::refs::{parse_refs, parse_remote_names};
use antler_lib::git::parsers::stash::parse_stashes;
use antler_lib::git::parsers::submodule::{parse_gitlinks, parse_gitmodules, parse_probe};
use antler_lib::git::parsers::wip_status::WipStatusParser;
use antler_lib::git::parsers::worktree::parse_worktrees;
use serde_json::{json, Map, Value};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().expect("src-tauri 의 상위").to_path_buf()
}

/// manifest args 의 문자열 값을 정수로(없으면 None)
fn num_arg(args: &Value, key: &str) -> Option<usize> {
    args.get(key).and_then(Value::as_str).map(|s| s.parse().unwrap_or_else(|_| panic!("args.{key} 가 정수가 아니다: {s}")))
}

/// 입력을 `chunk` 바이트씩 나눠 준다(없으면 통째로 한 번)
fn feed(input: &[u8], chunk: Option<usize>, mut push: impl FnMut(&[u8])) {
    match chunk.filter(|&c| c > 0) {
        Some(size) => input.chunks(size).for_each(&mut push),
        None => push(input),
    }
}

/// TS `summarizeDiff`: hunk 의 줄 목록 대신 줄 수와 처음·마지막 줄만 둔다
fn summarize_diff(mut d: Value) -> Value {
    for h in d["hunks"].as_array_mut().expect("hunks") {
        let lines = h.as_object_mut().expect("hunk").remove("lines").expect("lines");
        let lines = lines.as_array().expect("lines 배열");
        let obj: &mut Map<String, Value> = h.as_object_mut().expect("hunk");
        obj.insert("lineCount".into(), json!(lines.len()));
        if let Some(first) = lines.first() {
            obj.insert("first".into(), first.clone());
        }
        if let Some(last) = lines.last() {
            obj.insert("last".into(), last.clone());
        }
    }
    d
}

/// 파서 이름 → 실행. 구현하지 않은 이름은 None
fn run_parser(parser: &str, input: &[u8], args: &Value) -> Option<Value> {
    let to_value = |r: Result<Value, serde_json::Error>| r.expect("직렬화");
    let str_arg = |k: &str| args.get(k).and_then(Value::as_str);
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
        "diff" => {
            let opts = DiffParseOpts {
                max_lines: num_arg(args, "maxLines"),
                max_bytes: num_arg(args, "maxBytes"),
                count_limit: num_arg(args, "countLimit"),
                count_bytes: num_arg(args, "countBytes"),
            };
            let mut p = DiffParser::new(str_arg("path").expect("args.path"), str_arg("oldPath"), &opts);
            feed(input, num_arg(args, "chunk"), |c| {
                p.push(c);
            });
            let d = to_value(serde_json::to_value(p.finish()));
            if str_arg("summary").is_some() {
                summarize_diff(d)
            } else {
                d
            }
        }
        "wipStatus" => {
            let mut p = WipStatusParser::new(num_arg(args, "untrackedLimit").unwrap_or(5000));
            feed(input, num_arg(args, "chunk"), |c| p.push(c));
            to_value(serde_json::to_value(p.finish()))
        }
        "nameStatus" => to_value(serde_json::to_value(parse_name_status(input))),
        "numstat" => to_value(serde_json::to_value(parse_numstat(input))),
        "changes" => {
            let numstat = fs::read(repo_root().join("tests/fixtures").join(str_arg("numstat").expect("args.numstat"))).expect("numstat fixture");
            to_value(serde_json::to_value(parse_changes(input, &numstat)))
        }
        // TS 는 필드가 모자라면 던지고 골든에는 {error: 메시지} 로 남는다
        "commit" => match parse_commit_meta(input) {
            Ok(c) => to_value(serde_json::to_value(c)),
            Err(error) => json!({ "error": error }),
        },
        "blame" => to_value(serde_json::to_value(parse_blame(input))),
        "fileHistory" => to_value(serde_json::to_value(parse_file_history(input, str_arg("fallback").unwrap_or("")))),
        "gitlinks" => to_value(serde_json::to_value(parse_gitlinks(input))),
        "gitmodules" => to_value(serde_json::to_value(parse_gitmodules(input))),
        "probe" => to_value(serde_json::to_value(parse_probe(&String::from_utf8_lossy(input)))),
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
