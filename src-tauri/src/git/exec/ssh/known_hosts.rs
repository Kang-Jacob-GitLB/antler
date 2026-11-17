//! known_hosts 기반 호스트 키 검증(`src/main/executors/ssh/knownHosts.ts`). **키를 자동으로 추가하지 않는다(TOFU 없음)** — 모르는 호스트는 거부한다.
//! 이 모듈에는 known_hosts 를 쓰는 함수가 없다(읽기만 한다).
//!
//! 지원: 평문 항목(쉼표 목록·와일드카드·`!`부정·`[host]:port`), 해시 항목 `|1|salt|hash`(HMAC-SHA1), `@revoked`(거부).
//! 무시: `@cert-authority`(인증서 검증 미지원이므로 항목 자체를 건너뛴다).
//!
//! TS 와 같게 옮긴 곳: 해시 항목의 salt·hash 는 Node `Buffer.from(x, 'base64')` 처럼 **느슨하게** 푼다(URL-safe 문자·공백·누락된 `=` 를 받는다) —
//! [`node_base64_decode`]. 키 종류는 blob 앞 문자열을 Node `toString('ascii')` 처럼(바이트의 최상위 비트를 지운다) 읽는다.
//! 와일드카드 대소문자 규칙은 `mod.rs` 의 "이식 규칙"을 따른다.

use std::fs;
use std::path::Path;

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use hmac::{Hmac, Mac};
use sha1::Sha1;
use sha2::{Digest, Sha256};

