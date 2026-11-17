//! 저장소 미러. 라우터가 저장소 경로·환경을 얻는 **유일한** 출처다. 설정을 쓰는 쪽은 Node 사이드카(main 의 ConfigStore)이고,
//! 사이드카가 시작 때(ready 전)와 목록이 바뀔 때마다 보내는 `{t:"repos", gen, repos:[{id,kind,path,name,distro?,sshHost?}]}` 로 채운다
//! (`src/sidecar/protocol.ts`·`mirror.ts`). renderer 가 보낸 경로·kind·배포판은 어디서도 쓰지 않는다.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, PoisonError, RwLock};

use serde_json::Value;

use crate::git::exec::wsl_paths::is_wsl_unc;
use crate::git::parsers::js_trim;
use crate::git::scope::is_js_space;
use crate::router::RUST_FETCH_KINDS;

/// `src/main/ipc/handlers.ts` 의 `MAX_PATH_LEN`·`MAX_HOST_LEN`
const MAX_PATH_LEN: usize = 4096;
const MAX_DISTRO_LEN: usize = 255;
const MAX_HOST_LEN: usize = 255;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MirrorRepo {
    pub id: String,
    /// 저장소가 속한 소스의 종류: local · wsl · ssh · github
    pub kind: String,
    pub path: String,
    pub distro: Option<String>,
    /// ssh 소스의 호스트(`~/.ssh/config` 별칭 또는 `[user@]host[:port]`). ssh 가 아니면 없다
    pub ssh_host: Option<String>,
}

/// Rust 가 git 을 직접 돌려도 되는 저장소의 실행 대상(검증을 마친 값)
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Local { path: String },
    Wsl { distro: String, path: String },
    /// `host` 는 검사를 마친(앞뒤 공백을 지운) 값이고 실행기·큐·풀의 키가 모두 이 값이다
    Ssh { host: String, path: String },
}

impl Target {
    pub fn path(&self) -> &str {
        match self {
            Target::Local { path } | Target::Wsl { path, .. } | Target::Ssh { path, .. } => path,
        }
    }

