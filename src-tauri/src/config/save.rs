//! 설정 파일 쓰기(`ConfigStore.save`): 직렬화 + 임시 파일 → rename 으로 원자적으로 쓴다.
use std::ffi::OsString;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use crate::net::js_value::stringify_pretty;
use crate::net::json::{Json, MAX_DEPTH};

use super::model::strip_undefined;

/// 파일 내용: `JSON.stringify(config, null, 2)`(UTF-8, BOM 없음, LF, 끝 줄바꿈 없음)
pub fn to_file_text(config: &Json) -> String {
    stringify_pretty(&strip_undefined(config))
}

/// 값 `v` 를 루트로 봤을 때 `parse`(`net/json.rs`) 가 받아들이는 깊이인가. `budget` 은 `v` 아래로 더 내려갈 수 있는 겹 수다
/// (루트 객체 0, 그 안의 값 1 … — 비어 있지 않은 컨테이너의 자식은 한 겹 아래라 예산이 0 이면 거부). 읽기(로드)가 [`MAX_DEPTH`] 초과를 손상으로 보므로 쓰기도 같은 선을 지킨다
pub fn within_depth(v: &Json, budget: usize) -> bool {
    match v {
        Json::Arr(a) => a.is_empty() || (budget > 0 && a.iter().all(|x| within_depth(x, budget - 1))),
        Json::Obj(o) => o.is_empty() || (budget > 0 && o.iter().all(|(_, x)| within_depth(x, budget - 1))),
        _ => true,
    }
}

/// 저장할 문서 전체가 읽을 수 있는 깊이인가(쓰기 직전 방어 단언)
pub fn check_document_depth(doc: &Json) -> Result<(), String> {
    if within_depth(doc, MAX_DEPTH) {
        Ok(())
    } else {
        Err(format!("설정이 너무 깊게 중첩돼 저장할 수 없습니다(최대 {MAX_DEPTH}겹)"))
    }
}

/// `<file>` 에 덧붙인 경로(`antler.config.json` → `antler.config.json.tmp`)
pub fn with_suffix(file: &Path, suffix: &str) -> PathBuf {
    let mut s: OsString = file.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}

/// 재시도할 rename 실패인가. Windows 에서 백신·인덱서가 파일을 잠깐 잡으면 `ERROR_ACCESS_DENIED(5)`·`ERROR_SHARING_VIOLATION(32)` 로 실패한다
/// (TS 는 Node 가 EPERM 으로 옮긴 것만 다시 한다 — 여기는 범위가 조금 넓지만 같은 상황이고, 재시도가 보안을 낮추지 않는다)
fn is_transient(e: &io::Error) -> bool {
    if cfg!(windows) {
        matches!(e.raw_os_error(), Some(5 | 32))
    } else {
        e.kind() == io::ErrorKind::PermissionDenied
    }
}

/// 임시 파일에 쓰고 `sync_all`(TS 에는 없다 — 전원이 나가도 빈 파일이 남지 않게) 한 뒤 rename 으로 덮어쓴다. 최대 5회, 사이마다 30·60·90·120 ms 쉰다.
pub fn write_atomic(file: &Path, text: &str) -> io::Result<()> {
    if let Some(dir) = file.parent().filter(|d| !d.as_os_str().is_empty()) {
        fs::create_dir_all(dir)?;
    }
    let tmp = with_suffix(file, ".tmp");
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(text.as_bytes())?;
        f.sync_all()?;
    }
    for attempt in 0u64.. {
        match fs::rename(&tmp, file) {
            Ok(()) => return Ok(()),
            Err(e) if attempt >= 4 || !is_transient(&e) => return Err(e),
            Err(_) => thread::sleep(Duration::from_millis(30 * (attempt + 1))),
        }
    }
    unreachable!("재시도 루프는 반환으로만 끝난다")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::json::parse;

    fn nested(n: usize) -> String {
        format!("{}1{}", "[".repeat(n), "]".repeat(n))
    }

    #[test]
    fn 깊이_경계는_해석기와_같다() {
        // 배열 n 겹 + 스칼라는 스칼라가 깊이 n 이다. 해석기는 64 까지 받고 65 부터 거부한다
        let deepest = parse(&nested(MAX_DEPTH)).expect("64 겹은 받는다");
        assert!(check_document_depth(&deepest).is_ok());
        let too_deep = Json::Arr(vec![deepest]);
        assert!(check_document_depth(&too_deep).is_err());
        assert!(parse(&nested(MAX_DEPTH + 1)).is_err());
    }

    #[test]
    fn 파일_형식은_두_칸_들여쓰기이고_undefined_자리는_쓰지_않는다() {
        let mut j = parse(r#"{"a":[],"b":{},"c":[1,{"d":null}],"2":0}"#).unwrap();
        if let Json::Obj(o) = &mut j {
            o.push(("gone".into(), super::super::model::UNDEFINED));
        }
        assert_eq!(to_file_text(&j), "{\n  \"2\": 0,\n  \"a\": [],\n  \"b\": {},\n  \"c\": [\n    1,\n    {\n      \"d\": null\n    }\n  ]\n}");
    }
}
