//! OCR 표시/숨기기 모드(예약 6) — 보이지 않는 텍스트를 화면에 꺼내 보여 준다.
//!
//! 스캔 PDF의 OCR 텍스트는 정의상 화면에 안 보인다. 그래서 "이 쪽에 OCR이 제대로 들어갔나",
//! "왜 이 낱말이 검색에 안 걸리나", "가져온 OCR이 자리를 맞게 잡았나"를 눈으로 확인할 길이
//! 없었다. 이 모드는 그 텍스트의 **자리(테두리)와 내용(글자)** 을 지면 위에 겹쳐 그린다.
//!
//! **F1은 켜고 끄기만 한다.** 원본과 표시본을 빠르게 번갈아 보는 것이 이 기능의 쓸모라, 단계를
//! 늘리면 그 비교가 불가능해진다(2026-09-29 결정). 무엇을 보여 줄지(OCR 텍스트만 / 보이지 않는
//! 텍스트 전부)는 OCR 메뉴의 체크 항목으로 따로 정한다. 기본이 OCR 텍스트만인 이유는 확인 창과
//! 삭제가 쓰는 기준이 그것이기 때문이다 — 전부 보여 주면 "여기 보이는데 왜 안 지워지나"가 된다.
//!
//! **측정은 보고 있는 쪽만, 결과는 캐시한다.** 한 쪽의 글자를 pdfium에서 꺼내 줄로 묶는 일은
//! 수백 쪽 문서에서도 한 쪽분이면 가볍지만, 매 프레임 다시 하면 넘김이 끊긴다.

use pdf_ocr::layout::{build_lines, DRect, InputChar, LayoutOptions, TextSource};
use pdfium_render::prelude::PdfPage;

/// 화면에 그릴 낱말 하나.
#[derive(Debug, Clone, PartialEq)]
pub struct Word {
    /// 페이지 사용자 공간 `[left, bottom, right, top]`.
    pub bounds: [f64; 4],
    pub text: String,
    /// OCR 텍스트(이미지 영역 안이거나 걸친 `3 Tr`)인가. 아니면 그 밖의 안 보이는 텍스트다.
    pub is_ocr: bool,
}

/// 표시 모드의 상태.
///
/// **켜고 끄는 것은 F1 하나로 한다**(2026-09-29 결정). 처음에는 꺼짐 → OCR만 → 전부를 F1으로
/// 돌렸는데, 그러면 원본과 표시본을 빠르게 번갈아 보는 것이 불가능해진다. 실무에서 쓰는 방식이
/// 그것이므로, 범위(`include_all_hidden`)는 메뉴의 체크 항목으로 빼고 F1은 켜고 끄기만 한다.
pub struct OcrOverlay {
    pub on: bool,
    /// `7 Tr`·알파 0·크기 0까지 포함할지. 끄면 OCR 텍스트(이미지에 걸친 `3 Tr`)만 본다.
    pub include_all_hidden: bool,
    /// **원본 지면의 불투명도**(0~100). 100이면 원본이 원래대로 다 보이고, 0이면 완전히 가려
    /// 보이지 않는다. 주체가 장막이 아니라 원본이다(2026-09-30 정정).
    pub veil: u8,
    cache: std::cell::RefCell<std::collections::HashMap<u32, Vec<Word>>>,
    /// 사용자가 눌러 고른 낱말 (쪽 번호, 그 쪽 낱말 목록에서의 자리).
    ///
    /// 마우스를 떼도 남는다. 빽빽한 줄을 훑을 때 마우스를 정확히 올려 두는 것보다 한 번 눌러
    /// 두고 화살표로 옮기는 편이 낫기 때문이다(2026-09-30 요청, 2안). 그리는 쪽이 앱을 `&`로만
    /// 받으므로 `Cell`에 둔다.
    selected: std::cell::Cell<Option<(u32, usize)>>,
}

/// 캐시에 담아 둘 쪽 수. 연속 스크롤로 쭉 내려가도 무한정 쌓이지 않게 한다.
const CACHE_PAGES: usize = 8;

/// 원본 지면 불투명도의 기본값(%). 스캔 글자가 비칠 정도만 남긴다.
pub const DEFAULT_VEIL: u8 = 31;

