//! 상태표시줄에 보여 줄 현재 페이지 정보(크기·용지규격·회전·OCR 유무·유효 스캔 해상도).
//!
//! 값은 두 갈래로 나뉜다. **싼 것**(`measure`: 페이지 상자와 회전)은 페이지를 넘기는 즉시
//! 구할 수 있고, **비싼 것**(`inspect`: 텍스트 레이어 조회와 이미지 메타데이터 순회)은 페이지가
//! 화면에 그려진 뒤에 한 프레임 늦게 구한다 — 페이지 표시가 이 계산에 밀리지 않게 하기 위해서다
//! (사용자 요청, 2026-09-23). 호출 쪽 흐름은 `ui::status_bar` 참고.

use crate::text_layer;
use anyhow::{Context, Result};
use pdfium_render::prelude::*;

/// 1포인트 = 1/72인치.
const POINTS_PER_INCH: f64 = 72.0;
const CM_PER_INCH: f64 = 2.54;

/// 페이지를 넘기는 즉시 구할 수 있는 정보.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PageMetrics {
    /// 화면에 보이는 그대로의 폭·높이(포인트). `/Rotate`가 90·270이면 이미 맞바꾼 값이다.
    pub width_pt: f64,
    pub height_pt: f64,
    /// 0, 90, 180, 270.
    pub rotate: u16,
}

impl PageMetrics {
    pub fn size_cm(&self) -> (f64, f64) {
        (self.width_pt / POINTS_PER_INCH * CM_PER_INCH, self.height_pt / POINTS_PER_INCH * CM_PER_INCH)
    }

    /// 지정한 해상도로 뽑았을 때의 픽셀 크기(반올림).
    pub fn pixels_at(&self, dpi: f64) -> (u32, u32) {
        let convert = |pt: f64| (pt / POINTS_PER_INCH * dpi).round().max(0.0) as u32;
        (convert(self.width_pt), convert(self.height_pt))
    }

    /// 표준 용지규격 이름(A4, Letter 등). 어디에도 맞지 않으면 None.
    pub fn paper_name(&self) -> Option<&'static str> {
        paper_name(self.width_pt, self.height_pt)
    }
}

/// 이 페이지를 채우고 있는 스캔 이미지. 페이지 대부분을 덮는 이미지가 있을 때만 잡는다
/// (`COVERAGE`) — 쪽번호 옆 작은 도장이나 좁은 띠 이미지를 "이 페이지의 스캔"이라고
/// 말하면 픽셀 수가 페이지 크기와 어긋나 도리어 헷갈린다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Scan {
    /// 이미지의 실제 픽셀 수(페이지 회전에 맞춰 가로세로를 맞바꾼 값).
    pub width_px: u32,
    pub height_px: u32,
    /// 페이지에 놓인 크기 기준의 유효 해상도.
    pub dpi: u32,
}

/// 페이지가 그려진 뒤에 구하는 정보.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PageInspection {
    /// 보이지 않는 텍스트(OCR 레이어)가 있는지.
    pub has_ocr: bool,
    /// 이 페이지를 채우는 스캔 이미지. 없으면(디지털 문서 등) None.
    pub scan: Option<Scan>,
}

pub fn measure(page: &PdfPage) -> Result<PageMetrics> {
    let page_box = text_layer::page_box(page).context("페이지 상자 조회 실패")?;
    let [left, bottom, right, top] = page_box.crop;
    let (width, height) = ((right - left).abs(), (top - bottom).abs());
    // 90·270도 회전은 화면에서 가로세로가 바뀐다 — 사용자가 보는 크기를 적는 게 목적이므로
    // 여기서 맞바꿔 둔다.
    let swapped = matches!(page_box.rotate, 90 | 270);
    Ok(PageMetrics {
        width_pt: if swapped { height } else { width },
        height_pt: if swapped { width } else { height },
        rotate: page_box.rotate,
    })
}

pub fn inspect(page: &PdfPage, metrics: &PageMetrics) -> PageInspection {
    PageInspection { has_ocr: has_invisible_text(page), scan: scan(page, metrics) }
}

/// 보이지 않는 텍스트가 한 글자라도 있는지. `text_layer::page_chars`와 같은 판정을 쓰되
/// 하나만 찾으면 바로 끝낸다.
fn has_invisible_text(page: &PdfPage) -> bool {
    text_layer::page_chars(page).map(|chars| chars.iter().any(|c| c.invisible)).unwrap_or(false)
}

/// 스캔 이미지로 볼 최소 면적 — 페이지 면적 대비 비율. 0.6에서 0.9로 올렸다(2026-09-27
/// 요청) — "이 페이지 = 이 스캔"이라고 단언하는 값이므로 여지를 좁게 잡는다. 실제 코퍼스의
/// 책등 스캔 페이지(400 × 3679pt 페이지에 같은 크기 이미지)도 100%라 그대로 통과한다.
const COVERAGE: f32 = 0.9;

