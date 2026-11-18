//! 외부 도구로 열기의 실행부(`src/main/external/opener.ts` 의 이식). 파일 시스템·환경변수·프로세스 실행·파일 관리자는 [`OpenerDeps`] 로 주입받는다 —
//! 시험은 가짜로 바꿔 **어떤 프로그램도 띄우지 않고** 실행 요청(argv·cwd)만 검증하고, 운영 구현은 `launch.rs` 한 곳에 있다.
//! 저장소에는 아무것도 쓰지 않는다 — 폴더·파일을 가리켜 외부 프로그램을 띄울 뿐이다.

use serde_json::Value;

use super::paths::PathMod;
use super::plan::{
    absolute_target, assert_safe_name, build_editor_launch, build_reveal_plan, build_terminal_launch, check_real_containment, clsid_component, has_dot_component, is_double_sep,
    rel_segments, EditorKind, Launch, NameKind, Platform, PlanResult, RealIo, RevealState, RevealType, Target, TerminalTools,
};
use crate::git::exec::ssh::win32_path as w32;
use crate::git::exec::wsl_paths::{is_wsl_unc, unc_from_wsl, wsl_from_unc};

/// 외부 프로그램을 띄우면서 자식에게 넘기지 않을 환경변수. Electron 을 Node 로 돌리는 변수가 에디터(Electron 앱)에 새지 않게 한다
pub const ENV_REMOVE: [&str; 1] = ["ELECTRON_RUN_AS_NODE"];

