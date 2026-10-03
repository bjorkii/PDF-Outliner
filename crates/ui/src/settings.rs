//! 사용자 설정 — 지금은 색 고르기와 로그 파일 열기뿐이다(2026-09-30 접수).
//!
//! **색은 자유롭게 고르게 하지 않고 미리 만든 계열 중에서 고르게 한다.** 자리마다 테두리·배경·
//! 글자색 셋이 필요한데, 색상환에서 아무 색이나 집게 하면 그 셋의 대비를 보장할 수 없다. 흰 글자가
//! 옅은 배경 위에 얹혀 읽히지 않는 조합이 바로 나온다. 계열 하나를 고르면 셋이 함께 정해진다.
//!
//! 저장은 eframe 저장소(`app_id`가 같은 `.ron`)에 JSON 한 줄로 한다. 값이 몇 개뿐이고 앱이 이미
//! 최근 파일 목록을 같은 방식으로 남기고 있어서, 따로 파일을 두지 않는다.

use serde::{Deserialize, Serialize};

/// 색 계열. 이름은 사용자에게 그대로 보인다.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Palette {
    Purple,
    Blue,
    Teal,
    Green,
    Amber,
    Red,
    Slate,
}

impl Palette {
    pub const ALL: [Palette; 7] =
        [Palette::Purple, Palette::Blue, Palette::Teal, Palette::Green, Palette::Amber, Palette::Red, Palette::Slate];

    pub fn label(self) -> &'static str {
        match self {
            Palette::Purple => "보라",
            Palette::Blue => "파랑",
            Palette::Teal => "청록",
            Palette::Green => "초록",
            Palette::Amber => "주황",
            Palette::Red => "빨강",
            Palette::Slate => "회청",
        }
    }

    /// 테두리와 진한 배경에 쓰는 대표색. 흰 글자를 얹어도 읽히는 진하기로 골랐다.
    pub fn stroke(self) -> egui::Color32 {
        match self {
            Palette::Purple => egui::Color32::from_rgb(0x69, 0x17, 0x8a),
            Palette::Blue => egui::Color32::from_rgb(0x1f, 0x6f, 0xd0),
            Palette::Teal => egui::Color32::from_rgb(0x0e, 0x82, 0x8a),
            Palette::Green => egui::Color32::from_rgb(0x1b, 0x9e, 0x5f),
            Palette::Amber => egui::Color32::from_rgb(0xc4, 0x7d, 0x0b),
            Palette::Red => egui::Color32::from_rgb(0xc0, 0x39, 0x2b),
            Palette::Slate => egui::Color32::from_rgb(0x4a, 0x5b, 0x6b),
        }
    }

    /// 옅은 배경(상자 안을 채울 때). 대표색을 그대로 옅게 깐다.
    pub fn fill(self) -> egui::Color32 {
        self.stroke().gamma_multiply(0.12)
    }

    /// 진한 배경 위에 얹을 글자색. 대표색이 모두 진해서 흰 글자로 통일한다.
    pub fn on_stroke(self) -> egui::Color32 {
        egui::Color32::WHITE
    }
}

/// 색을 정할 수 있는 자리들.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Colors {
    /// 북마크 사이드바·뷰어·검색 결과 패널의 포커스 테두리.
    pub focus_border: Palette,
    /// 사이드바에서 고른 북마크의 배경.
    pub bookmark_selection: Palette,
    /// OCR 텍스트 상자(표시 모드).
    pub ocr_box: Palette,
    /// OCR이 아닌, 그 밖의 보이지 않는 텍스트 상자.
    pub other_box: Palette,
    /// 지금 가리키거나 고른 상자.
    pub focus_box: Palette,
    /// 썸네일 탭에서 지금 보고 있는 쪽을 두르는 테두리.
    ///
    /// 북마크 선택과 따로 두는 이유: macOS 미리보기가 쓰는 파란 테두리를 기본으로 하고 싶은데,
    /// 북마크 선택은 보라가 기본이다. `serde(default)`를 붙여 두어야 이 항목이 없던 시절에 저장된
    /// 설정을 읽을 때 나머지 색까지 초기화되지 않는다.
    #[serde(default = "default_thumbnail_selection")]
    pub thumbnail_selection: Palette,
}

