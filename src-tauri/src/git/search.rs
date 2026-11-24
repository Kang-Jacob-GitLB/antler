//! `git:search`(`src/main/git/commands.ts` 의 `search`·`reachable`·`mergeInGraphOrder`): 메시지·작성자·해시 검색. 큐·취소 등록·envelope 은 라우터가 맡는다.
//!
//! 보안(인자 주입 경계): 질의는 renderer 가 보낸 자유 문자열이다. 실행기가 인자를 전역 옵션 바로 뒤에 붙이므로 옵션으로 읽히는 값이 들어가면
//! `-c alias.x=!cmd`·`--output=<경로>` 가 된다. 그래서 질의는 **옵션 접두가 붙은 한 인자**(`--grep=<q>`·`--author=<q>`)로만 들어가고(`-` 로 시작해도 옵션이 될 수 없다),
//! `-F`(고정 문자열 — 정규식이 아니다)·`-i` 가 상수로 붙는다. 해시 갈래는 질의가 16진수 4~40자일 때만 열리고(`<q>^{commit}` 한 인자 — 16진수뿐이라 옵션이 될 수 없다),
//! git 이 돌려준 해시도 다시 16진수인지 확인한 뒤에야 `rev-list` 의 인자 자리에 넣는다. 범위(scope)는 `scope.rs` 가 검증한 값만 정해진 자리로 간다.
//! NUL 이 든 질의는 git 을 띄우기 전에 거부한다(TS 는 Node 의 spawn 인자 검사가 막는다 — Rust 는 WSL argv·SSH 인용으로 가기 전에 막아야 한다).
//! 질의 길이 상한은 TS 에 없어 두지 않는다(운영체제 명령줄 한도를 넘으면 실행기가 오류를 낸다).

use std::collections::HashSet;

use serde_json::{json, Value};
use tokio::io::AsyncReadExt;
use tokio_util::sync::CancellationToken;

use crate::git::commit::{ok, HexHash};
use crate::git::exec::ssh::is_read_ahead_exceeded;
use crate::git::exec::{ExecError, GitExecutor, RunOpts};
use crate::git::fetch::js_trim;
use crate::git::log_service::is_empty_repo_error;
use crate::git::parsers::nul_records::{NulRecordSplitter, MAX_STREAM_RECORD};
use crate::git::scope::{parse_scope, scope_args};

/// 검색 결과 상한. 넘으면 `truncated` 로 알린다(`SEARCH_LIMIT`)
pub const SEARCH_LIMIT: usize = 10_000;

/// 병합 스트림을 읽는 버퍼
const CHUNK: usize = 64 * 1024;

fn other(message: &str) -> ExecError {
    ExecError::Other { message: message.to_string() }
}

/// 검증을 마친 `git:search` 요청: 앞뒤 공백을 지운 질의와 보기 범위의 시작점 인자
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchRequest {
    query: String,
    scope_args: Vec<String>,
}

impl SearchRequest {
    /// TS 의 검증 순서: 범위(`scopeArgs` — `invalid scope`·`invalid solo`·`invalid hide`)가 먼저, 질의는 그다음이다.
    /// 문자열이 아닌 질의는 TS 가 `.trim` TypeError(code ERROR)라 문구만 다르다. 질의가 비면(공백뿐) 요청은 통과하고 git 없이 빈 결과가 된다
    pub fn parse(query: Option<&Value>, scope: Option<&Value>) -> Result<Self, ExecError> {
        let scope = parse_scope(scope).map_err(|m| ExecError::Other { message: m })?;
        let scope_args = scope_args(scope.as_ref()).map_err(|m| ExecError::Other { message: m })?;
        let Some(Value::String(query)) = query else { return Err(other("invalid query")) };
        let q = js_trim(query);
        if q.contains('\0') {
            return Err(other("invalid query"));
        }
        Ok(SearchRequest { query: q.to_string(), scope_args })
    }

    /// 앞뒤 공백을 지운 질의
    pub fn query(&self) -> &str {
        &self.query
    }

    /// 해시 갈래를 여는 질의인가: `/^[0-9a-f]{4,40}$/i`
    fn is_hash_like(&self) -> bool {
        (4..=40).contains(&self.query.len()) && self.query.bytes().all(|b| b.is_ascii_hexdigit())
    }