/// 화살표로 옮길 때 "같은 줄"로 볼 겹침 기준 — 고른 상자의 높이(가로 이동일 때) 대비 비율.
/// 이만큼 겹친 상자가 하나라도 있으면 그 안에서만 고른다.
const OVERLAP_RATE: f64 = 0.5;

impl Default for OcrOverlay {
    fn default() -> Self {
        Self {
            on: false,
            include_all_hidden: false,
            veil: DEFAULT_VEIL,
            cache: Default::default(),
            selected: Default::default(),
        }
    }
}

impl OcrOverlay {
    /// F1: 켜고 끈다. 끄면 골라 둔 낱말도 놓는다.
    pub fn toggle(&mut self) -> bool {
        self.on = !self.on;
        if !self.on {
            self.selected.set(None);
        }
        self.on
    }

    /// 지금 고른 낱말(쪽, 자리).
    pub fn selected(&self) -> Option<(u32, usize)> {
        self.selected.get()
    }

    /// 낱말을 고른다. `None`이면 고른 것을 놓는다.
    pub fn select(&self, at: Option<(u32, usize)>) {
        self.selected.set(at);
    }

    /// 고른 낱말이 있는가 — 화살표 키를 이 기능이 가져갈지 정하는 데 쓴다.
    pub fn has_selection(&self) -> bool {
        self.selected.get().is_some()
    }

    /// 고른 낱말에서 화살표 방향으로 이웃 낱말로 옮긴다.
    ///
    /// **방향은 화면 기준으로 받는다** — `down`이 양수면 화면 아래쪽이다. 낱말 좌표는 PDF 사용자
    /// 공간이라 y가 위로 커지므로 안에서 뒤집는다.
    ///
    /// **고르는 규칙**(2026-09-30 사용자 지정). 오른쪽 화살표를 예로 들면:
    ///
    /// 1. **두 모서리가 모두 오른쪽에 있는 상자**만 후보다(`PX1 < NX1` 그리고 `PX2 < NX2`).
    ///    상자끼리 가로로 겹쳐 있어도 두 모서리가 더 오른쪽이면 후보에 든다 — OCR 상자는 자주
    ///    겹치므로 "떨어져 있을 것"을 요구하면 놓친다.
    /// 2. **같은 줄인지**는 고른 상자의 세로 범위를 가로로 늘렸을 때 걸치는지로 본다. 물리적으로
    ///    상자가 포개졌는지가 아니라 "그 줄에 속한다고 볼 수 있는지"의 판정이다.
    /// 3. 걸침이 고른 상자 높이의 `OVERLAP_RATE` 이상인 것을 **먼저** 본다. 그런 상자가 하나도
    ///    없을 때만 덜 걸친 것에서 고른다 — 위첨자처럼 살짝 걸친 상자도 갈 데가 없으면 갈 수
    ///    있어야 하기 때문이다(영영 못 가는 상자를 만들지 않는다).
    /// 4. 그 안에서 **움직이는 방향의 틈**이 가장 작은 것, 같으면 중앙점 거리가 짧은 것.
    ///
    /// 위아래 화살표는 축만 바꿔 같은 규칙을 쓴다(넘김 판정은 y, 걸침 판정은 x).
    ///
    /// **줄 끝에서는 줄을 바꾼다**(가로 이동만). 걸치는 상자가 하나도 없으면 다음 줄(오른쪽이면
    /// 아래, 왼쪽이면 위)로 넘어가 그 줄의 **반대쪽 끝**을 고른다. 읽는 순서 그대로다. 세로
    /// 이동에는 적용하지 않는다 — 쪽을 넘어가는 이동은 이 기능의 몫이 아니다.
    pub fn move_selection(&self, right: f64, down: f64) -> bool {
        let Some((page, index)) = self.selected.get() else { return false };
        let cache = self.cache.borrow();
        let Some(words) = cache.get(&page) else { return false };
        let Some(from) = words.get(index) else { return false };
        let (dx, dy) = (right, -down);
        let (axis, sign) = if dx != 0.0 { (0usize, dx.signum()) } else { (1usize, dy.signum()) };

        let next = in_line(words, index, axis, sign).or_else(|| {
            // 가로로 갈 데가 없으면 줄을 바꾼다. 세로 이동에는 적용하지 않는다.
            (axis == 0).then(|| wrap_line(words, index, sign)).flatten()
        });
        match next {
            Some(i) => {
                self.selected.set(Some((page, i)));
                true
            }
            None => false,
        }
    }

