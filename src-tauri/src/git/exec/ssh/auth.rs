//! 인증 수단 구성과 시도(`src/main/executors/ssh/connection.ts` 의 `buildAuth`·`FilteredAgent`).
//!
//! 순서: agent → IdentityFile 키(없으면 `~/.ssh/id_ed25519`·`id_ecdsa`·`id_rsa`). **비밀번호·keyboard-interactive 는 쓰지 않는다**(clippy.toml 이 막는다).
//! 암호화된 개인키는 passphrase 를 저장·질문하지 않으므로 건너뛰고 사유를 실패 메시지에 싣는다(agent 사용을 안내).
//! agent 는 Windows 에서 OpenSSH 파이프(`\\.\pipe\openssh-ssh-agent`) → pageant 순으로 붙고, 그 밖의 플랫폼은 `SSH_AUTH_SOCK`(유닉스 소켓)이다
//! (Windows 에서 `SSH_AUTH_SOCK` 은 TS 와 같이 보지 않는다 — `IdentityAgent` 로 파이프 경로를 줄 때만 쓴다).
//! `IdentitiesOnly yes` 면 agent 키 중 IdentityFile 의 공개키와 같은 것만 쓴다.

use std::path::Path;
use std::time::Duration;

use russh::client::{self, AuthResult};
use russh::keys::agent::client::{AgentClient, AgentStream};
use russh::keys::{decode_secret_key, Error as KeyError, PrivateKey, PrivateKeyWithHashAlg, PublicKey};
use std::sync::Arc;

use super::config::{IdentityAgent, ResolvedHost};

#[cfg(windows)]
const WIN_AGENT_PIPE: &str = r"\\.\pipe\openssh-ssh-agent";
const DEFAULT_KEY_NAMES: [&str; 3] = ["id_ed25519", "id_ecdsa", "id_rsa"];
/// agent 연결 시도 제한(파이프가 바쁘면 russh 가 무한히 다시 시도한다)
const AGENT_CONNECT_TIMEOUT: Duration = Duration::from_secs(3);

