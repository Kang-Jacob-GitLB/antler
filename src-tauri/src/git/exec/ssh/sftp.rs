//! 원격 폴더 목록(`src/main/executors/ssh/remoteDir.ts` 의 `listRemoteDirs`) — 저장소 추가 창의 SSH 폴더 탐색. Phase 6b PR 4b, 결정 D4(`russh-sftp`).
//!
//! **SFTP 하위 시스템으로 읽기만 한다. 셸 명령을 보내지 않는다.** 쓰는 SFTP 요청은 `init`·`realpath`·`opendir`·`readdir`·`close`·`stat` 여섯 가지뿐이고,
//! 크레이트의 쓰기 계열(`open`·`write`·`remove`·`rename` …)은 이 파일 밖으로 나가지 않는다(세션 타입이 `pub(super)` 도 아니라 이 파일 안에서만 만든다).
//! 채널을 여는 곳(`request_subsystem`)은 `connection.rs` 한 곳이고, 이 파일은 그 채널을 이어 주는 바이트 스트림만 받는다.
//!
//! 서버가 보내는 값은 믿지 않는다:
//! - 패킷 하나는 [`MAX_PACKET_LEN`] 이하(크레이트가 넘으면 읽기를 끝낸다), 응답마다 [`REQUEST_TIMEOUT_SECS`], 전체는 호출자가 거는 시간 제한이다.
//! - 읽는 항목은 [`MAX_RAW_ENTRIES`] 까지(그 뒤는 `truncated`). 폴더 이름이 [`MAX_NAME_BYTES`] 를 넘거나 `/`·NUL 을 품으면 건너뛴다 — 이름은 stat 경로와
//!   `scan_from_listing` 이 내는 저장소 경로에 이어 붙으므로, `../x` 같은 이름이 목록 밖을 가리키지 못하게 한다.
//! - `realpath` 의 결과는 절대 경로·NUL 없음·4096 UTF-16 이하여야 한다. 이 값이 UI 로 가고 저장되어 원격 git 명령의 경로가 된다.
//!
//! TS 와 다른 점(docs/tauri.md "PR 4b"):
//! - 읽는 항목에 상한이 있다(TS 는 폴더 전체를 읽는다). 상한을 넘으면 `truncated`.
//! - 이름에 `/`·NUL 이 있거나 너무 긴 항목은 건너뛴다.
//! - 심볼릭 링크·`.git` 의 `stat` 은 한 번에 여러 개를 동시에 보낸다(링크가 많은 폴더에서 왕복 시간이 쌓이지 않게). 결과·순서는 같다.
//! - 이름순은 `scan.rs` 의 `locale_compare`(`localeCompare` 근사)다.
//! - SFTP 를 못 쓰는 경우(하위 시스템 거부·`init` 실패·SFTP 3 이 아닌 서버)는 [`SftpError::Unavailable`] 이다.

use std::fmt;

use futures_util::future::join_all;
use russh_sftp::client::error::Error as ClientError;
use russh_sftp::client::{Config, RawSftpSession};
use russh_sftp::protocol::{FileAttributes, StatusCode};
use tokio::io::{AsyncRead, AsyncWrite};

use crate::repos::scan::{locale_compare, RemoteDirEntry, RemoteDirListing, MAX_DIRS};

/// 하위 폴더마다 `.git` 을 확인하는 상한(그 뒤 항목은 저장소 여부를 표시하지 않는다)
const REPO_PROBE_LIMIT: usize = 200;
/// 한 번에 동시에 보내는 `stat` 수
const STAT_BATCH: usize = 32;
/// 목록에서 읽는 항목 수 상한(파일·숨김 포함). 넘으면 읽기를 멈추고 `truncated`
const MAX_RAW_ENTRIES: usize = 20_000;
/// 폴더 이름 길이 상한(바이트). Linux 의 `NAME_MAX` 는 255 다
const MAX_NAME_BYTES: usize = 1024;
/// 서버가 보내는 SFTP 패킷 하나의 상한(크레이트 기본값과 같다 — 여기서 못박아 둔다)
const MAX_PACKET_LEN: u32 = 256 * 1024;
/// 요청 하나에 주는 시간(초). 크레이트 기본값은 10 이다
const REQUEST_TIMEOUT_SECS: u64 = 15;
/// `realpath` 결과의 UTF-16 길이 상한(`mirror::check_listdir_path` 와 같다)
const MAX_PATH_UTF16: usize = 4096;
/// SFTP 프로토콜 버전(draft-ietf-secsh-filexfer-02). 크레이트가 말하는 유일한 판이다
const SFTP_VERSION: u32 = 3;

