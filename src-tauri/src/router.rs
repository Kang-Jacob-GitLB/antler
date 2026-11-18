//! 채널 라우터. renderer 의 `antler_invoke` 를 Rust 가 직접 처리할지 Node 사이드카로 넘길지 정한다.
//!
//! Rust 가 처리하는 것: [`RUST_CHANNELS`](`git:fetch` 포함 — fetch 가드·상태 기계는 `git::fetch`)에 있는 채널이고, 인자의 저장소 id 가 미러([`RepoMirror`])에 있고 kind 가 local·wsl(또는 ssh — 아래)이고,
//! 환경변수 `ANTLER_RUST_GIT` 이 `0` 이 아닐 때. 그 밖에는 전부 사이드카가 지금까지처럼 처리한다
//! (미러에 없는 id 의 오류 문구도 사이드카가 만든다). `ANTLER_RUST_GIT=0` 은 즉시 되돌리는 스위치다.
//!
//! fetch 의 소유권: 스위치가 켜져 있으면 local·wsl(그리고 ssh 가 Rust 로 풀릴 때는 ssh) 저장소의 fetch(수동·자동·포커스)는 Rust 의 서비스가 맡고, 앱은 사이드카를 env
//! `ANTLER_RUST_FETCH_KINDS=local,wsl[,ssh]` 로 띄워 사이드카의 FetchService 가 그 저장소를 fetch·예약하지 않게 한다([`sidecar_fetch_kinds`]).
//! 스위치가 꺼져 있으면 env 도 서비스도 없다 — 사이드카가 전부 맡는다(이중 fetch 없음).
//!
//! 감시의 소유권(Phase 6a PR 7b): 활성 저장소 감시(`repos:watch` 가 시작·교체·중지, 창 포커스는 [`Router::watch_focus`])도 **같은 술어 하나**로 정한다 — Rust 는 fetch 를 맡는 종류의 저장소만 감시하고
//! (notify·폴링 — `git::watcher`), 사이드카는 같은 env 로 그 저장소를 감시하지 않는다(`src/sidecar/index.ts` 가 `FetchService` 와 같은 `fetchOwner` 를 `RepoWatcher` 에 준다). github 저장소는 사이드카가 감시한다.
//! 스위치가 꺼져 있으면(`ANTLER_RUST_GIT=0`) env 도 감시자도 없다 — 사이드카가 전부 감시한다.
//!
//! ssh(Phase 5 PR 3): `ANTLER_RUST_SSH` 가 `0` 이 아니고(`ANTLER_RUST_GIT=0` 이면 함께 꺼진다) SSH 풀이 주입돼 있으면([`Router::ssh_active`]) ssh 저장소도 git 채널을 Rust 가 처리한다(Phase 5 PR 4: `git:fetch` 포함).
//! 호스트마다 실행기(= 연결 하나)를 [`SshPool`] 에서 받고 큐는 호스트마다 하나(동시 4)다. fetch 서비스도 같은 풀의 연결을 쓴다.
//! fetch 소유는 해석 결과가 아니라 소스 종류([`rust_fetch_kinds`])로 정한다(`RepoMirror::resolve_fetch`) — 사이드카 env 와 같은 술어 하나라 이중 fetch 도 미소유도 없다.
//!
//! `jira:*`·`gh:*`(`gh:repo` 제외, [`API_CHANNELS`])는 별개의 스위치다: `ANTLER_RUST_API` 가 `0` 이 아니고(`ANTLER_RUST_GIT=0` 이어도 꺼진다) 서비스가 주입돼 있으면 **미러와 무관하게**
//! Rust 가 처리한다([`api_host`]). 꺼져 있으면 앱은 서비스도 만들지 않고 키체인도 읽지 않으며 사이드카가 지금처럼 처리한다. 켜져 있으면 사이드카를
//! env `ANTLER_RUST_API=1` 로 띄워 jira·gh 를 소유하지 않음을 알린다(앱은 두 서비스가 모두 만들어졌을 때만 주입한다)(라우터가 가로채므로 닿지 않는다 — 방어).
//!
//! 앱 채널(Phase 6b PR 5, [`APP_CHANNELS`]: 폴더 선택·클립보드·링크 열기·타이틀바 테마)은 설정·미러와 무관하다: `ANTLER_RUST_GIT` 이 `0` 이 아니고 포트([`AppHost`])가 주입돼 있으면 Rust 가 처리한다([`app_host`]).
//!
//! `gh:repo`(Phase 6a PR 8)는 `gh:` 접두어이지만 저장소의 remote 를 git 으로 읽는 일이라 위 git 채널과 같은 소유 술어(`RepoMirror::resolve`)를 쓴다 — Rust 가 맡는 저장소는 Rust 가 답하고
//! (결과 캐시 60초), github kind·미러에 없는 id 는 사이드카가 답한다. GitHub 서비스가 사이드카에 묻던 저장소 정보(`repoq`: remote 목록·push 된 커밋)도 같은 술어로 갈린다(`routed_repos`).
//!
//! 보안 경계(docs/security.md · docs/tauri.md "실행기"):
//! - 저장소 경로·kind·배포판은 **미러에서만** 가져온다. renderer 가 보낸 값은 저장소 id 와 검증된 값(해시·ref 범위·파일 경로·숫자)뿐이다.
//! - git 인자는 `git::commit`·`git::scope`(log)·`git::file_channels`(compare·fileDiff·fileHistory·blame)·`git::code_browse`(resolveRev·tree·blob·onRemote)·`git::refs_service`(refs·refsDetails — 인자가 저장소 id 뿐)·`git::search`·`git::activity`(search·activity — 질의는 `--grep=`·`--author=` 접두가 붙은 한 인자)·
//!   `git::lfs`(repoExtras·lfsPaths)·`git::wip`(wipStatus·wipDiff — 작업 트리를 읽으므로 저장소의 filter 드라이버를 무력화하는 `-c` 가드가 전역 옵션 자리에 붙는다) 가 서브커맨드·옵션 상수 + 검증된 값으로만 만든다.
//!   파일 경로는 항상 `--` 뒤(blame 의 크기 조회·코드 탐색기의 `cat-file`·`ls-tree` 만 `<해시>:<경로>` 한 인자)다. 이 파일은 인자를 만들지 않는다.
//! - untracked `wipDiff` 는 저장소별 마지막 `wipStatus` 가 보고한 경로와 **정확히 일치**할 때만 연다([`Router`] 의 허용 목록 — 저장소가 미러에서 빠지면 지운다).
//! - 취소: `requestId → CancellationToken` 레지스트리. cancel 이 등록보다 먼저 도착해도(별개 호출이라 순서가 보장되지 않는다)
//!   tombstone 이 기억했다가 등록 때 이미 취소된 토큰을 준다.
//!
//! 이 모듈은 `Sidecar`·`AppHandle` 에 기대지 않는다 — 통합 테스트(`tests/channels.rs`)가 그대로 돌릴 수 있다.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use crate::api_host::{self, GithubHost, JiraHost};
use crate::app_host::{self, AppHost};
use crate::git::commit::{commit_detail, estimate_commit_bytes, HexHash};
use crate::git::activity::{commit_activity, now_secs, ActivityCache, ActivityRequest};
use crate::git::code_browse::{
    blob_cost, is_on_remote, list_tree, parse_ref, parse_rev, read_blob, resolve_rev, tree_cost, BlobRequest, CommitId, TreeRequest, CODE_TREE_MAX_ENTRIES,
};
use crate::git::exec::queue::{ProcessQueue, LOCAL_LIMIT, SSH_LIMIT, WSL_LIMIT};
use crate::git::exec::ssh::SshPool;
use crate::git::exec::{DistroGate, ExecError, GitExecutor, LocalExecutor, RunOpts, WslExecutor};
use crate::git::fetch::service::{FetchControl, FetchPrefs};
use crate::git::fs_watch::NotifyDirs;
use crate::git::file_channels::{
    compare, estimate_compare_bytes, estimate_diff_bytes, file_blame, file_diff, file_history, parse_compare, BlameRequest, DiffRequest, HistoryRequest,
};
use crate::git::lfs::{lfs_paths, repo_extras, LfsPathsRequest};
use crate::git::log_service::{parse_query, LogService, NoteSink};
use crate::git::lru::LruCache;
use crate::git::refs_service::{refs_details, refs_snapshot};
use crate::git::remotes::{check_pushed_request, pushed_shas, remotes};
use crate::git::repo_state::{RepoState, SigError};
use crate::github::ports::GhRepoRef;
use crate::github::remote_url::{pick_github_remote, pick_github_remotes, Remote};
use crate::git::search::{search, SearchRequest};
use crate::git::parsers::wip_status::WIP_UNTRACKED_LIMIT;
use crate::git::signature::{refs_signature_digest, Signature};
use crate::git::wip::{wip_file_diff, wip_status, WipDiffRequest};
use crate::git::watcher::{DirWatch, PollTimes, RepoWatcher};
use crate::mirror::{RepoMirror, Resolved, Target};
use crate::watch_host::RouterWatchHost;

/// 감시가 ref 변화를 알리는 곳(저장소 id). 앱은 renderer 의 `repo:changed` 이벤트로 보낸다. **감시 태스크(tokio 워커)에서 불리니** 막지 않는다
pub type ChangeSink = Arc<dyn Fn(&str) + Send + Sync>;

/// Rust 가 처리하는 채널. 전부 `src/shared/ipc.ts` 의 `IPC_CHANNELS` 에 있어야 한다
/// (`tests/channels.rs` 가 `tests/golden/exec/channels.json` 으로 강제한다)
pub const RUST_CHANNELS: &[&str] = &[
    "git:commit",
    "git:log",
    "git:compare",
    "git:fileDiff",
    "git:fileHistory",
    "git:blame",
    "git:fetch",
    "git:resolveRev",
    "git:tree",
    "git:blob",
    "git:onRemote",
    "git:refs",
    "git:refsDetails",
    "git:search",
    "git:activity",
    "git:repoExtras",
    "git:lfsPaths",
    // `gh:` 접두어이지만 저장소의 remote 를 git 으로 읽는 일이라 다른 git 채널과 같은 소유 술어를 쓴다(Phase 6a PR 8). github kind·미러에 없는 id 는 사이드카가 답한다
    "gh:repo",
    "git:wipStatus",
    "git:wipDiff",
];

/// Rust 가 처리하는 앱 채널(`app_host`, Phase 6b PR 5): 설정이 필요 없는 폴더 선택·클립보드·링크 열기·타이틀바 테마. `RUST_CHANNELS`·`API_CHANNELS` 와 겹치지 않고
/// 전부 `src/shared/ipc.ts` 의 `IPC_CHANNELS` 에 있어야 한다(`tests/app_route.rs` 가 강제한다). 스위치는 `ANTLER_RUST_GIT` 하나(`0` 이면 사이드카가 처리한다)
pub const APP_CHANNELS: &[&str] = &["dialog:pickFolder", "clipboard:writeText", "code:openLink", "win:setTitleBarTheme"];

/// Rust 가 처리하는 `jira:*`·`gh:*` 채널(`api_host`). `RUST_CHANNELS` 와 별개다 — 미러·저장소와 무관하고 `ANTLER_RUST_API` 스위치만 본다.
/// 전부 `src/shared/ipc.ts` 의 `IPC_CHANNELS` 에 있어야 하고, `IPC_CHANNELS` 의 `jira:`·`gh:` 채널에서 [`API_SIDECAR_CHANNELS`] 를 뺀 것과 정확히 같아야 한다(`tests/api_route.rs` 가 강제한다)
pub const API_CHANNELS: &[&str] = &[
    "jira:auth",
    "jira:connect",
    "jira:disconnect",
    "jira:issue",
    "jira:transitions",
    "jira:transition",
    "jira:comments",
    "jira:search",
    "jira:dev",
    "jira:worklogs",
    "jira:userSearch",
    "jira:avatar",
    "jira:openExternal",
    "gh:auth",
    "gh:loginPat",
    "gh:importCliToken",
    "gh:deviceStart",
    "gh:devicePoll",
    "gh:logout",
    "gh:pulls",
    "gh:pullsForKey",
    "gh:pull",
    "gh:pullFiles",
    "gh:pullCommits",
    "gh:issues",
    "gh:issue",
    "gh:comments",
    "gh:filters",
    "gh:avatars",
    "gh:commitChecks",
    "gh:commitCheckRuns",
    "gh:runs",
    "gh:runJobs",
    "gh:openExternal",
    "gh:launchpad",
    "gh:accessibleRepos",
];

/// `gh:` 접두어이지만 [`API_CHANNELS`] 가 아닌 채널(서비스가 아니라 저장소를 보는 `gh:repo`). Rust 는 이 채널을 [`RUST_CHANNELS`] 의 git 채널로서 **Rust 가 맡는 저장소(local·WSL·SSH)만** 처리하고
/// (`Router::gh_repo`), github kind·미러에 없는 id·스위치가 꺼졌을 때는 사이드카의 `RepoService` 가 처리한다 — 그래서 사이드카는 이 채널을 계속 받는다.
/// `src/sidecar/apiOwns.ts` 의 `SIDECAR_KEPT_CHANNELS` 와 같아야 한다
pub const API_SIDECAR_CHANNELS: &[&str] = &["gh:repo"];

/// Rust 가 fetch 를 맡는 소스 종류(사이드카 env `ANTLER_RUST_FETCH_KINDS` 의 값. `src/sidecar/fetchOwns.ts` 가 읽는다).
/// **계산 값이다**: local·wsl 은 항상, ssh 는 Rust 가 ssh 저장소를 푸는 때(`Router::ssh_active`)만 더한다. 미러의 fetch 소유 판정(`RepoMirror::resolve_fetch`)과
/// 사이드카 env 가 모두 이 함수 하나를 부르므로, 둘이 어긋나 아무도 fetch 하지 않거나(미소유) 둘이 fetch 하는(이중) 일이 없다
pub fn rust_fetch_kinds(ssh_active: bool) -> &'static str {
    if ssh_active {
        "local,wsl,ssh"
    } else {
        "local,wsl"
    }
}

/// 사이드카를 띄울 때 넣을 `ANTLER_RUST_FETCH_KINDS` 값. 라우터가 꺼져 있으면(`ANTLER_RUST_GIT=0`) 없다 — 사이드카가 fetch 를 전부 맡는다.
/// `ssh_active` 는 [`Router::ssh_active`] 와 같은 값이어야 한다(`ANTLER_RUST_SSH` 도 같은 값으로 넘어간다)
pub fn sidecar_fetch_kinds(router_enabled: bool, ssh_active: bool) -> Option<&'static str> {
    router_enabled.then(|| rust_fetch_kinds(ssh_active))
}

/// `ANTLER_RUST_SSH`(`0` 이면 끔)와 `ANTLER_RUST_GIT`(`0` 이면 함께 끔)으로 ssh 저장소의 git 채널(fetch 제외)을 Rust 가 처리할지 정한다.
/// 기본은 켬이다. 실제 판정은 [`Router::ssh_active`] 하나(이 스위치 + 풀 주입)이고 미러 해석·사이드카 env 가 같은 값을 쓴다
pub fn rust_ssh_enabled(ssh_var: Option<&str>, git_var: Option<&str>) -> bool {
    ssh_var != Some("0") && rust_git_enabled(git_var)
}

/// `ANTLER_RUST_API`(`0` 이면 끔)와 `ANTLER_RUST_GIT`(`0` 이면 함께 끔)으로 [`API_CHANNELS`] 를 Rust 가 처리할지 정한다.
/// 이 스위치는 미러·저장소와 무관하다 — 켜져 있으면(서비스가 만들어졌으면) 항상 Rust 가 처리한다
pub fn rust_api_enabled(api_var: Option<&str>, git_var: Option<&str>) -> bool {
    api_var != Some("0") && rust_git_enabled(git_var)
}

/// 주어진 tokio 런타임 안에서 `f` 를 부른다. 서비스의 예약·fetch 시작은 태스크를 만들어서 런타임 밖 스레드(사이드카 읽기 스레드·창 이벤트)에서는 패닉한다 —
/// 그 스레드들은 이 함수로 앱 런타임에 들어간 뒤 라우터를 부른다
pub fn in_runtime_of<T>(handle: &tokio::runtime::Handle, f: impl FnOnce() -> T) -> T {
    let _guard = handle.enter();
    f()
}

/// bridge 의 `encodeArgs` 가 인자 배열의 빈 자리(undefined)에 넣는 표식(`src/sidecar/protocol.ts` 의 `UNDEFINED_ARG`)
pub const UNDEFINED_ARG: &str = "__antlerUndefinedArg";