fn default_thumbnail_selection() -> Palette {
    Palette::Blue
}

impl Default for Colors {
    fn default() -> Self {
        Self {
            focus_border: Palette::Purple,
            bookmark_selection: Palette::Purple,
            ocr_box: Palette::Blue,
            other_box: Palette::Red,
            focus_box: Palette::Green,
            thumbnail_selection: default_thumbnail_selection(),
        }
    }
}

/// 한 줄에 하나씩 보여 줄 항목 — 이름, 설명, 그리고 그 자리의 계열을 꺼내는 길.
pub struct ColorRow {
    pub label: &'static str,
    pub hint: &'static str,
    pub pick: fn(&mut Colors) -> &mut Palette,
}

pub const COLOR_ROWS: [ColorRow; 6] = [
    ColorRow {
        label: "북마크/뷰어 포커스 테두리",
        hint: "지금 키보드 입력을 받는 영역을 둘러싸는 테두리입니다.",
        pick: |c| &mut c.focus_border,
    },
    ColorRow {
        label: "북마크 선택",
        hint: "사이드바에서 고른 북마크의 배경입니다.",
        pick: |c| &mut c.bookmark_selection,
    },
    ColorRow {
        label: "썸네일 선택",
        hint: "썸네일 탭에서 지금 보고 있는 쪽을 두르는 테두리입니다.",
        pick: |c| &mut c.thumbnail_selection,
    },
    ColorRow {
        label: "OCR 텍스트 박스",
        hint: "OCR 표시 모드에서 OCR 텍스트를 감싸는 상자입니다.",
        pick: |c| &mut c.ocr_box,
    },
    ColorRow {
        label: "OCR 외 텍스트 박스",
        hint: "'보이지 않는 텍스트 모두 포함'을 켰을 때, OCR이 아닌 텍스트를 감싸는 상자입니다.",
        pick: |c| &mut c.other_box,
    },
    ColorRow {
        label: "현재 주목하는 텍스트 박스",
        hint: "마우스를 올렸거나 눌러서 고른 상자입니다.",
        pick: |c| &mut c.focus_box,
    },
];

/// 창 안을 스크롤하는 영역의 높이 상한. 버튼 줄은 늘 그 아래에 보인다.
const BODY_MAX: f32 = 420.0;

/// 로그 보기 탭에 싣는 줄 수. 추적에 필요한 것은 늘 끝부분이다.
const LOG_TAIL_LINES: usize = 200;

/// 설정 창에서 보고 있는 탭.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tab {
    #[default]
    Colors,
    Shortcuts,
    Log,
}

/// 단축키를 고치는 중인 기능. 그 줄의 칸이 입력을 기다리는 상태가 된다.
#[derive(Debug, Clone, Default)]
pub struct ShortcutEditor {
    pub editing: Option<crate::shortcuts::Action>,
    /// 방금 받아들이지 못한 조합과 그 이유 — 고치는 줄 아래에 띄운다.
    pub rejected: Option<(crate::shortcuts::Action, crate::shortcuts::Conflict)>,
}

