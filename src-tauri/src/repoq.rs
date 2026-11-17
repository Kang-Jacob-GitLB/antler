//! Rust → Node 질의(`repoq`)와 토큰 사본(`ghToken`)의 메시지·검증(Phase 4 PR 9). 프로토콜은 `src/sidecar/protocol.ts`(`REPOQ_OPS`·`ToSidecar`)와 같아야 한다.
//!
//! GitHub 서비스([`GitHubService`](crate::github::service::GitHubService))가 사이드카에 묻는 것들(저장소 → owner/repo, remote 목록, push 된 sha, `githubClientId`, 등록된 저장소 ID)을
//! [`RepoQuery`] 의 실제 구현([`SidecarRepos`])으로 옮겼다. 전송은 [`QueryTransport`] 로 주입한다 — 앱은 사이드카의 stdio(`sidecar.rs`), 시험은 가짜.
//!
//! **사이드카의 응답은 신뢰하지 않는다**: 응답 크기·배열 길이·문자열 길이에 상한을 두고 모양이 틀리면 버리며(오류가 아니라 "없음"으로 줄이는 곳도 있다), sha 는 소문자 hex 40/64 만 받는다.
//! owner/repo 의 최종 검증은 서비스의 `Slug::parse` 가 한다(이 모듈은 길이만 본다). 시간 제한은 질의마다 건다. 알 수 없는 op 는 Rust 가 보낼 수 없다([`Op`] 가 닫힌 목록이다).
//!
//! `ghToken`: 사이드카에 남는 일(원격 전용 저장소의 git fetch 인증, `RepoService` 의 로그 가림)이 쓰는 GitHub 토큰의 **메모리 사본**이다. Rust 가 사이드카 stdin 으로만 보내고(env·인자·로그에 싣지 않는다),
//! 줄은 `json!`·`to_string` 을 거치지 않고 `Zeroizing` 버퍼에 직접 만든다.

use std::sync::Arc;
use std::time::Duration;

use futures_util::future::{BoxFuture, FutureExt};
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;
use zeroize::Zeroizing;

use crate::github::ports::{GhRepoRef, RepoQuery};
use crate::net::redact::SecretString;

/// 질의 하나의 기본 제한 시간. 사이드카가 git 으로 remote 를 읽는 일이 가장 오래 걸린다
pub const QUERY_TIMEOUT: Duration = Duration::from_secs(30);
/// 응답 하나의 상한(직렬화한 바이트)
pub const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
const MAX_REMOTES: usize = 100;
const MAX_REPO_IDS: usize = 5000;
const MAX_NAME_CHARS: usize = 200;
const MAX_ERROR_CHARS: usize = 300;
const MAX_CLIENT_ID_CHARS: usize = 256;
/// 사이드카로 보내는 토큰의 상한(실제 토큰은 수백 바이트다). 넘으면 보내지 않는다(없는 것으로)
pub const MAX_TOKEN_BYTES: usize = 4096;

/// 사이드카에 물을 수 있는 일. 닫힌 목록이다(`protocol.ts` 의 `REPOQ_OPS` 와 같다)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    GhRepo,
    GhRemotes,
    PushedShas,
    InvalidateGhRemotes,
    GithubClientId,
    RepoIds,
}

impl Op {
    pub const ALL: [Op; 6] = [Op::GhRepo, Op::GhRemotes, Op::PushedShas, Op::InvalidateGhRemotes, Op::GithubClientId, Op::RepoIds];

    pub fn as_str(self) -> &'static str {
        match self {
            Op::GhRepo => "ghRepo",
            Op::GhRemotes => "ghRemotes",
            Op::PushedShas => "pushedShas",
            Op::InvalidateGhRemotes => "invalidateGhRemotes",
            Op::GithubClientId => "githubClientId",
            Op::RepoIds => "repoIds",
        }
    }
}

