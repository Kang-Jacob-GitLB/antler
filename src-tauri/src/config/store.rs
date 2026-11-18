//! `ConfigStore`(`src/main/services/ConfigStore.ts` 의 이식): 소스·저장소 목록과 UI 설정을 JSON 파일 하나에 저장한다.
//!
//! - **디스크에 먼저 쓰고 성공했을 때만 메모리를 바꾼다.** 알림이 실패해도(패닉) 이미 저장된 변경은 성공이다.
//! - 모든 변경은 `Mutex` 안에서 읽기 → 수정 → 저장을 한다(TS 단일 스레드와 같은 직렬화). 알림은 **잠금을 놓은 뒤** 세대 번호를 붙여 보낸다 —
//!   받는 쪽(미러)이 오래된 세대를 버리므로 순서가 뒤집혀도 낡은 값이 이기지 않는다.
//! - 설정은 [`Json`] 트리다(`model.rs`). 반환값·알림에는 `undefined` 자리를 뺀 값이 간다.
//!
//! Phase 6b PR 9 에서 앱에 연결했다 — Rust 가 설정 파일의 유일한 작성자이고(`ANTLER_RUST_GIT=0` 이면 열지 않고 사이드카가 쓴다), 변경 알림이 `config::sync` 로 미러·fetch 설정을 맞춘다.
use std::fs;
use std::io;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::git::fetch::js_trim;
use crate::git::github_link::is_safe_branch;
use crate::net::json::{Json, MAX_DEPTH};

use super::load::{parse_config, Corrupt};
use super::model::{self, defaults, get, js_get, remove, set, spread, strip_undefined, Obj, LOCAL_SOURCE_ID, UNDEFINED};
use super::save::{check_document_depth, to_file_text, with_suffix, within_depth, write_atomic};
use super::validate::{check_editor_exe_path, check_set_ui, is_version};

/// 설정이 디스크에 쓰인 직후(메모리 상태를 바꾼 뒤) 세대 번호와 새 설정으로 불린다
pub type ChangeListener = Box<dyn Fn(u64, &Json) + Send + Sync>;

/// TS 가 `TypeError` 를 던지는 입력(예: `setUi(null)`·`name: null`). V8 의 문구는 옮기지 않는다
pub const TYPE_ERROR: &str = "TypeError";

type IdFn = Box<dyn Fn() -> String + Send + Sync>;
type ClockFn = Box<dyn Fn() -> u128 + Send + Sync>;

struct State {
    config: Obj,
    /// 쓰기마다 1 씩 올라간다(로드 직후는 0)
    gen: u64,
}

pub struct ConfigStore {
    file: PathBuf,
    state: Mutex<State>,
    /// 설정 파일이 있는데 읽거나 해석하지 못했다(저장소 목록이 비어 시작했다). 목록이 빈 것을 근거로 캐시를 지우면 안 된다
    load_failed: bool,
    listener: Mutex<Option<Arc<ChangeListener>>>,
    new_id: IdFn,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

fn now_ms() -> u128 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis())
}

/// JS 엄격 동등(`===`)에서 값이 `undefined` 일 수 있는 두 쪽을 비교한다. 객체·배열은 참조 비교라 같은 값이어도 거짓이다
fn js_eq(a: Option<&Json>, b: Option<&Json>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(Json::Null), Some(Json::Null)) => true,
        (Some(Json::Bool(x)), Some(Json::Bool(y))) => x == y,
        (Some(Json::Num(x)), Some(Json::Num(y))) => x == y,
        (Some(Json::Str(x)), Some(Json::Str(y))) => x == y,
        _ => false,
    }
}

fn arr_of<'a>(config: &'a Obj, key: &str) -> &'a [Json] {
    match get(config, key) {
        Some(Json::Arr(a)) => a,
        _ => &[],
    }
}

fn str_eq(v: Option<&Json>, s: &str) -> bool {
    matches!(v, Some(Json::Str(x)) if x == s)
}

fn ui_of(config: &Obj) -> Obj {
    spread(get(config, "ui"))
}

impl ConfigStore {
    /// 파일을 읽어 연다. `cache_dir` 가 있으면 로드할 때 github 저장소의 path 를 이 폴더와 링크로 다시 정한다
    pub fn open(file: impl Into<PathBuf>, cache_dir: Option<String>) -> Self {
        Self::open_with(file.into(), cache_dir, Box::new(|| uuid::Uuid::new_v4().to_string()), Box::new(now_ms))
    }

