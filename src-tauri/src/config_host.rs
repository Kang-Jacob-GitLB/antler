//! 설정 소유 채널 11개(Phase 6b PR 9)의 Rust 쪽 처리: `config:get`·`config:setUi`·`repos:add`·`repos:addMany`·`repos:addGithub`·`repos:update`·`repos:remove`·`repos:watch`·`external:open`·`external:pickEditorExe`·`req:cancel`.
//! 라우터가 `CONFIG_CHANNELS`(router.rs)를 이 모듈로 넘기면 `src/main/ipc/handlers.ts`·`external.ts` 와 `RepoService` 의 같은 입력 검증·결과·오류 봉투(`{ok:false,error,code}`, void 는 `data` 없는 `{ok:true}`)를 돌려준다.
//!
//! **Rust 가 설정 파일의 유일한 작성자다**(`ConfigStore`). 한 실행 동안 쓰는 쪽은 하나다 — 이 호스트가 주입되면(`Router::set_config`, `ANTLER_RUST_GIT≠0`) 사이드카는 설정을 읽기 전용 빈 값으로만 안다.
//! 변경은 모두 `ConfigStore` 의 변경자를 거치고, 변경 알림이 **응답보다 먼저** 미러·fetch 설정을 맞춘다(`config::sync`).
//!
//! 입력은 renderer 가 보낸 값이다 — 환경·경로·호스트는 `EnvHost::check_env_input`(TS `checkEnvInput` — 확인 창·승인 기억 포함)을 거치고, 저장소 루트·환경은 **설정에서만** 읽는다(`external:open`).
//! 승인 기억은 `EnvHost` 하나다(`env:test`·`repos:scan`·`repos:add` 가 같은 `EnvHost` 를 쓴다 — PR 8 의 확인 창 중복이 사라진다).
//! 대화상자·프로그램 실행은 **포트로 주입**한다([`ConfigHost`]) — 시험은 가짜를 달아 실제 창·프로세스를 건드리지 않는다.
//! 이 모듈은 `Sidecar`·`AppHandle` 에 기대지 않는다 — 통합 테스트(`tests/config_route.rs`)가 그대로 돌릴 수 있다.

use std::sync::Arc;

use serde_json::{json, Value};

use crate::config::model::js_get;
use crate::config::{source_for, ConfigStore};
use crate::env_host::EnvHost;
use crate::external::opener::{check_chosen_exe, EditorPrefs, ExternalAction, ExternalOpener, OpenerDeps};
use crate::external::plan::{EditorKind, Target};
use crate::git::exec::ssh::HoldGuard;
use crate::git::exec::{ExecError, GitExecutor, RunOpts};
use crate::git::fetch::js_trim;
use crate::git::exec::wsl_paths::wsl_from_unc;
use crate::mirror::check_path;
use crate::net::json::{parse, Json};
use crate::repos::env::{remote_relative, resolve_env, with_host_key_hint, EnvInput, ResolvedEnv};
use crate::repos::manage::{add_github_repos, blocking, canonical_key, github_target_of, remove_cache_dir, PathLocks};
use crate::router::Router;

const BAD_INPUT: &str = "잘못된 입력입니다";
const BAD_PATH: &str = "잘못된 경로입니다";
const NOT_FOUND: &str = "저장소를 찾을 수 없습니다";
const MAX_ADD_MANY: usize = 500;
/// `external:open` 의 동작 허용 목록(TS `ACTIONS`)
const ACTIONS: [&str; 3] = ["fileManager", "editor", "terminal"];

/// 에디터 실행 파일을 고르는 대화상자(파일 하나·`.exe`/`.com` 필터). 취소·창 없음이면 `None`. 막는 호출이다
pub type PickExe = Arc<dyn Fn() -> Result<Option<String>, String> + Send + Sync>;
/// 프로세스 실행·파일 관리자·파일 시스템을 건드리는 주입물(운영은 `external::launch::SystemDeps`)
pub type OpenerPort = Arc<dyn OpenerDeps + Send + Sync>;

