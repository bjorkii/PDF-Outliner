//! 문자 → 단어 → 줄 구성(설계 문서 5.2). 좌표는 표시 페이지 프레임(`geometry`) 기준 포인트이고
//! 원점은 좌상단, y는 아래로 커진다.
//!
//! 순서는 입력 순서(pdfium 추출 순서 = 콘텐츠 스트림 순서)를 따른다. 다단 조판에서는 읽기 순서와
//! 다를 수 있다. 구분은 두 가지를 함께 쓴다.
//! - 문자로 표현된 구분: 공백 문자(pdfium이 추론해 넣은 것 포함)는 단어 경계, 줄바꿈 문자는 줄 경계.
//! - 기하 구분: 앞 글자와 세로로 거의 겹치지 않거나 크게 뒤로 돌아가면 줄 경계, 글자 높이의 절반
//!   넘게 떨어져 있으면 단어 경계. pdfium이 구분 문자를 넣지 않은 경우를 위한 것이다.
//!
//! 세로쓰기(글자가 위에서 아래로 쌓이는 경우)는 가로 규칙으로는 글자마다 줄이 나뉘므로 따로
//! 잡는다: 앞 글자와 가로로 겹치면서 아래로 이어지면 같은 줄(세로줄)로 보고, 글자마다 단어를
//! 하나씩 둔다. 줄은 `vertical`로 표시한다. 페이지 안에서 회전된 텍스트(가로도 세로도 아닌 각도)는
//! 여전히 글자마다 나뉠 수 있다.

use unicode_normalization::UnicodeNormalization;

/// 표시 프레임 사각형(x0 < x1, y0 < y1, y0이 위쪽).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DRect {
    pub x0: f64,
    pub y0: f64,
    pub x1: f64,
    pub y1: f64,
}

impl DRect {
    pub fn from_points(points: &[(f64, f64)]) -> Self {
        let mut r = DRect { x0: f64::INFINITY, y0: f64::INFINITY, x1: f64::NEG_INFINITY, y1: f64::NEG_INFINITY };
        for &(x, y) in points {
            r.x0 = r.x0.min(x);
            r.y0 = r.y0.min(y);
            r.x1 = r.x1.max(x);
            r.y1 = r.y1.max(y);
        }
        r
    }

    pub fn width(&self) -> f64 {
        self.x1 - self.x0
    }

    pub fn height(&self) -> f64 {
        self.y1 - self.y0
    }

    pub fn union(&self, other: &DRect) -> DRect {
        DRect {
            x0: self.x0.min(other.x0),
            y0: self.y0.min(other.y0),
            x1: self.x1.max(other.x1),
            y1: self.y1.max(other.y1),
        }
    }

    fn vertical_overlap(&self, other: &DRect) -> f64 {
        (self.y1.min(other.y1) - self.y0.max(other.y0)).max(0.0)
    }

