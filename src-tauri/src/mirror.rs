//! 저장소 미러. 라우터가 저장소 경로·환경을 얻는 **유일한** 출처다. 설정을 쓰는 쪽은 Node 사이드카(main 의 ConfigStore)이고,
//! 사이드카가 시작 때(ready 전)와 목록이 바뀔 때마다 보내는 `{t:"repos", gen, repos:[{id,kind,path,name,distro?,sshHost?}]}` 로 채운다
//! (`src/sidecar/protocol.ts`·`mirror.ts`). renderer 가 보낸 경로·kind·배포판은 어디서도 쓰지 않는다.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, PoisonError, RwLock};

use serde_json::Value;

use crate::git::exec::wsl_paths::is_wsl_unc;
use crate::git::github_link::{is_safe_branch, sanitize_github_link};
use crate::git::parsers::js_trim;
use crate::net::json::Json;
use crate::git::scope::is_js_space;
use crate::router::rust_fetch_kinds;

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

/// GitHub 원격 전용 저장소의 `owner/repo` 링크(레코드의 값). `gh:repo` 가 git 없이 이 값으로 답한다
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GhLink {
    pub owner: String,
    pub repo: String,
}

/// Rust 가 git 을 직접 돌려도 되는 저장소의 실행 대상(검증을 마친 값)
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Local { path: String },
    Wsl { distro: String, path: String },
    /// `host` 는 검사를 마친(앞뒤 공백을 지운) 값이고 실행기·큐·풀의 키가 모두 이 값이다
    Ssh { host: String, path: String },
    /// GitHub 원격 전용 저장소(앱 캐시의 bare 저장소). 토큰·lazy fetch 환경은 **실행기가** 경로가 캐시 모양일 때만 연다(`GithubExecutor`) —
    /// 이 값은 경로·링크만 담고 환경을 정하지 않는다. `link` 는 레코드에 링크가 없으면 없다(그때 `gh:repo` 는 git 으로 읽는다)
    Github { path: String, link: Option<GhLink> },
}

impl Target {
    pub fn path(&self) -> &str {
        match self {
            Target::Local { path } | Target::Wsl { path, .. } | Target::Ssh { path, .. } | Target::Github { path, .. } => path,
        }
    }