/// 설정 소유자. 라우터가 하나만 가진다
pub struct ConfigHost {
    store: Arc<ConfigStore>,
    env: EnvHost,
    cache_dir: String,
    pick_exe: PickExe,
    opener: OpenerPort,
    locks: PathLocks,
}

impl ConfigHost {
    /// `env` 는 라우터의 환경 채널과 **같은 `EnvHost` 의 복제**여야 한다(승인 기억을 나눈다). `cache_dir` 는 GitHub 원격 전용 저장소의 캐시 기준 폴더이고
    /// `ConfigStore::open` 에 넘긴 것과 같아야 한다
    pub fn new(store: Arc<ConfigStore>, env: EnvHost, cache_dir: String, pick_exe: PickExe, opener: OpenerPort) -> Self {
        ConfigHost { store, env, cache_dir, pick_exe, opener, locks: PathLocks::default() }
    }

    pub fn store(&self) -> &Arc<ConfigStore> {
        &self.store
    }

    pub fn cache_dir(&self) -> &str {
        &self.cache_dir
    }

    pub fn locks(&self) -> &PathLocks {
        &self.locks
    }
}

/// 실패 봉투의 내용
pub struct Fail {
    error: String,
    code: &'static str,
}

impl From<String> for Fail {
    fn from(error: String) -> Self {
        Fail { error, code: "ERROR" }
    }
}

impl From<&str> for Fail {
    fn from(error: &str) -> Self {
        Fail { error: error.to_string(), code: "ERROR" }
    }
}

impl From<ExecError> for Fail {
    fn from(e: ExecError) -> Self {
        Fail { code: e.envelope_code(), error: e.to_string() }
    }
}

type R<T> = Result<T, Fail>;

/// 설정 트리를 응답 값으로. `serde_json` 의 `preserve_order` 가 꺼져 있어 **객체 키는 정렬돼 나간다**(renderer 는 순서에 기대지 않는다 — 파일 바이트는 순서를 보존하는 `Json` 트리가 쓴다)
pub fn to_value(j: &Json) -> Value {
    serde_json::from_str(&crate::net::js_value::stringify(j)).unwrap_or(Value::Null)
}

/// renderer 의 인자를 설정 트리로(깊이 상한을 넘으면 거부)
fn to_json(v: &Value) -> R<Json> {
    parse(&v.to_string()).map_err(|_| Fail::from("잘못된 입력입니다(중첩이 너무 깊습니다)"))
}

fn str_of(v: Option<&Json>) -> Option<&str> {
    v.and_then(Json::as_str)
}

/// 설정 소유 채널 하나를 처리해 IpcEnvelope 를 돌려준다. `args` 는 표식을 되돌린 인자 배열(`None` = undefined)이다
pub async fn config_envelope(router: &Router, host: &Arc<ConfigHost>, channel: &str, args: Vec<Option<Value>>) -> Value {
    let a = |i: usize| args.get(i).and_then(Option::as_ref);
    let result: R<Option<Value>> = match channel {
        "config:get" => Ok(Some(to_value(&host.store.get()))),
        "config:setUi" => set_ui(host, a(0)).await.map(Some),
        "repos:add" => add_repo(router, host, a(0)).await.map(Some),
        "repos:addMany" => add_many(router, host, a(0)).await.map(Some),
        "repos:addGithub" => add_github(router, host, a(0)).await.map(Some),
        "repos:update" => update_repo(router, host, a(0), a(1)).await.map(Some),
        "repos:remove" => remove_repo(router, host, a(0)).await.map(Some),
        "repos:watch" => {
            // renderer 는 응답을 기다리지 않는다(`void`). 활성 저장소가 아니면(null) 전부 멈춘다
            router.watch_repo(a(0).and_then(Value::as_str));
            Ok(None)
        }
        "external:open" => external_open(host, a(0), a(1)).await.map(|()| None),
        "external:pickEditorExe" => pick_editor_exe(host).await,
        "req:cancel" => {
            if let Some(id) = a(0).and_then(Value::as_str) {
                router.cancel_request(id);
            }
            Ok(None)
        }
        other => Err(Fail::from(format!("알 수 없는 채널: {other}"))),
    };
    match result {
        Ok(Some(data)) => json!({ "ok": true, "data": data }),
        Ok(None) => json!({ "ok": true }),
        Err(f) => json!({ "ok": false, "error": f.error, "code": f.code }),
    }
}

