//! 앱 업데이트의 호스트(Phase 6c PR 4): 서비스(`service.rs`)가 쓰는 [`Backend`] 의 운영 구현 — 네트워크(`net::http`)·서명 검증과 설치(`tauri-plugin-updater`)·설정(건너뛴 버전)·이벤트.
//!
//! **무결성의 흐름**(위조 설치본이 실행되지 않게 하는 한 줄기):
//! 1. 매니페스트(`latest.json`)는 우리 HTTP 클라이언트(`net::http`, [`Redirects::ReleaseAssets`])로 받고 [`check_manifest`] 가 검사한다 — 새 버전인가·URL 이 이 저장소의 **그 태그** 릴리스 자산인가·서명 형식.
//! 2. 설치본도 같은 클라이언트로 받는다(리다이렉트 허용 목록·300MB 상한·읽기 정지 제한). **검증 전에는 디스크에 쓰지 않는다** — 메모리에만 둔다.
//! 3. 검증은 플러그인의 `Updater::restore_update`(검사를 통과한 [`ManifestOffer`] 로 우리가 다시 만든 매니페스트 — 고른 플랫폼 키 하나·`target` 고정·**서명된 버전 일치 강제**) → `Update::verify`(minisign).
//! 4. 설치는 `Update::install` 이 **다시 검증한 뒤** 임시 파일을 만들어 설치 프로그램을 띄우고 프로세스를 끝낸다. 바이트 경로는 [`HostBackend`] 안의 하나뿐이다.
//!
//! **공개 키는 [`UPDATE_PUBKEY`] 한 곳이다**(믿는 키의 유일한 원천). 비어 있으면(`None`) **fail-closed**: 확인은 동작하지만 내려받기·설치는 시작하지 않고 설치 포트(`Engine::install`)는 절대 불리지 않는다.
//! 플러그인 설정은 `tauri.conf.json` 이 아니라 [`updater_plugin_config`] 가 만들어 `lib.rs` 가 넣는다(conf 에는 `plugins` 가 없다 — 시험이 강제). `configure_client`·플러그인의 다운로드·`check()` 는 쓰지 않는다
//! (HTTP 클라이언트는 `net::http` 한 곳이어야 리다이렉트 허용 목록·크기 상한이 우회되지 않는다 — 소스 검사가 강제한다).
//!
//! **수동 경로(macOS dmg·Linux deb, PR 4b)**: 1~3 은 같다(서명 검증은 같은 `Engine::verify`, 매니페스트 키만 `darwin-<arch>-dmg`·`linux-x86_64-deb`). 4 가 다르다 —
//! **`Engine::install` 을 부르지 않는다**(플러그인의 macOS·Linux 설치는 앱 번들·실행 파일을 직접 교체한다. 우리는 사용자가 설치하게 파일만 열어 준다).
//! 검증을 통과한 바이트를 앱 전용 폴더에 저장하고([`super::store`]) 기본 앱으로 연다(`OpenFile` 포트 — 운영 구현은 `lib.rs` 가 `external::launch::SystemDeps::open_file` 로 주입한다, 열기 실패는 오류). `install`(= 다시 열기)은 파일을 상한 읽기로 다시 읽어 **다시 검증한 뒤** 연다.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Runtime};
use tauri_plugin_updater::UpdaterExt;
use tokio_util::sync::CancellationToken;

use super::logic::{
    check_manifest_for, detect_target, fetch_release_notes, in_app_bundle, is_manifest_url, is_offerable, is_prerelease, is_release_asset_url, parse_release_response, prerelease_manifest_url, BundleMark, ManifestOffer,
    ReleaseNotes, Target, MAX_API_BYTES, MAX_INSTALLER_BYTES, MAX_MANIFEST_BYTES, RELEASE_LIST_PATH, REPO_NAME, REPO_OWNER, STABLE_MANIFEST_URL,
};
use super::service::{Backend, Deps, UpdateService, UpdateState};
use super::store::{read_stored, store_verified};
use crate::config::ConfigStore;
use crate::net::http::{classify, or_abort, read_capped, read_capped_progress, AssetError, Failure, HttpClient, HttpInitError, ReadError, Redirects};
use crate::net::json::Json;

