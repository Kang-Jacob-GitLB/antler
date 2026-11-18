//! git remote URL → GitHub owner/repo(`src/main/github/remoteUrl.ts` 의 이식, Phase 6a PR 8). github.com 만 다룬다
//! (GitHub Enterprise Server 같은 자체 호스트는 범위 밖 — 호스트마다 API 주소·인증이 달라 별도 설정이 필요하므로 github.com 이 아니면 없는 것으로 취급한다).
//!
//! TS 는 WHATWG `new URL` 을 쓰고 Rust 는 `url` 크레이트(같은 명세)를 쓴다. 손으로 옮긴 부분(scp 형식·공백·경로 나누기)과 두 URL 구현의 차이는
//! `tests/golden/parsers/githubRemote__*`(TS 가 만든 골든 — `tests/golden.rs`)가 맡는다. owner/repo 규칙은 [`Slug::parse`] 한 곳이다(`OWNER_RE`·`REPO_RE`·`.`·`..`).

use serde::Deserialize;
use url::Url;

use super::ports::GhRepoRef;
use super::queries::Slug;
use crate::git::scope::is_js_space;

const HOSTS: [&str; 2] = ["github.com", "www.github.com"];

/// `git config` 로 읽은 remote 하나(이름·fetch URL)
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, serde::Serialize)]
pub struct Remote {
    pub name: String,
    pub url: String,
}

/// 스킴별 기본 포트. 명시했다면 이 값이어야 github.com 으로 본다(`ssh://git@github.com:22/…` 허용)
fn default_port(scheme: &str) -> Option<u16> {
    match scheme {
        "http" => Some(80),
        "https" => Some(443),
        "ssh" => Some(22),
        "git" => Some(9418),
        _ => None,
    }
}

/// 끝의 `/` 는 허용, 중간의 빈 세그먼트·추가 세그먼트(`o/r/tree/main`)는 거부한다
fn to_slug(path: &str) -> Option<(String, String)> {
    let mut segs: Vec<&str> = path.trim_end_matches('/').split('/').collect();
    if segs[0].is_empty() {
        segs.remove(0);
    }
    let [owner, repo] = segs[..] else { return None };
    let repo = repo.strip_suffix(".git").unwrap_or(repo);
    Slug::parse(owner, repo).map(|s| (s.owner().to_string(), s.repo().to_string()))
}

/// `^[a-z][a-z0-9+.-]*:\/\/`(대소문자 무시)
fn has_scheme_prefix(s: &str) -> bool {
    let Some(end) = s.find(':') else { return false };
    let scheme = &s[..end];
    let mut chars = scheme.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic()) && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-')) && s[end..].starts_with("://")
}

/// scp 형식 `[user@]host:path` 를 `^(?:[^@/\s]+@)?([^:/\s]+):(.+)$` 와 같게 나눈다(공백은 호출자가 이미 걸렀다).
/// 정규식은 user 그룹을 먼저 시도하고 맞으면 그 결과를 쓴다(호스트가 github.com 이 아니어도 그룹 없는 해석으로 돌아가지 않는다)
fn split_scp(s: &str) -> Option<(&str, &str)> {
    let host_path = |rest: &'_ str| -> Option<(usize, usize)> {
        // host = [^:/\s]+ 다음에 ':' 가 와야 하고, path = (.+) 는 비어 있지 않다
        let colon = rest.find(':')?;
        let host = &rest[..colon];
        (!host.is_empty() && !host.contains('/') && colon + 1 < rest.len()).then_some((colon, colon + 1))
    };
    if let Some(at) = s.find('@') {
        let user = &s[..at];
        if !user.is_empty() && !user.contains('/') {
            let rest = &s[at + 1..];
            if let Some((c, p)) = host_path(rest) {
                return Some((&rest[..c], &rest[p..]));
            }
        }
    }
    host_path(s).map(|(c, p)| (&s[..c], &s[p..]))
}

