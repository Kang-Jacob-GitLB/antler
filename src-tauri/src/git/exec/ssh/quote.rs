//! 원격 명령 문자열 구성(`src/main/executors/ssh/quote.ts`). ssh exec 채널은 명령을 원격 로그인 셸에 문자열로 넘기므로 **인용이 곧 보안 경계**다.
//!
//! 인용 규칙(bash/zsh/sh/fish 공통으로 같은 뜻이 되는 교집합만 쓴다):
//!  - 일반 문자열은 `'…'` 안에 둔다. 작은따옴표 안에서는 `$` `` ` `` `!` 개행 등이 모두 리터럴이다.
//!  - 작은따옴표 자체는 따옴표 밖에서 `\'` 로 쓴다(`'a'\''b'` 형태).
//!  - 백슬래시는 따옴표 밖에서 `\\` 로 쓴다. fish 는 작은따옴표 안에서도 `\\` 와 `\'` 를 해석하므로 따옴표 안에 백슬래시를 두면 셸마다 뜻이 갈린다.
//!
//! NUL 은 어떤 셸에서도 인자에 담을 수 없으므로 거부한다. csh 계열 로그인 셸은 지원하지 않는다.
//! 결과 바이트는 TS 와 같아야 한다(`tests/golden/ssh/quote.json`·`tests/golden/exec/env.json` 의 `ssh` 절).
//!
//! TS 의 `buildRemoteCommand(repoPath, args, env: Record)` 는 자유 env 를 받지만, 여기서는 [`FetchEnv`] 만 받는다 —
//! 호출자가 `GIT_ALLOW_PROTOCOL` 같은 방어용 환경변수를 바꾸거나 임의 변수를 원격 셸에 넣을 수 없다.

use std::fmt;

use crate::git::exec::{build_git_args, merged_env, FetchEnv};

/// 인용·명령 구성 실패. 문구는 TS 의 `Error` 메시지와 같다(골든이 글자 단위로 비교한다)
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuoteError {
    /// 인자에 NUL
    Nul,
    /// 환경변수 이름이 `[A-Za-z_][A-Za-z0-9_]*` 가 아니다(닫힌 env 에서는 일어나지 않는다)
    BadEnvName(String),
}

impl fmt::Display for QuoteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            QuoteError::Nul => f.write_str("원격 명령 인자에 NUL 문자는 쓸 수 없습니다"),
            QuoteError::BadEnvName(k) => write!(f, "잘못된 환경변수 이름: {k}"),
        }
    }
}

impl std::error::Error for QuoteError {}

/// POSIX 셸 인용. 빈 문자열은 `''`
pub fn sh_quote(s: &str) -> Result<String, QuoteError> {
    if s.contains('\0') {
        return Err(QuoteError::Nul);
    }
    if s.is_empty() {
        return Ok("''".to_string());
    }
    let mut out = String::with_capacity(s.len() + 2);
    let mut run = String::new();
    let flush = |out: &mut String, run: &mut String| {
        if !run.is_empty() {
            out.push('\'');
            out.push_str(run);
            out.push('\'');
            run.clear();
        }
    };
    for ch in s.chars() {
        match ch {
            '\'' => {
                flush(&mut out, &mut run);
                out.push_str("\\'");
            }
            '\\' => {
                flush(&mut out, &mut run);
                out.push_str("\\\\");
            }
            _ => run.push(ch),
        }
    }
    flush(&mut out, &mut run);
    Ok(out)
}

/// `^[A-Za-z_][A-Za-z0-9_]*$`
fn valid_env_name(k: &str) -> bool {
    let mut it = k.chars();
    matches!(it.next(), Some(c) if c.is_ascii_alphabetic() || c == '_') && it.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// `env K='V' … git -C '<path>' '-c' … '<arg>'` — 환경은 [`merged_env`] 순서(GIT_ENV 위에 fetch 환경을 덮은 것)다.
/// `fetch` 가 없으면 `GIT_ENV` 만이다.
pub fn build_remote_command<S: AsRef<str>>(repo_path: &str, args: &[S], fetch: Option<&FetchEnv>) -> Result<String, QuoteError> {
    build_with_env(repo_path, args, &merged_env(fetch))
}

/// 자유 env 를 받는 안쪽 구현. **모듈 밖으로 내지 않는다** — 이름 검사 시험(이 파일의 tests)만 직접 부른다
fn build_with_env<S: AsRef<str>>(repo_path: &str, args: &[S], env: &[(String, String)]) -> Result<String, QuoteError> {
    let mut parts: Vec<String> = vec!["env".to_string()];
    for (k, v) in env {
        if !valid_env_name(k) {
            return Err(QuoteError::BadEnvName(k.clone()));
        }
        parts.push(format!("{k}={}", sh_quote(v)?));
    }
    parts.push("git".to_string());
    for a in build_git_args(repo_path, args) {
        parts.push(sh_quote(&a)?);
    }
    Ok(parts.join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 잘못된_환경변수_이름은_거부한다() {
        for bad in ["A B", "", "1A", "A-B", "A=B", "한글", "A\n"] {
            let r = build_with_env("/r", &[] as &[&str], &[(bad.to_string(), "1".to_string())]);
            assert_eq!(r, Err(QuoteError::BadEnvName(bad.to_string())), "{bad:?}");
        }
        assert!(build_with_env("/r", &[] as &[&str], &[("_A1".to_string(), "1".to_string())]).is_ok());
    }

    #[test]
    fn 거부_문구는_ts_와_같다() {
        assert_eq!(QuoteError::Nul.to_string(), "원격 명령 인자에 NUL 문자는 쓸 수 없습니다");
        assert_eq!(QuoteError::BadEnvName("A B".into()).to_string(), "잘못된 환경변수 이름: A B");
    }

    #[test]
    fn 인용_모양() {
        assert_eq!(sh_quote("").unwrap(), "''");
        assert_eq!(sh_quote("it's").unwrap(), "'it'\\''s'");
        assert_eq!(sh_quote("a\\b").unwrap(), "'a'\\\\'b'");
        assert_eq!(sh_quote("'").unwrap(), "\\'");
        assert_eq!(sh_quote("\\").unwrap(), "\\\\");
        assert_eq!(sh_quote("a\0b"), Err(QuoteError::Nul));
    }
}
