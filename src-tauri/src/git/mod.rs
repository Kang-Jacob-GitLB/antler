//! `src/main/git` 의 Rust 이식(파서·실행기·채널별 git 호출). 앱에서는 라우터(`crate::router`)가 등록된 채널만 부르고,
//! 나머지는 Node 사이드카가 처리한다.
pub mod activity;
pub mod code_browse;
pub mod commit;
pub mod exec;
pub mod fetch;
pub mod file_channels;
pub mod lfs;
pub mod link_guard;
pub mod log_service;
pub mod lru;
pub mod parsers;
pub mod refs_service;
pub mod repo_state;
pub mod scope;
pub mod search;
pub mod signature;
pub mod watcher;
pub mod wip;