/// 커밋 상세 캐시(`RepoService.ts` 의 COMMIT_CACHE_ENTRIES·COMMIT_CACHE_BYTES)
const COMMIT_CACHE_ENTRIES: usize = 300;
const COMMIT_CACHE_BYTES: u64 = 24 * 1024 * 1024;
/// 파일 diff 캐시(`DIFF_CACHE_ENTRIES`·`DIFF_CACHE_BYTES`). 비교 결과 캐시는 커밋 상세와 같은 크기다
const DIFF_CACHE_ENTRIES: usize = 500;
const DIFF_CACHE_BYTES: u64 = 32 * 1024 * 1024;
/// 코드 탐색기 트리 캐시(항목 하나를 200바이트로 추정)·파일 내용 캐시(항목 수를 작게 잡아 큰 파일이 메모리를 채우지 못하게 한다) — `TREE_CACHE_*`·`BLOB_CACHE_*`
const TREE_CACHE_ENTRIES: usize = 200;
const TREE_CACHE_BYTES: u64 = 8 * 1024 * 1024;
const BLOB_CACHE_ENTRIES: usize = 64;
const BLOB_CACHE_BYTES: u64 = 32 * 1024 * 1024;
/// GitHub remote 추론 결과(`gh:repo`) 캐시 시간·모든 GitHub remote 목록(Launchpad) 캐시 시간(`GH_REPO_TTL_MS`·`GH_REMOTES_TTL_MS`)
const GH_REPO_TTL: Duration = Duration::from_secs(60);
const GH_REMOTES_TTL: Duration = Duration::from_secs(10 * 60);
/// `gh_remotes` 응답 상한(사이드카 경로 `repoq` 의 응답 검증과 같은 값)
const GH_REMOTES_MAX: usize = 100;
const GH_REMOTE_NAME_MAX: usize = 200;
/// `git:onRemote` 가 true 였던 (저장소, 커밋)을 기억하는 개수 상한(넘으면 비운다)
const ON_REMOTE_MAX: usize = 2000;
/// 합치기만 하는 큐(`Router::joins`)의 슬롯 수 — 실제 git 은 소스의 큐에서 돌므로 이 큐는 막지 않는다
const JOIN_SLOTS: usize = 1 << 20;

/// 등록보다 먼저 도착한 cancel 을 기억하는 시간·개수
const TOMBSTONE_TTL: Duration = Duration::from_secs(10);
const TOMBSTONE_MAX: usize = 256;

/// 검증을 마친 파일 단위 채널 요청
enum FileJob {
    Compare(HexHash, HexHash),
    Diff(DiffRequest),
    History(HistoryRequest),
    Blame(BlameRequest),
}
/// 검증을 마친 코드 탐색기 채널 요청
enum CodeJob {
    Resolve(String),
    Tree(TreeRequest),
    Blob(BlobRequest),
    OnRemote(CommitId),
}
/// 검증을 마친 검색·활동 채널 요청
enum HistoryJob {
    Search(SearchRequest),
    Activity(ActivityRequest),
}
/// 검증을 마친 `git:repoExtras`·`git:lfsPaths` 요청
enum ExtrasJob {
    Extras,
    LfsPaths(LfsPathsRequest),
}
/// 검증을 마친 `git:wipStatus`·`git:wipDiff` 요청
enum WipJob {
    Status,
    Diff(WipDiffRequest),
}

/// 저장소 하나의 untracked diff 허용 목록: 마지막 `wipStatus` 가 보고한 경로와, 그 상태를 읽은 대상(경로·종류). 대상이 바뀐 저장소의 낡은 목록은 쓰지 않는다
struct AllowEntry {
    target: Target,
    paths: Arc<HashSet<String>>,
}

fn to_value<T: serde::Serialize>(v: &T) -> Result<Value, ExecError> {
    serde_json::to_value(v).map_err(|e| ExecError::Other { message: e.to_string() })
}

/// 시그니처 계산 한 번: 소스의 큐 슬롯을 받아(취소 토큰이 슬롯 대기에도 걸린다) 그 안에서 git 을 돌린다(실행에도 같은 토큰이 걸린다).
/// 공유 계산은 spawn 된 태스크라 future 를 버려도 멈추지 않으므로 제한 시간의 취소가 닿는 길은 이 토큰뿐이다. 큐의 결과 타입이 JSON 이라 값은 셀로 꺼낸다
pub(crate) async fn queued_signature(queue: ProcessQueue<Arc<Value>>, exec: Arc<GitExecutor>, path: String, token: CancellationToken) -> Result<Signature, SigError> {
    let cell: Arc<Mutex<Option<Signature>>> = Arc::default();
    let (out, t) = (cell.clone(), token.clone());
    queue
        .run(None, Some(token), move || async move {
            let sig = refs_signature_digest(exec.as_ref(), &path, &t).await?;
            *out.lock().unwrap_or_else(PoisonError::into_inner) = Some(sig);
            Ok(Arc::new(Value::Null))
        })
        .await?;
    let sig = cell.lock().unwrap_or_else(PoisonError::into_inner).take();
    sig.ok_or_else(|| SigError::Failed("signature missing".to_string()))
}

pub enum Routed {
    /// Rust 가 처리했다 — IpcEnvelope(`{ok:true,data}` / `{ok:false,error,code}`)
    Done(Value),
    /// 사이드카로 보낸다(인자는 호출자가 가진 원본 그대로)
    Sidecar,
}

/// `ANTLER_RUST_GIT` 값으로 Rust 경로를 켤지 정한다. `0` 만 끈다
pub fn rust_git_enabled(var: Option<&str>) -> bool {
    var != Some("0")
}

fn ok_env(data: Value) -> Value {
    json!({ "ok": true, "data": data })
}

const API_UNAVAILABLE: &str = "연결 서비스를 사용할 수 없습니다";

/// `gh:repo` 의 응답 값(TS `GhRepoRef`)
fn ref_json(r: &GhRepoRef) -> Value {
    json!({ "owner": r.owner, "repo": r.repo, "remote": r.remote })
}

fn err_env(error: &str, code: &str) -> Value {
    json!({ "ok": false, "error": error, "code": code })
}

/// 알림 팬아웃: 상태에 먼저 반영하고 사이드카로 보낸다. 상태는 Rust 가 푸는 저장소만 만진다(TS `noteLog` 가 모르는 저장소를 무시하는 것과 같다).
/// 순서가 중요하다 — `fetchChanged` 는 `repo:changed` 보다 먼저 시그니처를 버려야 하고, 싱크는 호출 시점에 읽는다(나중에 달아도 된다)
fn fanout_sink(state: &RepoState, mirror: &RepoMirror, user: &Arc<Mutex<Option<NoteSink>>>) -> NoteSink {
    let (state, mirror, user) = (state.clone(), mirror.clone(), user.clone());
    Arc::new(move |op, repo_id| {
        if matches!(mirror.resolve(repo_id), Resolved::Rust(_)) {
            state.note(op, repo_id);
        }
        let sink = user.lock().unwrap_or_else(PoisonError::into_inner).clone();
        if let Some(s) = sink {
            s(op, repo_id);
        }
    })
}

/// 인자 배열의 표식을 None(undefined)으로 되돌린다. 사이드카 `decodeArgs` 와 같다 — 키가 정확히 그것 하나이고 값이 `true` 인 객체만
fn decode_args(raw: &[Value]) -> Vec<Option<&Value>> {
    raw.iter()
        .map(|a| match a.as_object() {
            Some(o) if o.len() == 1 && o.get(UNDEFINED_ARG) == Some(&Value::Bool(true)) => None,
            _ => Some(a),
        })
        .collect()
}

// ── 취소 레지스트리 ────────────────────────────────────────

#[derive(Default)]
struct RegInner {
    live: HashMap<String, (u64, CancellationToken)>,
    tombs: VecDeque<(String, Instant)>,
    serial: u64,
}

/// `requestId → CancellationToken`. 같은 id 를 다시 등록하면 새 것이 이긴다(TS `withSignal` 의 `requests.set` 과 같다)
#[derive(Default)]
pub struct CancelRegistry {
    inner: Mutex<RegInner>,
}

/// 등록한 요청이 끝나면(버려지면) 레지스트리에서 자기 항목을 지운다
pub struct Registration {
    registry: Arc<CancelRegistry>,
    id: String,
    serial: u64,
    pub token: CancellationToken,
}

impl Drop for Registration {
    fn drop(&mut self) {
        let mut inner = self.registry.inner.lock().unwrap_or_else(PoisonError::into_inner);
        if inner.live.get(&self.id).is_some_and(|(s, _)| *s == self.serial) {
            inner.live.remove(&self.id);
        }
    }
}

impl RegInner {
    fn prune(&mut self, now: Instant) {
        while self.tombs.front().is_some_and(|(_, at)| now.saturating_duration_since(*at) > TOMBSTONE_TTL) {
            self.tombs.pop_front();
        }
    }
}

/// renderer 의 requestId(`newRequestId`: `prefix-<base36>-<base36>`, 수십 바이트) 상한.
/// 넘거나 비면 requestId 없음으로 취급한다 — tombstone 이 renderer 가 정한 큰 문자열을 붙잡아 두지 않게(보안 리뷰 L1)
const MAX_REQUEST_ID: usize = 128;

pub(crate) fn is_request_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= MAX_REQUEST_ID
}

impl CancelRegistry {
    /// 핸들러가 **첫 await 전에** 부른다. 이미 cancel 이 도착해 있었으면 취소된 토큰을 돌려준다
    pub fn register(self: &Arc<Self>, id: &str) -> Registration {
        self.register_at(id, Instant::now())
    }

    pub fn cancel(&self, id: &str) {
        self.cancel_at(id, Instant::now());
    }

    fn register_at(self: &Arc<Self>, id: &str, now: Instant) -> Registration {
        let token = CancellationToken::new();
        let mut inner = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        inner.prune(now);
        if let Some(pos) = inner.tombs.iter().position(|(t, _)| t == id) {
            inner.tombs.remove(pos);
            token.cancel();
        }
        inner.serial += 1;
        let serial = inner.serial;
        inner.live.insert(id.to_string(), (serial, token.clone()));
        Registration { registry: self.clone(), id: id.to_string(), serial, token }
    }

    fn cancel_at(&self, id: &str, now: Instant) {
        let mut inner = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        inner.prune(now);
        if let Some((_, token)) = inner.live.get(id) {
            token.cancel();
            return;
        }
        // 아직 등록 전이다 — 곧 도착할 요청을 위해 기억한다(같은 id 가 이미 있으면 새로 적지 않는다)
        if inner.tombs.iter().any(|(t, _)| t == id) {
            return;
        }
        inner.tombs.push_back((id.to_string(), now));
        while inner.tombs.len() > TOMBSTONE_MAX {
            inner.tombs.pop_front();
        }
    }

    #[cfg(test)]
    fn counts(&self) -> (usize, usize) {
        let inner = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        (inner.live.len(), inner.tombs.len())
    }
}

// ── 라우터 ────────────────────────────────────────────

/// 소스(local 하나, wsl 은 배포판마다 하나)마다 실행기와 동시성 큐를 하나씩 둔다(TS `RepoService.runtime` 과 같다).
/// 큐의 결과 타입은 직렬화된 JSON 이라 이후 채널들이 같은 큐(같은 6/4 상한)를 나눠 쓸 수 있다
/// ssh 소스의 실행기는 [`SshPool`] 에서 온다(호스트마다 하나 — fetch 서비스와 나눠 쓴다)
pub(crate) struct SourceRuntime {
    pub(crate) exec: Arc<GitExecutor>,
    pub(crate) queue: ProcessQueue<Arc<Value>>,
}

/// 소스별 실행기·큐와 SSH 풀. 라우터와 감시 호스트([`crate::watch_host`])가 **같은 것**을 쥔다 — 감시가 따로 연결을 맺으면 호스트당 연결이 둘이 된다.
/// 복제하면 같은 표를 본다
#[derive(Clone)]
pub(crate) struct Runtimes {
    mirror: RepoMirror,
    sources: Arc<Mutex<HashMap<String, Arc<SourceRuntime>>>>,
    /// 주입된 SSH 풀(스위치가 꺼져 있으면 없다)
    pool: Arc<Mutex<Option<Arc<SshPool>>>>,
    /// WSL 배포판 등록 관문 — 등록된 배포판이 없으면 WSL 실행기를 내주지 않는다(`wsl.exe` 스텁이 "WSL 설치" 창을 띄운다)
    wsl_gate: Arc<DistroGate>,
}

impl Runtimes {
    pub(crate) fn new(mirror: RepoMirror) -> Self {
        Self::with_wsl_gate(mirror, DistroGate::system())
    }

    /// 시험이 레지스트리 대신 가짜 프로브를 준다
    pub(crate) fn with_wsl_gate(mirror: RepoMirror, gate: DistroGate) -> Self {
        Runtimes { mirror, sources: Arc::default(), pool: Arc::default(), wsl_gate: Arc::new(gate) }
    }