/// 업데이트 서명의 **공개 키**(`tauri signer generate` 가 만든 `.pub` 파일 내용 — minisign 공개 키 텍스트를 base64 로 한 번 더 감싼 한 줄)다.
/// 사용자가 서명 키를 만들어 공개 키를 주면 **여기 한 줄만** `Some("…")` 로 바꾼다(방법·확인은 docs/updates.md). `None` 인 동안 이 빌드는 업데이트를 내려받지도 설치하지도 않는다(fail-closed)
pub const UPDATE_PUBKEY: Option<&str> = None;

/// 키가 없을 때의 오류 문구(내려받기·설치 단계)
pub const NO_KEY_MESSAGE: &str = "업데이트 서명 키가 설정되지 않았습니다. 이 빌드는 업데이트를 설치할 수 없습니다";

const API_BASE: &str = "https://api.github.com/repos";
const RATE_LIMITED: &str = "GitHub 요청 한도에 걸렸습니다. 잠시 후 다시 시도하세요";
const TOO_BIG: &str = "응답이 너무 큽니다";
const BAD_INFO: &str = "업데이트 정보가 올바르지 않습니다";
const NOT_STAGED: &str = "다시 확인한 뒤 시도하세요";
const OPEN_FAILED: &str = "설치 파일을 열지 못했습니다. 다운로드 폴더에서 직접 여세요";
const CANCELLED: &str = "취소되었습니다";

/// 업데이트 플러그인 설정(`plugins.updater`). `lib.rs` 가 `Context` 에 넣는다 — 공개 키는 인자(운영은 [`UPDATE_PUBKEY`], 비면 빈 문자열이고 [`PluginEngine`] 이 키 없음으로 막는다).
/// 엔드포인트는 비어 있다(우리가 매니페스트를 받고 플러그인은 `restore_update` 만 쓴다). `requireSignedVersion` 은 켠다(`restore_update` 가 어차피 강제한다), 다운그레이드는 허용하지 않는다
pub fn updater_plugin_config(pubkey: Option<&str>) -> Value {
    json!({ "pubkey": pubkey.unwrap_or(""), "endpoints": [], "requireSignedVersion": true, "allowDowngrades": false, "windows": { "installMode": "passive" } })
}

/// 번들러가 실행 파일에 심은 표식이 NSIS 인가 — 업데이트를 켜는 **유일한** 조건이다(환경 변수는 믿지 않는다). `--no-bundle` 개발·측정 빌드는 `None` 이라 꺼진다
pub fn bundle_is_nsis() -> bool {
    matches!(tauri::utils::platform::bundle_type(), Some(tauri::utils::config::BundleType::Nsis))
}

/// 번들러가 심은 표식을 순수 값으로 옮긴다(`detect_target` 의 입력)
pub fn bundle_mark() -> BundleMark {
    use tauri::utils::config::BundleType as B;
    match tauri::utils::platform::bundle_type() {
        Some(B::Nsis) => BundleMark::Nsis,
        Some(B::Deb) => BundleMark::Deb,
        Some(B::App | B::Dmg) => BundleMark::App,
        Some(_) => BundleMark::Other,
        None => BundleMark::Unknown,
    }
}

/// 이 빌드의 업데이트 대상. 번들 표식·OS·아키텍처·(macOS) 실행 파일 위치만 본다 — 환경 변수는 믿지 않는다. `None` 이면 확인하지 않는다
pub fn current_target() -> Option<Target> {
    let in_app = std::env::current_exe().is_ok_and(|p| in_app_bundle(&p));
    detect_target(std::env::consts::OS, std::env::consts::ARCH, bundle_mark(), in_app)
}

/// 개발 장치(`ANTLER_UPDATE_FAKE`)의 상태 이름. **개발·cdp 빌드에서만** 읽는다 — 배포 바이너리에는 이 이름 자체가 없다(PR 2 의 바이너리 가드가 확인한다)
#[cfg(any(debug_assertions, feature = "cdp"))]
pub fn fake_kind() -> Option<String> {
    std::env::var("ANTLER_UPDATE_FAKE").ok()
}

#[cfg(not(any(debug_assertions, feature = "cdp")))]
pub fn fake_kind() -> Option<String> {
    None
}

// ── 네트워크 ─────────────────────────────────────────────────────────────

