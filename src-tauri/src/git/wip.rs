//! 작업 트리(WIP) 조회 `git:wipStatus`·`git:wipDiff`(`src/main/git/wip.ts`): filter 드라이버 무력화 가드(`FilterGuard`)·요청 검증·git 호출.
//! untracked 허용 목록의 보관(저장소별 마지막 `wipStatus` 가 보고한 경로)은 라우터(`Router::untracked_allow`)가 맡는다.
//!
//! 보안(악성 저장소 방어):
//! - 작업 트리를 읽는 status·diff 는 `.gitattributes` 의 filter 드라이버(clean/process)를 실행한다. 저장소 설정에 심은 명령이면 저장소를 열기만 해도
//!   임의 명령이 돈다. 모든 드라이버(git-lfs 포함 — clean 이 `lfs.extension.*.clean` 을 실행하고 `.git/lfs` 에 쓴다)는 이번 실행에서만 빈 명령으로 덮어 쓴다 — 그 `-c` 인자는 [`FilterGuard`] 만 만든다.
//!   **status·staged·unstaged·untracked(`--no-index` 도 저장소의 설정·속성을 읽어 clean 을 돈다) 어느 영역이든** 가드를 전역 옵션 자리에 붙인다 —
//!   argv 빌더([`wip_status_args`]·[`wip_diff_args`])가 `&FilterGuard` 를 필수 인자로 받아 가드 없이는 만들 수 없다. 무력화할 수 없는 저장소는 작업 트리를 읽지 않고 거부한다.
//!   이 앱에서 `"-c"` 인자를 만드는 곳은 이 파일(저장소가 정한 값이 들어가는 유일한 곳)과 상수만 쓰는 `exec/common.rs`·`fetch/guard.rs` 뿐이다(소스 검사 시험 `channels.rs`).
//! - untracked diff 는 `diff --no-index` 로 파일을 직접 연다. 경로 문자열 검사([`assert_rel_path`])는 Windows 8.3 짧은 이름(`GIT~1/config`)을 `.git` 으로
//!   알아보지 못하므로, untracked 는 **마지막 `wipStatus` 가 git 출력으로 보고한 경로와 정확히 일치**할 때만 받는다(허용 목록, 없으면 거부 — 닫힌 기본값).
//!   unstaged 는 `~숫자` 8.3 모양 구성 요소를 거부하고, 로컬 저장소는 열기 직전에 `link_guard` 로 정션·symlink 를 확인한다.
//! - `--no-index` 는 차이가 있으면 종료 코드 1 이다. 그러나 **출력이 하나도 없는** 종료 코드 1 은 파일을 못 연 것이라 빈 diff 로 가리지 않고 오류로 올린다.

use std::collections::HashSet;
use std::path::Path;

use serde_json::Value;
use tokio::io::AsyncReadExt;

use crate::git::exec::{ExecError, GitExecutor, RunOpts};
use crate::git::fetch::js_to_lowercase;
use crate::git::file_channels::{js_truthy, DIFF_CHUNK, DIFF_FULL_MAX_BYTES, DIFF_FULL_MAX_LINES, DIFF_MAX_BYTES};
use crate::git::link_guard::assert_no_link_escape;
use crate::git::parsers::diff::{DiffParseOpts, DiffParser, FileDiff};
use crate::git::parsers::js_trim;
use crate::git::parsers::wip_status::{WipStatus, WipStatusParser, WIP_STATUS_ARGS};

/// 덮어쓸 수 없는 필터 드라이버가 있어 작업 트리 조회를 거부할 때의 메시지
pub const UNSAFE_FILTER_MSG: &str =
    "이 저장소의 filter 드라이버(이름이 비었거나 \"=\" 나 잘못된 UTF-8 이 있거나 개수가 너무 많음)를 안전하게 무력화할 수 없습니다. 작업 트리 변경은 표시하지 않습니다.";

/// 드라이버 수 상한. 드라이버마다 `-c` 4쌍(약 90자 + 이름 길이 ×4)이 붙는데 Windows 명령줄은 32767자까지라 256개면 이름이 짧아도 한계에 가깝다.
/// 정상 저장소의 드라이버는 한두 개다. 총 길이 상한([`MAX_OVERRIDE_CHARS`])도 함께 둔다.
const MAX_FILTER_DRIVERS: usize = 256;
/// 덮어쓰기 `-c` 인자 전체의 길이 상한(각 인자 UTF-16 단위 수 + 1 의 합 — TS 와 같은 셈법)
const MAX_OVERRIDE_CHARS: usize = 24000;

/// 덮어쓸 수 없는 드라이버가 있다 — 그런 저장소는 작업 트리를 읽지 않는다. 이유는 셋이다:
/// - 이름에 `=`: git 은 `-c` 인자를 첫 `=` 에서 키와 값으로 자른다. `-c filter.a=b.clean=` 은 `filter.a` = `b.clean=` 으로 읽혀 덮어쓰기가 조용히 실패한다
///   (gitattributes 에서 `filter=a=b` 로 쓸 수 있다).
/// - 키가 잘못된 UTF-8(또는 U+FFFD): 문자열로 바꾸면 U+FFFD 가 돼 `-c` 가 실제 드라이버를 덮지 못하고, Windows 는 argv 가 UTF-16 이라 원래 바이트로 `-c` 를 만들 수도 없다.
/// - 드라이버 수·`-c` 총 길이가 상한을 넘는다(명령줄 길이 한계).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnsafeFilterName;

impl UnsafeFilterName {
    pub fn message(self) -> &'static str {
        UNSAFE_FILTER_MSG
    }
}

/// 이번 git 실행에서 덮어쓸 filter 드라이버의 `-c` 인자. 값을 만드는 곳은 [`FilterGuard::from_config_output`] 하나뿐이다(닫힌 타입) —
/// 저장소 설정이 정한 드라이버 이름이 git 전역 옵션 자리에 들어가는 이 앱의 첫 경우라서, 이름 검사와 인자 조립이 갈라지지 않게 한다.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FilterGuard {
    args: Vec<String>,
}

