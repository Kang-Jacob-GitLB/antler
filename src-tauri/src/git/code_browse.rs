//! 코드 탐색기(`git:resolveRev`·`git:tree`·`git:blob`·`git:onRemote`)(`src/main/git/codeBrowse.ts`): 입력 검증·`ls-tree` 파서·정렬·이미지 확장자 판정과
//! 그 위의 git 호출(`resolve_rev`·`list_tree`·`read_blob`·`is_on_remote`). 캐시·큐·취소·envelope 은 라우터가 맡는다.
//!
//! 보안: renderer 값은 검증된 값(`CommitId`·`normalize_dir` 결과·`is_resolvable_ref` 를 통과한 ref)만 argv 에 들어가고, **서브커맨드·옵션은 상수**다.
//! 해시·경로는 `<해시>:<경로>` 한 인자이거나(첫 글자가 16진수라 옵션이 아니다) `--end-of-options` 뒤다. **파일 내용은 `cat-file blob` 으로만 읽는다** —
//! `git show`·diff 계열과 달리 textconv·필터·속성이 적용되지 않는다. `ls-tree -l` 은 쓰지 않는다(크기를 구하려고 blob 마다 lazy fetch 가 일어난다).
//!
//! JS 와 맞춘 것: 이름 비교는 `toLowerCase`(`js_to_lowercase` — 길이가 달라지는 `İ`·`ẞ` 포함) 뒤 **UTF-16 코드 단위** 순이다(코드 포인트 순이 아니다 —
//! U+FF5E 보다 비 BMP 문자가 앞선다). 오류 문구는 TS 와 같은 `invalid …` 이다.

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use serde::Serialize;
use serde_json::{json, Value};

use crate::git::commit::{utf16_len, HexHash};
use crate::git::exec::{ExecError, GitExecutor, RunOpts};
use crate::git::fetch::{js_to_lowercase, js_trim};
use crate::git::lfs::parse_lfs_pointer;
use crate::git::parsers::{js_number, JsNumber};
use crate::git::scope::is_valid_scope_ref;

/// 텍스트로 보여 줄 파일 크기 상한
pub const CODE_TEXT_MAX_BYTES: u64 = 2 * 1024 * 1024;
/// 이미지 미리보기 크기 상한
pub const CODE_IMAGE_MAX_BYTES: u64 = 10 * 1024 * 1024;
/// 한 디렉터리에서 돌려줄 항목 수 상한(넘으면 정렬한 앞부분만 담고 truncated)
pub const CODE_TREE_MAX_ENTRIES: usize = 5000;

/// 검증된 전체 커밋 해시(40·64자 16진수, **대소문자 무시** — TS `COMMIT_ID_RE`). 짧은 해시는 모호하고 캐시 키도 갈려 받지 않는다.
/// 소문자만 받는 `scope::is_hash`·4~64자를 받는 `HexHash` 와 다르다. 값은 원문 그대로 두고 `lower()` 로 캐시·git 인자용 소문자를 얻는다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitId(String);

/// `^(?:[0-9a-f]{40}|[0-9a-f]{64})$` (대소문자 무시)
pub fn is_commit_id(s: &str) -> bool {
    matches!(s.len(), 40 | 64) && s.bytes().all(|b| b.is_ascii_hexdigit())
}

impl CommitId {
    /// TS `assertCommitId`: 문자열이 아니거나 모양이 틀리면 `invalid commit hash`
    pub fn parse(rev: &str) -> Result<Self, &'static str> {
        if is_commit_id(rev) {
            Ok(CommitId(rev.to_string()))
        } else {
            Err("invalid commit hash")
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn lower(&self) -> String {
        self.0.to_ascii_lowercase()
    }
}

/// 디렉터리 경로 정규화: '/' 앞뒤를 정리한다. '' 는 최상위. NUL·'.'·'..'·빈 구성 요소는 거부(`invalid path`)
pub fn normalize_dir(dir: &str) -> Result<String, &'static str> {
    if dir.contains('\0') {
        return Err("invalid path");
    }
    let trimmed = dir.trim_matches('/');
    if trimmed.is_empty() {
        return Ok(String::new());
    }
    if trimmed.split('/').any(|seg| matches!(seg, "" | "." | "..")) {
        return Err("invalid path");
    }
    Ok(trimmed.to_string())
}

/// 파일 경로 정규화: `normalize_dir` 이고 비어 있지 않아야 한다
pub fn normalize_file_path(path: &str) -> Result<String, &'static str> {
    let p = normalize_dir(path)?;
    if p.is_empty() {
        Err("invalid path")
    } else {
        Ok(p)
    }
}

