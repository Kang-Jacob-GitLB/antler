//! `git:refs`·`git:refsDetails`(`src/main/git/commands.ts` 의 `refs`·`refsDetails`·`submodules`): 기본 브랜치 판정(`defaultBranch.ts`)·서브모듈 경로 검증(`isSafeSubmodulePath`)과
//! 그 위의 git 호출(스냅샷 조립·서브모듈 확인). 큐·키 합치기·첫 페이지 대기·envelope 은 라우터가 맡는다.
//!
//! 보안: **서브모듈 경로는 저장소가 정하는 값**(인덱스의 gitlink)이라 `git -C <저장소>/<경로>` 에 들어가기 전에 둘을 통과해야 한다 — ①문자열 검사(`is_safe_submodule_path`),
//! ②로컬 실행기면 실제 경로 검사(`link_guard::assert_inside_root` — 정션·symlink·8.3 이름으로 저장소 밖·`.git` 안을 가리키면 거부). 하나라도 실패하면 git 을 돌리지 않고 `unknown` 이다.
//! `link_guard` 에는 git 의 cwd/`-C` 와 같은 **비 verbatim 문자열**을 넘긴다(`canonicalize` 는 `link_guard` 안에서 한다). 서브모듈의 git 호출도 같은 실행기라 하드닝(`GIT_GLOBAL_ARGS`·`GIT_ENV`)이 그대로다.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use futures_util::stream::{self, StreamExt};
use serde::Serialize;

use crate::git::commit::ok;
use crate::git::exec::{ExecError, GitExecutor, RunOpts, RunResult};
use crate::git::fetch::js_trim;
use crate::git::link_guard::assert_inside_root;
use crate::git::parsers::refs::{parse_refs, parse_remote_names, parse_tracking, refs_args, refs_args_lite, Branch, RemoteGroup, Tag, Tracking, REFS_TRACK_ARGS};
use crate::git::parsers::stash::{parse_stashes, Stash, STASH_ARGS};
use crate::git::parsers::submodule::{parse_gitlinks, parse_gitmodules, parse_probe, submodule_status, SubmoduleStatus, GITLINK_ARGS, GITMODULES_ARGS};
use crate::git::parsers::worktree::{parse_worktrees, Worktree, WORKTREE_ARGS};

/// 서브모듈 수 상한
pub const SUBMODULE_MAX: usize = 200;
/// 이 수를 넘는 서브모듈은 HEAD 를 확인하지 않는다(status: unknown)
pub const SUBMODULE_PROBE_MAX: usize = 50;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DefaultBranch {
    /// 짧은 이름(origin/main, main, master)
    pub name: String,
    pub hash: String,
}

/// 저장소의 기본 브랜치 판정(읽기 전용). 순서:
/// 1) `refs/remotes/origin/HEAD` 가 가리키는 리모트 추적 브랜치(예: origin/main) — 로컬 main 이 체크아웃돼 있거나 앞서 있어도 원격 tip 을 쓴다
/// 2) origin/HEAD 가 없으면 로컬 main, 그다음 master  3) 그 밖에는 없음
///
/// `origin_head` 는 `git symbolic-ref -q refs/remotes/origin/HEAD` 의 출력(없으면 None).
pub fn pick_default_branch(origin_head: Option<&str>, local: &[Branch], remotes: &[RemoteGroup]) -> Option<DefaultBranch> {
    let target = origin_head.map(js_trim);
    if let Some(target) = target.filter(|t| t.starts_with("refs/remotes/")) {
        if let Some(b) = remotes.iter().find_map(|g| g.branches.iter().find(|b| b.full_name == target)) {
            return Some(DefaultBranch { name: b.name.clone(), hash: b.hash.clone() });
        }
    }
    ["main", "master"].iter().find_map(|name| local.iter().find(|b| b.name == *name)).map(|b| DefaultBranch { name: b.name.clone(), hash: b.hash.clone() })
}

