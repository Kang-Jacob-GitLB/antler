//! 저장소 추가 창의 "폴더 안 저장소 찾기"(`src/main/services/repoScan.ts`): 고른 폴더 바로 아래 하위 폴더 중 `.git` 이 있는 것을 고른다(읽기 전용).
//! 로컬과 WSL(`\\wsl.localhost\…` 공유 경로)은 같은 파일 시스템 API 로 읽는다. 진짜 저장소인지는 추가할 때 git 이 확인한다.
//!
//! TS 와 다른 점:
//! - 이름순 정렬은 `localeCompare` 의 **근사**다(ICU 전체를 옮기지 않았다 — 글자는 대소문자 무시·같으면 소문자 먼저, ASCII 기호는 CLDR root 순서, ASCII 밖은 코드 포인트순).
//!   TS 의 `localeCompare` 는 numeric 이 아니라 숫자도 글자처럼 한 자리씩 비교한다(`a10` < `a2`).
//! - 읽기 실패 문구는 Node 의 오류 메시지가 아니라 Rust 의 `io::Error` 문구다.
//! - 동기 함수다(호출자가 `spawn_blocking` 으로 부른다). `.git` 확인은 TS 처럼 동시에 8개씩 한다(WSL 공유(9P)가 느릴 때 스레드를 다 차지하지 않게).

use std::cmp::Ordering;
use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};

use serde::{Deserialize, Serialize};

use crate::fsguard::links_stay_local;
use crate::git::exec::ssh::win32_path;
use crate::git::parsers::refs::SYMBOL_ORDER;

/// 하위 폴더 상한 — 아주 큰 폴더(예: node_modules)에서 창이 굳지 않게
pub const MAX_DIRS: usize = 2000;
/// 동시에 띄우는 stat 수
pub const PROBE_CONCURRENCY: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ScanRepo {
    pub name: String,
    pub path: String,
}

/// TS `RepoScanResult`
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoScanResult {
    pub path: String,
    pub is_repo: bool,
    pub repos: Vec<ScanRepo>,
    pub truncated: bool,
}

/// TS `RemoteDirListing` 의 하위 폴더 항목. `is_repo` 가 `None` 이면 확인하지 않은 것이다
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteDirEntry {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_repo: Option<bool>,
}

/// TS `RemoteDirListing`(SFTP 폴더 목록 — `git/exec/ssh/sftp.rs` 가 만든다)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteDirListing {
    pub path: String,
    #[serde(default)]
    pub parent: Option<String>,
    pub is_repo: bool,
    pub dirs: Vec<RemoteDirEntry>,
    pub truncated: bool,
}

/// 로컬·WSL 공유 경로: 고른 폴더 바로 아래에서 `.git` 이 있는 폴더를 찾는다(고른 폴더가 저장소가 아님은 호출자가 확인했다).
/// 숨김(`.` 시작) 폴더는 뺀다. 링크(정션)는 **경로 전체(구성 요소마다)가 원격으로 이어지지 않을 때만**(`fsguard::links_stay_local`) 따라가 폴더인지 본다(원격 대상은 SMB 접속·NTLM 해시 유출 위험).
/// `.git` 은 `symlink_metadata` 로 존재만 본다(링크를 따라가지 않는다). 폴더가 2000개를 넘으면 앞쪽만 보고 `truncated`.
///
/// **호출자는 먼저 `mirror::check_path("local", root)` 를 통과시킨다.** 여기서도 root 가 WSL 이 아닌 UNC·NT 네임스페이스면 거부하지만(방어), 길이·NUL 같은 검사는 호출자 몫이다.
/// 고른 폴더(루트) 자체가 링크로 원격에 이어지는지는 여기서 보지 않는다 — `env_host` 가 git 을 돌리기 전에 `fsguard::check_local_root` 로 확인한다
pub fn scan_local_repos(root: &str) -> Result<RepoScanResult, String> {
    scan_local_repos_with(root, crate::fsguard::is_remote_link_target)
}