/// 업데이트가 쓰는 HTTP 클라이언트 셋. 토큰을 보내지 않는다(GitHub 서비스의 클라이언트와 공유하지 않는다)
#[derive(Clone)]
pub struct UpdateNet {
    /// 매니페스트(작은 본문, 15초): 릴리스 자산 리다이렉트만
    manifest: HttpClient,
    /// GitHub API(릴리스 목록·노트): 리다이렉트 없음
    api: HttpClient,
    /// 설치본(큰 본문, 전체 시간 제한 없이 연결·읽기 정지만): 릴리스 자산 리다이렉트만
    download: HttpClient,
}

impl UpdateNet {
    pub fn production() -> Result<Self, HttpInitError> {
        Ok(Self {
            manifest: HttpClient::production(Redirects::ReleaseAssets)?,
            api: HttpClient::production(Redirects::None)?,
            download: HttpClient::production_download(Redirects::ReleaseAssets)?,
        })
    }

    /// 이미 만든 클라이언트로 조립한다(시험이 루프백 클라이언트를 준다)
    pub fn from_clients(manifest: HttpClient, api: HttpClient, download: HttpClient) -> Self {
        Self { manifest, api, download }
    }

    async fn get_manifest(&self, url: &str) -> Result<String, String> {
        let mut res = self.manifest.get_release_asset(url, is_manifest_url, None).await.map_err(asset_error_text)?;
        let bytes = read_capped(&mut res, MAX_MANIFEST_BYTES).await.map_err(read_error_text)?;
        String::from_utf8(bytes).map_err(|_| BAD_INFO.to_string())
    }

    /// GitHub API GET(토큰 없음). 상태 코드와 본문(상한 2MB)을 돌려준다 — 상태 해석은 호출자가 한다
    async fn api_get(&self, path: &str) -> Result<(u16, String), String> {
        let url = format!("{API_BASE}/{REPO_OWNER}/{REPO_NAME}{path}");
        let sent = self.api.get(&url).header("Accept", "application/vnd.github+json").send().await;
        let mut res = sent.map_err(|e| failure_text(classify(&e)).to_string())?;
        let status = res.status().as_u16();
        if !(200..300).contains(&status) {
            return Ok((status, String::new()));
        }
        let bytes = read_capped(&mut res, MAX_API_BYTES).await.map_err(read_error_text)?;
        Ok((status, String::from_utf8(bytes).map_err(|_| BAD_INFO.to_string())?))
    }

    async fn download_installer(&self, url: &str, progress: &(dyn Fn(f64) + Sync), cancel: &CancellationToken) -> Result<Vec<u8>, String> {
        let mut res = self.download.get_release_asset(url, is_release_asset_url, Some(cancel)).await.map_err(asset_error_text)?;
        let read = or_abort(
            Some(cancel),
            read_capped_progress(&mut res, MAX_INSTALLER_BYTES, |got, total| {
                if let Some(total) = total.filter(|t| *t > 0) {
                    progress(got as f64 * 100.0 / total as f64);
                }
            }),
        )
        .await;
        match read {
            Err(_) => Err("취소되었습니다".to_string()),
            Ok(r) => r.map_err(read_error_text),
        }
    }
}

fn failure_text(f: Failure) -> &'static str {
    match f {
        Failure::Timeout => "응답 시간이 초과되었습니다",
        Failure::Connect => "서버에 연결하지 못했습니다",
        Failure::Body => "응답을 읽지 못했습니다",
    }
}

fn read_error_text(e: ReadError) -> String {
    match e {
        ReadError::TooBig => TOO_BIG.to_string(),
        ReadError::Failed(f) => failure_text(f).to_string(),
    }
}

fn asset_error_text(e: AssetError) -> String {
    match e {
        AssetError::Forbidden => "허용되지 않은 다운로드 주소입니다".to_string(),
        AssetError::TooManyRedirects => "리다이렉트가 너무 많습니다".to_string(),
        AssetError::BadRedirect => "잘못된 리다이렉트입니다".to_string(),
        AssetError::Status(s) => format!("내려받기 실패 (HTTP {s})"),
        AssetError::Failed(f) => failure_text(f).to_string(),
        AssetError::Aborted => "취소되었습니다".to_string(),
    }
}

// ── 서명 검증·설치(플러그인) ─────────────────────────────────────────────