/// `git:resolveRev` 가 받는 ref: `HEAD` · 16진수 4~64자 · 전체 ref 이름·40/64자 해시(`is_valid_scope_ref`). 단축 이름(main)·리비전 식(HEAD~1)·
/// 옵션처럼 보이는 값은 거부한다. 리모트 전체 패턴(`refs/remotes/x/*`)은 `is_valid_scope_ref` 를 통과하지만 하나의 ref 가 아니라 뺀다.
pub fn is_resolvable_ref(r: &str) -> bool {
    r == "HEAD" || HexHash::parse(r).is_ok() || (is_valid_scope_ref(&Value::String(r.to_string())) && !r.ends_with("/*"))
}

/// `rev-parse --verify` 결과를 커밋 해시로 확정한다. 종료 코드가 0 이 아니거나 출력이 40·64자 hex 가 아니면 오류(문구에 ref 를 넣는다 —
/// 검증을 통과한 값이라 제어 문자가 없다). 돌려주는 값은 소문자다.
pub fn resolved_commit_id(exit_ok: bool, stdout: &str, r: &str) -> Result<String, String> {
    let hash = if exit_ok { js_trim(stdout) } else { "" };
    if is_commit_id(hash) {
        Ok(hash.to_ascii_lowercase())
    } else {
        Err(format!("커밋을 찾을 수 없습니다: {r}"))
    }
}

/// `ls-tree` 가 실패했을 때 "객체가 로컬에 없다"로 볼 stderr 인가(partial clone·잘못된 해시). 정규식 `/…/i` 와 같다(패턴은 모두 ASCII)
pub fn is_missing_object_error(stderr: &str) -> bool {
    const PATTERNS: [&str; 7] = ["not a valid object name", "not a tree object", "bad object", "unable to read", "could not read", "could not parse", "missing"];
    let lower = stderr.to_ascii_lowercase();
    PATTERNS.iter().any(|p| lower.contains(p))
}

const IMAGE_MIME: [(&str, &str); 9] = [
    ("png", "image/png"),
    ("jpg", "image/jpeg"),
    ("jpeg", "image/jpeg"),
    ("gif", "image/gif"),
    ("webp", "image/webp"),
    ("bmp", "image/bmp"),
    ("ico", "image/x-icon"),
    ("svg", "image/svg+xml"),
    ("avif", "image/avif"),
];