    /// 소스 종류(`MirrorRepo::kind` 와 같은 값)
    pub fn kind(&self) -> &'static str {
        match self {
            Target::Local { .. } => "local",
            Target::Wsl { .. } => "wsl",
            Target::Ssh { .. } => "ssh",
            Target::Github { .. } => "github",
        }
    }

    /// 큐·실행기를 나누는 키. TS 는 소스(local 하나, wsl 은 배포판마다, ssh 는 호스트마다, github 하나)마다 큐를 둔다
    pub fn source_key(&self) -> String {
        match self {
            Target::Local { .. } => "local".to_string(),
            Target::Wsl { distro, .. } => format!("wsl:{distro}"),
            Target::Ssh { host, .. } => format!("ssh:{host}"),
            Target::Github { .. } => "github".to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolved {
    /// 미러에 있고 kind 가 local·wsl(ssh·github 는 스위치가 켜져 있을 때만)이며 검증을 통과했다
    Rust(Target),
    /// 미러에 없거나 Rust 가 다루지 않는 kind(스위치가 꺼진 ssh·github) — 사이드카가 처리한다(오류 문구도 사이드카가 만든다)
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
    /// 저장소 id → github 소스 저장소의 owner/repo 링크(`gh:repo` 의 답에만 쓴다. 경로·kind·환경 판정에는 쓰지 않는다 — `names` 와 같은 자리)
    links: HashMap<String, GhLink>,
    /// 저장소 id → 레코드의 기본 브랜치(`github.defaultBranch`, `is_safe_branch` 를 통과한 것만). fetch 가 HEAD 를 맞출 때 첫 후보로 쓴다.
    /// **`Target` 에 넣지 않는다** — 첫 fetch 가 브랜치를 기록해 미러가 바뀌어도 감시·로그 커서가 "대상이 바뀌었다"고 다시 시작하지 않게
    branches: HashMap<String, String>,
}

#[derive(Clone, Default)]
pub struct RepoMirror {
    inner: Arc<RwLock<Inner>>,
    /// ssh 저장소를 Rust 로 풀어도 되는가. 꺼져 있으면(기본) ssh 는 사이드카다. 라우터가 SSH 풀을 주입할 때 켠다(`Router::ssh_active`) —
    /// 해석이 이 한 곳에서만 갈려야 로그 커서·fetch 서비스·라우터가 같은 답을 본다
    ssh: Arc<AtomicBool>,
    /// github 소스 저장소를 Rust 로 풀어도 되는가(조회: Phase 6b PR 6, fetch·감시: PR 7). 꺼져 있으면(기본) github 는 사이드카다. 라우터가 GitHub 서비스(토큰)와 캐시 실행기를 주입할 때 켠다 —
    /// `resolve`(조회·로그 커서·`gh:*` 질의)와 fetch·감시의 소유(`resolve_fetch` — 종류 목록 `rust_fetch_kinds` 가 이 값을 입력으로 받는다)가 **같은 플래그 하나**를 본다
    github: Arc<AtomicBool>,
}

impl RepoMirror {
    /// 미러를 통째로 바꾼다. 이미 받은 것보다 gen 이 작거나 같으면 버린다(뒤처진 메시지). 바꿨으면 true
    pub fn apply(&self, gen: u64, repos: Vec<MirrorRepo>) -> bool {
        self.apply_named(gen, repos, HashMap::new(), HashMap::new(), HashMap::new())
    }

    /// github 링크가 있는 미러(시험·설정 소유 쪽이 쓴다). `links` 는 저장소 id → 링크
    pub fn apply_with_links(&self, gen: u64, repos: Vec<MirrorRepo>, links: HashMap<String, GhLink>) -> bool {
        self.apply_named(gen, repos, HashMap::new(), links, HashMap::new())
    }

    /// [`apply_with_links`](Self::apply_with_links) 에 레코드의 기본 브랜치(저장소 id → 브랜치)를 더한다(시험·설정 소유 쪽이 쓴다). 쓸 수 없는 이름은 버린다
    pub fn apply_with_branches(&self, gen: u64, repos: Vec<MirrorRepo>, links: HashMap<String, GhLink>, branches: HashMap<String, String>) -> bool {
        self.apply_named(gen, repos, HashMap::new(), links, branches)
    }

    fn apply_named(&self, gen: u64, repos: Vec<MirrorRepo>, names: HashMap<String, String>, links: HashMap<String, GhLink>, branches: HashMap<String, String>) -> bool {
        let mut inner = self.inner.write().unwrap_or_else(PoisonError::into_inner);
        if gen <= inner.gen {
            return false;
        }
        inner.gen = gen;
        inner.repos = repos.into_iter().map(|r| (r.id.clone(), r)).collect();
        inner.names = names;
        // github 소스가 아닌 저장소에 붙은 링크는 버린다
        inner.links = links.into_iter().filter(|(id, _)| inner.repos.get(id).is_some_and(|r| r.kind == "github")).collect();
        inner.branches = branches.into_iter().filter(|(id, b)| inner.repos.get(id).is_some_and(|r| r.kind == "github") && is_safe_branch(b)).collect();
        true
    }

    /// ssh 저장소를 Rust 로 풀지 정한다(라우터가 정한다)
    pub fn set_ssh(&self, on: bool) {
        self.ssh.store(on, Ordering::SeqCst);
    }

    /// github 소스 저장소의 조회를 Rust 로 풀지 정한다(라우터가 정한다)
    pub fn set_github(&self, on: bool) {
        self.github.store(on, Ordering::SeqCst);
    }

    /// 지금 github 저장소를 Rust 로 푸는가
    pub fn github_resolved(&self) -> bool {
        self.github.load(Ordering::SeqCst)
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

    /// github 저장소 레코드의 기본 브랜치(없거나 쓸 수 없는 이름이면 없다)
    pub fn github_branch(&self, repo_id: &str) -> Option<String> {
        self.inner.read().unwrap_or_else(PoisonError::into_inner).branches.get(repo_id).cloned()
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
        let links = list.iter().filter_map(|v| Some((v.get("id")?.as_str()?.to_string(), parse_link(v.get("github")?)?))).collect();
        // 기본 브랜치는 링크가 유효할 때만 받는다(링크가 버려진 레코드의 브랜치는 쓰지 않는다)
        let branches = list
            .iter()
            .filter_map(|v| {
                let github = v.get("github")?;
                parse_link(github)?;
                Some((v.get("id")?.as_str()?.to_string(), github.get("defaultBranch")?.as_str()?.to_string()))
            })
            .collect();
        self.apply_named(gen, repos, names, links, branches)
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
            // GitHub 원격 전용 저장소의 조회(Phase 6b PR 6): 라우터가 GitHub 서비스와 캐시 실행기를 주입했을 때만 Rust 다. 경로는 앱이 정한 캐시 경로이지만 local 과 같은 검사를 한다(UNC·NT 네임스페이스 거부).
            // 환경(토큰·lazy fetch)은 여기서 정하지 않는다 — 실행기가 경로가 캐시 모양일 때만 연다
            "github" if self.github.load(Ordering::SeqCst) => match check_path("local", &repo.path) {
                Ok(()) => Resolved::Rust(Target::Github { path: repo.path.clone(), link: inner.links.get(repo_id).cloned() }),
                Err(m) => Resolved::Invalid(m),
            },
            _ => Resolved::Sidecar,
        }
    }

    /// fetch 를 Rust 서비스가 맡는 저장소만 거른 해석. **소스 종류(`rust_fetch_kinds`)가 먼저다** — 해석 결과(`Resolved::Rust`)가 아니라 종류로 소유를 정해야
    /// 사이드카(`ANTLER_RUST_FETCH_KINDS`)와 같은 술어를 쓴다. 이 술어(ssh·github 는 각 스위치가 켜졌을 때만 포함)가 두 쪽에서 같으므로 이중 fetch 도 미소유도 없다.
    /// fetch·감시의 서비스·라우터·자동 예약은 모두 이 함수만 쓴다. 종류가 맡는 것이 아니면(꺼진 github·ssh) 검증보다 먼저 `Sidecar` 다 —
    /// 호스트가 잘못된 ssh 저장소의 fetch 는 켜져 있으면 Rust 가 `Invalid` 로, 꺼져 있으면 사이드카가 오류를 만든다
    pub fn resolve_fetch(&self, repo_id: &str) -> Resolved {
        let kinds = rust_fetch_kinds(self.ssh.load(Ordering::SeqCst), self.github.load(Ordering::SeqCst));
        let owned = {
            let inner = self.inner.read().unwrap_or_else(PoisonError::into_inner);
            inner.repos.get(repo_id).is_some_and(|r| kinds.split(',').any(|k| k == r.kind))
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

/// 미러 항목의 `github: {owner, repo}`. 링크 규칙(`validate_github_link`)을 통과하지 못하면 없다 — 사이드카가 이미 검증한 값이지만 `gh:repo` 의 답이 되기 전에 한 번 더 본다
fn parse_link(v: &Value) -> Option<GhLink> {
    let (owner, repo) = (v.get("owner")?.as_str()?, v.get("repo")?.as_str()?);
    let json = Json::Obj(vec![("owner".to_string(), Json::Str(owner.to_string())), ("repo".to_string(), Json::Str(repo.to_string()))]);
    let link = sanitize_github_link(Some(&json))?;
    Some(GhLink { owner: link.owner, repo: link.repo })
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

/// 원격 폴더 목록(`env:listDir`)의 경로 검사(handlers.ts:233-235). 비어 있어도 된다(홈을 연다). 4096 UTF-16 단위 이하·NUL 없음만 본다 —
/// SFTP 는 셸을 거치지 않으므로 그 밖의 문자는 서버가 판정한다
pub fn check_listdir_path(path: &str) -> Result<(), String> {
    if utf16_len(path) > MAX_PATH_LEN || path.contains('\0') {
        return Err("잘못된 경로입니다".to_string());
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
    fn github_는_스위치가_켜졌을_때만_조회와_fetch_소유를_rust_가_맡는다() {
        let m = RepoMirror::default();
        let msg = json!({ "t": "repos", "gen": 1, "repos": [
            { "id": "g", "kind": "github", "path": "C:\\c\\o\\r.git", "name": "r", "github": { "owner": "o", "repo": "r" } },
            { "id": "nolink", "kind": "github", "path": "C:\\c\\o\\x.git", "name": "x" },
            { "id": "unc", "kind": "github", "path": "\\\\host\\share\\o\\r.git", "name": "u" },
            { "id": "nt", "kind": "github", "path": "\\??\\UNC\\h\\s\\o\\r.git", "name": "n" },
            { "id": "a", "kind": "local", "path": "/r", "name": "a", "github": { "owner": "o", "repo": "r" } }
        ]});
        assert!(m.apply_message(&msg));
        // 기본(꺼짐)은 사이드카다
        for id in ["g", "nolink", "unc", "nt"] {
            assert_eq!(m.resolve(id), Resolved::Sidecar, "{id}");
        }
        m.set_github(true);
        let link = Some(GhLink { owner: "o".into(), repo: "r".into() });
        assert_eq!(m.resolve("g"), Resolved::Rust(Target::Github { path: "C:\\c\\o\\r.git".into(), link }));
        assert_eq!(m.resolve("nolink"), Resolved::Rust(Target::Github { path: "C:\\c\\o\\x.git".into(), link: None }));
        // local 과 같은 경로 검사(UNC·NT 네임스페이스는 git 을 돌리지 않는다)
        for id in ["unc", "nt"] {
            assert!(matches!(m.resolve(id), Resolved::Invalid(_)), "{id}");
        }
        // github 소스가 아닌 저장소에 붙은 링크는 버린다
        assert_eq!(m.resolve("a"), Resolved::Rust(Target::Local { path: "/r".into() }));
        // fetch·감시의 소유도 같은 스위치 하나다(종류 목록 `rust_fetch_kinds` 가 이 플래그를 입력으로 받는다 — 사이드카 env 와 같은 술어). 검증은 소유 판정 뒤다: 경로가 잘못된 것은 사이드카가 아니라 오류
        assert_eq!(m.resolve_fetch("g"), m.resolve("g"));
        assert_eq!(m.resolve_fetch("nolink"), m.resolve("nolink"));
        for id in ["unc", "nt"] {
            assert!(matches!(m.resolve_fetch(id), Resolved::Invalid(_)), "{id}");
        }
        m.set_github(false);
        for id in ["g", "nolink", "unc", "nt"] {
            assert_eq!(m.resolve_fetch(id), Resolved::Sidecar, "{id}: 스위치가 꺼지면 fetch 도 사이드카다");
        }
        m.set_github(true);
        let t = Target::Github { path: "/p".into(), link: None };
        assert_eq!((t.source_key().as_str(), t.kind(), t.path()), ("github", "github", "/p"));
    }

    #[test]
    fn 기본_브랜치는_유효한_링크와_쓸_수_있는_이름일_때만_받고_대상에는_넣지_않는다() {
        let m = RepoMirror::default();
        m.set_github(true);
        let rec = |id: &str, link: Value| json!({ "id": id, "kind": "github", "path": "C:\\c\\o\\r.git", "name": id, "github": link });
        let msg = json!({ "gen": 1, "repos": [
            rec("ok", json!({ "owner": "o", "repo": "r", "defaultBranch": "release/1.x" })),
            rec("none", json!({ "owner": "o", "repo": "r" })),
            rec("flag", json!({ "owner": "o", "repo": "r", "defaultBranch": "-x" })),
            rec("badlink", json!({ "owner": "-o", "repo": "r", "defaultBranch": "main" })),
            { "id": "local", "kind": "local", "path": "/r", "name": "l", "github": { "owner": "o", "repo": "r", "defaultBranch": "main" } }
        ]});
        assert!(m.apply_message(&msg));
        assert_eq!(m.github_branch("ok").as_deref(), Some("release/1.x"));
        for id in ["none", "flag", "badlink", "local", "nope"] {
            assert_eq!(m.github_branch(id), None, "{id}");
        }
        // 브랜치가 바뀌어도 해석 결과(= 감시·로그 커서가 비교하는 대상)는 같다
        let before = m.resolve("ok");
        assert!(m.apply_message(&json!({ "gen": 2, "repos": [rec("ok", json!({ "owner": "o", "repo": "r", "defaultBranch": "trunk" }))] })));
        assert_eq!(m.github_branch("ok").as_deref(), Some("trunk"));
        assert_eq!(m.resolve("ok"), before);
    }

    #[test]
    fn 링크는_규칙을_통과한_것만_받는다() {
        let m = RepoMirror::default();
        m.set_github(true);
        let bad = |owner: &str, repo: &str| json!({ "id": "g", "kind": "github", "path": "C:\\c\\o\\r.git", "github": { "owner": owner, "repo": repo } });
        for (i, (o, r)) in [("", "r"), ("o", ""), ("-o", "r"), ("o/x", "r"), ("o", "r.git"), ("o", ".."), ("o", "con")].into_iter().enumerate() {
            assert!(m.apply_message(&json!({ "gen": i + 1, "repos": [bad(o, r)] })));
            assert_eq!(m.resolve("g"), Resolved::Rust(Target::Github { path: "C:\\c\\o\\r.git".into(), link: None }), "{o}/{r}");
        }
        assert!(m.apply_message(&json!({ "gen": 99, "repos": [bad("Octo-Cat", "Hello.js")] })));
        assert_eq!(m.resolve("g"), Resolved::Rust(Target::Github { path: "C:\\c\\o\\r.git".into(), link: Some(GhLink { owner: "Octo-Cat".into(), repo: "Hello.js".into() }) }));
        // 링크는 gen 과 함께 통째로 바뀐다
        assert!(m.apply_message(&json!({ "gen": 100, "repos": [{ "id": "g", "kind": "github", "path": "C:\\c\\o\\r.git" }] })));
        assert_eq!(m.resolve("g"), Resolved::Rust(Target::Github { path: "C:\\c\\o\\r.git".into(), link: None }));
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
    fn fetch_소유는_해석이_아니라_소스_종류로_정하고_ssh_는_스위치가_켜졌을_때만_포함한다() {
        let m = RepoMirror::default();
        m.apply(1, vec![repo("a", "local", "/r", None), ssh_repo("s", Some("devbox"), "/srv/r"), ssh_repo("bad", Some("-x"), "/srv/r"), repo("g", "github", "/c", None)]);
        // 스위치가 꺼져 있으면(기본) ssh 의 fetch 는 사이드카다
        for id in ["s", "bad", "g", "nope"] {
            assert_eq!(m.resolve_fetch(id), Resolved::Sidecar, "{id}");
        }
        assert_eq!(m.resolve_fetch("a"), Resolved::Rust(Target::Local { path: "/r".into() }));
        m.set_ssh(true);
        // 켜지면 ssh 도 Rust 가 맡는다(사이드카 env 의 ssh 포함과 같은 술어). 호스트가 잘못되면 사이드카가 아니라 오류다
        assert_eq!(m.resolve_fetch("s"), Resolved::Rust(Target::Ssh { host: "devbox".into(), path: "/srv/r".into() }));
        assert!(matches!(m.resolve_fetch("bad"), Resolved::Invalid(_)));
        for id in ["g", "nope"] {
            assert_eq!(m.resolve_fetch(id), Resolved::Sidecar, "{id}");
        }
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
