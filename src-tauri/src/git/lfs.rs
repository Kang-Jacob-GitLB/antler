//! Git LFS·Gitflow 표시용 순수 함수(`src/shared/lfs.ts`·`src/shared/gitflow.ts` 중 main 이 쓰는 것)와, 그것을 쓰는 두 채널
//! `git:repoExtras`·`git:lfsPaths` 의 git 호출(`src/main/git/repoExtras.ts`). LFS 바이너리나 저장소 설정의 명령은 실행하지 않는다 —
//! 속성은 `check-attr`·텍스트로만, gitflow 는 `config --local` 값으로만 읽는다.
//!
//! 길이 한계는 JS 문자열 길이(UTF-16 코드 단위)로 센다. `\s`·`trim` 은 JS 의 공백 기준이다(`is_js_space`·fetch 의 `js_trim`).
//!
//! 보안(인자 주입 경계): renderer 가 정하는 값은 `lfsPaths` 의 `rev`(`HexHash` — 16진수 4~64자라 옵션이 될 수 없다)와 `paths`(전부 `--` 뒤,
//! NUL·빈 문자열 거부, 앞 1000개)뿐이다. `repoExtras` 는 인자가 저장소 id 뿐이고 git 인자는 상수와 **git 이 돌려준 경로**(HEAD 트리의
//! `.gitattributes` 이름은 `HEAD:` 접두가 붙은 한 인자, `info/attributes` 는 `rev-parse` 가 준 절대 경로)다.
//!
//! **필터 실행 차단(#121)**: 작업 트리 `.gitattributes`·`info/attributes` 는 `git diff --no-index` 로 읽는다. `--no-ext-diff`·`--no-textconv` 는 외부 diff·textconv 만 끈다.
//! **`--no-index` 도 파일에 맞는 속성 규칙이 있으면 `filter.<이름>.clean|process` 를 실행한다**(`.gitattributes filter=x` 규칙과 `filter.x.clean` 이 저장소 설정에 있으면
//! `.gitattributes` 자체를 읽는 순간 실행된다). 그래서 이 읽기는 [`read_worktree_attributes`] 한 곳뿐이고, 저장소당 한 번 `git config -z --get-regexp ^filter\.` 로 조회한
//! [`FilterGuard`] 의 `-c filter.<이름>.clean= … required=false` 를 전역 옵션 자리에 붙여 돌린다(TS `filterOverrides`). 무력화할 수 없으면(덮을 수 없는 이름·조회 실패)
//! 그 파일은 읽지 않고 `lfs:false` 다 — 가드 없이 `--no-index` argv 를 만들 수 없도록 [`no_index_args`] 가 `&FilterGuard` 를 받는다.

use std::future::Future;

use futures_util::future::{join_all, FutureExt};
use serde::Serialize;
use serde_json::Value;

use crate::git::code_browse::{cat_file_args, field};
use crate::git::commit::{utf16_len, HexHash};
use crate::git::exec::{ExecError, GitExecutor, RunOpts};
use crate::git::fetch::{js_to_lowercase, js_trim};
use crate::git::scope::is_js_space;
use crate::git::wip::{filter_overrides, FilterGuard};

/// LFS 포인터 파일 첫 줄
pub const LFS_POINTER_VERSION: &str = "https://git-lfs.github.com/spec/v1";
/// 포인터 파일은 100바이트 안팎이다. 이보다 길면 포인터가 아니다(UTF-16 코드 단위)
const POINTER_MAX_CHARS: usize = 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LfsPointer {
    /// 해시 알고리즘(현재 sha256 만 허용)
    pub algo: &'static str,
    /// 소문자 hex 64자
    pub oid: String,
    /// 실제 객체 크기(바이트)
    pub size: u64,
}

/// `^sha256:([0-9a-f]{64})$` 의 64자 hex
fn sha256_oid(value: &str) -> Option<&str> {
    let hex = value.strip_prefix("sha256:")?;
    (hex.len() == 64 && hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))).then_some(hex)
}

/// LFS 포인터 텍스트를 읽는다. 형식: `version <url>` 로 시작하고 `key value` 줄이 이어진다.
/// 형식이 조금이라도 어긋나면(필수 키 누락·oid 형식·크기가 정수가 아님·너무 긴 텍스트) None 이다.
pub fn parse_lfs_pointer(text: &str) -> Option<LfsPointer> {
    if text.encode_utf16().count() > POINTER_MAX_CHARS {
        return None;
    }
    // `split(/\r?\n/)` — 단독 `\r` 은 구분자가 아니다. `\r` 은 `\n` 이 뒤따르는 조각에서만 벗긴다(마지막 조각의 `\r` 은 남는다)
    let pieces: Vec<&str> = text.split('\n').collect();
    let last = pieces.len() - 1;
    let mut lines: Vec<&str> = pieces.iter().enumerate().map(|(i, l)| if i < last { l.strip_suffix('\r').unwrap_or(l) } else { l }).collect();
    if lines.last() == Some(&"") {
        lines.pop();
    }
    if lines.len() < 3 || lines[0].strip_prefix("version ") != Some(LFS_POINTER_VERSION) {
        return None;
    }
    let (mut oid, mut size): (Option<&str>, Option<u64>) = (None, None);
    for line in &lines[1..] {
        let sp = line.find(' ')?;
        if sp == 0 {
            return None;
        }
        let (key, value) = (&line[..sp], &line[sp + 1..]);
        if key == "oid" {
            let o = sha256_oid(value)?;
            if oid.is_some() {
                return None;
            }
            oid = Some(o);
        } else if key == "size" {
            // `^\d{1,15}$` — JS `\d` 는 ASCII 숫자뿐이다
            if value.is_empty() || value.len() > 15 || !value.bytes().all(|b| b.is_ascii_digit()) || size.is_some() {
                return None;
            }
            size = value.parse().ok();
        }
    }
    Some(LfsPointer { algo: "sha256", oid: oid?.to_string(), size: size? })
}