/// 확장자(대소문자 무관)로 본 이미지 MIME. 확장자가 없거나 목록에 없으면 None.
/// (TS 는 `IMAGE_MIME[ext]` 로 객체를 조회해 `constructor`·`toString` 같은 확장자가 함수로 잡힌다 — 거기에 맞추지 않는다.)
pub fn image_mime(path: &str) -> Option<&'static str> {
    let base = path.rfind('/').map_or(path, |i| &path[i + 1..]);
    let dot = base.rfind('.')?;
    let ext = js_to_lowercase(&base[dot + 1..]);
    IMAGE_MIME.iter().find(|(k, _)| *k == ext).map(|(_, m)| *m)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CodeEntryKind {
    Dir,
    File,
    Symlink,
    Submodule,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CodeTreeEntry {
    /// 이 디렉터리 안의 이름
    pub name: String,
    /// 저장소 최상위 기준 경로('/' 구분)
    pub path: String,
    pub kind: CodeEntryKind,
    /// git 파일 모드(예: 100644, 100755, 120000, 040000, 160000)
    pub mode: String,
    pub oid: String,
}

fn kind_of(mode: &str, ty: &str) -> CodeEntryKind {
    if ty == "tree" || mode == "040000" {
        CodeEntryKind::Dir
    } else if mode == "120000" {
        CodeEntryKind::Symlink
    } else if ty == "commit" || mode == "160000" {
        CodeEntryKind::Submodule
    } else {
        CodeEntryKind::File
    }
}

/// 대소문자를 무시하고(로캘과 무관하게 코드 유닛 순) 비교한 뒤 같으면 원문으로 비교한다
pub fn compare_names(a: &str, b: &str) -> std::cmp::Ordering {
    let (la, lb) = (js_to_lowercase(a), js_to_lowercase(b));
    la.encode_utf16().cmp(lb.encode_utf16()).then_with(|| a.encode_utf16().cmp(b.encode_utf16()))
}

/// `ls-tree -z` 출력(`<mode> <type> <oid>\t<name>\0` 반복)을 읽어 디렉터리 먼저, 이름순으로 정렬한다.
/// 탭이 없거나 필드가 비었거나 이름이 빈 레코드는 건너뛴다(마지막 NUL 이 없는 잘린 레코드도 같은 규칙으로 읽는다). `dir` 은 정규화된 값이다.
pub fn parse_tree(buf: &[u8], dir: &str) -> Vec<CodeTreeEntry> {
    let text = String::from_utf8_lossy(buf);
    let mut entries: Vec<CodeTreeEntry> = Vec::new();
    for rec in text.split('\0') {
        let Some(tab) = rec.find('\t') else { continue };
        let mut head = rec[..tab].split(' ');
        let (mode, ty, oid) = (head.next().unwrap_or(""), head.next().unwrap_or(""), head.next().unwrap_or(""));
        let name = &rec[tab + 1..];
        if mode.is_empty() || ty.is_empty() || oid.is_empty() || name.is_empty() {
            continue;
        }
        let path = if dir.is_empty() { name.to_string() } else { format!("{dir}/{name}") };
        entries.push(CodeTreeEntry { name: name.to_string(), path, kind: kind_of(mode, ty), mode: mode.to_string(), oid: oid.to_string() });
    }
    // 안정 정렬(JS `Array.prototype.sort` 도 안정)
    entries.sort_by(|x, y| {
        let rank = |e: &CodeTreeEntry| u8::from(e.kind != CodeEntryKind::Dir);
        rank(x).cmp(&rank(y)).then_with(|| compare_names(&x.name, &y.name))
    });
    entries
}

// ── git 호출 ───────────────────────────────────────────

/// 파일 내용을 읽을 때 상한을 조금 넘어도 실패하지 않도록 주는 여유(`BUFFER_SLACK`)
const BUFFER_SLACK: usize = 64 * 1024;
/// LFS 포인터는 이 크기 이하다(`LFS_POINTER_MAX_BYTES`)
const LFS_POINTER_MAX_BYTES: f64 = 1024.0;
/// NUL 바이트로 바이너리를 판정할 앞부분 길이(git 의 휴리스틱과 같다)
const BINARY_SNIFF_BYTES: usize = 8000;

fn other(message: &str) -> ExecError {
    ExecError::Other { message: message.to_string() }
}

/// `git:resolveRev` 의 ref 인자. 문자열이고 `is_resolvable_ref` 를 통과해야 한다(TS `resolveRev` 의 두 검사가 같은 문구 `invalid ref`)
pub fn parse_ref(v: Option<&Value>) -> Result<&str, ExecError> {
    match v {
        Some(Value::String(s)) if is_resolvable_ref(s) => Ok(s),
        _ => Err(other("invalid ref")),
    }
}

/// `git:onRemote` 의 rev 인자(TS `assertCommitId`)
pub fn parse_rev(v: Option<&Value>) -> Result<CommitId, ExecError> {
    match v {
        Some(Value::String(s)) => CommitId::parse(s).map_err(other),
        _ => Err(other("invalid commit hash")),
    }
}

/// 요청 객체의 필드. 객체가 아닌 값은 필드가 없는 것과 같다(`req.rev` 가 undefined) — 단 `null`·인자 없음은 TS 에서 TypeError(code ERROR)라 `invalid request` 로 거절한다
pub(crate) fn field<'a>(req: Option<&'a Value>, key: &str) -> Result<Option<&'a Value>, ExecError> {
    match req {
        None | Some(Value::Null) => Err(other("invalid request")),
        Some(Value::Object(o)) => Ok(o.get(key)),
        Some(_) => Ok(None),
    }
}

/// 검증을 마친 `git:tree` 요청. 검증 순서는 TS `listTree` 와 같다: rev → dir(키가 없으면 최상위가 아니라 `invalid path`)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeRequest {
    pub rev: CommitId,
    pub dir: String,
}

impl TreeRequest {
    pub fn parse(req: Option<&Value>) -> Result<Self, ExecError> {
        let rev = parse_rev(field(req, "rev")?)?;
        let dir = match field(req, "dir")? {
            Some(Value::String(s)) => normalize_dir(s).map_err(other)?,
            _ => return Err(other("invalid path")),
        };
        Ok(TreeRequest { rev, dir })
    }

    /// 캐시 키(TS `codeKey`: `repoId\0rev 소문자\0정규화 경로`)
    pub fn key(&self, repo_id: &str) -> String {
        [repo_id, &self.rev.lower(), &self.dir].join("\0")
    }
}

/// 검증을 마친 `git:blob` 요청
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlobRequest {
    pub rev: CommitId,
    pub path: String,
}

impl BlobRequest {
    pub fn parse(req: Option<&Value>) -> Result<Self, ExecError> {
        let rev = parse_rev(field(req, "rev")?)?;
        let path = match field(req, "path")? {
            Some(Value::String(s)) => normalize_file_path(s).map_err(other)?,
            _ => return Err(other("invalid path")),
        };
        Ok(BlobRequest { rev, path })
    }

    pub fn key(&self, repo_id: &str) -> String {
        [repo_id, &self.rev.lower(), &self.path].join("\0")
    }
}

