//! 진단용: `text_layer::page_chars` 결과를 문자별로 출력한다(OCR 내보내기의 입력 확인).
//!
//! 사용법: cargo run --example dump_text_layer -p pdf_engine -- <pdfium_dylib> <pdf> <page_0based> [최대 문자 수]

use pdf_engine::{text_layer, PdfEngine};
use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let lib_path = PathBuf::from(args.next().expect("첫 번째 인자: pdfium dylib 경로"));
    let pdf_path = PathBuf::from(args.next().expect("두 번째 인자: PDF 파일 경로"));
    let page_index: i32 = args.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let limit: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(80);

    let engine = PdfEngine::new_with_library_path(&lib_path)?;
    let document = engine.open_document(&pdf_path)?;
    let page = document.pages().get(page_index)?;
    let chars = text_layer::page_chars(&page)?;
    println!(
        "{:?} 레이블 {:?}, 문자 {}개(보이지 않음 {}, 생성 {})",
        text_layer::page_box(&page)?,
        text_layer::page_label(&page),
        chars.len(),
        chars.iter().filter(|c| c.invisible).count(),
        chars.iter().filter(|c| c.generated).count()
    );
    for (i, c) in chars.iter().take(limit).enumerate() {
        let b = c.bounds;
        println!(
            "{i:>5} {:>8?} gen={} inv={} box=[{:.1} {:.1} {:.1} {:.1}] origin={:?}",
            c.ch, c.generated as u8, c.invisible as u8, b[0], b[1], b[2], b[3], c.origin.map(|(x, y)| (x.round(), y.round()))
        );
    }
    Ok(())
}
