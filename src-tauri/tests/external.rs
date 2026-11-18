//! 외부 도구로 열기(`src/external`, Phase 6b PR 3)의 시험: 골든 패리티(`tests/golden/external/*.json` — TS 가 기준: `UPDATE_GOLDEN=1 npx vitest run tests/goldenExternal.test.ts`),
//! 가짜 의존성으로 도는 실행기 시나리오, 소스 검사. **어떤 프로그램도 띄우지 않는다** — 에디터·터미널·탐색기·wsl.exe 는 가짜 `OpenerDeps` 가 받은
//! 실행 요청(argv·cwd·지울 환경변수)만 기록한다. 운영 구현 `SystemDeps` 는 프로세스를 만들지 않는 조회(`realpath`·`stat`)만 시험한다.
//! TS 테스트 → Rust 테스트 대응표는 `tests/golden/external/test-map.json`(vitest 가 누락을 강제한다).

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use antler_lib::external::link::allowed_link_url;
use antler_lib::external::opener::{
    check_chosen_exe, find_editor_exe, resolve_editor_exe, terminal_tools, EditorPrefs, ExternalAction, ExternalOpener, FindKind, LaunchRequest, OpenerDeps, Stat, ENV_REMOVE,
};
use antler_lib::external::paths::PathMod;
use antler_lib::external::plan::{
    absolute_target, build_editor_launch, build_reveal_plan, build_terminal_launch, check_real_containment, clsid_component, has_dot_component, rel_segments, EditorKind, Launch,
    Platform, RealIo, RevealState, RevealType, Target, TerminalTools,
};
use serde_json::{json, Value};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().expect("src-tauri 의 상위").to_path_buf()
}

fn golden(name: &str) -> Value {
    let path = root().join("tests").join("golden").join("external").join(name);
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

fn strings(v: &Value) -> Vec<String> {
    v.as_array().unwrap_or_else(|| panic!("배열이 아니다: {v}")).iter().map(|x| s(x).to_string()).collect()
}

const HANDLED: [&str; 3] = ["link.json", "opener.json", "plan.json"];

#[test]
fn 골든_파일마다_처리기가_있다() {
    let mut found: Vec<String> = fs::read_dir(root().join("tests/golden/external"))
        .expect("tests/golden/external")
        .map(|e| e.expect("항목").file_name().to_string_lossy().into_owned())
        // test-map.json 은 TS 테스트 → Rust 테스트 이식 대응표(손글씨)라 골든이 아니다
        .filter(|n| n.ends_with(".json") && n != "test-map.json")
        .collect();
    found.sort();
    assert_eq!(found, HANDLED, "처리기가 없는 골든이 있거나 골든이 없다");
}

// ── 입력 해석 ─────────────────────────────────────────────────────────────────

fn platform(v: &Value) -> Platform {
    match s(v) {
        "win32" => Platform::Win32,
        "linux" => Platform::Linux,
        "darwin" => Platform::Darwin,
        other => panic!("알 수 없는 플랫폼: {other}"),
    }
}

fn target(v: &Value) -> Target {
    let root = s(&v["root"]).to_string();
    match s(&v["kind"]) {
        "local" => Target::Local { root },
        "wsl" => Target::Wsl { root, distro: s(&v["distro"]).to_string() },
        "ssh" => Target::Ssh { root, host: s(&v["host"]).to_string() },
        other => panic!("알 수 없는 kind: {other}"),
    }
}

fn editor(v: &Value) -> EditorKind {
    EditorKind::parse(s(v)).unwrap_or_else(|| panic!("알 수 없는 에디터: {v}"))
}

fn launch_json(l: &Launch) -> Value {
    let mut o = json!({ "file": l.file, "args": l.args });
    if let Some(cwd) = &l.cwd {
        o["cwd"] = json!(cwd);
    }
    o
}

fn out_json(r: Result<Value, String>) -> Value {
    match r {
        Ok(v) => json!({ "ok": v }),
        Err(e) => json!({ "err": e }),
    }
}

fn tools(v: &Value) -> TerminalTools {
    let get = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
    TerminalTools { system_root: get("systemRoot"), wt: get("wt"), linux_terminal: get("linuxTerminal"), mac_terminal: get("macTerminal") }
}

// ── plan 골든 ─────────────────────────────────────────────────────────────────

struct MapIo<'a>(&'a HashMap<String, String>);

impl RealIo for MapIo<'_> {
    fn realpath(&self, p: &str) -> Option<String> {
        self.0.get(p).cloned()
    }
}

