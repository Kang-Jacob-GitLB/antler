//! 채널 라우터. renderer 의 `antler_invoke` 를 Rust 가 직접 처리할지 Node 사이드카로 넘길지 정한다.
//!
//! Rust 가 처리하는 것: [`RUST_CHANNELS`] 에 있는 채널이고, 인자의 저장소 id 가 미러([`RepoMirror`])에 있고 kind 가 local·wsl 이고,
//! 환경변수 `ANTLER_RUST_GIT` 이 `0` 이 아닐 때. 그 밖에는 전부 사이드카가 지금까지처럼 처리한다
//! (미러에 없는 id 의 오류 문구도 사이드카가 만든다). `ANTLER_RUST_GIT=0` 은 즉시 되돌리는 스위치다.
//!
//! 보안 경계(docs/security.md · docs/tauri.md "실행기"):
//! - 저장소 경로·kind·배포판은 **미러에서만** 가져온다. renderer 가 보낸 값은 저장소 id 와 검증된 해시뿐이다.
//! - git 인자는 `git::commit` 이 서브커맨드·옵션 상수 + 검증된 해시로만 만든다. 이 파일은 인자를 만들지 않는다.
//! - 취소: `requestId → CancellationToken` 레지스트리. cancel 이 등록보다 먼저 도착해도(별개 호출이라 순서가 보장되지 않는다)
//!   tombstone 이 기억했다가 등록 때 이미 취소된 토큰을 준다.
//!
//! 이 모듈은 `Sidecar`·`AppHandle` 에 기대지 않는다 — 통합 테스트(`tests/channels.rs`)가 그대로 돌릴 수 있다.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use crate::git::commit::{commit_detail, estimate_commit_bytes, HexHash};
use crate::git::exec::queue::{ProcessQueue, LOCAL_LIMIT, WSL_LIMIT};
use crate::git::exec::{ExecError, GitExecutor, LocalExecutor, WslExecutor};
use crate::git::lru::LruCache;
use crate::mirror::{RepoMirror, Resolved, Target};

/// Rust 가 처리하는 채널. 전부 `src/shared/ipc.ts` 의 `IPC_CHANNELS` 에 있어야 한다
/// (`tests/channels.rs` 가 `tests/golden/exec/channels.json` 으로 강제한다)
pub const RUST_CHANNELS: &[&str] = &["git:commit"];

/// bridge 의 `encodeArgs` 가 인자 배열의 빈 자리(undefined)에 넣는 표식(`src/sidecar/protocol.ts` 의 `UNDEFINED_ARG`)
pub const UNDEFINED_ARG: &str = "__antlerUndefinedArg";

/// 커밋 상세 캐시(`RepoService.ts` 의 COMMIT_CACHE_ENTRIES·COMMIT_CACHE_BYTES)
const COMMIT_CACHE_ENTRIES: usize = 300;
const COMMIT_CACHE_BYTES: u64 = 24 * 1024 * 1024;

/// 등록보다 먼저 도착한 cancel 을 기억하는 시간·개수
const TOMBSTONE_TTL: Duration = Duration::from_secs(10);
const TOMBSTONE_MAX: usize = 256;

pub enum Routed {
    /// Rust 가 처리했다 — IpcEnvelope(`{ok:true,data}` / `{ok:false,error,code}`)
    Done(Value),
    /// 사이드카로 보낸다(인자는 호출자가 가진 원본 그대로)
    Sidecar,
}

/// `ANTLER_RUST_GIT` 값으로 Rust 경로를 켤지 정한다. `0` 만 끈다
pub fn rust_git_enabled(var: Option<&str>) -> bool {
    var != Some("0")
}

fn ok_env(data: Value) -> Value {
    json!({ "ok": true, "data": data })
}

fn err_env(error: &str, code: &str) -> Value {
    json!({ "ok": false, "error": error, "code": code })
}

