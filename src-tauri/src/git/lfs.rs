//! Git LFS·Gitflow 표시용 순수 함수(`src/shared/lfs.ts`·`src/shared/gitflow.ts` 중 main 이 쓰는 것). LFS 바이너리나 저장소 설정의 명령은
//! 실행하지 않고 git 오브젝트·속성 텍스트만 해석한다. git 을 부르는 부분(`repoExtras`·`lfsPaths`)은 해당 채널 PR 에서 옮긴다.
//!
//! 길이 한계는 JS 문자열 길이(UTF-16 코드 단위)로 센다. `\s`·`trim` 은 JS 의 공백 기준이다(`is_js_space`·fetch 의 `js_trim`).

use serde::Serialize;

use crate::git::fetch::{js_to_lowercase, js_trim};
use crate::git::scope::is_js_space;

/// LFS 포인터 파일 첫 줄
pub const LFS_POINTER_VERSION: &str = "https://git-lfs.github.com/spec/v1";
/// 포인터 파일은 100바이트 안팎이다. 이보다 길면 포인터가 아니다(UTF-16 코드 단위)
const POINTER_MAX_CHARS: usize = 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LfsPointer {
    /// 해시 알고리즘(현재 sha256 만 허용)
    pub algo: &'static str,
    /// 소문자 hex 64자
    pub oid: String,
    /// 실제 객체 크기(바이트)
    pub size: u64,
}

/// `^sha256:([0-9a-f]{64})$` 의 64자 hex
fn sha256_oid(value: &str) -> Option<&str> {
    let hex = value.strip_prefix("sha256:")?;
    (hex.len() == 64 && hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))).then_some(hex)
}

/// LFS 포인터 텍스트를 읽는다. 형식: `version <url>` 로 시작하고 `key value` 줄이 이어진다.
/// 형식이 조금이라도 어긋나면(필수 키 누락·oid 형식·크기가 정수가 아님·너무 긴 텍스트) None 이다.
pub fn parse_lfs_pointer(text: &str) -> Option<LfsPointer> {
    if text.encode_utf16().count() > POINTER_MAX_CHARS {
        return None;
    }
    // `split(/\r?\n/)` — 단독 `\r` 은 구분자가 아니다. `\r` 은 `\n` 이 뒤따르는 조각에서만 벗긴다(마지막 조각의 `\r` 은 남는다)
    let pieces: Vec<&str> = text.split('\n').collect();
    let last = pieces.len() - 1;
    let mut lines: Vec<&str> = pieces.iter().enumerate().map(|(i, l)| if i < last { l.strip_suffix('\r').unwrap_or(l) } else { l }).collect();
    if lines.last() == Some(&"") {
        lines.pop();
    }
    if lines.len() < 3 || lines[0].strip_prefix("version ") != Some(LFS_POINTER_VERSION) {
        return None;
    }
    let (mut oid, mut size): (Option<&str>, Option<u64>) = (None, None);
    for line in &lines[1..] {
        let sp = line.find(' ')?;
        if sp == 0 {
            return None;
        }
        let (key, value) = (&line[..sp], &line[sp + 1..]);
        if key == "oid" {
            let o = sha256_oid(value)?;
            if oid.is_some() {
                return None;
            }
            oid = Some(o);
        } else if key == "size" {
            // `^\d{1,15}$` — JS `\d` 는 ASCII 숫자뿐이다
            if value.is_empty() || value.len() > 15 || !value.bytes().all(|b| b.is_ascii_digit()) || size.is_some() {
                return None;
            }
            size = value.parse().ok();
        }
    }
    Some(LfsPointer { algo: "sha256", oid: oid?.to_string(), size: size? })
}

