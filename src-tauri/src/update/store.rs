//! 수동 경로(macOS dmg·Linux deb)의 설치 파일 저장(Phase 6c PR 4b). **서명 검증을 통과한 바이트만** 받는다 — 검증 전에는 디스크에 쓰지 않는다(`host.rs` 가 순서를 지킨다).
//!
//! TS 의 `downloadVerified`(`manual.ts`)와 같은 점: 앱 전용 폴더(`<userData>/updates`)에 자산 이름 그대로 저장하고, 이름은 `is_safe_file_name` 을 다시 통과해야 하며(경로 구분자·`..` 없음),
//! 임시 이름(`<이름>.part`)에 새 파일로만(`create_new` — 이미 있으면, 링크 포함, 실패) 쓴 뒤 최종 이름으로 옮긴다.
//! 다른 점: **최종 이름에 이미 있는 파일은 덮어쓰지 않는다**(TS 는 지우고 바꿨다) — 일반 파일이고 내용이 같으면 그대로 쓰고(같은 버전을 다시 받는 경우), 아니면 건드리지 않고 오류다.
//! 옮기기는 `hard_link` 라 목적지가 있으면 원자적으로 실패한다(덮어쓰는 `rename` 이 아니다). 오류 문구에는 경로가 들어가지 않는다.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use super::logic::{is_safe_file_name, MAX_INSTALLER_BYTES};

pub const BAD_NAME: &str = "잘못된 파일 이름입니다";
pub const EXISTS: &str = "같은 이름의 파일이 이미 있어 덮어쓰지 않았습니다. 업데이트 폴더에서 그 파일을 지운 뒤 다시 시도하세요";
pub const SAVE_FAILED: &str = "설치 파일을 저장하지 못했습니다";
const TOO_BIG: &str = "설치 파일이 너무 큽니다";

/// `dir`(없으면 만든다 — 유닉스는 0700) 안에 `name` 으로 `bytes` 를 저장하고 경로를 돌려준다
pub fn store_verified(dir: &Path, name: &str, bytes: &[u8]) -> Result<PathBuf, String> {
    if !is_safe_file_name(name) {
        return Err(BAD_NAME.to_string());
    }
    make_dir(dir)?;
    let final_path = dir.join(name);
    if exists_same(&final_path, bytes)? {
        return Ok(final_path);
    }
    let part = dir.join(format!("{name}.part"));
    // 이전 실행이 남긴 임시 파일은 이 전용 폴더 안에서만 지운다(링크면 링크 자체가 지워진다)
    match fs::remove_file(&part) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err(SAVE_FAILED.to_string()),
    }
    let result = write_new(&part, bytes).and_then(|()| match fs::hard_link(&part, &final_path) {
        Ok(()) => Ok(()),
        // 그 사이에 같은 이름이 생겼다 — 같은 내용이면 그것을 쓴다
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => exists_same(&final_path, bytes).and_then(|same| if same { Ok(()) } else { Err(EXISTS.to_string()) }),
        Err(_) => Err(SAVE_FAILED.to_string()),
    });
    let _ = fs::remove_file(&part);
    result.map(|()| final_path)
}

/// 최종 이름이 이미 있는가. 일반 파일이고 내용이 같으면 `true`, 없으면 `false`, 다른 것(링크·폴더·다른 내용)이면 오류
fn exists_same(path: &Path, bytes: &[u8]) -> Result<bool, String> {
    let meta = match fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(_) => return Err(SAVE_FAILED.to_string()),
    };
    if !meta.file_type().is_file() || meta.len() != bytes.len() as u64 {
        return Err(EXISTS.to_string());
    }
    let mut have = Vec::with_capacity(bytes.len());
    File::open(path).and_then(|f| f.take(bytes.len() as u64 + 1).read_to_end(&mut have)).map_err(|_| SAVE_FAILED.to_string())?;
    if have == bytes {
        Ok(true)
    } else {
        Err(EXISTS.to_string())
    }
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut opts = OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(path).map_err(|_| SAVE_FAILED.to_string())?;
    f.write_all(bytes).and_then(|()| f.sync_all()).map_err(|_| SAVE_FAILED.to_string())
}

fn make_dir(dir: &Path) -> Result<(), String> {
    let mut b = fs::DirBuilder::new();
    b.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        b.mode(0o700);
    }
    b.create(dir).map_err(|_| SAVE_FAILED.to_string())
}

/// 저장한 파일을 다시 열기 전에 읽는다(상한 [`MAX_INSTALLER_BYTES`]). 일반 파일이 아니거나(링크·폴더) 상한을 넘으면 오류
pub fn read_stored(path: &Path) -> Result<Vec<u8>, String> {
    let meta = fs::symlink_metadata(path).map_err(|_| "설치 파일을 찾지 못했습니다".to_string())?;
    if !meta.file_type().is_file() {
        return Err("설치 파일이 일반 파일이 아닙니다".to_string());
    }
    if meta.len() > MAX_INSTALLER_BYTES as u64 {
        return Err(TOO_BIG.to_string());
    }
    let mut out = Vec::with_capacity(meta.len() as usize);
    File::open(path).and_then(|f| f.take(MAX_INSTALLER_BYTES as u64 + 1).read_to_end(&mut out)).map_err(|_| "설치 파일을 읽지 못했습니다".to_string())?;
    if out.len() > MAX_INSTALLER_BYTES {
        return Err(TOO_BIG.to_string());
    }
    Ok(out)
}
