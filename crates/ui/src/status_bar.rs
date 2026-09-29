//! 창 아래 상태표시줄 — 왼쪽에 현재 페이지 정보, 오른쪽에 작업 결과 메시지(2026-09-23 요청).
//!
//! 페이지 정보는 두 단계로 채운다. 크기·용지규격·회전은 페이지를 넘기는 즉시 보여 주고,
//! OCR 유무와 유효 스캔 해상도는 **그 페이지가 화면에 그려진 다음 프레임에** 잰다
//! (`pdf_engine::page_info` 모듈 문서 참고). 페이지를 빠르게 연속으로 넘길 때 매 페이지마다
//! pdfium 조회가 끼어들어 넘김이 끊기는 것을 막기 위해서다 — 넘기는 중에는 그 자리가
//! 잠깐 비어 있다가, 멈추면 채워진다.

use crate::app::PdfViewerApp;
use pdf_engine::page_info::{PageInspection, PageMetrics};

/// 픽셀 크기를 적을 기준 해상도.
const REFERENCE_DPI: f64 = 300.0;

/// 한 페이지분 정보. `metrics`는 페이지를 넘긴 프레임에, `inspection`은 그다음 프레임에 찬다.
#[derive(Debug, Clone, Copy)]
pub struct PageInfo {
    pub page: u32,
    pub metrics: PageMetrics,
    pub inspection: Option<PageInspection>,
}

/// 매 프레임 호출. 페이지가 바뀌었으면 싼 값을 새로 재고, 이미 잰 페이지면 나머지를 채운다.
pub fn refresh(ctx: &egui::Context, app: &mut PdfViewerApp) {
    let Some(document) = app.document.as_ref() else {
        app.page_info = None;
        return;
    };
    let page_number = app.current_page;
    if page_number == 0 {
        app.page_info = None;
        return;
    }
    let stale = app.page_info.is_none_or(|info| info.page != page_number);
    if stale {
        let Ok(page) = document.pages().get((page_number - 1) as i32) else {
            app.page_info = None;
            return;
        };
        let Ok(metrics) = pdf_engine::page_info::measure(&page) else {
            app.page_info = None;
            return;
        };
        app.page_info = Some(PageInfo { page: page_number, metrics, inspection: None });
        // 이 프레임은 페이지를 그리는 데만 쓰고, 나머지 측정은 다음 프레임에 한다.
        ctx.request_repaint();
        return;
    }
    let Some(metrics) = app.page_info.filter(|info| info.inspection.is_none()).map(|info| info.metrics)
    else {
        return;
    };
    {
        let Ok(page) = document.pages().get((page_number - 1) as i32) else { return };
        let inspection = pdf_engine::page_info::inspect(&page, &metrics);
        if let Some(info) = app.page_info.as_mut() {
            info.inspection = Some(inspection);
        }
    }
}

pub fn show(ctx: &egui::Context, app: &mut PdfViewerApp) {
    egui::TopBottomPanel::bottom("status_bar").show(ctx, |ui| {
        ui.horizontal(|ui| {
            if let Some(info) = app.page_info {
                for (index, part) in describe(&info).into_iter().enumerate() {
                    if index > 0 {
                        ui.weak("/");
                    }
                    ui.label(part);
                }
            }
            // 작업 결과 메시지는 오른쪽 끝에. 잘리면 egui::Label이 전체 내용을 툴팁으로
            // 붙여 주므로(0.29.1 label.rs) on_hover_text를 따로 달지 않는다 — 달면 툴팁이
            // 두 개 겹친다(2026-07-16 리포트).
            if let Some(message) = &app.status_message {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add(egui::Label::new(message).truncate());
                });
            }
        });
    });
}

