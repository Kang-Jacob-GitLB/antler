//! ref 시그니처: HEAD(브랜치·해시)와 모든 ref 해시를 한 값으로 줄인 것. 커밋·체크아웃·fetch·브랜치 생성/삭제·스태시가 이 값을 바꾼다(작업 트리 파일 변경은 대상이 아니다).
//! TS `refsSignature`(`src/main/git/commands.ts`)의 이식이고 두 곳이 쓴다 — fetch 서비스(fetch 전후 비교, 원문)와 [`RepoState`](super::repo_state::RepoState)(기억·비교, 요약).
//!
//! git 호출(argv 3개, 동시에)은 한 곳이라 두 쓰임이 같은 의미를 본다. 원문은 ref 수에 비례해 커지므로 **저장하는 쪽은 요약([`Signature`])만 쥔다** — 같은지만 보면 되기 때문이다.

use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

use crate::git::exec::{ExecError, RunOpts};
use crate::git::fetch::run::GitRunner;

/// 시그니처의 요약(SHA-256). 비교 전용이라 원문으로 되돌릴 수 없다
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Signature([u8; 32]);

impl Signature {
    /// `for-each-ref` 출력 · `symbolic-ref` 출력 · `rev-parse` 출력의 **원시 바이트**를 `\0` 로 이어 요약한다(TS 도 문자열이 아니라 Buffer 를 해시한다)
    fn of(parts: &RefsParts) -> Signature {
        let mut h = Sha256::new();
        h.update(&parts.refs);
        h.update(b"\0");
        h.update(&parts.head_ref);
        h.update(b"\0");
        h.update(&parts.head_hash);
        Signature(h.finalize().into())
    }
}

struct RefsParts {
    refs: Vec<u8>,
    head_ref: Vec<u8>,
    head_hash: Vec<u8>,
}

/// 세 조회를 동시에 한다. `for-each-ref` 만 종료 코드를 본다(HEAD 가 없는 빈 저장소에서 나머지 둘은 실패해도 정상이다)
async fn read_parts<R: GitRunner>(r: &R, repo_path: &str, cancel: &CancellationToken) -> Result<RefsParts, ExecError> {
    let opts = RunOpts::with_cancel(cancel.clone());
    let s = |a: &[&str]| a.iter().map(|x| (*x).to_string()).collect::<Vec<String>>();
    let (list_args, head_ref_args, head_hash_args) =
        (s(&["for-each-ref", "--format=%(objectname) %(refname)"]), s(&["symbolic-ref", "--quiet", "HEAD"]), s(&["rev-parse", "--verify", "--quiet", "HEAD"]));
    let (refs, head_ref, head_hash) =
        tokio::join!(r.run(repo_path, &list_args, &opts), r.run(repo_path, &head_ref_args, &opts), r.run(repo_path, &head_hash_args, &opts));
    let (refs, head_ref, head_hash) = (refs?, head_ref?, head_hash?);
    if refs.code != 0 {
        // TS 는 `GitError`(code GIT)다 — activity 의 envelope 이 같은 code 를 내도록 같은 종류로 둔다(fetch 는 오류의 종류를 보지 않는다 — `.ok()`)
        let message = if refs.stderr.trim().is_empty() { "for-each-ref failed".into() } else { refs.stderr.trim().to_string() };
        return Err(ExecError::Git { message, code: refs.code, stderr: refs.stderr, args: Vec::new() });
    }
    Ok(RefsParts { refs: refs.stdout, head_ref: head_ref.stdout, head_hash: head_hash.stdout })
}

/// 시그니처 비교용 원문: HEAD(브랜치·해시)와 모든 ref 해시. TS 는 이것을 sha1 로 줄일 뿐이라 같은지만 보면 되는 fetch 서비스는 원문을 비교한다
pub async fn refs_signature<R: GitRunner>(r: &R, repo_path: &str, cancel: &CancellationToken) -> Result<String, ExecError> {
    let p = read_parts(r, repo_path, cancel).await?;
    let text = |b: &[u8]| String::from_utf8_lossy(b).into_owned();
    Ok(format!("{}\0{}\0{}", text(&p.refs), text(&p.head_ref), text(&p.head_hash)))
}

/// [`refs_signature`] 와 같은 조회의 요약. 기억하는 쪽([`RepoState`](super::repo_state::RepoState))이 쓴다
pub async fn refs_signature_digest<R: GitRunner>(r: &R, repo_path: &str, cancel: &CancellationToken) -> Result<Signature, ExecError> {
    Ok(Signature::of(&read_parts(r, repo_path, cancel).await?))
}

impl Signature {
    /// 시험·진단용: 원시 세 조각의 요약
    pub fn from_parts(refs: &[u8], head_ref: &[u8], head_hash: &[u8]) -> Signature {
        Signature::of(&RefsParts { refs: refs.to_vec(), head_ref: head_ref.to_vec(), head_hash: head_hash.to_vec() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 같은_조각이면_같고_조각의_경계가_다르면_다르다() {
        let a = Signature::from_parts(b"x", b"y", b"z");
        assert_eq!(a, Signature::from_parts(b"x", b"y", b"z"));
        assert_ne!(a, Signature::from_parts(b"x", b"yz", b""));
        assert_ne!(a, Signature::from_parts(b"xy", b"", b"z"));
        // 잘못된 UTF-8 은 lossy 로 뭉개지지 않고 바이트로 구분된다
        assert_ne!(Signature::from_parts(&[0xff], b"", b""), Signature::from_parts(&[0xfe], b"", b""));
    }
}