/// `git check-attr -z filter -- <paths>` 출력(`path\0attr\0value\0` 반복)에서 filter=lfs 인 경로를 모은다.
/// 경로에 개행이 있어도 NUL 구분이라 안전하다.
pub fn parse_check_attr_lfs(out: &str) -> Vec<String> {
    let parts: Vec<&str> = out.split('\0').collect();
    let mut paths = Vec::new();
    let mut i = 0;
    while i + 2 < parts.len() {
        if parts[i + 1] == "filter" && parts[i + 2] == "lfs" {
            paths.push(parts[i].to_string());
        }
        i += 3;
    }
    paths
}

/// `diff --no-index /dev/null <file>` 출력에서 추가된 줄(파일 내용)만 이어 붙인다. 머리(+++ …)는 건너뛴다
pub fn added_lines(patch: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    let mut in_hunk = false;
    for line in patch.split('\n') {
        if line.starts_with("@@") {
            in_hunk = true;
        } else if in_hunk {
            if let Some(rest) = line.strip_prefix('+') {
                out.push(rest);
            }
        }
    }
    out.join("\n")
}

/// `(^|\s)filter=lfs(\s|$)` — 줄 어디에서든 속성 토큰 하나가 정확히 `filter=lfs` 인가
fn has_filter_lfs_token(line: &str) -> bool {
    const NEEDLE: &str = "filter=lfs";
    line.match_indices(NEEDLE).any(|(at, _)| {
        let before_ok = line[..at].chars().next_back().is_none_or(is_js_space);
        let after_ok = line[at + NEEDLE.len()..].chars().next().is_none_or(is_js_space);
        before_ok && after_ok
    })
}

/// .gitattributes 텍스트가 LFS 를 쓰는지(`filter=lfs` 속성이 있는 규칙). 주석 줄은 무시한다
pub fn gitattributes_uses_lfs(text: &str) -> bool {
    text.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l)).any(|raw| {
        let line = js_trim(raw);
        !line.is_empty() && !line.starts_with('#') && has_filter_lfs_token(line)
    })
}

// ── Gitflow ────────────────────────────────────────────────────────────────────

pub const GITFLOW_PREFIX_KINDS: [&str; 5] = ["feature", "release", "hotfix", "bugfix", "support"];
/// 설정값 길이 상한(UTF-16 코드 단위). 브랜치 이름·접두사로 이보다 긴 값은 정상적이지 않다
pub const GITFLOW_VALUE_MAX: usize = 100;

/// `[\u0000-\u001f\u007f-\u009f  ]`
fn is_gitflow_control(c: char) -> bool {
    matches!(c, '\u{0}'..='\u{1f}' | '\u{7f}'..='\u{9f}' | '\u{2028}' | '\u{2029}')
}

/// 표시해도 되는 값만 통과시킨다: 제어문자·줄바꿈이 있거나 너무 길거나 비어 있으면 None
fn clean_value(v: &str) -> Option<&str> {
    (!v.is_empty() && v.encode_utf16().count() <= GITFLOW_VALUE_MAX && !v.chars().any(is_gitflow_control) && !js_trim(v).is_empty()).then_some(v)
}

/// 접두사 맵: JSON 에서는 TS 의 `Partial<Record<kind, string>>`(키는 설정에 나온 것만)
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct GitflowPrefixes {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub feature: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub release: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hotfix: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bugfix: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub support: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct GitflowConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub master: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub develop: Option<String>,
    pub prefixes: GitflowPrefixes,
}

