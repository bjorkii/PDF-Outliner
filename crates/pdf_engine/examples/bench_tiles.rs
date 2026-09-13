//! 타일(부분 영역) 렌더링 도입 전 위험 요소 진단 도구.
//! (1) /Rotate가 걸린 페이지에서도 영역 렌더가 전체 렌더와 일치하는지
//! (2) 비정수 배율에서 인접한 두 타일을 이어 붙였을 때 경계에 이음새가 생기는지
//! (3) PdfPage를 유지한 채 반복 렌더링 vs 매번 다시 로드했을 때의 시간 차이
//!
//! 사용법: cargo run --release --example bench_tiles -p pdf_engine -- <pdfium_dylib> <pdf> [page_0based]

use pdf_engine::PdfEngine;
use pdfium_render::prelude::*;
use std::path::PathBuf;
use std::time::Instant;

fn region_config(scale: f32, ox: i32, oy: i32, w: i32, h: i32) -> Result<PdfRenderConfig, PdfiumError> {
    PdfRenderConfig::new()
        .set_fixed_size(w, h)
        .scale_page_by_factor(scale)
        .translate(PdfPoints::new(-ox as f32 / scale), PdfPoints::new(-oy as f32 / scale))
}

/// 영역 비트맵(w×h)과 전체 비트맵의 (ox, oy)부터 같은 크기 영역의 채널별 평균 절대 차이.
fn mean_diff(region: &[u8], full: &[u8], full_width: i32, ox: i32, oy: i32, w: i32, h: i32) -> f64 {
    let mut diff = 0_u64;
    for y in 0..h {
        for x in 0..w {
            let a = ((y * w + x) * 4) as usize;
            let b = (((y + oy) * full_width + x + ox) * 4) as usize;
            for c in 0..3 {
                diff += (region[a + c] as i64 - full[b + c] as i64).unsigned_abs();
            }
        }
    }
    diff as f64 / (w as f64 * h as f64 * 3.0)
}

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let lib_path = PathBuf::from(args.next().expect("1: pdfium dylib 경로"));
    let pdf_path = PathBuf::from(args.next().expect("2: PDF 경로"));
    let page_index: PdfPageIndex = args.next().map(|s| s.parse()).transpose()?.unwrap_or(0);

    let engine = PdfEngine::new_with_library_path(&lib_path)?;
    let document = engine.open_document(&pdf_path)?;

    // (1) 회전
    println!("[1] /Rotate 페이지에서 영역 렌더 일치 여부 (평균 픽셀차, 0~255)");
    for rotation in [
        PdfPageRenderRotation::None,
        PdfPageRenderRotation::Degrees90,
        PdfPageRenderRotation::Degrees180,
        PdfPageRenderRotation::Degrees270,
    ] {
        let mut page = document.pages().get(page_index)?;
        let original = page.rotation()?;
        page.set_rotation(rotation);
        {
            let full = page.render_with_config(&PdfRenderConfig::new().set_target_width(3000))?;
            let (fw, fh) = (full.width(), full.height());
            let full_rgba = full.as_rgba_bytes();
            let scale = fw as f32 / page.width().value;
            let (w, h) = (800.min(fw), 600.min(fh));
            let (ox, oy) = ((fw - w) / 3, (fh - h) / 3);
            let region = page.render_with_config(&region_config(scale, ox, oy, w, h)?)?;
            println!(
                "  {:?}: 페이지 {}×{}pt, 전체 {}×{}px, 차이 {:.2}",
                rotation,
                page.width().value,
                page.height().value,
                fw,
                fh,
                mean_diff(&region.as_rgba_bytes(), &full_rgba, fw, ox, oy, w, h)
            );
        }
        page.set_rotation(original);
    }

    // (2) 비정수 배율에서 타일 경계
    println!("\n[2] 좌우 두 타일 경계 이음새 (배율 3.37, 경계 ±2열 vs 타일 내부)");
    let page = document.pages().get(page_index)?;
    let target_width = (page.width().value * 3.37) as i32;
    let full = page.render_with_config(&PdfRenderConfig::new().set_target_width(target_width))?;
    let (fw, fh) = (full.width(), full.height());
    drop(full);
    let scale = fw as f32 / page.width().value;
    let (tile_w, tile_h) = (512, 512.min(fh));
    let ox = fw / 2 - tile_w;
    let oy = (fh - tile_h) / 2;
    // 기준: 같은 행렬 경로로 두 타일 폭을 한 번에 렌더링한 영역. 전체 렌더(행렬 없는 API
    // 경로)와의 차이를 배제하고 순수하게 "타일로 나눈 탓"만 본다.
    let reference = {
        let pad = 32;
        let (bw, bh) = (tile_w * 2 + 2 * pad, tile_h + 2 * pad);
        let rgba = page
            .render_with_config(&region_config(scale, ox - pad, oy - pad, bw, bh)?)?
            .as_rgba_bytes();
        let mut core = Vec::with_capacity((tile_w * 2 * tile_h * 4) as usize);
        for y in 0..tile_h {
            let start = (((y + pad) * bw + pad) * 4) as usize;
            core.extend_from_slice(&rgba[start..start + (tile_w * 2 * 4) as usize]);
        }
        core
    };
    // bleed: 타일을 사방 bleed px만큼 크게 렌더링한 뒤 가운데만 잘라 쓴다.
    for bleed in [0, 4, 8, 16] {
        let render_core = |x0: i32| -> anyhow::Result<Vec<u8>> {
            let (bw, bh) = (tile_w + 2 * bleed, tile_h + 2 * bleed);
            let rgba = page
                .render_with_config(&region_config(scale, x0 - bleed, oy - bleed, bw, bh)?)?
                .as_rgba_bytes();
            let mut core = Vec::with_capacity((tile_w * tile_h * 4) as usize);
            for y in 0..tile_h {
                let start = (((y + bleed) * bw + bleed) * 4) as usize;
                core.extend_from_slice(&rgba[start..start + (tile_w * 4) as usize]);
            }
            Ok(core)
        };
        let left = render_core(ox)?;
        let right = render_core(ox + tile_w)?;
        let (mut seam, mut seam_n, mut inner, mut inner_n, mut seam_max) = (0_u64, 0_u64, 0_u64, 0_u64, 0_u64);
        for y in 0..tile_h {
            for x in 0..tile_w * 2 {
                let (src, sx) = if x < tile_w { (&left, x) } else { (&right, x - tile_w) };
                let a = ((y * tile_w + sx) * 4) as usize;
                let b = ((y * tile_w * 2 + x) * 4) as usize;
                let d: u64 = (0..3).map(|c| (src[a + c] as i64 - reference[b + c] as i64).unsigned_abs()).sum();
                if (x - tile_w).abs() <= 2 {
                    seam += d;
                    seam_n += 3;
                    seam_max = seam_max.max(d / 3);
                } else {
                    inner += d;
                    inner_n += 3;
                }
            }
        }
        println!(
            "  bleed {bleed}px: 경계 평균 {:.2} (최대 {}), 내부 평균 {:.2}",
            seam as f64 / seam_n as f64,
            seam_max,
            inner as f64 / inner_n as f64
        );
    }

    // (3) 페이지 유지 vs 재로드
    println!("\n[3] 800×600 영역 5회 렌더: 페이지 유지 vs 매번 재로드");
    let scale = 4.0 * 1800.0 / page.width().value;
    let config = region_config(scale, 2000, 3000, 800, 600)?;
    let t = Instant::now();
    for _ in 0..5 {
        page.render_with_config(&config)?;
    }
    println!("  유지: 회당 {:.1?}", t.elapsed() / 5);
    drop(page);
    let t = Instant::now();
    for _ in 0..5 {
        let fresh = document.pages().get(page_index)?;
        fresh.render_with_config(&config)?;
    }
    println!("  재로드: 회당 {:.1?}", t.elapsed() / 5);
    Ok(())
}