/// `^filter\.(.+)\.(clean|smudge|process)$` (`s` 플래그: 이름에 개행이 있어도 맞는다)의 드라이버 이름
fn driver_name(key: &str) -> Option<&str> {
    let rest = key.strip_prefix("filter.")?;
    let name = [".clean", ".smudge", ".process"].iter().find_map(|s| rest.strip_suffix(s))?;
    Some(name)
}

impl FilterGuard {
    /// 덮어쓸 것이 없는 가드(드라이버가 하나도 없는 저장소의 결과와 같다). **제품 코드의 비어 있지 않은 근거는 [`filter_overrides`] 하나뿐이다** —
    /// 이 함수를 이 파일 밖의 제품 코드에서 부르면 가드를 건너뛰는 것이라 소스 검사 시험이 막는다
    pub fn none() -> Self {
        Self::default()
    }

    /// `git config -z --get-regexp '^filter\.'` 의 stdout(`key\nvalue\0` 반복, **바이트**)에서 가드를 만든다([`filter_driver_names`] 로 이름을 뽑고
    /// `-c` 4쌍으로 펼친 뒤 총 길이 상한을 본다).
    pub fn from_config_output(stdout: &[u8]) -> Result<Self, UnsafeFilterName> {
        let drivers = filter_driver_names(stdout)?;
        let mut args = Vec::with_capacity(drivers.len() * 8);
        for d in drivers {
            // required=true 면 빈 명령을 "필터 없음 → 실패"로 본다. 이번 실행에서만 끈다
            for suffix in ["clean=", "smudge=", "process=", "required=false"] {
                args.push("-c".to_string());
                args.push(format!("filter.{d}.{suffix}"));
            }
        }
        if args.iter().map(|a| a.encode_utf16().count() + 1).sum::<usize>() > MAX_OVERRIDE_CHARS {
            return Err(UnsafeFilterName);
        }
        Ok(FilterGuard { args })
    }

    /// git 전역 옵션 자리에 넣을 인자(`-c`, `filter.<d>.clean=` …)
    pub fn args(&self) -> &[String] {
        &self.args
    }

    pub fn is_empty(&self) -> bool {
        self.args.is_empty()
    }
}

/// [`filter_overrides`] 가 가드를 만들지 못한 이유
#[derive(Debug)]
pub enum FilterGuardError {
    /// 덮어쓸 수 없는 드라이버가 있다
    Unsafe(UnsafeFilterName),
    /// `git config` 조회가 실패했다(코드 1 = 키 없음은 실패가 아니다)
    Git(ExecError),
    /// `git config` 가 0·1 이 아닌 코드로 끝났다(코드, stderr)
    Config(i32, String),
}

impl From<FilterGuardError> for ExecError {
    /// TS `filterOverrides` 가 던지는 `GitError` 와 같다(code GIT): 덮을 수 없는 이름은 [`UNSAFE_FILTER_MSG`] 코드 1, `git config` 실패는 stderr(없으면 `git config exited with <코드>`)
    fn from(e: FilterGuardError) -> Self {
        let args = vec!["config".to_string()];
        match e {
            FilterGuardError::Unsafe(u) => ExecError::Git { message: u.message().to_string(), code: 1, stderr: u.message().to_string(), args },
            FilterGuardError::Git(e) => e,
            FilterGuardError::Config(code, stderr) => {
                let message = match js_trim(&stderr) {
                    "" => format!("git config exited with {code}"),
                    t => t.to_string(),
                };
                ExecError::Git { message, code, stderr, args }
            }
        }
    }
}

/// `git -C <repo> config -z --get-regexp ^filter\.` 로 저장소의 filter 드라이버를 조회해 가드를 만든다(TS `filterOverrides`, argv 가 같다).
/// 종료 코드 1(해당 키 없음)은 빈 가드다. 그 밖의 실패·덮을 수 없는 이름은 오류 — 호출부는 **해당 작업 트리 읽기를 하지 않는다**.
pub async fn filter_overrides(exec: &GitExecutor, repo_path: &str, opts: &RunOpts) -> Result<FilterGuard, FilterGuardError> {
    // 정규식은 git 의 로캘·바이트 처리에 기대지 않도록 `^filter\.` 로만 좁히고 접미사는 `filter_driver_names` 가 판정한다
    let args = ["config", "-z", "--get-regexp", "^filter\\."];
    let r = exec.run(repo_path, &args, opts).await.map_err(FilterGuardError::Git)?;
    match r.code {
        1 => Ok(FilterGuard::none()),
        0 => FilterGuard::from_config_output(&r.stdout).map_err(FilterGuardError::Unsafe),
        c => Err(FilterGuardError::Config(c, r.stderr)),
    }
}

/// `git config -z --get-regexp '^filter\.'` 의 stdout 에서 덮어써야 할 드라이버 이름을 처음 나온 순서로 뽑는다(TS `parseFilterDrivers`).
/// 접미사(.clean/.smudge/.process) 판정은 git 정규식(로캘 의존)이 아니라 여기서 한다. 키가 하나라도 잘못된 UTF-8·U+FFFD 면 드라이버 키가 아니어도 거부한다.
/// 값은 보지 않는다 — git-lfs 기본 명령도 예외가 아니다(clean 이 `lfs.extension.*.clean` 을 실행하고 `.git/lfs` 에 쓴다).
pub fn filter_driver_names(stdout: &[u8]) -> Result<Vec<&str>, UnsafeFilterName> {
    let mut drivers: Vec<&str> = Vec::new();
    let mut seen: HashSet<&str> = HashSet::new();
    for entry in stdout.split(|&b| b == 0) {
        if entry.is_empty() {
            continue;
        }
        let key_bytes = match entry.iter().position(|&b| b == b'\n') {
            Some(nl) => &entry[..nl],
            None => entry,
        };
        let key = std::str::from_utf8(key_bytes).map_err(|_| UnsafeFilterName)?;
        if key.contains('\u{fffd}') {
            return Err(UnsafeFilterName);
        }
        let Some(name) = driver_name(key) else { continue };
        // git 은 `-c` 인자를 첫 '=' 에서 키와 값으로 자른다(UnsafeFilterName 설명)
        // 빈 이름(`[filter ""]`)도 드라이버다 — `filter=` 가 가리키면 git 이 그 clean 을 실행한다(실측). 정상 저장소에는 없으니 거부한다
        if name.is_empty() || name.contains('=') {
            return Err(UnsafeFilterName);
        }
        if seen.insert(name) {
            drivers.push(name);
            if drivers.len() > MAX_FILTER_DRIVERS {
                return Err(UnsafeFilterName);
            }
        }
    }
    Ok(drivers)
}