/// `git config --null --get-regexp ^gitflow\.` 출력(`key\nvalue\0` 반복)을 읽는다.
/// 같은 키가 여러 번 나오면 마지막 값이 이긴다. 알려진 키만 받고, 값이 안전하지 않으면 그 키를 버린다.
/// 쓸 만한 키가 하나도 없으면 None(설정 없음).
pub fn parse_gitflow_config(raw: &str) -> Option<GitflowConfig> {
    let mut cfg = GitflowConfig::default();
    let mut any = false;
    for rec in raw.split('\0') {
        let Some(nl) = rec.find('\n') else { continue };
        let key = js_to_lowercase(&rec[..nl]);
        let Some(value) = clean_value(&rec[nl + 1..]) else { continue };
        let value = Some(value.to_string());
        if key == "gitflow.branch.master" {
            cfg.master = value;
        } else if key == "gitflow.branch.develop" {
            cfg.develop = value;
        } else if let Some(kind) = key.strip_prefix("gitflow.prefix.") {
            let slot = match kind {
                "feature" => &mut cfg.prefixes.feature,
                "release" => &mut cfg.prefixes.release,
                "hotfix" => &mut cfg.prefixes.hotfix,
                "bugfix" => &mut cfg.prefixes.bugfix,
                "support" => &mut cfg.prefixes.support,
                _ => continue,
            };
            *slot = value;
        } else {
            continue;
        }
        any = true;
    }
    any.then_some(cfg)
}

#[cfg(test)]
mod tests {
    use super::*;

    const OID: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const OID2: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    const URL: &str = LFS_POINTER_VERSION;

    fn pointer(oid: &str, size: u64) -> String {
        format!("version {URL}\noid sha256:{oid}\nsize {size}\n")
    }

    fn rec(k: &str, v: &str) -> String {
        format!("{k}\n{v}\0")
    }

    // ---- tests/lfsGitflow.test.ts 를 같은 입력으로 옮긴 것 ----

    #[test]
    fn 정상_포인터를_읽는다() {
        let p = parse_lfs_pointer(&pointer(OID, 12345)).unwrap();
        assert_eq!((p.algo, p.oid.as_str(), p.size), ("sha256", OID, 12345));
    }

    #[test]
    fn crlf_끝_개행_없음도_읽는다() {
        let p = parse_lfs_pointer(pointer(OID, 7).replace('\n', "\r\n").trim_end()).unwrap();
        assert_eq!(p.size, 7);
    }

    #[test]
    fn 확장_키가_있어도_필수_키만_본다() {
        let p = parse_lfs_pointer(&format!("version {URL}\next-0-foo sha256:{OID2}\noid sha256:{OID}\nsize 3\n")).unwrap();
        assert_eq!(p.oid, OID);
    }

    #[test]
    fn 형식이_어긋나면_none() {
        assert!(parse_lfs_pointer("hello").is_none());
        assert!(parse_lfs_pointer("").is_none());
        assert!(parse_lfs_pointer(&format!("version https://example.com/v2\noid sha256:{OID}\nsize 1\n")).is_none());
        assert!(parse_lfs_pointer(&format!("version {URL}\noid sha256:{OID}\n")).is_none());
        assert!(parse_lfs_pointer(&format!("version {URL}\nsize 1\n")).is_none());
        assert!(parse_lfs_pointer(&pointer("xyz", 1)).is_none());
        assert!(parse_lfs_pointer(&pointer(&OID.to_uppercase(), 1)).is_none());
        assert!(parse_lfs_pointer(&format!("version {URL}\noid md5:{OID}\nsize 1\n")).is_none());
    }

    #[test]
    fn 크기가_정수가_아니거나_비정상이면_none() {
        let long = "9".repeat(40);
        for bad in ["-1", "1.5", "1e3", "abc", "", long.as_str()] {
            assert!(parse_lfs_pointer(&format!("version {URL}\noid sha256:{OID}\nsize {bad}\n")).is_none(), "{bad}");
        }
    }

    #[test]
    fn 중복_키_너무_긴_텍스트_키_없는_줄은_none() {
        assert!(parse_lfs_pointer(&(pointer(OID, 1) + &format!("oid sha256:{OID2}\n"))).is_none());
        assert!(parse_lfs_pointer(&(pointer(OID, 1) + &"x".repeat(2000))).is_none());
        assert!(parse_lfs_pointer(&(pointer(OID, 1) + "nospace\n")).is_none());
    }

