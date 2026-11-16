//! `jira:*` 채널 골든(Phase 4 PR 6). TS(`tests/goldenJiraChannels.test.ts`)가 `tests/fixtures/net/jira.scenarios.json` 으로 만든 `tests/golden/channels/jira.json` 을
//! Rust 라우터(`Router::route` → `api_host`)가 같은 시나리오·같은 목 서버 규칙으로 재현하는지 본다. 비교하는 것은 단계마다 봉투(오류는 code 만)와
//! 서버가 받은 요청 목록(method·path·정렬한 query·Authorization 종류·본문)이다. 골든은 TS 가 기준이다: `UPDATE_GOLDEN=1 npx vitest run tests/goldenJiraChannels.test.ts`.
//!
//! 서버는 127.0.0.1 의 목 서버뿐이다(외부 접속 없음). `test-hooks` feature 가 있어야 루프백 http 를 허용하는 클라이언트·사이트 옵션을 만들 수 있다.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use antler_lib::api_host::JiraHost;
use antler_lib::jira::client::JiraClient;
use antler_lib::jira::service::{JiraService, SystemClock, TokioSleeper};
use antler_lib::jira::validate::SiteOptions;
use antler_lib::mirror::RepoMirror;
use antler_lib::router::{Routed, Router, UNDEFINED_ARG};
use antler_lib::secret::memory::MemoryStore;
use antler_lib::secret::Vault;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use serde_json::{json, Map, Value};

use super::read_json;

// ── 목 서버 ─────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
struct Seen {
    method: String,
    target: String,
    auth: Option<String>,
    body: String,
}

#[derive(Clone)]
struct Route {
    method: String,
    path: String,
    query: Vec<(String, String)>,
    responses: Vec<Value>,
}

fn parse_routes(v: &Value) -> Vec<Route> {
    v.as_array()
        .map(|a| {
            a.iter()
                .map(|r| Route {
                    method: r["method"].as_str().unwrap().to_string(),
                    path: r["path"].as_str().unwrap().to_string(),
                    query: r["query"].as_object().map(|q| q.iter().map(|(k, v)| (k.clone(), v.as_str().unwrap().to_string())).collect()).unwrap_or_default(),
                    responses: r["responses"].as_array().unwrap().clone(),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// 경로표. 일치 규칙은 시나리오 파일 머리의 `_doc` 과 TS 의 `RouteTable` 이 같다: 단계 경로 → 기본 경로 순으로 첫 일치, 일치가 없으면 404
struct Table {
    base: Vec<Route>,
    step: Vec<Route>,
    /// (단계 경로인가, 위치) → 지금까지 맞은 횟수
    hits: HashMap<(bool, usize), usize>,
}

impl Table {
    fn find(&mut self, method: &str, target: &str) -> Option<Value> {
        let url = url::Url::parse(&format!("http://x{target}")).ok()?;
        let all = self.step.iter().enumerate().map(|(i, r)| ((true, i), r)).chain(self.base.iter().enumerate().map(|(i, r)| ((false, i), r)));
        let mut hit = None;
        for (key, r) in all {
            if r.method != method || r.path != url.path() {
                continue;
            }
            if !r.query.iter().all(|(k, v)| url.query_pairs().find(|(n, _)| n == k).is_some_and(|(_, val)| val == v.as_str())) {
                continue;
            }
            hit = Some((key, r.responses.clone()));
            break;
        }
        let (key, responses) = hit?;
        let n = self.hits.entry(key).or_insert(0);
        let resp = responses[(*n).min(responses.len() - 1)].clone();
        *n += 1;
        Some(resp)
    }

    fn reset_step(&mut self, routes: Vec<Route>) {
        self.hits.retain(|(is_step, _), _| !is_step);
        self.step = routes;
    }
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

fn read_request(s: &mut TcpStream) -> Option<Seen> {
    s.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    let head_end = loop {
        if let Some(p) = find(&buf, b"\r\n\r\n") {
            break p + 4;
        }
        let n = s.read(&mut tmp).ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&tmp[..n]);
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
    let mut lines = head.split("\r\n");
    let mut first = lines.next()?.split(' ');
    let (method, target) = (first.next()?.to_string(), first.next()?.to_string());
    let headers: Vec<(String, String)> = lines.filter_map(|l| l.split_once(':')).map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string())).collect();
    let header = |name: &str| headers.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone());
    let want: usize = header("content-length").and_then(|v| v.parse().ok()).unwrap_or(0);
    while buf.len() < head_end + want {
        let n = s.read(&mut tmp).ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&tmp[..n]);
    }
    Some(Seen { method, target, auth: header("authorization"), body: String::from_utf8_lossy(&buf[head_end..head_end + want]).into_owned() })
}

fn write_response(s: &mut TcpStream, resp: &Value) {
    let delay = resp["delayMs"].as_u64().unwrap_or(0);
    if delay > 0 {
        std::thread::sleep(Duration::from_millis(delay));
    }
    let status = resp["status"].as_u64().unwrap_or(200);
    let mut headers: Vec<(String, String)> = Vec::new();
    let body: Vec<u8> = if let Some(b64) = resp["base64"].as_str() {
        STANDARD.decode(b64).unwrap()
    } else {
        match &resp["body"] {
            Value::Null => Vec::new(),
            Value::String(t) => t.clone().into_bytes(),
            other => {
                headers.push(("content-type".into(), "application/json".into()));
                other.to_string().into_bytes()
            }
        }
    };
    if let Some(h) = resp["headers"].as_object() {
        for (k, v) in h {
            headers.retain(|(n, _)| !n.eq_ignore_ascii_case(k));
            headers.push((k.clone(), v.as_str().unwrap().to_string()));
        }
    }
    let mut head = format!("HTTP/1.1 {status} X\r\n");
    for (k, v) in &headers {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str(&format!("Content-Length: {}\r\nConnection: close\r\n\r\n", body.len()));
    let _ = s.write_all(head.as_bytes()).and_then(|()| s.write_all(&body));
}

struct Mock {
    origin: String,
    table: Arc<Mutex<Table>>,
    seen: Arc<Mutex<Vec<Seen>>>,
    addr: std::net::SocketAddr,
    stop: Arc<AtomicBool>,
}

impl Mock {
    fn start(base: Vec<Route>) -> Mock {
        let listener = TcpListener::bind("127.0.0.1:0").expect("목 서버");
        let addr = listener.local_addr().expect("목 서버");
        let table = Arc::new(Mutex::new(Table { base, step: Vec::new(), hits: HashMap::new() }));
        let (seen, stop) = (Arc::new(Mutex::new(Vec::new())), Arc::new(AtomicBool::new(false)));
        let (t2, s2, stop2) = (table.clone(), seen.clone(), stop.clone());
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                if stop2.load(Ordering::SeqCst) {
                    return;
                }
                let Ok(mut stream) = stream else { continue };
                let (table, seen) = (t2.clone(), s2.clone());
                std::thread::spawn(move || {
                    let Some(req) = read_request(&mut stream) else { return };
                    seen.lock().unwrap().push(req.clone());
                    let resp = table.lock().unwrap().find(&req.method, &req.target).unwrap_or_else(|| json!({ "status": 404, "body": { "errorMessages": ["nf"] } }));
                    write_response(&mut stream, &resp);
                });
            }
        });
        Mock { origin: format!("http://{addr}"), table, seen, addr, stop }
    }
}