/// renderer 가 보낸 저장소 상대 경로 검증. 절대 경로·상위 경로(..)·.git 안쪽은 거부한다(`invalid path`) —
/// 추적 안 되는 파일 diff 는 `--no-index` 로 파일을 직접 읽으므로 저장소 밖 파일을 읽는 통로가 되면 안 된다.
/// 길이 상한 4096 은 UTF-16 코드 단위다. 이 문자열 검사는 8.3 이름·정션을 알아보지 못한다(위 모듈 설명).
pub fn assert_rel_path(p: &str) -> Result<(), &'static str> {
    const INVALID: &str = "invalid path";
    if p.is_empty() || p.encode_utf16().count() > 4096 || p.contains('\0') {
        return Err(INVALID);
    }
    // `^([\\/]|[a-zA-Z]:)`
    let b = p.as_bytes();
    if matches!(b[0], b'/' | b'\\') || (b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':') {
        return Err(INVALID);
    }
    if p.split(['/', '\\']).any(|seg| seg == ".." || js_to_lowercase(seg) == ".git") {
        return Err(INVALID);
    }
    Ok(())
}

/// 구성 요소가 8.3 짧은 이름 모양(`~숫자` 로 끝남)인가. 정상 파일 이름에도 드물게 있는 형태(`foo~1`)라 오탐을 허용한다 — `.git` 안을 읽는 쪽이 더 나쁘다.
/// NTFS 는 끝의 `.`·공백을 떼므로 그것들을 벗긴 뒤 판정한다(`GIT~1.`·`GIT~1 ` 도 같은 이름).
pub fn has_short_name_segment(path: &str) -> bool {
    path.split(['/', '\\']).any(|seg| {
        let seg = seg.trim_end_matches(['.', ' ']);
        let head = seg.trim_end_matches(|c: char| c.is_ascii_digit());
        head.len() < seg.len() && head.ends_with('~')
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WipArea {
    Staged,
    Unstaged,
    Untracked,
}

impl WipArea {
    /// 허용 목록: `staged`·`unstaged`·`untracked` 만(`invalid area`)
    pub fn parse(s: &str) -> Result<Self, &'static str> {
        match s {
            "staged" => Ok(WipArea::Staged),
            "unstaged" => Ok(WipArea::Unstaged),
            "untracked" => Ok(WipArea::Untracked),
            _ => Err("invalid area"),
        }
    }
}

/// `wipDiff` 요청의 문자열 검증(파일 시스템을 보지 않는 부분). TS `wipFileDiff` 의 앞부분과 같은 순서다:
/// area → `path`·`oldPath` 의 `assert_rel_path` → untracked 면 허용 목록과 **정확히 일치**(목록이 없으면 거부, 대소문자 변형·8.3 이름도 거부) →
/// unstaged 면 `~숫자` 구성 요소 거부. staged 는 위 검사만 거친다.
/// 이 뒤에 로컬 저장소는 `link_guard::assert_no_link_escape`(untracked·unstaged)를 거쳐야 한다 — 호출자(WIP 채널) 몫이다.
pub fn validate_wip_diff_request(area: &str, path: &str, old_path: Option<&str>, untracked_allow: Option<&HashSet<String>>) -> Result<WipArea, &'static str> {
    let area = WipArea::parse(area)?;
    assert_rel_path(path)?;
    if let Some(old) = old_path {
        assert_rel_path(old)?;
    }
    if area == WipArea::Untracked && !untracked_allow.is_some_and(|set| set.contains(path)) {
        return Err("invalid path");
    }
    if area == WipArea::Unstaged && has_short_name_segment(path) {
        return Err("invalid path");
    }
    Ok(area)
}

/// 추적 안 되는 폴더(중첩 저장소, 끝이 `/`·`\`)는 파일이 아니라 내용을 비교할 수 없어 빈 diff 로 답한다
pub fn is_untracked_dir_entry(area: WipArea, path: &str) -> bool {
    area == WipArea::Untracked && path.ends_with(['/', '\\'])
}

/// 검증을 마친 `git:wipDiff` 요청. `path`·`old_path` 는 [`assert_rel_path`] 를 통과했고 untracked 면 허용 목록에 정확히 있다
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WipDiffRequest {
    pub area: WipArea,
    pub path: String,
    pub old_path: Option<String>,
    pub full: bool,
}

impl WipDiffRequest {
    /// TS `wipFileDiff` 의 검증 순서와 문구다: **객체가 아니거나 `area` 가 허용 목록 밖이면 `invalid area`**(요청이 `null`·문자열이어도 같다 — TS 는 `!req || !AREAS.has(req.area)`),
    /// 그다음 `path`(문자열이 아니면 `invalid path`)·`oldPath`(없으면 건너뜀, `null` 포함 문자열이 아니면 `invalid path`)의 경로 검증, untracked 의 허용 목록, unstaged 의 `~숫자` 거부([`validate_wip_diff_request`]).
    /// 전부 `Error`(code ERROR)다. `full` 은 JS 참 거짓이다
    pub fn parse(req: Option<&Value>, untracked_allow: Option<&HashSet<String>>) -> Result<Self, &'static str> {
        let Some(o) = req.and_then(Value::as_object) else { return Err("invalid area") };
        let Some(area) = o.get("area").and_then(Value::as_str) else { return Err("invalid area") };
        // area 가 틀리면 path 를 보지 않는다(TS 와 같은 순서) — 허용 목록 검사를 거치지 않고 area 만 먼저 확인한다
        WipArea::parse(area)?;
        let Some(Value::String(path)) = o.get("path") else { return Err("invalid path") };
        let old_path = match o.get("oldPath") {
            None => None,
            Some(Value::String(s)) => Some(s.as_str()),
            Some(_) => return Err("invalid path"),
        };
        let area = validate_wip_diff_request(area, path, old_path, untracked_allow)?;
        Ok(WipDiffRequest { area, path: path.clone(), old_path: old_path.map(str::to_string), full: js_truthy(o.get("full")) })
    }
}

