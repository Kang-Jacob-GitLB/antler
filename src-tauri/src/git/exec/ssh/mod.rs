//! SSH 실행기(`src/main/executors/ssh`)의 이식 — Phase 5. PR 1 은 순수 로직(소켓·russh 없음), PR 2 는 russh 실행기(`connection`·`auth`·`executor`). 아직 앱에서 호출하지 않는다.
//!
//! - [`config`]: `~/.ssh/config` 파서와 호스트 해석(`config.ts`). ProxyCommand·해당 Match 는 `unsupported` 로 표시해 연결 쪽이 거부하게 한다.
//! - [`known_hosts`]: known_hosts 검증(`knownHosts.ts`). 키를 자동으로 등록하지 않는다(TOFU 없음) — **쓰기 기능이 없다.**
//! - [`quote`]: 원격 명령 인용(`quote.ts`). 원격 로그인 셸에 문자열로 넘기므로 인용이 곧 주입 방어선이다 — 바이트 단위로 TS 와 같아야 한다.
//!   환경변수는 닫힌 타입(`FetchEnv`)으로만 받는다.
//! - [`SshExecutor`]: russh 로 원격 git 을 실행한다(`connection.rs` 의 모듈 문서에 방어 목록). 연결 수명·인증·원격 셸 확인.
//! - [`win32_path`]: Node `path.win32` 의 작은 이식(설정 값의 경로 정규화).
//!
//! 골든: `tests/golden/ssh/{config,knownHosts,quote}.json`·`tests/golden/exec/env.json`(`ssh` 절)을 `src-tauri/tests/ssh_golden.rs` 가 비교한다.
//!
//! 이식 규칙(어기면 TS 와 판정이 달라진다):
//! - 정규식 크레이트를 쓰지 않는다. JS 정규식의 의미는 손으로 옮겼고 어디서 다른지는 각 함수에 적었다.
//! - **와일드카드 매칭(`glob_match`)은 UTF-16 코드 단위로 한다.** JS 의 `.`(`?`)는 코드 단위 하나라 이모지 하나에 `?` 가 둘 필요하고, `.`(`*`·`?`)는 줄 끝 문자
//!   (`\n`·`\r`·U+2028·U+2029)를 맞추지 않는다.
//! - **대소문자 무시(`i` 플래그)는 ASCII 만 접는다.** TS 는 유니코드가 아닌 정규식의 `i` 라 `toUpperCase` 로 정규화한다(`é` 와 `É` 가 같다).
//!   Rust 는 Node 의 대문자 표가 없어 접지 않는다. **비교되는 이름 쪽을 ASCII 로 막아 닫는다**: JS 비유니코드 `i` 의 Canonicalize 는 비 ASCII 를 ASCII 로 접지 않으므로
//!   이름이 ASCII 면 어떤 패턴이든 ASCII 접기 결과가 TS 와 같다. 관문은 `config::resolve_host`(별칭·HostName·HostKeyAlias 가 비 ASCII 면 `unsupported`)와
//!   `known_hosts::check_host_key`(비 ASCII 호스트는 `Unsupported`)다 — `to_lowercase` 로 바꾸지 않는다(켈빈 기호 등으로 새 차이가 생긴다). docs/tauri.md "TS 와 다른 점".

mod auth;
pub mod config;
mod connection;
mod executor;
pub mod known_hosts;
pub mod quote;
pub mod win32_path;

pub use auth::AgentSetting;
pub use connection::{EnvFn, SshOptions, Timeouts, DEFAULT_MAX_CHANNELS, SHELL_PROBE};
pub use executor::SshExecutor;

/// JS `.` 가 맞추는 코드 단위인가(줄 끝 문자 제외)
fn dot_matches(unit: u16) -> bool {
    !matches!(unit, 0x0A | 0x0D | 0x2028 | 0x2029)
}

/// ASCII 대소문자만 접은 코드 단위
fn fold(unit: u16) -> u16 {
    if (u16::from(b'A')..=u16::from(b'Z')).contains(&unit) {
        unit + 32
    } else {
        unit
    }
}

/// `globToRegExp(pattern).test(text)` — `*` 는 `.*`, `?` 는 `.`, 나머지는 글자 그대로이고 대소문자는 ASCII 만 무시한다(위 "이식 규칙").
/// 정규식 `^…$` 라 전체가 맞아야 한다.
pub fn glob_match(pattern: &str, text: &str) -> bool {
    let p: Vec<u16> = pattern.encode_utf16().collect();
    let t: Vec<u16> = text.encode_utf16().collect();
    let (star, ques) = (u16::from(b'*'), u16::from(b'?'));
    let (mut pi, mut ti) = (0usize, 0usize);
    // 마지막 `*` 다음 패턴 위치와 그 `*` 가 지금까지 먹은 텍스트 끝
    let mut star_at: Option<(usize, usize)> = None;
    while ti < t.len() {
        if pi < p.len() && p[pi] == star {
            pi += 1;
            star_at = Some((pi, ti));
            continue;
        }
        if pi < p.len() && ((p[pi] == ques && dot_matches(t[ti])) || (p[pi] != ques && fold(p[pi]) == fold(t[ti]))) {
            pi += 1;
            ti += 1;
            continue;
        }
        match star_at {
            // `*` 가 한 단위 더 먹는다 — 줄 끝 문자는 먹지 못한다
            Some((sp, st)) if dot_matches(t[st]) => {
                star_at = Some((sp, st + 1));
                pi = sp;
                ti = st + 1;
            }
            _ => return false,
        }
    }
    while pi < p.len() && p[pi] == star {
        pi += 1;
    }
    pi == p.len()
}

/// JS `split(/\s+/)` — 앞뒤가 이미 `trim` 된 한 줄을 공백 덩어리로 쪼갠다
pub(crate) fn split_ws(line: &str) -> Vec<&str> {
    line.split(crate::git::scope::is_js_space).filter(|s| !s.is_empty()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glob_는_정규식_의미와_같다() {
        assert!(glob_match("web-*", "web-1"));
        assert!(glob_match("node?", "node7"));
        assert!(!glob_match("node?", "node77"));
        assert!(glob_match("*", ""));
        assert!(glob_match("A.B", "a.b"));
        assert!(!glob_match("a.b", "axb"), "점은 글자 그대로다");
        assert!(glob_match("*.corp.local", "a.corp.local"));
        assert!(glob_match("a*b*c", "aXXbYYc"));
        assert!(!glob_match("a*b*c", "aXXbYY"));
        // 이모지는 코드 단위 둘이라 ? 하나로 맞지 않고, 줄 끝 문자는 * 도 ? 도 맞추지 않는다
        assert!(!glob_match("?", "😀"));
        assert!(glob_match("??", "😀"));
        assert!(!glob_match("a*b", "a\nb"));
        assert!(!glob_match("a?b", "a\u{2028}b"));
        // 비 ASCII 는 접지 않는다(TS 는 접는다 — 알려진 차이)
        assert!(!glob_match("É", "é"));
        // ſ·K(켈빈)는 ASCII 와 같지 않다(TS 도 같다)
        assert!(!glob_match("s", "\u{17f}"));
    }

    #[test]
    fn split_ws_는_js_공백을_쓴다() {
        assert_eq!(split_ws("a \t b\u{a0}c"), ["a", "b", "c"]);
        assert_eq!(split_ws("a\u{85}b"), ["a\u{85}b"], "U+0085 는 JS 공백이 아니다");
    }
}
