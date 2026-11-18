//! GitHub 서비스가 저장소 정보를 묻는 길의 **소유 분기**(Phase 6a PR 8). [`RepoQuery`](crate::github::ports::RepoQuery) 의 실제 구현이다.
//!
//! - Rust 가 git 을 맡는 저장소(local·WSL·SSH, 그리고 GitHub 서비스와 캐시 실행기가 주입된 github kind — 라우터의 한 술어 `RepoMirror::resolve`)의 `gh_repo`·`gh_remotes`·`pushed_shas_by_author` 는 **Rust 의 라우터가 직접** 답한다.
//!   사이드카는 그 저장소에 git 을 돌리지 않는다(SSH 호스트당 연결이 앱 하나다). 실행기·큐·캐시는 채널 핸들러(`gh:repo`)가 쓰는 라우터의 것 그대로다.
//! - 그 밖(캐시 실행기가 없는 github kind·미러에 없는 id·`ANTLER_RUST_GIT=0`)은 사이드카에 묻는다([`SidecarRepos`](crate::repoq::SidecarRepos) 위임).
//! - 미러가 **거부한** 저장소(경로·호스트·배포판 검사에 걸림)는 위임하지 않는다 — 오류다. 위임하면 미러가 막은 경로에 사이드카가 git 을 돌린다.
//! - `github_client_id`·`repo_ids`(설정 읽기)는 항상 사이드카다. `invalidate_gh_remotes` 는 Rust 캐시를 비우고 사이드카에도 알린다.
//!
//! 라우터는 앱이 `app.manage` 로 가져가므로 이 구현은 라우터를 호출 때 찾는다([`RouterSource`]). 서비스는 라우터보다 먼저 만들어진다.
//! 질의마다 사이드카 질의와 같은 제한 시간([`QUERY_TIMEOUT`])을 건다(멈춘 원격이 `gh:*` 를 붙잡지 않게).

use std::ops::Deref;
use std::sync::Arc;
use std::time::Duration;

use futures_util::future::{BoxFuture, FutureExt};
use tokio_util::sync::CancellationToken;

use crate::github::ports::{GhRepoRef, RepoQuery};
use crate::repoq::QUERY_TIMEOUT;
use crate::router::Router;

/// 라우터를 찾는 곳. 앱은 `AppHandle` 의 관리 상태에서, 시험은 가진 라우터에서 찾는다
pub trait RouterSource: Send + Sync + 'static {
    type Guard<'a>: Deref<Target = Router> + Send
    where
        Self: 'a;
    /// 라우터가 아직 없으면(앱 시작 중) `None` — 그동안은 전부 사이드카에 묻는다
    fn router(&self) -> Option<Self::Guard<'_>>;
}

impl RouterSource for Arc<Router> {
    type Guard<'a> = &'a Router;
    fn router(&self) -> Option<&Router> {
        Some(self.as_ref())
    }
}

/// 앱의 라우터(`app.manage(router)`)를 호출 때 찾는다
pub struct AppRouter(pub tauri::AppHandle);

impl RouterSource for AppRouter {
    type Guard<'a> = tauri::State<'a, Router>;
    fn router(&self) -> Option<tauri::State<'_, Router>> {
        use tauri::Manager;
        self.0.try_state::<Router>()
    }
}

const TIMED_OUT: &str = "git 이 제때 응답하지 않았습니다";

pub struct RoutedRepos<S: RouterSource> {
    source: S,
    fallback: Arc<dyn RepoQuery>,
    timeout: Duration,
}

impl<S: RouterSource> RoutedRepos<S> {
    pub fn new(source: S, fallback: Arc<dyn RepoQuery>) -> Self {
        Self { source, fallback, timeout: QUERY_TIMEOUT }
    }

    /// 제한 시간을 바꾼다(시험은 멈춘 시계로 짧게 건다)
    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Rust 의 답(`Some`)에 제한 시간을 건다. 시간이 지나면 이 future 를 버려 대기 중인 git 이 정리된다
    async fn bounded<T>(&self, answer: impl std::future::Future<Output = Option<Result<T, crate::git::exec::ExecError>>>) -> Option<Result<T, String>> {
        match tokio::time::timeout(self.timeout, answer).await {
            Ok(found) => found.map(|r| r.map_err(|e| e.to_string())),
            Err(_) => Some(Err(TIMED_OUT.to_string())),
        }
    }
}

impl<S: RouterSource> RepoQuery for RoutedRepos<S> {
    fn gh_repo<'a>(&'a self, repo_id: &'a str) -> BoxFuture<'a, Result<Option<GhRepoRef>, String>> {
        async move {
            if let Some(router) = self.source.router() {
                if let Some(answer) = self.bounded(router.gh_repo(repo_id)).await {
                    return answer;
                }
            }
            self.fallback.gh_repo(repo_id).await
        }
        .boxed()
    }

    fn gh_remotes<'a>(&'a self, repo_id: &'a str, cancel: CancellationToken) -> BoxFuture<'a, Result<Vec<GhRepoRef>, String>> {
        async move {
            if let Some(router) = self.source.router() {
                if let Some(answer) = self.bounded(router.gh_remotes(repo_id, &cancel)).await {
                    return answer;
                }
            }
            self.fallback.gh_remotes(repo_id, cancel).await
        }
        .boxed()
    }

    fn pushed_shas_by_author<'a>(&'a self, repo_id: &'a str, email: &'a str, limit: usize, cancel: CancellationToken) -> BoxFuture<'a, Result<Vec<String>, String>> {
        async move {
            if let Some(router) = self.source.router() {
                if let Some(answer) = self.bounded(router.pushed_shas_by_author(repo_id, email, limit, &cancel)).await {
                    return answer;
                }
            }
            self.fallback.pushed_shas_by_author(repo_id, email, limit, cancel).await
        }
        .boxed()
    }

    fn invalidate_gh_remotes(&self) {
        if let Some(router) = self.source.router() {
            router.invalidate_gh_remotes(None);
        }
        self.fallback.invalidate_gh_remotes();
    }

    fn github_client_id(&self) -> BoxFuture<'_, Option<String>> {
        self.fallback.github_client_id()
    }

    fn repo_ids(&self) -> BoxFuture<'_, Vec<String>> {
        self.fallback.repo_ids()
    }
}
