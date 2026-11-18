//! 호스트별 SSH 실행기 풀. **같은 호스트는 같은 실행기(= 같은 연결·같은 채널 슬롯)** 를 쓴다 — 라우터의 큐와(이후) fetch 서비스가 이 풀을 나눠 쓰므로
//! 호스트당 연결이 하나이고 채널 6개 한도도 하나다(`RepoService.ts` 의 `runtimes` 가 소스당 실행기 하나를 두는 것과 같다).
//! 실행기를 만드는 방법(홈·agent·시간 제한)은 만들 때 주입한다 — 운영은 `SshPool::for_home`, 시험은 임시 홈과 `AgentSetting::Disabled`.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, PoisonError};

use super::{SshExecutor, SshOptions};

type Make = Box<dyn Fn(&str) -> SshExecutor + Send + Sync>;

/// 풀 잠금 하나로 지키는 상태. 실행기 표와 "저장 전 잡기"(`hold`) 장부를 한 잠금에 둔다 — `retain` 과 `HoldGuard` 의 해제가 서로 엇갈려 보지 않게
#[derive(Default)]
struct State {
    hosts: HashMap<String, SshExecutor>,
    /// 호스트별 잡기 횟수. 잡힌 호스트는 `retain` 이 닫지 않는다
    held: HashMap<String, usize>,
    /// `hold` 가 새로 만든 호스트(아직 아무도 `get` 으로 채택하지 않았다). 마지막 잡기가 풀릴 때 `retain` 의 보존 목록에 없으면 닫는다
    transient: HashSet<String>,
    /// 마지막 `retain` 의 보존 목록(미러에 있는 호스트). 아직 `retain` 이 없었으면 `None`
    keep: Option<HashSet<String>>,
}

pub struct SshPool {
    make: Make,
    state: Mutex<State>,
}

/// [`SshPool::hold`] 가 돌려주는 잡기. 버려질 때(마지막 잡기이고 미러에 없는 호스트면) 연결을 닫고 풀에서 뺀다
pub struct HoldGuard {
    pool: Arc<SshPool>,
    host: String,
    exec: SshExecutor,
}

impl HoldGuard {
    pub fn executor(&self) -> &SshExecutor {
        &self.exec
    }

    pub fn host(&self) -> &str {
        &self.host
    }
}

impl Drop for HoldGuard {
    fn drop(&mut self) {
        let closed = {
            let mut st = self.pool.state.lock().unwrap_or_else(PoisonError::into_inner);
            let Some(n) = st.held.get_mut(&self.host) else { return };
            *n -= 1;
            if *n > 0 {
                return;
            }
            st.held.remove(&self.host);
            let saved = st.keep.as_ref().is_some_and(|k| k.contains(&self.host));
            let transient = st.transient.remove(&self.host);
            // 미러에 있으면 남긴다. 없으면 우리가 만들었거나(transient), 잡혀 있어 `retain` 이 건너뛴 것이므로 닫는다
            if saved || !(transient || st.keep.is_some()) {
                None
            } else {
                st.hosts.remove(&self.host)
            }
        };
        if let Some(e) = closed {
            e.dispose();
        }
    }
}

