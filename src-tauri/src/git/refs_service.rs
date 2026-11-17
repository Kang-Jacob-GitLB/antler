//! `git:refs`·`git:refsDetails` 의 순수 로직: 기본 브랜치 판정(`src/main/git/defaultBranch.ts`)과 서브모듈 경로 검증(`commands.ts` 의 `isSafeSubmodulePath`).
//! git 을 부르는 부분(스냅샷 조립·서브모듈 확인)은 refs 채널 PR 에서 옮긴다.

use serde::Serialize;

use crate::git::fetch::js_trim;
use crate::git::parsers::refs::{Branch, RemoteGroup};

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
