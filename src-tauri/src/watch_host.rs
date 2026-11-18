//! 저장소 감시(`git::watcher::RepoWatcher`)의 앱 쪽 호스트(Phase 6a PR 7b). 라우터가 만들어 감시자에게 준다.
//!
//! - **저장소 종류·실행기는 라우터와 같은 것**이다: 소유 판정은 fetch 와 같은 한 술어(`RepoMirror::resolve_fetch` — 소스 종류가 먼저)이고
//!   실행기·큐는 라우터가 쥔 [`Runtimes`] 다. 그래서 SSH 저장소의 감시도 라우터·fetch 서비스와 같은 풀의 연결 하나를 쓴다(호스트당 연결 하나).
//! - **시그니처**: 취소 신호가 있는 호출(확인)은 `RepoState::compute_signature` 로 따로 계산하고(합치면 변화를 놓친다), 없는 호출(기준)은 제한 시간이 있는 공유 계산이다.
//!   둘 다 성공하면 `known_signature` 를 갱신한다 — `activity` 가 이 기억값을 재사용한다(TS 와 같다).
//! - **감시할 디렉터리**는 로컬 저장소와 GitHub 원격 전용 저장소(앱 캐시 — 실행기가 로컬이다)만 구한다(`rev-parse --git-dir --git-common-dir`). WSL·SSH 는 폴링만이라 `None` 이다.
//!
//! 이 모듈은 `AppHandle` 에 기대지 않는다 — 변화는 주입한 싱크([`ChangeSink`])로 낸다(앱은 renderer 이벤트로, 시험은 수집기로).

use std::sync::{Arc, Mutex, PoisonError};

use futures_util::future::{BoxFuture, FutureExt};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::git::exec::{ExecError, RunOpts};
use crate::git::repo_state::{RepoState, SigError};
use crate::git::signature::Signature;
use crate::git::watcher::{WatchHost, WatchKind};
use crate::mirror::{RepoMirror, Resolved, Target};
use crate::router::{queued_signature, ChangeSink, Runtimes};

/// `git rev-parse` 출력의 git 디렉터리 목록: 줄마다 앞뒤 공백을 지우고 빈 줄·중복을 뺀다(TS `gitDirs`)
fn parse_git_dirs(stdout: &str) -> Vec<String> {
    let mut dirs: Vec<String> = Vec::new();
    for line in stdout.split('\n') {
        let line = line.trim();
        if !line.is_empty() && !dirs.iter().any(|d| d == line) {
            dirs.push(line.to_string());
        }
    }
    dirs
}

pub(crate) struct RouterWatchHost {
    mirror: RepoMirror,
    state: RepoState,
    runtimes: Runtimes,
    /// 앱이 나중에 단다(호출 시점에 읽는다)
    sink: Arc<Mutex<Option<ChangeSink>>>,
}

impl RouterWatchHost {
    pub(crate) fn new(mirror: RepoMirror, state: RepoState, runtimes: Runtimes, sink: Arc<Mutex<Option<ChangeSink>>>) -> Self {
        RouterWatchHost { mirror, state, runtimes, sink }
    }
}

fn failed<T: Send + 'static>(message: &str) -> BoxFuture<'static, Result<T, SigError>> {
    let e = SigError::Failed(message.to_string());
    async move { Err(e) }.boxed()
}

impl WatchHost for RouterWatchHost {
    fn kind(&self, repo_id: &str) -> Option<WatchKind> {
        match self.mirror.resolve_fetch(repo_id) {
            Resolved::Rust(Target::Local { .. }) => Some(WatchKind::Local),
            Resolved::Rust(Target::Wsl { .. }) => Some(WatchKind::Wsl),
            Resolved::Rust(Target::Ssh { .. }) => Some(WatchKind::Ssh),
            // GitHub 원격 전용 저장소의 bare 캐시 폴더는 로컬 폴더라 notify 로 본다(Phase 6b PR 7)
            Resolved::Rust(Target::Github { .. }) => Some(WatchKind::Github),
            _ => None,
        }
    }