/// 사이드카와 말하는 길. 앱은 `sidecar.rs` 가 구현하고 시험은 가짜를 쓴다
pub trait QueryTransport: Send + Sync {
    /// 질의를 보내고 응답의 `data` 를 기다린다. 오류는 사용자에게 보여 줄 수 있는 문장이다.
    /// 돌려받기 전에 future 를 버리면(취소·시간 초과) 구현은 대기표를 지우고 사이드카에 취소를 알려야 한다
    fn ask(&self, op: Op, params: Value) -> BoxFuture<'_, Result<Value, String>>;
    /// 응답을 기다리지 않는 질의(`invalidateGhRemotes`). 같은 줄 순서가 지켜진다
    fn tell(&self, op: Op, params: Value);
}

/// 사이드카가 보낸 문장을 사용자에게 보여 줄 만큼만 남긴다
fn clip_error(message: &str) -> String {
    let m: String = message.chars().take(MAX_ERROR_CHARS).collect();
    if m.trim().is_empty() {
        "사이드카가 저장소 정보를 돌려주지 못했습니다".to_string()
    } else {
        m
    }
}

/// 응답 봉투(`{ok, data|error}`)에서 `data` 를 꺼낸다. 크기 상한은 여기서 건다
pub fn open_envelope(env: &Value) -> Result<Value, String> {
    if env.get("ok") != Some(&Value::Bool(true)) {
        let message = env.get("error").and_then(Value::as_str).map(clip_error).unwrap_or_else(|| clip_error(""));
        return Err(message);
    }
    let data = env.get("data").cloned().unwrap_or(Value::Null);
    if data.to_string().len() > MAX_RESPONSE_BYTES {
        return Err("사이드카의 응답이 너무 큽니다".to_string());
    }
    Ok(data)
}

fn name_ok(s: &str) -> bool {
    !s.is_empty() && s.chars().count() <= MAX_NAME_CHARS
}

fn parse_ref(v: &Value) -> Option<GhRepoRef> {
    let (owner, repo, remote) = (v.get("owner")?.as_str()?, v.get("repo")?.as_str()?, v.get("remote")?.as_str()?);
    (name_ok(owner) && name_ok(repo) && name_ok(remote)).then(|| GhRepoRef { owner: owner.to_string(), repo: repo.to_string(), remote: remote.to_string() })
}