/// 인자 배열의 표식을 None(undefined)으로 되돌린다. 사이드카 `decodeArgs` 와 같다 — 키가 정확히 그것 하나이고 값이 `true` 인 객체만
pub fn decode_args(raw: &[Value]) -> Vec<Option<&Value>> {
    raw.iter()
        .map(|a| match a.as_object() {
            Some(o) if o.len() == 1 && o.get(UNDEFINED_ARG) == Some(&Value::Bool(true)) => None,
            _ => Some(a),
        })
        .collect()
}

// ── 취소 레지스트리 ────────────────────────────────────────

#[derive(Default)]
struct RegInner {
    live: HashMap<String, (u64, CancellationToken)>,
    tombs: VecDeque<(String, Instant)>,
    serial: u64,
}

/// `requestId → CancellationToken`. 같은 id 를 다시 등록하면 새 것이 이긴다(TS `withSignal` 의 `requests.set` 과 같다)
#[derive(Default)]
pub struct CancelRegistry {
    inner: Mutex<RegInner>,
}

/// 등록한 요청이 끝나면(버려지면) 레지스트리에서 자기 항목을 지운다
pub struct Registration {
    registry: Arc<CancelRegistry>,
    id: String,
    serial: u64,
    pub token: CancellationToken,
}

impl Drop for Registration {
    fn drop(&mut self) {
        let mut inner = self.registry.inner.lock().unwrap_or_else(PoisonError::into_inner);
        if inner.live.get(&self.id).is_some_and(|(s, _)| *s == self.serial) {
            inner.live.remove(&self.id);
        }
    }
}

impl RegInner {
    fn prune(&mut self, now: Instant) {
        while self.tombs.front().is_some_and(|(_, at)| now.saturating_duration_since(*at) > TOMBSTONE_TTL) {
            self.tombs.pop_front();
        }
    }
}

/// renderer 의 requestId(`newRequestId`: `prefix-<base36>-<base36>`, 수십 바이트) 상한.
/// 넘거나 비면 requestId 없음으로 취급한다 — tombstone 이 renderer 가 정한 큰 문자열을 붙잡아 두지 않게(보안 리뷰 L1)
const MAX_REQUEST_ID: usize = 128;

fn is_request_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= MAX_REQUEST_ID
}

impl CancelRegistry {
    /// 핸들러가 **첫 await 전에** 부른다. 이미 cancel 이 도착해 있었으면 취소된 토큰을 돌려준다
    pub fn register(self: &Arc<Self>, id: &str) -> Registration {
        self.register_at(id, Instant::now())
    }

    pub fn cancel(&self, id: &str) {
        self.cancel_at(id, Instant::now());
    }

    fn register_at(self: &Arc<Self>, id: &str, now: Instant) -> Registration {
        let token = CancellationToken::new();
        let mut inner = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        inner.prune(now);
        if let Some(pos) = inner.tombs.iter().position(|(t, _)| t == id) {
            inner.tombs.remove(pos);
            token.cancel();
        }
        inner.serial += 1;
        let serial = inner.serial;
        inner.live.insert(id.to_string(), (serial, token.clone()));
        Registration { registry: self.clone(), id: id.to_string(), serial, token }
    }

    fn cancel_at(&self, id: &str, now: Instant) {
        let mut inner = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        inner.prune(now);
        if let Some((_, token)) = inner.live.get(id) {
            token.cancel();
            return;
        }
        // 아직 등록 전이다 — 곧 도착할 요청을 위해 기억한다(같은 id 가 이미 있으면 새로 적지 않는다)
        if inner.tombs.iter().any(|(t, _)| t == id) {
            return;
        }
        inner.tombs.push_back((id.to_string(), now));
        while inner.tombs.len() > TOMBSTONE_MAX {
            inner.tombs.pop_front();
        }
    }

    #[cfg(test)]
    fn counts(&self) -> (usize, usize) {
        let inner = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        (inner.live.len(), inner.tombs.len())
    }
}

