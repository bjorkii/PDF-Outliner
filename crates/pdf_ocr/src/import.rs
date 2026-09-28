//! OCR 가져오기의 판단 로직(설계 문서 6.2, 6.3) — pdfium 없이 쓸 수 있는 부분.
//!
//! - hOCR 픽셀 좌표 → 표시 페이지 프레임([`layer_lines`]).
//! - 페이지를 덮는 큰 이미지 비율([`image_coverage`]).
//! - 페이지 분류(6.3.2, [`classify_page`]).
//! - 디지털 텍스트 손상 추정([`looks_damaged`]): ToUnicode가 망가진 PDF는 보이는 글자를 복사하면
//!   깨진다. 페이지 분류에서 "판단 불가"로 돌리는 신호로 쓴다.
//!
//! 여기 있던 **문자 단위 중복 제거**(6.3.3)는 폐기했다(2026-09-28 결정). 쪽번호·면주가 디지털로
//! 심어진 자리에 OCR이 겹치는 문제는 내보낼·가져올 **영역을 지정하는 기능**으로 원천에서 풀기로
//! 했다(예약 12).
//!
//! 기준값은 [`Thresholds`] 한 곳에 모아 두고 리포트에 함께 적는다. 설정 화면에는 내놓지 않는다.

use crate::content::interp::{interpret_page, transform_point, Context, Matrix, Visitor, XObjectKind};
use crate::content::lexer::Operation;
use crate::geometry::PageFrame;
use crate::hocr::HocrPage;
use crate::insert::{LayerLine, LayerWord};
use crate::layout::DRect;
use lopdf::{Document, ObjectId};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Thresholds {
    /// 이 비율 이상을 덮는 이미지가 있으면 "스캔 페이지".
    pub big_image: f64,
    /// 페이지 가장자리 여백 폭(가로·세로 각각의 비율).
    pub margin: f64,
    /// 디지털 글자 중 사용자 정의 영역·대체 문자·제어 문자가 이 비율을 넘으면 손상 의심.
    pub damaged_ratio: f64,
    /// hOCR 페이지와 PDF 페이지의 가로세로 비율 허용 오차(상대).
    pub aspect_tolerance: f64,
}

pub const THRESHOLDS: Thresholds =
    Thresholds { big_image: 0.8, margin: 0.1, damaged_ratio: 0.3, aspect_tolerance: 0.03 };

/// hOCR 페이지의 줄을 표시 프레임 좌표로. 가로세로 비율이 맞지 않으면 오류(그 페이지는 건너뜀).
pub fn layer_lines(page: &HocrPage, frame: &PageFrame) -> anyhow::Result<Vec<LayerLine>> {
    let (sx, sy) = frame.pixel_scale(page.bbox.width(), page.bbox.height(), THRESHOLDS.aspect_tolerance)?;
    let to_display = |b: &crate::hocr::parse::BBox| DRect {
        x0: (b.x0 - page.bbox.x0) * sx,
        y0: (b.y0 - page.bbox.y0) * sy,
        x1: (b.x1 - page.bbox.x0) * sx,
        y1: (b.y1 - page.bbox.y0) * sy,
    };
    Ok(page
        .lines
        .iter()
        .map(|line| LayerLine {
            words: line
                .words
                .iter()
                .filter(|w| w.bbox.width() > 0.0 && w.bbox.height() > 0.0)
                .map(|w| LayerWord { text: w.text.clone(), rect: to_display(&w.bbox) })
                .collect(),
            angle: line.textangle,
            // 기울기는 픽셀 단위 비율이라 가로세로 배율이 같으면 그대로 쓸 수 있다.
            slope: line.baseline.map(|(slope, _)| slope * sy / sx).unwrap_or(0.0),
        })
        .filter(|line| !line.words.is_empty())
        .collect())
}

// ------------------------------------------------------------------ 이미지 덮임 비율

const GRID: usize = 200;

struct ImageCoverage<'a> {
    frame: &'a PageFrame,
    grid: Vec<bool>,
}