    /// 범위를 바꾼다. 대상이 달라지므로 재어 둔 것을 버린다.
    #[allow(clippy::needless_pass_by_ref_mut)]
    pub fn set_include_all_hidden(&mut self, include: bool) {
        if self.include_all_hidden != include {
            self.include_all_hidden = include;
            self.invalidate();
        }
    }

    /// 메뉴 첫 줄에 적을 말. 범위에 따라 가리키는 대상 이름이 바뀐다.
    pub fn toggle_label(&self) -> String {
        let what = if self.include_all_hidden { "보이지 않는 텍스트" } else { "OCR" };
        let action = if self.on { "숨김" } else { "표시" };
        format!("{what} {action} (F1)")
    }

    /// 문서가 바뀌었을 때. 모드는 그대로 두고 잰 것만 버린다.
    pub fn invalidate(&mut self) {
        self.cache.borrow_mut().clear();
        // 자리 번호는 그 쪽을 잰 결과에 매인다 — 다시 재면 가리키던 것이 달라진다.
        self.selected.set(None);
    }

    /// 이 쪽에 그릴 낱말들을 꺼내 `draw`에 넘긴다. 아직 재지 않았으면 여기서 잰다.
    ///
    /// 닫힘꼴로 넘기는 것은 빌림을 이 호출 안에 가두기 위해서다 — 슬라이스를 돌려주면 `RefCell`의
    /// 빌림이 밖으로 새어 나가 같은 프레임의 다음 쪽을 잴 때 겹친다.
    pub fn with_words<R>(&self, page: &PdfPage, page_number: u32, draw: impl FnOnce(&[Word]) -> R) -> R {
        if !self.on {
            return draw(&[]);
        }
        {
            let mut cache = self.cache.borrow_mut();
            if !cache.contains_key(&page_number) {
                if cache.len() >= CACHE_PAGES {
                    cache.clear();
                }
                let words = measure(page, self.include_all_hidden);
                cache.insert(page_number, words);
            }
        }
        let cache = self.cache.borrow();
        draw(cache.get(&page_number).map(Vec::as_slice).unwrap_or(&[]))
    }
}

/// `[low, high]` 꼴로 꺼낸다. bounds는 `[left, bottom, right, top]`이다.
fn span(w: &Word, axis: usize) -> (f64, f64) {
    if axis == 0 { (w.bounds[0], w.bounds[2]) } else { (w.bounds[1], w.bounds[3]) }
}

fn center(w: &Word) -> (f64, f64) {
    ((w.bounds[0] + w.bounds[2]) / 2.0, (w.bounds[1] + w.bounds[3]) / 2.0)
}

/// 고른 상자의 세로(또는 가로) 범위에 후보가 얼마나 걸치는지. 0 이하면 다른 줄이다.
fn straddle(from: &Word, w: &Word, cross: usize) -> f64 {
    let (a_low, a_high) = span(from, cross);
    let (b_low, b_high) = span(w, cross);
    b_high.min(a_high) - b_low.max(a_low)
}

/// 같은 줄 안에서 그 방향의 이웃을 고른다(규칙 1~4).
fn in_line(words: &[Word], index: usize, axis: usize, sign: f64) -> Option<usize> {
    let from = &words[index];
    let cross = 1 - axis;
    let (from_low, from_high) = span(from, axis);
    let (c_low, c_high) = span(from, cross);
    let threshold = OVERLAP_RATE * (c_high - c_low);
    let (fx, fy) = center(from);

    words
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != index)
        .filter_map(|(i, w)| {
            let (low, high) = span(w, axis);
            // 1. 두 모서리가 모두 그 방향으로 넘어가 있어야 한다(겹쳐 있어도 된다).
            let ahead = if sign > 0.0 { low > from_low && high > from_high } else { low < from_low && high < from_high };
            if !ahead {
                return None;
            }
            // 2. 같은 줄로 볼 만큼 걸쳐야 한다.
            let overlap = straddle(from, w, cross);
            if overlap <= 0.0 {
                return None;
            }
            // 3~4. 잘 걸친 것을 먼저, 그다음 이동 방향의 틈이 작은 것, 그다음 중앙점 거리.
            let gap = if sign > 0.0 { low - from_high } else { from_low - high };
            let (x, y) = center(w);
            Some((i, (overlap < threshold, gap.max(0.0), (x - fx).hypot(y - fy))))
        })
        .min_by(|(_, a), (_, b)| a.0.cmp(&b.0).then_with(|| a.1.total_cmp(&b.1)).then_with(|| a.2.total_cmp(&b.2)))
        .map(|(i, _)| i)
}

