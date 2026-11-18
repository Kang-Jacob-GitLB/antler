// jira:*·gh:* 채널 처리(Phase 4 PR 6·9). 통합 테스트(tests/channels.rs·api_route.rs)가 별도 크레이트라 pub 이다
pub mod api_host;
pub mod app_host;
mod build_marker;
mod commands;
// 설정 저장소(Phase 6b PR 1 코어, PR 9 에서 연결 — Rust 가 설정 파일의 유일한 작성자다). 통합 테스트(tests/config.rs)가 별도 크레이트라 pub 이다
pub mod config;
// 설정 소유 채널 11개의 처리(Phase 6b PR 9). 통합 테스트(tests/config_route.rs)가 별도 크레이트라 pub 이다
pub mod config_host;
// 저장소 추가 창의 환경 채널(Phase 6b PR 8)의 처리. 통합 테스트(tests/env_route.rs)가 별도 크레이트라 pub 이다
pub mod env_host;
mod dialogs;
// 외부 도구로 열기(Phase 6b PR 3 순수 계획·실행기 → PR 9 에서 `config_host` 가 연결). 프로세스를 띄우는 `launch::SystemDeps` 는 이 파일이 주입하는 한 곳뿐이다. 통합 테스트(tests/external.rs)가 별도 크레이트라 pub 이다
pub mod external;
mod fetch_host;
pub mod fsguard;
pub mod git;
// Phase 4 의 순수 로직(PR 1)·응답 매퍼(PR 2)·HTTP 클라이언트(PR 3). Jira(PR 6)·GitHub(PR 9)는 api_host 가 부른다.
// 통합 테스트(tests/net.rs)가 별도 크레이트라 pub 이다
pub mod github;
pub mod jira;
pub mod mirror;
pub mod net;
// 저장소 추가 흐름의 순수 로직(Phase 6b PR 4a). env_host(PR 8)가 연결한다. 통합 테스트(tests/ssh.rs)가 별도 크레이트라 pub 이다
pub mod repos;
pub mod router;
// GitHub 서비스의 저장소 질의를 Rust 가 맡는 저장소는 라우터가, 나머지는 설정을 소유한 Rust 가 답한다(Phase 6a PR 8·6b PR 9)
pub mod routed_repos;
// 토큰 보관(Phase 4 PR 4) — Jira(PR 6)·GitHub(PR 9) 서비스가 같은 Vault 를 쓴다
pub mod secret;
mod sidecar;
mod single_instance;
mod snap;
// 앱 업데이트: 판정 로직(Phase 6c PR 1)·상태 기계와 호스트(PR 4 — `update:*` 7채널을 Rust 가 처리한다). 통합 테스트(tests/update*.rs)가 별도 크레이트라 pub 이다
pub mod update;
// 저장소 감시의 앱 쪽 호스트(Phase 6a PR 7b) — 라우터가 만든다
mod watch_host;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tauri::window::Color;
use tauri::{Manager, RunEvent, WebviewUrl, WebviewWindowBuilder, WindowEvent};
use tauri_plugin_window_state::StateFlags;

use config::sync::ConfigSync;
use config::ConfigStore;
use git::exec::ssh::SshPool;
use git::exec::{GitExecutor, GithubExecutor, LocalExecutor, WslExecutor};
use git::fetch::service::FetchService;
use mirror::Target;

/// 첫 페인트 전 흰 화면을 막는다. 토큰 --bg-base(Dark)와 같은 값
const BG: Color = Color(0x14, 0x11, 0x0F, 0xFF);

/// WebView2 실행 인자. 하드웨어 가속은 기본으로 끈다(Electron 판과 같은 이유 — ANTLER_GPU=1 로 켠다)
fn browser_args() -> String {
    let mut args = String::from("--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection");
    if std::env::var("ANTLER_GPU").as_deref() != Ok("1") {
        args.push_str(" --disable-gpu");
    }
    #[cfg(any(debug_assertions, feature = "cdp"))]
    if let Ok(port) = std::env::var("ANTLER_CDP_PORT") {
        if port.chars().all(|c| c.is_ascii_digit()) && !port.is_empty() {
            args.push_str(&format!(" --remote-debugging-port={port}"));
        }
    }
    args
}

