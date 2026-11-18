//! fetch 서비스의 GitHub 원격 전용 저장소(github kind) 갈래(`FetchService.ts` 의 `runGithub`·`fetchIfEmpty`·`resumeGithubAuth` 이식, Phase 6b PR 7).
//!
//! 일반 저장소와 **다른 보안 모델**이다 — 허용 목록(`inspect_config`)·확인 창·자격 증명 도우미·승인이 없고, 대신:
//! - 설정이 앱이 만든 것과 **정확히 일치**해야만 받는다(`inspect_github_repo_at` — `include.path` 등 다른 키는 전부 차단, remote 는 `origin` 하나). 어긋나면 확인 없이 차단한다
//! - 레코드의 링크(owner/repo)와 캐시 경로가 맞지 않으면 일반 저장소 흐름으로 가지 않고 차단한다
//! - fetch 는 닫힌 진입점 `run_github_fetch` 하나다(`--filter=blob:none`·`http.followRedirects=false`·remote `origin`·고정 refspec·`--refmap=`). 환경(토큰)은 실행기(`GitExecutor::Github`)가 붙인다
//!
//! 이 파일이 `inspect_config`·`run_fetch`·`send_approval` 을 부르지 않고, 일반 갈래(`service.rs`)가 `run_github_fetch`·`inspect_github_repo_at` 를 부르지 않음은
//! `tests/github_fetch.rs` 가 소스로 강제한다.

use std::sync::Arc;
use std::time::Duration;

use tokio_util::sync::CancellationToken;

use super::{lock, Blocked, FetchService, Inner, RunFlags, Source, FETCH_TIMEOUT, GITHUB_FIRST_FETCH_TIMEOUT};
use crate::git::exec::ExecError;
use crate::git::fetch::run::{github_net_args, run_github_fetch, GitRunner, GithubFetchOpts};
use crate::git::github_cache::{ensure_github_head, has_no_refs, inspect_github_repo_at, GithubCachePath};
use crate::git::github_link::{cache_path_matches_link, GithubLink};
use crate::git::signature::refs_signature;
use crate::mirror::{Resolved, Target};

/// github 갈래가 서비스에 더하는 설정(`FetchService::with_github_cache` 가 정한다)
pub(super) struct GithubFetch {
    /// 앱 캐시 기준 폴더(`<userData>/remote-repos`) — 레코드의 캐시 경로가 이 아래의 `<owner>/<repo>.git` 일 때만 받는다
    cache_dir: String,
    /// 원격 URL 의 출처. 프로덕션은 `None`(= `https://github.com` 고정)이고, 시험만 `with_github_origin_for_test` 로 정한다
    origin: Option<String>,
}

impl<H: super::FetchHost, R: GitRunner + Send + 'static> FetchService<H, R> {
    /// GitHub 원격 전용 저장소의 fetch 를 켠다: 앱 캐시 기준 폴더를 정한다(만든 직후에만 부른다). 부르지 않으면 github 저장소는 "캐시 경로가 맞지 않음"으로 막힌다
    #[must_use]
    pub fn with_github_cache(mut self, cache_dir: impl Into<String>) -> Self {
        Arc::get_mut(&mut self.inner).expect("서비스를 만든 직후에만 부른다").github = Some(GithubFetch { cache_dir: cache_dir.into(), origin: None });
        self
    }

    /// 원격 URL 의 출처를 바꾼다(`file:///…`·`http://127.0.0.1:…`). **테스트 전용** — 프로덕션 검사는 `https://github.com` 으로 고정이다
    #[cfg(feature = "test-hooks")]
    #[doc(hidden)]
    #[must_use]
    pub fn with_github_origin_for_test(mut self, origin: impl Into<String>) -> Self {
        let inner = Arc::get_mut(&mut self.inner).expect("서비스를 만든 직후에만 부른다");
        if let Some(g) = inner.github.as_mut() {
            g.origin = Some(origin.into());
        }
        self
    }
}

