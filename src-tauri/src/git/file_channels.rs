//! 파일 단위 채널 `git:compare`·`git:fileDiff`·`git:fileHistory`·`git:blame` 의 요청 검증과 git 호출
//! (`src/main/git/commands.ts` 의 `compareCommits`·`fileDiff`, `src/main/git/fileInsight.ts`).
//!
//! 보안: 실행기는 호출자 인자를 전역 옵션 바로 뒤에 붙이므로 renderer 값이 옵션 자리에 오면 `-c alias.x=!cmd`·`--output=<경로>` 가 된다.
//! 그래서 renderer 가 준 값은 모두 검증된 newtype 으로만 들어오고, argv 를 만드는 함수(`diff_args`·`history_args`·`blame_args`·`cat_file_args`)는
//! 서브커맨드·옵션을 상수로 두고 그 newtype 을 정해진 자리에만 넣는다.
//! - 해시: [`HexHash`] — 16진수 4~64자뿐이라 `-` 로 시작할 수 없다.
//! - 파일 경로: [`FilePath`] — 비어 있지 않은 문자열이고 NUL 이 없다(TS `assertPath` 와 같다. **그 밖의 제한은 TS 에도 없다**: 길이·절대 경로·`..` 는
//!   거르지 않는다). 경로는 항상 `--` 뒤에 둔다. 예외는 blame 의 크기 조회 `cat-file -s <해시>:<경로>` 하나로, 경로가 `<16진수>:` 접두가 붙은 한 인자 안에 있어
//!   옵션으로 해석될 수 없다. `GIT_LITERAL_PATHSPECS=1`(실행기 `GIT_ENV`)이라 `:(glob)`·`*` 같은 pathspec 매직도 리터럴이다.
//! - git 이 돌려준 해시(`rev-parse HEAD`·`rev-list --parents`)도 [`HexHash`] 로 다시 검사한 뒤 argv 에 넣는다.

use serde::Serialize;
use serde_json::{json, Map, Value};
use tokio::io::AsyncReadExt;
use tokio_util::sync::CancellationToken;

use crate::git::commit::{changed_files, ok, summarize, utf16_len, HexHash, Range};
use crate::git::exec::{ExecError, GitExecutor, RunOpts};
use crate::git::log_service::is_empty_repo_error;
use crate::git::parsers::blame::parse_blame;
use crate::git::parsers::changes::FileChange;
use crate::git::parsers::diff::{DiffParseOpts, DiffParser, FileDiff};
use crate::git::parsers::file_history::{parse_file_history, FILE_HISTORY_FORMAT_ARGS};
use crate::git::parsers::{js_number_or_zero, js_trim, JsNumber};

/// 일반 diff 요청이 읽는 입력 바이트 상한(`DIFF_MAX_BYTES`)
pub const DIFF_MAX_BYTES: usize = 8 * 1024 * 1024;
/// "전체 보기"(full) 요청의 입력 바이트 상한(`DIFF_FULL_MAX_BYTES`)
pub const DIFF_FULL_MAX_BYTES: usize = 64 * 1024 * 1024;
/// "전체 보기"의 줄 수 상한. **TS 에는 없다(알려진 차이)** — 한 줄이 1바이트인 64MB 파일은 3,300만 줄이고, 그것을 `serde_json::Value`
/// 로 만들면(줄당 수백 바이트) 수십 GB 가 되어 앱이 죽는다(보안 리뷰). 잘린 diff 가 줄 수를 세는 상한(`DEFAULT_COUNT_LIMIT`)과 같은 값이다
pub const DIFF_FULL_MAX_LINES: usize = crate::git::parsers::diff::DEFAULT_COUNT_LIMIT;
/// 한 번에 받을 수 있는 히스토리 항목 수 상한
pub const FILE_HISTORY_MAX_COUNT: usize = 500;
/// 건너뛸 수 있는 최대 항목 수. 이 이상은 git 을 부르지 않고 빈 페이지다
pub const FILE_HISTORY_MAX_SKIP: f64 = 100_000.0;
/// 이보다 큰 파일은 blame 하지 않는다
pub const BLAME_MAX_BYTES: f64 = 512.0 * 1024.0;
const BLAME_MAX_BUFFER: usize = 128 * 1024 * 1024;
/// diff 스트림을 읽는 청크 크기(파서 결과는 청크 경계와 무관하다 — 골든이 1·3·7·4096 바이트 분할로 확인한다)
const DIFF_CHUNK: usize = 64 * 1024;