/// 서명 검증과 설치를 하는 쪽. 설치 포트(`install`)는 **검증을 통과한 바이트만** 받아야 하고, 구현은 한 번 더 검증한다
pub trait Engine: Send + Sync + 'static {
    /// 신뢰하는 공개 키가 있는가. 없으면 내려받기·설치를 시작하지 않는다(fail-closed)
    fn key_ready(&self) -> bool;
    /// minisign 서명과 **서명된 버전 = 매니페스트 버전**을 검증한다. 설치 포트가 아니다
    fn verify(&self, offer: &ManifestOffer, bytes: &[u8]) -> Result<(), String>;
    /// 다시 검증한 뒤 설치한다. 성공하면 프로세스가 끝난다(돌아오지 않는다)
    fn install(&self, offer: &ManifestOffer, bytes: &[u8]) -> Result<(), String>;
    /// `install` 이 되돌릴 수 없는 종료 정리를 이미 돌렸는가(돌린 뒤 실패하면 앱은 재시작이 필요하다)
    fn cleanup_ran(&self) -> bool {
        false
    }
}

/// 플러그인(`tauri-plugin-updater`)을 쓰는 운영 구현. `before_exit` 는 설치 프로그램을 띄운 뒤 `exit(0)` 하기 직전에 돈다(`RunEvent::Exit` 을 건너뛰므로 git 프로세스 트리 정리 등을 여기서 한다)
pub struct PluginEngine<R: Runtime> {
    app: AppHandle<R>,
    pubkey: Option<String>,
    before_exit: Arc<dyn Fn() + Send + Sync>,
    /// `before_exit` 가 불렸는가 — 설치 프로그램 실행이 실패해도 되돌릴 수 없는 정리는 이미 돌았다
    cleaned: Arc<std::sync::atomic::AtomicBool>,
}

impl<R: Runtime> PluginEngine<R> {
    /// `pubkey` 는 운영에서는 [`UPDATE_PUBKEY`] 다(시험만 시험 키를 생성자 인자로 준다 — 환경 변수로 바꾸는 길은 없다)
    pub fn new(app: AppHandle<R>, pubkey: Option<String>, before_exit: Arc<dyn Fn() + Send + Sync>) -> Self {
        Self { app, pubkey, before_exit, cleaned: Arc::default() }
    }

    /// 검사를 통과한 제안으로 `Update` 를 만든다. 매니페스트는 **우리가 다시 만든다** — 고른 플랫폼 키 하나만 담고 `target` 을 그 키로 고정해, 원문에 다른 항목이 섞여도 플러그인이 다른 것을 고를 수 없다.
    /// `restore_update` 는 서명된 버전 일치를 강제로 켠다(옛 서명 설치본에 새 버전 번호를 붙이는 공격 차단)
    fn restore(&self, offer: &ManifestOffer) -> Result<tauri_plugin_updater::Update, String> {
        const PREPARE_FAILED: &str = "업데이트를 준비하지 못했습니다";
        let key = self.pubkey.as_deref().ok_or_else(|| NO_KEY_MESSAGE.to_string())?;
        let endpoint = STABLE_MANIFEST_URL.parse().map_err(|_| PREPARE_FAILED.to_string())?;
        let (hook, cleaned) = (self.before_exit.clone(), self.cleaned.clone());
        let before_exit = move || {
            // 표식을 먼저 세운다 — 정리가 도중에 실패·중단돼도 "돌았다"로 본다
            cleaned.store(true, std::sync::atomic::Ordering::SeqCst);
            hook();
        };
        let updater = self
            .app
            .updater_builder()
            .endpoints(vec![endpoint])
            .map_err(|_| PREPARE_FAILED.to_string())?
            .pubkey(key)
            .target(offer.platform)
            .version_comparator(|current, release| is_offerable(&current.to_string(), &release.version.to_string()))
            .on_before_exit(before_exit)
            .build()
            .map_err(|_| PREPARE_FAILED.to_string())?;
        let mut platforms = serde_json::Map::new();
        platforms.insert(offer.platform.to_string(), json!({ "url": offer.url, "signature": offer.signature }));
        let manifest = json!({ "version": offer.version, "platforms": platforms });
        updater.restore_update(manifest).map_err(|_| PREPARE_FAILED.to_string())?.ok_or_else(|| PREPARE_FAILED.to_string())
    }
}

