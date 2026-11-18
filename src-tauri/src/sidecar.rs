//! Node 사이드카 중계. Rust 는 채널을 해석하지 않고 stdio JSON-lines 로 요청·응답·이벤트를 그대로 옮긴다.
//! 프로토콜은 src/sidecar/protocol.ts 와 같아야 한다.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_clipboard_manager::ClipboardExt;
use tokio::sync::oneshot;

use crate::dialogs;
use crate::mirror::RepoMirror;
use crate::router::{in_runtime_of, Router};

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Value>>>>;

pub struct Sidecar {
    stdin: Arc<Mutex<ChildStdin>>,
    pending: Pending,
    next_id: AtomicU64,
    child: Mutex<Child>,
    /// Rust 가 설정을 소유하는 실행인가. 그러면 사이드카는 `update:*` 만 받고 창 포커스(`focus`)도 보내지 않는다(감시·자동 fetch 는 Rust 가 한다)
    config_owned: bool,
}

fn error_env(msg: &str) -> Value {
    json!({ "ok": false, "error": msg, "code": "ERROR" })
}

impl Sidecar {
    /// `mirror` 는 설정을 사이드카가 소유할 때(`config_owned` 가 거짓 — `ANTLER_RUST_GIT=0`) 사이드카가 보내는 `repos` 메시지로 채워진다(라우터가 같은 것을 읽는다). Rust 가 설정을 소유하면 미러의 원천은 Rust 이고(`config::sync`) 사이드카가 보낸 줄은 버린다.
    /// `config_owned` 이면 env `ANTLER_RUST_CONFIG=1` 로 넘겨 사이드카가 `update:*` 7채널만 받고 그 밖의 서비스(설정·저장소·fetch·감시·GitHub·Jira)를 만들지 않게 한다 — **한 실행에서 설정 파일을 쓰는 쪽은 하나다**.
    /// 아니면 부모 환경의 값도 지운다(`ANTLER_RUST_GIT=0` 되돌리기 — 사이드카가 전부 맡는다, 6c 까지). 사이드카에 넘기는 env 는 이것과 `ANTLER_USER_DATA` 둘뿐이다(`ANTLER_RUST_FETCH_KINDS`·`ANTLER_RUST_API`·`ANTLER_RUST_SSH` 는 PR 10 에서 없앴다).
    pub fn spawn(app: &AppHandle, user_data: &Path, mirror: RepoMirror, config_owned: bool) -> Result<Self, String> {
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
        if config_owned {
            cmd.env("ANTLER_RUST_CONFIG", "1");
        } else {
            cmd.env_remove("ANTLER_RUST_CONFIG");
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
                handle_message(&app, &stdin_r, &pending_r, &mirror, config_owned, msg);
            }
            // 사이드카가 끝났다 — 기다리던 호출은 모두 실패로 돌려준다
            for (_, tx) in pending_r.lock().unwrap().drain() {
                let _ = tx.send(error_env("사이드카가 종료되었습니다"));
            }
        });

        Ok(Self { stdin, pending, next_id: AtomicU64::new(1), child: Mutex::new(child), config_owned })
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

    /// 창 포커스를 사이드카의 감시·fetch 에 알린다. Rust 가 설정을 소유하면(감시·자동 fetch 가 Rust 에 있다) 사이드카에는 받을 서비스가 없어 보내지 않는다
    pub fn set_focus(&self, focused: bool) {
        if self.config_owned {
            return;
        }
        let _ = self.write(&json!({ "t": "focus", "focused": focused }));
    }

    pub fn kill(&self) {
        let mut child = self.child.lock().unwrap();
        let _ = child.kill();
        let _ = child.wait();
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

/// Rust 가 설정을 소유하면(`ANTLER_RUST_CONFIG=1` 로 띄운 사이드카) 사이드카가 보내서는 안 되는 줄: 저장소 미러(`repos`)와 fetch 설정(`fetchPrefs`).
/// 사이드카는 이 모드에서 그 줄을 만들지 않는다 — 미러·fetch 설정의 원천은 Rust 의 설정(`config::sync`)이므로 실수로 와도 미러를 덮지 못하게 버린다. 설정을 사이드카가 소유하는 실행(`ANTLER_RUST_GIT=0`)에서는 미러만 받는다
fn is_config_line(t: Option<&str>) -> bool {
    matches!(t, Some("repos" | "fetchPrefs"))
}

fn handle_message(app: &AppHandle, stdin: &Arc<Mutex<ChildStdin>>, pending: &Pending, mirror: &RepoMirror, config_owned: bool, msg: Value) {
    let t = msg.get("t").and_then(Value::as_str);
    if config_owned && is_config_line(t) {
        eprintln!("[sidecar] Rust 가 설정을 소유하는 실행에서 받은 {} 줄을 버렸다", t.unwrap_or_default());
        return;
    }
    match t {
        // fetch 설정은 서비스를 만든 Rust 가 설정에서 직접 만든다(사이드카가 설정을 소유하는 실행에는 Rust 의 fetch 서비스가 없다)
        Some("fetchPrefs") => {}
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

#[cfg(test)]
mod tests {
    use super::is_config_line;

    #[test]
    fn 설정을_소유하면_사이드카의_미러와_fetch_설정_줄만_버린다() {
        assert!(is_config_line(Some("repos")) && is_config_line(Some("fetchPrefs")));
        for t in ["res", "evt", "host", "ready", "ghToken", "fetchApprove", ""] {
            assert!(!is_config_line(Some(t)), "{t}");
        }
        assert!(!is_config_line(None));
    }
}
