//! 링크 가드(`fsguard`)의 구성 요소 단위 판정 시험: 폴더 찾기(`scan_local_repos_with`)와 루트 확인(`check_local_root`)이 **중간 구성 요소의 링크**
//! (`b -> a\x`, `a -> <원격>`)도 잡는지 본다. 원격 대상 링크는 만들지 않는다 — 판정 함수를 주입해 "이 대상은 원격"이라고 가정하고, 정션은 로컬 폴더끼리만 잇는다.
//! 정션은 `cmd /c mklink /J`(관리자 불필요)로 만들고 Drop 에서 정션부터 `remove_dir`(rmdir)로 지운다 — 정션을 따라 원본을 지우지 않는다.

#![cfg(windows)]

use std::fs;
use std::path::{Path, PathBuf};

use antler_lib::fsguard::{check_local_root, is_remote_link_target, links_stay_local, REMOTE_LINK_ROOT};
use antler_lib::repos::scan::{scan_local_repos, scan_local_repos_with};

struct Rig {
    root: PathBuf,
    junctions: Vec<PathBuf>,
}

impl Rig {
    fn new(tag: &str) -> Rig {
        let root = std::env::temp_dir().join(format!("antler-fsguard-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        Rig { root, junctions: Vec::new() }
    }

    fn p(&self, rel: &str) -> PathBuf {
        self.root.join(rel)
    }

    fn dir(&self, rel: &str) {
        fs::create_dir_all(self.p(rel)).unwrap();
    }

    fn junction(&mut self, rel: &str, target: &Path) -> PathBuf {
        let link = self.p(rel);
        // git 이 아니라 cmd 의 mklink 를 부른다(정션은 std 로 만들 수 없다). 시험 전용이라 clippy 의 spawn 금지를 푼다
        #[allow(clippy::disallowed_methods)]
        let out = std::process::Command::new("cmd").arg("/c").arg("mklink").arg("/J").arg(&link).arg(target).output().expect("cmd 실행");
        assert!(out.status.success(), "mklink /J 실패: {} {}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        assert!(fs::symlink_metadata(&link).unwrap().file_type().is_symlink(), "정션이 링크로 보여야 한다: {}", link.display());
        self.junctions.push(link.clone());
        link
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        for j in self.junctions.iter().rev() {
            let _ = fs::remove_dir(j);
        }
        if self.junctions.iter().all(|j| fs::symlink_metadata(j).is_err()) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}

/// `real/{.git, x/.git}`, `plain/.git`, `a -> real`, `b -> a\x`(중간 구성 요소 `a` 가 링크), `c -> plain`
fn layout(tag: &str) -> Rig {
    let mut r = Rig::new(tag);
    for d in ["real/.git", "real/x/.git", "plain/.git"] {
        r.dir(d);
    }
    let (real, plain) = (r.p("real"), r.p("plain"));
    let a = r.junction("a", &real);
    r.junction("b", &a.join("x"));
    r.junction("c", &plain);
    r
}

/// 대상이 `\real` 로 끝나면 원격으로 본다(= `a` 의 대상). `b` 의 대상(`…\a\x`)은 문자열만 보면 로컬이다
fn remote_if_real(t: &str) -> bool {
    t.to_lowercase().ends_with("\\real")
}

fn names(root: &str, is_remote: impl Fn(&str) -> bool) -> Vec<String> {
    scan_local_repos_with(root, is_remote).unwrap().repos.into_iter().map(|r| r.name).collect()
}

#[test]
fn 폴더_찾기는_중간_구성_요소의_링크가_원격이면_그_아래를_가리키는_링크도_건너뛴다() {
    let r = layout("scan");
    let root = r.root.to_string_lossy().into_owned();
    // 대조: 판정이 모두 로컬이면 정션을 따라가 저장소로 찾는다
    assert_eq!(names(&root, |_| false), ["a", "b", "c", "plain", "real"]);
    // a 의 대상이 원격이면 a 와 (a 를 지나는) b 가 빠진다. c 는 남는다
    assert_eq!(names(&root, remote_if_real), ["c", "plain", "real"]);
    // 운영 판정(드라이브 문자 경로는 로컬)으로는 모두 따라간다
    let default: Vec<String> = scan_local_repos(&root).unwrap().repos.into_iter().map(|r| r.name).collect();
    assert_eq!(default, ["a", "b", "c", "plain", "real"]);
}

#[test]
fn 대상_문자열만_보는_판정이면_b_를_놓친다는_것을_보여_주는_대조() {
    // `b` 의 read_link 대상은 `…\a\x` 라 문자열만 보면 로컬이다 — 구성 요소를 따라가야 `a` 의 링크가 보인다
    let r = layout("contrast");
    let b_target = fs::read_link(r.p("b")).unwrap();
    assert!(!remote_if_real(&b_target.to_string_lossy()), "{}", b_target.display());
    assert!(!links_stay_local(&r.p("b"), remote_if_real));
}

#[test]
fn 루트_확인은_루트가_또는_루트의_중간_구성_요소가_원격으로_이어지면_거부한다() {
    let r = layout("root");
    let s = |rel: &str| r.p(rel).to_string_lossy().into_owned();
    assert_eq!(check_local_root(&s("a"), remote_if_real), Err(REMOTE_LINK_ROOT.to_string()));
    assert_eq!(check_local_root(&s("b"), remote_if_real), Err(REMOTE_LINK_ROOT.to_string()), "b -> a\\x 의 중간 a");
    assert_eq!(check_local_root(&s("b\\없는\\하위"), remote_if_real), Err(REMOTE_LINK_ROOT.to_string()));
    // 로컬로 이어지는 루트·링크 없는 루트는 통과
    assert_eq!(check_local_root(&s("c"), remote_if_real), Ok(()));
    assert_eq!(check_local_root(&s("plain"), remote_if_real), Ok(()));
    assert_eq!(check_local_root(&s("a"), |_| false), Ok(()));
    // 운영 판정으로는 로컬 정션이라 통과한다
    assert_eq!(check_local_root(&s("b"), is_remote_link_target), Ok(()));
}