/// 호출자가 정하는 agent 사용 방식. TS 의 `agent?: string | null`
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum AgentSetting {
    /// 설정(`IdentityAgent`)·기본값을 따른다
    #[default]
    UseConfig,
    /// agent 를 쓰지 않는다
    Disabled,
    /// 소켓·파이프 경로(또는 `pageant`)
    Path(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum AgentSource {
    /// Windows 기본: OpenSSH 파이프 → pageant
    Auto,
    Pipe(String),
    Pageant,
    Unix(String),
}

fn parse_agent_path(p: &str) -> AgentSource {
    if p.eq_ignore_ascii_case("pageant") {
        AgentSource::Pageant
    } else if p.starts_with(r"\\.\pipe\") || p.starts_with("//./pipe/") {
        AgentSource::Pipe(p.to_string())
    } else {
        AgentSource::Unix(p.to_string())
    }
}

fn default_agent(env: &dyn Fn(&str) -> Option<String>) -> Option<AgentSource> {
    if cfg!(windows) {
        Some(AgentSource::Auto)
    } else {
        env("SSH_AUTH_SOCK").filter(|s| !s.is_empty()).map(AgentSource::Unix)
    }
}

pub(super) fn resolve_agent(setting: &AgentSetting, node: &IdentityAgent, env: &dyn Fn(&str) -> Option<String>) -> Option<AgentSource> {
    match setting {
        AgentSetting::Disabled => None,
        AgentSetting::Path(p) => Some(parse_agent_path(p)),
        AgentSetting::UseConfig => match node {
            IdentityAgent::Disabled => None,
            IdentityAgent::Path(p) => Some(parse_agent_path(p)),
            IdentityAgent::Default => default_agent(env),
        },
    }
}

/// 연결된 agent. 종류가 달라도 같은 타입으로 다룬다
pub(super) type Agent = AgentClient<Box<dyn AgentStream + Send + Unpin>>;

/// agent 에 붙는다. 실패 사유는 사용자에게 보일 문구다
pub(super) async fn connect_agent(source: &AgentSource) -> Result<Agent, String> {
    let attempt = async {
        match source {
            #[cfg(windows)]
            AgentSource::Auto => match AgentClient::connect_named_pipe(WIN_AGENT_PIPE).await {
                Ok(c) => Ok(c.dynamic()),
                Err(first) => match AgentClient::connect_pageant().await {
                    Ok(c) => Ok(c.dynamic()),
                    Err(_) => Err(format!("OpenSSH agent 파이프와 pageant 에 모두 연결하지 못함: {first}")),
                },
            },
            #[cfg(windows)]
            AgentSource::Pipe(p) => AgentClient::connect_named_pipe(p).await.map(AgentClient::dynamic).map_err(|e| e.to_string()),
            #[cfg(windows)]
            AgentSource::Pageant => AgentClient::connect_pageant().await.map(AgentClient::dynamic).map_err(|e| e.to_string()),
            #[cfg(unix)]
            AgentSource::Unix(p) => AgentClient::connect_uds(p).await.map(AgentClient::dynamic).map_err(|e| e.to_string()),
            #[allow(unreachable_patterns)]
            _ => Err("이 플랫폼에서 지원하지 않는 agent 종류".to_string()),
        }
    };
    match tokio::time::timeout(AGENT_CONNECT_TIMEOUT, attempt).await {
        Ok(r) => r,
        Err(_) => Err("연결 시간 초과".to_string()),
    }
}

pub(super) struct KeyFile {
    pub key: PrivateKey,
}

/// 시도할 인증 수단. 건너뛴 사유는 `notes` 에 쌓는다
pub(super) struct AuthPlan {
    pub agent: Option<AgentSource>,
    pub keys: Vec<KeyFile>,
    /// `IdentitiesOnly` 면 agent 에서 허용할 공개키(IdentityFile 의 것)
    pub allowed: Option<Vec<PublicKey>>,
    pub notes: Vec<String>,
}

impl AuthPlan {
    pub fn is_empty(&self) -> bool {
        self.agent.is_none() && self.keys.is_empty()
    }
}

fn base_name(p: &str) -> String {
    Path::new(p).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| p.to_string())
}

/// 공개키만 읽는다(`<파일>.pub` → 파일 자체가 공개키(`IdentityFile` 이 `.pub`) → 개인키 파일의 공개 부분 순). 암호화된 OpenSSH 개인키도 공개 부분은 평문이다
fn public_key_of(file: &str) -> Option<PublicKey> {
    if let Ok(text) = std::fs::read_to_string(format!("{file}.pub")) {
        if let Ok(k) = PublicKey::from_openssh(text.trim()) {
            return Some(k);
        }
    }
    let text = std::fs::read_to_string(file).ok()?;
    if let Ok(k) = PublicKey::from_openssh(text.trim()) {
        return Some(k);
    }
    PrivateKey::from_openssh(text.as_bytes()).ok().map(|k| k.public_key().clone())
}

pub(super) fn build_plan(node: &ResolvedHost, home_dir: &str, setting: &AgentSetting, env: &dyn Fn(&str) -> Option<String>) -> AuthPlan {
    let mut notes = Vec::new();
    let explicit = !node.identity_files.is_empty();
    let files: Vec<String> = if explicit {
        node.identity_files.clone()
    } else {
        DEFAULT_KEY_NAMES.iter().map(|n| Path::new(home_dir).join(".ssh").join(n).to_string_lossy().into_owned()).collect()
    };
    let agent = resolve_agent(setting, &node.identity_agent, env);
    let allowed = (agent.is_some() && node.identities_only).then(|| files.iter().filter_map(|f| public_key_of(f)).collect());
    let mut keys = Vec::new();
    for f in &files {
        let Ok(text) = std::fs::read_to_string(f) else {
            if explicit {
                notes.push(format!("키 파일을 읽을 수 없어 건너뜀: {f}"));
            }
            continue;
        };
        match decode_secret_key(&text, None) {
            Ok(key) => keys.push(KeyFile { key }),
            Err(KeyError::KeyIsEncrypted) => {
                notes.push(format!("암호화된 키라 건너뜀(passphrase 는 저장·질문하지 않음): {} — ssh-add 로 agent 에 등록하세요.", base_name(f)))
            }
            Err(_) => notes.push(format!("지원하지 않는 키 형식이라 건너뜀: {}", base_name(f))),
        }
    }
    AuthPlan { agent, keys, allowed, notes }
}

/// 인증을 시도한다. 성공하면 true, 모든 수단이 거절되면 false. 전송 오류는 `Err`
pub(super) async fn authenticate<H: client::Handler>(handle: &mut client::Handle<H>, user: &str, plan: &mut AuthPlan) -> Result<bool, russh::Error> {
    if let Some(source) = plan.agent.clone() {
        match connect_agent(&source).await {
            Err(why) => plan.notes.push(format!("agent 사용 불가({why})")),
            Ok(mut agent) => match agent.request_identities().await {
                Err(e) => plan.notes.push(format!("agent 사용 불가({e})")),
                Ok(ids) => {
                    for id in ids {
                        let pk = id.public_key().into_owned();
                        if let Some(allowed) = &plan.allowed {
                            if !allowed.iter().any(|a| a.key_data() == pk.key_data()) {
                                continue;
                            }
                        }
                        let hash = if pk.algorithm().is_rsa() { handle.best_supported_rsa_hash().await?.flatten() } else { None };
                        match handle.authenticate_publickey_with(user, pk, hash, &mut agent).await {
                            Ok(AuthResult::Success) => return Ok(true),
                            Ok(AuthResult::Failure { .. }) => {}
                            // 세션이 이미 죽었으면 다른 수단도 소용없다(오류 타입이 비공개라 variant 로 가르지 못한다)
                            Err(_) if handle.is_closed() => return Err(russh::Error::SendError),
                            Err(e) => plan.notes.push(format!("agent 서명 실패({e})")),
                        }
                    }
                }
            },
        }
    }
    for k in &plan.keys {
        let hash = if k.key.algorithm().is_rsa() { handle.best_supported_rsa_hash().await?.flatten() } else { None };
        let with_hash = PrivateKeyWithHashAlg::new(Arc::new(k.key.clone()), hash);
        if handle.authenticate_publickey(user, with_hash).await?.success() {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};

    use russh::keys::ssh_key::LineEnding;
    use russh::keys::Algorithm;

    use super::super::config::{load_ssh_config, resolve_host, Ctx, RealFs};
    use super::*;

    struct Dir(std::path::PathBuf);

    impl Dir {
        fn new() -> Dir {
            static N: AtomicU32 = AtomicU32::new(0);
            let p = std::env::temp_dir().join(format!("antler-auth-{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
            std::fs::create_dir_all(&p).unwrap();
            Dir(p)
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn key() -> PrivateKey {
        PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).unwrap()
    }

    #[test]
    fn 공개키_읽기는_pub_파일_자체_개인키_순서로_본다() {
        let d = Dir::new();
        let k = key();
        let line = k.public_key().to_openssh().unwrap();
        // IdentityFile 이 `.pub` 자체인 경우(옆에 `.pub.pub` 은 없다)
        let only_pub = d.0.join("only.pub");
        std::fs::write(&only_pub, format!("{line}\n")).unwrap();
        assert_eq!(public_key_of(&only_pub.to_string_lossy()).unwrap().key_data(), k.public_key().key_data());
        // 개인키 파일(옆의 .pub 없음)
        let private = d.0.join("id");
        std::fs::write(&private, k.to_openssh(LineEnding::LF).unwrap().as_bytes()).unwrap();
        assert_eq!(public_key_of(&private.to_string_lossy()).unwrap().key_data(), k.public_key().key_data());
        // 공개키도 개인키도 아닌 파일·없는 파일
        let junk = d.0.join("junk");
        std::fs::write(&junk, "not a key").unwrap();
        assert!(public_key_of(&junk.to_string_lossy()).is_none());
        assert!(public_key_of(&d.0.join("missing").to_string_lossy()).is_none());
    }

    #[test]
    fn identities_only_이고_identity_file_이_pub_이면_agent_필터가_그_공개키를_쓴다() {
        let d = Dir::new();
        let k = key();
        let pub_path = d.0.join("work.pub");
        std::fs::write(&pub_path, format!("{}\n", k.public_key().to_openssh().unwrap())).unwrap();
        let cfg_path = d.0.join("config");
        std::fs::write(&cfg_path, format!("Host h\n  HostName 127.0.0.1\n  IdentityFile {}\n  IdentitiesOnly yes\n", pub_path.to_string_lossy().replace(std::path::MAIN_SEPARATOR, "/"))).unwrap();
        let home = d.0.to_string_lossy().into_owned();
        let config = load_ssh_config(&cfg_path.to_string_lossy(), &home, &RealFs);
        let env = |_: &str| None;
        let node = resolve_host(&config, "h", &Ctx { local_user: "t", env: &env });
        // agent 는 경로만 주면 켜진 것으로 본다(여기서는 접속하지 않는다)
        let plan = build_plan(&node, &home, &AgentSetting::Path("/nonexistent/agent.sock".to_string()), &env);
        let allowed = plan.allowed.expect("IdentitiesOnly + agent 면 필터가 있다");
        assert_eq!(allowed.len(), 1, "공개키 파일이 필터에 들어가야 한다");
        assert_eq!(allowed[0].key_data(), k.public_key().key_data());
    }
}
