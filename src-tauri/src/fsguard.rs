//! 링크 가드(공용): 링크(심볼릭 링크·정션)가 **원격(SMB/WebDAV)** 으로 이어지면 따라가지 않는다. 따라가는 순간(`stat`·`canonicalize`·`read_dir`·`git -C`) Windows 가
//! 인증(NTLM)을 시도해 해시가 공격자 서버로 샌다 — 이후 "저장소 밖"이라 거부해도 이미 늦으므로 **따라가기 전에** 링크 대상 문자열만 읽어 판정한다.
//! TS 판은 `src/main/git/linkGuard.ts`(`isRemoteLinkTarget`·`linksStayLocal`)다.
//!
//! 판정이 두 곳에 있으면 한쪽이 틀어진다(`\??\GLOBALROOT\Device\Mup\…` 처럼 한쪽만 놓친 모양이 실제로 있었다) — 외부 열기·폴더 선택·폴더 찾기·환경 채널이 모두 여기를 쓴다.
//! 소스 검사(tests/external.rs)가 이 파일 밖에 같은 이름의 구현이 없음을 강제한다.

use std::collections::VecDeque;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use crate::git::exec::wsl_paths::is_wsl_unc;

/// 링크 사슬을 따라가는 횟수의 상한. 넘으면(순환 포함) 거부한다
pub const MAX_LINK_HOPS: usize = 16;

/// 루트가 원격으로 이어지는 링크를 지날 때의 오류 문구(`mirror::check_path` 의 네트워크 공유 문구와 같은 관례)
pub const REMOTE_LINK_ROOT: &str = "네트워크 공유 경로는 지원하지 않습니다(링크가 네트워크 공유로 이어지거나 너무 깊게 이어집니다)";

/// `X:` 또는 `X:\…`(구분자는 `\` 로 맞춘 뒤). `X:foo`(드라이브 상대)는 아니다
fn is_drive_path(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b.len() == 2 || b[2] == b'\\')
}

/// 링크 대상 문자열이 따라가면 안 되는 **원격·장치 경로**인가(순수 — 허용 목록이다). **로컬로 인정하는 것만** 나열한다:
/// - 드라이브 문자 경로 `X:\…`·`X:/…`, 확장 접두가 붙은 `\\?\X:\…`·정션의 NT 접두 `\??\X:\…`
/// - WSL 공유 `\\wsl.localhost\…`·`\\wsl$\…`(`\\?\UNC\wsl…`·`\??\UNC\wsl…` 포함)
/// - 상대 경로·`\work`·`/usr/x` 같이 `\\`·접두가 없는 것(대상 드라이브·공유를 바꾸지 못한다)
///
/// 그 밖의 `\\…`·`//…`(UNC·WebDAV `host@SSL`)·`\??\…`·`\\?\…`(`UNC\`, `Volume{…}`, `GLOBALROOT\Device\Mup\…` 등)·`\\.\…` 는 전부 원격이다.
/// 사례표 `tests/fixtures/repos/link-target.cases.json`(TS 시험도 읽는다)과 Rust 전용 사례가 이 판정을 시험한다
pub fn is_remote_link_target(target: &str) -> bool {
    let t = target.trim().replace('/', "\\");
    if let Some(rest) = t.strip_prefix("\\??\\").or_else(|| t.strip_prefix("\\\\?\\")) {
        if is_drive_path(rest) {
            return false;
        }
        if rest.get(..4).is_some_and(|h| h.eq_ignore_ascii_case("UNC\\")) {
            return !is_wsl_unc(&format!("\\\\{}", &rest[4..]));
        }
        return true;
    }
    if is_drive_path(&t) {
        return false;
    }
    t.starts_with("\\\\") && !is_wsl_unc(&t)
}

fn push_components(p: &Path, cur: &mut PathBuf, pending: &mut VecDeque<OsString>) {
    use std::path::Component;
    for c in p.components() {
        match c {
            Component::Prefix(_) | Component::RootDir => cur.push(c.as_os_str()),
            other => pending.push_back(other.as_os_str().to_owned()),
        }
    }
}

/// 경로를 **따라가기 전에** 구성 요소마다 링크(심볼릭 링크·정션)인지 보고, 대상(`read_link`)이 `is_remote` 이면 따라가지 않고 false 를 돌려준다.
/// 상대·로컬 대상은 이어서 확인하고(링크 사슬 — `b -> a\x`, `a -> \\evil\share` 처럼 **중간 구성 요소**의 링크도 잡는다), [`MAX_LINK_HOPS`] 를 넘으면 false 다.
/// 읽을 수 없는 링크도 false(보수적). 없는 구성 요소는 링크가 아니다 — 뒤따르는 호출이 없는 경로로 처리한다.
/// 파일을 열지 않고 메타데이터·링크 대상만 읽는다(앱 실행 별칭 같은 다른 재해석 지점은 링크로 보지 않는다)
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

