//! GitHub 원격 전용 저장소의 추가·삭제·고아 정리(Phase 6b PR 9 — `RepoService.addGithubRepos`·`removeGithubCache`·`cleanupGithubCache` 의 이식).
//! 저장소 쓰기 규칙의 유일한 예외(앱 캐시 `<userData>/remote-repos/<owner>/<repo>.git`)를 만들고 지우는 곳이다 — **`GithubCachePath`(캐시 모양을 통과한 경로) 만 받는 함수만 쓴다.**
//!
//! - 경로 잠금([`PathLocks`]): 같은 캐시 경로에 대한 추가·삭제·고아 정리를 차례로 돌린다(TS `pathLocks`). 한 프로세스(Rust)가 설정과 캐시를 모두 쥐므로 가능하다.
//! - 삭제는 **레코드의 owner/repo 와 캐시 경로가 맞을 때만**(`cache_path_matches_link`) 지운다 — 다른 저장소의 캐시를 가리키는 레코드가 그 폴더를 지우지 못한다. 순서(fetch 정지 → 감시 정지 → 조회 드레인 뒤 삭제)는 호출자(`config_host`)가 지킨다.
//! - 고아 정리: 시작 때 한 번, 설정을 읽지 못한 채 시작했으면(`load_failed` — 저장소 목록이 빈 것이 사실이 아니다) 아무것도 지우지 않는다.
//! - 복구: 레코드가 있는데 캐시 폴더가 없으면(Electron 설정 가져오기는 폴더를 가져오지 않는다 — D1) 네트워크 없는 `init` 으로 빈 캐시를 다시 만든다. 내용은 처음 열 때 fetch 한다.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};

use serde_json::{json, Value};

use crate::config::model::js_get;
use crate::config::{source_for, ConfigStore};
use crate::git::exec::{GitExecutor, GithubExecutor};
use crate::git::fetch::js_trim;
use crate::git::github_cache::{init_github_repo, remove_github_cache, GithubCachePath};
use crate::git::github_link::{cache_path_matches_link, github_cache_path, github_cache_slug, sanitize_github_link, validate_github_link, GithubLink, GITHUB_SOURCE_ID, MAX_ADD_GITHUB};
use crate::net::json::Json;

/// 한 번에 추가하는 저장소의 그룹 이름 상한(UTF-16 단위)
const MAX_GROUP_LEN: usize = 200;
const BAD_INPUT: &str = "잘못된 입력입니다";
const BAD_GROUP: &str = "잘못된 그룹 이름입니다";
const NOT_AVAILABLE: &str = "GitHub 원격 저장소를 쓸 수 없는 환경입니다";

/// 캐시 경로 하나에 대한 작업을 차례로 돌리는 잠금(TS `withPathLock`). 항목은 가볍고(저장소 수만큼) 아무도 쥐지 않은 것은 자라지 않게 가끔 치운다
#[derive(Default)]
pub struct PathLocks {
    map: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

impl PathLocks {
    /// 그 경로의 잠금을 얻는다. 잠금은 대소문자를 구분하지 않는다(Windows 경로)
    pub async fn lock(&self, path: &str) -> tokio::sync::OwnedMutexGuard<()> {
        let cell = {
            let mut map = self.map.lock().unwrap_or_else(PoisonError::into_inner);
            if map.len() > 1024 {
                map.retain(|_, m| Arc::strong_count(m) > 1);
            }
            map.entry(path.to_lowercase()).or_default().clone()
        };
        cell.lock_owned().await
    }
}

fn str_of(v: Option<&Json>) -> Option<&str> {
    v.and_then(Json::as_str)
}

/// 레코드가 GitHub 소스이고 링크가 유효하며 캐시 경로가 그 링크의 것일 때만 그 폴더(`GithubCachePath`)와 링크를 돌려준다 — 삭제의 유일한 관문이다
pub fn github_target_of(store: &ConfigStore, cache_dir: &str, record: &Json) -> Option<(GithubCachePath, GithubLink)> {
    let source = store.source(str_of(js_get(record, "sourceId"))?)?;
    if str_of(js_get(&source, "kind")) != Some("github") {
        return None;
    }
    let link = sanitize_github_link(js_get(record, "github"))?;
    let path = str_of(js_get(record, "path"))?;
    if !cache_path_matches_link(cache_dir, path, &link.owner, &link.repo) {
        return None;
    }
    Some((GithubCachePath::new(cache_dir, path)?, link))
}

/// 캐시 폴더 하나를 지운다(블로킹 — 재시도에 잠잘 수 있다). 호출자는 이 저장소의 git(조회·fetch·감시)을 이미 멈췄다. 폴더가 없거나 링크면 `false`, 못 지우면 `Err`
pub fn remove_cache_dir(path: &GithubCachePath) -> Result<bool, String> {
    remove_github_cache(path)
}

fn is_link(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink())
}

