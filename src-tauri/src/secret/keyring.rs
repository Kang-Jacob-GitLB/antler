//! OS 키체인 저장소(keyring-core). **키링 항목을 만드는 유일한 곳**이다(소스 검사가 강제한다).
//! - Windows: 자격 증명 관리자(windows-native-keyring-store)
//! - macOS: 로그인 키체인(apple-native-keyring-store, `keychain`)
//! - Linux: Secret Service — GNOME Keyring·KWallet(zbus-secret-service-keyring-store, 순수 Rust)
//!
//! 값은 `set_secret`/`get_secret` 으로 **바이트 그대로** 둔다(Windows `set_password` 는 UTF-16 으로 바꿔 한도가 반으로 줄어든다).
//! 서비스 이름은 [`super::service_name`] 이 운영·디버그를 가르고, 계정 이름은 세 OS 가 같다(`github-token`·`jira-token`).
//! keyring 의 오류는 문구에 값(`BadEncoding` 의 바이트)이 들 수 있어 어디에도 싣지 않고 [`SecretError`] 로만 바꾼다.
//! 키링을 열지 못하거나(Linux 에 D-Bus·Secret Service 없음) 쓰지 못하면 호출부([`super::open_store`]·`Vault`)가 **디스크에 쓰지 않고 메모리에만** 둔다.
use std::sync::Arc;

use keyring_core::{CredentialStore, Error as KeyError};
use zeroize::Zeroizing;

use super::{SecretError, SecretStore};

pub struct KeyringStore {
    store: Arc<CredentialStore>,
    service: String,
}

/// 이 OS 의 키링 저장소를 연다. Linux 는 여기서 세션 D-Bus 에 연결하므로 키링이 없으면 `Err` 다
#[cfg(windows)]
fn new_store() -> keyring_core::Result<Arc<CredentialStore>> {
    windows_native_keyring_store::Store::new().map(|s| s as Arc<CredentialStore>)
}

#[cfg(target_os = "macos")]
fn new_store() -> keyring_core::Result<Arc<CredentialStore>> {
    apple_native_keyring_store::keychain::Store::new().map(|s| s as Arc<CredentialStore>)
}

#[cfg(target_os = "linux")]
fn new_store() -> keyring_core::Result<Arc<CredentialStore>> {
    zbus_secret_service_keyring_store::Store::new().map(|s| s as Arc<CredentialStore>)
}

impl KeyringStore {
    /// 앱 서비스 이름(`com.antler.app`, 디버그 빌드는 `com.antler.app.dev`)으로 연다
    pub fn open() -> Result<Self, SecretError> {
        Self::with_service(super::service_name())
    }

    /// 서비스 이름을 지정해 연다(수동 시험이 고유 이름을 쓴다)
    pub fn with_service(service: &str) -> Result<Self, SecretError> {
        Ok(Self { store: new_store().map_err(|_| SecretError::Backend)?, service: service.to_string() })
    }

    fn entry(&self, account: &str) -> Result<keyring_core::Entry, SecretError> {
        // Windows 의 보존 방식은 Local(이 PC 의 이 사용자 로그온에만 남고 도메인 로밍을 타지 않는다). 기본값 Enterprise 는 로밍된다.
        // 값을 쓸 때만 적용되며 이미 있는 항목을 읽는 데는 영향이 없다. macOS·Linux 는 수정자가 없다
        #[cfg(windows)]
        let mods = std::collections::HashMap::from([("persistence", "Local")]);
        #[cfg(windows)]
        let mods = Some(&mods);
        #[cfg(not(windows))]
        let mods = None;
        self.store.build(&self.service, account, mods).map_err(|_| SecretError::Backend)
    }
}

impl SecretStore for KeyringStore {
    fn get(&self, account: &str) -> Result<Option<Zeroizing<Vec<u8>>>, SecretError> {
        match self.entry(account)?.get_secret() {
            Ok(v) => Ok(Some(Zeroizing::new(v))),
            Err(KeyError::NoEntry) => Ok(None),
            Err(_) => Err(SecretError::Backend),
        }
    }

