//! hOCR 읽기(설계 문서 6.1).
//!
//! 엔진마다 hOCR이 조금씩 다르고(Tesseract, 다른 엔진, 사람이 고친 파일) XML로 올바르지 않은 파일도
//! 흔해서, 엄격한 XML 파서 대신 관대한 태그 스캐너를 쓴다. 닫는 태그가 어긋나면 맞는 여는 태그까지
//! 거슬러 닫고, 모르는 태그는 무시한다.
//!
//! - `title` 속성은 `;`로 나뉜 `키 값…` 목록이다. 모르는 키는 무시한다.
//! - 필수: `ocr_page`의 `bbox`, 단어의 `bbox`와 텍스트. 선택: `baseline`, `x_size`, `textangle`,
//!   `ppageno`.
//! - 줄: `ocr_line`, `ocr_textfloat`, `ocr_header`, `ocr_caption`. 단어: `ocrx_word`.
//! - 단어 없이 줄만 있으면 줄 전체를 단어 하나로 받는다(줄 단위 배치). 줄 없이 단어만 있으면
//!   단어마다 줄 하나로 둔다.
//! - 텍스트는 읽은 직후 NFC로 바꾼다.

use unicode_normalization::UnicodeNormalization;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BBox {
    pub x0: f64,
    pub y0: f64,
    pub x1: f64,
    pub y1: f64,
}

impl BBox {
    pub fn width(&self) -> f64 {
        self.x1 - self.x0
    }

