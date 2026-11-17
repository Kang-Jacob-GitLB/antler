//! 작업 트리(WIP) 조회 `git:wipStatus`·`git:wipDiff` 의 순수 로직(`src/main/git/wip.ts`): filter 드라이버 무력화 가드(`FilterGuard`)와 요청 경로 검증.
//! git 을 부르는 부분과 untracked 허용 목록의 보관(저장소별 마지막 `wipStatus` 가 보고한 경로)은 WIP 채널 PR 에서 옮긴다.
//!
//! 보안(악성 저장소 방어):
//! - 작업 트리를 읽는 status·diff 는 `.gitattributes` 의 filter 드라이버(clean/process)를 실행한다. 저장소 설정에 심은 명령이면 저장소를 열기만 해도
//!   임의 명령이 돈다. 모든 드라이버(git-lfs 포함 — clean 이 `lfs.extension.*.clean` 을 실행하고 `.git/lfs` 에 쓴다)는 이번 실행에서만 빈 명령으로 덮어 쓴다 — 그 `-c` 인자는 [`FilterGuard`] 만 만든다.
//! - untracked diff 는 `diff --no-index` 로 파일을 직접 연다. 경로 문자열 검사([`assert_rel_path`])는 Windows 8.3 짧은 이름(`GIT~1/config`)을 `.git` 으로
//!   알아보지 못하므로, untracked 는 **마지막 `wipStatus` 가 git 출력으로 보고한 경로와 정확히 일치**할 때만 받는다(허용 목록, 없으면 거부 — 닫힌 기본값).
//!   unstaged 는 `~숫자` 8.3 모양 구성 요소를 거부하고, 로컬 저장소는 열기 직전에 `link_guard` 로 정션·symlink 를 확인한다.

use std::collections::HashSet;

use crate::git::exec::{ExecError, GitExecutor, RunOpts};
use crate::git::fetch::js_to_lowercase;

/// 덮어쓸 수 없는 필터 드라이버가 있어 작업 트리 조회를 거부할 때의 메시지
pub const UNSAFE_FILTER_MSG: &str =
    "이 저장소의 filter 드라이버(이름에 \"=\" 나 잘못된 UTF-8 이 있거나 개수가 너무 많음)를 안전하게 무력화할 수 없습니다. 작업 트리 변경은 표시하지 않습니다.";

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
    (!name.is_empty()).then_some(name)
}

impl FilterGuard {
    /// 덮어쓸 것이 없다(untracked diff 는 `--no-index` 라 필터를 돌지 않는다)
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
    /// `git config` 가 0·1 이 아닌 코드로 끝났다
    Config(i32),
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
        c => Err(FilterGuardError::Config(c)),
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
        if name.contains('=') {
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
        // 이름이 비었거나 접미사가 다르면 드라이버가 아니다
        assert!(guard(&[("filter..clean", "x"), ("filter.a.required", "x"), ("filter.a", "x"), ("xfilter.a.clean", "x"), ("Filter.a.clean", "x")]).unwrap().is_empty());
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
    fn 추적_안_되는_폴더_항목() {
        assert!(is_untracked_dir_entry(WipArea::Untracked, "nested/"));
        assert!(is_untracked_dir_entry(WipArea::Untracked, "nested\\"));
        assert!(!is_untracked_dir_entry(WipArea::Untracked, "file"));
        assert!(!is_untracked_dir_entry(WipArea::Staged, "nested/"));
    }
}