/// [`OpenerDeps::spawn`] 에 넘기는, 판단이 모두 끝난 실행 요청. 운영 구현(`launch.rs`)은 이것을 그대로 실행할 뿐 아무것도 정하지 않는다:
/// 분리 프로세스·셸 없음·stdio 없음·`env_remove` 의 변수 제거
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LaunchRequest {
    pub file: String,
    pub args: Vec<String>,
    pub cwd: Option<String>,
    pub env_remove: Vec<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stat {
    pub is_dir: bool,
    pub is_file: bool,
}

/// 주입점. TS `OpenerDeps` 와 같은 모양이다
pub trait OpenerDeps {
    fn platform(&self) -> Platform;
    /// 환경변수 하나(Windows 는 이름의 대소문자를 가리지 않는다)
    fn env(&self, name: &str) -> Option<String>;
    /// 심볼릭 링크를 모두 푼 경로(Windows 의 `\\?\` 접두는 벗긴 형태 — [`normalize_canonical`]). 없으면 None
    fn realpath(&self, p: &str) -> Option<String>;
    /// 링크를 따라간 상태. 없거나 읽을 수 없으면 None
    fn stat(&self, p: &str) -> Option<Stat>;
    /// 링크 자체를 본다(Windows 앱 실행 별칭 `wt.exe` 는 `stat` 이 접근 거부로 실패한다)
    fn lstat(&self, p: &str) -> Option<Stat>;
    /// 분리된 프로세스로 띄운다. 실행에 실패(없는 파일 등)하면 Err
    fn spawn(&self, req: &LaunchRequest) -> Result<(), String>;
    /// 폴더를 파일 관리자로 연다(`opener::open_path`)
    fn open_path(&self, p: &str) -> Result<(), String>;
    /// 폴더에서 항목을 선택해 보여 준다(`opener::reveal_item_in_dir`)
    fn show_item(&self, p: &str) -> Result<(), String>;
}

/// `std::fs::canonicalize` 의 결과를 `node:fs.realpathSync` 와 같은 모양으로 맞춘다. Windows 의 `canonicalize` 는 `\\?\C:\x`·`\\?\UNC\host\share\x` 처럼
/// 확장 길이 접두를 붙이는데, 비교(저장소 루트 안인가)가 접두 유무에 흔들리면 안 된다.
/// - `\\?\UNC\server\share\…` → `\\server\share\…`, `\\?\C:\…` → `C:\…`
/// - 그 밖의 `\\?\`·`\\.\`(볼륨 GUID·장치 경로)는 거부(None)
/// - 벗긴 결과가 UNC 인데 WSL 공유(`\\wsl.localhost\`·`\\wsl$\`)가 아니면 거부한다 — 임의의 SMB 공유(NTLM 유출)를 열 대상으로 삼지 않는다
pub fn normalize_canonical(p: &str) -> Option<String> {
    let out = if let Some(rest) = p.strip_prefix("\\\\?\\") {
        if rest.get(..4).is_some_and(|h| h.eq_ignore_ascii_case("UNC\\")) {
            format!("\\\\{}", &rest[4..])
        } else {
            let b = rest.as_bytes();
            if b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && b[2] == b'\\' {
                rest.to_string()
            } else {
                return None;
            }
        }
    } else if p.starts_with("\\\\.\\") {
        return None;
    } else {
        p.to_string()
    };
    if is_double_sep(&out) && !is_wsl_unc(&out) {
        return None;
    }
    Some(out)
}

fn is_file(deps: &dyn OpenerDeps, p: &str) -> bool {
    deps.stat(p).is_some_and(|s| s.is_file)
}

/// PATH 의 폴더에서 실행 파일 이름을 찾아 절대 경로로 돌려준다(리눅스). 상대 항목(저장소 폴더 등)은 쓰지 않는다
fn find_in_path(deps: &dyn OpenerDeps, name: &str) -> Option<String> {
    let path = deps.env("PATH").unwrap_or_default();
    for dir in path.split(':') {
        if dir.is_empty() || !dir.starts_with('/') {
            continue;
        }
        let p = PathMod::Posix.join(&[dir, name]);
        if is_file(deps, &p) {
            return Some(p);
        }
    }
    None
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FindKind {
    Vscode,
    Cursor,
}

/// VS Code·Cursor 의 실행 파일. 저장소가 아니라 앱이 아는 설치 위치에서만 찾는다.
/// Windows 는 .cmd 셰임이 아니라 실제 .exe 를 쓴다(셸 없이는 .cmd 를 실행할 수 없다).
pub fn find_editor_exe(kind: FindKind, deps: &dyn OpenerDeps) -> Option<String> {
    let mut candidates: Vec<String> = Vec::new();
    match deps.platform() {
        Platform::Win32 => {
            let sub: [&str; 2] = if kind == FindKind::Vscode { ["Microsoft VS Code", "Code.exe"] } else { ["cursor", "Cursor.exe"] };
            let join = |base: &str, mid: &[&str]| {
                let mut parts: Vec<&str> = vec![base];
                parts.extend(mid);
                parts.extend(sub);
                w32::join(&parts)
            };
            // JS truthy: 빈 문자열은 없는 것으로 본다
            let var = |n: &str| deps.env(n).filter(|v| !v.is_empty());
            if let Some(local) = var("LOCALAPPDATA") {
                candidates.push(join(&local, &["Programs"]));
            }
            if kind == FindKind::Vscode {
                if let Some(pf) = var("ProgramFiles") {
                    candidates.push(join(&pf, &[]));
                }
                if let Some(pf86) = var("ProgramFiles(x86)") {
                    candidates.push(join(&pf86, &[]));
                }
            } else if let Some(pf) = var("ProgramFiles") {
                candidates.push(join(&pf, &[]));
            }
        }
        Platform::Darwin => candidates.push(
            if kind == FindKind::Vscode {
                "/Applications/Visual Studio Code.app/Contents/Resources/app/bin/code"
            } else {
                "/Applications/Cursor.app/Contents/Resources/app/bin/cursor"
            }
            .to_string(),
        ),
        Platform::Linux => {
            let name = if kind == FindKind::Vscode { "code" } else { "cursor" };
            if let Some(found) = find_in_path(deps, name) {
                candidates.push(found);
            }
            candidates.extend([format!("/usr/bin/{name}"), format!("/usr/local/bin/{name}"), format!("/snap/bin/{name}")]);
        }
    }
    candidates.into_iter().find(|p| is_file(deps, p))
}

/// 사용자가 설정 화면에서 고른 실행 파일을 다시 확인한다(절대 경로·존재·Windows 는 .exe/.com 만). `deps` 가 있으면 존재도 본다
pub fn check_chosen_exe(p: &str, platform: Platform, deps: Option<&dyn OpenerDeps>) -> PlanResult<String> {
    if p.is_empty() || p.encode_utf16().count() > 4096 || p.contains('\0') {
        return Err("잘못된 실행 파일 경로입니다".to_string());
    }
    let m = platform.path_mod();
    if !m.is_absolute(p) || (platform == Platform::Win32 && is_double_sep(p)) {
        return Err("로컬 폴더의 실행 파일만 고를 수 있습니다".to_string());
    }
    if platform == Platform::Win32 && !has_exe_ext(p) {
        return Err(".exe 파일만 고를 수 있습니다(.cmd·.bat 은 안전하게 실행할 수 없습니다)".to_string());
    }
    if let Some(d) = deps {
        if !is_file(d, p) {
            return Err("실행 파일을 찾을 수 없습니다. 설정에서 다시 선택하세요".to_string());
        }
    }
    Ok(p.to_string())
}

/// `/\.(exe|com)$/i`
fn has_exe_ext(p: &str) -> bool {
    let l = p.to_ascii_lowercase();
    l.ends_with(".exe") || l.ends_with(".com")
}

/// 에디터 설정(`ui`)에서 이 모듈이 쓰는 두 값
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EditorPrefs {
    pub editor: Option<EditorKind>,
    pub editor_exe_path: Option<String>,
}

pub fn resolve_editor_exe(editor: EditorKind, ui: &EditorPrefs, deps: &dyn OpenerDeps) -> PlanResult<String> {
    if matches!(editor, EditorKind::Vscode | EditorKind::Cursor) {
        let kind = if editor == EditorKind::Vscode { FindKind::Vscode } else { FindKind::Cursor };
        return find_editor_exe(kind, deps).ok_or_else(|| format!("{} 를 찾을 수 없습니다. 설정에서 다른 에디터를 고르세요", if editor == EditorKind::Vscode { "VS Code" } else { "Cursor" }));
    }
    match ui.editor_exe_path.as_deref().filter(|p| !p.is_empty()) {
        None => Err("에디터 실행 파일을 먼저 고르세요(설정)".to_string()),
        Some(p) => check_chosen_exe(p, deps.platform(), Some(deps)),
    }
}

/// `^[A-Za-z]:\\[^\0"<>|?*;]*$`
fn win_drive_dir(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && b[2] == b'\\' && !s[3..].chars().any(|c| matches!(c, '\0' | '"' | '<' | '>' | '|' | '?' | '*' | ';'))
}

/// 터미널로 띄울 프로그램의 절대 경로를 찾는다. 상대 이름은 쓰지 않는다(cwd 의 저장소 파일이 먼저 실행될 수 있다)
pub fn terminal_tools(deps: &dyn OpenerDeps) -> TerminalTools {
    match deps.platform() {
        Platform::Win32 => {
            let system_root = deps.env("SystemRoot").filter(|r| win_drive_dir(r)).unwrap_or_else(|| "C:\\Windows".to_string());
            let wt = deps.env("LOCALAPPDATA").filter(|l| win_drive_dir(l)).and_then(|local| {
                let p = w32::join(&[&local, "Microsoft", "WindowsApps", "wt.exe"]);
                deps.lstat(&p).map(|_| p)
            });
            TerminalTools { system_root: Some(system_root), wt, ..Default::default() }
        }
        Platform::Darwin => {
            // 번들 ID 조회(open -b)는 저장소의 가짜 Terminal.app 에 가로채일 수 있어 시스템 위치의 절대 경로로 고정한다
            let dirs = ["/System/Applications/Utilities/Terminal.app", "/Applications/Utilities/Terminal.app"];
            let mac = dirs.iter().find(|d| deps.lstat(d).is_some_and(|s| s.is_dir)).map(|d| d.to_string());
            TerminalTools { mac_terminal: mac, ..Default::default() }
        }
        Platform::Linux => {
            let term = "x-terminal-emulator";
            let linux = find_in_path(deps, term).or_else(|| {
                let p = format!("/usr/bin/{term}");
                is_file(deps, &p).then_some(p)
            });
            TerminalTools { linux_terminal: linux, ..Default::default() }
        }
    }
}

fn launch(deps: &dyn OpenerDeps, l: Launch) -> PlanResult<()> {
    let req = LaunchRequest { file: l.file, args: l.args, cwd: l.cwd, env_remove: ENV_REMOVE.iter().map(|s| s.to_string()).collect() };
    deps.spawn(&req).map_err(|e| format!("실행하지 못했습니다: {e}"))
}

/// realpath 기준 검사 결과. `real` 은 대상(없으면 가장 가까운 상위 폴더)의 실제 경로, WSL 은 UNC 형태
struct Inspected {
    exists: bool,
    is_dir: bool,
    real: String,
    real_root: String,
}

struct DepsIo<'a>(&'a dyn OpenerDeps);

impl RealIo for DepsIo<'_> {
    fn realpath(&self, p: &str) -> Option<String> {
        self.0.realpath(p)
    }
}