/// [`scan_local_repos`] 의 판정 함수 주입판(시험이 원격 링크를 만들지 않고도 거부 경로를 보게 한다)
pub fn scan_local_repos_with(root: &str, is_remote: impl Fn(&str) -> bool) -> Result<RepoScanResult, String> {
    crate::mirror::check_path("local", root)?;
    let entries = fs::read_dir(root).map_err(|e| e.to_string())?;
    let mut dirs: Vec<String> = Vec::new();
    let mut truncated = false;
    for entry in entries {
        let entry = entry.map_err(|e| e.to_string())?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let Ok(kind) = entry.file_type() else { continue };
        // 심볼릭 링크(정션)는 경로의 어느 구성 요소도 원격으로 이어지지 않을 때만 따라가 폴더인지 본다
        let is_dir = kind.is_dir() || (kind.is_symlink() && links_stay_local(&entry.path(), &is_remote) && fs::metadata(entry.path()).is_ok_and(|m| m.is_dir()));
        if !is_dir {
            continue;
        }
        if dirs.len() >= MAX_DIRS {
            truncated = true;
            break;
        }
        dirs.push(name);
    }
    let flags = probe_git(root, &dirs);
    let mut repos: Vec<ScanRepo> = dirs
        .iter()
        .zip(flags)
        .filter(|(_, has_git)| *has_git)
        .map(|(name, _)| ScanRepo { name: name.clone(), path: win32_path::join(&[root, name]) })
        .collect();
    repos.sort_by(|a, b| locale_compare(&a.name, &b.name));
    Ok(RepoScanResult { path: root.to_string(), is_repo: false, repos, truncated })
}

/// 각 하위 폴더의 `.git` 존재(파일이어도 된다 — 워크트리·서브모듈). `PROBE_CONCURRENCY` 개씩 동시에, 결과는 입력 순서대로
fn probe_git(root: &str, dirs: &[String]) -> Vec<bool> {
    let next = AtomicUsize::new(0);
    let mut out = vec![false; dirs.len()];
    let results: Vec<Vec<(usize, bool)>> = std::thread::scope(|s| {
        let workers: Vec<_> = (0..PROBE_CONCURRENCY.min(dirs.len()))
            .map(|_| {
                s.spawn(|| {
                    let mut done = Vec::new();
                    loop {
                        let i = next.fetch_add(1, AtomicOrdering::SeqCst);
                        let Some(dir) = dirs.get(i) else { break };
                        // 링크를 따라가지 않고 존재만 본다
                        done.push((i, Path::new(root).join(dir).join(".git").symlink_metadata().is_ok()));
                    }
                    done
                })
            })
            .collect();
        workers.into_iter().map(|w| w.join().unwrap_or_default()).collect()
    });
    for (i, found) in results.into_iter().flatten() {
        out[i] = found;
    }
    out
}

/// SSH: 원격 폴더 목록(SFTP)에서 저장소 표시가 된 하위 폴더를 고른다(TS `scanFromListing`)
pub fn scan_from_listing(listing: &RemoteDirListing) -> RepoScanResult {
    RepoScanResult {
        path: listing.path.clone(),
        is_repo: listing.is_repo,
        repos: if listing.is_repo {
            Vec::new()
        } else {
            listing.dirs.iter().filter(|d| d.is_repo == Some(true)).map(|d| ScanRepo { name: d.name.clone(), path: posix_join(&listing.path, &d.name) }).collect()
        },
        // 목록이 잘렸거나, 앞쪽 일부만 저장소 여부를 확인했다(SFTP 목록의 확인 상한)
        truncated: listing.truncated || listing.dirs.iter().any(|d| d.is_repo.is_none()),
    }
}

