//! SSH 시험 공용 도구(`tests/sshHarness.ts` 의 `makeHome`·`makeRepo`·`knownHostsLine`). 임시 홈·키·설정·known_hosts·저장소를 만든다.
//! 사용자의 `~/.ssh`·ssh-agent 는 읽지도 쓰지도 않는다 — 실행기에는 항상 임시 홈과 `AgentSetting::Disabled` 를 준다.

#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use antler_lib::git::exec::ssh::{AgentSetting, SshExecutor, SshOptions, Timeouts};
use russh::keys::ssh_key::LineEnding;
use russh::keys::{PrivateKey, PublicKey};

/// 임시 폴더(Drop 에서 지운다)
pub struct TempDir(pub PathBuf);

impl TempDir {
    pub fn new(tag: &str) -> Self {
        static N: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!("antler-ssh-{tag}-{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
        fs::create_dir_all(&dir).expect("임시 폴더");
        TempDir(dir)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// 임시 홈(`.ssh` 포함)과 그 안의 클라이언트 개인키·설정·known_hosts 경로
pub struct Home {
    pub dir: TempDir,
    pub config: PathBuf,
    pub known_hosts: PathBuf,
    pub key_path: PathBuf,
}

impl Home {
    /// `client_key` 를 `~/.ssh/id_test` 로 써 둔다(OpenSSH 형식, 암호 없음)
    pub fn new(client_key: &PrivateKey) -> Home {
        let dir = TempDir::new("home");
        let ssh = dir.path().join(".ssh");
        fs::create_dir_all(&ssh).unwrap();
        let key_path = ssh.join("id_test");
        fs::write(&key_path, client_key.to_openssh(LineEnding::LF).unwrap().as_bytes()).unwrap();
        Home { config: ssh.join("config"), known_hosts: ssh.join("known_hosts"), key_path, dir }
    }

    pub fn home_str(&self) -> String {
        self.dir.path().to_string_lossy().into_owned()
    }

    /// 키 경로를 설정 파일에 쓸 수 있는 슬래시 형식으로
    pub fn key_for_config(&self) -> String {
        self.key_path.to_string_lossy().replace('\\', "/")
    }

    pub fn write_config(&self, text: &str) {
        fs::write(&self.config, text).unwrap();
    }

    pub fn write_known_hosts(&self, text: &str) {
        fs::write(&self.known_hosts, text).unwrap();
    }

    /// 이 홈을 쓰는 실행기. agent 는 끈다(사용자의 agent 를 건드리지 않는다)
    pub fn executor(&self, spec: &str) -> SshExecutor {
        self.executor_with(spec, |_| {})
    }

    pub fn executor_with(&self, spec: &str, tweak: impl FnOnce(&mut SshOptions)) -> SshExecutor {
        let mut opts = SshOptions::for_home(&self.home_str());
        opts.agent = AgentSetting::Disabled;
        opts.env = Arc::new(|_| None);
        opts.local_user = "tester".to_string();
        opts.timeouts = Timeouts { ready: Duration::from_secs(20), ..Timeouts::default() };
        tweak(&mut opts);
        SshExecutor::new(spec, opts)
    }
}

/// `[127.0.0.1]:<포트> <키 종류> <base64>` 한 줄(개행 포함)
pub fn known_hosts_line(port: u16, key: &PublicKey) -> String {
    format!("[127.0.0.1]:{port} {}\n", openssh_no_comment(key))
}

/// `<종류> <base64>` (주석 없음)
pub fn openssh_no_comment(key: &PublicKey) -> String {
    let line = key.to_openssh().unwrap();
    line.split_whitespace().take(2).collect::<Vec<_>>().join(" ")
}

/// 테스트 준비용 날 git(제품 경로가 아니다 — 저장소를 만들 뿐이다). 격리 환경변수는 `isolate`(ctor)가 프로세스에 넣었다
#[allow(clippy::disallowed_methods)]
fn git(dir: &Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@e.x", "-c", "commit.gpgsign=false"])
        .args(args)
        .current_dir(dir)
        .output()
        .expect("git 실행");
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
}

/// 한글 커밋 메시지와(선택) 큰 바이너리가 든 임시 저장소
pub fn make_repo(big_bytes: usize) -> TempDir {
    let dir = TempDir::new("repo");
    git(dir.path(), &["init", "-q", "-b", "main"]);
    fs::write(dir.path().join("a.txt"), "hello\n").unwrap();
    if big_bytes > 0 {
        fs::write(dir.path().join("big.bin"), vec![b'x'; big_bytes]).unwrap();
    }
    git(dir.path(), &["add", "-A"]);
    git(dir.path(), &["commit", "-q", "-m", "첫 커밋: 한글 메시지"]);
    dir
}

/// 조건이 참이 될 때까지 기다린다(시간 기반 `sleep` 으로 동작을 가정하지 않는다 — 조건 + 상한)
pub async fn wait_for(what: &str, mut cond: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !cond() {
        assert!(Instant::now() < deadline, "시간 안에 {what} 가 되지 않았다");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}
