//! 작업 트리 경로가 symlink·정션·8.3 짧은 이름을 지나 저장소 밖(또는 `.git` 안)으로 새는지 로컬 파일 시스템에서 확인한다(`src/main/git/linkGuard.ts`).
//!
//! Git for Windows 의 `status -uall` 은 정션(`mklink /J j .git`)을 디렉터리처럼 따라 들어가 `j/config` 를 보고하고, `diff --no-index`·작업 트리 diff 는
//! 그 경로로 `.git/config` 나 저장소 밖 파일을 읽는다. 경로 문자열의 `.git` 비교만으로는 8.3 이름(`GIT~1`)도 알아보지 못한다 — 그래서 열기 직전에 실제 경로를 본다.
//!
//! - `assert_no_link_escape`: 루트부터 구성 요소마다 `symlink_metadata`(lstat). 링크면 거부(마지막 구성 요소는 `allow_leaf_link` 일 때만 허용 —
//!   추적 중인 symlink 는 git 이 링크 대상 문자열만 비교해 내용을 읽지 않는다). 그다음 `assert_inside_root`.
//! - `assert_inside_root`: `canonicalize`(Windows 에서는 `GetFinalPathNameByHandle` 이라 8.3 이름이 긴 이름으로 펴진다) 결과가 저장소 루트의 실제 경로 안이고
//!   `.git` 구성 요소가 없어야 한다. 비교 자체는 파일 시스템을 보지 않는 순수 함수 [`check_real_path`] 다.
//!
//! 없는 경로(NotFound·NotADirectory)는 막지 않는다 — 읽을 파일이 없으니 git 이 스스로 실패하거나 삭제로 보여 준다.
//! 확인과 git 의 읽기 사이의 경쟁(TOCTOU)은 막지 못한다. WSL·SSH 저장소에서는 호출하지 않는다(Rust 가 그 파일 시스템을 볼 수 없다).
//! 오류 문구는 TS 와 같은 `invalid path` 다.

use std::fs;
use std::io;
use std::path::{Component, Path};

const INVALID: &str = "invalid path";

/// 구성 요소 비교용 문자열. Windows 는 대소문자를 구분하지 않는다(Node 의 win32 `path.relative` 와 같다)
fn key(c: Component<'_>) -> String {
    let s = c.as_os_str().to_string_lossy().into_owned();
    if cfg!(windows) {
        s.to_lowercase()
    } else {
        s
    }
}

/// 이미 `canonicalize` 한 두 경로를 비교하는 순수 함수: `real` 이 `root` 와 같거나 그 아래이고, 아래 구성 요소 중 `.git`(대소문자 무시)이 없어야 한다.
pub fn check_real_path(root: &Path, real: &Path) -> Result<(), &'static str> {
    let root_keys: Vec<String> = root.components().map(key).collect();
    let real_keys: Vec<String> = real.components().map(key).collect();
    if real_keys.len() < root_keys.len() || real_keys[..root_keys.len()] != root_keys[..] {
        return Err(INVALID);
    }
    if real.components().skip(root_keys.len()).any(|c| matches!(c, Component::ParentDir) || c.as_os_str().to_string_lossy().to_lowercase() == ".git") {
        return Err(INVALID);
    }
    Ok(())
}

fn is_not_found(e: &io::Error) -> bool {
    matches!(e.kind(), io::ErrorKind::NotFound | io::ErrorKind::NotADirectory)
}

/// `target`(저장소 안이라고 주장하는 경로)의 실제 경로가 저장소 루트 실제 경로 안이고 `.git` 구성 요소가 없는지 확인한다(없는 경로는 통과).
pub fn assert_inside_root(repo_path: &Path, target: &Path) -> Result<(), &'static str> {
    let canon = |p: &Path| fs::canonicalize(p);
    match (canon(repo_path), canon(target)) {
        (Ok(root), Ok(real)) => check_real_path(&root, &real),
        (Err(e), _) | (_, Err(e)) if is_not_found(&e) => Ok(()),
        _ => Err(INVALID),
    }
}

