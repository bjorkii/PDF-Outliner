//! 표시 페이지 프레임(설계 문서 2.1) — 내보내기와 가져오기가 함께 쓰는 좌표 변환.
//!
//! 기준 프레임은 뷰어에 보이는 그대로의 페이지다: CropBox(없으면 MediaBox, MediaBox와 교차)
//! 영역에 `/Rotate`를 적용한 상태. 표시 프레임 좌표는 원점이 좌상단이고 y가 아래로 커지며
//! 단위는 포인트(사용자 공간 단위 × `/UserUnit`)다. hOCR 픽셀 좌표는 여기에 DPI/72를 곱한 값이다.
//!
//! `/Rotate` 정규화는 pdfium(`CPDF_Page::GetPageRotation`)과 같게 `값 / 90 % 4`(정수 나눗셈)로
//! 한다 — 화면에 보이는 모습이 기준이고, 화면은 pdfium이 그리기 때문이다.

use anyhow::{bail, Context, Result};
use lopdf::{Dictionary, Document, Object, ObjectId};

/// 사용자 공간의 사각형(정규화: llx < urx, lly < ury).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub llx: f64,
    pub lly: f64,
    pub urx: f64,
    pub ury: f64,
}

impl Rect {
    pub fn width(&self) -> f64 {
        self.urx - self.llx
    }

    pub fn height(&self) -> f64 {
        self.ury - self.lly
    }

    fn from_object(doc: &Document, object: &Object) -> Option<Self> {
        let array = resolve(doc, object).as_array().ok()?;
        if array.len() != 4 {
            return None;
        }
        let mut v = [0.0; 4];
        for (slot, item) in v.iter_mut().zip(array) {
            *slot = number(resolve(doc, item))?;
        }
        let rect = Rect {
            llx: v[0].min(v[2]),
            lly: v[1].min(v[3]),
            urx: v[0].max(v[2]),
            ury: v[1].max(v[3]),
        };
        (rect.width() > 0.0 && rect.height() > 0.0).then_some(rect)
    }