/// 상태표시줄에 적을 조각들. 값이 없거나 기본값인 항목(회전 0도 등)은 자리를 차지하지 않는다.
///
/// 픽셀 수는 **실제 해상도를 아는 경우와 모르는 경우를 다르게 적는다**(2026-09-27 요청).
/// 예전에는 `300dpi로 환산한 픽셀 수`와 `실제 스캔 해상도`를 나란히 적었는데, 서로 다른
/// dpi 두 개가 늘어서 있으니 어느 쪽이 진짜인지 알 수 없었다.
/// - 페이지를 채우는 스캔 이미지가 있으면: 그 이미지의 실제 픽셀 수와 실제 해상도를 한 쌍으로
///   `2604 × 3671 px (72dpi)`.
/// - 없으면(벡터·디지털 페이지): 픽셀도 해상도도 적지 않는다. 래스터가 없는 페이지의
///   "300dpi 환산 픽셀 수"는 실측이 아니라 산술값일 뿐이라, 실제 해상도로 오해할 여지만
///   남긴다(2026-09-27 판단).
/// - 아직 재기 전(한 프레임 동안)에만 환산값으로 자리를 채운다: `2480 × 3508 px` + `300dpi 추정`.
fn describe(info: &PageInfo) -> Vec<String> {
    let mut parts = Vec::new();
    let (width_cm, height_cm) = info.metrics.size_cm();
    let paper = info.metrics.paper_name().map(|name| format!(" ({name})")).unwrap_or_default();
    parts.push(format!("{width_cm:.1} × {height_cm:.1} cm{paper}"));

    match info.inspection {
        // 스캔을 찾았으면 그 이미지의 실제 픽셀 수와 실제 해상도.
        Some(inspection) => {
            if let Some(scan) = inspection.scan {
                parts.push(format!("{} × {} px ({}dpi)", scan.width_px, scan.height_px, scan.dpi));
            }
            // 스캔이 없으면(벡터·디지털 페이지) 픽셀도 해상도도 적지 않는다. 래스터가 아예
            // 없는 페이지에 "300dpi로 뽑으면 몇 픽셀"을 적어 봐야 실측이 아닌 산술값이고,
            // 사용자가 그것을 실제 해상도로 오해할 여지만 남는다(2026-09-27 판단).
        }
        // 아직 재지 않은 동안에는 환산값으로 자리를 채워 둔다 — 한 프레임 뒤에 정해진다.
        None => {
            let (width_px, height_px) = info.metrics.pixels_at(REFERENCE_DPI);
            parts.push(format!("{width_px} × {height_px} px"));
            parts.push(format!("{REFERENCE_DPI:.0}dpi 추정"));
        }
    }

    if info.metrics.rotate != 0 {
        parts.push(format!("회전 {}°", info.metrics.rotate));
    }
    // 아직 재지 않은 동안에는 아예 적지 않는다(빈칸이 깜빡이는 것보다 낫다).
    if let Some(inspection) = info.inspection {
        parts.push(if inspection.has_ocr { "OCR 있음".to_string() } else { "OCR 없음".to_string() });
    }
    parts
}

#[cfg(test)]
mod tests {
    use super::*;
    use pdf_engine::page_info::Scan;

    fn info(rotate: u16, inspection: Option<PageInspection>) -> PageInfo {
        PageInfo {
            page: 1,
            metrics: PageMetrics { width_pt: 595.28, height_pt: 841.89, rotate },
            inspection,
        }
    }

    fn scanned(dpi: u32) -> Option<PageInspection> {
        Some(PageInspection { has_ocr: true, scan: Some(Scan { width_px: 2604, height_px: 3671, dpi }) })
    }

    #[test]
    fn scanned_page_shows_the_real_pixels_and_dpi() {
        let parts = describe(&info(0, scanned(300)));
        assert_eq!(parts, ["21.0 × 29.7 cm (A4)", "2604 × 3671 px (300dpi)", "OCR 있음"]);
    }

    /// 예전에는 "300dpi 환산 픽셀 수"와 "실제 72dpi"가 나란히 적혀 어느 쪽이 진짜인지
    /// 알 수 없었다(2026-09-27 리포트). 화면에 dpi는 많아야 하나만 나온다.
    #[test]
    fn at_most_one_dpi_is_ever_shown() {
        let no_scan = Some(PageInspection { has_ocr: false, scan: None });
        for inspection in [scanned(72), no_scan, None] {
            let parts = describe(&info(0, inspection));
            assert!(parts.iter().filter(|p| p.contains("dpi")).count() <= 1, "{parts:?}");
        }
    }

    /// 래스터가 없는 페이지에는 픽셀 수도 해상도도 적지 않는다.
    #[test]
    fn page_without_a_scan_shows_no_pixels_at_all() {
        let parts = describe(&info(0, Some(PageInspection { has_ocr: false, scan: None })));
        assert_eq!(parts, ["21.0 × 29.7 cm (A4)", "OCR 없음"]);
    }

    /// 재기 전 한 프레임 동안만 환산값을 보여 준다(빈 줄이 깜빡이지 않게).
    #[test]
    fn unmeasured_page_falls_back_to_the_converted_pixels() {
        let parts = describe(&info(0, None));
        assert_eq!(parts, ["21.0 × 29.7 cm (A4)", "2480 × 3508 px", "300dpi 추정"]);
    }

    #[test]
    fn rotation_shows_only_when_set() {
        assert!(describe(&info(0, None)).iter().all(|p| !p.contains("회전")));
        assert!(describe(&info(90, None)).iter().any(|p| p == "회전 90°"));
    }

}
