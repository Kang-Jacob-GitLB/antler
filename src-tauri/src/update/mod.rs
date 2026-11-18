//! 앱 안 업데이트(Phase 6c). **PR 1 은 판정 로직뿐이다 — 앱에 연결하지 않는다**(네트워크·다운로드·설치 없음, 상태 기계는 PR 4).
//!
//! - [`logic`]: 버전 비교·프리릴리스 규칙·다운로드 URL 허용 목록·매니페스트 검사·나중에/건너뛰기 정책·입력 검증·오류 정화·릴리스 노트 해석.
//!   원본은 `src/main/update/{logic,releaseNotes}.ts` 다. 골든(`tests/golden/update/*.json`, TS 가 기준)과 `tests/update.rs` 가 같은 결과를 내는지 본다.
//!
//! TS 의 설치 형태 판별·자산 선택·`latest*.yml`·해시 검증(`detectInstallKind`·`pickAsset`·`parseMetadata`·`verifyDigests`)은 옮기지 않는다 —
//! Tauri 는 번들러가 심은 표식과 minisign 서명으로 대신한다(대응표의 `N/A`).
pub mod logic;
