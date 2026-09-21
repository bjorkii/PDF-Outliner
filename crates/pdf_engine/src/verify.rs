//! OCR 작업 결과 검증(설계 문서 2.6) — 원본과 결과 페이지를 pdfium으로 비교한다.
//!
//! - 렌더 비교: 같은 크기로 렌더링해 픽셀을 비교한다. 보이지 않는 텍스트만 지웠거나 넣었다면
//!   화면은 같아야 한다. 안티앨리어싱 차이를 감안해 작은 허용치를 둔다.
//! - 텍스트 비교: 보이는 텍스트(생성 문자·공백 제외, NFC 아님 — 원본과 같은 추출기로 비교하므로
//!   정규화가 필요 없다)가 같아야 한다.

use crate::text_layer;
use anyhow::{Context, Result};
use pdfium_render::prelude::*;

/// 렌더 비교 해상도 상한(긴 변 픽셀). 스캔 페이지의 72dpi 사용자 공간 크기가 수천 pt인 경우가
/// 있어 해상도가 아니라 크기로 제한한다.
const RENDER_MAX_SIDE: i32 = 1000;
/// 채널 차이가 이보다 큰 픽셀을 "다름"으로 센다.
const CHANNEL_TOLERANCE: u8 = 32;
/// 다른 픽셀이 이 비율(또는 10개)을 넘으면 화면이 바뀐 것으로 본다.
const PIXEL_FRACTION_TOLERANCE: f64 = 0.0002;

#[derive(Debug, Clone, PartialEq)]
pub struct PageSnapshot {
    pub width: i32,
    pub height: i32,
    pub rgba: Vec<u8>,
    /// 보이는 글자(공백·생성 문자 제외)를 이어 붙인 것.
    pub visible_text: String,
    pub invisible_chars: usize,
}

pub fn snapshot(page: &PdfPage) -> Result<PageSnapshot> {
    let bitmap = page
        .render_with_config(
            &PdfRenderConfig::new()
                .set_maximum_width(RENDER_MAX_SIDE)
                .set_maximum_height(RENDER_MAX_SIDE)
                .render_form_data(true),
        )
        .context("검증용 렌더링 실패")?;
    let chars = text_layer::page_chars(page).unwrap_or_default();
    let visible_text = chars
        .iter()
        .filter(|c| !c.invisible && !c.generated && !c.ch.is_whitespace())
        .map(|c| c.ch)
        .collect();
    let invisible_chars = chars.iter().filter(|c| c.invisible).count();
    Ok(PageSnapshot {
        width: bitmap.width(),
        height: bitmap.height(),
        rgba: bitmap.as_rgba_bytes(),
        visible_text,
        invisible_chars,
    })
}

/// 두 스냅샷이 "화면과 보이는 텍스트가 같음"인지. 다르면 이유.
pub fn compare(before: &PageSnapshot, after: &PageSnapshot) -> Result<(), String> {
    if (before.width, before.height) != (after.width, after.height) {
        return Err(format!(
            "페이지 크기가 바뀜({}×{} → {}×{})",
            before.width, before.height, after.width, after.height
        ));
    }
    let differing = before
        .rgba
        .chunks_exact(4)
        .zip(after.rgba.chunks_exact(4))
        .filter(|(a, b)| a.iter().zip(b.iter()).any(|(x, y)| x.abs_diff(*y) > CHANNEL_TOLERANCE))
        .count();
    let total = (before.width.max(0) as usize) * (before.height.max(0) as usize);
    let allowed = ((total as f64 * PIXEL_FRACTION_TOLERANCE) as usize).max(10);
    if differing > allowed {
        return Err(format!("화면이 달라짐(픽셀 {differing}개 차이)"));
    }
    if before.visible_text != after.visible_text {
        return Err("보이는 텍스트가 달라짐".to_string());
    }
    Ok(())
}
