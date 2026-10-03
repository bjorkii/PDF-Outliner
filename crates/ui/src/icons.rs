//! 툴바·사이드바 아이콘 — `Phosphor-Custom-Light.ttf`의 글리프 하나하나에 이름을 붙여 둔 곳.
//!
//! 아이콘을 **글자로** 쓴다. 테마 글자색을 저절로 따라가고(다크 모드가 공짜다), 어느 크기에서도
//! 또렷하며, egui가 글자처럼 재어 주므로 줄 안에서 저절로 정렬된다.
//!
//! **합자(ligature)는 쓸 수 없다.** egui 0.29는 `ab_glyph`로 글자를 그리는데 shaping을 하지 않아
//! Material Symbols 식의 "이름을 치면 아이콘이 나온다"가 먹지 않는다. 그래서 코드포인트를 직접
//! 적는다. 어느 코드가 어느 아이콘인지는 Phosphor가 주는 `style.css`에 적혀 있고, 아래 주석에
//! 원래 이름을 함께 남겨 둔다.

/// Phosphor 글리프. 주석의 이름은 phosphoricons.com에서 찾을 수 있는 이름이다.
pub const PAGE_MODE: char = '\u{e230}'; // file
pub const FIT_WIDTH: char = '\u{eb06}'; // arrows-horizontal
pub const FIT_PAGE: char = '\u{e1d0}'; // corners-out
pub const ZOOM_OUT: char = '\u{e32a}'; // minus
pub const ZOOM_IN: char = '\u{e3d4}'; // plus
pub const PREV_PAGE: char = '\u{edac}'; // less-than
pub const NEXT_PAGE: char = '\u{edc4}'; // greater-than
pub const HISTORY_BACK: char = '\u{e5a4}'; // skip-back
pub const HISTORY_FORWARD: char = '\u{e5a6}'; // skip-forward
pub const SEARCH: char = '\u{e30c}'; // magnifying-glass
pub const PREV_HIT: char = '\u{e05a}'; // arrow-circle-left
pub const NEXT_HIT: char = '\u{e02e}'; // arrow-circle-right
pub const DOCKED: char = '\u{e65c}'; // push-pin-simple
pub const FLOATING: char = '\u{e3e2}'; // push-pin
pub const SETTINGS: char = '\u{e270}'; // gear
/// 설정 창의 단축키 탭에서 쓴다 — 그 화면은 아직 만드는 중이다.
#[allow(dead_code)]
pub const EDIT_SHORTCUT: char = '\u{e3b4}'; // pencil-simple
pub const TAB_BOOKMARKS: char = '\u{e0e4}'; // book-bookmark
pub const TAB_THUMBNAILS: char = '\u{e0a8}'; // article
pub const ADD: char = '\u{ed4a}'; // plus-square
pub const REMOVE: char = '\u{ed4c}'; // minus-square
pub const UNDO: char = '\u{e014}'; // arrow-arc-left
pub const REDO: char = '\u{e016}'; // arrow-arc-right
pub const CLOSE: char = '\u{e4f6}'; // x

/// 상수와 Phosphor 이름의 짝. 시험이 `assets/fonts/codepoints.txt`와 맞춰 본다.
#[cfg(test)]
const NAMED: &[(&str, char, &str)] = &[
    ("PAGE_MODE", PAGE_MODE, "file"),
    ("FIT_WIDTH", FIT_WIDTH, "arrows-horizontal"),
    ("FIT_PAGE", FIT_PAGE, "corners-out"),
    ("ZOOM_OUT", ZOOM_OUT, "minus"),
    ("ZOOM_IN", ZOOM_IN, "plus"),
    ("PREV_PAGE", PREV_PAGE, "less-than"),
    ("NEXT_PAGE", NEXT_PAGE, "greater-than"),
    ("HISTORY_BACK", HISTORY_BACK, "skip-back"),
    ("HISTORY_FORWARD", HISTORY_FORWARD, "skip-forward"),
    ("SEARCH", SEARCH, "magnifying-glass"),
    ("PREV_HIT", PREV_HIT, "arrow-circle-left"),
    ("NEXT_HIT", NEXT_HIT, "arrow-circle-right"),
    ("DOCKED", DOCKED, "push-pin-simple"),
    ("FLOATING", FLOATING, "push-pin"),
    ("SETTINGS", SETTINGS, "gear"),
    ("EDIT_SHORTCUT", EDIT_SHORTCUT, "pencil-simple"),
    ("TAB_BOOKMARKS", TAB_BOOKMARKS, "book-bookmark"),
    ("TAB_THUMBNAILS", TAB_THUMBNAILS, "article"),
    ("ADD", ADD, "plus-square"),
    ("REMOVE", REMOVE, "minus-square"),
    ("UNDO", UNDO, "arrow-arc-left"),
    ("REDO", REDO, "arrow-arc-right"),
    ("CLOSE", CLOSE, "x"),
    ("SCROLL_MODE", SCROLL_MODE, "scroll-mode"),
];

