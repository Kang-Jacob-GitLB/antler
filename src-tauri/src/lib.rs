// jira:*·gh:* 채널 처리(Phase 4 PR 6·9). 통합 테스트(tests/channels.rs·api_route.rs)가 별도 크레이트라 pub 이다
pub mod api_host;
mod commands;
mod dialogs;
mod fetch_host;
pub mod git;
// Phase 4 의 순수 로직(PR 1)·응답 매퍼(PR 2)·HTTP 클라이언트(PR 3). Jira(PR 6)·GitHub(PR 9)는 api_host 가 부른다.
// 통합 테스트(tests/net.rs)가 별도 크레이트라 pub 이다
pub mod github;
pub mod jira;
pub mod mirror;
pub mod net;
// Rust → Node 질의(`repoq`)·토큰 사본(`ghToken`)의 메시지와 검증(Phase 4 PR 9). 통합 테스트(tests/repoq.rs)가 별도 크레이트라 pub 이다
pub mod repoq;
pub mod router;
// 토큰 보관(Phase 4 PR 4) — Jira(PR 6)·GitHub(PR 9) 서비스가 같은 Vault 를 쓴다
pub mod secret;
mod sidecar;
mod snap;

use std::path::PathBuf;

use tauri::window::Color;
use tauri::{Manager, RunEvent, WebviewUrl, WebviewWindowBuilder, WindowEvent};
use tauri_plugin_window_state::StateFlags;

use git::exec::ssh::SshPool;
use git::exec::{GitExecutor, LocalExecutor, WslExecutor};
use git::fetch::service::{FetchPrefsSink, FetchService};
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

