//! 텍스트 표시 연산자 분류(설계 문서 3장). 표준 모드의 자동 삭제 대상만 판정한다.
//!
//! - A·B: 표시 시점의 render mode 3(페이지 콘텐츠든 Form 안이든 해석기가 상태를 이어 준다).
//! - D: 폰트 크기 0, 가로 배율 0, 텍스트 행렬 × CTM의 행렬식 0.
//! - E: 칠하는 방식에 해당하는 불투명도가 0(채우기 `ca`, 선 `CA`, 둘 다 쓰면 둘 다 0).
//! - K(render mode 7, 보이지 않는 클리핑)는 지우면 뒤에 그려지는 것이 달라질 수 있어 남기고
//!   보고만 한다. 4~6도 클리핑이라 건드리지 않는다.

use crate::content::interp::{determinant, multiply, Context};
use crate::content::lexer::Operation;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HiddenKind {
    /// render mode 3.
    InvisibleMode,
    /// 크기·배율 0 또는 퇴화 행렬.
    ZeroSize,
    /// 완전 투명.
    Transparent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// 표시 연산자가 아니거나 보이는 텍스트.
    Keep,
    Remove(HiddenKind),
    /// render mode 7 — 남기고 보고.
    ClipOnlyKept,
}

pub fn is_show_operator(op: &Operation) -> bool {
    matches!(op.operator.as_slice(), b"Tj" | b"TJ" | b"'" | b"\"")
}

pub fn classify(context: &Context, op: &Operation) -> Verdict {
    if !is_show_operator(op) {
        return Verdict::Keep;
    }
    let state = context.state;
    let text = &state.text;
    let mode = text.render_mode;
    if mode == 3 {
        return Verdict::Remove(HiddenKind::InvisibleMode);
    }
    if mode == 7 {
        return Verdict::ClipOnlyKept;
    }
    if !(0..=2).contains(&mode) {
        return Verdict::Keep; // 4~6: 클리핑이 걸려 있어 건드리지 않는다
    }
    let text_space = [text.font_size * text.horizontal_scaling / 100.0, 0.0, 0.0, text.font_size, 0.0, text.rise];
    let rendering = multiply(&multiply(&text_space, context.text_matrix), &state.ctm);
    if determinant(&rendering).abs() < 1e-12 {
        return Verdict::Remove(HiddenKind::ZeroSize);
    }
    let fill_clear = state.fill_alpha <= 1e-6;
    let stroke_clear = state.stroke_alpha <= 1e-6;
    let transparent = match mode {
        0 => fill_clear,
        1 => stroke_clear,
        _ => fill_clear && stroke_clear,
    };
    if transparent {
        return Verdict::Remove(HiddenKind::Transparent);
    }
    Verdict::Keep
}