// ── 라우터 ────────────────────────────────────────────

/// 소스(local 하나, wsl 은 배포판마다 하나)마다 실행기와 동시성 큐를 하나씩 둔다(TS `RepoService.runtime` 과 같다).
/// 큐의 결과 타입은 직렬화된 JSON 이라 이후 채널들이 같은 큐(같은 6/4 상한)를 나눠 쓸 수 있다
struct SourceRuntime {
    exec: Arc<GitExecutor>,
    queue: ProcessQueue<Arc<Value>>,
}

pub struct Router {
    enabled: bool,
    log: bool,
    mirror: RepoMirror,
    cancels: Arc<CancelRegistry>,
    sources: Mutex<HashMap<String, Arc<SourceRuntime>>>,
    commit_cache: Arc<Mutex<LruCache<Arc<Value>>>>,
    rust_handled: AtomicU64,
}

impl Router {
    pub fn new(enabled: bool, log: bool, mirror: RepoMirror) -> Self {
        Router {
            enabled,
            log,
            mirror,
            cancels: Arc::default(),
            sources: Mutex::default(),
            commit_cache: Arc::new(Mutex::new(LruCache::new(COMMIT_CACHE_ENTRIES, COMMIT_CACHE_BYTES))),
            rust_handled: AtomicU64::new(0),
        }
    }

    /// 환경변수로 켜고 끈다. `ANTLER_RUST_GIT=0` 이면 모든 채널을 사이드카로 보낸다.
    /// `ANTLER_ROUTE_LOG=1` 이면 [`RUST_CHANNELS`] 채널이 어디로 갔는지 stderr 에 한 줄씩 남긴다(확인용 — 기본은 조용하다)
    pub fn from_env(mirror: RepoMirror) -> Self {
        let enabled = rust_git_enabled(std::env::var("ANTLER_RUST_GIT").ok().as_deref());
        let log = std::env::var("ANTLER_ROUTE_LOG").as_deref() == Ok("1");
        Router::new(enabled, log, mirror)
    }

    /// 지금까지 Rust 가 처리한 호출 수(테스트·확인용)
    pub fn rust_handled(&self) -> u64 {
        self.rust_handled.load(Ordering::Relaxed)
    }

    /// 채널 하나를 라우팅한다. `args` 는 renderer 가 보낸 인자 배열 그대로다(표식 포함)
    pub async fn route(&self, channel: &str, args: &Value) -> Routed {
        let Some(raw) = args.as_array() else { return Routed::Sidecar };
        if channel == "req:cancel" {
            // Rust 레지스트리를 먼저 취소하고, 요청이 어디서 돌든(사이드카 포함) 항상 사이드카에도 전달한다
            if let Some(Value::String(id)) = raw.first() {
                if is_request_id(id) {
                    self.cancels.cancel(id);
                }
            }
            return Routed::Sidecar;
        }
        if !RUST_CHANNELS.contains(&channel) {
            return Routed::Sidecar;
        }
        if !self.enabled {
            self.note(channel, "sidecar(ANTLER_RUST_GIT=0)");
            return Routed::Sidecar;
        }
        let args = decode_args(raw);
        match channel {
            "git:commit" => self.git_commit(&args).await,
            _ => Routed::Sidecar,
        }
    }

    fn note(&self, channel: &str, to: &str) {
        if self.log {
            eprintln!("[route] {channel} -> {to}");
        }
    }