    /// 소스 종류(`MirrorRepo::kind` 와 같은 값)
    pub fn kind(&self) -> &'static str {
        match self {
            Target::Local { .. } => "local",
            Target::Wsl { .. } => "wsl",
            Target::Ssh { .. } => "ssh",
        }
    }

    /// 큐·실행기를 나누는 키. TS 는 소스(local 하나, wsl 은 배포판마다, ssh 는 호스트마다 하나)마다 큐를 둔다
    pub fn source_key(&self) -> String {
        match self {
            Target::Local { .. } => "local".to_string(),
            Target::Wsl { distro, .. } => format!("wsl:{distro}"),
            Target::Ssh { host, .. } => format!("ssh:{host}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolved {
    /// 미러에 있고 kind 가 local·wsl(ssh 는 스위치가 켜져 있을 때만)이며 검증을 통과했다
    Rust(Target),
    /// 미러에 없거나 Rust 가 다루지 않는 kind(github, 스위치가 꺼진 ssh) — 사이드카가 처리한다(오류 문구도 사이드카가 만든다)
    Sidecar,
    /// 미러에 있는 값이 경로·배포판 검사에 걸렸다 — 이 메시지를 ERROR 로 돌려준다(사이드카로 넘기지 않는다)
    Invalid(String),
}

#[derive(Default)]
struct Inner {
    gen: u64,
    repos: HashMap<String, MirrorRepo>,
    /// 저장소 id → 사용자가 붙인 이름(확인 창 제목에만 쓴다. 비밀이 아니고 경로·kind 판정에는 쓰지 않는다)
    names: HashMap<String, String>,
}

#[derive(Clone, Default)]
pub struct RepoMirror {
    inner: Arc<RwLock<Inner>>,
    /// ssh 저장소를 Rust 로 풀어도 되는가. 꺼져 있으면(기본) ssh 는 사이드카다. 라우터가 SSH 풀을 주입할 때 켠다(`Router::ssh_active`) —
    /// 해석이 이 한 곳에서만 갈려야 로그 커서·fetch 서비스·라우터가 같은 답을 본다
    ssh: Arc<AtomicBool>,
}

impl RepoMirror {
    /// 미러를 통째로 바꾼다. 이미 받은 것보다 gen 이 작거나 같으면 버린다(뒤처진 메시지). 바꿨으면 true
    pub fn apply(&self, gen: u64, repos: Vec<MirrorRepo>) -> bool {
        self.apply_named(gen, repos, HashMap::new())
    }

    fn apply_named(&self, gen: u64, repos: Vec<MirrorRepo>, names: HashMap<String, String>) -> bool {
        let mut inner = self.inner.write().unwrap_or_else(PoisonError::into_inner);
        if gen <= inner.gen {
            return false;
        }
        inner.gen = gen;
        inner.repos = repos.into_iter().map(|r| (r.id.clone(), r)).collect();
        inner.names = names;
        true
    }

    /// ssh 저장소를 Rust 로 풀지 정한다(라우터가 정한다)
    pub fn set_ssh(&self, on: bool) {
        self.ssh.store(on, Ordering::SeqCst);
    }

    /// 지금 Rust 로 풀리는 ssh 저장소의 호스트 전부(풀의 연결을 정리할 때 "아직 쓰는 호스트")
    pub fn ssh_hosts(&self) -> HashSet<String> {
        let ids: Vec<String> = {
            let inner = self.inner.read().unwrap_or_else(PoisonError::into_inner);
            inner.repos.values().filter(|r| r.kind == "ssh").map(|r| r.id.clone()).collect()
        };
        ids.iter()
            .filter_map(|id| match self.resolve(id) {
                Resolved::Rust(Target::Ssh { host, .. }) => Some(host),
                _ => None,
            })
            .collect()
    }

    /// 저장소 이름(사이드카가 `name` 을 보냈을 때만)
    pub fn name(&self, repo_id: &str) -> Option<String> {
        self.inner.read().unwrap_or_else(PoisonError::into_inner).names.get(repo_id).cloned()
    }

    /// 사이드카의 `repos` 메시지(JSON)를 받아 적용한다. 모양이 틀린 메시지는 버리고, 틀린 항목은 그 항목만 뺀다
    pub fn apply_message(&self, msg: &Value) -> bool {
        let Some(gen) = msg.get("gen").and_then(Value::as_u64) else { return false };
        let Some(list) = msg.get("repos").and_then(Value::as_array) else { return false };
        let repos: Vec<MirrorRepo> = list.iter().filter_map(parse_repo).collect();
        let names = list
            .iter()
            .filter_map(|v| Some((v.get("id")?.as_str()?.to_string(), v.get("name")?.as_str()?.to_string())))
            .filter(|(id, _)| repos.iter().any(|r| &r.id == id))
            .collect();
        self.apply_named(gen, repos, names)
    }

    pub fn resolve(&self, repo_id: &str) -> Resolved {
        let inner = self.inner.read().unwrap_or_else(PoisonError::into_inner);
        let Some(repo) = inner.repos.get(repo_id) else { return Resolved::Sidecar };
        match repo.kind.as_str() {
            "local" => match check_path(&repo.kind, &repo.path) {
                Ok(()) => Resolved::Rust(Target::Local { path: repo.path.clone() }),
                Err(m) => Resolved::Invalid(m),
            },
            // WSL 은 Windows 에서만 연다(TS createExecutor 와 같다) — 그 밖의 OS 는 사이드카가 오류를 만든다
            "wsl" if cfg!(windows) => {
                let checked = check_path(&repo.kind, &repo.path).and_then(|()| check_distro(repo.distro.as_deref()));
                match checked {
                    Ok(distro) => Resolved::Rust(Target::Wsl { distro, path: repo.path.clone() }),
                    Err(m) => Resolved::Invalid(m),
                }
            }
            // 라우터가 SSH 풀을 주입하지 않았거나 스위치가 꺼져 있으면 사이드카다
            "ssh" if self.ssh.load(Ordering::SeqCst) => {
                let checked = check_path(&repo.kind, &repo.path).and_then(|()| check_ssh_host(repo.ssh_host.as_deref()));
                match checked {
                    Ok(host) => Resolved::Rust(Target::Ssh { host, path: repo.path.clone() }),
                    Err(m) => Resolved::Invalid(m),
                }
            }
            _ => Resolved::Sidecar,
        }
    }

    /// fetch 를 Rust 서비스가 맡는 저장소만 거른 해석. **소스 종류(`RUST_FETCH_KINDS`)가 먼저다** — 해석 결과(`Resolved::Rust`)로 소유를 정하면
    /// ssh 가 Rust 로 풀리는 순간 Rust 도 ssh 를 자동 fetch 해 사이드카와 이중 fetch 가 된다. fetch 의 서비스·라우터·자동 예약은 모두 이 함수만 쓴다.
    /// 종류가 맡는 것이 아니면(ssh·github) 검증보다 먼저 `Sidecar` 다 — 호스트가 잘못된 ssh 저장소의 fetch 도 사이드카가 오류를 만든다
    pub fn resolve_fetch(&self, repo_id: &str) -> Resolved {
        let owned = {
            let inner = self.inner.read().unwrap_or_else(PoisonError::into_inner);
            inner.repos.get(repo_id).is_some_and(|r| RUST_FETCH_KINDS.split(',').any(|k| k == r.kind))
        };
        if owned {
            self.resolve(repo_id)
        } else {
            Resolved::Sidecar
        }
    }

    #[cfg(test)]
    fn count(&self) -> usize {
        self.inner.read().unwrap_or_else(PoisonError::into_inner).repos.len()
    }
}

fn parse_repo(v: &Value) -> Option<MirrorRepo> {
    Some(MirrorRepo {
        id: v.get("id")?.as_str()?.to_string(),
        kind: v.get("kind")?.as_str()?.to_string(),
        path: v.get("path")?.as_str()?.to_string(),
        distro: v.get("distro").and_then(Value::as_str).map(str::to_string),
        ssh_host: v.get("sshHost").and_then(Value::as_str).map(str::to_string),
    })
}

/// JS `string.length` — UTF-16 코드 단위 수
fn utf16_len(s: &str) -> usize {
    s.encode_utf16().count()
}

/// 저장소 경로 검사. `src/main/ipc/handlers.ts` 의 `checkPath` 와 같은 규칙이다.
/// - 비었거나(JS trim 기준 공백뿐) 4096 UTF-16 단위를 넘거나 NUL 이 있으면 거부
/// - 로컬·WSL 의 `\\host\share` 형태 네트워크 경로는 거부(git 이 SMB 로 접근하며 NTLM 해시가 새어 나갈 수 있다. `\\wsl.localhost\…` 는 허용)
///
/// 길이·NUL 은 원래 값으로, UNC 는 앞뒤 공백을 지운 값으로 본다(` \\host\share` 가 빠져나가지 못하게 — TS 와 같다).
/// **ssh 에도 NT 네임스페이스 검사가 걸린다**(`checkPath` 도 모든 kind 에 같은 검사를 한다 — 다른 점은 TS 는 저장소를 **추가할 때만** 보고 Rust 는 쓸 때마다 미러 값을 다시 본다는 것). 원격 POSIX 경로에 `/??/` 가 들어가는 일은 현실에 없다.
/// `[\\/]??[\\/]` 가 들어 있는가
fn has_nt_namespace(p: &str) -> bool {
    p.as_bytes().windows(4).any(|w| matches!(w[0], b'\\' | b'/') && w[1] == b'?' && w[2] == b'?' && matches!(w[3], b'\\' | b'/'))
}

pub fn check_path(kind: &str, path: &str) -> Result<(), String> {
    if js_trim(path).is_empty() || utf16_len(path) > MAX_PATH_LEN || path.contains('\0') {
        return Err("잘못된 경로입니다".to_string());
    }
    let p = js_trim(path);
    // NT 네임스페이스 접두어(\??\UNC\host\share): 앞이 역슬래시 하나라 아래 UNC 검사를 빠져나가지만 Windows 는 SMB 로 연다.
    // TS 의 NT_NAMESPACE(src/main/git/fetch.ts) 와 같다 — 경로 어디에 있어도 거부
    if has_nt_namespace(p) {
        return Err("네트워크 공유 경로는 지원하지 않습니다(WSL 경로는 \\\\wsl.localhost\\… 로 고르세요)".to_string());
    }
    let mut chars = p.chars();
    let unc = matches!((chars.next(), chars.next()), (Some('\\' | '/'), Some('\\' | '/')));
    if (kind == "local" || kind == "wsl") && unc && !is_wsl_unc(p) {
        return Err("네트워크 공유 경로는 지원하지 않습니다(WSL 경로는 \\\\wsl.localhost\\… 로 고르세요)".to_string());
    }
    Ok(())
}

/// SSH 호스트 검사. `src/main/ipc/handlers.ts` 의 `checkEnvInput` 과 같은 규칙이다 — 앞뒤 공백을 지운 값이 비지 않고, 255 UTF-16 단위 이하이고,
/// 공백(JS `\s`)·NUL 이 없고, `-` 로 시작하지 않는다(옵션으로 읽힐 수 있다). **TS 보다 엄격한 점**: 제어 문자 전체를 거부한다(TS 는 NUL 과 공백류만).
/// 검사한 값(앞뒤 공백을 지운 것)을 돌려주고 호출자는 이 값만 쓴다 — 검사한 값과 실행기에 넘기는 값이 다르면 ` -x` 같은 값이 `-` 검사를 빠져나간다.
/// 호스트가 `~/.ssh/config` 에 있는지·known_hosts 에 있는지는 연결할 때 실행기가 본다(확인 창은 저장소를 추가할 때만 뜬다 — 사이드카 몫)
pub fn check_ssh_host(host: Option<&str>) -> Result<String, String> {
    let bad = || "잘못된 SSH 호스트입니다".to_string();
    let h = js_trim(host.ok_or_else(bad)?);
    if h.is_empty() || utf16_len(h) > MAX_HOST_LEN || h.starts_with('-') || h.chars().any(|c| is_js_space(c) || c.is_control()) {
        return Err(bad());
    }
    Ok(h.to_string())
}

/// WSL 배포판 이름 검사. TS 는 저장소를 **추가할 때** 설치된 목록(`wsl.exe -l -q`)과 대조해 소스에 저장하고, 쓸 때는 다시 확인하지 않는다.
/// Rust 는 사이드카가 보낸(= 그렇게 검증돼 저장된) 값이라도 `wsl.exe -d <이름>` 에 넣기 전에 형식을 본다 —
/// 비었거나 길거나 제어 문자가 있거나 `-` 로 시작하면(옵션으로 읽힐 수 있다) 거부한다. 설치 여부는 `wsl.exe` 가 실행할 때 알려 준다.
pub fn check_distro(distro: Option<&str>) -> Result<String, String> {
    let bad = || "설치된 WSL 배포판이 아닙니다".to_string();
    let d = distro.ok_or_else(bad)?;
    if js_trim(d).is_empty() || utf16_len(d) > MAX_DISTRO_LEN || d.starts_with('-') || d.chars().any(char::is_control) {
        return Err(bad());
    }
    Ok(d.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn repo(id: &str, kind: &str, path: &str, distro: Option<&str>) -> MirrorRepo {
        MirrorRepo { id: id.into(), kind: kind.into(), path: path.into(), distro: distro.map(Into::into), ssh_host: None }
    }

    fn ssh_repo(id: &str, host: Option<&str>, path: &str) -> MirrorRepo {
        MirrorRepo { id: id.into(), kind: "ssh".into(), path: path.into(), distro: None, ssh_host: host.map(Into::into) }
    }

    #[test]
    fn 경로_검사는_check_path_와_같다() {
        assert!(check_path("local", "C:\\work\\repo").is_ok());
        assert!(check_path("local", "/home/me/repo").is_ok());
        assert!(check_path("wsl", "/home/me/repo").is_ok());
        assert!(check_path("local", "\\\\wsl.localhost\\Ubuntu\\home\\me").is_ok());
        assert!(check_path("local", "\\\\wsl$\\Ubuntu\\home").is_ok());
        for bad in [
            "",
            "   ",
            "\u{feff}",
            "a\0b",
            "\\\\host\\share",
            "//host/share",
            " \\\\host\\share",
            "\t//host/share",
            "\\/host",
            "\\\\wsl.localhost",
            // NT 네임스페이스 접두어 — 앞이 역슬래시 하나라 UNC 검사를 빠져나간다(git -C 가 실제로 연다)
            "\\??\\UNC\\host\\share\\r",
            " \\??\\UNC\\host\\share",
            "/??/UNC/host/share",
            "\\??\\C:\\work\\repo",
        ] {
            assert!(check_path("local", bad).is_err(), "{bad:?}");
            assert!(check_path("wsl", bad).is_err(), "{bad:?}");
        }
        // 길이는 UTF-16 단위 — 이모지(char 1·UTF-16 2)로 채우면 2049 개에서 4098 단위로 넘는다
        assert!(check_path("local", &"a".repeat(4096)).is_ok());
        assert!(check_path("local", &"a".repeat(4097)).is_err());
        assert!(check_path("local", &"😀".repeat(2048)).is_ok());
        assert!(check_path("local", &"😀".repeat(2049)).is_err());
    }

    #[test]
    fn 배포판_이름은_형식을_본다() {
        assert_eq!(check_distro(Some("Ubuntu-24.04")).unwrap(), "Ubuntu-24.04");
        assert_eq!(check_distro(Some("My Distro")).unwrap(), "My Distro");
        for bad in [None, Some(""), Some("  "), Some("-d"), Some("--exec"), Some("a\nb"), Some("a\0b"), Some(&"x".repeat(256)[..])] {
            assert!(check_distro(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn 미러에_없으면_사이드카_ssh_github_도_사이드카() {
        let m = RepoMirror::default();
        assert!(m.apply(1, vec![repo("a", "local", "/r", None), repo("s", "ssh", "/srv", None), repo("g", "github", "/cache", None)]));
        assert_eq!(m.resolve("a"), Resolved::Rust(Target::Local { path: "/r".into() }));
        assert_eq!(m.resolve("nope"), Resolved::Sidecar);
        assert_eq!(m.resolve("s"), Resolved::Sidecar);
        assert_eq!(m.resolve("g"), Resolved::Sidecar);
    }

    #[test]
    fn ssh_호스트_검사는_handlers_ts_와_같고_제어_문자는_더_엄격하다() {
        assert_eq!(check_ssh_host(Some("devbox")).unwrap(), "devbox");
        assert_eq!(check_ssh_host(Some("me@host:2222")).unwrap(), "me@host:2222");
        // 앞뒤 공백은 지우고 그 값을 돌려준다
        assert_eq!(check_ssh_host(Some("  devbox\t")).unwrap(), "devbox");
        assert_eq!(check_ssh_host(Some(&"h".repeat(255))).unwrap().len(), 255);
        for bad in [
            None,
            Some(""),
            Some("   "),
            Some("a b"),
            Some("a\tb"),
            Some("a\u{a0}b"),
            Some("-oProxyCommand=x"),
            Some("  -x"),
            Some("a\0b"),
            Some("a\x07b"),
            Some("a\nb"),
            Some(&"h".repeat(256)[..]),
        ] {
            assert!(check_ssh_host(bad).is_err(), "{bad:?}");
        }
        // 길이는 UTF-16 단위
        assert!(check_ssh_host(Some(&"😀".repeat(127))).is_ok());
        assert!(check_ssh_host(Some(&"😀".repeat(128))).is_err());
    }

    #[test]
    fn ssh_는_스위치가_켜졌을_때만_rust_로_풀린다() {
        let m = RepoMirror::default();
        assert!(m.apply(
            1,
            vec![ssh_repo("s", Some(" devbox "), "/srv/r"), ssh_repo("nohost", None, "/srv/r"), ssh_repo("dash", Some("-x"), "/srv/r"), ssh_repo("nt", Some("h"), "\\??\\C:\\x")]
        ));
        // 기본(꺼짐)은 사이드카다 — 호스트가 잘못돼도 사이드카가 오류를 만든다
        for id in ["s", "nohost", "dash", "nt"] {
            assert_eq!(m.resolve(id), Resolved::Sidecar, "{id}");
        }
        assert!(m.ssh_hosts().is_empty());
        m.set_ssh(true);
        assert_eq!(m.resolve("s"), Resolved::Rust(Target::Ssh { host: "devbox".into(), path: "/srv/r".into() }));
        for id in ["nohost", "dash", "nt"] {
            assert!(matches!(m.resolve(id), Resolved::Invalid(_)), "{id}");
        }
        assert_eq!(m.ssh_hosts(), HashSet::from(["devbox".to_string()]));
        let t = Target::Ssh { host: "devbox".into(), path: "/p".into() };
        assert_eq!((t.source_key().as_str(), t.kind(), t.path()), ("ssh:devbox", "ssh", "/p"));
    }

    #[test]
    fn fetch_소유는_해석이_아니라_소스_종류로_정한다() {
        let m = RepoMirror::default();
        m.apply(1, vec![repo("a", "local", "/r", None), ssh_repo("s", Some("devbox"), "/srv/r"), ssh_repo("bad", Some("-x"), "/srv/r"), repo("g", "github", "/c", None)]);
        m.set_ssh(true);
        // ssh 는 Rust 로 풀리지만 fetch 는 사이드카다(이중 fetch 방지) — 호스트가 잘못돼도 오류가 아니라 사이드카
        assert!(matches!(m.resolve("s"), Resolved::Rust(_)));
        for id in ["s", "bad", "g", "nope"] {
            assert_eq!(m.resolve_fetch(id), Resolved::Sidecar, "{id}");
        }
        assert_eq!(m.resolve_fetch("a"), Resolved::Rust(Target::Local { path: "/r".into() }));
    }

    #[test]
    fn 검사에_걸린_미러_값은_invalid() {
        let m = RepoMirror::default();
        m.apply(1, vec![repo("unc", "local", "\\\\host\\share", None), repo("nul", "local", "a\0b", None)]);
        assert!(matches!(m.resolve("unc"), Resolved::Invalid(_)));
        assert!(matches!(m.resolve("nul"), Resolved::Invalid(_)));
    }

    #[cfg(windows)]
    #[test]
    fn wsl_은_배포판이_있어야_한다() {
        let m = RepoMirror::default();
        m.apply(1, vec![repo("w", "wsl", "/home/me/r", Some("Ubuntu")), repo("x", "wsl", "/home/me/r", None), repo("y", "wsl", "/r", Some("-d"))]);
        assert_eq!(m.resolve("w"), Resolved::Rust(Target::Wsl { distro: "Ubuntu".into(), path: "/home/me/r".into() }));
        assert!(matches!(m.resolve("x"), Resolved::Invalid(_)));
        assert!(matches!(m.resolve("y"), Resolved::Invalid(_)));
    }

    #[test]
    fn 오래된_gen_은_버린다() {
        let m = RepoMirror::default();
        assert!(m.apply(2, vec![repo("a", "local", "/r", None)]));
        assert!(!m.apply(2, vec![]));
        assert!(!m.apply(1, vec![]));
        assert_eq!(m.count(), 1);
        assert!(m.apply(3, vec![]));
        assert_eq!(m.resolve("a"), Resolved::Sidecar);
    }

    #[test]
    fn 메시지의_틀린_항목만_뺀다() {
        let m = RepoMirror::default();
        let msg = json!({ "t": "repos", "gen": 1, "repos": [
            { "id": "a", "kind": "local", "path": "/r", "name": "에이" },
            { "id": 5, "kind": "local", "path": "/r" },
            { "id": "b", "kind": "wsl", "path": "/x", "distro": "U" },
            { "id": "c", "kind": "ssh", "path": "/srv/c", "sshHost": "devbox" },
            "x"
        ]});
        assert!(m.apply_message(&msg));
        assert_eq!(m.count(), 3);
        m.set_ssh(true);
        assert_eq!(m.resolve("c"), Resolved::Rust(Target::Ssh { host: "devbox".into(), path: "/srv/c".into() }));
        // 이름은 보낸 저장소에만 있다
        assert_eq!(m.name("a").as_deref(), Some("에이"));
        assert_eq!(m.name("b"), None);
        assert!(!m.apply_message(&json!({ "gen": "x", "repos": [] })));
        assert!(!m.apply_message(&json!({ "gen": 9 })));
    }
}
