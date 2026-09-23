//! 전체 재작성 저장(설계 문서 1.1, 2.5).
//!
//! 증분 저장은 원본 바이트를 그대로 두고 변경분만 덧붙이므로, 지운 OCR 텍스트가 옛 리비전에
//! 그대로 남는다(뷰어엔 안 보여도 파싱하면 복원된다). 그래서 OCR 작업은 문서 전체를 새로 쓰고
//! 쓰이지 않는 객체를 버린다 — 입력에 쌓여 있던 옛 리비전(형태 O)도 함께 사라진다.
//!
//! - 트레일러는 `Root`, `Info`, `ID`만 남긴다. 읽을 때 쓴 xref 스트림용 키(`W`, `Index` 등)나
//!   선형화 흔적이 남지 않게 하기 위해서다. 선형화는 풀린다.
//! - 원본이 객체 스트림(xref 스트림)을 썼으면 같은 방식으로 저장해 파일이 불어나지 않게 한다
//!   ([`uses_object_streams`]). PDF/A-1은 둘 다 금지라 호출 측이 끄고, 그 밖엔 표 형식으로 쓴다.
//! - Info `/ModDate`와 XMP `xmp:ModifyDate`·`xmp:MetadataDate`를 같은 시각으로 맞춘다(불일치는
//!   대표적인 PDF/A 위반). XMP에 해당 항목이 없으면 새로 넣지 않는다.

use anyhow::{Context, Result};
use chrono::{DateTime, FixedOffset};
use lopdf::xref::XrefType;
use lopdf::{Dictionary, Document, Object, StringFormat};
use std::io::{BufWriter, Write};
use std::path::Path;

/// 문서를 읽을 때 xref 스트림이었는지(객체 스트림을 썼을 가능성이 큼). 저장하면 표 형식으로
/// 바뀌므로 첫 저장 전에 확인해 둘 것.
pub fn uses_object_streams(doc: &Document) -> bool {
    matches!(doc.reference_table.cross_reference_type, XrefType::CrossReferenceStream)
}

/// 쓰이지 않는 객체를 문서에서 빼서 돌려준다. 검증에 실패한 페이지를 되돌릴 때 원본 콘텐츠
/// 스트림이 필요하므로, 저장 전에 이걸로 빼 두었다가 [`restore`]로 되살린다(저장 때 그냥 지워
/// 버리면 되돌린 페이지가 빈 페이지가 된다 — 실제로 겪음, 2026-09-23).
pub fn take_unreferenced(doc: &mut Document) -> std::collections::BTreeMap<lopdf::ObjectId, Object> {
    let referenced: std::collections::HashSet<lopdf::ObjectId> = doc.traverse_objects(|_| {}).into_iter().collect();
    let unreferenced: Vec<lopdf::ObjectId> = doc.objects.keys().copied().filter(|id| !referenced.contains(id)).collect();
    unreferenced.into_iter().filter_map(|id| doc.objects.remove(&id).map(|object| (id, object))).collect()
}

/// [`take_unreferenced`]로 뺀 객체를 되돌린다.
pub fn restore(doc: &mut Document, objects: std::collections::BTreeMap<lopdf::ObjectId, Object>) {
    for (id, object) in objects {
        doc.objects.entry(id).or_insert(object);
    }
}

/// `compact`면 객체 스트림 + xref 스트림으로, 아니면 표 형식으로 쓴다.
pub fn save_rewritten(doc: &mut Document, path: &Path, now: DateTime<FixedOffset>, compact: bool) -> Result<()> {
    let mut trailer = Dictionary::new();
    for key in [&b"Root"[..], b"Info", b"ID"] {
        if let Ok(value) = doc.trailer.get(key) {
            trailer.set(key.to_vec(), value.clone());
        }
    }
    doc.trailer = trailer;
    doc.reference_table.cross_reference_type = XrefType::CrossReferenceTable;

    update_modification_dates(doc, now);
    doc.prune_objects();
    // /Size는 max_id로 쓰므로, 정리 뒤 실제 최대 번호에 맞춘다(안 하면 qpdf가 경고).
    doc.max_id = doc.objects.keys().map(|(id, _)| *id).max().unwrap_or(0);

    let file = std::fs::File::create(path).with_context(|| format!("파일을 만들 수 없음: {}", path.display()))?;
    let mut writer = BufWriter::with_capacity(1 << 20, file);
    if compact {
        doc.save_modern(&mut writer).context("PDF 쓰기 실패")?;
    } else {
        doc.save_to(&mut writer).context("PDF 쓰기 실패")?;
    }
    writer.flush()?;
    writer.into_inner().map_err(|e| e.into_error())?.sync_all()?;
    Ok(())
}

