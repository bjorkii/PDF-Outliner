//! 텍스트 선택/복사 핵심 설계: "좌표 교차 방식"이 아닌 "문자 인덱스 range" 기반.
//!
//! 배경: 텍스트 레이어가 있는 PDF에서 "선택은 되는데 복사가 안 되거나 깨지는" 버그의
//! 원인은 대개 뷰어가 마우스 드래그 사각형과 문자 렌더링 좌표를 기하학적으로 교차시켜
//! 선택 범위를 계산하기 때문이다. 이 방식은 스큐(기울어진 스캔+OCR 페이지),
//! 세로쓰기, 자간이 있는 문서에서 문자 순서가 깨진다.
//!
//! 대신 다음 3단계로 처리한다:
//! 1. 마우스 다운 시 좌표 → 문자 인덱스 변환은 PDFium 자체 히트테스트(FPDFText_GetCharIndexAtPos)에 위임
//! 2. 드래그 중에는 시작~끝 "인덱스"만 갱신 (좌표 재계산 아님)
//! 3. 복사 시 FPDFText_GetText(start_index, count)로 range 텍스트를 통째로 추출
//!    (PDFium이 내부적으로 ToUnicode 매핑을 처리하므로, 문서 자체 결함이 없다면 정확한 문자열이 나옴)
//!
//! 문자 인덱스 순서 자체가 PDF 콘텐츠 스트림 상의 논리적 읽기 순서를 따르므로,
//! 세로쓰기 문서(CID 폰트 + vertical writing mode로 올바르게 인코딩된 경우)도
//! 별도 분기 없이 동일한 로직으로 처리된다.

use anyhow::{Context, Result};
use pdfium_render::prelude::*;

/// 선택 범위는 좌표가 아니라 인덱스로 표현한다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextSelectionRange {
    pub start_index: i32,
    /// end_index는 exclusive가 아니라 마지막으로 선택된 문자의 인덱스(inclusive).
    pub end_index: i32,
}

impl TextSelectionRange {
    /// 두 앵커 인덱스(마우스 다운 지점, 현재 드래그 지점)로부터 정규화된 range를 만든다.
    /// 사용자가 오른쪽에서 왼쪽으로 드래그해도 항상 start <= end가 되도록 정렬.
    pub fn from_anchors(anchor_a: i32, anchor_b: i32) -> Self {
        Self {
            start_index: anchor_a.min(anchor_b),
            end_index: anchor_a.max(anchor_b),
        }
    }

    pub fn count(&self) -> i32 {
        self.end_index - self.start_index + 1
    }
}

/// 마우스 좌표(페이지 좌표계, PdfPoints 단위) → 문자 인덱스.
/// tolerance는 클릭 판정 여유 범위(포인트 단위). 히트테스트 자체는 pdfium에 위임하고
/// 우리가 직접 기하 계산을 재구현하지 않는다 — 스큐된 문자에도 별도 보정 불필요.
pub fn char_index_at_point(
    text_page: &PdfPageText,
    x: PdfPoints,
    y: PdfPoints,
    tolerance_x: PdfPoints,
    tolerance_y: PdfPoints,
) -> Option<i32> {
    text_page
        .chars()
        .get_char_near_point(x, tolerance_x, y, tolerance_y)
        .map(|c| c.index() as i32)
}

/// range 내 문자들을 순서대로 가져온다.
/// pdfium-render 0.9.x에는 `chars_range` 같은 슬라이스 API가 없어, 전체 문자 컬렉션에서
/// 인덱스로 하나씩 조회해 모은다(문자 인덱스 자체가 논리적 읽기 순서를 따르므로 정렬은 불필요).
fn chars_in_range<'a>(
    text_page: &'a PdfPageText,
    range: TextSelectionRange,
) -> Result<Vec<PdfPageTextChar<'a>>> {
    let chars = text_page.chars();
    (range.start_index..=range.end_index)
        .map(|i| {
            chars
                .get(i as usize)
                .with_context(|| format!("문자 인덱스 {} 조회 실패", i))
        })
        .collect()
}

/// 인덱스 range로 텍스트를 추출한다(복사용). ToUnicode 매핑은 pdfium이 내부 처리.
pub fn extract_text(text_page: &PdfPageText, range: TextSelectionRange) -> Result<String> {
    let chars = chars_in_range(text_page, range)?;
    Ok(crate::skew::text_with_line_breaks(&chars))
}

/// 선택 영역 하이라이트 렌더링용 quad 목록.
/// 전체 선택을 하나의 큰 사각형으로 뭉뚱그리지 않고, 문자별로 개별 quad를 반환한다 —
/// 세로쓰기처럼 문자별 위치가 불규칙한 경우에도 선택 영역이 실제 문자 배치를 따라간다.
/// (회전 quad는 더 이상 적용하지 않음: crate::skew 모듈 문서의 2026-07-12 기록 참고)
pub fn selection_quads(
    text_page: &PdfPageText,
    range: TextSelectionRange,
) -> Result<Vec<crate::skew::CharQuad>> {
    let chars = chars_in_range(text_page, range)?;
    crate::skew::char_quads_with_rotation(&chars)
}