impl ImageCoverage<'_> {
    fn paint(&mut self, ctm: &Matrix) {
        let corners = [(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)].map(|(x, y)| {
            let (ux, uy) = transform_point(ctm, x, y);
            self.frame.user_to_display(ux, uy)
        });
        let rect = DRect::from_points(&corners);
        let (w, h) = self.frame.display_size();
        if w <= 0.0 || h <= 0.0 {
            return;
        }
        // 셀 중심이 이미지 사각형 안에 있으면 칠한다(겹친 조각을 두 번 세지 않는다).
        for row in 0..GRID {
            let cy = (row as f64 + 0.5) / GRID as f64 * h;
            if cy < rect.y0 || cy > rect.y1 {
                continue;
            }
            for col in 0..GRID {
                let cx = (col as f64 + 0.5) / GRID as f64 * w;
                if cx >= rect.x0 && cx <= rect.x1 {
                    self.grid[row * GRID + col] = true;
                }
            }
        }
    }
}

impl Visitor for ImageCoverage<'_> {
    fn operation(&mut self, context: &Context, _index: usize, op: &Operation) {
        if op.is(b"BI") {
            self.paint(&context.state.ctm);
        }
    }

    fn xobject(&mut self, context: &Context, _name: &[u8], _id: Option<ObjectId>, kind: &XObjectKind) {
        if *kind == XObjectKind::Image {
            self.paint(&context.state.ctm);
        }
    }
}

/// 이미지(XObject·인라인, Form 안 포함)가 덮는 페이지 비율(0~1). 클리핑은 무시한 근사값이다.
/// 콘텐츠를 해석하지 못하면 0.
pub fn image_coverage(doc: &Document, page_id: ObjectId, frame: &PageFrame) -> f64 {
    let mut visitor = ImageCoverage { frame, grid: vec![false; GRID * GRID] };
    if interpret_page(doc, page_id, &mut visitor).is_err() {
        return 0.0;
    }
    visitor.grid.iter().filter(|&&c| c).count() as f64 / (GRID * GRID) as f64
}

// ------------------------------------------------------------------ 페이지 분류

