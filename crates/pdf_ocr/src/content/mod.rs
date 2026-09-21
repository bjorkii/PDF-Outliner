//! 콘텐츠 스트림 읽기(토크나이저·해석기).

pub mod interp;
pub mod lexer;

use anyhow::{Context, Result};
use lopdf::{Document, Object, ObjectId};

/// 페이지 `/Contents`의 스트림 객체 id 목록(단일 스트림이면 하나).
pub fn page_content_ids(doc: &Document, page_id: ObjectId) -> Result<Vec<ObjectId>> {
    let page = doc.get_dictionary(page_id).context("페이지 딕셔너리 조회 실패")?;
    let Ok(contents) = page.get(b"Contents") else {
        return Ok(Vec::new()); // 빈 페이지
    };
    let ids = match contents {
        Object::Reference(id) => match doc.get_object(*id).context("/Contents 참조 해석 실패")? {
            // 간접 배열(드묾)
            Object::Array(items) => items.iter().filter_map(|o| o.as_reference().ok()).collect(),
            _ => vec![*id],
        },
        Object::Array(items) => items.iter().filter_map(|o| o.as_reference().ok()).collect(),
        _ => anyhow::bail!("/Contents 형식이 올바르지 않음"),
    };
    Ok(ids)
}

/// 페이지 콘텐츠를 풀어 이어 붙인 바이트. 배열이면 스트림 사이에 개행을 넣는다 — pdfium도
/// 스트림을 이어 붙일 때 구분 공백을 넣으므로, 토큰 중간에서 잘린 파일도 화면에 보이는 것과
/// 같은 방식으로 해석된다.
pub fn page_content_bytes(doc: &Document, page_id: ObjectId) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    for (i, id) in page_content_ids(doc, page_id)?.into_iter().enumerate() {
        if i > 0 {
            out.push(b'\n');
        }
        out.extend_from_slice(&stream_bytes(doc, id)?);
    }
    Ok(out)
}

/// 스트림 객체를 필터를 풀어 읽는다(필터가 없으면 원본 그대로).
pub fn stream_bytes(doc: &Document, id: ObjectId) -> Result<Vec<u8>> {
    let stream = doc
        .get_object(id)
        .and_then(Object::as_stream)
        .with_context(|| format!("스트림 객체 {id:?} 조회 실패"))?;
    if stream.dict.get(b"Filter").is_err() {
        return Ok(stream.content.clone());
    }
    stream
        .decompressed_content()
        .with_context(|| format!("스트림 {id:?} 압축 해제 실패"))
}
