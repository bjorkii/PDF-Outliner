//! 문서 전체 텍스트 검색. pdfium 자체 텍스트 검색(`FPDFText_FindStart`류)을 페이지마다
//! 돌려서 문서 전체의 일치 항목을 모은다 — pdfium은 한 페이지 단위로만 검색을 지원하므로
//! (`PdfPageText::search`), 문서 전체 검색은 이 크레이트가 그 위에 조립해야 한다.
//!
//! 하이라이트용 bounding box는 문자 인덱스에서 우리가 직접 계산하지 않고, pdfium이 이미
//! 제공하는 `PdfPageTextSegment::bounds()`(줄바꿈/폰트 경계에 맞춰 병합된 사각형)를 그대로
//! 쓴다 — `crate::selection`의 문자별 quad 방식과 달리 검색 결과는 스큐/세로쓰기 보정이
//! 필요 없는 일반 하이라이트라 이 편이 더 간단하고 정확하다.
//!
//! 결과 목록(ui 검색 사이드바)에 보여줄 앞뒤 문맥도 검색 시점에 함께 뽑아 둔다(`SearchMatch`
//! 의 context 필드) — 페이지 텍스트 객체가 살아 있는 이때가 가장 싸다.
//!
//! **PDFium은 스레드 안전하지 않다.** pdfium-render의 README는 `thread_safe` feature가
//! "뮤텍스로 Pdfium 접근을 감싼다"고 설명하지만, 실제 0.9.2 소스(`pdfium.rs`,
//! `bindings/dynamic_bindings.rs`)를 확인해보면 실제 FFI 호출을 감싸는 뮤텍스는 어디에도
//! 없고, 그냥 내부 `OnceCell` 초기화 대기 방식만 다를 뿐이다(2026-07-13, 실제로 검색을
//! 백그라운드 스레드에서 돌렸다가 검색 버튼을 누르는 즉시 세그폴트로 재현·확인함). 따라서
//! **이 크레이트를 쓰는 모든 pdfium 호출은 항상 같은 스레드(UI 메인 스레드)에서만 실행해야
//! 한다** — 이 모듈이 `IncrementalSearch`(한 프레임에 정해진 페이지 수만큼만 진행)를 제공하는
//! 이유가 바로 이것이다: 스레드를 늘리지 않고도 한 번에 문서 전체를 훑는 부담을 여러
//! 프레임에 걸쳐 나눠, UI를 막지 않으면서 스레드 경계도 넘지 않는다.

use pdfium_render::prelude::*;

/// 결과 목록에 보여줄 일치 문자열 앞뒤 문맥 길이(문자 수).
const CONTEXT_CHARS: usize = 24;
/// 결과 사각형으로 되짚은 문자 범위를 앞뒤로 이만큼 넓혀 검색어 위치를 다시 찾는다.
const MATCH_SLACK: usize = 4;

/// 문서 내 한 번의 검색 일치 — 한 페이지 안에서 검색어가 걸린 자리(줄바꿈을 걸치면 여러
/// 사각형으로 나뉠 수 있음).
#[derive(Debug, Clone)]
pub struct SearchMatch {
    /// 1-based 페이지 번호.
    pub page: u32,
    /// 페이지 좌표계(PdfPoints) 기준 하이라이트 사각형들.
    pub rects: Vec<PdfRect>,
    /// 일치 문자열 바로 앞 문맥(최대 `CONTEXT_CHARS`자). 줄바꿈·탭 등은 공백 한 칸으로.
    pub context_before: String,
    /// 페이지에 실제로 적힌 일치 문자열 — 대소문자 무시 검색이라 검색어와 표기가 다를 수 있다.
    pub matched_text: String,
    /// 일치 문자열 바로 뒤 문맥(최대 `CONTEXT_CHARS`자).
    pub context_after: String,
}