pub fn run() {
    commands::mark_start();
    let mut builder = tauri::Builder::default()
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
            // 라우터와 사이드카가 같은 미러를 본다(사이드카가 채우고 라우터가 읽는다)
            let mirror = mirror::RepoMirror::default();
            let router = router::Router::from_env(mirror.clone());
            // Rust 가 처리한 git:log 가 사이드카에 알리는 일(첫 페이지·이력 변경). 사이드카는 이 뒤에 managed 된다
            let handle = app.handle().clone();
            router.set_note_sink(std::sync::Arc::new(move |op, repo_id| {
                if let Some(s) = handle.try_state::<sidecar::Sidecar>() {
                    s.note(op, repo_id);
                }
            }));
            // SSH 풀(Phase 5 PR 3). 스위치가 꺼져 있으면(ANTLER_RUST_SSH=0·ANTLER_RUST_GIT=0) 만들지 않는다 — ssh 저장소는 사이드카가 전부 맡는다.
            // 풀을 주입해야 미러가 ssh 를 Rust 로 풀기 시작한다(Router::ssh_active). 홈을 알 수 없으면 주입하지 않는다(사이드카가 처리한다).
            // **fetch 서비스·사이드카 env 보다 먼저다** — 서비스의 실행기 공장이 이 풀을 쓰고, 사이드카 env 가 같은 판정(ssh_active)을 따른다
            // 홈은 Node `os.homedir()`(libuv)와 같은 순서로 정한다 — 환경변수(Windows `USERPROFILE`·그 밖 `HOME`)가 있으면 그것, 없으면 Tauri 의 `home_dir`.
            // 그래야 사이드카와 Rust 가 같은 `~/.ssh` 를 읽는다
            if router.ssh_enabled() {
                let env_home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).filter(|v| !v.is_empty()).map(PathBuf::from);
                match env_home.or_else(|| app.path().home_dir().ok()) {
                    Some(home) => router.set_ssh_pool(SshPool::for_home(&home.to_string_lossy())),
                    None => eprintln!("[ssh] 홈 디렉터리를 알 수 없다 — 사이드카가 SSH 를 처리한다"),
                }
            }
            // fetch 서비스(local·wsl 저장소). 스위치가 꺼져 있으면(ANTLER_RUST_GIT=0) 만들지 않는다 — 사이드카가 전부 맡는다
            if router.enabled() {
                let ssh_pool = router.ssh_pool();
                let host = fetch_host::AppFetchHost::new(app.handle().clone(), mirror.clone());
                let no_hooks_dir = user_data.join("no-hooks").to_string_lossy().into_owned();
                let make_runner = move |target: &Target| match target {
                    Target::Local { .. } => std::sync::Arc::new(GitExecutor::Local(LocalExecutor::default())),
                    Target::Wsl { distro, .. } => std::sync::Arc::new(GitExecutor::Wsl(WslExecutor::new(distro.clone()))),
                    // ssh fetch(Phase 5 PR 4): 라우터와 같은 풀의 실행기(호스트당 연결 하나·채널 6개 한도 공유)를 쓴다. 풀이 닫은 실행기는 서비스가 `is_closed` 로 알아보고 새로 받는다
                    Target::Ssh { host, .. } => std::sync::Arc::new(GitExecutor::Ssh(match &ssh_pool {
                        Some(pool) => pool.get(host),
                        // 풀이 없으면 오지 않는 길이다 — 닿아도 어떤 설정 파일도 읽지 않게 닫힌 실행기를 준다(모든 호출이 거부된다)
                        None => {
                            let closed = SshPool::for_home("").get(host);
                            closed.dispose();
                            closed
                        }
                    })),
                };
                router.set_fetch(std::sync::Arc::new(FetchService::new(host, mirror.clone(), make_runner, Some(no_hooks_dir))));
            }
            // Jira·GitHub 서비스(PR 6·9). 스위치가 꺼져 있으면(ANTLER_RUST_API=0·ANTLER_RUST_GIT=0) 서비스도 보관소도 만들지 않는다 — 키체인을 읽지 않고 사이드카가 전부 맡는다.
            // 켜져 있으면 보관소(ANTLER_USER_DATA 면 메모리, 아니면 자격 증명 관리자) → Vault → 서비스 순으로 만들고, 저장된 연결·로그인은 여기서 복원한다.
            // 사이드카의 env(ANTLER_RUST_API) 하나가 두 서비스를 함께 넘기므로 **둘 다** 만들어졌을 때만 둘 다 주입한다(하나만 되면 사이드카가 둘 다 처리한다)
            let mut gh_service: Option<github::service::GitHubService> = None;
            if router.api_enabled() {
                let vault = std::sync::Arc::new(secret::Vault::new(secret::open_store(std::env::var_os("ANTLER_USER_DATA").as_deref())));
                let jira = jira::service::JiraService::new(vault.clone());
                // 저장소 정보는 사이드카에 묻는다(repoq). 사이드카는 이 서비스보다 늦게 만들어지므로 전송이 호출 때 찾는다
                let repos = std::sync::Arc::new(repoq::SidecarRepos::new(std::sync::Arc::new(sidecar::SidecarQuery::new(app.handle().clone()))));
                let cli: std::sync::Arc<dyn github::ports::ExternalTokenProvider> = std::sync::Arc::new(github::gh_cli::GhCli::new());
                let opts = github::service::ServiceOptions { avatar_cache_file: Some(user_data.join(github::avatars::AVATAR_CACHE_FILE)), ..Default::default() };
                let gh = github::service::GitHubService::new(vault, repos, Some(cli), opts);
                match (jira, gh) {
                    (Ok(jira), Ok(gh)) => {
                        router.set_jira(api_host::JiraHost { service: jira, opener: std::sync::Arc::new(open_https) });
                        router.set_github(api_host::GithubHost { service: gh.clone(), opener: std::sync::Arc::new(open_https) });
                        // 토큰이 바뀌면(로그인·로그아웃·폐기) 사이드카의 메모리 사본을 갱신한다. 서비스가 자기 자신을 쥐는 고리라 앱이 끝날 때까지 산다
                        let (handle, svc) = (app.handle().clone(), gh.clone());
                        gh.set_on_auth_changed(move || {
                            if let Some(s) = handle.try_state::<sidecar::Sidecar>() {
                                s.push_gh_token(|| svc.git_token());
                            }
                        });
                        gh_service = Some(gh);
                    }
                    (jira, gh) => {
                        if let Err(e) = jira {
                            eprintln!("[api] Jira 서비스를 만들지 못했다({e}) — 사이드카가 처리한다");
                        }
                        if let Err(e) = gh {
                            eprintln!("[api] GitHub 서비스를 만들지 못했다({e}) — 사이드카가 처리한다");
                        }
                    }
                }
            }
            // 사이드카에 ssh 를 Rust 가 처리함을 알리는 값. 미러의 ssh 해석과 같은 판정(ssh_active) 하나다 — fetch 소유 종류(local,wsl[,ssh])도 이 값으로 정한다
            let rust_ssh = router.ssh_active();
            let fetch_kinds = router::sidecar_fetch_kinds(router.enabled(), rust_ssh);
            // 사이드카에 jira·gh 를 소유하지 않음을 알리는 값. 두 서비스가 실제로 만들어졌을 때만 켠다(만들지 못했으면 사이드카가 처리한다)
            let rust_api = router.claim_api();
            app.manage(router);
            // fetchPrefs 를 받는 곳은 **사이드카를 띄우기 전에** 등록한다 — 첫 fetchPrefs 는 ready 전에 한 번만 오고 같은 내용이면 다시 오지 않는다.
            // 사이드카 읽기 스레드에서 불리므로 앱 런타임에 들어가서 라우터를 부른다(예약이 태스크를 만든다)
            let prefs_app = app.handle().clone();
            app.manage(FetchPrefsSink(Box::new(move |prefs| {
                if let Some(r) = prefs_app.try_state::<router::Router>() {
                    router::in_runtime_of(tauri::async_runtime::handle().inner(), || r.apply_fetch_prefs(prefs));
                }
            })));
            app.manage(sidecar::Sidecar::spawn(app.handle(), &user_data, mirror, fetch_kinds, rust_api, rust_ssh)?);
            // 저장된 로그인의 토큰 사본을 사이드카에 한 번 보낸다(사이드카의 원격 전용 저장소 fetch 용). 이후는 위의 인증 변경 콜백이 보낸다. Rust 가 GitHub 를 처리할 때만이다
            if let (true, Some(gh)) = (rust_api, gh_service) {
                if let Some(s) = app.try_state::<sidecar::Sidecar>() {
                    s.push_gh_token(|| gh.git_token());
                }
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
                // Rust 가 맡는 저장소의 자동 fetch(밀렸으면 바로). 이 스레드는 런타임 밖이라 들어가서 부른다
                if *focused {
                    if let Some(r) = window.app_handle().try_state::<router::Router>() {
                        router::in_runtime_of(tauri::async_runtime::handle().inner(), || r.fetch_on_focus());
                    }
                }
            }
        })
        .build(tauri::generate_context!())
        .expect("Tauri 앱 빌드 실패")
        .run(|app, event| {
            if let RunEvent::Exit = event {
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
        });
}
