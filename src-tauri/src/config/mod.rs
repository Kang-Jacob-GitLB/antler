//! 설정 저장소 코어(Phase 6b PR 1). `src/main/services/ConfigStore.ts` 의 이식이다 — **앱에 연결하지 않는다**(시험만 쓴다).
//! 설정 파일의 유일한 작성자는 아직 사이드카다. Rust 가 소유하는 전환은 PR 9 에서 한 번에 한다(docs/tauri.md 의 Phase 6b).
//!
//! - `model`: 설정은 순서 있는 JSON 트리다(알 수 없는 키·키 순서를 바이트까지 보존). `source_for`·기본값.
//! - `load`: 파일 해석과 정화(손상 판정은 TS 가 던지는 모양과 같다).
//! - `validate`: `config:setUi` 패치 검증기(TS 1:1).
//! - `save`: 원자적 쓰기와 `JSON.stringify(…, null, 2)` 바이트.
//! - `store`: [`ConfigStore`] — 변경 연산·잠금·변경 알림.
//! - `derive`: 미러(`mirror_of`)와 fetch 설정(`fetch_prefs_of`).
//!
//! 바이트 패리티는 `tests/golden/config/*.json`(TS 가 기준: `UPDATE_GOLDEN=1 npx vitest run tests/goldenConfig.test.ts`)과 `tests/config.rs` 가 지킨다.
pub mod derive;
pub mod load;
pub mod model;
pub mod save;
pub mod store;
pub mod validate;

pub use model::{source_for, LOCAL_SOURCE_ID};
pub use store::{ChangeListener, ConfigStore, TYPE_ERROR};