fn verify_error_text(e: &tauri_plugin_updater::Error) -> &'static str {
    use tauri_plugin_updater::Error as E;
    match e {
        E::Minisign(_) | E::Base64(_) | E::SignatureUtf8(_) => "업데이트 서명이 올바르지 않습니다",
        E::SignedVersionMismatch { .. } | E::MissingSignedVersion => "업데이트 서명의 버전이 맞지 않습니다",
        _ => "업데이트를 검증하지 못했습니다",
    }
}

fn install_error_text(e: &tauri_plugin_updater::Error) -> &'static str {
    use tauri_plugin_updater::Error as E;
    match e {
        E::Minisign(_) | E::Base64(_) | E::SignatureUtf8(_) | E::SignedVersionMismatch { .. } | E::MissingSignedVersion => verify_error_text(e),
        E::InvalidUpdaterFormat => "설치 파일 형식이 올바르지 않습니다",
        E::Io(_) => "설치 프로그램을 시작하지 못했습니다",
        _ => "설치를 시작하지 못했습니다",
    }
}

impl<R: Runtime> Engine for PluginEngine<R> {
    fn key_ready(&self) -> bool {
        self.pubkey.is_some()
    }

    fn verify(&self, offer: &ManifestOffer, bytes: &[u8]) -> Result<(), String> {
        let update = self.restore(offer)?;
        update.verify(bytes).map_err(|e| verify_error_text(&e).to_string())
    }

    fn install(&self, offer: &ManifestOffer, bytes: &[u8]) -> Result<(), String> {
        let update = self.restore(offer)?;
        update.install(bytes).map_err(|e| install_error_text(&e).to_string())
    }

    fn cleanup_ran(&self) -> bool {
        self.cleaned.load(std::sync::atomic::Ordering::SeqCst)
    }
}

// ── 백엔드 ───────────────────────────────────────────────────────────────

/// 검증을 통과한 설치본(메모리에만 있다)
struct Staged {
    offer: ManifestOffer,
    bytes: Arc<Vec<u8>>,
}

/// 설치 파일을 여는 포트(운영은 `lib.rs` 가 `external::launch::SystemDeps::open_file` 로 주입하고, 시험은 기록하는 가짜를 준다)
pub type OpenFile = Arc<dyn Fn(&Path) -> Result<(), String> + Send + Sync>;

/// 수동 경로의 저장·열기 설정
struct Manual {
    dir: PathBuf,
    open: OpenFile,
}

/// 저장해 연 설치 파일(수동 경로) — 다시 열기가 이 파일을 다시 검증한다
struct Delivered {
    offer: ManifestOffer,
    path: PathBuf,
}

pub struct HostBackend<E: Engine> {
    net: UpdateNet,
    engine: Arc<E>,
    current: String,
    target: Target,
    /// 수동 모드(dmg·deb)면 있다
    manual: Option<Manual>,
    /// 마지막 확인이 고른 제안
    offer: Mutex<Option<ManifestOffer>>,
    staged: Mutex<Option<Staged>>,
    delivered: Mutex<Option<Delivered>>,
}

impl<E: Engine> HostBackend<E> {
    /// Windows NSIS(앱 안 설치)
    pub fn new(net: UpdateNet, engine: Arc<E>, current: String) -> Self {
        Self { net, engine, current, target: Target::nsis(), manual: None, offer: Mutex::new(None), staged: Mutex::new(None), delivered: Mutex::new(None) }
    }

    /// 수동 경로(macOS dmg·Linux deb): 검증한 파일을 `dir` 에 저장하고 `open` 으로 연다. `target` 이 NSIS 면 `new` 와 같다
    pub fn with_target(net: UpdateNet, engine: Arc<E>, current: String, target: Target, dir: PathBuf, open: OpenFile) -> Self {
        let manual = target.is_manual().then_some(Manual { dir, open });
        Self { target, manual, ..Self::new(net, engine, current) }
    }

    /// 저장한 설치 파일을 연다(열기 실패는 고정 문구)
    fn open_delivered(m: &Manual, path: &Path) -> Result<(), String> {
        (m.open)(path).map_err(|_| OPEN_FAILED.to_string())
    }