/// 저장소 루트 아래 `rel` 이 실제로 저장소 안의 일반 경로인지 확인한다. 아니면 `invalid path`.
/// `rel` 의 구분자는 `/`·`\` 둘 다이고 빈 구성 요소는 건너뛴다. `.`·`..`(과 Windows 의 `:` 가 든 구성 요소 — 드라이브 상대 경로·대체 데이터 스트림)는
/// 어휘적으로 걷어 내지 않고 **거부**한다 — TS 는 `path.join` 이 걷어 내므로 `j/.`(`j` 가 정션)에서 `j` 가 마지막 구성 요소가 아니어서 거부되는데,
/// `.` 를 건너뛰면 `j` 가 마지막이 되어 `allow_leaf_link`(unstaged)에서 통과하기 때문이다. `assert_rel_path` 는 `.`·`:` 를 통과시키므로(`..` 만 거른다) 이 차이는 닿을 수 있는 입력이다.
pub fn assert_no_link_escape(repo_path: &Path, rel: &str, allow_leaf_link: bool) -> Result<(), &'static str> {
    let parts: Vec<&str> = rel.split(['/', '\\']).filter(|p| !p.is_empty()).collect();
    let mut cur = repo_path.to_path_buf();
    let mut leaf_is_link = false;
    for (i, part) in parts.iter().enumerate() {
        if matches!(*part, "." | "..") || (cfg!(windows) && part.contains(':')) {
            return Err(INVALID);
        }
        cur.push(part);
        match fs::symlink_metadata(&cur) {
            Ok(md) => {
                // Windows 의 정션(마운트 지점)도 Rust 는 symlink 로 보고한다
                if md.file_type().is_symlink() {
                    if allow_leaf_link && i == parts.len() - 1 {
                        leaf_is_link = true;
                    } else {
                        return Err(INVALID);
                    }
                }
            }
            Err(e) if is_not_found(&e) => return Ok(()),
            Err(_) => return Err(INVALID),
        }
    }
    if leaf_is_link {
        cur.pop();
    }
    assert_inside_root(repo_path, &cur)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[test]
    fn 실제_경로_비교는_루트_안과_git_구성_요소만_본다() {
        let root = if cfg!(windows) { p(r"\\?\C:\Users\x\repo") } else { p("/home/x/repo") };
        let at = |rel: &str| root.join(rel);
        assert!(check_real_path(&root, &root).is_ok());
        assert!(check_real_path(&root, &at("src/a.txt")).is_ok());
        assert!(check_real_path(&root, &at("src/.gitignore")).is_ok(), ".git 이 아니라 .gitignore");
        assert!(check_real_path(&root, &at("sub/.github/x")).is_ok());
        for bad in [".git", ".git/config", "a/.GIT/x", "a/.Git"] {
            assert_eq!(check_real_path(&root, &at(bad)), Err(INVALID), "{bad}");
        }
        let outside = if cfg!(windows) { p(r"\\?\C:\Users\x\repo-other\a") } else { p("/home/x/repo-other/a") };
        assert_eq!(check_real_path(&root, &outside), Err(INVALID), "접두 문자열만 같은 형제 디렉터리");
        assert_eq!(check_real_path(&root, root.parent().unwrap()), Err(INVALID));
    }

    #[cfg(windows)]
    #[test]
    fn 윈도우는_대소문자를_가리지_않는다() {
        assert!(check_real_path(Path::new(r"\\?\C:\Repo"), Path::new(r"\\?\C:\repo\a")).is_ok());
        assert_eq!(check_real_path(Path::new(r"\\?\C:\Repo"), Path::new(r"\\?\C:\repo\.GIT\config")), Err(INVALID));
    }

    /// 시험용 임시 디렉터리(새 크레이트 없이): 프로세스 ID + 이름으로 구분하고 끝에 지운다
    struct Tmp(PathBuf);

    impl Tmp {
        fn new(name: &str) -> Self {
            let d = std::env::temp_dir().join(format!("antler-link-guard-{}-{name}", std::process::id()));
            let _ = fs::remove_dir_all(&d);
            fs::create_dir_all(&d).unwrap();
            Tmp(d)
        }
    }

    impl Drop for Tmp {
        fn drop(&mut self) {
            // 정션(`repo/` 바로 아래에 만든다)은 대상을 따라가지 않고 링크만 지운다(Rust 의 remove_dir_all 도 같지만 한 번 더 먼저 지운다)
            if let Ok(rd) = fs::read_dir(self.0.join("repo")) {
                for e in rd.flatten() {
                    if e.file_type().is_ok_and(|t| t.is_symlink()) {
                        let _ = fs::remove_dir(e.path());
                    }
                }
            }
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn repo_with_git(t: &Tmp) -> PathBuf {
        let repo = t.0.join("repo");
        fs::create_dir_all(repo.join(".git")).unwrap();
        fs::write(repo.join(".git/config"), "[core]\n").unwrap();
        fs::create_dir_all(repo.join("sub/deep")).unwrap();
        fs::write(repo.join("sub/deep/ok.txt"), "x").unwrap();
        fs::write(repo.join("plain.txt"), "x").unwrap();
        repo
    }

    #[test]
    fn 일반_경로와_없는_경로는_통과하고_git_안은_거부한다() {
        let t = Tmp::new("plain");
        let repo = repo_with_git(&t);
        for rel in ["plain.txt", "sub/deep/ok.txt", "sub\\deep", "없는/경로", "plain.txt/not-a-dir", ""] {
            assert_eq!(assert_no_link_escape(&repo, rel, false), Ok(()), "{rel}");
        }
        // `.`·`..`·`:` 구성 요소는 걷어 내지 않고 거부한다(TS 의 path.join 은 걷어 낸다 — 위 함수 설명)
        for rel in ["./sub", "sub/.", "sub/../plain.txt", "..\\other"] {
            assert_eq!(assert_no_link_escape(&repo, rel, false), Err(INVALID), "{rel}");
        }
        #[cfg(windows)]
        for rel in ["C:foo", "a:b"] {
            assert_eq!(assert_no_link_escape(&repo, rel, false), Err(INVALID), "{rel}");
        }
        assert_eq!(assert_no_link_escape(&repo, ".git/config", false), Err(INVALID));
        assert_eq!(assert_no_link_escape(&repo, "sub/../.git/config", false), Err(INVALID));
        assert_eq!(assert_no_link_escape(&repo, "..\\other", false), Err(INVALID));
        assert_eq!(assert_inside_root(&repo, &repo.join("sub")), Ok(()));
        assert_eq!(assert_inside_root(&repo, &repo.join("있다가/없음")), Ok(()));
        assert_eq!(assert_inside_root(&repo, &repo.join(".git")), Err(INVALID));
        assert_eq!(assert_inside_root(&repo, &t.0), Err(INVALID), "저장소 밖");
    }

    /// 정션은 `cmd /C mklink /J` 로만 만들 수 있다(std 에 API 가 없다). 시험에서만 자식 프로세스를 띄운다
    #[cfg(windows)]
    #[allow(clippy::disallowed_methods)]
    fn make_junction(link: &Path, target: &Path) {
        let out = std::process::Command::new("cmd").args(["/C", "mklink", "/J"]).arg(link).arg(target).output().expect("cmd 실행");
        assert!(out.status.success(), "mklink /J 실패: {}", String::from_utf8_lossy(&out.stdout));
    }

    /// tests/wip.test.ts 'Node 는 정션을 symlink 로 본다' 와 같은 전제: Rust 의 `symlink_metadata` 도 정션을 링크로 보고한다
    #[cfg(windows)]
    #[test]
    fn 정션을_symlink_로_본다() {
        let t = Tmp::new("junction-kind");
        let repo = repo_with_git(&t);
        let outside = t.0.join("outside");
        fs::create_dir_all(&outside).unwrap();
        make_junction(&repo.join("gitj"), &repo.join(".git"));
        make_junction(&repo.join("outj"), &outside);
        for j in ["gitj", "outj"] {
            assert!(fs::symlink_metadata(repo.join(j)).unwrap().file_type().is_symlink(), "{j}");
        }
    }

    #[cfg(windows)]
    #[test]
    fn 정션을_symlink_로_보고_정션_아래_경로를_거부한다() {
        let t = Tmp::new("junction");
        let repo = repo_with_git(&t);
        let outside = t.0.join("outside");
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("secret.txt"), "s").unwrap();
        make_junction(&repo.join("jgit"), &repo.join(".git"));
        make_junction(&repo.join("jout"), &outside);
        make_junction(&repo.join("jsub"), &repo.join("sub"));

        assert!(fs::symlink_metadata(repo.join("jgit")).unwrap().file_type().is_symlink(), "std 가 정션을 symlink 로 보고한다");
        for rel in ["jgit", "jgit/config", "jout/secret.txt", "jout", "jsub/deep/ok.txt", "jgit/.", "jout/."] {
            for leaf in [false, true] {
                // 마지막 구성 요소가 링크여도 허용하는 경우(`allow_leaf_link`)는 링크 자체를 읽지 않는 추적 symlink 용이다.
                // 링크 아래로 더 들어가는 경로는 어느 쪽이든 거부다
                let r = assert_no_link_escape(&repo, rel, leaf);
                let leaf_only = leaf && !rel.contains('/');
                if leaf_only {
                    // 링크 자체를 가리키는 경로: 링크가 놓인 디렉터리(저장소 안)만 확인한다
                    assert_eq!(r, Ok(()), "{rel} leaf={leaf}");
                } else {
                    assert_eq!(r, Err(INVALID), "{rel} leaf={leaf}");
                }
            }
        }
        // 정션 옆의 정상 파일·하위 디렉터리 파일은 계속 열린다
        for rel in ["plain.txt", "sub/deep/ok.txt"] {
            assert_eq!(assert_no_link_escape(&repo, rel, false), Ok(()), "{rel}");
        }
        // assert_inside_root 는 정션을 따라가 실제 위치를 본다(저장소 밖·.git 안이면 거부, 저장소 안이면 통과)
        assert_eq!(assert_inside_root(&repo, &repo.join("jout/secret.txt")), Err(INVALID));
        assert_eq!(assert_inside_root(&repo, &repo.join("jgit/config")), Err(INVALID));
        assert_eq!(assert_inside_root(&repo, &repo.join("jsub/deep/ok.txt")), Ok(()));
    }

    /// 8.3 짧은 이름: 이 볼륨이 짧은 이름을 만드는 경우에만 의미가 있다(TS 시험의 `skipIf(!shortGit)` 와 같다)
    #[cfg(windows)]
    #[test]
    fn 짧은_이름은_긴_이름으로_펴져_git_안을_거부한다() {
        let t = Tmp::new("short");
        let repo = repo_with_git(&t);
        let short = repo.join("GIT~1").join("config");
        if !short.exists() {
            eprintln!("이 볼륨은 8.3 짧은 이름을 만들지 않아 건너뛴다");
            return;
        }
        // 문자열로는 `.git` 이 아니지만 실제 경로는 `.git\config` 다
        assert_eq!(assert_no_link_escape(&repo, "GIT~1/config", false), Err(INVALID));
        assert_eq!(assert_inside_root(&repo, &repo.join("GIT~1")), Err(INVALID));
    }
}