fn update_modification_dates(doc: &mut Document, now: DateTime<FixedOffset>) {
    let pdf_date = format_pdf_date(now);
    let info_id = doc.trailer.get(b"Info").ok().and_then(|o| o.as_reference().ok());
    if let Some(id) = info_id {
        if let Ok(info) = doc.get_dictionary_mut(id) {
            info.set("ModDate", Object::String(pdf_date.into_bytes(), StringFormat::Literal));
        }
    } else if let Ok(Object::Dictionary(info)) = doc.trailer.get_mut(b"Info") {
        info.set("ModDate", Object::String(pdf_date.into_bytes(), StringFormat::Literal));
    }

    let xmp_date = now.to_rfc3339_opts(chrono::SecondsFormat::Secs, false);
    let metadata_id = doc
        .catalog()
        .ok()
        .and_then(|c| c.get(b"Metadata").ok())
        .and_then(|o| o.as_reference().ok());
    let Some(id) = metadata_id else { return };
    let Ok(bytes) = crate::content::stream_bytes(doc, id) else { return };
    let Ok(xmp) = String::from_utf8(bytes) else { return };
    let mut updated = xmp.clone();
    for key in ["xmp:ModifyDate", "xmp:MetadataDate"] {
        updated = replace_xmp_value(&updated, key, &xmp_date);
    }
    if updated != xmp {
        if let Ok(stream) = doc.get_object_mut(id).and_then(Object::as_stream_mut) {
            // PDF/A는 메타데이터 스트림을 압축하지 않는 것을 권하므로 평문으로 둔다.
            stream.set_plain_content(updated.into_bytes());
        }
    }
}

/// `D:YYYYMMDDHHmmSS+HH'mm'`
pub fn format_pdf_date(now: DateTime<FixedOffset>) -> String {
    let offset = now.offset().local_minus_utc();
    let sign = if offset < 0 { '-' } else { '+' };
    let offset = offset.abs();
    format!("D:{}{sign}{:02}'{:02}'", now.format("%Y%m%d%H%M%S"), offset / 3600, offset % 3600 / 60)
}

/// `key="값"`, `key='값'`, `<key>값</key>` 형태의 값을 모두 바꾼다.
fn replace_xmp_value(xmp: &str, key: &str, value: &str) -> String {
    let mut out = xmp.to_string();
    for quote in ['"', '\''] {
        let pattern = format!("{key}={quote}");
        let mut from = 0;
        while let Some(start) = out[from..].find(&pattern).map(|i| i + from + pattern.len()) {
            let Some(end) = out[start..].find(quote).map(|i| i + start) else { break };
            out.replace_range(start..end, value);
            from = start + value.len();
        }
    }
    let open = format!("<{key}>");
    let close = format!("</{key}>");
    let mut from = 0;
    while let Some(start) = out[from..].find(&open).map(|i| i + from + open.len()) {
        let Some(end) = out[start..].find(&close).map(|i| i + start) else { break };
        out.replace_range(start..end, value);
        from = start + value.len();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use lopdf::{dictionary, Stream};

    #[test]
    fn dates_and_xmp_replacement() {
        let now = FixedOffset::east_opt(9 * 3600).unwrap().with_ymd_and_hms(2026, 9, 21, 17, 5, 9).unwrap();
        assert_eq!(format_pdf_date(now), "D:20260921170509+09'00'");
        let xmp = r#"<x xmp:ModifyDate="2020-01-01T00:00:00Z"/><xmp:MetadataDate>old</xmp:MetadataDate>"#;
        let out = replace_xmp_value(&replace_xmp_value(xmp, "xmp:ModifyDate", "NEW"), "xmp:MetadataDate", "NEW");
        assert_eq!(out, r#"<x xmp:ModifyDate="NEW"/><xmp:MetadataDate>NEW</xmp:MetadataDate>"#);
    }

    #[test]
    fn rewrite_drops_orphans_and_extra_trailer_keys() {
        let mut doc = Document::with_version("1.7");
        let pages = doc.add_object(dictionary! { "Type" => "Pages", "Kids" => vec![], "Count" => 0 });
        let meta = doc.add_object(Stream::new(dictionary! {}, br#"<r xmp:ModifyDate="x"/>"#.to_vec()));
        let root = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages, "Metadata" => meta });
        let info = doc.add_object(dictionary! { "Producer" => Object::string_literal("t") });
        let orphan = doc.add_object(dictionary! { "Old" => "OCR" });
        doc.trailer.set("Root", root);
        doc.trailer.set("Info", info);
        doc.trailer.set("W", vec![1.into(), 2.into(), 1.into()]);
        let dir = std::env::temp_dir().join(format!("pdf_ocr_save_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("out.pdf");
        let now = FixedOffset::east_opt(0).unwrap().with_ymd_and_hms(2026, 1, 2, 3, 4, 5).unwrap();
        save_rewritten(&mut doc, &path, now, false).unwrap();
        let reloaded = Document::load(&path).unwrap();
        assert!(reloaded.get_object(orphan).is_err());
        assert!(!reloaded.trailer.has(b"W"));
        let info = reloaded.get_dictionary(info).unwrap();
        assert_eq!(info.get(b"ModDate").unwrap().as_str().unwrap(), b"D:20260102030405+00'00'");
        let xmp = crate::content::stream_bytes(&reloaded, meta).unwrap();
        assert!(String::from_utf8(xmp).unwrap().contains("2026-01-02T03:04:05+00:00"));
        std::fs::remove_dir_all(dir).ok();
    }
}