    /// 시험용: 저장소 id 와 손상 파일 백업 이름의 시각을 정한다(골든이 무작위·시각에 기대지 않게)
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn open_for_test(file: impl Into<PathBuf>, cache_dir: Option<String>, new_id: impl Fn() -> String + Send + Sync + 'static, now_ms: impl Fn() -> u128 + Send + Sync + 'static) -> Self {
        Self::open_with(file.into(), cache_dir, Box::new(new_id), Box::new(now_ms))
    }

    fn open_with(file: PathBuf, cache_dir: Option<String>, new_id: IdFn, now_ms: ClockFn) -> Self {
        let (config, load_failed) = Self::load(&file, cache_dir.as_deref(), &*now_ms);
        ConfigStore { file, state: Mutex::new(State { config, gen: 0 }), load_failed, listener: Mutex::new(None), new_id }
    }

    fn load(file: &Path, cache_dir: Option<&str>, now_ms: &dyn Fn() -> u128) -> (Obj, bool) {
        let bytes = match fs::read(file) {
            Ok(b) => b,
            // 첫 실행(손상 아님). 그 밖의 읽기 실패는 손상이다
            Err(e) if e.kind() == io::ErrorKind::NotFound => return (defaults(), false),
            Err(_) => return (defaults(), true),
        };
        // readFileSync(file, 'utf8') 처럼 잘못된 UTF-8 은 U+FFFD 로 바뀐다(오류가 아니다)
        let text = String::from_utf8_lossy(&bytes);
        match parse_config(&text, cache_dir) {
            Ok(config) => (config, false),
            Err(Corrupt) => {
                // 손상된 설정은 덮어쓰기 전에 보존해 둔다(사용자가 저장소 목록을 복구할 수 있게). 보존 실패는 무시하고 기본값으로 시작한다
                let _ = fs::copy(file, with_suffix(file, &format!(".corrupt-{}.bak", now_ms())));
                (defaults(), true)
            }
        }
    }

    pub fn load_failed(&self) -> bool {
        self.load_failed
    }

    /// 변경 알림을 정한다(미러·fetch 설정으로 보낸다). 알림이 패닉해도 저장된 변경은 실패가 아니다
    pub fn set_on_change(&self, f: ChangeListener) {
        *lock(&self.listener) = Some(Arc::new(f));
    }

    /// 지금 설정(`config:get`). `undefined` 자리는 뺀 값이다
    pub fn get(&self) -> Json {
        strip_undefined(&Json::Obj(lock(&self.state).config.clone()))
    }

    pub fn source(&self, id: &str) -> Option<Json> {
        let st = lock(&self.state);
        arr_of(&st.config, "sources").iter().find(|s| str_eq(js_get(s, "id"), id)).cloned()
    }

    pub fn repo(&self, id: &str) -> Option<Json> {
        let st = lock(&self.state);
        arr_of(&st.config, "repos").iter().find(|r| str_eq(js_get(r, "id"), id)).map(strip_undefined)
    }

    pub fn trusted_config(&self, repo_id: &str) -> Option<String> {
        self.map_entry("trustedConfigs", repo_id)
    }

    pub fn fetch_approval(&self, repo_id: &str) -> Option<String> {
        self.map_entry("fetchApprovals", repo_id)
    }

    fn map_entry(&self, key: &str, repo_id: &str) -> Option<String> {
        let st = lock(&self.state);
        match get(&st.config, key).and_then(|m| js_get(m, repo_id)) {
            Some(Json::Str(s)) => Some(s.clone()),
            _ => None,
        }
    }

    /// 읽기 → 수정 → 저장을 한 잠금 안에서 한다. `f` 가 `None` 을 돌려주면 쓰지 않는다(같은 값이면 쓰지 않는 변경자들)
    fn update_with<T>(&self, f: impl FnOnce(&Obj) -> Result<(Option<Obj>, T), String>) -> Result<T, String> {
        let (out, change) = {
            let mut st = lock(&self.state);
            let (next, out) = f(&st.config)?;
            let Some(next) = next else { return Ok(out) };
            // 다음 실행의 로드가 손상으로 보는 깊이는 쓰지 않는다(변경 경로마다 검사하지 않아도 되는 마지막 방어선)
            check_document_depth(&Json::Obj(next.clone()))?;
            // 디스크에 먼저 쓰고, 성공했을 때만 메모리 상태를 바꾼다
            write_atomic(&self.file, &to_file_text(&Json::Obj(next.clone()))).map_err(|e| format!("설정 파일을 쓰지 못했습니다: {e}"))?;
            st.config = next;
            st.gen += 1;
            (out, (st.gen, strip_undefined(&Json::Obj(st.config.clone()))))
        };
        // 잠금을 놓은 뒤 알린다. 알림이 패닉해도 이미 저장된 설정 변경을 실패로 만들지 않는다
        let listener = lock(&self.listener).clone();
        if let Some(l) = listener {
            let _ = catch_unwind(AssertUnwindSafe(|| l(change.0, &change.1)));
        }
        Ok(out)
    }