/// 한 페이지 안에서 `query`를 찾아 그 페이지의 일치 항목들을 반환한다(내부 헬퍼).
/// 텍스트 레이어가 없거나(이미지 전용 스캔) 파싱에 실패하면 빈 벡터 — 문서 전체 검색이
/// 페이지 하나 때문에 전부 실패하면 안 되므로 조용히 건너뛴다.
fn search_page(page: &PdfPage, page_number: u32, query: &str, options: &PdfSearchOptions) -> Vec<SearchMatch> {
    let mut matches = Vec::new();

    let Ok(text_page) = page.text() else {
        return matches;
    };
    let Ok(search) = text_page.search(query, options) else {
        return matches;
    };
    let chars = text_page.chars();
    let char_count = chars.len();

    for segments in search.iter(PdfSearchDirection::SearchForward) {
        let rects: Vec<PdfRect> = segments.iter().map(|segment| segment.bounds()).collect();
        if rects.is_empty() {
            continue;
        }

        let first = segments
            .iter()
            .next()
            .and_then(|segment| segment.chars().ok())
            .and_then(|segment_chars| segment_chars.first_char_index());
        let last = segments
            .iter()
            .last()
            .and_then(|segment| segment.chars().ok())
            .and_then(|segment_chars| segment_chars.last_char_index());

        let (context_before, matched_text, context_after) = match (first, last) {
            (Some(first), Some(last)) if first <= last => {
                // 결과 사각형 안의 문자를 되짚은 범위라 경계에 이웃 글자가 섞일 수 있다 —
                // 조금 넓힌 창에서 검색어 위치를 다시 찾아 정확한 범위로 좁힌다.
                let window_start = first.saturating_sub(MATCH_SLACK);
                let window_end = (last + 1 + MATCH_SLACK).min(char_count);
                let pieces: Vec<String> =
                    (window_start..window_end).map(|index| char_string(&chars, index)).collect();
                let (start, end) = locate_query(&pieces, query).map_or((first, last + 1), |(s, e)| {
                    (window_start + s, window_start + e)
                });
                (
                    range_text(&chars, start.saturating_sub(CONTEXT_CHARS), start),
                    range_text(&chars, start, end),
                    range_text(&chars, end, (end + CONTEXT_CHARS).min(char_count)),
                )
            }
            _ => (
                String::new(),
                segments.iter().map(|segment| segment.text()).collect(),
                String::new(),
            ),
        };

        matches.push(SearchMatch {
            page: page_number,
            rects,
            context_before: one_line(&context_before),
            matched_text: one_line(&matched_text),
            context_after: one_line(&context_after),
        });
    }

    matches
}

fn char_string(chars: &PdfPageTextChars, index: PdfPageTextCharIndex) -> String {
    chars
        .get(index)
        .ok()
        .and_then(|ch| ch.unicode_string())
        .unwrap_or_default()
}

fn range_text(chars: &PdfPageTextChars, start: PdfPageTextCharIndex, end: PdfPageTextCharIndex) -> String {
    (start..end).map(|index| char_string(chars, index)).collect()
}

/// 목록 한 줄에 들어가게 줄바꿈·탭 등 공백류와 제어 문자를 공백 한 칸으로 합친다.
fn one_line(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut previous_space = false;
    for ch in text.chars() {
        if ch.is_whitespace() || ch.is_control() {
            if !previous_space {
                out.push(' ');
            }
            previous_space = true;
        } else {
            out.push(ch);
            previous_space = false;
        }
    }
    out
}

/// 문자 조각들(문자 인덱스 순) 안에서 `query`가 시작·끝나는 조각 위치 `[start, end)`를 찾는다.
/// 대소문자와 공백은 무시한다(pdfium 기본 검색 옵션, 그리고 줄바꿈 자리에 끼는 생성 공백).
fn locate_query(pieces: &[String], query: &str) -> Option<(usize, usize)> {
    let needle: Vec<char> = query
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect();
    if needle.is_empty() {
        return None;
    }
    'starts: for start in 0..pieces.len() {
        if pieces[start].trim().is_empty() {
            continue;
        }
        let mut matched = 0;
        for (offset, piece) in pieces[start..].iter().enumerate() {
            for ch in piece
                .chars()
                .filter(|ch| !ch.is_whitespace())
                .flat_map(char::to_lowercase)
            {
                if matched == needle.len() {
                    break;
                }
                if ch != needle[matched] {
                    continue 'starts;
                }
                matched += 1;
            }
            if matched == needle.len() {
                return Some((start, start + offset + 1));
            }
        }
    }
    None
}

