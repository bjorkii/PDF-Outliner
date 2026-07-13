//! 임의의 실제 PDF 파일에 대한 범용 스모크 테스트. `verify_render.rs`는 합성 테스트
//! PDF의 정확한 문구를 assert하는 반면, 이 스크립트는 구조적 정상성만 확인한다:
//! 렌더링이 비어있지 않은지, 텍스트 레이어가 있다면 히트테스트/range 추출/quad 계산이
//! 에러 없이 왕복하는지. 실제 사용자 문서(스캔본/이미지 전용 페이지 포함 가능)를
//! 대상으로 하므로 텍스트 레이어가 없는 페이지도 정상 케이스로 취급한다.
//!
//! 사용법: cargo run --example smoke_test -p pdf_engine -- <pdfium_dylib_path> <pdf_path> [max_pages]

use pdf_engine::selection::{char_index_at_point, extract_text, selection_quads, TextSelectionRange};
use pdf_engine::PdfEngine;
use pdfium_render::prelude::*;
use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let lib_path = PathBuf::from(args.next().expect("첫 번째 인자: pdfium dylib 경로"));
    let pdf_path = PathBuf::from(args.next().expect("두 번째 인자: PDF 파일 경로"));
    let max_pages: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(3);

    let engine = PdfEngine::new_with_library_path(&lib_path)?;
    let document = engine.open_document(&pdf_path)?;
    let page_count = document.pages().len();

    println!("파일: {}", pdf_path.display());
    println!("총 페이지 수: {}", page_count);

    let pages_to_check = (page_count as usize).min(max_pages);
    for i in 0..pages_to_check {
        let page = document.pages().get(i as PdfPageIndex)?;
        println!("\n--- 페이지 {} ---", i + 1);
        println!(
            "  크기(pt): {:.1} x {:.1}",
            page.width().value,
            page.height().value
        );

        let config = PdfRenderConfig::new().set_target_width(900);
        let bitmap = page.render_with_config(&config)?;
        let (w, h) = (bitmap.width(), bitmap.height());
        let rgba = bitmap.as_rgba_bytes();
        assert_eq!(rgba.len(), (w as usize) * (h as usize) * 4, "RGBA 버퍼 크기 불일치");
        let non_white = rgba
            .chunks_exact(4)
            .filter(|px| px[0] < 250 || px[1] < 250 || px[2] < 250)
            .count();
        println!(
            "  렌더링: {}x{}, 비-흰색 픽셀 {} ({:.2}%)",
            w,
            h,
            non_white,
            100.0 * non_white as f64 / (w as f64 * h as f64)
        );

        let text_page = page.text()?;
        let full_text = text_page.all();
        let char_count = full_text.chars().count();
        println!("  텍스트 레이어 문자 수: {}", char_count);

        if char_count == 0 {
            println!("  텍스트 레이어 없음 — 이미지 전용/스캔 페이지일 수 있음(선택 불가는 정상 동작)");
            continue;
        }

        let preview: String = full_text
            .chars()
            .filter(|c| !c.is_control())
            .take(60)
            .collect();
        println!("  미리보기: {:?}", preview);

        let chars = text_page.chars();
        let total_chars = chars.len();
        if total_chars == 0 {
            continue;
        }

        // 히트테스트 왕복: 문서 중간 지점 문자 하나로 검증
        let idx = (total_chars / 2) as i32;
        let ch = chars.get(idx as usize)?;
        let bounds = ch.loose_bounds()?;
        let cx = (bounds.left().value + bounds.right().value) / 2.0;
        let cy = (bounds.top().value + bounds.bottom().value) / 2.0;
        let tol = PdfPoints::new(4.0);
        let found = char_index_at_point(
            &text_page,
            PdfPoints::new(cx),
            PdfPoints::new(cy),
            tol,
            tol,
        );
        let angle = ch.angle_radians().unwrap_or(0.0);
        println!(
            "  히트테스트 왕복(index {}, 문자 {:?}, 회전 {:.3}rad): {}",
            idx,
            ch.unicode_char(),
            angle,
            if found == Some(idx) {
                "OK".to_string()
            } else {
                format!("MISMATCH (got {:?})", found)
            }
        );

        let range_end = (idx + 10).min(total_chars as i32 - 1);
        let range = TextSelectionRange::from_anchors(idx, range_end);
        match extract_text(&text_page, range) {
            Ok(text) => println!("  range 추출({}..={}): {:?}", idx, range_end, text),
            Err(err) => println!("  range 추출 실패: {err}"),
        }
        match selection_quads(&text_page, range) {
            Ok(quads) => println!(
                "  quad 개수: {} (기대: {}) {}",
                quads.len(),
                range.count(),
                if quads.len() as i32 == range.count() {
                    "OK"
                } else {
                    "MISMATCH"
                }
            ),
            Err(err) => println!("  quad 계산 실패: {err}"),
        }
    }

    println!("\n스모크 테스트 완료.");
    Ok(())
}
