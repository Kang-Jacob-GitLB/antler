//! 설정이 바뀔 때 미러(`RepoMirror`)와 fetch 설정(`FetchPrefs`)을 맞추는 곳(Phase 6b PR 9). Rust 가 원천으로서 직접 넘긴다.
//!
//! - **변경 알림에서 동기로 불린다**(`ConfigStore::set_on_change`) — 채널 응답보다 먼저 미러가 바뀌어 renderer 의 다음 호출이 옛 미러를 보지 않는다.
//! - **링크·기본 브랜치는 `RepoMirror::apply_message` 의 같은 경로**(`sanitize_github_link`·`is_safe_branch`)로 들어간다 — `mirror_of` 가 만든 값을 그 길 밖으로 넘기지 않는다
//!   (PR 6·7 의 "PR 9 계약": 검증되지 않은 owner/repo·브랜치가 `gh:repo`·`ref` 이름으로 가지 못하게). 같은 이유로 `FetchPrefs` 도 `from_message`(지문 모양 검사)를 거친다.
//! - 세대 번호는 여기서 센다(1부터) — `RepoMirror` 와 `FetchPrefs` 가 `gen <= 받은 값` 을 버리고 초기값이 0 이라, 설정의 세대(로드 직후 0)를 그대로 쓰면 첫 반영이 버려진다.
//!   설정의 세대는 알림이 뒤집혀 도착했을 때 낡은 값을 버리는 데만 쓴다.
//! - 직전과 같은 미러·같은 fetch 설정이면 보내지 않는다(UI 설정 같은 무관한 쓰기가 `on_mirror_changed`·재예약을 돌리지 않게). 반영 전체가 잠금 하나 안에서 돈다 — 두 알림이 겹쳐도 순서가 뒤집히지 않는다.
//!
//! 이 모듈은 `Router`·`AppHandle` 에 기대지 않는다 — 반영 뒤 할 일은 콜백으로 주입된다(앱은 `lib.rs` 가 `Router::on_mirror_changed`·`apply_fetch_prefs` 를 단다).

use std::sync::{Arc, Mutex, PoisonError};

use serde_json::{json, Map, Value};

use crate::git::fetch::service::FetchPrefs;
use crate::mirror::RepoMirror;
use crate::net::json::Json;

use super::derive::{fetch_prefs_of, mirror_of, FetchPrefsOf, MirrorEntry};

/// 미러가 바뀐 직후 부른다(라우터의 `on_mirror_changed`)
pub type OnMirror = Arc<dyn Fn() + Send + Sync>;
/// fetch 설정이 바뀌었을 때 부른다(라우터의 `apply_fetch_prefs`)
pub type OnPrefs = Arc<dyn Fn(FetchPrefs) + Send + Sync>;

#[derive(Default)]
struct State {
    /// 마지막으로 반영한 설정의 세대(없으면 아직 반영한 적 없다)
    config_gen: Option<u64>,
    mirror_gen: u64,
    last_mirror: Option<Vec<MirrorEntry>>,
    prefs_gen: u64,
    last_prefs: Option<FetchPrefsOf>,
}

pub struct ConfigSync {
    mirror: RepoMirror,
    on_mirror: OnMirror,
    on_prefs: OnPrefs,
    state: Mutex<State>,
}

/// 미러 항목 하나를 `repos` 메시지의 한 줄로(`tests/fixtures/rustPortSpecs.ts` 의 `MirrorRepo` 와 같은 모양)
fn entry_json(e: &MirrorEntry) -> Value {
    let mut o = Map::new();
    o.insert("id".into(), json!(e.repo.id));
    o.insert("kind".into(), json!(e.repo.kind));
    o.insert("path".into(), json!(e.repo.path));
    if let Some(n) = &e.name {
        o.insert("name".into(), json!(n));
    }
    if let Some(d) = &e.repo.distro {
        o.insert("distro".into(), json!(d));
    }
    if let Some(h) = &e.repo.ssh_host {
        o.insert("sshHost".into(), json!(h));
    }
    if let Some((owner, repo, branch)) = &e.github {
        let mut g = Map::new();
        g.insert("owner".into(), json!(owner));
        g.insert("repo".into(), json!(repo));
        if let Some(b) = branch {
            g.insert("defaultBranch".into(), json!(b));
        }
        o.insert("github".into(), Value::Object(g));
    }
    Value::Object(o)
}