/// `git check-attr -z filter -- <paths>` 출력(`path\0attr\0value\0` 반복)에서 filter=lfs 인 경로를 모은다.
/// 경로에 개행이 있어도 NUL 구분이라 안전하다.
pub fn parse_check_attr_lfs(out: &str) -> Vec<String> {
    let parts: Vec<&str> = out.split('\0').collect();
    let mut paths = Vec::new();
    let mut i = 0;
    while i + 2 < parts.len() {
        if parts[i + 1] == "filter" && parts[i + 2] == "lfs" {
            paths.push(parts[i].to_string());
        }
        i += 3;
    }
    paths
}

/// `diff --no-index /dev/null <file>` 출력에서 추가된 줄(파일 내용)만 이어 붙인다. 머리(+++ …)는 건너뛴다
pub fn added_lines(patch: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    let mut in_hunk = false;
    for line in patch.split('\n') {
        if line.starts_with("@@") {
            in_hunk = true;
        } else if in_hunk {
            if let Some(rest) = line.strip_prefix('+') {
                out.push(rest);
            }
        }
    }
    out.join("\n")
}

/// `(^|\s)filter=lfs(\s|$)` — 줄 어디에서든 속성 토큰 하나가 정확히 `filter=lfs` 인가
fn has_filter_lfs_token(line: &str) -> bool {
    const NEEDLE: &str = "filter=lfs";
    line.match_indices(NEEDLE).any(|(at, _)| {
        let before_ok = line[..at].chars().next_back().is_none_or(is_js_space);
        let after_ok = line[at + NEEDLE.len()..].chars().next().is_none_or(is_js_space);
        before_ok && after_ok
    })
}

/// .gitattributes 텍스트가 LFS 를 쓰는지(`filter=lfs` 속성이 있는 규칙). 주석 줄은 무시한다
pub fn gitattributes_uses_lfs(text: &str) -> bool {
    text.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l)).any(|raw| {
        let line = js_trim(raw);
        !line.is_empty() && !line.starts_with('#') && has_filter_lfs_token(line)
    })
}

// ── Gitflow ────────────────────────────────────────────────────────────────────

pub const GITFLOW_PREFIX_KINDS: [&str; 5] = ["feature", "release", "hotfix", "bugfix", "support"];
/// 설정값 길이 상한(UTF-16 코드 단위). 브랜치 이름·접두사로 이보다 긴 값은 정상적이지 않다
pub const GITFLOW_VALUE_MAX: usize = 100;

/// `[\u0000-\u001f\u007f-\u009f  ]`
fn is_gitflow_control(c: char) -> bool {
    matches!(c, '\u{0}'..='\u{1f}' | '\u{7f}'..='\u{9f}' | '\u{2028}' | '\u{2029}')
}

/// 표시해도 되는 값만 통과시킨다: 제어문자·줄바꿈이 있거나 너무 길거나 비어 있으면 None
fn clean_value(v: &str) -> Option<&str> {
    (!v.is_empty() && v.encode_utf16().count() <= GITFLOW_VALUE_MAX && !v.chars().any(is_gitflow_control) && !js_trim(v).is_empty()).then_some(v)
}

/// 접두사 맵: JSON 에서는 TS 의 `Partial<Record<kind, string>>`(키는 설정에 나온 것만)
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct GitflowPrefixes {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub feature: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub release: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hotfix: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bugfix: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub support: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct GitflowConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub master: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub develop: Option<String>,
    pub prefixes: GitflowPrefixes,
}

/// `git config --null --get-regexp ^gitflow\.` 출력(`key\nvalue\0` 반복)을 읽는다.
/// 같은 키가 여러 번 나오면 마지막 값이 이긴다. 알려진 키만 받고, 값이 안전하지 않으면 그 키를 버린다.
/// 쓸 만한 키가 하나도 없으면 None(설정 없음).
pub fn parse_gitflow_config(raw: &str) -> Option<GitflowConfig> {
    let mut cfg = GitflowConfig::default();
    let mut any = false;
    for rec in raw.split('\0') {
        let Some(nl) = rec.find('\n') else { continue };
        let key = js_to_lowercase(&rec[..nl]);
        let Some(value) = clean_value(&rec[nl + 1..]) else { continue };
        let value = Some(value.to_string());
        if key == "gitflow.branch.master" {
            cfg.master = value;
        } else if key == "gitflow.branch.develop" {
            cfg.develop = value;
        } else if let Some(kind) = key.strip_prefix("gitflow.prefix.") {
            let slot = match kind {
                "feature" => &mut cfg.prefixes.feature,
                "release" => &mut cfg.prefixes.release,
                "hotfix" => &mut cfg.prefixes.hotfix,
                "bugfix" => &mut cfg.prefixes.bugfix,
                "support" => &mut cfg.prefixes.support,
                _ => continue,
            };
            *slot = value;
        } else {
            continue;
        }
        any = true;
    }
    any.then_some(cfg)
}