fn other(message: &str) -> ExecError {
    ExecError::Other { message: message.to_string() }
}

/// 검증된 파일 경로. TS `assertPath`(fileInsight.ts)·`fileDiff` 의 경로 검사와 같다: 문자열이고 비어 있지 않고 NUL 이 없다
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilePath(String);

impl FilePath {
    /// `v` 가 None 이면 요청에 키가 없는 것(undefined)이다
    pub fn parse(v: Option<&Value>) -> Result<Self, ExecError> {
        match v {
            Some(Value::String(s)) if !s.is_empty() && !s.contains('\0') => Ok(FilePath(s.clone())),
            _ => Err(other("invalid path")),
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// JS 의 참 거짓(`req.full ? … : …`)
pub fn js_truthy(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().is_some_and(|f| f != 0.0 && !f.is_nan()),
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Array(_) | Value::Object(_)) => true,
    }
}

/// JS `Number.isInteger(v)` 가 참인 숫자(JSON 에서는 `5.0` 도 정수다)
fn js_integer(v: Option<&Value>) -> Option<f64> {
    let f = v?.as_f64()?;
    (f.is_finite() && f.fract() == 0.0).then_some(f)
}

fn object(req: Option<&Value>) -> Result<&Map<String, Value>, ExecError> {
    req.and_then(Value::as_object).ok_or_else(|| other("invalid request"))
}

/// 요청 객체의 해시 필드. 키가 없으면 None, 있는데 16진수 문자열이 아니면(null 포함) 오류
fn hex_field(o: &Map<String, Value>, key: &str, what: &str) -> Result<Option<HexHash>, ExecError> {
    match o.get(key) {
        None => Ok(None),
        Some(Value::String(s)) => HexHash::parse_named(s, what).map(Some),
        Some(_) => Err(other(&format!("invalid {what}"))),
    }
}

fn hex_arg(v: Option<&Value>, what: &str) -> Result<HexHash, ExecError> {
    match v {
        Some(Value::String(s)) => HexHash::parse_named(s, what),
        _ => Err(other(&format!("invalid {what}"))),
    }
}

/// git 이 돌려준 해시. 16진수가 아니면 출력이 이상한 것이다(인자 자리에 그대로 쓰지 않는다)
fn git_hash(s: &str) -> Result<HexHash, ExecError> {
    HexHash::parse(s).map_err(|_| other("unexpected git output"))
}

/// `rev-parse --verify --quiet HEAD`. 커밋이 없거나 풀 수 없으면 None
async fn resolve_head(exec: &GitExecutor, repo_path: &str, opts: &RunOpts) -> Result<Option<HexHash>, ExecError> {
    let r = exec.run(repo_path, &["rev-parse", "--verify", "--quiet", "HEAD"], opts).await?;
    if r.code != 0 {
        return Ok(None);
    }
    let out = String::from_utf8_lossy(&r.stdout);
    match js_trim(&out) {
        "" => Ok(None),
        h => git_hash(h).map(Some),
    }
}

// ── git:compare ────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompareResult {
    /// 요청에서 받은 그대로(대소문자 유지)
    pub from: String,
    pub to: String,
    pub files: Vec<FileChange>,
    pub additions: JsNumber,
    pub deletions: JsNumber,
    pub truncated: bool,
}

/// `git:compare(repoId, from, to)` 의 renderer 인자. 검증 순서(from → to)와 문구는 TS `compareCommits` 와 같다
pub fn parse_compare(from: Option<&Value>, to: Option<&Value>) -> Result<(HexHash, HexHash), ExecError> {
    Ok((hex_arg(from, "from hash")?, hex_arg(to, "to hash")?))
}

/// 두 커밋 사이(from → to)의 변경 파일 목록. `git diff from to` 와 같은 트리 대 트리 비교이고 커밋 상세와 같은 파서·상한을 쓴다
pub async fn compare(exec: &GitExecutor, repo_path: &str, from: &HexHash, to: &HexHash) -> Result<CompareResult, ExecError> {
    let all = changed_files(exec, repo_path, &Range::From(from.clone(), to.clone())).await?;
    let (files, additions, deletions, truncated) = summarize(all);
    Ok(CompareResult { from: from.as_str().to_string(), to: to.as_str().to_string(), files, additions, deletions, truncated })
}

