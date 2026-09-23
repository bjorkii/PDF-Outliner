//! OCR 텍스트 레이어 작업의 PDF 구조 쪽 — 설계는 `plan/ocr_feature_considerations.md`.
//!
//! 이 크레이트의 원칙: **처리할 수 없는 경우를 감지해서 원본을 지키고 알린다.** 모든 PDF를
//! 자동으로 완벽하게 처리하려 하지 않는다. 해석에 실패한 페이지는 건드리지 않고 이유를 돌려준다.
//!
//! 콘텐츠 스트림 편집에 lopdf의 `Content::decode`/`encode`를 쓰지 않는다. lopdf 0.44의 인라인
//! 이미지 파서는 해석하지 못한 이미지를 빈 연산자로 바꾸고 필터가 걸린 인라인 이미지를 지원하지
//! 않아서, 한 번 decode했다 encode하면 이미지가 사라질 수 있다(`parser/mod.rs` `inline_image`).
//! 대신 바이트 범위를 보존하는 자체 토크나이저(`content::lexer`)로 연산자 위치만 찾고, 편집은
//! 원본 바이트에서 그 범위만 잘라 바꾼다.

/// ui 크레이트가 같은 버전의 lopdf 타입을 쓰도록 다시 내보낸다.
pub use lopdf;

pub mod classify;
pub mod content;
pub mod fonts;
pub mod geometry;
pub mod glyphless;
pub mod hocr;
pub mod import;
pub mod insert;
pub mod layout;
pub mod preflight;
pub mod remove;
pub mod resources;
pub mod save;
pub mod txt;