/// 소문자 16진수 40자(sha1) 또는 64자(sha256)
fn is_sha(s: &str) -> bool {
    matches!(s.len(), 40 | 64) && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// [`RepoQuery`] 의 실제 구현: 사이드카의 `RepoService`·설정에 묻는다
pub struct SidecarRepos {
    transport: Arc<dyn QueryTransport>,
    timeout: Duration,
}

impl SidecarRepos {
    pub fn new(transport: Arc<dyn QueryTransport>) -> Self {
        Self { transport, timeout: QUERY_TIMEOUT }
    }

    /// 제한 시간을 바꾼다(시험은 멈춘 시계로 짧게 건다)
    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// 질의 하나: 제한 시간·취소를 걸고 봉투를 열어 `data` 를 돌려준다. 시간 초과·취소로 이 future 가 끝나면 전송의 `ask` future 가 버려진다
    async fn query(&self, op: Op, params: Value, cancel: Option<&CancellationToken>) -> Result<Value, String> {
        let ask = self.transport.ask(op, params);
        let run = async {
            match tokio::time::timeout(self.timeout, ask).await {
                Ok(r) => r,
                Err(_) => Err("사이드카가 제때 응답하지 않았습니다".to_string()),
            }
        };
        match cancel {
            Some(c) => tokio::select! {
                biased;
                () = c.cancelled() => Err("aborted".to_string()),
                r = run => r,
            },
            None => run.await,
        }
    }
}

impl RepoQuery for SidecarRepos {
    fn gh_repo<'a>(&'a self, repo_id: &'a str) -> BoxFuture<'a, Result<Option<GhRepoRef>, String>> {
        async move {
            let data = self.query(Op::GhRepo, json!({ "repoId": repo_id }), None).await?;
            match data {
                Value::Null => Ok(None),
                v => parse_ref(&v).map(Some).ok_or_else(|| "사이드카의 응답 형식이 올바르지 않습니다".to_string()),
            }
        }
        .boxed()
    }

    fn gh_remotes<'a>(&'a self, repo_id: &'a str, cancel: CancellationToken) -> BoxFuture<'a, Result<Vec<GhRepoRef>, String>> {
        async move {
            let data = self.query(Op::GhRemotes, json!({ "repoId": repo_id }), Some(&cancel)).await?;
            let list = data.as_array().filter(|a| a.len() <= MAX_REMOTES).ok_or_else(|| "사이드카의 응답 형식이 올바르지 않습니다".to_string())?;
            list.iter().map(|v| parse_ref(v).ok_or_else(|| "사이드카의 응답 형식이 올바르지 않습니다".to_string())).collect()
        }
        .boxed()
    }

    fn pushed_shas_by_author<'a>(&'a self, repo_id: &'a str, email: &'a str, limit: usize, cancel: CancellationToken) -> BoxFuture<'a, Result<Vec<String>, String>> {
        async move {
            let data = self.query(Op::PushedShas, json!({ "repoId": repo_id, "email": email, "limit": limit }), Some(&cancel)).await?;
            let list = data.as_array().ok_or_else(|| "사이드카의 응답 형식이 올바르지 않습니다".to_string())?;
            // 모양이 틀린 항목은 버리고, 물은 개수까지만 받는다
            Ok(list.iter().filter_map(Value::as_str).filter(|s| is_sha(s)).take(limit).map(str::to_string).collect())
        }
        .boxed()
    }

    fn invalidate_gh_remotes(&self) {
        self.transport.tell(Op::InvalidateGhRemotes, json!({}));
    }

    fn github_client_id(&self) -> BoxFuture<'_, Option<String>> {
        async move {
            let data = self.query(Op::GithubClientId, json!({}), None).await.ok()?;
            data.as_str().filter(|s| !s.is_empty() && s.chars().count() <= MAX_CLIENT_ID_CHARS).map(str::to_string)
        }
        .boxed()
    }

    fn repo_ids(&self) -> BoxFuture<'_, Vec<String>> {
        async move {
            let Ok(data) = self.query(Op::RepoIds, json!({}), None).await else { return Vec::new() };
            let Some(list) = data.as_array().filter(|a| a.len() <= MAX_REPO_IDS) else { return Vec::new() };
            list.iter().filter_map(Value::as_str).filter(|s| name_ok(s)).map(str::to_string).collect()
        }
        .boxed()
    }
}

// ── 사이드카로 가는 줄 ────────────────────────────────────────────────────────

/// `{t:"repoq",id,op,p}` 줄(개행 없음). 매개변수는 `p` 안에만 들어가 `t`·`id`·`op` 를 덮지 못한다
pub fn query_line(id: u64, op: Op, params: &Value) -> String {
    json!({ "t": "repoq", "id": id, "op": op.as_str(), "p": params }).to_string()
}

/// 응답을 기다리지 않는 질의(`id` 없음)
pub fn tell_line(op: Op, params: &Value) -> String {
    json!({ "t": "repoq", "op": op.as_str(), "p": params }).to_string()
}

/// 취소 알림: 사이드카가 그 질의의 `AbortSignal` 을 끊는다
pub fn cancel_line(id: u64) -> String {
    json!({ "t": "repoqCancel", "id": id }).to_string()
}

