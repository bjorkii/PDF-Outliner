//! 문서 전체 텍스트 검색. pdfium 자체 텍스트 검색(`FPDFText_FindStart`류)을 페이지마다
//! 돌려서 문서 전체의 일치 항목을 모은다 — pdfium은 한 페이지 단위로만 검색을 지원하므로
//! (`PdfPageText::search`), 문서 전체 검색은 이 크레이트가 그 위에 조립해야 한다.
//!
//! 하이라이트용 bounding box는 문자 인덱스에서 우리가 직접 계산하지 않고, pdfium이 이미
//! 제공하는 `PdfPageTextSegment::bounds()`(줄바꿈/폰트 경계에 맞춰 병합된 사각형)를 그대로
//! 쓴다 — `crate::selection`의 문자별 quad 방식과 달리 검색 결과는 스큐/세로쓰기 보정이
//! 필요 없는 일반 하이라이트라 이 편이 더 간단하고 정확하다.
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

/// 문서 내 한 번의 검색 일치 — 한 페이지 안에서 검색어가 걸린 자리(줄바꿈을 걸치면 여러
/// 사각형으로 나뉠 수 있음).
#[derive(Debug, Clone)]
pub struct SearchMatch {
    /// 1-based 페이지 번호.
    pub page: u32,
    /// 페이지 좌표계(PdfPoints) 기준 하이라이트 사각형들.
    pub rects: Vec<PdfRect>,
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

    for segments in search.iter(PdfSearchDirection::SearchForward) {
        let rects: Vec<PdfRect> = segments.iter().map(|segment| segment.bounds()).collect();
        if !rects.is_empty() {
            matches.push(SearchMatch {
                page: page_number,
                rects,
            });
        }
    }

    matches
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

    /// 지금까지(또는 완료 시 전체) 찾은 결과를 소비한다.
    pub fn into_matches(self) -> Vec<SearchMatch> {
        self.matches
    }
}