/// `{t:"repos", gen, repos}` — 미러 메시지 모양(`RepoMirror::apply_message` 가 받는다)
pub fn repos_message(gen: u64, entries: &[MirrorEntry]) -> Value {
    json!({ "t": "repos", "gen": gen, "repos": entries.iter().map(entry_json).collect::<Vec<_>>() })
}

/// `{t:"fetchPrefs", gen, autoFetchMinutes, approvals, trusted}` — `FetchPrefs::from_message` 가 받는다
pub fn prefs_message(gen: u64, prefs: &FetchPrefsOf) -> Value {
    let map = |pairs: &[(String, String)]| Value::Object(pairs.iter().map(|(k, v)| (k.clone(), json!(v))).collect());
    json!({ "t": "fetchPrefs", "gen": gen, "autoFetchMinutes": prefs.auto_fetch_minutes, "approvals": map(&prefs.approvals), "trusted": map(&prefs.trusted) })
}

impl ConfigSync {
    pub fn new(mirror: RepoMirror, on_mirror: OnMirror, on_prefs: OnPrefs) -> Self {
        ConfigSync { mirror, on_mirror, on_prefs, state: Mutex::new(State::default()) }
    }

    /// 설정을 반영한다. `config_gen` 은 `ConfigStore` 의 세대이고 시작 때 한 번의 첫 반영은 `None` 이다. 이미 반영한 것 이하의 세대는 버린다(뒤집혀 도착한 낡은 알림)
    pub fn push(&self, config_gen: Option<u64>, config: &Json) {
        let mut st = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if let (Some(new), Some(seen)) = (config_gen, st.config_gen) {
            if new <= seen {
                return;
            }
        }
        if config_gen.is_some() {
            st.config_gen = config_gen;
        }
        // 미러 먼저, 그다음 fetch 설정이다 — 저장소가 사라진 설정을 승인 맵이 앞서가지 않게(TS 와 같은 순서)
        let entries = mirror_of(config);
        if st.last_mirror.as_ref() != Some(&entries) {
            st.mirror_gen += 1;
            if self.mirror.apply_message(&repos_message(st.mirror_gen, &entries)) {
                st.last_mirror = Some(entries);
                (self.on_mirror)();
            }
        }
        let prefs = fetch_prefs_of(config);
        if st.last_prefs.as_ref() != Some(&prefs) {
            st.prefs_gen += 1;
            if let Some(p) = FetchPrefs::from_message(&prefs_message(st.prefs_gen, &prefs)) {
                st.last_prefs = Some(prefs);
                (self.on_prefs)(p);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mirror::Resolved;
    use crate::net::json::parse;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn rig() -> (ConfigSync, RepoMirror, Arc<AtomicUsize>, Arc<Mutex<Vec<FetchPrefs>>>) {
        let mirror = RepoMirror::default();
        let changed = Arc::new(AtomicUsize::new(0));
        let prefs: Arc<Mutex<Vec<FetchPrefs>>> = Arc::default();
        let (c, p) = (changed.clone(), prefs.clone());
        let sync = ConfigSync::new(mirror.clone(), Arc::new(move || {
            c.fetch_add(1, Ordering::SeqCst);
        }), Arc::new(move |x| p.lock().unwrap().push(x)));
        (sync, mirror, changed, prefs)
    }

    fn cfg(repos: &str, ui: &str) -> Json {
        parse(&format!(r#"{{"version":1,"sources":[{{"id":"local","kind":"local","label":"Local"}},{{"id":"github","kind":"github","label":"GitHub"}}],"repos":{repos},"ui":{ui}}}"#)).unwrap()
    }

    #[test]
    fn 첫_반영은_세대_0_에서도_버려지지_않고_같은_내용은_다시_보내지_않는다() {
        let (sync, mirror, changed, prefs) = rig();
        let c = cfg(r#"[{"id":"a","sourceId":"local","path":"C:\\a","name":"A"}]"#, r#"{"autoFetchMinutes":7}"#);
        sync.push(None, &c);
        assert!(matches!(mirror.resolve("a"), Resolved::Rust(_)));
        assert_eq!(mirror.name("a").as_deref(), Some("A"));
        assert_eq!((changed.load(Ordering::SeqCst), prefs.lock().unwrap().len()), (1, 1));
        assert_eq!(prefs.lock().unwrap()[0].auto_fetch_minutes, 7.0);
        // 저장소와 무관한 설정 쓰기(같은 미러·같은 fetch 설정)는 아무것도 부르지 않는다
        sync.push(Some(1), &cfg(r#"[{"id":"a","sourceId":"local","path":"C:\\a","name":"A"}]"#, r#"{"autoFetchMinutes":7,"theme":"dark"}"#));
        assert_eq!((changed.load(Ordering::SeqCst), prefs.lock().unwrap().len()), (1, 1));
    }

    #[test]
    fn 낡은_세대의_알림은_버린다() {
        let (sync, mirror, changed, _) = rig();
        sync.push(Some(2), &cfg(r#"[{"id":"b","sourceId":"local","path":"C:\\b","name":"B"}]"#, "{}"));
        sync.push(Some(1), &cfg(r#"[{"id":"a","sourceId":"local","path":"C:\\a","name":"A"}]"#, "{}"));
        assert!(matches!(mirror.resolve("b"), Resolved::Rust(_)));
        assert!(matches!(mirror.resolve("a"), Resolved::Unserved));
        assert_eq!(changed.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn github_링크와_브랜치는_검증을_거쳐_들어간다() {
        let (sync, mirror, _, _) = rig();
        mirror.set_github(true);
        let ok = r#"{"id":"g1","sourceId":"github","path":"C:\\cache\\o\\r.git","name":"r","github":{"owner":"o","repo":"r","defaultBranch":"main"}}"#;
        let bad = r#"{"id":"g2","sourceId":"github","path":"C:\\cache\\x\\y.git","name":"y","github":{"owner":"..","repo":"y","defaultBranch":"-x"}}"#;
        sync.push(None, &cfg(&format!("[{ok},{bad}]"), "{}"));
        assert!(matches!(mirror.resolve("g1"), Resolved::Rust(crate::mirror::Target::Github { link: Some(_), .. })));
        assert_eq!(mirror.github_branch("g1").as_deref(), Some("main"));
        // 링크가 규칙을 통과하지 못한 레코드는 링크도 브랜치도 받지 못한다
        assert!(matches!(mirror.resolve("g2"), Resolved::Rust(crate::mirror::Target::Github { link: None, .. })));
        assert_eq!(mirror.github_branch("g2"), None);
    }

    #[test]
    fn 승인은_fetch_설정으로_가고_모양이_틀린_지문은_버려진다() {
        let (sync, _, _, prefs) = rig();
        let fp = "a".repeat(64);
        let c = parse(&format!(
            r#"{{"version":1,"sources":[{{"id":"local","kind":"local","label":"Local"}}],"repos":[{{"id":"a","sourceId":"local","path":"C:\\a","name":"A"}}],"ui":{{}},"fetchApprovals":{{"a":"{fp}","zz":"{fp}"}},"trustedConfigs":{{"a":"nothex"}}}}"#
        ))
        .unwrap();
        sync.push(None, &c);
        let p = prefs.lock().unwrap()[0].clone();
        assert_eq!(p.approvals.get("a"), Some(&fp));
        assert!(!p.approvals.contains_key("zz"), "저장소 목록에 없는 id 의 승인은 싣지 않는다");
        assert!(p.trusted.is_empty());
    }
}
