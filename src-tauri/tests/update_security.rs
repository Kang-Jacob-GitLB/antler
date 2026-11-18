//! 앱 업데이트의 보안 시험(Phase 6c PR 4). **127.0.0.1 만** 쓴다 — 요청 주소는 `Rewrite` 로 루프백 서버에 보내고(판정은 원래 주소로 한다), 실제 GitHub·설치 프로그램·사용자 설정에 닿지 않는다.
//!
//! 서명은 **시험 전용 키쌍**이다: `tests/fixtures/update_keys/TEST-ONLY-*`(공개 키 둘·서명 넷). 비밀 키는 저장소에 없다(만들어 서명한 뒤 버렸다 — 서명은 `tauri signer sign` 의 실물이다).
//! 운영 공개 키(`update::host::UPDATE_PUBKEY`)와 무관하고, 시험 키는 **생성자 인자로만** 들어간다(환경 변수·설정으로 바꾸는 길은 없다).
//! 설치 포트(`Engine::install`)는 가짜다 — 진짜 설치 프로그램을 띄우지 않는다. 진짜 검증(`Update::verify`)은 가짜 앱(`tauri::test`)에 진짜 플러그인을 달아 돈다.
//! 서명 표본(바이트 `PAYLOAD`): `good` = 키 A·버전 2.0.0, `wrongkey` = 키 B·2.0.0, `oldver` = 키 A·1.9.9, `nover` = 키 A·버전 없음(옛 CLI 방식).

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use antler_lib::net::http::{AssetError, HttpClient, Redirects, Rewrite};
use antler_lib::update::host::{bundle_is_nsis, updater_plugin_config, Engine, HostBackend, OpenFile, PluginEngine, UpdateNet, NO_KEY_MESSAGE, UPDATE_PUBKEY};
use antler_lib::update::logic::{detect_target, is_manifest_url, is_release_asset_url, BundleMark, ManifestOffer, Target, STABLE_MANIFEST_URL};
use antler_lib::update::service::{Backend, Deps, Status, UpdateService, UpdateState};
use serde_json::{json, Value};
use tauri::test::MockRuntime;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_util::sync::CancellationToken;

// ── 시험 전용 표본 ───────────────────────────────────────────────────────────

const PAYLOAD: &[u8] = b"ANTLER-FAKE-INSTALLER-BYTES-FOR-TESTS-ONLY";
const PUBKEY_A: &str = include_str!("fixtures/update_keys/TEST-ONLY-pubkey-a.pub");
const PUBKEY_B: &str = include_str!("fixtures/update_keys/TEST-ONLY-pubkey-b.pub");
const SIG_GOOD: &str = include_str!("fixtures/update_keys/TEST-ONLY-sig-good.sig");
const SIG_WRONGKEY: &str = include_str!("fixtures/update_keys/TEST-ONLY-sig-wrongkey.sig");
const SIG_OLDVER: &str = include_str!("fixtures/update_keys/TEST-ONLY-sig-oldver.sig");
const SIG_NOVER: &str = include_str!("fixtures/update_keys/TEST-ONLY-sig-nover.sig");

const REPO: &str = "https://github.com/Kang-Jacob-GitLB/antler/releases/download";
const INSTALLER: &str = "https://github.com/Kang-Jacob-GitLB/antler/releases/download/v2.0.0/Antler-Setup-2.0.0-x64.exe";
/// 가짜 앱(`tauri::test`)의 현재 버전
const CURRENT: &str = "0.1.0";

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().expect("src-tauri 의 상위").to_path_buf()
}

fn read_src(rel: &str) -> String {
    // CI(Windows)의 체크아웃은 CRLF 일 수 있다 — 소스 검사는 LF 기준으로 본다
    fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}")).replace("\r\n", "\n")
}

