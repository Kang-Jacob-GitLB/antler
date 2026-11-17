//! Node 사이드카 중계. Rust 는 채널을 해석하지 않고 stdio JSON-lines 로 요청·응답·이벤트를 그대로 옮긴다.
//! 프로토콜은 src/sidecar/protocol.ts 와 같아야 한다.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use futures_util::future::{BoxFuture, FutureExt};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_clipboard_manager::ClipboardExt;
use tokio::sync::oneshot;

use crate::dialogs;
use crate::git::fetch::service::{approve_message, ApprovalKind, FetchPrefs, FetchPrefsSink};
use crate::git::log_service::NoteOp;
use crate::mirror::RepoMirror;
use crate::net::redact::SecretString;
use crate::repoq::{cancel_line, gh_token_line, open_envelope, query_line, tell_line, Op, QueryTransport};
use crate::router::{in_runtime_of, Router};

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Value>>>>;

pub struct Sidecar {
    stdin: Arc<Mutex<ChildStdin>>,
    pending: Pending,
    next_id: AtomicU64,
    child: Mutex<Child>,
    /// `ghToken` 을 "토큰 읽기 → 쓰기" 한 덩어리로 순서 있게 보낸다(낡은 토큰이 나중에 써지지 않게)
    token_order: Mutex<()>,
}

fn error_env(msg: &str) -> Value {
    json!({ "ok": false, "error": msg, "code": "ERROR" })
}

