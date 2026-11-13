//! Node 사이드카 중계. Rust 는 채널을 해석하지 않고 stdio JSON-lines 로 요청·응답·이벤트를 그대로 옮긴다.
//! 프로토콜은 src/sidecar/protocol.ts 와 같아야 한다.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use tauri::{AppHandle, Emitter};
use tauri_plugin_clipboard_manager::ClipboardExt;
use tokio::sync::oneshot;

use crate::dialogs;

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Value>>>>;

pub struct Sidecar {
    stdin: Arc<Mutex<ChildStdin>>,
    pending: Pending,
    next_id: AtomicU64,
    child: Mutex<Child>,
}

fn error_env(msg: &str) -> Value {
    json!({ "ok": false, "error": msg, "code": "ERROR" })
}

impl Sidecar {
    pub fn spawn(app: &AppHandle, user_data: &Path) -> Result<Self, String> {
        let node = std::env::var("ANTLER_NODE").unwrap_or_else(|_| "node".into());
        let script = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../out-tauri/sidecar/index.cjs"));
        let mut cmd = Command::new(node);
        cmd.arg(script)
            .env("ANTLER_USER_DATA", user_data)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
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
                handle_message(&app, &stdin_r, &pending_r, msg);
            }
            // 사이드카가 끝났다 — 기다리던 호출은 모두 실패로 돌려준다
            for (_, tx) in pending_r.lock().unwrap().drain() {
                let _ = tx.send(error_env("사이드카가 종료되었습니다"));
            }
        });

        Ok(Self { stdin, pending, next_id: AtomicU64::new(1), child: Mutex::new(child) })
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

    pub fn set_focus(&self, focused: bool) {
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

fn handle_message(app: &AppHandle, stdin: &Arc<Mutex<ChildStdin>>, pending: &Pending, msg: Value) {
    match msg.get("t").and_then(Value::as_str) {
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