pub struct ExternalOpener<'a> {
    deps: &'a dyn OpenerDeps,
}

/// `external:open` 의 동작 허용 목록
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExternalAction {
    FileManager,
    Editor,
    Terminal,
}

impl ExternalAction {
    pub fn parse(s: &str) -> Option<ExternalAction> {
        match s {
            "fileManager" => Some(ExternalAction::FileManager),
            "editor" => Some(ExternalAction::Editor),
            "terminal" => Some(ExternalAction::Terminal),
            _ => None,
        }
    }
}

impl<'a> ExternalOpener<'a> {
    pub fn new(deps: &'a dyn OpenerDeps) -> Self {
        ExternalOpener { deps }
    }

    /// `target.root` 는 저장소 최상위, `rel` 은 그 안의 경로. 검증을 통과한 경우에만 외부 프로그램을 띄운다
    pub fn open(&self, target: &Target, action: ExternalAction, rel: &Value, ui: &EditorPrefs) -> PlanResult<()> {
        let deps = self.deps;
        let platform = deps.platform();
        let segs = rel_segments(rel, target, platform)?;
        let abs = absolute_target(target, &segs, platform)?;
        let root = absolute_target(target, &[], platform)?;
        let state = self.inspect(target, &root, &abs)?;

        match action {
            ExternalAction::FileManager => {
                let plan = build_reveal_plan(target, &RevealState { path: &state.real, is_dir: state.is_dir, root: &state.real_root }, platform)?;
                match plan.kind {
                    RevealType::ShowItem => deps.show_item(&plan.path).map_err(|e| format!("파일 관리자를 열지 못했습니다: {e}"))?,
                    RevealType::OpenPath => deps.open_path(&plan.path).map_err(|e| format!("파일 관리자를 열지 못했습니다: {e}"))?,
                }
                Ok(())
            }
            ExternalAction::Editor => {
                let editor = ui.editor.unwrap_or(EditorKind::Vscode);
                let exe = resolve_editor_exe(editor, ui, deps)?;
                // 저장소 폴더를 열고, 파일이면 그 파일도 함께 연다(없는 파일은 폴더만)
                let paths = if state.exists && abs != root { vec![root.clone(), abs.clone()] } else { vec![root.clone()] };
                launch(deps, build_editor_launch(target, &paths, editor, &exe, platform)?)
            }
            ExternalAction::Terminal => {
                let dm = platform.path_mod();
                // 터미널은 realpath 가 폴더인 곳에서만 연다(파일·번들을 넘기면 open 이 실행해 버린다)
                let mut dir = if state.exists && !state.is_dir { dm.dirname(&state.real) } else { state.real.clone() };
                if platform == Platform::Darwin {
                    // 점이 든 폴더(번들일 수 있음)는 피해 점 없는 가장 가까운 상위로 올라간다. 저장소 루트까지 없으면 거부
                    while has_dot_component(&state.real_root, &dir) {
                        if dir == state.real_root || dir == dm.dirname(&dir) {
                            return Err("이 저장소 경로에는 번들일 수 있는 폴더(이름에 .)가 있어 터미널을 열 수 없습니다".to_string());
                        }
                        dir = dm.dirname(&dir);
                    }
                }
                if platform == Platform::Win32 {
                    // 이름 확장 폴더(name.{CLSID})는 셸이 바로 가기로 다루므로 피해 가장 가까운 상위로 올라간다. 루트까지 없으면 거부
                    while clsid_component(&state.real_root, &dir).is_some() {
                        if dir == state.real_root || dir == dm.dirname(&dir) {
                            return Err("이 저장소 경로에는 폴더 바로 가기로 해석되는 이름(.{GUID})이 있어 터미널을 열 수 없습니다".to_string());
                        }
                        dir = dm.dirname(&dir);
                    }
                }
                if matches!(target, Target::Wsl { .. }) {
                    dir = wsl_from_unc(&dir).map(|(_, p)| p).unwrap_or_else(|| root.clone());
                }
                launch(deps, build_terminal_launch(target, &dir, platform, &terminal_tools(deps))?)
            }
        }
    }

