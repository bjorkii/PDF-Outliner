//! OCR 표시/숨기기 모드(예약 6) — 보이지 않는 텍스트를 화면에 꺼내 보여 준다.
//!
//! 스캔 PDF의 OCR 텍스트는 정의상 화면에 안 보인다. 그래서 "이 쪽에 OCR이 제대로 들어갔나",
//! "왜 이 낱말이 검색에 안 걸리나", "가져온 OCR이 자리를 맞게 잡았나"를 눈으로 확인할 길이
//! 없었다. 이 모드는 그 텍스트의 **자리(테두리)와 내용(글자)** 을 지면 위에 겹쳐 그린다.
//!
//! **세 단계로 돈다**(F1 또는 OCR 메뉴): 꺼짐 → OCR 텍스트만 → 보이지 않는 텍스트 전부 → 꺼짐.
//! 평소에는 이 앱이 실제로 건드리는 것(OCR 텍스트 = 이미지 영역 안이거나 걸친 `3 Tr`)만 보여
//! 주고, 한 번 더 누르면 `7 Tr`·알파 0·크기 0까지 포함한 **안 보이는 텍스트 전부**를 보여 준다.
//! 둘을 나눈 이유는 확인 창·삭제가 쓰는 기준이 앞의 것이기 때문이다 — 뒤의 것까지 한꺼번에
//! 보여 주면 "여기 보이는데 왜 안 지워지나"가 된다.
//!
//! **측정은 보고 있는 쪽만, 결과는 캐시한다.** 한 쪽의 글자를 pdfium에서 꺼내 줄로 묶는 일은
//! 수백 쪽 문서에서도 한 쪽분이면 가볍지만, 매 프레임 다시 하면 넘김이 끊긴다.

use pdf_ocr::layout::{build_lines, DRect, InputChar, LayoutOptions, TextSource};
use pdfium_render::prelude::PdfPage;

/// 지금 무엇을 보여 주는 중인가.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OverlayMode {
    #[default]
    Off,
    /// OCR 텍스트만 — 내보내기·삭제가 건드리는 것과 같은 기준.
    OcrOnly,
    /// 보이지 않는 텍스트 전부(`7 Tr`·알파 0·크기 0 포함).
    AllHidden,
}

impl OverlayMode {
    /// 꺼짐 → OCR만 → 전부 → 꺼짐.
    pub fn next(self) -> Self {
        match self {
            OverlayMode::Off => OverlayMode::OcrOnly,
            OverlayMode::OcrOnly => OverlayMode::AllHidden,
            OverlayMode::AllHidden => OverlayMode::Off,
        }
    }

    pub fn is_on(self) -> bool {
        self != OverlayMode::Off
    }

    /// 상태표시줄·메뉴에 적을 이름.
    pub fn label(self) -> &'static str {
        match self {
            OverlayMode::Off => "OCR 표시 꺼짐",
            OverlayMode::OcrOnly => "OCR 텍스트 표시",
            OverlayMode::AllHidden => "보이지 않는 텍스트 전부 표시",
        }
    }
}

/// 화면에 그릴 낱말 하나.
#[derive(Debug, Clone, PartialEq)]
pub struct Word {
    /// 페이지 사용자 공간 `[left, bottom, right, top]`.
    pub bounds: [f64; 4],
    pub text: String,
    /// OCR 텍스트(이미지 영역 안이거나 걸친 `3 Tr`)인가. 아니면 그 밖의 안 보이는 텍스트다.
    pub is_ocr: bool,
}

/// 모드와, 이미 재어 둔 쪽들.
///
/// 캐시를 `RefCell`로 두는 이유: 그리는 쪽(`viewer_panel`의 draw 함수들)은 앱을 `&`로만 받는데,
/// 연속 스크롤에서는 한 프레임에 여러 쪽이 보여 어느 쪽을 잴지 미리 알 수 없다. 그릴 때 그 쪽을
/// 재서 채우는 편이 "보이는 쪽 목록을 미리 뽑아 넘기는" 것보다 단순하다.
#[derive(Default)]
pub struct OcrOverlay {
    pub mode: OverlayMode,
    cache: std::cell::RefCell<std::collections::HashMap<u32, Vec<Word>>>,
}

/// 캐시에 담아 둘 쪽 수. 연속 스크롤로 쭉 내려가도 무한정 쌓이지 않게 한다.
const CACHE_PAGES: usize = 8;

