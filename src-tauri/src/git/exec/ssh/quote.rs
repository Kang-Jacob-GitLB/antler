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
//!
//! **PowerShell 로그인 셸(Windows OpenSSH 기본 셸을 PowerShell 로 둔 서버)** 은 [`build_remote_command_for`] 가 따로 만든다(연결의 셸 확인 결과로 고른다):
//!  - 값은 작은따옴표 리터럴(`'…'`)에 둔다. 안에서 `$`·`` ` ``·`"` 는 리터럴이고, 작은따옴표(PowerShell 은 U+2018~U+201B 도 작은따옴표로 본다)만 두 번 쓴다.
//!  - 환경은 `$env:K='v'`, 실행은 `& git.exe …`(별칭·함수가 아니라 실행 파일), 끝은 `exit $LASTEXITCODE`(git 이 돌지 못했으면 127).
//!  - **Windows PowerShell 5.1 은 실행 파일에 인자를 넘길 때 명령줄을 잘못 만든다**(실측): `"` 를 이스케이프하지 않아 인자가 합쳐지거나 갈라지고(git 옵션 주입),
//!    빈 인자를 버리고, 공백이 있으면서 `\` 로 끝나는 인자는 뒤 인자를 삼킨다. 그래서 그런 인자(+ 탭 밖의 제어 문자)는 **보내지 않고 거부한다**.
//!  - 빈 환경 값은 PowerShell 에서 변수를 지우므로 표현할 수 없다. `GIT_ASKPASS=''`(askpass 를 쓰지 않음)는 같은 효과의 `-c core.askPass=` 로 바꾼다.
//!  - SFTP 가 주는 `/C:/…` 경로는 git 이 받지 않아 `C:/…` 로 바꾼다.

use std::fmt;

use crate::git::exec::common::NO_ASKPASS_ARGS;
use crate::git::exec::{build_git_args, merged_env, FetchEnv};

/// 인용·명령 구성 실패. 문구는 TS 의 `Error` 메시지와 같다(골든이 글자 단위로 비교한다)
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuoteError {
    /// 인자에 NUL
    Nul,
    /// 환경변수 이름이 `[A-Za-z_][A-Za-z0-9_]*` 가 아니다(닫힌 env 에서는 일어나지 않는다)
    BadEnvName(String),
    /// PowerShell 원격에서 안전하게 넘길 수 없는 git 인자(큰따옴표·빈 값·제어 문자·공백이 있고 `\` 로 끝남)
    PsUnsafeArg(String),
    /// PowerShell 원격에서 표현할 수 없는 빈 환경 값
    PsEmptyEnv(String),
}

impl fmt::Display for QuoteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            QuoteError::Nul => f.write_str("원격 명령 인자에 NUL 문자는 쓸 수 없습니다"),
            QuoteError::BadEnvName(k) => write!(f, "잘못된 환경변수 이름: {k}"),
            QuoteError::PsUnsafeArg(a) => {
                let shown: String = a.chars().take(80).map(|c| if c.is_control() { '?' } else { c }).collect();
                write!(f, "PowerShell 원격에는 큰따옴표·빈 값·제어 문자가 있거나 공백이 있으면서 \\ 로 끝나는 인자를 넘길 수 없습니다: {shown}")
            }
            QuoteError::PsEmptyEnv(k) => write!(f, "PowerShell 원격에는 빈 환경 값을 넘길 수 없습니다: {k}"),
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

/// 원격 로그인 셸 종류(연결마다 셸 확인으로 정한다)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteShell {
    /// sh·bash·zsh·fish — [`build_remote_command`]
    Posix,
    /// Windows PowerShell·pwsh — [`build_ps_command`]
    PowerShell,
}