/// 앱 자신의 화면 주소인가. 배포 빌드는 tauri.localhost 만, 개발 빌드는 vite dev 서버(devUrl)도 허용한다
fn is_app_url(url: &tauri::Url) -> bool {
    let host = url.host_str().unwrap_or("");
    let scheme_ok = matches!(url.scheme(), "http" | "https" | "tauri");
    if scheme_ok && (host == "tauri.localhost" || url.scheme() == "tauri" && host == "localhost") {
        return true;
    }
    cfg!(debug_assertions) && url.scheme() == "http" && host == "localhost" && url.port() == Some(5174)
}

/// https://github.com 의 링크인가(사용자 정보 없음, 2048자 이하)
fn is_github_link(url: &tauri::Url) -> bool {
    url.as_str().len() <= 2048
        && url.scheme() == "https"
        && url.host_str() == Some("github.com")
        && url.port().is_none()
        && url.username().is_empty()
        && url.password().is_none()
}

/// `jira:openExternal` 이 검증한 주소를 기본 브라우저로 연다. 서비스의 검증(연결한 사이트의 주소)에 더해 https 만 통과시킨다
fn open_https(url: &str) -> Result<(), String> {
    if !url.starts_with("https://") {
        return Err("https 주소만 열 수 있습니다".to_string());
    }
    tauri_plugin_opener::open_url(url, None::<&str>).map_err(|_| "주소를 열지 못했습니다".to_string())
}

/// `dialog:pickFolder` 의 폴더 선택 포트. 시작 위치는 라우터가 관문(`app_host::safe_default_path`)을 통과시킨 값만 온다. 창이 없으면 취소(`None`)다
fn pick_folder_port(app: tauri::AppHandle) -> app_host::PickFolder {
    std::sync::Arc::new(move |default| {
        let mut arg = serde_json::json!({ "mode": "folder", "title": "Git 저장소 선택" });
        if let Some(d) = default {
            arg["defaultPath"] = serde_json::Value::String(d);
        }
        let picked = dialogs::pick_path(&app, &arg);
        Ok(picked.and_then(|v| v.get("filePaths")?.as_array()?.first()?.as_str().map(str::to_string)))
    })
}

/// `clipboard:writeText` 의 클립보드 쓰기 포트(읽기는 없다)
fn clipboard_port(app: tauri::AppHandle) -> app_host::ClipboardWrite {
    use tauri_plugin_clipboard_manager::ClipboardExt;
    std::sync::Arc::new(move |text| app.clipboard().write_text(text.to_string()).map_err(|_| "복사하지 못했습니다".to_string()))
}

/// `external:pickEditorExe` 의 파일 선택 포트(`.exe`/`.com` 필터는 Windows 만 — TS 와 같다). 창이 없으면 취소(`None`)다
fn pick_exe_port(app: tauri::AppHandle) -> config_host::PickExe {
    Arc::new(move || {
        let mut arg = serde_json::json!({ "mode": "file", "title": "에디터 실행 파일 선택" });
        if cfg!(windows) {
            arg["filters"] = serde_json::json!([{ "name": "실행 파일", "extensions": ["exe", "com"] }]);
        }
        let picked = dialogs::pick_path(&app, &arg);
        Ok(picked.and_then(|v| v.get("filePaths")?.as_array()?.first()?.as_str().map(str::to_string)))
    })
}

/// 설정 파일을 연다(Rust 가 설정을 소유할 때만 — `ANTLER_RUST_GIT=0` 이면 부르지 않는다. 그 실행은 사이드카가 파일의 유일한 작성자다).
/// Tauri 설정이 **없을 때마다** Electron 판의 설정을 읽어 가져온다(사용자 결정 D1 — `config::import`: Electron 파일은 읽기만, `trustedConfigs`·창 위치·GitHub 캐시·토큰은 가져오지 않는다. 설정 파일을 지우면 다시 가져온다 — 의도).
/// `ANTLER_USER_DATA` 로 도는 측정·자동화 실행은 가져오지 않는다(실사용 설정을 건드리지 않는다) — 판정은 `config::import::should_import` 한 곳이다
fn open_config(user_data: &Path, cache_dir: &str) -> Arc<ConfigStore> {
    let file = user_data.join("antler.config.json");
    if config::import::should_import(std::env::var_os("ANTLER_USER_DATA").as_deref(), &file) {
        if let Some(electron) = config::import::electron_config_file(std::env::var_os("APPDATA").as_deref()) {
            match config::import::import_electron_config(&file, &electron, cache_dir) {
                Ok(true) => eprintln!("[config] Electron 판 설정을 가져왔다"),
                Ok(false) => {}
                Err(e) => eprintln!("[config] Electron 판 설정을 가져오지 못했다: {e}"),
            }
        }
    }
    // 계약: github 저장소의 경로를 이 앱의 캐시 폴더·링크로 다시 정하도록 `cache_dir` 에 반드시 `Some` 을 준다
    Arc::new(ConfigStore::open(file, Some(cache_dir.to_string())))
}

