//! 외부 도구로 열기의 순수 계산부: 경로 검증과 실행 인자 구성(`src/main/external/plan.ts` 의 이식). 프로세스·파일 시스템을 건드리지 않는다.
//!
//! 원칙(TS 와 같다)
//! - 저장소 안의 파일 이름·경로는 "데이터"다. 항상 절대 경로로 만들어 넘기므로 옵션(`-x`, `--remote` …)으로 해석될 수 없다.
//! - 실행할 프로그램은 앱이 찾은 설치 위치나 사용자가 설정 화면에서 고른 파일뿐이다(저장소 내용이 정하지 못한다).
//! - 모든 실행은 인자 배열(셸 없음)이다. cmd.exe 를 쓸 때도 사용자 경로는 명령줄이 아니라 cwd 로 넘긴다.
//!
//! 정규식 크레이트가 없어 TS 의 정규식은 손으로 푼다(모두 ASCII 문자 부류라 JS 와 의미가 같다). 길이 상한은 JS `length` 와 같은 UTF-16 단위다.
//! 오류 문구는 TS 와 같은 문자열이다(골든이 비교한다).

use serde_json::Value;

use super::paths::{posix_basename, PathMod};
use crate::git::exec::ssh::win32_path as w32;
use crate::git::exec::wsl_paths::unc_from_wsl;

/// 실행 환경. TS `NodeJS.Platform` 중 앱이 구분하는 셋
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    Win32,
    Darwin,
    Linux,
}

impl Platform {
    /// 이 플랫폼의 로컬 경로 규칙
    pub fn path_mod(self) -> PathMod {
        if self == Platform::Win32 {
            PathMod::Win32
        } else {
            PathMod::Posix
        }
    }

    /// 지금 도는 플랫폼
    pub fn current() -> Platform {
        if cfg!(windows) {
            Platform::Win32
        } else if cfg!(target_os = "macos") {
            Platform::Darwin
        } else {
            Platform::Linux
        }
    }
}

/// 열 대상. 루트는 저장소 최상위(설정에서 온 값)다
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    Local { root: String },
    Wsl { root: String, distro: String },
    Ssh { root: String, host: String },
}

impl Target {
    pub fn root(&self) -> &str {
        match self {
            Target::Local { root } | Target::Wsl { root, .. } | Target::Ssh { root, .. } => root,
        }
    }
}

/// 에디터 종류(`EditorKind`)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditorKind {
    Vscode,
    Cursor,
    Jetbrains,
    Custom,
}

impl EditorKind {
    pub fn parse(s: &str) -> Option<EditorKind> {
        match s {
            "vscode" => Some(EditorKind::Vscode),
            "cursor" => Some(EditorKind::Cursor),
            "jetbrains" => Some(EditorKind::Jetbrains),
            "custom" => Some(EditorKind::Custom),
            _ => None,
        }
    }
}

/// 실행 계획. `cwd` 가 없으면 부모의 것을 쓴다
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Launch {
    pub file: String,
    pub args: Vec<String>,
    pub cwd: Option<String>,
}

pub type PlanResult<T> = Result<T, String>;

const MAX_PATH_LEN: usize = 4096;

fn utf16_len(s: &str) -> usize {
    s.encode_utf16().count()
}