/// 우리가 그린 글리프 — Phosphor에 마땅한 것이 없었다(`assets/fonts/src/scroll-mode.svg`).
pub const SCROLL_MODE: char = '\u{f000}';

/// 툴바 아이콘의 글자 크기(pt).
///
/// **em 상자가 그대로 보이는 크기다.** 이 폰트는 글리프가 em을 꽉 채우도록 그려져 있어, 글자 크기
/// 그대로가 아이콘의 한 변이 된다. 22pt 버튼 안에 테두리 여백을 조금 남기는 값으로 잡았다.
pub const SIZE: f32 = 17.0;

/// 아이콘 한 글자를 그릴 `RichText`.
pub fn text(icon: char) -> egui::RichText {
    sized(icon, SIZE)
}

/// 아이콘 버튼 — **바탕을 칠하지 않고 마우스를 올렸을 때만 테두리**를 두르며, 글리프 좌우에
/// 여백을 두지 않는다(2026-10-03 요청).
///
/// 여백이 생기던 두 군데를 모두 막는다.
/// - `button_padding.x`: 버튼 테두리와 글자 사이.
/// - `interact_size.x`: egui 버튼의 **최소 폭**이고 기본값이 40이다. 아이콘 글리프는 17pt뿐이라
///   이 값이 양옆에 11pt씩을 밀어 넣고 있었다. 이쪽이 눈에 보이던 여백의 대부분이다.
pub fn button(ui: &mut egui::Ui, icon: char, tip: &str) -> egui::Response {
    flat(ui, icon, true, false).on_hover_text(tip)
}

/// 잠글 수 있는 아이콘 버튼.
pub fn button_enabled(ui: &mut egui::Ui, enabled: bool, icon: char, tip: &str) -> egui::Response {
    flat(ui, icon, enabled, false).on_hover_text(tip)
}

/// 켜고 끄는 아이콘 버튼. 켜져 있으면 테두리를 늘 두른다 — 바탕을 칠하지 않기로 했으니
/// "지금 켜져 있다"를 보일 길이 테두리뿐이다.
pub fn toggle(ui: &mut egui::Ui, selected: bool, icon: char, tip: &str) -> egui::Response {
    flat(ui, icon, true, selected).on_hover_text(tip)
}

fn flat(ui: &mut egui::Ui, icon: char, enabled: bool, selected: bool) -> egui::Response {
    ui.scope(|ui| {
        let visuals = ui.visuals();
        let line = visuals.widgets.inactive.fg_stroke.color.gamma_multiply(0.45);
        let strong = visuals.widgets.active.fg_stroke.color.gamma_multiply(0.7);

        ui.spacing_mut().button_padding.x = 0.0;
        ui.spacing_mut().interact_size.x = 0.0;

        let widgets = &mut ui.visuals_mut().widgets;
        for state in [&mut widgets.inactive, &mut widgets.hovered, &mut widgets.active, &mut widgets.open] {
            state.weak_bg_fill = egui::Color32::TRANSPARENT;
            state.bg_fill = egui::Color32::TRANSPARENT;
            state.bg_stroke = egui::Stroke::NONE;
        }
        let hover = egui::Stroke::new(1.0_f32, line);
        widgets.hovered.bg_stroke = hover;
        widgets.active.bg_stroke = hover;
        if selected {
            widgets.inactive.bg_stroke = egui::Stroke::new(1.0_f32, strong);
            widgets.hovered.bg_stroke = egui::Stroke::new(1.0_f32, strong);
        }
        ui.add_enabled(enabled, egui::Button::new(text(icon)))
    })
    .inner
}

