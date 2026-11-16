//! `src/main/github/ghCli.ts` 의 `ghEnv`: gh 에 넘기는 최소 환경 — 실행 파일 탐색과 gh 설정·자격 증명 위치를 찾는 데 필요한 것만.
//! gh 를 실행하는 코드(자식 프로세스 생성)는 뒤 PR 의 몫이고, 여기는 거르는 규칙만 있다.
use std::collections::HashSet;

use crate::git::fetch::js_to_lowercase;

/// 넘기는 변수 이름(대소문자 무시)
pub const KEEP: [&str; 15] = [
    "PATH", "PATHEXT", "SystemRoot", "ComSpec", "TEMP", "TMP", "HOME", "USERPROFILE", "APPDATA", "LOCALAPPDATA", "XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_STATE_HOME", "GH_CONFIG_DIR", "GH_HOST",
];

/// 원본(`src`)의 순서를 지켜 허용 목록에 있는 변수만 남긴다. Windows 는 이름의 대소문자를 구분하지 않아 `PATH`/`Path` 가 함께 보일 수 있다 —
/// 이름(소문자)당 처음 것 하나만 넘긴다. 그 뒤에 `GH_PROMPT_DISABLED=1`·`NO_COLOR=1`(대화형 프롬프트·색 코드 차단)을 붙인다
pub fn gh_env(src: &[(String, String)]) -> Vec<(String, String)> {
    let keep: HashSet<String> = KEEP.iter().map(|k| js_to_lowercase(k)).collect();
    let mut seen: HashSet<String> = HashSet::new();
    let mut env: Vec<(String, String)> = Vec::new();
    for (k, v) in src {
        let lk = js_to_lowercase(k);
        if !keep.contains(&lk) || !seen.insert(lk) {
            continue;
        }
        env.push((k.clone(), v.clone()));
    }
    env.push(("GH_PROMPT_DISABLED".into(), "1".into()));
    env.push(("NO_COLOR".into(), "1".into()));
    env
}