impl SshPool {
    pub fn new(make: impl Fn(&str) -> SshExecutor + Send + Sync + 'static) -> Arc<Self> {
        Arc::new(SshPool { make: Box::new(make), state: Mutex::default() })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// 아직 저장하지 않은 호스트(연결 테스트·폴더 탐색·추가 중)의 실행기를 잡아 둔다(TS `holdRuntime`). 같은 새 호스트를 여러 요청이 동시에 쓰면
    /// 먼저 끝난 쪽이 연결을 닫아 다른 쪽을 끊지 않도록 참조 횟수로 센다. 잡힌 호스트는 `retain`(다른 저장소의 미러 변경)이 닫지 않는다.
    /// 마지막 잡기가 풀릴 때 미러에 없는 호스트면 닫는다(`HoldGuard` 의 Drop). 이미 저장된 호스트는 건드리지 않고 그 실행기를 쓴다
    pub fn hold(self: &Arc<Self>, host: &str) -> HoldGuard {
        let exec = {
            let mut st = self.lock();
            let existed = st.hosts.contains_key(host);
            let exec = st.hosts.entry(host.to_string()).or_insert_with(|| (self.make)(host)).clone();
            if !existed {
                st.transient.insert(host.to_string());
            }
            *st.held.entry(host.to_string()).or_default() += 1;
            exec
        };
        HoldGuard { pool: self.clone(), host: host.to_string(), exec }
    }

    /// 운영 풀: 사용자 홈의 `~/.ssh/{config,known_hosts}`·기본 키·agent·프로세스 환경변수
    pub fn for_home(home_dir: &str) -> Arc<Self> {
        let home = home_dir.to_string();
        SshPool::new(move |host| SshExecutor::new(host, SshOptions::for_home(&home)))
    }

    /// 호스트의 실행기. 없으면 만들고 이후 같은 값을 돌려준다. 이미 정리한(`dispose`) 호스트는 새로 만든다
    pub fn get(&self, host: &str) -> SshExecutor {
        let mut st = self.lock();
        // 저장된 호스트로 쓰이기 시작했다 — 잡기가 풀려도 더는 "임시"가 아니다(닫는 쪽은 `retain` 이다)
        st.transient.remove(host);
        st.hosts.entry(host.to_string()).or_insert_with(|| (self.make)(host)).clone()
    }

    /// `get` 과 같되, 풀 잠금을 쥔 채 `alive` 가 참일 때만 만든다(거짓이면 `None`).
    /// 호스트 확인과 등록을 `retain` 과 같은 잠금 하나로 묶는다 — 미러 적용 뒤의 호출은 거부되고, 그 전의 호출이 넣은 실행기는 뒤이은 `retain` 이 닫는다.
    /// 라우터와 다른 잠금으로 확인하는 쪽(fetch 서비스)이 지운 호스트를 닫히지 않은 채 다시 등록하지 않게 한다
    pub fn get_if(&self, host: &str, alive: impl FnOnce() -> bool) -> Option<SshExecutor> {
        let mut st = self.lock();
        if !alive() {
            return None;
        }
        st.transient.remove(host);
        Some(st.hosts.entry(host.to_string()).or_insert_with(|| (self.make)(host)).clone())
    }

    /// `keep` 에 없는 호스트의 연결을 닫고 풀에서 뺀다(닫은 실행기는 영구히 거부하므로 **풀에서도 빼야** 같은 호스트를 다시 쓸 때 새로 맺는다). 닫은 호스트 이름을 돌려준다.
    /// **`hold` 로 잡힌 호스트는 닫지 않는다**(저장 전 연결 테스트가 다른 미러 변경에 끊기지 않게). 마지막 잡기가 풀릴 때 이 `keep` 에 없으면 그때 닫는다
    pub fn retain(&self, keep: &HashSet<String>) -> Vec<String> {
        let gone: Vec<(String, SshExecutor)> = {
            let mut st = self.lock();
            st.keep = Some(keep.clone());
            let names: Vec<String> = st.hosts.keys().filter(|h| !keep.contains(*h) && !st.held.contains_key(*h)).cloned().collect();
            names.into_iter().filter_map(|h| st.hosts.remove(&h).map(|e| (h, e))).collect()
        };
        let mut names: Vec<String> = gone.iter().map(|(h, _)| h.clone()).collect();
        for (_, e) in &gone {
            e.dispose();
        }
        names.sort();
        names
    }

    /// 잡기를 포함해 모든 연결을 닫고 비운다(앱 종료). 남아 있는 `HoldGuard` 는 해제될 때 아무것도 하지 않는다
    pub fn dispose_all(&self) {
        let all: Vec<SshExecutor> = {
            let mut st = self.lock();
            st.held.clear();
            st.transient.clear();
            st.keep = Some(HashSet::new());
            st.hosts.drain().map(|(_, e)| e).collect()
        };
        for e in &all {
            e.dispose();
        }
    }

    /// 풀에 있는 호스트(정렬)
    pub fn hosts(&self) -> Vec<String> {
        let mut v: Vec<String> = self.lock().hosts.keys().cloned().collect();
        v.sort();
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pool() -> Arc<SshPool> {
        SshPool::for_home("C:\\nonexistent-antler-home")
    }

    #[tokio::test]
    async fn 같은_호스트는_같은_실행기를_받는다() {
        let p = pool();
        let a = p.get("a");
        a.dispose();
        // 같은 실행기라 한쪽에서 닫으면 다른 쪽도 닫힌 것이다 — 연결을 나눠 쓴다는 뜻
        let err = p.get("a").run("/r", &["status"], &crate::git::exec::RunOpts::default()).await.unwrap_err();
        assert!(err.to_string().contains("이미 종료"), "{err}");
        assert_eq!(p.hosts(), ["a"]);
        p.get("b");
        assert_eq!(p.hosts(), ["a", "b"]);
    }

    #[test]
    fn retain_은_쓰지_않는_호스트를_빼고_다시_쓰면_새로_만든다() {
        let made = Arc::new(Mutex::new(0usize));
        let m = made.clone();
        let counting = SshPool::new(move |h| {
            *m.lock().unwrap() += 1;
            SshExecutor::new(h, SshOptions::for_home("C:\\nonexistent-antler-home"))
        });
        counting.get("a");
        counting.get("a");
        counting.get("b");
        assert_eq!(*made.lock().unwrap(), 2);
        assert_eq!(counting.retain(&HashSet::from(["a".to_string()])), ["b"]);
        assert_eq!(counting.hosts(), ["a"]);
        counting.get("b");
        assert_eq!(*made.lock().unwrap(), 3, "닫은 호스트는 풀에서 빠져 있어 새로 만든다");
        counting.dispose_all();
        assert!(counting.hosts().is_empty());
    }

    fn keep(hosts: &[&str]) -> HashSet<String> {
        hosts.iter().map(|h| (*h).to_string()).collect()
    }

    #[tokio::test]
    async fn 잡은_호스트는_retain_이_닫지_않고_마지막_잡기가_풀리면_닫는다() {
        let p = pool();
        let a = p.hold("new");
        let b = p.hold("new");
        assert_eq!(p.hosts(), ["new"]);
        // 다른 저장소의 미러 변경(retain)이 와도 잡힌 호스트는 남는다
        assert!(p.retain(&keep(&["saved"])).is_empty());
        assert_eq!(p.hosts(), ["new"]);
        assert!(!a.executor().is_disposed());
        drop(a);
        assert_eq!(p.hosts(), ["new"], "아직 잡기가 남았다");
        assert!(!b.executor().is_disposed());
        let exec = b.executor().clone();
        drop(b);
        assert!(p.hosts().is_empty(), "마지막 잡기가 풀리면 미러에 없는 호스트는 풀에서 빠진다");
        assert!(exec.is_disposed());
    }

    #[tokio::test]
    async fn 저장된_호스트는_잡기가_풀려도_닫지_않는다() {
        let p = pool();
        // retain 이 아직 없었고 hold 가 만든 것도 아니면(다른 곳에서 get 으로 이미 쓰는 호스트) 건드리지 않는다
        let used = p.get("used");
        drop(p.hold("used"));
        assert!(!used.is_disposed());
        assert_eq!(p.hosts(), ["used"]);
        // 잡혀 있는 동안 저장돼 미러 목록에 들어왔다
        let g = p.hold("new");
        p.retain(&keep(&["new", "used"]));
        let exec = g.executor().clone();
        drop(g);
        assert!(!exec.is_disposed());
        assert_eq!(p.hosts(), ["new", "used"]);
        // 이미 저장된 호스트를 잡았다 놓아도 그대로다
        drop(p.hold("new"));
        assert_eq!(p.hosts(), ["new", "used"]);
    }

    #[tokio::test]
    async fn 잡힌_사이_retain_이_건너뛴_호스트는_풀릴_때_닫는다() {
        let p = pool();
        p.get("old");
        let g = p.hold("old");
        // 미러에서 빠졌지만 잡혀 있어 남는다
        assert!(p.retain(&keep(&[])).is_empty());
        assert_eq!(p.hosts(), ["old"]);
        drop(g);
        assert!(p.hosts().is_empty());
    }

    #[tokio::test]
    async fn 잡은_호스트를_get_으로_채택하면_풀려도_임시로_닫지_않는다() {
        let p = pool();
        let g = p.hold("x");
        p.get("x");
        drop(g);
        assert_eq!(p.hosts(), ["x"], "저장된 호스트로 쓰이기 시작했으면 닫는 쪽은 retain 이다");
        assert_eq!(p.retain(&keep(&[])), ["x"]);
    }

    #[tokio::test]
    async fn dispose_all_은_잡힌_호스트도_닫고_남은_가드는_아무것도_하지_않는다() {
        let p = pool();
        let g = p.hold("x");
        p.dispose_all();
        assert!(g.executor().is_disposed());
        assert!(p.hosts().is_empty());
        let again = p.get("x");
        drop(g);
        assert!(!again.is_disposed(), "앱 종료 뒤 새로 만든 실행기를 낡은 가드가 닫지 않는다");
    }

    #[test]
    fn get_if_는_확인이_거짓이면_만들지도_등록하지도_않는다() {
        let p = pool();
        assert!(p.get_if("a", || false).is_none());
        assert!(p.hosts().is_empty(), "거부한 호스트는 풀에 남지 않는다");
        assert!(p.get_if("a", || true).is_some());
        assert_eq!(p.hosts(), ["a"]);
    }
}