fn js_string(v: Option<&Json>) -> String {
    match v {
        None => "undefined".to_string(),
        Some(Json::Null) => "null".to_string(),
        Some(Json::Bool(b)) => b.to_string(),
        Some(Json::Num(n)) => crate::net::json::js_number_string(*n),
        Some(Json::Str(s)) => s.clone(),
        // JS `String([a, b])` = `a,b`(null·undefined 항목은 빈 문자열, 중첩 배열은 재귀)
        Some(Json::Arr(a)) => a.iter().map(|x| if matches!(x, Json::Null) { String::new() } else { js_string(Some(x)) }).collect::<Vec<_>>().join(","),
        Some(Json::Obj(_)) => "[object Object]".to_string(),
    }
}

/// 캐시 경로의 정규형(`github_cache_path` — 구분자·`..` 를 정리하고 소문자) 비교·잠금 키. 캐시 모양이 아니면 구분자만 맞춘 소문자 경로
pub(crate) fn canonical_key(cache_dir: &str, path: &str) -> String {
    match github_cache_slug(cache_dir, path) {
        Some((owner, repo)) => github_cache_path(cache_dir, &owner, &repo),
        None => path.to_string(),
    }
    .to_lowercase()
    .replace('/', "\\")
}

/// 오류 목록의 항목 이름(TS: `${owner}/${repo}`.slice(0, 200) — UTF-16 단위로 자른다)
fn label_of(raw: &Json) -> String {
    let Json::Obj(_) = raw else { return "?".to_string() };
    let s = format!("{}/{}", js_string(raw.get("owner")), js_string(raw.get("repo")));
    let units: Vec<u16> = s.encode_utf16().take(200).collect();
    String::from_utf16_lossy(&units)
}

/// 이미 등록된 레코드(같은 소스·경로)가 있으면 그것
fn registered(store: &ConfigStore, path: &str) -> Option<Json> {
    let Json::Obj(cfg) = store.get() else { return None };
    let repos = crate::config::model::get(&cfg, "repos");
    let Some(Json::Arr(repos)) = repos else { return None };
    repos.iter().find(|r| str_of(js_get(r, "sourceId")) == Some(GITHUB_SOURCE_ID) && str_of(js_get(r, "path")) == Some(path)).cloned()
}

/// 저장소 하나를 추가한다(경로 잠금 안에서만 부른다). 실패하면 만든 폴더를 지우고 던진다
async fn add_one(store: &Arc<ConfigStore>, exec: &GitExecutor, cache_dir: &str, link: &GithubLink, path: &str, group: &str) -> Result<Json, String> {
    let gp = GithubCachePath::new(cache_dir, path).ok_or_else(|| "잘못된 저장소입니다".to_string())?;
    let mut created = false;
    let result: Result<Json, String> = async {
        let repo = match registered(store, path) {
            Some(r) => {
                // 이미 등록된 저장소를 다시 추가하면서 그룹을 골랐으면 그 그룹으로 옮긴다
                if !group.is_empty() && str_of(js_get(&r, "group")) != Some(group) {
                    let id = str_of(js_get(&r, "id")).unwrap_or_default().to_string();
                    let patch = Json::Obj(vec![("group".to_string(), Json::Str(group.to_string()))]);
                    let s = store.clone();
                    blocking(move || s.update_repo(&id, &patch)).await??;
                }
                r
            }
            None => {
                // 등록되지 않은 같은 이름의 폴더는 이전 실패·삭제 실패가 남긴 찌꺼기다 — 우리 캐시 모양일 때만(`GithubCachePath`) 지우고 새로 만든다
                let stray = gp.clone();
                blocking(move || remove_cache_dir(&stray)).await??;
                let dir = Path::new(path);
                if std::fs::symlink_metadata(dir).is_ok() || dir.parent().is_some_and(is_link) {
                    return Err("캐시 폴더를 정리하지 못했습니다".to_string());
                }
                std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
                created = true;
                init_github_repo(exec, &gp, link, None).await.map_err(|e| e.to_string())?;
                // 다른 org 의 같은 이름 저장소(acme/web · globex/web)가 이미 있으면 사이드바에서 구분되게 owner 를 붙인다
                let taken = taken_name(store, &link.repo);
                let name = if taken { format!("{}/{}", link.owner, link.repo) } else { link.repo.clone() };
                let mut rec = vec![
                    ("sourceId".to_string(), Json::Str(GITHUB_SOURCE_ID.to_string())),
                    ("path".to_string(), Json::Str(path.to_string())),
                    ("name".to_string(), Json::Str(name)),
                ];
                if !group.is_empty() {
                    rec.push(("group".to_string(), Json::Str(group.to_string())));
                }
                rec.push(("github".to_string(), link.to_json()));
                let source = source_for("github", None)?;
                let (s, rec) = (store.clone(), Json::Obj(rec));
                let added = blocking(move || s.add_repo(&rec, Some(&source))).await??;
                created = false;
                added
            }
        };
        let id = str_of(js_get(&repo, "id")).unwrap_or_default();
        Ok(store.repo(id).unwrap_or(repo))
    }
    .await;
    if result.is_err() && created {
        let gp = gp.clone();
        let _ = blocking(move || remove_cache_dir(&gp)).await;
    }
    result
}

