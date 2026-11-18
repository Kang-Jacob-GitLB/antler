//! 앱 안 업데이트(Phase 6c). PR 1 이 판정 로직, PR 4 가 상태 기계·호스트(앱 연결)다.
//!
//! - [`logic`]: 버전 비교·프리릴리스 규칙·다운로드 URL 허용 목록·매니페스트 검사·나중에/건너뛰기 정책·입력 검증·오류 정화·릴리스 노트 해석. **순수 로직이다 — 네트워크·파일·프로세스를 쓰지 않는다.**
//!   원본은 `src/main/update/{logic,releaseNotes}.ts` 다. 골든(`tests/golden/update/*.json`, TS 가 기준)과 `tests/update.rs` 가 같은 결과를 내는지 본다.
//! - [`service`]: 상태 기계(`UpdateService.ts` 이식)와 `update:*` 7채널의 봉투. 네트워크·설치는 [`service::Backend`] 포트 뒤에 있다. 골든: `tests/fixtures/update.scenarios.json`(`tests/updateScenarios.rs`).
//! - [`host`]: 운영 [`service::Backend`](net::http 로 받고 `tauri-plugin-updater` 로 검증·설치), 설정·이벤트 연결, **업데이트 공개 키 상수 `UPDATE_PUBKEY`**(비어 있으면 fail-closed).
//!
//! TS 의 설치 형태 판별·자산 선택·`latest*.yml`·해시 검증(`detectInstallKind`·`pickAsset`·`parseMetadata`·`verifyDigests`)은 옮기지 않는다 —
//! Tauri 는 번들러가 심은 표식과 minisign 서명으로 대신한다(대응표의 `N/A`).
pub mod host;
pub mod logic;
pub mod service;