/// 줄 끝에서 다음 줄로 넘어간다. 오른쪽이면 아래 줄, 왼쪽이면 위 줄로 가서 **반대쪽 끝**을 고른다.
///
/// 다음 줄은 "고른 상자의 세로 범위에 걸치지 않으면서 그쪽에 있는 상자들 가운데 세로 틈이 가장
/// 작은 것"이 정한다. 그 상자와 같은 줄인 것들(그 상자의 세로 범위에 걸치는 것들) 중에서 고른
/// 상자로부터 중앙점이 가장 먼 것을 고르면 그 줄의 반대쪽 끝이 된다.
fn wrap_line(words: &[Word], index: usize, sign: f64) -> Option<usize> {
    let from = &words[index];
    // 오른쪽으로 가다 막히면 아래 줄(PDF에서 y가 작은 쪽), 왼쪽이면 위 줄.
    let toward = -sign;
    let (fy_low, fy_high) = span(from, 1);
    let (fx, fy) = center(from);

    // 다음 줄을 대표할 상자: 세로로 걸치지 않으면서 그쪽에 있고, 세로 틈이 가장 작은 것.
    let anchor = words
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != index)
        .filter_map(|(i, w)| {
            if straddle(from, w, 1) > 0.0 {
                return None;
            }
            let (low, high) = span(w, 1);
            let gap = if toward > 0.0 { low - fy_high } else { fy_low - high };
            (gap > 0.0).then_some((i, gap))
        })
        .min_by(|(_, a), (_, b)| a.total_cmp(b))
        .map(|(i, _)| i)?;

    // 그 줄에서 가장 먼 끝.
    let row = &words[anchor];
    words
        .iter()
        .enumerate()
        .filter(|(i, w)| *i != index && straddle(row, w, 1) > 0.0)
        .map(|(i, w)| {
            let (x, y) = center(w);
            (i, (x - fx).hypot(y - fy))
        })
        .max_by(|(_, a), (_, b)| a.total_cmp(b))
        .map(|(i, _)| i)
}