/// 주석 줄을 뺀 코드
fn code(rel: &str) -> String {
    read_src(rel).lines().filter(|l| !l.trim_start().starts_with("//")).collect::<Vec<_>>().join("\n")
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in fs::read_dir(dir).expect("폴더") {
        let p = e.expect("항목").path();
        if p.is_dir() {
            rust_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

// ── 루프백 서버 ──────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
struct Req {
    /// `/<원래 호스트>/<경로>?<쿼리>` — 요청 주소 치환(`rewrite`)이 호스트를 경로 맨 앞에 붙인다
    path: String,
    headers: Vec<(String, String)>,
}

enum Resp {
    Full { status: u16, location: Option<String>, body: Vec<u8> },
    /// 헤더(Content-Length 만)를 보내고 끊는다
    Lie { content_length: u64 },
    /// `content_length` 를 약속하고 `prefix` 만 보낸 뒤 멈춘다(읽기 정지)
    Stall { content_length: usize, prefix: Vec<u8> },
    /// 조금씩 보낸다 — 전체 시간은 길지만 멈추지는 않는다
    Trickle { total: usize, chunk: usize, delay: Duration },
    /// 연결을 받고 요청을 읽은 뒤 **헤더조차 보내지 않는다**
    Silent,
}

impl Resp {
    fn ok(body: &[u8]) -> Self {
        Resp::Full { status: 200, location: None, body: body.to_vec() }
    }
    fn redirect(to: &str) -> Self {
        Resp::Full { status: 302, location: Some(to.to_string()), body: Vec::new() }
    }
    fn status(status: u16) -> Self {
        Resp::Full { status, location: None, body: Vec::new() }
    }
}

struct Server {
    port: u16,
    reqs: Arc<Mutex<Vec<Req>>>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Server {
    async fn start(route: impl Fn(&Req) -> Resp + Send + Sync + 'static) -> Server {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("루프백 바인드");
        let port = listener.local_addr().expect("주소").port();
        let reqs: Arc<Mutex<Vec<Req>>> = Arc::default();
        let (route, log) = (Arc::new(route), reqs.clone());
        let task = tokio::spawn(async move {
            while let Ok((sock, _)) = listener.accept().await {
                let (route, log) = (route.clone(), log.clone());
                tokio::spawn(async move { serve(sock, route, log).await });
            }
        });
        Server { port, reqs, task }
    }

    fn requests(&self) -> Vec<Req> {
        self.reqs.lock().unwrap().clone()
    }

    /// 경로에 `needle` 이 든 요청의 수
    fn hits(&self, needle: &str) -> usize {
        self.requests().iter().filter(|r| r.path.contains(needle)).count()
    }

    fn rewrite(&self) -> Rewrite {
        let port = self.port;
        Arc::new(move |url: &str| format!("http://127.0.0.1:{port}/{}", url.strip_prefix("https://").or_else(|| url.strip_prefix("http://")).unwrap_or(url)))
    }
}

async fn serve(mut sock: TcpStream, route: Arc<dyn Fn(&Req) -> Resp + Send + Sync>, log: Arc<Mutex<Vec<Req>>>) {
    // 작은 쓰기가 Nagle·지연 ACK 에 묶이지 않게 한다(조금씩 보내는 시험이 정확한 간격을 갖는다)
    let _ = sock.set_nodelay(true);
    let mut buf = Vec::new();
    let mut chunk = [0u8; 2048];
    while !buf.windows(4).any(|w| w == b"\r\n\r\n") && buf.len() < 16 * 1024 {
        match sock.read(&mut chunk).await {
            Ok(0) | Err(_) => return,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    }
    let head = String::from_utf8_lossy(&buf).into_owned();
    let mut lines = head.split("\r\n");
    let path = lines.next().and_then(|l| l.split(' ').nth(1)).unwrap_or("").to_string();
    let headers = lines.filter_map(|l| l.split_once(':')).map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string())).collect();
    let req = Req { path, headers };
    log.lock().unwrap().push(req.clone());
    let head = |status: u16, content_length: u64, location: Option<&str>| {
        let mut h = format!("HTTP/1.1 {status} X\r\nContent-Length: {content_length}\r\nConnection: close\r\n");
        if let Some(l) = location {
            h.push_str(&format!("Location: {l}\r\n"));
        }
        h.push_str("\r\n");
        h
    };
    match route(&req) {
        Resp::Full { status, location, body } => {
            let _ = sock.write_all(head(status, body.len() as u64, location.as_deref()).as_bytes()).await;
            let _ = sock.write_all(&body).await;
        }
        Resp::Lie { content_length } => {
            let _ = sock.write_all(head(200, content_length, None).as_bytes()).await;
        }
        Resp::Stall { content_length, prefix } => {
            let _ = sock.write_all(head(200, content_length as u64, None).as_bytes()).await;
            let _ = sock.write_all(&prefix).await;
            let _ = sock.flush().await;
            tokio::time::sleep(Duration::from_secs(30)).await;
        }
        Resp::Silent => {
            tokio::time::sleep(Duration::from_secs(30)).await;
        }
        Resp::Trickle { total, chunk, delay } => {
            let _ = sock.write_all(head(200, total as u64, None).as_bytes()).await;
            let mut sent = 0;
            while sent < total {
                let n = chunk.min(total - sent);
                if sock.write_all(&vec![b'x'; n]).await.is_err() {
                    return;
                }
                let _ = sock.flush().await;
                sent += n;
                tokio::time::sleep(delay).await;
            }
        }
    }
    let _ = sock.shutdown().await;
}

fn clients(server: &Server, stall: Duration) -> (HttpClient, HttpClient, HttpClient) {
    (
        HttpClient::new_for_test(Redirects::ReleaseAssets, Duration::from_secs(5), Some(server.rewrite())).expect("클라이언트"),
        HttpClient::new_for_test(Redirects::None, Duration::from_secs(5), Some(server.rewrite())).expect("클라이언트"),
        HttpClient::new_download_for_test(Redirects::ReleaseAssets, stall, Some(server.rewrite())).expect("클라이언트"),
    )
}

// ── 클라이언트: 리다이렉트 허용 목록 ─────────────────────────────────────────────

fn asset_client(server: &Server) -> HttpClient {
    clients(server, Duration::from_secs(5)).0
}

#[tokio::test]
async fn 리다이렉트를_허용_호스트_안에서_따라가고_토큰을_보내지_않는다() {
    let server = Server::start(|r| match r.path.as_str() {
        "/github.com/Kang-Jacob-GitLB/antler/releases/latest/download/latest.json" => Resp::redirect("https://github.com/Kang-Jacob-GitLB/antler/releases/download/v2.0.0/latest.json"),
        "/github.com/Kang-Jacob-GitLB/antler/releases/download/v2.0.0/latest.json" => Resp::redirect("https://objects.githubusercontent.com/a?x=1"),
        "/objects.githubusercontent.com/a?x=1" => Resp::redirect("https://release-assets.githubusercontent.com/b"),
        "/release-assets.githubusercontent.com/b" => Resp::ok(b"{}"),
        _ => Resp::status(404),
    })
    .await;
    let mut res = asset_client(&server).get_release_asset(STABLE_MANIFEST_URL, is_manifest_url, None).await.expect("따라간다");
    assert_eq!(res.chunk().await.unwrap().unwrap().as_ref(), b"{}");
    assert_eq!(server.requests().len(), 4);
    for r in server.requests() {
        for (name, _) in &r.headers {
            assert!(!["authorization", "cookie", "proxy-authorization"].contains(&name.as_str()), "{name} 를 보냈다: {}", r.path);
        }
    }
}

#[tokio::test]
async fn 리다이렉트가_허용_목록_밖으로_가면_거부하고_따라가지_않는다() {
    let targets = [
        "https://evil.example/x",
        "http://github.com/x",
        "https://github.com:8443/x",
        "https://user:pw@github.com/x",
        "https://github.com.evil.example/x",
        "https://objects.githubusercontent.com:444/x",
        "https://api.github.com/x",
        "ftp://github.com/x",
        "//evil.example/x",
        "file:///c:/windows/x",
    ];
    for target in targets {
        let to = target.to_string();
        let server = Server::start(move |r| if r.path == "/github.com/start" { Resp::redirect(&to) } else { Resp::ok(b"must-not-follow") }).await;
        let got = asset_client(&server).get_release_asset("https://github.com/start", |_| true, None).await;
        assert_eq!(got.err(), Some(AssetError::Forbidden), "{target}");
        assert_eq!(server.requests().len(), 1, "{target} 로 따라갔다: {:?}", server.requests());
    }
}

#[tokio::test]
async fn 리다이렉트는_다섯_번까지만_따라간다() {
    // 체인: /github.com/r0 → r1 → … . `end` 번째가 200 이다
    for (end, ok) in [(5usize, true), (6, false), (9, false)] {
        let server = Server::start(move |r| {
            let n: usize = r.path.trim_start_matches("/github.com/r").parse().unwrap_or(999);
            if n < end {
                Resp::redirect(&format!("https://github.com/r{}", n + 1))
            } else if n == end {
                Resp::ok(b"end")
            } else {
                Resp::status(404)
            }
        })
        .await;
        let got = asset_client(&server).get_release_asset("https://github.com/r0", |_| true, None).await;
        if ok {
            assert!(got.is_ok(), "다섯 번째 리다이렉트까지는 따라간다");
        } else {
            assert_eq!(got.err(), Some(AssetError::TooManyRedirects), "end={end}");
            assert_eq!(server.requests().len(), 6, "여섯 번째 리다이렉트는 따라가지 않는다");
        }
    }
}

#[tokio::test]
async fn 첫_주소는_호출자의_판정과_릴리스_자산_클라이언트일_때만_요청한다() {
    let server = Server::start(|_| Resp::ok(b"x")).await;
    let c = asset_client(&server);
    // 첫 판정 거부(다른 저장소·다른 호스트·http)
    for url in ["https://github.com/Evil/antler/releases/download/v1.0.0/a.exe", "https://evil.example/a.exe", "http://github.com/Kang-Jacob-GitLB/antler/releases/download/v1.0.0/a.exe"] {
        assert_eq!(c.get_release_asset(url, is_release_asset_url, None).await.err(), Some(AssetError::Forbidden), "{url}");
    }
    // 판정을 통과해도 다운로드 허용 목록 밖이면 거부(첫 판정이 느슨해도 호스트는 막힌다는 것은 아니다 — 첫 판정이 책임이다)
    assert_eq!(c.get_release_asset("https://github.com/x", |_| false, None).await.err(), Some(AssetError::Forbidden));
    // ReleaseAssets 가 아닌 클라이언트는 이 길을 쓰지 못한다
    let plain = clients(&server, Duration::from_secs(5)).1;
    assert_eq!(plain.get_release_asset("https://github.com/x", |_| true, None).await.err(), Some(AssetError::Forbidden));
    assert!(server.requests().is_empty(), "거부는 요청 전이다: {:?}", server.requests());
}

#[tokio::test]
async fn 최종_응답이_2xx_가_아니거나_위치가_없으면_실패한다() {
    let server = Server::start(|r| match r.path.as_str() {
        "/github.com/a/b/c" => Resp::redirect("next"),
        "/github.com/a/b/next" => Resp::status(404),
        "/github.com/noloc" => Resp::status(302),
        "/github.com/limited" => Resp::status(403),
        _ => Resp::status(500),
    })
    .await;
    let c = asset_client(&server);
    // 상대 Location 은 현재 주소를 기준으로 풀고 그 주소도 판정한다
    assert_eq!(c.get_release_asset("https://github.com/a/b/c", |_| true, None).await.err(), Some(AssetError::Status(404)));
    assert_eq!(c.get_release_asset("https://github.com/noloc", |_| true, None).await.err(), Some(AssetError::BadRedirect));
    assert_eq!(c.get_release_asset("https://github.com/limited", |_| true, None).await.err(), Some(AssetError::Status(403)));
}

#[tokio::test]
async fn 이미_취소된_요청은_시작하지_않는다() {
    let server = Server::start(|_| Resp::ok(b"x")).await;
    let cancel = CancellationToken::new();
    cancel.cancel();
    let got = asset_client(&server).get_release_asset("https://github.com/slow", |_| true, Some(&cancel)).await;
    assert_eq!(got.err(), Some(AssetError::Aborted));
    assert!(server.requests().is_empty());
}

// ── 백엔드: 서명·키·크기·매니페스트 ─────────────────────────────────────────────

/// 가짜 설치 포트가 있는 엔진: 검증은 진짜 플러그인(`Update::verify`)이고 `install` 은 기록만 한다(설치 프로그램을 띄우지 않는다)
struct TestEngine {
    real: PluginEngine<MockRuntime>,
    verifies: AtomicUsize,
    installs: Mutex<Vec<Vec<u8>>>,
}

impl Engine for TestEngine {
    fn key_ready(&self) -> bool {
        self.real.key_ready()
    }
    fn verify(&self, offer: &ManifestOffer, bytes: &[u8]) -> Result<(), String> {
        self.verifies.fetch_add(1, Ordering::SeqCst);
        self.real.verify(offer, bytes)
    }
    fn install(&self, _offer: &ManifestOffer, bytes: &[u8]) -> Result<(), String> {
        self.installs.lock().unwrap().push(bytes.to_vec());
        Ok(())
    }
}

/// 진짜 업데이트 플러그인을 단 가짜 앱. `pubkey` 는 시험 키(생성자 인자)다
fn mock_app(pubkey: Option<&str>) -> tauri::App<MockRuntime> {
    let mut ctx = tauri::test::mock_context(tauri::test::noop_assets());
    ctx.config_mut().plugins.0.insert("updater".to_string(), updater_plugin_config(pubkey));
    tauri::test::mock_builder().plugin(tauri_plugin_updater::Builder::new().build()).build(ctx).expect("가짜 앱")
}

struct Env {
    _app: tauri::App<MockRuntime>,
    server: Server,
    engine: Arc<TestEngine>,
    backend: Arc<HostBackend<TestEngine>>,
}

async fn env_with(pubkey: Option<&str>, stall: Duration, route: impl Fn(&Req) -> Resp + Send + Sync + 'static) -> Env {
    let app = mock_app(pubkey);
    let server = Server::start(route).await;
    let (manifest, api, download) = clients(&server, stall);
    let engine = Arc::new(TestEngine { real: PluginEngine::new(app.handle().clone(), pubkey.map(|k| k.trim().to_string()), Arc::new(|| {})), verifies: AtomicUsize::new(0), installs: Mutex::new(Vec::new()) });
    let backend = Arc::new(HostBackend::new(UpdateNet::from_clients(manifest, api, download), engine.clone(), CURRENT.to_string()));
    Env { _app: app, server, engine, backend }
}

fn manifest(version: &str, url: &str, sig: &str) -> String {
    let entry = json!({ "url": url, "signature": sig.trim() });
    json!({ "version": version, "platforms": { "windows-x86_64-nsis": entry, "windows-x86_64": entry } }).to_string()
}

/// 정식 판의 매니페스트 → 설치본 경로(리다이렉트 둘)를 서버로 낸다. `installer` 는 마지막 응답이다
fn site(manifest_body: String, installer: impl Fn() -> Resp + Send + Sync + 'static) -> impl Fn(&Req) -> Resp + Send + Sync + 'static {
    move |req| match req.path.as_str() {
        "/github.com/Kang-Jacob-GitLB/antler/releases/latest/download/latest.json" => Resp::redirect("https://github.com/Kang-Jacob-GitLB/antler/releases/download/v2.0.0/latest.json"),
        "/github.com/Kang-Jacob-GitLB/antler/releases/download/v2.0.0/latest.json" => Resp::ok(manifest_body.as_bytes()),
        "/github.com/Kang-Jacob-GitLB/antler/releases/download/v2.0.0/Antler-Setup-2.0.0-x64.exe" => Resp::redirect("https://release-assets.githubusercontent.com/gh/asset?token=redirected"),
        p if p.starts_with("/release-assets.githubusercontent.com/gh/asset") => installer(),
        _ => Resp::status(404),
    }
}

async fn download(env: &Env, version: &str) -> Result<(), String> {
    env.backend.download(version, &|_| {}, &CancellationToken::new()).await
}

#[tokio::test]
async fn 리다이렉트를_따라가_서명을_검증한_바이트를_보관하고_진행률을_보고한다() {
    let env = env_with(Some(PUBKEY_A), Duration::from_secs(5), site(manifest("2.0.0", INSTALLER, SIG_GOOD), || Resp::ok(PAYLOAD))).await;
    assert_eq!(env.backend.check().await, Ok(Some("2.0.0".to_string())));
    let seen = Mutex::new(Vec::<f64>::new());
    env.backend.download("2.0.0", &|p| seen.lock().unwrap().push(p), &CancellationToken::new()).await.expect("내려받기");
    assert_eq!(*seen.lock().unwrap(), [100.0]);
    assert_eq!(env.engine.verifies.load(Ordering::SeqCst), 1);
    assert!(env.engine.installs.lock().unwrap().is_empty(), "내려받기는 설치하지 않는다");
    // 설치는 가짜 포트에 검증을 통과한 바이트 그대로 간다
    env.backend.install().expect("설치");
    assert_eq!(*env.engine.installs.lock().unwrap(), [PAYLOAD.to_vec()]);
    // 요청은 매니페스트 둘(리다이렉트)·설치본 둘이다 — 토큰·쿠키 없음
    for r in env.server.requests() {
        assert!(r.headers.iter().all(|(k, _)| k != "authorization" && k != "cookie"), "{r:?}");
    }
}

#[tokio::test]
async fn 서명이_틀린_바이트는_설치_포트를_부르지_않는다() {
    // (이름, 서버가 주는 바이트, 매니페스트의 서명, 매니페스트 버전, 기대 오류)
    let cases: [(&str, Vec<u8>, &str, &str, &str); 5] = [
        ("바이트 변조", [PAYLOAD, b"x"].concat(), SIG_GOOD, "2.0.0", "업데이트 서명이 올바르지 않습니다"),
        ("다른 키로 서명", PAYLOAD.to_vec(), SIG_WRONGKEY, "2.0.0", "업데이트 서명이 올바르지 않습니다"),
        ("서명된 버전이 다르다(옛 서명 + 새 버전 번호)", PAYLOAD.to_vec(), SIG_OLDVER, "2.0.0", "업데이트 서명의 버전이 맞지 않습니다"),
        ("서명에 버전이 없다(옛 CLI)", PAYLOAD.to_vec(), SIG_NOVER, "2.0.0", "업데이트 서명의 버전이 맞지 않습니다"),
        ("서명이 깨졌다", PAYLOAD.to_vec(), "AAAA", "2.0.0", "업데이트 서명이 올바르지 않습니다"),
    ];
    for (name, bytes, sig, version, want) in cases {
        let env = env_with(Some(PUBKEY_A), Duration::from_secs(5), site(manifest(version, INSTALLER, sig), move || Resp::ok(&bytes))).await;
        assert_eq!(env.backend.check().await, Ok(Some(version.to_string())), "{name}");
        assert_eq!(download(&env, version).await, Err(want.to_string()), "{name}");
        assert_eq!(env.engine.verifies.load(Ordering::SeqCst), 1, "{name}: 검증이 돌았다");
        // 검증에 실패한 바이트는 남지 않는다 — 설치는 보관한 것이 없다고 거부한다
        assert_eq!(env.backend.install(), Err("다시 확인한 뒤 시도하세요".to_string()), "{name}");
        assert!(env.engine.installs.lock().unwrap().is_empty(), "{name}: 설치 포트가 불렸다");
    }
}

#[tokio::test]
async fn 서명이_틀리면_서비스는_오류로_알리고_설치를_시작하지_않는다() {
    let env = env_with(Some(PUBKEY_B), Duration::from_secs(5), site(manifest("2.0.0", INSTALLER, SIG_GOOD), || Resp::ok(PAYLOAD))).await;
    let sent: Arc<Mutex<Vec<UpdateState>>> = Arc::default();
    let sink = sent.clone();
    let svc = UpdateService::new(
        env.backend.clone(),
        Deps { current_version: CURRENT.to_string(), enabled: true, fake: None, skipped: Arc::new(|| None), set_skipped: Arc::new(|_| Ok(())), send: Arc::new(move |s| sink.lock().unwrap().push(s.clone())) },
    );
    assert_eq!(svc.check(true).await.status, Status::Available);
    let done = svc.download().await;
    // 시험 공개 키가 B 라 키 A 로 서명한 표본이 맞지 않는다
    assert_eq!((done.status, done.error.as_deref()), (Status::Error, Some("업데이트 서명이 올바르지 않습니다")));
    svc.install().await;
    assert!(env.engine.installs.lock().unwrap().is_empty());
    assert!(sent.lock().unwrap().iter().all(|s| s.status != Status::Downloaded), "받은 것으로 알리지 않는다");
}

#[tokio::test]
async fn 키가_없으면_확인은_되고_내려받기는_네트워크를_열지_않고_설치_포트는_불리지_않는다() {
    let env = env_with(None, Duration::from_secs(5), site(manifest("2.0.0", INSTALLER, SIG_GOOD), || Resp::ok(PAYLOAD))).await;
    assert!(!env.engine.key_ready());
    // 확인은 동작한다
    assert_eq!(env.backend.check().await, Ok(Some("2.0.0".to_string())));
    let before = env.server.requests().len();
    assert_eq!(download(&env, "2.0.0").await, Err(NO_KEY_MESSAGE.to_string()));
    assert_eq!(env.server.requests().len(), before, "키가 없으면 설치본을 요청하지 않는다");
    assert_eq!(env.server.hits("Setup-2.0.0"), 0);
    assert_eq!(env.engine.verifies.load(Ordering::SeqCst), 0);
    assert_eq!(env.backend.install(), Err(NO_KEY_MESSAGE.to_string()));
    assert!(env.engine.installs.lock().unwrap().is_empty());
    // 서비스: 오류 상태에 키 문구가 보이고(다시 시도할 수 있다) 설치로 넘어가지 않는다
    let svc = UpdateService::new(
        env.backend.clone(),
        Deps { current_version: CURRENT.to_string(), enabled: true, fake: None, skipped: Arc::new(|| None), set_skipped: Arc::new(|_| Ok(())), send: Arc::new(|_| {}) },
    );
    svc.check(true).await;
    let done = svc.download().await;
    assert_eq!((done.status, done.error.as_deref()), (Status::Error, Some(NO_KEY_MESSAGE)));
    svc.install().await;
    assert!(env.engine.installs.lock().unwrap().is_empty());
}

#[tokio::test]
async fn 키_없는_배포_빌드는_업데이트를_설치하지_못한다() {
    // 불변식: 운영 상수 `UPDATE_PUBKEY` 가 비어 있는 동안, 운영 엔진은 키가 없고 검증·설치가 모두 키 없음으로 끝난다(플러그인에 닿지 않는다)
    let app = mock_app(UPDATE_PUBKEY);
    let engine = PluginEngine::new(app.handle().clone(), UPDATE_PUBKEY.map(str::to_string), Arc::new(|| panic!("종료 정리가 불리면 안 된다")));
    assert_eq!(engine.key_ready(), UPDATE_PUBKEY.is_some());
    assert_eq!(updater_plugin_config(UPDATE_PUBKEY)["pubkey"], UPDATE_PUBKEY.unwrap_or(""));
    if UPDATE_PUBKEY.is_none() {
        let offer = ManifestOffer { version: "2.0.0".to_string(), url: INSTALLER.to_string(), signature: SIG_GOOD.trim().to_string(), platform: "windows-x86_64-nsis" };
        assert_eq!(engine.verify(&offer, PAYLOAD), Err(NO_KEY_MESSAGE.to_string()));
        assert_eq!(engine.install(&offer, PAYLOAD), Err(NO_KEY_MESSAGE.to_string()));
    }
    // 키가 들어오면 형식을 지킨다: minisign 공개 키 텍스트(두 줄 — 설명, `RW…` 56자)를 base64 한 겹으로 감싼 한 줄이고, 시험 키가 아니다
    if let Some(key) = UPDATE_PUBKEY {
        assert!(key.trim() == key && !key.contains('\n'), "한 줄이어야 한다");
        // base64 겹이 달라도(줄바꿈·설명 줄만 다른 포장) 같은 키 본체를 못 쓰게 디코드한 키 본체(둘째 줄 `RW…`, 키 id 를 담는다)로 비교한다
        for test_key in [PUBKEY_A, PUBKEY_B] {
            assert_ne!(key_body(key), key_body(test_key), "운영 키가 시험 키다");
        }
        let decoded = decode_base64(key);
        let mut lines = decoded.lines();
        assert!(lines.next().is_some_and(|l| l.starts_with("untrusted comment: minisign public key")), "{decoded}");
        assert!(lines.next().is_some_and(|l| l.len() == 56 && l.starts_with("RW")), "{decoded}");
    }
}

/// base64 한 겹을 벗긴 minisign 공개 키 텍스트의 **키 본체**(둘째 줄 `RW…` — 알고리즘·키 id·공개 키). 첫 줄은 설명이라 키가 아니다
fn key_body(b64: &str) -> String {
    decode_base64(b64.trim()).lines().nth(1).unwrap_or_default().trim().to_string()
}

/// 표준 base64 해독(시험용 — 새 크레이트 없이)
fn decode_base64(s: &str) -> String {
    let table = |c: u8| match c {
        b'A'..=b'Z' => Some(c - b'A'),
        b'a'..=b'z' => Some(c - b'a' + 26),
        b'0'..=b'9' => Some(c - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    };
    let (mut out, mut acc, mut bits) = (Vec::new(), 0u32, 0u32);
    for c in s.bytes().filter(|c| *c != b'=') {
        acc = (acc << 6) | u32::from(table(c).expect("base64 문자"));
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    String::from_utf8(out).expect("utf8")
}

#[tokio::test]
async fn 진짜_플러그인_엔진은_서명이_틀리면_설치_프로그램을_띄우지_않는다() {
    // 진짜 `Update::install` 은 다시 검증한 뒤에만 설치 프로그램을 띄운다. 여기서는 어느 갈래도 띄우지 못한다:
    // 서명이 틀리면 검증에서 끝나고, 서명이 맞아도 표본 바이트가 실행 파일이 아니라 형식 검사에서 끝난다(종료 정리가 불리지 않는다 = 설치 단계에 가지 않았다)
    let exits = Arc::new(AtomicUsize::new(0));
    let counter = exits.clone();
    let app = mock_app(Some(PUBKEY_A));
    let engine = PluginEngine::new(app.handle().clone(), Some(PUBKEY_A.trim().to_string()), Arc::new(move || {
        counter.fetch_add(1, Ordering::SeqCst);
    }));
    let offer = |sig: &str, version: &str| ManifestOffer { version: version.to_string(), url: INSTALLER.to_string(), signature: sig.trim().to_string(), platform: "windows-x86_64-nsis" };
    // 다시 열기 전 재검증(TS "다시 열기 전에 해시를 다시 확인한다"): 받은 뒤 바이트가 바뀌었으면 설치 포트(`install`)가 거부한다
    assert_eq!(engine.install(&offer(SIG_GOOD, "2.0.0"), &[PAYLOAD, b"x"].concat()), Err("업데이트 서명이 올바르지 않습니다".to_string()));
    assert_eq!(engine.install(&offer(SIG_WRONGKEY, "2.0.0"), PAYLOAD), Err("업데이트 서명이 올바르지 않습니다".to_string()));
    assert_eq!(engine.install(&offer(SIG_OLDVER, "2.0.0"), PAYLOAD), Err("업데이트 서명의 버전이 맞지 않습니다".to_string()));
    assert_eq!(engine.install(&offer(SIG_NOVER, "2.0.0"), PAYLOAD), Err("업데이트 서명의 버전이 맞지 않습니다".to_string()));
    // 서명이 맞아도 실행 파일 형식이 아니면 설치하지 않는다(검증은 통과했다는 증거 — 오류가 서명 오류가 아니다)
    assert_eq!(engine.install(&offer(SIG_GOOD, "2.0.0"), PAYLOAD), Err("설치 파일 형식이 올바르지 않습니다".to_string()));
    assert_eq!(exits.load(Ordering::SeqCst), 0, "설치 단계(종료 정리)에 가지 않았다");
    // 검증만(설치 포트가 아니다): 맞는 서명은 통과한다
    assert_eq!(engine.verify(&offer(SIG_GOOD, "2.0.0"), PAYLOAD), Ok(()));
}

#[tokio::test]
async fn 설치본_크기_상한을_넘으면_받지도_검증하지도_않는다() {
    let env = env_with(Some(PUBKEY_A), Duration::from_secs(5), site(manifest("2.0.0", INSTALLER, SIG_GOOD), || Resp::Lie { content_length: 300_000_001 })).await;
    env.backend.check().await.expect("확인");
    assert_eq!(download(&env, "2.0.0").await, Err("응답이 너무 큽니다".to_string()));
    assert_eq!(env.engine.verifies.load(Ordering::SeqCst), 0);
    assert!(env.engine.installs.lock().unwrap().is_empty());
}

#[tokio::test]
async fn 매니페스트_크기_상한을_넘으면_거부한다() {
    let env = env_with(Some(PUBKEY_A), Duration::from_secs(5), site(" ".repeat(1_000_001), || Resp::ok(PAYLOAD))).await;
    assert_eq!(env.backend.check().await, Err("응답이 너무 큽니다".to_string()));
    assert_eq!(env.server.hits("Setup-2.0.0"), 0);
}

#[tokio::test]
async fn 리다이렉트가_허용_목록_밖으로_가면_설치본을_받지_않는다() {
    for target in ["https://evil.example/Antler-Setup-2.0.0-x64.exe", "http://release-assets.githubusercontent.com/a", "https://github.com:8443/a", "https://u:p@github.com/a"] {
        let to = target.to_string();
        let route = move |r: &Req| match r.path.as_str() {
            "/github.com/Kang-Jacob-GitLB/antler/releases/latest/download/latest.json" => Resp::redirect("https://github.com/Kang-Jacob-GitLB/antler/releases/download/v2.0.0/latest.json"),
            "/github.com/Kang-Jacob-GitLB/antler/releases/download/v2.0.0/latest.json" => Resp::ok(manifest("2.0.0", INSTALLER, SIG_GOOD).as_bytes()),
            "/github.com/Kang-Jacob-GitLB/antler/releases/download/v2.0.0/Antler-Setup-2.0.0-x64.exe" => Resp::redirect(&to),
            _ => Resp::ok(PAYLOAD),
        };
        let env = env_with(Some(PUBKEY_A), Duration::from_secs(5), route).await;
        env.backend.check().await.expect("확인");
        assert_eq!(download(&env, "2.0.0").await, Err("허용되지 않은 다운로드 주소입니다".to_string()), "{target}");
        assert_eq!(env.engine.verifies.load(Ordering::SeqCst), 0, "{target}");
        assert_eq!(env.server.hits("evil.example") + env.server.hits("asset"), 0, "{target}: 따라갔다");
    }
}

#[tokio::test]
async fn 읽기가_멈추면_실패하고_느려도_정지_제한_안이면_끝까지_받는다() {
    // 멈춤: 약속한 길이의 일부만 보내고 멈춘다 → 읽기 정지 제한(여기서는 300ms)
    let env = env_with(Some(PUBKEY_A), Duration::from_millis(300), site(manifest("2.0.0", INSTALLER, SIG_GOOD), || Resp::Stall { content_length: 1000, prefix: vec![b'x'; 10] })).await;
    env.backend.check().await.expect("확인");
    let t = std::time::Instant::now();
    assert_eq!(download(&env, "2.0.0").await, Err("응답 시간이 초과되었습니다".to_string()));
    assert!(t.elapsed() < Duration::from_secs(5));
    assert_eq!(env.engine.verifies.load(Ordering::SeqCst), 0);
    // 느림(조금씩 0.9초): 정지 제한(3초) 안이면 끝까지 받아 검증까지 간다 — 표본이 아니라 서명 오류로 끝난다.
    // (전체 시간 제한이 없다는 것은 `net/http.rs` 의 다운로드 프로필이 총 제한을 두지 않는 구조로 소스 검사한다 — 이 하네스는 루프백의 작은 쓰기를 마지막에 한꺼번에 전달해 간격을 시험할 수 없다)
    let env = env_with(Some(PUBKEY_A), Duration::from_secs(3), site(manifest("2.0.0", INSTALLER, SIG_GOOD), || Resp::Trickle { total: 600, chunk: 100, delay: Duration::from_millis(150) })).await;
    env.backend.check().await.expect("확인");
    assert_eq!(download(&env, "2.0.0").await, Err("업데이트 서명이 올바르지 않습니다".to_string()));
    assert_eq!(env.engine.verifies.load(Ordering::SeqCst), 1, "끝까지 받아 검증했다");
}

#[tokio::test]
async fn 응답_헤더가_오지_않아도_읽기_정지_제한이_끊는다() {
    // 전체 시간 제한이 없는 내려받기 클라이언트가 헤더를 영영 기다리지 않는다(서버가 연결만 받고 입을 닫은 경우)
    let env = env_with(Some(PUBKEY_A), Duration::from_millis(300), site(manifest("2.0.0", INSTALLER, SIG_GOOD), || Resp::Silent)).await;
    env.backend.check().await.expect("확인");
    let t = std::time::Instant::now();
    assert_eq!(download(&env, "2.0.0").await, Err("응답 시간이 초과되었습니다".to_string()));
    assert!(t.elapsed() < Duration::from_secs(5), "{:?}", t.elapsed());
    assert_eq!(env.engine.verifies.load(Ordering::SeqCst), 0);
}

#[test]
fn 내려받기_프로필은_전체_시간_제한_없이_연결과_읽기_정지만_제한한다() {
    let http = code("src/net/http.rs");
    assert!(http.contains("Profile::Download => (true, None, Some(READ_STALL_TIMEOUT)),"), "다운로드 프로필: https 만·전체 제한 없음·읽기 정지만");
    assert!(http.contains("Profile::Production => (true, Some(REQUEST_TIMEOUT), None),"), "일반 프로필은 15초 전체 제한 그대로");
    assert!(http.contains(".connect_timeout(CONNECT_TIMEOUT).read_timeout(s)"));
    assert!(http.contains("pub const MAX_ASSET_REDIRECTS: usize = 5;"));
    // 설치본·매니페스트는 릴리스 자산 전용 클라이언트로만 받는다(일반 리다이렉트 정책으로 받지 않는다)
    let host = code("src/update/host.rs");
    assert_eq!(host.matches("Redirects::ReleaseAssets").count(), 2, "매니페스트·설치본 — 업데이트 요청의 릴리스 자산 정책은 이 둘뿐(API 는 리다이렉트 없음)");
    assert!(host.contains("HttpClient::production_download(Redirects::ReleaseAssets)"));
    assert!(!host.contains("Redirects::SameOrigin"));
}

#[tokio::test]
async fn 내려받기_취소는_검증도_보관도_하지_않는다() {
    let env = env_with(Some(PUBKEY_A), Duration::from_secs(5), site(manifest("2.0.0", INSTALLER, SIG_GOOD), || Resp::Stall { content_length: 1000, prefix: vec![b'x'; 10] })).await;
    env.backend.check().await.expect("확인");
    let cancel = CancellationToken::new();
    let canceller = async {
        tokio::time::sleep(Duration::from_millis(200)).await;
        cancel.cancel();
    };
    let (got, ()) = tokio::join!(env.backend.download("2.0.0", &|_| {}, &cancel), canceller);
    assert_eq!(got, Err("취소되었습니다".to_string()));
    assert_eq!(env.engine.verifies.load(Ordering::SeqCst), 0);
    assert_eq!(env.backend.install(), Err("다시 확인한 뒤 시도하세요".to_string()));
}

#[tokio::test]
async fn 다른_저장소_다른_태그_허용_밖_주소의_매니페스트는_거부하고_설치본을_요청하지_않는다() {
    let bad = [
        ("다른 저장소", "https://github.com/Evil/antler/releases/download/v2.0.0/Antler-Setup-2.0.0-x64.exe"),
        ("다른 태그", &format!("{REPO}/v1.9.0/Antler-Setup-2.0.0-x64.exe")),
        ("다른 호스트", "https://evil.example/Kang-Jacob-GitLB/antler/releases/download/v2.0.0/a.exe"),
        ("http", "http://github.com/Kang-Jacob-GitLB/antler/releases/download/v2.0.0/a.exe"),
        ("포트", "https://github.com:8443/Kang-Jacob-GitLB/antler/releases/download/v2.0.0/a.exe"),
        ("사용자 정보", "https://user:pw@github.com/Kang-Jacob-GitLB/antler/releases/download/v2.0.0/a.exe"),
        ("경로 이탈", &format!("{REPO}/v2.0.0/../v1.0.0/a.exe")),
        ("인코딩된 점", &format!("{REPO}/v2.0.0/%2e%2e/a.exe")),
        ("쿼리", &format!("{REPO}/v2.0.0/a.exe?x=1")),
        ("태그 폴더 밖", &format!("{REPO}/a.exe")),
    ];
    for (name, url) in bad {
        let env = env_with(Some(PUBKEY_A), Duration::from_secs(5), site(manifest("2.0.0", url, SIG_GOOD), || Resp::ok(PAYLOAD))).await;
        let got = env.backend.check().await;
        assert_eq!(got, Err("업데이트 주소가 올바르지 않습니다".to_string()), "{name}");
        assert_eq!(env.server.hits("Setup") + env.server.hits("asset") + env.server.hits("evil"), 0, "{name}: 설치본을 요청했다");
        assert_eq!(download(&env, "2.0.0").await, Err("다시 확인한 뒤 시도하세요".to_string()), "{name}: 거부한 제안으로 받을 수 없다");
    }
}

#[tokio::test]
async fn 서명이_없는_매니페스트는_내려받기_전에_거부한다() {
    for sig in ["", "not base64 !!", "a".repeat(16 * 1024 + 1).as_str()] {
        let env = env_with(Some(PUBKEY_A), Duration::from_secs(5), site(manifest("2.0.0", INSTALLER, sig), || Resp::ok(PAYLOAD))).await;
        assert_eq!(env.backend.check().await, Err("업데이트 서명 정보가 올바르지 않습니다".to_string()), "{sig:.20}");
        assert_eq!(env.server.hits("asset"), 0);
    }
}

#[tokio::test]
async fn nsis_표식이_없으면_확인_요청이_0회다() {
    // 시험 바이너리에는 번들러가 심은 표식이 없다 — 운영 조립(`build_service`)과 같은 술어(`bundle_is_nsis`)로 서비스를 켠다
    assert!(!bundle_is_nsis(), "시험 바이너리는 NSIS 번들이 아니다");
    let env = env_with(Some(PUBKEY_A), Duration::from_secs(5), site(manifest("2.0.0", INSTALLER, SIG_GOOD), || Resp::ok(PAYLOAD))).await;
    let svc = UpdateService::new(
        env.backend.clone(),
        Deps { current_version: CURRENT.to_string(), enabled: bundle_is_nsis(), fake: None, skipped: Arc::new(|| None), set_skipped: Arc::new(|_| Ok(())), send: Arc::new(|_| {}) },
    );
    assert_eq!(svc.check(false).await.status, Status::Idle);
    assert_eq!(svc.check(true).await.status, Status::NotAvailable);
    assert_eq!(svc.download().await.status, Status::NotAvailable);
    svc.start(&tokio::runtime::Handle::current());
    assert_eq!(env.server.requests().len(), 0, "확인 요청이 한 번도 나가지 않았다");
}

#[tokio::test]
async fn 프리릴리스_판은_릴리스_목록에서_고른_태그의_매니페스트를_받는다() {
    // 현재 버전이 프리릴리스면 `/releases?per_page=10` 의 첫 비초안 릴리스 태그로 매니페스트를 받는다(정식 판의 고정 주소가 아니다). 토큰 없음
    let app = mock_app(Some(PUBKEY_A));
    let list = json!([{ "tag_name": "v2.0.0", "draft": true }, { "tag_name": "v2.0.0-rc.1", "draft": false, "prerelease": true }]).to_string();
    let server = Server::start(move |r| match r.path.as_str() {
        p if p.starts_with("/api.github.com/repos/Kang-Jacob-GitLB/antler/releases?per_page=10") => Resp::ok(list.as_bytes()),
        "/github.com/Kang-Jacob-GitLB/antler/releases/download/v2.0.0-rc.1/latest.json" => {
            let url = format!("{REPO}/v2.0.0-rc.1/Antler-Setup-2.0.0-rc.1-x64.exe");
            Resp::ok(manifest("2.0.0-rc.1", &url, SIG_GOOD).as_bytes())
        }
        _ => Resp::status(404),
    })
    .await;
    let (m, a, d) = clients(&server, Duration::from_secs(5));
    let engine = Arc::new(TestEngine { real: PluginEngine::new(app.handle().clone(), Some(PUBKEY_A.trim().to_string()), Arc::new(|| {})), verifies: AtomicUsize::new(0), installs: Mutex::new(Vec::new()) });
    let backend = HostBackend::new(UpdateNet::from_clients(m, a, d), engine, "0.1.0-rc.0".to_string());
    assert_eq!(backend.check().await, Ok(Some("2.0.0-rc.1".to_string())));
    assert_eq!(server.hits("latest/download"), 0, "정식 판의 고정 주소를 쓰지 않는다");
    for r in server.requests() {
        assert!(r.headers.iter().all(|(k, _)| k != "authorization"), "{r:?}");
    }
}

#[tokio::test]
async fn 릴리스_노트는_토큰_없이_api_에서_받는다() {
    let server = Server::start(|r| match r.path.as_str() {
        "/api.github.com/repos/Kang-Jacob-GitLB/antler/releases/tags/v0.1.0" => Resp::ok(json!({ "tag_name": "v0.1.0", "name": "첫 판", "body": "본문", "draft": false, "html_url": "https://evil.example/x" }).to_string().as_bytes()),
        _ => Resp::status(404),
    })
    .await;
    let env = env_with(Some(PUBKEY_A), Duration::from_secs(5), |_| Resp::status(404)).await;
    let (m, a, d) = clients(&server, Duration::from_secs(5));
    let backend = HostBackend::new(UpdateNet::from_clients(m, a, d), env.engine.clone(), CURRENT.to_string());
    let notes = backend.release_notes(CURRENT).await.expect("노트");
    assert_eq!((notes.version.as_str(), notes.exact, notes.body.as_str()), ("0.1.0", true, "본문"));
    // 링크는 응답의 html_url 이 아니라 이 저장소의 릴리스 페이지로 만든다
    assert_eq!(notes.url, "https://github.com/Kang-Jacob-GitLB/antler/releases/tag/v0.1.0");
    assert!(server.requests().iter().all(|r| r.headers.iter().all(|(k, _)| k != "authorization")));
}

// ── 수동 경로(macOS dmg·Linux deb, PR 4b): 검증한 파일만 저장하고 연다 ─────────────────────

/// 시험 대상: (이름, 대상, 매니페스트 키, 자산 이름 — 버전 2.0.0)
fn manual_cases() -> Vec<(&'static str, Target, &'static str, &'static str)> {
    let dmg_arm = detect_target("macos", "aarch64", BundleMark::App, true).expect("dmg arm64");
    let dmg_x64 = detect_target("macos", "x86_64", BundleMark::App, true).expect("dmg x64");
    let deb = detect_target("linux", "x86_64", BundleMark::Deb, false).expect("deb");
    vec![
        ("dmg arm64", dmg_arm, "darwin-aarch64-dmg", "Antler-2.0.0-arm64.dmg"),
        ("dmg x64", dmg_x64, "darwin-x86_64-dmg", "Antler-2.0.0-x64.dmg"),
        ("deb", deb, "linux-x86_64-deb", "antler_2.0.0_amd64.deb"),
    ]
}

static DIRS: AtomicUsize = AtomicUsize::new(0);

/// 수동 경로의 시험판: 저장 폴더는 시스템 임시 폴더 아래 새 폴더(끝나면 지운다), 여는 포트는 기록만 한다(진짜 열기 없음)
struct ManualEnv {
    inner: Env,
    dir: PathBuf,
    opened: Arc<Mutex<Vec<PathBuf>>>,
}

impl Drop for ManualEnv {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(self.dir.parent().unwrap_or(&self.dir));
    }
}

impl ManualEnv {
    fn backend(&self) -> &HostBackend<TestEngine> {
        &self.inner.backend
    }
    fn engine(&self) -> &TestEngine {
        &self.inner.engine
    }
    fn server(&self) -> &Server {
        &self.inner.server
    }
    /// 저장 폴더의 파일 이름들(폴더가 없으면 빈 목록)
    fn files(&self) -> Vec<String> {
        let mut v: Vec<String> = fs::read_dir(&self.dir).map(|rd| rd.map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect()).unwrap_or_default();
        v.sort();
        v
    }
}

async fn manual_env(target: Target, pubkey: Option<&str>, stall: Duration, open_result: Result<(), String>, route: impl Fn(&Req) -> Resp + Send + Sync + 'static) -> ManualEnv {
    let app = mock_app(pubkey);
    let server = Server::start(route).await;
    let (manifest, api, download) = clients(&server, stall);
    let engine = Arc::new(TestEngine { real: PluginEngine::new(app.handle().clone(), pubkey.map(|k| k.trim().to_string()), Arc::new(|| {})), verifies: AtomicUsize::new(0), installs: Mutex::new(Vec::new()) });
    // 폴더는 한 겹 더 둔다 — 정리는 바깥 폴더를 지운다(저장 폴더 자체는 처음엔 없어야 한다: 앱이 만든다)
    let outer = std::env::temp_dir().join(format!("antler-update-manual-{}-{}", std::process::id(), DIRS.fetch_add(1, Ordering::SeqCst)));
    let dir = outer.join("updates");
    let opened: Arc<Mutex<Vec<PathBuf>>> = Arc::default();
    let log = opened.clone();
    let open: OpenFile = Arc::new(move |p: &Path| {
        log.lock().unwrap().push(p.to_path_buf());
        open_result.clone()
    });
    let backend = Arc::new(HostBackend::with_target(UpdateNet::from_clients(manifest, api, download), engine.clone(), CURRENT.to_string(), target, dir.clone(), open));
    ManualEnv { inner: Env { _app: app, server, engine, backend }, dir, opened }
}

fn manual_manifest(version: &str, key: &str, url: &str, sig: &str) -> String {
    json!({ "version": version, "platforms": { key: { "url": url, "signature": sig.trim() } } }).to_string()
}

/// 정식 판의 매니페스트 → 자산(`file`) 경로(리다이렉트 둘)
fn manual_site(manifest_body: String, file: &'static str, installer: impl Fn() -> Resp + Send + Sync + 'static) -> impl Fn(&Req) -> Resp + Send + Sync + 'static {
    let asset = format!("/github.com/Kang-Jacob-GitLB/antler/releases/download/v2.0.0/{file}");
    move |req| match req.path.as_str() {
        "/github.com/Kang-Jacob-GitLB/antler/releases/latest/download/latest.json" => Resp::redirect("https://github.com/Kang-Jacob-GitLB/antler/releases/download/v2.0.0/latest.json"),
        "/github.com/Kang-Jacob-GitLB/antler/releases/download/v2.0.0/latest.json" => Resp::ok(manifest_body.as_bytes()),
        p if p == asset => Resp::redirect("https://release-assets.githubusercontent.com/gh/asset?token=redirected"),
        p if p.starts_with("/release-assets.githubusercontent.com/gh/asset") => installer(),
        _ => Resp::status(404),
    }
}

fn asset_url(file: &str) -> String {
    format!("{REPO}/v2.0.0/{file}")
}

#[tokio::test]
async fn 수동_경로는_서명을_검증한_파일을_저장해_열고_설치_포트를_부르지_않는다() {
    for (name, target, key, file) in manual_cases() {
        let env = manual_env(target, Some(PUBKEY_A), Duration::from_secs(5), Ok(()), manual_site(manual_manifest("2.0.0", key, &asset_url(file), SIG_GOOD), file, || Resp::ok(PAYLOAD))).await;
        assert_eq!(env.backend().mode(), target.mode(), "{name}");
        assert_eq!(env.backend().check().await, Ok(Some("2.0.0".to_string())), "{name}");
        assert!(env.files().is_empty(), "{name}: 검증 전에는 디스크에 아무것도 없다(폴더도 없다)");
        let seen = Mutex::new(Vec::<f64>::new());
        env.backend().download("2.0.0", &|p| seen.lock().unwrap().push(p), &CancellationToken::new()).await.unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(*seen.lock().unwrap(), [100.0], "{name}");
        // 자산 이름 그대로 저장되고(임시 파일은 남지 않는다) 그 경로를 한 번 열었다
        assert_eq!(env.files(), [file.to_string()], "{name}");
        assert_eq!(fs::read(env.dir.join(file)).unwrap(), PAYLOAD, "{name}");
        assert_eq!(*env.opened.lock().unwrap(), [env.dir.join(file)], "{name}");
        assert_eq!(env.backend().file_name().as_deref(), Some(file), "{name}");
        assert!(env.engine().installs.lock().unwrap().is_empty(), "{name}: 수동 경로는 설치 포트를 부르지 않는다");
        // 다시 열기: 파일을 다시 읽어 **다시 검증한 뒤** 연다
        env.backend().install().unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(env.engine().verifies.load(Ordering::SeqCst), 2, "{name}: 내려받을 때 + 다시 열기 전");
        assert_eq!(env.opened.lock().unwrap().len(), 2, "{name}");
        assert!(env.engine().installs.lock().unwrap().is_empty(), "{name}");
        // 요청은 토큰·쿠키 없이 나갔다
        assert!(env.server().requests().iter().all(|r| r.headers.iter().all(|(k, _)| k != "authorization" && k != "cookie")), "{name}");
    }
}

#[tokio::test]
async fn 수동_경로도_서명이_틀린_바이트는_저장하지도_열지도_않는다() {
    // (이름, 서버가 주는 바이트, 매니페스트의 서명, 기대 오류)
    let cases: [(&str, Vec<u8>, &str, &str); 5] = [
        ("바이트 변조", [PAYLOAD, b"x"].concat(), SIG_GOOD, "업데이트 서명이 올바르지 않습니다"),
        ("다른 키로 서명", PAYLOAD.to_vec(), SIG_WRONGKEY, "업데이트 서명이 올바르지 않습니다"),
        ("서명된 버전이 다르다", PAYLOAD.to_vec(), SIG_OLDVER, "업데이트 서명의 버전이 맞지 않습니다"),
        ("서명에 버전이 없다", PAYLOAD.to_vec(), SIG_NOVER, "업데이트 서명의 버전이 맞지 않습니다"),
        ("서명이 깨졌다", PAYLOAD.to_vec(), "AAAA", "업데이트 서명이 올바르지 않습니다"),
    ];
    for (cname, target, key, file) in manual_cases() {
        for (name, bytes, sig, want) in &cases {
            let bytes = bytes.clone();
            let env = manual_env(target, Some(PUBKEY_A), Duration::from_secs(5), Ok(()), manual_site(manual_manifest("2.0.0", key, &asset_url(file), sig), file, move || Resp::ok(&bytes))).await;
            assert_eq!(env.backend().check().await, Ok(Some("2.0.0".to_string())), "{cname} {name}");
            assert_eq!(env.backend().download("2.0.0", &|_| {}, &CancellationToken::new()).await, Err(want.to_string()), "{cname} {name}");
            assert!(env.files().is_empty(), "{cname} {name}: 최종 위치(폴더 포함)에 아무것도 없다: {:?}", env.files());
            assert!(env.opened.lock().unwrap().is_empty(), "{cname} {name}: 열었다");
            assert!(env.engine().installs.lock().unwrap().is_empty(), "{cname} {name}");
            assert_eq!(env.backend().file_name(), None, "{cname} {name}");
        }
    }
}

#[tokio::test]
async fn 수동_경로는_공개_키가_없으면_네트워크를_열기_전에_거부하고_폴더도_만들지_않는다() {
    for (name, target, key, file) in manual_cases() {
        let env = manual_env(target, None, Duration::from_secs(5), Ok(()), manual_site(manual_manifest("2.0.0", key, &asset_url(file), SIG_GOOD), file, || Resp::ok(PAYLOAD))).await;
        assert_eq!(env.backend().check().await, Ok(Some("2.0.0".to_string())), "{name}: 확인은 동작한다");
        let before = env.server().requests().len();
        assert_eq!(env.backend().download("2.0.0", &|_| {}, &CancellationToken::new()).await, Err(NO_KEY_MESSAGE.to_string()), "{name}");
        assert_eq!(env.server().requests().len(), before, "{name}: 설치본을 요청하지 않았다");
        assert!(env.files().is_empty() && !env.dir.exists(), "{name}");
        assert_eq!(env.backend().install(), Err(NO_KEY_MESSAGE.to_string()), "{name}");
        assert!(env.opened.lock().unwrap().is_empty() && env.engine().installs.lock().unwrap().is_empty(), "{name}");
    }
}

#[tokio::test]
async fn 수동_경로는_서명이_없거나_다른_플랫폼_키뿐인_매니페스트를_받지_않는다() {
    let (_, target, key, file) = manual_cases().remove(0);
    // 서명 없음·깨짐: 설치본 요청 전에 거부
    for sig in ["", "not base64 !!"] {
        let env = manual_env(target, Some(PUBKEY_A), Duration::from_secs(5), Ok(()), manual_site(manual_manifest("2.0.0", key, &asset_url(file), sig), file, || Resp::ok(PAYLOAD))).await;
        assert_eq!(env.backend().check().await, Err("업데이트 서명 정보가 올바르지 않습니다".to_string()), "{sig}");
        assert_eq!(env.server().hits("asset"), 0);
        assert_eq!(env.backend().download("2.0.0", &|_| {}, &CancellationToken::new()).await, Err("다시 확인한 뒤 시도하세요".to_string()));
        assert!(env.files().is_empty());
    }
    // 이 환경의 키가 없고 다른 키만 있다(일반 키·다른 아키텍처·Windows 키 포함) — 일반 키로 내려가지 않는다
    for other in ["darwin-aarch64", "darwin-x86_64-dmg", "linux-x86_64-deb", "windows-x86_64-nsis", "windows-x86_64", "darwin-aarch64-app"] {
        let env = manual_env(target, Some(PUBKEY_A), Duration::from_secs(5), Ok(()), manual_site(manual_manifest("2.0.0", other, &asset_url(file), SIG_GOOD), file, || Resp::ok(PAYLOAD))).await;
        assert_eq!(env.backend().check().await, Err("이 환경에 맞는 업데이트가 없습니다".to_string()), "{other}");
        assert_eq!(env.server().hits("asset"), 0, "{other}");
    }
}

#[tokio::test]
async fn 수동_경로는_이미_있는_파일을_덮어쓰지_않는다() {
    let (_, target, key, file) = manual_cases().remove(0);
    // 같은 이름의 다른 내용: 건드리지 않고 거부한다(검증은 통과했지만 저장하지 않는다 — 열지도 않는다)
    let env = manual_env(target, Some(PUBKEY_A), Duration::from_secs(5), Ok(()), manual_site(manual_manifest("2.0.0", key, &asset_url(file), SIG_GOOD), file, || Resp::ok(PAYLOAD))).await;
    fs::create_dir_all(&env.dir).unwrap();
    fs::write(env.dir.join(file), b"OLD-CONTENT").unwrap();
    env.backend().check().await.expect("확인");
    assert_eq!(env.backend().download("2.0.0", &|_| {}, &CancellationToken::new()).await, Err(antler_lib::update::store::EXISTS.to_string()));
    assert_eq!(fs::read(env.dir.join(file)).unwrap(), b"OLD-CONTENT", "기존 파일은 그대로다");
    assert_eq!(env.files(), [file.to_string()], "임시 파일이 남지 않았다");
    assert!(env.opened.lock().unwrap().is_empty());
    // 같은 내용(같은 버전을 다시 받는 경우)은 다시 쓰지 않고 그것을 연다
    fs::write(env.dir.join(file), PAYLOAD).unwrap();
    env.backend().download("2.0.0", &|_| {}, &CancellationToken::new()).await.expect("같은 내용은 재사용");
    assert_eq!(env.files(), [file.to_string()]);
    assert_eq!(*env.opened.lock().unwrap(), [env.dir.join(file)]);
}

#[tokio::test]
async fn 수동_경로의_다시_열기는_디스크의_파일이_바뀌었으면_열지_않는다() {
    let (_, target, key, file) = manual_cases().remove(0);
    let env = manual_env(target, Some(PUBKEY_A), Duration::from_secs(5), Ok(()), manual_site(manual_manifest("2.0.0", key, &asset_url(file), SIG_GOOD), file, || Resp::ok(PAYLOAD))).await;
    env.backend().check().await.expect("확인");
    env.backend().download("2.0.0", &|_| {}, &CancellationToken::new()).await.expect("내려받기");
    assert_eq!(env.opened.lock().unwrap().len(), 1);
    // 받은 뒤 파일이 바뀌었다
    fs::write(env.dir.join(file), [PAYLOAD, b"tampered"].concat()).unwrap();
    assert_eq!(env.backend().install(), Err("다시 열기 전 검증 실패: 업데이트 서명이 올바르지 않습니다".to_string()));
    assert_eq!(env.opened.lock().unwrap().len(), 1, "열지 않았다");
    // 지워졌다
    fs::remove_file(env.dir.join(file)).unwrap();
    let e = env.backend().install().expect_err("없다");
    assert!(e.starts_with("다시 열기 전 검증 실패: "), "{e}");
    assert_eq!(env.opened.lock().unwrap().len(), 1);
    // 링크·폴더로 바뀌었다(일반 파일만 읽는다)
    fs::create_dir(env.dir.join(file)).unwrap();
    assert!(env.backend().install().is_err());
    assert_eq!(env.opened.lock().unwrap().len(), 1);
    assert!(env.engine().installs.lock().unwrap().is_empty());
}

#[tokio::test]
async fn 수동_경로의_열기_실패는_오류로_알리고_파일은_남긴다() {
    let (_, target, key, file) = manual_cases().remove(0);
    let env = manual_env(target, Some(PUBKEY_A), Duration::from_secs(5), Err("denied C:\\secret\\path".to_string()), manual_site(manual_manifest("2.0.0", key, &asset_url(file), SIG_GOOD), file, || Resp::ok(PAYLOAD))).await;
    env.backend().check().await.expect("확인");
    // 오류 문구에 열기 실패의 원문(경로)이 섞이지 않는다
    assert_eq!(env.backend().download("2.0.0", &|_| {}, &CancellationToken::new()).await, Err("설치 파일을 열지 못했습니다. 다운로드 폴더에서 직접 여세요".to_string()));
    assert_eq!(env.files(), [file.to_string()], "검증한 파일은 남는다(직접 열 수 있게)");
}

#[tokio::test]
async fn 수동_경로의_크기_상한_리다이렉트_취소는_아무것도_저장하지_않는다() {
    let (_, target, key, file) = manual_cases().remove(2);
    // 크기 상한(300MB)은 받기 전에
    let env = manual_env(target, Some(PUBKEY_A), Duration::from_secs(5), Ok(()), manual_site(manual_manifest("2.0.0", key, &asset_url(file), SIG_GOOD), file, || Resp::Lie { content_length: 300_000_001 })).await;
    env.backend().check().await.expect("확인");
    assert_eq!(env.backend().download("2.0.0", &|_| {}, &CancellationToken::new()).await, Err("응답이 너무 큽니다".to_string()));
    assert!(env.files().is_empty());
    assert_eq!(env.engine().verifies.load(Ordering::SeqCst), 0);
    // 허용 목록 밖 리다이렉트
    for to in ["https://evil.example/a.deb", "http://release-assets.githubusercontent.com/a", "https://github.com:8443/a", "https://u:p@github.com/a"] {
        let (label, to) = (to, to.to_string());
        let route = move |r: &Req| match r.path.as_str() {
            "/github.com/Kang-Jacob-GitLB/antler/releases/latest/download/latest.json" => Resp::redirect("https://github.com/Kang-Jacob-GitLB/antler/releases/download/v2.0.0/latest.json"),
            "/github.com/Kang-Jacob-GitLB/antler/releases/download/v2.0.0/latest.json" => Resp::ok(manual_manifest("2.0.0", key, &asset_url(file), SIG_GOOD).as_bytes()),
            p if p.ends_with(file) => Resp::redirect(&to),
            _ => Resp::ok(PAYLOAD),
        };
        let env = manual_env(target, Some(PUBKEY_A), Duration::from_secs(5), Ok(()), route).await;
        env.backend().check().await.expect("확인");
        assert_eq!(env.backend().download("2.0.0", &|_| {}, &CancellationToken::new()).await, Err("허용되지 않은 다운로드 주소입니다".to_string()), "{label}");
        assert!(env.files().is_empty() && env.opened.lock().unwrap().is_empty(), "{label}");
    }
    // 받는 도중 취소
    let env = manual_env(target, Some(PUBKEY_A), Duration::from_secs(5), Ok(()), manual_site(manual_manifest("2.0.0", key, &asset_url(file), SIG_GOOD), file, || Resp::Stall { content_length: 1000, prefix: vec![b'x'; 10] })).await;
    env.backend().check().await.expect("확인");
    let cancel = CancellationToken::new();
    let canceller = async {
        tokio::time::sleep(Duration::from_millis(200)).await;
        cancel.cancel();
    };
    let (got, ()) = tokio::join!(env.backend().download("2.0.0", &|_| {}, &cancel), canceller);
    assert_eq!(got, Err("취소되었습니다".to_string()));
    assert!(env.files().is_empty() && env.opened.lock().unwrap().is_empty());
}

#[tokio::test]
async fn 수동_경로의_매니페스트는_기대_파일_이름과_다르면_설치본을_요청하지_않는다() {
    let (_, target, key, _) = manual_cases().remove(0);
    // 같은 키로 서명된 다른 플랫폼·다른 버전의 자산을 끼워 넣는 경우
    for wrong in ["Antler-2.0.0-x64.dmg", "antler_2.0.0_amd64.deb", "Antler-Setup-2.0.0-x64.exe", "Antler-1.9.0-arm64.dmg", "Antler-2.0.0-arm64.dmg.exe", "latest.json"] {
        let env = manual_env(target, Some(PUBKEY_A), Duration::from_secs(5), Ok(()), manual_site(manual_manifest("2.0.0", key, &asset_url(wrong), SIG_GOOD), "x", || Resp::ok(PAYLOAD))).await;
        assert_eq!(env.backend().check().await, Err("업데이트 주소가 올바르지 않습니다".to_string()), "{wrong}");
        assert_eq!(env.server().hits("asset"), 0, "{wrong}");
    }
}

#[tokio::test]
async fn 수동_서비스는_상태_순서를_지키고_다시_열기를_여러_번_누를_수_있다() {
    let (_, target, key, file) = manual_cases().remove(0);
    let env = manual_env(target, Some(PUBKEY_A), Duration::from_secs(5), Ok(()), manual_site(manual_manifest("2.0.0", key, &asset_url(file), SIG_GOOD), file, || Resp::ok(PAYLOAD))).await;
    let sent: Arc<Mutex<Vec<UpdateState>>> = Arc::default();
    let sink = sent.clone();
    let svc = UpdateService::new(
        env.inner.backend.clone(),
        Deps { current_version: CURRENT.to_string(), enabled: true, fake: None, skipped: Arc::new(|| None), set_skipped: Arc::new(|_| Ok(())), send: Arc::new(move |s| sink.lock().unwrap().push(s.clone())) },
    );
    assert_eq!(svc.get().mode, "manual-dmg");
    assert_eq!(svc.check(true).await.status, Status::Available);
    let done = svc.download().await;
    assert_eq!((done.status, done.file_name.as_deref(), done.mode), (Status::Downloaded, Some(file), "manual-dmg"));
    svc.install().await;
    svc.install().await;
    assert_eq!(env.opened.lock().unwrap().len(), 3, "내려받을 때 한 번 + 다시 열기 두 번(표식이 풀려 두 번째도 동작한다)");
    assert_eq!(svc.get().status, Status::Downloaded);
    let order: Vec<Status> = sent.lock().unwrap().iter().map(|s| s.status).collect();
    assert_eq!(order, [Status::Checking, Status::Available, Status::Downloading, Status::Downloading, Status::Downloaded]);
    assert!(env.engine().installs.lock().unwrap().is_empty());
    // 파일이 바뀌면 다시 열기는 오류 상태로 알린다(열지 않는다)
    fs::write(env.dir.join(file), b"tampered").unwrap();
    svc.install().await;
    let s = svc.get();
    assert_eq!(s.status, Status::Error);
    assert!(s.error.as_deref().is_some_and(|e| e.starts_with("다시 열기 전 검증 실패: ")), "{:?}", s.error);
    assert_eq!(env.opened.lock().unwrap().len(), 3);
}

// ── 수동 경로의 파일 저장(`update::store`) ─────────────────────────────────────

struct TmpDir(PathBuf);

impl TmpDir {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!("antler-update-store-{}-{}", std::process::id(), DIRS.fetch_add(1, Ordering::SeqCst)));
        fs::create_dir_all(&p).unwrap();
        TmpDir(p)
    }
    fn names(&self, dir: &str) -> Vec<String> {
        let mut v: Vec<String> = fs::read_dir(self.0.join(dir)).map(|rd| rd.map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect()).unwrap_or_default();
        v.sort();
        v
    }
}

impl Drop for TmpDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn 저장은_안전한_이름만_받고_폴더_밖에_쓰지_않는다() {
    use antler_lib::update::store::{store_verified, BAD_NAME};
    let tmp = TmpDir::new();
    let dir = tmp.0.join("updates");
    let long = "a".repeat(202);
    for bad in ["../evil.dmg", "..\\evil.dmg", "a/b.dmg", "a\\b.dmg", "", ".hidden.dmg", "/abs.dmg", "C:evil.dmg", "x\0.dmg", "a..b.dmg", "..", "evil.dmg\n", "é.dmg", long.as_str()] {
        assert_eq!(store_verified(&dir, bad, PAYLOAD), Err(BAD_NAME.to_string()), "{bad:?}");
    }
    assert!(!dir.exists(), "거부는 폴더를 만들기 전이다");
    assert!(tmp.names("").is_empty(), "임시 폴더 밖·안 어디에도 파일이 생기지 않았다: {:?}", tmp.names(""));
}

#[test]
fn 저장은_덮어쓰지_않고_같은_내용만_재사용한다() {
    use antler_lib::update::store::{store_verified, EXISTS, SAVE_FAILED};
    let tmp = TmpDir::new();
    let dir = tmp.0.join("updates");
    let name = "Antler-2.0.0-arm64.dmg";
    // 새 파일: 만들고 임시 파일은 남기지 않는다
    assert_eq!(store_verified(&dir, name, PAYLOAD), Ok(dir.join(name)));
    assert_eq!(fs::read(dir.join(name)).unwrap(), PAYLOAD);
    assert_eq!(tmp.names("updates"), [name.to_string()]);
    // 같은 내용을 다시: 재사용(다시 쓰지 않는다)
    assert_eq!(store_verified(&dir, name, PAYLOAD), Ok(dir.join(name)));
    // 다른 내용: 거부하고 기존 파일은 그대로
    assert_eq!(store_verified(&dir, name, b"OTHER"), Err(EXISTS.to_string()));
    assert_eq!(store_verified(&dir, name, &[PAYLOAD, b"x"].concat()), Err(EXISTS.to_string()));
    assert_eq!(fs::read(dir.join(name)).unwrap(), PAYLOAD);
    assert_eq!(tmp.names("updates"), [name.to_string()]);
    // 같은 이름이 폴더다: 거부
    fs::create_dir(dir.join("dir.dmg")).unwrap();
    assert_eq!(store_verified(&dir, "dir.dmg", PAYLOAD), Err(EXISTS.to_string()));
    assert!(dir.join("dir.dmg").is_dir());
    // 이전 실행이 남긴 임시 파일은 지우고 새로 쓴다
    fs::write(dir.join("stale.dmg.part"), b"STALE").unwrap();
    assert_eq!(store_verified(&dir, "stale.dmg", PAYLOAD), Ok(dir.join("stale.dmg")));
    assert_eq!(fs::read(dir.join("stale.dmg")).unwrap(), PAYLOAD);
    assert!(!dir.join("stale.dmg.part").exists());
    // 임시 이름이 폴더면 저장하지 못한다(최종 이름은 생기지 않는다)
    fs::create_dir(dir.join("blocked.dmg.part")).unwrap();
    assert_eq!(store_verified(&dir, "blocked.dmg", PAYLOAD), Err(SAVE_FAILED.to_string()));
    assert!(!dir.join("blocked.dmg").exists());
    // 저장 폴더 자리에 파일이 있으면 저장하지 못한다
    let file_as_dir = tmp.0.join("not-a-dir");
    fs::write(&file_as_dir, b"x").unwrap();
    assert_eq!(store_verified(&file_as_dir, name, PAYLOAD), Err(SAVE_FAILED.to_string()));
}

#[cfg(unix)]
#[test]
fn 저장은_심볼릭_링크를_따라가지_않는다() {
    use antler_lib::update::store::{store_verified, EXISTS};
    let tmp = TmpDir::new();
    let dir = tmp.0.join("updates");
    fs::create_dir_all(&dir).unwrap();
    let victim = tmp.0.join("victim.txt");
    fs::write(&victim, b"SECRET").unwrap();
    // 최종 이름이 (같은 내용을 가리키는) 링크·끊긴 링크여도 거부한다 — 링크 너머에 쓰지 않는다
    std::os::unix::fs::symlink(&victim, dir.join("a.dmg")).unwrap();
    std::os::unix::fs::symlink(tmp.0.join("nowhere"), dir.join("b.dmg")).unwrap();
    assert_eq!(store_verified(&dir, "a.dmg", b"SECRET"), Err(EXISTS.to_string()));
    assert_eq!(store_verified(&dir, "b.dmg", PAYLOAD), Err(EXISTS.to_string()));
    // 임시 이름이 링크면 링크 자체를 지우고(대상은 건드리지 않는다) 새 파일을 만든다
    std::os::unix::fs::symlink(&victim, dir.join("c.dmg.part")).unwrap();
    assert!(store_verified(&dir, "c.dmg", PAYLOAD).is_ok());
    assert_eq!(fs::read(&victim).unwrap(), b"SECRET");
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(fs::metadata(dir.join("c.dmg")).unwrap().permissions().mode() & 0o777, 0o600);
}

#[test]
fn 다시_읽기는_일반_파일만_상한_안에서_읽는다() {
    use antler_lib::update::store::{read_stored, store_verified};
    let tmp = TmpDir::new();
    let dir = tmp.0.join("updates");
    let path = store_verified(&dir, "a.deb", PAYLOAD).unwrap();
    assert_eq!(read_stored(&path), Ok(PAYLOAD.to_vec()));
    assert!(read_stored(&dir.join("none.deb")).is_err());
    assert!(read_stored(&dir).is_err(), "폴더");
    // 상한(300MB)을 넘으면 읽지 않고 거부한다(희소 파일 — 실제로 쓰지 않는다)
    let big = dir.join("big.deb");
    fs::File::create(&big).unwrap().set_len(300_000_001).unwrap();
    assert_eq!(read_stored(&big), Err("설치 파일이 너무 큽니다".to_string()));
}

// ── 소스 검사 ────────────────────────────────────────────────────────────────

#[test]
fn 수동_경로는_설치_포트_앞에서_갈라진다() {
    let host = code("src/update/host.rs");
    // `Engine::install` 의 호출은 NSIS 경로 한 곳이고, 수동 경로(`reopen`)로 갈라지는 줄이 그 앞에 있다
    assert_eq!(host.matches("engine.install(").count(), 1);
    assert!(host.find("return self.reopen(m)").expect("수동 분기") < host.find("engine.install(").expect("설치 포트"));
    // 수동 경로의 내려받기도 저장 전에 서명을 검증한다(검증 줄이 저장 호출보다 앞)
    let dl = host.find("async fn download(").expect("download");
    let body = &host[dl..];
    assert!(body.find("self.engine.verify(").expect("검증") < body.find("self.deliver(").expect("저장"));
    // 저장은 `store_verified` 한 곳, 다시 읽기는 `read_stored` 한 곳이다
    assert_eq!(host.matches("store_verified(").count(), 1);
    assert_eq!(host.matches("read_stored(").count(), 1);
    // 파일을 만들고 쓰는 곳은 `store.rs` 뿐이다
    for rel in ["src/update/host.rs", "src/update/service.rs", "src/update/logic.rs"] {
        let c = code(rel);
        for bad in ["std::fs", "fs::write", "File::create", "OpenOptions", "remove_file", "remove_dir"] {
            assert!(!c.contains(bad), "{rel}: `{bad}`");
        }
    }
}

#[test]
fn 건너뛴_버전을_쓰는_곳은_업데이트_호스트_한_곳이다() {
    let mut files = Vec::new();
    rust_files(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut files);
    let mut callers = Vec::new();
    for f in &files {
        let rel = f.strip_prefix(Path::new(env!("CARGO_MANIFEST_DIR"))).unwrap().to_string_lossy().replace('\\', "/");
        let text = code(&rel);
        // 정의(`pub fn set_skipped_update_version(`)는 빼고 호출만 센다
        let calls = text.matches("set_skipped_update_version(").count() - text.matches("fn set_skipped_update_version(").count();
        if calls > 0 {
            callers.push((rel, calls));
        }
    }
    assert_eq!(callers, [("src/update/host.rs".to_string(), 1)], "건너뛴 버전 변경자의 호출자");
    // 그 한 곳(`write_skipped`)을 부르는 것은 서비스에 주입한 클로저(조립) 하나다
    let lib_and_host = code("src/update/host.rs");
    assert_eq!(lib_and_host.matches("write_skipped(").count(), 2, "정의 하나 + 조립의 주입 하나");
    for rel in ["src/lib.rs", "src/router.rs", "src/update/service.rs", "src/config_host.rs"] {
        assert!(!code(rel).contains("write_skipped"), "{rel}");
    }
}

#[test]
fn capability_에_업데이트_플러그인_권한이_없고_설정에_플러그인이_없다() {
    // 플러그인의 JS command(`check`·`download`·`install`)는 `headers`·`proxy` 를 받는다 — renderer 가 부르지 못해야 한다
    for entry in fs::read_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("capabilities")).unwrap() {
        let text = fs::read_to_string(entry.unwrap().path()).unwrap();
        assert!(!text.contains("updater"), "capability 에 updater 권한이 있다");
    }
    let conf: Value = serde_json::from_str(&read_src("tauri.conf.json")).expect("tauri.conf.json");
    assert!(conf.get("plugins").is_none(), "플러그인 설정은 conf 가 아니라 lib.rs 가 넣는다(공개 키가 UPDATE_PUBKEY 한 곳이어야 한다)");
    // lib.rs 는 conf 를 덮어쓰는 한 곳에서 같은 상수로 설정을 만든다
    let lib = code("src/lib.rs");
    assert_eq!(lib.matches("updater_plugin_config(update::host::UPDATE_PUBKEY)").count(), 1);
    assert_eq!(lib.matches("tauri_plugin_updater::Builder::new().build()").count(), 1);
    let host = code("src/update/host.rs");
    assert!(host.contains("UPDATE_PUBKEY.map(str::to_string)"), "엔진의 키도 같은 상수다");
    assert_eq!(host.matches("UPDATE_PUBKEY").count(), 2, "정의 하나·엔진 하나 (그 밖의 키 출처가 없다. 플러그인 설정은 lib.rs 가 같은 상수로 만든다)");
    assert!(host.contains("enabled: detected.is_some()") && host.contains("let detected = current_target();"), "사용 여부는 번들 표식(과 실행 파일 위치)으로만 정한다");
    assert_eq!(host.matches("std::env::var(").count(), 1, "환경 변수를 읽는 곳은 개발 장치(fake_kind) 하나뿐이다 — 키·사용 여부를 환경 변수로 바꾸지 않는다");
}

#[test]
fn 플러그인은_복원_검증_설치만_쓴다() {
    // `configure_client`(HTTP 클라이언트를 net::http 밖에서 만진다)·플러그인의 다운로드·확인·`download_and_install`·`updater()` 단축은 쓰지 않는다
    let mut files = Vec::new();
    rust_files(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut files);
    for f in &files {
        let rel = f.strip_prefix(Path::new(env!("CARGO_MANIFEST_DIR"))).unwrap().to_string_lossy().replace('\\', "/");
        let text = code(&rel);
        for bad in ["configure_client", "download_and_install", "Update::download", "update.download(", "updater.check(", ".updater()", "checked_update", "tauri_plugin_updater::check"] {
            assert!(!text.contains(bad), "{rel}: `{bad}`");
        }
        // `Update::install` 은 호스트 한 곳(엔진)만 부른다
        if rel != "src/update/host.rs" {
            assert!(!text.contains("update.install(") && !text.contains("restore_update"), "{rel}: 플러그인의 설치·복원은 host.rs 만");
        }
    }
    let host = code("src/update/host.rs");
    assert_eq!(host.matches("update.install(").count(), 1);
    assert_eq!(host.matches("update.verify(").count(), 1);
    assert_eq!(host.matches("restore_update(").count(), 1);
    // 복원은 반드시 검사를 통과한 제안으로 다시 만든 매니페스트 + 고른 키로 고정한 target + 우리가 정한 키·비교기다
    for needle in [".target(offer.platform)", ".pubkey(key)", ".version_comparator(", ".on_before_exit("] {
        assert!(host.contains(needle), "{needle}");
    }
    // 업데이트 요청은 토큰을 보내지 않는다(GitHub 서비스의 클라이언트·토큰과 공유하지 않는다)
    for rel in ["src/update/host.rs", "src/update/service.rs"] {
        let c = code(rel).to_ascii_lowercase();
        assert!(!c.contains("authorization") && !c.contains("bearer") && !c.contains("vault") && !c.contains("secret::"), "{rel}");
    }
}

#[test]
fn 개발_장치_환경_변수는_개발_빌드_코드에만_있다() {
    // 배포 바이너리에는 `ANTLER_UPDATE_FAKE` 문자열 자체가 없어야 한다(PR 2 의 바이너리 가드가 같은 방향으로 본다) — 읽는 함수는 개발·cdp 빌드 cfg 뒤 하나다
    let host = read_src("src/update/host.rs");
    assert_eq!(host.matches("ANTLER_UPDATE_FAKE").count(), 2, "문서 줄 하나 + 읽는 곳 하나");
    let at = host.find("std::env::var(\"ANTLER_UPDATE_FAKE\")").expect("읽는 곳");
    let before: Vec<&str> = host[..at].lines().rev().take(3).collect();
    assert!(before.iter().any(|l| l.trim() == "#[cfg(any(debug_assertions, feature = \"cdp\"))]"), "{before:?}");
    // 다른 소스에는 없다
    let mut files = Vec::new();
    rust_files(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut files);
    for f in files.iter().filter(|f| !f.ends_with("update/host.rs") && !f.ends_with("update\\host.rs")) {
        let rel = f.strip_prefix(Path::new(env!("CARGO_MANIFEST_DIR"))).unwrap().to_string_lossy().replace('\\', "/");
        assert!(!code(&rel).contains("ANTLER_UPDATE_FAKE"), "{rel}");
    }
}

#[test]
fn 시험_키는_이름으로_운영_키와_구별되고_비밀_키는_저장소에_없다() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/update_keys");
    let mut names: Vec<String> = fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    assert_eq!(names.len(), 6);
    for n in &names {
        assert!(n.starts_with("TEST-ONLY-"), "{n}");
        assert!(n.ends_with(".pub") || n.ends_with(".sig"), "비밀 키 파일이 있다: {n}");
        let text = fs::read_to_string(dir.join(n)).unwrap();
        assert!(!text.contains("secret key"), "{n}: 비밀 키 내용");
    }
    // 시험 키는 운영 상수가 아니다
    if let Some(key) = UPDATE_PUBKEY {
        assert!(![PUBKEY_A, PUBKEY_B].iter().any(|k| key_body(k) == key_body(key)));
    }
    assert_ne!(key_body(PUBKEY_A), key_body(PUBKEY_B));
    let _ = root();
}
