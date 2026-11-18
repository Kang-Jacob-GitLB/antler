//! 외부 열기의 운영 구현 [`SystemDeps`]: 프로세스 실행·파일 관리자 호출·파일 시스템·환경변수를 실제로 건드리는 **유일한 곳**이다.
//! 판단은 여기에 없다 — 무엇을 어떤 인자로 띄울지는 `plan.rs`·`opener.rs` 가 정하고(시험은 가짜 `OpenerDeps` 로 argv 만 본다),
//! 이 파일은 [`LaunchRequest`] 를 그대로 실행할 뿐이다. 소스 검사(tests/external.rs)가 `src/external/` 에서 `Command::new` 와 opener 플러그인 호출이
//! 이 파일에만 있음을 강제한다.
//!
//! 실행 규칙(TS `launch()` 와 같다): 분리된 프로세스(부모가 끝나도 살아 있다)·셸 없음(인자 배열만)·stdio 연결 없음·
//! `env_remove` 의 변수 제거·cwd 는 요청이 정할 때만. 인자는 `.arg()` 로만 넘긴다(문자열 조립·`raw_arg` 금지 — 인용은 표준 라이브러리 몫이다).

use std::collections::VecDeque;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use super::opener::{is_remote_link_target, normalize_canonical, LaunchRequest, OpenerDeps, Stat};
use super::plan::Platform;

/// 링크 사슬을 따라가는 횟수의 상한. 넘으면(순환 포함) 거부한다
const MAX_LINK_HOPS: usize = 16;

fn push_components(p: &Path, cur: &mut PathBuf, pending: &mut VecDeque<OsString>) {
    use std::path::Component;
    for c in p.components() {
        match c {
            Component::Prefix(_) | Component::RootDir => cur.push(c.as_os_str()),
            other => pending.push_back(other.as_os_str().to_owned()),
        }
    }
}

/// `canonicalize` **전에** 경로의 구성 요소마다 링크(심볼릭 링크·정션)인지 보고, 대상(`read_link`)이 `is_remote` 이면 따라가지 않고 false 를 돌려준다.
/// 상대·로컬 대상은 이어서 확인하고(링크 사슬), [`MAX_LINK_HOPS`] 를 넘으면 false 다. 읽을 수 없는 링크도 false(보수적). 없는 구성 요소는 링크가 아니다 —
/// 뒤따르는 `canonicalize` 가 없는 경로로 처리한다. 파일을 열지 않고 메타데이터·링크 대상만 읽는다(앱 실행 별칭 같은 다른 재해석 지점은 링크로 보지 않는다)
pub fn links_stay_local(p: &Path, is_remote: impl Fn(&str) -> bool) -> bool {
    let mut cur = PathBuf::new();
    let mut pending = VecDeque::new();
    push_components(p, &mut cur, &mut pending);
    let mut hops = 0;
    while let Some(c) = pending.pop_front() {
        if c == ".." {
            cur.pop();
            continue;
        }
        cur.push(&c);
        let Ok(meta) = fs::symlink_metadata(&cur) else { continue };
        if !meta.file_type().is_symlink() {
            continue;
        }
        let Ok(target) = fs::read_link(&cur) else { return false };
        let Some(t) = target.to_str() else { return false };
        // 정션의 대상은 NT 경로(`\??\…`)로 나올 수 있다 — Win32 의 `\\?\` 로 맞춘다
        let t = match t.strip_prefix("\\??\\") {
            Some(rest) => format!("\\\\?\\{rest}"),
            None => t.to_string(),
        };
        hops += 1;
        if hops > MAX_LINK_HOPS || is_remote(&t) {
            return false;
        }
        let joined = cur.parent().unwrap_or(Path::new("")).join(&t);
        cur.clear();
        let rest = std::mem::take(&mut pending);
        push_components(&joined, &mut cur, &mut pending);
        pending.extend(rest);
    }
    true
}

/// Windows 의 `DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP`(Node `detached: true` 가 쓰는 것과 같다). 콘솔을 물려주지 않고 부모의 Ctrl+C 를 받지 않는다
#[cfg(windows)]
const DETACHED_FLAGS: u32 = 0x0000_0008 | 0x0000_0200;

/// 실제 시스템을 쓰는 [`OpenerDeps`]
pub struct SystemDeps;

impl OpenerDeps for SystemDeps {
    fn platform(&self) -> Platform {
        Platform::current()
    }

    fn env(&self, name: &str) -> Option<String> {
        std::env::var(name).ok()
    }

    fn realpath(&self, p: &str) -> Option<String> {
        // `canonicalize` 는 링크를 따라가는 순간 원격 UNC 에 접속(NTLM 인증)하므로, 결과를 거르는 `normalize_canonical` 로는 늦다 — 먼저 링크 대상을 읽어 거른다
        if cfg!(windows) && !links_stay_local(Path::new(p), is_remote_link_target) {
            return None;
        }
        let real = fs::canonicalize(p).ok()?;
        let s = real.to_str()?;
        // Windows 의 canonicalize 는 `\\?\` 접두를 붙인다 — 벗기고, WSL 이 아닌 UNC·장치 경로는 없는 것으로 취급한다(거부)
        if cfg!(windows) {
            normalize_canonical(s)
        } else {
            Some(s.to_string())
        }
    }

    fn stat(&self, p: &str) -> Option<Stat> {
        fs::metadata(p).ok().map(|m| Stat { is_dir: m.is_dir(), is_file: m.is_file() })
    }

    fn lstat(&self, p: &str) -> Option<Stat> {
        fs::symlink_metadata(p).ok().map(|m| Stat { is_dir: m.is_dir(), is_file: m.is_file() })
    }

    fn spawn(&self, req: &LaunchRequest) -> Result<(), String> {
        // 에디터·터미널은 git 이 아니라 사용자가 고른 외부 프로그램이다 — git 실행기(src/git/exec)의 대상이 아니다.
        // 프로그램·인자는 plan.rs 가 모두 절대 경로로 만든 것이고 이 파일은 아무것도 정하지 않는다
        #[allow(clippy::disallowed_methods)]
        let mut cmd = Command::new(&req.file);
        cmd.args(&req.args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        if let Some(cwd) = &req.cwd {
            cmd.current_dir(cwd);
        }
        for name in &req.env_remove {
            cmd.env_remove(name);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(DETACHED_FLAGS);
        }
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            cmd.process_group(0);
        }
        let child = cmd.spawn().map_err(|e| e.to_string())?;
        // 기다리지 않는다. 유닉스는 끝난 자식이 좀비로 남지 않게 별도 스레드에서 수확한다
        #[cfg(unix)]
        std::thread::spawn(move || {
            let mut child = child;
            let _ = child.wait();
        });
        #[cfg(not(unix))]
        drop(child);
        Ok(())
    }

    fn open_path(&self, p: &str) -> Result<(), String> {
        tauri_plugin_opener::open_path(p, None::<&str>).map_err(|e| e.to_string())
    }

    fn show_item(&self, p: &str) -> Result<(), String> {
        tauri_plugin_opener::reveal_item_in_dir(p).map_err(|e| e.to_string())
    }
}