    fn horizontal_overlap(&self, other: &DRect) -> f64 {
        (self.x1.min(other.x1) - self.x0.max(other.x0)).max(0.0)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct InputChar {
    pub ch: char,
    pub rect: DRect,
    /// 기준선의 표시 프레임 y(글리프 원점).
    pub baseline: Option<f64>,
    /// 추출기가 추론해 넣은 구분 문자(공백·줄바꿈).
    pub generated: bool,
    pub invisible: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TextSource {
    /// 보이지 않는 텍스트(OCR 레이어)만.
    #[default]
    InvisibleOnly,
    All,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct LayoutOptions {
    pub source: TextSource,
    /// 소프트 하이픈(U+00AD)과 폭 없는 문자(U+200B 등)를 남긴다. 기본은 제거.
    pub keep_format_chars: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Word {
    /// NFC 정규화된 텍스트.
    pub text: String,
    pub rect: DRect,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    pub words: Vec<Word>,
    pub rect: DRect,
    /// 기준선 y(글자들의 중앙값). 원점 정보가 없으면 None.
    pub baseline: Option<f64>,
    /// 글자 높이 중앙값.
    pub char_height: f64,
    /// 글자 방향(반시계 방향 각도, hOCR `textangle`). 0이면 가로쓰기 그대로.
    pub text_angle: u16,
    /// 글자가 위에서 아래로 쌓이는 세로줄(글자는 바로 서 있다).
    pub vertical: bool,
}

impl Line {
    pub fn text(&self) -> String {
        self.words.iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" ")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LayoutStats {
    /// 높이가 페이지 중앙값보다 지나치게 커서 기준선 기준으로 줄인 글자 수.
    pub clamped_chars: usize,
}

/// 페이지 글자 높이 중앙값의 이 배수를 넘는 글자는 높이를 보정한다. 실제 파일에서 OCR 도구가
/// 폰트 크기를 극단적으로 잡아 loose box 높이가 페이지 전체만 한 글자가 나왔다
/// (`BZR001088_01-mod.pdf` 2쪽 첫 줄, 2026-09-21).
const HEIGHT_OUTLIER_FACTOR: f64 = 4.0;

fn is_format_char(c: char) -> bool {
    matches!(c, '\u{00AD}' | '\u{200B}' | '\u{200C}' | '\u{200D}' | '\u{2060}' | '\u{FEFF}')
}

fn is_line_break(c: char) -> bool {
    matches!(c, '\r' | '\n' | '\u{2028}' | '\u{2029}' | '\u{0085}')
}

enum Piece<'a> {
    Char(&'a InputChar, DRect),
    /// 뺀 서식 문자 — 글자는 남기지 않지만 다음 글자와의 간격 판정 기준 위치가 된다.
    Skip(DRect),
    WordBreak,
    LineBreak,
}

pub fn build_lines(chars: &[InputChar], options: &LayoutOptions) -> (Vec<Line>, LayoutStats) {
    let mut stats = LayoutStats::default();
    let wanted = |c: &InputChar| match options.source {
        TextSource::InvisibleOnly => c.invisible,
        TextSource::All => true,
    };
    let is_glyph = |c: &InputChar| !c.generated && !c.ch.is_whitespace() && !is_line_break(c.ch);

    let mut heights: Vec<f64> = chars.iter().filter(|c| is_glyph(c) && wanted(c)).map(|c| c.rect.height()).collect();
    let median_height = median(&mut heights).unwrap_or(0.0);

    let mut pieces = Vec::with_capacity(chars.len());
    for c in chars {
        if is_line_break(c.ch) {
            pieces.push(Piece::LineBreak);
        } else if c.generated || c.ch.is_whitespace() || c.ch.is_control() {
            pieces.push(Piece::WordBreak);
        } else if !wanted(c) {
            // 고르지 않은 글자는 경계로 남겨, 양옆 단어가 붙지 않게 한다.
            pieces.push(Piece::WordBreak);
        } else if is_format_char(c.ch) && !options.keep_format_chars {
            // 단어 안에서 조용히 뺀다(경계 아님).
            pieces.push(Piece::Skip(c.rect));
        } else {
            let mut rect = c.rect;
            if median_height > 0.0 && rect.height() > median_height * HEIGHT_OUTLIER_FACTOR {
                let base = c.baseline.filter(|b| *b >= rect.y0 && *b <= rect.y1).unwrap_or(rect.y1);
                rect.y0 = base - median_height * 0.8;
                rect.y1 = base + median_height * 0.2;
                stats.clamped_chars += 1;
            }
            pieces.push(Piece::Char(c, rect));
        }
    }

    let mut builder = Builder::default();
    for piece in pieces {
        match piece {
            Piece::LineBreak => builder.end_line(),
            Piece::WordBreak => builder.end_word(),
            Piece::Skip(rect) => {
                if builder.last_rect.is_some() {
                    builder.last_rect = Some(rect);
                }
            }
            Piece::Char(c, rect) => {
                if let Some(prev) = builder.last_rect {
                    let h = prev.height().max(rect.height()).max(1e-6);
                    let w = prev.width().max(rect.width()).max(1e-6);
                    let overlap = prev.vertical_overlap(&rect) / prev.height().min(rect.height()).max(1e-6);
                    let gap = rect.x0 - prev.x1;
                    // 세로쓰기: 가로로 겹치면서 바로 아래로 이어지는 글자.
                    let side_overlap = prev.horizontal_overlap(&rect) / prev.width().min(rect.width()).max(1e-6);
                    let down = rect.y0 - prev.y1;
                    let vertical_run = side_overlap >= 0.5 && down > -h * 0.5 && down < h * 0.8;
                    if vertical_run && (builder.vertical || overlap < 0.3) {
                        builder.vertical = true;
                        builder.end_word(); // 세로줄은 글자마다 단어 하나
                    } else if overlap < 0.3 || gap < -h {
                        builder.end_line();
                    } else if gap > h * 0.5 {
                        builder.end_word();
                    }
                    let _ = w;
                }
                builder.push(c, rect);
            }
        }
    }
    builder.end_line();
    (builder.lines, stats)
}

#[derive(Default)]
struct Builder {
    lines: Vec<Line>,
    words: Vec<Word>,
    word_text: String,
    word_rect: Option<DRect>,
    last_rect: Option<DRect>,
    baselines: Vec<f64>,
    heights: Vec<f64>,
    /// 지금 만들고 있는 줄이 세로줄인지.
    vertical: bool,
}

impl Builder {
    fn push(&mut self, c: &InputChar, rect: DRect) {
        self.word_text.push(c.ch);
        self.word_rect = Some(self.word_rect.map_or(rect, |r| r.union(&rect)));
        self.last_rect = Some(rect);
        if let Some(b) = c.baseline {
            self.baselines.push(b);
        }
        self.heights.push(rect.height());
    }

    fn end_word(&mut self) {
        if let Some(rect) = self.word_rect.take() {
            let text: String = self.word_text.nfc().collect();
            self.words.push(Word { text, rect });
        }
        self.word_text.clear();
    }

    fn end_line(&mut self) {
        self.end_word();
        self.last_rect = None;
        if self.words.is_empty() {
            self.baselines.clear();
            self.heights.clear();
            return;
        }
        let words = std::mem::take(&mut self.words);
        let rect = words.iter().skip(1).fold(words[0].rect, |acc, w| acc.union(&w.rect));
        let baseline = median(&mut self.baselines).map(|b| b.clamp(rect.y0, rect.y1));
        let char_height = median(&mut self.heights).unwrap_or(rect.height());
        self.baselines.clear();
        self.heights.clear();
        self.lines.push(Line { words, rect, baseline, char_height, text_angle: 0, vertical: std::mem::take(&mut self.vertical) });
    }
}

fn median(values: &mut [f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(|a, b| a.total_cmp(b));
    Some(values[values.len() / 2])
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// 한 줄 문자열을 글자당 폭 10, 높이 12로 배치한다(y는 줄 번호 × 20).
    pub(crate) fn line_chars(text: &str, line: usize, invisible: bool) -> Vec<InputChar> {
        text.chars()
            .enumerate()
            .map(|(i, ch)| {
                let x = i as f64 * 10.0;
                let y = line as f64 * 20.0;
                InputChar {
                    ch,
                    rect: DRect { x0: x, y0: y, x1: x + 10.0, y1: y + 12.0 },
                    baseline: Some(y + 10.0),
                    generated: false,
                    invisible,
                }
            })
            .collect()
    }

    fn generated(ch: char) -> InputChar {
        InputChar { ch, rect: DRect { x0: 0.0, y0: 0.0, x1: 0.0, y1: 0.0 }, baseline: None, generated: true, invisible: false }
    }

    fn texts(lines: &[Line]) -> Vec<String> {
        lines.iter().map(Line::text).collect()
    }

    #[test]
    fn spaces_and_generated_breaks() {
        let mut chars = line_chars("ab cd", 0, true);
        chars.push(generated('\r'));
        chars.push(generated('\n'));
        chars.extend(line_chars("ef", 1, true));
        let (lines, _) = build_lines(&chars, &LayoutOptions::default());
        assert_eq!(texts(&lines), vec!["ab cd", "ef"]);
        assert_eq!(lines[0].words[1].rect, DRect { x0: 30.0, y0: 0.0, x1: 50.0, y1: 12.0 });
        assert_eq!(lines[0].baseline, Some(10.0));
    }

    #[test]
    fn geometric_line_break_without_separator() {
        let mut chars = line_chars("ab", 0, true);
        chars.extend(line_chars("cd", 1, true));
        let (lines, _) = build_lines(&chars, &LayoutOptions::default());
        assert_eq!(texts(&lines), vec!["ab", "cd"]);
    }

    #[test]
    fn source_filter_splits_around_visible_text() {
        let mut chars = line_chars("ab", 0, true);
        let mut visible = line_chars("XY", 0, false);
        for c in &mut visible {
            c.rect.x0 += 20.0;
            c.rect.x1 += 20.0;
        }
        chars.extend(visible);
        let mut tail = line_chars("cd", 0, true);
        for c in &mut tail {
            c.rect.x0 += 40.0;
            c.rect.x1 += 40.0;
        }
        chars.extend(tail);
        let (lines, _) = build_lines(&chars, &LayoutOptions::default());
        assert_eq!(texts(&lines), vec!["ab cd"]);
        let all = LayoutOptions { source: TextSource::All, ..Default::default() };
        assert_eq!(texts(&build_lines(&chars, &all).0), vec!["abXYcd"]);
    }

    #[test]
    fn nfc_and_format_chars() {
        // NFD 한글 "한" + 소프트 하이픈
        let chars = line_chars("\u{1112}\u{1161}\u{11AB}\u{00AD}a", 0, true);
        let (lines, _) = build_lines(&chars, &LayoutOptions::default());
        assert_eq!(texts(&lines), vec!["한a"]);
        let keep = LayoutOptions { keep_format_chars: true, ..Default::default() };
        assert_eq!(texts(&build_lines(&chars, &keep).0), vec!["한\u{00AD}a"]);
    }

    #[test]
    fn huge_char_heights_are_clamped_to_baseline() {
        let mut chars = line_chars("abcdefgh", 0, true);
        chars[0].rect.y0 = -3000.0;
        let (lines, stats) = build_lines(&chars, &LayoutOptions::default());
        assert_eq!(stats.clamped_chars, 1);
        assert!(lines[0].rect.y0 > -1.0);
    }

    #[test]
    fn vertical_run_becomes_one_line_of_single_characters() {
        // 같은 x에 글자가 아래로 쌓이는 세로쓰기.
        let chars: Vec<InputChar> = "문화독립"
            .chars()
            .enumerate()
            .map(|(i, ch)| InputChar {
                ch,
                rect: DRect { x0: 100.0, y0: i as f64 * 14.0, x1: 112.0, y1: i as f64 * 14.0 + 12.0 },
                baseline: None,
                generated: false,
                invisible: true,
            })
            .collect();
        let (lines, _) = build_lines(&chars, &LayoutOptions::default());
        assert_eq!(lines.len(), 1, "세로줄은 한 줄로 묶인다");
        assert!(lines[0].vertical);
        assert_eq!(lines[0].text(), "문 화 독 립");
        assert_eq!(lines[0].rect, DRect { x0: 100.0, y0: 0.0, x1: 112.0, y1: 54.0 });
    }

    #[test]
    fn wide_gap_splits_words() {
        let mut chars = line_chars("ab", 0, true);
        let mut far = line_chars("cd", 0, true);
        for c in &mut far {
            c.rect.x0 += 100.0;
            c.rect.x1 += 100.0;
        }
        chars.extend(far);
        assert_eq!(texts(&build_lines(&chars, &LayoutOptions::default()).0), vec!["ab cd"]);
    }
}