/// 한 쪽의 안 보이는 텍스트를 낱말로 묶어 돌려준다.
///
/// 글자를 하나씩 그리지 않고 [`build_lines`]로 묶는 이유: 내보내기가 쓰는 것과 **같은 묶기**를
/// 써야 화면에서 본 것과 내보낸 hOCR이 어긋나지 않는다. 글자 단위로 그리면 테두리가 수천 개가
/// 되어 읽을 수도 없다.
fn measure(page: &PdfPage, include_all_hidden: bool) -> Vec<Word> {
    let Ok(page_box) = pdf_engine::text_layer::page_box(page) else { return Vec::new() };
    let [llx, lly, urx, ury] = page_box.crop;
    let frame = pdf_ocr::geometry::PageFrame {
        crop: pdf_ocr::geometry::Rect { llx, lly, urx, ury },
        rotate: page_box.rotate,
        user_unit: 1.0,
    };
    // `/Rotate`로 돌린 스캔은 회전 전 프레임에서 글자가 바로 서 있다 — 줄 묶기는 거기서 한다
    // (내보내기의 `extract_page`와 같은 순서).
    let upright = frame.upright();
    let scans = image_boxes(page);
    let Ok(chars) = pdf_engine::text_layer::page_chars(page) else { return Vec::new() };

    // 어느 글자를 "안 보이는 것"으로 볼지가 모드의 전부다. 줄 묶기는 그 표시만 보고 움직인다.
    let mut ocr_flags = Vec::with_capacity(chars.len());
    let input: Vec<InputChar> = chars
        .iter()
        .map(|c| {
            let is_ocr = c.invisible_mode && on_scan(&c.bounds, &scans);
            ocr_flags.push(is_ocr);
            InputChar {
                ch: c.ch,
                rect: upright.user_rect_to_display(c.bounds),
                baseline: c.origin.map(|(x, y)| upright.user_to_display(x, y).1),
                generated: c.generated,
                invisible: if include_all_hidden { c.invisible } else { is_ocr },
            }
        })
        .collect();

    // 낱말이 OCR 텍스트인지는 그 낱말에 속한 글자로 정한다. 한 낱말에 섞이는 일은 거의 없지만,
    // 섞이면 "OCR이 하나라도 있으면 OCR"로 본다(색은 눈에 띄는 쪽을 쓴다).
    let ocr_boxes: Vec<DRect> = chars
        .iter()
        .zip(&ocr_flags)
        .filter(|(_, is_ocr)| **is_ocr)
        .map(|(c, _)| upright.user_rect_to_display(c.bounds))
        .collect();

    let options = LayoutOptions { source: TextSource::InvisibleOnly, keep_format_chars: false };
    let (lines, _) = build_lines(&input, &options);
    frame
        .orient_lines(lines)
        .into_iter()
        .flat_map(|line| line.words)
        .filter(|word| !word.text.trim().is_empty())
        .map(|word| Word {
            is_ocr: ocr_boxes.iter().any(|b| overlaps(b, &word.rect)),
            bounds: {
                let (a, b) = (frame.display_to_user(word.rect.x0, word.rect.y0), frame.display_to_user(word.rect.x1, word.rect.y1));
                [a.0.min(b.0), a.1.min(b.1), a.0.max(b.0), a.1.max(b.1)]
            },
            text: word.text,
        })
        .collect()
}

fn overlaps(a: &DRect, b: &DRect) -> bool {
    a.x0 <= b.x1 && b.x0 <= a.x1 && a.y0 <= b.y1 && b.y0 <= a.y1
}

/// 이 쪽의 이미지 상자들(사용자 공간). `ocr_worker`의 같은 이름 함수와 같은 기준이다.
fn image_boxes(page: &PdfPage) -> Vec<[f64; 4]> {
    use pdfium_render::prelude::{PdfPageObjectCommon, PdfPageObjectsCommon};
    page.objects()
        .iter()
        .filter(|object| object.as_image_object().is_some())
        .filter_map(|object| object.bounds().ok())
        .map(|b| [b.left().value as f64, b.bottom().value as f64, b.right().value as f64, b.top().value as f64])
        .collect()
}

