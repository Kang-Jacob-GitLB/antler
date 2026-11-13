//! `git:commit` 채널의 git 호출(`src/main/git/commands.ts` 의 `commitDetail`·`changedFiles`·`summarizeChanges`)과
//! 결과 크기 추정(`RepoService.ts` 의 `estimateCommitBytes`).
//!
//! 보안: 실행기는 호출자 인자를 전역 옵션 바로 뒤에 붙이므로 renderer 값이 인자 앞자리·옵션 자리에 오면
//! `-c alias.x=!cmd`·`--output=<경로>` 가 된다. 그래서 renderer 가 준 해시는 [`HexHash`] 로만 들어오고(16진수 4~64자뿐이라 `-` 로
//! 시작할 수 없다), argv 를 만드는 함수([`meta_args`]·[`diff_tree_args`])는 서브커맨드·옵션을 상수로 두고 `HexHash` 를 정해진
//! 자리에만 넣는다. 인자 목록을 만드는 곳은 이 파일뿐이다.

use serde::Serialize;

use crate::git::exec::{ExecError, GitExecutor, RunOpts, RunResult};
use crate::git::parsers::changes::{parse_changes, FileChange};
use crate::git::parsers::commit::{parse_commit_meta, CommitMeta, COMMIT_FORMAT_ARGS};
use crate::git::parsers::{js_trim, JsNumber};

/// 변경 파일 목록 상한(`COMMIT_FILES_LIMIT`)
pub const COMMIT_FILES_LIMIT: usize = 5000;

/// 검증된 커밋 해시(16진수 4~64자). TS `assertHex` 의 `/^[0-9a-f]{4,64}$/i` 와 같다.
/// 정규식 대신 손으로 검사한다 — JS 의 `$` 는 끝에서만 맞고(`\n` 앞에서 맞지 않는다) Rust regex 와 문자 부류 규칙도 달라서 옮기지 않는다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HexHash(String);

