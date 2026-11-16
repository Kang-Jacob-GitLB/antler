//! 토큰 보관(Phase 4 PR 4) — 앱에는 아직 연결하지 않는다. 원본은 `src/main/github/tokenStore.ts`(저장 방식)와 `src/main/jira/service.ts`(저장값 복원)다.
//!
//! - [`SecretStore`]: 이름 붙은 바이트 덩어리를 두고 꺼내는 저장소. [`keyring::KeyringStore`](Windows 자격 증명 관리자)와 [`memory::MemoryStore`] 가 있다.
//!   keyring-core·windows-native-keyring-store 는 이 모듈에서만 쓴다(소스 검사가 강제한다).
//! - [`Vault`]: 저장소 위에서 GitHub·Jira 자격 증명을 **비밀과 메타데이터를 한 덩어리 UTF-8 JSON** 으로 저장·복원한다(메타 파일은 없다).
//!   - GitHub `{method, token}`, Jira `{kind, siteUrl, email, token, displayName}`.
//!   - 읽은 값이 깨졌으면(JSON 실패·검증 실패) 그 항목을 지우고 `None`. Jira 는 `jira::validate::parse_connect_input` 으로 다시 검증한다.
//!   - 쓰기에 실패하거나 저장소가 남지 않는 종류(Windows 아님·`ANTLER_USER_DATA`)면 메모리에만 두고 `storage_unavailable` 이다(tokenStore.ts 와 같다).
//!   - 한 덩어리는 [`MAX_BLOB`] 바이트(Windows 자격 증명 blob 상한, 실측은 keyring.rs 의 수동 시험)를 넘지 못한다. Jira 표시 이름은 먼저 200자로 자른다.
//!   - 직렬화는 용량 [`MAX_BLOB`] 로 미리 잡은 버퍼에 재할당 없이 쓴다(넘으면 `TooLarge`). 재할당되면 토큰이 든 옛 버퍼가 0 으로 덮이지 않은 채 해제되기 때문이다.
//!   - GitHub 토큰은 `usable_token` 을 통과해야 저장한다(`InvalidToken`). 메모리로 떨어진 상태(`storage_unavailable`)는 GitHub·Jira 가 따로다.
//!   - `clear_*` 는 키체인 삭제 실패를 `Err` 로 돌려준다 — 호출부가 사용자에게 보여 줘야 한다.
//! - 값이 든 버퍼는 [`zeroize::Zeroizing`]·[`SecretString`] 이 버릴 때 0 으로 덮는다. 오류에는 값을 싣지 않는다.
//! - 이 모듈은 로그를 남기지 않는다(소스 검사가 `println!`·`eprintln!`·`dbg!`·`log` 를 막는다).
use std::collections::HashMap;
use std::ffi::OsStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde::Serialize;
use serde_json::Value;
use zeroize::{Zeroize, Zeroizing};

use crate::jira::validate::{parse_connect_input, ConnectInput, SiteOptions};
use crate::jira::JiraKind;
use crate::net::redact::{usable_token, SecretString};

#[cfg(windows)]
pub mod keyring;
pub mod memory;

/// 자격 증명 하나의 blob 상한(바이트) — Windows `CRED_MAX_CREDENTIAL_BLOB_SIZE`(5 × 512). 값은 UTF-8 JSON 바이트 그대로 저장한다
pub const MAX_BLOB: usize = 2560;
/// Jira 표시 이름은 이 글자 수로 먼저 자른다(예산 확보)
pub const MAX_DISPLAY_NAME_CHARS: usize = 200;

pub const GITHUB_ACCOUNT: &str = "github-token";
pub const JIRA_ACCOUNT: &str = "jira-token";