/// 비교 결과가 차지하는 메모리 추정치(`estimateCompareBytes`)
pub fn estimate_compare_bytes(d: &CompareResult) -> u64 {
    let mut n = 256;
    for f in &d.files {
        n += 96 + (utf16_len(&f.path) + f.old_path.as_deref().map_or(0, utf16_len)) as u64 * 2;
    }
    n
}

// ── git:fileDiff ───────────────────────────────────────

/// 검증을 마친 `git:fileDiff` 요청
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffRequest {
    pub hash: HexHash,
    pub parent: Option<HexHash>,
    pub path: FilePath,
    pub old_path: Option<FilePath>,
    pub full: bool,
}

impl DiffRequest {
    /// 검증 순서(해시 → parent → 경로 → oldPath)와 문구는 TS `fileDiff` 와 같다. `parent: null`·`oldPath: null` 은 오류다(키가 없어야 생략)
    pub fn parse(req: Option<&Value>) -> Result<Self, ExecError> {
        let o = object(req)?;
        let hash = hex_field(o, "hash", "commit hash")?.ok_or_else(|| other("invalid commit hash"))?;
        let parent = hex_field(o, "parent", "parent hash")?;
        let path = FilePath::parse(o.get("path"))?;
        let old_path = match o.get("oldPath") {
            None => None,
            some => Some(FilePath::parse(some)?),
        };
        Ok(DiffRequest { hash, parent, path, old_path, full: js_truthy(o.get("full")) })
    }

    /// 캐시·합치기 키(TS: `[repoId, hash 소문자, parent 소문자 또는 '', oldPath 또는 '', path, full|part].join('\0')`)
    pub fn key(&self, repo_id: &str) -> String {
        [
            repo_id,
            &self.hash.lower(),
            &self.parent.as_ref().map(HexHash::lower).unwrap_or_default(),
            self.old_path.as_ref().map_or("", FilePath::as_str),
            self.path.as_str(),
            if self.full { "full" } else { "part" },
        ]
        .join("\0")
    }
}

/// 파일 diff 의 `git diff-tree -p` 인자. `parent` 가 없으면 루트 커밋(빈 트리 대비)이다.
/// 경로(renderer 값)는 `--` 뒤에만 오고, 이름이 바뀐 파일이면 옛 경로가 먼저다
pub fn diff_args(req: &DiffRequest, parent: Option<&HexHash>) -> Vec<String> {
    let mut v: Vec<String> =
        ["diff-tree", "-p", "-M", "--no-color", "--no-ext-diff", "--no-textconv", "--no-commit-id", "-U3"].iter().map(|s| (*s).to_string()).collect();
    match parent {
        Some(p) => v.extend([p.as_str().to_string(), req.hash.as_str().to_string()]),
        None => v.extend(["--root".to_string(), req.hash.as_str().to_string()]),
    }
    v.push("--".to_string());
    match &req.old_path {
        Some(old) if old != &req.path => v.extend([old.as_str().to_string(), req.path.as_str().to_string()]),
        _ => v.push(req.path.as_str().to_string()),
    }
    v
}

/// 첫 부모 해시. 루트 커밋이면 None
async fn first_parent(exec: &GitExecutor, repo_path: &str, hash: &HexHash) -> Result<Option<HexHash>, ExecError> {
    let args = ["rev-list".to_string(), "--parents".to_string(), "-n1".to_string(), hash.as_str().to_string(), "--".to_string()];
    let out = ok(exec.run(repo_path, &args, &RunOpts::default()).await?, &args)?;
    let text = String::from_utf8_lossy(&out);
    js_trim(&text).split(' ').nth(1).map(git_hash).transpose()
}