    #[test]
    fn check_attr_출력에서_filter_lfs_경로만_모은다() {
        let out = ["a.bin", "filter", "lfs", "b.txt", "filter", "unspecified", "we ird\nname.psd", "filter", "lfs", "c", "filter", "other", ""].join("\0");
        assert_eq!(parse_check_attr_lfs(&out), vec!["a.bin".to_string(), "we ird\nname.psd".to_string()]);
        assert!(parse_check_attr_lfs("").is_empty());
        assert!(parse_check_attr_lfs("a\0filter").is_empty());
    }

    #[test]
    fn gitattributes_에서_filter_lfs_규칙을_찾는다() {
        assert!(gitattributes_uses_lfs("*.psd filter=lfs diff=lfs merge=lfs -text\n"));
        assert!(gitattributes_uses_lfs("*.txt text\r\n*.bin filter=lfs\r\n"));
        assert!(!gitattributes_uses_lfs("# *.psd filter=lfs\n*.txt text\n"));
        assert!(!gitattributes_uses_lfs("*.psd filter=lfsx\n*.a myfilter=lfs\n"));
        assert!(!gitattributes_uses_lfs(""));
    }

    #[test]
    fn added_lines_는_diff_no_index_출력에서_파일_내용_줄만_모은다() {
        let patch = "diff --git a/dev/null b/x\nnew file mode 100644\n--- /dev/null\n+++ b/x\n@@ -0,0 +1,2 @@\n+*.psd filter=lfs\n+# +++ not header\n";
        assert_eq!(added_lines(patch), "*.psd filter=lfs\n# +++ not header");
        assert_eq!(added_lines(""), "");
    }

    #[test]
    fn branch_prefix_값을_읽는다() {
        let raw = rec("gitflow.branch.master", "main") + &rec("gitflow.branch.develop", "dev") + &rec("gitflow.prefix.feature", "feat/") + &rec("gitflow.prefix.versiontag", "v");
        let want = GitflowConfig {
            master: Some("main".into()),
            develop: Some("dev".into()),
            prefixes: GitflowPrefixes { feature: Some("feat/".into()), ..Default::default() },
        };
        assert_eq!(parse_gitflow_config(&raw), Some(want));
    }

    #[test]
    fn 설정이_없으면_none() {
        assert!(parse_gitflow_config("").is_none());
        assert!(parse_gitflow_config(&(rec("gitflow.prefix.versiontag", "v") + &rec("gitflow.unknown", "x"))).is_none());
    }

    #[test]
    fn 같은_키는_마지막_값이_이긴다() {
        let c = parse_gitflow_config(&(rec("gitflow.branch.develop", "a") + &rec("gitflow.branch.develop", "b"))).unwrap();
        assert_eq!(c.develop.as_deref(), Some("b"));
    }

    #[test]
    fn 줄바꿈_제어문자_너무_긴_값은_버린다() {
        let raw = rec("gitflow.branch.master", "ma\nin")
            + &rec("gitflow.branch.develop", "de\u{1b}[31mv")
            + &rec("gitflow.prefix.feature", &"x".repeat(GITFLOW_VALUE_MAX + 1))
            + &rec("gitflow.prefix.release", "rel\u{7}/")
            + &rec("gitflow.prefix.hotfix", "   ")
            + &rec("gitflow.prefix.bugfix", "fix/");
        let want = GitflowConfig { prefixes: GitflowPrefixes { bugfix: Some("fix/".into()), ..Default::default() }, ..Default::default() };
        assert_eq!(parse_gitflow_config(&raw), Some(want));
        assert!(parse_gitflow_config(&rec("gitflow.branch.master", "m\u{2028}x")).is_none());
        let c = parse_gitflow_config(&rec("gitflow.branch.master", "$(touch pwned)")).unwrap();
        assert_eq!(c, GitflowConfig { master: Some("$(touch pwned)".into()), ..Default::default() });
    }

