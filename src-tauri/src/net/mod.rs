//! GitHub·Jira 클라이언트가 공유하는 순수 로직(PR 1)과 HTTP 코어(PR 3). 키체인은 없다(뒤 PR).
//!
//! - [`http`]: reqwest 클라이언트를 만드는 **유일한 곳**(https 전용·프록시 무시·15초·리다이렉트 정책), 본문 상한 읽기, 취소, 오류 분류, 오류 봉투(`NetError`)
//! - 아래 나머지는 순수 로직이다(네트워크 없음).
//! - [`json`]: JS `JSON.parse` 와 같은 관대한 해석(짝 없는 서로게이트·삽입 순서·중복 키)과 JS 값 의미(`Number(문자열)`·숫자 문자열화)
//! - [`url_rules`]: WHATWG URL 판정(사이트 안 주소·공개 사진 호스트·요청 URL 조립)
//! - [`http_error`]: 응답 헤더·본문 해석(Retry-After·rate limit·Link·오류 본문 메시지·오류 분류)
//! - [`redact`]: [`redact::SecretString`] 과 오류 문구에서 비밀을 가리는 함수
//! - [`js_value`]·[`js_date`]: 응답 매퍼(PR 2)가 쓰는 JS 값 의미(`??`·`Number()`·`MapError`·`JSON.stringify` 꼴 직렬화)와 `Date.parse`·`toISOString`
//!
//! 원본은 `src/main/github/{client,validate,deviceFlow,ghCli}.ts`·`src/main/jira/{client,validate}.ts`·`src/main/git/githubCache.ts` 다.
//! 골든(`tests/golden/net/*.json`, TS 가 기준)과 `tests/net.rs` 가 같은 결과를 내는지 본다.
//!
//! 이식 규칙은 `git::fetch` 와 같다: 공백은 [`crate::git::scope::is_js_space`], 길이는 UTF-16 단위, 소문자화는 Node 표(`js_to_lowercase`).
//! "Rust 가 허용하면 TS 도 허용한다"가 깨지지 않는 방향으로만 다르게 만든다(다른 곳은 각 함수에 적었다).
pub mod http;
pub mod http_error;
pub mod js_date;
pub mod js_value;
pub mod json;
pub mod redact;
pub mod url_rules;

/// TS 에서 손으로 옮긴 정규식의 소스 조각(이름, 조각). 골든 `validate.json` 의 handWritten 과 글자 그대로 같아야 한다 —
/// TS 소스가 바뀌면 `tests/goldenNet.test.ts` 가 실패하고, 이 목록과 구현(손 판정)을 함께 다시 본다
pub const HAND_WRITTEN: &[(&str, &str)] = &[
    ("PUBLIC_AVATAR_HOST_RE", r"/^(?:secure\.gravatar\.com|i[0-3]\.wp\.com|[a-z0-9-]+(?:\.[a-z0-9-]+)*\.atl-paas\.net)$/"),
    ("IN_SITE_ENCODED", r"/%(?:2f|5c|2e)/i"),
    ("LINK_NEXT", r#"/<[^>]*>\s*;\s*rel="?next"?/i"#),
    ("CONTROL_RE", r"/[\u0000-\u001f\u007f]/"),
    ("CONTROL_RE(jira)", r"/[\u0000-\u001f\u007f]/"),
    ("PAT_CHARS", r"/^[\x21-\x7e]+$/"),
    ("TOKEN_CHARS", r"/^[\x21-\x7e]+$/"),
    ("SHA_RE", r"/^(?:[0-9a-f]{40}|[0-9a-f]{64})$/i"),
    ("KEY_RE", r"/^[A-Z][A-Z0-9_]{1,19}-\d{1,9}$/"),
    ("CURSOR_RE", r"/^[!-~]{1,2000}$/"),
    ("TRANSITION_ID", r"/^\d{1,10}$/"),
    ("USER_ID_RE", r"/^[A-Za-z0-9._@:+-]{1,128}$/"),
    ("DATE_RE", r"/^(\d{4})-(\d{2})-(\d{2})$/"),
    ("JIRA_BODY_SPACES", r"/[\s\u0000-\u001f\u007f]+/g"),
    ("USABLE_TOKEN", r"/[\s\0]/"),
    ("TRAILING_SLASH", r"/\/+$/"),
    ("REPOSITORY_URL", r"/^https:\/\/api\.github\.com\/repos\/([^/]+)\/([^/]+)$/"),
    ("CUSTOM_FIELD_RE", r"/^customfield_\d+$/"),
    ("DATE_ONLY_RE", r"/^\d{4}-\d{2}-\d{2}/"),
    ("DEV_INSTANCE_RE", r"/^[A-Za-z0-9_.-]{1,40}$/"),
    ("DEV_HTTPS_URL", r"/^https:\/\/[^\s]+$/"),
    ("DEV_COMMIT_SHA", r"/^[0-9a-f]{7,64}$/i"),
    ("DEV_ISSUE_ID", r"/^\d{1,18}$/"),
    ("AVATAR_SCHEME", r"/^https?:\/\//i"),
    ("CONTROL_RE(jira api)", r"/[\u0000-\u001f\u007f]/"),
];