    fn intersect(&self, other: &Rect) -> Option<Rect> {
        let rect = Rect {
            llx: self.llx.max(other.llx),
            lly: self.lly.max(other.lly),
            urx: self.urx.min(other.urx),
            ury: self.ury.min(other.ury),
        };
        (rect.width() > 0.0 && rect.height() > 0.0).then_some(rect)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PageFrame {
    /// 보이는 영역(CropBox ∩ MediaBox), 사용자 공간 단위.
    pub crop: Rect,
    /// 0, 90, 180, 270 중 하나(시계 방향).
    pub rotate: u16,
    pub user_unit: f64,
}

/// 규격 기본 MediaBox(US Letter) — MediaBox가 아예 없는 손상 파일용. pdfium도 같은 값을 쓴다.
const DEFAULT_MEDIA_BOX: Rect = Rect { llx: 0.0, lly: 0.0, urx: 612.0, ury: 792.0 };

impl PageFrame {
    pub fn from_page(doc: &Document, page_id: ObjectId) -> Result<Self> {
        let page = doc.get_dictionary(page_id).context("페이지 딕셔너리 조회 실패")?;
        let media = inherited(doc, page, b"MediaBox")
            .and_then(|o| Rect::from_object(doc, o))
            .unwrap_or(DEFAULT_MEDIA_BOX);
        let crop = inherited(doc, page, b"CropBox")
            .and_then(|o| Rect::from_object(doc, o))
            .and_then(|c| c.intersect(&media))
            .unwrap_or(media);
        let rotate = inherited(doc, page, b"Rotate")
            .and_then(|o| number(resolve(doc, o)))
            .map(normalize_rotate)
            .unwrap_or(0);
        // UserUnit은 상속되지 않는다(규격 표 30).
        let user_unit = page
            .get(b"UserUnit")
            .ok()
            .and_then(|o| number(resolve(doc, o)))
            .filter(|u| *u > 0.0)
            .unwrap_or(1.0);
        Ok(Self { crop, rotate, user_unit })
    }

    /// 회전 적용 후 표시 크기(포인트).
    pub fn display_size(&self) -> (f64, f64) {
        let (w, h) = (self.crop.width() * self.user_unit, self.crop.height() * self.user_unit);
        if self.rotate.is_multiple_of(180) {
            (w, h)
        } else {
            (h, w)
        }
    }

    /// 사용자 공간 → 표시 프레임(포인트, 좌상단 원점, y 아래로).
    pub fn user_to_display(&self, x: f64, y: f64) -> (f64, f64) {
        let (w, h) = (self.crop.width(), self.crop.height());
        let (u, v) = (x - self.crop.llx, y - self.crop.lly);
        let (dx, dy) = match self.rotate {
            90 => (v, u),
            180 => (w - u, v),
            270 => (h - v, w - u),
            _ => (u, h - v),
        };
        (dx * self.user_unit, dy * self.user_unit)
    }

    /// 사용자 공간 사각형 `[left, bottom, right, top]` → 표시 프레임 사각형(네 꼭짓점을 변환해 감쌈).
    pub fn user_rect_to_display(&self, rect: [f64; 4]) -> crate::layout::DRect {
        let corners = [(rect[0], rect[1]), (rect[2], rect[1]), (rect[0], rect[3]), (rect[2], rect[3])]
            .map(|(x, y)| self.user_to_display(x, y));
        crate::layout::DRect::from_points(&corners)
    }

    /// 표시 프레임(포인트) → 사용자 공간. `user_to_display`의 역변환.
    pub fn display_to_user(&self, dx: f64, dy: f64) -> (f64, f64) {
        let (w, h) = (self.crop.width(), self.crop.height());
        let (dx, dy) = (dx / self.user_unit, dy / self.user_unit);
        let (u, v) = match self.rotate {
            90 => (dy, dx),
            180 => (w - dx, dy),
            270 => (w - dy, h - dx),
            _ => (dx, h - dy),
        };
        (u + self.crop.llx, v + self.crop.lly)
    }

    /// 표시 프레임 → 사용자 공간 변환 행렬 `[a b c d e f]`(PDF 행 벡터 규약). 가져오기에서
    /// 표시 프레임 좌표로 조판한 텍스트를 `cm` 하나로 사용자 공간에 놓을 때 쓴다.
    pub fn display_to_user_matrix(&self) -> [f64; 6] {
        let (w, h) = (self.crop.width(), self.crop.height());
        let s = 1.0 / self.user_unit;
        let (llx, lly) = (self.crop.llx, self.crop.lly);
        // (dx, dy) → (u, v) 선형 부분과 이동을 각 회전별로 적는다(display_to_user와 같은 식).
        match self.rotate {
            90 => [0.0, s, s, 0.0, llx, lly],
            180 => [-s, 0.0, 0.0, s, llx + w, lly],
            270 => [0.0, -s, -s, 0.0, llx + w, lly + h],
            _ => [s, 0.0, 0.0, -s, llx, lly + h],
        }
    }

    /// 같은 영역을 회전 없이 본 프레임. `/Rotate`로 돌려 놓은 스캔 페이지는 이 프레임에서 글자가
    /// 바로 서 있으므로, 줄 구성(`layout::build_lines`)은 여기서 하고 결과를
    /// [`orient_lines`](Self::orient_lines)로 표시 프레임에 옮긴다.
    pub fn upright(&self) -> PageFrame {
        PageFrame { rotate: 0, ..*self }
    }

    /// [`upright`](Self::upright) 프레임에서 구성한 줄을 표시 프레임으로 옮긴다. 회전된 페이지의
    /// 줄에는 `text_angle`(반시계 방향)을 붙이고, 기준선은 hOCR에서 회전 줄에 쓰는 방식이 도구마다
    /// 달라 넣지 않는다.
    pub fn orient_lines(&self, lines: Vec<crate::layout::Line>) -> Vec<crate::layout::Line> {
        if self.rotate == 0 {
            return lines;
        }
        let upright = self.upright();
        let map = |r: &crate::layout::DRect| {
            let corners = [(r.x0, r.y0), (r.x1, r.y0), (r.x0, r.y1), (r.x1, r.y1)].map(|(x, y)| {
                let (ux, uy) = upright.display_to_user(x, y);
                self.user_to_display(ux, uy)
            });
            crate::layout::DRect::from_points(&corners)
        };
        let text_angle = (360 - self.rotate) % 360;
        lines
            .into_iter()
            .map(|mut line| {
                line.rect = map(&line.rect);
                for word in &mut line.words {
                    word.rect = map(&word.rect);
                }
                line.baseline = None;
                line.text_angle = text_angle;
                line
            })
            .collect()
    }

    /// 픽셀 크기(W, H)의 hOCR 페이지를 이 페이지에 대응시킬 때의 배율(포인트/픽셀). 가로세로
    /// 비율이 `tolerance`(상대 오차)를 넘게 다르면 오류 — 회전된 스캔이나 다른 파일의 hOCR일
    /// 가능성이 크다(설계 문서 2.1).
    pub fn pixel_scale(&self, width_px: f64, height_px: f64, tolerance: f64) -> Result<(f64, f64)> {
        if width_px <= 0.0 || height_px <= 0.0 {
            bail!("hOCR 페이지 크기가 0");
        }
        let (w, h) = self.display_size();
        let (sx, sy) = (w / width_px, h / height_px);
        if (sx / sy - 1.0).abs() > tolerance {
            bail!(
                "페이지 종횡비 불일치: 페이지 {:.1}×{:.1}pt, hOCR {}×{}px",
                w,
                h,
                width_px,
                height_px
            );
        }
        Ok((sx, sy))
    }
}

fn normalize_rotate(value: f64) -> u16 {
    let quarter = ((value as i64) / 90).rem_euclid(4);
    (quarter * 90) as u16
}

/// Pages 트리를 따라 올라가며 상속 가능한 속성을 찾는다(순환 방지로 깊이 제한).
pub fn inherited<'a>(doc: &'a Document, page: &'a Dictionary, key: &[u8]) -> Option<&'a Object> {
    let mut node = page;
    for _ in 0..64 {
        if let Ok(value) = node.get(key) {
            return Some(value);
        }
        let parent = node.get(b"Parent").ok()?.as_reference().ok()?;
        node = doc.get_dictionary(parent).ok()?;
    }
    None
}

pub(crate) fn resolve<'a>(doc: &'a Document, object: &'a Object) -> &'a Object {
    doc.dereference(object).map(|(_, o)| o).unwrap_or(object)
}

pub(crate) fn number(object: &Object) -> Option<f64> {
    match object {
        Object::Integer(i) => Some(*i as f64),
        Object::Real(r) => Some(*r as f64),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lopdf::dictionary;

    fn frame(crop: [f64; 4], rotate: u16) -> PageFrame {
        PageFrame {
            crop: Rect { llx: crop[0], lly: crop[1], urx: crop[2], ury: crop[3] },
            rotate,
            user_unit: 1.0,
        }
    }

    fn apply(m: [f64; 6], x: f64, y: f64) -> (f64, f64) {
        (m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5])
    }

    fn close(a: (f64, f64), b: (f64, f64)) -> bool {
        (a.0 - b.0).abs() < 1e-9 && (a.1 - b.1).abs() < 1e-9
    }

    #[test]
    fn corners_map_for_each_rotation() {
        // 가로 200, 세로 100, 원점이 0이 아닌 CropBox.
        let cases = [
            (0, (36.0, 136.0), (0.0, 0.0)),    // 좌상단이 좌상단
            (90, (36.0, 36.0), (0.0, 0.0)),    // 90° 시계: 좌하단이 좌상단으로
            (180, (236.0, 36.0), (0.0, 0.0)),  // 우하단이 좌상단으로
            (270, (236.0, 136.0), (0.0, 0.0)), // 우상단이 좌상단으로
        ];
        for (rotate, user_point, display_point) in cases {
            let f = frame([36.0, 36.0, 236.0, 136.0], rotate);
            assert!(close(f.user_to_display(user_point.0, user_point.1), display_point), "rotate {rotate}");
        }
        assert_eq!(frame([36.0, 36.0, 236.0, 136.0], 90).display_size(), (100.0, 200.0));
    }

    #[test]
    fn round_trip_and_matrix_agree() {
        for rotate in [0, 90, 180, 270] {
            let mut f = frame([10.0, 20.0, 310.0, 420.0], rotate);
            f.user_unit = 2.0;
            for &(x, y) in &[(10.0, 20.0), (123.4, 56.7), (310.0, 420.0)] {
                let d = f.user_to_display(x, y);
                assert!(close(f.display_to_user(d.0, d.1), (x, y)), "rotate {rotate}");
                assert!(close(apply(f.display_to_user_matrix(), d.0, d.1), (x, y)), "matrix rotate {rotate}");
            }
        }
    }

    #[test]
    fn rotate_normalization_matches_pdfium() {
        assert_eq!(normalize_rotate(-90.0), 270);
        assert_eq!(normalize_rotate(450.0), 90);
        assert_eq!(normalize_rotate(100.0), 90); // 90의 배수가 아니면 내림
        assert_eq!(normalize_rotate(-45.0), 0);
    }

    #[test]
    fn inherited_boxes_and_crop_intersection() {
        let mut doc = Document::with_version("1.7");
        let pages_id = doc.new_object_id();
        let page_id = doc.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "CropBox" => vec![0.into(), 0.into(), 1000.into(), 50.into()],
        });
        doc.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages",
                "Kids" => vec![page_id.into()],
                "Count" => 1,
                "MediaBox" => vec![0.into(), 0.into(), 500.into(), 700.into()],
                "Rotate" => -90,
            }),
        );
        let f = PageFrame::from_page(&doc, page_id).unwrap();
        assert_eq!(f.crop, Rect { llx: 0.0, lly: 0.0, urx: 500.0, ury: 50.0 });
        assert_eq!(f.rotate, 270);
    }

    #[test]
    fn rotated_page_lines_are_laid_out_upright() {
        use crate::layout::{build_lines, tests::line_chars, LayoutOptions};
        // 가로 200 x 세로 100 페이지를 90° 돌려 표시(표시 크기 100 x 200).
        let f = frame([0.0, 0.0, 200.0, 100.0], 90);
        let chars = line_chars("abc de", 0, true);
        let lines = f.orient_lines(build_lines(&chars, &LayoutOptions::default()).0);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].text(), "abc de");
        assert_eq!(lines[0].text_angle, 270);
        // 정립 프레임의 가로 줄(y 0~12, x 0~60)이 표시 프레임에서는 오른쪽 세로 띠가 된다.
        let r = lines[0].rect;
        assert!((r.x0 - 88.0).abs() < 1e-9 && (r.x1 - 100.0).abs() < 1e-9, "{r:?}");
        assert!((r.y0 - 0.0).abs() < 1e-9 && (r.y1 - 60.0).abs() < 1e-9, "{r:?}");
    }

    #[test]
    fn aspect_ratio_check() {
        let f = frame([0.0, 0.0, 612.0, 792.0], 0);
        let (sx, sy) = f.pixel_scale(2550.0, 3300.0, 0.02).unwrap();
        assert!((sx - 0.24).abs() < 1e-9 && (sy - 0.24).abs() < 1e-9);
        assert!(f.pixel_scale(3300.0, 2550.0, 0.02).is_err()); // 가로로 누운 스캔
    }
}