#[test]
fn plan_골든이_ts_와_같다() {
    let g = golden("plan.json");
    let section = |name: &str| arr(&g[name], name).clone();

    for c in section("relSegments") {
        let rel = c.get("rel").cloned().unwrap_or(Value::Null);
        let got = out_json(rel_segments(&rel, &target(&c["target"]), platform(&c["platform"])).map(|v| json!(v)));
        assert_eq!(got, c["out"], "relSegments: {}", c["name"]);
    }
    for c in section("absoluteTarget") {
        let got = out_json(absolute_target(&target(&c["target"]), &strings(&c["segs"]), platform(&c["platform"])).map(|v| json!(v)));
        assert_eq!(got, c["out"], "absoluteTarget: {}", c["name"]);
    }
    for c in section("checkRealContainment") {
        let links: HashMap<String, String> = c["links"].as_object().expect("links").iter().map(|(k, v)| (k.clone(), s(v).to_string())).collect();
        let m = if s(&c["mod"]) == "posix" { PathMod::Posix } else { PathMod::Win32 };
        let r = check_real_containment(s(&c["root"]), s(&c["abs"]), &MapIo(&links), m).map(|o| match o {
            None => Value::Null,
            Some(x) => json!({ "exists": x.exists, "existing": x.existing, "real": x.real, "realRoot": x.real_root }),
        });
        assert_eq!(out_json(r), c["out"], "checkRealContainment: {}", c["name"]);
    }
    for c in section("buildEditorLaunch") {
        let r = build_editor_launch(&target(&c["target"]), &strings(&c["paths"]), editor(&c["editor"]), s(&c["exe"]), platform(&c["platform"])).map(|l| launch_json(&l));
        assert_eq!(out_json(r), c["out"], "buildEditorLaunch: {}", c["name"]);
    }
    for c in section("buildTerminalLaunch") {
        let r = build_terminal_launch(&target(&c["target"]), s(&c["dir"]), platform(&c["platform"]), &tools(&c["tools"])).map(|l| launch_json(&l));
        assert_eq!(out_json(r), c["out"], "buildTerminalLaunch: {}", c["name"]);
    }
    for c in section("hasDotComponent") {
        assert_eq!(json!({ "ok": has_dot_component(s(&c["root"]), s(&c["p"])) }), c["out"], "hasDotComponent: {}", c["name"]);
    }
    for c in section("clsidComponent") {
        assert_eq!(json!({ "ok": clsid_component(s(&c["root"]), s(&c["p"])) }), c["out"], "clsidComponent: {}", c["name"]);
    }
    for c in section("buildRevealPlan") {
        let st = &c["state"];
        let state = RevealState { path: s(&st["path"]), is_dir: st["isDir"].as_bool().expect("isDir"), root: s(&st["root"]) };
        let r = build_reveal_plan(&target(&c["target"]), &state, platform(&c["platform"])).map(|p| {
            json!({ "type": if p.kind == RevealType::OpenPath { "openPath" } else { "showItem" }, "path": p.path })
        });
        assert_eq!(out_json(r), c["out"], "buildRevealPlan: {}", c["name"]);
    }
}

// ── 가짜 의존성 ───────────────────────────────────────────────────────────────

/// TS 시험의 가짜 fs 와 같다: files·dirs 는 존재하는 실제 경로, links 는 경로 → 푼 결과(통째 대응). 목록에 없는 경로는 없다
#[derive(Default)]
struct Fake {
    platform: Option<Platform>,
    files: HashSet<String>,
    dirs: HashSet<String>,
    links: HashMap<String, String>,
    env: HashMap<String, String>,
    lstat_always: bool,
    spawn_error: Option<String>,
    spawned: RefCell<Vec<LaunchRequest>>,
    shown: RefCell<Vec<Value>>,
    /// open_path 가 돌려줄 오류
    open_error: Option<String>,
}

impl Fake {
    fn from_json(c: &Value) -> Fake {
        let set = |k: &str| -> HashSet<String> { c.get(k).map(|v| strings(v).into_iter().collect()).unwrap_or_default() };
        let map = |k: &str| -> HashMap<String, String> {
            c.get(k).and_then(Value::as_object).map(|o| o.iter().map(|(k, v)| (k.clone(), s(v).to_string())).collect()).unwrap_or_default()
        };
        Fake {
            platform: Some(platform(&c["platform"])),
            files: set("files"),
            dirs: set("dirs"),
            links: map("links"),
            env: map("env"),
            lstat_always: c.get("lstatAlways").and_then(Value::as_bool).unwrap_or(false),
            ..Default::default()
        }
    }

    fn real(&self, p: &str) -> Option<String> {
        let r = self.links.get(p).cloned().unwrap_or_else(|| p.to_string());
        (self.files.contains(&r) || self.dirs.contains(&r)).then_some(r)
    }

