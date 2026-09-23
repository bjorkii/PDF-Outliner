//! OCR 텍스트 레이어 삽입(설계 문서 6.5, 6.6)과 앱이 넣은 레이어 떼어 내기(형태 C).
//!
//! **구조** — 삭제하기 쉽게 레이어를 구조적으로 분리한다.
//! - 페이지마다 OCR 전용 Form XObject를 하나 만든다. Form 공간은 표시 페이지 프레임(포인트, 좌상단
//!   원점, y 아래로)이고 `/Matrix`가 사용자 공간으로 옮긴다(`PageFrame::display_to_user_matrix`).
//! - 페이지 `/Contents`는 `[시작 스트림(q), 기존 콘텐츠…, 끝 스트림(Q…), 호출 스트림]`이 된다.
//!   기존 콘텐츠가 CTM이나 상태를 바꾼 채 끝날 수 있어 `q … Q`로 감싸며, 기존 콘텐츠의 `q`/`Q`가
//!   짝이 맞지 않으면 모자란 `Q`를 끝 스트림에 더한다.
//! - 세 스트림 첫 줄에 `% PDFOutliner-OCR …` 주석을, Form에는 `/PieceInfo /PDFOutliner`(규격이 정한
//!   애플리케이션 전용 데이터 자리)를 넣어 앱이 넣은 레이어임을 표시한다.
//!
//! **단어 배치** — 줄마다 `BT … ET`, 렌더 모드 3. 단어는 줄 방향(`textangle`)을 바로 세운 좌표에서
//! 계산한다: 글자 크기 = 그 단어 bbox 높이, 기준선 = 그 단어 bbox 아래 변(glyphless 폰트의 descent가
//! 0이라 선택 상자가 bbox와 정확히 겹친다, `glyphless` 모듈 문서), 가로 배율 `Tz` = 단어 폭 ÷
//! (글자 수 × 폭 × 글자 크기). 단어 뒤에는 공백 글자를 붙여 추출기가 단어를 붙이지 않게 한다.
//!
//! 설계 문서 6.6은 "한 줄의 단어는 같은 크기"였지만, 첫 글자가 큰 장식 글자(드롭캡)인 줄에서 줄 전체의
//! 크기·기준선이 그 글자에 끌려가 다음 줄과 섞이는 것을 왕복 시험(내보내기 → 가져오기 → 내보내기)에서
//! 확인해(2026-09-22) 단어별로 바꿨다. hOCR `baseline`의 기울기는 글자 방향 회전으로만 반영하고,
//! 오프셋은 쓰지 않는다(위 기준선 규칙이 우선).

use crate::content::{page_content_ids, page_content_bytes};
use crate::content::lexer::tokenize;
use crate::geometry::PageFrame;
use crate::glyphless::{self, CidAssigner, GLYPH_WIDTH};
use crate::layout::DRect;
use crate::remove::Applied;
use crate::resources::{ensure_private_page_resources, page_subdict_mut, unique_name, ReferenceCounts};
use anyhow::{Context, Result};
use lopdf::{dictionary, Document, Object, ObjectId, Stream, StringFormat};

const MARK_BEGIN: &[u8] = b"% PDFOutliner-OCR begin\n";
const MARK_END: &[u8] = b"% PDFOutliner-OCR end\n";
const MARK_CALL: &[u8] = b"% PDFOutliner-OCR layer\n";