// ── config:* · repos:update ──────────────────────────────────────────────

async fn set_ui(host: &ConfigHost, patch: Option<&Value>) -> R<Value> {
    // TS: `patch.githubClientId` — 없는 값·null 은 TypeError 다(검증기가 같은 문구로 거부한다)
    let patch = match patch {
        None => Json::Null,
        Some(v) => to_json(v)?,
    };
    let store = host.store.clone();
    let out = blocking(move || store.set_ui(&patch)).await??;
    Ok(to_value(&out))
}

async fn update_repo(router: &Router, host: &ConfigHost, id: Option<&Value>, patch: Option<&Value>) -> R<Value> {
    let id = id.and_then(Value::as_str);
    // TS 핸들러가 저장소를 바꾸기 전에 부르는 `invalidateGhRemotes(repoId)` — 모든 GitHub remote 목록 캐시를 비운다
    router.invalidate_gh_remotes(id);
    let patch = match patch {
        None => Json::Null,
        Some(v) => to_json(v)?,
    };
    let (store, id) = (host.store.clone(), id.unwrap_or("").to_string());
    let out = blocking(move || store.update_repo(&id, &patch)).await??;
    Ok(to_value(&out))
}

// ── repos:remove ─────────────────────────────────────────────────────────

/// 저장소를 지운다. GitHub 원격 전용 저장소는 **fetch 정지 → 감시 정지 → 진행 중 조회·로그 커서 드레인 → 캐시 폴더 삭제 → 레코드 삭제** 순서다
/// (`Router::release_github_repo` 가 앞 셋을 한다 — PR 6·7). 폴더를 못 지워도 레코드는 지운다(남은 폴더는 다음 시작 때 고아 정리가 지운다).
/// 폴더는 **레코드의 링크와 캐시 경로가 맞을 때만**(`github_target_of` — `cache_path_matches_link`) 지운다
async fn remove_repo(router: &Router, host: &ConfigHost, id: Option<&Value>) -> R<Value> {
    let id = id.and_then(Value::as_str).unwrap_or("").to_string();
    router.invalidate_gh_remotes(Some(&id));
    // 레코드는 지우기 **전에** 읽는다(경로·링크가 그 레코드의 것이다)
    let record = host.store.repo(&id);
    let is_github = record.as_ref().and_then(|r| str_of(js_get(r, "sourceId"))).and_then(|s| host.store.source(s)).is_some_and(|s| str_of(js_get(&s, "kind")) == Some("github"));
    if is_github {
        router.release_github_repo(&id).await;
        if let Some((gp, link)) = record.as_ref().and_then(|r| github_target_of(&host.store, &host.cache_dir, r)) {
            // 같은 경로의 추가·고아 정리와 겹치지 않게 경로 잠금 **안에서** 레코드를 다시 읽어 아직 같은 레코드(id·링크·경로)인지 보고,
            // 폴더 삭제와 레코드 삭제를 모두 이 잠금 안에서 한다 — 잠금을 놓은 사이에 끼어든 같은 owner/repo 의 재추가가 만든 새 캐시를 지우지 않고,
            // `addGithub` 가 폴더가 이미 지워진 삭제 중 레코드를 등록된 것으로 보지 않는다
            let _guard = host.locks.lock(&canonical_key(&host.cache_dir, gp.as_str())).await;
            let same = host.store.repo(&id).and_then(|r| github_target_of(&host.store, &host.cache_dir, &r)).is_some_and(|(p, l)| p == gp && l.owner == link.owner && l.repo == link.repo);
            if same {
                let target = gp.clone();
                if let Err(e) = blocking(move || remove_cache_dir(&target)).await? {
                    eprintln!("[repos] 원격 저장소 캐시 폴더를 지우지 못했다(다음 시작 때 정리한다): {e}");
                }
            }
            let store = host.store.clone();
            let out = blocking(move || store.remove_repo(&id)).await??;
            return Ok(to_value(&out));
        }
    }
    let store = host.store.clone();
    let out = blocking(move || store.remove_repo(&id)).await??;
    Ok(to_value(&out))
}