/// 가드의 `-c` 인자를 **맨 앞**(git 전역 옵션 자리)에 붙인다. 이 파일에서 `FilterGuard::args` 를 읽는 곳은 여기 하나다
fn guarded(guard: &FilterGuard, rest: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut v: Vec<String> = guard.args().to_vec();
    v.extend(rest);
    v
}

/// `git status` 인자: `<가드 -c…> status --porcelain=v2 -z --untracked-files=all --renames --ignore-submodules=all --`. 가드는 필수 인자다
pub fn wip_status_args(guard: &FilterGuard) -> Vec<String> {
    guarded(guard, WIP_STATUS_ARGS.iter().map(|s| (*s).to_string()))
}

/// `git diff` 인자(TS `wipFileDiff` 와 바이트까지 같다). 가드는 필수 인자이고 **모든 영역에** 붙는다(`--no-index` 도 필터를 돈다). 경로(renderer 값)는 항상 `--` 뒤다.
/// staged 는 `--cached -M -- [old] path`, unstaged 는 `--no-renames -- path`, untracked 는 `--no-index -- /dev/null path`(`/dev/null` 은 git 이 모든 OS 에서 직접 해석한다)
pub fn wip_diff_args(guard: &FilterGuard, req: &WipDiffRequest) -> Vec<String> {
    let mut rest: Vec<String> = ["diff", "--no-color", "--no-ext-diff", "--no-textconv", "-U3"].iter().map(|s| (*s).to_string()).collect();
    match req.area {
        WipArea::Staged => {
            rest.extend(["--cached".to_string(), "-M".to_string(), "--".to_string()]);
            if let Some(old) = req.old_path.as_ref().filter(|old| **old != req.path) {
                rest.push(old.clone());
            }
            rest.push(req.path.clone());
        }
        WipArea::Unstaged => rest.extend(["--no-renames".to_string(), "--".to_string(), req.path.clone()]),
        WipArea::Untracked => rest.extend(["--no-index".to_string(), "--".to_string(), "/dev/null".to_string(), req.path.clone()]),
    }
    guarded(guard, rest)
}

/// stdout 을 청크로 읽어 `feed` 에 넘긴다. `feed` 가 `false` 를 주면 스트림을 버려 git 을 끊고(종료 코드를 보지 않는다) 끝낸다. EOF 까지 읽었으면 종료 코드를 확인한다.
/// 읽기 오류는 취소면 `Aborted`, SSH 선읽기 상한(16MB) 초과면 그 설명(`file_diff` 와 같은 알려진 차이 — docs/tauri.md)이다
async fn pump(exec: &GitExecutor, repo_path: &str, args: &[String], opts: &RunOpts, what: &str, mut feed: impl FnMut(&[u8]) -> bool) -> Result<(), ExecError> {
    let mut stream = exec.stream(repo_path, args, opts).await?;
    let mut chunk = vec![0u8; DIFF_CHUNK];
    loop {
        let n = match stream.read(&mut chunk).await {
            Ok(n) => n,
            Err(_) if opts.cancel.as_ref().is_some_and(tokio_util::sync::CancellationToken::is_cancelled) => return Err(ExecError::Aborted),
            Err(e) if crate::git::exec::ssh::is_read_ahead_exceeded(&e) => {
                return Err(ExecError::Other { message: format!("SSH 선읽기 상한(16MB)을 넘어 {what}을 중단했습니다") });
            }
            Err(e) => return Err(ExecError::Other { message: e.to_string() }),
        };
        if n == 0 {
            return stream.finish().await;
        }
        if !feed(&chunk[..n]) {
            drop(stream);
            return Ok(());
        }
    }
}

/// 작업 트리 상태(TS `wipStatus`). 출력은 스트림으로 읽어 추적 안 되는 파일을 `untracked_limit` 까지만 담는다. 저장소에 커밋이 없어도 동작한다.
/// 가드를 못 만들면(덮을 수 없는 드라이버·`git config` 실패) 작업 트리를 읽지 않고 오류다
pub async fn wip_status(exec: &GitExecutor, repo_path: &str, opts: &RunOpts, untracked_limit: usize) -> Result<WipStatus, ExecError> {
    let mut parser = WipStatusParser::new(untracked_limit);
    let guard = filter_overrides(exec, repo_path, opts).await?;
    pump(exec, repo_path, &wip_status_args(&guard), opts, "작업 트리 상태 조회", |c| {
        parser.push(c);
        true
    })
    .await?;
    Ok(parser.finish())
}