/// 폴더 목록이 실패한 이유
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SftpError {
    /// 서버가 SFTP 를 막았다(하위 시스템 거부·`init` 실패) — 폴더를 둘러볼 수 없으니 경로를 직접 입력해야 한다. `repos:scan` 은 이때 찾기 없이 그 경로를 받는다
    Unavailable(String),
    /// 호출자가 취소했다
    Aborted,
    /// 그 밖의 실패(연결·경로 없음·권한·시간 초과·서버의 잘못된 응답)
    Failed(String),
}

impl fmt::Display for SftpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SftpError::Unavailable(detail) => write!(f, "이 서버는 SFTP 를 쓸 수 없어 폴더를 둘러볼 수 없습니다. 경로를 직접 입력하세요 ({detail})"),
            SftpError::Aborted => f.write_str("취소되었습니다"),
            SftpError::Failed(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for SftpError {}

/// 오류 문구가 담을 수 있는 글자 수 상한
const MAX_MESSAGE_CHARS: usize = 300;

/// 서버가 STATUS 로 보낸 문구(`error_message`)를 사람에게 보여 줘도 되게 정리한다 — 제어 문자(개행·ESC 등)와 방향·보이지 않는 서식 문자
/// (U+200B–U+200F·U+2028–U+202E·U+2060–U+2069·U+061C·U+FEFF)를 지우고 [`MAX_MESSAGE_CHARS`] 자로 자른다. 오류 문구가 UI 로 가므로 화면을 속이는 문구를 막는다
fn clean_message(s: &str) -> String {
    let hidden = |c: char| matches!(c, '\u{200B}'..='\u{200F}' | '\u{2028}'..='\u{202E}' | '\u{2060}'..='\u{2069}' | '\u{061C}' | '\u{FEFF}');
    s.chars().filter(|c| !c.is_control() && !hidden(*c)).take(MAX_MESSAGE_CHARS).collect()
}

fn failed(e: impl fmt::Display) -> SftpError {
    SftpError::Failed(clean_message(&e.to_string()))
}

/// `~`·`~/x`·빈 문자열을 SFTP 기준(시작 디렉터리 = 홈)으로 바꾼다(`toSftpPath`). 공백 제거는 JS `trim` 이다.
/// 길이(4096 UTF-16)·NUL 은 호출자가 `mirror::check_listdir_path` 로 먼저 거른다 — 이 함수도 같은 검사를 하므로 빠뜨려도 연결 전에 거부된다.
/// `..` 는 막지 않는다: SFTP 에는 셸 해석이 없고 읽기만 하며, `realpath` 가 서버에서 정리한다(TS 와 같다)
pub fn to_sftp_path(path: &str) -> Result<String, SftpError> {
    crate::mirror::check_listdir_path(path).map_err(SftpError::Failed)?;
    let s = crate::git::fetch::js_trim(path);
    Ok(if s.is_empty() || s == "~" {
        ".".to_string()
    } else if let Some(rest) = s.strip_prefix("~/") {
        format!("./{rest}")
    } else {
        s.to_string()
    })
}

/// 서버가 `realpath` 로 준 경로를 검사한다 — 절대 경로이고 NUL 이 없고 4096 UTF-16 이하
fn check_real_path(p: &str) -> Result<(), SftpError> {
    if !p.starts_with('/') || p.contains('\0') || p.encode_utf16().count() > MAX_PATH_UTF16 {
        return Err(SftpError::Failed("서버가 올바르지 않은 경로를 돌려주었습니다".to_string()));
    }
    Ok(())
}

/// `path.posix.dirname` — 절대 경로 전용(루트의 부모는 `None`). 끝의 `/` 는 무시한다
fn parent_of(base: &str) -> Option<String> {
    let trimmed = base.trim_end_matches('/');
    if trimmed.is_empty() {
        return None;
    }
    Some(match trimmed.rfind('/') {
        Some(0) | None => "/".to_string(),
        Some(i) => trimmed[..i].trim_end_matches('/').to_string(),
    })
}

/// `path.posix.join(base, name)` — `base` 는 절대 경로, `name` 은 [`usable_name`] 을 통과한 한 조각
fn child(base: &str, name: &str) -> String {
    if base.ends_with('/') {
        format!("{base}{name}")
    } else {
        format!("{base}/{name}")
    }
}

/// 폴더 후보로 쓸 수 있는 이름인가 — 숨김(`.` 시작)·경로 조각이 아닌 것(`/`·NUL)·너무 긴 이름은 아니다
fn usable_name(name: &str) -> bool {
    !name.is_empty() && !name.starts_with('.') && !name.contains(['/', '\0']) && name.len() <= MAX_NAME_BYTES
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Dir,
    Link,
    Other,
}

/// SFTP 속성의 파일 종류(`st_mode & S_IFMT`). 속성이 없으면 `Other`
fn kind_of(attrs: &FileAttributes) -> Kind {
    match attrs.permissions.map(|p| p & 0o170_000) {
        Some(0o040_000) => Kind::Dir,
        Some(0o120_000) => Kind::Link,
        _ => Kind::Other,
    }
}

struct Raw {
    name: String,
    kind: Kind,
}

/// 읽은 항목 전체(상한까지)와 `.git` 유무
struct Dump {
    entries: Vec<Raw>,
    /// 상한을 넘어 읽기를 멈췄다
    capped: bool,
    /// 읽은 항목 수(`entries` 에는 폴더 후보가 될 수 없는 것도 든다 — TS 의 `entries.length`)
    total: usize,
    is_repo: bool,
}

/// 스트림 위에서 SFTP 를 시작하고 `path` 의 목록을 만든다. 스트림은 이 함수가 끝나면(결과와 무관하게) 닫힌다.
/// 전체 시간 제한은 호출자가 건다(이 함수의 future 를 버리면 세션도 버려져 닫힌다)
pub(super) async fn list_dirs<S>(stream: S, path: &str) -> Result<RemoteDirListing, SftpError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let start = to_sftp_path(path)?;
    let cfg = Config { max_packet_len: MAX_PACKET_LEN, request_timeout_secs: REQUEST_TIMEOUT_SECS, ..Config::default() };
    let session = RawSftpSession::new_with_config(stream, cfg);
    let result = run(&session, &start).await;
    // 세션을 버리면 쓰기 쪽이 닫힌다. 명시적으로 닫아 두면 결과를 돌려주기 전에 닫기 요청이 나간다
    let _ = session.close_session();
    result
}