/// 파일 하나의 diff. 출력은 스트림으로 읽어 줄 수 상한(기본 3000)까지만 객체로 만들고 나머지는 줄 수만 센다. full 이면 줄 수 상한 없이 64MB 까지 읽는다.
/// 상한을 넘기고 줄 수 세기도 끝나면 git 을 끊는다(스트림을 버리면 프로세스 트리가 죽는다). 끝까지 읽었으면 종료 코드를 확인한다
pub async fn file_diff(exec: &GitExecutor, repo_path: &str, req: &DiffRequest) -> Result<FileDiff, ExecError> {
    let parent = match &req.parent {
        Some(p) => Some(p.clone()),
        None => first_parent(exec, repo_path, &req.hash).await?,
    };
    let args = diff_args(req, parent.as_ref());
    let parse_opts = if req.full {
        DiffParseOpts { max_lines: Some(DIFF_FULL_MAX_LINES), max_bytes: Some(DIFF_FULL_MAX_BYTES), ..DiffParseOpts::default() }
    } else {
        DiffParseOpts { max_bytes: Some(DIFF_MAX_BYTES), ..DiffParseOpts::default() }
    };
    let mut parser = DiffParser::new(req.path.as_str(), req.old_path.as_ref().map(FilePath::as_str), &parse_opts);
    let mut stream = exec.stream(repo_path, &args, &RunOpts::default())?;
    let mut chunk = vec![0u8; DIFF_CHUNK];
    loop {
        let n = stream.read(&mut chunk).await.map_err(|e| ExecError::Other { message: e.to_string() })?;
        if n == 0 {
            stream.finish().await?;
            break;
        }
        if !parser.push(&chunk[..n]) {
            // 더 읽을 필요가 없다 — 스트림을 버려 git 을 끊는다
            drop(stream);
            break;
        }
    }
    Ok(parser.finish())
}

/// diff 가 차지하는 메모리 추정치(`estimateDiffBytes` — 길이는 UTF-16 단위)
pub fn estimate_diff_bytes(d: &FileDiff) -> u64 {
    let mut n = 256 + (utf16_len(&d.path) + d.old_path.as_deref().map_or(0, utf16_len)) as u64 * 2;
    for m in &d.meta {
        n += utf16_len(m) as u64 * 2 + 16;
    }
    for h in &d.hunks {
        n += 96 + utf16_len(&h.header) as u64 * 2;
        for l in &h.lines {
            n += 48 + utf16_len(&l.text) as u64 * 2;
        }
    }
    n
}

// ── git:fileHistory ────────────────────────────────────

/// 검증을 마친 `git:fileHistory` 요청. `skip`·`count` 는 TS 처럼 오류 없이 기본값·상한으로 맞춘다
#[derive(Debug, Clone, PartialEq)]
pub struct HistoryRequest {
    pub path: FilePath,
    pub rev: Option<HexHash>,
    /// 0 이상 정수(아주 큰 값은 `FILE_HISTORY_MAX_SKIP` 이상이라 git 을 부르지 않는다)
    pub skip: f64,
    /// 1~500
    pub count: usize,
}

impl HistoryRequest {
    pub fn parse(req: Option<&Value>) -> Result<Self, ExecError> {
        let o = object(req)?;
        let path = FilePath::parse(o.get("path"))?;
        let rev = hex_field(o, "rev", "commit hash")?;
        let skip = js_integer(o.get("skip")).filter(|s| *s > 0.0).unwrap_or(0.0);
        let count = js_integer(o.get("count")).unwrap_or(100.0).max(1.0).min(FILE_HISTORY_MAX_COUNT as f64) as usize;
        Ok(HistoryRequest { path, rev, skip, count })
    }
}

/// `git log --follow` 인자. rev 는 `--end-of-options` 뒤, 경로는 `--` 뒤다
pub fn history_args(rev: &HexHash, path: &FilePath, max_count: usize) -> Vec<String> {
    let mut v: Vec<String> = ["log", "--follow", "--no-color", "--no-ext-diff"].iter().map(|s| (*s).to_string()).collect();
    v.extend(FILE_HISTORY_FORMAT_ARGS.iter().map(|s| (*s).to_string()));
    v.push(format!("--max-count={max_count}"));
    v.extend(["--end-of-options".to_string(), rev.as_str().to_string(), "--".to_string(), path.as_str().to_string()]);
    v
}