    /// 최상위 키 하나를 바꾼 설정(`{...config, key: value}`)
    fn with_key(config: &Obj, key: &str, value: Json) -> Obj {
        let mut next = config.clone();
        set(&mut next, key, value);
        next
    }

    fn snapshot(next: &Obj) -> Json {
        strip_undefined(&Json::Obj(next.clone()))
    }

    /// `config:setUi`. 검증은 `validate::check_set_ui`, 병합은 `{...ui, ...patch}`
    pub fn set_ui(&self, patch: &Json) -> Result<Json, String> {
        // 패치가 객체가 아니면 TS 는 `patch.githubClientId`·`in` 에서 던진다(배열은 던지지 않고 인덱스 키로 펼쳐진다)
        let patch: Obj = match patch {
            Json::Obj(_) | Json::Arr(_) => spread(Some(patch)),
            _ => return Err(TYPE_ERROR.to_string()),
        };
        check_set_ui(&patch)?;
        // 패치 값은 파일에서 루트 → `ui` 아래(2겹)에 놓인다. 로드가 받는 깊이를 넘기면 다음 실행에 설정이 초기화되므로 저장 전에 거부한다
        if !patch.iter().all(|(_, v)| within_depth(v, MAX_DEPTH - 2)) {
            return Err("잘못된 UI 설정입니다(중첩이 너무 깊습니다)".to_string());
        }
        self.update_with(|config| {
            let mut ui = ui_of(config);
            for (k, v) in &patch {
                set(&mut ui, k, v.clone());
            }
            // 병합 뒤 하루 표시 시각을 다시 검사한다(한쪽만 바꿔도 저장된 다른 쪽과 어긋나지 않게)
            if model::has_key(&patch, "worklogDayStart") || model::has_key(&patch, "worklogDayEnd") {
                let num = |key: &str, default: f64| match get(&ui, key) {
                    Some(Json::Num(n)) => *n,
                    _ => default,
                };
                if num("worklogDayStart", 7.0) >= num("worklogDayEnd", 21.0) {
                    return Err("잘못된 Jira 설정입니다".to_string());
                }
            }
            // 보일 그룹이 빈 목록이면 제한 없음이다(패치로는 키를 지울 수 없어 빈 목록으로 알린다)
            if matches!(get(&ui, "visibleGroups"), Some(Json::Arr(a)) if a.is_empty()) || str_eq(get(&ui, "visibleGroups"), "") {
                remove(&mut ui, "visibleGroups");
            }
            let next = Self::with_key(config, "ui", Json::Obj(ui));
            let out = Self::snapshot(&next);
            Ok((Some(next), out))
        })
    }

    /// main 전용: 사용자가 파일 선택 창에서 고른 에디터 실행 파일(호출자가 `check_chosen_exe` 로 검증한다). IPC 로 직접 노출하지 않는다
    pub fn set_editor_exe_path(&self, path: &str) -> Result<Json, String> {
        if !check_editor_exe_path(path) {
            return Err("잘못된 실행 파일 경로입니다".to_string());
        }
        self.set_ui_key("editorExePath", path)
    }

    /// main 전용: 사용자가 '이 버전 건너뛰기'를 고른 업데이트 버전
    pub fn set_skipped_update_version(&self, version: &str) -> Result<Json, String> {
        if !is_version(version) {
            return Err("잘못된 버전입니다".to_string());
        }
        self.set_ui_key("skippedUpdateVersion", version)
    }

    fn set_ui_key(&self, key: &str, value: &str) -> Result<Json, String> {
        self.update_with(|config| {
            let mut ui = ui_of(config);
            set(&mut ui, key, Json::Str(value.to_string()));
            let next = Self::with_key(config, "ui", Json::Obj(ui));
            let out = Self::snapshot(&next);
            Ok((Some(next), out))
        })
    }