async fn run(session: &RawSftpSession, start: &str) -> Result<RemoteDirListing, SftpError> {
    let version = session.init().await.map_err(|e| SftpError::Unavailable(clean_message(&e.to_string())))?;
    if version.version != SFTP_VERSION {
        return Err(SftpError::Unavailable(format!("지원하지 않는 SFTP 버전 {}", version.version)));
    }
    let base = session.realpath(start).await.map_err(failed)?.files.into_iter().next().map(|f| f.filename).ok_or_else(|| failed("서버가 경로를 돌려주지 않았습니다"))?;
    check_real_path(&base)?;

    let dump = read_entries(session, &base).await?;
    let mut dirs = pick_dirs(session, &base, &dump.entries).await;
    dirs.sort_by(|a, b| locale_compare(&a.name, &b.name));
    let probed = dirs.len().min(REPO_PROBE_LIMIT);
    for chunk in dirs[..probed].chunks_mut(STAT_BATCH) {
        let found = join_all(chunk.iter().map(|d| async { session.stat(child(&base, &d.name) + "/.git").await.is_ok() })).await;
        for (d, has_git) in chunk.iter_mut().zip(found) {
            d.is_repo = Some(has_git);
        }
    }
    Ok(RemoteDirListing { parent: parent_of(&base), path: base, is_repo: dump.is_repo, dirs, truncated: dump.capped || dump.total > MAX_DIRS })
}