/// `rev-parse --verify --end-of-options <ref>^{commit}`. ref 는 `is_resolvable_ref` 를 통과한 값이다
pub fn resolve_rev_args(r: &str) -> Vec<String> {
    ["rev-parse", "--verify", "--end-of-options"].iter().map(|s| (*s).to_string()).chain([format!("{r}^{{commit}}")]).collect()
}

/// `cat-file -t|-s|blob <rev>:<경로>` — 경로는 16진수 접두가 붙은 **한 인자**라 옵션으로 해석될 수 없다
pub fn cat_file_args(kind: &str, spec: &str) -> Vec<String> {
    vec!["cat-file".to_string(), kind.to_string(), spec.to_string()]
}

/// `ls-tree -z --end-of-options <spec>`(`-l` 은 쓰지 않는다)
pub fn ls_tree_args(spec: &str) -> Vec<String> {
    vec!["ls-tree".to_string(), "-z".to_string(), "--end-of-options".to_string(), spec.to_string()]
}

/// `for-each-ref --count=1 --contains <rev 소문자> --format=%(refname) refs/remotes/`
pub fn on_remote_args(rev: &CommitId) -> Vec<String> {
    ["for-each-ref", "--count=1", "--contains"].iter().map(|s| (*s).to_string()).chain([rev.lower(), "--format=%(refname)".to_string(), "refs/remotes/".to_string()]).collect()
}

fn stdout_text(stdout: &[u8]) -> String {
    String::from_utf8_lossy(stdout).into_owned()
}

/// 'HEAD'·전체 ref 이름·커밋 해시를 커밋 해시(소문자)로 푼다. 못 풀면 ERROR(`커밋을 찾을 수 없습니다: …`)
pub async fn resolve_rev(exec: &GitExecutor, repo_path: &str, r: &str, opts: &RunOpts) -> Result<String, ExecError> {
    let res = exec.run(repo_path, &resolve_rev_args(r), opts).await?;
    resolved_commit_id(res.code == 0, &stdout_text(&res.stdout), r).map_err(|message| ExecError::Other { message })
}

/// `git:tree` 의 응답. 응답의 `rev` 는 요청이 보낸 표기 그대로다(소문자화는 캐시 키뿐)
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CodeTreeResult {
    pub rev: String,
    pub dir: String,
    pub entries: Vec<CodeTreeEntry>,
    pub missing: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub truncated: Option<bool>,
}

/// 커밋 시점 디렉터리 하나의 항목. 경로가 없거나 트리가 아니면 missing, 객체가 로컬에 없는 경우(partial clone)도 같다
pub async fn list_tree(exec: &GitExecutor, repo_path: &str, req: &TreeRequest, opts: &RunOpts, max_entries: usize) -> Result<CodeTreeResult, ExecError> {
    let (rev, dir) = (req.rev.as_str().to_string(), req.dir.clone());
    let missing = |rev: String, dir: String| CodeTreeResult { rev, dir, entries: Vec::new(), missing: true, truncated: None };
    // rev 는 16진수만 허용하므로 `rev:dir` 이 옵션으로 해석될 수 없다
    let spec = if dir.is_empty() { rev.clone() } else { format!("{rev}:{dir}") };
    if !dir.is_empty() {
        let t = exec.run(repo_path, &cat_file_args("-t", &spec), opts).await?;
        if t.code != 0 || js_trim(&stdout_text(&t.stdout)) != "tree" {
            return Ok(missing(rev, dir));
        }
    }
    let args = ls_tree_args(&spec);
    let r = exec.run(repo_path, &args, opts).await?;
    if r.code != 0 {
        // 커밋·트리 객체가 로컬에 없다(partial clone, 잘못된 해시)
        if is_missing_object_error(&r.stderr) {
            return Ok(missing(rev, dir));
        }
        let trimmed = js_trim(&r.stderr);
        let message = if trimmed.is_empty() { format!("git ls-tree failed ({})", r.code) } else { trimmed.to_string() };
        return Err(ExecError::Git { message, code: r.code, stderr: r.stderr, args });
    }
    let mut entries = parse_tree(&r.stdout, &dir);
    if entries.len() > max_entries {
        entries.truncate(max_entries);
        return Ok(CodeTreeResult { rev, dir, entries, missing: false, truncated: Some(true) });
    }
    Ok(CodeTreeResult { rev, dir, entries, missing: false, truncated: None })
}

/// TextDecoder(`fatal: false`)처럼: 깨진 바이트는 U+FFFD 로, 선두 BOM 은 한 번 벗긴다
fn decode_text(buf: &[u8]) -> String {
    let s = String::from_utf8_lossy(buf);
    s.strip_prefix('\u{feff}').map_or_else(|| s.to_string(), str::to_string)
}