/// 저장소 오류. 키링 오류의 문구에는 값이 들 수 있어 싣지 않는다
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SecretError {
    /// 저장할 덩어리가 [`MAX_BLOB`] 를 넘는다
    TooLarge,
    /// 쓸 수 없는 토큰(빈 값·공백이나 NUL 이 든 값·너무 긴 값)이라 저장하지 않았다
    InvalidToken,
    /// 저장소가 값을 읽거나 쓰지 못했다
    Backend,
}

impl std::fmt::Display for SecretError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::TooLarge => "자격 증명이 너무 길어 안전 저장소에 저장할 수 없습니다(사이트 주소·이메일·토큰 길이를 확인하세요)",
            Self::InvalidToken => "쓸 수 없는 토큰이라 저장하지 않았습니다(공백이 있거나 비어 있는지 확인하세요)",
            Self::Backend => "안전 저장소에 접근하지 못했습니다",
        })
    }
}

impl std::error::Error for SecretError {}

/// 이름(계정) 붙은 바이트 덩어리 저장소
pub trait SecretStore: Send + Sync {
    /// 없으면 `Ok(None)`
    fn get(&self, account: &str) -> Result<Option<Zeroizing<Vec<u8>>>, SecretError>;
    fn set(&self, account: &str, value: &[u8]) -> Result<(), SecretError>;
    /// 없는 항목을 지워도 오류가 아니다
    fn delete(&self, account: &str) -> Result<(), SecretError>;
    /// 값이 프로세스를 넘어 남는가(키체인). false 면 호출부가 메모리에만 두고 `storage_unavailable` 로 알린다
    fn persistent(&self) -> bool;

    /// UTF-8 문자열 값을 [`SecretString`] 으로 꺼낸다. UTF-8 이 아니면 깨진 값으로 보고 `None` 이다
    fn get_secret_string(&self, account: &str) -> Result<Option<SecretString>, SecretError> {
        Ok(self.get(account)?.and_then(|v| std::str::from_utf8(&v).ok().map(SecretString::new)))
    }

    fn set_secret_string(&self, account: &str, value: &SecretString) -> Result<(), SecretError> {
        self.set(account, value.expose().as_bytes())
    }
}

/// 서비스 이름: 운영 `com.antler.app`, 디버그 빌드 `com.antler.app.dev`(개발 중에 실사용 항목을 건드리지 않게)
pub fn service_name() -> &'static str {
    if cfg!(debug_assertions) {
        "com.antler.app.dev"
    } else {
        "com.antler.app"
    }
}

/// 쓸 저장소를 고른다: `ANTLER_USER_DATA`(측정·자동화 실행)가 있거나 Windows 가 아니거나 키링을 열 수 없으면 메모리, 아니면 자격 증명 관리자
pub fn open_store(user_data: Option<&OsStr>) -> Arc<dyn SecretStore> {
    if user_data.is_none() {
        #[cfg(windows)]
        if let Ok(k) = keyring::KeyringStore::open() {
            return Arc::new(k);
        }
    }
    Arc::new(memory::MemoryStore::new())
}