/// `opendir` → `readdir`(EOF 까지, 상한까지) → `close`. 핸들은 어떤 경로로든 닫는다
async fn read_entries(session: &RawSftpSession, base: &str) -> Result<Dump, SftpError> {
    let handle = session.opendir(base).await.map_err(failed)?.handle;
    let mut dump = Dump { entries: Vec::new(), capped: false, total: 0, is_repo: false };
    // 센 항목이 하나도 없는 응답이 연속으로 온 횟수
    let mut barren = 0;
    let outcome = loop {
        match session.readdir(handle.as_str()).await {
            Ok(batch) => {
                // 항목 없는 응답을 EOF 로 보지 않고 거부한다(끝없이 빈 응답을 보내는 서버에서 무한 반복하지 않게)
                if batch.files.is_empty() {
                    break Err(failed("서버가 빈 목록 응답을 보냈습니다"));
                }
                let before = dump.total;
                for f in batch.files {
                    // ssh2 의 `readdir(경로)` 처럼 `.`·`..` 는 항목으로 세지 않는다(`truncated` 경계가 TS 와 같아야 한다)
                    if f.filename == "." || f.filename == ".." {
                        continue;
                    }
                    if dump.total >= MAX_RAW_ENTRIES {
                        dump.capped = true;
                        break;
                    }
                    dump.total += 1;
                    if f.filename == ".git" {
                        dump.is_repo = true;
                    }
                    if usable_name(&f.filename) {
                        dump.entries.push(Raw { kind: kind_of(&f.attrs), name: f.filename });
                    }
                }
                if dump.capped {
                    break Ok(());
                }
                // `.`·`..` 만 계속 보내는 서버도 빈 응답과 같게 본다(연속 2회)
                barren = if dump.total == before { barren + 1 } else { 0 };
                if barren >= 2 {
                    break Err(failed("서버가 새 항목 없는 목록 응답을 되풀이했습니다"));
                }
            }
            Err(ClientError::Status(s)) if s.status_code == StatusCode::Eof => break Ok(()),
            Err(e) => break Err(failed(e)),
        }
    };
    let _ = session.close(handle).await;
    outcome.map(|()| dump)
}