// ── repos:add · repos:addMany ────────────────────────────────────────────

/// 저장소 최상위를 읽는 실행기: local·wsl 은 실행기 하나, ssh 는 풀에서 **잡은**(`hold`) 실행기(저장 전 호스트라 다른 미러 변경이 연결을 끊지 않는다)
enum Runner {
    Exec(GitExecutor),
    Ssh { guard: HoldGuard, host: String },
}

impl Runner {
    /// 환경의 실행기를 만든다. ssh 는 풀의 `hold` 를 이 값이 버려질 때까지 쥔다(처음 쓰는 호스트가 실패하면 연결을 남기지 않는다 — 마지막 잡기가 풀릴 때 미러에 없으면 닫힌다)
    fn open(router: &Router, host: &ConfigHost, env: &ResolvedEnv) -> R<Runner> {
        if env.kind == "ssh" {
            let ssh_host = env.target.clone().unwrap_or_default();
            let pool = router.ssh_pool().ok_or_else(|| Fail::from("SSH 를 사용할 수 없습니다"))?;
            return Ok(Runner::Ssh { guard: pool.hold(&ssh_host), host: ssh_host });
        }
        Ok(Runner::Exec((host.env.exec_for)(env.kind, env.target.as_deref())))
    }

    /// `git rev-parse --show-toplevel`(TS `resolveTopLevel`): 비어 있지 않은 최상위 경로. 0 이 아닌 종료는 `GIT` 오류다
    async fn top_level(&self, rel: &str) -> R<String> {
        let args = ["rev-parse", "--show-toplevel"];
        let opts = RunOpts::default();
        let ran = match self {
            Runner::Exec(e) => e.run(rel, &args, &opts).await,
            Runner::Ssh { guard, .. } => guard.executor().run(rel, &args, &opts).await,
        };
        let hint = |e: ExecError| {
            let error = match self {
                Runner::Ssh { host, .. } => with_host_key_hint(host, &e.to_string()),
                Runner::Exec(_) => e.to_string(),
            };
            Fail { error, code: e.envelope_code() }
        };
        let out = ran.map_err(hint)?;
        if out.code != 0 {
            let stderr = js_trim(&out.stderr);
            let error = if stderr.is_empty() { format!("git rev-parse failed ({})", out.code) } else { stderr.to_string() };
            return Err(Fail { error, code: "GIT" });
        }
        Ok(js_trim(&String::from_utf8_lossy(&out.stdout)).to_string())
    }
}

/// 경로의 마지막 구성 요소(TS: `normalized.split(/[\\/]/).filter(Boolean).pop() ?? normalized`)
fn repo_name_of(path: &str) -> String {
    path.split(['\\', '/']).rfind(|s| !s.is_empty()).unwrap_or(path).to_string()
}