    fn after_first_page(&self, repo_id: &str) -> BoxFuture<'static, ()> {
        self.state.after_first_page(repo_id).boxed()
    }

    fn signature(&self, repo_id: &str, cancel: Option<CancellationToken>) -> BoxFuture<'static, Result<Signature, SigError>> {
        let Resolved::Rust(target) = self.mirror.resolve_fetch(repo_id) else { return failed("저장소를 찾을 수 없습니다") };
        let rt = match self.runtimes.get(&target) {
            Ok(rt) => rt,
            Err(e) => {
                let e = SigError::from(e);
                return async move { Err(e) }.boxed();
            }
        };
        let (queue, exec, path) = (rt.queue.clone(), rt.exec.clone(), target.path().to_string());
        match cancel {
            // 확인: 합치지 않고 따로 계산한다. 순번은 이 호출 시점에 받는다(`compute_signature`)
            Some(token) => self.state.compute_signature(repo_id, queued_signature(queue, exec, path, token)).boxed(),
            // 기준: 제한 시간(20초)이 있는 공유 계산 — 멈춘 원격이어도 기준 계산이 영영 남지 않는다
            None => {
                let (state, id) = (self.state.clone(), repo_id.to_string());
                async move { state.shared_signature(&id, move |tok| queued_signature(queue, exec, path, tok), None).await }.boxed()
            }
        }
    }

    fn local_git_dirs(&self, repo_id: &str, cancel: CancellationToken) -> BoxFuture<'static, Option<Vec<String>>> {
        let target = match self.mirror.resolve_fetch(repo_id) {
            Resolved::Rust(t @ (Target::Local { .. } | Target::Github { .. })) => t,
            _ => return async { None }.boxed(),
        };
        let Ok(rt) = self.runtimes.get(&target) else { return async { None }.boxed() };
        let (queue, exec, path) = (rt.queue.clone(), rt.exec.clone(), target.path().to_string());
        async move {
            // 큐의 결과 타입이 JSON 이라 출력은 문자열로 싣는다. 종료 코드가 0 이 아니면 `Null`(= 모름)
            let out = queue
                .run(None, Some(cancel.clone()), move || async move {
                    let args = ["rev-parse", "--path-format=absolute", "--git-dir", "--git-common-dir"];
                    let r = exec.run(&path, &args, &RunOpts::with_cancel(cancel)).await?;
                    Ok::<_, ExecError>(Arc::new(if r.code == 0 { Value::String(String::from_utf8_lossy(&r.stdout).into_owned()) } else { Value::Null }))
                })
                .await
                .ok()?;
            Some(parse_git_dirs(out.as_str()?))
        }
        .boxed()
    }

    fn on_change(&self, repo_id: &str) {
        let sink = self.sink.lock().unwrap_or_else(PoisonError::into_inner).clone();
        if let Some(sink) = sink {
            sink(repo_id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 등록된_배포판이_없으면_wsl_실행기를_내주지_않는다_로컬은_영향이_없다() {
        use crate::git::exec::DistroGate;
        use std::sync::atomic::{AtomicBool, Ordering};
        let registered = Arc::new(AtomicBool::new(false));
        let r = registered.clone();
        // 간격 0 — 호출마다 다시 읽는다(관문의 기억은 wsl.rs 시험이 본다)
        let runtimes = Runtimes::with_wsl_gate(RepoMirror::default(), DistroGate::new(move || r.load(Ordering::SeqCst), std::time::Duration::ZERO));
        let wsl = Target::Wsl { distro: "Ubuntu".into(), path: "/home/me/repo".into() };
        let err = runtimes.get(&wsl).err().expect("등록이 없으면 wsl.exe 를 부를 실행기를 만들지 않는다");
        assert!(err.to_string().contains("등록"), "{err}");
        assert!(runtimes.get(&Target::Local { path: "C:/r".into() }).is_ok());
        // 설치된 뒤에는 만들어지고, 지워지면 기억해 둔 실행기도 막는다
        registered.store(true, Ordering::SeqCst);
        assert!(runtimes.get(&wsl).is_ok());
        registered.store(false, Ordering::SeqCst);
        assert!(runtimes.get(&wsl).is_err());
    }

    #[test]
    fn git_디렉터리_목록은_줄마다_다듬고_빈_줄과_중복을_뺀다() {
        assert_eq!(parse_git_dirs("C:/r/.git\nC:/r/.git\n"), vec!["C:/r/.git"]);
        // 링크된 워크트리는 git-dir 과 공통 디렉터리가 다르다
        assert_eq!(parse_git_dirs("  C:/r/.git/worktrees/w \r\nC:/r/.git\n\n"), vec!["C:/r/.git/worktrees/w", "C:/r/.git"]);
        assert!(parse_git_dirs("").is_empty());
        assert!(parse_git_dirs("\n \n").is_empty());
    }
}
