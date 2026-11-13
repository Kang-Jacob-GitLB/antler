//! `src/main/git` 의 Rust 이식(파서·실행기·채널별 git 호출). 앱에서는 라우터(`crate::router`)가 등록된 채널만 부르고,
//! 나머지는 Node 사이드카가 처리한다.
pub mod commit;
pub mod exec;
pub mod lru;
pub mod parsers;