/// `path.posix.join(a, b)` — 빈 조각은 건너뛰고 `.`·`..`·중복 `/` 를 정리한다
fn posix_join(a: &str, b: &str) -> String {
    let joined = [a, b].iter().filter(|p| !p.is_empty()).copied().collect::<Vec<_>>().join("/");
    if joined.is_empty() {
        return ".".to_string();
    }
    let absolute = joined.starts_with('/');
    let trailing = joined.ends_with('/');
    let mut stack: Vec<&str> = Vec::new();
    for seg in joined.split('/') {
        match seg {
            "" | "." => {}
            ".." => match stack.last() {
                Some(&last) if last != ".." => {
                    stack.pop();
                }
                _ if !absolute => stack.push(".."),
                _ => {}
            },
            s => stack.push(s),
        }
    }
    let mut path = stack.join("/");
    if path.is_empty() && !absolute {
        path.push('.');
    }
    if trailing && !path.is_empty() && path != "/" {
        path.push('/');
    }
    if absolute {
        format!("/{path}")
    } else {
        path
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum SortKey {
    Symbol(usize),
    Digit(char),
    Letter(u32),
}

fn sort_keys(s: &str) -> Vec<SortKey> {
    s.chars()
        .filter(|c| !c.is_ascii_control())
        .map(|c| {
            if let Some(i) = SYMBOL_ORDER.find(c) {
                SortKey::Symbol(i)
            } else if c.is_ascii_digit() {
                SortKey::Digit(c)
            } else {
                SortKey::Letter(c.to_lowercase().next().map_or(0, u32::from))
            }
        })
        .collect()
}

/// 2차 키: 키와 같은 필터(제어 문자 제외)를 거친 글자마다 소문자(0)·그 밖(1). 사전식으로 비교하면 처음 다른 대소문자는 소문자가 앞이다
fn case_keys(s: &str) -> Vec<u8> {
    s.chars().filter(|c| !c.is_ascii_control()).map(|c| u8::from(!c.is_lowercase())).collect()
}

/// `a.localeCompare(b)` 의 근사(모듈 문서). **전순서**다 — 1차 키 → 대소문자 키 → 원문 바이트순.
/// 서버(SFTP)가 고른 이름도 정렬하므로 추이성이 깨지면 `sort_by` 가 패닉할 수 있다(`İ`·`i`·`I` 처럼 키가 같은 서로 다른 글자, 키에서 빠지는 제어 문자)
pub(crate) fn locale_compare(a: &str, b: &str) -> Ordering {
    sort_keys(a).cmp(&sort_keys(b)).then_with(|| case_keys(a).cmp(&case_keys(b))).then_with(|| a.cmp(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 시험마다 따로 쓰는 임시 폴더(끝나면 지운다)
    struct Tmp(std::path::PathBuf);

    impl Tmp {
        fn new(name: &str) -> Tmp {
            let p = std::env::temp_dir().join(format!("antler-scan-{}-{name}", std::process::id()));
            let _ = fs::remove_dir_all(&p);
            fs::create_dir_all(&p).unwrap();
            Tmp(p)
        }

        fn root(&self) -> String {
            self.0.to_string_lossy().into_owned()
        }

        fn dir(&self, rel: &str) {
            fs::create_dir_all(self.0.join(rel)).unwrap();
        }
    }

    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn names(r: &RepoScanResult) -> Vec<&str> {
        r.repos.iter().map(|x| x.name.as_str()).collect()
    }

    #[test]
    fn git_이_있는_하위_폴더만_이름순으로_고르고_숨김과_파일은_뺀다() {
        let t = Tmp::new("basic");
        for d in ["beta/.git", "Alpha/.git", "gamma", ".hidden/.git", "delta/.git"] {
            t.dir(d);
        }
        fs::write(t.0.join("file.txt"), "x").unwrap();
        // `.git` 이 파일인 폴더(워크트리·서브모듈)도 저장소 후보다
        t.dir("wt");
        fs::write(t.0.join("wt").join(".git"), "gitdir: ../x").unwrap();
        let r = scan_local_repos(&t.root()).unwrap();
        assert_eq!(names(&r), ["Alpha", "beta", "delta", "wt"]);
        assert!(!r.is_repo && !r.truncated);
        assert_eq!(r.path, t.root());
        assert_eq!(r.repos[1].path, win32_path::join(&[&t.root(), "beta"]));
    }

    #[test]
    fn 폴더가_상한을_넘으면_앞쪽만_보고_truncated() {
        let t = Tmp::new("cap");
        for i in 0..MAX_DIRS + 5 {
            t.dir(&format!("d{i:04}"));
        }
        let r = scan_local_repos(&t.root()).unwrap();
        assert!(r.truncated);
        assert!(r.repos.is_empty(), "상한 안의 폴더에 .git 이 없다");
        // 정확히 상한이면 잘리지 않는다
        let t2 = Tmp::new("cap-exact");
        for i in 0..MAX_DIRS {
            t2.dir(&format!("d{i:04}"));
        }
        assert!(!scan_local_repos(&t2.root()).unwrap().truncated);
    }

    #[test]
    fn 없는_폴더는_오류() {
        let t = Tmp::new("missing");
        assert!(scan_local_repos(&format!("{}\\nope", t.root())).is_err());
    }

    #[test]
    fn root_가_원격_공유면_읽지_않고_거부한다() {
        for root in [r"\\host\share", "//host/share", r"\??\UNC\host\share"] {
            assert!(scan_local_repos(root).is_err(), "{root}");
        }
    }

    #[test]
    fn 이름순은_대소문자를_무시하고_같으면_소문자가_앞이며_기호가_숫자와_글자보다_앞이다() {
        let mut v = vec!["b", "A", "a", "_x", "1x", "-y", "B", "a10", "a2"];
        v.sort_by(|a, b| locale_compare(a, b));
        assert_eq!(v, ["_x", "-y", "1x", "a", "A", "a10", "a2", "b", "B"]);
    }

    #[test]
    fn 이름순은_전순서다_키가_같은_다른_글자와_제어_문자가_있어도_추이적이고_패닉하지_않는다() {
        // 키가 같은 서로 다른 글자(İ·i·I)·키에서 빠지는 제어 문자(\x01)·대소문자 변형이 섞인 반례들
        let names = ["i", "I", "İ", "ı", "a\u{1}b", "ab", "aB", "Ab", "AB", "a\u{1}B", "\u{1}", "", "x\u{7f}", "x", "X", "İi", "iİ", "Ii", "iI", "ǅ", "ǆ", "Ǆ"];
        for a in names {
            assert_eq!(locale_compare(a, a), Ordering::Equal, "{a:?}");
            for b in names {
                assert_eq!(locale_compare(a, b), locale_compare(b, a).reverse(), "반대칭 {a:?} {b:?}");
                for c in names {
                    if locale_compare(a, b) != Ordering::Greater && locale_compare(b, c) != Ordering::Greater {
                        assert_ne!(locale_compare(a, c), Ordering::Greater, "추이성 {a:?} {b:?} {c:?}");
                    }
                }
            }
        }
        // 서로 다른 문자열은 절대 같지 않다(최종 결정자) — 어떤 순서로 섞어도 같은 결과
        let mut forward: Vec<&str> = names.to_vec();
        let mut backward: Vec<&str> = names.iter().rev().copied().collect();
        forward.sort_by(|a, b| locale_compare(a, b));
        backward.sort_by(|a, b| locale_compare(a, b));
        assert_eq!(forward, backward);
        forward.dedup();
        assert_eq!(forward.len(), names.len());
    }

    fn listing(path: &str, is_repo: bool, dirs: &[(&str, Option<bool>)], truncated: bool) -> RemoteDirListing {
        RemoteDirListing {
            path: path.into(),
            parent: Some("/".into()),
            is_repo,
            dirs: dirs.iter().map(|(n, r)| RemoteDirEntry { name: (*n).into(), is_repo: *r }).collect(),
            truncated,
        }
    }

    #[test]
    fn 원격_목록에서_저장소_표시가_된_하위_폴더를_고른다() {
        let r = scan_from_listing(&listing("/home/me/work", false, &[("a", Some(true)), ("b", Some(false)), ("c", Some(true))], false));
        assert_eq!(r.repos, [ScanRepo { name: "a".into(), path: "/home/me/work/a".into() }, ScanRepo { name: "c".into(), path: "/home/me/work/c".into() }]);
        assert!(!r.truncated && !r.is_repo);
        // 확인하지 않은 항목이 있거나 목록이 잘렸으면 truncated
        assert!(scan_from_listing(&listing("/w", false, &[("a", None)], false)).truncated);
        assert!(scan_from_listing(&listing("/w", false, &[("a", Some(false))], true)).truncated);
        // 폴더 자체가 저장소면 하위는 찾지 않는다
        let own = scan_from_listing(&listing("/w", true, &[("a", Some(true))], false));
        assert!(own.is_repo && own.repos.is_empty());
        assert_eq!(scan_from_listing(&listing("/", false, &[("a", Some(true))], false)).repos[0].path, "/a");
    }

    #[test]
    fn posix_join_은_node_와_같이_정리한다() {
        assert_eq!(posix_join("/a/b", "c"), "/a/b/c");
        assert_eq!(posix_join("/a/b/", "c"), "/a/b/c");
        assert_eq!(posix_join("/", "c"), "/c");
        assert_eq!(posix_join("/a//b/.", "c"), "/a/b/c");
        assert_eq!(posix_join("/a/b", ".."), "/a");
        assert_eq!(posix_join("/a", "../.."), "/");
        assert_eq!(posix_join("", "c"), "c");
        assert_eq!(posix_join("", ""), ".");
        assert_eq!(posix_join("a", ".."), ".");
    }
}