impl Drop for Mock {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(self.addr);
    }
}

/// 닫힌 포트의 주소(연결이 거부된다)
fn closed_origin() -> String {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    drop(l);
    format!("http://{addr}")
}

// ── 시나리오 실행 ───────────────────────────────────────────────────────

fn substitute(v: &Value, base: &str, closed: &str) -> Value {
    match v {
        Value::String(s) => Value::String(s.replace("{base}", base).replace("{closed}", closed)),
        Value::Array(a) => Value::Array(a.iter().map(|x| substitute(x, base, closed)).collect()),
        Value::Object(o) if o.len() == 1 && o.contains_key("$undefined") => json!({ UNDEFINED_ARG: true }),
        Value::Object(o) => Value::Object(o.iter().map(|(k, x)| (k.clone(), substitute(x, base, closed))).collect::<Map<_, _>>()),
        other => other.clone(),
    }
}

/// 숫자를 전부 f64 로 맞춘다(`5` 와 `5.0` 은 같은 값이다)
fn norm(v: &Value) -> Value {
    match v {
        Value::Number(n) => json!(n.as_f64().expect("f64")),
        Value::Array(a) => Value::Array(a.iter().map(norm).collect()),
        Value::Object(o) => Value::Object(o.iter().map(|(k, x)| (k.clone(), norm(x))).collect()),
        other => other.clone(),
    }
}

fn service(vault: Arc<Vault>) -> JiraService {
    let client = JiraClient::new_for_test(Duration::from_secs(5), None, Duration::from_millis(1)).expect("클라이언트");
    JiraService::with_parts(vault, client, SiteOptions::insecure_loopback_for_test(), Arc::new(SystemClock), Arc::new(TokioSleeper), Duration::from_millis(4000))
}

fn router_with(vault: Arc<Vault>, opened: Arc<Mutex<Vec<String>>>) -> Router {
    let router = Router::new(true, false, RepoMirror::default());
    let opener: antler_lib::api_host::Opener = Arc::new(move |url: &str| {
        opened.lock().unwrap().push(url.to_string());
        Ok(())
    });
    router.set_jira(JiraHost { service: service(vault), opener });
    router
}