/// 배포판 이름: `^[A-Za-z0-9][A-Za-z0-9._-]{0,99}$` — 옵션으로 해석되거나 인자를 가르는 문자를 갖지 못한다
fn distro_ok(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty() && b.len() <= 100 && b[0].is_ascii_alphanumeric() && b[1..].iter().all(|&c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-'))
}

/// 호스트: `^[A-Za-z0-9][A-Za-z0-9._@:-]{0,199}$`
fn host_ok(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty() && b.len() <= 200 && b[0].is_ascii_alphanumeric() && b[1..].iter().all(|&c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'@' | b':' | b'-'))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NameKind {
    Distro,
    Host,
}

/// WSL 배포판·SSH 호스트 이름 검사. 통과하면 같은 값을 돌려준다
pub fn assert_safe_name(kind: NameKind, v: &str) -> PlanResult<&str> {
    let ok = match kind {
        NameKind::Distro => distro_ok(v),
        NameKind::Host => host_ok(v),
    };
    if ok {
        Ok(v)
    } else {
        Err(match kind {
            NameKind::Distro => "잘못된 WSL 배포판 이름입니다",
            NameKind::Host => "잘못된 SSH 호스트입니다",
        }
        .to_string())
    }
}

/// `/^[A-Za-z]:/`
fn starts_with_drive(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':'
}

/// 저장소 루트 기준 상대 경로('/' 구분)를 구성 요소로 나눈다. 없으면(`null`·빈 문자열) 루트(빈 배열).
/// 절대 경로·`..`·NUL·(Windows 로컬) 역슬래시·콜론(드라이브·대체 스트림)은 거부한다. 문자열이 아닌 값도 거부한다(채널 입력이라 `Value` 다)
pub fn rel_segments(rel: &Value, target: &Target, platform: Platform) -> PlanResult<Vec<String>> {
    let rel = match rel {
        Value::Null => return Ok(Vec::new()),
        Value::String(s) if s.is_empty() => return Ok(Vec::new()),
        Value::String(s) => s,
        _ => return Err("잘못된 경로입니다".to_string()),
    };
    if utf16_len(rel) > MAX_PATH_LEN || rel.contains('\0') {
        return Err("잘못된 경로입니다".to_string());
    }
    if rel.starts_with('/') || starts_with_drive(rel) {
        return Err("저장소 밖의 경로입니다".to_string());
    }
    let win_local = matches!(target, Target::Local { .. }) && platform == Platform::Win32;
    if win_local && rel.contains(['\\', ':']) {
        return Err("잘못된 경로입니다".to_string());
    }
    let mut segs = Vec::new();
    for s in rel.split('/') {
        match s {
            "" | "." => {}
            ".." => return Err("저장소 밖의 경로입니다".to_string()),
            s => segs.push(s.to_string()),
        }
    }
    Ok(segs)
}

/// 대상 환경의 절대 경로(로컬은 OS 경로, WSL·SSH 는 그 환경의 POSIX 경로). 루트가 절대 경로가 아니면 거부
pub fn absolute_target(target: &Target, segs: &[String], platform: Platform) -> PlanResult<String> {
    let m = if matches!(target, Target::Local { .. }) { platform.path_mod() } else { PathMod::Posix };
    let root = target.root();
    if root.is_empty() || root.contains('\0') || !m.is_absolute(root) {
        return Err("저장소 경로가 절대 경로가 아닙니다".to_string());
    }
    if matches!(target, Target::Local { .. }) && platform == Platform::Win32 && is_double_sep(root) {
        return Err("네트워크 공유 경로는 지원하지 않습니다".to_string());
    }
    if segs.is_empty() {
        return Ok(m.normalize(root));
    }
    let mut parts: Vec<&str> = vec![root];
    parts.extend(segs.iter().map(String::as_str));
    Ok(m.join(&parts))
}

/// `/^[\\/]{2}/`
pub fn is_double_sep(s: &str) -> bool {
    let mut c = s.chars();
    matches!((c.next(), c.next()), (Some('\\' | '/'), Some('\\' | '/')))
}

/// 실제 경로 조회(심볼릭 링크를 모두 푼 경로). 없는 경로면 None
pub trait RealIo {
    fn realpath(&self, p: &str) -> Option<String>;
}

/// [`check_real_containment`] 의 결과
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Contained {
    pub exists: bool,
    pub existing: String,
    pub real: String,
    pub real_root: String,
}

/// 심볼릭 링크를 풀어도 저장소 루트 안에 있는지 확인한다. 대상이 없으면(삭제된 파일) 존재하는 가장 가까운 상위로 검사하고,
/// 그 상위 경로(`existing`)와 대상 자체가 존재하는지(`exists`)를 돌려준다. 루트를 풀 수 없으면 `Ok(None)`(호출자가 어휘적 검사만으로 갈지 정한다).
pub fn check_real_containment(root: &str, abs: &str, io: &dyn RealIo, m: PathMod) -> PlanResult<Option<Contained>> {
    let Some(real_root) = io.realpath(root) else {
        return Ok(None);
    };
    // 상대 경로를 계산할 수 없으면(절대 경로가 아님) 밖으로 본다
    let outside = |from: &str, to: &str| -> bool {
        match m.relative(from, to) {
            None => true,
            Some(rel) => {
                let sep = m.sep();
                rel == ".." || rel.starts_with(&format!("..{sep}")) || m.is_absolute(&rel)
            }
        }
    };
    let mut cur = abs.to_string();
    let mut exists = true;
    loop {
        if let Some(real) = io.realpath(&cur) {
            if outside(&real_root, &real) {
                return Err("저장소 밖을 가리키는 경로입니다(심볼릭 링크)".to_string());
            }
            return Ok(Some(Contained { exists, existing: cur, real, real_root }));
        }
        let parent = m.dirname(&cur);
        // 루트까지 올라가도 없으면 루트가 사라진 것
        if parent == cur || outside(root, &parent) {
            return Err("경로를 확인할 수 없습니다".to_string());
        }
        cur = parent;
        exists = false;
    }
}

/// 옵션으로 해석될 수 있는(`-` 로 시작하는) 경로는 넘기지 않는다. 절대 경로면 일어나지 않는 방어선
fn assert_not_option(p: &str) -> PlanResult<String> {
    if p.starts_with('-') || p.contains('\0') {
        return Err("잘못된 경로입니다".to_string());
    }
    Ok(p.to_string())
}

/// 에디터 실행 인자. `paths` 는 모두 절대 경로(대상 환경 기준)이고 첫 항목이 저장소 루트다.
/// - VS Code·Cursor: 로컬은 경로만, WSL 은 `--remote wsl+<distro>`, SSH 는 `--remote ssh-remote+<host>`
/// - JetBrains·사용자 지정: 경로만 넘긴다. WSL 은 UNC 로 바꿔 넘기고, SSH 는 지원하지 않는다
pub fn build_editor_launch(target: &Target, paths: &[String], editor: EditorKind, exe: &str, platform: Platform) -> PlanResult<Launch> {
    // `[...new Set(paths)]` — 중복은 첫 항목만 남긴다
    let mut uniq: Vec<&String> = Vec::new();
    for p in paths {
        if !uniq.contains(&p) {
            uniq.push(p);
        }
    }
    let list = uniq.into_iter().map(|p| assert_not_option(p)).collect::<PlanResult<Vec<_>>>()?;
    let launch = |args: Vec<String>| Launch { file: exe.to_string(), args, cwd: None };
    if matches!(editor, EditorKind::Vscode | EditorKind::Cursor) {
        return match target {
            Target::Wsl { distro, .. } => {
                let mut args = vec!["--remote".to_string(), format!("wsl+{}", assert_safe_name(NameKind::Distro, distro)?)];
                args.extend(list);
                Ok(launch(args))
            }
            Target::Ssh { host, .. } => {
                let mut args = vec!["--remote".to_string(), format!("ssh-remote+{}", assert_safe_name(NameKind::Host, host)?)];
                args.extend(list);
                Ok(launch(args))
            }
            Target::Local { .. } => Ok(launch(list)),
        };
    }
    match target {
        Target::Ssh { .. } => Err("SSH 저장소는 VS Code·Cursor 로만 열 수 있습니다".to_string()),
        Target::Wsl { distro, .. } => {
            if platform != Platform::Win32 {
                return Err("WSL 저장소는 Windows 에서만 열 수 있습니다".to_string());
            }
            let distro = assert_safe_name(NameKind::Distro, distro)?;
            let args = list.iter().map(|p| assert_not_option(&unc_from_wsl(distro, p))).collect::<PlanResult<Vec<_>>>()?;
            Ok(launch(args))
        }
        Target::Local { .. } => Ok(launch(list)),
    }
}

/// cmd.exe 를 거치는 WSL 터미널 대체 경로에서 허용하는 문자(셸 메타문자·% 확장을 원천 차단): `^\/[A-Za-z0-9_./+=,:@-]*$`
fn safe_cmd_path(s: &str) -> bool {
    s.starts_with('/') && s.bytes().skip(1).all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'.' | b'/' | b'+' | b'=' | b',' | b':' | b'@' | b'-'))
}