/// 환경 하나에서 경로 하나를 추가한다(TS `addRepo`). `group` 은 trim 한 값(비면 없음)
async fn add_path(host: &ConfigHost, runner: &Runner, env: &ResolvedEnv, group: &str) -> R<Json> {
    if env.path.is_empty() {
        return Err("경로를 입력하세요".into());
    }
    // 배포판 이름은 설치된 표기로 맞춰 오지만(`check_env_input`), 설정에 이미 대소문자만 다른 같은 배포판 소스(Electron 에서 가져온 레코드)가 있으면 그 표기를 쓴다 —
    // 소스 id(`wsl:<이름>`)가 달라져 같은 저장소가 소스 둘·레코드 둘로 갈라지지 않게. TS 는 입력 표기를 그대로 쓰지만 설치 목록과 정확히 일치해야 통과하므로 결과가 같다
    let existing_distro = (env.kind == "wsl")
        .then(|| match host.store.get() {
            Json::Obj(cfg) => match crate::config::model::get(&cfg, "sources") {
                Some(Json::Arr(sources)) => sources
                    .iter()
                    .filter(|s| str_of(js_get(s, "kind")) == Some("wsl"))
                    .filter_map(|s| str_of(js_get(s, "distro")))
                    .find(|d| env.target.as_deref().is_some_and(|t| d.eq_ignore_ascii_case(t)))
                    .map(str::to_string),
                _ => None,
            },
            _ => None,
        })
        .flatten();
    let source = source_for(env.kind, existing_distro.as_deref().or(env.target.as_deref()))?;
    let rel = remote_relative(env.kind, &env.path);
    // 고른 폴더(로컬)가 링크로 원격에 이어지면 `git -C` 가 따라가는 순간 SMB 인증(NTLM)이 샌다 — git 을 띄우기 전에 구성 요소 단위로 확인한다(`repos:scan` 과 같은 관문)
    if cfg!(windows) && env.kind == "local" {
        let root = env.path.clone();
        blocking(move || crate::fsguard::check_local_root(&root, crate::fsguard::is_remote_link_target)).await??;
    }
    let top = runner.top_level(&rel).await?;
    // Windows git 은 C:/a/b 형식으로 돌려주므로 사용자가 고른 표기(역슬래시)로 맞춘다
    let normalized = if env.kind == "local" && cfg!(windows) { top.replace('/', "\\") } else { top };
    // git 이 돌려준 경로도 쓸 때마다 미러가 다시 보는 검사를 통과해야 한다 — 아니면 열 때마다 거부되는 레코드가 저장된다
    check_path(env.kind, &normalized)?;
    let name = repo_name_of(&normalized);
    let mut rec = vec![("sourceId".to_string(), source.get("id").cloned().unwrap_or(Json::Null)), ("path".to_string(), Json::Str(normalized)), ("name".to_string(), Json::Str(name))];
    if !group.is_empty() {
        rec.push(("group".to_string(), Json::Str(group.to_string())));
    }
    let (store, rec) = (host.store.clone(), Json::Obj(rec));
    let repo = blocking(move || store.add_repo(&rec, Some(&source))).await??;
    // 이미 등록된 저장소를 다시 추가하면서 그룹을 골랐으면 그 그룹으로 옮긴다(조용히 버리지 않는다)
    if !group.is_empty() && str_of(js_get(&repo, "group")) != Some(group) {
        let id = str_of(js_get(&repo, "id")).unwrap_or_default().to_string();
        let patch = Json::Obj(vec![("group".to_string(), Json::Str(group.to_string()))]);
        let store = host.store.clone();
        blocking(move || store.update_repo(&id, &patch)).await??;
    }
    let id = str_of(js_get(&repo, "id")).unwrap_or_default();
    Ok(host.store.repo(id).unwrap_or(repo))
}

/// 그룹 입력(검증은 `check_env_input` 이 이미 했다 — 문자열이 아니거나 200자를 넘으면 거기서 거부된다). JS trim 한 값
fn group_of(input: &Value) -> String {
    input.get("group").and_then(Value::as_str).map(|g| js_trim(g).to_string()).unwrap_or_default()
}

async fn add_repo(router: &Router, host: &ConfigHost, input: Option<&Value>) -> R<Value> {
    // 환경·경로 검증(설정에 없는 SSH 호스트는 확인 창 — 승인 기억은 환경 채널과 같다)
    let env = host.env.check_env_input(input, true).await?;
    let group = input.map(group_of).unwrap_or_default();
    let runner = Runner::open(router, host, &env)?;
    let repo = add_path(host, &runner, &env, &group).await?;
    Ok(to_value(&repo))
}