    pub fn height(&self) -> f64 {
        self.y1 - self.y0
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct HocrWord {
    pub bbox: BBox,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct HocrLine {
    pub bbox: BBox,
    /// `baseline 기울기 오프셋`.
    pub baseline: Option<(f64, f64)>,
    pub x_size: Option<f64>,
    /// 반시계 방향 각도.
    pub textangle: f64,
    pub words: Vec<HocrWord>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct HocrPage {
    /// 페이지 bbox — 대개 `0 0 W H`.
    pub bbox: BBox,
    pub ppageno: Option<usize>,
    pub lines: Vec<HocrLine>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ParseReport {
    /// bbox가 없거나 망가져 버린 단어·줄 수.
    pub dropped_items: usize,
    /// 텍스트가 비어 버린 단어 수(공백뿐인 단어 포함).
    pub empty_words: usize,
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum ParseError {
    #[error("hOCR 페이지(ocr_page)가 없습니다.")]
    NoPages,
    #[error("{0}번째 ocr_page에 bbox가 없습니다.")]
    PageWithoutBBox(usize),
}

// ------------------------------------------------------------------ title 속성

/// `title` 속성에서 키의 값 목록(숫자로 읽을 수 있는 것만).
fn title_numbers(title: &str, key: &str) -> Option<Vec<f64>> {
    for part in title.split(';') {
        let mut tokens = part.split_whitespace();
        if tokens.next() == Some(key) {
            let values: Vec<f64> = tokens.filter_map(|t| t.trim_matches('"').parse().ok()).collect();
            return Some(values);
        }
    }
    None
}

fn title_bbox(title: &str) -> Option<BBox> {
    let v = title_numbers(title, "bbox")?;
    if v.len() < 4 {
        return None;
    }
    let b = BBox { x0: v[0].min(v[2]), y0: v[1].min(v[3]), x1: v[0].max(v[2]), y1: v[1].max(v[3]) };
    (b.x1.is_finite() && b.y1.is_finite()).then_some(b)
}

// ------------------------------------------------------------------ 태그 스캐너

#[derive(Debug)]
enum Token<'a> {
    Open { name: String, attrs: Vec<(String, String)>, self_closing: bool },
    Close { name: String },
    Text(&'a str),
}

fn scan(html: &str) -> Vec<Token<'_>> {
    let mut tokens = Vec::new();
    let bytes = html.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'<' {
            let end = html[i..].find('<').map_or(html.len(), |p| i + p);
            tokens.push(Token::Text(&html[i..end]));
            i = end;
            continue;
        }
        // 주석, DOCTYPE, 처리 지시, CDATA
        if html[i..].starts_with("<!--") {
            i = html[i..].find("-->").map_or(html.len(), |p| i + p + 3);
            continue;
        }
        if html[i..].starts_with("<!") || html[i..].starts_with("<?") {
            i = html[i..].find('>').map_or(html.len(), |p| i + p + 1);
            continue;
        }
        let Some(end) = find_tag_end(html, i) else {
            tokens.push(Token::Text(&html[i..]));
            break;
        };
        let inner = &html[i + 1..end];
        i = end + 1;
        if let Some(name) = inner.strip_prefix('/') {
            tokens.push(Token::Close { name: name.trim().to_ascii_lowercase() });
            continue;
        }
        let self_closing = inner.trim_end().ends_with('/');
        let inner = inner.trim_end().trim_end_matches('/');
        let name_end = inner.find(|c: char| c.is_whitespace()).unwrap_or(inner.len());
        let name = inner[..name_end].to_ascii_lowercase();
        if name.is_empty() {
            continue;
        }
        tokens.push(Token::Open { name, attrs: parse_attrs(&inner[name_end..]), self_closing });
    }
    tokens
}

/// 따옴표 안의 `>`를 건너뛰고 태그 끝 위치.
fn find_tag_end(html: &str, start: usize) -> Option<usize> {
    let mut quote: Option<u8> = None;
    for (offset, &b) in html.as_bytes()[start + 1..].iter().enumerate() {
        match (quote, b) {
            (Some(q), _) if b == q => quote = None,
            (Some(_), _) => {}
            (None, b'"' | b'\'') => quote = Some(b),
            (None, b'>') => return Some(start + 1 + offset),
            _ => {}
        }
    }
    None
}

fn parse_attrs(s: &str) -> Vec<(String, String)> {
    let mut attrs = Vec::new();
    let mut rest = s.trim_start();
    while !rest.is_empty() {
        let name_end = rest.find(|c: char| c == '=' || c.is_whitespace()).unwrap_or(rest.len());
        let name = rest[..name_end].to_ascii_lowercase();
        rest = rest[name_end..].trim_start();
        let value = if let Some(after) = rest.strip_prefix('=') {
            let after = after.trim_start();
            match after.chars().next() {
                Some(q @ ('"' | '\'')) => {
                    let body = &after[1..];
                    let end = body.find(q).unwrap_or(body.len());
                    rest = body.get(end + 1..).unwrap_or("");
                    body[..end].to_string()
                }
                _ => {
                    let end = after.find(char::is_whitespace).unwrap_or(after.len());
                    rest = &after[end..];
                    after[..end].to_string()
                }
            }
        } else {
            String::new()
        };
        if !name.is_empty() {
            attrs.push((name, unescape(&value)));
        }
        rest = rest.trim_start();
    }
    attrs
}

fn unescape(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        rest = &rest[amp..];
        // 엔티티는 짧다 — 앞 12바이트 안에서 ';'를 찾는다(바이트로 찾아야 글자 중간에서 자르지 않는다).
        let Some(semi) = rest.as_bytes().iter().take(12).position(|&b| b == b';') else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let entity = &rest[1..semi];
        let decoded = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some('\u{00A0}'),
            _ => entity
                .strip_prefix("#x")
                .or_else(|| entity.strip_prefix("#X"))
                .and_then(|h| u32::from_str_radix(h, 16).ok())
                .or_else(|| entity.strip_prefix('#').and_then(|d| d.parse().ok()))
                .and_then(char::from_u32),
        };
        match decoded {
            Some(c) => {
                out.push(c);
                rest = &rest[semi + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

// ------------------------------------------------------------------ 구조 조립

#[derive(Debug, Clone, Copy, PartialEq)]
enum Role {
    Page,
    Line,
    Word,
    Other,
}

const LINE_CLASSES: &[&str] = &["ocr_line", "ocr_textfloat", "ocr_header", "ocr_caption"];
/// 내용이 없는 요소 — 닫는 태그가 없어도 스택에 올리지 않는다.
const VOID_ELEMENTS: &[&str] = &["br", "img", "meta", "link", "hr", "input"];

struct Open {
    name: String,
    role: Role,
}

struct PendingLine {
    line: HocrLine,
    /// 단어 요소 밖에 있던 텍스트(단어가 없을 때 줄 텍스트로 쓴다).
    loose_text: String,
}

pub fn parse(html: &str) -> Result<(Vec<HocrPage>, ParseReport), ParseError> {
    let mut report = ParseReport::default();
    let mut pages: Vec<HocrPage> = Vec::new();
    let mut page_index = 0usize;
    let mut stack: Vec<Open> = Vec::new();
    let mut line: Option<PendingLine> = None;
    let mut word: Option<(BBox, String)> = None;
    let mut word_dropped = false;

    let finish_word = |word: &mut Option<(BBox, String)>,
                       line: &mut Option<PendingLine>,
                       pages: &mut Vec<HocrPage>,
                       report: &mut ParseReport| {
        let Some((bbox, text)) = word.take() else { return };
        let text: String = text.split_whitespace().collect::<Vec<_>>().join(" ").nfc().collect();
        if text.is_empty() {
            report.empty_words += 1;
            return;
        }
        let w = HocrWord { bbox, text };
        match line {
            Some(pending) => pending.line.words.push(w),
            // 줄 밖의 단어: 단어 하나짜리 줄
            None => {
                if let Some(page) = pages.last_mut() {
                    page.lines.push(HocrLine { bbox, baseline: None, x_size: None, textangle: 0.0, words: vec![w] });
                }
            }
        }
    };
    let finish_line = |line: &mut Option<PendingLine>, pages: &mut Vec<HocrPage>, report: &mut ParseReport| {
        let Some(mut pending) = line.take() else { return };
        if pending.line.words.is_empty() {
            let text: String = pending.loose_text.split_whitespace().collect::<Vec<_>>().join(" ").nfc().collect();
            if text.is_empty() {
                return;
            }
            pending.line.words.push(HocrWord { bbox: pending.line.bbox, text });
        }
        match pages.last_mut() {
            Some(page) => page.lines.push(pending.line),
            None => report.dropped_items += 1,
        }
    };

    for token in scan(html) {
        match token {
            Token::Open { name, attrs, self_closing } => {
                let attr = |key: &str| attrs.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str());
                let classes: Vec<&str> = attr("class").map(|c| c.split_whitespace().collect()).unwrap_or_default();
                let title = attr("title").unwrap_or("");
                let role = if classes.contains(&"ocr_page") {
                    Role::Page
                } else if classes.iter().any(|c| LINE_CLASSES.contains(c)) {
                    Role::Line
                } else if classes.contains(&"ocrx_word") {
                    Role::Word
                } else {
                    Role::Other
                };
                match role {
                    Role::Page => {
                        finish_word(&mut word, &mut line, &mut pages, &mut report);
                        finish_line(&mut line, &mut pages, &mut report);
                        page_index += 1;
                        let bbox = title_bbox(title).ok_or(ParseError::PageWithoutBBox(page_index))?;
                        let ppageno = title_numbers(title, "ppageno").and_then(|v| v.first().copied()).map(|v| v as usize);
                        pages.push(HocrPage { bbox, ppageno, lines: Vec::new() });
                    }
                    Role::Line => {
                        finish_word(&mut word, &mut line, &mut pages, &mut report);
                        finish_line(&mut line, &mut pages, &mut report);
                        match title_bbox(title) {
                            Some(bbox) => {
                                let baseline = title_numbers(title, "baseline")
                                    .filter(|v| v.len() >= 2)
                                    .map(|v| (v[0], v[1]));
                                let x_size = title_numbers(title, "x_size").and_then(|v| v.first().copied());
                                let textangle =
                                    title_numbers(title, "textangle").and_then(|v| v.first().copied()).unwrap_or(0.0);
                                line = Some(PendingLine {
                                    line: HocrLine { bbox, baseline, x_size, textangle, words: Vec::new() },
                                    loose_text: String::new(),
                                });
                            }
                            None => report.dropped_items += 1,
                        }
                    }
                    Role::Word => {
                        finish_word(&mut word, &mut line, &mut pages, &mut report);
                        match title_bbox(title) {
                            Some(bbox) => {
                                word = Some((bbox, String::new()));
                                word_dropped = false;
                            }
                            None => {
                                report.dropped_items += 1;
                                word_dropped = true;
                            }
                        }
                    }
                    Role::Other => {
                        if name == "br" {
                            if let Some((_, text)) = word.as_mut() {
                                text.push(' ');
                            }
                        }
                    }
                }
                if !self_closing && !VOID_ELEMENTS.contains(&name.as_str()) {
                    stack.push(Open { name, role });
                }
            }
            Token::Close { name } => {
                // 맞는 여는 태그까지 거슬러 닫는다(어긋난 중첩 허용). 없으면 무시.
                let Some(pos) = stack.iter().rposition(|o| o.name == name) else { continue };
                for open in stack.drain(pos..).rev() {
                    match open.role {
                        Role::Word => {
                            finish_word(&mut word, &mut line, &mut pages, &mut report);
                            word_dropped = false;
                        }
                        Role::Line => {
                            finish_word(&mut word, &mut line, &mut pages, &mut report);
                            finish_line(&mut line, &mut pages, &mut report);
                        }
                        Role::Page => {
                            finish_word(&mut word, &mut line, &mut pages, &mut report);
                            finish_line(&mut line, &mut pages, &mut report);
                        }
                        Role::Other => {}
                    }
                }
            }
            Token::Text(text) => {
                let text = unescape(text);
                if let Some((_, w)) = word.as_mut() {
                    w.push_str(&text);
                } else if let Some(pending) = line.as_mut() {
                    if !word_dropped {
                        pending.loose_text.push_str(&text);
                        pending.loose_text.push(' ');
                    }
                }
            }
        }
    }
    finish_word(&mut word, &mut line, &mut pages, &mut report);
    finish_line(&mut line, &mut pages, &mut report);
    if pages.is_empty() {
        return Err(ParseError::NoPages);
    }
    Ok((pages, report))
}

/// 여러 파일(Tesseract의 페이지별 출력 등)의 순서 — 파일 이름 자연 정렬("p2" < "p10").
pub fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    let (mut a, mut b) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (a.peek().copied(), b.peek().copied()) {
            (None, None) => return std::cmp::Ordering::Equal,
            (None, _) => return std::cmp::Ordering::Less,
            (_, None) => return std::cmp::Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let take = |it: &mut std::iter::Peekable<std::str::Chars>| {
                    let mut s = String::new();
                    while let Some(c) = it.peek().copied().filter(char::is_ascii_digit) {
                        s.push(c);
                        it.next();
                    }
                    s
                };
                let (na, nb) = (take(&mut a), take(&mut b));
                let (ta, tb) = (na.trim_start_matches('0'), nb.trim_start_matches('0'));
                let ord = ta.len().cmp(&tb.len()).then_with(|| ta.cmp(tb)).then_with(|| na.len().cmp(&nb.len()));
                if ord != std::cmp::Ordering::Equal {
                    return ord;
                }
            }
            (Some(x), Some(y)) => {
                let ord = x.to_lowercase().cmp(y.to_lowercase());
                if ord != std::cmp::Ordering::Equal {
                    return ord;
                }
                a.next();
                b.next();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TESSERACT_LIKE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE html PUBLIC "-//W3C//DTD XHTML 1.0 Transitional//EN" "x">
<html><head><title></title></head><body>
  <div class='ocr_page' id='page_1' title='image "a.png"; bbox 0 0 2480 3508; ppageno 0; scan_res 300 300'>
   <div class='ocr_carea' title="bbox 10 10 900 200">
    <p class='ocr_par'>
     <span class='ocr_line' id='line_1_1' title="bbox 100 100 800 150; baseline 0.01 -10; x_size 40; x_descenders 10">
      <span class='ocrx_word' title='bbox 100 100 300 150; x_wconf 95'>Hello</span>
      <span class='ocrx_word' title='bbox 320 100 800 150; x_wconf 90'><strong>w&amp;rld</strong></span>
     </span>
     <span class='ocr_header' title="bbox 100 200 400 260; textangle 90">
      <span class='ocrx_word' title='bbox 100 200 400 260'>&#xD55C;&#44544;</span>
     </span>
    </p>
   </div>
  </div>
  <div class='ocr_page' title='bbox 0 0 100 200'></div>
</body></html>"#;

    #[test]
    fn parses_tesseract_structure() {
        let (pages, report) = parse(TESSERACT_LIKE).unwrap();
        assert_eq!(report, ParseReport::default());
        assert_eq!(pages.len(), 2);
        assert_eq!(pages[0].bbox, BBox { x0: 0.0, y0: 0.0, x1: 2480.0, y1: 3508.0 });
        assert_eq!(pages[0].ppageno, Some(0));
        let line = &pages[0].lines[0];
        assert_eq!(line.baseline, Some((0.01, -10.0)));
        assert_eq!(line.x_size, Some(40.0));
        assert_eq!(line.words.iter().map(|w| w.text.as_str()).collect::<Vec<_>>(), vec!["Hello", "w&rld"]);
        assert_eq!(pages[0].lines[1].textangle, 90.0);
        assert_eq!(pages[0].lines[1].words[0].text, "한글");
        assert!(pages[1].lines.is_empty());
    }

    #[test]
    fn line_without_words_and_word_without_line() {
        let html = "<div class=ocr_page title='bbox 0 0 10 10'>\
            <span class=ocr_line title='bbox 1 1 9 3'>줄  단위\n텍스트</span>\
            <span class=ocrx_word title='bbox 1 5 4 7'>외톨이</span></div>";
        let (pages, _) = parse(html).unwrap();
        let texts: Vec<_> = pages[0].lines.iter().map(|l| l.words[0].text.clone()).collect();
        assert_eq!(texts, vec!["줄 단위 텍스트", "외톨이"]);
    }

    #[test]
    fn nfd_input_is_normalized_and_bad_items_counted() {
        let html = "<div class='ocr_page' title='bbox 0 0 10 10'><span class='ocr_line' title='bbox 0 0 10 2'>\
            <span class='ocrx_word' title='bbox 0 0 5 2'>\u{1112}\u{1161}\u{11AB}</span>\
            <span class='ocrx_word' title='x_wconf 3'>bad</span>\
            <span class='ocrx_word' title='bbox 6 0 9 2'>  </span></span></div>";
        let (pages, report) = parse(html).unwrap();
        assert_eq!(pages[0].lines[0].words.len(), 1);
        assert_eq!(pages[0].lines[0].words[0].text, "한");
        assert_eq!((report.dropped_items, report.empty_words), (1, 1));
    }

    #[test]
    fn misnested_tags_and_errors() {
        // </span> 하나 빠짐 — 줄이 page 닫힘에서 끝난다.
        let html = "<div class='ocr_page' title='bbox 0 0 10 10'><span class='ocr_line' title='bbox 0 0 9 2'>\
            <span class='ocrx_word' title='bbox 0 0 4 2'>a</span></div>";
        assert_eq!(parse(html).unwrap().0[0].lines.len(), 1);
        assert_eq!(parse("<html></html>"), Err(ParseError::NoPages));
        assert_eq!(parse("<div class='ocr_page' title='ppageno 0'></div>"), Err(ParseError::PageWithoutBBox(1)));
    }

    #[test]
    fn ampersand_before_multibyte_text() {
        assert_eq!(unescape("&다른 &amp; 글"), "&다른 & 글");
        assert_eq!(unescape("가&#xAC01;&#44032;&bogus"), "가각가&bogus");
    }

    #[test]
    fn natural_order_of_page_files() {
        let mut names = vec!["p10.hocr", "p2.hocr", "p1.hocr", "P3.hocr"];
        names.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(names, vec!["p1.hocr", "p2.hocr", "P3.hocr", "p10.hocr"]);
    }
}