// ── git 호출: git:repoExtras · git:lfsPaths ────────────────────────────────────

/// 한 번에 물어볼 수 있는 경로 수 상한(넘는 경로는 표시하지 않는다 — 거부하지 않고 앞에서 자른다)
pub const LFS_PATHS_MAX: usize = 1000;
/// check-attr 한 번에 넘기는 인자 길이 합계(UTF-16 코드 단위 — 명령줄 길이 제한 안쪽)
const CHUNK_CHARS: usize = 6000;
/// 속성 파일 하나를 읽는 상한. 이보다 크면 LFS 규칙을 찾지 않는다
const ATTRIBUTES_MAX_BYTES: usize = 256 * 1024;
/// HEAD 트리에서 살펴보는 .gitattributes 파일 수 상한
const ATTRIBUTES_MAX_FILES: usize = 20;
/// HEAD 트리 목록 출력 상한
const TREE_LIST_MAX_BYTES: usize = 16 * 1024 * 1024;
const CONFIG_MAX_BYTES: usize = 256 * 1024;

/// `git:repoExtras` 의 응답(`RepoExtras`)
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RepoExtras {
    pub lfs: bool,
    pub gitflow: Option<GitflowConfig>,
}

fn other(message: &str) -> ExecError {
    ExecError::Other { message: message.to_string() }
}

/// 이 호출만의 stdout 상한을 얹은 옵션(취소 토큰은 그대로 이어진다)
fn capped(opts: &RunOpts, max: usize) -> RunOpts {
    RunOpts { max_buffer: Some(max), ..opts.clone() }
}

fn strings(args: &[&str]) -> Vec<String> {
    args.iter().map(|s| (*s).to_string()).collect()
}

/// `ls-tree -r -z --name-only --full-tree HEAD`
pub fn ls_tree_names_args() -> Vec<String> {
    strings(&["ls-tree", "-r", "-z", "--name-only", "--full-tree", "HEAD"])
}

/// `rev-parse --path-format=absolute --git-path info/attributes`
pub fn info_attributes_args() -> Vec<String> {
    strings(&["rev-parse", "--path-format=absolute", "--git-path", "info/attributes"])
}

/// `<가드 -c 인자…> diff --no-index --no-ext-diff --no-textconv --no-color -- /dev/null <path>` — **TS 와 같은 인자다**.
/// 가드(저장소의 filter 드라이버를 이번 실행에서 무력화하는 `-c`)는 git 전역 옵션 자리(`diff` 앞)에 붙는다. 가드 없이는 만들 수 없다(위 모듈 설명)
pub fn no_index_args(guard: &FilterGuard, path: &str) -> Vec<String> {
    let mut v = guard.args().to_vec();
    v.extend(strings(&["diff", "--no-index", "--no-ext-diff", "--no-textconv", "--no-color", "--", "/dev/null"]));
    v.push(path.to_string());
    v
}

/// `config --local --null --get-regexp ^gitflow\.` — 저장소 설정만 읽어 전역·시스템 설정이 섞이지 않게 한다
pub fn gitflow_args() -> Vec<String> {
    strings(&["config", "--local", "--null", "--get-regexp", "^gitflow\\."])
}

/// `check-attr [--source=<rev>] -z filter -- <경로…>`. rev 는 `HexHash`(16진수뿐)이고 경로는 전부 `--` 뒤다
pub fn check_attr_args(rev: Option<&HexHash>, chunk: &[String]) -> Vec<String> {
    let mut v = vec!["check-attr".to_string()];
    if let Some(r) = rev {
        v.push(format!("--source={}", r.as_str()));
    }
    v.extend(strings(&["-z", "filter", "--"]));
    v.extend(chunk.iter().cloned());
    v
}

/// 저장소 단위 부가 정보. 둘 다 부가 표시라 실패하면 없는 것으로 본다(`lfs` false·`gitflow` null)
pub async fn repo_extras(exec: &GitExecutor, repo_path: &str, opts: &RunOpts) -> RepoExtras {
    let (lfs, gitflow) = tokio::join!(uses_lfs(exec, repo_path, opts), gitflow_config(exec, repo_path, opts));
    RepoExtras { lfs, gitflow }
}

/// 저장소가 LFS 를 쓰는지: HEAD 트리의 모든 .gitattributes, 작업 트리 루트 .gitattributes, .git/info/attributes 중
/// 하나라도 filter=lfs 규칙이 있으면 true. 하나가 읽히지 않아도(오류) 나머지로 판단한다
async fn uses_lfs(exec: &GitExecutor, repo_path: &str, opts: &RunOpts) -> bool {
    // 작업 트리·git-dir 파일을 읽는 `--no-index` 두 호출이 같은 가드를 쓰도록 저장소당 한 번만 조회한다(TS 와 같이 다른 읽기와 동시에 시작).
    // 무력화할 수 없으면(None) 두 호출 모두 읽기를 건너뛴다
    let guard = async { filter_overrides(exec, repo_path, opts).await.ok() }.shared();
    let worktree = async {
        let g = guard.clone().await.ok_or(())?;
        read_worktree_attributes(exec, repo_path, ".gitattributes", &g, opts).await.map_err(|_| ())
    };
    let info = async {
        let g = guard.clone();
        info_attributes_use_lfs(exec, repo_path, g, opts).await
    };
    let (committed, worktree, info) = tokio::join!(committed_attributes_use_lfs(exec, repo_path, opts), worktree, info);
    committed.unwrap_or(false) || worktree.unwrap_or(false) || info.unwrap_or(false)
}