/// 터미널을 띄울 때 쓰는 외부 프로그램의 절대 경로.
/// 상대 이름(cmd.exe 등)은 cwd(=저장소 폴더)에서 먼저 찾아져 저장소가 심은 파일이 실행될 수 있으므로 모든 file 은 절대 경로여야 한다.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TerminalTools {
    /// Windows: 검증된 `%SystemRoot%`(없으면 `C:\Windows`)
    pub system_root: Option<String>,
    /// Windows Terminal 의 절대 경로(없으면 None)
    pub wt: Option<String>,
    /// 리눅스: 찾은 x-terminal-emulator 의 절대 경로
    pub linux_terminal: Option<String>,
    /// macOS: Terminal.app 의 절대 경로(번들 ID 조회는 저장소의 가짜 앱에 가로채일 수 있어 경로로 고정한다)
    pub mac_terminal: Option<String>,
}

/// JS 의 truthy 검사(빈 문자열은 거짓)
fn some(v: &Option<String>) -> Option<&str> {
    v.as_deref().filter(|s| !s.is_empty())
}

/// wt 는 인자를 다시 조합하며 `;` 를 구분자로, `"` 를 따옴표로 쓰므로 그런 문자나 제어 문자가 든 경로에는 쓰지 않는다
fn wt_safe(dir: &str) -> bool {
    !dir.chars().any(|c| c == ';' || c == '"' || ('\u{0}'..='\u{1f}').contains(&c) || c == '\u{7f}')
}