fn lfs_result(rev: &str, path: &str, size: JsNumber, text: &str) -> Option<Value> {
    let lfs = parse_lfs_pointer(text)?;
    Some(json!({ "rev": rev, "path": path, "kind": "lfs", "size": size, "lfs": { "oid": lfs.oid, "size": lfs.size } }))
}

/// 커밋 시점 파일 내용(`CodeBlobResult` 의 JSON). `cat-file -t/-s` 로 종류·크기를 먼저 보고 상한을 넘으면 내용을 읽지 않는다.
/// 판정 순서: (이미지 확장자이고 포인터 상한 이하면 LFS 포인터) → 이미지 → NUL 이 있으면 바이너리 → LFS 포인터 → 텍스트.
pub async fn read_blob(exec: &GitExecutor, repo_path: &str, req: &BlobRequest, opts: &RunOpts) -> Result<Value, ExecError> {
    let (rev, path) = (req.rev.as_str(), req.path.as_str());
    let spec = format!("{rev}:{path}");
    let none = || json!({ "rev": rev, "path": path, "kind": "missing", "size": 0 });

    // 디렉터리(tree)·서브모듈(commit 객체는 이 저장소에 없다)·없는 경로·로컬에 없는 객체는 여기서 걸러진다
    let t = exec.run(repo_path, &cat_file_args("-t", &spec), opts).await?;
    if t.code != 0 || js_trim(&stdout_text(&t.stdout)) != "blob" {
        return Ok(none());
    }
    let s = exec.run(repo_path, &cat_file_args("-s", &spec), opts).await?;
    if s.code != 0 {
        return Ok(none());
    }
    let size = js_number(&stdout_text(&s.stdout));
    if !size.0.is_finite() || size.0 < 0.0 {
        return Ok(none());
    }

    let mime = image_mime(path);
    let limit = if mime.is_some() { CODE_IMAGE_MAX_BYTES } else { CODE_TEXT_MAX_BYTES };
    if size.0 > limit as f64 {
        return Ok(json!({ "rev": rev, "path": path, "kind": "tooLarge", "size": size }));
    }

    let run_opts = RunOpts { cancel: opts.cancel.clone(), max_buffer: Some(limit as usize + BUFFER_SLACK), ..RunOpts::default() };
    let r = exec.run(repo_path, &cat_file_args("blob", &spec), &run_opts).await?;
    if r.code != 0 {
        return Ok(none());
    }
    let buf = r.stdout;

    if let Some(mime) = mime {
        if size.0 <= LFS_POINTER_MAX_BYTES {
            if let Some(v) = lfs_result(rev, path, size, &decode_text(&buf)) {
                return Ok(v);
            }
        }
        return Ok(json!({ "rev": rev, "path": path, "kind": "image", "size": size, "dataUrl": format!("data:{mime};base64,{}", STANDARD.encode(&buf)) }));
    }
    if buf[..buf.len().min(BINARY_SNIFF_BYTES)].contains(&0) {
        return Ok(json!({ "rev": rev, "path": path, "kind": "binary", "size": size }));
    }

    let text = decode_text(&buf);
    if let Some(v) = lfs_result(rev, path, size, &text) {
        return Ok(v);
    }
    let mut v = json!({ "rev": rev, "path": path, "kind": "text", "size": size, "text": text });
    if std::str::from_utf8(&buf).is_err() {
        v["invalidUtf8"] = json!(true);
    }
    Ok(v)
}

/// 트리 캐시 항목의 비용(`200 * (항목 수 + 1)`)
pub fn tree_cost(t: &CodeTreeResult) -> u64 {
    200 * (t.entries.len() as u64 + 1)
}

/// 파일 내용 캐시 항목의 비용(`text 길이 * 2 + dataUrl 길이 + 1024` — 길이는 UTF-16 단위)
pub fn blob_cost(v: &Value) -> u64 {
    let len = |k: &str| v.get(k).and_then(Value::as_str).map_or(0, utf16_len) as u64;
    len("text") * 2 + len("dataUrl") + 1024
}

