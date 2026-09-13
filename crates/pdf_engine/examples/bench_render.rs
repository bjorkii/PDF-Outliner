//! 고배율 렌더링 비용 진단 도구. 배율별로 (a) 페이지 전체를 그 해상도로 렌더링하는 현재
//! 방식과 (b) 화면 크기 영역만 렌더링하는 방식(고정 크기 비트맵 + 이동 행렬)의 시간을 재고,
//! (b)가 (a)의 같은 영역과 픽셀 단위로 일치하는지 평균 차이로 확인한다.
//!
//! 사용법: cargo run --release --example bench_render -p pdf_engine -- <pdfium_dylib> <pdf> [page_0based] [panel_px]

use pdf_engine::PdfEngine;
use pdfium_render::prelude::*;
use std::path::PathBuf;
use std::time::Instant;

const VIEW_HEIGHT_PX: i32 = 1200;

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let lib_path = PathBuf::from(args.next().expect("1: pdfium dylib 경로"));
    let pdf_path = PathBuf::from(args.next().expect("2: PDF 경로"));
    let page_index: PdfPageIndex = args.next().map(|s| s.parse()).transpose()?.unwrap_or(0);
    let panel_px: i32 = args.next().map(|s| s.parse()).transpose()?.unwrap_or(1800);

    let engine = PdfEngine::new_with_library_path(&lib_path)?;
    let t = Instant::now();
    let document = engine.open_document(&pdf_path)?;
    println!("문서 열기 {:?}, 총 {}쪽", t.elapsed(), document.pages().len());

    let t = Instant::now();
    let page = document.pages().get(page_index)?;
    let load = t.elapsed();
    let t = Instant::now();
    let text_chars = page.text().map(|text| text.chars().len()).unwrap_or(0);
    println!(
        "페이지 {} 로드 {:?}, 텍스트 레이어 {:?}({}자), 크기 {}×{}pt",
        page_index + 1,
        load,
        t.elapsed(),
        text_chars,
        page.width().value,
        page.height().value
    );

    let (pw, ph) = (page.width().value, page.height().value);
    println!("\n배율 | 전체 비트맵 | 전체 렌더 | RGBA 변환 | 영역 렌더(+변환) | 영역 픽셀차");
    for zoom in [1.0_f32, 2.0, 4.0, 6.0, 8.0] {
        let height_bound = (16384.0 * pw / ph) as i32;
        let requested = ((panel_px as f32 * zoom) as i32).min(16384).min(height_bound);

        let t = Instant::now();
        let full = page.render_with_config(&PdfRenderConfig::new().set_target_width(requested))?;
        let full_time = t.elapsed();
        let t = Instant::now();
        let full_rgba = full.as_rgba_bytes();
        let convert_time = t.elapsed();
        let (width, height) = (full.width(), full.height());
        let scale = width as f32 / pw;

        // 페이지 중앙의 화면 크기 영역.
        let vw = panel_px.min(width);
        let vh = VIEW_HEIGHT_PX.min(height);
        let (ox, oy) = ((width - vw) / 2, (height - vh) / 2);

        let t = Instant::now();
        let config = PdfRenderConfig::new()
            .set_fixed_size(vw, vh)
            .scale_page_by_factor(scale)
            .translate(PdfPoints::new(-ox as f32 / scale), PdfPoints::new(-oy as f32 / scale))?;
        let view = page.render_with_config(&config)?;
        let view_rgba = view.as_rgba_bytes();
        let view_time = t.elapsed();

        let mut diff = 0_u64;
        for y in 0..vh {
            for x in 0..vw {
                let a = ((y * vw + x) * 4) as usize;
                let b = (((y + oy) * width + x + ox) * 4) as usize;
                for c in 0..3 {
                    diff += (view_rgba[a + c] as i64 - full_rgba[b + c] as i64).unsigned_abs();
                }
            }
        }
        let mean_diff = diff as f64 / (vw as f64 * vh as f64 * 3.0);

        println!(
            "{:>4.0}% | {:>5}×{:<5} | {:>9.1?} | {:>9.1?} | {:>9.1?} | {:.2}",
            zoom * 100.0,
            width,
            height,
            full_time,
            convert_time,
            view_time,
            mean_diff
        );
    }
    Ok(())
}