impl Sidecar {
    /// `mirror` 는 사이드카가 보내는 `repos` 메시지로 채워진다(라우터가 같은 것을 읽는다).
    /// `fetch_kinds` 가 있으면 env `ANTLER_RUST_FETCH_KINDS` 로 넘겨 사이드카가 그 환경의 저장소를 fetch 하지 않게 한다(Rust 가 맡는다). 없으면 사이드카가 전부 맡는다.
    /// **`FetchPrefsSink` 를 먼저 manage 한 뒤에 불러야 한다** — 첫 fetchPrefs 가 ready 전에 한 번만 온다
    /// `rust_api` 이면 env `ANTLER_RUST_API=1` 로 넘겨 사이드카가 `jira:*`·`gh:*`(`gh:repo` 제외)를 소유하지 않게 한다(Rust 가 처리한다. GitHub 토큰은 env 가 아니라 [`Sidecar::push_gh_token`] 으로만 간다). 아니면 부모 환경의 값도 지운다.
    /// `rust_ssh` 이면 env `ANTLER_RUST_SSH=1` 로 넘겨 ssh 저장소의 git 채널(fetch 포함)을 Rust 가 처리함을 알린다(`Router::ssh_active` 와 같은 값 — `fetch_kinds` 의 ssh 포함 여부도 이 값이 정한다). 아니면 부모 환경의 값도 지운다.
    pub fn spawn(app: &AppHandle, user_data: &Path, mirror: RepoMirror, fetch_kinds: Option<&str>, rust_api: bool, rust_ssh: bool) -> Result<Self, String> {
        let node = std::env::var("ANTLER_NODE").unwrap_or_else(|_| "node".into());
        let script = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../out-tauri/sidecar/index.cjs"));
        // git 이 아니라 개발 전용 Node 사이드카를 띄운다 — git 실행기(src/git/exec)의 대상이 아니다
        #[allow(clippy::disallowed_methods)]
        let mut cmd = Command::new(node);
        cmd.arg(script)
            .env("ANTLER_USER_DATA", user_data)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // 부모 환경에 남아 있는 값이 되돌리기(없음)를 뒤집지 않게 항상 정한다
        match fetch_kinds {
            Some(kinds) => cmd.env("ANTLER_RUST_FETCH_KINDS", kinds),
            None => cmd.env_remove("ANTLER_RUST_FETCH_KINDS"),
        };
        if rust_api {
            cmd.env("ANTLER_RUST_API", "1");
        } else {
            cmd.env_remove("ANTLER_RUST_API");
        }
        if rust_ssh {
            cmd.env("ANTLER_RUST_SSH", "1");
        } else {
            cmd.env_remove("ANTLER_RUST_SSH");
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        let mut child = cmd.spawn().map_err(|e| format!("사이드카 실행 실패: {e}"))?;
        let stdin = Arc::new(Mutex::new(child.stdin.take().ok_or("사이드카 stdin 없음")?));
        let stdout = child.stdout.take().ok_or("사이드카 stdout 없음")?;
        let stderr = child.stderr.take().ok_or("사이드카 stderr 없음")?;
        let pending: Pending = Arc::default();

        // 사이드카의 console 출력은 stderr 로 온다 — 읽어 비우지 않으면 파이프가 차서 멈춘다
        std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                eprintln!("[sidecar] {line}");
            }
        });

        let (app, stdin_r, pending_r) = (app.clone(), stdin.clone(), pending.clone());
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                let Ok(msg) = serde_json::from_str::<Value>(&line) else {
                    // 파싱 못 한 응답이라도 그 호출은 끝내 준다 — 버리면 renderer 의 invoke 가 영원히 기다린다
                    if let Some(tx) = res_id(&line).and_then(|id| pending_r.lock().unwrap().remove(&id)) {
                        let _ = tx.send(error_env("사이드카 응답을 해석하지 못했습니다"));
                    }
                    eprintln!("[sidecar] JSON 아닌 줄: {}", line.chars().take(200).collect::<String>());
                    continue;
                };
                handle_message(&app, &stdin_r, &pending_r, &mirror, msg);
            }
            // 사이드카가 끝났다 — 기다리던 호출은 모두 실패로 돌려준다
            for (_, tx) in pending_r.lock().unwrap().drain() {
                let _ = tx.send(error_env("사이드카가 종료되었습니다"));
            }
        });

        Ok(Self { stdin, pending, next_id: AtomicU64::new(1), child: Mutex::new(child), token_order: Mutex::new(()) })
    }

    fn write(&self, msg: &Value) -> Result<(), String> {
        write_line(&self.stdin, msg)
    }

    /// 채널 호출을 보내고 envelope 를 기다린다
    pub async fn request(&self, channel: String, args: Value) -> Result<Value, String> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, tx);
        if let Err(e) = self.write(&json!({ "t": "req", "id": id, "ch": channel, "args": args })) {
            self.pending.lock().unwrap().remove(&id);
            return Err(e);
        }
        rx.await.map_err(|_| "사이드카 응답 대기가 끊겼습니다".to_string())
    }

    /// Rust 가 처리한 `git:log` 가 사이드카의 RepoService 에 알릴 일을 보낸다(`{t:"note",op,repoId}` — protocol.ts 의 `NOTE_OPS`).
    /// 이 줄은 Rust 만 만든다: renderer 의 값은 `req` 의 `args` 안에만 들어가고, 여기서는 미러가 아는 저장소 id 와 상수 op 만 쓴다
    pub fn note(&self, op: NoteOp, repo_id: &str) {
        let _ = self.write(&json!({ "t": "note", "op": op.as_str(), "repoId": repo_id }));
    }

    /// fetch 승인을 설정 파일에 남겨 달라고 사이드카에 부탁한다(`{t:"fetchApprove"}`). `FetchHost::approve` 의 구현(`fetch_host.rs`)이 부르는 유일한 길이다 —
    /// 서비스가 두 지점(수동 fetch 성공 뒤, "항상 허용" 응답 뒤)에서만 호출하고, 사이드카는 이 메시지가 아니면 승인을 받지 않는다.
    /// 그래서 crate 밖에 열지 않고, 호출이 그 한 곳뿐임을 `tests/channels.rs` 가 소스로 강제한다
    pub(crate) fn approve(&self, kind: ApprovalKind, repo_id: &str, fingerprint: &str) {
        let _ = self.write(&approve_message(kind, repo_id, fingerprint));
    }

    /// 사이드카에 GitHub 토큰의 메모리 사본을 보낸다(`{t:"ghToken"}`). Rust 가 GitHub 를 처리할 때(`ANTLER_RUST_API=1`)만 부른다 —
    /// 사이드카에 남는 원격 전용 저장소의 fetch 가 쓴다. 읽기와 쓰기를 한 잠금 안에서 해서, 로그인 직후 로그아웃이 겹쳐도 마지막 상태가 마지막에 간다.
    /// 줄은 stdin 으로만 가고 env·인자·로그에 실리지 않는다
    pub fn push_gh_token(&self, read: impl FnOnce() -> Option<SecretString>) {
        let _order = self.token_order.lock().unwrap_or_else(|e| e.into_inner());
        let token = read();
        let line = gh_token_line(token.as_ref());
        if write_raw(&self.stdin, &line).is_err() {
            eprintln!("[sidecar] ghToken 을 보내지 못했다");
        }
    }

    /// 저장소 정보 질의(`{t:"repoq",id,op,p}`)를 보내고 응답(`res`)의 `data` 를 기다린다.
    /// 응답 전에 이 future 를 버리면(시간 초과·취소) 대기표를 지우고 사이드카에 `repoqCancel` 을 보낸다
    async fn query(&self, op: Op, params: Value) -> Result<Value, String> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, tx);
        let mut guard = QueryGuard { id, pending: self.pending.clone(), stdin: self.stdin.clone(), armed: true };
        if let Err(e) = write_raw(&self.stdin, &format!("{}\n", query_line(id, op, &params))) {
            self.pending.lock().unwrap().remove(&id);
            guard.armed = false;
            return Err(e);
        }
        let env = rx.await.map_err(|_| "사이드카 응답 대기가 끊겼습니다".to_string());
        guard.armed = false;
        open_envelope(&env?)
    }

    pub fn set_focus(&self, focused: bool) {
        let _ = self.write(&json!({ "t": "focus", "focused": focused }));
    }

    pub fn kill(&self) {
        let mut child = self.child.lock().unwrap();
        let _ = child.kill();
        let _ = child.wait();
    }
}

