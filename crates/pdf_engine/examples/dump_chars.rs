//! 특정 페이지의 모든 문자를 인덱스/좌표/회전각과 함께 덤프한다.
//! 세로쓰기·디자인 레이아웃처럼 문자 인덱스 순서가 실제 읽기 순서와 다를 수 있는
//! 케이스를 진단하기 위한 도구.
//!
//! 사용법: cargo run --example dump_chars -p pdf_engine -- <pdfium_dylib_path> <pdf_path> <page_index_0based>

use pdf_engine::PdfEngine;
use pdfium_render::prelude::*;
use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let lib_path = PathBuf::from(args.next().expect("첫 번째 인자: pdfium dylib 경로"));
    let pdf_path = PathBuf::from(args.next().expect("두 번째 인자: PDF 파일 경로"));
    let page_index: PdfPageIndex = args
        .next()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);

    let engine = PdfEngine::new_with_library_path(&lib_path)?;
    let document = engine.open_document(&pdf_path)?;
    let page = document.pages().get(page_index)?;
    let text_page = page.text()?;
    let chars = text_page.chars();

    println!(
        "페이지 {} 문자 수: {} (페이지 크기 {:.0}x{:.0}pt)",
        page_index,
        chars.len(),
        page.width().value,
        page.height().value
    );
    println!("{:>4} {:>6} {:>8} {:>8} {:>10}", "idx", "char", "x", "y", "angle(rad)");

    for (i, ch) in chars.iter().enumerate() {
        let bounds = ch.loose_bounds();
        let (x, y, w, h) = bounds
            .map(|b| {
                (
                    (b.left().value + b.right().value) / 2.0,
                    (b.top().value + b.bottom().value) / 2.0,
                    (b.right().value - b.left().value).abs(),
                    (b.top().value - b.bottom().value).abs(),
                )
            })
            .unwrap_or((f32::NAN, f32::NAN, f32::NAN, f32::NAN));
        let angle = ch.angle_radians().unwrap_or(f32::NAN);
        println!(
            "{:>4} {:>6?} {:>8.1} {:>8.1}  w={:>7.1} h={:>7.1} {:>10.3}",
            i,
            ch.unicode_char(),
            x,
            y,
            w,
            h,
            angle
        );
    }

    Ok(())
}
