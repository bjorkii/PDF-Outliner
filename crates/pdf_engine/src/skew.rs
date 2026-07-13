//! 스캔+OCR 텍스트 레이어의 스큐(기울어짐) 대응.
//!
//! **2026-07-12 실제 PDF 테스트로 뒤집힌 가정**: 원래 설계는 `FPDFText_GetCharAngle`
//! (`angle_radians()`)이 문자의 실제 화면상 회전각이라고 가정하고 하이라이트 quad를
//! 그만큼 회전시켰다. 그런데 실제 사용자 PDF 2건(`BZB000877_01.pdf`의 idx 26,
//! `BZR001088_01.pdf`의 idx 0 — 둘 다 디자인/포스터류 문서)에서 `angle_radians()`가
//! 각각 6.215rad, 1.571rad(≈90°)를 반환했지만, `render_crop` 예제로 해당 문자를
//! 실제로 렌더링해 크롭한 이미지를 육안 확인한 결과 글리프는 완전히 똑바로(upright)
//! 서 있었다. 즉 이 각도는 폰트가 내부적으로 상쇄하는 "쓰기 방향/텍스트 배치 행렬"의
//! 회전일 뿐, 최종적으로 화면에 그려지는 글리프의 시각적 회전과 다를 수 있다.
//!
//! `loose_bounds()`는 이미 최종 렌더링(=시각) 좌표계 기준 축정렬 박스이므로, 여기에
//! `angle_radians()`를 그대로 적용해 회전시키면 오히려 똑바로 선 글자 위에 90도 가까이
//! 뒤틀린 하이라이트가 그려지는 역효과가 난다. 실제 스캔 스큐(글리프 자체가 시각적으로
//! 기울어진 경우)와 이 "쓰기 방향 행렬 회전"을 안전하게 구분할 방법을 찾기 전까지는,
//! 회전을 적용하지 않고 axis-aligned 박스를 그대로 사용한다 — 스캔 스큐 문서에서는
//! 하이라이트가 살짝 헐렁하게(loose) 나올 수 있지만, 디자인 문서에서 하이라이트가
//! 완전히 엉뚱한 방향으로 뒤집히는 것보다는 안전하다.

use anyhow::Result;
use pdfium_render::prelude::*;

/// 문자 하나의 렌더링용 quad(축정렬).
#[derive(Debug, Clone, Copy)]
pub struct CharQuad {
    pub top_left: (f32, f32),
    pub top_right: (f32, f32),
    pub bottom_right: (f32, f32),
    pub bottom_left: (f32, f32),
    /// `angle_radians()` 원본값. 위 모듈 문서 참고 — 시각적 회전과 다를 수 있어 quad
    /// 계산에는 더 이상 쓰지 않고, 디버그/진단 목적으로만 남겨둔다.
    pub rotation_radians: f32,
}

/// 문자 슬라이스로부터 quad 목록을 만든다. `loose_bounds()`의 축정렬 박스를 그대로 쓴다.
pub fn char_quads_with_rotation(chars: &[PdfPageTextChar]) -> Result<Vec<CharQuad>> {
    let mut quads = Vec::with_capacity(chars.len());

    for ch in chars {
        let bounds = ch.loose_bounds()?;
        let rotation = ch.angle_radians().unwrap_or(0.0);

        let left = bounds.left().value;
        let right = bounds.right().value;
        let top = bounds.top().value;
        let bottom = bounds.bottom().value;

        quads.push(CharQuad {
            top_left: (left, top),
            top_right: (right, top),
            bottom_right: (right, bottom),
            bottom_left: (left, bottom),
            rotation_radians: rotation,
        });
    }

    Ok(quads)
}

/// 문자 range를 텍스트로 합치되, 줄 경계에서 개행을 삽입한다.
/// 가로쓰기는 y좌표 급변, 세로쓰기는 x좌표 급변으로 줄바꿈을 감지한다(둘 다 대비해 두 축 모두 확인).
/// PDF가 vertical writing mode로 올바르게 인코딩돼 있다면 문자 인덱스 순서 자체가
/// 이미 논리적 읽기 순서이므로, 여기서는 "표시상 개행 위치"만 판별하면 된다.
pub fn text_with_line_breaks(chars: &[PdfPageTextChar]) -> String {
    let mut result = String::new();
    let mut prev_pos: Option<(f32, f32)> = None;

    for ch in chars {
        if let (Ok(bounds), Some((prev_x, prev_y))) = (ch.loose_bounds(), prev_pos) {
            let cur_x = bounds.left().value;
            let cur_y = bounds.top().value;
            let dy = (cur_y - prev_y).abs();
            let dx = (cur_x - prev_x).abs();
            let font_scale = (bounds.top().value - bounds.bottom().value).abs().max(1.0);

            // 같은 줄 내 정상 진행 범위를 크게 벗어나면 줄바꿈으로 간주
            if dy > font_scale * 1.5 || dx > font_scale * 6.0 {
                result.push('\n');
            }
        }

        if let Some(unicode) = ch.unicode_char() {
            result.push(unicode);
        }

        prev_pos = ch.loose_bounds().ok().map(|b| (b.left().value, b.top().value));
    }

    result
}