/// `cmd /c start` 로 넘기는 인자가 cmd 의 해석(`& | < > ^ % "`·제어 문자)에 걸리지 않는가. **TS 에 없는 추가 방어**다 —
/// 프로그램 경로(`%SystemRoot%` 에서 온다)는 `WIN_DRIVE_DIR_RE` 가 `& ^ %` 를 거르지 않아 `start` 뒤 명령줄에서 cmd 가 해석할 수 있었다
fn cmd_arg_safe(s: &str) -> bool {
    !s.chars().any(|c| matches!(c, '&' | '|' | '<' | '>' | '^' | '%' | '"') || c.is_control())
}

/// 터미널 실행. `dir` 은 대상 환경의 절대 경로(디렉터리). 모든 file 은 절대 경로다.
/// Windows: Windows Terminal(wt)이 있으면 wt, 없으면 cmd.exe 의 start 로 새 콘솔(경로는 cwd 로만 넘긴다).
/// WSL 은 `wsl.exe -d <distro> --cd <path>`. SSH 는 지원하지 않는다.
pub fn build_terminal_launch(target: &Target, dir: &str, platform: Platform, tools: &TerminalTools) -> PlanResult<Launch> {
    if matches!(target, Target::Ssh { .. }) {
        return Err("SSH 저장소는 터미널로 열 수 없습니다".to_string());
    }
    assert_not_option(dir)?;
    if platform == Platform::Win32 {
        let sys32 = w32::join(&[tools.system_root.as_deref().unwrap_or("C:\\Windows"), "System32"]);
        let cmd = w32::join(&[&sys32, "cmd.exe"]);
        let wt = some(&tools.wt).filter(|_| wt_safe(dir));
        if let Target::Wsl { distro, .. } = target {
            let distro = assert_safe_name(NameKind::Distro, distro)?;
            let wsl_args = vec![w32::join(&[&sys32, "wsl.exe"]), "-d".to_string(), distro.to_string(), "--cd".to_string(), dir.to_string()];
            if let Some(wt) = wt {
                return Ok(Launch { file: wt.to_string(), args: wsl_args, cwd: None });
            }
            if !safe_cmd_path(dir) {
                return Err("이 경로는 Windows Terminal 없이 열 수 없습니다(영문·숫자·._-/ 만 가능)".to_string());
            }
            // wsl.exe 의 경로도 cmd 가 해석한다
            if !cmd_arg_safe(&wsl_args[0]) {
                return Err("이 Windows 경로는 Windows Terminal 없이 열 수 없습니다".to_string());
            }
            let mut args: Vec<String> = ["/d", "/c", "start", ""].iter().map(|s| s.to_string()).collect();
            args.extend(wsl_args);
            return Ok(Launch { file: cmd, args, cwd: None });
        }
        if let Some(wt) = wt {
            return Ok(Launch { file: wt.to_string(), args: vec!["-d".to_string(), dir.to_string()], cwd: None });
        }
        if !cmd_arg_safe(&cmd) {
            return Err("이 Windows 경로는 Windows Terminal 없이 열 수 없습니다".to_string());
        }
        let args = ["/d", "/c", "start", ""].iter().map(|s| s.to_string()).chain([cmd.clone()]).collect();
        return Ok(Launch { file: cmd, args, cwd: Some(dir.to_string()) });
    }
    if matches!(target, Target::Wsl { .. }) {
        return Err("WSL 저장소는 Windows 에서만 열 수 있습니다".to_string());
    }
    if platform == Platform::Darwin {
        let Some(term) = some(&tools.mac_terminal) else {
            return Err("Terminal.app 을 찾을 수 없습니다".to_string());
        };
        return Ok(Launch { file: "/usr/bin/open".to_string(), args: vec!["-a".to_string(), term.to_string(), dir.to_string()], cwd: None });
    }
    let Some(term) = some(&tools.linux_terminal) else {
        return Err("터미널 프로그램(x-terminal-emulator)을 찾을 수 없습니다".to_string());
    };
    Ok(Launch { file: term.to_string(), args: Vec::new(), cwd: Some(dir.to_string()) })
}

