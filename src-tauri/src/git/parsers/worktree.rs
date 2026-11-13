//! `src/main/git/parsers/worktree.ts`

use serde::Serialize;

/// `git worktree list --porcelain -z` 인자. `-z` 는 git 2.36+ 가 필요하다
pub const WORKTREE_ARGS: [&str; 4] = ["worktree", "list", "--porcelain", "-z"];

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Worktree {
    pub path: String,
    /// HEAD 커밋. 아직 커밋이 없으면 null
    pub head: Option<String>,
    /// 체크아웃된 브랜치 짧은 이름. detached 면 null
    pub branch: Option<String>,
    pub detached: bool,
    pub bare: bool,
    pub locked: bool,
    pub prunable: bool,
    /// 목록 첫 항목(주 작업 트리)
    pub is_main: bool,
}

/// `--porcelain -z` 출력을 해석한다. 속성은 NUL 로 끝나고 레코드 사이에는 빈 필드(NUL NUL)가 들어간다.
pub fn parse_worktrees(buf: &[u8]) -> Vec<Worktree> {
    let mut out: Vec<Worktree> = Vec::new();
    let mut cur: Option<Worktree> = None;

    for field in String::from_utf8_lossy(buf).split('\0') {
        if field.is_empty() {
            out.extend(cur.take());
            continue;
        }
        let (key, value) = field.split_once(' ').unwrap_or((field, ""));
        if key == "worktree" {
            out.extend(cur.take());
            cur = Some(Worktree {
                path: value.to_string(),
                head: None,
                branch: None,
                detached: false,
                bare: false,
                locked: false,
                prunable: false,
                is_main: out.is_empty(),
            });
            continue;
        }
        let Some(w) = cur.as_mut() else { continue };
        match key {
            // 커밋이 없는 새 저장소는 전부 0 인 해시를 돌려준다
            "HEAD" => w.head = if !value.is_empty() && value.bytes().all(|b| b == b'0') { None } else { Some(value.to_string()) },
            "branch" => w.branch = Some(value.strip_prefix("refs/heads/").unwrap_or(value).to_string()),
            "detached" => w.detached = true,
            "bare" => w.bare = true,
            "locked" => w.locked = true,
            "prunable" => w.prunable = true,
            _ => {}
        }
    }
    out.extend(cur);
    out
}
