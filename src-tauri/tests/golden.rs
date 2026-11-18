//! 골든 패리티 테스트. `tests/golden/parsers/manifest.json` 의 항목 중 Rust 로 옮긴 파서의 것만 돌려,
//! 출력 JSON 이 TS 가 만든 골든과 같은지 본다(골든은 `UPDATE_GOLDEN=1 npx vitest run tests/golden.test.ts` 로 갱신).
//! 아직 옮기지 않은 파서의 항목은 건너뛰고 몇 개를 건너뛰었는지 출력한다(`--nocapture` 로 보인다).

use std::fs;
use std::path::{Path, PathBuf};

use antler_lib::git::activity::{bucketize, TimeStreamParser};
use antler_lib::git::code_browse::{image_mime, is_resolvable_ref, normalize_dir, normalize_file_path, parse_tree, CommitId};
use antler_lib::git::exec::{parse_lxss_query, parse_wsl_list};
use antler_lib::repos::env::{remote_relative, resolve_env, EnvInput};
use antler_lib::repos::scan::{scan_from_listing, RemoteDirListing};
use antler_lib::git::lfs::{added_lines, gitattributes_uses_lfs, parse_check_attr_lfs, parse_gitflow_config, parse_lfs_pointer};
use antler_lib::git::parsers::blame::parse_blame;
use antler_lib::git::parsers::changes::{parse_changes, parse_name_status, parse_numstat};
use antler_lib::git::parsers::commit::parse_commit_meta;
use antler_lib::git::parsers::diff::{DiffParseOpts, DiffParser};
use antler_lib::git::parsers::file_history::parse_file_history;
use antler_lib::git::parsers::log::parse_log;
use antler_lib::git::parsers::nul_records::split_nul;
use antler_lib::git::parsers::refs::{parse_refs, parse_remote_names, Branch, RemoteGroup};
use antler_lib::git::parsers::stash::parse_stashes;
use antler_lib::git::parsers::submodule::{parse_gitlinks, parse_gitmodules, parse_probe};
use antler_lib::git::parsers::wip_status::WipStatusParser;
use antler_lib::git::parsers::worktree::parse_worktrees;
use antler_lib::git::refs_service::{is_safe_submodule_path, pick_default_branch};
use antler_lib::git::remotes::{parse_pushed_shas, parse_remote_config};
use antler_lib::git::wip::{assert_rel_path, filter_driver_names, has_short_name_segment, FilterGuard};
use antler_lib::github::ports::GhRepoRef;
use antler_lib::github::remote_url::{parse_github_remote, pick_github_remote, pick_github_remotes, Remote};
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

/// JSON 입력 목록(`[케이스, …]`)의 각 케이스를 `{in, out}`(오류면 `{in, error}`)로 바꾼다. TS `cases` 와 같은 모양이다
fn cases(input: &[u8], f: impl Fn(&Value) -> Result<Value, String>) -> Value {
    let list: Vec<Value> = serde_json::from_slice(input).expect("케이스 목록");
    Value::Array(
        list.into_iter()
            .map(|c| match f(&c) {
                Ok(out) => json!({ "in": c, "out": out }),
                Err(error) => json!({ "in": c, "error": error }),
            })
            .collect(),
    )
}

/// 케이스가 문자열이 아니면 TS 가 던지는 오류(`JSON.parse` 로 들어온 5·null 은 검증 함수가 거부한다)
fn as_text<'a>(c: &'a Value, error: &str) -> Result<&'a str, String> {
    c.as_str().ok_or_else(|| error.to_string())
}

/// filterOverrides 케이스를 바이트로 푼다: 문자열은 UTF-8, `{parts}` 는 조각(문자열 | `{hex}` | `{repeat: [단위, 횟수]}` — 단위의 `{i}` 는 0 부터의 번호)을 이어 붙인다
fn case_bytes(c: &Value) -> Result<Vec<u8>, String> {
    if let Some(s) = c.as_str() {
        return Ok(s.as_bytes().to_vec());
    }
    // `{utf16, bom?}`: UTF-16LE(bom 이면 FF FE 가 앞에 붙는다)
    if let Some(text) = c["utf16"].as_str() {
        let mut out = if c["bom"].as_bool() == Some(true) { vec![0xff, 0xfe] } else { Vec::new() };
        out.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
        return Ok(out);
    }
    let mut out = Vec::new();
    for p in c["parts"].as_array().ok_or("parts")? {
        if let Some(s) = p.as_str() {
            out.extend_from_slice(s.as_bytes());
        } else if let Some(h) = p["hex"].as_str() {
            for i in (0..h.len()).step_by(2) {
                out.push(u8::from_str_radix(&h[i..i + 2], 16).map_err(|e| e.to_string())?);
            }
        } else {
            let unit = p["repeat"][0].as_str().ok_or("repeat 단위")?;
            for i in 0..p["repeat"][1].as_u64().ok_or("repeat 횟수")? {
                out.extend_from_slice(unit.replace("{i}", &i.to_string()).as_bytes());
            }
        }
    }
    Ok(out)
}