/// 글자 상자가 이미지 영역 안이거나 걸쳐 있는지. 경계가 닿는 것도 걸친 것으로 본다.
fn on_scan(bounds: &[f64; 4], scans: &[[f64; 4]]) -> bool {
    if scans.is_empty() {
        return false;
    }
    let degenerate = bounds[0] >= bounds[2] || bounds[1] >= bounds[3];
    scans.iter().any(|s| degenerate || (s[0] <= bounds[2] && bounds[0] <= s[2] && s[1] <= bounds[3] && bounds[1] <= s[3]))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// F1은 켜고 끄기만 한다 — 범위는 따로 정한다(2026-09-29 결정).
    #[test]
    fn f1_only_toggles_on_and_off() {
        let mut overlay = OcrOverlay::default();
        assert!(!overlay.on);
        assert!(overlay.toggle());
        assert!(!overlay.toggle());
        assert!(!overlay.include_all_hidden, "F1이 범위를 건드리면 안 된다");
    }

    /// 메뉴 첫 줄은 범위와 켜짐 여부를 함께 드러낸다.
    #[test]
    fn the_menu_line_names_what_it_will_show() {
        let mut overlay = OcrOverlay::default();
        assert_eq!(overlay.toggle_label(), "OCR 표시 (F1)");
        overlay.on = true;
        assert_eq!(overlay.toggle_label(), "OCR 숨김 (F1)");
        overlay.set_include_all_hidden(true);
        assert_eq!(overlay.toggle_label(), "보이지 않는 텍스트 숨김 (F1)");
    }

    /// 범위를 바꾸면 재어 둔 것을 버려야 한다 — 대상이 달라지기 때문이다.
    #[test]
    fn changing_the_scope_drops_what_was_measured() {
        let mut overlay = OcrOverlay::default();
        overlay.cache.borrow_mut().insert(
            3,
            vec![Word { bounds: [0.0, 0.0, 1.0, 1.0], text: "가".to_string(), is_ocr: true }],
        );
        overlay.set_include_all_hidden(true);
        assert!(overlay.cache.borrow().is_empty());
        // 같은 값을 다시 넣는 것은 아무 일도 하지 않는다.
        overlay.cache.borrow_mut().insert(3, Vec::new());
        overlay.set_include_all_hidden(true);
        assert_eq!(overlay.cache.borrow().len(), 1);
    }

    fn word(x0: f64, y0: f64, x1: f64, y1: f64, text: &str) -> Word {
        Word { bounds: [x0, y0, x1, y1], text: text.to_string(), is_ocr: true }
    }

    /// 화살표는 그 방향으로 **넘어간** 낱말만 후보로 보고, 옆으로 벗어난 정도에 벌점을 준다.
    /// 한 줄을 훑을 때 옆줄로 튀지 않아야 한다.
    #[test]
    fn arrows_walk_along_the_line_before_jumping_rows() {
        // 윗줄 "가 나 다", 아랫줄 "라"가 '나' 바로 밑에 있다(PDF 좌표는 y가 위로 커진다).
        let overlay = OcrOverlay { on: true, ..Default::default() };
        overlay.cache.borrow_mut().insert(
            1,
            vec![
                word(0.0, 100.0, 10.0, 110.0, "가"),
                word(20.0, 100.0, 30.0, 110.0, "나"),
                word(40.0, 100.0, 50.0, 110.0, "다"),
                word(20.0, 80.0, 30.0, 90.0, "라"),
            ],
        );
        let text = |overlay: &OcrOverlay| {
            let (page, index) = overlay.selected().unwrap();
            overlay.cache.borrow()[&page][index].text.clone()
        };

        overlay.select(Some((1, 0))); // "가"
        assert!(overlay.move_selection(1.0, 0.0));
        assert_eq!(text(&overlay), "나", "오른쪽은 같은 줄의 다음 낱말");
        assert!(overlay.move_selection(1.0, 0.0));
        assert_eq!(text(&overlay), "다");
        // 줄 끝에서 오른쪽은 아랫줄 머리로 넘어간다(아래에 "라"가 있다).
        assert!(overlay.move_selection(1.0, 0.0));
        assert_eq!(text(&overlay), "라");

        // 방향은 화면 기준이다 — 아래 화살표는 +1이고, 그 자리 낱말은 PDF 좌표로 y가 작다.
        overlay.select(Some((1, 1))); // "나"
        assert!(overlay.move_selection(0.0, 1.0));
        assert_eq!(text(&overlay), "라", "아래 화살표는 바로 밑 낱말로");
        assert!(overlay.move_selection(0.0, -1.0));
        assert_eq!(text(&overlay), "나", "위 화살표로 되돌아온다");
    }

    /// 중심점 거리로 고르던 때 틀렸던 배치들. 넓은 낱말은 중심이 멀어 지고, 좁은 낱말은 딴
    /// 줄에 있어도 이겼다(2026-09-30 리포트: "인근 박스가 있어도 몇 단계 점프하거나 딴 방향으로
    /// 튄다").
    #[test]
    fn a_wide_neighbour_wins_over_a_far_narrow_one_on_another_line() {
        let overlay = OcrOverlay { on: true, ..Default::default() };
        overlay.cache.borrow_mut().insert(
            1,
            vec![
                word(0.0, 100.0, 10.0, 110.0, "기준"),
                // 바로 오른쪽에 붙은 아주 넓은 낱말 — 중심은 멀지만 가장자리는 붙어 있다.
                word(12.0, 100.0, 200.0, 110.0, "아주긴낱말"),
                // 윗줄의 좁은 낱말 — 중심 거리로는 더 가깝다.
                word(14.0, 120.0, 20.0, 130.0, "딴줄"),
            ],
        );
        let text = |o: &OcrOverlay| {
            let (page, index) = o.selected().unwrap();
            o.cache.borrow()[&page][index].text.clone()
        };
        overlay.select(Some((1, 0)));
        assert!(overlay.move_selection(1.0, 0.0));
        assert_eq!(text(&overlay), "아주긴낱말", "같은 줄에서 가장자리가 가까운 쪽");
    }

    /// 줄 끝에서는 다음 줄의 반대쪽 끝으로 넘어간다(2026-09-30 사용자 지정). 오른쪽으로 가다
    /// 막히면 아래 줄 머리로, 왼쪽으로 가다 막히면 위 줄 끝으로 — 읽는 순서 그대로다.
    #[test]
    fn horizontal_arrows_wrap_to_the_next_line() {
        let overlay = OcrOverlay { on: true, ..Default::default() };
        overlay.cache.borrow_mut().insert(
            1,
            vec![
                word(0.0, 100.0, 10.0, 110.0, "윗줄머리"),
                word(20.0, 100.0, 30.0, 110.0, "윗줄끝"),
                word(0.0, 80.0, 10.0, 90.0, "아랫줄머리"),
                word(20.0, 80.0, 30.0, 90.0, "아랫줄끝"),
            ],
        );
        let at = |o: &OcrOverlay| o.selected().unwrap().1;

        overlay.select(Some((1, 1))); // 윗줄끝
        assert!(overlay.move_selection(1.0, 0.0));
        assert_eq!(at(&overlay), 2, "오른쪽으로 막히면 아랫줄 머리로");

        overlay.select(Some((1, 2))); // 아랫줄머리
        assert!(overlay.move_selection(-1.0, 0.0));
        assert_eq!(at(&overlay), 1, "왼쪽으로 막히면 윗줄 끝으로");

        // 마지막 줄 끝에서 오른쪽은 더 갈 데가 없다.
        overlay.select(Some((1, 3)));
        assert!(!overlay.move_selection(1.0, 0.0));
    }

    /// 세로 이동에는 줄바꿈을 적용하지 않는다 — 쪽을 넘어가는 이동은 이 기능의 몫이 아니다.
    #[test]
    fn vertical_arrows_do_not_wrap() {
        let overlay = OcrOverlay { on: true, ..Default::default() };
        overlay.cache.borrow_mut().insert(
            1,
            vec![word(0.0, 100.0, 10.0, 110.0, "위"), word(80.0, 80.0, 90.0, 90.0, "아래멀리")],
        );
        overlay.select(Some((1, 0)));
        assert!(!overlay.move_selection(0.0, 1.0), "가로로 걸치지 않으면 내려가지 않는다");
    }

    /// 세로 이동은 **가장 가까운 줄**을 먼저 본다. 중앙점 거리만 보면 두세 줄 아래로 건너뛴다
    /// (2026-09-30 리포트: "수직방향 이동시 점프가 빈번").
    #[test]
    fn vertical_moves_take_the_nearest_row_not_the_nearest_centre() {
        let overlay = OcrOverlay { on: true, ..Default::default() };
        overlay.cache.borrow_mut().insert(
            1,
            vec![
                word(0.0, 100.0, 40.0, 110.0, "기준"),
                // 바로 아래 줄이지만 가로로 살짝 어긋나 중앙점은 조금 멀다.
                word(18.0, 84.0, 58.0, 94.0, "바로아래"),
                // 두 줄 아래인데 가로 위치가 똑같아 중앙점 거리는 더 가까울 수 있다.
                word(0.0, 60.0, 40.0, 70.0, "두줄아래"),
            ],
        );
        overlay.select(Some((1, 0)));
        assert!(overlay.move_selection(0.0, 1.0));
        assert_eq!(overlay.selected().unwrap().1, 1, "바로 아래 줄로 가야 한다");
    }

    /// 아래 화살표는 가로로 겹치는 상자로만 간다    /// 아래 화살표는 가로로 겹치는 상자로만 간다 — 바로 밑에 있는 것.
    #[test]
    fn down_goes_to_the_box_right_below() {
        let overlay = OcrOverlay { on: true, ..Default::default() };
        overlay.cache.borrow_mut().insert(
            1,
            vec![
                word(20.0, 100.0, 40.0, 110.0, "기준"),
                word(22.0, 80.0, 42.0, 90.0, "바로밑"),   // 가로로 크게 겹친다
                word(80.0, 80.0, 100.0, 90.0, "멀리밑"),  // 겹치지 않는다
            ],
        );
        overlay.select(Some((1, 0)));
        assert!(overlay.move_selection(0.0, 1.0));
        assert_eq!(overlay.selected(), Some((1, 1)));
    }

    /// 기준치(높이의 50%)를 넘게 겹친 상자가 있으면, 더 가깝더라도 살짝만 겹친 상자에는 가지
    /// 않는다. 위첨자나 쉼표처럼 줄에 살짝 걸친 것으로 튀는 것을 막는다.
    #[test]
    fn a_well_overlapped_box_beats_a_closer_sliver() {
        let overlay = OcrOverlay { on: true, ..Default::default() };
        overlay.cache.borrow_mut().insert(
            1,
            vec![
                word(0.0, 100.0, 10.0, 120.0, "기준"),      // 높이 20, 기준치는 10
                word(11.0, 118.0, 16.0, 124.0, "윗첨자"),   // 겹침 2 — 더 가깝지만 살짝만 걸쳤다
                word(20.0, 100.0, 40.0, 120.0, "같은줄"),   // 겹침 20
            ],
        );
        overlay.select(Some((1, 0)));
        assert!(overlay.move_selection(1.0, 0.0));
        assert_eq!(overlay.selected(), Some((1, 2)), "기준치를 넘게 겹친 쪽으로 가야 한다");
    }

    /// 살짝 겹쳐 있는 이웃도 그 방향에 있는 것으로 본다(OCR 상자는 자주 겹친다).
    #[test]
    fn slightly_overlapping_neighbours_still_count() {
        let overlay = OcrOverlay { on: true, ..Default::default() };
        overlay.cache.borrow_mut().insert(
            1,
            vec![word(0.0, 100.0, 10.0, 110.0, "가"), word(9.5, 100.0, 20.0, 110.0, "나")],
        );
        overlay.select(Some((1, 0)));
        assert!(overlay.move_selection(1.0, 0.0));
        assert_eq!(overlay.selected(), Some((1, 1)));
    }

    /// F1로 끄거나 다시 재면 골라 둔 것을 놓는다 — 자리 번호가 그 쪽을 잰 결과에 매여 있다.
    #[test]
    fn the_selection_is_dropped_when_it_could_go_stale() {
        let mut overlay = OcrOverlay { on: true, ..Default::default() };
        overlay.select(Some((1, 0)));
        overlay.toggle();
        assert!(!overlay.has_selection(), "끄면 놓는다");

        overlay.toggle();
        overlay.select(Some((1, 0)));
        overlay.invalidate();
        assert!(!overlay.has_selection(), "다시 재면 놓는다");
    }

    /// 실제 스캔 PDF에서 OCR 낱말을 집어내는지. 이 샘플은 22~24쪽에만 OCR이 있고, 내보내기가
    /// 같은 기준으로 609개를 찾는다(2026-09-29 실측) — 화면에 그리는 것도 그와 같아야 한다.
    #[test]
    fn ocr_words_are_found_on_a_real_scan() {
        let Some(engine) = crate::app::create_engine() else {
            eprintln!("pdfium이 없어 건너뜀");
            return;
        };
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../pdf-samples/BZR001088_01-mod.pdf");
        let Ok(document) = engine.open_document(&path) else {
            eprintln!("샘플이 없어 건너뜀: {}", path.display());
            return;
        };
        let overlay = OcrOverlay { on: true, ..Default::default() };

        let page = document.pages().get(22).unwrap();
        overlay.with_words(&page, 23, |words| {
            assert!(!words.is_empty(), "23쪽에서 OCR 낱말을 찾지 못했다");
            assert!(words.iter().all(|w| w.is_ocr), "OCR이 아닌 것으로 잡힌 낱말이 있다");
            assert!(words.iter().any(|w| w.text.contains("SCREEN")), "{:?}", words.iter().take(8).collect::<Vec<_>>());
        });

        let blank = document.pages().get(0).unwrap();
        overlay.with_words(&blank, 1, |words| assert!(words.is_empty(), "{words:?}"));
    }
}