    /// 메시지·작성자 갈래의 `git log` 인자. `filter` 는 `--grep=<q>`·`--author=<q>` 한 인자다
    pub fn filter_args(&self, filter: &str) -> Vec<String> {
        let mut v: Vec<String> = ["log", "--format=%H", "-z", "--date-order", "-i", "-F"].iter().map(|s| (*s).to_string()).collect();
        v.push(format!("--max-count={}", SEARCH_LIMIT + 1));
        v.push(filter.to_string());
        v.extend(self.scope_args.iter().cloned());
        v.push("--".to_string());
        v
    }

    /// 해시 갈래의 인자. `--end-of-options` 가 없다 — 질의가 16진수뿐이라 옵션이 될 수 없다
    pub fn hash_args(&self) -> Vec<String> {
        ["rev-parse", "--verify", "--quiet"].iter().map(|s| (*s).to_string()).chain([format!("{}^{{commit}}", self.query)]).collect()
    }

    /// 도달 가능성 확인 인자. 뒤에 `--` 가 없다(TS 와 같다) — 앞의 해시는 호출자가 16진수로 확인한 값이다
    pub fn reachable_args(&self, hash: &HexHash) -> Vec<String> {
        let mut v: Vec<String> = ["rev-list", "-n1"].iter().map(|s| (*s).to_string()).collect();
        v.push(hash.as_str().to_string());
        v.push("--not".to_string());
        v.extend(self.scope_args.iter().cloned());
        v
    }

    /// 그래프 순서 병합용 스트림 인자
    pub fn merge_args(&self) -> Vec<String> {
        let mut v: Vec<String> = ["log", "--format=%H", "-z", "--date-order"].iter().map(|s| (*s).to_string()).collect();
        v.extend(self.scope_args.iter().cloned());
        v.push("--".to_string());
        v
    }
}

fn empty_result() -> Value {
    json!({ "hashes": [], "truncated": false })
}

/// 한 갈래: 커밋이 아직 없는 저장소는 일치 항목이 없는 것과 같다. 그 밖의 실패는 GIT 오류다
async fn run_filter(exec: &GitExecutor, repo_path: &str, args: &[String], opts: &RunOpts) -> Result<Vec<String>, ExecError> {
    let r = exec.run(repo_path, args, opts).await?;
    if r.code != 0 && is_empty_repo_error(&r.stderr) {
        return Ok(Vec::new());
    }
    let out = ok(r, args)?;
    Ok(String::from_utf8_lossy(&out).split('\0').filter(|h| !h.is_empty()).map(str::to_string).collect())
}

/// 메시지·작성자·해시 검색. 결과는 그래프와 같은 `--date-order` 순서를 유지한 합집합이고 해시 prefix 에 일치하는 커밋은 맨 앞이다.
/// 세 갈래는 큐 슬롯 하나 안에서 동시에 돈다. 취소 토큰은 모든 git 호출에 걸린다
pub async fn search(exec: &GitExecutor, repo_path: &str, req: &SearchRequest, cancel: &CancellationToken) -> Result<Value, ExecError> {
    if req.query.is_empty() {
        return Ok(empty_result());
    }
    let opts = RunOpts::with_cancel(cancel.clone());
    let (message_args, author_args) = (req.filter_args(&format!("--grep={}", req.query)), req.filter_args(&format!("--author={}", req.query)));
    let hash_args = req.hash_args();
    let (by_message, by_author, by_hash) = tokio::try_join!(
        run_filter(exec, repo_path, &message_args, &opts),
        run_filter(exec, repo_path, &author_args, &opts),
        async {
            if req.is_hash_like() {
                exec.run(repo_path, &hash_args, &opts).await.map(Some)
            } else {
                Ok(None)
            }
        }
    )?;

    let matched = merge_in_graph_order(exec, repo_path, req, by_message, by_author, cancel).await?;
    let mut seen: HashSet<String> = HashSet::new();
    let mut all: Vec<String> = Vec::new();
    let resolved = match &by_hash {
        Some(r) if r.code == 0 => js_trim(&String::from_utf8_lossy(&r.stdout)).to_string(),
        _ => String::new(),
    };
    // 스태시처럼 지금 보는 이력에서 닿지 않는 커밋은 그래프에서 찾아갈 수 없으므로 뺀다. git 이 돌려준 값이 16진수가 아니면 쓰지 않는다
    if let Ok(hash) = HexHash::parse(&resolved) {
        if reachable(exec, repo_path, req, &hash, &opts).await? && seen.insert(resolved.clone()) {
            all.push(resolved);
        }
    }
    for h in matched {
        if seen.insert(h.clone()) {
            all.push(h);
        }
    }
    let truncated = all.len() > SEARCH_LIMIT;
    all.truncate(SEARCH_LIMIT);
    Ok(json!({ "hashes": all, "truncated": truncated }))
}

