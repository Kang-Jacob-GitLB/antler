//! 저장소 미러. 라우터가 저장소 경로·환경을 얻는 **유일한** 출처다. 설정을 쓰는 쪽은 Node 사이드카(main 의 ConfigStore)이고,
//! 사이드카가 시작 때(ready 전)와 목록이 바뀔 때마다 보내는 `{t:"repos", gen, repos:[{id,kind,path,distro?}]}` 로 채운다
//! (`src/sidecar/protocol.ts`·`mirror.ts`). renderer 가 보낸 경로·kind·배포판은 어디서도 쓰지 않는다.

use std::collections::HashMap;
use std::sync::{Arc, PoisonError, RwLock};

use serde_json::Value;

use crate::git::exec::wsl_paths::is_wsl_unc;
use crate::git::parsers::js_trim;

/// `src/main/ipc/handlers.ts` 의 `MAX_PATH_LEN`·`MAX_HOST_LEN`
const MAX_PATH_LEN: usize = 4096;
const MAX_DISTRO_LEN: usize = 255;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MirrorRepo {
    pub id: String,
    /// 저장소가 속한 소스의 종류: local · wsl · ssh · github
    pub kind: String,
    pub path: String,
    pub distro: Option<String>,
}

/// Rust 가 git 을 직접 돌려도 되는 저장소의 실행 대상(검증을 마친 값)
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Local { path: String },
    Wsl { distro: String, path: String },
}

impl Target {
    pub fn path(&self) -> &str {
        match self {
            Target::Local { path } | Target::Wsl { path, .. } => path,
        }
    }

    /// 큐·실행기를 나누는 키. TS 는 소스(local 하나, wsl 은 배포판마다 하나)마다 큐를 둔다
    pub fn source_key(&self) -> String {
        match self {
            Target::Local { .. } => "local".to_string(),
            Target::Wsl { distro, .. } => format!("wsl:{distro}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolved {
    /// 미러에 있고 kind 가 local·wsl 이며 검증을 통과했다
    Rust(Target),
    /// 미러에 없거나 Rust 가 다루지 않는 kind(ssh·github) — 사이드카가 처리한다(오류 문구도 사이드카가 만든다)
    Sidecar,
    /// 미러에 있는 값이 경로·배포판 검사에 걸렸다 — 이 메시지를 ERROR 로 돌려준다(사이드카로 넘기지 않는다)
    Invalid(String),
}

#[derive(Default)]
struct Inner {
    gen: u64,
    repos: HashMap<String, MirrorRepo>,
}

#[derive(Clone, Default)]
pub struct RepoMirror {
    inner: Arc<RwLock<Inner>>,
}

impl RepoMirror {
    /// 미러를 통째로 바꾼다. 이미 받은 것보다 gen 이 작거나 같으면 버린다(뒤처진 메시지). 바꿨으면 true
    pub fn apply(&self, gen: u64, repos: Vec<MirrorRepo>) -> bool {
        let mut inner = self.inner.write().unwrap_or_else(PoisonError::into_inner);
        if gen <= inner.gen {
            return false;
        }
        inner.gen = gen;
        inner.repos = repos.into_iter().map(|r| (r.id.clone(), r)).collect();
        true
    }

    /// 사이드카의 `repos` 메시지(JSON)를 받아 적용한다. 모양이 틀린 메시지는 버리고, 틀린 항목은 그 항목만 뺀다
    pub fn apply_message(&self, msg: &Value) -> bool {
        let Some(gen) = msg.get("gen").and_then(Value::as_u64) else { return false };
        let Some(list) = msg.get("repos").and_then(Value::as_array) else { return false };
        let repos = list.iter().filter_map(parse_repo).collect();
        self.apply(gen, repos)
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
            _ => Resolved::Sidecar,
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
        MirrorRepo { id: id.into(), kind: kind.into(), path: path.into(), distro: distro.map(Into::into) }
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
            { "id": "a", "kind": "local", "path": "/r" },
            { "id": 5, "kind": "local", "path": "/r" },
            { "id": "b", "kind": "wsl", "path": "/x", "distro": "U" },
            "x"
        ]});
        assert!(m.apply_message(&msg));
        assert_eq!(m.count(), 2);
        assert!(!m.apply_message(&json!({ "gen": "x", "repos": [] })));
        assert!(!m.apply_message(&json!({ "gen": 9 })));
    }
}