/// 셸에 맞는 원격 명령. POSIX 는 [`build_remote_command`] 와 바이트까지 같다
pub fn build_remote_command_for<S: AsRef<str>>(shell: RemoteShell, repo_path: &str, args: &[S], fetch: Option<&FetchEnv>) -> Result<String, QuoteError> {
    match shell {
        RemoteShell::Posix => build_remote_command(repo_path, args, fetch),
        RemoteShell::PowerShell => build_ps_command(repo_path, args, &merged_env(fetch)),
    }
}

/// PowerShell 이 작은따옴표로 보는 문자(U+0027·U+2018·U+2019·U+201A·U+201B)
fn is_ps_single_quote(c: char) -> bool {
    matches!(c, '\'' | '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}')
}

/// PowerShell 작은따옴표 리터럴. 작은따옴표류만 두 번 쓴다(그 밖은 모두 리터럴이다)
pub fn ps_quote(s: &str) -> Result<String, QuoteError> {
    if s.contains('\0') {
        return Err(QuoteError::Nul);
    }
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for c in s.chars() {
        out.push(c);
        if is_ps_single_quote(c) {
            out.push(c);
        }
    }
    out.push('\'');
    Ok(out)
}

/// Windows PowerShell 5.1 이 실행 파일 명령줄로 그대로 옮기는 인자인가(모듈 문서 참고)
fn ps_arg_ok(a: &str) -> bool {
    !a.is_empty() && !a.contains('"') && !a.chars().any(|c| c.is_control() && c != '\t') && !(a.ends_with('\\') && a.chars().any(char::is_whitespace))
}

/// SFTP(Windows OpenSSH)가 주는 `/C:/…`·`/C:` 를 git 이 받는 `C:/…` 로 바꾼다. 그 밖은 그대로
fn ps_repo_path(p: &str) -> &str {
    let b = p.as_bytes();
    if b.len() >= 3 && b[0] == b'/' && b[1].is_ascii_alphabetic() && b[2] == b':' && (b.len() == 3 || b[3] == b'/' || b[3] == b'\\') {
        &p[1..]
    } else {
        p
    }
}