impl HexHash {
    pub fn parse(value: &str) -> Result<Self, ExecError> {
        let n = value.len();
        if (4..=64).contains(&n) && value.bytes().all(|b| b.is_ascii_hexdigit()) {
            Ok(HexHash(value.to_string()))
        } else {
            Err(other("invalid commit hash"))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// 캐시·합치기 키에 쓰는 소문자 형태(`hash.toLowerCase()`)
    pub fn lower(&self) -> String {
        self.0.to_ascii_lowercase()
    }
}

fn other(message: &str) -> ExecError {
    ExecError::Other { message: message.to_string() }
}

/// 메타데이터를 읽는 `git log -1 …` 인자. 해시는 16진수뿐이라 옵션으로 해석될 수 없다(TS 와 같게 뒤에 `--`)
pub fn meta_args(hash: &HexHash) -> Vec<String> {
    let mut v: Vec<String> = vec!["log".into(), "-1".into()];
    v.extend(COMMIT_FORMAT_ARGS.iter().map(|s| (*s).to_string()));
    v.push(hash.as_str().to_string());
    v.push("--".into());
    v
}

/// 변경 파일을 읽는 `git diff-tree …` 인자. `range` 는 `[부모, 해시]` 또는 `["--root", 해시]` 이고 `mode` 는 상수다
pub fn diff_tree_args(rename: bool, mode: DiffMode, range: &Range) -> Vec<String> {
    let mut v: Vec<String> = vec!["diff-tree".into(), "-r".into(), "-z".into()];
    if rename {
        v.push("-M".into());
    }
    v.extend(["--no-commit-id".into(), "--no-ext-diff".into(), mode.flag().into()]);
    match range {
        Range::Root(h) => v.extend(["--root".into(), h.as_str().to_string()]),
        Range::From(parent, h) => v.extend([parent.as_str().to_string(), h.as_str().to_string()]),
    }
    v.push("--".into());
    v
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffMode {
    NameStatus,
    Numstat,
}

impl DiffMode {
    fn flag(self) -> &'static str {
        match self {
            DiffMode::NameStatus => "--name-status",
            DiffMode::Numstat => "--numstat",
        }
    }
}

/// diff-tree 의 비교 범위. 값은 모두 검증된 해시다
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Range {
    Root(HexHash),
    From(HexHash, HexHash),
}

/// `ok`: 종료 코드가 0 이 아니면 `GitError`(메시지는 stderr, 비면 `git <서브커맨드> failed (<코드>)`)
fn ok(r: RunResult, args: &[String]) -> Result<Vec<u8>, ExecError> {
    if r.code != 0 {
        let trimmed = js_trim(&r.stderr);
        let message = if trimmed.is_empty() { format!("git {} failed ({})", args[0], r.code) } else { trimmed.to_string() };
        return Err(ExecError::Git { message, code: r.code, stderr: r.stderr, args: args.to_vec() });
    }
    Ok(r.stdout)
}

fn fail(r: RunResult, mode: DiffMode) -> ExecError {
    let flag = mode.flag();
    let trimmed = js_trim(&r.stderr);
    let message = if trimmed.is_empty() { format!("git diff-tree {flag} failed ({})", r.code) } else { trimmed.to_string() };
    ExecError::Git { message, code: r.code, stderr: r.stderr, args: vec!["diff-tree".into(), flag.into()] }
}

/// 커밋 상세. 첫 부모를 알아야 하므로 메타데이터를 먼저 읽고, 이어서 name-status 와 numstat 을 동시에 돌린다.
/// 루트 커밋은 `--root`(빈 트리 대비), merge 는 첫 부모 대비다. 취소는 호출한 future 를 버려서 한다(프로세스 트리는 실행기가 정리한다).
pub async fn commit_detail(exec: &GitExecutor, repo_path: &str, hash: &HexHash) -> Result<CommitDetail, ExecError> {
    let opts = RunOpts::default();
    let args = meta_args(hash);
    let meta_out = ok(exec.run(repo_path, &args, &opts).await?, &args)?;
    let meta = parse_commit_meta(&meta_out).map_err(|m| ExecError::Other { message: m })?;
    let base_parent = meta.parents.first().cloned();

    // git 이 돌려준 해시이므로 16진수여야 한다 — 아니면 출력이 이상한 것이다(인자 자리에 그대로 쓰지 않는다)
    let bad_output = || other("unexpected commit metadata output");
    let this = HexHash::parse(&meta.hash).map_err(|_| bad_output())?;
    let range = match &base_parent {
        Some(p) => Range::From(HexHash::parse(p).map_err(|_| bad_output())?, this),
        None => Range::Root(this),
    };

    let ns_args = diff_tree_args(true, DiffMode::NameStatus, &range);
    let num_args = diff_tree_args(true, DiffMode::Numstat, &range);
    let (ns, num) = tokio::join!(exec.run(repo_path, &ns_args, &opts), exec.run(repo_path, &num_args, &opts));
    let (ns, num) = (ns?, num?);
    if ns.code != 0 {
        return Err(fail(ns, DiffMode::NameStatus));
    }
    if num.code != 0 {
        return Err(fail(num, DiffMode::Numstat));
    }
    let all = parse_changes(&ns.stdout, &num.stdout);
    let (files, additions, deletions, truncated) = summarize(all);
    Ok(CommitDetail { meta, base_parent, files, additions, deletions, truncated })
}

/// 변경 파일 목록에 +/- 합계를 매기고 상한에서 자른다. 합계는 잘리기 전 전체 기준이다(바이너리는 null 이라 빠진다)
fn summarize(all: Vec<FileChange>) -> (Vec<FileChange>, JsNumber, JsNumber, bool) {
    let (mut add, mut del) = (0.0_f64, 0.0_f64);
    for f in &all {
        add += f.additions.map_or(0.0, |n| n.0);
        del += f.deletions.map_or(0.0, |n| n.0);
    }
    let truncated = all.len() > COMMIT_FILES_LIMIT;
    let files = if truncated { all.into_iter().take(COMMIT_FILES_LIMIT).collect() } else { all };
    (files, JsNumber(add), JsNumber(del), truncated)
}

/// 커밋 메타데이터 + 첫 부모 대비 변경 파일(`CommitDetail`)
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitDetail {
    #[serde(flatten)]
    pub meta: CommitMeta,
    /// 루트 커밋이면 null
    pub base_parent: Option<String>,
    pub files: Vec<FileChange>,
    pub additions: JsNumber,
    pub deletions: JsNumber,
    pub truncated: bool,
}

/// JS `string.length`(UTF-16 코드 단위 수)
pub fn utf16_len(s: &str) -> usize {
    s.encode_utf16().count()
}

/// 커밋 상세가 차지하는 메모리 추정치(바이트). `estimateCommitBytes` 와 같은 식이다(길이는 UTF-16 단위)
pub fn estimate_commit_bytes(d: &CommitDetail) -> u64 {
    let m = &d.meta;
    let mut n = 512 + (utf16_len(&m.subject) + utf16_len(&m.body) + utf16_len(&m.author_name) + utf16_len(&m.committer_name)) as u64 * 2;
    for f in &d.files {
        n += 96 + (utf16_len(&f.path) + f.old_path.as_deref().map_or(0, utf16_len)) as u64 * 2;
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(s: &str) -> HexHash {
        HexHash::parse(s).unwrap()
    }

    #[test]
    fn 해시는_16진수_4_64자만_받는다() {
        for ok in ["abcd", "ABCD", "0123456789abcdefABCDEF", &"a".repeat(64), &"0".repeat(40)] {
            assert!(HexHash::parse(ok).is_ok(), "{ok}");
        }
        let long = "a".repeat(65);
        for bad in [
            "", "abc", "zzzz", &long, "-x", "-abc", "--output=pwned.txt", "-calias.x=!echo", "HEAD", "abcd\n", " abcd", "abcd ", "ab cd", "abc\u{0}",
            "abcd\u{feff}", "ａｂｃｄ", "abcd/..", "abcd:", "abcd^", "abcd~1", "abcd--",
        ] {
            assert!(HexHash::parse(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn 거부는_code_error_다() {
        assert_eq!(HexHash::parse("zzzz").unwrap_err().envelope_code(), "ERROR");
    }

    #[test]
    fn 메타_인자는_상수_뒤에_해시와_앞뒤_구분자만_온다() {
        let v = meta_args(&h("DEADbeef"));
        assert_eq!(&v[..2], ["log", "-1"]);
        assert_eq!(&v[2..5], COMMIT_FORMAT_ARGS);
        assert_eq!(&v[5..], ["DEADbeef", "--"]);
    }

    #[test]
    fn diff_tree_인자는_상수_뒤에_범위만_온다() {
        let a = h("1111");
        let b = h("2222");
        let v = diff_tree_args(true, DiffMode::NameStatus, &Range::From(a.clone(), b.clone()));
        assert_eq!(v, ["diff-tree", "-r", "-z", "-M", "--no-commit-id", "--no-ext-diff", "--name-status", "1111", "2222", "--"]);
        let v = diff_tree_args(true, DiffMode::Numstat, &Range::Root(b));
        assert_eq!(v, ["diff-tree", "-r", "-z", "-M", "--no-commit-id", "--no-ext-diff", "--numstat", "--root", "2222", "--"]);
    }

    #[test]
    fn 어떤_해시를_넣어도_16진수가_아닌_조각은_상수뿐이다() {
        // 해시 자리에 들어간 값 말고는 항상 같은 상수가 앞에 온다 — renderer 값이 첫 자리·옵션 자리에 올 수 없다
        for s in ["abcd", "FFFF", "0123456789abcdef0123456789abcdef01234567"] {
            let v = meta_args(&h(s));
            assert_eq!(v[0], "log");
            assert!(v.iter().filter(|a| a.as_str() == s).count() == 1);
            assert!(v.iter().filter(|a| a.starts_with('-')).all(|a| a == "-1" || a == "-z" || a == "--no-notes" || a.starts_with("--format=") || a == "--"));
        }
    }

    #[test]
    fn 크기_추정은_utf16_길이를_쓴다() {
        // 이모지는 UTF-16 으로 2 단위(Rust 의 char 수 1, 바이트 4 와 모두 다르다)
        assert_eq!(utf16_len("a😀"), 3);
        assert_eq!(utf16_len("한글"), 2);
    }
}