/// 파일을 바꾼 커밋 목록. rev 를 생략하면 HEAD 를 지금 해시로 풀어 돌려준다(이후 페이지가 같은 이력을 보게).
/// `--follow` 는 `--skip` 을 무시하므로 skip+count+1 개를 읽어 직접 자른다
pub async fn file_history(exec: &GitExecutor, repo_path: &str, req: &HistoryRequest, cancel: &CancellationToken) -> Result<Value, ExecError> {
    let empty = || json!({ "entries": [], "hasMore": false });
    // 상한을 넘는 쪽은 더 읽지 않는다(hasMore=false 라 renderer 가 다시 요청하지 않는다)
    if req.skip >= FILE_HISTORY_MAX_SKIP {
        return Ok(empty());
    }
    let skip = req.skip as usize;
    let opts = RunOpts::with_cancel(cancel.clone());
    let rev = match &req.rev {
        Some(r) => r.clone(),
        None => match resolve_head(exec, repo_path, &opts).await? {
            Some(h) => h,
            // 커밋이 아직 없는 저장소는 이력이 없는 것과 같다
            None => return Ok(empty()),
        },
    };
    let args = history_args(&rev, &req.path, skip + req.count + 1);
    let r = exec.run(repo_path, &args, &opts).await?;
    if r.code != 0 && is_empty_repo_error(&r.stderr) {
        return Ok(empty());
    }
    let out = ok(r, &args)?;
    let all = parse_file_history(&out, req.path.as_str());
    let end = (skip + req.count).min(all.len());
    let entries = all.get(skip..end).unwrap_or(&[]);
    // 이번에 새 항목이 하나도 없으면 더 받을 게 없다. skip 상한에 닿는 페이지도 마지막으로 친다
    let has_more = !entries.is_empty() && all.len() > skip + req.count && ((skip + req.count) as f64) < FILE_HISTORY_MAX_SKIP;
    let entries = serde_json::to_value(entries).map_err(|e| ExecError::Other { message: e.to_string() })?;
    Ok(json!({ "entries": entries, "hasMore": has_more, "rev": rev.as_str() }))
}

// ── git:blame ──────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlameRequest {
    pub path: FilePath,
    pub rev: Option<HexHash>,
}

impl BlameRequest {
    pub fn parse(req: Option<&Value>) -> Result<Self, ExecError> {
        let o = object(req)?;
        let path = FilePath::parse(o.get("path"))?;
        Ok(BlameRequest { path, rev: hex_field(o, "rev", "commit hash")? })
    }
}

/// 파일 크기 조회 인자. 경로는 renderer 값이지만 `<16진수>:` 접두가 붙은 **한 인자**라 옵션으로 해석될 수 없다
pub fn cat_file_args(rev: &HexHash, path: &FilePath) -> Vec<String> {
    vec!["cat-file".to_string(), "-s".to_string(), format!("{}:{}", rev.as_str(), path.as_str())]
}

/// `git blame` 인자. 저장소 설정의 textconv 를 돌리지 않는다(`--no-textconv`). `ignore_revs_file_off` 는 `--no-ignore-revs-file`
pub fn blame_args(rev: &HexHash, path: &FilePath, ignore_revs_file_off: bool) -> Vec<String> {
    let mut v = vec!["blame".to_string()];
    if ignore_revs_file_off {
        v.push("--no-ignore-revs-file".to_string());
    }
    v.extend(["--no-textconv".to_string(), "--porcelain".to_string(), rev.as_str().to_string(), "--".to_string(), path.as_str().to_string()]);
    v
}

