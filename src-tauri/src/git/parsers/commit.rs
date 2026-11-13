//! `src/main/git/parsers/commit.ts` — `git log -1 -z` 의 커밋 메타데이터 출력.

use serde::Serialize;

use super::{js_number, js_trim, JsNumber};

/// 필드 구분자는 NUL(%x00) — 본문(%b)에 들어갈 수 없는 유일한 바이트다. 본문은 마지막에 둔다.
const FIELD_COUNT: usize = 10;

/// `git log -1` 에 붙일 출력 형식 인자. `-z` 라 레코드 끝에 NUL 이 하나 더 붙는다.
pub const COMMIT_FORMAT_ARGS: [&str; 3] = ["-z", "--no-notes", "--format=%H%x00%P%x00%an%x00%ae%x00%at%x00%cn%x00%ce%x00%ct%x00%s%x00%b"];

/// 변경 파일 목록(files·통계)은 별도 명령에서 채운다
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitMeta {
    pub hash: String,
    pub parents: Vec<String>,
    pub author_name: String,
    pub author_email: String,
    /// epoch seconds
    pub author_date: JsNumber,
    pub committer_name: String,
    pub committer_email: String,
    /// epoch seconds
    pub commit_date: JsNumber,
    pub subject: String,
    /// 제목 줄을 뺀 본문(앞뒤 공백 제거)
    pub body: String,
}

/// 필드가 모자라면 TS 가 던지는 것과 같은 메시지의 `Err`
pub fn parse_commit_meta(buf: &[u8]) -> Result<CommitMeta, String> {
    let text = String::from_utf8_lossy(buf);
    let f: Vec<&str> = text.split('\0').collect();
    // 본문은 마지막 필드 + 종결 NUL 뒤의 빈 조각이므로 최소 FIELD_COUNT 개가 있어야 한다
    if f.len() < FIELD_COUNT {
        return Err("unexpected commit metadata output".to_string());
    }
    Ok(CommitMeta {
        hash: f[0].to_string(),
        parents: if f[1].is_empty() { Vec::new() } else { f[1].split(' ').map(str::to_string).collect() },
        author_name: f[2].to_string(),
        author_email: f[3].to_string(),
        author_date: js_number(f[4]),
        committer_name: f[5].to_string(),
        committer_email: f[6].to_string(),
        commit_date: js_number(f[7]),
        subject: f[8].to_string(),
        body: js_trim(f[9]).to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn too_few_fields_is_an_error() {
        assert_eq!(parse_commit_meta(b"a\0b").unwrap_err(), "unexpected commit metadata output");
        assert!(parse_commit_meta(b"").is_err());
    }
}