/// 응답 전에 버려진 질의의 뒷정리: 대기표를 지우고 사이드카에 취소를 알린다
struct QueryGuard {
    id: u64,
    pending: Pending,
    stdin: Arc<Mutex<ChildStdin>>,
    armed: bool,
}

impl Drop for QueryGuard {
    fn drop(&mut self) {
        if self.armed {
            self.pending.lock().unwrap_or_else(|e| e.into_inner()).remove(&self.id);
            let _ = write_raw(&self.stdin, &format!("{}\n", cancel_line(self.id)));
        }
    }
}

/// [`QueryTransport`] 의 앱 구현. 사이드카는 서비스보다 늦게 만들어지므로 호출할 때 찾는다(없으면 오류)
pub struct SidecarQuery {
    app: AppHandle,
}

impl SidecarQuery {
    pub fn new(app: AppHandle) -> Self {
        Self { app }
    }
}

impl QueryTransport for SidecarQuery {
    fn ask(&self, op: Op, params: Value) -> BoxFuture<'_, Result<Value, String>> {
        async move {
            let Some(sidecar) = self.app.try_state::<Sidecar>() else { return Err("사이드카가 아직 준비되지 않았습니다".to_string()) };
            sidecar.query(op, params).await
        }
        .boxed()
    }

    fn tell(&self, op: Op, params: Value) {
        if let Some(sidecar) = self.app.try_state::<Sidecar>() {
            let _ = write_raw(&sidecar.stdin, &format!("{}\n", tell_line(op, &params)));
        }
    }
}

