//! Electron 설정 가져오기(Phase 6b PR 9, 사용자 결정 D1). Tauri 판 설정 파일이 **없을 때 한 번만**, Electron 판의 `antler.config.json` 을 **읽기만** 해서 같은 로드·정화를 거쳐 Tauri 경로에 새로 쓴다.
//!
//! - 가져오는 것: `sources`·`repos`(GitHub 저장소의 경로는 정화가 이 앱의 캐시 경로로 다시 계산한다 — 폴더는 가져오지 않는다)·`ui`(그대로 — `editorExePath` 는 쓸 때 `check_chosen_exe` 가 다시 검사한다)·`fetchApprovals`
//!   (같은 remote 지문일 때 자격 증명 도우미를 켜는 승인이라 위험이 낮다).
//! - **가져오지 않는 것**: `trustedConfigs`(명령을 실행할 수 있는 저장소 설정의 "항상 허용" — 이관 뒤 한 번 다시 묻는 편이 싸고 안전하다)·`window`(Tauri 는 window-state 플러그인이 맡는다)·토큰(결정 D2)·GitHub 캐시 폴더.
//! - Electron 파일은 **고치지도 백업하지도 않는다**(Electron 으로 되돌아갈 수 있다). 그래서 `ConfigStore::open` 이 아니라 `parse_config` 를 직접 쓴다 — `open` 은 손상된 파일 옆에 `.corrupt-*.bak` 를 쓴다.
//! - 손상(해석 실패)이거나 읽을 수 없으면 가져오지 않는다(빈 설정으로 시작한다). 경로는 호출자가 정한다(`ANTLER_USER_DATA` 로 도는 실행은 호출하지 않는다 — `lib.rs`).

use std::fs;
use std::path::{Path, PathBuf};

use crate::net::json::Json;

use super::load::parse_config;
use super::model::remove;
use super::save::{check_document_depth, to_file_text, write_atomic};

/// 가져오기에서 빼는 최상위 키
const EXCLUDED_KEYS: [&str; 2] = ["trustedConfigs", "window"];

/// Electron 판의 설정 파일 위치(`%APPDATA%\Antler\antler.config.json`). `appdata` 는 `APPDATA` 환경 변수 값이다
pub fn electron_config_file(appdata: Option<&std::ffi::OsStr>) -> Option<PathBuf> {
    let base = appdata.filter(|v| !v.is_empty())?;
    Some(Path::new(base).join("Antler").join("antler.config.json"))
}