/// macOS 에서 openPath·open 이 "열기"가 아니라 "실행"할 수 있는 디렉터리(번들: .app .xcodeproj .service …)를 이름 목록으로 막을 수 없어
/// 허용 방식으로 판정한다: 저장소 루트부터 대상까지의 **실제 경로(realpath)** 의 모든 구성 요소 이름에 '.' 이 없어야 일반 폴더다.
/// 점이 든 폴더는 번들일 수 있으므로 true(= 폴더로 열지 않고 보여 주기만 한다). root·p 는 둘 다 realpath 여야 한다.
pub fn has_dot_component(real_root: &str, p: &str) -> bool {
    // 상대 경로를 구할 수 없으면(절대 경로가 아님) 안전한 쪽(점 있음)으로 본다
    let Some(rel) = PathMod::Posix.relative(real_root, p) else {
        return true;
    };
    std::iter::once(posix_basename(real_root)).chain(rel.split('/').filter(|s| !s.is_empty()).map(String::from)).any(|c| c.contains('.'))
}

/// Windows 셸이 폴더 바로 가기(특수 폴더)로 해석하는 이름 확장: `name.{GUID}` (`/\.\{[0-9a-f-]{36}\}$/i`)
fn is_clsid_name(name: &str) -> bool {
    let c: Vec<char> = name.chars().collect();
    if c.len() < 39 {
        return false;
    }
    let tail = &c[c.len() - 39..];
    tail[0] == '.' && tail[1] == '{' && tail[38] == '}' && tail[2..38].iter().all(|&x| x.is_ascii_hexdigit() || x == '-')
}

/// `real_root` 에서 `p` 까지의 구성 요소(루트 포함) 중 이름이 `.{GUID}` 로 끝나는 첫 경로(그 구성 요소 자신의 경로). 없으면 None.
/// 이런 폴더는 일반 폴더로 보이지만 셸이 바로 가기로 다뤄 저장소 밖을 열 수 있다. Windows(win32 경로)용.
pub fn clsid_component(real_root: &str, p: &str) -> Option<String> {
    if is_clsid_name(&w32::basename(real_root)) {
        return Some(real_root.to_string());
    }
    let mut cur = real_root.to_string();
    let rel = PathMod::Win32.relative(real_root, p)?;
    for n in rel.split(['\\', '/']).filter(|s| !s.is_empty()) {
        cur = w32::join(&[&cur, n]);
        if is_clsid_name(n) {
            return Some(cur);
        }
    }
    None
}

/// 파일 관리자 동작: openPath 는 **realpath 가 폴더**이고(macOS 는 점 없는 일반 폴더)일 때만 쓴다 —
/// 파일·번들은 ShellExecute·open 이 실행해 버린다. 그 외에는 폴더에서 선택해 보여 준다(showItemInFolder).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RevealType {
    OpenPath,
    ShowItem,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RevealPlan {
    pub kind: RevealType,
    pub path: String,
}

/// `path`·`root` 는 realpath(WSL 은 UNC)이고, 대상이 없으면 존재하는 가장 가까운 상위 폴더다
pub struct RevealState<'a> {
    pub path: &'a str,
    pub is_dir: bool,
    pub root: &'a str,
}