/// 응답 줄 앞부분 `{"t":"res","id":N,` 에서 id 를 읽는다(JSON.stringify 는 키 순서를 유지한다)
fn res_id(line: &str) -> Option<u64> {
    let rest = line.strip_prefix(r#"{"t":"res","id":"#)?;
    rest[..rest.find(',')?].parse().ok()
}

fn write_line(stdin: &Mutex<ChildStdin>, msg: &Value) -> Result<(), String> {
    let mut line = msg.to_string();
    line.push('\n');
    let mut w = stdin.lock().unwrap();
    w.write_all(line.as_bytes()).and_then(|_| w.flush()).map_err(|e| format!("사이드카 쓰기 실패: {e}"))
}

fn write_raw(stdin: &Mutex<ChildStdin>, line: &str) -> Result<(), String> {
    let mut w = stdin.lock().unwrap_or_else(|e| e.into_inner());
    w.write_all(line.as_bytes()).and_then(|_| w.flush()).map_err(|e| format!("사이드카 쓰기 실패: {e}"))
}

fn handle_message(app: &AppHandle, stdin: &Arc<Mutex<ChildStdin>>, pending: &Pending, mirror: &RepoMirror, msg: Value) {
    match msg.get("t").and_then(Value::as_str) {
        // 저장소 미러. 같은 stdout 에서 순서대로 읽으므로 저장소를 바꾼 호출의 res 보다 먼저 반영된다
        Some("repos") => {
            if mirror.apply_message(&msg) {
                // 사라졌거나 대상이 바뀐 저장소의 로그 커서(열린 git 프로세스)를 닫는다
                if let Some(router) = app.try_state::<Router>() {
                    // 이 스레드는 tokio 런타임 밖이다 — 서비스 정리가 태스크를 만질 수 있어 앱 런타임에 들어가서 부른다
                    in_runtime_of(tauri::async_runtime::handle().inner(), || router.on_mirror_changed());
                }
            } else {
                eprintln!("[sidecar] repos 메시지를 적용하지 않았다(모양이 틀렸거나 오래된 gen)");
            }
        }
        // fetch 설정(자동 주기·승인 지문). 미러와 따로 보낸다 — 미러의 중복 제거·on_mirror_changed 와 무관하다
        Some("fetchPrefs") => match FetchPrefs::from_message(&msg) {
            Some(prefs) => {
                if let Some(sink) = app.try_state::<FetchPrefsSink>() {
                    (sink.0)(prefs);
                }
            }
            None => eprintln!("[sidecar] fetchPrefs 메시지를 적용하지 않았다(모양이 틀렸다)"),
        },
        Some("res") => {
            let id = msg.get("id").and_then(Value::as_u64);
            let tx = id.and_then(|id| pending.lock().unwrap().remove(&id));
            if let Some(tx) = tx {
                let _ = tx.send(msg.get("env").cloned().unwrap_or(Value::Null));
            }
        }
        Some("evt") => {
            let _ = app.emit("antler:evt", json!({ "name": msg["name"], "payload": msg["payload"] }));
        }
        Some("host") => {
            let id = msg["id"].clone();
            let op = msg["op"].as_str().unwrap_or("").to_string();
            let arg = msg["arg"].clone();
            let (app, stdin) = (app.clone(), stdin.clone());
            // 외부 프로그램 기동·대화상자(사용자가 닫을 때까지 블로킹)는 읽기 스레드를 막지 않게 따로 돌린다
            std::thread::spawn(move || {
                let res = match run_host(&app, &op, &arg) {
                    Some(value) => json!({ "t": "hostres", "id": id, "ok": true, "value": value }),
                    None => json!({ "t": "hostres", "id": id, "ok": false }),
                };
                let _ = write_line(&stdin, &res);
            });
        }
        Some("ready") => {}
        _ => eprintln!("[sidecar] 알 수 없는 메시지: {msg}"),
    }
}

/// 사이드카가 요청한 OS 동작. 성공이면 응답 값(없으면 null), 실패면 None.
/// openExternal 은 기존 핸들러가 이미 검증하지만 Rust 도 https 만 통과시킨다
fn run_host(app: &AppHandle, op: &str, arg: &Value) -> Option<Value> {
    match op {
        "openExternal" => {
            let url = arg.as_str().filter(|u| u.starts_with("https://"))?;
            tauri_plugin_opener::open_url(url, None::<&str>).ok().map(|_| Value::Null)
        }
        "clipboard" => app.clipboard().write_text(arg.as_str()?.to_string()).ok().map(|_| Value::Null),
        "messageBox" => dialogs::message_box(app, arg),
        "pickPath" => dialogs::pick_path(app, arg),
        _ => None,
    }
}