/// 가져왔으면 `true`. Tauri 설정이 이미 있으면(폴더·손상 파일 포함) 아무것도 하지 않는다. 쓰기 실패는 `Err`(호출자는 로그만 남기고 빈 설정으로 시작한다)
pub fn import_electron_config(tauri_file: &Path, electron_file: &Path, cache_dir: &str) -> Result<bool, String> {
    // 있는지만 본다(링크를 따라가도 좋다 — 내용을 읽지 않는다). 없을 때만 가져온다
    if fs::symlink_metadata(tauri_file).is_ok() {
        return Ok(false);
    }
    // 읽기만 한다. 파일이 없거나 읽을 수 없으면 가져올 것이 없다
    let Ok(bytes) = fs::read(electron_file) else { return Ok(false) };
    let text = String::from_utf8_lossy(&bytes);
    let Ok(mut config) = parse_config(&text, Some(cache_dir)) else { return Ok(false) };
    for key in EXCLUDED_KEYS {
        remove(&mut config, key);
    }
    let doc = Json::Obj(config);
    check_document_depth(&doc)?;
    write_atomic(tauri_file, &to_file_text(&doc)).map_err(|e| format!("설정 파일을 쓰지 못했습니다: {e}"))?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::model::js_get;
    use crate::net::json::parse;

    struct Dir(PathBuf);
    impl Dir {
        fn new(tag: &str) -> Dir {
            let p = std::env::temp_dir().join(format!("antler-import-{tag}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&p);
            fs::create_dir_all(&p).unwrap();
            Dir(p)
        }
    }
    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    const ELECTRON: &str = r#"{"version":1,"sources":[{"id":"local","kind":"local","label":"Local"}],"repos":[{"id":"r1","sourceId":"local","path":"C:\\a","name":"a"}],"ui":{"theme":"dark","lastRepoId":"r1"},"fetchApprovals":{"r1":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"},"trustedConfigs":{"r1":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"},"window":{"x":1,"y":2,"width":800,"height":600}}"#;

    #[test]
    fn 없을_때만_가져오고_trusted_와_window_는_뺀다_electron_파일은_그대로다() {
        let d = Dir::new("ok");
        let (electron, tauri) = (d.0.join("electron.json"), d.0.join("tauri").join("antler.config.json"));
        fs::create_dir_all(tauri.parent().unwrap()).unwrap();
        fs::write(&electron, ELECTRON).unwrap();
        assert!(import_electron_config(&tauri, &electron, "C:/cache").unwrap());
        let got = parse(&fs::read_to_string(&tauri).unwrap()).unwrap();
        assert!(js_get(&got, "trustedConfigs").is_none() && js_get(&got, "window").is_none());
        assert!(js_get(&got, "fetchApprovals").is_some());
        assert_eq!(js_get(js_get(&got, "ui").unwrap(), "lastRepoId").and_then(Json::as_str), Some("r1"));
        assert_eq!(fs::read_to_string(&electron).unwrap(), ELECTRON, "Electron 파일은 읽기만 한다");
        // 두 번째부터는 Tauri 설정이 있으므로 가져오지 않는다 — 사용자가 바꾼 설정을 덮지 않는다
        fs::write(&tauri, "{}").unwrap();
        assert!(!import_electron_config(&tauri, &electron, "C:/cache").unwrap());
        assert_eq!(fs::read_to_string(&tauri).unwrap(), "{}");
    }

    #[test]
    fn 손상되었거나_없는_electron_파일은_가져오지_않고_옆에_아무것도_만들지_않는다() {
        let d = Dir::new("bad");
        let (electron, tauri) = (d.0.join("electron.json"), d.0.join("antler.config.json"));
        assert!(!import_electron_config(&tauri, &electron, "C:/cache").unwrap());
        fs::write(&electron, "{ 손상").unwrap();
        assert!(!import_electron_config(&tauri, &electron, "C:/cache").unwrap());
        assert!(!tauri.exists());
        let names: Vec<_> = fs::read_dir(&d.0).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(names, ["electron.json"], "백업 파일(.corrupt-*.bak)을 만들지 않는다");
    }

    #[test]
    fn github_저장소의_경로는_이_앱의_캐시로_다시_계산된다() {
        let d = Dir::new("gh");
        let (electron, tauri) = (d.0.join("e.json"), d.0.join("t.json"));
        let text = r#"{"version":1,"sources":[{"id":"local","kind":"local","label":"Local"},{"id":"github","kind":"github","label":"GitHub"}],"repos":[{"id":"g","sourceId":"github","path":"D:\\old\\o\\r.git","name":"r","github":{"owner":"o","repo":"r"}}],"ui":{}}"#;
        fs::write(&electron, text).unwrap();
        assert!(import_electron_config(&tauri, &electron, "C:\\new\\cache").unwrap());
        let got = parse(&fs::read_to_string(&tauri).unwrap()).unwrap();
        let Some(Json::Arr(repos)) = js_get(&got, "repos") else { panic!("repos") };
        assert_eq!(js_get(&repos[0], "path").and_then(Json::as_str), Some("C:\\new\\cache\\o\\r.git"));
    }

    #[test]
    fn 위치는_appdata_아래_antler() {
        let f = electron_config_file(Some(std::ffi::OsStr::new("C:\\Users\\u\\AppData\\Roaming"))).unwrap();
        assert!(f.ends_with("Antler\\antler.config.json") || f.ends_with("Antler/antler.config.json"));
        assert!(electron_config_file(None).is_none() && electron_config_file(Some(std::ffi::OsStr::new(""))).is_none());
    }
}
