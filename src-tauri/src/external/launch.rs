//! 외부 열기의 운영 구현 [`SystemDeps`]: 프로세스 실행·파일 관리자 호출·파일 시스템·환경변수를 실제로 건드리는 **유일한 곳**이다.
//! 판단은 여기에 없다 — 무엇을 어떤 인자로 띄울지는 `plan.rs`·`opener.rs` 가 정하고(시험은 가짜 `OpenerDeps` 로 argv 만 본다),
//! 이 파일은 [`LaunchRequest`] 를 그대로 실행할 뿐이다. 소스 검사(tests/external.rs)가 `src/external/` 에서 `Command::new` 와 opener 플러그인 호출이
//! 이 파일에만 있음을 강제한다.
//!
//! 실행 규칙(TS `launch()` 와 같다): 분리된 프로세스(부모가 끝나도 살아 있다)·셸 없음(인자 배열만)·stdio 연결 없음·
//! `env_remove` 의 변수 제거·cwd 는 요청이 정할 때만. 인자는 `.arg()` 로만 넘긴다(문자열 조립·`raw_arg` 금지 — 인용은 표준 라이브러리 몫이다).

use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};

use super::opener::{normalize_canonical, restore_mapped_drive, LaunchRequest, OpenerDeps, Stat};
use super::plan::Platform;
use crate::fsguard::path_stays_local;

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
        if cfg!(windows) && !path_stays_local(Path::new(p)) {
            return None;
        }
        let real = fs::canonicalize(p).ok()?;
        let s = real.to_str()?;
        // Windows 의 canonicalize 는 `\\?\` 접두를 붙인다 — 벗기고, WSL 이 아닌 UNC·장치 경로는 없는 것으로 취급한다(거부)
        if cfg!(windows) {
            // 매핑된 네트워크 드라이브(`Z:\repo`)로 등록된 저장소는 canonicalize 가 UNC 로 풀어 거부되는데 Electron 판은 열린다 — 같은 공유 아래일 때만 드라이브 표기로 되돌린다(PR 9 정책)
            normalize_canonical(s).or_else(|| {
                let root = fs::canonicalize(p.get(..3)?).ok()?;
                restore_mapped_drive(p, s, root.to_str()?)
            })
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