pub fn build_reveal_plan(target: &Target, state: &RevealState, platform: Platform) -> PlanResult<RevealPlan> {
    match target {
        Target::Ssh { .. } => return Err("SSH 저장소는 파일 관리자로 열 수 없습니다".to_string()),
        Target::Wsl { distro, .. } => {
            if platform != Platform::Win32 {
                return Err("WSL 저장소는 Windows 에서만 열 수 있습니다".to_string());
            }
            assert_safe_name(NameKind::Distro, distro)?;
        }
        Target::Local { .. } => {}
    }
    if platform == Platform::Win32 {
        // 이름 확장 폴더(name.{CLSID})는 셸이 바로 가기로 다루므로 열지 않고 그 폴더를 부모에서 보여 준다
        if let Some(clsid) = clsid_component(state.root, state.path) {
            return Ok(RevealPlan { kind: RevealType::ShowItem, path: clsid });
        }
    }
    let plain = platform != Platform::Darwin || !has_dot_component(state.root, state.path);
    let kind = if state.is_dir && plain { RevealType::OpenPath } else { RevealType::ShowItem };
    Ok(RevealPlan { kind, path: state.path.to_string() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn 이름_정규식은_경계를_지킨다() {
        assert!(distro_ok("Ubuntu-22.04"));
        assert!(distro_ok(&"a".repeat(100)));
        assert!(!distro_ok(&"a".repeat(101)));
        for bad in ["", "-x", "--exec", "a b", "a\n", ".a", "a/b", "a\\b", "é"] {
            assert!(!distro_ok(bad), "{bad:?}");
        }
        assert!(host_ok("me@dev-box.example.com:22"));
        assert!(host_ok(&"a".repeat(200)));
        assert!(!host_ok(&"a".repeat(201)));
        for bad in ["-oProxyCommand=x", "h;rm", "h h", "", "h\n"] {
            assert!(!host_ok(bad), "{bad:?}");
        }
    }

    #[test]
    fn clsid_이름은_정확히_36자_guid_로_끝나야_한다() {
        let g = "{0AFACED1-E828-11D1-9187-B532F1E9575D}";
        assert!(is_clsid_name(&format!("fs.{g}")));
        assert!(is_clsid_name(&format!(".{g}")));
        assert!(is_clsid_name(&format!("x.{}", g.to_lowercase())));
        assert!(!is_clsid_name(&format!("fs{g}")));
        assert!(!is_clsid_name(&format!("fs.{g}.txt")));
        assert!(!is_clsid_name(&format!("fs.{}", g.replace('E', "G"))));
        assert!(!is_clsid_name("fs.{0AFACED1-E828-11D1-9187-B532F1E9575}"));
    }

    #[test]
    fn cmd_start_경로에_cmd_메타문자가_있으면_wt_없이는_거부한다() {
        let wsl = Target::Wsl { root: "/r".into(), distro: "U".into() };
        let local = Target::Local { root: "C:\\r".into() };
        for root in ["C:\\Win&dows", "C:\\Win^dows", "C:\\%X%"] {
            let tools = TerminalTools { system_root: Some(root.to_string()), ..Default::default() };
            assert!(build_terminal_launch(&wsl, "/r", Platform::Win32, &tools).is_err(), "{root}");
            assert!(build_terminal_launch(&local, "C:\\r", Platform::Win32, &tools).is_err(), "{root}");
        }
        // wt 가 있으면 cmd 를 거치지 않는다
        let tools = TerminalTools { system_root: Some("C:\\Win&dows".into()), wt: Some("C:\\L\\wt.exe".into()), ..Default::default() };
        assert!(build_terminal_launch(&local, "C:\\r", Platform::Win32, &tools).is_ok());
    }

    #[test]
    fn rel_segments_는_채널_입력의_형식도_본다() {
        let t = Target::Local { root: "/r".into() };
        assert_eq!(rel_segments(&json!(null), &t, Platform::Linux).unwrap(), Vec::<String>::new());
        assert!(rel_segments(&json!(42), &t, Platform::Linux).is_err());
        assert!(rel_segments(&json!(["a"]), &t, Platform::Linux).is_err());
        // UTF-16 단위로 센다: 4096 단위까지는 되고 넘으면 거부(이모지는 2단위)
        assert!(rel_segments(&json!("a".repeat(4096)), &t, Platform::Linux).is_ok());
        assert!(rel_segments(&json!("😀".repeat(2049)), &t, Platform::Linux).is_err());
        assert!(rel_segments(&json!("😀".repeat(2048)), &t, Platform::Linux).is_ok());
    }

    #[test]
    fn 길이가_넘치는_이름과_상위_이동은_거부() {
        let t = Target::Local { root: "C:\\r".into() };
        assert!(rel_segments(&json!("a/../../x"), &t, Platform::Win32).is_err());
        assert!(rel_segments(&json!("C:/x"), &t, Platform::Win32).is_err());
    }
}