    fn ssh_pool(&self) -> Option<Arc<SshPool>> {
        self.pool.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// 소스의 실행기·큐. ssh 는 호스트마다 하나이고 실행기는 풀에서 온다(풀이 없으면 — 미러가 ssh 를 풀지 않으므로 오지 않는 길 — 오류)
    pub(crate) fn get(&self, target: &Target) -> Result<Arc<SourceRuntime>, ExecError> {
        // 기억해 둔 실행기가 있어도 먼저 본다 — 배포판이 지워진 뒤에도 폴링이 `wsl.exe` 를 부르지 않게
        if matches!(target, Target::Wsl { .. }) && !self.wsl_gate.is_registered() {
            return Err(ExecError::Other { message: "WSL 배포판이 등록돼 있지 않습니다".to_string() });
        }
        let mut sources = self.sources.lock().unwrap_or_else(PoisonError::into_inner);
        let key = target.source_key();
        if let Some(rt) = sources.get(&key) {
            return Ok(rt.clone());
        }
        let rt = Arc::new(match target {
            Target::Local { .. } => SourceRuntime { exec: Arc::new(GitExecutor::Local(LocalExecutor::default())), queue: ProcessQueue::new(LOCAL_LIMIT) },
            Target::Wsl { distro, .. } => SourceRuntime { exec: Arc::new(GitExecutor::Wsl(WslExecutor::new(distro.clone()))), queue: ProcessQueue::new(WSL_LIMIT) },
            Target::Ssh { host, .. } => {
                // resolve 와 이 사이에 미러가 바뀌어 정리된 호스트면 다시 만들지 않는다(낡은 대상이 지운 호스트로 접속·등록하지 않게).
                // sources 잠금 안에서 본다 — 정리(sync_ssh_hosts)도 이 잠금을 잡으므로, 여기서 통과하면 뒤이은 정리가 지운다
                if !self.mirror.ssh_hosts().contains(host) {
                    return Err(ExecError::Other { message: "저장소 목록이 바뀌었습니다. 다시 시도하세요".to_string() });
                }
                let pool = self.ssh_pool().ok_or_else(|| ExecError::Other { message: "SSH 를 사용할 수 없습니다".to_string() })?;
                SourceRuntime { exec: Arc::new(GitExecutor::Ssh(pool.get(host))), queue: ProcessQueue::new(SSH_LIMIT) }
            }
        });
        sources.insert(key, rt.clone());
        Ok(rt)
    }
}

pub struct Router {
    enabled: bool,
    log: bool,
    mirror: RepoMirror,
    cancels: Arc<CancelRegistry>,
    /// 소스별 실행기·큐·SSH 풀(감시 호스트와 나눠 쓴다)
    runtimes: Runtimes,
    /// 활성 저장소 감시(Phase 6a PR 7b). **Rust 가 맡는 저장소(fetch 와 같은 소유 판정 — `RepoMirror::resolve_fetch`)만** 감시한다. 나머지(github·미러에 없음)는 사이드카가 감시한다
    watcher: RepoWatcher,
    watch_host: Arc<RouterWatchHost>,
    watch_dirs: Arc<dyn DirWatch>,
    watch_polls: PollTimes,
    /// 앱이 단 `repo:changed` 싱크(호스트가 호출 시점에 읽는다)
    watch_sink: Arc<Mutex<Option<ChangeSink>>>,
    /// 지금 감시 중인 저장소와 그 대상(미러가 바뀌었을 때 대상이 달라졌는지 본다). 감시자의 `current` 와 항상 함께 바뀐다(이 잠금 안에서)
    watch_target: Mutex<Option<(String, Target)>>,
    commit_cache: Arc<Mutex<LruCache<Arc<Value>>>>,
    compare_cache: Arc<Mutex<LruCache<Arc<Value>>>>,
    diff_cache: Arc<Mutex<LruCache<Arc<Value>>>>,
    tree_cache: Arc<Mutex<LruCache<Arc<Value>>>>,
    blob_cache: Arc<Mutex<LruCache<Arc<Value>>>>,
    /// `git:onRemote` 가 true 였던 `repoId:rev 소문자`. 원격 ref 가 사라질 수 있지만 경고용이라 허용한다(false 는 fetch 로 바뀔 수 있어 기억하지 않는다)
    on_remote_yes: Mutex<HashSet<String>>,
    /// 활동 결과 캐시(범위 키 → 시그니처·결과, 8개)
    activity_cache: Arc<Mutex<ActivityCache>>,
    /// `gh:repo` 추론 결과(없음도 담는다, 60초)·Launchpad 용 모든 GitHub remote(10분). 저장소 id 가 키다 — 목록은 미러가 바뀔 때 사라진 저장소 것을 치운다
    gh_repo_cache: Mutex<HashMap<String, (Instant, Option<GhRepoRef>)>>,
    gh_remotes_cache: Mutex<HashMap<String, (Instant, Vec<GhRepoRef>)>>,
    /// 슬롯과 무관하게 같은 키의 진행 중 요청을 합치는 큐(TS `RepoService.shared`). 슬롯을 쥔 채 시그니처의 슬롯을 기다리지 않도록 소스의 큐와 분리한다
    joins: ProcessQueue<Arc<Value>>,
    logs: LogService,
    /// 첫 페이지 문·ref 시그니처 저장소(Phase 6a PR 3a). 소비자: refsDetails(문)·activity(문·시그니처)·감시(문·시그니처 — 확인마다 기억값을 갱신한다). wipStatus(문)도 쓴다
    repo_state: RepoState,
    /// 저장소별 untracked diff 허용 목록(TS `untrackedAllow`). `wipStatus` 가 끝나면 교체하고, 미러에서 빠졌거나 대상이 바뀐 저장소는 `on_mirror_changed` 가 지운다. 없으면 untracked diff 는 거부다
    untracked_allow: Arc<Mutex<HashMap<String, AllowEntry>>>,
    /// 앱이 단 사이드카 알림 싱크. 상태에 먼저 반영한 뒤 부른다(`note_sink`)
    user_sink: Arc<Mutex<Option<NoteSink>>>,
    rust_handled: AtomicU64,
    /// fetch 서비스(앱이 켜져 있을 때만 주입한다). 없으면 fetch 채널은 오류다
    fetch: Mutex<Option<Arc<dyn FetchControl>>>,
    /// 서비스가 주입되기 전에 도착한 fetchPrefs(마지막 것만). 주입될 때 적용한다
    pending_prefs: Mutex<Option<FetchPrefs>>,
    /// `ANTLER_RUST_SSH` 스위치(`ANTLER_RUST_GIT=0` 이어도 꺼진다). 꺼져 있으면 ssh 저장소는 풀 없이 사이드카로 간다
    ssh_enabled: bool,
    /// `ANTLER_RUST_API` 스위치(`ANTLER_RUST_GIT=0` 이어도 꺼진다). 꺼져 있으면 `jira:*` 는 서비스 없이 사이드카로 간다
    api_enabled: bool,
    /// Jira 서비스(앱이 스위치가 켜져 있을 때만 주입한다). 없으면 `jira:*` 는 사이드카가 처리한다
    jira: Mutex<Option<JiraHost>>,
    /// GitHub 서비스(Jira 와 같은 조건으로 주입한다). 없으면 `gh:*` 는 사이드카가 처리한다
    github: Mutex<Option<GithubHost>>,
    /// 사이드카를 `ANTLER_RUST_API=1` 로 띄웠다([`Router::claim_api`]). 켜지면 사이드카는 `jira:*`·`gh:*` 를 받지 않는다
    api_owned: AtomicBool,
    /// 대화상자·클립보드·열기 포트(앱이 주입한다). 없거나 `ANTLER_RUST_GIT=0` 이면 [`APP_CHANNELS`] 는 사이드카가 처리한다
    app: Mutex<Option<AppHost>>,
}

impl Router {
    pub fn new(enabled: bool, log: bool, mirror: RepoMirror) -> Self {
        let repo_state = RepoState::default();
        let user_sink: Arc<Mutex<Option<NoteSink>>> = Arc::default();
        let logs = LogService::new(mirror.clone());
        logs.set_sink(fanout_sink(&repo_state, &mirror, &user_sink));
        let runtimes = Runtimes::new(mirror.clone());
        let watch_sink: Arc<Mutex<Option<ChangeSink>>> = Arc::default();
        let watch_host = Arc::new(RouterWatchHost::new(mirror.clone(), repo_state.clone(), runtimes.clone(), watch_sink.clone()));
        let watch_dirs: Arc<dyn DirWatch> = Arc::new(NotifyDirs::new());
        let watcher = RepoWatcher::new(watch_host.clone(), watch_dirs.clone());
        Router {
            watcher,
            watch_host,
            watch_dirs,
            watch_polls: PollTimes::default(),
            watch_sink,
            watch_target: Mutex::default(),
            enabled,
            ssh_enabled: enabled,
            api_enabled: enabled,
            jira: Mutex::default(),
            github: Mutex::default(),
            api_owned: AtomicBool::new(false),
            app: Mutex::default(),
            log,
            mirror: mirror.clone(),
            cancels: Arc::default(),
            runtimes,
            commit_cache: Arc::new(Mutex::new(LruCache::new(COMMIT_CACHE_ENTRIES, COMMIT_CACHE_BYTES))),
            compare_cache: Arc::new(Mutex::new(LruCache::new(COMMIT_CACHE_ENTRIES, COMMIT_CACHE_BYTES))),
            diff_cache: Arc::new(Mutex::new(LruCache::new(DIFF_CACHE_ENTRIES, DIFF_CACHE_BYTES))),
            tree_cache: Arc::new(Mutex::new(LruCache::new(TREE_CACHE_ENTRIES, TREE_CACHE_BYTES))),
            blob_cache: Arc::new(Mutex::new(LruCache::new(BLOB_CACHE_ENTRIES, BLOB_CACHE_BYTES))),
            on_remote_yes: Mutex::default(),
            activity_cache: Arc::default(),
            gh_repo_cache: Mutex::default(),
            gh_remotes_cache: Mutex::default(),
            joins: ProcessQueue::new(JOIN_SLOTS),
            logs,
            repo_state,
            untracked_allow: Arc::default(),
            user_sink,
            rust_handled: AtomicU64::new(0),
            fetch: Mutex::default(),
            pending_prefs: Mutex::default(),
        }
    }

    /// 스위치가 켜져 있는가(`ANTLER_RUST_GIT` 이 0 이 아니다)
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// `jira:*` 스위치를 정한다(만든 직후에만). 기본은 git 스위치와 같다
    pub fn with_api(mut self, enabled: bool) -> Self {
        self.api_enabled = enabled;
        self
    }

    /// `ssh` 스위치를 정한다(만든 직후에만). 기본은 git 스위치와 같다
    pub fn with_ssh(mut self, enabled: bool) -> Self {
        self.ssh_enabled = enabled;
        self
    }

    /// `ANTLER_RUST_SSH` 스위치가 켜져 있는가(풀이 주입됐는지와는 별개 — 만들기 전에 이 값으로 정한다)
    pub fn ssh_enabled(&self) -> bool {
        self.enabled && self.ssh_enabled
    }

    /// Rust 가 ssh 저장소의 git 채널(fetch 제외)을 처리하는가: 스위치가 켜져 있고 풀이 주입됐다. **미러의 ssh 해석**(`RepoMirror::set_ssh`)과
    /// 사이드카 env(`ANTLER_RUST_SSH`)가 모두 이 값 하나로 정해진다
    pub fn ssh_active(&self) -> bool {
        self.enabled && self.ssh_enabled && self.ssh_pool().is_some()
    }

    /// SSH 풀을 주입한다. 스위치가 꺼져 있으면 주입하지 않는다(연결 설정을 읽지 않고 ssh 는 사이드카가 전부 맡는다). 주입하면 미러가 ssh 를 Rust 로 풀기 시작한다.
    /// 이후 fetch 서비스도 이 풀을 쓴다(호스트당 연결 하나)
    pub fn set_ssh_pool(&self, pool: Arc<SshPool>) {
        if self.enabled && self.ssh_enabled {
            *self.runtimes.pool.lock().unwrap_or_else(PoisonError::into_inner) = Some(pool);
            self.mirror.set_ssh(true);
        }
    }

    /// 주입된 SSH 풀(스위치가 꺼져 있으면 없다)
    pub fn ssh_pool(&self) -> Option<Arc<SshPool>> {
        self.runtimes.ssh_pool()
    }

    /// `ANTLER_RUST_API` 스위치가 켜져 있는가(서비스가 만들어졌는지와는 별개 — 만들기 전에 이 값으로 정한다)
    pub fn api_enabled(&self) -> bool {
        self.api_enabled
    }

    /// Rust 가 `jira:*`·`gh:*` 를 처리하는가: 스위치가 켜져 있고 **두 서비스가 모두** 주입됐다. 사이드카 env(`ANTLER_RUST_API`) 하나가 둘을 함께 넘기므로
    /// 한쪽만 있으면 활성이 아니다(둘 다 사이드카). 사이드카를 `ANTLER_RUST_API=1` 로 띄울지, 채널을 Rust 가 받을지 모두 이 값 하나로 정한다
    pub fn api_active(&self) -> bool {
        self.api_enabled && self.jira_port().is_some() && self.github_port().is_some()
    }

    /// 사이드카를 띄우기 직전에 부른다: [`Router::api_active`] 를 돌려주고, 켜졌으면 "사이드카에 jira·gh 를 맡기지 않았다(소유는 Rust)"를 기록한다.
    /// 이후 이 상태에서 서비스가 없는 채널은 사이드카로 가지 않고 오류다(사이드카는 이미 그 채널을 처리하지 않는다)
    pub fn claim_api(&self) -> bool {
        let active = self.api_active();
        self.api_owned.store(active, Ordering::SeqCst);
        active
    }

    /// 앱 채널의 포트를 주입한다. 스위치가 꺼져 있으면(`ANTLER_RUST_GIT=0`) 주입하지 않는다 — 사이드카가 전부 맡는다
    pub fn set_app(&self, host: AppHost) {
        if self.enabled {
            *self.app.lock().unwrap_or_else(PoisonError::into_inner) = Some(host);
        }
    }