/// 크기를 직접 정해 그릴 때.
pub fn sized(icon: char, size: f32) -> egui::RichText {
    egui::RichText::new(icon).font(egui::FontId::new(
        size,
        egui::FontFamily::Name(crate::fonts::ICON_FAMILY.into()),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **상수가 가리키는 아이콘이 뜻대로인지 확인한다.** 글리프가 있는지만 보면 코드포인트를 잘못
    /// 적어도 통과한다 — 실제로 `minus`를 U+E352로 적었다가 `number-circle-eight`가 나올 뻔했다
    /// (2026-10-03). 폰트의 글리프 이름은 `uniE230` 꼴이라 뜻을 알 수 없으므로, 빌드 스크립트가
    /// 함께 적어 둔 이름표를 본다.
    #[test]
    fn every_icon_points_at_the_intended_phosphor_glyph() {
        let table = include_str!("../../../assets/fonts/codepoints.txt");
        let map: std::collections::HashMap<&str, u32> = table
            .lines()
            .filter_map(|line| line.split_once('\t'))
            .filter_map(|(name, code)| u32::from_str_radix(code, 16).ok().map(|c| (name, c)))
            .collect();
        for (constant, icon, phosphor) in NAMED {
            let want = map
                .get(phosphor)
                .unwrap_or_else(|| panic!("{constant}: 이름표에 '{phosphor}'가 없다"));
            assert_eq!(
                *want, *icon as u32,
                "{constant}는 '{phosphor}'(U+{want:04X})여야 하는데 U+{:04X}를 가리킨다",
                *icon as u32
            );
        }
    }

    /// 글리프가 모두 폰트에 들어 있어야 한다. 하나라도 빠지면 그 자리에 빈 사각형이 뜬다.
    #[test]
    fn every_icon_exists_in_the_font() {
        let font = include_bytes!("../../../assets/fonts/Phosphor-Custom-Light.ttf");
        let face = ttf_parser::Face::parse(font, 0).expect("아이콘 폰트를 읽지 못했다");
        for (constant, icon, _) in NAMED {
            let id = face
                .glyph_index(*icon)
                .unwrap_or_else(|| panic!("{constant}(U+{:04X})가 폰트에 없다", *icon as u32));
            assert!(
                face.glyph_bounding_box(id).is_some(),
                "{constant}(U+{:04X})에 외곽선이 없다 — 빈 글리프다",
                *icon as u32
            );
        }
    }

    /// 아이콘은 폭이 em과 같다 — 그래서 글자 크기가 곧 아이콘의 한 변이고, 폭을 따로 재지 않아도
    /// 툴바 배치를 계산할 수 있다(`toolbar::icon_button_width`).
    #[test]
    fn every_icon_is_one_em_wide() {
        let font = include_bytes!("../../../assets/fonts/Phosphor-Custom-Light.ttf");
        let face = ttf_parser::Face::parse(font, 0).expect("아이콘 폰트를 읽지 못했다");
        let em = face.units_per_em();
        for (constant, icon, _) in NAMED {
            let id = face.glyph_index(*icon).unwrap();
            assert_eq!(face.glyph_hor_advance(id), Some(em), "{constant}의 폭이 em과 다르다");
        }
    }

    /// 두 아이콘이 같은 글리프를 가리키면 둘 중 하나는 뜻이 어긋난 것이다.
    #[test]
    fn no_two_icons_share_a_codepoint() {
        let unique: std::collections::BTreeSet<char> = NAMED.iter().map(|(_, icon, _)| *icon).collect();
        assert_eq!(unique.len(), NAMED.len(), "같은 코드포인트를 쓰는 아이콘이 있다");
    }
}
