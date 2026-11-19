//! 메모리 저장소. 키링을 열 수 없거나(Linux 에 Secret Service 없음 등) `ANTLER_USER_DATA`(측정·자동화 실행)가 있을 때 쓰며, 앱을 끄면 사라진다.
//! 디스크에는 아무것도 쓰지 않는다(Electron 판 safeStorage 의 `basic_text` 처리와 같은 의미).
use std::collections::HashMap;
use std::sync::Mutex;

use zeroize::Zeroizing;

use super::{SecretError, SecretStore};

#[derive(Default)]
pub struct MemoryStore {
    items: Mutex<HashMap<String, Zeroizing<Vec<u8>>>>,
    /// 키체인처럼 "남는" 저장소 흉내인가. 운영에서는 항상 false(`new`) — 보관할 곳이 없다는 뜻이라 호출부가 메모리 보관 상태로 알린다
    persistent: bool,
}

impl MemoryStore {
    /// 운영용: 디스크에 남지 않는 저장소(`storage_unavailable`)
    pub fn new() -> Self {
        Self::default()
    }

    /// 키체인 대신 쓰는 프로세스 안의 저장소 — 저장·복원·손상 처리 경로를 키체인 없이 시험한다. 운영 경로는 쓰지 않는다
    pub fn standing_in_for_keychain() -> Self {
        Self { items: Mutex::default(), persistent: true }
    }
}

impl SecretStore for MemoryStore {
    fn get(&self, account: &str) -> Result<Option<Zeroizing<Vec<u8>>>, SecretError> {
        let items = self.items.lock().map_err(|_| SecretError::Backend)?;
        Ok(items.get(account).map(|v| Zeroizing::new(v.to_vec())))
    }

    fn set(&self, account: &str, value: &[u8]) -> Result<(), SecretError> {
        self.items.lock().map_err(|_| SecretError::Backend)?.insert(account.to_string(), Zeroizing::new(value.to_vec()));
        Ok(())
    }

    fn delete(&self, account: &str) -> Result<(), SecretError> {
        self.items.lock().map_err(|_| SecretError::Backend)?.remove(account);
        Ok(())
    }

    fn persistent(&self) -> bool {
        self.persistent
    }
}
