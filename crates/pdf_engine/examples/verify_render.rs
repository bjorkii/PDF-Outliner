//! 실제 pdfium 라이브러리 + 실제 PDF 파일로 렌더링/텍스트 선택 로직을 검증하는 스크립트.
//! ui 크레이트(viewer_panel.rs)가 호출하는 것과 동일한 pdf_engine 함수들을 그대로 사용해,
//! GUI 없이도 핵심 로직(문자 인덱스 히트테스트, range 텍스트 추출, quad 계산)이 실제
//! pdfium 바인딩에서 올바르게 동작하는지 확인한다.
//!
//! 사용법: cargo run --example verify_render -- <pdfium_dylib_path> <pdf_path>

use pdf_engine::selection::{char_index_at_point, extract_text, selection_quads, TextSelectionRange};
use pdf_engine::PdfEngine;
use pdfium_render::prelude::*;
use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let lib_path = PathBuf::from(args.next().expect("첫 번째 인자: pdfium dylib 경로"));
    let pdf_path = PathBuf::from(args.next().expect("두 번째 인자: PDF 파일 경로"));

    println!("[1] pdfium 라이브러리 로드: {:?}", lib_path);
    let engine = PdfEngine::new_with_library_path(&lib_path)?;

    println!("[2] PDF 문서 열기: {:?}", pdf_path);
    let document = engine.open_document(&pdf_path)?;
    let page_count = document.pages().len();
    println!("    총 페이지 수: {}", page_count);
    assert!(page_count > 0, "페이지가 0개면 열기 자체가 실패한 것");

    let page = document.pages().get(0)?;
    println!(
        "    페이지 크기(포인트): {:.1} x {:.1}",
        page.width().value,
        page.height().value
    );

    // --- 렌더링 검증 ---
    println!("\n[3] 페이지 렌더링 (target_width=900px)");
    let config = PdfRenderConfig::new().set_target_width(900);
    let bitmap = page.render_with_config(&config)?;
    let (w, h) = (bitmap.width(), bitmap.height());
    println!("    비트맵 크기: {}x{}", w, h);
    assert!(w > 0 && h > 0, "비트맵 크기가 0이면 렌더링 실패");

    let rgba = bitmap.as_rgba_bytes();
    let expected_len = (w as usize) * (h as usize) * 4;
    assert_eq!(rgba.len(), expected_len, "RGBA 버퍼 크기가 w*h*4와 불일치");

    // 완전히 새하얀 빈 비트맵이 아닌지 확인 (텍스트가 그려졌다면 어두운 픽셀이 존재해야 함)
    let non_white_pixels = rgba
        .chunks_exact(4)
        .filter(|px| px[0] < 250 || px[1] < 250 || px[2] < 250)
        .count();
    println!("    흰색이 아닌 픽셀 수: {} / {}", non_white_pixels, w * h);
    assert!(
        non_white_pixels > 100,
        "렌더링된 비트맵에 텍스트로 보이는 어두운 픽셀이 거의 없음 — 렌더링이 비어있을 가능성"
    );

    // --- 텍스트 레이어 검증 ---
    println!("\n[4] 텍스트 레이어 추출 (text_page.all())");
    let text_page = page.text()?;
    let full_text = text_page.all();
    println!("    전체 텍스트 길이: {} chars", full_text.chars().count());
    let preview: String = full_text.chars().take(40).collect();
    println!("    미리보기: {:?}", preview);
    assert!(
        full_text.contains("PDF Bookmark Editor"),
        "예상한 영문 문구를 찾지 못함 — ToUnicode 매핑 문제 가능성"
    );
    assert!(
        full_text.contains("렌더링"),
        "예상한 한글 문구를 찾지 못함 — 한글 인코딩 문제 가능성"
    );

    // --- 문자 인덱스 히트테스트 왕복 검증 ---
    // (screen click -> char index 로직이 UI에서 쓰는 것과 동일한 함수)
    println!("\n[5] 문자 인덱스 히트테스트 왕복 검증 (char_index_at_point)");
    let target_char_index: i32 = 5; // 첫 줄 중간쯤 문자 하나
    let chars = text_page.chars();
    let target_char = chars.get(target_char_index as usize)?;
    let bounds = target_char.loose_bounds()?;
    let center_x = (bounds.left().value + bounds.right().value) / 2.0;
    let center_y = (bounds.top().value + bounds.bottom().value) / 2.0;
    println!(
        "    문자 인덱스 {} ({:?})의 중심점: ({:.1}, {:.1})",
        target_char_index,
        target_char.unicode_char(),
        center_x,
        center_y
    );

    let tolerance = PdfPoints::new(4.0);
    let found = char_index_at_point(
        &text_page,
        PdfPoints::new(center_x),
        PdfPoints::new(center_y),
        tolerance,
        tolerance,
    );
    println!("    히트테스트 결과 인덱스: {:?}", found);
    assert_eq!(
        found,
        Some(target_char_index),
        "문자 중심점을 클릭했는데 다른 인덱스가 반환됨 — 좌표 변환 로직 문제"
    );

    // --- range 텍스트 추출 검증 ---
    println!("\n[6] range 텍스트 추출 검증 (extract_text)");
    let range = TextSelectionRange::from_anchors(0, 19); // "PDF Bookmark Editor " 부근
    let extracted = extract_text(&text_page, range)?;
    println!("    range(0..=19) 추출 결과: {:?}", extracted);
    assert!(
        extracted.starts_with("PDF Bookmark"),
        "range 추출 텍스트가 예상과 다름"
    );

    // --- 선택 quad 검증 ---
    println!("\n[7] 선택 하이라이트 quad 검증 (selection_quads)");
    let quads = selection_quads(&text_page, range)?;
    println!("    quad 개수: {}", quads.len());
    assert_eq!(
        quads.len() as i32,
        range.count(),
        "quad 개수가 선택한 문자 개수와 일치하지 않음"
    );
    for (i, q) in quads.iter().take(3).enumerate() {
        println!(
            "    quad[{}]: top_left={:?} bottom_right={:?} rotation={:.3}rad",
            i, q.top_left, q.bottom_right, q.rotation_radians
        );
        assert!(
            q.top_left.0 != q.bottom_right.0 || q.top_left.1 != q.bottom_right.1,
            "quad가 퇴화(점으로 붕괴)됨"
        );
    }

    // --- 화면 좌표 <-> PDF 포인트 왕복 (viewer_panel.rs가 쓰는 것과 동일한 API) ---
    println!("\n[8] pixels_to_points / points_to_pixels 왕복 검증");
    let (px, py) = page.points_to_pixels(PdfPoints::new(center_x), PdfPoints::new(center_y), &config)?;
    let (back_x, back_y) = page.pixels_to_points(px, py, &config)?;
    println!(
        "    포인트({:.1},{:.1}) -> 픽셀({},{}) -> 포인트({:.1},{:.1})",
        center_x, center_y, px, py, back_x.value, back_y.value
    );
    assert!(
        (back_x.value - center_x).abs() < 1.0 && (back_y.value - center_y).abs() < 1.0,
        "좌표 왕복 변환 오차가 1pt를 초과함"
    );

    println!("\n모든 검증 통과.");
    Ok(())
}