    fn app_port(&self) -> Option<AppHost> {
        self.app.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    fn jira_port(&self) -> Option<JiraHost> {
        self.jira.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    fn github_port(&self) -> Option<GithubHost> {
        self.github.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// Rust 가 `gh:*` 를 처리하는가: 스위치가 켜져 있고 GitHub 서비스가 주입됐다(앱은 Jira 와 둘 다 만들어졌을 때만 주입한다)
    pub fn github_active(&self) -> bool {
        self.api_enabled && self.github_port().is_some()
    }

    /// GitHub 서비스를 주입한다. 스위치가 꺼져 있으면 주입하지 않는다(서비스·키체인이 없어야 한다)
    pub fn set_github(&self, host: GithubHost) {
        if self.api_enabled {
            *self.github.lock().unwrap_or_else(PoisonError::into_inner) = Some(host);
        }
    }

    /// Jira 서비스를 주입한다. 스위치가 꺼져 있으면 주입하지 않는다(서비스·키체인이 없어야 한다)
    pub fn set_jira(&self, host: JiraHost) {
        if self.api_enabled {
            *self.jira.lock().unwrap_or_else(PoisonError::into_inner) = Some(host);
        }
    }

    fn fetch_port(&self) -> Option<Arc<dyn FetchControl>> {
        self.fetch.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// fetch 서비스를 주입한다. 앞서 도착해 있던 fetchPrefs 가 있으면 적용한다. 스위치가 꺼져 있으면 주입하지 않는다(이중 fetch 방지)
    pub fn set_fetch(&self, service: Arc<dyn FetchControl>) {
        if !self.enabled {
            return;
        }
        *self.fetch.lock().unwrap_or_else(PoisonError::into_inner) = Some(service.clone());
        if let Some(prefs) = self.pending_prefs.lock().unwrap_or_else(PoisonError::into_inner).take() {
            service.apply_prefs(prefs);
        }
    }

    /// 사이드카의 fetchPrefs 를 서비스에 적용한다(서비스가 아직 없으면 마지막 것을 보관한다). 런타임 안에서 불러야 한다([`in_runtime_of`])
    pub fn apply_fetch_prefs(&self, prefs: FetchPrefs) {
        if !self.enabled {
            return;
        }
        match self.fetch_port() {
            Some(service) => {
                service.apply_prefs(prefs);
            }
            None => *self.pending_prefs.lock().unwrap_or_else(PoisonError::into_inner) = Some(prefs),
        }
    }

    /// 창이 포커스를 얻었다 — 활성 저장소의 자동 fetch 가 밀렸으면 바로 한다. 런타임 안에서 불러야 한다
    pub fn fetch_on_focus(&self) {
        if let Some(service) = self.fetch_port() {
            service.on_focus();
        }
    }

    /// 환경변수로 켜고 끈다. `ANTLER_RUST_GIT=0` 이면 모든 채널을 사이드카로 보낸다.
    /// `ANTLER_ROUTE_LOG=1` 이면 [`RUST_CHANNELS`] 채널이 어디로 갔는지 stderr 에 한 줄씩 남긴다(확인용 — 기본은 조용하다)
    pub fn from_env(mirror: RepoMirror) -> Self {
        let enabled = rust_git_enabled(std::env::var("ANTLER_RUST_GIT").ok().as_deref());
        let api = rust_api_enabled(std::env::var("ANTLER_RUST_API").ok().as_deref(), std::env::var("ANTLER_RUST_GIT").ok().as_deref());
        let ssh = rust_ssh_enabled(std::env::var("ANTLER_RUST_SSH").ok().as_deref(), std::env::var("ANTLER_RUST_GIT").ok().as_deref());
        let log = std::env::var("ANTLER_ROUTE_LOG").as_deref() == Ok("1");
        Router::new(enabled, log, mirror).with_api(api).with_ssh(ssh)
    }

    /// 로그 커서의 유휴 시간·훑는 주기를 바꾼다(테스트용 — 프로세스 목록을 읽는 테스트가 느려도 커서가 먼저 닫히지 않게). 만든 직후에만 부른다
    pub fn with_log_timing(mut self, idle: Duration, sweep: Duration) -> Self {
        self.logs = LogService::with_timing(self.mirror.clone(), idle, sweep);
        self.logs.set_sink(self.note_sink());
        self
    }

    /// `git:log` 가 사이드카에 알릴 일(첫 페이지가 나갔다·이력이 바뀌었다)을 받는 곳. 앱은 사이드카에 `note` 메시지로 보낸다
    pub fn set_note_sink(&self, sink: NoteSink) {
        *self.user_sink.lock().unwrap_or_else(PoisonError::into_inner) = Some(sink);
    }

    /// Rust 가 사이드카에 알릴 일(첫 페이지·이력 변경·fetch 변경)의 **유일한 길**. 공유 상태에 먼저 반영하고(동기), 그다음 [`set_note_sink`](Self::set_note_sink) 의 싱크로 보낸다.
    /// 로그 서비스는 이것을 쓰고, fetch 호스트도 이것으로 알려야 한다(`AppFetchHost`) — 그래야 사이드카와 Rust 의 기억이 같은 시점에 낡은 것이 된다
    pub fn note_sink(&self) -> NoteSink {
        fanout_sink(&self.repo_state, &self.mirror, &self.user_sink)
    }

    /// 첫 페이지 문·ref 시그니처 저장소
    pub fn repo_state(&self) -> &RepoState {
        &self.repo_state
    }

    /// 감시가 이벤트원을 바꿔 끼운다(만든 직후에만 — 시험이 가짜·계수 가능한 이벤트원을 준다)
    pub fn with_dir_watch(mut self, dirs: Arc<dyn DirWatch>) -> Self {
        self.watch_dirs = dirs;
        self.rebuild_watcher();
        self
    }

    /// 감시 폴링 주기를 바꾼다(만든 직후에만 — 시험이 실제 소켓에서 짧은 주기로 폴링을 본다)
    pub fn with_watch_poll(mut self, polls: PollTimes) -> Self {
        self.watch_polls = polls;
        self.rebuild_watcher();
        self
    }

    fn rebuild_watcher(&mut self) {
        self.watcher = RepoWatcher::with_poll(self.watch_host.clone(), self.watch_dirs.clone(), self.watch_polls);
    }

    /// 감시가 ref 변화를 알릴 곳(`repo:changed`)을 단다. 감시 태스크에서 불리므로 막지 않는다. 나중에 달아도 된다
    pub fn set_change_sink(&self, sink: ChangeSink) {
        *self.watch_sink.lock().unwrap_or_else(PoisonError::into_inner) = Some(sink);
    }

    /// 창 포커스 변화(앱이 창 이벤트에서 부른다). 다시 포커스를 얻으면 감시가 바로 한 번 확인하고, 잃으면 폴링을 늦춘다. 어느 스레드에서 불러도 된다
    pub fn watch_focus(&self, focused: bool) {
        self.watcher.set_focused(focused);
    }

    /// 지금 Rust 가 감시 중인 저장소(시험·확인용)
    pub fn watching(&self) -> Option<String> {
        self.watcher.current()
    }

    /// 감시할 저장소를 정한다. **소유 판정은 fetch 와 같은 한 술어**(`resolve_fetch` — 종류 대조. 사이드카가 env `ANTLER_RUST_FETCH_KINDS` 로 같은 판정을 한다)라
    /// Rust 가 맡는 저장소가 아니면(github·미러에 없음·검사에 걸림) `None` 이고 이전 감시를 멈춘다 — 사이드카가 맡지 않는 구간도 겹치는 구간도 없다.
    /// 같은 저장소라도 대상(경로·호스트)이 달라졌으면 다시 시작한다. tokio 런타임 안에서 불러야 한다
    fn set_watch(&self, active: Option<&str>) {
        let next = active.and_then(|id| match self.mirror.resolve_fetch(id) {
            Resolved::Rust(target) => Some((id.to_string(), target)),
            _ => None,
        });
        let mut current = self.watch_target.lock().unwrap_or_else(PoisonError::into_inner);
        match &next {
            Some((id, target)) => {
                if current.as_ref().is_some_and(|(cur_id, cur)| cur_id == id && cur != target) {
                    self.watcher.watch(None);
                }
                self.watcher.watch(Some(id));
            }
            None => self.watcher.watch(None),
        }
        *current = next;
    }

    /// 미러가 바뀐 뒤 감시 대상을 맞춘다 — 저장소가 사라졌거나 Rust 가 맡지 않게 됐으면 `unwatch_if`(그 사이 다른 저장소로 바뀐 감시를 끊지 않는다), 대상이 바뀌었으면 다시 시작한다
    fn sync_watch(&self) {
        let mut current = self.watch_target.lock().unwrap_or_else(PoisonError::into_inner);
        let Some((id, old)) = current.clone() else { return };
        match self.mirror.resolve_fetch(&id) {
            Resolved::Rust(target) if target == old => {}
            Resolved::Rust(target) => {
                self.watcher.watch(None);
                self.watcher.watch(Some(&id));
                *current = Some((id, target));
            }
            _ => {
                self.watcher.unwatch_if(&id);
                *current = None;
            }
        }
    }

    /// 미러가 바뀐 직후 불린다 — 사라졌거나 대상이 바뀐 저장소의 로그 커서를 닫고, 더 이상 쓰지 않는 ssh 호스트의 연결·큐를 정리한다.
    /// 감시도 맞춘다(연결 정리보다 먼저 — 지워진 호스트에 감시가 확인을 걸지 않게)
    pub fn on_mirror_changed(&self) {
        self.sync_watch();
        self.logs.sync_mirror();
        self.sync_ssh_hosts();
        // 사라졌거나 Rust 가 맡지 않게 된 저장소의 remote 추론 결과를 치운다(저장소 id 로 쌓이는 표가 앱 수명 동안 자라지 않게)
        let mirror = &self.mirror;
        let owned = |id: &String| matches!(mirror.resolve(id), Resolved::Rust(_));
        self.gh_repo_cache.lock().unwrap_or_else(PoisonError::into_inner).retain(|id, _| owned(id));
        // Launchpad 의 remote 목록은 통째로 비운다 — `repos:update` 무효화(route 시점)와 미러 반영(비동기) 사이에 옛 경로로 채운 항목이
        // 소유만 보는 retain 으로는 남는다(10분 캐시라 다시 채우는 비용이 작다)
        self.invalidate_gh_remotes(None);
        // 미러에서 빠졌거나 Rust 가 맡지 않게 됐거나 대상이 바뀐 저장소의 untracked diff 허용 목록을 버린다(TS `forgetWip` — 저장소를 지울 때)
        self.untracked_allow.lock().unwrap_or_else(PoisonError::into_inner).retain(|id, entry| matches!(self.mirror.resolve(id), Resolved::Rust(t) if t == entry.target));
        // 사라졌거나 Rust 가 맡지 않게 된 저장소의 fetch 상태를 비운다
        if let Some(service) = self.fetch_port() {
            service.sync_mirror();
        }
    }

    /// 미러에 없는(또는 Rust 로 풀리지 않는) 호스트의 SSH 연결을 닫고 풀·큐에서 뺀다. 닫은 실행기는 영구히 거부하므로 **풀과 `sources` 를 함께** 비워야
    /// 같은 호스트를 다시 추가했을 때 새 연결이 맺어진다. `sources` 잠금을 쥔 채 풀을 만진다 — `runtime` 과 같은 순서(sources → 풀)라 그 사이에 낡은 실행기가 끼어들지 않는다
    fn sync_ssh_hosts(&self) {
        let Some(pool) = self.ssh_pool() else { return };
        let keep = self.mirror.ssh_hosts();
        let mut sources = self.runtimes.sources.lock().unwrap_or_else(PoisonError::into_inner);
        sources.retain(|key, _| key.strip_prefix("ssh:").is_none_or(|host| keep.contains(host)));
        pool.retain(&keep);
    }

    /// 앱 종료: 열려 있는 로그 커서(git 프로세스)를 모두 닫는다
    pub fn shutdown(&self) {
        self.logs.close_all();
        // 감시를 먼저 끝낸다(notify 핸들·스레드를 닫는다). 문을 풀기 전이어야 종료 중에 기준 계산 git 이 뜨지 않는다
        self.watcher.dispose();
        *self.watch_target.lock().unwrap_or_else(PoisonError::into_inner) = None;
        // 첫 페이지 문을 기다리는 조회를 풀고 시그니처 기억을 비운다
        self.repo_state.dispose();
        self.activity_cache.lock().unwrap_or_else(PoisonError::into_inner).clear();
        self.invalidate_gh_remotes(None);
        self.gh_repo_cache.lock().unwrap_or_else(PoisonError::into_inner).clear();
        self.untracked_allow.lock().unwrap_or_else(PoisonError::into_inner).clear();
        // SSH 연결을 닫는다(런타임 밖 스레드면 정중한 해제 메시지는 보내지 못하고 연결만 버린다 — 프로세스가 끝나며 소켓이 닫힌다)
        if let Some(pool) = self.ssh_pool() {
            pool.dispose_all();
        }
        // 진행 중인 fetch 를 끊는다(git 프로세스가 끝나기를 기다리지는 않는다 — 종료 때 Job 이 정리한다)
        if let Some(service) = self.fetch_port() {
            service.dispose();
        }
        // GitHub 서비스: 진행 중인 조회를 끊고 아바타 캐시를 파일에 쓴다(2초 지연 쓰기라 부르지 않으면 유실된다)
        if let Some(host) = self.github_port() {
            host.service.dispose();
        }
    }

    /// 열려 있는 로그 커서 수(테스트·확인용)
    pub fn log_cursor_count(&self) -> usize {
        self.logs.cursor_count()
    }

    /// 열려 있는 로그 커서의 git PID(테스트·확인용)
    pub fn log_cursor_pids(&self) -> Vec<u32> {
        self.logs.cursor_pids()
    }

    /// 지금까지 Rust 가 처리한 호출 수(테스트·확인용)
    pub fn rust_handled(&self) -> u64 {
        self.rust_handled.load(Ordering::Relaxed)
    }

    /// 채널 하나를 라우팅한다. `args` 는 renderer 가 보낸 인자 배열 그대로다(표식 포함)
    pub async fn route(&self, channel: &str, args: &Value) -> Routed {
        // jira:*·gh:*(gh:repo 제외)는 인자 모양과 무관하게 먼저 가로챈다 — 배열이 아닌 인자(토큰 포함 가능)가 사이드카 stdin 으로 가지 않게.
        // 배열이 아니면 빈 인자로 넘겨 서비스 검증이 거부한다(보안 리뷰)
        if API_CHANNELS.contains(&channel) {
            let raw = args.as_array().map(Vec::as_slice).unwrap_or(&[]);
            return if channel.starts_with("gh:") { self.gh_channel(channel, raw).await } else { self.jira_channel(channel, raw).await };
        }
        // 앱 채널(폴더 선택·클립보드·링크·타이틀바 테마)은 설정·미러와 무관하다. 인자 모양과 무관하게 먼저 가로챈다 — 복사할 텍스트가 배열이 아닌 인자로도 사이드카 stdin 으로 가지 않게.
        // 배열이 아니면 빈 인자로 넘겨 검증이 거부한다
        if APP_CHANNELS.contains(&channel) {
            let Some(host) = self.app_port().filter(|_| self.enabled) else {
                self.note(channel, "sidecar(ANTLER_RUST_GIT=0)");
                return Routed::Sidecar;
            };
            self.note(channel, "rust");
            self.rust_handled.fetch_add(1, Ordering::Relaxed);
            let raw = args.as_array().map(Vec::as_slice).unwrap_or(&[]);
            let args: Vec<Option<Value>> = decode_args(raw).into_iter().map(|a| a.cloned()).collect();
            return Routed::Done(app_host::app_envelope(host, channel, args).await);
        }
        let Some(raw) = args.as_array() else { return Routed::Sidecar };
        if channel == "req:cancel" {
            // Rust 레지스트리를 먼저 취소하고, 요청이 어디서 돌든(사이드카 포함) 항상 사이드카에도 전달한다
            if let Some(Value::String(id)) = raw.first() {
                if is_request_id(id) {
                    self.cancels.cancel(id);
                }
                // Jira 서비스도 자기 요청 표(TS `JiraService.cancel` 과 같다)에서 같은 id 를 찾아 끊는다. 없는 id 는 무시한다
                // (레지스트리의 `is_request_id` 상한과 무관하다 — 서비스는 상한 없이 등록하므로 TS 와 같게 빈 id 가 아니면 부른다)
                if let Some(host) = self.jira_port() {
                    host.service.cancel(id);
                }
                // GitHub 서비스도 같다(TS `GitHubService.cancel`). 없는 id 는 무시한다
                if let Some(host) = self.github_port() {
                    host.service.cancel(id);
                }
            }
            return Routed::Sidecar;
        }
        if channel == "repos:watch" {
            // 사이드카로 넘기기 전에 다른 저장소(또는 null 이면 전부)의 로그 커서를 닫는다 — 멈춘 git log 가 pack 파일 핸들을 쥐지 않게
            let active = match raw.first() {
                Some(Value::String(id)) => Some(id.as_str()),
                _ => None,
            };
            self.logs.close_others(active);
            // 활성화된 저장소의 첫 페이지 문을 닫고 기억한 시그니처를 버린다. 응답은 사이드카가 한다(renderer 는 기다리지 않는다).
            // TS 의 `if (repoId && config.repo(repoId)) armFirstPage(repoId)` — Rust 가 푸는 저장소만(소비자가 Rust 쪽에 있는 저장소). null 은 arm 하지 않는다
            if self.enabled {
                if let Some(id) = active.filter(|id| matches!(self.mirror.resolve(id), Resolved::Rust(_))) {
                    self.repo_state.arm(id);
                }
                // 감시: arm 다음이다(기준 시그니처는 첫 페이지 문 뒤에 잡는다). Rust 가 맡는 저장소가 아니면 이전 감시를 멈춘다
                self.set_watch(active);
            }
            // 자동 fetch 대상. Rust 가 fetch 를 맡는 저장소(미러에 있고 소스 종류가 local·wsl, 그리고 Rust 가 ssh 를 풀 때는 ssh)만이고, 아니면 비운다(사이드카 쪽 저장소로 옮겨 갔을 때 이전 예약이 남지 않게).
            // TS 의 `setActive(repoId && config.repo(repoId) ? repoId : null)` 에 해당한다
            if self.enabled {
                if let Some(service) = self.fetch_port() {
                    service.set_active(active.filter(|id| matches!(self.mirror.resolve_fetch(id), Resolved::Rust(_))));
                }
            }
            return Routed::Sidecar;
        }
        if channel == "repos:update" || channel == "repos:remove" {
            // TS 핸들러가 저장소를 바꾸기 전에 부르는 `invalidateGhRemotes(repoId)` — 모든 GitHub remote 목록 캐시만 비운다(`gh:repo` 추론 결과는 TS 도 60초 그대로다).
            // 응답은 사이드카가 한다. 사이드카 쪽 캐시는 그 핸들러가 직접 비운다
            if let Some(Value::String(id)) = raw.first() {
                self.invalidate_gh_remotes(Some(id));
            }
            return Routed::Sidecar;
        }
        if !RUST_CHANNELS.contains(&channel) {
            return Routed::Sidecar;
        }
        if !self.enabled {
            self.note(channel, "sidecar(ANTLER_RUST_GIT=0)");
            return Routed::Sidecar;
        }
        let args = decode_args(raw);
        match channel {
            "git:commit" => self.git_commit(&args).await,
            "git:log" => self.git_log(&args).await,
            "git:compare" | "git:fileDiff" | "git:fileHistory" | "git:blame" => self.git_file_channel(channel, &args).await,
            "git:fetch" => self.git_fetch(&args).await,
            "git:resolveRev" | "git:tree" | "git:blob" | "git:onRemote" => self.git_code_channel(channel, &args).await,
            "git:refs" | "git:refsDetails" => self.git_refs_channel(channel, &args).await,
            "git:search" | "git:activity" => self.git_history_channel(channel, &args).await,
            "git:repoExtras" | "git:lfsPaths" => self.git_extras_channel(channel, &args).await,
            "gh:repo" => self.gh_repo_channel(&args).await,
            "git:wipStatus" | "git:wipDiff" => self.git_wip_channel(channel, &args).await,
            _ => Routed::Sidecar,
        }
    }

    /// 서비스가 없는 `jira:*`·`gh:*` 채널의 행선지. Rust 가 소유(사이드카를 env=1 로 띄움)했는데 서비스가 없으면 사이드카는 받지 않으므로 오류다.
    /// 아니면(스위치 꺼짐·서비스 없음·한쪽만 있음) 사이드카가 처리한다
    fn api_unserved(&self, channel: &str) -> Routed {
        if self.api_owned.load(Ordering::SeqCst) {
            self.note(channel, "error(서비스 없음)");
            return Routed::Done(err_env(API_UNAVAILABLE, "ERROR"));
        }
        self.note(channel, "sidecar(ANTLER_RUST_API=0)");
        Routed::Sidecar
    }

    /// `jira:*` 한 채널. 서비스가 없으면(스위치 꺼짐·만들지 못함) 사이드카다
    async fn jira_channel(&self, channel: &str, raw: &[Value]) -> Routed {
        let Some(host) = self.jira_port().filter(|_| self.api_active()) else {
            return self.api_unserved(channel);
        };
        self.note(channel, "rust");
        self.rust_handled.fetch_add(1, Ordering::Relaxed);
        let args: Vec<Option<Value>> = decode_args(raw).into_iter().map(|a| a.cloned()).collect();
        Routed::Done(api_host::jira_envelope(host, self.cancels.clone(), channel.to_string(), args).await)
    }

    /// `gh:*` 한 채널(`gh:repo` 는 오지 않는다). 서비스가 없으면(스위치 꺼짐·만들지 못함) 사이드카다
    async fn gh_channel(&self, channel: &str, raw: &[Value]) -> Routed {
        let Some(host) = self.github_port().filter(|_| self.api_active()) else {
            return self.api_unserved(channel);
        };
        self.note(channel, "rust");
        self.rust_handled.fetch_add(1, Ordering::Relaxed);
        let args: Vec<Option<Value>> = decode_args(raw).into_iter().map(|a| a.cloned()).collect();
        Routed::Done(api_host::gh_envelope(host, channel.to_string(), args).await)
    }

    fn note(&self, channel: &str, to: &str) {
        if self.log {
            eprintln!("[route] {channel} -> {to}");
        }
    }

    /// `git:commit(repoId, hash, requestId?)`
    async fn git_commit(&self, args: &[Option<&Value>]) -> Routed {
        // 저장소 id 가 문자열이 아니면 사이드카가 TS 와 같은 오류를 만든다
        let Some(Some(Value::String(repo_id))) = args.first() else { return Routed::Sidecar };
        let target = match self.mirror.resolve(repo_id) {
            Resolved::Rust(t) => t,
            Resolved::Sidecar => {
                self.note("git:commit", "sidecar");
                return Routed::Sidecar;
            }
            Resolved::Invalid(message) => return Routed::Done(err_env(&message, "ERROR")),
        };
        // TS 의 withSignal 과 같게 빈 문자열은 requestId 없음이다. 첫 await 전에 등록한다
        let reg = match args.get(2) {
            Some(Some(Value::String(id))) if is_request_id(id) => Some(self.cancels.register(id)),
            _ => None,
        };
        let token = reg.as_ref().map_or_else(CancellationToken::new, |r| r.token.clone());
        let hash = match args.get(1) {
            Some(Some(Value::String(s))) => Some(s.as_str()),
            _ => None,
        };
        self.note("git:commit", "rust");
        self.rust_handled.fetch_add(1, Ordering::Relaxed);
        let result = self.commit(repo_id, &target, hash, &token).await;
        drop(reg);
        Routed::Done(match result {
            Ok(detail) => ok_env((*detail).clone()),
            Err(e) => err_env(&e.to_string(), e.envelope_code()),
        })
    }

    /// `git:log(repoId, query, requestId?)`. query 는 renderer 가 보낸 값이라 숫자·scope 형식을 검증한 뒤에야 git 인자가 된다
    async fn git_log(&self, args: &[Option<&Value>]) -> Routed {
        let Some(Some(Value::String(repo_id))) = args.first() else { return Routed::Sidecar };
        let target = match self.mirror.resolve(repo_id) {
            Resolved::Rust(t) => t,
            Resolved::Sidecar => {
                self.note("git:log", "sidecar");
                return Routed::Sidecar;
            }
            Resolved::Invalid(message) => return Routed::Done(err_env(&message, "ERROR")),
        };
        // 검증 실패는 git 도 커서도 건드리지 않는다
        let query = match parse_query(args.get(1).copied().flatten()) {
            Ok(q) => q,
            Err(message) => return Routed::Done(err_env(&message, "ERROR")),
        };
        let reg = match args.get(2) {
            Some(Some(Value::String(id))) if is_request_id(id) => Some(self.cancels.register(id)),
            _ => None,
        };
        let token = reg.as_ref().map_or_else(CancellationToken::new, |r| r.token.clone());
        self.note("git:log", "rust");
        self.rust_handled.fetch_add(1, Ordering::Relaxed);
        let rt = match self.runtime(&target) {
            Ok(rt) => rt,
            Err(e) => {
                drop(reg);
                return Routed::Done(err_env(&e.to_string(), e.envelope_code()));
            }
        };
        let result = self.logs.log(repo_id, &rt.exec, &target, &query, &token).await;
        drop(reg);
        Routed::Done(match result.and_then(|page| serde_json::to_value(&page).map_err(|e| crate::git::exec::ExecError::Other { message: e.to_string() }.into())) {
            Ok(page) => ok_env(page),
            Err(e) => err_env(&e.to_string(), e.envelope_code()),
        })
    }

    /// `git:fetch(repoId, manual)`. `manual` 은 정확히 `true` 일 때만 참이다(`handlers.ts` 의 `manual === true`) — renderer 가 주장하는 값이라 다른 모양은 모두 자동 fetch 다.
    /// 저장소의 경로·kind 는 미러에서만 얻고(서비스가 다시 미러로 푼다), 미러에 없거나 github(또는 ssh 스위치가 꺼진 ssh)면 사이드카가 처리한다(`resolve_fetch` 는 소스 종류로 거른다).
    /// local·wsl·(켜진) ssh 이면 **절대 사이드카로 보내지 않는다** — 사이드카는 그 저장소를 맡지 않아(`ANTLER_RUST_FETCH_KINDS`) 보내면 오류가 나고, 보내서 둘이 fetch 하는 길도 없어야 한다
    async fn git_fetch(&self, args: &[Option<&Value>]) -> Routed {
        let Some(Some(Value::String(repo_id))) = args.first() else { return Routed::Sidecar };
        match self.mirror.resolve_fetch(repo_id) {
            Resolved::Rust(_) => {}
            Resolved::Sidecar => {
                self.note("git:fetch", "sidecar");
                return Routed::Sidecar;
            }
            Resolved::Invalid(message) => return Routed::Done(err_env(&message, "ERROR")),
        }
        let Some(service) = self.fetch_port() else {
            return Routed::Done(err_env("fetch 서비스가 준비되지 않았습니다", "ERROR"));
        };
        let manual = matches!(args.get(1), Some(Some(Value::Bool(true))));
        self.note("git:fetch", "rust");
        self.rust_handled.fetch_add(1, Ordering::Relaxed);
        let status = service.fetch(repo_id, manual).await;
        Routed::Done(match to_value(&status) {
            Ok(data) => ok_env(data),
            Err(e) => err_env(&e.to_string(), e.envelope_code()),
        })
    }

    /// 파일 단위 채널. 인자는 `git:compare(repoId, from, to, requestId?)`, `git:fileDiff|fileHistory|blame(repoId, req, requestId?)`.
    /// 검증이 먼저고(실패하면 git·레지스트리를 건드리지 않는다) 통과한 값만 `git::file_channels` 의 argv 빌더로 간다
    async fn git_file_channel(&self, channel: &str, args: &[Option<&Value>]) -> Routed {
        let Some(Some(Value::String(repo_id))) = args.first() else { return Routed::Sidecar };
        let target = match self.mirror.resolve(repo_id) {
            Resolved::Rust(t) => t,
            Resolved::Sidecar => {
                self.note(channel, "sidecar");
                return Routed::Sidecar;
            }
            Resolved::Invalid(message) => return Routed::Done(err_env(&message, "ERROR")),
        };
        let arg = |i: usize| args.get(i).copied().flatten();
        let job = match channel {
            "git:compare" => parse_compare(arg(1), arg(2)).map(|(from, to)| FileJob::Compare(from, to)),
            "git:fileDiff" => DiffRequest::parse(arg(1)).map(FileJob::Diff),
            "git:fileHistory" => HistoryRequest::parse(arg(1)).map(FileJob::History),
            _ => BlameRequest::parse(arg(1)).map(FileJob::Blame),
        };
        let job = match job {
            Ok(j) => j,
            Err(e) => return Routed::Done(err_env(&e.to_string(), e.envelope_code())),
        };
        let request_id = if channel == "git:compare" { 3 } else { 2 };
        let reg = match args.get(request_id) {
            Some(Some(Value::String(id))) if is_request_id(id) => Some(self.cancels.register(id)),
            _ => None,
        };
        let token = reg.as_ref().map_or_else(CancellationToken::new, |r| r.token.clone());
        self.note(channel, "rust");
        self.rust_handled.fetch_add(1, Ordering::Relaxed);
        let result = self.file_job(repo_id, &target, job, &token).await;
        drop(reg);
        Routed::Done(match result {
            // 캐시에 들어가지 않은(상한 초과) 큰 결과는 참조가 하나뿐이다 — 복사하지 않고 꺼낸다
            Ok(data) => ok_env(Arc::try_unwrap(data).unwrap_or_else(|a| (*a).clone())),
            Err(e) => err_env(&e.to_string(), e.envelope_code()),
        })
    }

    async fn file_job(&self, repo_id: &str, target: &Target, job: FileJob, token: &CancellationToken) -> Result<Arc<Value>, ExecError> {
        let rt = self.runtime(target)?;
        let (exec, path) = (rt.exec.clone(), target.path().to_string());
        match job {
            // 해시 쌍 기준이라 불변: 캐시 + 같은 키 요청 합치기
            FileJob::Compare(from, to) => {
                let key = format!("compare\0{repo_id}\0{}\0{}", from.lower(), to.lower());
                self.cached_shared(&rt, &self.compare_cache, key, token, move || async move {
                    let result = compare(&exec, &path, &from, &to).await?;
                    let cost = estimate_compare_bytes(&result);
                    Ok((to_value(&result)?, cost))
                })
                .await
            }
            // part/full 이 서로 다른 항목이다. 잘렸는지와 상관없이 캐시한다(TS 와 같다)
            FileJob::Diff(req) => {
                let key = req.key(repo_id);
                self.cached_shared(&rt, &self.diff_cache, key, token, move || async move {
                    let diff = file_diff(&exec, &path, &req).await?;
                    let cost = estimate_diff_bytes(&diff);
                    Ok((to_value(&diff)?, cost))
                })
                .await
            }
            // HEAD 가 움직일 수 있어 캐시하지 않고 합치지도 않는다. 취소는 이 요청의 프로세스에 바로 닿는다
            FileJob::History(req) => {
                let t = token.clone();
                rt.queue.run(None, Some(token.clone()), move || async move { file_history(&exec, &path, &req, &t).await.map(Arc::new) }).await
            }
            FileJob::Blame(req) => {
                let t = token.clone();
                rt.queue.run(None, Some(token.clone()), move || async move { file_blame(&exec, &path, &req, &t).await.map(Arc::new) }).await
            }
        }
    }

    /// 코드 탐색기 채널: `git:resolveRev(repoId, ref)`·`git:tree(repoId, {rev, dir}, requestId?)`·`git:blob(repoId, {rev, path}, requestId?)`·`git:onRemote(repoId, rev)`.
    /// 검증이 먼저고(실패하면 git·레지스트리를 건드리지 않는다) 통과한 값만 `git::code_browse` 의 argv 빌더로 간다. github 소스는 미러가 사이드카로 보낸다
    async fn git_code_channel(&self, channel: &str, args: &[Option<&Value>]) -> Routed {
        let Some(Some(Value::String(repo_id))) = args.first() else { return Routed::Sidecar };
        let target = match self.mirror.resolve(repo_id) {
            Resolved::Rust(t) => t,
            Resolved::Sidecar => {
                self.note(channel, "sidecar");
                return Routed::Sidecar;
            }
            Resolved::Invalid(message) => return Routed::Done(err_env(&message, "ERROR")),
        };
        let arg = |i: usize| args.get(i).copied().flatten();
        let job = match channel {
            "git:resolveRev" => parse_ref(arg(1)).map(|r| CodeJob::Resolve(r.to_string())),
            "git:tree" => TreeRequest::parse(arg(1)).map(CodeJob::Tree),
            "git:blob" => BlobRequest::parse(arg(1)).map(CodeJob::Blob),
            _ => parse_rev(arg(1)).map(CodeJob::OnRemote),
        };
        let job = match job {
            Ok(j) => j,
            Err(e) => return Routed::Done(err_env(&e.to_string(), e.envelope_code())),
        };
        // 취소할 수 있는 것은 tree·blob 뿐이다(requestId 는 셋째 인자). 등록은 첫 await 전에 한다
        let reg = match (&job, args.get(2)) {
            (CodeJob::Tree(_) | CodeJob::Blob(_), Some(Some(Value::String(id)))) if is_request_id(id) => Some(self.cancels.register(id)),
            _ => None,
        };
        let token = reg.as_ref().map_or_else(CancellationToken::new, |r| r.token.clone());
        self.note(channel, "rust");
        self.rust_handled.fetch_add(1, Ordering::Relaxed);
        let result = self.code_job(repo_id, &target, job, &token).await;
        drop(reg);
        Routed::Done(match result {
            Ok(data) => ok_env(Arc::try_unwrap(data).unwrap_or_else(|a| (*a).clone())),
            Err(e) => err_env(&e.to_string(), e.envelope_code()),
        })
    }

    /// 코드 탐색기 한 요청. TS `RepoService` 와 같다: 캐시 조회 → 큐 슬롯(키 없음 — 합치지 않는다) 안에서 git → 조건부 캐시.
    /// 취소는 이 요청의 프로세스에 바로 닿는다(`queue.run(None, Some(token))` + `RunOpts::with_cancel`)
    async fn code_job(&self, repo_id: &str, target: &Target, job: CodeJob, token: &CancellationToken) -> Result<Arc<Value>, ExecError> {
        let rt = self.runtime(target)?;
        let (exec, path) = (rt.exec.clone(), target.path().to_string());
        let t = token.clone();
        match job {
            // ref 는 움직이므로 캐시하지 않는다
            CodeJob::Resolve(r) => rt.queue.run(None, None, move || async move { resolve_rev(&exec, &path, &r, &RunOpts::default()).await.map(|h| Arc::new(json!(h))) }).await,
            // missing 은 캐시하지 않는다(커밋이 나중에 fetch 로 들어올 수 있다)
            CodeJob::Tree(req) => {
                let key = req.key(repo_id);
                if let Some(hit) = self.tree_cache.lock().unwrap_or_else(PoisonError::into_inner).get(&key) {
                    return Ok(hit);
                }
                let cache = self.tree_cache.clone();
                rt.queue
                    .run(None, Some(token.clone()), move || async move {
                        let result = list_tree(&exec, &path, &req, &RunOpts::with_cancel(t), CODE_TREE_MAX_ENTRIES).await?;
                        let value = Arc::new(to_value(&result)?);
                        if !result.missing {
                            cache.lock().unwrap_or_else(PoisonError::into_inner).set(&key, value.clone(), tree_cost(&result));
                        }
                        Ok(value)
                    })
                    .await
            }
            CodeJob::Blob(req) => {
                let key = req.key(repo_id);
                if let Some(hit) = self.blob_cache.lock().unwrap_or_else(PoisonError::into_inner).get(&key) {
                    return Ok(hit);
                }
                let cache = self.blob_cache.clone();
                rt.queue
                    .run(None, Some(token.clone()), move || async move {
                        let value = read_blob(&exec, &path, &req, &RunOpts::with_cancel(t)).await?;
                        let cost = blob_cost(&value);
                        let missing = value["kind"] == "missing";
                        let value = Arc::new(value);
                        if !missing {
                            cache.lock().unwrap_or_else(PoisonError::into_inner).set(&key, value.clone(), cost);
                        }
                        Ok(value)
                    })
                    .await
            }
            // true 만 기억한다(원격 ref 가 사라질 수 있지만 경고용이라 허용)
            CodeJob::OnRemote(rev) => {
                let key = format!("{repo_id}:{}", rev.lower());
                if self.on_remote_yes.lock().unwrap_or_else(PoisonError::into_inner).contains(&key) {
                    return Ok(Arc::new(json!(true)));
                }
                let yes = rt.queue.run(None, None, move || async move { is_on_remote(&exec, &path, &rev, &RunOpts::default()).await.map(|b| Arc::new(json!(b))) }).await?;
                if *yes == json!(true) {
                    let mut set = self.on_remote_yes.lock().unwrap_or_else(PoisonError::into_inner);
                    if set.len() >= ON_REMOTE_MAX {
                        set.clear();
                    }
                    set.insert(key);
                }
                Ok(yes)
            }
        }
    }

    /// 사이드바 ref 채널: `git:refs(repoId)`·`git:refsDetails(repoId)`. 인자는 저장소 id 뿐이라 검증할 값이 없고(경로·kind 는 미러에서만), github 소스는 미러가 사이드카로 보낸다.
    /// TS `RepoService` 와 같다: 큐 슬롯 안에서 git(키 `refs:<id>`·`refsDetails:<id>` 로 같은 요청을 합친다 — 취소 신호가 없다). 캐시는 없다.
    /// `refsDetails` 만 첫 로그 페이지 문(`RepoState::after_first_page`)을 **큐에 들어가기 전에** 기다린다 — 비싼 조회가 첫 화면을 늦추지 않게(문이 없으면 바로 간다)
    async fn git_refs_channel(&self, channel: &str, args: &[Option<&Value>]) -> Routed {
        let Some(Some(Value::String(repo_id))) = args.first() else { return Routed::Sidecar };
        let target = match self.mirror.resolve(repo_id) {
            Resolved::Rust(t) => t,
            Resolved::Sidecar => {
                self.note(channel, "sidecar");
                return Routed::Sidecar;
            }
            Resolved::Invalid(message) => return Routed::Done(err_env(&message, "ERROR")),
        };
        self.note(channel, "rust");
        self.rust_handled.fetch_add(1, Ordering::Relaxed);
        let details = channel == "git:refsDetails";
        let result = async {
            if details {
                self.repo_state.after_first_page(repo_id).await;
            }
            let rt = self.runtime(&target)?;
            let (exec, path) = (rt.exec.clone(), target.path().to_string());
            let key = format!("{}:{repo_id}", if details { "refsDetails" } else { "refs" });
            rt.queue
                .run(Some(&key), None, move || async move {
                    let opts = RunOpts::default();
                    Ok(Arc::new(if details { to_value(&refs_details(&exec, &path, &opts, false).await?)? } else { to_value(&refs_snapshot(&exec, &path, &opts, true).await?)? }))
                })
                .await
        }
        .await;
        Routed::Done(match result {
            Ok(data) => ok_env(Arc::try_unwrap(data).unwrap_or_else(|a| (*a).clone())),
            Err(e) => err_env(&e.to_string(), e.envelope_code()),
        })
    }

    /// 검색·활동 채널: `git:search(repoId, query, scope?, requestId?)`·`git:activity(repoId, scope?, requestId?)`. 검증이 먼저고(실패하면 git·레지스트리를 건드리지 않는다)
    /// 통과한 값만 `git::search`·`git::activity` 의 argv 빌더로 간다. 둘 다 requestId 로 취소할 수 있다(renderer 가 새 질의·범위를 보내면 이전 것을 취소한다). github 소스는 미러가 사이드카로 보낸다
    async fn git_history_channel(&self, channel: &str, args: &[Option<&Value>]) -> Routed {
        let Some(Some(Value::String(repo_id))) = args.first() else { return Routed::Sidecar };
        let target = match self.mirror.resolve(repo_id) {
            Resolved::Rust(t) => t,
            Resolved::Sidecar => {
                self.note(channel, "sidecar");
                return Routed::Sidecar;
            }
            Resolved::Invalid(message) => return Routed::Done(err_env(&message, "ERROR")),
        };
        let arg = |i: usize| args.get(i).copied().flatten();
        let (job, request_id) = if channel == "git:search" {
            (SearchRequest::parse(arg(1), arg(2)).map(HistoryJob::Search), 3)
        } else {
            (ActivityRequest::parse(arg(1)).map(HistoryJob::Activity), 2)
        };
        let job = match job {
            Ok(j) => j,
            Err(e) => return Routed::Done(err_env(&e.to_string(), e.envelope_code())),
        };
        // 등록은 첫 await 전에 한다. 빈 문자열은 requestId 없음이다
        let reg = match args.get(request_id) {
            Some(Some(Value::String(id))) if is_request_id(id) => Some(self.cancels.register(id)),
            _ => None,
        };
        let token = reg.as_ref().map_or_else(CancellationToken::new, |r| r.token.clone());
        self.note(channel, "rust");
        self.rust_handled.fetch_add(1, Ordering::Relaxed);
        let result = self.history_job(repo_id, &target, job, &token).await;
        drop(reg);
        Routed::Done(match result {
            Ok(data) => ok_env(Arc::try_unwrap(data).unwrap_or_else(|a| (*a).clone())),
            Err(e) => err_env(&e.to_string(), e.envelope_code()),
        })
    }

    /// LFS·gitflow 채널: `git:repoExtras(repoId)`·`git:lfsPaths(repoId, {rev?, paths}, requestId?)`. 검증이 먼저고(실패하면 git·레지스트리를 건드리지 않는다)
    /// 통과한 값만 `git::lfs` 의 argv 빌더로 간다. `lfsPaths` 만 requestId 로 취소할 수 있다. 캐시·합치기는 없다(TS 도 같다 — 설정이 바뀔 수 있다). github 소스는 미러가 사이드카로 보낸다
    async fn git_extras_channel(&self, channel: &str, args: &[Option<&Value>]) -> Routed {
        let Some(Some(Value::String(repo_id))) = args.first() else { return Routed::Sidecar };
        let target = match self.mirror.resolve(repo_id) {
            Resolved::Rust(t) => t,
            Resolved::Sidecar => {
                self.note(channel, "sidecar");
                return Routed::Sidecar;
            }
            Resolved::Invalid(message) => return Routed::Done(err_env(&message, "ERROR")),
        };
        let job = if channel == "git:lfsPaths" { LfsPathsRequest::parse(args.get(1).copied().flatten()).map(ExtrasJob::LfsPaths) } else { Ok(ExtrasJob::Extras) };
        let job = match job {
            Ok(j) => j,
            Err(e) => return Routed::Done(err_env(&e.to_string(), e.envelope_code())),
        };
        // 등록은 첫 await 전에 한다. 빈 문자열은 requestId 없음이다
        let reg = match (&job, args.get(2)) {
            (ExtrasJob::LfsPaths(_), Some(Some(Value::String(id)))) if is_request_id(id) => Some(self.cancels.register(id)),
            _ => None,
        };
        let token = reg.as_ref().map_or_else(CancellationToken::new, |r| r.token.clone());
        self.note(channel, "rust");
        self.rust_handled.fetch_add(1, Ordering::Relaxed);
        let result = self.extras_job(&target, job, &token).await;
        drop(reg);
        Routed::Done(match result {
            Ok(data) => ok_env(Arc::try_unwrap(data).unwrap_or_else(|a| (*a).clone())),
            Err(e) => err_env(&e.to_string(), e.envelope_code()),
        })
    }

    async fn extras_job(&self, target: &Target, job: ExtrasJob, token: &CancellationToken) -> Result<Arc<Value>, ExecError> {
        let rt = self.runtime(target)?;
        let (exec, path) = (rt.exec.clone(), target.path().to_string());
        match job {
            // 큐 슬롯 하나 안에서 git 여러 번이 동시에 돈다. 취소 신호가 없다(TS 도 같다)
            ExtrasJob::Extras => rt.queue.run(None, None, move || async move { Ok(Arc::new(to_value(&repo_extras(&exec, &path, &RunOpts::default()).await)?)) }).await,
            // 취소는 이 요청의 프로세스에 바로 닿는다(`withSignal`)
            ExtrasJob::LfsPaths(req) => {
                let t = token.clone();
                rt.queue.run(None, Some(token.clone()), move || async move { Ok(Arc::new(json!(lfs_paths(&exec, &path, &req, &RunOpts::with_cancel(t)).await?))) }).await
            }
        }
    }

    /// `gh:repo(repoId)` — 저장소의 GitHub remote(없으면 `null`). 인자는 저장소 id 뿐이라 검증할 값이 없고(경로·kind 는 미러에서만), github kind·미러에 없는 id 는 사이드카가 답한다
    /// (원격 전용 저장소는 레코드가 어느 GitHub 저장소인지 안다 — git 을 부르지 않는다)
    async fn gh_repo_channel(&self, args: &[Option<&Value>]) -> Routed {
        let Some(Some(Value::String(repo_id))) = args.first() else { return Routed::Sidecar };
        let target = match self.mirror.resolve(repo_id) {
            Resolved::Rust(t) => t,
            Resolved::Sidecar => {
                self.note("gh:repo", "sidecar");
                return Routed::Sidecar;
            }
            Resolved::Invalid(message) => return Routed::Done(err_env(&message, "ERROR")),
        };
        self.note("gh:repo", "rust");
        self.rust_handled.fetch_add(1, Ordering::Relaxed);
        Routed::Done(match self.gh_repo_of(repo_id, &target).await {
            Ok(found) => ok_env(found.as_ref().map_or(Value::Null, ref_json)),
            Err(e) => err_env(&e.to_string(), e.envelope_code()),
        })
    }

    /// Rust 가 git 을 돌릴 저장소인가. `Ok(None)` 이면 사이드카의 몫(스위치 꺼짐·미러에 없음·github·풀 없는 ssh)이고, `Err` 는 미러가 거부한 저장소(경로·호스트·배포판 검사)다 —
    /// 그 저장소에는 사이드카도 git 을 돌리지 않는다. 채널 라우팅과 GitHub 서비스의 저장소 질의가 **같은 술어**(`RepoMirror::resolve`)를 쓴다
    fn rust_target(&self, repo_id: &str) -> Result<Option<Target>, ExecError> {
        if !self.enabled {
            return Ok(None);
        }
        match self.mirror.resolve(repo_id) {
            Resolved::Rust(t) => Ok(Some(t)),
            Resolved::Sidecar => Ok(None),
            Resolved::Invalid(message) => Err(ExecError::Other { message }),
        }
    }

    /// GitHub 서비스의 `gh_repo` 질의(`RepoQuery`): Rust 가 맡는 저장소면 `Some`(결과 또는 오류), 아니면 `None`(호출자가 사이드카에 묻는다)
    pub async fn gh_repo(&self, repo_id: &str) -> Option<Result<Option<GhRepoRef>, ExecError>> {
        match self.rust_target(repo_id) {
            Ok(Some(target)) => Some(self.gh_repo_of(repo_id, &target).await),
            Ok(None) => None,
            Err(e) => Some(Err(e)),
        }
    }

    /// remote URL 에서 추론한 GitHub 저장소. TS `RepoService.ghRepo` 와 같은 순서: 60초 캐시(없음도) → 첫 로그 페이지 문 → 큐(키 `remotes:<id>`)
    async fn gh_repo_of(&self, repo_id: &str, target: &Target) -> Result<Option<GhRepoRef>, ExecError> {
        if let Some((at, value)) = self.gh_repo_cache.lock().unwrap_or_else(PoisonError::into_inner).get(repo_id) {
            if at.elapsed() < GH_REPO_TTL {
                return Ok(value.clone());
            }
        }
        self.repo_state.after_first_page(repo_id).await;
        let list = self.remote_list(repo_id, target, None).await?;
        let value = pick_github_remote(&list);
        self.gh_repo_cache.lock().unwrap_or_else(PoisonError::into_inner).insert(repo_id.to_string(), (Instant::now(), value.clone()));
        Ok(value)
    }

    /// GitHub 서비스(Launchpad)의 `gh_remotes` 질의: 저장소의 모든 GitHub remote. 10분 캐시이고 `invalidate_gh_remotes` 로 비운다.
    /// 취소하면 기다리기만 그만둔다(같은 키로 합쳐진 `gh_repo` 의 git 은 그 요청이 남아 있는 동안 계속 돈다)
    pub async fn gh_remotes(&self, repo_id: &str, cancel: &CancellationToken) -> Option<Result<Vec<GhRepoRef>, ExecError>> {
        let target = match self.rust_target(repo_id) {
            Ok(Some(t)) => t,
            Ok(None) => return None,
            Err(e) => return Some(Err(e)),
        };
        if let Some((at, value)) = self.gh_remotes_cache.lock().unwrap_or_else(PoisonError::into_inner).get(repo_id) {
            if at.elapsed() < GH_REMOTES_TTL {
                return Some(Ok(value.clone()));
            }
        }
        Some(
            async {
                let list = self.remote_list(repo_id, &target, Some(cancel)).await?;
                // 사이드카 경로(repoq)와 같은 응답 모양 상한 — remote 100개·이름 200자. 악성 `.git/config` 의 remote 수십만 개가 10분 캐시에 남지 않게
                let value: Vec<GhRepoRef> = pick_github_remotes(&list).into_iter().filter(|r| r.remote.chars().count() <= GH_REMOTE_NAME_MAX).take(GH_REMOTES_MAX).collect();
                self.gh_remotes_cache.lock().unwrap_or_else(PoisonError::into_inner).insert(repo_id.to_string(), (Instant::now(), value.clone()));
                Ok(value)
            }
            .await,
        )
    }

    /// remote 이름·URL 목록. `gh_repo`·`gh_remotes` 가 키 `remotes:<id>` 로 합친다(같은 git 한 번). 취소 신호가 있으면 이 호출자의 기다림만 끊는다
    async fn remote_list(&self, repo_id: &str, target: &Target, cancel: Option<&CancellationToken>) -> Result<Vec<Remote>, ExecError> {
        let rt = self.runtime(target)?;
        let (exec, path) = (rt.exec.clone(), target.path().to_string());
        let key = format!("remotes:{repo_id}");
        let run = rt.queue.run(Some(&key), None, move || async move { Ok(Arc::new(to_value(&remotes(&exec, &path, &RunOpts::default()).await?)?)) });
        let data = match cancel {
            Some(token) => tokio::select! {
                biased;
                () = token.cancelled() => Err(ExecError::Aborted),
                r = run => r,
            },
            None => run.await,
        }?;
        serde_json::from_value((*data).clone()).map_err(|e| ExecError::Other { message: e.to_string() })
    }

    /// 아바타 조회용 `pushed_shas_by_author` 질의: 그 작성자의 push 된 커밋 해시. Rust 가 맡는 저장소면 `Some`, 아니면 `None`.
    /// 요청 검증(이메일·개수)은 사이드카의 `repoq` 검사와 같고, 큐 슬롯을 기다리는 동안과 git 실행 중 모두 `cancel` 이 닿는다
    pub async fn pushed_shas_by_author(&self, repo_id: &str, email: &str, limit: usize, cancel: &CancellationToken) -> Option<Result<Vec<String>, ExecError>> {
        let target = match self.rust_target(repo_id) {
            Ok(Some(t)) => t,
            Ok(None) => return None,
            Err(e) => return Some(Err(e)),
        };
        Some(
            async {
                check_pushed_request(email, limit).map_err(|m| ExecError::Other { message: m.to_string() })?;
                let rt = self.runtime(&target)?;
                let (exec, path, email, t) = (rt.exec.clone(), target.path().to_string(), email.to_string(), cancel.clone());
                let data = rt
                    .queue
                    .run(None, Some(cancel.clone()), move || async move { Ok(Arc::new(json!(pushed_shas(&exec, &path, &email, limit, &RunOpts::with_cancel(t)).await?))) })
                    .await?;
                serde_json::from_value((*data).clone()).map_err(|e| ExecError::Other { message: e.to_string() })
            }
            .await,
        )
    }

    /// Launchpad 가 쓰는 모든 GitHub remote 캐시를 비운다(`repo_id` 가 없으면 전부). `gh:repo` 의 60초 추론 결과는 건드리지 않는다(TS `invalidateGhRemotes` 와 같다)
    pub fn invalidate_gh_remotes(&self, repo_id: Option<&str>) {
        let mut cache = self.gh_remotes_cache.lock().unwrap_or_else(PoisonError::into_inner);
        match repo_id {
            Some(id) => {
                cache.remove(id);
            }
            None => cache.clear(),
        }
    }

    /// 작업 트리 채널: `git:wipStatus(repoId, requestId?)`·`git:wipDiff(repoId, req, requestId?)`. 소유는 다른 git 채널과 같은 한 술어(`mirror.resolve`)다(github 소스는 미러가 사이드카로 보낸다 —
    /// TS 의 빈 결과 분기는 옮기지 않는다). `wipDiff` 의 요청 검증은 untracked 허용 목록을 보므로 이 라우터가 목록을 쥔 채로 하고(실패하면 git·레지스트리를 건드리지 않는다),
    /// 통과한 값만 `git::wip` 의 argv 빌더로 간다. 둘 다 requestId 로 취소할 수 있다
    async fn git_wip_channel(&self, channel: &str, args: &[Option<&Value>]) -> Routed {
        let Some(Some(Value::String(repo_id))) = args.first() else { return Routed::Sidecar };
        let target = match self.mirror.resolve(repo_id) {
            Resolved::Rust(t) => t,
            Resolved::Sidecar => {
                self.note(channel, "sidecar");
                return Routed::Sidecar;
            }
            Resolved::Invalid(message) => return Routed::Done(err_env(&message, "ERROR")),
        };
        let (job, request_id) = if channel == "git:wipStatus" {
            (WipJob::Status, 1)
        } else {
            let allow = self.untracked_allow.lock().unwrap_or_else(PoisonError::into_inner).get(repo_id.as_str()).filter(|e| e.target == target).map(|e| e.paths.clone());
            match WipDiffRequest::parse(args.get(1).copied().flatten(), allow.as_deref()) {
                Ok(req) => (WipJob::Diff(req), 2),
                Err(message) => return Routed::Done(err_env(message, "ERROR")),
            }
        };
        // 등록은 첫 await 전에 한다. 빈 문자열은 requestId 없음이다
        let reg = match args.get(request_id) {
            Some(Some(Value::String(id))) if is_request_id(id) => Some(self.cancels.register(id)),
            _ => None,
        };
        let token = reg.as_ref().map_or_else(CancellationToken::new, |r| r.token.clone());
        self.note(channel, "rust");
        self.rust_handled.fetch_add(1, Ordering::Relaxed);
        let result = self.wip_job(repo_id, &target, job, &token).await;
        drop(reg);
        Routed::Done(match result {
            Ok(data) => ok_env(Arc::try_unwrap(data).unwrap_or_else(|a| (*a).clone())),
            Err(e) => err_env(&e.to_string(), e.envelope_code()),
        })
    }

    /// 작업 트리 한 요청. 캐시는 없다(파일 저장만으로 바뀐다).
    /// - 상태(TS `RepoService.wipStatus`): 같은 저장소의 진행 중 요청에 합류(키 `wip\0<id>`, 슬롯과 무관) → 첫 페이지 문 → 큐 슬롯 안에서 가드 조회 + `git status`. 합친 요청의 취소는 기다리는 쪽이 모두 떠났을 때만 프로세스에 닿는다.
    ///   **성공했을 때만** untracked 허용 목록을 이 저장소의 보고 결과로 바꾼다(실패·취소는 이전 목록을 그대로 둔다 — TS 와 같다). 그 사이 저장소가 미러에서 빠졌거나 대상이 바뀌었으면 기록하지 않는다
    /// - diff(TS `RepoService.wipDiff`): 합치지 않고 큐 슬롯 하나, 취소는 이 요청의 프로세스(가드 조회와 diff 모두)에 바로 닿는다
    async fn wip_job(&self, repo_id: &str, target: &Target, job: WipJob, token: &CancellationToken) -> Result<Arc<Value>, ExecError> {
        let rt = self.runtime(target)?;
        let (exec, path) = (rt.exec.clone(), target.path().to_string());
        match job {
            WipJob::Diff(req) => {
                let t = token.clone();
                rt.queue.run(None, Some(token.clone()), move || async move { Ok(Arc::new(to_value(&wip_file_diff(&exec, &path, &req, &RunOpts::with_cancel(t)).await?)?)) }).await
            }
            WipJob::Status => {
                let (state, queue, allow, mirror) = (self.repo_state.clone(), rt.queue.clone(), self.untracked_allow.clone(), self.mirror.clone());
                let (repo, target) = (repo_id.to_string(), target.clone());
                let join_key = format!("wip\0{repo_id}");
                let run = self.joins.run(Some(&join_key), None, move || async move {
                    state.after_first_page(&repo).await;
                    queue
                        .run(None, None, move || async move {
                            let status = wip_status(&exec, &path, &RunOpts::default(), WIP_UNTRACKED_LIMIT).await?;
                            if matches!(mirror.resolve(&repo), Resolved::Rust(t) if t == target) {
                                let paths = Arc::new(status.untracked.iter().cloned().collect::<HashSet<String>>());
                                allow.lock().unwrap_or_else(PoisonError::into_inner).insert(repo, AllowEntry { target, paths });
                            }
                            Ok(Arc::new(to_value(&status)?))
                        })
                        .await
                });
                tokio::select! {
                    biased;
                    () = token.cancelled() => Err(ExecError::Aborted),
                    r = run => r,
                }
            }
        }
    }

    async fn history_job(&self, repo_id: &str, target: &Target, job: HistoryJob, token: &CancellationToken) -> Result<Arc<Value>, ExecError> {
        let rt = self.runtime(target)?;
        let (exec, path) = (rt.exec.clone(), target.path().to_string());
        match job {
            // 전체 이력을 훑을 수 있다 — 한 큐 슬롯 안에서 세 갈래가 돌고, 취소는 이 요청의 프로세스에 바로 닿는다(`withSignal`)
            HistoryJob::Search(req) => {
                let t = token.clone();
                rt.queue.run(None, Some(token.clone()), move || async move { search(&exec, &path, &req, &t).await.map(Arc::new) }).await
            }
            HistoryJob::Activity(req) => self.activity_job(repo_id, &rt, path, req, token).await,
        }
    }

    /// 커밋 활동. TS `RepoService.activity` 와 같은 순서다: 같은 키의 진행 중 요청에 합류(슬롯과 무관) → 첫 페이지 문 → **시그니처(매번 새로 계산)** → 캐시(8개) → 큐 슬롯 안에서 `log --format=%at` 스트림.
    /// 합친 요청의 취소는 기다리는 쪽이 모두 떠났을 때만 프로세스에 닿는다(기다리는 쪽이 모두 사라지면 future 가 버려져 git 이 정리된다). 각 호출자의 토큰은 그 호출자의 기다림만 끊는다.
    ///
    /// **기억한 시그니처(`known_signature`)를 먼저 쓴다**(TS `knownSigs.get(repoId) ?? sharedSignature`): 감시(PR 7b)가 확인마다 갱신하고 `logStale`·`fetchChanged`·재 `arm` 이 버린다.
    /// 기억이 없으면 `shared_signature`(진행 중인 계산에 합류, 제한 20초)로 얻는다. 감시하지 않는 저장소의 기억은 알림 없이 낡을 수 있다 — TS 도 같다
    async fn activity_job(&self, repo_id: &str, rt: &SourceRuntime, path: String, req: ActivityRequest, token: &CancellationToken) -> Result<Arc<Value>, ExecError> {
        let (state, cache, queue, exec) = (self.repo_state.clone(), self.activity_cache.clone(), rt.queue.clone(), rt.exec.clone());
        let repo = repo_id.to_string();
        let join_key = format!("activity\0{repo_id}\0{}", req.scope_key());
        let cache_key = format!("{repo_id}\0{}", req.scope_key());
        let run = self.joins.run(Some(&join_key), None, move || async move {
            state.after_first_page(&repo).await;
            let sig = match state.known_signature(&repo) {
                Some(known) => known,
                None => {
                    let (queue, exec, path) = (queue.clone(), exec.clone(), path.clone());
                    state.shared_signature(&repo, move |tok| queued_signature(queue, exec, path, tok), None).await.map_err(ExecError::from)?
                }
            };
            if let Some(hit) = cache.lock().unwrap_or_else(PoisonError::into_inner).get(&cache_key, &sig) {
                return Ok(hit);
            }
            let value = queue
                .run(None, None, move || async move {
                    let activity = commit_activity(&exec, &path, &req, &RunOpts::default(), now_secs()).await?;
                    Ok(Arc::new(to_value(&activity)?))
                })
                .await?;
            cache.lock().unwrap_or_else(PoisonError::into_inner).set(&cache_key, sig, value.clone());
            Ok(value)
        });
        tokio::select! {
            biased;
            () = token.cancelled() => Err(ExecError::Aborted),
            r = run => r,
        }
    }

    /// 불변 결과의 캐시 → 같은 키 요청 합치기 → 큐 슬롯 안에서 `task`. `commit` 과 같은 방식이다:
    /// 요청 토큰은 실행기에 넘기지 않고(합쳐진 다른 호출자의 프로세스까지 죽이게 된다) 이 호출자는 기다리기만 그만둔다
    async fn cached_shared<F, Fut>(
        &self,
        rt: &SourceRuntime,
        cache: &Arc<Mutex<LruCache<Arc<Value>>>>,
        key: String,
        token: &CancellationToken,
        task: F,
    ) -> Result<Arc<Value>, ExecError>
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: std::future::Future<Output = Result<(Value, u64), ExecError>> + Send + 'static,
    {
        if let Some(hit) = cache.lock().unwrap_or_else(PoisonError::into_inner).get(&key) {
            return Ok(hit);
        }
        let (cache, cache_key) = (cache.clone(), key.clone());
        let run = rt.queue.run(Some(&key), None, move || async move {
            let (value, cost) = task().await?;
            let value = Arc::new(value);
            cache.lock().unwrap_or_else(PoisonError::into_inner).set(&cache_key, value.clone(), cost);
            Ok(value)
        });
        tokio::select! {
            biased;
            () = token.cancelled() => Err(ExecError::Aborted),
            r = run => r,
        }
    }

    /// 소스의 실행기·큐([`Runtimes::get`])
    fn runtime(&self, target: &Target) -> Result<Arc<SourceRuntime>, ExecError> {
        self.runtimes.get(target)
    }

    /// 커밋 상세. TS `RepoService.commit` 과 같은 순서: 해시 기준 불변 캐시 → 같은 키 요청 합치기 → 큐 슬롯 안에서 git 세 번.
    /// 합친 요청의 취소는 기다리는 쪽이 모두 취소했을 때만 프로세스에 닿는다(기다리는 쪽이 모두 사라지면 큐가 프로세스를 정리한다)
    async fn commit(&self, repo_id: &str, target: &Target, hash: Option<&str>, token: &CancellationToken) -> Result<Arc<Value>, ExecError> {
        let hash = HexHash::parse(hash.unwrap_or(""))?;
        let key = format!("{repo_id}\0{}", hash.lower());
        if let Some(hit) = self.commit_cache.lock().unwrap_or_else(PoisonError::into_inner).get(&key) {
            return Ok(hit);
        }
        let rt = self.runtime(target)?;
        let (exec, path, cache, cache_key) = (rt.exec.clone(), target.path().to_string(), self.commit_cache.clone(), key.clone());
        let run = rt.queue.run(Some(&key), None, move || async move {
            let detail = commit_detail(&exec, &path, &hash).await?;
            let cost = estimate_commit_bytes(&detail);
            let value = Arc::new(serde_json::to_value(&detail).map_err(|e| ExecError::Other { message: e.to_string() })?);
            cache.lock().unwrap_or_else(PoisonError::into_inner).set(&cache_key, value.clone(), cost);
            Ok(value)
        });
        // 요청 토큰은 실행기에 넘기지 않는다 — 합쳐진 다른 호출자의 프로세스까지 죽이게 된다. 이 호출자는 기다리기만 그만둔다
        tokio::select! {
            biased;
            () = token.cancelled() => Err(ExecError::Aborted),
            r = run => r,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::log_service::NoteOp;
    use crate::mirror::MirrorRepo;

    fn mirror_of(repos: Vec<MirrorRepo>) -> RepoMirror {
        let m = RepoMirror::default();
        m.apply(1, repos);
        m
    }

    fn repo(id: &str, kind: &str, path: &str) -> MirrorRepo {
        MirrorRepo { id: id.into(), kind: kind.into(), path: path.into(), distro: None, ssh_host: None }
    }

    fn is_sidecar(r: &Routed) -> bool {
        matches!(r, Routed::Sidecar)
    }

    fn envelope(r: Routed) -> Value {
        match r {
            Routed::Done(v) => v,
            Routed::Sidecar => panic!("사이드카로 갔다"),
        }
    }

    #[test]
    fn 스위치는_0_만_끈다() {
        assert!(rust_git_enabled(None));
        assert!(rust_git_enabled(Some("1")));
        assert!(rust_git_enabled(Some("")));
        assert!(rust_git_enabled(Some("false")));
        assert!(!rust_git_enabled(Some("0")));
    }

    #[test]
    fn ssh_스위치는_0_과_git_0_이_끈다() {
        assert!(rust_ssh_enabled(None, None));
        assert!(rust_ssh_enabled(Some("1"), None));
        assert!(rust_ssh_enabled(Some(""), Some("1")));
        assert!(!rust_ssh_enabled(Some("0"), None));
        assert!(!rust_ssh_enabled(None, Some("0")));
        assert!(!rust_ssh_enabled(Some("1"), Some("0")));
    }

    fn ssh_repo(id: &str, host: &str) -> MirrorRepo {
        MirrorRepo { id: id.into(), kind: "ssh".into(), path: "/srv/r".into(), distro: None, ssh_host: Some(host.into()) }
    }

    #[tokio::test]
    async fn ssh_는_스위치와_풀이_둘_다_있어야_rust_로_간다() {
        let m = mirror_of(vec![ssh_repo("bad", "-x"), ssh_repo("ok", "devbox")]);
        let r = Router::new(true, false, m.clone());
        // 풀이 없다 — ssh 는 사이드카
        assert!(!r.ssh_active());
        assert!(is_sidecar(&r.route("git:commit", &json!(["bad", "abcd1234"])).await));
        r.set_ssh_pool(SshPool::for_home("C:\\nonexistent-antler-home"));
        assert!(r.ssh_active());
        // 호스트 검사에 걸리면 연결하지 않고 ERROR 다(같은 판정이 미러에서 한 번만 난다)
        let env = envelope(r.route("git:commit", &json!(["bad", "abcd1234"])).await);
        assert_eq!((env["ok"].clone(), env["error"].clone()), (json!(false), json!("잘못된 SSH 호스트입니다")));
        // 스위치가 꺼져 있으면(ANTLER_RUST_SSH=0) 풀을 주입해도 무시한다
        let off = Router::new(true, false, mirror_of(vec![ssh_repo("ok", "devbox")])).with_ssh(false);
        off.set_ssh_pool(SshPool::for_home("C:\\nonexistent-antler-home"));
        assert!(!off.ssh_active() && off.ssh_pool().is_none() && !off.ssh_enabled());
        assert!(is_sidecar(&off.route("git:commit", &json!(["ok", "abcd1234"])).await));
        // ANTLER_RUST_GIT=0(라우터 전체가 꺼짐)이면 함께 꺼진다
        let git_off = Router::new(false, false, mirror_of(vec![ssh_repo("ok", "devbox")]));
        git_off.set_ssh_pool(SshPool::for_home("C:\\nonexistent-antler-home"));
        assert!(!git_off.ssh_active() && !git_off.ssh_enabled());
        assert!(is_sidecar(&git_off.route("git:commit", &json!(["ok", "abcd1234"])).await));
        assert_eq!(off.rust_handled() + git_off.rust_handled(), 0);
    }

    #[test]
    fn 표식_객체만_undefined_로_되돌린다() {
        let raw = [
            json!("a"),
            json!({ UNDEFINED_ARG: true }),
            json!({ UNDEFINED_ARG: true, "x": 1 }),
            json!({ UNDEFINED_ARG: false }),
            json!(null),
            json!([{ UNDEFINED_ARG: true }]),
        ];
        let d = decode_args(&raw);
        assert_eq!(d[0], Some(&json!("a")));
        assert_eq!(d[1], None);
        assert_eq!(d[2], Some(&raw[2]));
        assert_eq!(d[3], Some(&raw[3]));
        assert_eq!(d[4], Some(&Value::Null));
        assert_eq!(d[5], Some(&raw[5]));
    }

    #[tokio::test]
    async fn 사이드카로_가는_경우() {
        let m = mirror_of(vec![repo("a", "local", "/nonexistent-antler"), repo("s", "ssh", "/srv"), repo("g", "github", "/cache")]);
        let r = Router::new(true, false, m.clone());
        // 채널이 등록되지 않았다(PR 6·7 까지 사이드카에 남는 git 채널은 피하고 6b 까지 남는 비 git 채널을 쓴다)
        assert!(!RUST_CHANNELS.contains(&"config:get"));
        assert!(is_sidecar(&r.route("config:get", &json!(["a"])).await));
        // 미러에 없는 저장소·ssh·github
        for id in ["nope", "s", "g"] {
            assert!(is_sidecar(&r.route("git:commit", &json!([id, "abcd"])).await), "{id}");
        }
        // 저장소 id 가 문자열이 아니거나 없다
        for args in [json!([]), json!([null]), json!([5, "abcd"]), json!([{ UNDEFINED_ARG: true }, "abcd"]), json!({ "a": 1 }), json!("x")] {
            assert!(is_sidecar(&r.route("git:commit", &args).await), "{args}");
        }
        // 스위치를 끄면 미러에 있어도 사이드카
        let off = Router::new(false, false, m);
        assert!(is_sidecar(&off.route("git:commit", &json!(["a", "abcd"])).await));
        assert_eq!(off.rust_handled() + r.rust_handled(), 0);
    }

    /// 코드 탐색기 채널의 소유 판정은 다른 git 채널과 같은 한 술어(`mirror.resolve` — 미러에 있고 local·wsl·(켜진) ssh 인 저장소)와 스위치뿐이다.
    /// 사이드카는 이 채널을 따로 소유하지 않는다(소유 env 가 없다): 미러가 사이드카로 푸는 저장소(없는 id·github·풀 없는 ssh)와 스위치가 꺼진 라우터만 사이드카로 간다
    #[tokio::test]
    async fn 코드_탐색기_채널의_소유는_다른_git_채널과_같은_한_술어다() {
        let rev = "a".repeat(40);
        let m = mirror_of(vec![repo("a", "local", "/nonexistent-antler"), repo("s", "ssh", "/srv"), repo("g", "github", "/cache"), repo("unc", "local", "\\\\evil\\share\\repo")]);
        let r = Router::new(true, false, m.clone());
        let off = Router::new(false, false, m);
        let calls = |id: &str| {
            [
                ("git:resolveRev", json!([id, "HEAD"])),
                ("git:tree", json!([id, { "rev": rev, "dir": "" }])),
                ("git:blob", json!([id, { "rev": rev, "path": "a" }])),
                ("git:onRemote", json!([id, rev])),
            ]
        };
        for ch in ["git:resolveRev", "git:tree", "git:blob", "git:onRemote"] {
            assert!(RUST_CHANNELS.contains(&ch), "{ch}");
        }
        for id in ["nope", "s", "g"] {
            for (ch, args) in calls(id) {
                assert!(is_sidecar(&r.route(ch, &args).await), "{id} {ch}");
            }
        }
        for (ch, args) in calls("a") {
            assert!(is_sidecar(&off.route(ch, &args).await), "{ch}");
        }
        // 미러 검사에 걸린 저장소는 사이드카가 아니라 ERROR 다(다른 채널과 같다)
        for (ch, args) in calls("unc") {
            let env = envelope(r.route(ch, &args).await);
            assert_eq!((env["ok"].clone(), env["code"].clone()), (json!(false), json!("ERROR")), "{ch}");
        }
        assert_eq!(r.rust_handled() + off.rust_handled(), 0);
    }

    /// 사이드바 ref 채널(`git:refs`·`git:refsDetails`)의 소유도 같은 한 술어다 — 인자는 저장소 id 뿐이다
    #[tokio::test]
    async fn ref_채널의_소유는_다른_git_채널과_같은_한_술어다() {
        let m = mirror_of(vec![repo("a", "local", "/nonexistent-antler"), repo("s", "ssh", "/srv"), repo("g", "github", "/cache"), repo("unc", "local", "\\\\evil\\share\\repo")]);
        let r = Router::new(true, false, m.clone());
        let off = Router::new(false, false, m);
        for ch in ["git:refs", "git:refsDetails"] {
            assert!(RUST_CHANNELS.contains(&ch), "{ch}");
            for id in ["nope", "s", "g"] {
                assert!(is_sidecar(&r.route(ch, &json!([id])).await), "{id} {ch}");
            }
            assert!(is_sidecar(&off.route(ch, &json!(["a"])).await), "{ch}");
            let env = envelope(r.route(ch, &json!(["unc"])).await);
            assert_eq!((env["ok"].clone(), env["code"].clone()), (json!(false), json!("ERROR")), "{ch}");
        }
        assert_eq!(r.rust_handled() + off.rust_handled(), 0);
    }

    /// 검색·활동 채널의 소유도 같은 한 술어다 — 새로 만든 소유 판정이 없다(사이드카에는 이 채널의 소유 env 가 없다)
    #[tokio::test]
    async fn 검색_활동_채널의_소유는_다른_git_채널과_같은_한_술어다() {
        let m = mirror_of(vec![repo("a", "local", "/nonexistent-antler"), repo("s", "ssh", "/srv"), repo("g", "github", "/cache"), repo("unc", "local", "\\\\evil\\share\\repo")]);
        let r = Router::new(true, false, m.clone());
        let off = Router::new(false, false, m);
        for (ch, tail) in [("git:search", json!(["q"])), ("git:activity", json!([]))] {
            assert!(RUST_CHANNELS.contains(&ch), "{ch}");
            let call = |id: &str| Value::Array(std::iter::once(json!(id)).chain(tail.as_array().unwrap().iter().cloned()).collect());
            for id in ["nope", "s", "g"] {
                assert!(is_sidecar(&r.route(ch, &call(id)).await), "{id} {ch}");
            }
            assert!(is_sidecar(&off.route(ch, &call("a")).await), "{ch}");
            let env = envelope(r.route(ch, &call("unc")).await);
            assert_eq!((env["ok"].clone(), env["code"].clone()), (json!(false), json!("ERROR")), "{ch}");
        }
        assert_eq!(r.rust_handled() + off.rust_handled(), 0);
    }

    /// LFS·gitflow 채널의 소유도 같은 한 술어다 — 사이드카에는 이 채널의 소유 env 가 없다
    #[tokio::test]
    async fn lfs_채널의_소유는_다른_git_채널과_같은_한_술어다() {
        let m = mirror_of(vec![repo("a", "local", "/nonexistent-antler"), repo("s", "ssh", "/srv"), repo("g", "github", "/cache"), repo("unc", "local", "\\\\evil\\share\\repo")]);
        let r = Router::new(true, false, m.clone());
        let off = Router::new(false, false, m);
        for (ch, tail) in [("git:repoExtras", json!([])), ("git:lfsPaths", json!([{ "paths": ["a"] }]))] {
            assert!(RUST_CHANNELS.contains(&ch), "{ch}");
            let call = |id: &str| Value::Array(std::iter::once(json!(id)).chain(tail.as_array().unwrap().iter().cloned()).collect());
            for id in ["nope", "s", "g"] {
                assert!(is_sidecar(&r.route(ch, &call(id)).await), "{id} {ch}");
            }
            assert!(is_sidecar(&off.route(ch, &call("a")).await), "{ch}");
            let env = envelope(r.route(ch, &call("unc")).await);
            assert_eq!((env["ok"].clone(), env["code"].clone()), (json!(false), json!("ERROR")), "{ch}");
        }
        assert_eq!(r.rust_handled() + off.rust_handled(), 0);
    }

    /// 작업 트리 채널의 소유도 같은 한 술어다 — 사이드카에는 이 채널의 소유 env 가 없다
    #[tokio::test]
    async fn 작업_트리_채널의_소유는_다른_git_채널과_같은_한_술어다() {
        let m = mirror_of(vec![repo("a", "local", "/nonexistent-antler"), repo("s", "ssh", "/srv"), repo("g", "github", "/cache"), repo("unc", "local", "\\\\evil\\share\\repo")]);
        let r = Router::new(true, false, m.clone());
        let off = Router::new(false, false, m);
        for (ch, tail) in [("git:wipStatus", json!([])), ("git:wipDiff", json!([{ "area": "unstaged", "path": "a.txt" }]))] {
            assert!(RUST_CHANNELS.contains(&ch), "{ch}");
            let call = |id: &str| Value::Array(std::iter::once(json!(id)).chain(tail.as_array().unwrap().iter().cloned()).collect());
            for id in ["nope", "s", "g"] {
                assert!(is_sidecar(&r.route(ch, &call(id)).await), "{id} {ch}");
            }
            assert!(is_sidecar(&off.route(ch, &call("a")).await), "{ch}");
            let env = envelope(r.route(ch, &call("unc")).await);
            assert_eq!((env["ok"].clone(), env["code"].clone()), (json!(false), json!("ERROR")), "{ch}");
        }
        assert_eq!(r.rust_handled() + off.rust_handled(), 0);
    }

    #[tokio::test]
    async fn 미러의_경로가_검사에_걸리면_error_로_돌려준다() {
        let m = mirror_of(vec![repo("unc", "local", "\\\\evil\\share\\repo")]);
        let r = Router::new(true, false, m);
        let env = envelope(r.route("git:commit", &json!(["unc", "abcd1234"])).await);
        assert_eq!(env["ok"], false);
        assert_eq!(env["code"], "ERROR");
    }

    #[tokio::test]
    async fn 해시가_잘못되면_git_을_띄우기_전에_error() {
        // 저장소 경로가 없는 곳이다 — 검증이 먼저 걸러야 한다(git 을 띄웠다면 GIT 이나 다른 오류가 난다)
        let m = mirror_of(vec![repo("a", "local", "/nonexistent-antler")]);
        let r = Router::new(true, false, m);
        let hostile: Vec<Value> = vec![
            json!("-x"),
            json!("--output=pwned.txt"),
            json!("-calias.x=!echo"),
            json!("zzzz"),
            json!("abc"),
            json!("abcd\n"),
            json!(" abcd"),
            json!(123),
            json!(null),
            json!({ "x": 1 }),
            json!({ UNDEFINED_ARG: true }),
        ];
        for h in hostile {
            let env = envelope(r.route("git:commit", &json!(["a", h, "req-1"])).await);
            assert_eq!(env["ok"], false, "{h}");
            assert_eq!(env["code"], "ERROR", "{h}");
            assert_eq!(env["error"], "invalid commit hash", "{h}");
        }
        // 해시를 아예 안 보냈다
        assert_eq!(envelope(r.route("git:commit", &json!(["a"])).await)["code"], "ERROR");
    }

    #[tokio::test]
    async fn req_cancel_은_항상_사이드카에도_간다() {
        let r = Router::new(true, false, RepoMirror::default());
        assert!(is_sidecar(&r.route("req:cancel", &json!(["x"])).await));
        assert!(is_sidecar(&r.route("req:cancel", &json!([])).await));
        assert!(is_sidecar(&r.route("req:cancel", &json!([5])).await));
        // 스위치가 꺼져 있어도 마찬가지
        let off = Router::new(false, false, RepoMirror::default());
        assert!(is_sidecar(&off.route("req:cancel", &json!(["x"])).await));
    }

    #[tokio::test]
    async fn 등록_전에_온_cancel_은_등록한_요청을_취소한다() {
        let m = mirror_of(vec![repo("a", "local", "/nonexistent-antler")]);
        let r = Router::new(true, false, m);
        assert!(is_sidecar(&r.route("req:cancel", &json!(["early"])).await));
        // git 을 띄우지 않고 ABORTED — 경로가 없어서 띄웠다면 GIT/ERROR 가 난다
        let env = envelope(r.route("git:commit", &json!(["a", "abcd1234", "early"])).await);
        assert_eq!(env["code"], "ABORTED");
        // 쓴 tombstone 은 사라지고 등록도 끝나며 지워진다(이 테스트는 git 을 띄우지 않는다)
        assert_eq!(r.cancels.counts(), (0, 0));
    }

    #[tokio::test]
    async fn 등록_전에_온_cancel_은_로그_요청도_git_없이_취소하고_첫_페이지를_알린다() {
        let m = mirror_of(vec![repo("a", "local", "/nonexistent-antler")]);
        let r = Router::new(true, false, m);
        let notes: Arc<Mutex<Vec<(String, String)>>> = Arc::default();
        let sink = notes.clone();
        r.set_note_sink(Arc::new(move |op, id| sink.lock().unwrap().push((op.as_str().to_string(), id.to_string()))));
        assert!(is_sidecar(&r.route("req:cancel", &json!(["early"])).await));
        // skip 0 — 취소돼도 백그라운드 조회를 풀어 준다(TS 는 실패해도 openFirstPage). git·커서는 만들지 않는다
        let env = envelope(r.route("git:log", &json!(["a", { "skip": 0, "count": 10 }, "early"])).await);
        assert_eq!(env["code"], "ABORTED");
        assert_eq!(r.log_cursor_count(), 0);
        assert_eq!(*notes.lock().unwrap(), [("logFirstPage".to_string(), "a".to_string())]);
        // skip > 0 이면 알리지 않는다
        assert!(is_sidecar(&r.route("req:cancel", &json!(["early2"])).await));
        let env = envelope(r.route("git:log", &json!(["a", { "skip": 5, "count": 10 }, "early2"])).await);
        assert_eq!(env["code"], "ABORTED");
        assert_eq!(notes.lock().unwrap().len(), 1);
        // 검증에서 거부되면 알리지 않는다(TS 는 검증 오류를 첫 페이지 처리 전에 던진다)
        let env = envelope(r.route("git:log", &json!(["a", { "skip": 0, "count": "10" }])).await);
        assert_eq!(env["error"], "invalid query");
        assert_eq!(notes.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn repos_watch_는_rust_가_푸는_저장소만_arm_하고_응답은_사이드카가_한다() {
        let m = mirror_of(vec![repo("a", "local", "/nonexistent-antler"), repo("g", "github", "x/y"), repo("s", "ssh", "/srv")]);
        let r = Router::new(true, false, m.clone());
        assert!(is_sidecar(&r.route("repos:watch", &json!(["a"])).await));
        assert!(r.repo_state().is_armed("a"));
        // github·풀 없는 ssh·미러에 없는 id·null 은 arm 하지 않는다
        for args in [json!(["g"]), json!(["s"]), json!(["nope"]), json!([null]), json!([])] {
            assert!(is_sidecar(&r.route("repos:watch", &args).await), "{args}");
        }
        for id in ["g", "s", "nope"] {
            assert!(!r.repo_state().is_armed(id), "{id}");
        }
        // 스위치가 꺼져 있으면 arm 하지 않는다
        let off = Router::new(false, false, m);
        assert!(is_sidecar(&off.route("repos:watch", &json!(["a"])).await));
        assert!(!off.repo_state().is_armed("a"));
    }

    #[tokio::test]
    async fn repos_watch_가_다시_arm_하면_기억한_시그니처를_버린다() {
        use crate::git::repo_state::SigError;
        let r = Router::new(true, false, mirror_of(vec![repo("a", "local", "/nonexistent-antler")]));
        let sig = crate::git::signature::Signature::from_parts(b"x", b"", b"");
        r.repo_state().compute_signature("a", async move { Ok::<_, SigError>(sig) }).await.unwrap();
        assert_eq!(r.repo_state().known_signature("a"), Some(sig));
        assert!(is_sidecar(&r.route("repos:watch", &json!(["a"])).await));
        assert_eq!(r.repo_state().known_signature("a"), None);
    }

    #[tokio::test]
    async fn 알림은_상태에_먼저_반영한_뒤_사이드카_싱크로_간다() {
        let m = mirror_of(vec![repo("a", "local", "/nonexistent-antler"), repo("g", "github", "x/y")]);
        let r = Router::new(true, false, m);
        let state = r.repo_state().clone();
        // (op, 저장소, 문이 닫혀 있는가, 시그니처를 기억하는가)
        type Seen = (String, String, bool, bool);
        let seen: Arc<Mutex<Vec<Seen>>> = Arc::default();
        let (log, probe) = (seen.clone(), state.clone());
        r.set_note_sink(Arc::new(move |op, id| {
            // 싱크가 불린 시점에 상태가 이미 반영됐는지: (문이 닫혀 있는가, 시그니처를 기억하는가)
            log.lock().unwrap().push((op.as_str().to_string(), id.to_string(), probe.is_armed(id), probe.known_signature(id).is_some()));
        }));
        let sig = crate::git::signature::Signature::from_parts(b"x", b"", b"");
        let notes = r.note_sink();
        for id in ["a", "g"] {
            state.arm(id);
            state.compute_signature(id, async move { Ok(sig) }).await.unwrap();
        }
        // fetch 변경: 시그니처를 버린 뒤에 사이드카로 간다
        notes(NoteOp::FetchChanged, "a");
        // 첫 페이지: 문을 연 뒤에 사이드카로 간다
        notes(NoteOp::FirstPage, "a");
        // 미러에서 Rust 가 풀지 않는 저장소는 상태를 만지지 않지만 사이드카에는 그대로 간다
        notes(NoteOp::Stale, "g");
        notes(NoteOp::FirstPage, "g");
        assert_eq!(
            *seen.lock().unwrap(),
            [
                ("fetchChanged".to_string(), "a".to_string(), true, false),
                ("logFirstPage".to_string(), "a".to_string(), false, false),
                ("logStale".to_string(), "g".to_string(), true, true),
                ("logFirstPage".to_string(), "g".to_string(), true, true),
            ]
        );
    }

    #[tokio::test]
    async fn 싱크를_달지_않아도_알림은_상태에_반영되고_나중에_단_싱크도_받는다() {
        let r = Router::new(true, false, mirror_of(vec![repo("a", "local", "/nonexistent-antler")]));
        let notes = r.note_sink();
        r.repo_state().arm("a");
        notes(NoteOp::FirstPage, "a");
        assert!(!r.repo_state().is_armed("a"));
        let seen: Arc<Mutex<Vec<String>>> = Arc::default();
        let log = seen.clone();
        r.set_note_sink(Arc::new(move |op, _| log.lock().unwrap().push(op.as_str().to_string())));
        // 먼저 만든 팬아웃도 호출 시점에 싱크를 읽는다
        notes(NoteOp::Stale, "a");
        assert_eq!(*seen.lock().unwrap(), ["logStale".to_string()]);
    }

    #[tokio::test]
    async fn 로그_타이밍을_바꿔도_로그의_첫_페이지_알림이_상태를_연다() {
        let r = Router::new(true, false, mirror_of(vec![repo("a", "local", "/nonexistent-antler")])).with_log_timing(Duration::from_secs(60), Duration::from_secs(60));
        let state = r.repo_state().clone();
        let seen: Arc<Mutex<Vec<bool>>> = Arc::default();
        let (log, probe) = (seen.clone(), state.clone());
        r.set_note_sink(Arc::new(move |_, id| log.lock().unwrap().push(probe.is_armed(id))));
        assert!(is_sidecar(&r.route("repos:watch", &json!(["a"])).await));
        assert!(state.is_armed("a"));
        assert!(is_sidecar(&r.route("req:cancel", &json!(["early"])).await));
        let env = envelope(r.route("git:log", &json!(["a", { "skip": 0, "count": 10 }, "early"])).await);
        assert_eq!(env["code"], "ABORTED");
        // 로그가 낸 logFirstPage 가 문을 열었고(싱크 시점에 이미 열려 있다) 사이드카에도 갔다
        assert_eq!(*seen.lock().unwrap(), [false]);
        assert!(!state.is_armed("a"));
    }

    #[tokio::test]
    async fn 종료하면_기다리던_첫_페이지_문이_풀린다() {
        let r = Router::new(true, false, mirror_of(vec![repo("a", "local", "/nonexistent-antler")]));
        assert!(is_sidecar(&r.route("repos:watch", &json!(["a"])).await));
        assert!(r.repo_state().is_armed("a"));
        r.shutdown();
        assert!(!r.repo_state().is_armed("a"));
    }

    #[tokio::test]
    async fn log_채널도_미러에_없거나_스위치가_꺼지면_사이드카() {
        let m = mirror_of(vec![repo("a", "local", "/nonexistent-antler"), repo("s", "ssh", "/srv")]);
        let r = Router::new(true, false, m.clone());
        for id in ["nope", "s"] {
            assert!(is_sidecar(&r.route("git:log", &json!([id, { "skip": 0, "count": 1 }])).await), "{id}");
        }
        for args in [json!([]), json!([null]), json!([5, { "skip": 0, "count": 1 }])] {
            assert!(is_sidecar(&r.route("git:log", &args).await), "{args}");
        }
        let off = Router::new(false, false, m);
        assert!(is_sidecar(&off.route("git:log", &json!(["a", { "skip": 0, "count": 1 }])).await));
        assert_eq!(r.rust_handled() + off.rust_handled(), 0);
    }

    #[test]
    fn 레지스트리는_등록한_것만_취소하고_끝나면_지운다() {
        let reg: Arc<CancelRegistry> = Arc::default();
        let a = reg.register("a");
        assert!(!a.token.is_cancelled());
        reg.cancel("a");
        assert!(a.token.is_cancelled());
        assert_eq!(reg.counts(), (1, 0));
        drop(a);
        assert_eq!(reg.counts(), (0, 0));
        // 같은 id 를 다시 등록하면 새 것이 이기고, 먼저 등록한 것의 Drop 이 새 것을 지우지 않는다
        let first = reg.register("x");
        let second = reg.register("x");
        drop(first);
        reg.cancel("x");
        assert!(second.token.is_cancelled());
    }

    #[test]
    fn tombstone_은_10초_뒤_사라지고_256개를_넘지_못한다() {
        let reg: Arc<CancelRegistry> = Arc::default();
        let t0 = Instant::now();
        reg.cancel_at("old", t0);
        // 11초 뒤 등록하면 이미 만료돼 취소되지 않는다
        let r = reg.register_at("old", t0 + Duration::from_secs(11));
        assert!(!r.token.is_cancelled());
        // 10초 안이면 취소된다
        reg.cancel_at("fresh", t0);
        let r = reg.register_at("fresh", t0 + Duration::from_secs(9));
        assert!(r.token.is_cancelled());
        // 개수 상한: 가장 오래된 것부터 버린다
        for i in 0..(TOMBSTONE_MAX + 5) {
            reg.cancel_at(&format!("id{i}"), t0);
        }
        assert_eq!(reg.counts().1, TOMBSTONE_MAX);
        assert!(!reg.register_at("id0", t0).token.is_cancelled());
        assert!(reg.register_at(&format!("id{}", TOMBSTONE_MAX + 4), t0).token.is_cancelled());
    }
}
