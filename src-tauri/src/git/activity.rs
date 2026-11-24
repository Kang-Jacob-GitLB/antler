//! 커밋 활동(미니맵) 집계(`src/main/git/activity.ts`): 순수 로직(`bucketize`·`TimeStreamParser`·`emptyActivity`)과 그 위의 git 호출(`commitActivity`).
//! `git:activity` 채널의 큐·합치기·시그니처·캐시는 라우터가 맡는다(`ActivityCache` 만 여기 있다).

use std::sync::Arc;

use serde::Serialize;
use serde_json::Value;
use tokio::io::AsyncReadExt;

use super::exec::ssh::is_read_ahead_exceeded;
use super::exec::{ExecError, GitExecutor, RunOpts};
use super::log_service::is_empty_repo_error;
use super::parsers::JsNumber;
use super::scope::{parse_scope, scope_args, scope_key};
use super::signature::Signature;

/// 구간 수 상한. 실제로는 아래 단위 중 구간 수가 이 값 이하가 되는 가장 작은 단위를 고른다
pub const MAX_BUCKETS: usize = 400;
const HOUR: f64 = 3600.0;
const DAY: f64 = 24.0 * HOUR;
/// 구간 단위(초): 1시간 · 6시간 · 1일 · 1주 · 30일 · 1년
const STEPS: [f64; 6] = [HOUR, 6.0 * HOUR, DAY, 7.0 * DAY, 30.0 * DAY, 365.0 * DAY];
/// 시계가 어긋난 미래 커밋 허용 폭 — 이보다 먼 미래 값은 차트 범위에서 제외한다
const FUTURE_SLACK: f64 = 2.0 * DAY;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Bucket {
    pub t: JsNumber,
    pub count: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitActivity {
    /// 시간 오름차순, 구간 시작 시각(epoch 초)과 개수. 빈 구간도 0 으로 채운다
    pub buckets: Vec<Bucket>,
    /// 가장 오래된·가장 최근 커밋 시각(epoch 초)
    pub first: JsNumber,
    pub last: JsNumber,
    /// 보기 범위의 전체 커밋 수
    pub total: usize,
    /// 구간 길이(초)
    pub bucket_sec: JsNumber,
}

pub fn empty_activity() -> CommitActivity {
    CommitActivity { buckets: Vec::new(), first: JsNumber(0.0), last: JsNumber(0.0), total: 0, bucket_sec: JsNumber(DAY) }
}

/// 커밋 시각(epoch 초) 목록을 구간별 개수로 집계한다. TS `bucketize(times, n, now)` 의 `n` 은 슬라이스 길이다.
/// 0 이하이거나 `now` 보다 훨씬 미래인 값은 차트 범위를 왜곡하므로 구간에서 빼지만 total 에는 센다.
/// 구간은 비어 있어도 연속으로 채워(개수 0) 시간축이 끊기지 않게 한다. 계산은 TS 와 같은 f64 연산이다.
pub fn bucketize<T: Copy + Into<f64>>(times: &[T], now: f64) -> CommitActivity {
    let n = times.len();
    let limit = now + FUTURE_SLACK;
    let in_range = |t: f64| !(t <= 0.0 || t > limit);
    let (mut first, mut last) = (f64::INFINITY, f64::NEG_INFINITY);
    for &t in times {
        let t: f64 = t.into();
        if !in_range(t) {
            continue;
        }
        if t < first {
            first = t;
        }
        if t > last {
            last = t;
        }
    }
    if first == f64::INFINITY {
        return CommitActivity { total: n, ..empty_activity() };
    }

    let span = last - first;
    let bucket_sec = STEPS.iter().copied().find(|s| span / s + 1.0 <= MAX_BUCKETS as f64).unwrap_or(STEPS[STEPS.len() - 1]);
    let origin = (first / bucket_sec).floor() * bucket_sec;
    let count = ((last - origin) / bucket_sec).floor() as usize + 1;
    let mut counts = vec![0u32; count];
    for &t in times {
        let t: f64 = t.into();
        if in_range(t) {
            counts[((t - origin) / bucket_sec).floor() as usize] += 1;
        }
    }
    let buckets = counts.iter().enumerate().map(|(i, &c)| Bucket { t: JsNumber(origin + i as f64 * bucket_sec), count: c }).collect();
    CommitActivity { buckets, first: JsNumber(first), last: JsNumber(last), total: n, bucket_sec: JsNumber(bucket_sec) }
}

/// JS `ToUint32`(Uint32Array 에 넣을 때의 변환): 정수부를 2^32 로 나눈 나머지. NaN·무한대는 0
fn to_uint32(v: f64) -> u32 {
    if !v.is_finite() {
        return 0;
    }
    v.trunc().rem_euclid(4_294_967_296.0) as u32
}

/// 숫자가 아닌 바이트로 구분된 십진수 스트림 파서. 청크 경계에 걸친 숫자를 이어 붙인다.
/// TS 는 숫자를 double 로 쌓아 `Uint32Array` 에 넣으므로 2^32 이상이면 감긴다(`to_uint32` — 2^53 넘는 값은 double 반올림까지 같다).
#[derive(Debug, Default)]
pub struct TimeStreamParser {
    values: Vec<u32>,
    cur: f64,
    in_num: bool,
}

impl TimeStreamParser {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, chunk: &[u8]) {
        for &c in chunk {
            if c.is_ascii_digit() {
                self.cur = self.cur * 10.0 + f64::from(c - b'0');
                self.in_num = true;
            } else if self.in_num {
                self.flush();
            }
        }
    }

    pub fn end(&mut self) {
        if self.in_num {
            self.flush();
        }
    }

    fn flush(&mut self) {
        self.values.push(to_uint32(self.cur));
        self.cur = 0.0;
        self.in_num = false;
    }

    /// 지금까지 읽은 값(`end` 전에는 마지막 숫자가 아직 없을 수 있다)
    pub fn values(&self) -> &[u32] {
        &self.values
    }
}

