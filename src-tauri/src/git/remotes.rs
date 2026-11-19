//! remote 목록·push 된 커밋 조회(Phase 6a PR 8). TS: `src/main/git/commands.ts` 의 `remotes`·`parseRemoteConfig`·`parsePushedShas` 와 `RepoService.pushedShasByAuthor` 의 git 호출.
//! `gh:repo`·Launchpad 의 remote 목록·아바타 404 대비 조회가 쓴다(라우터 `Router::gh_repo` 등). git 인자는 서브커맨드·옵션 상수 + 검증된 값 하나뿐이다.

use std::collections::HashSet;

use super::exec::{ExecError, GitExecutor, RunOpts};
use super::parsers::js_trim;
use super::scope::is_js_space;
use crate::github::remote_url::Remote;

/// 이메일 길이 상한(옛 `repoQuery.ts` 의 `MAX_EMAIL_LENGTH` — Rust 경로에서도 같은 값을 건다)
pub const MAX_EMAIL_LEN: usize = 320;
/// 해시 개수 상한(`MAX_LIMIT`)
pub const MAX_PUSHED_LIMIT: usize = 100;

/// `git config --null --get-regexp ^remote\..*\.url$`. TS 와 글자까지 같다
pub fn remotes_args() -> Vec<String> {
    ["config", "--null", "--get-regexp", "^remote\\..*\\.url$"].map(String::from).to_vec()
}

/// remote 이름과 fetch URL 목록. `git config --get-regexp` 는 remote 가 없으면 종료 코드 1 이라 빈 목록으로 본다.
/// 키는 `remote.<name>.url` — remote 이름에 점이 있을 수 있어(`my.fork`) 앞 `remote.` 와 마지막 `.url` 사이를 이름으로 쓴다
pub async fn remotes(exec: &GitExecutor, repo_path: &str, opts: &RunOpts) -> Result<Vec<Remote>, ExecError> {
    let args = remotes_args();
    let r = exec.run(repo_path, &args, opts).await?;
    if r.code == 1 {
        return Ok(Vec::new());
    }
    if r.code != 0 {
        let trimmed = js_trim(&r.stderr);
        let message = if trimmed.is_empty() { format!("git config failed ({})", r.code) } else { trimmed.to_string() };
        return Err(ExecError::Git { message, code: r.code, stderr: r.stderr, args });
    }
    Ok(parse_remote_config(&String::from_utf8_lossy(&r.stdout)))
}

/// `-z` 출력은 `key\nvalue\0` 레코드라 URL 에 공백이 있어도 안전하다. 같은 remote 에 url 이 여러 개면 첫 값만 쓴다
pub fn parse_remote_config(stdout: &str) -> Vec<Remote> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for rec in stdout.split('\0') {
        let Some(nl) = rec.find('\n') else { continue };
        let key = &rec[..nl];
        // JS `key.slice('remote.'.length, key.length - '.url'.length)` — 정규식이 앞뒤를 보장하지만 모자라면 JS 처럼 빈 이름이다
        let name = key.get("remote.".len()..key.len().saturating_sub(".url".len())).unwrap_or("");
        if name.is_empty() || !seen.insert(name.to_string()) {
            continue;
        }
        out.push(Remote { name: name.to_string(), url: rec[nl + 1..].to_string() });
    }
    out
}

/// 그 작성자의 push 된(원격 추적 ref 에서 닿는) 커밋을 찾는 `git log`. 이메일은 고정 문자열(`-F`)·대소문자 무시(`-i`)이고 `--author=<…>` 한 인자로만 들어간다
pub fn pushed_shas_args(email: &str, limit: usize) -> Vec<String> {
    ["log", "--remotes"].map(String::from).into_iter().chain([format!("--max-count={limit}"), "--format=%H".into(), "-F".into(), "-i".into(), format!("--author=<{email}>")]).collect()
}

/// `git log --format=%H` 출력에서 커밋 해시(소문자 40·64자)만 고른다
pub fn parse_pushed_shas(stdout: &str) -> Vec<String> {
    stdout
        .split('\n')
        .map(|s| s.trim_matches(is_js_space).to_lowercase())
        .filter(|s| matches!(s.len(), 40 | 64) && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
        .collect()
}

/// 이메일·개수가 git 인자가 될 수 있는 값인가(`repoQuery.ts` 의 검사 + NUL·개행 거부 — 프로세스 인자·원격 셸 줄에 들어갈 수 없다)
pub fn check_pushed_request(email: &str, limit: usize) -> Result<(), &'static str> {
    if email.is_empty() || email.chars().count() > MAX_EMAIL_LEN || email.contains(['\0', '\n', '\r']) {
        return Err("잘못된 이메일입니다");
    }
    if !(1..=MAX_PUSHED_LIMIT).contains(&limit) {
        return Err("잘못된 개수입니다");
    }
    Ok(())
}

/// 작성자의 push 된 커밋 해시. 로컬 브랜치에만 있는 커밋은 GitHub 가 몰라 404 이므로 같은 작성자의 다른 커밋으로 다시 묻는 데 쓴다.
/// git 이 실패하면(`code != 0`) 빈 목록이다(TS 와 같다)
pub async fn pushed_shas(exec: &GitExecutor, repo_path: &str, email: &str, limit: usize, opts: &RunOpts) -> Result<Vec<String>, ExecError> {
    let r = exec.run(repo_path, &pushed_shas_args(email, limit), opts).await?;
    if r.code != 0 {
        return Ok(Vec::new());
    }
    Ok(parse_pushed_shas(&String::from_utf8_lossy(&r.stdout)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn argv_는_ts_와_글자까지_같다() {
        assert_eq!(remotes_args(), ["config", "--null", "--get-regexp", "^remote\\..*\\.url$"]);
        assert_eq!(pushed_shas_args("a+b@x.io", 4), ["log", "--remotes", "--max-count=4", "--format=%H", "-F", "-i", "--author=<a+b@x.io>"]);
        // 옵션처럼 보이는 이메일도 `--author=<` 접두가 붙은 한 인자다
        let v = pushed_shas_args("--output=pwned", 1);
        assert_eq!(v.last().unwrap(), "--author=<--output=pwned>");
        assert_eq!(v.len(), 7);
    }

    #[test]
    fn 요청_검사는_길이_개수_제어문자를_본다() {
        assert!(check_pushed_request("me@x.io", 1).is_ok());
        assert!(check_pushed_request("me@x.io", 100).is_ok());
        assert_eq!(check_pushed_request("", 3), Err("잘못된 이메일입니다"));
        assert_eq!(check_pushed_request(&"a".repeat(321), 3), Err("잘못된 이메일입니다"));
        assert!(check_pushed_request(&"a".repeat(320), 3).is_ok());
        assert_eq!(check_pushed_request("a\0b", 3), Err("잘못된 이메일입니다"));
        assert_eq!(check_pushed_request("a\nb", 3), Err("잘못된 이메일입니다"));
        assert_eq!(check_pushed_request("me@x.io", 0), Err("잘못된 개수입니다"));
        assert_eq!(check_pushed_request("me@x.io", 101), Err("잘못된 개수입니다"));
    }
}
