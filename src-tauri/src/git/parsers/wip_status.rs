//! `src/main/git/parsers/wipStatus.ts` — `git status --porcelain=v2 -z` 출력을 `WipStatus` 로 만든다.

use std::collections::HashSet;

use serde::Serialize;

use super::changes::{FileChange, FileStatus};
use super::nul_records::NulRecordSplitter;

/// 추적 안 되는 파일 목록 상한. 넘는 항목은 개수만 센다
pub const WIP_UNTRACKED_LIMIT: usize = 5000;

/// 작업 트리 상태 조회 인자. `-uall` 로 추적 안 되는 폴더 안의 파일을 하나씩 받는다.
/// --ignore-submodules=all: 서브모듈마다 git 을 하나씩 더 띄우지 않게 한다.
pub const WIP_STATUS_ARGS: [&str; 7] = ["status", "--porcelain=v2", "-z", "--untracked-files=all", "--renames", "--ignore-submodules=all", "--"];

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WipCounts {
    /// 기존 파일의 변경(수정·삭제·이름 변경 등). 스테이지/작업 트리를 합쳐 경로 기준으로 중복을 뺀다
    pub modified: usize,
    /// 새 파일(스테이지된 추가 + 추적 안 됨, 상한 밖 포함)
    pub added: usize,
    pub staged: usize,
    pub unstaged: usize,
    pub untracked: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WipStatus {
    /// 인덱스(스테이지) 변경. additions/deletions 는 계산하지 않아 null 이다
    pub staged: Vec<FileChange>,
    /// 작업 트리 변경(충돌 파일 포함, status 'U')
    pub unstaged: Vec<FileChange>,
    /// 추적 안 되는 파일 경로(상한까지만 담는다)
    pub untracked: Vec<String>,
    /// 상한 때문에 목록에 담지 못한 추적 안 되는 파일 수
    pub untracked_more: usize,
    /// 충돌(병합 미해결) 파일 수 — unstaged 에도 'U' 로 들어 있다
    pub conflicted: usize,
    pub counts: WipCounts,
}

/// porcelain v2 의 XY 한 글자 → 파일 상태. '.'(변경 없음)과 그 밖의 글자는 None
fn to_status(c: Option<char>) -> Option<FileStatus> {
    match c? {
        'A' => Some(FileStatus::A),
        'M' => Some(FileStatus::M),
        'D' => Some(FileStatus::D),
        'R' => Some(FileStatus::R),
        'C' => Some(FileStatus::C),
        'T' => Some(FileStatus::T),
        _ => None,
    }
}

/// 앞쪽 공백 구분 필드 n 개를 건너뛴 나머지(경로 — 공백이 들어 있을 수 있다)
fn after_fields(rec: &str, n: usize) -> &str {
    let mut i = 0;
    for _ in 0..n {
        match rec[i..].find(' ') {
            Some(p) => i += p + 1,
            None => return "",
        }
    }
    &rec[i..]
}

fn file(status: FileStatus, path: &str) -> FileChange {
    FileChange { status, path: path.to_string(), old_path: None, similarity: None, additions: None, deletions: None, binary: false }
}

/// 이름 변경 레코드가 만든 항목의 위치(true 면 staged, false 면 unstaged 의 인덱스)
type MadeRef = (bool, usize);

/// `git status --porcelain=v2 -z` 출력을 청크 단위로 받아 `WipStatus` 로 만든다.
/// 레코드: `1 XY sub mH mI mW hH hI path` · `2 XY sub mH mI mW hH hI Xscore path\0origPath`(이름 변경: 새 경로가 먼저)
/// · `u XY sub m1 m2 m3 mW h1 h2 h3 path`(충돌) · `? path`. X 는 인덱스(스테이지), Y 는 작업 트리 열이라 `MM` 은 양쪽 목록에 모두 들어간다.
/// git 은 추적 중인 항목을 모두 낸 뒤에 `?` 항목을 내므로, 추적 안 되는 파일은 상한까지만 담고 나머지는 세기만 한다.
pub struct WipStatusParser {
    splitter: NulRecordSplitter,
    staged: Vec<FileChange>,
    unstaged: Vec<FileChange>,
    untracked: Vec<String>,
    untracked_total: usize,
    conflicted: usize,
    /// 이름 변경 레코드 뒤에 오는 원래 경로를 기다리는 중인 항목
    rename_of: Option<Vec<MadeRef>>,
    modified_paths: HashSet<String>,
    added_paths: HashSet<String>,
    untracked_limit: usize,
}

impl WipStatusParser {
    pub fn new(untracked_limit: usize) -> Self {
        Self {
            splitter: NulRecordSplitter::new(),
            staged: Vec::new(),
            unstaged: Vec::new(),
            untracked: Vec::new(),
            untracked_total: 0,
            conflicted: 0,
            rename_of: None,
            modified_paths: HashSet::new(),
            added_paths: HashSet::new(),
            untracked_limit,
        }
    }

    pub fn push(&mut self, chunk: &[u8]) {
        for rec in self.splitter.push(chunk) {
            self.record(&rec);
        }
    }

    pub fn finish(mut self) -> WipStatus {
        for rec in self.splitter.end() {
            self.record(&rec);
        }
        let added = self.added_paths.len() + self.untracked_total;
        WipStatus {
            counts: WipCounts {
                modified: self.modified_paths.len(),
                added,
                staged: self.staged.len(),
                unstaged: self.unstaged.len(),
                untracked: self.untracked_total,
            },
            untracked_more: self.untracked_total - self.untracked.len(),
            staged: self.staged,
            unstaged: self.unstaged,
            untracked: self.untracked,
            conflicted: self.conflicted,
        }
    }

    fn record(&mut self, rec: &str) {
        if let Some(made) = self.rename_of.take() {
            // 직전 `2` 레코드의 원래 경로
            for (staged, i) in made {
                let list = if staged { &mut self.staged } else { &mut self.unstaged };
                list[i].old_path = Some(rec.to_string());
            }
            return;
        }
        match rec.chars().next() {
            Some('?') => {
                self.untracked_total += 1;
                if self.untracked.len() < self.untracked_limit {
                    // `? ` 접두 두 글자를 뗀다
                    let path = rec.char_indices().nth(2).map_or("", |(i, _)| &rec[i..]);
                    self.untracked.push(path.to_string());
                }
            }
            Some(kind @ ('1' | '2')) => {
                let path = after_fields(rec, if kind == '1' { 8 } else { 9 });
                if path.is_empty() {
                    return;
                }
                let mut chars = rec.chars().skip(2);
                let (x, y) = (to_status(chars.next()), to_status(chars.next()));
                let mut made: Vec<MadeRef> = Vec::new();
                if let Some(x) = x {
                    self.staged.push(file(x, path));
                    made.push((true, self.staged.len() - 1));
                    if x == FileStatus::A {
                        self.added_paths.insert(path.to_string());
                    } else {
                        self.modified_paths.insert(path.to_string());
                    }
                }
                if let Some(y) = y {
                    self.unstaged.push(file(y, path));
                    made.push((false, self.unstaged.len() - 1));
                    if !self.added_paths.contains(path) {
                        self.modified_paths.insert(path.to_string());
                    }
                }
                if kind == '2' {
                    self.rename_of = Some(made);
                }
            }
            Some('u') => {
                let path = after_fields(rec, 10);
                if path.is_empty() {
                    return;
                }
                self.conflicted += 1;
                self.unstaged.push(file(FileStatus::U, path));
                self.modified_paths.insert(path.to_string());
            }
            _ => {}
        }
    }
}

/// 이미 메모리에 있는 출력을 한 번에 파싱한다
pub fn parse_wip_status(buf: &[u8], untracked_limit: Option<usize>) -> WipStatus {
    let mut p = WipStatusParser::new(untracked_limit.unwrap_or(WIP_UNTRACKED_LIMIT));
    p.push(buf);
    p.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mm_is_in_both_lists_and_counted_once() {
        let rec = format!("1 MM N... 100644 100644 100644 {0} {0} a b.txt\0", "1".repeat(40));
        let s = parse_wip_status(rec.as_bytes(), None);
        assert_eq!((s.staged.len(), s.unstaged.len(), s.counts.modified), (1, 1, 1));
        assert_eq!(s.staged[0].path, "a b.txt");
    }

    #[test]
    fn rename_takes_next_record_as_old_path() {
        let rec = format!("2 R. N... 100644 100644 100644 {0} {0} R100 new.txt\0old.txt\0", "1".repeat(40));
        let s = parse_wip_status(rec.as_bytes(), None);
        assert_eq!(s.staged[0].old_path.as_deref(), Some("old.txt"));
    }
}
