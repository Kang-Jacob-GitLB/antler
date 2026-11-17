//! 호스트별 SSH 실행기 풀. **같은 호스트는 같은 실행기(= 같은 연결·같은 채널 슬롯)** 를 쓴다 — 라우터의 큐와(이후) fetch 서비스가 이 풀을 나눠 쓰므로
//! 호스트당 연결이 하나이고 채널 6개 한도도 하나다(`RepoService.ts` 의 `runtimes` 가 소스당 실행기 하나를 두는 것과 같다).
//! 실행기를 만드는 방법(홈·agent·시간 제한)은 만들 때 주입한다 — 운영은 `SshPool::for_home`, 시험은 임시 홈과 `AgentSetting::Disabled`.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, PoisonError};

use super::{SshExecutor, SshOptions};

type Make = Box<dyn Fn(&str) -> SshExecutor + Send + Sync>;

pub struct SshPool {
    make: Make,
    hosts: Mutex<HashMap<String, SshExecutor>>,
}

impl SshPool {
    pub fn new(make: impl Fn(&str) -> SshExecutor + Send + Sync + 'static) -> Arc<Self> {
        Arc::new(SshPool { make: Box::new(make), hosts: Mutex::default() })
    }

    /// 운영 풀: 사용자 홈의 `~/.ssh/{config,known_hosts}`·기본 키·agent·프로세스 환경변수
    pub fn for_home(home_dir: &str) -> Arc<Self> {
        let home = home_dir.to_string();
        SshPool::new(move |host| SshExecutor::new(host, SshOptions::for_home(&home)))
    }

    /// 호스트의 실행기. 없으면 만들고 이후 같은 값을 돌려준다. 이미 정리한(`dispose`) 호스트는 새로 만든다
    pub fn get(&self, host: &str) -> SshExecutor {
        self.hosts.lock().unwrap_or_else(PoisonError::into_inner).entry(host.to_string()).or_insert_with(|| (self.make)(host)).clone()
    }

    /// `keep` 에 없는 호스트의 연결을 닫고 풀에서 뺀다(닫은 실행기는 영구히 거부하므로 **풀에서도 빼야** 같은 호스트를 다시 쓸 때 새로 맺는다). 닫은 호스트 이름을 돌려준다
    pub fn retain(&self, keep: &HashSet<String>) -> Vec<String> {
        let gone: Vec<(String, SshExecutor)> = {
            let mut hosts = self.hosts.lock().unwrap_or_else(PoisonError::into_inner);
            let names: Vec<String> = hosts.keys().filter(|h| !keep.contains(*h)).cloned().collect();
            names.into_iter().filter_map(|h| hosts.remove(&h).map(|e| (h, e))).collect()
        };
        let mut names: Vec<String> = gone.iter().map(|(h, _)| h.clone()).collect();
        for (_, e) in &gone {
            e.dispose();
        }
        names.sort();
        names
    }

    /// 모든 연결을 닫고 비운다(앱 종료)
    pub fn dispose_all(&self) {
        self.retain(&HashSet::new());
    }

    /// 풀에 있는 호스트(정렬)
    pub fn hosts(&self) -> Vec<String> {
        let mut v: Vec<String> = self.hosts.lock().unwrap_or_else(PoisonError::into_inner).keys().cloned().collect();
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
}
