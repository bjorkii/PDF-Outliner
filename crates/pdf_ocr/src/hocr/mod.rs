//! hOCR 읽기·쓰기(설계 문서 5.3, 6.1).

pub mod parse;
pub mod write;

pub use parse::{parse, HocrLine, HocrPage, HocrWord};
pub use write::HocrWriter;