    fn stat_of(&self, p: &str) -> Option<Stat> {
        let r = self.real(p)?;
        Some(Stat { is_dir: self.dirs.contains(&r), is_file: self.files.contains(&r) })
    }
}

impl OpenerDeps for Fake {
    fn platform(&self) -> Platform {
        self.platform.expect("플랫폼")
    }
    fn env(&self, name: &str) -> Option<String> {
        self.env.get(name).cloned()
    }
    fn realpath(&self, p: &str) -> Option<String> {
        self.real(p)
    }
    fn stat(&self, p: &str) -> Option<Stat> {
        self.stat_of(p)
    }
    fn lstat(&self, p: &str) -> Option<Stat> {
        if self.lstat_always {
            return Some(Stat { is_dir: false, is_file: true });
        }
        self.stat_of(p)
    }
    fn spawn(&self, req: &LaunchRequest) -> Result<(), String> {
        if let Some(e) = &self.spawn_error {
            return Err(e.clone());
        }
        self.spawned.borrow_mut().push(req.clone());
        Ok(())
    }
    fn open_path(&self, p: &str) -> Result<(), String> {
        if let Some(e) = &self.open_error {
            return Err(e.clone());
        }
        self.shown.borrow_mut().push(json!({ "type": "openPath", "path": p }));
        Ok(())
    }
    fn show_item(&self, p: &str) -> Result<(), String> {
        self.shown.borrow_mut().push(json!({ "type": "showItem", "path": p }));
        Ok(())
    }
}

fn prefs(ui: Option<&Value>) -> EditorPrefs {
    let Some(ui) = ui else { return EditorPrefs::default() };
    EditorPrefs { editor: ui.get("editor").map(editor), editor_exe_path: ui.get("editorExePath").and_then(Value::as_str).map(str::to_string) }
}

fn request_json(r: &LaunchRequest) -> Value {
    launch_json(&Launch { file: r.file.clone(), args: r.args.clone(), cwd: r.cwd.clone() })
}

#[test]
fn opener_시나리오_골든이_ts_와_같다() {
    let g = golden("opener.json");
    for sc in arr(&g["scenarios"], "scenarios") {
        let fake = Fake::from_json(sc);
        let opener = ExternalOpener::new(&fake);
        for (i, st) in arr(&sc["steps"], "steps").iter().enumerate() {
            let (sp, sh) = (fake.spawned.borrow().len(), fake.shown.borrow().len());
            let action = ExternalAction::parse(s(&st["action"])).expect("action");
            let rel = st.get("rel").cloned().unwrap_or(Value::Null);
            let r = opener.open(&target(&st["target"]), action, &rel, &prefs(st.get("ui")));
            let got = match r {
                Ok(()) => {
                    let spawned = fake.spawned.borrow()[sp..].to_vec();
                    // 판단은 모두 요청에 있다: 실행기는 항상 이 변수를 지우라는 요청을 받는다
                    for req in &spawned {
                        assert_eq!(req.env_remove, ENV_REMOVE, "{}: 단계 {i}", sc["name"]);
                    }
                    json!({ "ok": { "spawned": spawned.iter().map(request_json).collect::<Vec<_>>(), "shown": fake.shown.borrow()[sh..].to_vec() } })
                }
                Err(e) => {
                    assert_eq!(fake.spawned.borrow().len(), sp, "{}: 단계 {i} 오류인데 실행했다", sc["name"]);
                    assert_eq!(fake.shown.borrow().len(), sh, "{}: 단계 {i} 오류인데 표시했다", sc["name"]);
                    json!({ "err": e })
                }
            };
            assert_eq!(got, st["out"], "{}: 단계 {i}", sc["name"]);
        }
    }
}

#[test]
fn opener_실행_파일_골든이_ts_와_같다() {
    let g = golden("opener.json");
    for c in arr(&g["find"], "find") {
        let fake = Fake::from_json(c);
        let got = json!({ "vscode": find_editor_exe(FindKind::Vscode, &fake), "cursor": find_editor_exe(FindKind::Cursor, &fake) });
        assert_eq!(got, c["out"], "find: {}", c["name"]);
    }
    for c in arr(&g["chosen"], "chosen") {
        let p = s(&c["p"]);
        let plat = platform(&c["platform"]);
        let fake = Fake { platform: Some(plat), files: c.get("files").map(|v| strings(v).into_iter().collect()).unwrap_or_default(), ..Default::default() };
        let deps: Option<&dyn OpenerDeps> = c.get("files").map(|_| &fake as &dyn OpenerDeps);
        let got = out_json(check_chosen_exe(p, plat, deps).map(|v| json!(v)));
        assert_eq!(got, c["out"], "chosen: {}", c["name"]);
    }
    for c in arr(&g["resolve"], "resolve") {
        let fake = Fake::from_json(c);
        let got = out_json(resolve_editor_exe(editor(&c["editor"]), &prefs(Some(&c["ui"])), &fake).map(|v| json!(v)));
        assert_eq!(got, c["out"], "resolve: {}", c["name"]);
    }
}