/// 활동 캐시 항목 수 상한(`ACTIVITY_CACHE_ENTRIES`)
pub const ACTIVITY_CACHE_ENTRIES: usize = 8;

/// 읽기 버퍼
const CHUNK: usize = 64 * 1024;

/// 검증을 마친 `git:activity` 요청: 보기 범위의 시작점 인자와 합치기·캐시 키에 쓰는 범위 키
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivityRequest {
    scope_args: Vec<String>,
    scope_key: String,
}

impl ActivityRequest {
    /// TS `git.scopeArgs(scope)` 와 같은 검증(`invalid scope`·`invalid solo`·`invalid hide`). 오류는 ERROR 다
    pub fn parse(scope: Option<&Value>) -> Result<Self, ExecError> {
        let other = |m: String| ExecError::Other { message: m };
        let scope = parse_scope(scope).map_err(other)?;
        let args = scope_args(scope.as_ref()).map_err(other)?;
        Ok(ActivityRequest { scope_args: args, scope_key: scope_key(scope.as_ref()) })
    }

    /// 같은 범위는 항목 순서와 무관하게 같은 키다
    pub fn scope_key(&self) -> &str {
        &self.scope_key
    }

    /// `git log --format=%at <범위> --`. `%at` 는 목록의 authorDate 와 같은 필드라 뷰포트 표시·점프가 어긋나지 않는다
    pub fn args(&self) -> Vec<String> {
        let mut v: Vec<String> = vec!["log".into(), "--format=%at".into()];
        v.extend(self.scope_args.iter().cloned());
        v.push("--".into());
        v
    }
}

/// 현재 시각(epoch 초 — TS `Date.now() / 1000`)
pub fn now_secs() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_millis() as f64 / 1000.0)
}

/// 보기 범위의 모든 커밋 작성 시각을 스트림으로 읽어 집계한다. 커밋이 하나도 없는 새 저장소는 빈 차트다.
/// SSH 스트림은 선읽기 상한(16MB)을 넘으면 오류로 끝난다 — 소비가 숫자 파싱뿐이라 지연으로 넘는 일은 거의 없다(TS 는 backpressure 라 넘지 않는다)
pub async fn commit_activity(exec: &GitExecutor, repo_path: &str, req: &ActivityRequest, opts: &RunOpts, now: f64) -> Result<CommitActivity, ExecError> {
    let mut parser = TimeStreamParser::new();
    let mut stream = exec.stream(repo_path, &req.args(), opts).await?;
    let mut chunk = vec![0u8; CHUNK];
    loop {
        let n = match stream.read(&mut chunk).await {
            Ok(n) => n,
            // 토큰으로 깨어난 읽기 오류는 취소다
            Err(_) if opts.cancel.as_ref().is_some_and(tokio_util::sync::CancellationToken::is_cancelled) => return Err(ExecError::Aborted),
            Err(e) if is_read_ahead_exceeded(&e) => return Err(ExecError::Other { message: "SSH 선읽기 상한(16MB)을 넘어 활동 집계를 중단했습니다".to_string() }),
            Err(e) => return Err(ExecError::Other { message: e.to_string() }),
        };
        if n == 0 {
            match stream.finish().await {
                Ok(()) => {}
                // 커밋이 하나도 없는 새 저장소는 빈 차트로 취급한다
                Err(ExecError::Git { stderr, .. }) if is_empty_repo_error(&stderr) => {}
                Err(e) => return Err(e),
            }
            break;
        }
        parser.push(&chunk[..n]);
    }
    parser.end();
    Ok(bucketize(parser.values(), now))
}