    /// 수동 경로의 내려받기 마무리: 검증을 마친 바이트를 저장하고 연다
    async fn deliver(&self, m: &Manual, offer: ManifestOffer, bytes: Vec<u8>, cancel: &CancellationToken) -> Result<(), String> {
        // 검증 뒤에도 취소를 존중한다(저장·열기 전에)
        if cancel.is_cancelled() {
            return Err(CANCELLED.to_string());
        }
        let (dir, name) = (m.dir.clone(), offer.file_name().to_string());
        let path = tokio::task::spawn_blocking(move || store_verified(&dir, &name, &bytes)).await.map_err(|_| crate::update::store::SAVE_FAILED.to_string())??;
        *self.delivered.lock().unwrap_or_else(PoisonError::into_inner) = Some(Delivered { offer, path: path.clone() });
        Self::open_delivered(m, &path)
    }

    /// 수동 경로의 다시 열기: 저장한 파일을 다시 읽어 **다시 검증한 뒤** 연다(받은 뒤 파일이 바뀌었을 수 있다)
    fn reopen(&self, m: &Manual) -> Result<(), String> {
        let (offer, path) = {
            let d = self.delivered.lock().unwrap_or_else(PoisonError::into_inner);
            let d = d.as_ref().ok_or_else(|| NOT_STAGED.to_string())?;
            (d.offer.clone(), d.path.clone())
        };
        let bytes = read_stored(&path).map_err(|e| format!("다시 열기 전 검증 실패: {e}"))?;
        self.engine.verify(&offer, &bytes).map_err(|e| format!("다시 열기 전 검증 실패: {e}"))?;
        Self::open_delivered(m, &path)
    }

    /// 프리릴리스 판이 최근 릴리스 목록에서 매니페스트 주소를 고른다
    async fn prerelease_manifest(&self) -> Result<Option<String>, String> {
        let (status, body) = self.net.api_get(RELEASE_LIST_PATH).await?;
        match status {
            404 => return Ok(None),
            403 | 429 => return Err(RATE_LIMITED.to_string()),
            s if !(200..300).contains(&s) => return Err(format!("릴리스 정보를 가져오지 못했습니다 (HTTP {s})")),
            _ => {}
        }
        prerelease_manifest_url(&body, &self.current)
    }
}

impl<E: Engine> Backend for HostBackend<E> {
    async fn check(&self) -> Result<Option<String>, String> {
        *self.offer.lock().unwrap_or_else(PoisonError::into_inner) = None;
        *self.staged.lock().unwrap_or_else(PoisonError::into_inner) = None;
        *self.delivered.lock().unwrap_or_else(PoisonError::into_inner) = None;
        let url = if is_prerelease(&self.current) {
            match self.prerelease_manifest().await? {
                Some(url) => url,
                None => return Ok(None),
            }
        } else {
            STABLE_MANIFEST_URL.to_string()
        };
        let text = self.net.get_manifest(&url).await?;
        let Some(offer) = check_manifest_for(&text, &self.current, &self.target)? else { return Ok(None) };
        let version = offer.version.clone();
        *self.offer.lock().unwrap_or_else(PoisonError::into_inner) = Some(offer);
        Ok(Some(version))
    }

    async fn download(&self, version: &str, progress: &(dyn Fn(f64) + Sync), cancel: &CancellationToken) -> Result<(), String> {
        let offer = self.offer.lock().unwrap_or_else(PoisonError::into_inner).clone().filter(|o| o.version == version).ok_or_else(|| NOT_STAGED.to_string())?;
        // 키가 없으면 **아무것도 받지 않는다**(fail-closed)
        if !self.engine.key_ready() {
            return Err(NO_KEY_MESSAGE.to_string());
        }
        *self.staged.lock().unwrap_or_else(PoisonError::into_inner) = None;
        let bytes = self.net.download_installer(&offer.url, progress, cancel).await?;
        // 검증 전에는 디스크에 쓰지 않는다 — 메모리 바이트에 서명·서명된 버전을 건다. 실패하면 버린다
        self.engine.verify(&offer, &bytes)?;
        // 수동 경로(dmg·deb)는 설치 포트를 쓰지 않는다 — 검증한 파일을 저장해 열어 줄 뿐이다
        if let Some(m) = &self.manual {
            return self.deliver(m, offer, bytes, cancel).await;
        }
        *self.staged.lock().unwrap_or_else(PoisonError::into_inner) = Some(Staged { offer, bytes: Arc::new(bytes) });
        Ok(())
    }

