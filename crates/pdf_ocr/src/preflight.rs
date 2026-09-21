//! 작업 전 파일 상태 점검(설계 문서 2.4). 판정만 하고 처리 방침(거부·경고·확인)은 호출 측이
//! 정한다. pdfium과 lopdf의 페이지 수 비교는 pdfium이 필요해서 ui 쪽 작업 프로세스가 한다.

use crate::geometry::resolve;
use lopdf::{Dictionary, Document, Object};

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Preflight {
    pub page_count: usize,
    /// 트레일러에 `/Encrypt`가 있었음 — 1차 버전은 거부한다.
    pub encrypted: bool,
    /// 값이 채워진 서명 필드가 있음 — 전체 재작성하면 서명이 무효가 된다.
    pub signed: bool,
    /// XMP의 PDF/A 선언(예: "1B", "2U").
    pub pdfa: Option<String>,
    /// `/MarkInfo /Marked true` 또는 `/StructTreeRoot`.
    pub tagged: bool,
    /// 원본에 쌓인 증분 업데이트 수(추정) — 전체 재작성으로 옛 리비전이 사라진다고 알릴 때 쓴다.
    pub incremental_updates: usize,
    pub linearized: bool,
}

impl Preflight {
    /// `raw`는 파일 원본 바이트(증분 업데이트·linearization 추정용).
    pub fn inspect(doc: &Document, raw: &[u8]) -> Self {
        let catalog = doc.catalog().ok();
        let linearized = is_linearized(raw);
        let eof_markers = count_occurrences(raw, b"%%EOF");
        Self {
            page_count: doc.get_pages().len(),
            encrypted: doc.trailer.has(b"Encrypt") || doc.was_encrypted(),
            signed: catalog.is_some_and(|c| has_signature(doc, c)),
            pdfa: catalog.and_then(|c| pdfa_declaration(doc, c)),
            tagged: catalog.is_some_and(|c| is_tagged(doc, c)),
            // linearized 파일은 리비전이 하나여도 %%EOF가 두 번 나온다.
            incremental_updates: eof_markers.saturating_sub(1 + linearized as usize),
            linearized,
        }
    }
}

fn count_occurrences(haystack: &[u8], needle: &[u8]) -> usize {
    haystack.windows(needle.len()).filter(|w| *w == needle).count()
}

fn is_linearized(raw: &[u8]) -> bool {
    // 선형화 딕셔너리는 파일 맨 앞 첫 객체여야 한다(규격 부록 F).
    let head = &raw[..raw.len().min(1024)];
    count_occurrences(head, b"/Linearized") > 0
}

fn is_tagged(doc: &Document, catalog: &Dictionary) -> bool {
    let marked = catalog
        .get(b"MarkInfo")
        .ok()
        .and_then(|o| resolve(doc, o).as_dict().ok())
        .and_then(|m| m.get(b"Marked").ok())
        .and_then(|o| resolve(doc, o).as_bool().ok())
        .unwrap_or(false);
    marked || catalog.has(b"StructTreeRoot")
}

fn has_signature(doc: &Document, catalog: &Dictionary) -> bool {
    let Some(acroform) = catalog.get(b"AcroForm").ok().and_then(|o| resolve(doc, o).as_dict().ok()) else {
        return false;
    };
    // SigFlags 비트 1(SignaturesExist)
    let flags = acroform
        .get(b"SigFlags")
        .ok()
        .and_then(|o| resolve(doc, o).as_i64().ok())
        .unwrap_or(0);
    if flags & 1 != 0 {
        return true;
    }
    let Some(fields) = acroform.get(b"Fields").ok().and_then(|o| resolve(doc, o).as_array().ok()) else {
        return false;
    };
    let mut stack: Vec<&Object> = fields.iter().collect();
    let mut visited = 0;
    while let Some(field) = stack.pop() {
        visited += 1;
        if visited > 10_000 {
            break; // 순환 참조 방어
        }
        let Ok(field) = resolve(doc, field).as_dict() else { continue };
        let is_sig = field.get(b"FT").ok().and_then(|o| o.as_name().ok()) == Some(b"Sig");
        if is_sig && field.has(b"V") {
            return true;
        }
        if let Some(kids) = field.get(b"Kids").ok().and_then(|o| resolve(doc, o).as_array().ok()) {
            stack.extend(kids.iter());
        }
    }
    false
}