#[test]
fn opener_터미널_도구_골든이_ts_와_같다() {
    let g = golden("opener.json");
    for c in arr(&g["tools"], "tools") {
        let fake = Fake::from_json(c);
        let t = terminal_tools(&fake);
        let got = json!({ "systemRoot": t.system_root, "wt": t.wt, "linuxTerminal": t.linux_terminal, "macTerminal": t.mac_terminal });
        assert_eq!(got, c["out"], "tools: {}", c["name"]);
    }
}

#[test]
fn link_골든이_ts_와_같다() {
    let g = golden("link.json");
    for c in arr(&g["cases"], "cases") {
        let got = match allowed_link_url(s(&c["raw"])) {
            Some(h) => json!(h),
            None => Value::Null,
        };
        assert_eq!(got, c["out"], "link: {}", c["name"]);
    }
}

// ── Rust 쪽 시험(TS 시험에 대응하되 골든으로 담기지 않는 것) ──────────────────

fn linux_fs() -> Fake {
    Fake {
        platform: Some(Platform::Linux),
        files: ["/home/u/repo/a.txt", "/usr/bin/x-terminal-emulator"].iter().map(|s| s.to_string()).collect(),
        dirs: ["/home/u/repo"].iter().map(|s| s.to_string()).collect(),
        ..Default::default()
    }
}

#[test]
fn 실행_실패를_오류로_돌려준다() {
    let mut fake = linux_fs();
    fake.spawn_error = Some("spawn ENOENT".to_string());
    let t = Target::Local { root: "/home/u/repo".into() };
    let e = ExternalOpener::new(&fake).open(&t, ExternalAction::Terminal, &Value::Null, &EditorPrefs::default()).unwrap_err();
    assert_eq!(e, "실행하지 못했습니다: spawn ENOENT");
    assert!(fake.spawned.borrow().is_empty());
}

#[test]
fn 파일_관리자_호출이_실패하면_오류로_돌려준다() {
    let mut fake = linux_fs();
    fake.open_error = Some("access denied".to_string());
    let t = Target::Local { root: "/home/u/repo".into() };
    let e = ExternalOpener::new(&fake).open(&t, ExternalAction::FileManager, &Value::Null, &EditorPrefs::default()).unwrap_err();
    assert_eq!(e, "파일 관리자를 열지 못했습니다: access denied");
}

#[test]
fn electron_run_as_node_는_자식에게_넘기지_않는다() {
    let fake = linux_fs();
    let t = Target::Local { root: "/home/u/repo".into() };
    ExternalOpener::new(&fake).open(&t, ExternalAction::Terminal, &Value::Null, &EditorPrefs::default()).unwrap();
    let spawned = fake.spawned.borrow();
    assert_eq!(spawned.len(), 1);
    assert_eq!(spawned[0].env_remove, vec!["ELECTRON_RUN_AS_NODE".to_string()]);
}

#[test]
fn start_뒤의_빈_창_제목은_인자로_남는다() {
    // `cmd /c start "" <program>` — 빈 문자열이 빠지면 따옴표로 감싼 첫 인자가 창 제목으로 먹힌다
    let t = TerminalTools { system_root: Some("C:\\Windows".into()), wt: None, ..Default::default() };
    let l = build_terminal_launch(&Target::Wsl { root: "/r".into(), distro: "U".into() }, "/r", Platform::Win32, &t).unwrap();
    assert_eq!(l.file, "C:\\Windows\\System32\\cmd.exe");
    assert_eq!(&l.args[..5], ["/d", "/c", "start", "", "C:\\Windows\\System32\\wsl.exe"]);
    assert_eq!(&l.args[5..], ["-d", "U", "--cd", "/r"]);
    let l = build_terminal_launch(&Target::Local { root: "C:\\r".into() }, "C:\\r", Platform::Win32, &t).unwrap();
    assert_eq!(l.args, ["/d", "/c", "start", "", "C:\\Windows\\System32\\cmd.exe"]);
    assert_eq!(l.cwd.as_deref(), Some("C:\\r"));
}