    /// 같은 소스·경로가 이미 있으면 기존 항목을 돌려준다(쓰지 않는다).
    /// `source` 를 함께 주면(처음 쓰는 WSL 배포판·SSH 호스트) 저장소와 같은 쓰기로 소스도 추가한다. `input` 은 `id` 가 없는 저장소 레코드다
    pub fn add_repo(&self, input: &Json, source: Option<&Json>) -> Result<Json, String> {
        let entries = spread(Some(input));
        self.update_with(|config| {
            let repos = arr_of(config, "repos");
            let existing = repos.iter().find(|r| js_eq(js_get(r, "sourceId"), get(&entries, "sourceId")) && js_eq(js_get(r, "path"), get(&entries, "path")));
            if let Some(existing) = existing {
                return Ok((None, strip_undefined(existing)));
            }
            let mut repo = entries.clone();
            set(&mut repo, "id", Json::Str((self.new_id)()));
            let repo = Json::Obj(repo);
            let mut sources = arr_of(config, "sources").to_vec();
            if let Some(s) = source {
                if !sources.iter().any(|x| js_eq(js_get(x, "id"), js_get(s, "id"))) {
                    sources.push(s.clone());
                }
            }
            let mut next = Self::with_key(config, "sources", Json::Arr(sources));
            set(&mut next, "repos", Json::Arr(repos.iter().cloned().chain([repo.clone()]).collect()));
            Ok((Some(next), strip_undefined(&repo)))
        })
    }

    /// 저장소 이름(비어 있지 않은 trim 값)과 그룹(trim 한 값, 비면 해제)을 바꾼다. 아는 id 가 없어도 쓴다(TS 와 같다)
    pub fn update_repo(&self, id: &str, patch: &Json) -> Result<Json, String> {
        // TS: `patch.name !== undefined && patch.name.trim()` — 문자열이 아닌 값에서 `.trim` 이 던진다. null 패치는 속성 읽기에서 던진다.
        // 던지는 곳은 `repos.map` 콜백 안이라 **id 가 맞는 저장소가 있을 때만** 던진다(모르는 id 는 던지지 않고 그대로 쓴다)
        let parsed = (|| {
            let (name, group) = match patch {
                Json::Null => return Err(TYPE_ERROR.to_string()),
                Json::Obj(o) => (get(o, "name"), get(o, "group")),
                _ => (None, None),
            };
            let new_name = match name {
                None => None,
                Some(Json::Str(s)) => Some(js_trim(s).to_string()).filter(|t| !t.is_empty()),
                Some(_) => return Err(TYPE_ERROR.to_string()),
            };
            // `patch.group?.trim()` — null 은 undefined 가 되어 그룹을 해제한다
            let new_group = match group {
                None => None,
                Some(Json::Null) => Some(None),
                Some(Json::Str(s)) => Some(Some(js_trim(s).to_string()).filter(|t| !t.is_empty())),
                Some(_) => return Err(TYPE_ERROR.to_string()),
            };
            Ok((new_name, new_group))
        })();
        self.update_with(|config| {
            let matches_id = arr_of(config, "repos").iter().any(|r| str_eq(js_get(r, "id"), id));
            let (new_name, new_group) = match (matches_id, parsed) {
                (true, Err(e)) => return Err(e),
                (_, Ok(v)) => v,
                (false, Err(_)) => (None, None),
            };
            let repos: Vec<Json> = arr_of(config, "repos")
                .iter()
                .map(|r| {
                    if !str_eq(js_get(r, "id"), id) {
                        return r.clone();
                    }
                    let mut next = spread(Some(r));
                    if let Some(n) = &new_name {
                        set(&mut next, "name", Json::Str(n.clone()));
                    }
                    match &new_group {
                        Some(Some(g)) => set(&mut next, "group", Json::Str(g.clone())),
                        Some(None) => remove(&mut next, "group"),
                        None => {}
                    }
                    Json::Obj(next)
                })
                .collect();
            let next = Self::with_key(config, "repos", Json::Arr(repos));
            let out = Self::snapshot(&next);
            Ok((Some(next), out))
        })
    }