/// `{t:"ghToken",token}` 줄(끝 개행 포함). 토큰이 없거나 상한을 넘으면 `null`(사이드카가 사본을 비운다).
/// `json!`·`to_string` 의 임시 문자열을 남기지 않으려고 손으로 이스케이프해 `Zeroizing` 버퍼에 직접 쓴다
pub fn gh_token_line(token: Option<&SecretString>) -> Zeroizing<String> {
    let mut line = Zeroizing::new(String::with_capacity(token.map_or(0, |t| t.expose().len()) + 32));
    line.push_str(r#"{"t":"ghToken","token":"#);
    match token.map(SecretString::expose).filter(|t| !t.is_empty() && t.len() <= MAX_TOKEN_BYTES) {
        None => line.push_str("null"),
        Some(t) => {
            line.push('"');
            for c in t.chars() {
                match c {
                    '"' => line.push_str("\\\""),
                    '\\' => line.push_str("\\\\"),
                    c if (c as u32) < 0x20 => line.push_str(&format!("\\u{:04x}", c as u32)),
                    c => line.push(c),
                }
            }
            line.push('"');
        }
    }
    line.push_str("}\n");
    line
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    /// 가짜 전송: 받은 질의를 기록하고, 정해 둔 응답(없으면 영원히 기다린다)을 돌려준다. 버려지면 `dropped` 를 센다
    #[derive(Default)]
    struct Fake {
        asked: Mutex<Vec<(Op, Value)>>,
        told: Mutex<Vec<(Op, Value)>>,
        reply: Mutex<Option<Result<Value, String>>>,
        dropped: Arc<AtomicUsize>,
    }

    struct DropCount(Arc<AtomicUsize>);
    impl Drop for DropCount {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    impl QueryTransport for Fake {
        fn ask(&self, op: Op, params: Value) -> BoxFuture<'_, Result<Value, String>> {
            self.asked.lock().unwrap().push((op, params));
            let reply = self.reply.lock().unwrap().clone();
            let guard = DropCount(self.dropped.clone());
            async move {
                let _guard = guard;
                match reply {
                    Some(r) => r,
                    None => std::future::pending().await,
                }
            }
            .boxed()
        }

        fn tell(&self, op: Op, params: Value) {
            self.told.lock().unwrap().push((op, params));
        }
    }

    fn repos(reply: Option<Result<Value, String>>) -> (SidecarRepos, Arc<Fake>) {
        let fake = Arc::new(Fake::default());
        *fake.reply.lock().unwrap() = reply;
        (SidecarRepos::new(fake.clone()), fake)
    }

    fn data(v: Value) -> Option<Result<Value, String>> {
        Some(Ok(v))
    }

    #[tokio::test]
    async fn gh_repo_는_null_이면_없음_모양이_맞으면_참조() {
        let (r, f) = repos(data(Value::Null));
        assert_eq!(r.gh_repo("r1").await, Ok(None));
        assert_eq!(f.asked.lock().unwrap()[0], (Op::GhRepo, json!({ "repoId": "r1" })));
        let (r, _) = repos(data(json!({ "owner": "octo", "repo": "hello", "remote": "origin" })));
        assert_eq!(r.gh_repo("r1").await, Ok(Some(GhRepoRef { owner: "octo".into(), repo: "hello".into(), remote: "origin".into() })));
    }

    #[tokio::test]
    async fn gh_repo_의_틀린_모양은_거부한다() {
        for bad in [json!("x"), json!(5), json!({ "owner": "o" }), json!({ "owner": 1, "repo": "r", "remote": "o" }), json!({ "owner": "", "repo": "r", "remote": "o" }), json!({ "owner": "o".repeat(201), "repo": "r", "remote": "o" }), json!([])] {
            let (r, _) = repos(data(bad.clone()));
            assert!(r.gh_repo("r1").await.is_err(), "{bad}");
        }
    }

    #[tokio::test]
    async fn 사이드카의_오류_문장은_잘라서_돌려준다() {
        let long = "가".repeat(1000);
        let (r, _) = repos(Some(Err(clip_error(&long))));
        assert_eq!(r.gh_repo("r1").await.unwrap_err().chars().count(), MAX_ERROR_CHARS);
        let env = json!({ "ok": false, "error": long, "code": "ERROR" });
        assert_eq!(open_envelope(&env).unwrap_err().chars().count(), MAX_ERROR_CHARS);
        assert!(open_envelope(&json!({ "ok": false })).unwrap_err().contains("돌려주지 못했습니다"));
        assert!(open_envelope(&json!({ "ok": false, "error": 5 })).is_err());
        assert!(open_envelope(&json!("x")).is_err());
        assert_eq!(open_envelope(&json!({ "ok": true })).unwrap(), Value::Null);
    }

    #[test]
    fn 응답_크기에_상한이_있다() {
        let huge = json!({ "ok": true, "data": "x".repeat(MAX_RESPONSE_BYTES + 1) });
        assert!(open_envelope(&huge).unwrap_err().contains("너무 큽니다"));
        let fine = json!({ "ok": true, "data": "x".repeat(1000) });
        assert!(open_envelope(&fine).is_ok());
    }

    #[tokio::test]
    async fn gh_remotes_는_개수_상한과_항목_모양을_본다() {
        let item = |i: usize| json!({ "owner": "octo", "repo": format!("r{i}"), "remote": "origin" });
        let (r, _) = repos(data(json!((0..3).map(item).collect::<Vec<_>>())));
        assert_eq!(r.gh_remotes("r1", CancellationToken::new()).await.unwrap().len(), 3);
        let (r, _) = repos(data(json!((0..=MAX_REMOTES).map(item).collect::<Vec<_>>())));
        assert!(r.gh_remotes("r1", CancellationToken::new()).await.is_err());
        let (r, _) = repos(data(json!([item(1), { "owner": "x" }])));
        assert!(r.gh_remotes("r1", CancellationToken::new()).await.is_err());
        let (r, _) = repos(data(json!({ "not": "array" })));
        assert!(r.gh_remotes("r1", CancellationToken::new()).await.is_err());
    }

    #[tokio::test]
    async fn pushed_shas_는_hex_만_받고_개수를_자른다() {
        let a = "a".repeat(40);
        let b = "0123456789abcdef".repeat(4);
        let (r, f) = repos(data(json!([a, "ABCDEF".repeat(7), "g".repeat(40), 5, b, "a".repeat(41), a])));
        let got = r.pushed_shas_by_author("r1", "me@x", 3, CancellationToken::new()).await.unwrap();
        assert_eq!(got, vec![a.clone(), b.clone(), a.clone()]);
        assert_eq!(f.asked.lock().unwrap()[0], (Op::PushedShas, json!({ "repoId": "r1", "email": "me@x", "limit": 3 })));
        let got = r.pushed_shas_by_author("r1", "me@x", 1, CancellationToken::new()).await.unwrap();
        assert_eq!(got, vec![a]);
        let (r, _) = repos(data(json!("nope")));
        assert!(r.pushed_shas_by_author("r1", "me@x", 3, CancellationToken::new()).await.is_err());
    }

    #[tokio::test]
    async fn client_id_와_repo_ids_는_틀리면_없음으로_줄인다() {
        let (r, _) = repos(data(json!("Iv1.abc")));
        assert_eq!(r.github_client_id().await.as_deref(), Some("Iv1.abc"));
        for bad in [json!(""), json!(5), Value::Null, json!("x".repeat(257))] {
            let (r, _) = repos(data(bad));
            assert_eq!(r.github_client_id().await, None);
        }
        let (r, _) = repos(Some(Err("실패".to_string())));
        assert_eq!(r.github_client_id().await, None);
        assert!(r.repo_ids().await.is_empty());

        let (r, _) = repos(data(json!(["a", "", 5, "b"])));
        assert_eq!(r.repo_ids().await, vec!["a".to_string(), "b".to_string()]);
        let (r, _) = repos(data(json!((0..=MAX_REPO_IDS).map(|i| format!("r{i}")).collect::<Vec<_>>())));
        assert!(r.repo_ids().await.is_empty());
        let (r, _) = repos(data(json!({ "a": 1 })));
        assert!(r.repo_ids().await.is_empty());
    }

    #[tokio::test]
    async fn invalidate_는_기다리지_않는_질의로_간다() {
        let (r, f) = repos(None);
        r.invalidate_gh_remotes();
        assert_eq!(f.told.lock().unwrap().as_slice(), &[(Op::InvalidateGhRemotes, json!({}))]);
        assert!(f.asked.lock().unwrap().is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn 응답이_없으면_제한_시간에_끝나고_대기를_버린다() {
        let (r, f) = repos(None);
        let r = r.with_timeout(Duration::from_secs(5));
        let err = r.gh_repo("r1").await.unwrap_err();
        assert!(err.contains("제때 응답하지 않았습니다"));
        assert_eq!(f.dropped.load(Ordering::SeqCst), 1, "시간 초과면 전송의 대기 future 가 버려진다");
    }

    #[tokio::test]
    async fn 취소하면_바로_끝나고_대기를_버린다() {
        let (r, f) = repos(None);
        let cancel = CancellationToken::new();
        cancel.cancel();
        assert_eq!(r.gh_remotes("r1", cancel).await.unwrap_err(), "aborted");
        assert_eq!(f.dropped.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn op_이름은_닫힌_목록이고_사이드카의_repoq_ops_와_같다() {
        let names: Vec<&str> = Op::ALL.iter().map(|o| o.as_str()).collect();
        assert_eq!(names, ["ghRepo", "ghRemotes", "pushedShas", "invalidateGhRemotes", "githubClientId", "repoIds"]);
        let proto = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../src/sidecar/protocol.ts")).unwrap();
        let line = proto.lines().find(|l| l.contains("export const REPOQ_OPS")).expect("REPOQ_OPS");
        for n in &names {
            assert!(line.contains(&format!("'{n}'")), "{n}");
        }
        assert_eq!(line.matches('\'').count(), names.len() * 2);
    }

    #[test]
    fn 질의_줄은_매개변수가_예약_키를_덮지_못한다() {
        let v: Value = serde_json::from_str(&query_line(7, Op::GhRepo, &json!({ "t": "evil", "id": 9, "op": "x" }))).unwrap();
        assert_eq!(v["t"], "repoq");
        assert_eq!(v["id"], 7);
        assert_eq!(v["op"], "ghRepo");
        assert_eq!(v["p"]["t"], "evil");
        let t: Value = serde_json::from_str(&tell_line(Op::InvalidateGhRemotes, &json!({}))).unwrap();
        assert!(t.get("id").is_none());
        assert_eq!(serde_json::from_str::<Value>(&cancel_line(3)).unwrap(), json!({ "t": "repoqCancel", "id": 3 }));
    }

    #[test]
    fn 토큰_줄은_json_이고_없으면_null() {
        assert_eq!(gh_token_line(None).as_str(), "{\"t\":\"ghToken\",\"token\":null}\n");
        let tok = SecretString::new("ghp_abc\"\\\n\u{1}é");
        let line = gh_token_line(Some(&tok));
        assert!(line.ends_with("}\n"));
        let v: Value = serde_json::from_str(line.trim_end()).unwrap();
        assert_eq!(v, json!({ "t": "ghToken", "token": "ghp_abc\"\\\n\u{1}é" }));
        // 비었거나 상한을 넘으면 없는 것이다
        assert!(gh_token_line(Some(&SecretString::new(""))).contains("null"));
        let long = SecretString::new("x".repeat(MAX_TOKEN_BYTES + 1));
        assert!(gh_token_line(Some(&long)).contains("null"));
        assert!(!gh_token_line(Some(&long)).contains("xxxx"));
        assert!(gh_token_line(Some(&SecretString::new("x".repeat(MAX_TOKEN_BYTES)))).contains("xxxx"));
    }
}
