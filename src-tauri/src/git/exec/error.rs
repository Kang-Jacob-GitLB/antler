//! 실행기 오류. TS 의 `GitError`·`AbortError`·spawn 실패(`Error`)에 대응하며,
//! 이후 IPC envelope 의 `code`(GIT / ABORTED / ERROR)로 그대로 옮겨진다(`src/main/ipc/handlers.ts`).

use std::fmt;

/// `Shared` future 의 출력으로 복제돼야 하므로 `io::Error` 는 문자열로 담는다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecError {
    /// git(또는 wsl.exe)이 실패했거나 출력이 상한을 넘었다. `code` 는 종료 코드(상한 초과·신호 종료는 -1)
    Git { message: String, code: i32, stderr: String, args: Vec<String> },
    /// 취소 신호로 끝났다
    Aborted,
    /// 프로세스를 띄우지 못했거나 기다리다 실패했다(git 없음 등)
    SpawnFailed { message: String },
    /// 실행기 밖의 검증·해석 실패(잘못된 입력, 파서가 거부한 출력). TS 의 일반 `Error` 에 해당해 code 는 ERROR 다
    Other { message: String },
}

impl ExecError {
    /// envelope 의 `code` 문자열
    pub fn envelope_code(&self) -> &'static str {
        match self {
            ExecError::Git { .. } => "GIT",
            ExecError::Aborted => "ABORTED",
            ExecError::SpawnFailed { .. } | ExecError::Other { .. } => "ERROR",
        }
    }

    pub(super) fn spawn(e: &std::io::Error) -> Self {
        ExecError::SpawnFailed { message: e.to_string() }
    }
}

impl fmt::Display for ExecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ExecError::Git { message, .. } => f.write_str(message),
            ExecError::Aborted => f.write_str("aborted"),
            ExecError::SpawnFailed { message } | ExecError::Other { message } => f.write_str(message),
        }
    }
}

impl std::error::Error for ExecError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_코드가_종류별로_나뉜다() {
        let git = ExecError::Git { message: "m".into(), code: 128, stderr: String::new(), args: vec![] };
        assert_eq!(git.envelope_code(), "GIT");
        assert_eq!(ExecError::Aborted.envelope_code(), "ABORTED");
        assert_eq!(ExecError::SpawnFailed { message: "x".into() }.envelope_code(), "ERROR");
        assert_eq!(ExecError::Other { message: "x".into() }.envelope_code(), "ERROR");
        assert_eq!(ExecError::Aborted.to_string(), "aborted");
    }
}