    fn install(&self) -> Result<(), String> {
        if !self.engine.key_ready() {
            return Err(NO_KEY_MESSAGE.to_string());
        }
        // 수동 경로의 설치는 파일을 다시 여는 것이다(검증 뒤). 설치 포트에는 닿지 않는다
        if let Some(m) = &self.manual {
            return self.reopen(m);
        }
        let (offer, bytes) = {
            let staged = self.staged.lock().unwrap_or_else(PoisonError::into_inner);
            let s = staged.as_ref().ok_or_else(|| NOT_STAGED.to_string())?;
            (s.offer.clone(), s.bytes.clone())
        };
        self.engine.install(&offer, &bytes)
    }

    fn cleanup_ran(&self) -> bool {
        self.engine.cleanup_ran()
    }

    fn mode(&self) -> &'static str {
        self.target.mode()
    }

    fn file_name(&self) -> Option<String> {
        self.delivered.lock().unwrap_or_else(PoisonError::into_inner).as_ref().map(|d| d.offer.file_name().to_string())
    }

    async fn release_notes(&self, version: &str) -> Result<ReleaseNotes, String> {
        fetch_release_notes(version, |path| async move {
            let (status, body) = self.net.api_get(&path).await?;
            parse_release_response(status, &body)
        })
        .await
    }
}

// ── 앱 연결 ──────────────────────────────────────────────────────────────

/// 설정의 건너뛴 버전(없거나 문자열이 아니면 `None`)
pub fn read_skipped(store: &ConfigStore) -> Option<String> {
    let config = store.get();
    config.get("ui").and_then(|ui| ui.get("skippedUpdateVersion")).and_then(Json::as_str).map(str::to_string)
}

/// 건너뛴 버전을 설정에 쓴다. **`set_skipped_update_version` 을 부르는 곳은 운영 코드에서 이 함수 하나다**(`tests/update_security.rs` 가 소스로 강제한다) —
/// 호출자는 서비스의 `dismiss` 뿐이고, renderer 의 `config:setUi` 는 이 키를 거부한다
pub fn write_skipped(store: &ConfigStore, version: &str) -> Result<(), String> {
    store.set_skipped_update_version(version).map(|_| ())
}

/// 상태가 바뀔 때 renderer 로 `update:state` 이벤트를 낸다(사이드카의 `evt` 와 같은 모양)
pub fn state_sink<R: Runtime>(app: AppHandle<R>) -> Arc<dyn Fn(&UpdateState) + Send + Sync> {
    Arc::new(move |state| {
        let _ = app.emit("antler:evt", json!({ "name": "update:state", "payload": state }));
    })
}

pub type AppBackend<R> = HostBackend<PluginEngine<R>>;

/// 운영 서비스. 공개 키는 [`UPDATE_PUBKEY`], 사용 여부·방식은 [`current_target`](번들 표식 — NSIS 는 앱 안 설치, dmg·deb 는 `updates_dir` 에 저장해 열기), 개발 장치는 [`fake_kind`] 다. `start` 는 호출자가 한다(런타임 안에서)
pub fn build_service<R: Runtime>(
    app: AppHandle<R>,
    store: Arc<ConfigStore>,
    before_exit: Arc<dyn Fn() + Send + Sync>,
    updates_dir: PathBuf,
    open: OpenFile,
) -> Result<UpdateService<AppBackend<R>>, HttpInitError> {
    let current = app.package_info().version.to_string();
    let engine = Arc::new(PluginEngine::new(app.clone(), UPDATE_PUBKEY.map(str::to_string), before_exit));
    // 대상이 없으면(개발 빌드 등) 서비스는 꺼진 채 만든다 — 확인 0회, 수동 확인은 `not-available`
    let detected = current_target();
    let backend = Arc::new(HostBackend::with_target(UpdateNet::production()?, engine, current.clone(), detected.unwrap_or(Target::nsis()), updates_dir, open));
    let (read, write) = (store.clone(), store);
    Ok(UpdateService::new(
        backend,
        Deps {
            current_version: current,
            enabled: detected.is_some(),
            fake: fake_kind(),
            skipped: Arc::new(move || read_skipped(&read)),
            set_skipped: Arc::new(move |v| write_skipped(&write, v)),
            send: state_sink(app),
        },
    ))
}
