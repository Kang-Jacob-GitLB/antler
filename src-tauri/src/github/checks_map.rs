//! GitHub Actions(체크) 응답 변환(`src/main/github/checks.ts` 의 순수 부분). 캐시(`CheckStateCache`)·일괄 조회 흐름은 서비스 계층(뒤 PR)의 몫이다.
use super::api_map::{check_run_state, commit_status_state};
use crate::net::js_date::to_epoch;
use crate::net::js_value::{field, field_strict, items, nn, num, obj, push_some, s, truthy, MapError, MapResult};
use crate::net::json::Json;

/// 한 번의 GraphQL 호출에 싣는 커밋 수
pub const CHECKS_BATCH: usize = 50;

/// `rollupState`: GraphQL StatusState → 공유 상태
pub fn rollup_state(state: Option<&Json>) -> &'static str {
    match state {
        Some(Json::Str(t)) => match t.as_str() {
            "SUCCESS" => "success",
            "FAILURE" | "ERROR" => "failure",
            "PENDING" | "EXPECTED" => "pending",
            _ => "none",
        },
        _ => "none",
    }
}

/// `rollupQuery`: 별칭(c0…)별로 `object(oid:)` 를 묻는 쿼리. sha 는 변수로 넘겨 쿼리 문자열에 끼어들지 않는다
pub fn rollup_query(n: usize) -> String {
    let vars: Vec<String> = (0..n).map(|i| format!("$o{i}:GitObjectID!")).collect();
    let fields: Vec<String> = (0..n).map(|i| format!("c{i}:object(oid:$o{i}){{...on Commit{{statusCheckRollup{{state}}}}}}")).collect();
    format!("query($owner:String!,$name:String!,{}){{repository(owner:$owner,name:$name){{{}}}}}", vars.join(","), fields.join(" "))
}

/// 일괄 조회 응답 → 별칭 순서대로의 상태. 저장소를 볼 수 없으면(`repository` 가 null) `None`(그 sha 들은 결과에서 빠진다)
pub fn rollup_states(data: &Json, n: usize) -> MapResult<Option<Vec<&'static str>>> {
    let repo = field_strict(Some(data), "repository")?;
    if !truthy(repo) {
        return Ok(None);
    }
    Ok(Some((0..n).map(|k| rollup_state(field(field(field(repo, &format!("c{k}")), "statusCheckRollup"), "state"))).collect()))
}

fn non_null(v: &Json) -> MapResult<()> {
    if matches!(v, Json::Null) {
        Err(MapError)
    } else {
        Ok(())
    }
}

/// `durationOf`: 시작·종료가 모두 있고 종료가 같거나 늦을 때만 초 단위 길이
fn duration_of(start: Option<&Json>, end: Option<&Json>) -> Option<f64> {
    let (a, b) = (to_epoch(start), to_epoch(end));
    (a != 0.0 && b != 0.0 && b >= a).then_some(b - a)
}

fn conclusion_if_truthy(v: Option<&Json>) -> Option<Json> {
    if truthy(v) {
        v.cloned()
    } else {
        None
    }
}

/// `fetchCheckDetails`: check-run(소요 시간·결론 포함) + legacy status
pub fn check_details(runs_data: &Json, status_data: &Json) -> MapResult<Vec<Json>> {
    let mut out = Vec::new();
    for run in items(field_strict(Some(runs_data), "check_runs")?)? {
        non_null(run)?;
        let mut o = vec![("name", nn(field(Some(run), "name"), s(""))), ("state", s(check_run_state(field(Some(run), "status"), field(Some(run), "conclusion"))))];
        push_some(&mut o, "conclusion", conclusion_if_truthy(field(Some(run), "conclusion")));
        push_some(&mut o, "durationSec", duration_of(field(Some(run), "started_at"), field(Some(run), "completed_at")).map(num));
        push_some(&mut o, "url", conclusion_if_truthy(field(Some(run), "html_url")));
        out.push(obj(o));
    }
    for st in items(field_strict(Some(status_data), "statuses")?)? {
        non_null(st)?;
        let mut o = vec![("name", nn(field(Some(st), "context"), s(""))), ("state", s(commit_status_state(field(Some(st), "state"))))];
        push_some(&mut o, "url", conclusion_if_truthy(field(Some(st), "target_url")));
        out.push(obj(o));
    }
    Ok(out)
}

/// `mapRun`: 워크플로 실행 한 건
pub fn map_run(x: &Json) -> MapResult<Json> {
    non_null(x)?;
    let mut o: Vec<(&str, Json)> = Vec::new();
    push_some(&mut o, "id", field(Some(x), "id").cloned());
    o.push(("name", nn(field(Some(x), "name"), s(""))));
    o.push(("title", nn(field(Some(x), "display_title"), s(""))));
    o.push(("branch", nn(field(Some(x), "head_branch"), s(""))));
    o.push(("event", nn(field(Some(x), "event"), s(""))));
    o.push(("state", s(check_run_state(field(Some(x), "status"), field(Some(x), "conclusion")))));
    o.push(("status", nn(field(Some(x), "status"), s(""))));
    push_some(&mut o, "conclusion", conclusion_if_truthy(field(Some(x), "conclusion")));
    o.push(("number", nn(field(Some(x), "run_number"), num(0.0))));
    o.push(("sha", nn(field(Some(x), "head_sha"), s(""))));
    o.push(("createdAt", num(to_epoch(field(Some(x), "created_at")))));
    o.push(("updatedAt", num(to_epoch(field(Some(x), "updated_at")))));
    o.push(("htmlUrl", nn(field(Some(x), "html_url"), s(""))));
    Ok(obj(o))
}

/// `listRuns` 한 쪽의 항목
pub fn runs_page(data: &Json) -> MapResult<Vec<Json>> {
    items(field_strict(Some(data), "workflow_runs")?)?.iter().map(map_run).collect()
}

/// `listJobs` 의 항목
pub fn jobs(data: &Json) -> MapResult<Vec<Json>> {
    let mut out = Vec::new();
    for j in items(field_strict(Some(data), "jobs")?)? {
        non_null(j)?;
        let mut o: Vec<(&str, Json)> = Vec::new();
        push_some(&mut o, "id", field(Some(j), "id").cloned());
        o.push(("name", nn(field(Some(j), "name"), s(""))));
        o.push(("state", s(check_run_state(field(Some(j), "status"), field(Some(j), "conclusion")))));
        o.push(("status", nn(field(Some(j), "status"), s(""))));
        push_some(&mut o, "conclusion", conclusion_if_truthy(field(Some(j), "conclusion")));
        push_some(&mut o, "durationSec", duration_of(field(Some(j), "started_at"), field(Some(j), "completed_at")).map(num));
        o.push(("htmlUrl", nn(field(Some(j), "html_url"), s(""))));
        out.push(obj(o));
    }
    Ok(out)
}