/// TS `wslFromUnc(String(p))?.distro`
fn unc_distro(v: &Value) -> Option<String> {
    let s = match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    wsl_from_unc(&s).map(|(d, _)| d)
}

async fn add_many(router: &Router, host: &ConfigHost, input: Option<&Value>) -> R<Value> {
    // 핸들러의 모양·개수 검사
    let Some(obj) = input.and_then(Value::as_object) else { return Err(BAD_INPUT.into()) };
    let paths = match obj.get("paths") {
        Some(Value::Array(p)) if !p.is_empty() && p.len() <= MAX_ADD_MANY => p,
        _ => return Err(BAD_INPUT.into()),
    };
    let kind = obj.get("kind").and_then(Value::as_str).unwrap_or("");
    let first_distro = unc_distro(&paths[0]);
    for p in paths {
        // 모든 경로에 같은 경로 검사(문자열·길이·NT 네임스페이스·UNC)를 한다
        match p.as_str() {
            Some(s) => check_path(kind, s)?,
            None => return Err(BAD_PATH.into()),
        }
        if unc_distro(p) != first_distro {
            return Err("한 번에 추가하는 저장소는 같은 환경이어야 합니다".into());
        }
    }
    // 환경 검증(설정에 없는 SSH 호스트 확인 창 포함)은 한 번만 한다. 모든 경로가 한 폴더 아래라 첫 경로로 WSL 배포판을 본다
    let mut first = serde_json::Map::new();
    for key in ["kind", "distro", "sshHost", "group"] {
        if let Some(v) = obj.get(key) {
            first.insert(key.to_string(), v.clone());
        }
    }
    first.insert("path".to_string(), paths[0].clone());
    let first = Value::Object(first);
    let env0 = host.env.check_env_input(Some(&first), true).await?;
    let group = group_of(&first);
    let (distro, ssh_host) = (obj.get("distro").and_then(Value::as_str), obj.get("sshHost").and_then(Value::as_str));
    let (mut added, mut failed): (Vec<Value>, Vec<Value>) = (Vec::new(), Vec::new());
    // 묶음 동안 실행기를 잡아 둔다 — 처음 쓰는 원격 환경에서 앞쪽 경로가 실패해도 연결을 매번 다시 맺지 않는다
    let runner = Runner::open(router, host, &env0)?;
    for p in paths {
        let path = p.as_str().unwrap_or_default();
        let one = async {
            // 환경은 첫 경로에서 이미 확인했다 — 경로마다 해석만 다시 한다(WSL 목록·확인 창을 500번 부르지 않는다)
            let mut env = resolve_env(&EnvInput { kind, path, distro: env0.target.as_deref().filter(|_| env0.kind == "wsl").or(distro), ssh_host })?;
            if env.kind != env0.kind || env.target.as_deref().map(str::to_lowercase) != env0.target.as_deref().map(str::to_lowercase) {
                return Err(Fail::from("한 번에 추가하는 저장소는 같은 환경이어야 합니다"));
            }
            // 설치된 표기·검증된 호스트 값으로 맞춘다(첫 경로가 확인한 값)
            env.target.clone_from(&env0.target);
            add_path(host, &runner, &env, &group).await
        };
        match one.await {
            Ok(repo) => {
                let value = to_value(&repo);
                if !added.iter().any(|r| r.get("id") == value.get("id")) {
                    added.push(value);
                }
            }
            Err(f) => failed.push(json!({ "path": path, "error": f.error })),
        }
    }
    Ok(json!({ "added": added, "failed": failed }))
}

// ── repos:addGithub ──────────────────────────────────────────────────────

async fn add_github(router: &Router, host: &ConfigHost, input: Option<&Value>) -> R<Value> {
    let json = input.map(to_json).transpose()?;
    Ok(add_github_repos(&host.store, router.github_executor(), &host.cache_dir, &host.locks, json.as_ref()).await?)
}