/// 설정 창. 다른 기능 창과 같은 규칙이다 — 끌어서 옮길 수 있고, Esc로 닫히고, 본문에 여백이 있다.
///
/// **아래 버튼 줄은 스크롤과 무관하게 고정한다**(2026-09-30 요청). 항목이 늘어 본문이 창을 넘치면
/// 그 줄이 화면 밖으로 밀려 누를 수 없게 된다 — 북마크 폴더 일괄 화면에서 같은 일이 있었다.
pub fn show(ctx: &egui::Context, app: &mut crate::app::PdfViewerApp) {
    if !app.settings_open {
        return;
    }
    // 단축키를 받는 중에는 Esc도 그 입력의 하나다 — 창을 닫지 않고 고치기만 그만둔다.
    if crate::app::escape_to_close(ctx) {
        // 고치는 중이면 그 입력만 그만둔다. 여기서 `return`하면 창이 한 프레임 사라진다.
        if app.settings_editor.editing.take().is_some() {
            app.settings_editor.rejected = None;
        } else {
            app.settings_open = false;
            return;
        }
    }
    let mut close = false;
    let mut open_logs = false;
    egui::Window::new("설정")
        .collapsible(false)
        .resizable(true)
        .default_width(520.0)
        .pivot(egui::Align2::CENTER_CENTER)
        .default_pos(ctx.screen_rect().center())
        .show(ctx, |ui| {
            crate::app::window_body(ui, |ui| {
                crate::tabs::bar(
                    ui,
                    "settings_tabs",
                    &mut app.settings_tab,
                    &[
                        (Tab::Colors, crate::icons::TAB_COLORS, "색상 지정"),
                        (Tab::Shortcuts, crate::icons::TAB_SHORTCUTS, "단축키"),
                        (Tab::Log, crate::icons::TAB_LOG, "로그 보기"),
                    ],
                );
                ui.add_space(10.0);
                egui::ScrollArea::vertical().auto_shrink([false, true]).max_height(BODY_MAX).show(ui, |ui| {
                    match app.settings_tab {
                        Tab::Colors => color_tab(ui, &mut app.colors),
                        Tab::Shortcuts => shortcut_tab(ui, &mut app.shortcuts, &mut app.settings_editor),
                        Tab::Log => log_tab(ui),
                    }
                });
                ui.add_space(10.0);
                ui.separator();
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    // 탭마다 그 탭에서만 뜻이 있는 버튼을 왼쪽에 둔다.
                    match app.settings_tab {
                        Tab::Shortcuts => {
                            if ui.button("단축키 초기화").clicked() {
                                app.shortcuts.reset_all();
                                app.settings_editor = ShortcutEditor::default();
                            }
                        }
                        Tab::Log => {
                            if ui.button("로그파일 위치 열기").clicked() {
                                open_logs = true;
                            }
                        }
                        Tab::Colors => {}
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("닫기").clicked() {
                            close = true;
                        }
                    });
                });
            });
        });

    if open_logs {
        match crate::crash_log::log_path() {
            Some(path) => {
                if let Err(err) = crate::app::reveal_in_file_manager(&path) {
                    app.status_message = Some(format!("해당 위치를 열 수 없습니다({err}): {}", path.display()));
                }
            }
            None => app.status_message = Some("로그 파일 위치를 알 수 없습니다.".to_string()),
        }
    }
    if close {
        app.settings_open = false;
        app.settings_editor = ShortcutEditor::default();
    }
}

fn color_tab(ui: &mut egui::Ui, colors: &mut Colors) {
    for (index, row) in COLOR_ROWS.iter().enumerate() {
        if index > 0 {
            ui.add_space(10.0);
        }
        color_row(ui, colors, row);
    }
}

/// 단축키 탭 — 갈래마다 기능 / 단축키 / 수정 세 칸.
fn shortcut_tab(
    ui: &mut egui::Ui,
    shortcuts: &mut crate::shortcuts::Shortcuts,
    editor: &mut ShortcutEditor,
) {
    use crate::shortcuts::{Action, Category};

    // 고치는 중이면 이번 프레임에 들어온 키를 먼저 집어 든다. 그려 놓고 받으면 한 프레임 늦는다.
    if let Some(action) = editor.editing {
        if let Some(binding) = pressed_combination(ui.ctx()) {
            match shortcuts.set(action, binding) {
                Ok(()) => {
                    editor.editing = None;
                    editor.rejected = None;
                }
                // **고치는 상태를 그대로 둔다** — 바로 다른 조합을 눌러 볼 수 있어야 한다
                // (2026-10-03 요청). 받아들여지면 그때 안내가 사라진다.
                Err(conflict) => editor.rejected = Some((action, conflict)),
            }
        }
    }

    for (index, category) in Category::ALL.iter().enumerate() {
        if index > 0 {
            ui.add_space(14.0);
        }
        ui.label(egui::RichText::new(category.label()).strong());
        ui.add_space(4.0);
        egui::Frame::none()
            .fill(ui.visuals().faint_bg_color)
            .rounding(6.0)
            .stroke(egui::Stroke::new(1.0_f32, ui.visuals().widgets.noninteractive.bg_stroke.color))
            .inner_margin(egui::Margin::symmetric(12.0, 9.0))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                egui::Grid::new(("shortcut_rows", index)).num_columns(3).spacing([14.0, 7.0]).show(ui, |ui| {
                    for action in Action::ALL.iter().filter(|a| a.category() == *category) {
                        shortcut_row(ui, *action, shortcuts, editor);
                    }
                });
            });
    }
}