/// GitHub remote URL 을 해석한다. 지원 형태: `https://github.com/o/r(.git)` · `http(s)://user:pass@github.com/o/r`(자격 증명은 버린다) ·
/// `git@github.com:o/r.git` · `ssh://git@github.com(:22)/o/r.git` · `git://github.com/o/r` · `www.github.com` · 끝 슬래시
pub fn parse_github_remote(url: &str) -> Option<(String, String)> {
    let s = url.trim_matches(is_js_space);
    if s.is_empty() || s.chars().any(|c| is_js_space(c) || c == '\0') {
        return None;
    }
    if has_scheme_prefix(s) {
        let u = Url::parse(s).ok()?;
        let default = default_port(u.scheme())?;
        if !HOSTS.contains(&u.host_str()?.to_lowercase().as_str()) {
            return None;
        }
        // URL 은 특수 스킴의 기본 포트를 비워 주므로 비어 있지 않으면 스킴의 기본 포트와 다른 포트다
        if u.port().is_some_and(|p| p != default) {
            return None;
        }
        return to_slug(u.path());
    }
    let (host, path) = split_scp(s)?;
    HOSTS.contains(&host.to_lowercase().as_str()).then(|| to_slug(path)).flatten()
}

/// 추론 우선순위: origin → upstream → 목록에서 처음 나오는 GitHub remote
pub fn pick_github_remote(remotes: &[Remote]) -> Option<GhRepoRef> {
    let parsed: Vec<(&str, (String, String))> = remotes.iter().filter_map(|r| parse_github_remote(&r.url).map(|slug| (r.name.as_str(), slug))).collect();
    let hit = parsed.iter().find(|(n, _)| *n == "origin").or_else(|| parsed.iter().find(|(n, _)| *n == "upstream")).or_else(|| parsed.first())?;
    Some(GhRepoRef { owner: hit.1 .0.clone(), repo: hit.1 .1.clone(), remote: hit.0.to_string() })
}

/// 모든 GitHub remote(origin·upstream 등)의 저장소. 같은 저장소(대소문자 무시)는 먼저 나온 remote 하나만 둔다
pub fn pick_github_remotes(remotes: &[Remote]) -> Vec<GhRepoRef> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for r in remotes {
        let Some((owner, repo)) = parse_github_remote(&r.url) else { continue };
        if !seen.insert(format!("{owner}/{repo}").to_lowercase()) {
            continue;
        }
        out.push(GhRepoRef { owner, repo, remote: r.name.clone() });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn remote(name: &str, url: &str) -> Remote {
        Remote { name: name.into(), url: url.into() }
    }

    #[test]
    fn scp_분해는_정규식과_같다() {
        assert_eq!(split_scp("git@github.com:o/r"), Some(("github.com", "o/r")));
        assert_eq!(split_scp("github.com:o/r"), Some(("github.com", "o/r")));
        // user 그룹은 첫 `@` 까지다. 호스트(`[^:/\s]+`)는 `@` 를 품을 수 있어 "b@github.com" 이 되고 github.com 이 아니다
        assert_eq!(split_scp("a@b@github.com:o/r"), Some(("b@github.com", "o/r")));
        assert_eq!(split_scp("x:y@github.com:o/r"), Some(("github.com", "o/r")));
        assert_eq!(split_scp("git@github.com:"), None);
        assert_eq!(split_scp("git@github.com"), None);
        assert_eq!(split_scp("git@github.com/o:r"), None);
        assert_eq!(split_scp("@github.com:o/r"), Some(("@github.com", "o/r")));
        assert_eq!(split_scp(":o/r"), None);
    }

    #[test]
    fn origin_upstream_첫_remote_순이다() {
        let r = [remote("fork", "git@github.com:a/b.git"), remote("upstream", "git@github.com:c/d.git"), remote("origin", "git@github.com:e/f.git")];
        assert_eq!(pick_github_remote(&r), Some(GhRepoRef { owner: "e".into(), repo: "f".into(), remote: "origin".into() }));
        assert_eq!(pick_github_remote(&r[..2]).map(|x| x.remote), Some("upstream".to_string()));
        assert_eq!(pick_github_remote(&[remote("x", "git@gitlab.com:a/b.git")]), None);
        assert_eq!(pick_github_remotes(&[remote("a", "git@github.com:O/R.git"), remote("b", "https://github.com/o/r")]).len(), 1);
    }
}