async fn committed_attributes_use_lfs(exec: &GitExecutor, repo_path: &str, opts: &RunOpts) -> Result<bool, ExecError> {
    // 커밋이 없는 저장소는 실패 코드로 끝난다 — LFS 를 쓰지 않는 것과 같다
    let ls = exec.run(repo_path, &ls_tree_names_args(), &capped(opts, TREE_LIST_MAX_BYTES)).await?;
    if ls.code != 0 {
        return Ok(false);
    }
    let listing = String::from_utf8_lossy(&ls.stdout);
    let names: Vec<&str> = listing.split('\0').filter(|n| *n == ".gitattributes" || n.ends_with("/.gitattributes")).take(ATTRIBUTES_MAX_FILES).collect();
    // 내용은 `cat-file blob` 으로만 읽는다(`HEAD:<경로>` 한 인자 — 필터·textconv 가 돌지 않는다). 하나가 실패하면 그 파일만 없는 것으로 본다
    let read_opts = capped(opts, ATTRIBUTES_MAX_BYTES);
    let found = join_all(names.iter().map(|n| {
        let args = cat_file_args("blob", &format!("HEAD:{n}"));
        let read_opts = &read_opts;
        async move { exec.run(repo_path, &args, read_opts).await.is_ok_and(|r| r.code == 0 && gitattributes_uses_lfs(&String::from_utf8_lossy(&r.stdout))) }
    }))
    .await;
    Ok(found.into_iter().any(|b| b))
}

async fn info_attributes_use_lfs(exec: &GitExecutor, repo_path: &str, guard: impl Future<Output = Option<FilterGuard>>, opts: &RunOpts) -> Result<bool, ExecError> {
    let r = exec.run(repo_path, &info_attributes_args(), opts).await?;
    let path = if r.code == 0 {
        let out = String::from_utf8_lossy(&r.stdout);
        let first = out.split('\n').next().unwrap_or("");
        first.strip_suffix('\r').unwrap_or(first).to_string()
    } else {
        String::new()
    };
    if path.is_empty() {
        return Ok(false);
    }
    // 가드를 만들 수 없으면 이 파일도 읽지 않는다(Err → 호출부가 false 로 본다)
    let Some(guard) = guard.await else { return Ok(false) };
    read_worktree_attributes(exec, repo_path, &path, &guard, opts).await
}

/// 작업 트리·git-dir 의 속성 파일이 LFS 규칙을 갖는가. executor 로는 git 만 실행할 수 있어 `git diff --no-index /dev/null <파일>` 의
/// 추가 줄로 읽는다. 파일이 없거나 상한(출력 512KB)을 넘으면 false(오류).
///
/// **`--no-index` 는 저장소의 `filter.*.clean|process` 를 실행할 수 있다**(모듈 설명) — `guard` 로 이번 실행에서 무력화한다. 읽는 곳을 여기 하나로 모았다
async fn read_worktree_attributes(exec: &GitExecutor, repo_path: &str, path: &str, guard: &FilterGuard, opts: &RunOpts) -> Result<bool, ExecError> {
    let r = exec.run(repo_path, &no_index_args(guard, path), &capped(opts, ATTRIBUTES_MAX_BYTES * 2)).await?;
    // --no-index 는 차이가 있으면 종료 코드 1 이다
    if r.code != 1 {
        return Ok(false);
    }
    Ok(gitattributes_uses_lfs(&added_lines(&String::from_utf8_lossy(&r.stdout))))
}

/// `git config --local --null --get-regexp ^gitflow\.` — 값은 표시용 데이터로만 파싱한다. 설정이 없으면(종료 코드 1)·실패하면 None
async fn gitflow_config(exec: &GitExecutor, repo_path: &str, opts: &RunOpts) -> Option<GitflowConfig> {
    let r = exec.run(repo_path, &gitflow_args(), &capped(opts, CONFIG_MAX_BYTES)).await.ok()?;
    if r.code != 0 {
        return None;
    }
    parse_gitflow_config(&String::from_utf8_lossy(&r.stdout))
}

/// 검증을 마친 `git:lfsPaths` 요청(앞 1000개로 자른 경로)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LfsPathsRequest {
    rev: Option<HexHash>,
    paths: Vec<String>,
}

impl LfsPathsRequest {
    /// TS 의 검증 순서: rev(있으면 `HexHash` — `null` 도 있는 것이라 거부) → paths 는 배열 → 앞 1000개의 각 항목은 비어 있지 않은 NUL 없는 문자열.
    /// 요청이 `null`·인자 없음이면 TS 는 TypeError(code ERROR)라 Rust 는 `invalid request`(code ERROR)다. 객체가 아닌 값은 필드가 없는 것과 같다(`invalid paths`)
    pub fn parse(req: Option<&Value>) -> Result<Self, ExecError> {
        let rev = match field(req, "rev")? {
            None => None,
            Some(Value::String(s)) => Some(HexHash::parse(s)?),
            Some(_) => return Err(other("invalid commit hash")),
        };
        let Some(Value::Array(list)) = field(req, "paths")? else { return Err(other("invalid paths")) };
        let mut paths = Vec::with_capacity(list.len().min(LFS_PATHS_MAX));
        for p in list.iter().take(LFS_PATHS_MAX) {
            match p {
                Value::String(s) if !s.is_empty() && !s.contains('\0') => paths.push(s.clone()),
                _ => return Err(other("invalid path")),
            }
        }
        Ok(LfsPathsRequest { rev, paths })
    }

