//! 페이지를 렌더링해 PNG로 저장한다. 화면 캡처가 불가능한 세션에서 실제 렌더링 결과를
//! 눈으로 확인하기 위한 진단 도구. 문자 인덱스를 주면 그 문자 주변을 크롭한다.
//!
//! 사용법: cargo run --example render_crop -p pdf_engine -- <pdfium_dylib> <pdf> <page_0based> <out.png> [char_index]

use pdf_engine::PdfEngine;
use pdfium_render::prelude::*;
use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let lib_path = PathBuf::from(args.next().expect("1: pdfium dylib 경로"));
    let pdf_path = PathBuf::from(args.next().expect("2: PDF 경로"));
    let page_index: PdfPageIndex = args.next().expect("3: 페이지 인덱스").parse()?;
    let out_path = PathBuf::from(args.next().expect("4: 출력 PNG 경로"));
    let char_index: Option<i32> = args.next().and_then(|s| s.parse().ok());

    let engine = PdfEngine::new_with_library_path(&lib_path)?;
    let document = engine.open_document(&pdf_path)?;
    let page = document.pages().get(page_index)?;

    let target_width = 1600;
    let config = PdfRenderConfig::new().set_target_width(target_width);
    let bitmap = page.render_with_config(&config)?;
    let image = bitmap.as_image()?;

    if let Some(idx) = char_index {
        let text_page = page.text()?;
        let ch = text_page.chars().get(idx as usize)?;
        let bounds = ch.loose_bounds()?;
        let (px_left, px_top) = page.points_to_pixels(bounds.left(), bounds.top(), &config)?;
        let (px_right, px_bottom) = page.points_to_pixels(bounds.right(), bounds.bottom(), &config)?;

        let margin = 60i32;
        let x0 = px_left.min(px_right) - margin;
        let y0 = px_top.min(px_bottom) - margin;
        let x1 = px_left.max(px_right) + margin;
        let y1 = px_top.max(px_bottom) + margin;

        let x0 = x0.clamp(0, image.width() as i32) as u32;
        let y0 = y0.clamp(0, image.height() as i32) as u32;
        let x1 = x1.clamp(0, image.width() as i32) as u32;
        let y1 = y1.clamp(0, image.height() as i32) as u32;

        println!(
            "문자 {:?}(idx {}) 크롭 영역(px): ({},{}) - ({},{})",
            ch.unicode_char(),
            idx,
            x0,
            y0,
            x1,
            y1
        );

        let cropped = image.crop_imm(x0, y0, x1.saturating_sub(x0).max(1), y1.saturating_sub(y0).max(1));
        cropped.save(&out_path)?;
    } else {
        image.save(&out_path)?;
    }

    println!("저장됨: {:?}", out_path);
    Ok(())
}