/// 활동 결과 캐시: 범위 키 → (시그니처, 결과). TS `Map` 처럼 **삽입 순서**다 — 적중해도 순서가 바뀌지 않고, 같은 키를 다시 넣으면 맨 뒤로 가며,
/// 상한을 넘으면 가장 오래 전에 넣은 것부터 버린다(저장소·필터를 오가도 메모리가 쌓이지 않게)
#[derive(Default)]
pub struct ActivityCache {
    entries: Vec<(String, Signature, Arc<Value>)>,
}

impl ActivityCache {
    /// 시그니처가 같을 때만 적중이다
    pub fn get(&self, key: &str, sig: &Signature) -> Option<Arc<Value>> {
        self.entries.iter().find(|(k, s, _)| k == key && s == sig).map(|(_, _, v)| v.clone())
    }

    pub fn set(&mut self, key: &str, sig: Signature, value: Arc<Value>) {
        self.entries.retain(|(k, _, _)| k != key);
        self.entries.push((key.to_string(), sig, value));
        if self.entries.len() > ACTIVITY_CACHE_ENTRIES {
            self.entries.remove(0);
        }
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY_S: u32 = 86_400;
    const NOW: u32 = 1_800_000_000;

    fn parse(chunks: &[&[u8]]) -> Vec<u32> {
        let mut p = TimeStreamParser::new();
        chunks.iter().for_each(|c| p.push(c));
        p.end();
        p.values().to_vec()
    }

    fn sum(a: &CommitActivity) -> u32 {
        a.buckets.iter().map(|b| b.count).sum()
    }

    // ---- tests/activity.test.ts 의 bucketize·TimeStreamParser 를 같은 입력으로 옮긴 것 ----

    #[test]
    fn 빈_입력은_빈_결과() {
        let r = bucketize::<u32>(&[], f64::from(NOW));
        assert!(r.buckets.is_empty());
        assert_eq!(r.total, 0);
        assert_eq!(r, empty_activity());
    }

    #[test]
    fn 커밋_하나는_구간_하나() {
        let r = bucketize(&[NOW - 5 * DAY_S], f64::from(NOW));
        assert_eq!(r.buckets.len(), 1);
        assert_eq!(r.buckets[0].count, 1);
        assert_eq!(r.first, r.last);
    }

    #[test]
    fn 짧은_이력은_일_이하_단위_구간_합_total_구간은_연속() {
        let times = [NOW - 100 * DAY_S, NOW - 100 * DAY_S + 60, NOW - 50 * DAY_S, NOW - DAY_S];
        let r = bucketize(&times, f64::from(NOW));
        assert!(r.bucket_sec.0 <= f64::from(DAY_S));
        assert_eq!(sum(&r), 4);
        assert_eq!(r.total, 4);
        for w in r.buckets.windows(2) {
            assert_eq!(w[1].t.0 - w[0].t.0, r.bucket_sec.0);
        }
        assert!(r.buckets[0].t.0 <= r.first.0);
        assert!(r.buckets.last().unwrap().t.0 + r.bucket_sec.0 > r.last.0);
    }

    #[test]
    fn 수년_이력은_주_이상_단위로_묶여_구간_수가_상한_이내() {
        let times: Vec<u32> = (0..5000u32).map(|i| NOW - (f64::from(i) / 5000.0 * 8.0 * 365.0 * f64::from(DAY_S)).floor() as u32).collect();
        let r = bucketize(&times, f64::from(NOW));
        assert!(r.bucket_sec.0 >= 7.0 * f64::from(DAY_S));
        assert!(r.buckets.len() <= MAX_BUCKETS);
        assert_eq!(sum(&r), 5000);
        assert_eq!(r.last.0, f64::from(NOW));
    }

    #[test]
    fn 영_먼_미래_시각은_범위에서_빼되_total_에는_센다() {
        let r = bucketize(&[0, NOW - DAY_S, NOW + 400 * DAY_S], f64::from(NOW));
        assert_eq!(r.total, 3);
        assert_eq!(sum(&r), 1);
        assert_eq!(r.first.0, f64::from(NOW - DAY_S));
    }

    #[test]
    fn 청크_경계에_걸친_숫자를_이어_붙인다() {
        assert_eq!(parse(&[b"1700000", b"000
17000", b"00001
1
", b"42"]), vec![1_700_000_000, 1_700_000_001, 1, 42]);
        assert_eq!(parse(&[b"1
", b"

", b"2"]), vec![1, 2]);
        assert!(parse(&[b"", b"
"]).is_empty());
    }

    #[test]
    fn 큰_입력에서_버퍼가_늘어난다() {
        let mut p = TimeStreamParser::new();
        let text: String = (1..=50_000).map(|i| i.to_string()).collect::<Vec<_>>().join("
") + "
";
        p.push(text.as_bytes());
        p.end();
        assert_eq!(p.values().len(), 50_000);
        assert_eq!(p.values()[49_999], 50_000);
    }

    // ---- 이 PR 에서 더한 경계(골든도 같은 입력을 본다) ----

    #[test]
    fn 이_넘는_수는_2의_32승으로_감긴다() {
        assert_eq!(parse(&[b"4294967296
4294967297
4294967295
"]), vec![0, 1, u32::MAX]);
        // double 반올림: 2^53+1 은 2^53 으로 읽힌다(TS 와 같다)
        assert_eq!(parse(&[b"9007199254740993
"]), vec![0]);
        // 309 자리 이상은 Infinity → 0
        let huge = "9".repeat(400);
        assert_eq!(parse(&[huge.as_bytes()]), vec![0]);
    }

    #[test]
    fn 미래_시각_허용_폭은_정확히_이틀() {
        let now = 1_700_000_000.0;
        let a = bucketize(&[1_700_172_800u32, 1_700_172_801], now);
        assert_eq!((a.first, a.last), (JsNumber(1_700_172_800.0), JsNumber(1_700_172_800.0)));
        assert_eq!(a.total, 2);
    }

    // ---- git:activity 요청·캐시(채널 PR) ----

    #[test]
    fn 인자는_ts_와_바이트까지_같다() {
        // activity.ts: ['log', '--format=%at', ...scopeArgs(scope), '--']
        let strs = |v: &[&str]| v.iter().map(|s| (*s).to_string()).collect::<Vec<String>>();
        assert_eq!(ActivityRequest::parse(None).unwrap().args(), strs(&["log", "--format=%at", "--ignore-missing", "--branches", "--remotes", "--tags", "HEAD", "--"]));
        let solo = ActivityRequest::parse(Some(&serde_json::json!({ "solo": ["refs/heads/a", "refs/remotes/o/*"] }))).unwrap();
        assert_eq!(solo.args(), strs(&["log", "--format=%at", "--ignore-missing", "--glob=refs/remotes/o/*", "refs/heads/a", "--"]));
    }

    #[test]
    fn 범위_검증은_ts_scope_args_와_같다() {
        let bad = |v: serde_json::Value| ActivityRequest::parse(Some(&v)).unwrap_err().to_string();
        assert_eq!(bad(serde_json::json!(null)), "invalid scope");
        assert_eq!(bad(serde_json::json!("x")), "invalid scope");
        assert_eq!(bad(serde_json::json!({ "solo": ["-x"] })), "invalid solo");
        assert_eq!(bad(serde_json::json!({ "hide": ["a".repeat(40)] })), "invalid hide");
        assert_eq!(bad(serde_json::json!({ "includeHead": 1 })), "invalid scope");
        // 같은 범위는 항목 순서와 무관하게 같은 키다
        let k = |v: serde_json::Value| ActivityRequest::parse(Some(&v)).unwrap().scope_key().to_string();
        assert_eq!(k(serde_json::json!({ "solo": ["refs/heads/a", "refs/heads/b"] })), k(serde_json::json!({ "solo": ["refs/heads/b", "refs/heads/a"] })));
        assert_ne!(k(serde_json::json!({ "solo": ["refs/heads/a"] })), k(serde_json::json!({ "solo": ["refs/heads/b"] })));
    }

    #[test]
    fn 캐시는_시그니처가_같을_때만_적중하고_삽입_순서로_8개를_둔다() {
        let sig = |n: u8| Signature::from_parts(&[n], b"", b"");
        let val = |n: u64| Arc::new(serde_json::json!(n));
        let mut c = ActivityCache::default();
        assert!(c.is_empty());
        c.set("k0", sig(0), val(0));
        assert_eq!(c.get("k0", &sig(0)), Some(val(0)));
        assert_eq!(c.get("k0", &sig(1)), None, "시그니처가 다르면 적중이 아니다");
        // 같은 키를 다시 넣으면 교체되고 맨 뒤로 간다
        c.set("k0", sig(1), val(10));
        assert_eq!((c.len(), c.get("k0", &sig(1))), (1, Some(val(10))));
        for i in 1..ACTIVITY_CACHE_ENTRIES as u8 {
            c.set(&format!("k{i}"), sig(i), val(u64::from(i)));
        }
        assert_eq!(c.len(), ACTIVITY_CACHE_ENTRIES);
        // 적중해도 순서가 바뀌지 않는다(TS Map 과 같다) — k0 가 가장 오래된 채로 남아 아홉 번째 항목이 밀어낸다
        assert!(c.get("k0", &sig(1)).is_some());
        c.set("extra", sig(99), val(99));
        assert_eq!((c.len(), c.get("k0", &sig(1)), c.get("k1", &sig(1)).is_some()), (ACTIVITY_CACHE_ENTRIES, None, true));
        c.clear();
        assert!(c.is_empty());
    }
}