/// 지정 커밋(생략하면 HEAD) 시점 파일의 줄별 blame. 먼저 `cat-file -s` 로 크기를 보고 상한을 넘으면 blame 하지 않는다.
/// 바이너리는 출력에 NUL 이 있는 것으로 판별한다
pub async fn file_blame(exec: &GitExecutor, repo_path: &str, req: &BlameRequest, cancel: &CancellationToken) -> Result<Value, ExecError> {
    let opts = RunOpts::with_cancel(cancel.clone());
    let rev = match &req.rev {
        Some(r) => r.clone(),
        None => resolve_head(exec, repo_path, &opts).await?.ok_or_else(|| other("no commits yet"))?,
    };
    let path = req.path.as_str();
    let result = |binary: bool, too_large: bool, missing: bool, size: JsNumber, rest: Option<Value>| -> Value {
        let mut v = json!({ "rev": rev.as_str(), "path": path, "binary": binary, "tooLarge": too_large, "missing": missing, "size": size, "commits": {}, "groups": [], "lines": [] });
        if let (Some(Value::Object(extra)), Some(o)) = (rest, v.as_object_mut()) {
            o.extend(extra);
        }
        v
    };
    let sz = exec.run(repo_path, &cat_file_args(&rev, &req.path), &opts).await?;
    // 그 시점에 경로가 없거나(작업 트리에서 새로 add 한 파일 등) 파일이 아닌 객체(서브모듈)면 blame 할 게 없다
    if sz.code != 0 {
        return Ok(result(false, false, true, JsNumber(0.0), None));
    }
    let size = js_number_or_zero(&String::from_utf8_lossy(&sz.stdout));
    if size.0 > BLAME_MAX_BYTES {
        return Ok(result(false, true, false, size, None));
    }

    let run_opts = RunOpts { cancel: Some(cancel.clone()), max_buffer: Some(BLAME_MAX_BUFFER), ..RunOpts::default() };
    let mut args = blame_args(&rev, &req.path, false);
    let mut r = exec.run(repo_path, &args, &run_opts).await?;
    // blame.ignoreRevsFile 이 없는 파일을 가리키면 git 이 blame 자체를 거부한다 — 이번 실행에서만 설정된 목록을 비운다.
    // 목록 파일이 OID 가 아닌 줄을 담으면 git 은 `invalid object name: <그 줄>` 로 **파일 내용**을 stderr 에 낸다 —
    // 악성 저장소가 ignoreRevsFile 을 사용자 파일(자격 증명 등)로 가리키면 그 첫 줄이 오류 메시지로 renderer 에 간다(보안 리뷰).
    // 그런 실패도 목록을 끄고 다시 돌리고, 그래도 실패하면 원문 대신 고정 문구를 돌려준다
    let list_broken = |stderr: &str| {
        let s = stderr.to_ascii_lowercase();
        s.contains("object name list") || s.contains("invalid object name")
    };
    if r.code != 0 && list_broken(&r.stderr) {
        args = blame_args(&rev, &req.path, true);
        r = exec.run(repo_path, &args, &run_opts).await?;
        if r.code != 0 && list_broken(&r.stderr) {
            return Err(ExecError::Other { message: "git blame failed".into() });
        }
    }
    let out = ok(r, &args)?;
    let parsed = parse_blame(&out);
    if parsed.binary {
        return Ok(result(true, false, false, size, None));
    }
    let to_json = |e: serde_json::Error| ExecError::Other { message: e.to_string() };
    let rest = json!({
        "commits": serde_json::to_value(&parsed.commits).map_err(to_json)?,
        "groups": serde_json::to_value(&parsed.groups).map_err(to_json)?,
        "lines": parsed.lines,
    });
    Ok(result(false, false, false, size, Some(rest)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(s: &str) -> HexHash {
        HexHash::parse(s).unwrap()
    }

    fn p(s: &str) -> FilePath {
        FilePath::parse(Some(&json!(s))).unwrap()
    }

    fn err(r: Result<impl std::fmt::Debug, ExecError>) -> String {
        r.unwrap_err().to_string()
    }

    #[test]
    fn 경로는_비어_있지_않은_nul_없는_문자열만_받는다() {
        for ok in ["a", "dir/a b.txt", "한글/파일", "-x", "--output=pwned", ":(glob)*", "../a", "/abs", "a\nb", "[ab].txt", "\\\\evil\\share"] {
            assert_eq!(FilePath::parse(Some(&json!(ok))).unwrap().as_str(), ok);
        }
        for bad in [json!(""), json!("a\u{0}"), json!("\u{0}"), json!(5), json!(null), json!(["a"]), json!({ "a": 1 }), json!(true)] {
            assert_eq!(err(FilePath::parse(Some(&bad))), "invalid path", "{bad}");
        }
        assert_eq!(err(FilePath::parse(None)), "invalid path");
    }

    #[test]
    fn js_참_거짓() {
        for t in [json!(true), json!(1), json!(-1), json!(0.5), json!("no"), json!("0"), json!([]), json!({})] {
            assert!(js_truthy(Some(&t)), "{t}");
        }
        for f in [json!(false), json!(0), json!(0.0), json!(""), json!(null)] {
            assert!(!js_truthy(Some(&f)), "{f}");
        }
        assert!(!js_truthy(None));
    }

    #[test]
    fn 비교_인자는_from_to_순서로_검증한다() {
        assert_eq!(err(parse_compare(Some(&json!("zzzz")), Some(&json!("abcd")))), "invalid from hash");
        assert_eq!(err(parse_compare(Some(&json!("abcd")), Some(&json!("-x")))), "invalid to hash");
        assert_eq!(err(parse_compare(Some(&json!("abcd")), None)), "invalid to hash");
        assert_eq!(err(parse_compare(None, None)), "invalid from hash");
        assert_eq!(err(parse_compare(Some(&json!(1)), Some(&json!(2)))), "invalid from hash");
        let (a, b) = parse_compare(Some(&json!("ABCD")), Some(&json!("ef01"))).unwrap();
        assert_eq!((a.as_str(), b.as_str()), ("ABCD", "ef01"));
    }

    #[test]
    fn diff_요청은_해시_parent_경로_순으로_검증하고_null_을_거부한다() {
        let ok = DiffRequest::parse(Some(&json!({ "hash": "abcd", "path": "a.txt" }))).unwrap();
        assert_eq!((ok.parent, ok.old_path, ok.full), (None, None, false));
        let cases = [
            (json!({ "hash": "zzzz", "path": "a" }), "invalid commit hash"),
            (json!({ "hash": "abcd", "path": "a", "parent": null }), "invalid parent hash"),
            (json!({ "hash": "abcd", "path": "a", "parent": "-c" }), "invalid parent hash"),
            (json!({ "hash": "abcd", "path": "" }), "invalid path"),
            (json!({ "hash": "abcd" }), "invalid path"),
            (json!({ "hash": "abcd", "path": "a", "oldPath": null }), "invalid path"),
            (json!({ "hash": "abcd", "path": "a", "oldPath": "" }), "invalid path"),
            (json!({ "hash": "abcd", "path": "a", "oldPath": "x\u{0}" }), "invalid path"),
            (json!({ "path": "a" }), "invalid commit hash"),
            (json!({ "hash": 5, "path": "a" }), "invalid commit hash"),
        ];
        for (req, msg) in cases {
            assert_eq!(err(DiffRequest::parse(Some(&req))), msg, "{req}");
        }
        for req in [json!(null), json!("a.txt"), json!(5), json!([])] {
            assert_eq!(err(DiffRequest::parse(Some(&req))), "invalid request", "{req}");
        }
        assert_eq!(err(DiffRequest::parse(None)), "invalid request");
    }

    #[test]
    fn diff_캐시_키는_ts_와_같다() {
        let r = DiffRequest::parse(Some(&json!({ "hash": "ABCD", "path": "n.txt", "oldPath": "o.txt", "parent": "EF01", "full": 1 }))).unwrap();
        assert_eq!(r.key("repo"), "repo\0abcd\0ef01\0o.txt\0n.txt\0full");
        let r = DiffRequest::parse(Some(&json!({ "hash": "abcd", "path": "n.txt" }))).unwrap();
        assert_eq!(r.key("repo"), "repo\0abcd\0\0\0n.txt\0part");
    }

    #[test]
    fn diff_인자는_상수_뒤_해시들과_구분자_뒤_경로뿐이다() {
        let r = DiffRequest::parse(Some(&json!({ "hash": "2222", "path": "new.txt", "oldPath": "old.txt" }))).unwrap();
        let v = diff_args(&r, Some(&h("1111")));
        assert_eq!(v, ["diff-tree", "-p", "-M", "--no-color", "--no-ext-diff", "--no-textconv", "--no-commit-id", "-U3", "1111", "2222", "--", "old.txt", "new.txt"]);
        let v = diff_args(&r, None);
        assert_eq!(&v[8..], ["--root", "2222", "--", "old.txt", "new.txt"]);
        // oldPath 가 path 와 같으면 경로는 하나
        let r = DiffRequest::parse(Some(&json!({ "hash": "2222", "path": "a", "oldPath": "a" }))).unwrap();
        assert_eq!(&diff_args(&r, None)[8..], ["--root", "2222", "--", "a"]);
    }

    #[test]
    fn 적대적_경로는_구분자_뒤_한_인자로만_들어간다() {
        for path in ["--output=pwned.txt", "-calias.x=!echo", "--", "-c", ":(glob)*", "a\n--output=x"] {
            let r = DiffRequest::parse(Some(&json!({ "hash": "abcd", "path": path }))).unwrap();
            // 인자 목록의 끝은 `--`, 경로 한 개다. 그 앞(상수·해시)에는 경로와 같은 인자가 없다
            for v in [diff_args(&r, Some(&h("1111"))), history_args(&h("abcd"), &p(path), 5), blame_args(&h("abcd"), &p(path), false)] {
                let n = v.len();
                assert_eq!((v[n - 2].as_str(), v[n - 1].as_str()), ("--", path), "{v:?}");
                assert!(!v[..n - 2].iter().any(|a| a == path), "{path}: {v:?}");
            }
            // 크기 조회는 `<해시>:<경로>` 한 인자 — 첫 글자가 16진수라 옵션이 아니다
            let v = cat_file_args(&h("abcd"), &p(path));
            assert_eq!(v, ["cat-file", "-s", &format!("abcd:{path}")]);
            assert!(v[2].as_bytes()[0].is_ascii_hexdigit());
        }
    }

    #[test]
    fn 히스토리_요청은_skip_count_를_ts_처럼_맞춘다() {
        let req = |v: Value| HistoryRequest::parse(Some(&v)).unwrap();
        let r = req(json!({ "path": "a" }));
        assert_eq!((r.skip, r.count, r.rev), (0.0, 100, None));
        for (skip, want) in [(json!(5), 5.0), (json!(5.0), 5.0), (json!(1.5), 0.0), (json!(-3), 0.0), (json!("5"), 0.0), (json!(null), 0.0), (json!(0), 0.0), (json!(1e300), 1e300)] {
            assert_eq!(req(json!({ "path": "a", "skip": skip })).skip, want, "{skip}");
        }
        for (count, want) in [(json!(0), 1), (json!(-5), 1), (json!(1), 1), (json!(500), 500), (json!(1000), 500), (json!(2.5), 100), (json!("1"), 100), (json!(null), 100), (json!(7.0), 7)] {
            assert_eq!(req(json!({ "path": "a", "count": count })).count, want, "{count}");
        }
        for (v, msg) in [
            (json!({ "path": "" }), "invalid path"),
            (json!({ "path": "a", "rev": "HEAD" }), "invalid commit hash"),
            (json!({ "path": "a", "rev": "" }), "invalid commit hash"),
            (json!({ "path": "a", "rev": null }), "invalid commit hash"),
            (json!({ "path": "a", "rev": "-x" }), "invalid commit hash"),
            (json!(null), "invalid request"),
        ] {
            assert_eq!(err(HistoryRequest::parse(Some(&v))), msg, "{v}");
        }
        assert_eq!(err(HistoryRequest::parse(None)), "invalid request");
    }

    #[test]
    fn 히스토리_인자는_ts_와_같다() {
        let v = history_args(&h("ABCD"), &p("a b.txt"), 12);
        assert_eq!(
            v,
            [
                "log",
                "--follow",
                "--no-color",
                "--no-ext-diff",
                "-z",
                "--name-status",
                "-M",
                "--format=%x01%H%x1f%P%x1f%an%x1f%ae%x1f%at%x1f%s",
                "--max-count=12",
                "--end-of-options",
                "ABCD",
                "--",
                "a b.txt"
            ]
        );
    }

    #[test]
    fn blame_인자는_ts_와_같다() {
        assert_eq!(blame_args(&h("abcd"), &p("a.txt"), false), ["blame", "--no-textconv", "--porcelain", "abcd", "--", "a.txt"]);
        assert_eq!(blame_args(&h("abcd"), &p("a.txt"), true), ["blame", "--no-ignore-revs-file", "--no-textconv", "--porcelain", "abcd", "--", "a.txt"]);
        assert_eq!(BlameRequest::parse(Some(&json!({ "path": "a", "rev": "ABCD" }))).unwrap().rev.unwrap().as_str(), "ABCD");
        assert_eq!(err(BlameRequest::parse(Some(&json!({ "path": "a", "rev": null })))), "invalid commit hash");
    }

    #[test]
    fn 크기_추정은_utf16_길이를_쓴다() {
        let d = FileDiff { path: "a😀".into(), old_path: None, binary: false, hunks: vec![], truncated: false, total_lines: 0, total_is_lower_bound: None, meta: vec!["m".into()] };
        assert_eq!(estimate_diff_bytes(&d), 256 + 3 * 2 + (2 + 16));
    }
}