fn taken_name(store: &ConfigStore, repo: &str) -> bool {
    let Json::Obj(cfg) = store.get() else { return false };
    let Some(Json::Arr(repos)) = crate::config::model::get(&cfg, "repos") else { return false };
    repos.iter().any(|r| str_of(js_get(r, "sourceId")) == Some(GITHUB_SOURCE_ID) && str_of(js_get(r, "name")).is_some_and(|n| n.to_lowercase() == repo.to_lowercase()))
}

pub(crate) async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Result<T, String> {
    tokio::task::spawn_blocking(f).await.map_err(|_| "처리하지 못했습니다".to_string())
}

/// `repos:addGithub`(TS `addGithubRepos` + 핸들러 검사). 하나가 실패해도 나머지는 추가하고 실패한 폴더는 지운다. 이미 등록된 저장소는 기존 항목을 돌려준다.
/// 결과 `{added:[Repo], failed:[{fullName, error}]}`
pub async fn add_github_repos(store: &Arc<ConfigStore>, exec: Option<GithubExecutor>, cache_dir: &str, locks: &PathLocks, input: Option<&Json>) -> Result<Value, String> {
    // 핸들러: 모양과 개수만(저장소별 검증은 아래에서)
    let Some(Json::Obj(_)) = input else { return Err(BAD_INPUT.to_string()) };
    let input = input.unwrap_or(&Json::Null);
    let Some(Json::Arr(repos)) = input.get("repos") else { return Err(BAD_INPUT.to_string()) };
    if repos.is_empty() || repos.len() > MAX_ADD_GITHUB {
        return Err(BAD_INPUT.to_string());
    }
    let group = match input.get("group") {
        None => String::new(),
        Some(Json::Str(g)) if g.encode_utf16().count() <= MAX_GROUP_LEN => js_trim(g).to_string(),
        Some(_) => return Err(BAD_GROUP.to_string()),
    };
    let exec = GitExecutor::Github(exec.ok_or_else(|| NOT_AVAILABLE.to_string())?);
    let (mut added, mut failed): (Vec<Json>, Vec<Value>) = (Vec::new(), Vec::new());
    for raw in repos {
        let one = async {
            let link = validate_github_link(Some(raw))?;
            let path = github_cache_path(cache_dir, &link.owner, &link.repo);
            // 같은 경로를 동시에 추가하면 서로의 폴더를 지우고 만드는 경쟁이 생긴다 — 경로 단위로 차례로 처리한다(뒤 호출은 앞 호출이 등록한 항목을 돌려받는다)
            let _guard = locks.lock(&path).await;
            add_one(store, &exec, cache_dir, &link, &path, &group).await
        };
        match one.await {
            Ok(latest) => {
                let id = str_of(js_get(&latest, "id")).map(str::to_string);
                if !added.iter().any(|r| str_of(js_get(r, "id")).map(str::to_string) == id) {
                    added.push(latest);
                }
            }
            Err(error) => failed.push(json!({ "fullName": label_of(raw), "error": error })),
        }
    }
    let added: Vec<Value> = added.iter().map(crate::config_host::to_value).collect();
    Ok(json!({ "added": added, "failed": failed }))
}

/// 레코드가 가리키는 캐시 폴더들(저장된 경로와 링크로 다시 계산한 경로, 소문자·구분자 통일) — 등록 여부 판정용
fn record_paths(store: &ConfigStore, cache_dir: &str) -> HashSet<String> {
    let norm = |p: &str| canonical_key(cache_dir, p);
    let mut out = HashSet::new();
    let Json::Obj(cfg) = store.get() else { return out };
    let Some(Json::Arr(repos)) = crate::config::model::get(&cfg, "repos") else { return out };
    for r in repos {
        if str_of(js_get(r, "sourceId")) != Some(GITHUB_SOURCE_ID) {
            continue;
        }
        if let Some(p) = str_of(js_get(r, "path")) {
            out.insert(norm(p));
        }
        if let Some(l) = sanitize_github_link(js_get(r, "github")) {
            out.insert(norm(&github_cache_path(cache_dir, &l.owner, &l.repo)));
        }
    }
    out
}

