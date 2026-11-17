//! SSH 실행기(`src/main/executors/SshExecutor.ts`). 원격 호스트의 git 을 russh 로 실행한다. `hostSpec` 은 `~/.ssh/config` 별칭 또는 `[user@]host[:port]`.
//! 연결은 실행기마다 하나를 재사용하고(끊기면 다음 호출에서 재연결), 채널은 동시 6개로 제한한다. 인증은 agent 와 암호 없는 키 파일뿐이다.
//! 원격 명령은 `build_remote_command`(PR 1)의 결과만 보낸다 — 이 모듈 밖에서 임의 명령을 보낼 길이 없다.

use super::connection::{Connection, SshOptions};
use super::quote::build_remote_command;
use crate::git::exec::common::{RunOpts, RunResult};
use crate::git::exec::error::ExecError;

#[derive(Clone)]
pub struct SshExecutor {
    host_spec: String,
    conn: std::sync::Arc<Connection>,
}

impl SshExecutor {
    pub fn new(host_spec: &str, opts: SshOptions) -> Self {
        SshExecutor { host_spec: host_spec.to_string(), conn: std::sync::Arc::new(Connection::new(host_spec, opts)) }
    }

    pub fn host_spec(&self) -> &str {
        &self.host_spec
    }

    /// 원격 git 의 stdout 을 전부 모아 돌려준다(상한 `opts.max_buffer`, 기본 64MB; stderr 는 앞 4MB). 종료 코드가 0 이 아니어도 `Ok` — 호출자가 본다.
    /// 종료 코드를 받지 못하고 연결이 끊기면 "SSH 연결이 끊겨…" 오류다.
    pub async fn run<S: AsRef<str>>(&self, repo_path: &str, args: &[S], opts: &RunOpts) -> Result<RunResult, ExecError> {
        if opts.cancel.as_ref().is_some_and(tokio_util::sync::CancellationToken::is_cancelled) {
            return Err(ExecError::Aborted);
        }
        let command = build_remote_command(repo_path, args, opts.fetch_env.as_ref()).map_err(|e| ExecError::Other { message: e.to_string() })?;
        let git_args: Vec<String> = args.iter().map(|a| a.as_ref().to_string()).collect();
        self.conn.run(command, &git_args, opts).await
    }

    /// 연결을 닫고 이후 호출을 거부한다
    pub fn dispose(&self) {
        self.conn.dispose();
    }

    /// 지금 살아 있는 연결이 있는가(시험·진단용)
    pub fn is_connected(&self) -> bool {
        self.conn.is_connected()
    }

    /// 비어 있는 채널 슬롯 수(시험·진단용)
    pub fn free_slots(&self) -> usize {
        self.conn.free_slots()
    }
}
