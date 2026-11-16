//! 비밀 값 보관(`SecretString`)과 오류 문구·로그에서 비밀 가리기.
//!
//! `src/main/git/githubCache.ts` 의 `usableToken`·`redactToken` 이식과, Rust 전용 [`redact_secrets`](envelope 마지막 단계)다.
use base64::engine::general_purpose::STANDARD;
use base64::Engine;

use crate::git::commit::utf16_len;
use crate::git::scope::is_js_space;

/// 토큰·비밀번호 같은 비밀. **`Display`·`Serialize` 가 없고 `Debug` 는 `***` 다** — 실수로 포맷·직렬화하면 컴파일이 안 되거나 가려진 값이 나온다.
/// 값은 [`SecretString::expose`] 로만 꺼낸다(요청 헤더를 만드는 곳). 새 `Display`/`Serialize` 구현을 추가하지 않는다(소스 검사 테스트가 막는다).
#[derive(Clone)]
pub struct SecretString(String);

impl SecretString {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// 비밀 원문. 호출하는 곳이 헤더·환경 변수·키체인 같은 의도된 곳뿐이어야 한다
    pub fn expose(&self) -> &str {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl std::fmt::Debug for SecretString {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("***")
    }
}

/// 환경 변수·헤더에 넣어도 되는 토큰인가(개행 등이 든 값은 헤더를 깨뜨린다): 4096 UTF-16 단위 이하, 공백(JS `\s`)·NUL 없음
pub fn usable_token(token: &str) -> bool {
    !token.is_empty() && utf16_len(token) <= 4096 && !token.chars().any(|c| is_js_space(c) || c == '\0')
}

/// `x-access-token:<토큰>` 의 base64(git 의 Basic 인증 헤더 값)
pub fn git_basic_token(token: &str) -> String {
    STANDARD.encode(format!("x-access-token:{token}"))
}

/// TS `redactToken`: 토큰과 그 base64 형태를 `***` 로 바꾼다(순서대로 한 번씩). 쓸 수 없는 토큰이면 그대로 둔다
pub fn redact_token(text: &str, token: Option<&str>) -> String {
    let Some(token) = token.filter(|t| usable_token(t)) else { return text.to_string() };
    text.replace(token, "***").replace(&git_basic_token(token), "***")
}

/// 오류 문구·로그에서 비밀을 가린다(Rust 전용 — TS 에는 같은 단계가 없다). 빈 비밀은 건너뛰고(빈 문자열 치환은 글자 사이마다 끼워 넣는다)
/// 긴 것부터 바꿔, 짧은 비밀이 긴 비밀의 일부여도 긴 쪽이 통째로 가려지게 한다
pub fn redact_secrets(text: &str, secrets: &[&str]) -> String {
    let mut list: Vec<&str> = secrets.iter().copied().filter(|s| !s.is_empty()).collect();
    list.sort_by_key(|s| std::cmp::Reverse(s.len()));
    list.dedup();
    let mut out = text.to_string();
    for s in list {
        out = out.replace(s, "***");
    }
    out
}

/// GitHub 요청에서 오류 문구로 샐 수 있는 모양들: 토큰과 `x-access-token:<토큰>` 의 base64
pub fn github_secrets(token: &SecretString) -> Vec<String> {
    vec![token.expose().to_string(), git_basic_token(token.expose())]
}

/// Jira Basic 인증에서 샐 수 있는 모양들: 토큰과 `<이메일>:<토큰>` 의 base64(Data Center 는 이메일이 비어 있어 토큰만)
pub fn jira_secrets(email: &str, token: &SecretString) -> Vec<String> {
    let mut v = vec![token.expose().to_string()];
    if !email.is_empty() {
        v.push(STANDARD.encode(format!("{email}:{}", token.expose())));
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_는_값을_드러내지_않는다() {
        let s = SecretString::new("ghp_secret");
        assert_eq!(format!("{s:?}"), "***");
        assert_eq!(format!("{:?}", Some(&s)), "Some(***)");
        assert_eq!(s.expose(), "ghp_secret");
    }

    #[test]
    fn 빈_비밀은_건너뛰고_긴_것부터_가린다() {
        assert_eq!(redact_secrets("abc", &[""]), "abc");
        assert_eq!(redact_secrets("tok-long tok", &["tok", "tok-long"]), "*** ***");
    }

    #[test]
    fn 토큰이_든_base64_도_가린다() {
        let t = SecretString::new("ghp_abc");
        let b64 = git_basic_token(t.expose());
        let all = github_secrets(&t);
        let refs: Vec<&str> = all.iter().map(String::as_str).collect();
        assert_eq!(redact_secrets(&format!("x {} y {b64}", t.expose()), &refs), "x *** y ***");
    }
}
