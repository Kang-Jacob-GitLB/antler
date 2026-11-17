//! SSH 순수 로직(`src/git/exec/ssh`)의 골든 패리티 테스트. `tests/golden/ssh/*.json`(TS 가 기준: `UPDATE_GOLDEN=1 npx vitest run tests/goldenSsh.test.ts`)과
//! `tests/golden/exec/env.json` 의 `ssh` 절(원격 명령 원문 바이트)을 읽어 Rust 이식이 같은 결과를 내는지 본다. **건너뛰는 골든이 없다** — 처리기 없는 파일이 있으면 실패한다.
//! 입력 표는 `tests/fixtures/sshCases.ts`. 네트워크·ssh·git 을 쓰지 않는다. 알려진 차이(비 ASCII 대소문자)는 골든의 `rust` 필드로 고정하고, 그 값이 TS 와 **실제로 다른지**도 단언한다.

use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use antler_lib::git::exec::ssh::config::{list_hosts, parse_host_spec, parse_ssh_config, resolve_host, Ctx, IdentityAgent, MemFs, ResolvedHost};
use antler_lib::git::exec::ssh::known_hosts::{
    check_host_key, fingerprint, key_type_of_blob, known_host_key_algorithms, known_hosts_name, parse_known_hosts, verify_host_key_text, Marker,
};
use antler_lib::git::exec::ssh::quote::{build_remote_command, sh_quote, QuoteError};
use antler_lib::git::exec::{build_git_args, FetchEnv};
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use serde_json::{json, Value};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().expect("src-tauri 의 상위").to_path_buf()
}