/// 디지털 텍스트(보이는 텍스트) 글자 하나 — 표시 프레임 좌표.
#[derive(Debug, Clone, PartialEq)]
pub struct DigitalChar {
    pub ch: char,
    pub rect: DRect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageClass {
    /// 디지털 텍스트도 보이지 않는 텍스트도 없음 → 삽입.
    NoText,
    /// 보이지 않는 텍스트만 있음 → 덮어쓰기를 고르면 삭제 후 삽입.
    ExistingOcrOnly,
    /// 스캔 이미지 + 여백의 디지털 텍스트(쪽번호·머리글 등) → 중복 제거하고 삽입.
    ScanWithExtras,
    /// 스캔 이미지가 아니고 디지털 텍스트가 있음 → 건너뜀.
    Digital,
    /// 스캔 이미지 위 본문 영역에 디지털 텍스트가 있음(디지털 본문·ClearScan형·손상 등) → 사용자 확인.
    Undetermined,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PageSignals {
    pub digital_chars: usize,
    /// 디지털 글자가 모두 가장자리 여백에 있음.
    pub digital_only_in_margins: bool,
    pub has_invisible_text: bool,
    pub image_coverage: f64,
    pub damaged_digital: bool,
}

pub fn classify_page(s: &PageSignals) -> PageClass {
    if s.digital_chars == 0 {
        return if s.has_invisible_text { PageClass::ExistingOcrOnly } else { PageClass::NoText };
    }
    let scanned = s.image_coverage >= THRESHOLDS.big_image;
    if !scanned {
        return PageClass::Digital;
    }
    if s.digital_only_in_margins && !s.damaged_digital {
        PageClass::ScanWithExtras
    } else {
        PageClass::Undetermined
    }
}

/// 디지털 글자가 모두 가장자리 여백 띠(중심 기준)에 있는지.
pub fn only_in_margins(chars: &[DigitalChar], page_size: (f64, f64)) -> bool {
    let (w, h) = page_size;
    let (mx, my) = (w * THRESHOLDS.margin, h * THRESHOLDS.margin);
    chars.iter().all(|c| {
        let (cx, cy) = ((c.rect.x0 + c.rect.x1) / 2.0, (c.rect.y0 + c.rect.y1) / 2.0);
        cx < mx || cx > w - mx || cy < my || cy > h - my
    })
}

pub fn looks_damaged(chars: &[DigitalChar]) -> bool {
    if chars.is_empty() {
        return false;
    }
    let bad = chars
        .iter()
        .filter(|c| {
            let v = c.ch as u32;
            c.ch == '\u{FFFD}'
                || (c.ch.is_control() && !c.ch.is_whitespace())
                || (0xE000..=0xF8FF).contains(&v)
                || (0xF0000..=0x10FFFF).contains(&v)
        })
        .count();
    bad as f64 / chars.len() as f64 > THRESHOLDS.damaged_ratio
}

// ------------------------------------------------------------------ 검증 보조

/// 넣으려던 글자(`expected`: 글자, 칸 중심)와 추출된 글자(`extracted`: 글자, 중심)를 비교해 추출되지
/// 않은 `expected`의 위치를 돌려준다. 좌표는 같은 좌표계여야 한다.
///
/// 먼저 글자별 개수로 모자란 글자를 확정하고(순서와 무관), 모자란 글자 종류에 대해서만 추출된 같은
/// 글자를 가장 가까운 기대 위치부터 짝지어 짝이 없는 위치를 고른다. pdfium은 줄을 다시 묶거나
/// 오른쪽에서 왼쪽 글을 뒤집는 등 추출 순서를 바꾸므로, 순서로 맞추면 있는 글자도 빠진 것으로 본다
/// (실제 파일에서 확인, 2026-09-22).
pub fn missing_positions(expected: &[(char, (f64, f64))], extracted: &[(char, (f64, f64))]) -> Vec<usize> {
    use std::collections::HashMap;
    let mut expected_by_char: HashMap<char, Vec<usize>> = HashMap::new();
    for (i, (c, _)) in expected.iter().enumerate() {
        expected_by_char.entry(*c).or_default().push(i);
    }
    let mut extracted_by_char: HashMap<char, Vec<usize>> = HashMap::new();
    for (j, (c, _)) in extracted.iter().enumerate() {
        extracted_by_char.entry(*c).or_default().push(j);
    }
    let mut missing = Vec::new();
    for (c, positions) in &expected_by_char {
        let found = extracted_by_char.get(c).map(Vec::as_slice).unwrap_or(&[]);
        if found.len() >= positions.len() {
            continue;
        }
        let distance = |i: usize, j: usize| {
            let ((ax, ay), (bx, by)) = (expected[i].1, extracted[j].1);
            (ax - bx).hypot(ay - by)
        };
        let mut pairs: Vec<(f64, usize, usize)> =
            positions.iter().flat_map(|&i| found.iter().map(move |&j| (distance(i, j), i, j))).collect();
        pairs.sort_by(|a, b| a.0.total_cmp(&b.0));
        let (mut used_e, mut used_x) = (std::collections::HashSet::new(), std::collections::HashSet::new());
        for (_, i, j) in pairs {
            if !used_e.contains(&i) && !used_x.contains(&j) {
                used_e.insert(i);
                used_x.insert(j);
            }
        }
        missing.extend(positions.iter().copied().filter(|i| !used_e.contains(i)));
    }
    missing.sort_unstable();
    missing
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::interp::tests::one_page_doc;
    use crate::geometry::Rect;
    use crate::hocr::parse;
    use lopdf::{dictionary, Stream};

    fn frame() -> PageFrame {
        PageFrame { crop: Rect { llx: 0.0, lly: 0.0, urx: 612.0, ury: 792.0 }, rotate: 0, user_unit: 1.0 }
    }

    fn r(x0: f64, y0: f64, x1: f64, y1: f64) -> DRect {
        DRect { x0, y0, x1, y1 }
    }

    #[test]
    fn hocr_pixels_to_points() {
        let html = "<div class='ocr_page' title='bbox 0 0 2550 3300'><span class='ocr_line' title='bbox 300 600 900 700; baseline 0.02 -5'>\
            <span class='ocrx_word' title='bbox 300 600 900 700'>x</span></span></div>";
        let (pages, _) = parse(html).unwrap();
        let lines = layer_lines(&pages[0], &frame()).unwrap();
        let w = &lines[0].words[0].rect;
        assert!((w.x0 - 72.0).abs() < 1e-9 && (w.y1 - 168.0).abs() < 1e-9, "{w:?}");
        assert!((lines[0].slope - 0.02).abs() < 1e-12);
        // 가로로 누운 hOCR은 거부
        let rotated = "<div class='ocr_page' title='bbox 0 0 3300 2550'></div>";
        assert!(layer_lines(&parse(rotated).unwrap().0[0], &frame()).is_err());
    }

    #[test]
    fn coverage_of_full_page_scan_and_pieces() {
        let image = || Stream::new(dictionary! { "Type" => "XObject", "Subtype" => "Image" }, vec![]);
        let (doc, page) = one_page_doc(b"q 612 0 0 792 0 0 cm /Im0 Do Q", vec![("Im0", image())]);
        assert!(image_coverage(&doc, page, &frame()) > 0.99);
        // 위아래 반쪽 두 조각 + 겹치는 조각 — 중복 없이 100%
        let (doc, page) = one_page_doc(
            b"q 612 0 0 396 0 0 cm /Im0 Do Q q 612 0 0 396 0 396 cm /Im0 Do Q q 612 0 0 400 0 200 cm /Im0 Do Q",
            vec![("Im0", image())],
        );
        assert!(image_coverage(&doc, page, &frame()) > 0.99);
        let (doc, page) = one_page_doc(b"q 306 0 0 792 0 0 cm /Im0 Do Q BT (x) Tj ET", vec![("Im0", image())]);
        let c = image_coverage(&doc, page, &frame());
        assert!((c - 0.5).abs() < 0.01, "{c}");
    }

    #[test]
    fn missing_positions_by_count_then_nearest() {
        let at = |c: char, x: f64| (c, (x, 0.0));
        let expected = vec![at('a', 0.0), at('b', 10.0), at('a', 20.0), at('c', 30.0)];
        // 순서가 바뀌어도 개수가 맞으면 빠진 것 없음
        assert!(missing_positions(&expected, &[at('c', 30.0), at('a', 20.0), at('b', 10.0), at('a', 0.0)]).is_empty());
        // 'a' 하나 모자람 → 추출된 'a'(x=19)와 먼 쪽(x=0)이 빠진 자리
        assert_eq!(missing_positions(&expected, &[at('b', 10.0), at('a', 19.0), at('c', 30.0)]), vec![0]);
        assert_eq!(missing_positions(&expected, &[at('a', 0.0), at('a', 20.0), at('c', 30.0)]), vec![1]);
    }

    #[test]
    fn classification_table() {
        let base = PageSignals {
            digital_chars: 0,
            digital_only_in_margins: true,
            has_invisible_text: false,
            image_coverage: 1.0,
            damaged_digital: false,
        };
        assert_eq!(classify_page(&base), PageClass::NoText);
        assert_eq!(classify_page(&PageSignals { has_invisible_text: true, ..base }), PageClass::ExistingOcrOnly);
        let with_text = PageSignals { digital_chars: 5, ..base };
        assert_eq!(classify_page(&with_text), PageClass::ScanWithExtras);
        assert_eq!(classify_page(&PageSignals { image_coverage: 0.1, ..with_text }), PageClass::Digital);
        assert_eq!(classify_page(&PageSignals { digital_only_in_margins: false, ..with_text }), PageClass::Undetermined);
        assert_eq!(classify_page(&PageSignals { damaged_digital: true, ..with_text }), PageClass::Undetermined);
    }

    #[test]
    fn margins_and_damage() {
        let page_number = DigitalChar { ch: '1', rect: r(300.0, 760.0, 306.0, 772.0) };
        let body = DigitalChar { ch: '가', rect: r(300.0, 400.0, 310.0, 410.0) };
        assert!(only_in_margins(std::slice::from_ref(&page_number), (612.0, 792.0)));
        assert!(!only_in_margins(&[page_number, body.clone()], (612.0, 792.0)));
        let pua = DigitalChar { ch: '\u{E001}', rect: body.rect };
        assert!(looks_damaged(&[pua.clone(), pua, body.clone()]));
        assert!(!looks_damaged(&[body]));
    }

}