    /// main 전용: 첫 fetch 뒤 알아낸 GitHub 저장소의 기본 브랜치를 레코드에 남긴다. 링크가 없거나 같은 값이거나 쓸 수 없는 이름이면 아무것도 하지 않는다
    pub fn set_github_default_branch(&self, id: &str, branch: &str) -> Result<(), String> {
        self.update_with(|config| {
            let cur = arr_of(config, "repos").iter().find(|r| str_eq(js_get(r, "id"), id));
            let Some(github) = cur.and_then(|r| js_get(r, "github")).filter(|g| crate::net::js_value::truthy(Some(g))) else { return Ok((None, ())) };
            if str_eq(js_get(github, "defaultBranch"), branch) || !is_safe_branch(branch) {
                return Ok((None, ()));
            }
            let mut link = spread(Some(github));
            set(&mut link, "defaultBranch", Json::Str(branch.to_string()));
            let repos: Vec<Json> = arr_of(config, "repos")
                .iter()
                .map(|r| {
                    if !str_eq(js_get(r, "id"), id) {
                        return r.clone();
                    }
                    let mut next = spread(Some(r));
                    set(&mut next, "github", Json::Obj(link.clone()));
                    Json::Obj(next)
                })
                .collect();
            Ok((Some(Self::with_key(config, "repos", Json::Arr(repos))), ()))
        })
    }

    /// 저장소를 지우고, 더 이상 쓰는 저장소가 없는 WSL·SSH·GitHub 소스도 함께 정리한다(로컬 소스는 항상 남긴다).
    /// 그 저장소의 fetch 승인·허용 지문도 지운다(모르는 id 여도 두 맵을 `{}` 로 남긴다 — TS 와 같다)
    pub fn remove_repo(&self, id: &str) -> Result<Json, String> {
        self.update_with(|config| {
            let mut ui = ui_of(config);
            if str_eq(get(&ui, "lastRepoId"), id) {
                // `{...ui, lastRepoId: undefined}` — 키는 남고 값만 없다(다음 저장 때 파일에서 사라진다)
                set(&mut ui, "lastRepoId", UNDEFINED);
            }
            let repos: Vec<Json> = arr_of(config, "repos").iter().filter(|r| !str_eq(js_get(r, "id"), id)).cloned().collect();
            let used: Vec<Option<&Json>> = repos.iter().map(|r| js_get(r, "sourceId")).collect();
            let sources: Vec<Json> = arr_of(config, "sources")
                .iter()
                .filter(|s| str_eq(js_get(s, "id"), LOCAL_SOURCE_ID) || used.iter().any(|u| js_eq(*u, js_get(s, "id"))))
                .cloned()
                .collect();
            let without = |key: &str| {
                let mut m = spread(get(config, key));
                remove(&mut m, id);
                Json::Obj(m)
            };
            let mut next = Self::with_key(config, "ui", Json::Obj(ui));
            set(&mut next, "repos", Json::Arr(repos));
            set(&mut next, "sources", Json::Arr(sources));
            set(&mut next, "fetchApprovals", without("fetchApprovals"));
            set(&mut next, "trustedConfigs", without("trustedConfigs"));
            let out = Self::snapshot(&next);
            Ok((Some(next), out))
        })
    }

    /// fetch 확인 창에서 항상 허용한 저장소 설정 지문을 기록한다. main 에서만 호출한다. 같은 값이면 쓰지 않는다
    pub fn set_trusted_config(&self, repo_id: &str, fingerprint: &str) -> Result<(), String> {
        self.update_with(|config| {
            let cur = get(config, "trustedConfigs").and_then(|m| js_get(m, repo_id));
            if str_eq(cur, fingerprint) {
                return Ok((None, ()));
            }
            let mut m = spread(get(config, "trustedConfigs"));
            set(&mut m, repo_id, Json::Str(fingerprint.to_string()));
            Ok((Some(Self::with_key(config, "trustedConfigs", Json::Obj(m))), ()))
        })
    }

    /// 수동 fetch 가 성공한 remote 지문을 기록한다(`None` 이면 지운다). main 에서만 호출한다. 같은 값이면 쓰지 않는다.
    /// 지문을 새로 쓰면 키가 맨 끝으로 옮겨진다(지우고 다시 넣는다 — TS 와 같다). 빈 문자열은 지우는 것과 같다
    pub fn set_fetch_approval(&self, repo_id: &str, fingerprint: Option<&str>) -> Result<(), String> {
        self.update_with(|config| {
            let cur = get(config, "fetchApprovals").and_then(|m| js_get(m, repo_id));
            let same = match fingerprint {
                Some(f) => str_eq(cur, f),
                None => cur.is_none(),
            };
            if same {
                return Ok((None, ()));
            }
            let mut m = spread(get(config, "fetchApprovals"));
            remove(&mut m, repo_id);
            if let Some(f) = fingerprint.filter(|f| !f.is_empty()) {
                m.push((repo_id.to_string(), Json::Str(f.to_string())));
            }
            Ok((Some(Self::with_key(config, "fetchApprovals", Json::Obj(m))), ()))
        })
    }
}
