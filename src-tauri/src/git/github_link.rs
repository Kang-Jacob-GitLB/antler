//! GitHub 원격 전용 저장소의 링크(owner/repo)·캐시 경로 순수 함수(`src/main/git/githubCache.ts` 23-117 의 이식, Phase 6b PR 1).
//!
//! 저장소 쓰기 예외의 유일한 경로(앱 캐시)를 가리키는 값이라 **판정 하나가 여러 곳을 지킨다**: 설정을 읽을 때 레코드를 다시 만들고(`config::load`),
//! 이후 PR 들이 실행 환경(lazy fetch·토큰)·삭제를 "앱 캐시 안의 우리 저장소"일 때만 열 때 모두 [`github_cache_slug`] 로 판정한다.
//! 입력은 renderer 나 설정 파일에서 온 값이다 — 이름 규칙을 더 느슨하게 하지 않는다.
//!
//! 경로는 항상 **win32 규칙**(`ssh/win32_path.rs` — 골든이 플랫폼마다 달라지지 않게)이다. Antler 가 Windows 용이라 TS 도 거기서 돈다.
//! 알려진 차이(TS 보다 엄격한 쪽): [`github_cache_slug`] 는 절대 경로가 아닌 입력(드라이브 없는 `\foo`·상대 경로)을 `None` 으로 돌린다.
//! TS 의 `resolve` 는 작업 디렉터리를 붙이지만, 캐시 기준 폴더와 레코드 경로는 항상 드라이브 절대 경로(또는 UNC)다.
use crate::git::exec::ssh::win32_path;
use crate::net::json::Json;

/// 앱 캐시 아래 저장소 폴더 이름의 접미사
const CACHE_SUFFIX: &str = ".git";
/// GitHub 의 한도: 사용자·조직 이름 39자, 저장소 이름 100자
const OWNER_MAX: usize = 39;
const REPO_MAX: usize = 100;
const BRANCH_MAX: usize = 200;
/// 한 번에 추가할 수 있는 원격 저장소 수
pub const MAX_ADD_GITHUB: usize = 200;
/// 설정의 github 소스 id(`config::source_for`)
pub const GITHUB_SOURCE_ID: &str = "github";
pub const GITHUB_ORIGIN: &str = "https://github.com";

/// 받아 오는 GitHub 저장소(`GithubRepoLink`)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GithubLink {
    pub owner: String,
    pub repo: String,
    /// 쓸 수 있는 이름(`is_safe_branch`)일 때만 담는다
    pub default_branch: Option<String>,
}

impl GithubLink {
    /// 설정 파일의 레코드 모양(`{owner, repo, defaultBranch?}` — 키 순서가 TS 와 같다)
    pub fn to_json(&self) -> Json {
        let mut o = vec![("owner".to_string(), Json::Str(self.owner.clone())), ("repo".to_string(), Json::Str(self.repo.clone()))];
        if let Some(b) = &self.default_branch {
            o.push(("defaultBranch".to_string(), Json::Str(b.clone())));
        }
        Json::Obj(o)
    }
}

/// `/^[A-Za-z0-9_.-]+$/`
fn name_chars_ok(v: &str) -> bool {
    !v.is_empty() && v.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
}

/// Windows 예약 장치 이름(확장자가 붙어도 예약이다: con.git). 폴더 이름이 되므로 모든 OS 에서 거부해 결과를 같게 한다
fn is_win_reserved(stem: &str) -> bool {
    let l = stem.to_ascii_lowercase();
    matches!(l.as_str(), "con" | "prn" | "aux" | "nul")
        || ((l.starts_with("com") || l.starts_with("lpt")) && l.len() == 4 && l.as_bytes()[3].is_ascii_digit())
}

fn check_name(v: Option<&Json>, what: &str, max: usize) -> Result<String, String> {
    let bad = || format!("잘못된 {what}입니다");
    let Some(Json::Str(v)) = v else { return Err(bad()) };
    // 정규식이 ASCII 만 받으므로 바이트 길이가 UTF-16 길이와 같다(비 ASCII 는 어차피 아래에서 거부)
    if v.is_empty() || v.len() > max || !name_chars_ok(v) {
        return Err(bad());
    }
    // . / .. 는 경로가 되고, - 로 시작하면 옵션처럼 보이며, 끝의 . 은 Windows 가 지워서 다른 폴더와 겹친다
    if v == "." || v == ".." || v.starts_with('-') || v.ends_with('.') || is_win_reserved(v.split('.').next().unwrap_or("")) {
        return Err(bad());
    }
    Ok(v.clone())
}