    /// `git:commit(repoId, hash, requestId?)`
    async fn git_commit(&self, args: &[Option<&Value>]) -> Routed {
        // 저장소 id 가 문자열이 아니면 사이드카가 TS 와 같은 오류를 만든다
        let Some(Some(Value::String(repo_id))) = args.first() else { return Routed::Sidecar };
        let target = match self.mirror.resolve(repo_id) {
            Resolved::Rust(t) => t,
            Resolved::Sidecar => {
                self.note("git:commit", "sidecar");
                return Routed::Sidecar;
            }
            Resolved::Invalid(message) => return Routed::Done(err_env(&message, "ERROR")),
        };
        // TS 의 withSignal 과 같게 빈 문자열은 requestId 없음이다. 첫 await 전에 등록한다
        let reg = match args.get(2) {
            Some(Some(Value::String(id))) if is_request_id(id) => Some(self.cancels.register(id)),
            _ => None,
        };
        let token = reg.as_ref().map_or_else(CancellationToken::new, |r| r.token.clone());
        let hash = match args.get(1) {
            Some(Some(Value::String(s))) => Some(s.as_str()),
            _ => None,
        };
        self.note("git:commit", "rust");
        self.rust_handled.fetch_add(1, Ordering::Relaxed);
        let result = self.commit(repo_id, &target, hash, &token).await;
        drop(reg);
        Routed::Done(match result {
            Ok(detail) => ok_env((*detail).clone()),
            Err(e) => err_env(&e.to_string(), e.envelope_code()),
        })
    }

    fn runtime(&self, target: &Target) -> Arc<SourceRuntime> {
        let mut sources = self.sources.lock().unwrap_or_else(PoisonError::into_inner);
        sources
            .entry(target.source_key())
            .or_insert_with(|| {
                Arc::new(match target {
                    Target::Local { .. } => SourceRuntime { exec: Arc::new(GitExecutor::Local(LocalExecutor::default())), queue: ProcessQueue::new(LOCAL_LIMIT) },
                    Target::Wsl { distro, .. } => SourceRuntime { exec: Arc::new(GitExecutor::Wsl(WslExecutor::new(distro.clone()))), queue: ProcessQueue::new(WSL_LIMIT) },
                })
            })
            .clone()
    }