fn shortcut_row(
    ui: &mut egui::Ui,
    action: crate::shortcuts::Action,
    shortcuts: &mut crate::shortcuts::Shortcuts,
    editor: &mut ShortcutEditor,
) {
    ui.label(action.label());

    let editing = editor.editing == Some(action);
    if editing {
        ui.colored_label(ui.visuals().strong_text_color(), "새 단축키를 누르세요…");
    } else if let Some(fixed) = action.fixed_display() {
        ui.label(egui::RichText::new(fixed).monospace());
    } else {
        let text = egui::RichText::new(shortcuts.get(action).display()).monospace();
        let text = if shortcuts.is_default(action) { text } else { text.strong() };
        ui.label(text);
    }

    ui.horizontal(|ui| {
        // 고정된 기능은 수정 아이콘조차 보이지 않는다(md 확정).
        if action.changeable() {
            let tip = if editing { "그만두기 (Esc)" } else { "단축키 바꾸기" };
            let icon = if editing { crate::icons::CLOSE } else { crate::icons::EDIT_SHORTCUT };
            if crate::icons::button(ui, icon, tip).clicked() {
                editor.editing = (!editing).then_some(action);
                editor.rejected = None;
            }
            // 되돌리기는 **고친 줄에만** 보인다 — 늘 두면 줄마다 아이콘이 둘씩 늘어선다.
            if !shortcuts.is_default(action)
                && crate::icons::button(ui, crate::icons::UNDO, "기본값으로 되돌리기").clicked()
            {
                shortcuts.reset(action);
                editor.editing = None;
                editor.rejected = None;
            }
        }
    });
    ui.end_row();

    if let Some((rejected, conflict)) = editor.rejected {
        if rejected == action {
            ui.label("");
            ui.colored_label(ui.visuals().error_fg_color, conflict.message());
            ui.label("");
            ui.end_row();
        }
    }
}

/// 로그 보기 탭 — 패닉 기록과 작업 진단이 함께 쌓이는 파일(`crash_log`)의 끝부분을 보여 준다.
fn log_tab(ui: &mut egui::Ui) {
    let Some(path) = crate::crash_log::log_path() else {
        ui.label("로그 파일 위치를 알 수 없습니다.");
        return;
    };
    match std::fs::read_to_string(&path) {
        Ok(text) if !text.trim().is_empty() => {
            // 끝에서부터 보여 준다 — 방금 일어난 일이 맨 아래에 있다.
            let tail: Vec<&str> = text.lines().rev().take(LOG_TAIL_LINES).collect();
            let shown: String = tail.into_iter().rev().collect::<Vec<_>>().join("\n");
            ui.label(
                egui::RichText::new(format!("{} (마지막 {LOG_TAIL_LINES}줄까지)", path.display()))
                    .color(ui.visuals().weak_text_color())
                    .small(),
            );
            ui.add_space(4.0);
            egui::Frame::none()
                .fill(ui.visuals().extreme_bg_color)
                .inner_margin(egui::Margin::same(8.0))
                .rounding(4.0)
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.add(egui::Label::new(egui::RichText::new(shown).monospace().small()).wrap());
                });
        }
        Ok(_) => {
            ui.label("아직 기록된 것이 없습니다.");
        }
        Err(err) => {
            ui.label(format!("로그 파일을 읽지 못했습니다: {err}"));
        }
    }
}