/// git 에 ref 이름으로 넘겨도 되는 기본 브랜치 이름인가(보수적으로: 영문·숫자·._/+- 만)
pub fn is_safe_branch(b: &str) -> bool {
    !b.is_empty()
        && b.len() <= BRANCH_MAX
        && b.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'/' | b'+' | b'-'))
        && !b.starts_with('-')
        && !b.starts_with('/')
        && !b.ends_with('/')
        && !b.ends_with('.')
        && !b.ends_with(".lock")
        && !b.contains("//")
        && !b.contains("..")
        // `/(^|\/)\./` — 점으로 시작하는 구성 요소
        && !b.split('/').any(|part| part.starts_with('.'))
}

/// owner/repo(와 기본 브랜치) 검증. 잘못되면 TS 와 같은 문구로 돌려준다.
/// 기본 브랜치는 부가 정보라 쓸 수 없는 값이면 버린다(첫 fetch 때 원격에 물어 알아낸다).
pub fn validate_github_link(input: Option<&Json>) -> Result<GithubLink, String> {
    // `!input || typeof input !== 'object'` — 배열도 객체다(필드가 없어 아래에서 걸린다)
    if !matches!(input, Some(Json::Obj(_) | Json::Arr(_))) {
        return Err("잘못된 저장소입니다".to_string());
    }
    let owner = check_name(input.and_then(|i| i.get("owner")), "소유자 이름", OWNER_MAX)?;
    let repo = check_name(input.and_then(|i| i.get("repo")), "저장소 이름", REPO_MAX)?;
    if repo.to_ascii_lowercase().ends_with(CACHE_SUFFIX) {
        return Err("잘못된 저장소 이름입니다".to_string());
    }
    let default_branch = match input.and_then(|i| i.get("defaultBranch")) {
        Some(Json::Str(b)) if is_safe_branch(b) => Some(b.clone()),
        _ => None,
    };
    Ok(GithubLink { owner, repo, default_branch })
}

/// 저장 파일을 읽을 때 쓰는 느슨한 형태: 검증에 실패하면 `None`
pub fn sanitize_github_link(input: Option<&Json>) -> Option<GithubLink> {
    validate_github_link(input).ok()
}

pub fn github_full_name(link: &GithubLink) -> String {
    format!("{}/{}", link.owner, link.repo)
}

/// 캐시 폴더. 대소문자만 다른 같은 저장소가 둘로 갈라지지 않게 소문자로 정한다
pub fn github_cache_path(base: &str, owner: &str, repo: &str) -> String {
    let repo_dir = format!("{}{CACHE_SUFFIX}", repo.to_ascii_lowercase());
    win32_path::join(&[base, &owner.to_ascii_lowercase(), &repo_dir])
}

/// 절대 경로의 해석: (루트, 구성 요소). 비교는 대소문자를 구분하지 않는다(Windows)
struct Resolved {
    root: String,
    parts: Vec<String>,
}

/// `path.win32.resolve(p)` 의 절대 경로 부분집합(드라이브 절대·UNC 만). 그 밖은 `None`
fn resolve_absolute(p: &str) -> Option<Resolved> {
    let n = win32_path::normalize(p);
    let segs: Vec<&str> = n.split('\\').filter(|s| !s.is_empty()).collect();
    if n.starts_with("\\\\") {
        // \\host\share\rest — 호스트·공유 이름이 모두 있어야 루트다
        let [host, share, rest @ ..] = segs.as_slice() else { return None };
        return Some(Resolved { root: format!("\\\\{host}\\{share}"), parts: rest.iter().map(|s| (*s).to_string()).collect() });
    }
    let b = n.as_bytes();
    if b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && b[2] == b'\\' {
        return Some(Resolved { root: n[..2].to_string(), parts: segs[1..].iter().map(|s| (*s).to_string()).collect() });
    }
    None
}

