//! 탭 줄 — 사이드바(북마크/썸네일)와 설정 창(색상 지정/단축키)이 함께 쓴다.
//!
//! **고른 탭만 아래 테두리를 끊어 본문과 이어지게 그린다.** 이것이 눌린 단추가 아니라 탭으로
//! 읽히게 하는 핵심이다(2026-10-01 요청). egui에 탭 위젯이 없어 직접 그린다 — 칸을 잡고,
//! 안 고른 탭 → 아래 가로줄 → 고른 탭 순으로 얹으면 고른 탭의 바탕이 그 가로줄을 덮는다.

const HEIGHT: f32 = 27.0;
const PAD_X: f32 = 15.0;
const GAP: f32 = 3.0;
const LEFT: f32 = 8.0;
/// 위 두 모서리만 둥글린다 — 아래는 본문과 이어져야 한다.
const RADIUS: f32 = 7.0;
/// 탭 아이콘 크기. 옆 글자보다 조금 커야 눈에 같은 무게로 보인다.
const ICON_SIZE: f32 = 15.0;
/// 아이콘과 글자 사이.
const ICON_GAP: f32 = 5.0;

/// 탭 줄 하나. `tabs`는 (값, 아이콘, 이름)이고, 눌린 탭이 `current`에 들어간다.
pub fn bar<T: Copy + PartialEq>(ui: &mut egui::Ui, id_salt: &str, current: &mut T, tabs: &[(T, char, &str)]) {
    let (strip, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), HEIGHT), egui::Sense::hover());
    if !ui.is_rect_visible(strip) {
        return;
    }

    let visuals = ui.visuals().clone();
    let border = visuals.widgets.noninteractive.bg_stroke.color;
    let font = egui::TextStyle::Button.resolve(ui.style());
    let icon_font = egui::FontId::new(ICON_SIZE, egui::FontFamily::Name(crate::fonts::ICON_FAMILY.into()));
    let rounding = egui::Rounding { nw: RADIUS, ne: RADIUS, sw: 0.0, se: 0.0 };

    // 칸을 먼저 다 잡아 둔다. 그려야 할 순서와 자리를 정하는 순서가 다르기 때문이다.
    let mut placed = Vec::with_capacity(tabs.len());
    let mut x = strip.left() + LEFT;
    for (value, icon, label) in tabs {
        let icon_galley =
            ui.painter().layout_no_wrap(icon.to_string(), icon_font.clone(), egui::Color32::PLACEHOLDER);
        let galley = ui.painter().layout_no_wrap((*label).to_string(), font.clone(), egui::Color32::PLACEHOLDER);
        let inner = icon_galley.size().x + ICON_GAP + galley.size().x;
        let rect = egui::Rect::from_min_size(egui::pos2(x, strip.top()), egui::vec2(inner + PAD_X * 2.0, HEIGHT));
        x = rect.right() + GAP;
        let response = ui.interact(rect, ui.id().with((id_salt, *label)), egui::Sense::click());
        if response.clicked() {
            *current = *value;
        }
        placed.push((*value, icon_galley, galley, rect, response));
    }

    let painter = ui.painter().clone();
    let draw = |value: &T,
                icon_galley: &std::sync::Arc<egui::Galley>,
                galley: &std::sync::Arc<egui::Galley>,
                rect: egui::Rect,
                hovered: bool| {
        let active = *current == *value;
        let fill = if active {
            visuals.panel_fill
        } else if hovered {
            visuals.widgets.hovered.bg_fill
        } else {
            visuals.faint_bg_color
        };
        painter.rect_filled(rect, rounding, fill);
        painter.rect_stroke(rect, rounding, egui::Stroke::new(1.0_f32, border));
        if active {
            // 아래 테두리만 지운다 — 본문과 이어져 보이도록.
            painter.line_segment(
                [egui::pos2(rect.left() + 1.0, rect.bottom()), egui::pos2(rect.right() - 1.0, rect.bottom())],
                egui::Stroke::new(2.0_f32, fill),
            );
        }
        // 마우스를 올리면 바탕이 회색으로 짙어지므로 흐린 글자는 묻힌다(2026-10-01 리포트).
        let color = if active {
            visuals.strong_text_color()
        } else if hovered {
            visuals.text_color()
        } else {
            visuals.weak_text_color()
        };
        // 아이콘 + 글자를 한 덩어리로 보고 칸 가운데에 놓는다.
        let inner = icon_galley.size().x + ICON_GAP + galley.size().x;
        let left = rect.center().x - inner / 2.0;
        painter.galley(egui::pos2(left, rect.center().y - icon_galley.size().y / 2.0), icon_galley.clone(), color);
        painter.galley(
            egui::pos2(left + icon_galley.size().x + ICON_GAP, rect.center().y - galley.size().y / 2.0),
            galley.clone(),
            color,
        );
    };

    for (value, icon_galley, galley, rect, response) in &placed {
        if *current != *value {
            draw(value, icon_galley, galley, *rect, response.hovered());
        }
    }
    painter.line_segment(
        [egui::pos2(strip.left(), strip.bottom()), egui::pos2(strip.right(), strip.bottom())],
        egui::Stroke::new(1.0_f32, border),
    );
    for (value, icon_galley, galley, rect, response) in &placed {
        if *current == *value {
            draw(value, icon_galley, galley, *rect, response.hovered());
        }
    }
}