fn pdfa_declaration(doc: &Document, catalog: &Dictionary) -> Option<String> {
    let id = catalog.get(b"Metadata").ok()?.as_reference().ok()?;
    let xmp = crate::content::stream_bytes(doc, id).ok()?;
    let xmp = String::from_utf8_lossy(&xmp);
    let part = xmp_value(&xmp, "pdfaid:part")?;
    let conformance = xmp_value(&xmp, "pdfaid:conformance").unwrap_or_default();
    Some(format!("{part}{}", conformance.to_uppercase()))
}

/// XMP에서 `key="값"`(속성) 또는 `<key>값</key>`(요소) 형태의 값을 찾는다.
fn xmp_value(xmp: &str, key: &str) -> Option<String> {
    for quote in ['"', '\''] {
        let pattern = format!("{key}={quote}");
        if let Some(start) = xmp.find(&pattern) {
            let rest = &xmp[start + pattern.len()..];
            return rest.find(quote).map(|end| rest[..end].trim().to_string());
        }
    }
    let open = format!("<{key}>");
    let start = xmp.find(&open)? + open.len();
    let end = xmp[start..].find('<')?;
    Some(xmp[start..start + end].trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use lopdf::{dictionary, Stream};

    fn doc_with_catalog(extra: Dictionary) -> Document {
        let mut doc = Document::with_version("1.7");
        let pages = doc.add_object(dictionary! { "Type" => "Pages", "Kids" => vec![], "Count" => 0 });
        let mut catalog = dictionary! { "Type" => "Catalog", "Pages" => pages };
        for (k, v) in extra.into_iter() {
            catalog.set(k, v);
        }
        let catalog = doc.add_object(catalog);
        doc.trailer.set("Root", catalog);
        doc
    }

    #[test]
    fn plain_document() {
        let doc = doc_with_catalog(Dictionary::new());
        let p = Preflight::inspect(&doc, b"%PDF-1.7\n...%%EOF\n");
        assert_eq!(p, Preflight::default());
    }

    #[test]
    fn detects_signature_tags_and_updates() {
        let mut doc = doc_with_catalog(dictionary! { "MarkInfo" => dictionary! { "Marked" => true } });
        let sig = doc.add_object(dictionary! { "FT" => "Sig", "V" => dictionary! {} });
        let parent = doc.add_object(dictionary! { "Kids" => vec![sig.into()] });
        let root = doc.trailer.get(b"Root").unwrap().as_reference().unwrap();
        doc.get_dictionary_mut(root).unwrap().set("AcroForm", dictionary! { "Fields" => vec![parent.into()] });
        let p = Preflight::inspect(&doc, b"%%EOF %%EOF %%EOF");
        assert!(p.signed && p.tagged);
        assert_eq!(p.incremental_updates, 2);
    }

    #[test]
    fn detects_pdfa_in_attribute_and_element_forms() {
        for xmp in [
            &b"<rdf:Description pdfaid:part=\"2\" pdfaid:conformance=\"u\"/>"[..],
            b"<pdfaid:part>1</pdfaid:part><pdfaid:conformance>B</pdfaid:conformance>",
        ] {
            let mut doc = doc_with_catalog(Dictionary::new());
            let meta = doc.add_object(Stream::new(dictionary! { "Type" => "Metadata" }, xmp.to_vec()));
            let root = doc.trailer.get(b"Root").unwrap().as_reference().unwrap();
            doc.get_dictionary_mut(root).unwrap().set("Metadata", meta);
            let p = Preflight::inspect(&doc, b"");
            assert!(matches!(p.pdfa.as_deref(), Some("2U") | Some("1B")));
        }
    }
}