/// `path` 가 `base` 바로 아래 정확히 `<owner>/<repo>.git`(소문자) 모양이면 그 owner/repo(소문자)를, 아니면 `None`.
/// 실행 환경·삭제처럼 "앱 캐시 안의 우리 저장소"임을 확인해야 하는 곳이 모두 이 함수로 판정한다
/// (TS 는 `relative(resolve(base), resolve(path))` 가 두 구성 요소인지 본다 — 두 경로가 같은 루트에서 base 가 path 의 접두일 때만 같은 결과다).
pub fn github_cache_slug(base: &str, path: &str) -> Option<(String, String)> {
    if path.contains('\0') {
        return None;
    }
    let from = resolve_absolute(base)?;
    let to = resolve_absolute(path)?;
    if from.root.to_lowercase() != to.root.to_lowercase() {
        return None;
    }
    if to.parts.len() != from.parts.len() + 2 {
        return None;
    }
    if !from.parts.iter().zip(&to.parts).all(|(a, b)| a.to_lowercase() == b.to_lowercase()) {
        return None;
    }
    let owner = &to.parts[from.parts.len()];
    let repo_dir = &to.parts[from.parts.len() + 1];
    let repo = repo_dir.strip_suffix(CACHE_SUFFIX).filter(|r| !r.is_empty())?;
    check_name(Some(&Json::Str(owner.clone())), "소유자 이름", OWNER_MAX).ok()?;
    check_name(Some(&Json::Str(repo.to_string())), "저장소 이름", REPO_MAX).ok()?;
    (*owner == owner.to_lowercase() && repo == repo.to_lowercase()).then(|| (owner.clone(), repo.to_string()))
}

pub fn is_github_cache_path(base: &str, path: &str) -> bool {
    github_cache_slug(base, path).is_some()
}

/// 캐시 경로가 이 레코드(owner/repo)의 것인가
pub fn cache_path_matches_link(base: &str, path: &str, owner: &str, repo: &str) -> bool {
    github_cache_slug(base, path).is_some_and(|(o, r)| o == owner.to_lowercase() && r == repo.to_lowercase())
}

/// origin 의 URL. 초기화와 fetch 앞의 정확 일치 검사가 같은 함수를 쓰므로 둘이 어긋날 수 없다
pub fn github_remote_url(owner: &str, repo: &str, origin: &str) -> String {
    format!("{origin}/{owner}/{repo}{CACHE_SUFFIX}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::json::parse;

    fn link(s: &str) -> Result<GithubLink, String> {
        validate_github_link(Some(&parse(s).unwrap()))
    }

    #[test]
    fn 이름_경계를_거른다() {
        assert!(link(r#"{"owner":"a","repo":"b"}"#).is_ok());
        assert!(link(&format!(r#"{{"owner":"{}","repo":"b"}}"#, "a".repeat(39))).is_ok());
        assert!(link(&format!(r#"{{"owner":"{}","repo":"b"}}"#, "a".repeat(40))).is_err());
        assert!(link(&format!(r#"{{"owner":"a","repo":"{}"}}"#, "b".repeat(100))).is_ok());
        for bad in ["-a", "a.", "con", "CON.txt", "lpt1", "..", ".", "a b", "한"] {
            assert!(link(&format!(r#"{{"owner":"{bad}","repo":"b"}}"#)).is_err(), "{bad}");
        }
        assert!(link(r#"{"owner":"a","repo":"B.GIT"}"#).is_err());
        assert!(link("null").is_err());
        assert_eq!(link(r#"{"owner":"a","repo":"b","defaultBranch":"-x"}"#).unwrap().default_branch, None);
    }

    #[test]
    fn 캐시_경로와_slug_왕복() {
        let p = github_cache_path("C:\\golden\\cache", "Acme", "Web");
        assert_eq!(p, "C:\\golden\\cache\\acme\\web.git");
        assert_eq!(github_cache_slug("C:\\golden\\cache", &p), Some(("acme".into(), "web".into())));
        assert_eq!(github_cache_slug("C:\\golden\\cache", "C:\\golden\\cache\\Acme\\web.git"), None);
        assert_eq!(github_cache_slug("C:\\golden\\cache", "C:\\GOLDEN\\Cache\\acme\\web.git"), Some(("acme".into(), "web".into())));
        assert_eq!(github_cache_slug("C:\\golden\\cache", "C:\\golden\\cache\\acme\\web.git\\x"), None);
        assert_eq!(github_cache_slug("C:\\golden\\cache", "C:\\golden\\other\\acme\\web.git"), None);
        assert_eq!(github_cache_slug("C:\\golden\\cache", "relative\\acme\\web.git"), None);
    }
}
