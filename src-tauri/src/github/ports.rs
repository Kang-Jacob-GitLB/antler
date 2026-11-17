//! GitHub 서비스가 밖에 묻는 것들의 포트(트레이트). 이 PR 은 트레이트만 두고 실제 구현은 뒤 PR 이 맞춘다:
//! - [`RepoQuery`]: 사이드카가 쥔 저장소 정보(저장소 → owner/repo, remote 목록, push 된 sha, `githubClientId`) — PR 9 가 `repoq` 질의로 구현한다. 이 PR 의 시험은 가짜로 채운다.
//! - [`ExternalTokenProvider`]: GitHub CLI(gh) 같은 밖의 토큰 출처 — PR 8 이 구현한다.
use futures_util::future::BoxFuture;
use tokio_util::sync::CancellationToken;

use crate::net::redact::SecretString;

/// `GhRepoRef`: 저장소의 GitHub remote 하나
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GhRepoRef {
    pub owner: String,
    pub repo: String,
    /// 추론에 쓴 remote 이름(origin 우선)
    pub remote: String,
}

/// 사이드카에 묻는 저장소 정보. 오류는 사용자에게 보여 줄 수 있는 문장이다(서비스가 그대로 `ERROR` 로 돌려준다)
pub trait RepoQuery: Send + Sync {
    /// `gh:repo`: 저장소의 GitHub remote(없으면 `None`)
    fn gh_repo<'a>(&'a self, repo_id: &'a str) -> BoxFuture<'a, Result<Option<GhRepoRef>, String>>;
    /// Launchpad: 저장소의 모든 GitHub remote. `cancel` 이 끊기면 읽기를 멈춘다
    fn gh_remotes<'a>(&'a self, repo_id: &'a str, cancel: CancellationToken) -> BoxFuture<'a, Result<Vec<GhRepoRef>, String>>;
    /// 아바타 404 대비: 그 작성자의 push 된 커밋 sha(최대 `limit`개)
    fn pushed_shas_by_author<'a>(&'a self, repo_id: &'a str, email: &'a str, limit: usize, cancel: CancellationToken) -> BoxFuture<'a, Result<Vec<String>, String>>;
    /// Launchpad 새로고침: remote 목록 캐시를 버린다
    fn invalidate_gh_remotes(&self);
    /// 설정의 `ui.githubClientId`(없으면 `None`)
    fn github_client_id(&self) -> BoxFuture<'_, Option<String>>;
    /// 앱에 등록된 저장소 ID 목록(Launchpad 가 훑는다)
    fn repo_ids(&self) -> BoxFuture<'_, Vec<String>>;
}

/// 외부 토큰 출처가 실패했다. `message` 는 사용자에게 그대로 보여 줘도 되는 한국어 문장이다
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExternalTokenError {
    pub message: String,
}

/// GitHub CLI(gh) 같은 밖의 토큰 출처(`GhCli`). 토큰은 서비스에게만 돌려주고 로그·오류 문구에 싣지 않는다
pub trait ExternalTokenProvider: Send + Sync {
    /// 쓸 수 있는지(결과는 호출자가 한 번만 묻는다)
    fn available(&self) -> BoxFuture<'_, bool>;
    /// github.com 에 로그인된 쪽의 토큰
    fn token(&self) -> BoxFuture<'_, Result<SecretString, ExternalTokenError>>;
}