// ── external:* ───────────────────────────────────────────────────────────

/// `external:open`(TS `external.ts`): 동작 허용 목록·저장소 id → **설정에서** 루트·환경을 읽는다. GitHub 원격 전용 저장소는 작업 폴더가 없어 거부한다
async fn external_open(host: &ConfigHost, repo_id: Option<&Value>, req: Option<&Value>) -> R<()> {
    let Some(req) = req.and_then(Value::as_object) else { return Err(BAD_INPUT.into()) };
    let action = req.get("action").and_then(Value::as_str).filter(|a| ACTIONS.contains(a)).and_then(ExternalAction::parse);
    // `path` 는 문자열이거나 없다(null 은 거부 — Rust 의 opener 는 null 을 루트로 받으므로 여기서 먼저 거른다)
    let rel = match req.get("path") {
        None => Value::Null,
        Some(v @ Value::String(_)) => v.clone(),
        Some(_) => return Err(BAD_INPUT.into()),
    };
    let Some(action) = action else { return Err(BAD_INPUT.into()) };
    let repo = repo_id.and_then(Value::as_str).and_then(|id| host.store.repo(id));
    let source = repo.as_ref().and_then(|r| str_of(js_get(r, "sourceId"))).and_then(|s| host.store.source(s));
    let (Some(repo), Some(source)) = (repo, source) else { return Err(NOT_FOUND.into()) };
    let root = str_of(js_get(&repo, "path")).unwrap_or_default().to_string();
    let kind = str_of(js_get(&source, "kind")).unwrap_or_default();
    // 원격 전용 저장소는 작업 폴더가 없다(path 는 앱 캐시) — 폴더·에디터·터미널로 열 대상이 아니다
    if kind == "github" {
        return Err("원격 전용 저장소는 작업 폴더가 없어 열 수 없습니다".into());
    }
    // 설정에 든 루트도 쓰기 전에 미러가 쓸 때마다 하는 검사(NT 네임스페이스·UNC(WSL 제외)·NUL·길이)를 거친다 — 손상·변조된 설정 파일의 UNC 루트로 `realpath`·`stat` 이 SMB 에 접속(NTLM)하지 않게
    check_path(if matches!(kind, "wsl" | "ssh") { kind } else { "local" }, &root)?;
    let target = match kind {
        "wsl" => Target::Wsl { root, distro: str_of(js_get(&source, "distro")).unwrap_or_default().to_string() },
        "ssh" => Target::Ssh { root, host: str_of(js_get(&source, "sshHost")).unwrap_or_default().to_string() },
        _ => Target::Local { root },
    };
    let ui = editor_prefs(&host.store.get());
    let deps = host.opener.clone();
    blocking(move || ExternalOpener::new(&*deps).open(&target, action, &rel, &ui)).await?.map_err(Fail::from)
}

/// 설정의 `ui` 에서 에디터 설정 둘(`editor`·`editorExePath`)
fn editor_prefs(config: &Json) -> EditorPrefs {
    let ui = js_get(config, "ui");
    EditorPrefs {
        editor: ui.and_then(|u| str_of(js_get(u, "editor"))).and_then(EditorKind::parse),
        editor_exe_path: ui.and_then(|u| str_of(js_get(u, "editorExePath"))).map(str::to_string),
    }
}

/// `external:pickEditorExe`: 파일 선택 → `check_chosen_exe`(절대 경로·존재·`.exe`/`.com`) → 설정에 저장한다. 취소면 `null`
async fn pick_editor_exe(host: &ConfigHost) -> R<Option<Value>> {
    let pick = host.pick_exe.clone();
    let Some(path) = blocking(move || pick()).await?? else { return Ok(Some(Value::Null)) };
    let (deps, store) = (host.opener.clone(), host.store.clone());
    let out = blocking(move || {
        let exe = check_chosen_exe(&path, deps.platform(), Some(&*deps))?;
        store.set_editor_exe_path(&exe)
    })
    .await??;
    Ok(Some(to_value(&out)))
}