    /// 커밋 상세. TS `RepoService.commit` 과 같은 순서: 해시 기준 불변 캐시 → 같은 키 요청 합치기 → 큐 슬롯 안에서 git 세 번.
    /// 합친 요청의 취소는 기다리는 쪽이 모두 취소했을 때만 프로세스에 닿는다(기다리는 쪽이 모두 사라지면 큐가 프로세스를 정리한다)
    async fn commit(&self, repo_id: &str, target: &Target, hash: Option<&str>, token: &CancellationToken) -> Result<Arc<Value>, ExecError> {
        let hash = HexHash::parse(hash.unwrap_or(""))?;
        let key = format!("{repo_id}\0{}", hash.lower());
        if let Some(hit) = self.commit_cache.lock().unwrap_or_else(PoisonError::into_inner).get(&key) {
            return Ok(hit);
        }
        let rt = self.runtime(target);
        let (exec, path, cache, cache_key) = (rt.exec.clone(), target.path().to_string(), self.commit_cache.clone(), key.clone());
        let run = rt.queue.run(Some(&key), None, move || async move {
            let detail = commit_detail(&exec, &path, &hash).await?;
            let cost = estimate_commit_bytes(&detail);
            let value = Arc::new(serde_json::to_value(&detail).map_err(|e| ExecError::Other { message: e.to_string() })?);
            cache.lock().unwrap_or_else(PoisonError::into_inner).set(&cache_key, value.clone(), cost);
            Ok(value)
        });
        // 요청 토큰은 실행기에 넘기지 않는다 — 합쳐진 다른 호출자의 프로세스까지 죽이게 된다. 이 호출자는 기다리기만 그만둔다
        tokio::select! {
            biased;
            () = token.cancelled() => Err(ExecError::Aborted),
            r = run => r,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mirror::MirrorRepo;

    fn mirror_of(repos: Vec<MirrorRepo>) -> RepoMirror {
        let m = RepoMirror::default();
        m.apply(1, repos);
        m
    }

    fn repo(id: &str, kind: &str, path: &str) -> MirrorRepo {
        MirrorRepo { id: id.into(), kind: kind.into(), path: path.into(), distro: None }
    }

    fn is_sidecar(r: &Routed) -> bool {
        matches!(r, Routed::Sidecar)
    }

    fn envelope(r: Routed) -> Value {
        match r {
            Routed::Done(v) => v,
            Routed::Sidecar => panic!("사이드카로 갔다"),
        }
    }

    #[test]
    fn 스위치는_0_만_끈다() {
        assert!(rust_git_enabled(None));
        assert!(rust_git_enabled(Some("1")));
        assert!(rust_git_enabled(Some("")));
        assert!(rust_git_enabled(Some("false")));
        assert!(!rust_git_enabled(Some("0")));
    }

    #[test]
    fn 표식_객체만_undefined_로_되돌린다() {
        let raw = [
            json!("a"),
            json!({ UNDEFINED_ARG: true }),
            json!({ UNDEFINED_ARG: true, "x": 1 }),
            json!({ UNDEFINED_ARG: false }),
            json!(null),
            json!([{ UNDEFINED_ARG: true }]),
        ];
        let d = decode_args(&raw);
        assert_eq!(d[0], Some(&json!("a")));
        assert_eq!(d[1], None);
        assert_eq!(d[2], Some(&raw[2]));
        assert_eq!(d[3], Some(&raw[3]));
        assert_eq!(d[4], Some(&Value::Null));
        assert_eq!(d[5], Some(&raw[5]));
    }

    #[tokio::test]
    async fn 사이드카로_가는_경우() {
        let m = mirror_of(vec![repo("a", "local", "/nonexistent-antler"), repo("s", "ssh", "/srv"), repo("g", "github", "/cache")]);
        let r = Router::new(true, false, m.clone());
        // 채널이 등록되지 않았다
        assert!(is_sidecar(&r.route("git:log", &json!(["a", {}])).await));
        // 미러에 없는 저장소·ssh·github
        for id in ["nope", "s", "g"] {
            assert!(is_sidecar(&r.route("git:commit", &json!([id, "abcd"])).await), "{id}");
        }
        // 저장소 id 가 문자열이 아니거나 없다
        for args in [json!([]), json!([null]), json!([5, "abcd"]), json!([{ UNDEFINED_ARG: true }, "abcd"]), json!({ "a": 1 }), json!("x")] {
            assert!(is_sidecar(&r.route("git:commit", &args).await), "{args}");
        }
        // 스위치를 끄면 미러에 있어도 사이드카
        let off = Router::new(false, false, m);
        assert!(is_sidecar(&off.route("git:commit", &json!(["a", "abcd"])).await));
        assert_eq!(off.rust_handled() + r.rust_handled(), 0);
    }

    #[tokio::test]
    async fn 미러의_경로가_검사에_걸리면_error_로_돌려준다() {
        let m = mirror_of(vec![repo("unc", "local", "\\\\evil\\share\\repo")]);
        let r = Router::new(true, false, m);
        let env = envelope(r.route("git:commit", &json!(["unc", "abcd1234"])).await);
        assert_eq!(env["ok"], false);
        assert_eq!(env["code"], "ERROR");
    }

    #[tokio::test]
    async fn 해시가_잘못되면_git_을_띄우기_전에_error() {
        // 저장소 경로가 없는 곳이다 — 검증이 먼저 걸러야 한다(git 을 띄웠다면 GIT 이나 다른 오류가 난다)
        let m = mirror_of(vec![repo("a", "local", "/nonexistent-antler")]);
        let r = Router::new(true, false, m);
        let hostile: Vec<Value> = vec![
            json!("-x"),
            json!("--output=pwned.txt"),
            json!("-calias.x=!echo"),
            json!("zzzz"),
            json!("abc"),
            json!("abcd\n"),
            json!(" abcd"),
            json!(123),
            json!(null),
            json!({ "x": 1 }),
            json!({ UNDEFINED_ARG: true }),
        ];
        for h in hostile {
            let env = envelope(r.route("git:commit", &json!(["a", h, "req-1"])).await);
            assert_eq!(env["ok"], false, "{h}");
            assert_eq!(env["code"], "ERROR", "{h}");
            assert_eq!(env["error"], "invalid commit hash", "{h}");
        }
        // 해시를 아예 안 보냈다
        assert_eq!(envelope(r.route("git:commit", &json!(["a"])).await)["code"], "ERROR");
    }

    #[tokio::test]
    async fn req_cancel_은_항상_사이드카에도_간다() {
        let r = Router::new(true, false, RepoMirror::default());
        assert!(is_sidecar(&r.route("req:cancel", &json!(["x"])).await));
        assert!(is_sidecar(&r.route("req:cancel", &json!([])).await));
        assert!(is_sidecar(&r.route("req:cancel", &json!([5])).await));
        // 스위치가 꺼져 있어도 마찬가지
        let off = Router::new(false, false, RepoMirror::default());
        assert!(is_sidecar(&off.route("req:cancel", &json!(["x"])).await));
    }

    #[tokio::test]
    async fn 등록_전에_온_cancel_은_등록한_요청을_취소한다() {
        let m = mirror_of(vec![repo("a", "local", "/nonexistent-antler")]);
        let r = Router::new(true, false, m);
        assert!(is_sidecar(&r.route("req:cancel", &json!(["early"])).await));
        // git 을 띄우지 않고 ABORTED — 경로가 없어서 띄웠다면 GIT/ERROR 가 난다
        let env = envelope(r.route("git:commit", &json!(["a", "abcd1234", "early"])).await);
        assert_eq!(env["code"], "ABORTED");
        // 쓴 tombstone 은 사라지고 등록도 끝나며 지워진다(이 테스트는 git 을 띄우지 않는다)
        assert_eq!(r.cancels.counts(), (0, 0));
    }

    #[test]
    fn 레지스트리는_등록한_것만_취소하고_끝나면_지운다() {
        let reg: Arc<CancelRegistry> = Arc::default();
        let a = reg.register("a");
        assert!(!a.token.is_cancelled());
        reg.cancel("a");
        assert!(a.token.is_cancelled());
        assert_eq!(reg.counts(), (1, 0));
        drop(a);
        assert_eq!(reg.counts(), (0, 0));
        // 같은 id 를 다시 등록하면 새 것이 이기고, 먼저 등록한 것의 Drop 이 새 것을 지우지 않는다
        let first = reg.register("x");
        let second = reg.register("x");
        drop(first);
        reg.cancel("x");
        assert!(second.token.is_cancelled());
    }

    #[test]
    fn tombstone_은_10초_뒤_사라지고_256개를_넘지_못한다() {
        let reg: Arc<CancelRegistry> = Arc::default();
        let t0 = Instant::now();
        reg.cancel_at("old", t0);
        // 11초 뒤 등록하면 이미 만료돼 취소되지 않는다
        let r = reg.register_at("old", t0 + Duration::from_secs(11));
        assert!(!r.token.is_cancelled());
        // 10초 안이면 취소된다
        reg.cancel_at("fresh", t0);
        let r = reg.register_at("fresh", t0 + Duration::from_secs(9));
        assert!(r.token.is_cancelled());
        // 개수 상한: 가장 오래된 것부터 버린다
        for i in 0..(TOMBSTONE_MAX + 5) {
            reg.cancel_at(&format!("id{i}"), t0);
        }
        assert_eq!(reg.counts().1, TOMBSTONE_MAX);
        assert!(!reg.register_at("id0", t0).token.is_cancelled());
        assert!(reg.register_at(&format!("id{}", TOMBSTONE_MAX + 4), t0).token.is_cancelled());
    }
}