    /// 대상이 있는지·폴더인지, 없으면 존재하는 가장 가까운 상위를 구한다. 심볼릭 링크를 풀어도 저장소 안인지 확인한다.
    /// 로컬은 반드시 확인한다. WSL 은 UNC 경로로 확인하되 풀 수 없으면(배포판이 꺼져 있는 등) 어휘적 검사만 한다.
    /// SSH 는 원격이라 확인할 수 없다(어휘적 검사만) — 원격 쪽 링크는 그 호스트의 에디터가 다룬다.
    fn inspect(&self, target: &Target, root: &str, abs: &str) -> PlanResult<Inspected> {
        let deps = self.deps;
        if matches!(target, Target::Ssh { .. }) {
            return Ok(Inspected { exists: true, is_dir: abs == root, real: abs.to_string(), real_root: root.to_string() });
        }
        let distro = match target {
            Target::Wsl { distro, .. } => Some(distro.as_str()),
            _ => None,
        };
        let wsl = distro.is_some();
        // 배포판 이름은 UNC 경로에 쓰기 전에 검증한다
        if let Some(d) = distro {
            assert_safe_name(NameKind::Distro, d)?;
        }
        if wsl && deps.platform() != Platform::Win32 {
            return Err("WSL 저장소는 Windows 에서만 열 수 있습니다".to_string());
        }
        let m = if !wsl && deps.platform() != Platform::Win32 { PathMod::Posix } else { PathMod::Win32 };
        let (n_root, n_abs) = match distro {
            Some(d) => (unc_from_wsl(d, root), unc_from_wsl(d, abs)),
            None => (root.to_string(), abs.to_string()),
        };
        let checked = check_real_containment(&n_root, &n_abs, &DepsIo(deps), m)?;
        let Some(checked) = checked else {
            if !wsl {
                return Err("저장소 폴더를 찾을 수 없습니다".to_string());
            }
            // 배포판이 꺼져 있는 등 풀 수 없으면 어휘 경로를 그대로 쓰되 폴더로 취급하지 않는다(openPath 안 함)
            return Ok(Inspected { exists: true, is_dir: false, real: n_abs, real_root: n_root });
        };
        let is_directory = |p: &str| deps.stat(p).is_some_and(|s| s.is_dir);
        // 모든 판정은 링크를 푼 실제 경로(real) 기준이다. 대상이 없으면 존재하는 가장 가까운 상위이고,
        // 그 상위가 파일이면(evil.exe/x → evil.exe) 폴더가 될 때까지 한 단계씩 올라간다
        let mut real = checked.real;
        if checked.exists {
            return Ok(Inspected { exists: true, is_dir: is_directory(&real), real, real_root: checked.real_root });
        }
        while real != checked.real_root && !is_directory(&real) {
            let up = m.dirname(&real);
            if up == real {
                break;
            }
            real = up;
        }
        Ok(Inspected { exists: false, is_dir: true, real, real_root: checked.real_root })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn realpath_의_확장_길이_접두를_벗기고_wsl_이_아닌_unc_는_거부한다() {
        assert_eq!(normalize_canonical("\\\\?\\C:\\work\\repo").as_deref(), Some("C:\\work\\repo"));
        assert_eq!(normalize_canonical("\\\\?\\c:\\").as_deref(), Some("c:\\"));
        assert_eq!(normalize_canonical("\\\\?\\UNC\\wsl.localhost\\Ubuntu\\home\\u").as_deref(), Some("\\\\wsl.localhost\\Ubuntu\\home\\u"));
        assert_eq!(normalize_canonical("\\\\?\\UNC\\wsl$\\Ubuntu").as_deref(), Some("\\\\wsl$\\Ubuntu"));
        assert_eq!(normalize_canonical("\\\\?\\unc\\WSL.LOCALHOST\\Ubuntu").as_deref(), Some("\\\\WSL.LOCALHOST\\Ubuntu"));
        // WSL 이 아닌 UNC(임의의 SMB 공유)·장치 경로·볼륨 GUID 는 거부
        assert_eq!(normalize_canonical("\\\\?\\UNC\\host\\share"), None);
        assert_eq!(normalize_canonical("\\\\?\\UNC\\host\\share\\x"), None);
        assert_eq!(normalize_canonical("\\\\host\\share\\x"), None);
        assert_eq!(normalize_canonical("\\\\?\\Volume{01234567-89ab-cdef-0123-456789abcdef}\\x"), None);
        assert_eq!(normalize_canonical("\\\\?\\GLOBALROOT\\Device\\x"), None);
        assert_eq!(normalize_canonical("\\\\.\\pipe\\x"), None);
        // 접두 뒤가 비 ASCII 여도(바이트 경계) 패닉하지 않는다
        assert_eq!(normalize_canonical("\\\\?\\가나다"), None);
        assert_eq!(normalize_canonical("\\\\?\\U가"), None);
        // 접두 없는 일반 경로는 그대로
        assert_eq!(normalize_canonical("C:\\a").as_deref(), Some("C:\\a"));
        assert_eq!(normalize_canonical("/home/u").as_deref(), Some("/home/u"));
    }

    #[test]
    fn 선택한_실행_파일은_확장자_대소문자를_가리지_않는다() {
        assert!(check_chosen_exe("C:\\a\\Idea64.EXE", Platform::Win32, None).is_ok());
        assert!(check_chosen_exe("C:\\a\\x.com", Platform::Win32, None).is_ok());
        assert!(check_chosen_exe("C:\\a\\x.exe ", Platform::Win32, None).is_err());
        assert!(check_chosen_exe(&format!("/{}", "a".repeat(4096)), Platform::Linux, None).is_err());
    }
}