#[test]
fn 에디터가_받는_모든_경로_인자는_옵션으로_해석될_수_없다() {
    // 저장소가 정한 이름이 `-` 로 시작해도 항상 절대 경로 앞에 붙는다
    let fake = Fake {
        platform: Some(Platform::Linux),
        files: ["/r/-rf", "/usr/bin/code"].iter().map(|s| s.to_string()).collect(),
        dirs: ["/r", "/r/--remote"].iter().map(|s| s.to_string()).collect(),
        env: [("PATH".to_string(), "/usr/bin".to_string())].into(),
        ..Default::default()
    };
    let o = ExternalOpener::new(&fake);
    let t = Target::Local { root: "/r".into() };
    o.open(&t, ExternalAction::Editor, &json!("-rf"), &EditorPrefs::default()).unwrap();
    o.open(&t, ExternalAction::Editor, &json!("--remote"), &EditorPrefs::default()).unwrap();
    for req in fake.spawned.borrow().iter() {
        assert!(req.args.iter().all(|a| a.starts_with('/')), "{:?}", req.args);
    }
}

// ── 운영 구현(SystemDeps): 프로세스를 만들지 않는 조회만 ───────────────────────

#[cfg(windows)]
#[test]
fn 운영_realpath_는_확장_길이_접두를_벗긴다() {
    use antler_lib::external::launch::SystemDeps;
    let dir = std::env::temp_dir().join(format!("antler-external-{}", std::process::id()));
    fs::create_dir_all(dir.join("sub")).unwrap();
    let d = SystemDeps;
    let p = dir.join("sub").to_string_lossy().into_owned();
    let real = d.realpath(&p).expect("realpath");
    assert!(!real.starts_with("\\\\?\\"), "{real}");
    assert!(real.to_lowercase().ends_with("\\sub"), "{real}");
    assert!(d.stat(&p).unwrap().is_dir);
    assert!(d.realpath(&format!("{p}\\없는폴더")).is_none());
    assert!(d.stat(&format!("{p}\\없는폴더")).is_none());
    fs::remove_dir_all(&dir).unwrap();
}

/// 디렉터리 링크를 만든다. Windows 는 권한이 없으면 실패한다 — 그때는 false 로 시험을 건너뛴다
fn dir_link(target: &Path, link: &Path) -> bool {
    #[cfg(windows)]
    let r = std::os::windows::fs::symlink_dir(target, link);
    #[cfg(unix)]
    let r = std::os::unix::fs::symlink(target, link);
    r.is_ok()
}