    #[test]
    fn 깨진_레코드와_아주_큰_입력도_던지지_않는다() {
        assert!(parse_gitflow_config("no-newline\0\n\0").is_none());
        assert_eq!(parse_gitflow_config(&rec("gitflow.branch.master", "m").repeat(50_000)).unwrap().master.as_deref(), Some("m"));
    }

    // ---- 이 PR 에서 더한 경계(골든도 같은 입력을 본다) ----

    #[test]
    fn 포인터_길이는_utf16_단위_1024가_경계() {
        let base = pointer(OID, 12345);
        // 한글은 UTF-16 한 단위지만 UTF-8 로는 3바이트다 — 바이트가 아니라 글자 수로 센다
        let pad = |n: usize| format!("{base}ext-0-x {}\n", "가".repeat(n - base.len() - "ext-0-x ".len() - 1));
        assert_eq!(pad(1024).encode_utf16().count(), 1024);
        assert!(parse_lfs_pointer(&pad(1024)).is_some());
        assert!(parse_lfs_pointer(&pad(1025)).is_none());
        // 비 BMP 는 두 단위
        let emoji = format!("{base}ext-0-x {}\n", "😀".repeat(500));
        assert!(parse_lfs_pointer(&emoji).is_none());
    }

    #[test]
    fn 포인터_크기는_15자리까지_ascii_숫자만() {
        let with = |size: &str| parse_lfs_pointer(&format!("version {URL}\noid sha256:{OID}\nsize {size}\n"));
        assert_eq!(with("999999999999999").unwrap().size, 999_999_999_999_999);
        assert_eq!(with("007").unwrap().size, 7);
        assert!(with("1234567890123456").is_none());
        assert!(with("１２").is_none(), "전각 숫자");
        assert!(with("12 ").is_none());
        // 끝의 단독 \r 은 마지막 조각에 남아 값이 어긋난다
        assert!(parse_lfs_pointer(&format!("{}\r", pointer(OID, 1).trim_end())).is_none());
        assert!(parse_lfs_pointer(&format!("\u{feff}{}", pointer(OID, 1))).is_none());
    }

    #[test]
    fn gitattributes_토큰_경계는_js_공백_기준() {
        assert!(gitattributes_uses_lfs("# c\r\n  *.bin\tfilter=lfs\r\n"));
        assert!(gitattributes_uses_lfs("filter=lfs"));
        // JS \s 는 U+0085 를 공백으로 보지 않는다(Rust is_whitespace 는 본다)
        assert!(!gitattributes_uses_lfs("*.a\u{85}filter=lfs\n"));
        assert!(gitattributes_uses_lfs("*.a\u{feff}filter=lfs\u{a0}\n"));
        assert!(gitattributes_uses_lfs("a filter=lfsx filter=lfs"));
    }

    #[test]
    fn gitflow_값의_경계() {
        let one = |v: &str| parse_gitflow_config(&format!("gitflow.branch.master\n{v}\0")).and_then(|c| c.master);
        assert!(one(&"x".repeat(100)).is_some());
        assert!(one(&"x".repeat(101)).is_none());
        assert!(one(&"가".repeat(100)).is_some());
        assert!(one(&"😀".repeat(51)).is_none(), "UTF-16 로 102");
        assert!(one(&"😀".repeat(50)).is_some());
        for bad in ["a\nb", "a\tb", "a\u{7f}", "a\u{9f}", "a\u{2028}", "a\u{2029}", "", " ", "\u{feff}", "\u{a0}\u{3000}", "\u{a0}\u{85}"] {
            assert!(one(bad).is_none(), "{bad:?}");
        }
        assert_eq!(one(" a ").as_deref(), Some(" a "));
        assert_eq!(one("a\u{a0}").as_deref(), Some("a\u{a0}"));
        // 키는 JS toLowerCase 로 소문자화한다
        let c = parse_gitflow_config(&rec("GitFlow.Branch.Master", "M")).unwrap();
        assert_eq!(c.master.as_deref(), Some("M"));
    }
}