/// PowerShell 원격 명령: `$env:K='v'; … & git.exe '-C' '<path>' … ; if ($null -eq $LASTEXITCODE) { exit 127 }; exit $LASTEXITCODE`
fn build_ps_command<S: AsRef<str>>(repo_path: &str, args: &[S], env: &[(String, String)]) -> Result<String, QuoteError> {
    let mut out = String::new();
    let mut no_askpass = false;
    for (k, v) in env {
        if !valid_env_name(k) {
            return Err(QuoteError::BadEnvName(k.clone()));
        }
        if v.is_empty() {
            // 빈 값 = 변수 삭제(PowerShell). askpass 끄기만 같은 효과의 git 설정으로 옮긴다
            if k == "GIT_ASKPASS" {
                no_askpass = true;
                out.push_str("Remove-Item Env:GIT_ASKPASS -ErrorAction SilentlyContinue; ");
                continue;
            }
            return Err(QuoteError::PsEmptyEnv(k.clone()));
        }
        out.push_str(&format!("$env:{k}={}; ", ps_quote(v)?));
    }
    let mut argv = build_git_args(ps_repo_path(repo_path), args);
    if no_askpass {
        // `-C <path>` 바로 뒤(전역 인자와 같은 자리). 빈 core.askPass 는 SSH_ASKPASS 로도 내려가지 않는다(git prompt.c)
        argv.splice(2..2, NO_ASKPASS_ARGS.iter().map(|s| (*s).to_string()));
    }
    out.push_str("& git.exe");
    for a in &argv {
        if a.contains('\0') {
            return Err(QuoteError::Nul);
        }
        if !ps_arg_ok(a) {
            return Err(QuoteError::PsUnsafeArg(a.clone()));
        }
        out.push(' ');
        out.push_str(&ps_quote(a)?);
    }
    out.push_str("; if ($null -eq $LASTEXITCODE) { exit 127 }; exit $LASTEXITCODE");
    Ok(out)
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

    #[test]
    fn powershell_인용과_명령_모양() {
        assert_eq!(ps_quote("").unwrap(), "''");
        assert_eq!(ps_quote("it's").unwrap(), "'it''s'");
        assert_eq!(ps_quote("a\u{2019}b").unwrap(), "'a\u{2019}\u{2019}b'");
        assert_eq!(ps_quote("$x `n \"q\" \\").unwrap(), "'$x `n \"q\" \\'");
        assert_eq!(ps_quote("a\0b"), Err(QuoteError::Nul));
        let c = build_ps_command("/C:/Users/u/repo", &["log", "-z", "--format=%H%x00%s"], &[("GIT_PAGER".into(), "cat".into())]).unwrap();
        assert!(c.starts_with("$env:GIT_PAGER='cat'; & git.exe '-C' 'C:/Users/u/repo' '-c' 'core.quotepath=false'"), "{c}");
        assert!(c.ends_with("'log' '-z' '--format=%H%x00%s'; if ($null -eq $LASTEXITCODE) { exit 127 }; exit $LASTEXITCODE"), "{c}");
        // POSIX 는 기존과 바이트까지 같다
        assert_eq!(build_remote_command_for(RemoteShell::Posix, "/r", &["log"], None), build_remote_command("/r", &["log"], None));
    }

    #[test]
    fn powershell_로_못_넘기는_인자는_보내지_않는다() {
        for bad in ["a\"b", "", "a\nb", "a\rb", "\u{1b}[0m", "C:\\a b\\", "x \\"] {
            assert_eq!(build_ps_command("C:/r", &[bad], &[]), Err(QuoteError::PsUnsafeArg(bad.to_string())), "{bad:?}");
        }
        for ok in ["a\tb", "C:\\a\\b\\", "end\\\\", "a b", "%PATH%", "& | ^ ; <>", "$HOME", "`n$(whoami)", "it\u{2019}s", "한글 경로"] {
            assert!(build_ps_command("C:/r", &[ok], &[]).is_ok(), "{ok:?}");
        }
        assert_eq!(build_ps_command("C:/r", &["x"], &[("A".into(), String::new())]), Err(QuoteError::PsEmptyEnv("A".into())));
        assert_eq!(build_ps_command("C:/r", &["a\0"], &[]), Err(QuoteError::Nul));
    }

    #[test]
    fn powershell_은_빈_askpass_를_git_설정으로_옮긴다() {
        let c = build_ps_command("C:/r", &["fetch"], &[("GIT_ASKPASS".into(), String::new()), ("GIT_TERMINAL_PROMPT".into(), "0".into())]).unwrap();
        assert!(c.starts_with("Remove-Item Env:GIT_ASKPASS -ErrorAction SilentlyContinue; $env:GIT_TERMINAL_PROMPT='0'; & git.exe '-C' 'C:/r' '-c' 'core.askPass=' '-c' 'core.quotepath=false'"), "{c}");
        // fetch 환경 전체(GIT_ENV + FetchEnv)도 만들어진다
        let fetch = crate::git::exec::FetchEnv { allow_file: false, ssh_command: "ssh -o BatchMode=yes".into() };
        assert!(build_remote_command_for(RemoteShell::PowerShell, "C:/r", &["fetch"], Some(&fetch)).is_ok());
    }

    #[test]
    fn sftp_의_드라이브_경로를_git_이_받는_모양으로() {
        assert_eq!(ps_repo_path("/C:/Users/u"), "C:/Users/u");
        assert_eq!(ps_repo_path("/c:"), "c:");
        assert_eq!(ps_repo_path("/c:\\x"), "c:\\x");
        assert_eq!(ps_repo_path("C:/Users/u"), "C:/Users/u");
        assert_eq!(ps_repo_path("/home/u"), "/home/u");
        assert_eq!(ps_repo_path("/cx/u"), "/cx/u");
        assert_eq!(ps_repo_path("/1:/u"), "/1:/u");
    }
}