impl<H: super::FetchHost, R: GitRunner + Send + 'static> Inner<H, R> {
    /// 레코드의 링크와 캐시 경로가 맞는 github 대상의 캐시 폴더·링크(TS `githubTarget`). 링크가 없거나 경로가 캐시 모양이 아니거나 레코드의 owner/repo 와 다르면 `None`.
    /// 기본 브랜치는 미러가 따로 쥔다(`Target` 에 없다)
    fn github_dir(&self, repo_id: &str, target: &Target) -> Option<(GithubCachePath, GithubLink)> {
        let Target::Github { path, link: Some(l) } = target else { return None };
        let cache_dir = &self.github.as_ref()?.cache_dir;
        if !cache_path_matches_link(cache_dir, path, &l.owner, &l.repo) {
            return None;
        }
        let dir = GithubCachePath::new(cache_dir, path)?;
        Some((dir, GithubLink { owner: l.owner.clone(), repo: l.repo.clone(), default_branch: self.mirror.github_branch(repo_id) }))
    }

    /// 이 fetch 의 제한: 아직 ref 가 없는 첫 fetch 는 60분, 아니면 60초. ref 를 읽지 못하면 60초(TS `.catch(() => false)`)
    pub(super) async fn github_limit(&self, repo_id: &str, target: &Target, source: &Source<R>, token: &CancellationToken) -> Duration {
        let Some((dir, _)) = self.github_dir(repo_id, target) else { return FETCH_TIMEOUT };
        match has_no_refs(&*source.runner, &dir, Some(token)).await {
            Ok(true) => GITHUB_FIRST_FETCH_TIMEOUT,
            _ => FETCH_TIMEOUT,
        }
    }

    /// 큐 슬롯 안에서 도는 github fetch. 바뀐 ref 가 있으면 true. **검사 직후 바로 실행한다**(확인 창 같은 대기가 없다)
    pub(super) async fn github_task(self: Arc<Self>, repo_id: String, target: Target, source: Arc<Source<R>>, token: CancellationToken, flags: Arc<RunFlags>) -> Result<bool, ExecError> {
        // 제한은 슬롯을 얻은 뒤부터 잰다
        flags.deadline.arm(&token, Duration::from_secs(flags.github_limit_secs.load(std::sync::atomic::Ordering::SeqCst)));
        let r = &*source.runner;
        self.with_state(&repo_id, |s| {
            s.no_remote = false;
            s.blocked = None;
        });
        let blocked = |keys: Vec<String>| Some(Blocked { keys, confirmable: false });
        let Some((dir, link)) = self.github_dir(&repo_id, &target) else {
            self.with_state(&repo_id, |s| s.blocked = blocked(vec!["github (앱 캐시 경로가 저장소와 맞지 않음)".to_string()]));
            return Ok(false);
        };
        let origin = self.github.as_ref().and_then(|g| g.origin.as_deref());
        let problems = inspect_github_repo_at(r, &dir, &link, origin, Some(&token)).await?;
        if !problems.is_empty() {
            self.with_state(&repo_id, |s| s.blocked = blocked(problems));
            return Ok(false);
        }
        let no_hooks_dir = self.no_hooks_dir.as_deref().ok_or_else(|| ExecError::Other { message: "훅 차단 디렉터리를 쓸 수 없습니다".to_string() })?;
        let net_args = github_net_args(no_hooks_dir)?;
        let before = refs_signature(r, dir.as_str(), &token).await.ok();
        run_github_fetch(r, &dir, &GithubFetchOpts { cancel: Some(token.clone()), no_hooks_dir }).await?;
        // HEAD 를 받아 온 기본 브랜치에 맞춘다. 실패해도 fetch 자체는 성공이다(취소만 그대로 올린다)
        match ensure_github_head(r, &dir, &link, Some(&token), &net_args).await {
            Ok(Some(branch)) if link.default_branch.as_deref() != Some(branch.as_str()) => self.host.github_branch(&repo_id, &branch),
            Err(ExecError::Aborted) => return Err(ExecError::Aborted),
            _ => {}
        }
        let after = refs_signature(r, dir.as_str(), &token).await.ok();
        self.with_state(&repo_id, |s| s.last_ok_at = Some(super::now_ms()));
        Ok(before != after)
    }

    /// 선택한 github 저장소에 받은 것이 없으면(ref 가 없다) 바로 fetch 한다(일반 저장소의 첫 자동 fetch 지연은 선택 직후의 조회들이 먼저 끝나게 하려는 것이다).
    /// github 가 아니면·막혔거나 인증이 멈췄거나 진행 중이면·ref 를 읽지 못하면 아무것도 하지 않는다 — 다음 자동 fetch 에 맡긴다
    pub(super) async fn fetch_if_empty(self: &Arc<Self>, repo_id: &str) {
        if !matches!(self.mirror.resolve_fetch(repo_id), Resolved::Rust(Target::Github { .. })) {
            return;
        }
        // 삭제 중인 저장소는 폴더를 건드리지 않는다(아래 ref 조회도 git 을 띄운다)
        if lock(&self.core).removing.get(repo_id).is_some_and(|until| *until > tokio::time::Instant::now()) {
            return;
        }
        let Ok((target, source)) = self.resolve(repo_id) else { return };
        let Some((dir, _)) = self.github_dir(repo_id, &target) else { return };
        {
            let mut core = lock(&self.core);
            let s = core.state(repo_id);
            let idle = s.blocked.is_none() && !s.auth_paused;
            if !idle || core.inflight.contains_key(repo_id) {
                return;
            }
        }
        if !has_no_refs(&*source.runner, &dir, None).await.unwrap_or(false) {
            return;
        }
        let still_active = lock(&self.core).active.as_deref() == Some(repo_id);
        if still_active {
            let _ = self.fetch(repo_id, false).await;
        }
    }

    /// 로그인 상태가 바뀌었다: 인증 문제로 멈춰 있던 github 저장소의 정지를 풀고(백오프·수동 필요 표시도), 예약을 다시 하고, 열고 있는 저장소가 비어 있으면 바로 받는다
    pub(super) fn resume_github_auth(self: &Arc<Self>) {
        let paused: Vec<String> = lock(&self.core).states.iter().filter(|(_, s)| s.auth_paused).map(|(id, _)| id.clone()).collect();
        for id in paused {
            if !matches!(self.mirror.resolve_fetch(&id), Resolved::Rust(Target::Github { .. })) {
                continue;
            }
            self.with_state(&id, |s| {
                s.auth_paused = false;
                s.failures = 0;
                s.needs_manual = false;
            });
            self.emit_status(&id);
        }
        self.schedule();
        let active = lock(&self.core).active.clone();
        if let Some(id) = active {
            let this = self.clone();
            tokio::spawn(async move { this.fetch_if_empty(&id).await });
        }
    }
}