    /// 인자 길이 합계가 6000(UTF-16 단위)을 넘지 않게 묶는다. 한 묶음은 항상 하나 이상이다
    fn chunks(&self) -> Vec<Vec<String>> {
        let mut chunks = Vec::new();
        let mut cur: Vec<String> = Vec::new();
        let mut len = 0usize;
        for p in &self.paths {
            let n = utf16_len(p);
            if !cur.is_empty() && len + n > CHUNK_CHARS {
                chunks.push(std::mem::take(&mut cur));
                len = 0;
            }
            cur.push(p.clone());
            len += n + 1;
        }
        if !cur.is_empty() {
            chunks.push(cur);
        }
        chunks
    }
}

/// `--source` 를 모르는 옛 git 의 오류(`/unknown option|unrecognized option|usage: git check-attr/i` — 패턴이 ASCII 라 ASCII 대소문자만 무시한다)
fn is_old_git_error(stderr: &str) -> bool {
    let s = stderr.to_ascii_lowercase();
    ["unknown option", "unrecognized option", "usage: git check-attr"].iter().any(|p| s.contains(p))
}

/// 주어진 경로 중 `filter=lfs` 속성인 것. `git check-attr` 는 속성 파일만 읽고 필터 명령을 실행하지 않는다.
/// rev 가 있으면 그 커밋 트리의 .gitattributes 기준(`--source`, git 2.40+; 더 옛 git 이면 현재 작업 트리 기준으로 대신한다)
pub async fn lfs_paths(exec: &GitExecutor, repo_path: &str, req: &LfsPathsRequest, opts: &RunOpts) -> Result<Vec<String>, ExecError> {
    let mut use_source = req.rev.is_some();
    let mut found = Vec::new();
    for chunk in req.chunks() {
        let mut r = exec.run(repo_path, &check_attr_args(req.rev.as_ref().filter(|_| use_source), &chunk), opts).await?;
        if r.code != 0 && use_source && is_old_git_error(&r.stderr) {
            // --source 를 모르는 옛 git(그 외 실패는 폴백하지 않고 그대로 던진다). 이후 묶음도 같은 방식으로 처리한다
            use_source = false;
            r = exec.run(repo_path, &check_attr_args(None, &chunk), opts).await?;
        }
        if r.code != 0 {
            let trimmed = js_trim(&r.stderr);
            let message = if trimmed.is_empty() { format!("git check-attr failed ({})", r.code) } else { trimmed.to_string() };
            return Err(ExecError::Git { message, code: r.code, stderr: r.stderr, args: vec!["check-attr".to_string()] });
        }
        found.extend(parse_check_attr_lfs(&String::from_utf8_lossy(&r.stdout)));
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    const OID: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const OID2: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    const URL: &str = LFS_POINTER_VERSION;

    fn pointer(oid: &str, size: u64) -> String {
        format!("version {URL}\noid sha256:{oid}\nsize {size}\n")
    }

    fn rec(k: &str, v: &str) -> String {
        format!("{k}\n{v}\0")
    }

    // ---- tests/lfsGitflow.test.ts 를 같은 입력으로 옮긴 것 ----

    #[test]
    fn 정상_포인터를_읽는다() {
        let p = parse_lfs_pointer(&pointer(OID, 12345)).unwrap();
        assert_eq!((p.algo, p.oid.as_str(), p.size), ("sha256", OID, 12345));
    }

    #[test]
    fn crlf_끝_개행_없음도_읽는다() {
        let p = parse_lfs_pointer(pointer(OID, 7).replace('\n', "\r\n").trim_end()).unwrap();
        assert_eq!(p.size, 7);
    }

    #[test]
    fn 확장_키가_있어도_필수_키만_본다() {
        let p = parse_lfs_pointer(&format!("version {URL}\next-0-foo sha256:{OID2}\noid sha256:{OID}\nsize 3\n")).unwrap();
        assert_eq!(p.oid, OID);
    }

    #[test]
    fn 형식이_어긋나면_none() {
        assert!(parse_lfs_pointer("hello").is_none());
        assert!(parse_lfs_pointer("").is_none());
        assert!(parse_lfs_pointer(&format!("version https://example.com/v2\noid sha256:{OID}\nsize 1\n")).is_none());
        assert!(parse_lfs_pointer(&format!("version {URL}\noid sha256:{OID}\n")).is_none());
        assert!(parse_lfs_pointer(&format!("version {URL}\nsize 1\n")).is_none());
        assert!(parse_lfs_pointer(&pointer("xyz", 1)).is_none());
        assert!(parse_lfs_pointer(&pointer(&OID.to_uppercase(), 1)).is_none());
        assert!(parse_lfs_pointer(&format!("version {URL}\noid md5:{OID}\nsize 1\n")).is_none());
    }

    #[test]
    fn 크기가_정수가_아니거나_비정상이면_none() {
        let long = "9".repeat(40);
        for bad in ["-1", "1.5", "1e3", "abc", "", long.as_str()] {
            assert!(parse_lfs_pointer(&format!("version {URL}\noid sha256:{OID}\nsize {bad}\n")).is_none(), "{bad}");
        }
    }

    #[test]
    fn 중복_키_너무_긴_텍스트_키_없는_줄은_none() {
        assert!(parse_lfs_pointer(&(pointer(OID, 1) + &format!("oid sha256:{OID2}\n"))).is_none());
        assert!(parse_lfs_pointer(&(pointer(OID, 1) + &"x".repeat(2000))).is_none());
        assert!(parse_lfs_pointer(&(pointer(OID, 1) + "nospace\n")).is_none());
    }

    #[test]
    fn check_attr_출력에서_filter_lfs_경로만_모은다() {
        let out = ["a.bin", "filter", "lfs", "b.txt", "filter", "unspecified", "we ird\nname.psd", "filter", "lfs", "c", "filter", "other", ""].join("\0");
        assert_eq!(parse_check_attr_lfs(&out), vec!["a.bin".to_string(), "we ird\nname.psd".to_string()]);
        assert!(parse_check_attr_lfs("").is_empty());
        assert!(parse_check_attr_lfs("a\0filter").is_empty());
    }

    #[test]
    fn gitattributes_에서_filter_lfs_규칙을_찾는다() {
        assert!(gitattributes_uses_lfs("*.psd filter=lfs diff=lfs merge=lfs -text\n"));
        assert!(gitattributes_uses_lfs("*.txt text\r\n*.bin filter=lfs\r\n"));
        assert!(!gitattributes_uses_lfs("# *.psd filter=lfs\n*.txt text\n"));
        assert!(!gitattributes_uses_lfs("*.psd filter=lfsx\n*.a myfilter=lfs\n"));
        assert!(!gitattributes_uses_lfs(""));
    }

    #[test]
    fn added_lines_는_diff_no_index_출력에서_파일_내용_줄만_모은다() {
        let patch = "diff --git a/dev/null b/x\nnew file mode 100644\n--- /dev/null\n+++ b/x\n@@ -0,0 +1,2 @@\n+*.psd filter=lfs\n+# +++ not header\n";
        assert_eq!(added_lines(patch), "*.psd filter=lfs\n# +++ not header");
        assert_eq!(added_lines(""), "");
    }

    #[test]
    fn branch_prefix_값을_읽는다() {
        let raw = rec("gitflow.branch.master", "main") + &rec("gitflow.branch.develop", "dev") + &rec("gitflow.prefix.feature", "feat/") + &rec("gitflow.prefix.versiontag", "v");
        let want = GitflowConfig {
            master: Some("main".into()),
            develop: Some("dev".into()),
            prefixes: GitflowPrefixes { feature: Some("feat/".into()), ..Default::default() },
        };
        assert_eq!(parse_gitflow_config(&raw), Some(want));
    }

    #[test]
    fn 설정이_없으면_none() {
        assert!(parse_gitflow_config("").is_none());
        assert!(parse_gitflow_config(&(rec("gitflow.prefix.versiontag", "v") + &rec("gitflow.unknown", "x"))).is_none());
    }

    #[test]
    fn 같은_키는_마지막_값이_이긴다() {
        let c = parse_gitflow_config(&(rec("gitflow.branch.develop", "a") + &rec("gitflow.branch.develop", "b"))).unwrap();
        assert_eq!(c.develop.as_deref(), Some("b"));
    }

    #[test]
    fn 줄바꿈_제어문자_너무_긴_값은_버린다() {
        let raw = rec("gitflow.branch.master", "ma\nin")
            + &rec("gitflow.branch.develop", "de\u{1b}[31mv")
            + &rec("gitflow.prefix.feature", &"x".repeat(GITFLOW_VALUE_MAX + 1))
            + &rec("gitflow.prefix.release", "rel\u{7}/")
            + &rec("gitflow.prefix.hotfix", "   ")
            + &rec("gitflow.prefix.bugfix", "fix/");
        let want = GitflowConfig { prefixes: GitflowPrefixes { bugfix: Some("fix/".into()), ..Default::default() }, ..Default::default() };
        assert_eq!(parse_gitflow_config(&raw), Some(want));
        assert!(parse_gitflow_config(&rec("gitflow.branch.master", "m\u{2028}x")).is_none());
        let c = parse_gitflow_config(&rec("gitflow.branch.master", "$(touch pwned)")).unwrap();
        assert_eq!(c, GitflowConfig { master: Some("$(touch pwned)".into()), ..Default::default() });
    }

    #[test]
    fn 깨진_레코드와_아주_큰_입력도_던지지_않는다() {
        assert!(parse_gitflow_config("no-newline\0\n\0").is_none());
        assert_eq!(parse_gitflow_config(&rec("gitflow.branch.master", "m").repeat(50_000)).unwrap().master.as_deref(), Some("m"));
    }

    // ---- 이 PR 에서 더한 경계(골든도 같은 입력을 본다) ----

    #[test]
    fn 포인터_길이는_utf16_단위_1024가_경계() {
        let base = pointer(OID, 12345);
        // 한글은 UTF-16 한 단위지만 UTF-8 로는 3바이트다 — 바이트가 아니라 글자 수로 센다
        let pad = |n: usize| format!("{base}ext-0-x {}\n", "가".repeat(n - base.len() - "ext-0-x ".len() - 1));
        assert_eq!(pad(1024).encode_utf16().count(), 1024);
        assert!(parse_lfs_pointer(&pad(1024)).is_some());
        assert!(parse_lfs_pointer(&pad(1025)).is_none());
        // 비 BMP 는 두 단위
        let emoji = format!("{base}ext-0-x {}\n", "😀".repeat(500));
        assert!(parse_lfs_pointer(&emoji).is_none());
    }

    #[test]
    fn 포인터_크기는_15자리까지_ascii_숫자만() {
        let with = |size: &str| parse_lfs_pointer(&format!("version {URL}\noid sha256:{OID}\nsize {size}\n"));
        assert_eq!(with("999999999999999").unwrap().size, 999_999_999_999_999);
        assert_eq!(with("007").unwrap().size, 7);
        assert!(with("1234567890123456").is_none());
        assert!(with("１２").is_none(), "전각 숫자");
        assert!(with("12 ").is_none());
        // 끝의 단독 \r 은 마지막 조각에 남아 값이 어긋난다
        assert!(parse_lfs_pointer(&format!("{}\r", pointer(OID, 1).trim_end())).is_none());
        assert!(parse_lfs_pointer(&format!("\u{feff}{}", pointer(OID, 1))).is_none());
    }

    #[test]
    fn gitattributes_토큰_경계는_js_공백_기준() {
        assert!(gitattributes_uses_lfs("# c\r\n  *.bin\tfilter=lfs\r\n"));
        assert!(gitattributes_uses_lfs("filter=lfs"));
        // JS \s 는 U+0085 를 공백으로 보지 않는다(Rust is_whitespace 는 본다)
        assert!(!gitattributes_uses_lfs("*.a\u{85}filter=lfs\n"));
        assert!(gitattributes_uses_lfs("*.a\u{feff}filter=lfs\u{a0}\n"));
        assert!(gitattributes_uses_lfs("a filter=lfsx filter=lfs"));
    }

    #[test]
    fn gitflow_값의_경계() {
        let one = |v: &str| parse_gitflow_config(&format!("gitflow.branch.master\n{v}\0")).and_then(|c| c.master);
        assert!(one(&"x".repeat(100)).is_some());
        assert!(one(&"x".repeat(101)).is_none());
        assert!(one(&"가".repeat(100)).is_some());
        assert!(one(&"😀".repeat(51)).is_none(), "UTF-16 로 102");
        assert!(one(&"😀".repeat(50)).is_some());
        for bad in ["a\nb", "a\tb", "a\u{7f}", "a\u{9f}", "a\u{2028}", "a\u{2029}", "", " ", "\u{feff}", "\u{a0}\u{3000}", "\u{a0}\u{85}"] {
            assert!(one(bad).is_none(), "{bad:?}");
        }
        assert_eq!(one(" a ").as_deref(), Some(" a "));
        assert_eq!(one("a\u{a0}").as_deref(), Some("a\u{a0}"));
        // 키는 JS toLowerCase 로 소문자화한다
        let c = parse_gitflow_config(&rec("GitFlow.Branch.Master", "M")).unwrap();
        assert_eq!(c.master.as_deref(), Some("M"));
    }

    // ---- git 호출(PR 5): 인자·요청 검증·묶음·옛 git 판정(실제 git 은 channels.rs 의 골든이 돌린다) ----

    fn req(v: serde_json::Value) -> Result<LfsPathsRequest, ExecError> {
        LfsPathsRequest::parse(Some(&v))
    }

    fn msg(r: Result<LfsPathsRequest, ExecError>) -> String {
        r.expect_err("거부돼야 한다").to_string()
    }

    #[test]
    fn argv_는_ts_와_바이트까지_같다() {
        assert_eq!(ls_tree_names_args(), ["ls-tree", "-r", "-z", "--name-only", "--full-tree", "HEAD"]);
        assert_eq!(cat_file_args("blob", "HEAD:sub/.gitattributes"), ["cat-file", "blob", "HEAD:sub/.gitattributes"]);
        assert_eq!(info_attributes_args(), ["rev-parse", "--path-format=absolute", "--git-path", "info/attributes"]);
        assert_eq!(
            no_index_args(&FilterGuard::none(), ".gitattributes"),
            ["diff", "--no-index", "--no-ext-diff", "--no-textconv", "--no-color", "--", "/dev/null", ".gitattributes"]
        );
        // 가드의 `-c` 는 전역 옵션 자리(diff 앞)에 붙는다 — TS `[...guard, 'diff', '--no-index', …]`
        let g = FilterGuard::from_config_output(b"filter.evil.clean\nsh -c x\0").unwrap();
        assert_eq!(
            no_index_args(&g, "a"),
            [
                "-c", "filter.evil.clean=", "-c", "filter.evil.smudge=", "-c", "filter.evil.process=", "-c", "filter.evil.required=false", "diff", "--no-index", "--no-ext-diff",
                "--no-textconv", "--no-color", "--", "/dev/null", "a"
            ]
        );
        // gitflow 는 `--local` 만(전역·시스템 설정이 섞이지 않는다)
        assert_eq!(gitflow_args(), ["config", "--local", "--null", "--get-regexp", "^gitflow\\."]);
        let chunk = vec!["-a.psd".to_string(), "b c".to_string()];
        assert_eq!(check_attr_args(None, &chunk), ["check-attr", "-z", "filter", "--", "-a.psd", "b c"]);
        let rev = HexHash::parse("ABCDEF0123").unwrap();
        assert_eq!(check_attr_args(Some(&rev), &chunk), ["check-attr", "--source=ABCDEF0123", "-z", "filter", "--", "-a.psd", "b c"]);
    }

    #[test]
    fn 요청은_rev_해시_paths_배열_각_항목_순서로_검증한다() {
        assert_eq!(msg(req(json!({ "rev": "--help", "paths": 5 }))), "invalid commit hash");
        assert_eq!(msg(req(json!({ "rev": null, "paths": ["a"] }))), "invalid commit hash");
        assert_eq!(msg(req(json!({ "rev": 5, "paths": ["a"] }))), "invalid commit hash");
        assert_eq!(msg(req(json!({ "rev": "abc", "paths": ["a"] }))), "invalid commit hash");
        for bad in [json!("x"), json!(5), json!([]), json!({}), json!({ "paths": "a" }), json!({ "paths": { "length": 1 } })] {
            assert_eq!(msg(req(bad.clone())), "invalid paths", "{bad}");
        }
        for bad in [json!([""]), json!(["a\u{0}b"]), json!([5]), json!([null]), json!(["a", {}])] {
            assert_eq!(msg(req(json!({ "paths": bad.clone() }))), "invalid path", "{bad}");
        }
        // 요청이 null·없음이면 TS 는 TypeError(ERROR) — 문구만 다르다
        assert_eq!(msg(LfsPathsRequest::parse(None)), "invalid request");
        assert_eq!(msg(LfsPathsRequest::parse(Some(&serde_json::Value::Null))), "invalid request");
        let ok = req(json!({ "rev": "abCD", "paths": ["a b", "한글"] })).unwrap();
        assert_eq!((ok.rev.as_ref().map(HexHash::as_str), ok.paths), (Some("abCD"), vec!["a b".to_string(), "한글".to_string()]));
        assert!(req(json!({ "paths": [] })).unwrap().chunks().is_empty());
    }

    #[test]
    fn 경로는_앞_1000개만_검증하고_본다() {
        let mut paths: Vec<serde_json::Value> = (0..1000).map(|i| json!(format!("f{i}"))).collect();
        paths.push(json!(""));
        paths.push(json!(5));
        let r = req(json!({ "paths": paths })).unwrap();
        assert_eq!(r.paths.len(), LFS_PATHS_MAX);
        assert_eq!(r.paths.last().map(String::as_str), Some("f999"));
        // 1000번째 안의 잘못된 항목은 거부한다
        let mut paths: Vec<serde_json::Value> = (0..999).map(|i| json!(format!("f{i}"))).collect();
        paths.push(json!(""));
        assert_eq!(msg(req(json!({ "paths": paths }))), "invalid path");
    }

    #[test]
    fn 묶음은_utf16_단위_6000_을_넘지_않게_나눈다() {
        let r = |paths: Vec<String>| LfsPathsRequest { rev: None, paths };
        // 항목마다 길이 + 1(구분자)이 쌓인다: 5999 + 1 = 6000 이 쌓인 뒤 다음 항목(길이 ≥ 1)은 `6000 + n > 6000` 이라 새 묶음이다
        let a = "a".repeat(5999);
        let chunks = r(vec![a.clone(), "b".into(), "c".into()]).chunks();
        assert_eq!(chunks, vec![vec![a.clone()], vec!["b".to_string(), "c".to_string()]]);
        // 합이 정확히 6000 이면 같은 묶음이다(`len + n > 6000` 만 나눈다): 길이 3000 + 1 에 이어 길이 2999 → 3001 + 2999 = 6000
        let x = "x".repeat(3000);
        let y = "y".repeat(2999);
        assert_eq!(r(vec![x.clone(), y.clone()]).chunks(), vec![vec![x, y]]);
        // 한 항목이 6000 보다 길어도 혼자 한 묶음이다. 한글은 한 단위·이모지는 두 단위로 센다
        let huge = "가".repeat(7000);
        assert_eq!(r(vec![huge.clone(), "z".into()]).chunks(), vec![vec![huge], vec!["z".to_string()]]);
        let emoji = "😀".repeat(1500); // 3000 단위
        let two = "😀".repeat(1499); // 2998 단위
        assert_eq!(r(vec![emoji.clone(), two.clone()]).chunks(), vec![vec![emoji, two]], "3000 + 1 + 2998 = 5999 ≤ 6000");
        // 1000 개를 모두 순서대로 담는다
        let many: Vec<String> = (0..1000).map(|i| format!("dir/{}{i}", "y".repeat(60))).collect();
        let flat: Vec<String> = r(many.clone()).chunks().into_iter().flatten().collect();
        assert_eq!(flat, many);
    }

    #[test]
    fn 옛_git_판정은_ascii_대소문자를_무시한_세_문구뿐이다() {
        for yes in ["error: unknown option `source'", "ERROR: Unrecognized Option --source", "Usage: git check-attr [-a | --all | <attr>...] [--] <pathname>..."] {
            assert!(is_old_git_error(yes), "{yes}");
        }
        for no in ["fatal: bad tree object abc", "fatal: ../x is outside repository", "", "unknown  option", "unknown optio"] {
            assert!(!is_old_git_error(no), "{no}");
        }
    }
}