    fn set(&self, account: &str, value: &[u8]) -> Result<(), SecretError> {
        if value.len() > super::MAX_BLOB {
            return Err(SecretError::TooLarge);
        }
        self.entry(account)?.set_secret(value).map_err(|_| SecretError::Backend)
    }

    fn delete(&self, account: &str) -> Result<(), SecretError> {
        match self.entry(account)?.delete_credential() {
            Ok(()) | Err(KeyError::NoEntry) => Ok(()),
            Err(_) => Err(SecretError::Backend),
        }
    }

    fn persistent(&self) -> bool {
        true
    }
}

/// 실제 자격 증명 관리자에 쓰는 수동 시험 — `cargo test --lib -- --ignored secret::keyring`. 고유한 서비스 이름
/// (`com.antler.test.<pid>-<시각>`)만 쓰고 끝나면(실패해도) 지운다. 사용자의 실제 항목(`com.antler.app*`)은 건드리지 않는다
#[cfg(windows)]
#[cfg(test)]
mod manual {
    use super::*;

    struct Cleanup(KeyringStore, &'static [&'static str]);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            for a in self.1 {
                let _ = self.0.delete(a);
            }
        }
    }

    // blob 상한(2561 실패)·UTF-16 대조는 Windows 자격 증명 관리자 실측이다. macOS·Linux 의 수동 시험은 사람이 한다(docs/tauri.md)
    #[test]
    #[ignore = "실제 Windows 자격 증명 관리자를 쓴다"]
    fn 자격_증명_blob_상한과_인코딩을_잰다() {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let service = format!("com.antler.test.{}-{nanos}", std::process::id());
        assert!(!service.starts_with("com.antler.app"));
        let k = Cleanup(KeyringStore::with_service(&service).unwrap(), &["blob", "text", "pwd"]);
        let raw = &k.0;

        // 한도: 우리 `set` 의 MAX_BLOB 검사를 거치지 않도록 keyring 의 `set_secret` 을 직접 불러 윈도우가 받아 주는 크기를 본다
        let e = raw.entry("blob").unwrap();
        let at = |n: usize| e.set_secret(&vec![b'a'; n]).is_ok();
        let (ok_2560, ok_2561) = (at(2560), at(2561));
        eprintln!("[실측] blob 2560바이트 저장={ok_2560}, 2561바이트 저장={ok_2561}");
        assert!(ok_2560 && !ok_2561);
        assert_eq!(e.get_secret().unwrap().len(), 2560, "2561 실패가 기존 값을 건드리면 안 된다");

        // 인코딩: 바이트를 그대로 저장하는가(UTF-8 "가나" = 6바이트) / set_password 는 UTF-16 으로 바꾸는가
        raw.set("text", "가나".as_bytes()).unwrap();
        let back = raw.get("text").unwrap().unwrap();
        eprintln!("[실측] set_secret(UTF-8 가나) → 읽은 길이 {} 바이트, 같은가={}", back.len(), back.as_slice() == "가나".as_bytes());
        assert_eq!(back.as_slice(), "가나".as_bytes());
        let p = raw.entry("pwd").unwrap();
        p.set_password("가나").unwrap();
        let utf16 = p.get_secret().unwrap();
        eprintln!("[실측] set_password(가나) → 읽은 길이 {} 바이트(UTF-16LE)", utf16.len());
        assert_eq!(utf16.len(), 4);
        assert!(p.set_password(&"a".repeat(1280)).is_ok() && p.set_password(&"a".repeat(1281)).is_err(), "set_password 한도는 UTF-16 로 2560바이트");

        // 우리 `set` 은 한도 초과를 윈도우에 보내기 전에 거부하고, 없는 항목 삭제는 오류가 아니다
        assert!(matches!(raw.set("blob", &vec![b'a'; 2561]), Err(SecretError::TooLarge)));
        raw.delete("blob").unwrap();
        assert!(raw.get("blob").unwrap().is_none());
        raw.delete("blob").unwrap();
    }
}