/// 이번 프레임에 눌린 조합. 보조키만 누른 것은 아직 조합이 아니다.
fn pressed_combination(ctx: &egui::Context) -> Option<crate::shortcuts::Binding> {
    ctx.input(|i| {
        i.events.iter().find_map(|event| match event {
            egui::Event::Key { key, pressed: true, modifiers, repeat: false, .. } => {
                Some(crate::shortcuts::Binding {
                    command: modifiers.command,
                    shift: modifiers.shift,
                    alt: modifiers.alt,
                    key: *key,
                })
            }
            _ => None,
        })
    })
}

/// 항목 한 줄: 이름 + 계열 단추들 + 지금 색이 어떻게 보이는지.
fn color_row(ui: &mut egui::Ui, colors: &mut Colors, row: &ColorRow) {
    ui.label(row.label).on_hover_text(row.hint);
    ui.horizontal_wrapped(|ui| {
        let current = *(row.pick)(colors);
        for palette in Palette::ALL {
            // 견본은 글로 쓰지 않고 색 자체를 보여 준다. 고른 것에는 테두리를 둘러 표시한다.
            let (rect, response) = ui.allocate_exact_size(egui::vec2(28.0, 22.0), egui::Sense::click());
            let visible = ui.is_rect_visible(rect);
            if visible {
                ui.painter().rect_filled(rect, 3.0, palette.stroke());
                if palette == current {
                    ui.painter().rect_stroke(rect.expand(2.0), 4.0, egui::Stroke::new(2.0_f32, ui.visuals().text_color()));
                }
            }
            let response = response.on_hover_text(palette.label());
            if response.clicked() {
                *(row.pick)(colors) = palette;
            }
        }
    });
    // 고른 계열이 실제로 어떻게 보이는지 — 테두리·배경·글자색 셋을 한 번에 보여 준다.
    let palette = *(row.pick)(colors);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width().min(240.0), 26.0), egui::Sense::hover());
    if ui.is_rect_visible(rect) {
        ui.painter().rect_filled(rect, 2.0, palette.fill());
        ui.painter().rect_stroke(rect, 2.0, egui::Stroke::new(1.5_f32, palette.stroke()));
        let inner = egui::Rect::from_min_size(rect.min + egui::vec2(6.0, 4.0), egui::vec2(66.0, 18.0));
        ui.painter().rect_filled(inner, 2.0, palette.stroke());
        ui.painter().text(
            inner.center(),
            egui::Align2::CENTER_CENTER,
            palette.label(),
            egui::FontId::proportional(12.0),
            palette.on_stroke(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 계열마다 테두리와 배경이 다르고, 배경은 테두리보다 옅어야 한다(글자가 읽히도록).
    #[test]
    fn every_palette_has_a_lighter_fill_than_its_stroke() {
        for palette in Palette::ALL {
            let (stroke, fill) = (palette.stroke(), palette.fill());
            assert!(fill.a() < stroke.a() || fill != stroke, "{}: 배경이 테두리와 같다", palette.label());
        }
    }

    /// 항목마다 가리키는 자리가 달라야 한다 — 같은 자리를 두 줄이 건드리면 설정이 서로 덮는다.
    #[test]
    fn each_row_points_at_a_different_field() {
        let mut colors = Colors::default();
        for (index, row) in COLOR_ROWS.iter().enumerate() {
            // 이 줄만 회청으로 바꾼 뒤, 다른 줄이 따라 바뀌지 않는지 본다.
            let mut probe = colors;
            *(row.pick)(&mut probe) = Palette::Slate;
            let changed = COLOR_ROWS
                .iter()
                .enumerate()
                .filter(|(other, _)| {
                    let (mut a, mut b) = (colors, probe);
                    (COLOR_ROWS[*other].pick)(&mut a) != (COLOR_ROWS[*other].pick)(&mut b)
                })
                .count();
            assert_eq!(changed, 1, "{}번째 줄이 다른 자리까지 바꾼다", index);
            *(row.pick)(&mut colors) = Palette::Slate;
        }
    }
}