/// 문서의 모든 페이지에서 `query`를 순서대로(페이지 순, 페이지 내에서는 읽기 순서) 찾는다.
/// 대소문자 구분 없음(기본 `PdfSearchOptions`) — 검색창에서 흔히 기대하는 동작.
///
/// 페이지 수가 많으면 한 번에 오래 걸릴 수 있다 — UI에서는 이 함수를 그대로 부르지 말고
/// 아래 `IncrementalSearch`로 여러 프레임에 나눠 실행할 것. 이 함수는 headless 검증/테스트용.
pub fn search_document(document: &PdfDocument, query: &str) -> Vec<SearchMatch> {
    if query.trim().is_empty() {
        return Vec::new();
    }

    let options = PdfSearchOptions::new();
    let mut matches = Vec::new();

    for (page_index, page) in document.pages().iter().enumerate() {
        matches.extend(search_page(&page, page_index as u32 + 1, query, &options));
    }

    matches
}

/// 문서 전체 검색을 여러 프레임에 걸쳐 나눠 실행하기 위한 상태. `step()`을 매 프레임 호출해
/// 정해진 페이지 수만큼만 진행시킨다 — **반드시 매번 같은 스레드(UI 메인 스레드)에서
/// 호출할 것**(모듈 문서 참고, PDFium은 스레드 안전하지 않음).
pub struct IncrementalSearch {
    query: String,
    options: PdfSearchOptions,
    next_page_index: usize,
    total_pages: usize,
    matches: Vec<SearchMatch>,
}

impl IncrementalSearch {
    /// 새 검색을 시작한다. `total_pages`는 검색 대상 문서의 전체 페이지 수.
    pub fn new(query: String, total_pages: u32) -> Self {
        Self {
            query,
            options: PdfSearchOptions::new(),
            next_page_index: 0,
            total_pages: total_pages as usize,
            matches: Vec::new(),
        }
    }

    /// 최대 `batch_size`페이지만큼 검색을 진행한다. 이번 호출로 검색이 끝까지 완료됐으면
    /// `true`를 반환한다(그 뒤엔 `into_matches()`로 결과를 가져갈 것).
    pub fn step(&mut self, document: &PdfDocument, batch_size: usize) -> bool {
        let end = (self.next_page_index + batch_size).min(self.total_pages);

        for page_index in self.next_page_index..end {
            if let Ok(page) = document.pages().get(page_index as PdfPageIndex) {
                self.matches
                    .extend(search_page(&page, page_index as u32 + 1, &self.query, &self.options));
            }
        }

        self.next_page_index = end;
        self.is_finished()
    }

    pub fn is_finished(&self) -> bool {
        self.next_page_index >= self.total_pages
    }

    /// (검색을 마친 페이지 수, 전체 페이지 수).
    pub fn progress(&self) -> (usize, usize) {
        (self.next_page_index, self.total_pages)
    }

    /// 지난 호출 이후 새로 찾은 결과를 가져간다 — 검색이 끝나기 전에도 결과 목록에 찾은
    /// 만큼 흘려 보여주기 위해. 페이지 순서는 유지된다.
    pub fn take_new_matches(&mut self) -> Vec<SearchMatch> {
        std::mem::take(&mut self.matches)
    }

    /// 지금까지(또는 완료 시 전체) 찾은 결과 중 아직 가져가지 않은 것을 소비한다.
    pub fn into_matches(self) -> Vec<SearchMatch> {
        self.matches
    }
}

#[cfg(test)]
mod tests {
    use super::{locate_query, one_line};

    fn pieces(text: &[&str]) -> Vec<String> {
        text.iter().map(|piece| piece.to_string()).collect()
    }

    #[test]
    fn one_line_collapses_line_breaks_and_tabs() {
        assert_eq!(one_line("앞 줄\r\n\t다음  줄"), "앞 줄 다음 줄");
    }

    /// 되짚은 범위에 이웃 글자가 섞여도 검색어 자리만 정확히 찾는다(대소문자 무시).
    #[test]
    fn locate_query_narrows_to_the_match() {
        let window = pieces(&["x", "H", "e", "l", "l", "o", "!"]);
        assert_eq!(locate_query(&window, "hello"), Some((1, 6)));
    }

    /// 줄바꿈 자리에 끼는 생성 공백과 검색어 안의 공백은 무시한다.
    #[test]
    fn locate_query_ignores_whitespace() {
        let window = pieces(&[" ", "의", "료", "\r\n", "지", "원"]);
        assert_eq!(locate_query(&window, "의료 지원"), Some((1, 6)));
    }

    #[test]
    fn locate_query_reports_absence() {
        assert_eq!(locate_query(&pieces(&["a", "b"]), "zz"), None);
        assert_eq!(locate_query(&pieces(&["a"]), "  "), None);
    }
}