/// 커밋이 원격 추적 ref(`refs/remotes/`) 중 하나에 들어 있는가(= push 된 커밋인가). 줄 링크 경고용이다. 커밋이 로컬에 없으면 false
pub async fn is_on_remote(exec: &GitExecutor, repo_path: &str, rev: &CommitId, opts: &RunOpts) -> Result<bool, ExecError> {
    let r = exec.run(repo_path, &on_remote_args(rev), opts).await?;
    Ok(r.code == 0 && !js_trim(&stdout_text(&r.stdout)).is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(mode: &str, ty: &str, name: &str) -> String {
        format!("{mode} {ty} {}\t{name}\0", "a".repeat(40))
    }

    #[test]
    fn 커밋_해시는_대소문자를_가리지_않고_전체_길이만() {
        assert!(CommitId::parse(&"aB".repeat(20)).is_ok());
        assert!(CommitId::parse(&"f".repeat(64)).is_ok());
        let bads = ["".to_string(), "abc".into(), "a".repeat(41), "a".repeat(63), "g".repeat(40), format!("{}\n", "a".repeat(40)), format!("--{}", "a".repeat(38))];
        for bad in &bads {
            assert_eq!(CommitId::parse(bad), Err("invalid commit hash"), "{bad:?}");
        }
        assert_eq!(CommitId::parse(&"AB".repeat(20)).unwrap().lower(), "ab".repeat(20));
    }

    #[test]
    fn 경로_정규화() {
        assert_eq!(normalize_dir(""), Ok(String::new()));
        assert_eq!(normalize_dir("///"), Ok(String::new()));
        assert_eq!(normalize_dir("/src/a/"), Ok("src/a".into()));
        assert_eq!(normalize_dir("한글/디렉터리"), Ok("한글/디렉터리".into()));
        for bad in ["a/../b", "..", ".", "a//b", "a/./b", "a\0b", "a/.."] {
            assert_eq!(normalize_dir(bad), Err("invalid path"), "{bad:?}");
        }
        assert_eq!(normalize_file_path("/a/b.txt/"), Ok("a/b.txt".into()));
        assert_eq!(normalize_file_path(""), Err("invalid path"));
        assert_eq!(normalize_file_path("/"), Err("invalid path"));
        // 역슬래시·콜론은 구분자가 아니다(TS 와 같다)
        assert_eq!(normalize_dir("a\\..\\b"), Ok("a\\..\\b".into()));
    }

    #[test]
    fn resolve_ref_허용_규칙() {
        let oks = ["HEAD".to_string(), "abcd".into(), "A".repeat(64), "refs/heads/main".into(), "refs/tags/v1.0".into(), "refs/remotes/origin/feature/x".into(), "a".repeat(40)];
        for ok in &oks {
            assert!(is_resolvable_ref(ok), "{ok}");
        }
        let bads = ["", "main", "head", "HEAD~1", "HEAD^", "--output=x", "-x", "abc", "refs/remotes/origin/*", "refs/heads/a b", "refs/heads/../x", "refs/heads/x.lock"];
        for bad in bads {
            assert!(!is_resolvable_ref(bad), "{bad}");
        }
        assert!(!is_resolvable_ref(&"a".repeat(65)));
    }

    #[test]
    fn 해석_결과는_소문자_전체_해시여야_한다() {
        let h = "AB".repeat(20);
        assert_eq!(resolved_commit_id(true, &format!("  {h}\r\n"), "HEAD"), Ok("ab".repeat(20)));
        assert_eq!(resolved_commit_id(false, &h, "HEAD"), Err("커밋을 찾을 수 없습니다: HEAD".into()));
        assert!(resolved_commit_id(true, "abcd\n", "x").is_err());
        assert!(resolved_commit_id(true, "", "x").is_err());
    }

    #[test]
    fn 없는_객체_stderr_판정() {
        assert!(is_missing_object_error("fatal: Not a valid object name abc"));
        assert!(is_missing_object_error("error: unable to read tree"));
        assert!(is_missing_object_error("fatal: BAD OBJECT x"));
        assert!(is_missing_object_error("promisor remote: MISSING blob"));
        assert!(!is_missing_object_error("fatal: permission denied"));
        assert!(!is_missing_object_error(""));
    }

    #[test]
    fn 이미지_확장자() {
        assert_eq!(image_mime("a/b/logo.PNG"), Some("image/png"));
        assert_eq!(image_mime("x.jpeg"), Some("image/jpeg"));
        assert_eq!(image_mime(".svg"), Some("image/svg+xml"));
        assert_eq!(image_mime("dir.png/file"), None);
        assert_eq!(image_mime("a."), None);
        assert_eq!(image_mime("a.png.txt"), None);
        assert_eq!(image_mime("noext"), None);
        // 객체 프로토타입 이름은 확장자로 쳐 주지 않는다
        for e in ["constructor", "toString", "__proto__", "hasOwnProperty"] {
            assert_eq!(image_mime(&format!("a.{e}")), None);
        }
    }

    #[test]
    fn 정렬은_디렉터리_먼저_소문자화_뒤_utf16_순() {
        let mut input = String::new();
        for (m, t, n) in [
            ("100644", "blob", "b.txt"),
            ("100644", "blob", "A.txt"),
            ("040000", "tree", "zdir"),
            ("100644", "blob", "a.txt"),
            ("040000", "tree", "Adir"),
            ("120000", "blob", "link"),
            ("160000", "commit", "sub"),
        ] {
            input.push_str(&rec(m, t, n));
        }
        let names: Vec<_> = parse_tree(input.as_bytes(), "").into_iter().map(|e| (e.name, e.kind)).collect();
        use CodeEntryKind::*;
        assert_eq!(
            names,
            vec![("Adir".into(), Dir), ("zdir".into(), Dir), ("A.txt".into(), File), ("a.txt".into(), File), ("b.txt".into(), File), ("link".into(), Symlink), ("sub".into(), Submodule)]
        );
        // 대소문자만 다르면 원문 순(대문자가 앞)
        assert_eq!(compare_names("A", "a"), std::cmp::Ordering::Less);
        assert_eq!(compare_names("a", "B"), std::cmp::Ordering::Less);
    }

    #[test]
    fn 비교는_utf16_코드_단위_순() {
        // U+FF5E(BMP) 는 코드 포인트로는 U+1D49C(비 BMP)보다 앞이지만 UTF-16 으로는 서로게이트(D835)가 앞선다
        assert_eq!(compare_names("\u{1d49c}", "\u{ff5e}"), std::cmp::Ordering::Less);
        // İ(U+0130)는 소문자화하면 'i' + U+0307 로 길어진다: "i̇" < "ib" 가 아니라 'i'(69) 뒤 U+0307 > 'b' 이므로 "ib" 가 앞선다
        assert_eq!(compare_names("\u{130}", "ib"), std::cmp::Ordering::Greater);
        // ẞ(U+1E9E) → ß(U+00DF)
        assert_eq!(compare_names("\u{1e9e}", "\u{df}"), std::cmp::Ordering::Greater);
    }

    #[test]
    fn 트리_파서의_건너뛰기_규칙() {
        let h = "a".repeat(40);
        let mut input = String::new();
        input.push_str(&rec("100644", "blob", "ok.txt"));
        input.push_str(&format!("100644 blob {h} no-tab\0")); // 탭 없음
        input.push_str(&format!("100644 blob\t{h}x\0")); // oid 없음
        input.push_str(&format!(" blob {h}\tname\0")); // mode 비어 있음
        input.push_str(&format!("100644 blob {h}\t\0")); // 이름 비어 있음
        input.push_str(&format!("100644 blob {h} extra\twith\ttabs\0")); // 헤더의 여분 필드와 이름 속 탭
        input.push_str("\0\0");
        input.push_str(&format!("100644 blob {h}\ttruncated")); // 마지막 NUL 없음
        let got = parse_tree(input.as_bytes(), "src");
        let names: Vec<_> = got.iter().map(|e| (e.name.as_str(), e.path.as_str())).collect();
        assert_eq!(names, vec![("ok.txt", "src/ok.txt"), ("truncated", "src/truncated"), ("with\ttabs", "src/with\ttabs")]);
        assert!(parse_tree(b"", "").is_empty());
    }

    #[test]
    fn 깨진_utf8_이름은_대체_문자로() {
        let mut buf = format!("100644 blob {}\tbad-", "a".repeat(40)).into_bytes();
        buf.extend_from_slice(&[0xff, 0xfe]);
        buf.push(0);
        assert_eq!(parse_tree(&buf, "")[0].name, "bad-\u{fffd}\u{fffd}");
    }

    fn err(r: Result<impl std::fmt::Debug, ExecError>) -> String {
        r.unwrap_err().to_string()
    }

    #[test]
    fn 요청_검증은_rev_다음_경로_순이고_모양이_틀린_요청도_ts_와_같은_문구다() {
        let rev = "AB".repeat(20);
        let ok = TreeRequest::parse(Some(&json!({ "rev": rev, "dir": "/src/lib/" }))).unwrap();
        assert_eq!((ok.rev.as_str(), ok.dir.as_str()), (rev.as_str(), "src/lib"));
        assert_eq!(ok.key("r"), format!("r\0{}\0src/lib", "ab".repeat(20)));
        let blob = BlobRequest::parse(Some(&json!({ "rev": rev, "path": "/a/b.txt/" }))).unwrap();
        assert_eq!(blob.key("r"), format!("r\0{}\0a/b.txt", "ab".repeat(20)));
        let cases = [
            (json!({ "rev": "zzzz", "dir": "..", "path": ".." }), "invalid commit hash"),
            (json!({ "rev": 5, "dir": "", "path": "a" }), "invalid commit hash"),
            (json!({ "dir": "", "path": "a" }), "invalid commit hash"),
            (json!({ "rev": rev }), "invalid path"),
            (json!({ "rev": rev, "dir": null, "path": null }), "invalid path"),
            (json!({ "rev": rev, "dir": "a/../b", "path": "a/../b" }), "invalid path"),
            (json!({ "rev": rev, "dir": 5, "path": 5 }), "invalid path"),
            (json!("src"), "invalid commit hash"),
            (json!([]), "invalid commit hash"),
            (json!(5), "invalid commit hash"),
            (json!(null), "invalid request"),
        ];
        for (req, msg) in &cases {
            assert_eq!(err(TreeRequest::parse(Some(req))), *msg, "tree {req}");
            assert_eq!(err(BlobRequest::parse(Some(req))), *msg, "blob {req}");
        }
        // 파일 경로는 비어 있으면 안 되지만 디렉터리는 최상위(빈 값)가 된다
        assert_eq!(TreeRequest::parse(Some(&json!({ "rev": rev, "dir": "" }))).unwrap().dir, "");
        assert_eq!(err(BlobRequest::parse(Some(&json!({ "rev": rev, "path": "/" })))), "invalid path");
        assert_eq!(err(TreeRequest::parse(None)), "invalid request");
        assert_eq!(err(BlobRequest::parse(None)), "invalid request");
    }

    #[test]
    fn ref_와_rev_인자() {
        assert_eq!(parse_ref(Some(&json!("refs/heads/main"))).unwrap(), "refs/heads/main");
        for bad in [json!("--output=x"), json!("main"), json!(""), json!(5), json!(null)] {
            assert_eq!(err(parse_ref(Some(&bad))), "invalid ref", "{bad}");
        }
        assert_eq!(err(parse_ref(None)), "invalid ref");
        assert!(parse_rev(Some(&json!("a".repeat(64)))).is_ok());
        for bad in [json!("abcd"), json!(5), json!(null), json!("--all")] {
            assert_eq!(err(parse_rev(Some(&bad))), "invalid commit hash", "{bad}");
        }
        assert_eq!(err(parse_rev(None)), "invalid commit hash");
    }

    #[test]
    fn argv_는_ts_와_바이트까지_같다() {
        assert_eq!(resolve_rev_args("HEAD"), ["rev-parse", "--verify", "--end-of-options", "HEAD^{commit}"]);
        assert_eq!(cat_file_args("-t", "abc:src"), ["cat-file", "-t", "abc:src"]);
        assert_eq!(cat_file_args("-s", "abc:-x"), ["cat-file", "-s", "abc:-x"]);
        assert_eq!(cat_file_args("blob", "abc:a b"), ["cat-file", "blob", "abc:a b"]);
        assert_eq!(ls_tree_args("abc:src"), ["ls-tree", "-z", "--end-of-options", "abc:src"]);
        let rev = CommitId::parse(&"AB".repeat(20)).unwrap();
        assert_eq!(on_remote_args(&rev), ["for-each-ref", "--count=1", "--contains", &"ab".repeat(20), "--format=%(refname)", "refs/remotes/"]);
    }

    #[test]
    fn 텍스트_디코드는_깨진_바이트를_대체하고_선두_bom_을_한_번만_벗긴다() {
        assert_eq!(decode_text(b"\xef\xbb\xbfhi"), "hi");
        assert_eq!(decode_text(b"\xef\xbb\xbf\xef\xbb\xbfhi"), "\u{feff}hi");
        assert_eq!(decode_text(b"a\xe9b"), "a\u{fffd}b");
        assert_eq!(decode_text(b""), "");
    }

    #[test]
    fn 캐시_비용은_ts_와_같다() {
        let t = CodeTreeResult { rev: String::new(), dir: String::new(), entries: Vec::new(), missing: false, truncated: None };
        assert_eq!(tree_cost(&t), 200);
        assert_eq!(blob_cost(&json!({ "kind": "binary", "size": 4 })), 1024);
        // text 는 UTF-16 길이 × 2(비 BMP 문자는 2 단위), dataUrl 은 길이 그대로
        assert_eq!(blob_cost(&json!({ "text": "a😀" })), 3 * 2 + 1024);
        assert_eq!(blob_cost(&json!({ "dataUrl": "data:image/png;base64,AAAA" })), 26 + 1024);
    }

    #[test]
    fn 트리_응답은_truncated_가_없으면_키를_내지_않는다() {
        let mut t = CodeTreeResult { rev: "r".into(), dir: "d".into(), entries: Vec::new(), missing: true, truncated: None };
        assert_eq!(serde_json::to_value(&t).unwrap(), json!({ "rev": "r", "dir": "d", "entries": [], "missing": true }));
        t.truncated = Some(true);
        assert_eq!(serde_json::to_value(&t).unwrap()["truncated"], json!(true));
    }
}