/// 커밋이 보기 범위에서 도달 가능한지. `hash` 는 호출자가 `rev-parse` 결과를 16진수로 확인한 전체 해시다
async fn reachable(exec: &GitExecutor, repo_path: &str, req: &SearchRequest, hash: &HexHash, opts: &RunOpts) -> Result<bool, ExecError> {
    let r = exec.run(repo_path, &req.reachable_args(hash), opts).await?;
    Ok(r.code == 0 && js_trim(&String::from_utf8_lossy(&r.stdout)).is_empty())
}

/// 메시지 일치와 작성자 일치를 그래프(`--date-order`) 순서로 합친다. 두 목록은 같은 순서의 부분 수열이지만 서로 다른 항목끼리의 선후는 알 수 없으므로,
/// 둘 다 비어 있지 않을 때만 전체 순서를 스트림으로 훑어 합집합을 다 찾는 순간 멈춘다(스트림을 버리면 git 이 끊긴다).
/// SSH 스트림은 선읽기 상한(16MB)을 넘으면 오류로 끝난다 — 소비가 해시 집합 삽입뿐이라 지연으로 넘는 일은 거의 없다(TS 는 backpressure 라 넘지 않는다)
async fn merge_in_graph_order(
    exec: &GitExecutor,
    repo_path: &str,
    req: &SearchRequest,
    a: Vec<String>,
    b: Vec<String>,
    cancel: &CancellationToken,
) -> Result<Vec<String>, ExecError> {
    if a.is_empty() {
        return Ok(b);
    }
    if b.is_empty() {
        return Ok(a);
    }
    let want: HashSet<&str> = a.iter().chain(b.iter()).map(String::as_str).collect();
    let mut out: Vec<String> = Vec::new();
    let mut splitter = NulRecordSplitter::with_limit(MAX_STREAM_RECORD);
    let mut stream = exec.stream(repo_path, &req.merge_args(), &RunOpts::with_cancel(cancel.clone())).await?;
    let mut chunk = vec![0u8; CHUNK];
    loop {
        let n = match stream.read(&mut chunk).await {
            Ok(n) => n,
            // 토큰으로 깨어난 읽기 오류는 취소다
            Err(_) if cancel.is_cancelled() => return Err(ExecError::Aborted),
            Err(e) if is_read_ahead_exceeded(&e) => return Err(other("SSH 선읽기 상한(16MB)을 넘어 검색을 중단했습니다")),
            Err(e) => return Err(ExecError::Other { message: e.to_string() }),
        };
        if n == 0 {
            stream.finish().await?;
            break;
        }
        let records = splitter.push_checked(&chunk[..n]).map_err(|_| other("git log record too long"))?;
        for h in records {
            if want.contains(h.as_str()) {
                out.push(h);
            }
        }
        if out.len() >= want.len() {
            break;
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn req(q: &str, scope: Option<Value>) -> SearchRequest {
        SearchRequest::parse(Some(&json!(q)), scope.as_ref()).unwrap()
    }

    fn strs(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn 인자는_ts_와_바이트까지_같다() {
        // commands.ts:296-324 의 `base`·`revs`·`rev-parse`·`rev-list`·`mergeInGraphOrder` 와 같은 순서·값이다
        let r = req("  a b  ", None);
        assert_eq!(r.query(), "a b", "앞뒤 공백은 지운다");
        assert_eq!(
            r.filter_args("--grep=a b"),
            strs(&["log", "--format=%H", "-z", "--date-order", "-i", "-F", "--max-count=10001", "--grep=a b", "--ignore-missing", "--branches", "--remotes", "--tags", "HEAD", "--"])
        );
        assert_eq!(r.merge_args(), strs(&["log", "--format=%H", "-z", "--date-order", "--ignore-missing", "--branches", "--remotes", "--tags", "HEAD", "--"]));
        let h = HexHash::parse(&"a".repeat(40)).unwrap();
        assert_eq!(r.reachable_args(&h), strs(&["rev-list", "-n1", &"a".repeat(40), "--not", "--ignore-missing", "--branches", "--remotes", "--tags", "HEAD"]));
        assert_eq!(req("deadbeef", None).hash_args(), strs(&["rev-parse", "--verify", "--quiet", "deadbeef^{commit}"]));
        // 범위: solo 는 rev 그대로, glob 은 한 인자
        let solo = req("x", Some(json!({ "solo": ["refs/heads/main", "refs/remotes/origin/*"] })));
        assert_eq!(
            solo.filter_args("--author=x")[8..],
            strs(&["--ignore-missing", "--glob=refs/remotes/origin/*", "refs/heads/main", "--"])
        );
    }

    #[test]
    fn 옵션_모양의_질의는_접두가_붙은_한_인자로만_들어간다() {
        for q in ["--output=x", "-c alias.x=!echo", "--force", "-1", "--end-of-options", "--"] {
            let r = req(q, None);
            for f in ["--grep=", "--author="] {
                let args = r.filter_args(&format!("{f}{}", r.query()));
                // 질의는 접두가 붙은 한 인자로만 있고, 질의 그대로인 인자는 없다(옵션 자리에 올 수 없다). `--`·`--end-of-options` 는 상수 인자와 겹치지 않게 접두 인자만 센다
                assert!(args.contains(&format!("{f}{q}")), "{q}");
                assert_eq!(args.iter().filter(|a| a.starts_with(f)).count(), 1, "{q}");
                assert!(!args[..args.len() - 1].iter().any(|a| a == q && a != "--"), "{q}: 단독 인자로 들어갔다");
            }
            // 해시 갈래는 16진수 질의만 연다
            assert!(!r.is_hash_like(), "{q}");
        }
    }

    #[test]
    fn 해시_갈래는_16진수_4_40자일_때만_열린다() {
        for (q, want) in [("abcd", true), ("ABCD", true), (&"a".repeat(40), true), (&"a".repeat(41), false), ("abc", false), ("abcg", false), ("abcd ef", false), ("", false)] {
            let r = SearchRequest { query: q.to_string(), scope_args: Vec::new() };
            assert_eq!(r.is_hash_like(), want, "{q:?}");
        }
    }

    #[test]
    fn 검증은_범위가_먼저이고_질의는_문자열이며_nul_이_없어야_한다() {
        let msg = |r: Result<SearchRequest, ExecError>| r.unwrap_err().to_string();
        // 범위 오류가 질의 오류보다 먼저다(TS 는 scopeArgs 가 먼저 던진다)
        assert_eq!(msg(SearchRequest::parse(Some(&json!(5)), Some(&json!(null)))), "invalid scope");
        assert_eq!(msg(SearchRequest::parse(Some(&json!("a")), Some(&json!({ "hide": [&"a".repeat(40)] })))), "invalid hide");
        for q in [json!(5), json!(null), json!(["a"]), json!({ "x": 1 })] {
            assert_eq!(msg(SearchRequest::parse(Some(&q), None)), "invalid query", "{q}");
        }
        assert_eq!(msg(SearchRequest::parse(None, None)), "invalid query");
        assert_eq!(msg(SearchRequest::parse(Some(&json!("a\0b")), None)), "invalid query");
        assert_eq!(msg(SearchRequest::parse(Some(&json!("  \0  ")), None)), "invalid query", "NUL 은 공백이 아니라 지워지지 않는다");
        // 공백뿐인 질의는 통과하고 빈 질의가 된다(git 없이 빈 결과)
        assert_eq!(SearchRequest::parse(Some(&json!(" \t\u{feff}\u{a0} ")), None).unwrap().query(), "");
        // U+0085 는 JS trim 이 지우지 않는다
        assert_eq!(SearchRequest::parse(Some(&json!("\u{85}x")), None).unwrap().query(), "\u{85}x");
    }

    #[tokio::test]
    async fn 빈_질의는_git_을_띄우지_않고_빈_결과를_낸다() {
        // 존재하지 않는 저장소 경로 — git 을 띄웠다면 오류가 난다
        let exec = GitExecutor::Local(crate::git::exec::LocalExecutor::default());
        let r = search(&exec, "/nonexistent-antler-search", &req("   ", None), &CancellationToken::new()).await.unwrap();
        assert_eq!(r, json!({ "hashes": [], "truncated": false }));
    }
}
