//! OCR 내보내기·검증용 문자 추출(설계 문서 5.1, 5.2).
//!
//! pdfium 텍스트 페이지의 문자를 순서대로 꺼내 위치와 "보이지 않는 텍스트" 여부를 붙인다.
//! 단어·줄 구성과 좌표 변환은 pdfium과 무관한 순수 로직이라 `pdf_ocr::layout`이 맡는다.
//!
//! - 위치는 **loose char box**를 쓴다. OCR 레이어에 흔한 glyphless 폰트는 글리프 외곽선이 비어
//!   있어 tight box가 0 크기로 나올 수 있다. loose box는 폰트 ascent·descent와 advance 폭으로
//!   계산된다.
//! - 보이지 않는 텍스트 판정(설계 3장 A·B·D·E를 pdfium이 볼 수 있는 범위에서):
//!   render mode 3·7, 채우기 알파 0(채우기 모드일 때), loose box 면적이 사실상 0.
//!   흰 글씨·이미지 아래 텍스트(G~J)는 판정하지 않는다.
//! - pdfium이 만들어 넣은 문자(단어 사이 공백, 줄바꿈)는 `generated`로 표시해 구분자로만 쓴다.
//! - pdfium이 줄 끝 하이픈을 나타내는 U+0002는 `-`로 되돌린다.

use anyhow::{Context, Result};
use pdfium_render::prelude::*;

#[derive(Debug, Clone, PartialEq)]
pub struct PageChar {
    pub ch: char,
    /// loose box — 페이지 사용자 공간 `[left, bottom, right, top]`.
    pub bounds: [f64; 4],
    /// 글리프 원점(기준선 위의 점), 사용자 공간.
    pub origin: Option<(f64, f64)>,
    /// pdfium이 추론해 넣은 공백·줄바꿈(원본 콘텐츠에는 없는 문자).
    pub generated: bool,
    /// 어떤 이유로든 화면에 그려지지 않는다(렌더 모드 3·7, 알파 0, 크기 0).
    pub invisible: bool,
    /// 렌더 모드가 **`3 Tr`(Invisible)** 이다 — OCR 도구가 쓰는 기법.
    ///
    /// `invisible`과 따로 두는 이유: OCR 텍스트의 정의가 "이미지 영역 안이거나 걸친 `3 Tr`"이라
    /// (2026-09-28 확정), `7 Tr`·알파 0·크기 0은 안 보이기는 해도 OCR이 아니다. 내보내기가 이
    /// 둘을 구분해야 한다.
    pub invisible_mode: bool,
}

/// 페이지의 표시 영역 정보(pdfium 기준) — `pdf_ocr::geometry::PageFrame`을 만들 때 쓴다.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PageBox {
    /// CropBox ∩ MediaBox, 사용자 공간 `[left, bottom, right, top]`.
    pub crop: [f64; 4],
    /// 0, 90, 180, 270.
    pub rotate: u16,
}

pub fn page_box(page: &PdfPage) -> Result<PageBox> {
    let rect = page
        .boundaries()
        .bounding()
        .map(|b| b.bounds)
        .or_else(|_| page.boundaries().media().map(|b| b.bounds))
        .context("페이지 영역 조회 실패")?;
    let rotate = match page.rotation().unwrap_or(PdfPageRenderRotation::None) {
        PdfPageRenderRotation::None => 0,
        PdfPageRenderRotation::Degrees90 => 90,
        PdfPageRenderRotation::Degrees180 => 180,
        PdfPageRenderRotation::Degrees270 => 270,
    };
    Ok(PageBox { crop: rect_to_array(&rect), rotate })
}

/// `PdfPage::label()`의 복사본(페이지 레이블이 없으면 None).
pub fn page_label(page: &PdfPage) -> Option<String> {
    page.label().map(str::to_string).filter(|l| !l.is_empty())
}

/// 페이지의 모든 문자(텍스트 레이어가 없으면 빈 목록).
pub fn page_chars(page: &PdfPage) -> Result<Vec<PageChar>> {
    let text_page = page.text().context("텍스트 페이지 로드 실패")?;
    let chars = text_page.chars();
    let mut out = Vec::with_capacity(chars.len());
    for ch in chars.iter() {
        let Some(c) = ch.unicode_char() else {
            continue;
        };
        // pdfium은 줄 끝 하이픈(단어가 다음 줄로 이어지는 표시로 추정한 것)을 U+0002로 돌려준다.
        let c = if c == '\u{2}' { '-' } else { c };
        let generated = ch.is_generated().unwrap_or(false);
        let bounds = ch.loose_bounds().map(|r| rect_to_array(&r)).unwrap_or([0.0; 4]);
        let origin = ch.origin().ok().map(|(x, y)| (x.value as f64, y.value as f64));
        let invisible = !generated && is_invisible(&ch, &bounds);
        let invisible_mode = !generated
            && ch.render_mode().is_ok_and(|mode| matches!(mode, PdfPageTextRenderMode::Invisible));
        out.push(PageChar { ch: c, bounds, origin, generated, invisible, invisible_mode });
    }
    Ok(out)
}

fn is_invisible(ch: &PdfPageTextChar, bounds: &[f64; 4]) -> bool {
    let mode = ch.render_mode().unwrap_or(PdfPageTextRenderMode::Unknown);
    if matches!(mode, PdfPageTextRenderMode::Invisible | PdfPageTextRenderMode::InvisibleClipping) {
        return true;
    }
    let fills = matches!(
        mode,
        PdfPageTextRenderMode::FilledUnstroked | PdfPageTextRenderMode::FilledUnstrokedClipping
    );
    if fills && ch.fill_color().is_ok_and(|c| c.alpha() == 0) {
        return true;
    }
    // 크기 0 폰트·퇴화 행렬(형태 D): 화면에 아무것도 그려지지 않는다.
    let area = (bounds[2] - bounds[0]).abs() * (bounds[3] - bounds[1]).abs();
    area < 1e-6 && !ch.unicode_char().is_some_and(char::is_whitespace)
}

fn rect_to_array(rect: &PdfRect) -> [f64; 4] {
    [
        rect.left().value as f64,
        rect.bottom().value as f64,
        rect.right().value as f64,
        rect.top().value as f64,
    ]
}