/// 환경(`ANTLER_USER_DATA`)을 읽어 [`open_store`] 로 고른다
pub fn open_default_store() -> Arc<dyn SecretStore> {
    open_store(std::env::var_os("ANTLER_USER_DATA").as_deref())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GithubMethod {
    Pat,
    Device,
    GhCli,
}

impl GithubMethod {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pat => "pat",
            Self::Device => "device",
            Self::GhCli => "gh-cli",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        match s {
            "pat" => Some(Self::Pat),
            "device" => Some(Self::Device),
            "gh-cli" => Some(Self::GhCli),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct StoredGithub {
    pub token: SecretString,
    pub method: GithubMethod,
}

#[derive(Clone, Debug)]
pub struct StoredJira {
    pub input: ConnectInput,
    pub display_name: String,
}

#[derive(Serialize)]
struct GithubWire<'a> {
    method: &'a str,
    token: &'a str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct JiraWire<'a> {
    kind: &'a str,
    site_url: &'a str,
    email: &'a str,
    token: &'a str,
    display_name: &'a str,
}

/// [`MAX_BLOB`] 를 넘는 쓰기를 io 오류로 바꾸는 writer. `Vec` 가 재할당되면 토큰이 든 옛 버퍼가 0 으로 덮이지 않은 채 해제되므로,
/// 미리 잡은 용량 안에서만 쓰게 막는다
struct Capped<'a>(&'a mut Vec<u8>);

impl std::io::Write for Capped<'_> {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        if self.0.len() + data.len() > MAX_BLOB {
            return Err(std::io::Error::other("blob too large"));
        }
        self.0.extend_from_slice(data);
        Ok(data.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// `buf`(용량 [`MAX_BLOB`] 이상으로 미리 잡은 것)에 재할당 없이 직렬화한다. 넘치면 `TooLarge`(내용은 호출부가 버릴 때 0 으로 덮는다)
fn serialize_capped<T: Serialize>(v: &T, buf: &mut Vec<u8>) -> Result<(), SecretError> {
    serde_json::to_writer(Capped(buf), v).map_err(|_| SecretError::TooLarge)
}

/// 직렬화 결과를 재할당 없이(복사본이 남지 않게) 담는다. [`MAX_BLOB`] 를 넘으면 `TooLarge`
fn to_json<T: Serialize>(v: &T) -> Result<Zeroizing<Vec<u8>>, SecretError> {
    let mut buf = Zeroizing::new(Vec::with_capacity(MAX_BLOB));
    serialize_capped(v, &mut buf)?;
    Ok(buf)
}

/// 파싱한 JSON 의 문자열 값을 0 으로 덮는다(최상위 필드만 — 저장 형식이 평평하다)
fn scrub(v: &mut Value) {
    if let Value::Object(m) = v {
        for (_, x) in m.iter_mut() {
            if let Value::String(s) = x {
                s.zeroize();
            }
        }
    }
}

fn str_field<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(Value::as_str)
}

fn decode_github(bytes: &[u8]) -> Option<StoredGithub> {
    let mut v: Value = serde_json::from_slice(bytes).ok()?;
    let out = (|| {
        let method = GithubMethod::parse(str_field(&v, "method")?)?;
        let token = str_field(&v, "token").filter(|t| usable_token(t))?;
        Some(StoredGithub { token: SecretString::new(token), method })
    })();
    scrub(&mut v);
    out
}

fn decode_jira(bytes: &[u8]) -> Option<StoredJira> {
    let mut v: Value = serde_json::from_slice(bytes).ok()?;
    // 저장된 값도 다시 검증한다(옛 버전이 쓴 값이거나 바뀐 값일 수 있다) — `jira:connect` 와 같은 검증이다
    let out = parse_connect_input(&v, SiteOptions::default()).ok().map(|input| {
        let display_name = str_field(&v, "displayName").unwrap_or_default().to_string();
        StoredJira { input, display_name }
    });
    scrub(&mut v);
    out
}

/// 자격 증명 저장·복원. 저장소가 못 남기는 값은 메모리에만 둔다
pub struct Vault {
    store: Arc<dyn SecretStore>,
    /// 키체인에 쓰지 못할 때만 쓰는 메모리 보관분(앱을 끄면 다시 로그인)
    memory: Mutex<HashMap<&'static str, Zeroizing<Vec<u8>>>>,
    /// 계정별로, 마지막 쓰기가 실패해 메모리에만 있는가(한쪽 실패가 다른 쪽 상태를 바꾸지 않는다)
    degraded_github: AtomicBool,
    degraded_jira: AtomicBool,
}

impl Vault {
    pub fn new(store: Arc<dyn SecretStore>) -> Self {
        Self { store, memory: Mutex::default(), degraded_github: AtomicBool::new(false), degraded_jira: AtomicBool::new(false) }
    }

    fn degraded(&self, account: &str) -> &AtomicBool {
        if account == GITHUB_ACCOUNT {
            &self.degraded_github
        } else {
            &self.degraded_jira
        }
    }

    /// 지금 GitHub 토큰이 디스크(키체인)에 남지 않는 상태인가 — 저장소가 남지 않는 종류거나 GitHub 마지막 쓰기가 실패했다
    pub fn github_storage_unavailable(&self) -> bool {
        !self.store.persistent() || self.degraded_github.load(Ordering::SeqCst)
    }

    /// 지금 Jira 토큰이 디스크(키체인)에 남지 않는 상태인가 — 저장소가 남지 않는 종류거나 Jira 마지막 쓰기가 실패했다
    pub fn jira_storage_unavailable(&self) -> bool {
        !self.store.persistent() || self.degraded_jira.load(Ordering::SeqCst)
    }

    fn mem(&self) -> std::sync::MutexGuard<'_, HashMap<&'static str, Zeroizing<Vec<u8>>>> {
        self.memory.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn read(&self, account: &'static str) -> Option<Zeroizing<Vec<u8>>> {
        if let Some(v) = self.mem().get(account) {
            return Some(Zeroizing::new(v.to_vec()));
        }
        // 읽기 실패(접근 거부 등)는 "없음" 이다 — 깨진 값이 아니므로 지우지 않는다
        if self.store.persistent() {
            self.store.get(account).ok().flatten()
        } else {
            None
        }
    }

    /// 덩어리를 두고, 저장소에 남겼으면 true, 메모리에만 뒀으면 false
    fn write(&self, account: &'static str, bytes: &[u8]) -> Result<bool, SecretError> {
        if bytes.is_empty() {
            return Err(SecretError::Backend);
        }
        if bytes.len() > MAX_BLOB {
            return Err(SecretError::TooLarge);
        }
        let keep_in_memory = |this: &Self| {
            this.mem().insert(account, Zeroizing::new(bytes.to_vec()));
        };
        if !self.store.persistent() {
            keep_in_memory(self);
            return Ok(false);
        }
        match self.store.set(account, bytes) {
            Ok(()) => {
                self.mem().remove(account);
                self.degraded(account).store(false, Ordering::SeqCst);
                Ok(true)
            }
            Err(_) => {
                keep_in_memory(self);
                self.degraded(account).store(true, Ordering::SeqCst);
                Ok(false)
            }
        }
    }

    /// 항목을 메모리와 저장소에서 지운다. 저장소가 지우지 못해도 메모리는 비운다
    fn remove(&self, account: &'static str) -> Result<(), SecretError> {
        self.mem().remove(account);
        if self.store.persistent() {
            self.store.delete(account)
        } else {
            Ok(())
        }
    }

    // ── GitHub ───────────────────────────────────────────────────────────

    /// 저장된(또는 메모리의) GitHub 토큰. 없거나 깨졌으면 None(깨진 항목은 지운다)
    pub fn load_github(&self) -> Option<StoredGithub> {
        let bytes = self.read(GITHUB_ACCOUNT)?;
        let out = decode_github(&bytes);
        if out.is_none() {
            let _ = self.remove(GITHUB_ACCOUNT);
        }
        out
    }

    /// 저장한다. 키체인에 남겼으면 `Ok(true)`, 메모리에만 뒀으면 `Ok(false)`.
    /// 쓸 수 없는 토큰(`usable_token` 실패)은 저장하지 않고 `Err(InvalidToken)` 이다 — 읽을 때 걸러질 값을 두지 않는다
    pub fn save_github(&self, token: &SecretString, method: GithubMethod) -> Result<bool, SecretError> {
        if !usable_token(token.expose()) {
            return Err(SecretError::InvalidToken);
        }
        let json = to_json(&GithubWire { method: method.as_str(), token: token.expose() })?;
        self.write(GITHUB_ACCOUNT, &json)
    }

    /// 메모리 사본은 항상 비우고, 키체인 삭제가 실패하면 그 `Err` 를 그대로 돌려준다.
    /// 호출부는 이 오류를 삼키지 말고 사용자에게 보여 줘야 한다(로그아웃했는데 토큰이 키체인에 남아 있을 수 있다)
    pub fn clear_github(&self) -> Result<(), SecretError> {
        self.remove(GITHUB_ACCOUNT)
    }

    // ── Jira ─────────────────────────────────────────────────────────────

    /// 저장된(또는 메모리의) Jira 연결. 없거나 깨졌거나 검증을 통과하지 못하면 None(깨진 항목은 지운다)
    pub fn load_jira(&self) -> Option<StoredJira> {
        let bytes = self.read(JIRA_ACCOUNT)?;
        let out = decode_jira(&bytes);
        if out.is_none() {
            let _ = self.remove(JIRA_ACCOUNT);
        }
        out
    }

    /// 저장한다(표시 이름은 200자로 자른다). 한 덩어리가 [`MAX_BLOB`] 를 넘으면 `Err(TooLarge)` — 연결하지 않는다
    pub fn save_jira(&self, input: &ConnectInput, display_name: &str) -> Result<bool, SecretError> {
        let name: String = display_name.chars().take(MAX_DISPLAY_NAME_CHARS).collect();
        let kind = match input.kind {
            JiraKind::Cloud => "cloud",
            JiraKind::Server => "server",
        };
        let json = to_json(&JiraWire { kind, site_url: &input.site_url, email: &input.email, token: input.token.expose(), display_name: &name })?;
        self.write(JIRA_ACCOUNT, &json)
    }

    /// 메모리 사본은 항상 비우고, 키체인 삭제가 실패하면 그 `Err` 를 그대로 돌려준다.
    /// 호출부는 이 오류를 삼키지 말고 사용자에게 보여 줘야 한다(연결을 끊었는데 토큰이 키체인에 남아 있을 수 있다)
    pub fn clear_jira(&self) -> Result<(), SecretError> {
        self.remove(JIRA_ACCOUNT)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 직렬화는_재할당하지_않고_넘치면_too_large_이다() {
        // 경계: 정확히 MAX_BLOB 바이트(`{"method":"pat","token":"…"}`)는 담기고 버퍼의 포인터·용량이 그대로다
        let overhead = r#"{"method":"pat","token":""}"#.len();
        let fit = "a".repeat(MAX_BLOB - overhead);
        let mut buf: Vec<u8> = Vec::with_capacity(MAX_BLOB);
        let (ptr, cap) = (buf.as_ptr(), buf.capacity());
        assert!(serialize_capped(&GithubWire { method: "pat", token: &fit }, &mut buf).is_ok());
        assert_eq!(buf.len(), MAX_BLOB);
        assert_eq!((buf.as_ptr(), buf.capacity()), (ptr, cap), "재할당되면 옛 버퍼가 0 으로 덮이지 않은 채 해제된다");

        // 1바이트 넘치면 오류이고 역시 재할당하지 않는다(쓰다 만 내용은 호출부가 버릴 때 덮는다)
        let over = "a".repeat(MAX_BLOB - overhead + 1);
        let mut buf: Vec<u8> = Vec::with_capacity(MAX_BLOB);
        let (ptr, cap) = (buf.as_ptr(), buf.capacity());
        assert_eq!(serialize_capped(&GithubWire { method: "pat", token: &over }, &mut buf), Err(SecretError::TooLarge));
        assert_eq!((buf.as_ptr(), buf.capacity()), (ptr, cap));
        assert!(buf.len() <= MAX_BLOB);

        // to_json 도 같은 결과다
        assert_eq!(to_json(&GithubWire { method: "pat", token: &fit }).unwrap().len(), MAX_BLOB);
        assert_eq!(to_json(&GithubWire { method: "pat", token: &over }).err(), Some(SecretError::TooLarge));
    }
}