fn golden(name: &str) -> Value {
    let path = root().join("tests").join("golden").join("ssh").join(name);
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

const HANDLED: [&str; 3] = ["config.json", "knownHosts.json", "quote.json"];

#[test]
fn 골든_파일마다_처리기가_있다() {
    let mut found: Vec<String> = fs::read_dir(root().join("tests/golden/ssh"))
        .expect("tests/golden/ssh")
        .map(|e| e.expect("항목").file_name().to_string_lossy().into_owned())
        // test-map.json 은 TS 테스트 → Rust 테스트 이식 대응표(손글씨)라 골든이 아니다
        .filter(|n| n.ends_with(".json") && n != "test-map.json")
        .collect();
    found.sort();
    assert_eq!(found, HANDLED, "처리기가 없는 골든이 있거나 골든이 없다");
}

// ── 설정 파서 ─────────────────────────────────────────────────────────────────

fn resolved_json(r: &ResolvedHost) -> Value {
    let agent = match &r.identity_agent {
        IdentityAgent::Default => json!({ "kind": "default" }),
        IdentityAgent::Disabled => json!({ "kind": "none" }),
        IdentityAgent::Path(p) => json!({ "kind": "path", "path": p }),
    };
    json!({
        "alias": r.alias,
        "hostName": r.host_name,
        "user": r.user,
        "port": r.port,
        "identityFiles": r.identity_files,
        "proxyJump": r.proxy_jump.iter().map(|h| json!({ "spec": h.spec, "user": h.user, "host": h.host, "port": h.port })).collect::<Vec<_>>(),
        "identitiesOnly": r.identities_only,
        "identityAgent": agent,
        "unsupported": r.unsupported,
        "knownHostsFiles": r.known_hosts_files,
        "hostKeyAlias": r.host_key_alias,
    })
}

#[test]
fn config_골든이_ts_와_같다() {
    let g = golden("config.json");
    let home = s(&g["home"]);
    let local_user = s(&g["localUser"]);
    let (mut queries, mut diffs) = (0, 0);
    for case in arr(&g["cases"], "cases") {
        let name = s(&case["name"]);
        let files: Vec<(String, String)> = case["files"].as_object().unwrap().iter().map(|(k, v)| (k.clone(), s(v).to_string())).collect();
        let env: HashMap<String, String> = case["env"].as_object().unwrap().iter().map(|(k, v)| (k.clone(), s(v).to_string())).collect();
        let lookup = |n: &str| env.get(n).cloned();
        let ctx = Ctx { local_user, env: &lookup };
        let cfg = parse_ssh_config(s(&case["text"]), home, &MemFs::new(files));
        for q in arr(&case["queries"], "queries") {
            let spec = s(&q["spec"]);
            let got = resolved_json(&resolve_host(&cfg, spec, &ctx));
            let mut want = q["resolved"].clone();
            if let Some(over) = q.get("rust") {
                // 알려진 차이: 골든의 `rust` 가 Rust 의 기대값이고, 그 값은 TS 와 실제로 달라야 한다(안 달라지면 차이 문서가 낡았다)
                for (k, v) in over.as_object().unwrap() {
                    want[k] = v.clone();
                }
                assert_ne!(want, q["resolved"], "{name} / {spec}: rust 필드가 TS 결과와 같다 — 차이 표기를 지운다");
                diffs += 1;
            }
            assert_eq!(got, want, "{name} / {spec}");
            queries += 1;
        }
        let hosts: Vec<Value> = list_hosts(&cfg, &ctx)
            .iter()
            .map(|h| {
                let mut v = json!({ "alias": h.alias, "hostName": h.host_name, "user": h.user, "port": h.port });
                if let Some(pj) = &h.proxy_jump {
                    v["proxyJump"] = json!(pj);
                }
                v
            })
            .collect();
        assert_eq!(Value::Array(hosts), case["hosts"], "{name} / listHosts");
    }
    eprintln!("[config] 질의 {queries}(알려진 차이 {diffs})");
    assert!(queries > 80 && diffs == 2);
}

#[test]
fn config_호스트_스펙_골든이_ts_와_같다() {
    let g = golden("config.json");
    for c in arr(&g["specs"], "specs") {
        let spec = s(&c["spec"]);
        let h = parse_host_spec(spec);
        assert_eq!(json!({ "user": h.user, "host": h.host, "port": h.port }), c["out"], "{spec:?}");
    }
}

// ── known_hosts ───────────────────────────────────────────────────────────────

#[test]
fn known_hosts_골든이_ts_와_같다() {
    let g = golden("knownHosts.json");
    let (mut queries, mut diffs) = (0, 0);
    for case in arr(&g["cases"], "cases") {
        let name = s(&case["name"]);
        let text = s(&case["text"]);
        let entries: Vec<Value> = parse_known_hosts(text)
            .iter()
            .map(|e| {
                let marker = e.marker.map(|m| if m == Marker::Revoked { "revoked" } else { "cert-authority" });
                json!({ "marker": marker, "hosts": e.hosts, "keyType": e.key_type, "keyBase64": e.key_base64 })
            })
            .collect();
        assert_eq!(Value::Array(entries), case["entries"], "{name} / parse");
        for q in arr(&case["queries"], "queries") {
            let (host, port) = (s(&q["host"]), q["port"].as_u64().unwrap() as u32);
            let key = STANDARD.decode(s(&q["keyBase64"])).unwrap();
            let label = format!("{name} / {host}:{port} / {}", s(&q["key"]));
            assert_eq!(known_hosts_name(host, port), s(&q["name"]), "{label} / name");
            assert_eq!(key_type_of_blob(&key), s(&q["type"]), "{label} / type");
            assert_eq!(fingerprint(&key), s(&q["fingerprint"]), "{label} / fingerprint");
            let status = check_host_key(text, host, port, &key);
            let verify = verify_host_key_text(text, host, port, &key);
            if let Some(rust) = q.get("rustStatus") {
                // 알려진 차이(비 ASCII 대소문자): 판정은 Rust 기대값을, 검증 결과는 그 판정과 맞는 쪽을 본다
                assert_eq!(status.as_str(), s(rust), "{label} / rustStatus");
                assert_ne!(status.as_str(), s(&q["status"]), "{label}: rustStatus 가 TS 와 같다 — 차이 표기를 지운다");
                assert_eq!(verify.is_ok(), s(rust) == "ok", "{label} / verify");
                diffs += 1;
            } else {
                assert_eq!(status.as_str(), s(&q["status"]), "{label} / status");
                let algs: Vec<String> = q["algorithms"].as_array().unwrap().iter().map(|a| s(a).to_string()).collect();
                assert_eq!(known_host_key_algorithms(text, host, port), algs, "{label} / algorithms");
                let want = &q["verify"];
                match &verify {
                    Ok(()) => assert_eq!(want, &json!({ "ok": true }), "{label} / verify"),
                    Err(m) => assert_eq!(want, &json!({ "ok": false, "message": m }), "{label} / verify"),
                }
            }
            queries += 1;
        }
    }
    eprintln!("[known_hosts] 질의 {queries}(알려진 차이 {diffs})");
    assert!(queries > 40 && diffs == 4);
}

// ── 인용 ──────────────────────────────────────────────────────────────────────

/// POSIX 셸 인용 해제(작은따옴표·큰따옴표·백슬래시). 원격 셸이 하는 일을 흉내 내 argv 를 복원한다(`tests/sshHarness.ts` 의 `unquote` 와 같다)
fn unquote(cmd: &str) -> Vec<String> {
    let c: Vec<char> = cmd.chars().collect();
    let mut args = Vec::new();
    let mut cur = String::new();
    let mut has = false;
    let mut i = 0;
    while i < c.len() {
        match c[i] {
            '\'' => {
                let end = c[i + 1..].iter().position(|&x| x == '\'').map(|p| p + i + 1).expect("닫히지 않은 작은따옴표");
                cur.extend(&c[i + 1..end]);
                i = end;
                has = true;
            }
            '"' => {
                i += 1;
                while i < c.len() && c[i] != '"' {
                    if c[i] == '\\' && c.get(i + 1).is_some_and(|n| "\"\\$`".contains(*n)) {
                        i += 1;
                    }
                    cur.push(c[i]);
                    i += 1;
                }
                has = true;
            }
            '\\' => {
                i += 1;
                if let Some(&n) = c.get(i) {
                    cur.push(n);
                }
                has = true;
            }
            ' ' | '\t' | '\n' => {
                if has {
                    args.push(std::mem::take(&mut cur));
                }
                cur.clear();
                has = false;
            }
            ch => {
                cur.push(ch);
                has = true;
            }
        }
        i += 1;
    }
    if has {
        args.push(cur);
    }
    args
}

fn tricky() -> Vec<String> {
    arr(&golden("quote.json")["tricky"], "tricky").iter().map(|v| s(v).to_string()).collect()
}

#[test]
fn 인용_골든이_ts_와_같다() {
    let g = golden("quote.json");
    let mut errors = 0;
    for c in arr(&g["quote"], "quote") {
        let input = s(&c["input"]);
        match sh_quote(input) {
            Ok(q) => assert_eq!(q, s(&c["ok"]), "{input:?}"),
            Err(e) => {
                assert_eq!(e.to_string(), s(&c["error"]), "{input:?}");
                errors += 1;
            }
        }
    }
    assert_eq!(errors, 2, "NUL 입력 둘은 거부돼야 한다");
}

#[test]
fn 인용_왕복이_ts_와_같다() {
    for t in tricky() {
        assert_eq!(unquote(&sh_quote(&t).unwrap()), std::slice::from_ref(&t), "{t:?}");
    }
}

#[test]
fn 인용은_작은따옴표_안에_백슬래시를_두지_않는다() {
    for t in tricky() {
        // 왼쪽부터 스캔해 '…' 구간 안에는 백슬래시가 없고, 바깥의 백슬래시는 \\ 또는 \' 쌍이어야 한다
        let q: Vec<char> = sh_quote(&t).unwrap().chars().collect();
        let mut i = 0;
        while i < q.len() {
            if q[i] == '\'' {
                let end = q[i + 1..].iter().position(|&x| x == '\'').map(|p| p + i + 1).unwrap();
                assert!(!q[i + 1..end].contains(&'\\'), "{t:?}");
                i = end;
            } else {
                assert_eq!(q[i], '\\', "{t:?}");
                assert!(matches!(q.get(i + 1), Some('\\' | '\'')), "{t:?}");
                i += 1;
            }
            i += 1;
        }
    }
}

#[test]
fn 인용은_nul_을_거부한다() {
    assert_eq!(sh_quote("a\0b"), Err(QuoteError::Nul));
    for (repo, args) in [("/r", vec!["x\0"]), ("/r\0", vec![])] {
        let e = build_remote_command(repo, &args, None).unwrap_err();
        assert_eq!(e, QuoteError::Nul);
        assert!(e.to_string().contains("NUL"));
    }
}

#[test]
fn 원격_명령이_argv_로_복원된다() {
    let args = ["log", "--format=%s", "--", "한글 파일.txt", "o'neil"];
    let fetch = FetchEnv { allow_file: false, ssh_command: "ssh -F 'a b'".to_string() };
    for f in [None, Some(&fetch)] {
        let cmd = build_remote_command("/home/me/my repo's", &args, f).unwrap();
        let argv = unquote(&cmd);
        assert_eq!(argv[0], "env");
        let gi = argv.iter().position(|a| a == "git").unwrap();
        let envs = &argv[1..gi];
        assert!(envs.contains(&"LC_ALL=C.UTF-8".to_string()));
        assert_eq!(envs.contains(&"GIT_SSH_COMMAND=ssh -F 'a b'".to_string()), f.is_some());
        assert_eq!(argv[gi + 1..], build_git_args("/home/me/my repo's", &args));
    }
}

// ── 원격 명령 원문(exec/env.json 의 ssh 절) ──────────────────────────────────

#[test]
fn 원격_명령_골든이_ts_와_같다() {
    let path = root().join("tests/golden/exec/env.json");
    let g: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    let (mut commands, mut errors) = (0, 0);
    for c in arr(&g["ssh"], "ssh") {
        let fetch = (!c["fetch"].is_null()).then(|| FetchEnv {
            allow_file: c["fetch"]["allowFile"].as_bool().unwrap(),
            ssh_command: s(&c["fetch"]["sshCommand"]).to_string(),
        });
        let args: Vec<&str> = c["args"].as_array().unwrap().iter().map(s).collect();
        let label = format!("{} / {} / {:?}", s(&c["env"]), s(&c["repoPath"]), args);
        match build_remote_command(s(&c["repoPath"]), &args, fetch.as_ref()) {
            Ok(cmd) => {
                assert_eq!(cmd, s(&c["command"]), "{label}");
                commands += 1;
            }
            Err(e) => {
                assert_eq!(e.to_string(), s(&c["error"]), "{label}");
                errors += 1;
            }
        }
    }
    eprintln!("[원격 명령] 명령 {commands}, 거부 {errors}");
    assert!(commands >= 18 && errors == 6);
}

// ── 대응표 ────────────────────────────────────────────────────────────────────

#[test]
fn 대응표가_가리키는_rust_테스트가_있다() {
    let map: Value = serde_json::from_str(&fs::read_to_string(root().join("tests/golden/ssh/test-map.json")).unwrap()).unwrap();
    let dir = root().join("src-tauri");
    let source = |file: &str| -> String {
        let p = if file == "ssh_golden.rs" { dir.join("tests").join(file) } else { dir.join("src/git/exec/ssh").join(file) };
        fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
    };
    let (mut ported, mut later, mut na) = (0, 0, 0);
    let mut targets: BTreeSet<String> = BTreeSet::new();
    for (title, v) in map.as_object().expect("대응표는 객체") {
        let v = v.as_str().unwrap();
        if let Some(why) = v.strip_prefix("N/A: ") {
            if let Some(pr) = why.strip_prefix("PR ") {
                assert!(pr.len() == 1 && ("2"..="9").contains(&pr), "{title}: 'N/A: PR 2'..'PR 9' 형식이어야 한다 → {v}");
                later += 1;
            } else {
                assert!(why.chars().count() > 8, "{title}: N/A 사유가 비어 있다");
                na += 1;
            }
            continue;
        }
        let (file, name) = v.split_once("::").unwrap_or_else(|| panic!("{title}: '<파일>.rs::<이름>' 이어야 한다 → {v}"));
        assert!(source(file).contains(&format!("fn {name}(")), "{title}: {file} 에 fn {name} 이 없다");
        targets.insert(v.to_string());
        ported += 1;
    }
    eprintln!("[대응표] 이식 {ported}(테스트 {}개), 뒤 PR 몫 {later}, N/A {na}", targets.len());
    assert_eq!(ported + later + na, map.as_object().unwrap().len());
}