use super::{glob_match, split_ws};
use crate::git::fetch::{js_to_lowercase, js_trim};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Marker {
    Revoked,
    CertAuthority,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnownHostEntry {
    pub marker: Option<Marker>,
    pub hosts: String,
    pub key_type: String,
    pub key_base64: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostKeyCheck {
    Ok,
    Unknown,
    Mismatch,
    Revoked,
    /// 비 ASCII 호스트 이름 — 대소문자 접기가 TS 와 달라질 수 있어 판정하지 않고 거부한다(`mod.rs` "이식 규칙")
    Unsupported,
}

impl HostKeyCheck {
    /// 골든·로그에 쓰는 이름(TS 의 `status`)
    pub fn as_str(self) -> &'static str {
        match self {
            HostKeyCheck::Ok => "ok",
            HostKeyCheck::Unknown => "unknown",
            HostKeyCheck::Mismatch => "mismatch",
            HostKeyCheck::Revoked => "revoked",
            HostKeyCheck::Unsupported => "unsupported",
        }
    }
}

pub fn parse_known_hosts(text: &str) -> Vec<KnownHostEntry> {
    let mut out = Vec::new();
    // `split(/\r?\n/)` + `trim()`: 줄 끝 \r 은 trim 이 지운다
    for raw in text.split('\n') {
        let line = js_trim(raw);
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut f = split_ws(line);
        let mut marker = None;
        if f[0].starts_with('@') {
            match f.remove(0) {
                "@revoked" => marker = Some(Marker::Revoked),
                "@cert-authority" => marker = Some(Marker::CertAuthority),
                _ => continue,
            }
        }
        let (Some(hosts), Some(key_type), Some(key_base64)) = (f.first(), f.get(1), f.get(2)) else {
            continue;
        };
        out.push(KnownHostEntry { marker, hosts: (*hosts).to_string(), key_type: (*key_type).to_string(), key_base64: (*key_base64).to_string() });
    }
    out
}

/// SSH 공개키 blob 의 앞 문자열(키 종류). 짧거나 길이가 넘치면 빈 문자열
pub fn key_type_of_blob(blob: &[u8]) -> String {
    if blob.len() < 4 {
        return String::new();
    }
    let len = u32::from_be_bytes([blob[0], blob[1], blob[2], blob[3]]) as usize;
    if len > blob.len() - 4 {
        return String::new();
    }
    // Node 'ascii' 디코딩: 바이트의 최상위 비트를 지운다
    blob[4..4 + len].iter().map(|&b| char::from(b & 0x7f)).collect()
}

/// known_hosts 에서 쓰는 호스트 표기: 22 번 포트는 host, 그 외는 `[host]:port`
pub fn known_hosts_name(host: &str, port: u32) -> String {
    let h = js_to_lowercase(host);
    if port == 22 {
        h
    } else {
        format!("[{h}]:{port}")
    }
}

/// Node `Buffer.from(s, 'base64')`: 두 알파벳(`+/` 와 `-_`)을 모두 받고, 알파벳 밖 문자(공백 등)는 건너뛰며, `=` 를 만나면 멈추고, 남는 비트는 버린다
pub fn node_base64_decode(s: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in s.chars() {
        let v = match c {
            'A'..='Z' => c as u32 - 'A' as u32,
            'a'..='z' => c as u32 - 'a' as u32 + 26,
            '0'..='9' => c as u32 - '0' as u32 + 52,
            '+' | '-' => 62,
            '/' | '_' => 63,
            '=' => break,
            _ => continue,
        };
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    out
}

fn hash_matches(hosts: &str, name: &str) -> bool {
    let mut it = hosts.split('|').skip(2);
    let (Some(salt_b64), Some(hash_b64)) = (it.next(), it.next()) else {
        return false;
    };
    if salt_b64.is_empty() || hash_b64.is_empty() {
        return false;
    }
    let Ok(mut mac) = Hmac::<Sha1>::new_from_slice(&node_base64_decode(salt_b64)) else {
        return false;
    };
    mac.update(name.as_bytes());
    mac.finalize().into_bytes().as_slice() == node_base64_decode(hash_b64).as_slice()
}

fn host_matches(hosts: &str, name: &str) -> bool {
    if hosts.starts_with("|1|") {
        return hash_matches(hosts, name);
    }
    let mut positive = false;
    for p in hosts.split(',') {
        if let Some(neg) = p.strip_prefix('!') {
            if glob_match(neg, name) {
                return false;
            }
        } else if glob_match(p, name) {
            positive = true;
        }
    }
    positive
}

fn matching(entries: Vec<KnownHostEntry>, host: &str, port: u32) -> Vec<KnownHostEntry> {
    let name = known_hosts_name(host, port);
    entries.into_iter().filter(|e| e.marker != Some(Marker::CertAuthority) && host_matches(&e.hosts, &name)).collect()
}

pub fn check_host_key(text: &str, host: &str, port: u32, key: &[u8]) -> HostKeyCheck {
    if !host.is_ascii() {
        return HostKeyCheck::Unsupported;
    }
    let hits = matching(parse_known_hosts(text), host, port);
    let b64 = STANDARD.encode(key);
    let ty = key_type_of_blob(key);
    if hits.iter().any(|e| e.marker == Some(Marker::Revoked) && e.key_base64 == b64) {
        return HostKeyCheck::Revoked;
    }
    let same_type: Vec<&KnownHostEntry> = hits.iter().filter(|e| e.marker != Some(Marker::Revoked) && e.key_type == ty).collect();
    if same_type.iter().any(|e| e.key_base64 == b64) {
        return HostKeyCheck::Ok;
    }
    if !same_type.is_empty() {
        return HostKeyCheck::Mismatch;
    }
    HostKeyCheck::Unknown
}

/// known_hosts 에 이 호스트로 등록된 키 종류에 대응하는 서버 호스트 키 알고리즘(서버가 다른 종류를 내밀지 않게 제한). `ssh-rsa` 는 SHA-2 서명 이름이 먼저다
pub fn known_host_key_algorithms(text: &str, host: &str, port: u32) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    // 비 ASCII 호스트는 판정하지 않는다([`check_host_key`])
    if !host.is_ascii() {
        return out;
    }
    for e in matching(parse_known_hosts(text), host, port) {
        if e.marker == Some(Marker::Revoked) {
            continue;
        }
        let algs: Vec<String> = if e.key_type == "ssh-rsa" {
            vec!["rsa-sha2-512".into(), "rsa-sha2-256".into(), "ssh-rsa".into()]
        } else {
            vec![e.key_type.clone()]
        };
        for a in algs {
            if !out.contains(&a) {
                out.push(a);
            }
        }
    }
    out
}

/// `SHA256:<base64, = 없음>` — 공개 지문
pub fn fingerprint(key: &[u8]) -> String {
    format!("SHA256:{}", STANDARD.encode(Sha256::digest(key)).trim_end_matches('='))
}

/// 여러 파일(`UserKnownHostsFile` 은 여러 개일 수 있다)을 줄바꿈으로 이어 읽는다. 없는 파일·읽지 못하는 파일은 빈 내용이다(UTF-8 이 아닌 바이트는 U+FFFD)
pub fn read_known_hosts<P: AsRef<Path>>(paths: &[P]) -> String {
    paths.iter().map(|p| fs::read(p).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default()).collect::<Vec<_>>().join("\n")
}

/// 검증 결과: 거부 사유는 사용자에게 보이는 문구다(공개 지문만 싣는다)
pub type VerifyResult = Result<(), String>;

/// 이미 읽은 known_hosts 텍스트로 검증한다([`verify_host_key`] 의 본체)
pub fn verify_host_key_text(text: &str, host: &str, port: u32, key: &[u8]) -> VerifyResult {
    let label = known_hosts_name(host, port);
    match check_host_key(text, host, port, key) {
        HostKeyCheck::Ok => Ok(()),
        HostKeyCheck::Unknown => Err(format!(
            "알 수 없는 호스트입니다: {label} (키 지문 {}). 터미널에서 ssh CLI 로 한 번 접속해 호스트 키를 수락한 뒤 다시 시도하세요. Antler 는 호스트 키를 자동으로 등록하지 않습니다.",
            fingerprint(key)
        )),
        HostKeyCheck::Mismatch => Err(format!(
            "호스트 키가 known_hosts 의 기록과 다릅니다: {label} (받은 키 지문 {}). 중간자 공격일 수 있어 연결을 거부했습니다. 서버를 재설치한 것이 확실하다면 known_hosts 의 해당 줄을 지우고 ssh CLI 로 다시 확인하세요.",
            fingerprint(key)
        )),
        HostKeyCheck::Revoked => Err(format!("폐기된(@revoked) 호스트 키입니다: {label}. 연결을 거부했습니다.")),
        HostKeyCheck::Unsupported => Err(format!("비 ASCII 호스트 이름은 호스트 키를 검증할 수 없어 연결을 거부했습니다: {label}. punycode 로 적으세요.")),
    }
}

/// 연결 시점마다 파일을 다시 읽어 검증한다(사용자가 ssh CLI 로 키를 수락한 직후 재시도하면 바로 반영되도록)
pub fn verify_host_key<P: AsRef<Path>>(known_hosts_paths: &[P], host: &str, port: u32, key: &[u8]) -> VerifyResult {
    verify_host_key_text(&read_known_hosts(known_hosts_paths), host, port, key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_base64_는_느슨하다() {
        assert_eq!(node_base64_decode("aGk="), b"hi");
        assert_eq!(node_base64_decode("aGk"), b"hi", "= 가 없어도 푼다");
        assert_eq!(node_base64_decode("a-_+/A=="), node_base64_decode("a+/+/A"), "URL-safe 문자도 받는다");
        assert_eq!(node_base64_decode("aG k\n="), b"hi", "공백은 건너뛴다");
        assert_eq!(node_base64_decode("aGk=zzzz"), b"hi", "= 에서 멈춘다");
        assert_eq!(node_base64_decode("a"), b"", "한 글자는 바이트가 되지 못한다");
        assert_eq!(node_base64_decode(""), b"");
    }

    #[test]
    fn 키_종류는_앞_문자열이다() {
        let mut blob = vec![0, 0, 0, 3];
        blob.extend_from_slice(b"abcxyz");
        assert_eq!(key_type_of_blob(&blob), "abc");
        assert_eq!(key_type_of_blob(&[0, 0, 0]), "");
        assert_eq!(key_type_of_blob(&[0, 0, 0, 9, 1]), "");
        assert_eq!(key_type_of_blob(&[0, 0, 0, 1, 0xE5]), "e", "최상위 비트를 지운다");
    }

    #[test]
    fn 비_ascii_호스트는_판정하지_않고_거부한다() {
        let key = {
            let mut b = vec![0, 0, 0, 11];
            b.extend_from_slice(b"ssh-ed25519");
            b.extend_from_slice(&[1, 2, 3]);
            b
        };
        let b64 = STANDARD.encode(&key);
        // 일반 항목·와일드카드·@revoked 모두 비 ASCII 호스트에서는 판정하지 않는다
        let text = format!("Éxample.com ssh-ed25519 {b64}\n@revoked *.É ssh-ed25519 {b64}\n* ssh-ed25519 {b64}\n");
        for host in ["Éxample.com", "éxample.com", "x.é", "x.É", "\u{212a}", "例.test"] {
            assert_eq!(check_host_key(&text, host, 22, &key), HostKeyCheck::Unsupported, "{host}");
            assert!(known_host_key_algorithms(&text, host, 22).is_empty(), "{host}");
            let e = verify_host_key_text(&text, host, 22, &key).unwrap_err();
            assert!(e.contains("비 ASCII"), "{e}");
        }
        // ASCII 호스트는 그대로 판정한다
        assert_eq!(check_host_key(&text, "a.test", 22, &key), HostKeyCheck::Ok);
    }

    #[test]
    fn 없는_파일은_빈_내용이다() {
        assert_eq!(read_known_hosts(&[Path::new("Z:/없는/known_hosts")]), "");
    }
}
