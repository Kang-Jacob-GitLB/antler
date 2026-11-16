//! Launchpad(`src/main/github/launchpad.ts`)의 순수 부분: 섹션 검색어·검색 결과의 저장소 식별·항목 변환·병합. 검색 요청·원격 읽기·시간 제한은 서비스 계층(뒤 PR)의 몫이다.
use std::collections::{HashMap, HashSet};

use super::api_map::{map_issue, map_search_pull};
use crate::git::fetch::js_to_lowercase;
use crate::net::js_value::{field, items, nn, num, obj, push_some, template_string, to_number, truthy, MapError, MapResult};
use crate::net::json::Json;

/// 섹션 하나에서 읽는 쪽 수 상한(검색 API 가 한 검색에 주는 최대 1000건의 절반)
pub const MAX_PAGES_PER_SECTION: usize = 5;
/// GitHub 검색이 한 검색어에 돌려줄 수 있는 최대 건수
pub const SEARCH_RESULT_CAP: f64 = 1000.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SectionDef {
    pub section: &'static str,
    /// `pull` 또는 `issue`
    pub kind: &'static str,
    /// 검색어. 열린 항목만, 보관된 저장소는 제외
    pub q: &'static str,
}

pub const SECTIONS: [SectionDef; 3] = [
    SectionDef { section: "authored", kind: "pull", q: "is:pr is:open author:@me archived:false" },
    SectionDef { section: "review", kind: "pull", q: "is:pr is:open review-requested:@me archived:false" },
    SectionDef { section: "issues", kind: "issue", q: "is:issue is:open assignee:@me archived:false" },
];

/// JS `decodeURIComponent`. 잘못된 `%` 이스케이프나 UTF-8 이면 `None`(TS 는 URIError)
pub fn decode_uri_component(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let h = b.get(i + 1..i + 3)?;
            let hex = |c: u8| (c as char).to_digit(16);
            out.push((hex(h[0])? * 16 + hex(h[1])?) as u8);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// `slugFromRepositoryUrl`: `https://api.github.com/repos/{owner}/{repo}` 에서 (owner, repo). 형식이 다르거나 해석할 수 없으면 `None`
pub fn slug_from_repository_url(url: Option<&Json>) -> Option<(String, String)> {
    let Some(Json::Str(u)) = url else { return None };
    let rest = u.strip_prefix("https://api.github.com/repos/")?;
    let (owner, repo) = rest.split_once('/')?;
    if owner.is_empty() || repo.is_empty() || repo.contains('/') {
        return None;
    }
    Some((decode_uri_component(owner)?, decode_uri_component(repo)?))
}

/// `slugKey`: 소문자 `owner/repo`
pub fn slug_key(owner: &str, repo: &str) -> String {
    js_to_lowercase(&format!("{owner}/{repo}"))
}

/// 검색 한 쪽의 `(항목, incomplete)`. incomplete 는 `incomplete_results` 이거나 `total_count` 가 1000 을 넘을 때
pub fn search_page(data: &Json) -> MapResult<(&[Json], bool)> {
    if matches!(data, Json::Null) {
        return Err(MapError);
    }
    let list = items(field(Some(data), "items"))?;
    let incomplete = truthy(field(Some(data), "incomplete_results")) || to_number(Some(&nn(field(Some(data), "total_count"), num(0.0)))) > SEARCH_RESULT_CAP;
    Ok((list, incomplete))
}

/// `toItem`: 섹션 종류에 맞춰 검색 결과 한 건을 Launchpad 항목으로
pub fn launchpad_item(def: &SectionDef, raw: &Json, repo_ids: &[String], owner: &str, repo: &str) -> MapResult<Json> {
    let base = |draft: Json, src: &Json| -> MapResult<Json> {
        let get = |k: &str| field(Some(src), k).cloned();
        let mut o: Vec<(&str, Json)> = vec![
            ("section", Json::Str(def.section.into())),
            ("kind", Json::Str(def.kind.into())),
            ("repoIds", Json::Arr(repo_ids.iter().cloned().map(Json::Str).collect())),
            ("owner", Json::Str(owner.into())),
            ("repo", Json::Str(repo.into())),
        ];
        for k in ["number", "title", "state"] {
            push_some(&mut o, k, get(k));
        }
        o.push(("draft", draft));
        for k in ["author", "labels", "comments", "updatedAt", "createdAt", "htmlUrl"] {
            push_some(&mut o, k, get(k));
        }
        Ok(obj(o))
    };
    if def.kind == "pull" {
        let p = map_search_pull(raw)?;
        let draft = field(Some(&p), "draft").cloned().unwrap_or(Json::Bool(false));
        base(draft, &p)
    } else {
        let i = map_issue(raw)?;
        base(Json::Bool(false), &i)
    }
}

/// 병합 입력: 섹션 정의와 그 섹션의 검색 결과 원본
pub struct SectionHits<'a> {
    pub def: SectionDef,
    pub items: &'a [Json],
}

/// 섹션 검색 결과 + 저장소 remote 매칭(소문자 `owner/repo` → 앱 저장소 ID, 등록 순서)으로 항목을 모은다(`fetchLaunchpad` 의 병합).
/// 등록된 저장소에 속한 것만 남기고, 섹션·저장소·번호가 같으면 먼저 온 것 하나만 두며, updatedAt 내림차순(같으면 먼저 온 순)이다
pub fn merge_launchpad(sections: &[SectionHits<'_>], by_key: &HashMap<String, Vec<String>>) -> MapResult<Vec<Json>> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut out: Vec<Json> = Vec::new();
    for hits in sections {
        for raw in hits.items {
            if matches!(raw, Json::Null) {
                return Err(MapError);
            }
            let Some((owner, repo)) = slug_from_repository_url(field(Some(raw), "repository_url")) else { continue };
            let key = slug_key(&owner, &repo);
            let Some(ids) = by_key.get(&key) else { continue };
            let dedup = format!("{}:{key}#{}", hits.def.section, template_string(field(Some(raw), "number")));
            if !seen.insert(dedup) {
                continue;
            }
            out.push(launchpad_item(&hits.def, raw, ids, &owner, &repo)?);
        }
    }
    out.sort_by(|a, b| {
        let at = |x: &Json| x.get("updatedAt").and_then(Json::as_f64).unwrap_or(0.0);
        at(b).total_cmp(&at(a))
    });
    Ok(out)
}
