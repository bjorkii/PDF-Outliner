//! PDF 자체에 내장된 북마크(목차/outline)를 읽어 `bookmark::BookmarkNode` 트리로 변환한다.
//! 문서를 열 때 사이드바 초기 상태를 채우는 데 쓰인다.
//!
//! 쓰기는 여기서 다루지 않는다 — pdfium 자체가 outline 쓰기 API(FPDFBookmark_Add류)를
//! 제공하지 않는다(읽기 전용). 북마크를 PDF에 저장하는 기능은 `pdf_outline_writer` 크레이트가
//! lopdf로 별도 담당한다.

use bookmark::BookmarkNode;
use pdfium_render::prelude::*;

/// 문서의 최상위 북마크부터 재귀적으로 순회해 트리를 만든다. 북마크가 전혀 없으면 빈 벡터.
///
/// destination이 없거나 페이지 인덱스를 못 구하는 항목(외부 링크 북마크 등)은 1페이지로
/// 대체한다 — 원본에 있던 북마크 항목 자체가 사이드바에서 조용히 사라지는 것보다는, 페이지
/// 번호가 부정확하더라도 항목이 보이는 편이 사용자가 알아차리고 고치기 쉽다.
pub fn read_bookmarks(document: &PdfDocument) -> Vec<BookmarkNode> {
    siblings_to_nodes(document.bookmarks().root())
}

fn siblings_to_nodes(first: Option<PdfBookmark>) -> Vec<BookmarkNode> {
    let mut nodes = Vec::new();
    let mut current = first;

    while let Some(bookmark) = current {
        let title = bookmark.title().unwrap_or_default();
        let page = bookmark
            .destination()
            .and_then(|dest| dest.page_index().ok())
            .map(|idx| (idx + 1) as u32) // pdfium은 0-based, 우리 모델은 1-based
            .unwrap_or(1);

        let mut node = BookmarkNode::new(title, page);
        node.children = siblings_to_nodes(bookmark.first_child());

        current = bookmark.next_sibling();
        nodes.push(node);
    }

    nodes
}