/// 표시 프레임(포인트)의 단어.
#[derive(Debug, Clone, PartialEq)]
pub struct LayerWord {
    pub text: String,
    pub rect: DRect,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LayerLine {
    pub words: Vec<LayerWord>,
    /// 글자 방향(반시계, 도).
    pub angle: f64,
    /// hOCR baseline 기울기(y 아래 방향 기준).
    pub slope: f64,
}

/// 가로쓰기 좌표를 반시계로 `deg`만큼 돌린다(y가 아래로 커지는 좌표).
fn rotate(deg: f64, (x, y): (f64, f64)) -> (f64, f64) {
    let (s, c) = deg.to_radians().sin_cos();
    (x * c + y * s, -x * s + y * c)
}

fn rotate_rect(deg: f64, r: &DRect) -> DRect {
    DRect::from_points(&[(r.x0, r.y0), (r.x1, r.y0), (r.x0, r.y1), (r.x1, r.y1)].map(|p| rotate(deg, p)))
}

fn fmt(v: f64) -> String {
    let s = format!("{v:.3}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" || s.is_empty() {
        "0".to_string()
    } else {
        s.to_string()
    }
}

/// 한 페이지 OCR Form의 콘텐츠(폰트 리소스 이름 `/F0`).
fn layer_content(lines: &[LayerLine], cids: &mut CidAssigner) -> Vec<u8> {
    let mut out = String::new();
    for line in lines {
        if line.words.is_empty() {
            continue;
        }
        // 줄을 바로 세운 좌표로.
        let local: Vec<DRect> = line.words.iter().map(|w| rotate_rect(-line.angle, &w.rect)).collect();
        let skew = -line.slope.atan().to_degrees();
        let glyph_angle = line.angle + skew;
        let (s, c) = glyph_angle.to_radians().sin_cos();
        out.push_str("BT\n3 Tr\n");
        let last = line.words.len() - 1;
        for (i, (word, rect)) in line.words.iter().zip(&local).enumerate() {
            let chars = word.text.chars().count();
            let height = rect.height();
            if chars == 0 || height <= 0.0 || rect.width() <= 0.0 {
                continue;
            }
            let text = if i < last { format!("{} ", word.text) } else { word.text.clone() };
            let encoded = cids.encode(&text);
            if encoded.is_empty() {
                continue;
            }
            let scaling = rect.width() / (chars as f64 * GLYPH_WIDTH / 1000.0 * height) * 100.0;
            let (x, y) = rotate(line.angle, (rect.x0, rect.y1));
            let hex: String = encoded.iter().map(|b| format!("{b:02X}")).collect();
            out.push_str(&format!(
                "/F0 {} Tf\n{} Tz\n{} {} {} {} {} {} Tm\n<{hex}> Tj\n",
                fmt(height),
                fmt(scaling.max(0.01)),
                fmt(c),
                fmt(-s),
                fmt(-s),
                fmt(-c),
                fmt(x),
                fmt(y)
            ));
        }
        out.push_str("ET\n");
    }
    out.into_bytes()
}

/// 기존 콘텐츠를 다 실행했을 때 닫히지 않고 남는 `q` 수(해석 실패 시 0).
fn unclosed_saves(doc: &Document, page_id: ObjectId) -> usize {
    let Ok(bytes) = page_content_bytes(doc, page_id) else { return 0 };
    let Ok(operations) = tokenize(&bytes) else { return 0 };
    let mut depth = 0usize;
    for op in &operations {
        match op.operator.as_slice() {
            b"q" => depth += 1,
            b"Q" => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    depth
}

/// 여러 페이지에 레이어를 넣는다. 폰트는 문서에 하나만 넣고 모든 페이지가 함께 쓴다.
/// 각 페이지는 넣기 전 상태를 `applied`에 기록한다(되돌리기용).
pub fn insert_layers(
    doc: &mut Document,
    pages: &[(usize, ObjectId, PageFrame, Vec<LayerLine>)],
    now_pdf_date: &str,
    applied: &mut Applied,
) -> Result<()> {
    let mut cids = CidAssigner::default();
    let contents: Vec<Vec<u8>> = pages.iter().map(|(_, _, _, lines)| layer_content(lines, &mut cids)).collect();
    if cids.is_empty() {
        return Ok(());
    }
    let font = glyphless::add_font(doc, &cids);
    let mut counts = ReferenceCounts::count(doc);

    for ((index, page_id, frame, _), content) in pages.iter().zip(contents) {
        if content.is_empty() {
            continue;
        }
        applied.record_page(doc, *index, *page_id)?;
        let (w, h) = frame.display_size();
        let matrix: Vec<Object> = frame.display_to_user_matrix().iter().map(|v| Object::Real(*v as f32)).collect();
        let mut form = Stream::new(
            dictionary! {
                "Type" => "XObject",
                "Subtype" => "Form",
                "BBox" => vec![0.into(), 0.into(), Object::Real(w as f32), Object::Real(h as f32)],
                "Matrix" => matrix,
                "Resources" => dictionary! { "Font" => dictionary! { "F0" => font } },
                "PieceInfo" => dictionary! {
                    "PDFOutliner" => dictionary! {
                        "LastModified" => Object::String(now_pdf_date.as_bytes().to_vec(), StringFormat::Literal),
                        "Private" => dictionary! {
                            "Type" => "OCRLayer",
                            "Version" => 1,
                            "Source" => Object::String(b"hOCR".to_vec(), StringFormat::Literal),
                        },
                    },
                },
            },
            content,
        );
        let _ = form.compress();
        let form_id = doc.add_object(form);

        ensure_private_page_resources(doc, &mut counts, *page_id)?;
        let xobjects = page_subdict_mut(doc, &mut counts, *page_id, b"XObject")?;
        let name = unique_name(xobjects, "OCR");
        xobjects.set(name.clone(), form_id);

        let extra = unclosed_saves(doc, *page_id);
        let original = page_content_ids(doc, *page_id)?;
        let begin = doc.add_object(Stream::new(dictionary! {}, [MARK_BEGIN, b"q\n"].concat()));
        let mut end_bytes = MARK_END.to_vec();
        end_bytes.extend(std::iter::repeat_n(&b"Q\n"[..], 1 + extra).flatten());
        let end = doc.add_object(Stream::new(dictionary! {}, end_bytes));
        let mut call_bytes = MARK_CALL.to_vec();
        call_bytes.extend_from_slice(b"q /");
        call_bytes.extend_from_slice(&name);
        call_bytes.extend_from_slice(b" Do Q\n");
        let call = doc.add_object(Stream::new(dictionary! {}, call_bytes));

        let mut new_contents: Vec<Object> = vec![begin.into()];
        new_contents.extend(original.into_iter().map(Object::Reference));
        new_contents.push(end.into());
        new_contents.push(call.into());
        doc.get_dictionary_mut(*page_id)?.set("Contents", new_contents);
    }
    Ok(())
}

/// 페이지에 앱이 넣은 레이어가 있으면 (시작, 끝, 호출 스트림의 `/Contents` 위치, 호출이 쓰는 리소스 이름).
fn own_layer(doc: &Document, page_id: ObjectId) -> Option<(usize, usize, usize, Vec<u8>)> {
    let ids = page_content_ids(doc, page_id).ok()?;
    let starts_with = |id: ObjectId, mark: &[u8]| {
        crate::content::stream_bytes(doc, id).ok().filter(|b| b.starts_with(mark))
    };
    let begin = ids.iter().position(|&id| starts_with(id, MARK_BEGIN).is_some())?;
    let end = ids.iter().rposition(|&id| starts_with(id, MARK_END).is_some())?;
    let call = ids.len().checked_sub(1)?;
    let call_bytes = starts_with(ids[call], MARK_CALL)?;
    if !(begin < end && end < call) {
        return None;
    }
    let operations = tokenize(&call_bytes).ok()?;
    let name = operations.iter().find(|op| op.is(b"Do"))?.operand_name(0)?.to_vec();
    Some((begin, end, call, name))
}

/// 앱이 넣은 레이어가 있는 페이지(0부터).
pub fn pages_with_own_layer(doc: &Document) -> Vec<usize> {
    doc.get_pages()
        .values()
        .enumerate()
        .filter(|(_, &id)| own_layer(doc, id).is_some())
        .map(|(i, _)| i)
        .collect()
}

/// 앱이 넣은 레이어를 구조째 떼어 낸다(시작·끝·호출 스트림과 리소스 이름). `only`가 있으면 그
/// 페이지들(0부터)만. 호출 스트림이 `/Contents` 맨 끝에 있을 때만 뗀다 — 뒤에 다른 도구가 덧붙인
/// 내용이 있으면 그 내용이 끝 스트림의 `Q`에 기대고 있을 수 있어서다(그런 페이지의 텍스트는 일반
/// 삭제 규칙이 지운다). 뗀 페이지 번호를 돌려준다.
pub fn strip_own_layers(
    doc: &mut Document,
    only: Option<&std::collections::BTreeSet<usize>>,
    applied: &mut Applied,
) -> Result<Vec<usize>> {
    let pages: Vec<ObjectId> = doc.get_pages().into_values().collect();
    let mut stripped = Vec::new();
    for (index, page_id) in pages.into_iter().enumerate() {
        if only.is_some_and(|set| !set.contains(&index)) {
            continue;
        }
        let Some((begin, end, call, name)) = own_layer(doc, page_id) else { continue };
        // 이름이 페이지 자신의 Resources에 있어야 안전하게 지울 수 있다(삽입이 늘 그렇게 만든다).
        let direct = doc
            .get_dictionary(page_id)?
            .get(b"Resources")
            .ok()
            .and_then(|r| r.as_dict().ok())
            .and_then(|r| r.get(b"XObject").ok())
            .and_then(|x| x.as_dict().ok())
            .is_some_and(|x| x.has(&name));
        if !direct {
            continue;
        }
        applied.record_page(doc, index, page_id)?;
        let ids = page_content_ids(doc, page_id)?;
        let kept: Vec<Object> = ids
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != begin && *i != end && *i != call)
            .map(|(_, id)| Object::Reference(*id))
            .collect();
        let page = doc.get_dictionary_mut(page_id)?;
        page.set("Contents", kept);
        page.get_mut(b"Resources")?
            .as_dict_mut()?
            .get_mut(b"XObject")?
            .as_dict_mut()
            .context("XObject 딕셔너리")?
            .remove(&name);
        stripped.push(index);
    }
    Ok(stripped)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::interp::tests::one_page_doc;
    use crate::geometry::Rect;

    fn frame(rotate: u16) -> PageFrame {
        PageFrame { crop: Rect { llx: 0.0, lly: 0.0, urx: 200.0, ury: 100.0 }, rotate, user_unit: 1.0 }
    }

    fn word(text: &str, x0: f64, y0: f64, x1: f64, y1: f64) -> LayerWord {
        LayerWord { text: text.to_string(), rect: DRect { x0, y0, x1, y1 } }
    }

    #[test]
    fn horizontal_line_content() {
        let mut cids = CidAssigner::default();
        let line = LayerLine { words: vec![word("ab", 10.0, 20.0, 30.0, 30.0), word("c", 40.0, 20.0, 45.0, 30.0)], angle: 0.0, slope: 0.0 };
        let content = String::from_utf8(layer_content(&[line], &mut cids)).unwrap();
        // 글자 크기 10(단어 높이), 기준선 y=30, "ab " → Tz = 20 / (2 × 0.5 × 10) × 100 = 200
        assert!(content.contains("/F0 10 Tf\n200 Tz\n1 0 0 -1 10 30 Tm\n<000100020003> Tj"), "{content}");
        // 마지막 단어엔 공백 없음, Tz = 5 / (1 × 0.5 × 10) × 100 = 100
        assert!(content.contains("/F0 10 Tf\n100 Tz\n1 0 0 -1 40 30 Tm\n<0004> Tj"), "{content}");
    }

    #[test]
    fn rotated_line_starts_at_the_rotated_corner() {
        // 표시 프레임에서 위에서 아래로 읽는 줄(textangle 270): 세로 띠 x 88~100, y 0~60.
        let mut cids = CidAssigner::default();
        let line = LayerLine { words: vec![word("abc", 88.0, 0.0, 100.0, 60.0)], angle: 270.0, slope: 0.0 };
        let content = String::from_utf8(layer_content(&[line], &mut cids)).unwrap();
        // 바로 세우면 높이 12, 폭 60, 글자는 아래로 흐르고(방향 (0,1)) 위쪽은 오른쪽(1,0) —
        // 기준선은 왼쪽 변 x=88, 시작은 위 y=0.
        assert!(content.contains("/F0 12 Tf"), "{content}");
        assert!(content.contains("0 1 1 0 88 0 Tm"), "{content}");
    }

    #[test]
    fn insert_then_strip_restores_page() {
        let (mut doc, page) = one_page_doc(b"q 1 0 0 1 5 5 cm", vec![]); // 닫히지 않은 q
        let before = doc.get_object(page).unwrap().clone();
        let mut applied = Applied::default();
        let lines = vec![LayerLine { words: vec![word("가", 0.0, 0.0, 10.0, 10.0)], angle: 0.0, slope: 0.0 }];
        insert_layers(&mut doc, &[(0, page, frame(0), lines)], "D:20260101000000Z", &mut applied).unwrap();
        let text = String::from_utf8(page_content_bytes(&doc, page).unwrap()).unwrap();
        assert!(text.starts_with("% PDFOutliner-OCR begin\nq\n"));
        assert!(text.contains("% PDFOutliner-OCR end\nQ\nQ\n"), "닫히지 않은 q 하나만큼 Q 추가: {text}");
        assert!(text.ends_with("q /OCR0 Do Q\n"));
        assert_eq!(pages_with_own_layer(&doc), vec![0]);

        let mut applied = Applied::default();
        assert_eq!(strip_own_layers(&mut doc, None, &mut applied).unwrap(), vec![0]);
        assert!(pages_with_own_layer(&doc).is_empty());
        assert_eq!(page_content_bytes(&doc, page).unwrap(), b"q 1 0 0 1 5 5 cm");
        let _ = before;
    }
}