/// 페이지에서 **가장 크게 자리를 차지하는 이미지**를 이 페이지의 스캔으로 본다. 유효
/// 해상도는 pdfium이 이미지의 원본 픽셀 수와 페이지에 놓인 크기로 이미 계산해 주는 값
/// (`FPDFImageObj_GetImageMetadata`)이라 이미지를 디코딩하지 않는다. 가로세로 해상도가
/// 다르면 작은 쪽을 쓴다(그쪽이 실제 판독 품질을 좌우한다).
fn scan(page: &PdfPage, metrics: &PageMetrics) -> Option<Scan> {
    let page_area = (metrics.width_pt * metrics.height_pt) as f32;
    let mut best: Option<(f32, Scan)> = None; // (페이지에 놓인 면적, 스캔)
    for object in page.objects().iter() {
        let Some(image) = object.as_image_object() else { continue };
        // 면적은 원본 픽셀 수가 아니라 **페이지에 놓인 크기**로 잰다 — 쪽번호 옆 작은 도장이
        // 고해상도라는 이유로 본문 스캔 이미지를 제칠 수 있기 때문이다. `image.width()`는
        // 픽셀 수를 돌려주므로(트레이트의 동명 메서드를 가린다) 면적 계산에는 쓰지 않는다.
        let (Ok(width), Ok(height)) = (object.width(), object.height()) else { continue };
        let area = width.value * height.value;
        if page_area <= 0.0 || area < page_area * COVERAGE {
            continue;
        }
        let (Ok(horizontal), Ok(vertical)) = (image.horizontal_dpi(), image.vertical_dpi()) else {
            continue;
        };
        let dpi = horizontal.min(vertical);
        let (Ok(pixels_wide), Ok(pixels_high)) = (image.width(), image.height()) else { continue };
        if !dpi.is_finite() || dpi <= 0.0 || pixels_wide <= 0 || pixels_high <= 0 {
            continue;
        }
        // 화면에 보이는 방향에 맞춘다(cm 표기와 같은 기준).
        let swapped = matches!(metrics.rotate, 90 | 270);
        let (width_px, height_px) = if swapped {
            (pixels_high as u32, pixels_wide as u32)
        } else {
            (pixels_wide as u32, pixels_high as u32)
        };
        let found = Scan { width_px, height_px, dpi: dpi.round() as u32 };
        if best.is_none_or(|(best_area, _)| area > best_area) {
            best = Some((area, found));
        }
    }
    best.map(|(_, scan)| scan)
}

/// 표준 용지규격 판정. 허용 오차는 ±2mm(≈5.7pt) — 스캔본은 재단 오차만큼 어긋나는 일이
/// 흔하고, 가장 가까운 규격끼리도 그보다는 훨씬 멀다(A4 210mm ↔ B5 176mm).
fn paper_name(width_pt: f64, height_pt: f64) -> Option<&'static str> {
    const TOLERANCE_PT: f64 = 5.7;
    /// (이름, 짧은 변 mm, 긴 변 mm)
    const PAPERS: &[(&str, f64, f64)] = &[
        ("A6", 105.0, 148.0),
        ("A5", 148.0, 210.0),
        ("A4", 210.0, 297.0),
        ("A3", 297.0, 420.0),
        ("A2", 420.0, 594.0),
        ("A1", 594.0, 841.0),
        ("A0", 841.0, 1189.0),
        ("B6", 128.0, 182.0),
        ("B5", 176.0, 250.0),
        ("B4", 250.0, 353.0),
        ("B5(JIS)", 182.0, 257.0),
        ("B4(JIS)", 257.0, 364.0),
        ("Letter", 215.9, 279.4),
        ("Legal", 215.9, 355.6),
        ("Tabloid", 279.4, 431.8),
    ];
    let to_mm = |pt: f64| pt / POINTS_PER_INCH * CM_PER_INCH * 10.0;
    let (short, long) = {
        let (a, b) = (to_mm(width_pt), to_mm(height_pt));
        if a <= b {
            (a, b)
        } else {
            (b, a)
        }
    };
    let tolerance_mm = to_mm(TOLERANCE_PT);
    PAPERS
        .iter()
        .find(|(_, w, h)| (short - w).abs() <= tolerance_mm && (long - h).abs() <= tolerance_mm)
        .map(|(name, _, _)| *name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metrics(width_pt: f64, height_pt: f64, rotate: u16) -> PageMetrics {
        PageMetrics { width_pt, height_pt, rotate }
    }

    #[test]
    fn a4_is_recognized_in_both_orientations() {
        let portrait = metrics(595.28, 841.89, 0);
        let landscape = metrics(841.89, 595.28, 0);
        assert_eq!(portrait.paper_name(), Some("A4"));
        assert_eq!(landscape.paper_name(), Some("A4"));
    }

    #[test]
    fn letter_is_not_confused_with_a4() {
        assert_eq!(metrics(612.0, 792.0, 0).paper_name(), Some("Letter"));
    }

    #[test]
    fn odd_sizes_have_no_paper_name() {
        assert_eq!(metrics(400.0, 400.0, 0).paper_name(), None);
    }

    #[test]
    fn a4_at_300dpi_is_2480_by_3508() {
        // 스캔 업계에서 A4 300dpi의 표준 픽셀 크기로 통하는 값.
        assert_eq!(metrics(595.28, 841.89, 0).pixels_at(300.0), (2480, 3508));
    }

    #[test]
    fn size_in_cm_matches_a4() {
        let (width, height) = metrics(595.28, 841.89, 0).size_cm();
        assert!((width - 21.0).abs() < 0.05, "{width}");
        assert!((height - 29.7).abs() < 0.05, "{height}");
    }
}