/// 서버가 받은 요청을 골든의 모양으로: `{method, path, query(정렬), auth(Basic|Bearer|null), body?}`
fn request_json(r: &Seen) -> Value {
    let url = url::Url::parse(&format!("http://x{}", r.target)).unwrap();
    let mut query: Vec<(String, String)> = url.query_pairs().map(|(k, v)| (k.into_owned(), v.into_owned())).collect();
    query.sort();
    let auth = r.auth.as_deref().and_then(|a| a.split(' ').next()).filter(|s| *s == "Basic" || *s == "Bearer");
    let mut o = json!({ "method": r.method, "path": url.path(), "query": query.iter().map(|(k, v)| json!([k, v])).collect::<Vec<_>>(), "auth": auth });
    if !r.body.is_empty() {
        o["body"] = serde_json::from_str(&r.body).expect("요청 본문이 JSON 이 아니다");
    }
    o
}

fn sorted(mut v: Vec<Value>) -> Vec<Value> {
    v.sort_by_key(Value::to_string);
    v
}

fn short(v: &Value) -> String {
    let s = v.to_string();
    if s.chars().count() > 400 {
        format!("{}…", s.chars().take(400).collect::<String>())
    } else {
        s
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn jira_채널_시나리오가_ts_골든과_같다() {
    let sc = read_json("tests/fixtures/net/jira.scenarios.json");
    let golden = read_json("tests/golden/channels/jira.json");
    let steps = sc["steps"].as_array().expect("steps");
    let gsteps = golden["steps"].as_array().expect("골든 steps");
    assert_eq!(steps.len(), gsteps.len(), "단계 수가 골든과 다르다 — UPDATE_GOLDEN=1 npx vitest run tests/goldenJiraChannels.test.ts 로 골든을 다시 만든다");

    let mock = Mock::start(parse_routes(&sc["routes"]));
    let closed = closed_origin();
    let vault = Arc::new(Vault::new(Arc::new(MemoryStore::standing_in_for_keychain())));
    let opened: Arc<Mutex<Vec<String>>> = Arc::default();
    let mut router = router_with(vault.clone(), opened.clone());
    let unsub = |s: &str| s.replace(&mock.origin, "{base}").replace(&closed, "{closed}");

    let mut diffs = Vec::new();
    for (step, g) in steps.iter().zip(gsteps) {
        let name = step["name"].as_str().unwrap();
        assert_eq!(g["name"], step["name"], "{name}: 골든의 단계 순서가 다르다");
        if step["restart"] == true {
            // 앱을 다시 켠다 — 같은 보관소(키체인 대역)에서 서비스를 새로 만들어 연결을 복원한다
            router = router_with(vault.clone(), opened.clone());
        }
        mock.table.lock().unwrap().reset_step(parse_routes(&step["routes"]));
        let from = mock.seen.lock().unwrap().len();
        opened.lock().unwrap().clear();
        let ch = step["ch"].as_str().unwrap();
        let args = substitute(&step["args"], &mock.origin, &closed);
        let cancel_id = step["cancelAfterMs"].as_u64().and_then(|ms| args.get(1).and_then(Value::as_str).map(|id| (ms, id.to_string())));

        let call = router.route(ch, &args);
        let routed = match &cancel_id {
            Some((ms, id)) => {
                let canceller = async {
                    tokio::time::sleep(Duration::from_millis(*ms)).await;
                    // 렌더러의 req:cancel 과 같은 길(라우터가 레지스트리와 서비스를 모두 취소하고 사이드카에도 넘긴다)
                    assert!(matches!(router.route("req:cancel", &json!([id])).await, Routed::Sidecar));
                };
                tokio::join!(call, canceller).0
            }
            None => call.await,
        };
        let Routed::Done(env) = routed else { panic!("{name}: jira 채널이 사이드카로 갔다") };
        let envelope = if env["ok"] == true {
            let mut o = json!({ "ok": true });
            if let Some(d) = env.get("data") {
                o["data"] = d.clone();
            }
            o
        } else {
            json!({ "ok": false, "code": env["code"] })
        };
        let envelope: Value = serde_json::from_str(&unsub(&envelope.to_string())).unwrap();
        let requests = sorted(mock.seen.lock().unwrap()[from..].iter().map(request_json).collect());
        let mut got = json!({ "name": name, "envelope": envelope, "requests": requests });
        if ch == "jira:openExternal" {
            got["opened"] = json!(opened.lock().unwrap().iter().map(|u| unsub(u)).collect::<Vec<_>>());
        }
        let mut want = g.clone();
        want["requests"] = Value::Array(sorted(g["requests"].as_array().unwrap().clone()));
        if norm(&got) != norm(&want) {
            diffs.push(format!("{name}: 골든과 다르다\n  Rust: {}\n  TS  : {}", short(&got), short(&want)));
        }
    }
    assert!(diffs.is_empty(), "{}", diffs.join("\n"));
}