/// [`links_stay_local`] 을 운영 판정 [`is_remote_link_target`] 으로 돌린다
pub fn path_stays_local(p: &Path) -> bool {
    links_stay_local(p, is_remote_link_target)
}

/// 고른 폴더(루트)가 원격으로 이어지는 링크를 지나지 않는지 확인한다. 아니면 [`REMOTE_LINK_ROOT`]. 루트를 읽거나 `git -C` 를 돌리기 **전에** 부른다
pub fn check_local_root(path: &str, is_remote: impl Fn(&str) -> bool) -> Result<(), String> {
    if links_stay_local(Path::new(path), is_remote) {
        Ok(())
    } else {
        Err(REMOTE_LINK_ROOT.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(cases: &serde_json::Value, k: &str) -> Vec<String> {
        cases[k].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_string()).collect()
    }

    /// TS 판과 공유하는 사례표를 그대로 돌린다
    #[test]
    fn 링크_대상_판정이_ts_사례표와_같다() {
        let cases: serde_json::Value = serde_json::from_str(include_str!("../../tests/fixtures/repos/link-target.cases.json")).unwrap();
        let (remote, local) = (list(&cases, "remote"), list(&cases, "local"));
        assert!(remote.len() > 10 && local.len() > 5);
        for t in remote {
            assert!(is_remote_link_target(&t), "원격이어야 한다: {t:?}");
        }
        for t in local {
            assert!(!is_remote_link_target(&t), "로컬이어야 한다: {t:?}");
        }
    }

    /// Rust 전용 사례(사례표는 TS 시험과 함께 바꾼다 — TS 수정 PR 과 맞춘 뒤 옮긴다)
    #[test]
    fn 허용_목록_밖의_접두와_장치_경로는_모두_원격이다() {
        for t in [
            r"\??\GLOBALROOT\Device\Mup\evil\share",
            r"\\?\GLOBALROOT\Device\Mup\evil\share",
            r"\\?\globalroot\device\mup\evil",
            r"\??\Device\Mup\evil\share",
            r"\\?\Device\HarddiskVolume1\x",
            r"\??\Volume{01234567-89ab-cdef-0123-456789abcdef}\x",
            r"\??\UNC\host\share",
            r"\??\UNC\host\share\x",
            "//?/GLOBALROOT/Device/Mup/evil",
            "/??/GLOBALROOT/Device/Mup/evil",
            r"\\.\GLOBALROOT\Device\Mup\evil",
            r"\\.\UNC\host\share",
            r"\\host@SSL\DavWWWRoot\x",
            r"\\?\C",
            r"\\?\",
            r"\??\",
            r"\\?\C:foo",
            r"\\wsl.localhostevil\x",
            r"\\?\UNC\wsl.localhostevil\x",
            r"\\wsl.localhost",
            "  \\\\host\\s",
        ] {
            assert!(is_remote_link_target(t), "원격이어야 한다: {t:?}");
        }
    }

    #[test]
    fn 드라이브_상대_wsl_공유만_로컬이다() {
        for t in [
            r"C:\work\repo",
            "c:/work",
            "C:",
            r"\??\C:\work\repo",
            r"\\?\C:\work",
            r"\??\c:/work",
            "//?/C:/work",
            r"\\wsl.localhost\Ubuntu\home",
            r"\\wsl$\Ubuntu\x",
            "//wsl$/Ubuntu/x",
            r"\\?\UNC\wsl.localhost\Ubuntu\home",
            r"\??\UNC\wsl$\Ubuntu",
            "../sibling",
            r"..\other",
            "repo",
            r"\work",
            "/usr/x",
            "",
        ] {
            assert!(!is_remote_link_target(t), "로컬이어야 한다: {t:?}");
        }
    }

    #[test]
    fn 비_ascii_가_접두_뒤에_와도_패닉하지_않는다() {
        for t in [r"\\?\가나다", r"\??\U가", r"\\?\UN가", "가나다", r"\\가"] {
            let _ = is_remote_link_target(t);
        }
        assert!(is_remote_link_target(r"\\?\가나다"));
    }
}