/// 인덱스의 gitlink 경로는 저장소가 정하는 값이다(`git -C` 로 저장소 밖을 가리킬 수 있다). 절대 경로(`/`·`\` 시작, 드라이브 문자)나
/// 빈·`.`·`..` 구성 요소(`/`·`\` 모두 구분자)가 있으면 git 을 돌리지 않고 unknown 으로 둔다.
/// 이 검사는 문자열뿐이다 — 정션·심볼릭 링크·8.3 이름으로 저장소 밖을 가리키는 경로는 실제 경로를 확인하는 `link_guard::assert_inside_root` 가 맡는다.
pub fn is_safe_submodule_path(p: &str) -> bool {
    let b = p.as_bytes();
    if matches!(b.first(), Some(b'/' | b'\\')) || (b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':') {
        return false;
    }
    p.split(['/', '\\']).all(|seg| !matches!(seg, "" | "." | ".."))
}

/// 서브모듈 경로 `-C` 의 기준: 저장소 경로 끝의 `/`·`\` 를 지운다(TS `repoPath.replace(/[\\/]+$/, '')`)
pub fn submodule_base(repo_path: &str) -> &str {
    repo_path.trim_end_matches(['/', '\\'])
}

/// 이 수까지 서브모듈 확인(`git -C`)을 동시에 돌린다
const SUBMODULE_PROBE_CONCURRENCY: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SubmoduleInfo {
    /// `.gitmodules` 의 이름(없으면 경로)
    pub name: String,
    /// 상위 저장소 기준 상대 경로(POSIX 구분자)
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// 상위 저장소가 기록한 커밋
    pub recorded: String,
    /// 체크아웃된 HEAD(초기화되지 않았거나 알 수 없으면 없다)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub head: Option<String>,
    pub status: SubmoduleStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HeadInfo {
    /// 현재 브랜치. detached 면 null
    pub branch: Option<String>,
    pub hash: Option<String>,
}

/// `git:refs` 의 응답(`RefsSnapshot`)
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RefsSnapshot {
    pub head: HeadInfo,
    pub local: Vec<Branch>,
    pub remotes: Vec<RemoteGroup>,
    pub tags: Vec<Tag>,
    pub worktrees: Vec<Worktree>,
    pub stashes: Vec<Stash>,
    pub submodules: Vec<SubmoduleInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_branch: Option<DefaultBranch>,
}

/// `git:refsDetails` 의 응답(`RefsDetails`)
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RefsDetails {
    pub tracking: BTreeMap<String, Tracking>,
    pub submodules: Vec<SubmoduleInfo>,
}

/// 종료 코드가 0 이 아니면 빈 출력(스태시 없음·커밋 없음 등). 실행 실패·취소는 그대로 오류다(TS `okOrEmpty`)
async fn ok_or_empty(exec: &GitExecutor, repo_path: &str, args: &[String], opts: &RunOpts) -> Result<Vec<u8>, ExecError> {
    let r = exec.run(repo_path, args, opts).await?;
    Ok(if r.code == 0 { r.stdout } else { Vec::new() })
}

fn text(stdout: &[u8]) -> String {
    String::from_utf8_lossy(stdout).into_owned()
}

/// 현재 브랜치와 HEAD 해시(TS `head`). 없으면 null
async fn head_info(exec: &GitExecutor, repo_path: &str, opts: &RunOpts) -> Result<HeadInfo, ExecError> {
    let branch_args = ["symbolic-ref", "--quiet", "--short", "HEAD"].map(String::from);
    let hash_args = ["rev-parse", "--verify", "--quiet", "HEAD"].map(String::from);
    let (branch, hash) = tokio::join!(exec.run(repo_path, &branch_args, opts), exec.run(repo_path, &hash_args, opts));
    let pick = |r: RunResult| (r.code == 0).then(|| js_trim(&text(&r.stdout)).to_string());
    Ok(HeadInfo { branch: pick(branch?), hash: pick(hash?) })
}

/// 서브모듈 하나의 상태 확인. 저장소가 정한 경로라 문자열 검사와(로컬이면) 실제 경로 검사를 통과해야만 `git -C` 를 돌린다.
/// 실패는 이 서브모듈만 `unknown` 으로 둔다(`status`·`head` 를 건드리지 않는다)
async fn probe_submodule(exec: &GitExecutor, base: &str, parent_git_dir: &str, s: &mut SubmoduleInfo, opts: &RunOpts) {
    if !is_safe_submodule_path(&s.path) {
        return;
    }
    let sub = format!("{base}/{}", s.path);
    // 로컬이면 정션·symlink 로 저장소 밖(또는 .git 안)을 가리키는 경로도 거부한다. WSL·SSH 는 Rust 가 그 파일 시스템을 볼 수 없어 하지 않는다
    // GitHub 원격 전용 저장소 실행기(`"github"`)도 로컬 git 이다 — TS 에서는 같은 `LocalExecutor`(kind `local`)다. 다만 그 경로는 서브모듈을 건너뛰어(`skip_submodules`) 여기에 오지 않는다
    if matches!(exec.kind(), "local" | "github") {
        let (root, target) = (base.to_string(), sub.clone());
        match tokio::task::spawn_blocking(move || assert_inside_root(Path::new(&root), Path::new(&target))).await {
            Ok(Ok(())) => {}
            _ => return,
        }
    }
    let args = ["rev-parse", "--absolute-git-dir", "HEAD"].map(String::from);
    let Ok(r) = exec.run(&sub, &args, opts).await else { return };
    let probe = parse_probe(&text(&r.stdout));
    let Some(git_dir) = probe.git_dir else {
        // 디렉터리 자체가 없는 경우(체크아웃된 적 없음)도 미초기화다
        if r.code != 0 {
            let lower = r.stderr.to_ascii_lowercase();
            if ["cannot change to", "no such file", "not a directory"].iter().any(|p| lower.contains(p)) {
                s.status = SubmoduleStatus::Uninitialized;
            }
        }
        return;
    };
    // 초기화되지 않은 서브모듈 디렉터리는 비어 있어 git 이 상위 저장소를 찾아 올라간다 — git-dir 이 상위와 같으면 미초기화다
    if git_dir == parent_git_dir {
        s.status = SubmoduleStatus::Uninitialized;
        return;
    }
    s.head = probe.head.clone();
    s.status = submodule_status(&s.recorded, probe.head.as_deref());
}

/// 최상위 서브모듈 목록. `git submodule` 은 쓰지 않는다(하위 저장소 설정까지 건드린다).
/// 실패는 목록 전체를 막지 않는다: 읽을 수 없으면 빈 목록, 하나의 확인 실패는 unknown(TS `submodules`). 중첩 서브모듈은 보지 않는다(최상위만)
pub async fn submodules(exec: &GitExecutor, repo_path: &str, opts: &RunOpts) -> Result<Vec<SubmoduleInfo>, ExecError> {
    let link_out = exec.run(repo_path, &GITLINK_ARGS.map(String::from), opts).await?;
    if link_out.code != 0 {
        return Ok(Vec::new());
    }
    let mut links = parse_gitlinks(&link_out.stdout);
    links.truncate(SUBMODULE_MAX);
    if links.is_empty() {
        return Ok(Vec::new());
    }
    let parent_args = ["rev-parse", "--absolute-git-dir"].map(String::from);
    let modules_args = GITMODULES_ARGS.map(String::from);
    let (mod_out, parent_probe) = tokio::join!(exec.run(repo_path, &modules_args, opts), exec.run(repo_path, &parent_args, opts));
    let (mod_out, parent_probe) = (mod_out?, parent_probe?);
    let entries = if mod_out.code == 0 { parse_gitmodules(&mod_out.stdout) } else { Vec::new() };
    let modules: HashMap<&str, _> = entries.iter().map(|m| (m.path.as_str(), m)).collect();
    let parent_git_dir = (parent_probe.code == 0)
        .then(|| text(&parent_probe.stdout).split('\n').next().map(|l| l.strip_suffix('\r').unwrap_or(l).to_string()))
        .flatten()
        .filter(|d| !d.is_empty());

    let mut result: Vec<SubmoduleInfo> = links
        .into_iter()
        .map(|l| {
            let m = modules.get(l.path.as_str());
            SubmoduleInfo {
                name: m.map_or_else(|| l.path.clone(), |m| m.name.clone()),
                url: m.and_then(|m| m.url.clone()),
                branch: m.and_then(|m| m.branch.clone()),
                path: l.path,
                recorded: l.sha,
                head: None,
                status: SubmoduleStatus::Unknown,
            }
        })
        .collect();
    let Some(parent_git_dir) = parent_git_dir else { return Ok(result) };

    let base = submodule_base(repo_path);
    let n = result.len().min(SUBMODULE_PROBE_MAX);
    let parent_git_dir = parent_git_dir.as_str();
    let probed: Vec<SubmoduleInfo> = stream::iter(result.drain(..n))
        .map(|mut s| async move {
            probe_submodule(exec, base, parent_git_dir, &mut s, opts).await;
            s
        })
        .buffered(SUBMODULE_PROBE_CONCURRENCY)
        .collect()
        .await;
    // 확인한 앞부분(최대 SUBMODULE_PROBE_MAX)과 확인하지 않은 나머지(unknown)를 원래 순서로 잇는다
    let rest = std::mem::take(&mut result);
    Ok(probed.into_iter().chain(rest).collect())
}

/// 사이드바용 ref 스냅샷. 여러 조회를 동시에 돌린다(TS `refs`). `lite` 면 비싼 부가 정보(업스트림 ahead/behind·서브모듈)를 빼고 돌려준다 — `refs_details` 가 따로 채운다
pub async fn refs_snapshot(exec: &GitExecutor, repo_path: &str, opts: &RunOpts, lite: bool) -> Result<RefsSnapshot, ExecError> {
    let refs_a = if lite { refs_args_lite() } else { refs_args() };
    let remote_a = ["remote".to_string()];
    let origin_a = ["symbolic-ref", "--quiet", "refs/remotes/origin/HEAD"].map(String::from);
    let (worktree_a, stash_a) = (WORKTREE_ARGS.map(String::from), STASH_ARGS.map(String::from));
    let (refs_out, remote_out, worktree_out, stash_out, head, origin_head, subs) = tokio::join!(
        async { ok(exec.run(repo_path, &refs_a, opts).await?, &refs_a) },
        ok_or_empty(exec, repo_path, &remote_a, opts),
        ok_or_empty(exec, repo_path, &worktree_a, opts),
        // 스태시가 없거나 reflog 가 없는 저장소는 빈 목록이다
        ok_or_empty(exec, repo_path, &stash_a, opts),
        head_info(exec, repo_path, opts),
        // origin/HEAD 가 없으면(clone 이 아니거나 `git remote set-head` 를 안 한 저장소) 실패 코드로 끝난다 — 기본 브랜치는 대체 규칙으로 정한다
        async { exec.run(repo_path, &origin_a, opts).await.map(|r| (r.code == 0).then(|| text(&r.stdout))) },
        // 서브모듈은 부가 정보라 실패해도 ref 스냅샷을 막지 않는다
        async { if lite { Vec::new() } else { submodules(exec, repo_path, opts).await.unwrap_or_default() } }
    );
    let (refs_out, remote_out, worktree_out, stash_out, head, origin_head) = (refs_out?, remote_out?, worktree_out?, stash_out?, head?, origin_head?);
    let parsed = parse_refs(&refs_out, &parse_remote_names(&remote_out));
    let default_branch = pick_default_branch(origin_head.as_deref(), &parsed.local, &parsed.remotes);
    Ok(RefsSnapshot {
        head,
        local: parsed.local,
        remotes: parsed.remotes,
        tags: parsed.tags,
        worktrees: parse_worktrees(&worktree_out),
        stashes: parse_stashes(&stash_out),
        submodules: subs,
        default_branch,
    })
}

/// `refs_snapshot(lite)` 에서 미룬 부가 정보: 로컬 브랜치의 업스트림 ahead/behind 와 서브모듈. 서브모듈 확인이 실패해도 빈 값이다(TS `refsDetails`)
pub async fn refs_details(exec: &GitExecutor, repo_path: &str, opts: &RunOpts, skip_submodules: bool) -> Result<RefsDetails, ExecError> {
    let track_a = REFS_TRACK_ARGS.map(String::from);
    let (track_out, subs) = tokio::join!(ok_or_empty(exec, repo_path, &track_a, opts), async {
        if skip_submodules {
            Vec::new()
        } else {
            submodules(exec, repo_path, opts).await.unwrap_or_default()
        }
    });
    Ok(RefsDetails { tracking: parse_tracking(&track_out?), submodules: subs })
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- tests/defaultBranch.test.ts 의 pickDefaultBranch 를 같은 입력으로 옮긴 것 ----

    fn br(name: &str, hash: &str, remote: bool) -> Branch {
        let full_name = if remote { format!("refs/remotes/{name}") } else { format!("refs/heads/{name}") };
        Branch { name: name.into(), full_name, hash: hash.into(), upstream: None, ahead: None, behind: None, upstream_gone: None, is_head: false }
    }

    fn group(branches: Vec<Branch>) -> RemoteGroup {
        RemoteGroup { name: "origin".into(), branches }
    }

    fn picked(name: &str, hash: &str) -> Option<DefaultBranch> {
        Some(DefaultBranch { name: name.into(), hash: hash.into() })
    }

    #[test]
    fn origin_head_가_가리키는_원격_브랜치를_우선한다() {
        let r = pick_default_branch(Some("refs/remotes/origin/trunk\n"), &[br("main", "L", false)], &[group(vec![br("origin/trunk", "T", true), br("origin/main", "M", true)])]);
        assert_eq!(r, picked("origin/trunk", "T"));
    }

    #[test]
    fn origin_head_가_없으면_로컬_main_master_순() {
        assert_eq!(pick_default_branch(None, &[br("master", "A", false), br("main", "B", false)], &[]), picked("main", "B"));
        assert_eq!(pick_default_branch(None, &[br("dev", "D", false), br("master", "A", false)], &[]), picked("master", "A"));
    }

    #[test]
    fn origin_head_가_가리키는_브랜치를_찾을_수_없으면_대체_규칙으로_간다() {
        assert_eq!(pick_default_branch(Some("refs/remotes/origin/gone"), &[br("main", "B", false)], &[group(vec![br("origin/x", "X", true)])]), picked("main", "B"));
        assert_eq!(pick_default_branch(Some("garbage"), &[], &[]), None);
    }

    #[test]
    fn 아무것도_없으면_none() {
        assert_eq!(pick_default_branch(None, &[br("dev", "D", false)], &[]), None);
    }

    // ---- tests/submodule.test.ts 의 isSafeSubmodulePath ----

    #[test]
    fn 상대_경로의_정상_구성_요소만_허용한다() {
        for p in ["sub", "libs/한글 모듈", "a/b/c", "..outside", "a..b"] {
            assert!(is_safe_submodule_path(p), "{p}");
        }
        for p in ["..", "../x", "a/..", "..\\outside", "a\\..\\b", ".", "./x", "a//b", "a/", "/abs", "\\abs", "C:/abs", "c:x"] {
            assert!(!is_safe_submodule_path(p), "{p}");
        }
    }

    // ---- 이 PR 에서 더한 경계 ----

    #[test]
    fn 기본_브랜치_판정은_js_trim_기준() {
        let local = [br("main", "L", false)];
        let remotes = [group(vec![br("origin/main", "R", true)])];
        // BOM·NBSP·전각 공백은 JS trim 이 지운다
        assert_eq!(pick_default_branch(Some("\u{feff}refs/remotes/origin/main\u{3000}"), &local, &remotes), picked("origin/main", "R"));
        // U+0085 는 지우지 않는다 → 접두 검사에서 탈락해 로컬 main
        assert_eq!(pick_default_branch(Some("\u{85}refs/remotes/origin/main"), &local, &remotes), picked("main", "L"));
        // 같은 이름이 두 리모트에 있으면 먼저 나온 것
        let dup = [RemoteGroup { name: "origin".into(), branches: vec![br("origin/main", "first", true)] }, RemoteGroup { name: "dup".into(), branches: vec![br("origin/main", "second", true)] }];
        assert_eq!(pick_default_branch(Some("refs/remotes/origin/main"), &[], &dup), picked("origin/main", "first"));
        // 대소문자는 구분한다
        assert_eq!(pick_default_branch(None, &[br("Main", "L9", false), br("master", "L2", false)], &[]), picked("master", "L2"));
    }

    #[test]
    fn 서브모듈_경로_경계() {
        for ok in ["a\\b", "한글/모듈", "a.b/.c", "..."] {
            assert!(is_safe_submodule_path(ok), "{ok}");
        }
        for bad in ["", "a\\\\b", "//host/share", "z:", ".git/../x"] {
            assert!(!is_safe_submodule_path(bad), "{bad}");
        }
        // 드라이브 문자는 알파벳뿐이다: "1:" 은 일반 구성 요소다
        assert!(is_safe_submodule_path("1:/x"));
    }

    #[test]
    fn 기준_경로의_끝_구분자를_지운다() {
        assert_eq!(submodule_base("C:\\repo\\\\"), "C:\\repo");
        assert_eq!(submodule_base("/r/"), "/r");
        assert_eq!(submodule_base("/r"), "/r");
    }
}