#[test]
fn 링크_사슬은_원격_대상과_깊이_상한에서_거부하고_로컬_대상은_통과한다() {
    use antler_lib::fsguard::links_stay_local;
    let dir = std::env::temp_dir().join(format!("antler-links-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("real").join("deep")).unwrap();
    // 링크가 없는 경로·없는 경로는 통과(없는 경로는 뒤따르는 canonicalize 가 처리한다)
    assert!(links_stay_local(&dir.join("real").join("deep"), |_| true));
    assert!(links_stay_local(&dir.join("real").join("없음").join("x"), |_| true));
    // a -> real(상대) , b -> a(상대) : 로컬 사슬
    let ok = dir_link(Path::new("real"), &dir.join("a")) && dir_link(Path::new("a"), &dir.join("b"));
    if ok {
        let via = dir.join("b").join("deep");
        assert!(links_stay_local(&via, |_| false), "로컬 대상 사슬은 통과해야 한다");
        // 판정 함수가 원격이라 하면 첫 링크에서 멈춘다
        assert!(!links_stay_local(&via, |_| true));
        // 링크 대상 문자열을 그대로 판정 함수에 넘긴다
        let seen = std::cell::RefCell::new(Vec::new());
        assert!(links_stay_local(&via, |t| {
            seen.borrow_mut().push(t.to_string());
            false
        }));
        assert_eq!(*seen.borrow(), vec!["a".to_string(), "real".to_string()]);
    }
    // 순환 링크 x -> y, y -> x 는 깊이 상한(16)에서 거부한다
    if dir_link(Path::new("y"), &dir.join("x")) && dir_link(Path::new("x"), &dir.join("y")) {
        assert!(!links_stay_local(&dir.join("x").join("f"), |_| false));
    }
    // 길이 16 의 사슬은 통과, 17 은 거부
    let mut made = true;
    for i in 0..17 {
        let target = if i == 0 { "real".to_string() } else { format!("c{}", i - 1) };
        made &= dir_link(Path::new(&target), &dir.join(format!("c{i}")));
    }
    if made {
        assert!(links_stay_local(&dir.join("c15"), |_| false));
        assert!(!links_stay_local(&dir.join("c16"), |_| false));
    }
    fs::remove_dir_all(&dir).unwrap();
}

/// 정션 시험용 임시 디렉터리. 만든 정션을 기억해 두었다가 Drop 에서 정션부터 `remove_dir`(rmdir)로 지운다 —
/// 정션을 따라 원본을 지우지 않도록 `remove_dir_all` 은 정션이 하나도 남지 않은 뒤에만 쓴다
#[cfg(windows)]
struct JunctionDir {
    root: PathBuf,
    junctions: Vec<PathBuf>,
}

#[cfg(windows)]
impl JunctionDir {
    fn new(tag: &str) -> Self {
        let root = std::env::temp_dir().join(format!("antler-junc-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("real").join("deep")).unwrap();
        JunctionDir { root, junctions: Vec::new() }
    }

    /// `cmd /c mklink /J` 로 정션을 만든다. 실패하면 시험이 실패한다(건너뛰지 않는다)
    fn junction(&mut self, name: &str, target: &Path) -> PathBuf {
        let link = self.root.join(name);
        // git 이 아니라 cmd 의 mklink 를 부른다(정션은 std 로 만들 수 없다)
        #[allow(clippy::disallowed_methods)]
        let out = std::process::Command::new("cmd")
            .arg("/c")
            .arg("mklink")
            .arg("/J")
            .arg(&link)
            .arg(target)
            .output()
            .expect("cmd 실행");
        assert!(
            out.status.success(),
            "mklink /J 실패: {} {}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        let meta = fs::symlink_metadata(&link).expect("정션이 만들어져야 한다");
        assert!(meta.is_dir() || meta.file_type().is_symlink(), "{}", link.display());
        self.junctions.push(link.clone());
        link
    }
}

#[cfg(windows)]
impl Drop for JunctionDir {
    fn drop(&mut self) {
        for j in self.junctions.iter().rev() {
            let _ = fs::remove_dir(j);
        }
        // 정션이 남아 있으면 원본을 지울 수 있으니 지우지 않는다
        if self.junctions.iter().all(|j| fs::symlink_metadata(j).is_err()) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}

#[cfg(windows)]
#[test]
fn 정션_사슬은_로컬_대상이면_통과하고_원격_판정이면_사슬_중간에서_거부한다() {
    use antler_lib::fsguard::links_stay_local;
    let mut d = JunctionDir::new("chain");
    let real = d.root.join("real");
    // b -> a -> real (정션은 대상을 절대 경로로 저장한다)
    let a = d.junction("a", &real);
    let b = d.junction("b", &a);
    // 정션을 링크로 인식해야 한다 — 인식 못 하면 아래 판정 함수가 한 번도 불리지 않는다
    assert!(fs::symlink_metadata(&a).unwrap().file_type().is_symlink(), "정션이 링크로 보여야 한다");
    let via = b.join("deep");
    assert!(links_stay_local(&via, |_| false), "로컬 대상 정션 사슬은 통과해야 한다");
    // 판정 함수가 호출된 횟수와 대상(사슬 b -> a -> real 의 두 홉)
    let seen = RefCell::new(Vec::<String>::new());
    assert!(links_stay_local(&via, |t| {
        seen.borrow_mut().push(t.to_string());
        false
    }));
    assert_eq!(seen.borrow().len(), 2, "{:?}", seen.borrow());
    // 사슬 중간(a 의 대상 = real)만 원격이라 하면 b -> a 는 통과하고 a -> real 에서 거부한다
    let real_lower = real.to_string_lossy().to_lowercase();
    let calls = RefCell::new(0);
    let reject_real = |t: &str| {
        *calls.borrow_mut() += 1;
        t.to_lowercase().ends_with(&real_lower)
    };
    assert!(!links_stay_local(&via, reject_real), "사슬 중간 정션에서 거부해야 한다");
    assert_eq!(*calls.borrow(), 2, "첫 홉은 통과하고 둘째 홉에서 거부해야 한다");
    // 첫 홉(b 의 대상 = a)만 원격이라 하면 첫 홉에서 멈춘다
    let a_lower = a.to_string_lossy().to_lowercase();
    let calls = RefCell::new(0);
    assert!(!links_stay_local(&via, |t| {
        *calls.borrow_mut() += 1;
        t.to_lowercase().ends_with(&a_lower)
    }));
    assert_eq!(*calls.borrow(), 1);
}

#[cfg(windows)]
#[test]
fn 정션_대상의_nt_접두는_win32_확장_접두로_바뀌어_판정_함수에_넘어간다() {
    use antler_lib::fsguard::links_stay_local;
    let mut d = JunctionDir::new("prefix");
    let real = d.root.join("real");
    let a = d.junction("a", &real);
    let seen = RefCell::new(Vec::<String>::new());
    assert!(links_stay_local(&a.join("deep"), |t| {
        seen.borrow_mut().push(t.to_string());
        false
    }));
    let seen = seen.borrow();
    assert_eq!(seen.len(), 1, "{seen:?}");
    assert!(!seen[0].starts_with("\\??\\"), "NT 접두가 남아 있다: {}", seen[0]);
    // 로컬 드라이브 정션은 std 의 read_link 가 이미 `\??\` 를 벗겨 `C:\…` 로 돌려준다 — `\\?\` 로 바뀐 형태가 아니어도 정상이다.
    // (launch.rs 의 `\??\` → `\\?\` 변환은 std 가 벗기지 않는 대상용이며, 원격 UNC 정션은 만들 수 없어 여기서는 닿지 않는다)
    let b = seen[0].as_bytes();
    let drive_path = b.len() > 3 && b[0].is_ascii_alphabetic() && &seen[0][1..3] == ":\\";
    assert!(drive_path || seen[0].starts_with("\\\\?\\"), "절대 경로여야 한다: {}", seen[0]);
    assert!(seen[0].to_lowercase().ends_with("\\real"), "{}", seen[0]);
}

#[cfg(windows)]
#[test]
fn 정션_사슬_길이_16_은_통과하고_17_은_거부한다() {
    use antler_lib::fsguard::links_stay_local;
    let mut d = JunctionDir::new("hops");
    let mut prev = d.root.join("real");
    let mut links = Vec::new();
    for i in 0..17 {
        prev = d.junction(&format!("c{i}"), &prev);
        links.push(prev.clone());
    }
    assert_eq!(links.len(), 17);
    assert!(links_stay_local(&links[15].join("deep"), |_| false), "홉 16 은 통과해야 한다");
    assert!(!links_stay_local(&links[16].join("deep"), |_| false), "홉 17 은 거부해야 한다");
}

/// 앱 실행 별칭(`%LOCALAPPDATA%\Microsoft\WindowsApps\wt.exe`)은 `stat` 이 접근 거부로 실패하고 `lstat` 만 된다 — 있는 PC 에서 직접 확인하는 시험이다
/// (`cargo test --test external -- --ignored`). 파일을 **열거나 실행하지 않고** 메타데이터만 읽는다
#[cfg(windows)]
#[test]
#[ignore = "wt.exe 앱 실행 별칭이 있는 PC 에서만 의미가 있다"]
fn wt_앱_실행_별칭을_lstat_으로_볼_수_있다() {
    use antler_lib::external::launch::SystemDeps;
    let Some(local) = std::env::var_os("LOCALAPPDATA") else { return };
    let p = Path::new(&local).join("Microsoft").join("WindowsApps").join("wt.exe");
    if fs::symlink_metadata(&p).is_err() {
        return;
    }
    let d = SystemDeps;
    let got = d.lstat(&p.to_string_lossy());
    assert!(got.is_some(), "lstat 으로 볼 수 없다: {}", p.display());
    println!("wt.exe lstat = {got:?}, stat = {:?}", d.stat(&p.to_string_lossy()));
}

// ── 소스 검사 ─────────────────────────────────────────────────────────────────

fn rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in fs::read_dir(dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display())) {
        let p = e.expect("항목").path();
        if p.is_dir() {
            rs_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

/// 주석(`//`)·단위 시험(`#[cfg(test)]` 이후)을 뺀 운영 코드
fn production_code(path: &Path) -> String {
    let text = fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let body = text.split("#[cfg(test)]").next().unwrap_or("");
    body.lines().filter(|l| !l.trim_start().starts_with("//")).collect::<Vec<_>>().join("\n")
}

#[test]
fn 에디터와_터미널_프로세스는_launch_rs_한_곳에서만_띄운다() {
    let dir = root().join("src-tauri/src/external");
    let mut files = Vec::new();
    rs_files(&dir, &mut files);
    assert!(files.len() >= 6, "{files:?}");
    let spawners = ["Command::new", "tauri_plugin_opener", "std::process", "tokio::process", "CreateProcess", "ShellExecute", "raw_arg"];
    let mut users = Vec::new();
    for f in &files {
        let code = production_code(f);
        let name = f.file_name().unwrap().to_string_lossy().into_owned();
        for pat in spawners {
            if code.contains(pat) {
                users.push(format!("{name}: {pat}"));
            }
        }
    }
    users.sort();
    // launch.rs 만: Command(생성·spawn) 과 opener 플러그인(open_path·reveal_item_in_dir)
    assert!(users.iter().all(|u| u.starts_with("launch.rs: ")), "launch.rs 밖에서 프로세스를 띄운다: {users:?}");
    assert!(users.iter().any(|u| u == "launch.rs: Command::new"), "launch.rs 가 Command::new 를 쓰지 않는다: {users:?}");
    assert!(!users.iter().any(|u| u.ends_with("raw_arg")), "인자를 문자열로 조립하지 않는다(.arg 만): {users:?}");
    // Command::new 는 launch.rs 안에서도 한 번뿐이고 allow 와 이유를 단다
    let launch = production_code(&dir.join("launch.rs"));
    assert_eq!(launch.matches("Command::new").count(), 1);
    assert!(launch.contains("#[allow(clippy::disallowed_methods)]\n        let mut cmd = Command::new"));
    // 셸 없이 인자 배열만, stdio 없음, 분리 실행
    assert!(launch.contains(".args(&req.args)") && launch.contains("Stdio::null()"));
    assert!(launch.contains("creation_flags(DETACHED_FLAGS)"));
    assert!(!launch.contains("\"/c\"") && !launch.contains("cmd.exe") && !launch.contains("sh -c"), "launch.rs 는 무엇을 띄울지 정하지 않는다");
}

#[test]
fn 외부_열기는_링크_허용_판정만_앱에_연결되어_있다() {
    // PR 9 가 `external:open` 을 연결하기 전까지 `external::link`(code:openLink 의 주소 허용, PR 5)를 `app_host.rs` 가 쓰는 것 말고는 이 모듈을 쓰는 곳이 없다.
    // 에디터·터미널·파일 관리자 실행(plan·opener·launch)은 앱에 닿지 않는다
    let src = root().join("src-tauri/src");
    let mut files = Vec::new();
    rs_files(&src, &mut files);
    for f in files {
        if f.starts_with(src.join("external")) {
            continue;
        }
        let code = production_code(&f);
        let name = f.file_name().unwrap().to_string_lossy().into_owned();
        let uses = code.matches("external::").count();
        if name == "app_host.rs" {
            assert_eq!(uses, code.matches("external::link::allowed_link_url").count(), "app_host.rs 는 링크 판정만 쓴다");
            assert_eq!(uses, 1);
        } else if name == "dialogs.rs" {
            // 폴더 선택 시작 위치의 링크 확인은 공용 링크 가드(`fsguard`)를 쓴다 — external 은 쓰지 않는다
            assert_eq!(uses, 0, "dialogs.rs 가 external 을 쓴다");
            assert_eq!(code.matches("fsguard::path_stays_local").count(), 1);
        } else {
            assert_eq!(uses, 0, "{} 가 external 을 쓴다", f.display());
        }
    }
    let lib = fs::read_to_string(src.join("lib.rs")).unwrap();
    assert_eq!(lib.matches("pub mod external;").count(), 1);
}

#[test]
fn 링크_판정_구현은_fsguard_한_곳에만_있고_쓰는_쪽은_모두_그것을_부른다() {
    let src = root().join("src-tauri/src");
    let mut files = Vec::new();
    rs_files(&src, &mut files);
    let mut definers = Vec::new();
    for f in &files {
        let code = production_code(f);
        let name = f.file_name().unwrap().to_string_lossy().into_owned();
        for pat in ["fn is_remote_link_target", "fn links_stay_local", "fn link_leads_remote", "fn push_components"] {
            if code.contains(pat) {
                definers.push(format!("{name}: {pat}"));
            }
        }
    }
    definers.sort();
    assert_eq!(definers, ["fsguard.rs: fn is_remote_link_target", "fsguard.rs: fn links_stay_local", "fsguard.rs: fn push_components"], "중복 구현");
    // 쓰는 곳: 외부 열기(realpath)·폴더 선택·폴더 찾기·환경 채널의 루트 확인
    let uses = |rel: &str, pat: &str| production_code(&src.join(rel)).matches(pat).count();
    assert_eq!(uses("external/launch.rs", "path_stays_local(Path::new(p))"), 1);
    assert_eq!(uses("dialogs.rs", "fsguard::path_stays_local"), 1);
    assert_eq!(uses("repos/scan.rs", "links_stay_local(&entry.path(), &is_remote)"), 1);
    assert_eq!(uses("env_host.rs", "fsguard::check_local_root("), 1);
}

#[test]
fn 계획과_실행기_모듈은_파일_시스템과_환경을_직접_읽지_않는다() {
    // plan·opener·paths·link 는 주입받은 `OpenerDeps` 로만 세상을 본다(시험이 가짜로 바꿀 수 있어야 한다)
    for name in ["plan.rs", "opener.rs", "paths.rs", "link.rs"] {
        let code = production_code(&root().join("src-tauri/src/external").join(name));
        for pat in ["std::fs", "std::env", "File::open", "read_to_string", "canonicalize", "metadata("] {
            assert!(!code.contains(pat), "{name} 가 {pat} 를 직접 쓴다");
        }
    }
}