/// 시작할 때 한 번: 캐시 기준 폴더 아래에서 어떤 저장소 레코드도 가리키지 않는 `<owner>/<repo>.git` 폴더를 지운다(삭제에 실패했거나 로드 때 버려진 레코드, 추가하다 중단된 폴더).
/// 정확히 그 모양이고 링크가 아닌 폴더만 지우며 그 밖의 파일·폴더는 건드리지 않는다. 비게 된 owner 폴더도 정리한다. 지운 폴더 수를 돌려준다.
/// **설정을 읽지 못한 채 시작했으면(저장소 목록이 빈 것이 사실이 아니다) 아무것도 지우지 않는다**
pub async fn cleanup_github_cache(store: &Arc<ConfigStore>, cache_dir: &str, locks: &PathLocks) -> usize {
    if store.load_failed() {
        return 0;
    }
    let base = cache_dir.to_string();
    let owners: Vec<std::path::PathBuf> = blocking(move || {
        // 링크·파일은 따라가지도 지우지도 않는다(`file_type` 은 링크를 따라가지 않는다)
        std::fs::read_dir(&base).map(|rd| rd.filter_map(Result::ok).filter(|e| e.file_type().is_ok_and(|t| t.is_dir())).map(|e| e.path()).collect()).unwrap_or_default()
    })
    .await
    .unwrap_or_default();
    let mut removed = 0;
    for owner_dir in owners {
        let od = owner_dir.clone();
        let entries: Vec<std::path::PathBuf> = blocking(move || std::fs::read_dir(&od).map(|rd| rd.filter_map(Result::ok).filter(|e| e.file_type().is_ok_and(|t| t.is_dir())).map(|e| e.path()).collect()).unwrap_or_default())
            .await
            .unwrap_or_default();
        for entry in entries {
            let path = entry.to_string_lossy().into_owned();
            // 모양이 틀린 폴더는 건드리지 않는다
            let Some(gp) = GithubCachePath::new(cache_dir, &path) else { continue };
            // 추가 중인 폴더를 지우지 않도록 같은 경로 잠금 안에서, 등록 여부를 다시 확인한 뒤 지운다. 잠금 키·비교는 디스크 항목 경로가 아니라 정규형(`..`·중복 구분자를 정리한 `github_cache_path`)이다 —
            // 추가·삭제가 쓰는 키와 같아야 서로 겹치는 작업을 직렬화한다
            let key = canonical_key(cache_dir, &path);
            let _guard = locks.lock(&key).await;
            if record_paths(store, cache_dir).contains(&key) {
                continue;
            }
            // 아직 쥐고 있다 — 다음 시작 때 다시 시도한다
            if blocking(move || remove_cache_dir(&gp)).await.is_ok_and(|r| r == Ok(true)) {
                removed += 1;
            }
        }
        let od = owner_dir;
        let _ = blocking(move || std::fs::remove_dir(od)).await; // 비어 있을 때만 지워진다
    }
    removed
}

/// 레코드는 있는데 캐시 폴더가 없는 GitHub 저장소(Electron 설정 가져오기 뒤 — 폴더는 가져오지 않는다)의 빈 캐시를 다시 만든다(네트워크 없음 — 내용은 처음 열 때 fetch 한다). 만든 수
pub async fn restore_missing_github_caches(store: &Arc<ConfigStore>, exec: Option<GithubExecutor>, cache_dir: &str, locks: &PathLocks) -> usize {
    let Some(exec) = exec else { return 0 };
    let exec = GitExecutor::Github(exec);
    let Json::Obj(cfg) = store.get() else { return 0 };
    let Some(Json::Arr(repos)) = crate::config::model::get(&cfg, "repos").cloned() else { return 0 };
    let mut made = 0;
    for r in repos {
        // 소스가 github 이고 링크가 유효하며 저장된 경로가 그 링크의 캐시 경로인 레코드만(`github_target_of` — 로드가 경로를 이미 맞춘다)
        let Some((gp, link)) = github_target_of(store, cache_dir, &r) else { continue };
        let path = gp.as_str().to_string();
        let _guard = locks.lock(&canonical_key(cache_dir, &path)).await;
        // 잠금을 기다리는 사이 삭제·재추가가 있었을 수 있다 — 잠금 안에서 등록 여부(같은 id·링크·경로)를 다시 확인한다
        let id = str_of(js_get(&r, "id")).unwrap_or_default();
        if !store.repo(id).and_then(|cur| github_target_of(store, cache_dir, &cur)).is_some_and(|(p, _)| p == gp) {
            continue;
        }
        let dir = Path::new(&path);
        if std::fs::symlink_metadata(dir).is_ok() || dir.parent().is_some_and(is_link) {
            continue;
        }
        if std::fs::create_dir_all(dir).is_err() {
            continue;
        }
        if init_github_repo(&exec, &gp, &link, None).await.is_ok() {
            made += 1;
        } else {
            let gp = gp.clone();
            let _ = blocking(move || remove_cache_dir(&gp)).await;
        }
    }
    made
}