/// 폴더인 항목만 서버 순서로 최대 [`MAX_DIRS`] 개. 심볼릭 링크는 따라가 폴더인지 본다(`stat` 실패는 폴더가 아님)
async fn pick_dirs(session: &RawSftpSession, base: &str, entries: &[Raw]) -> Vec<RemoteDirEntry> {
    let mut dirs: Vec<RemoteDirEntry> = Vec::new();
    'outer: for chunk in entries.chunks(STAT_BATCH) {
        let flags = join_all(chunk.iter().map(|e| async {
            match e.kind {
                Kind::Dir => true,
                Kind::Link => session.stat(child(base, &e.name)).await.is_ok_and(|a| kind_of(&a.attrs) == Kind::Dir),
                Kind::Other => false,
            }
        }))
        .await;
        for (e, is_dir) in chunk.iter().zip(flags) {
            if !is_dir {
                continue;
            }
            if dirs.len() >= MAX_DIRS {
                break 'outer;
            }
            dirs.push(RemoteDirEntry { name: e.name.clone(), is_repo: None });
        }
    }
    dirs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 홈_경로는_sftp_시작_디렉터리로_바뀐다() {
        for (input, want) in [("", "."), ("   ", "."), ("~", "."), (" ~ ", "."), ("~/work", "./work"), ("/srv/a", "/srv/a"), ("~x", "~x"), ("work", "work")] {
            assert_eq!(to_sftp_path(input).unwrap(), want, "{input:?}");
        }
        // JS trim 이다: NBSP 는 지우고 U+0085 는 지우지 않는다
        assert_eq!(to_sftp_path("\u{a0}~/a\u{a0}").unwrap(), "./a");
        assert_eq!(to_sftp_path("\u{85}").unwrap(), "\u{85}");
    }

    #[test]
    fn 경로_검사는_nul_과_긴_경로를_연결_전에_거부한다() {
        assert!(to_sftp_path("/a\0b").is_err());
        assert!(to_sftp_path(&"a".repeat(4097)).is_err());
        assert!(to_sftp_path(&"a".repeat(4096)).is_ok());
        // `..` 는 막지 않는다(서버의 realpath 가 정리한다)
        assert_eq!(to_sftp_path("~/../x").unwrap(), "./../x");
    }

    #[test]
    fn 서버가_준_realpath_는_절대_경로만_받는다() {
        assert!(check_real_path("/home/u").is_ok());
        assert!(check_real_path("/").is_ok());
        for bad in ["", "home", "./a", "../a", "/a\0b"] {
            assert!(check_real_path(bad).is_err(), "{bad:?}");
        }
        assert!(check_real_path(&format!("/{}", "a".repeat(4096))).is_err());
    }

    #[test]
    fn 부모_경로는_posix_dirname_이다() {
        assert_eq!(parent_of("/"), None);
        assert_eq!(parent_of("/a").as_deref(), Some("/"));
        assert_eq!(parent_of("/a/b").as_deref(), Some("/a"));
        assert_eq!(parent_of("/a/b/").as_deref(), Some("/a"));
        assert_eq!(child("/", "x"), "/x");
        assert_eq!(child("/a", "x"), "/a/x");
    }

    #[test]
    fn 폴더_후보_이름은_숨김과_경로_조각을_거른다() {
        assert!(usable_name("app"));
        assert!(usable_name("a b"));
        for bad in ["", ".", "..", ".git", ".hidden", "a/b", "../x", "a\0b"] {
            assert!(!usable_name(bad), "{bad:?}");
        }
        assert!(usable_name(&"a".repeat(MAX_NAME_BYTES)));
        assert!(!usable_name(&"a".repeat(MAX_NAME_BYTES + 1)));
    }

    #[test]
    fn 서버_문구는_제어_문자와_방향_서식_문자를_지우고_300자로_자른다() {
        let dirty = "No such\nfile\x1b[31m\u{202E}evil\u{2066}x\u{2069}\u{200E}\u{200F}\u{202A}\u{202B}\u{202C}\u{202D}\u{2028}\u{feff}ok";
        assert_eq!(clean_message(dirty), "No suchfile[31mevilxok");
        assert_eq!(clean_message("권한 없음"), "권한 없음");
        assert_eq!(clean_message(&"가".repeat(1000)).chars().count(), MAX_MESSAGE_CHARS);
        assert_eq!(failed("a\u{202e}b\0").to_string(), "ab");
    }

    #[test]
    fn 파일_종류는_모드_비트의_파일_형식_부분만_본다() {
        let attrs = |p: Option<u32>| FileAttributes { permissions: p, ..FileAttributes::empty() };
        assert_eq!(kind_of(&attrs(Some(0o040_755))), Kind::Dir);
        assert_eq!(kind_of(&attrs(Some(0o120_777))), Kind::Link);
        assert_eq!(kind_of(&attrs(Some(0o100_644))), Kind::Other);
        assert_eq!(kind_of(&attrs(Some(0o755))), Kind::Other);
        assert_eq!(kind_of(&attrs(None)), Kind::Other);
    }
}