/// 환경 채널(`env:*`·`repos:scan`)의 운영 포트. 확인 창은 `dialogs::message_box`(취소가 기본 버튼) — 실패·닫기·Esc 는 거부다
fn env_host_port(app: tauri::AppHandle, home: Option<String>) -> env_host::EnvHost {
    use std::sync::Arc;
    env_host::EnvHost::new(
        Arc::new(|| Box::pin(git::exec::list_distros())),
        Arc::new(move || home.as_deref().map(env_host::ssh_hosts_for_home).unwrap_or_default()),
        Arc::new(move |arg| dialogs::message_box(&app, arg).and_then(|v| v.get("response")?.as_i64()) == Some(0)),
        Arc::new(|kind, distro| match (kind, distro) {
            ("wsl", Some(distro)) => GitExecutor::Wsl(WslExecutor::new(distro)),
            _ => GitExecutor::Local(LocalExecutor::default()),
        }),
    )
}


pub fn run() {
    build_marker::announce();
    commands::mark_start();
    let mut builder = tauri::Builder::default();
    // 단일 인스턴스 — **다른 플러그인보다 먼저** 등록한다(플러그인 문서: 두 번째 실행은 나머지 설정이 돌기 전에 감지돼 종료해야 한다).
    // 두 번째 실행은 기존 창을 앞으로 가져오고 끝난다(single_instance.rs). ANTLER_USER_DATA 로 도는 측정·자동화 실행은 등록하지 않는다
    if single_instance::enabled(std::env::var_os("ANTLER_USER_DATA").as_deref()) {
        builder = builder.plugin(tauri_plugin_single_instance::init(single_instance::on_second_instance));
    }
    builder = builder
        .plugin(tauri_plugin_clipboard_manager::init())
        // 확인 창·폴더 선택은 Rust(dialogs.rs)에서만 띄운다. capability 에 dialog 권한이 없어 renderer 는 부를 수 없다
        .plugin(tauri_plugin_dialog::init());
    // 창 위치·크기·최대화만 기억한다(visible·decorations·fullscreen 은 제외). ANTLER_USER_DATA 로 도는 측정·자동화 실행은
    // 실사용 창 상태를 덮어쓰지 않게 등록하지 않는다. 플러그인의 JS command 도 capability 에 주지 않는다
    if std::env::var_os("ANTLER_USER_DATA").is_none() {
        builder = builder.plugin(
            tauri_plugin_window_state::Builder::new()
                .with_state_flags(StateFlags::SIZE | StateFlags::POSITION | StateFlags::MAXIMIZED)
                .build(),
        );
    }
    // 업데이트 플러그인(Phase 6c PR 4): 서명 검증·설치에만 쓴다(`update::host`). 플러그인 설정은 tauri.conf.json 이 아니라 여기서 넣는다 — 믿는 공개 키가 `update::host::UPDATE_PUBKEY` 한 곳이어야 해서다.
    // JS command 는 capability 에 권한을 주지 않아 renderer 가 부를 수 없다(`tests/update_security.rs` 가 강제한다)
    let mut context = tauri::generate_context!();
    context.config_mut().plugins.0.insert("updater".to_string(), update::host::updater_plugin_config(update::host::UPDATE_PUBKEY));
    builder = builder.plugin(tauri_plugin_updater::Builder::new().build());
    builder
        .invoke_handler(tauri::generate_handler![
            snap::show_snap_overlay,
            commands::antler_invoke,
            commands::antler_uptime_ms
        ])
        .setup(|app| {
            // Electron 판의 %APPDATA%\Antler 와 설정이 섞이지 않게 기본값은 Tauri 의 app_data_dir 다
            let user_data: PathBuf = match std::env::var_os("ANTLER_USER_DATA") {
                Some(p) => p.into(),
                None => app.path().app_data_dir()?,
            };
            std::fs::create_dir_all(&user_data)?;
            // 라우터와 사이드카가 같은 미러를 본다(Rust 가 설정이 바뀔 때 채우고(`ConfigSync`) 라우터가 읽는다. `ANTLER_RUST_GIT=0` 이면 사이드카가 채운다)
            let mirror = mirror::RepoMirror::default();
            let router = router::Router::from_env(mirror.clone());
            // Rust 가 맡는 저장소의 감시(Phase 6a PR 7b)가 ref 변화를 알리는 곳 — 사이드카의 `repo:changed` 와 같은 모양의 renderer 이벤트
            router.set_change_sink(fetch_host::change_sink(app.handle().clone()));
            // 설정이 필요 없는 앱 채널(Phase 6b PR 5)의 포트: 폴더 선택(dialogs.rs)·클립보드(플러그인)·링크 열기(open_https). 스위치가 꺼져 있으면 주입하지 않는다
            router.set_app(app_host::AppHost {
                pick_folder: pick_folder_port(app.handle().clone()),
                clipboard: clipboard_port(app.handle().clone()),
                open_link: std::sync::Arc::new(open_https),
            });
            // SSH 풀(Phase 5 PR 3). 스위치가 꺼져 있으면(ANTLER_RUST_GIT=0) 만들지 않는다 — ssh 저장소는 사이드카가 전부 맡는다. 하위 스위치 ANTLER_RUST_SSH 는 은퇴했다(Phase 6b PR 9, 결정 D5).
            // 풀을 주입해야 미러가 ssh 를 Rust 로 풀기 시작한다(Router::ssh_active). 홈을 알 수 없으면 주입하지 않는다(설정을 소유한 실행에서는 Rust 가 "SSH 를 사용할 수 없습니다" 로 답한다).
            // **fetch 서비스보다 먼저다** — 서비스의 실행기 공장이 이 풀을 쓴다
            // 홈은 Node `os.homedir()`(libuv)와 같은 순서로 정한다 — 환경변수(Windows `USERPROFILE`·그 밖 `HOME`)가 있으면 그것, 없으면 Tauri 의 `home_dir`.
            // 그래야 사이드카와 Rust 가 같은 `~/.ssh` 를 읽는다
            let env_home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).filter(|v| !v.is_empty()).map(PathBuf::from);
            let home: Option<PathBuf> = env_home.or_else(|| app.path().home_dir().ok());
            if router.ssh_enabled() {
                match &home {
                    Some(home) => router.set_ssh_pool(SshPool::for_home(&home.to_string_lossy())),
                    None => eprintln!("[ssh] 홈 디렉터리를 알 수 없다 — 사이드카가 SSH 를 처리한다"),
                }
            }
            // 저장소 추가 창의 환경 채널(Phase 6b PR 8)의 포트: WSL 목록(레지스트리 관문 뒤의 `list_distros` — `wsl.exe` 를 직접 부르지 않는다)·`~/.ssh/config` 별칭(홈을 모르면 빈 목록)·
            // 설정에 없는 SSH 호스트의 확인 창(dialogs.rs)·local/wsl 실행기. ssh 연결은 위의 풀이 맡는다(풀이 없으면 ssh 종류는 Rust 가 오류로 답한다). 스위치가 꺼져 있으면 주입하지 않는다
            let env_host = env_host_port(app.handle().clone(), home.map(|h| h.to_string_lossy().into_owned()));
            router.set_env(env_host.clone());
            // 설정 소유(Phase 6b PR 9): **Rust 가 설정 파일의 유일한 작성자다**. 스위치가 켜져 있을 때만(`router.enabled()` 하나가 소유 술어다) 파일을 열고 — 꺼져 있으면(`ANTLER_RUST_GIT=0`)
            // 열지도 쓰지도 않고 사이드카가 전부 맡는다. 변경 알림이 미러(`RepoMirror`)·fetch 설정을 직접 맞춘다 — 사이드카가 보내던 `repos`·`fetchPrefs` 줄을 대신한다.
            // 반영 뒤 할 일(`on_mirror_changed`·`apply_fetch_prefs`)은 라우터가 managed 된 뒤에 찾는다(알림은 어느 스레드에서 오든 앱 런타임에 들어가서 부른다)
            let cache_dir = user_data.join("remote-repos").to_string_lossy().into_owned();
            let config_store: Option<Arc<ConfigStore>> = router.enabled().then(|| open_config(&user_data, &cache_dir));
            let config_sync: Option<Arc<ConfigSync>> = config_store.as_ref().map(|store| {
                let (mirror_app, prefs_app) = (app.handle().clone(), app.handle().clone());
                let sync = Arc::new(ConfigSync::new(
                    mirror.clone(),
                    Arc::new(move || {
                        if let Some(r) = mirror_app.try_state::<router::Router>() {
                            router::in_runtime_of(tauri::async_runtime::handle().inner(), || r.on_mirror_changed());
                        }
                    }),
                    Arc::new(move |prefs| {
                        if let Some(r) = prefs_app.try_state::<router::Router>() {
                            router::in_runtime_of(tauri::async_runtime::handle().inner(), || r.apply_fetch_prefs(prefs));
                        }
                    }),
                ));
                let listener = sync.clone();
                store.set_on_change(Box::new(move |gen, config| listener.push(Some(gen), config)));
                sync
            });
            let config_host: Option<Arc<config_host::ConfigHost>> = config_store.as_ref().map(|store| {
                Arc::new(config_host::ConfigHost::new(store.clone(), env_host.clone(), cache_dir.clone(), pick_exe_port(app.handle().clone()), Arc::new(external::launch::SystemDeps)))
            });
            if let Some(host) = &config_host {
                router.set_config(host.clone());
            }
            // Jira·GitHub 서비스(PR 6·9). 스위치가 꺼져 있으면(ANTLER_RUST_GIT=0) 서비스도 보관소도 만들지 않는다 — 키체인을 읽지 않고 사이드카가 전부 맡는다(하위 스위치 ANTLER_RUST_API 는 은퇴했다).
            // 켜져 있으면 보관소(ANTLER_USER_DATA 면 메모리, 아니면 자격 증명 관리자) → Vault → 서비스 순으로 만들고, 저장된 연결·로그인은 여기서 복원한다.
            // **둘 다** 만들어졌을 때만 둘 다 주입한다. 하나라도 만들지 못하면(키체인 오류 등) Rust 가 설정을 소유하므로 jira:*·gh:* 는 오류로 답한다 — 사이드카는 설정·토큰을 모른다
            // github 캐시 실행기의 복제(라우터가 받은 것과 같은 `LiveProcs` 를 본다) — 아래 fetch 서비스의 github 실행기가 된다. 라우터가 받지 않았으면(서비스 없음·스위치 꺼짐) 없다
            let mut github_exec: Option<GithubExecutor> = None;
            // 위 실행기가 읽는 네트워크 설정 칸(읽기 완료 신호를 fetch 서비스가 기다린다)
            let mut github_net: Option<git::exec::NetConfig> = None;
            // 서비스는 설정을 소유한 Rust 에서만 만든다(`config_store` 가 있다 = 스위치가 켜져 있다). 하위 스위치 `ANTLER_RUST_API` 는 은퇴했다(결정 D5)
            if let (true, Some(store)) = (router.api_enabled(), config_store.clone()) {
                let vault = std::sync::Arc::new(secret::Vault::new(secret::open_store(std::env::var_os("ANTLER_USER_DATA").as_deref())));
                let jira = jira::service::JiraService::new(vault.clone());
                // 저장소 정보: Rust 가 git 을 맡는 저장소(local·WSL·SSH·github)는 라우터가 직접 답하고, 나머지와 설정 읽기는 설정(`ConfigRepos`)에서 답한다 — 사이드카에 묻지 않는다.
                // 라우터는 이 서비스보다 늦게 만들어지므로 호출 때 찾는다
                let repos = std::sync::Arc::new(routed_repos::RoutedRepos::new(routed_repos::AppRouter(app.handle().clone()), std::sync::Arc::new(routed_repos::ConfigRepos::new(store))));
                let cli: std::sync::Arc<dyn github::ports::ExternalTokenProvider> = std::sync::Arc::new(github::gh_cli::GhCli::new());
                let opts = github::service::ServiceOptions { avatar_cache_file: Some(user_data.join(github::avatars::AVATAR_CACHE_FILE)), ..Default::default() };
                let gh = github::service::GitHubService::new(vault, repos, Some(cli), opts);
                match (jira, gh) {
                    (Ok(jira), Ok(gh)) => {
                        router.set_jira(api_host::JiraHost { service: jira, opener: std::sync::Arc::new(open_https) });
                        router.set_github(api_host::GithubHost { service: gh.clone(), opener: std::sync::Arc::new(open_https) });
                        // GitHub 원격 전용 저장소의 조회(Phase 6b PR 6): 토큰은 이 서비스의 것이 호출 때마다 읽혀 실행기의 환경 변수로만 간다(로그인·로그아웃이 바로 반영된다).
                        // 사용자 git 설정의 네트워크 키(프록시·인증서)는 앱 시작 때 한 번 읽는다 — 읽는 동안·읽지 못하면 빈 목록이다(TS 의 `netConfig` 와 같다).
                        // fetch·감시도 같은 스위치로 Rust 가 맡는다(PR 7 — `rust_fetch_kinds` 의 github). 서비스가 없거나 스위치가 꺼져 있으면 주입이 무시되고 github 는 사이드카가 전부 처리한다
                        if router.enabled() {
                            // 읽기가 끝나면 신호가 선다 — fetch 서비스가 첫 fetch 앞에서 그것을 기다린다(TS `githubReady`, 상한 5초)
                            let net_config = git::exec::NetConfig::default();
                            let (slot, reader, token_svc) = (net_config.clone(), net_config.clone(), gh.clone());
                            tauri::async_runtime::spawn(async move {
                                slot.set(git::github_cache::read_network_config("git").await);
                            });
                            github_net = Some(net_config);
                            let exec = GithubExecutor::new(
                                "git",
                                user_data.join("remote-repos").to_string_lossy().into_owned(),
                                user_data.join("no-hooks").to_string_lossy().into_owned(),
                                std::sync::Arc::new(move || token_svc.git_token()),
                                std::sync::Arc::new(move || reader.get()),
                            );
                            router.set_github_cache(exec.clone());
                            github_exec = router.github_cache_active().then_some(exec);
                        }
                        // 토큰이 바뀌면(로그인·로그아웃·폐기) 인증 문제로 멈춘 원격 전용 저장소의 fetch 를 다시 연다(TS `onAuthChanged` → `resumeGithubAuth`). 토큰은 사이드카로 보내지 않는다 —
                        // Rust 가 GitHub 토큰의 유일한 쥔 쪽이다(Phase 6b PR 9 부터 `ghToken` 줄도 없다). 서비스가 자기 자신을 쥐는 고리라 앱이 끝날 때까지 산다.
                        // 서비스가 어느 스레드에서 부르는지 모르므로 앱 런타임에 들어가서 라우터를 부른다
                        let handle = app.handle().clone();
                        gh.set_on_auth_changed(move || {
                            if let Some(r) = handle.try_state::<router::Router>() {
                                router::in_runtime_of(tauri::async_runtime::handle().inner(), || r.github_auth_changed());
                            }
                        });
                    }
                    (jira, gh) => {
                        if let Err(e) = jira {
                            eprintln!("[api] Jira 서비스를 만들지 못했다({e}) — jira:*·gh:* 는 오류로 답한다(사이드카는 설정·토큰을 모른다)");
                        }
                        if let Err(e) = gh {
                            eprintln!("[api] GitHub 서비스를 만들지 못했다({e}) — jira:*·gh:* 는 오류로 답한다(사이드카는 설정·토큰을 모른다)");
                        }
                    }
                }
            }
            // fetch 서비스(local·wsl 저장소, 그리고 Rust 가 푸는 ssh·github). 스위치가 꺼져 있으면(ANTLER_RUST_GIT=0) 만들지 않는다 — 사이드카가 전부 맡는다.
            // **SSH 풀·GitHub 캐시 실행기보다 뒤다** — 실행기 공장이 둘을 쓰고, 미러의 fetch 소유 판정(`ssh_active`·`github_cache_active`)이 같은 값을 따른다
            if let (true, Some(store)) = (router.enabled(), config_store.clone()) {
                let ssh_pool = router.ssh_pool();
                let host = fetch_host::AppFetchHost::new(app.handle().clone(), mirror.clone(), router.note_sink(), store);
                let no_hooks_dir = user_data.join("no-hooks").to_string_lossy().into_owned();
                let runner_mirror = mirror.clone();
                let github_runner = github_exec.clone();
                let make_runner = move |target: &Target| match target {
                    Target::Local { .. } => std::sync::Arc::new(GitExecutor::Local(LocalExecutor::default())),
                    Target::Wsl { distro, .. } => std::sync::Arc::new(GitExecutor::Wsl(WslExecutor::new(distro.clone()))),
                    // ssh fetch(Phase 5 PR 4): 라우터와 같은 풀의 실행기(호스트당 연결 하나·채널 6개 한도 공유)를 쓴다. 풀이 닫은 실행기는 서비스가 `is_closed` 로 알아보고 새로 받는다.
                    // 호스트가 아직 미러에 있는지는 **풀 잠금 안에서** 다시 본다(`get_if`) — 서비스의 확인은 라우터와 다른 잠금이라, 그 사이 정리된 호스트를 닫히지 않은 채 풀에 다시 넣지 않게
                    Target::Ssh { host, .. } => std::sync::Arc::new(GitExecutor::Ssh(
                        ssh_pool.as_ref().and_then(|pool| pool.get_if(host, || runner_mirror.ssh_hosts().contains(host))).unwrap_or_else(|| {
                            // 풀이 없거나(오지 않는 길) 호스트가 방금 빠졌다 — 어떤 설정 파일도 읽지 않게 닫힌 실행기를 준다(모든 호출이 거부되고, 서비스는 다음 resolve 에서 버린다)
                            let closed = SshPool::for_home("").get(host);
                            closed.dispose();
                            closed
                        }),
                    )),
                    // github fetch(Phase 6b PR 7): **라우터가 받은 것과 같은 캐시 실행기**(복제 — 진행 중 프로세스 표가 같다. `repos:remove` 가 fetch 프로세스도 기다릴 수 있다).
                    // 실행기가 없으면(`resolve_fetch` 가 github 를 풀지 않으므로 오지 않는 길) 일반 fetch 의 허용 목록·환경으로 앱 캐시를 받는 일이 없게 닫힌 실행기(모든 호출 거부)를 준다
                    Target::Github { .. } => match &github_runner {
                        Some(exec) => std::sync::Arc::new(GitExecutor::Github(exec.clone())),
                        None => std::sync::Arc::new(GitExecutor::Ssh({
                            let closed = SshPool::for_home("").get("github");
                            closed.dispose();
                            closed
                        })),
                    },
                };
                let mut service = FetchService::new(host, mirror.clone(), make_runner, Some(no_hooks_dir));
                if github_exec.is_some() {
                    service = service.with_github_cache(user_data.join("remote-repos").to_string_lossy().into_owned());
                    if let Some(net) = github_net.clone() {
                        service = service.with_github_net_ready(net);
                    }
                }
                router.set_fetch(std::sync::Arc::new(service));
            }
            // 앱 업데이트(Phase 6c PR 4): 설정을 소유한 실행(스위치가 켜져 있다)에서 Rust 가 `update:*` 7채널을 처리한다. 만들지 못하면(TLS 초기화 실패) 주입하지 않고 사이드카가 맡는다(되돌리기).
            // 설치 프로그램을 띄운 뒤 `Update::install` 이 `exit(0)` 으로 끝나 `RunEvent::Exit` 을 건너뛰므로, 그 직전에 같은 종료 정리를 한다
            if let (true, Some(store)) = (router.enabled(), config_store.clone()) {
                let exit_app = app.handle().clone();
                let before_exit: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
                    exit_cleanup(&exit_app);
                    exit_app.cleanup_before_exit();
                });
                match update::host::build_service(app.handle().clone(), store, before_exit) {
                    Ok(service) => {
                        router.set_update(Arc::new(service.clone()));
                        service.start(tauri::async_runtime::handle().inner());
                    }
                    Err(e) => eprintln!("[update] 업데이트 서비스를 만들지 못했다({e}) — update:* 는 사이드카가 처리한다"),
                }
            }
            // jira·gh 의 소유를 기록한다(두 서비스가 실제로 만들어졌을 때만 켜진다 — 만들지 못했으면 서비스 없는 채널은 Rust 가 오류로 답한다). 사이드카에 알릴 값이 아니다(PR 10)
            router.claim_api();
            let config_owned = router.config_owned();
            app.manage(router);
            // 설정의 첫 반영(미러·fetch 설정): 라우터가 managed 된 뒤여야 한다(반영 뒤 할 일이 라우터를 찾는다 — 서비스는 위에서 주입했으므로 예약된 주기·승인이 바로 들어간다).
            // 렌더러의 첫 호출·사이드카 기동보다 먼저다
            if let (Some(sync), Some(store)) = (&config_sync, &config_store) {
                sync.push(None, &store.get());
            }
            app.manage(sidecar::Sidecar::spawn(app.handle(), &user_data, mirror, config_owned)?);
            // 시작 때 한 번: 어떤 레코드도 가리키지 않는 원격 전용 저장소 캐시 폴더(삭제에 실패했거나 로드 때 버려진 레코드·중단된 추가)를 지우고, 폴더가 없는 레코드(Electron 설정 가져오기)의 빈 캐시를 다시 만든다.
            // 설정을 읽지 못한 채 시작했으면 지우지 않는다(`load_failed`). 앱 시작을 막지 않게 따로 돌린다
            if let Some(host) = config_host {
                let exec = app.try_state::<router::Router>().and_then(|r| r.github_executor());
                tauri::async_runtime::spawn(async move {
                    let removed = repos::manage::cleanup_github_cache(host.store(), host.cache_dir(), host.locks()).await;
                    let restored = repos::manage::restore_missing_github_caches(host.store(), exec, host.cache_dir(), host.locks()).await;
                    if removed > 0 || restored > 0 {
                        eprintln!("[repos] 원격 저장소 캐시 정리: 고아 {removed}개 삭제, 빈 캐시 {restored}개 복구");
                    }
                });
            }

            let mut win = WebviewWindowBuilder::new(app, "main", WebviewUrl::default())
                .title("Antler")
                .inner_size(1440.0, 900.0)
                .min_inner_size(960.0, 600.0)
                .decorations(false)
                // 앱이 HTML5 드래그 앤 드롭을 쓴다 — Tauri 의 파일 드롭 처리가 이를 가로채지 않게 끈다
                .disable_drag_drop_handler()
                .background_color(BG)
                .data_directory(user_data.join("webview2"))
                .additional_browser_args(&browser_args())
                // src/main/index.ts 의 will-navigate·setWindowOpenHandler·권한 거부와 같다.
                // 드롭한 파일·링크로 창이 앱 밖 주소로 바뀌어 앱처럼 보이는 가짜 화면을 띄우지 못하게 한다
                .on_navigation(is_app_url)
                .on_new_window(|url, _| {
                    // 새 창은 열지 않고, github.com 링크만 기본 브라우저로 보낸다(src/main/github/validate.ts 의 allowedExternalUrl 과 같은 규칙)
                    if is_github_link(&url) {
                        let _ = tauri_plugin_opener::open_url(url.as_str(), None::<&str>);
                    }
                    tauri::webview::NewWindowResponse::Deny
                })
                .on_permission_request(|_, _| tauri::webview::PermissionResponse::Deny);
            if std::env::var("ANTLER_MEASURE").as_deref() == Ok("1") {
                win = win.initialization_script("window.__ANTLER_MEASURE__=true");
            }
            win.build()?;
            Ok(())
        })
        .on_window_event(|window, event| {
            // 창이 뒤에 있을 때는 원격 폴링을 늦춘다(src/main/index.ts 의 focus·blur 처리와 같다)
            if let WindowEvent::Focused(focused) = event {
                if let Some(s) = window.app_handle().try_state::<sidecar::Sidecar>() {
                    s.set_focus(*focused);
                }
                if let Some(r) = window.app_handle().try_state::<router::Router>() {
                    // Rust 가 맡는 저장소의 감시(포커스를 얻으면 바로 확인하고, 잃으면 폴링을 늦춘다). 플래그와 `Notify` 만 만지므로 런타임 밖 스레드에서 불러도 된다
                    r.watch_focus(*focused);
                    // Rust 가 맡는 저장소의 자동 fetch(밀렸으면 바로). 이 스레드는 런타임 밖이라 들어가서 부른다
                    if *focused {
                        router::in_runtime_of(tauri::async_runtime::handle().inner(), || r.fetch_on_focus());
                    }
                }
            }
        })
        .build(context)
        .expect("Tauri 앱 빌드 실패")
        .run(|app, event| {
            if let RunEvent::Exit = event {
                exit_cleanup(app);
            }
        });
}

/// 앱이 끝나기 전의 정리. `RunEvent::Exit` 와 업데이트 설치(`Update::install` 의 `on_before_exit` — 설치 프로그램을 띄우고 `exit(0)` 으로 끝나 `RunEvent::Exit` 을 건너뛴다)가 **같은 함수**를 부른다
fn exit_cleanup(app: &tauri::AppHandle) {
    // 열려 있는 로그 커서(멈춘 git log)를 먼저 닫는다
    if let Some(r) = app.try_state::<router::Router>() {
        r.shutdown();
    }
    // 폴백 taskkill 이 진행 중이면 기다린다(Job 으로 끝나는 보통의 경로에는 없다). 종료 때 CreateProcess 가 끊기면 정지된 고아가 남는다
    git::exec::shutdown_kills(std::time::Duration::from_secs(2));
    if let Some(s) = app.try_state::<sidecar::Sidecar>() {
        s.kill();
    }
}
