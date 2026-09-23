//! 텍스트 표시 연산자 분류(설계 문서 3장).
//!
//! **표준 모드에서 지우는 것**(자동)
//! - A·B: 표시 시점의 render mode 3(페이지 콘텐츠든 Form 안이든 해석기가 상태를 이어 준다).
//! - D: 폰트 크기 0, 가로 배율 0, 텍스트 행렬 × CTM의 행렬식 0.
//! - E: 칠하는 방식에 해당하는 불투명도가 0(채우기 `ca`, 선 `CA`, 둘 다 쓰면 둘 다 0).
//!
//! **적극 모드에서만 지우는 것**(표준 모드에서는 세어서 보고만 한다). 모두 휴리스틱이라 오탐이
//! 있을 수 있고, 지우더라도 렌더 비교로 화면이 바뀌지 않음을 확인해야 반영된다.
//! - F: 기본 상태에서 꺼져 있는 레이어(OCG·OCMD) 안의 텍스트.
//! - G: 흰 글씨(배경과 같은 색으로 숨긴 것).
//! - H: 나중에 그려지는 불투명 이미지에 완전히 덮이는 텍스트.
//! - I: CropBox 밖의 텍스트.
//! - J: 클리핑 영역 밖의 텍스트.
//! - K: render mode 7(보이지 않는 클리핑). 지우면 뒤에 그려지는 것이 달라질 수 있다.
//!
//! render mode 4~6은 보이는 글자이면서 클리핑도 하므로 어느 모드에서도 건드리지 않는다.

use crate::content::interp::{boxes_overlap, contains_box, determinant, multiply, Context};
use crate::content::lexer::Operation;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum HiddenKind {
    /// A·B: render mode 3.
    InvisibleMode,
    /// D: 크기·배율 0 또는 퇴화 행렬.
    ZeroSize,
    /// E: 완전 투명.
    Transparent,
    /// F: 기본으로 꺼진 레이어.
    HiddenLayer,
    /// G: 흰 글씨.
    WhiteText,
    /// H: 불투명 이미지 아래.
    UnderImage,
    /// I: 페이지(CropBox) 밖.
    OutsidePage,
    /// J: 클리핑 영역 밖.
    Clipped,
    /// K: render mode 7.
    ClipMode,
}

impl HiddenKind {
    pub fn label(&self) -> &'static str {
        match self {
            HiddenKind::InvisibleMode => "보이지 않게 그린 텍스트",
            HiddenKind::ZeroSize => "크기 0 텍스트",
            HiddenKind::Transparent => "완전 투명 텍스트",
            HiddenKind::HiddenLayer => "꺼진 레이어 안 텍스트",
            HiddenKind::WhiteText => "흰 글씨",
            HiddenKind::UnderImage => "이미지에 덮인 텍스트",
            HiddenKind::OutsidePage => "페이지 밖 텍스트",
            HiddenKind::Clipped => "클리핑 밖 텍스트",
            HiddenKind::ClipMode => "보이지 않는 클리핑 텍스트",
        }
    }

    /// 표준 모드에서도 지우는 형태인지.
    pub fn is_standard(&self) -> bool {
        matches!(self, HiddenKind::InvisibleMode | HiddenKind::ZeroSize | HiddenKind::Transparent)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// 표시 연산자가 아니거나 보이는 텍스트.
    Keep,
    Hidden(HiddenKind),
    /// 보이는 텍스트지만, 나중에 그려지는 이미지에 덮이는지 확인해야 한다(H).
    MaybeUnderImage,
}

/// 페이지 단위로 미리 구해 두는 판정 자료.
pub struct PageFacts {
    /// CropBox(사용자 공간 `[left, bottom, right, top]`).
    pub crop: [f64; 4],
    /// 지금 위치가 기본으로 꺼진 레이어 안인지.
    pub in_hidden_layer: bool,
}

pub fn is_show_operator(op: &Operation) -> bool {
    matches!(op.operator.as_slice(), b"Tj" | b"TJ" | b"'" | b"\"")
}

/// 흰색(또는 그에 가까운) 칠인지 — 색 공간을 아는 경우만.
fn is_white(space: &[u8], color: &[f64]) -> bool {
    let bright = |v: &f64| *v > 0.95;
    match space {
        b"DeviceGray" | b"CalGray" | b"G" => color.len() == 1 && bright(&color[0]),
        b"DeviceRGB" | b"CalRGB" | b"RGB" => color.len() == 3 && color.iter().all(bright),
        b"DeviceCMYK" | b"CMYK" => color.len() == 4 && color.iter().all(|v| *v < 0.05),
        _ => false,
    }
}

pub fn classify(context: &Context, op: &Operation, facts: &PageFacts) -> Verdict {
    if !is_show_operator(op) {
        return Verdict::Keep;
    }
    let state = context.state;
    let text = &state.text;
    let mode = text.render_mode;
    if mode == 3 {
        return Verdict::Hidden(HiddenKind::InvisibleMode);
    }
    if mode == 7 {
        return Verdict::Hidden(HiddenKind::ClipMode);
    }
    if !(0..=2).contains(&mode) {
        return Verdict::Keep; // 4~6: 보이는 글자이면서 클리핑
    }
    let text_space = [text.font_size * text.horizontal_scaling / 100.0, 0.0, 0.0, text.font_size, 0.0, text.rise];
    let rendering = multiply(&multiply(&text_space, context.text_matrix), &state.ctm);
    if determinant(&rendering).abs() < 1e-12 {
        return Verdict::Hidden(HiddenKind::ZeroSize);
    }
    let fill_clear = state.fill_alpha <= 1e-6;
    let stroke_clear = state.stroke_alpha <= 1e-6;
    let transparent = match mode {
        0 => fill_clear,
        1 => stroke_clear,
        _ => fill_clear && stroke_clear,
    };
    if transparent {
        return Verdict::Hidden(HiddenKind::Transparent);
    }
    if facts.in_hidden_layer {
        return Verdict::Hidden(HiddenKind::HiddenLayer);
    }
    if let Some(bbox) = context.show.map(|s| s.bbox) {
        if !boxes_overlap(&facts.crop, &bbox) {
            return Verdict::Hidden(HiddenKind::OutsidePage);
        }
        if let Some(clip) = state.clip {
            if !boxes_overlap(&clip, &bbox) {
                return Verdict::Hidden(HiddenKind::Clipped);
            }
        }
    }
    // 채우기만 하는(또는 채우고 선도 긋는) 흰 글씨.
    if mode != 1 && is_white(&state.fill_color_space, &state.fill_color) {
        return Verdict::Hidden(HiddenKind::WhiteText);
    }
    if context.show.is_some() {
        return Verdict::MaybeUnderImage;
    }
    Verdict::Keep
}

/// 글자 상자가 나중에 그려진 불투명 이미지에 완전히 덮이는지(H).
pub fn covered_by_image(bbox: &[f64; 4], order: usize, images: &[(usize, [f64; 4], bool)]) -> bool {
    images.iter().any(|(image_order, image_box, opaque)| *opaque && *image_order > order && contains_box(image_box, bbox))
}