impl OcrOverlay {
    /// 다음 단계로 돌린다. 재어 둔 것은 버린다(모드가 바뀌면 대상이 달라진다).
    pub fn cycle(&mut self) -> OverlayMode {
        self.mode = self.mode.next();
        self.invalidate();
        self.mode
    }

    /// 문서가 바뀌었을 때. 모드는 그대로 두고 잰 것만 버린다.
    pub fn invalidate(&mut self) {
        self.cache.borrow_mut().clear();
    }

    /// 이 쪽에 그릴 낱말들을 꺼내 `draw`에 넘긴다. 아직 재지 않았으면 여기서 잰다.
    ///
    /// 닫힘꼴로 넘기는 것은 빌림을 이 호출 안에 가두기 위해서다 — 슬라이스를 돌려주면 `RefCell`의
    /// 빌림이 밖으로 새어 나가 같은 프레임의 다음 쪽을 잴 때 겹친다.
    pub fn with_words<R>(&self, page: &PdfPage, page_number: u32, draw: impl FnOnce(&[Word]) -> R) -> R {
        if !self.mode.is_on() {
            return draw(&[]);
        }
        {
            let mut cache = self.cache.borrow_mut();
            if !cache.contains_key(&page_number) {
                if cache.len() >= CACHE_PAGES {
                    cache.clear();
                }
                let words = measure(page, self.mode);
                cache.insert(page_number, words);
            }
        }
        let cache = self.cache.borrow();
        draw(cache.get(&page_number).map(Vec::as_slice).unwrap_or(&[]))
    }
}

/// 한 쪽의 안 보이는 텍스트를 낱말로 묶어 돌려준다.
///
/// 글자를 하나씩 그리지 않고 [`build_lines`]로 묶는 이유: 내보내기가 쓰는 것과 **같은 묶기**를
/// 써야 화면에서 본 것과 내보낸 hOCR이 어긋나지 않는다. 글자 단위로 그리면 테두리가 수천 개가
/// 되어 읽을 수도 없다.
fn measure(page: &PdfPage, mode: OverlayMode) -> Vec<Word> {
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
                invisible: match mode {
                    OverlayMode::OcrOnly => is_ocr,
                    OverlayMode::AllHidden => c.invisible,
                    OverlayMode::Off => false,
                },
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

    #[test]
    fn mode_cycles_through_three_states() {
        let mut overlay = OcrOverlay::default();
        assert_eq!(overlay.mode, OverlayMode::Off);
        assert_eq!(overlay.cycle(), OverlayMode::OcrOnly);
        assert_eq!(overlay.cycle(), OverlayMode::AllHidden);
        assert_eq!(overlay.cycle(), OverlayMode::Off);
    }

    /// 모드를 돌리면 재어 둔 것을 버려야 한다 — 대상이 달라지기 때문이다.
    #[test]
    fn cycling_drops_what_was_measured() {
        let overlay = OcrOverlay::default();
        overlay.cache.borrow_mut().insert(
            3,
            vec![Word { bounds: [0.0, 0.0, 1.0, 1.0], text: "가".to_string(), is_ocr: true }],
        );
        let mut overlay = overlay;
        overlay.cycle();
        assert!(overlay.cache.borrow().is_empty());
    }

    /// 꺼져 있으면 쪽을 재지 않는다(pdfium을 부르지 않는다).
    #[test]
    fn nothing_is_measured_while_off() {
        let overlay = OcrOverlay::default();
        assert_eq!(overlay.mode, OverlayMode::Off);
        assert!(overlay.cache.borrow().is_empty());
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
        let overlay = OcrOverlay { mode: OverlayMode::OcrOnly, ..Default::default() };

        // 글이 있는 쪽(23쪽, 0부터 세어 22)에서는 낱말이 나오고 모두 OCR 텍스트로 잡힌다.
        let page = document.pages().get(22).unwrap();
        overlay.with_words(&page, 23, |words| {
            assert!(!words.is_empty(), "23쪽에서 OCR 낱말을 찾지 못했다");
            assert!(words.iter().all(|w| w.is_ocr), "OCR이 아닌 것으로 잡힌 낱말이 있다");
            assert!(words.iter().any(|w| w.text.contains("SCREEN")), "{:?}", words.iter().take(8).collect::<Vec<_>>());
        });

        // 글이 없는 쪽에서는 아무것도 나오지 않는다.
        let blank = document.pages().get(0).unwrap();
        overlay.with_words(&blank, 1, |words| assert!(words.is_empty(), "{words:?}"));
    }
}
