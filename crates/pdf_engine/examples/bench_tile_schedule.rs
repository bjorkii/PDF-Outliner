//! 타일 렌더링 스케줄 진단 — ui::tile_render가 한 화면을 채우는 과정을 pdfium만으로 재현해
//! "선명해지기까지 몇 ms/몇 프레임", "한 프레임을 막는 최대 시간", "페이지 넘김 비용"을 잰다.
//! 기존 전체 렌더(한 장) 시간과 나란히 출력해 비교한다.
//!
//! 사용법: cargo run --release --example bench_tile_schedule -p pdf_engine -- <pdfium_dylib> <pdf> [page_0based]

use pdf_engine::PdfEngine;
use pdfium_render::prelude::*;
use std::path::PathBuf;
use std::time::{Duration, Instant};

// ui/src/tile_render.rs와 같은 값.
const TILE_SIZE: i32 = 1024;
const TILE_BLEED: i32 = 16;
const FRAME_BUDGET: Duration = Duration::from_millis(6);
const UNDERLAY_MAX_SIDE: f32 = 1600.0;
// 기본 창 크기의 뷰어 영역(Retina 물리 px) 근사.
const VIEW_W: i32 = 1800;
const VIEW_H: i32 = 1400;

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let lib_path = PathBuf::from(args.next().expect("1: pdfium dylib 경로"));
    let pdf_path = PathBuf::from(args.next().expect("2: PDF 경로"));
    let page_index: PdfPageIndex = args.next().map(|s| s.parse()).transpose()?.unwrap_or(0);

    let engine = PdfEngine::new_with_library_path(&lib_path)?;
    let document = engine.open_document(&pdf_path)?;

    // 페이지 넘김 비용: 처음 로드 / 바탕 렌더 / 같은 페이지 재로드
    let t = Instant::now();
    let page = document.pages().get(page_index)?;
    let first_load = t.elapsed();
    let (pw, ph) = (page.width().value, page.height().value);
    let underlay_width = if ph > pw { UNDERLAY_MAX_SIDE * pw / ph } else { UNDERLAY_MAX_SIDE };
    let t = Instant::now();
    let _ = page
        .render_with_config(&PdfRenderConfig::new().set_target_width(underlay_width as i32))?
        .as_rgba_bytes();
    let underlay = t.elapsed();
    drop(page);
    let t = Instant::now();
    let page = document.pages().get(page_index)?;
    let reload = t.elapsed();
    println!(
        "페이지 {} ({}×{}pt): 첫 로드 {:.1}ms, 재로드 {:.1}ms, 바탕({}px 폭) {:.1}ms",
        page_index + 1,
        pw,
        ph,
        ms(first_load),
        ms(reload),
        underlay_width as i32,
        ms(underlay)
    );

    println!("\n배율 | 기존 전체1장 | 보이는타일 수·합계 | 여유타일 수·합계 | 최대1장 | 선명까지 프레임");
    for zoom in [1.0_f32, 2.0, 4.0, 8.0] {
        let page_width = ((VIEW_W as f32 * zoom) as i32).min(16384).min((16384.0 * pw / ph) as i32);
        let scale = page_width as f32 / pw;
        let page_h = (ph * scale).round() as i32;

        let t = Instant::now();
        let _ = page
            .render_with_config(&PdfRenderConfig::new().set_target_width(page_width))?
            .as_rgba_bytes();
        let legacy = t.elapsed();

        // 페이지 중앙을 보는 화면
        let vx0 = ((page_width - VIEW_W) / 2).max(0);
        let vy0 = ((page_h - VIEW_H) / 2).max(0);
        let visible = [vx0, vy0, (vx0 + VIEW_W).min(page_width), (vy0 + VIEW_H).min(page_h)];
        let pad = TILE_SIZE / 2;
        let margin = [visible[0] - pad, visible[1] - pad, visible[2] + pad, visible[3] + pad];
        let tiles_in = |r: [i32; 4]| -> Vec<(i32, i32)> {
            let (x0, y0, x1, y1) = (r[0].max(0), r[1].max(0), r[2].min(page_width), r[3].min(page_h));
            let mut v = Vec::new();
            for row in y0 / TILE_SIZE..=(y1 - 1) / TILE_SIZE {
                for col in x0 / TILE_SIZE..=(x1 - 1) / TILE_SIZE {
                    v.push((col, row));
                }
            }
            v
        };
        let visible_tiles = tiles_in(visible);
        let margin_tiles: Vec<_> = tiles_in(margin)
            .into_iter()
            .filter(|t| !visible_tiles.contains(t))
            .collect();

        let render_tile = |(col, row): (i32, i32)| -> anyhow::Result<Duration> {
            let (x0, y0) = (col * TILE_SIZE, row * TILE_SIZE);
            let w = TILE_SIZE.min(page_width - x0) + 2 * TILE_BLEED;
            let h = TILE_SIZE.min(page_h - y0) + 2 * TILE_BLEED;
            let t = Instant::now();
            let config = PdfRenderConfig::new()
                .set_fixed_size(w, h)
                .scale_page_by_factor(scale)
                .translate(
                    PdfPoints::new(-(x0 - TILE_BLEED) as f32 / scale),
                    PdfPoints::new(-(y0 - TILE_BLEED) as f32 / scale),
                )?;
            let _ = page.render_with_config(&config)?.as_rgba_bytes();
            Ok(t.elapsed())
        };

        // 프레임 시뮬레이션: 프레임마다 페이지 재로드 1회 + 예산 안에서 타일(최소 1장)
        let mut frames_to_sharp = 0;
        let mut max_tile = Duration::ZERO;
        let (mut visible_sum, mut margin_sum) = (Duration::ZERO, Duration::ZERO);
        let mut queue: Vec<(i32, i32, bool)> = visible_tiles
            .iter()
            .map(|&(c, r)| (c, r, true))
            .chain(margin_tiles.iter().map(|&(c, r)| (c, r, false)))
            .collect();
        queue.reverse();
        let mut visible_left = visible_tiles.len();
        while !queue.is_empty() {
            let frame_start = Instant::now();
            // 이 프레임을 시작할 때 아직 보이는 타일이 남아 있으면 "선명해지기 전" 프레임.
            if visible_left > 0 {
                frames_to_sharp += 1;
            }
            let _reloaded = document.pages().get(page_index)?;
            while let Some(&(c, r, is_visible)) = queue.last() {
                if frame_start.elapsed() >= FRAME_BUDGET {
                    break;
                }
                queue.pop();
                let d = render_tile((c, r))?;
                max_tile = max_tile.max(d);
                if is_visible {
                    visible_sum += d;
                    visible_left -= 1;
                } else {
                    margin_sum += d;
                }
            }
        }

        println!(
            "{:>4.0}% | {:>9.1}ms | {:>2}장 {:>7.1}ms | {:>2}장 {:>7.1}ms | {:>6.1}ms | {:>3}프레임",
            zoom * 100.0,
            ms(legacy),
            visible_tiles.len(),
            ms(visible_sum),
            margin_tiles.len(),
            ms(margin_sum),
            ms(max_tile),
            frames_to_sharp
        );
    }
    Ok(())
}
