//! 채널 골든(`jira.rs`·`github.rs`)이 같이 쓰는 목 서버와 시나리오 도우미. 서버는 127.0.0.1 뿐이다(외부 접속 없음).
//! 경로표 규칙은 시나리오 파일 머리의 `_doc` 과 TS 의 `RouteTable` 이 같다. 일치하는 경로가 없을 때의 본문(`not_found`)만 서비스마다 다르다.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use antler_lib::router::UNDEFINED_ARG;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use serde_json::{json, Map, Value};

// ── 목 서버 ─────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct Seen {
    pub method: String,
    pub target: String,
    pub auth: Option<String>,
    pub body: String,
}

#[derive(Clone)]
pub struct Route {
    method: String,
    path: String,
    query: Vec<(String, String)>,
    responses: Vec<Value>,
}

pub fn parse_routes(v: &Value) -> Vec<Route> {
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
pub struct Table {
    base: Vec<Route>,
    step: Vec<Route>,
    /// (단계 경로인가, 위치) → 지금까지 맞은 횟수
    hits: HashMap<(bool, usize), usize>,
}

impl Table {
    pub fn find(&mut self, method: &str, target: &str) -> Option<Value> {
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

    pub fn reset_step(&mut self, routes: Vec<Route>) {
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

pub struct Mock {
    pub origin: String,
    pub table: Arc<Mutex<Table>>,
    pub seen: Arc<Mutex<Vec<Seen>>>,
    addr: std::net::SocketAddr,
    stop: Arc<AtomicBool>,
}

impl Mock {
    /// `not_found`: 일치하는 경로가 없을 때 404 로 돌려줄 본문(JSON)
    pub fn start(base: Vec<Route>, not_found: Value) -> Mock {
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
                let (table, seen, not_found) = (t2.clone(), s2.clone(), not_found.clone());
                std::thread::spawn(move || {
                    let Some(req) = read_request(&mut stream) else { return };
                    seen.lock().unwrap().push(req.clone());
                    let resp = table.lock().unwrap().find(&req.method, &req.target).unwrap_or_else(|| json!({ "status": 404, "body": not_found }));
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
pub fn closed_origin() -> String {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    drop(l);
    format!("http://{addr}")
}


pub fn substitute(v: &Value, base: &str, closed: &str) -> Value {
    match v {
        Value::String(s) => Value::String(s.replace("{base}", base).replace("{closed}", closed)),
        Value::Array(a) => Value::Array(a.iter().map(|x| substitute(x, base, closed)).collect()),
        Value::Object(o) if o.len() == 1 && o.contains_key("$undefined") => json!({ UNDEFINED_ARG: true }),
        Value::Object(o) => Value::Object(o.iter().map(|(k, x)| (k.clone(), substitute(x, base, closed))).collect::<Map<_, _>>()),
        other => other.clone(),
    }
}

/// 숫자를 전부 f64 로 맞춘다(`5` 와 `5.0` 은 같은 값이다)
pub fn norm(v: &Value) -> Value {
    match v {
        Value::Number(n) => json!(n.as_f64().expect("f64")),
        Value::Array(a) => Value::Array(a.iter().map(norm).collect()),
        Value::Object(o) => Value::Object(o.iter().map(|(k, x)| (k.clone(), norm(x))).collect()),
        other => other.clone(),
    }
}

/// 서버가 받은 요청을 골든의 모양으로: `{method, path, query(정렬), auth(Basic|Bearer|null), body?}`
pub fn request_json(r: &Seen) -> Value {
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

pub fn sorted(mut v: Vec<Value>) -> Vec<Value> {
    v.sort_by_key(Value::to_string);
    v
}

pub fn short(v: &Value) -> String {
    let s = v.to_string();
    if s.chars().count() > 400 {
        format!("{}…", s.chars().take(400).collect::<String>())
    } else {
        s
    }
}