/// 작업 트리 파일 하나의 diff(TS `wipFileDiff`). `req` 는 [`WipDiffRequest::parse`] 를 통과한 값이다. 순서가 TS 와 같다(오류 code 가 달라지므로 바꾸지 않는다):
/// 추적 안 되는 폴더(중첩 저장소)는 git 없이 빈 diff → 로컬 실행기면 정션·symlink 확인(`link_guard`: untracked 는 마지막 구성 요소가 링크여도 거부, unstaged 는 추적 중인 링크 파일만 허용; 실패는 `invalid path`, code ERROR) →
/// 필터 가드(실패는 GIT) → diff 스트림. 링크 확인에는 git 의 `-C` 와 같은 문자열을 넘긴다(`\\?\` 접두를 붙이지 않는다 — 비교는 `link_guard` 안에서 canonicalize 한다).
/// 상한은 `file_diff` 와 같다(일반 8MB, full 64MB + 줄 수 상한). untracked 의 종료 코드 1 은 **읽은 바이트가 있을 때만** 정상이다
pub async fn wip_file_diff(exec: &GitExecutor, repo_path: &str, req: &WipDiffRequest, opts: &RunOpts) -> Result<FileDiff, ExecError> {
    let parse_opts = if req.full {
        DiffParseOpts { max_lines: Some(DIFF_FULL_MAX_LINES), max_bytes: Some(DIFF_FULL_MAX_BYTES), ..DiffParseOpts::default() }
    } else {
        DiffParseOpts { max_bytes: Some(DIFF_MAX_BYTES), ..DiffParseOpts::default() }
    };
    let mut parser = DiffParser::new(&req.path, req.old_path.as_deref(), &parse_opts);
    if is_untracked_dir_entry(req.area, &req.path) {
        return Ok(parser.finish());
    }
    if req.area != WipArea::Staged && exec.kind() == "local" {
        let (root, rel, allow_leaf_link) = (repo_path.to_string(), req.path.clone(), req.area == WipArea::Unstaged);
        match tokio::task::spawn_blocking(move || assert_no_link_escape(Path::new(&root), &rel, allow_leaf_link)).await {
            Ok(Ok(())) => {}
            _ => return Err(ExecError::Other { message: "invalid path".to_string() }),
        }
    }
    let guard = filter_overrides(exec, repo_path, opts).await?;
    let mut produced = false;
    let result = pump(exec, repo_path, &wip_diff_args(&guard, req), opts, "diff", |c| {
        produced |= !c.is_empty();
        // 상한을 넘기고 줄 수 세기도 끝났으면 git 을 끊는다
        parser.push(c)
    })
    .await;
    match result {
        Ok(()) => {}
        // `--no-index` 는 차이가 있으면 종료 코드 1 이다 — 오류가 아니다. 출력이 없는 1 은 차이가 아니라 파일을 못 연 것이다
        Err(ExecError::Git { code: 1, .. }) if req.area == WipArea::Untracked && produced => {}
        Err(e) => return Err(e),
    }
    Ok(parser.finish())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn guard(entries: &[(&str, &str)]) -> Result<FilterGuard, UnsafeFilterName> {
        let mut out = Vec::new();
        for (k, v) in entries {
            out.extend_from_slice(format!("{k}\n{v}\0").as_bytes());
        }
        FilterGuard::from_config_output(&out)
    }

    fn driver_args(d: &str) -> Vec<String> {
        ["clean=", "smudge=", "process=", "required=false"].iter().flat_map(|s| ["-c".to_string(), format!("filter.{d}.{s}")]).collect()
    }

    #[test]
    fn git_lfs_기본_명령도_덮어쓴다() {
        // clean 이 lfs.extension.*.clean 을 실행하고 .git/lfs 에 쓰므로 값과 상관없이 전부 무력화한다
        let g = guard(&[
            ("filter.lfs.clean", "git-lfs clean -- %f"),
            ("filter.lfs.smudge", "git-lfs smudge -- %f"),
            ("filter.lfs.process", "git-lfs filter-process"),
            ("filter.lfs2.smudge", "  git-lfs smudge --skip -- %f \n"),
            ("filter.lfs2.process", "git-lfs filter-process --skip"),
        ])
        .unwrap();
        let mut want = driver_args("lfs");
        want.extend(driver_args("lfs2"));
        assert_eq!(g.args(), want);
        assert_eq!(guard(&[("filter.x.clean", "")]).unwrap().args(), driver_args("x"));
    }

    #[test]
    fn 드라이버는_처음_나온_순서로_한_번씩() {
        let g = guard(&[("filter.b.clean", "x"), ("filter.a.smudge", "y"), ("filter.b.process", "z"), ("filter.a.clean", "w")]).unwrap();
        let mut want = driver_args("b");
        want.extend(driver_args("a"));
        assert_eq!(g.args(), want);
    }

    #[test]
    fn 이름의_점과_개행_키_모양() {
        assert_eq!(guard(&[("filter.a.b.clean", "x")]).unwrap().args(), driver_args("a.b"));
        assert_eq!(guard(&[("filter.a.clean.process", "x")]).unwrap().args(), driver_args("a.clean"));
        // 접미사가 다르면 드라이버가 아니다
        assert!(guard(&[("filter.a.required", "x"), ("filter.a", "x"), ("filter.clean", "x"), ("xfilter.a.clean", "x"), ("Filter.a.clean", "x")]).unwrap().is_empty());
        // 값에 개행이 있어도 첫 개행에서 키와 값이 갈린다
        let g = FilterGuard::from_config_output(b"filter.a.clean\nline1\nline2\0").unwrap();
        assert_eq!(g.args(), driver_args("a"));
        // 개행 없는 레코드(값 없음)
        assert_eq!(FilterGuard::from_config_output(b"filter.a.clean\0\0").unwrap().args(), driver_args("a"));
        assert!(FilterGuard::from_config_output(b"").unwrap().is_empty());
    }

    #[test]
    fn 이름에_등호가_있으면_거부한다() {
        assert_eq!(guard(&[("filter.a=b.clean", "x")]), Err(UnsafeFilterName));
        // git-lfs 기본 값이어도 예외가 없다
        assert_eq!(guard(&[("filter.a=b.clean", "git-lfs clean -- %f")]), Err(UnsafeFilterName));
        // 다른 드라이버가 먼저여도 거부는 전체다
        assert_eq!(guard(&[("filter.ok.clean", "x"), ("filter.a=b.smudge", "y")]), Err(UnsafeFilterName));
        assert!(UnsafeFilterName.message().contains('='));
    }

    #[test]
    fn 이름이_빈_드라이버는_거부한다() {
        for key in ["filter..clean", "filter..smudge", "filter..process"] {
            assert_eq!(guard(&[(key, "x")]), Err(UnsafeFilterName), "{key}");
        }
        // 다른 드라이버가 먼저여도 거부는 전체다
        assert_eq!(guard(&[("filter.ok.clean", "x"), ("filter..clean", "y")]), Err(UnsafeFilterName));
        // `filter...clean` 의 이름은 "." 이다(빈 이름 아님)
        assert_eq!(guard(&[("filter...clean", "x")]).unwrap().args(), driver_args("."));
    }

    #[test]
    fn 상대_경로_검증() {
        for ok in ["a.txt", "a/b/c.txt", "한글/파일 이름.txt", "a\\b", ".gitignore", "dir/.github/x", "a/b/", "..a", "a..", "...", "~1", "C"] {
            assert_eq!(assert_rel_path(ok), Ok(()), "{ok}");
        }
        let long_ok = "a".repeat(4096);
        let long_bad = "a".repeat(4097);
        let wide_ok = "가".repeat(4096);
        let wide_bad = "가".repeat(4097);
        let emoji_bad = "😀".repeat(2049); // UTF-16 으로 4098 단위
        assert_eq!(assert_rel_path(&long_ok), Ok(()));
        assert_eq!(assert_rel_path(&wide_ok), Ok(()), "한글은 UTF-16 한 단위");
        assert_eq!(assert_rel_path(&"😀".repeat(2048)), Ok(()));
        for bad in ["", "..", "../x", "a/../b", "a\\..\\b", ".git", ".GIT", ".Git/config", "a/.git/x", "a\\.GIT", "/abs", "\\abs", "C:/x", "c:x", "z:", "a\0b", long_bad.as_str(), wide_bad.as_str(), emoji_bad.as_str()] {
            assert_eq!(assert_rel_path(bad), Err("invalid path"), "{bad:.40}");
        }
        // 8.3 이름·끝 점·끝 공백은 이 문자열 검사로는 못 막는다(허용 목록·link_guard 몫)
        for passes in ["GIT~1/config", ".git./config", ".git /config"] {
            assert_eq!(assert_rel_path(passes), Ok(()), "{passes}");
        }
    }

    #[test]
    fn 짧은_이름_모양() {
        for yes in ["GIT~1", "a/GIT~1/config", "x\\PROGRA~2\\y", "foo~12", "~1", "a~0"] {
            assert!(has_short_name_segment(yes), "{yes}");
        }
        for no in ["", "a~", "a~b", "a~1b", "~", "a/b", "x.git~", "~１"] {
            assert!(!has_short_name_segment(no), "{no}");
        }
        // 끝의 구분자는 상관없이 `a~1` 구성 요소가 있으면 해당한다
        assert!(has_short_name_segment("a~1/"));
    }

    #[test]
    fn 끝의_점과_공백을_벗긴_뒤_짧은_이름을_잡는다() {
        for yes in ["GIT~1/config", "GIT~1./config", "GIT~1 /config", "a/GIT~1. ./c", "x\\GIT~12\\config", "GIT~1...", "GIT~1\\ ", "~1. "] {
            assert!(has_short_name_segment(yes), "{yes}");
        }
    }

    #[test]
    fn 일반_경로는_짧은_이름이_아니다() {
        for no in ["a/b.txt", "foo.~1x", "dir/file", "~/x", "a~b", ".", " ", "a~1.b", "a~ .", "a~1\t", "a~1\u{a0}"] {
            assert!(!has_short_name_segment(no), "{no:?}");
        }
    }

    fn bytes(parts: &[&[u8]]) -> Vec<u8> {
        parts.concat()
    }

    #[test]
    fn 잘못된_utf8_키와_u_fffd_는_거부한다() {
        for bad in [
            bytes(&[b"filter.", &[0xff], b".clean\ncmd\0"]),
            bytes(&[b"filter.x.", &[0xc3, 0x28], b"\ncmd\0"]),
            bytes(&[b"other.", &[0xff], b"\nv\0"]),
            bytes(&[b"filter.", &[0xed, 0xa0, 0x80], b".clean\ncmd\0"]),
            bytes(&[b"filter.", &[0xc0, 0xaf], b".clean\ncmd\0"]),
            "filter.\u{fffd}.clean\ncmd\0".as_bytes().to_vec(),
        ] {
            assert_eq!(FilterGuard::from_config_output(&bad), Err(UnsafeFilterName), "{bad:?}");
        }
        // 값의 U+FFFD 는 키가 아니다
        assert_eq!(FilterGuard::from_config_output("filter.x.clean\n\u{fffd}\0".as_bytes()).unwrap().args(), driver_args("x"));
    }

    #[test]
    fn 유효한_비_ascii_이름은_통과하고_값의_잘못된_바이트는_무시한다() {
        assert_eq!(FilterGuard::from_config_output("filter.한글.clean\ncmd\0".as_bytes()).unwrap().args(), driver_args("한글"));
        assert_eq!(FilterGuard::from_config_output(&bytes(&[b"filter.x.clean\ncmd", &[0xff], b"\0"])).unwrap().args(), driver_args("x"));
    }

    #[test]
    fn 접미사는_여기서_판정한다() {
        let g = guard(&[("filter.a.required", "true"), ("filter.a.smudge", "x"), ("filter.b.process", "y"), ("filter.lfs.clean", "git-lfs clean -- %f"), ("filter.c", "z")]).unwrap();
        let mut want = driver_args("a");
        want.extend(driver_args("b"));
        want.extend(driver_args("lfs"));
        assert_eq!(g.args(), want);
    }

    fn many(n: usize) -> Vec<u8> {
        (0..n).flat_map(|i| format!("filter.d{i}.clean\ncmd\0").into_bytes()).collect()
    }

    #[test]
    fn 드라이버_수_상한_256() {
        assert_eq!(filter_driver_names(&many(256)).unwrap().len(), 256);
        assert_eq!(filter_driver_names(&many(257)), Err(UnsafeFilterName));
        // 같은 이름의 반복은 세지 않는다
        let mut dup = many(256);
        dup.extend(many(256));
        assert_eq!(filter_driver_names(&dup).unwrap().len(), 256);
        // lfs 기본값도 센다
        let mut lfs = many(256);
        lfs.extend_from_slice(b"filter.lfs.clean\ngit-lfs clean -- %f\0");
        assert_eq!(filter_driver_names(&lfs), Err(UnsafeFilterName));
        let mut extra = many(256);
        extra.extend_from_slice(b"filter.extra.clean\ncmd\0");
        assert_eq!(filter_driver_names(&extra), Err(UnsafeFilterName));
        // 256개는 이름이 짧아도 `-c` 총 길이(24000)를 넘으므로 가드로는 만들어지지 않는다
        assert_eq!(FilterGuard::from_config_output(&many(256)), Err(UnsafeFilterName));
    }

    #[test]
    fn 명령줄_총_길이_상한은_utf16_단위로_센다() {
        let two = |a: &str, b: &str| format!("filter.{a}.clean\ncmd\0filter.{b}.clean\ncmd\0").into_bytes();
        // 드라이버 하나의 `-c` 묶음은 83 + 4×이름 길이: 두 이름 합 5958 이면 23998, 5959 면 24002
        assert!(FilterGuard::from_config_output(&two(&"a".repeat(2979), &"b".repeat(2979))).is_ok());
        assert_eq!(FilterGuard::from_config_output(&two(&"a".repeat(2979), &"b".repeat(2980))), Err(UnsafeFilterName));
        // 비 BMP 는 UTF-16 으로 두 단위(문자 수로 세면 둘 다 통과한다)
        assert!(FilterGuard::from_config_output(&two(&format!("{}a", "😀".repeat(1489)), &"b".repeat(2979))).is_ok());
        assert_eq!(FilterGuard::from_config_output(&two(&"😀".repeat(1490), &"b".repeat(2979))), Err(UnsafeFilterName));
    }

    #[test]
    fn 거부_문구는_ts_와_같다() {
        assert!(UnsafeFilterName.message().contains("잘못된 UTF-8"));
        assert!(UnsafeFilterName.message().ends_with("작업 트리 변경은 표시하지 않습니다."));
    }

    fn allow(paths: &[&str]) -> HashSet<String> {
        paths.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn area_허용_목록() {
        assert_eq!(WipArea::parse("staged"), Ok(WipArea::Staged));
        for bad in ["", "Staged", "STAGED", "index", "all", "staged "] {
            assert_eq!(WipArea::parse(bad), Err("invalid area"), "{bad}");
        }
    }

    #[test]
    fn untracked_는_보고된_경로와_정확히_일치해야_한다() {
        let set = allow(&["new file.txt", "한글.txt", "dir/x.txt", "nested/"]);
        let ok = |p: &str| validate_wip_diff_request("untracked", p, None, Some(&set));
        assert_eq!(ok("new file.txt"), Ok(WipArea::Untracked));
        assert_eq!(ok("한글.txt"), Ok(WipArea::Untracked));
        assert_eq!(ok("nested/"), Ok(WipArea::Untracked));
        // 목록이 없다·목록에 없다·대소문자 변형·8.3 이름·접두 일치는 거부
        assert_eq!(validate_wip_diff_request("untracked", "new file.txt", None, None), Err("invalid path"));
        for bad in ["other.txt", "NEW FILE.txt", "new file.tx", "dir", "dir/x.txt/", "GIT~1/config", "dir\\x.txt"] {
            assert_eq!(ok(bad), Err("invalid path"), "{bad}");
        }
        // 목록에 있어도 경로 모양 검사가 먼저다
        let hostile = allow(&[".git/config", "../x", "GIT~1/config"]);
        assert_eq!(validate_wip_diff_request("untracked", ".git/config", None, Some(&hostile)), Err("invalid path"));
        assert_eq!(validate_wip_diff_request("untracked", "../x", None, Some(&hostile)), Err("invalid path"));
        // 모양 검사를 통과하는 8.3 이름은 목록에 있어야만 통과한다(git 이 그 경로를 보고했다는 뜻이다)
        assert_eq!(validate_wip_diff_request("untracked", "GIT~1/config", None, Some(&hostile)), Ok(WipArea::Untracked));
        assert_eq!(validate_wip_diff_request("untracked", "GIT~1/config", None, Some(&set)), Err("invalid path"));
    }

    #[test]
    fn unstaged_는_짧은_이름_모양을_거부하고_staged_는_허용_목록을_보지_않는다() {
        assert_eq!(validate_wip_diff_request("unstaged", "src/a.ts", None, None), Ok(WipArea::Unstaged));
        assert_eq!(validate_wip_diff_request("unstaged", "GIT~1/config", None, None), Err("invalid path"));
        assert_eq!(validate_wip_diff_request("unstaged", "a/b~2", None, None), Err("invalid path"));
        assert_eq!(validate_wip_diff_request("staged", "GIT~1/config", None, None), Ok(WipArea::Staged));
        assert_eq!(validate_wip_diff_request("staged", "a.txt", Some("old.txt"), None), Ok(WipArea::Staged));
        // oldPath 도 같은 경로 검증을 거친다
        for bad in ["", "../x", ".git/config", "/abs", "C:/x"] {
            assert_eq!(validate_wip_diff_request("staged", "a.txt", Some(bad), None), Err("invalid path"), "{bad}");
        }
        // 검사 순서: area 가 먼저
        assert_eq!(validate_wip_diff_request("nope", "../x", None, None), Err("invalid area"));
    }

    #[test]
    fn git_lfs_처럼_보이게_뒤에_명령을_붙인_값도_덮어쓴다() {
        // 값은 보지 않는다 — 기본 명령 문자열과 정확히 같아야 통과시키던 예외가 없다
        let g = guard(&[("filter.lfs.clean", "git-lfs clean -- %f; touch x"), ("filter.lfs.process", "git-lfs filter-process && touch y")]).unwrap();
        assert_eq!(g.args(), driver_args("lfs"));
    }

    fn strs(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_string()).collect()
    }

    fn req(area: WipArea, path: &str, old: Option<&str>) -> WipDiffRequest {
        WipDiffRequest { area, path: path.to_string(), old_path: old.map(str::to_string), full: false }
    }

    #[test]
    fn status_argv_는_ts_와_바이트까지_같다() {
        assert_eq!(wip_status_args(&FilterGuard::none()), strs(&["status", "--porcelain=v2", "-z", "--untracked-files=all", "--renames", "--ignore-submodules=all", "--"]));
        // 가드는 전역 옵션 자리(맨 앞)에 붙는다
        let g = guard(&[("filter.evil.clean", "x")]).unwrap();
        let args = wip_status_args(&g);
        assert_eq!(&args[..8], driver_args("evil").as_slice());
        assert_eq!(&args[8..], strs(&["status", "--porcelain=v2", "-z", "--untracked-files=all", "--renames", "--ignore-submodules=all", "--"]).as_slice());
    }

    #[test]
    fn diff_argv_는_ts_와_바이트까지_같고_가드는_모든_영역에_붙는다() {
        let g = guard(&[("filter.evil.clean", "x"), ("filter.lfs.clean", "git-lfs clean -- %f")]).unwrap();
        let mut head = driver_args("evil");
        head.extend(driver_args("lfs"));
        let common = ["diff", "--no-color", "--no-ext-diff", "--no-textconv", "-U3"];
        let want = |tail: &[&str]| head.iter().cloned().chain(strs(&common)).chain(strs(tail)).collect::<Vec<_>>();
        assert_eq!(wip_diff_args(&g, &req(WipArea::Staged, "new name.ts", Some("old name.ts"))), want(&["--cached", "-M", "--", "old name.ts", "new name.ts"]));
        // oldPath 가 같거나 없으면 경로 하나
        assert_eq!(wip_diff_args(&g, &req(WipArea::Staged, "a.txt", Some("a.txt"))), want(&["--cached", "-M", "--", "a.txt"]));
        assert_eq!(wip_diff_args(&g, &req(WipArea::Staged, "a.txt", None)), want(&["--cached", "-M", "--", "a.txt"]));
        // unstaged 는 oldPath 를 쓰지 않는다
        assert_eq!(wip_diff_args(&g, &req(WipArea::Unstaged, "a.txt", Some("b.txt"))), want(&["--no-renames", "--", "a.txt"]));
        assert_eq!(wip_diff_args(&g, &req(WipArea::Untracked, "u 파일.txt", None)), want(&["--no-index", "--", "/dev/null", "u 파일.txt"]));
        // 가드가 비어 있어도 같은 모양이다. 옵션처럼 보이는 경로는 `--` 뒤에 그대로 있다
        assert_eq!(wip_diff_args(&FilterGuard::none(), &req(WipArea::Unstaged, "--output=x", None)), strs(&["diff", "--no-color", "--no-ext-diff", "--no-textconv", "-U3", "--no-renames", "--", "--output=x"]));
    }

    #[test]
    fn 요청_파싱은_ts_의_순서와_문구를_따른다() {
        use serde_json::json;
        let allow = allow(&["u.txt"]);
        let parse = |v: Value| WipDiffRequest::parse(Some(&v), Some(&allow));
        assert_eq!(parse(json!({ "area": "unstaged", "path": "a.txt" })), Ok(req(WipArea::Unstaged, "a.txt", None)));
        assert_eq!(parse(json!({ "area": "staged", "path": "n.ts", "oldPath": "o.ts", "full": 1 })), Ok(WipDiffRequest { full: true, ..req(WipArea::Staged, "n.ts", Some("o.ts")) }));
        assert_eq!(parse(json!({ "area": "untracked", "path": "u.txt", "full": "" })), Ok(req(WipArea::Untracked, "u.txt", None)));
        // 객체가 아니거나 area 가 틀리면 invalid area(경로보다 먼저)
        for bad in [json!(null), json!("x"), json!(5), json!([]), json!({}), json!({ "area": 5, "path": "a" }), json!({ "area": "nope", "path": "../x" }), json!({ "area": "Staged", "path": "a" })] {
            assert_eq!(parse(bad.clone()), Err("invalid area"), "{bad}");
        }
        assert_eq!(WipDiffRequest::parse(None, None), Err("invalid area"));
        // 경로: 문자열이 아니거나(oldPath: null 포함) 규칙에 걸리면 invalid path
        for bad in [
            json!({ "area": "staged" }),
            json!({ "area": "staged", "path": null }),
            json!({ "area": "staged", "path": 5 }),
            json!({ "area": "staged", "path": "a", "oldPath": null }),
            json!({ "area": "staged", "path": "a", "oldPath": 5 }),
            json!({ "area": "staged", "path": "a", "oldPath": "../x" }),
            json!({ "area": "unstaged", "path": "a\u{0}b" }),
            json!({ "area": "untracked", "path": "other.txt" }),
            json!({ "area": "unstaged", "path": "GIT~1/config" }),
        ] {
            assert_eq!(parse(bad.clone()), Err("invalid path"), "{bad}");
        }
        // 허용 목록이 없으면 untracked 는 거부
        assert_eq!(WipDiffRequest::parse(Some(&json!({ "area": "untracked", "path": "u.txt" })), None), Err("invalid path"));
    }

    #[test]
    fn 가드_오류는_ts_의_git_오류와_같은_모양이다() {
        let unsafe_err = ExecError::from(FilterGuardError::Unsafe(UnsafeFilterName));
        assert_eq!(unsafe_err.envelope_code(), "GIT");
        assert_eq!(unsafe_err.to_string(), UNSAFE_FILTER_MSG);
        let cfg = ExecError::from(FilterGuardError::Config(128, " fatal: bad config \n".to_string()));
        assert_eq!((cfg.envelope_code(), cfg.to_string()), ("GIT", "fatal: bad config".to_string()));
        assert_eq!(ExecError::from(FilterGuardError::Config(2, String::new())).to_string(), "git config exited with 2");
        assert_eq!(ExecError::from(FilterGuardError::Git(ExecError::Aborted)), ExecError::Aborted);
    }

    #[test]
    fn 추적_안_되는_폴더_항목() {
        assert!(is_untracked_dir_entry(WipArea::Untracked, "nested/"));
        assert!(is_untracked_dir_entry(WipArea::Untracked, "nested\\"));
        assert!(!is_untracked_dir_entry(WipArea::Untracked, "file"));
        assert!(!is_untracked_dir_entry(WipArea::Staged, "nested/"));
    }
}