fn sv<T: serde::Serialize>(v: T) -> Result<Value, String> {
    serde_json::to_value(v).map_err(|e| e.to_string())
}

/// 케이스 JSON 의 브랜치(`{name, fullName, hash}`)
fn branch_of(v: &Value) -> Branch {
    let s = |k: &str| v[k].as_str().expect("브랜치 필드").to_string();
    Branch { name: s("name"), full_name: s("fullName"), hash: s("hash"), upstream: None, ahead: None, behind: None, upstream_gone: None, is_head: false }
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
        // ---- Phase 6a PR 1 ----
        "tree" => to_value(serde_json::to_value(parse_tree(input, str_arg("dir").unwrap_or("")))),
        "commitId" => cases(input, |c| CommitId::parse(as_text(c, "invalid commit hash")?).map(|_| json!(true)).map_err(str::to_string)),
        "normalizeDir" => {
            let file = str_arg("file").is_some();
            cases(input, |c| {
                let s = as_text(c, "invalid path")?;
                let r = if file { normalize_file_path(s) } else { normalize_dir(s) };
                r.map(Value::from).map_err(str::to_string)
            })
        }
        "resolveRef" => cases(input, |c| Ok(json!(is_resolvable_ref(as_text(c, "invalid ref")?)))),
        "imageMime" => cases(input, |c| Ok(json!(image_mime(as_text(c, "invalid path")?)))),
        "lfsPointer" => cases(input, |c| sv(parse_lfs_pointer(as_text(c, "text")?))),
        "checkAttrLfs" => cases(input, |c| sv(parse_check_attr_lfs(as_text(c, "text")?))),
        "addedLines" => cases(input, |c| Ok(json!(added_lines(as_text(c, "text")?)))),
        "gitattributes" => cases(input, |c| Ok(json!(gitattributes_uses_lfs(as_text(c, "text")?)))),
        "gitflowConfig" => cases(input, |c| sv(parse_gitflow_config(as_text(c, "text")?))),
        "timeStream" => {
            let mut p = TimeStreamParser::new();
            feed(input, num_arg(args, "chunk"), |c| p.push(c));
            p.end();
            json!({ "n": p.values().len(), "values": p.values() })
        }
        "bucketize" => cases(input, |c| {
            let times: Vec<f64> = c["times"].as_array().expect("times").iter().map(|t| t.as_f64().expect("숫자")).collect();
            sv(bucketize(&times, c["now"].as_f64().expect("now")))
        }),
        "defaultBranch" => cases(input, |c| {
            let list = |v: &Value| v.as_array().expect("배열").iter().map(branch_of).collect::<Vec<_>>();
            let remotes: Vec<RemoteGroup> =
                c["remotes"].as_array().expect("remotes").iter().map(|g| RemoteGroup { name: g["name"].as_str().expect("name").to_string(), branches: list(&g["branches"]) }).collect();
            sv(pick_default_branch(c["originHead"].as_str(), &list(&c["local"]), &remotes))
        }),
        "filterDrivers" => cases(input, |c| match filter_driver_names(&case_bytes(c)?) {
            Ok(names) => sv(names),
            Err(e) => Err(e.message().to_string()),
        }),
        "filterOverrides" =>cases(input, |c| match FilterGuard::from_config_output(&case_bytes(c)?) {
            Ok(g) => sv(g.args()),
            Err(e) => Err(e.message().to_string()),
        }),
        "relPath" => cases(input, |c| assert_rel_path(as_text(c, "invalid path")?).map(|()| json!(true)).map_err(str::to_string)),
        "shortName" => cases(input, |c| Ok(json!(has_short_name_segment(as_text(c, "text")?)))),
        "submodulePath" => cases(input, |c| Ok(json!(is_safe_submodule_path(as_text(c, "text")?)))),
        // ---- Phase 6a PR 8 ----
        "githubRemote" => cases(input, |c| Ok(parse_github_remote(as_text(c, "text")?).map_or(Value::Null, |(owner, repo)| json!({ "owner": owner, "repo": repo })))),
        "pickRemote" => cases(input, |c| {
            let list: Vec<Remote> = serde_json::from_value(c.clone()).map_err(|e| e.to_string())?;
            let r = |x: &GhRepoRef| json!({ "owner": x.owner, "repo": x.repo, "remote": x.remote });
            Ok(json!({ "one": pick_github_remote(&list).as_ref().map_or(Value::Null, r), "all": pick_github_remotes(&list).iter().map(r).collect::<Vec<_>>() }))
        }),
        "remoteConfig" => cases(input, |c| sv(parse_remote_config(as_text(c, "text")?))),
        "pushedShas" => cases(input, |c| sv(parse_pushed_shas(as_text(c, "text")?))),
        // ---- Phase 6b PR 4a ----
        "wslList" => cases(input, |c| sv(parse_wsl_list(&case_bytes(c)?))),
        "lxssQuery" => cases(input, |c| sv(parse_lxss_query(as_text(c, "text")?))),
        "scanFromListing" => cases(input, |c| {
            let listing: RemoteDirListing = serde_json::from_value(c.clone()).map_err(|e| e.to_string())?;
            sv(scan_from_listing(&listing))
        }),
        "resolveEnv" => cases(input, |c| {
            let s = |k: &str| c[k].as_str();
            let r = resolve_env(&EnvInput { kind: s("kind").ok_or("kind")?, path: s("path").ok_or("path")?, distro: s("distro"), ssh_host: s("sshHost") })?;
            // TS 는 target 이 없으면 키를 두지 않는다(JSON 에서 undefined 는 빠진다)
            let mut out = json!({ "kind": r.kind, "path": r.path });
            if let Some(t) = r.target {
                out["target"] = json!(t);
            }
            Ok(out)
        }),
        "remoteRelative" => cases(input, |c| Ok(json!(remote_relative(c["kind"].as_str().ok_or("kind")?, c["path"].as_str().ok_or("path")?)))),
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

// ── Phase 6a 이식 대응표 ──────────────────────────────────────────────────────────────────────────

/// `tests/golden/channels/test-map.json`(TS 테스트 → Rust 테스트 대응표)의 값이 거짓말이 아닌지 본다. TS 쪽(`tests/golden6a.test.ts`)이 "대상 it 이 빠짐없이 있는가"를,
/// 이 테스트가 "가리키는 Rust 테스트가 있는가·값의 모양이 맞는가"를 본다(양쪽이 같은 규칙을 쓴다).
#[test]
fn phase6a_대응표가_가리키는_rust_테스트가_있다() {
    let root = repo_root();
    let map: Value = serde_json::from_slice(&fs::read(root.join("tests/golden/channels/test-map.json")).expect("test-map.json")).expect("대응표 형식");
    let source_of = |file: &str| -> PathBuf {
        let tests = root.join("src-tauri/tests");
        let git = root.join("src-tauri/src/git");
        match file {
            "golden.rs" | "exec.rs" | "channels.rs" => tests.join(file),
            "refs.rs" | "submodule.rs" => git.join("parsers").join(file),
            "common.rs" => git.join("exec").join(file),
            "activity.rs" | "code_browse.rs" | "lfs.rs" | "link_guard.rs" | "refs_service.rs" | "repo_state.rs" | "search.rs" | "wip.rs" => git.join(file),
            other => panic!("알 수 없는 Rust 파일 {other}"),
        }
    };
    let (mut ported, mut later, mut na) = (0, 0, 0);
    for (title, v) in map.as_object().expect("대응표는 객체") {
        let v = v.as_str().expect("값은 문자열");
        if let Some(why) = v.strip_prefix("N/A: ") {
            if let Some(pr) = why.strip_prefix("PR ") {
                let ok = matches!(pr.as_bytes(), [b'2'..=b'8'] | [b'2'..=b'8', b'a' | b'b']);
                assert!(ok, "{title}: 'N/A: PR 2'..'PR 8'(뒤에 a·b) 형식이어야 한다 → {v}");
                later += 1;
            } else {
                assert!(why.chars().count() > 8, "{title}: N/A 사유가 비어 있다");
                na += 1;
            }
            continue;
        }
        let (file, name) = v.split_once("::").unwrap_or_else(|| panic!("{title}: '<파일>.rs::<이름>' 이어야 한다 → {v}"));
        let path = source_of(file);
        let text = fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        assert!(text.contains(&format!("fn {name}(")), "{title}: {file} 에 fn {name} 이 없다");
        ported += 1;
    }
    println!("[6a 대응표] 이식 {ported}, 뒤 PR 몫 {later}, N/A {na}");
    assert_eq!(ported + later + na, map.as_object().unwrap().len());
}
