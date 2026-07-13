//! PDF 자체의 북마크(outline)를 편집해 저장하는 기능. pdfium은 outline 쓰기 API가 없어서
//! (읽기 전용) 이 크레이트는 lopdf로 별도 담당한다.
//!
//! **증분 업데이트(incremental update)만 사용한다.** 문서 전체를 파싱해 새로 재직렬화하는
//! 방식(lopdf의 일반 `Document::save`) 대신, 기존 파일 바이트는 한 글자도 건드리지 않고
//! 바뀐 객체(새 outline 트리)만 파일 끝에 追加하는 방식(`IncrementalDocument`)을 쓴다.
//! Adobe Acrobat이 주석/폼을 저장할 때 쓰는 것과 동일한 메커니즘이며, lopdf가 완전히
//! 이해하지 못하는 객체가 문서 안에 있어도(서명, 특이 압축 등) 우리가 건드리지 않는 한
//! 원본 그대로 보존되므로 손상 위험이 구조적으로 낮다.
//!
//! 호출 측(ui 크레이트)이 지켜야 할 안전 프로토콜: 이 함수는 `out_path`에만 쓰고 `source_pdf`는
//! 절대 건드리지 않는다 — 반드시 임시 파일을 `out_path`로 넘기고, pdfium으로 재오픈해 정상
//! 파싱되는지 검증한 뒤에만 원자적(rename)으로 원본과 교체할 것.

use anyhow::{Context, Result};
use bookmark::BookmarkNode;
use lopdf::{Bookmark as LoBookmark, IncrementalDocument, Object, ObjectId};
use std::collections::BTreeMap;
use std::path::Path;

/// `source_pdf`를 열어 북마크 트리를 통째로 새 outline으로 교체하고, 증분 저장으로
/// `out_path`에 기록한다. `source_pdf` 자체는 절대 수정하지 않는다.
pub fn write_bookmarks_incremental(
    source_pdf: &Path,
    out_path: &Path,
    bookmarks: &[BookmarkNode],
) -> Result<()> {
    let mut incremental = IncrementalDocument::load(source_pdf)
        .with_context(|| format!("lopdf로 PDF 열기 실패: {:?}", source_pdf))?;

    let pages: BTreeMap<u32, ObjectId> = incremental.get_prev_documents().get_pages();
    let catalog_id: ObjectId = incremental
        .get_prev_documents()
        .trailer
        .get(b"Root")
        .and_then(Object::as_reference)
        .context("PDF catalog(/Root) 참조를 찾을 수 없음")?;

    // 새 리비전에서 catalog를 고쳐야 하므로, 먼저 이전 리비전의 catalog 객체를
    // new_document로 복제해 온다(그래야 mutable하게 접근 가능).
    incremental
        .opt_clone_object_to_new_document(catalog_id)
        .context("catalog 객체 복제 실패")?;

    add_nodes(&mut incremental.new_document, bookmarks, None, &pages);
    let outline_id = incremental.new_document.build_outline();

    let catalog = incremental
        .new_document
        .get_object_mut(catalog_id)
        .and_then(Object::as_dict_mut)
        .context("catalog dict 조회 실패")?;
    match outline_id {
        Some(id) => {
            catalog.set("Outlines", id);
        }
        None => {
            // 북마크를 전부 지운 경우 — 기존 /Outlines 참조를 제거해 "북마크 없음" 상태로 만든다.
            catalog.remove(b"Outlines");
        }
    }

    incremental
        .save(out_path)
        .with_context(|| format!("PDF 증분 저장 실패: {:?}", out_path))?;

    Ok(())
}

/// 북마크 트리를 lopdf의 flat add_bookmark 호출들로 변환한다(재귀).
/// 페이지 번호가 실제 문서 페이지 범위를 벗어나면(있을 수 없는 상황이지만 방어적으로)
/// 첫 페이지로 대체한다 — 항목을 통째로 누락시키는 것보다 안전하다.
fn add_nodes(
    doc: &mut lopdf::Document,
    nodes: &[BookmarkNode],
    parent: Option<u32>,
    pages: &BTreeMap<u32, ObjectId>,
) {
    for node in nodes {
        let page_id = pages
            .get(&node.page)
            .or_else(|| pages.values().next())
            .copied()
            .unwrap_or((0, 0));

        let lo_bookmark = LoBookmark::new(node.title.clone(), [0.0, 0.0, 0.0], 0, page_id);
        let id = doc.add_bookmark(lo_bookmark, parent);
        add_nodes(doc, &node.children, Some(id), pages);
    }
}
