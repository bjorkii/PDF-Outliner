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

const DEFAULT_WIDTH: f32 = 560.0;
const DEFAULT_HEIGHT: f32 = 520.0;
/// 창을 아무리 줄여도 본문은 이만큼은 보인다.
const BODY_MIN: f32 = 160.0;
/// 창 크기 조정의 하한(제목 띠와 창 여백을 뺀 안쪽 크기). **내용이 이보다 크면 안 된다** — 그러면
/// 창이 내용에 밀려 하한보다 커지고, 왼쪽·위쪽 변을 붙잡는 계산(`EdgePin`)이 어긋난다.
/// `layout_tests`가 탭마다 확인한다.
const MIN_WIDTH: f32 = 430.0;
const MIN_HEIGHT: f32 = 300.0;
/// 본문과 구분선 사이.
const GAP_ABOVE_LINE: f32 = 10.0;

/// 창의 id. 크기 조정 손잡이의 id가 여기서 나온다(`dragged_edges`).
const WINDOW_ID: &str = "settings_window";

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

/// 왼쪽·위쪽 변으로 창을 줄일 때 **반대쪽 변을 붙잡아 둔다**(2026-10-03 리포트).
///
/// egui는 왼쪽 변을 끌면 창의 새 왼쪽을 포인터 자리로 바로 잡고(`window.rs::move_and_resize_window`),
/// 크기는 따로 최소값으로 자른다. 그래서 최소 크기에 닿은 뒤에도 계속 끌면 왼쪽이 포인터를 따라가고
/// 오른쪽 변이 그만큼 밀려 창 전체가 움직였다. 운영체제 창이라면 창 관리자가 막아 주지만, 이 창은
/// 앱 안에 egui가 그리는 창이라 그런 도움이 없다.
///
/// 막는 자리는 egui에 들어가기 **전의 입력**이다. 그 변을 끄는 동안 포인터 좌표를 "반대쪽 변 −
/// 최소 크기"보다 넘어가지 않게 묶어 두면, egui가 같은 계산을 해도 창이 최소 크기에서 멈춘다.
/// 묶는 것은 egui가 받는 좌표뿐이고 화면의 마우스 커서는 그대로 움직인다.
#[derive(Debug, Clone, Copy, Default)]
pub struct EdgePin {
    /// 지난 프레임에 그린 창의 바깥 테두리.
    outer: Option<egui::Rect>,
    /// 바깥 크기와 크기 조정 대상(안쪽) 크기의 차 — 제목 띠와 창 여백.
    margin: egui::Vec2,
    /// 끌기 시작 직전의 오른쪽 아래 꼭짓점. 끄는 동안 이 자리를 지킨다.
    anchor: Option<egui::Pos2>,
}

impl EdgePin {
    /// 왼쪽·위쪽 변을 끄는 중이면 이번 프레임의 포인터 좌표를 최소 크기 경계 안으로 묶는다.
    pub fn clamp_pointer(&self, ctx: &egui::Context, raw: &mut egui::RawInput) {
        let (left, top) = dragged_edges(ctx);
        let Some(anchor) = self.anchor.filter(|_| left || top) else {
            return;
        };
        let limit = anchor - (egui::vec2(MIN_WIDTH, MIN_HEIGHT) + self.margin);
        for event in &mut raw.events {
            let pos = match event {
                egui::Event::PointerMoved(pos) | egui::Event::PointerButton { pos, .. } => pos,
                _ => continue,
            };
            if left {
                pos.x = pos.x.min(limit.x);
            }
            if top {
                pos.y = pos.y.min(limit.y);
            }
        }
    }

    /// 창을 그린 뒤에 부른다. `before`는 이번 프레임에 그리기 전의 바깥 테두리다.
    fn record(&mut self, ctx: &egui::Context, before: Option<egui::Rect>, outer: egui::Rect, inner: egui::Vec2) {
        let (left, top) = dragged_edges(ctx);
        if left || top {
            // 끌기가 시작된 프레임에는 egui가 이미 창을 옮겨 놓았을 수 있다 — 그리기 전 자리를 쓴다.
            if self.anchor.is_none() {
                self.anchor = Some(before.unwrap_or(outer).max);
            }
        } else {
            self.anchor = None;
        }
        self.outer = Some(outer);
        self.margin = outer.size() - inner;
    }
}

/// 설정 창의 왼쪽 변과 위쪽 변 가운데 지금 끌리고 있는 것. 꼭짓점은 두 변에 함께 속한다.
///
/// 손잡이 id는 egui가 정한 규칙을 그대로 따른다(`window.rs::resize_interaction`:
/// `Id::new(layer_id).with("edge_drag").with(이름)`). egui를 올리면 이 규칙이 바뀌었는지
/// `layout_tests::left_and_top_edges_keep_the_opposite_edge`가 잡아낸다.
fn dragged_edges(ctx: &egui::Context) -> (bool, bool) {
    let Some(dragged) = ctx.dragged_id() else {
        return (false, false);
    };
    let layer = egui::LayerId::new(egui::Order::Middle, egui::Id::new(WINDOW_ID));
    let base = egui::Id::new(layer).with("edge_drag");
    let any = |names: [&str; 3]| names.iter().any(|name| dragged == base.with(*name));
    (any(["left", "left_top", "left_bottom"]), any(["top", "left_top", "right_top"]))
}

/// 창을 그린 결과 — 누른 버튼과, 시험에서 재어 볼 자리.
#[derive(Debug, Clone, Copy)]
struct Outcome {
    close: bool,
    open_logs: bool,
    /// 구분선의 y.
    line_y: f32,
    /// 아래 버튼 줄의 자리.
    row: egui::Rect,
    /// 창 바깥 테두리.
    outer: egui::Rect,
}

impl Default for Outcome {
    fn default() -> Self {
        Self { close: false, open_logs: false, line_y: 0.0, row: egui::Rect::NOTHING, outer: egui::Rect::NOTHING }
    }
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
    let outcome = window(
        ctx,
        &mut app.settings_tab,
        &mut app.colors,
        &mut app.shortcuts,
        &mut app.settings_editor,
        &mut app.settings_pin,
    );

    if outcome.open_logs {
        match crate::crash_log::log_path() {
            Some(path) => {
                if let Err(err) = crate::app::reveal_in_file_manager(&path) {
                    app.status_message = Some(format!("해당 위치를 열 수 없습니다({err}): {}", path.display()));
                }
            }
            None => app.status_message = Some("로그 파일 위치를 알 수 없습니다.".to_string()),
        }
    }
    if outcome.close {
        app.settings_open = false;
        app.settings_editor = ShortcutEditor::default();
    }
}

/// 창 자체. 앱 전체 없이 시험할 수 있게 쓰는 것만 받는다.
fn window(
    ctx: &egui::Context,
    tab: &mut Tab,
    colors: &mut Colors,
    shortcuts: &mut crate::shortcuts::Shortcuts,
    editor: &mut ShortcutEditor,
    pin: &mut EdgePin,
) -> Outcome {
    let mut outcome = Outcome::default();
    let mut inner = egui::Vec2::ZERO;
    let shown = egui::Window::new("설정")
        .id(egui::Id::new(WINDOW_ID))
        .collapsible(false)
        .resizable(true)
        .default_width(DEFAULT_WIDTH)
        // 최소 크기에 닿은 뒤 왼쪽·위쪽 변을 더 끌어도 창이 밀리지 않게 하는 것은 `EdgePin`이 맡는다.
        .min_width(MIN_WIDTH)
        .min_height(MIN_HEIGHT)
        // **pivot을 쓰지 않는다.** `CENTER_CENTER`로 두면 창이 늘 가운데를 축으로 커지고 줄어들어,
        // 왼쪽 변을 끌면 오른쪽 변까지 따라 움직이고 위쪽 변은 아예 잡히지 않는다(2026-10-03
        // 리포트). 기본 축(왼쪽 위)으로 두고, 처음 뜰 자리만 가운데로 계산해 준다.
        .default_pos(ctx.screen_rect().center() - egui::vec2(DEFAULT_WIDTH, DEFAULT_HEIGHT) / 2.0)
        .default_height(DEFAULT_HEIGHT)
        .show(ctx, |ui| {
            inner = ui.max_rect().size();
            crate::app::window_body(ui, |ui| {
                crate::tabs::bar(
                    ui,
                    "settings_tabs",
                    tab,
                    &[
                        (Tab::Colors, crate::icons::TAB_COLORS, "색상 지정"),
                        (Tab::Shortcuts, crate::icons::TAB_SHORTCUTS, "단축키"),
                        (Tab::Log, crate::icons::TAB_LOG, "로그 보기"),
                    ],
                );
                ui.add_space(10.0);
                // **탭 아래 남은 자리를 한 번에 정확히 잡고 나눈다.**
                //
                // 내용에 맡기면 창이 내용을 따라 자란다. egui의 `Resize`는 요청받은 크기보다 내용이
                // 크면 그만큼 커지고 그 크기를 기억하므로, 긴 탭(로그)을 한 번 열면 창이 화면 높이까지
                // 늘어나 아래 버튼 줄이 잘리고, 탭을 바꿔도 그 크기가 남았다(2026-10-03 리포트).
                // 자리를 먼저 못박고 그 안에서만 그리면 내용이 창을 밀지 못한다. 창을 세로로 늘리면
                // 본문만 함께 늘어난다.
                //
                // **버튼 줄은 구분선과 창 아래 테두리 사이 한가운데에 둔다**(2026-10-03 요청). 그 아래로는
                // 본문 여백과 창 여백이 이미 있으므로, 구분선에서 버튼까지도 같은 만큼 띄운다. 그러면
                // 버튼 줄의 아래 끝이 정확히 본문 칸의 아래 끝에 닿는다.
                let below = crate::app::BODY_PADDING + ui.style().spacing.window_margin.bottom;
                let row_height = button_height(ui);
                let width = ui.available_width();
                let rest_height =
                    ui.available_height().max(BODY_MIN + GAP_ABOVE_LINE + below + row_height);
                let (rest, _) = ui.allocate_exact_size(egui::vec2(width, rest_height), egui::Sense::hover());
                let row = egui::Rect::from_min_max(
                    egui::pos2(rest.left(), rest.bottom() - row_height),
                    rest.right_bottom(),
                );
                let line_y = row.top() - below;
                let body_rect = egui::Rect::from_min_max(rest.left_top(), egui::pos2(rest.right(), line_y - GAP_ABOVE_LINE));

                let mut body = ui.new_child(
                    egui::UiBuilder::new().max_rect(body_rect).layout(egui::Layout::top_down(egui::Align::Min)),
                );
                // **스크롤은 탭마다 맡는다.** 로그 탭은 머리글(경로)을 스크롤 밖에 두어야 내려도
                // 자리를 지킨다(2026-10-03 요청).
                match *tab {
                    Tab::Colors => scrolled(&mut body, |ui| color_tab(ui, colors)),
                    Tab::Shortcuts => scrolled(&mut body, |ui| shortcut_tab(ui, shortcuts, editor)),
                    Tab::Log => log_tab(&mut body),
                }

                ui.painter().hline(rest.x_range(), line_y, ui.visuals().widgets.noninteractive.bg_stroke);
                outcome.line_y = line_y;
                outcome.row = row;

                let mut row_ui = ui.new_child(
                    egui::UiBuilder::new().max_rect(row).layout(egui::Layout::left_to_right(egui::Align::Center)),
                );
                // 탭마다 그 탭에서만 뜻이 있는 버튼을 왼쪽에 둔다.
                match *tab {
                    Tab::Colors => {
                        if row_ui.button("색상 초기화").clicked() {
                            *colors = Colors::default();
                        }
                    }
                    Tab::Shortcuts => {
                        if row_ui.button("단축키 초기화").clicked() {
                            shortcuts.reset_all();
                            *editor = ShortcutEditor::default();
                        }
                    }
                    Tab::Log => {
                        if row_ui.button("로그파일 위치 열기").clicked() {
                            outcome.open_logs = true;
                        }
                    }
                }
                row_ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("닫기").clicked() {
                        outcome.close = true;
                    }
                });
            });
        });
    if let Some(shown) = shown {
        outcome.outer = shown.response.rect;
        pin.record(ctx, pin.outer, outcome.outer, inner);
    }
    outcome
}

/// 글자 버튼 하나의 높이 — 글자 높이에 위아래 안쪽 여백을 더한 것과 최소 높이 중 큰 쪽(`Button`이 정하는 대로).
fn button_height(ui: &egui::Ui) -> f32 {
    let font = egui::TextStyle::Button.resolve(ui.style());
    let text = ui.fonts(|fonts| fonts.row_height(&font));
    (text + 2.0 * ui.spacing().button_padding.y).max(ui.spacing().interact_size.y)
}

/// 본문을 세로 스크롤 영역에 담는다. **잡아 둔 자리를 그대로 채운다** — 줄어들게 두면 내용이 짧은
/// 탭으로 바꿀 때 창이 따라 줄었다가 다시 늘어난다.
fn scrolled(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, add);
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

    // **갈래마다 표를 따로 그리면 칸 너비가 제각각이 된다**(2026-10-03 리포트). 가장 긴 이름과 가장
    // 긴 단축키를 미리 재어 모든 갈래가 같은 너비를 쓰게 한다.
    let widths = ColumnWidths::measure(ui, shortcuts);

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
                        shortcut_row(ui, *action, shortcuts, editor, widths);
                    }
                });
            });
    }
}

/// 갈래가 달라도 칸이 어긋나지 않게, 가장 긴 것을 미리 재어 둔 너비.
#[derive(Debug, Clone, Copy)]
struct ColumnWidths {
    label: f32,
    binding: f32,
    /// 수정·되돌리기 아이콘이 들어갈 자리. **고정된 기능에도 같은 너비를 잡아 둔다** — 그러지 않으면
    /// 아이콘이 하나도 없는 갈래(일반)만 더 좁아져, 창을 줄일 때 그 상자만 따로 줄어든다
    /// (2026-10-03 리포트).
    actions: f32,
}

impl ColumnWidths {
    fn measure(ui: &egui::Ui, shortcuts: &crate::shortcuts::Shortcuts) -> Self {
        use crate::shortcuts::Action;
        let body = egui::TextStyle::Body.resolve(ui.style());
        let mono = egui::TextStyle::Monospace.resolve(ui.style());
        let width = |text: String, font: egui::FontId| {
            ui.fonts(|fonts| fonts.layout_no_wrap(text, font, egui::Color32::WHITE).size().x)
        };
        let label = Action::ALL
            .iter()
            .map(|action| width(action.label().to_string(), body.clone()))
            .fold(0.0_f32, f32::max);
        let binding = Action::ALL
            .iter()
            .map(|action| {
                let text = action.fixed_display().map_or_else(
                    || shortcuts.get(*action).display(),
                    |fixed| fixed.to_string(),
                );
                width(text, mono.clone())
            })
            .fold(0.0_f32, f32::max)
            // 고치는 중에 뜨는 안내 문구가 더 길다 — 그때 칸이 넓어졌다 좁아지지 않게 함께 잰다.
            .max(width("새 단축키를 누르세요…".to_string(), body));
        let actions = crate::icons::SIZE * 2.0 + ui.spacing().item_spacing.x;
        Self { label, binding, actions }
    }
}

fn shortcut_row(
    ui: &mut egui::Ui,
    action: crate::shortcuts::Action,
    shortcuts: &mut crate::shortcuts::Shortcuts,
    editor: &mut ShortcutEditor,
    widths: ColumnWidths,
) {
    let row = ui.spacing().interact_size.y;
    // **칸을 정확히 그 폭으로 잡는다.** `allocate_ui_with_layout`은 요청한 크기가 아니라 **내용
    // 크기만큼** 자리를 잡아서(`Ui::allocate_ui_with_layout_dyn`가 `child.min_rect()`를 할당한다),
    // 아이콘이 없는 갈래만 좁아졌다(실측: 일반 186 대 나머지 259). `allocate_exact_size`로 자리를
    // 먼저 못박고 그 안에 왼쪽으로 쌓는다 — `add_sized`나 `put`은 위젯을 가운데에 놓는다.
    let cell_at = |ui: &mut egui::Ui, width: f32| {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(width, row), egui::Sense::hover());
        ui.new_child(
            egui::UiBuilder::new().max_rect(rect).layout(egui::Layout::left_to_right(egui::Align::Center)),
        )
    };
    cell_at(ui, widths.label).label(action.label());

    let editing = editor.editing == Some(action);
    let cell = |ui: &mut egui::Ui, text: egui::RichText| {
        cell_at(ui, widths.binding).label(text);
    };
    if editing {
        cell(ui, egui::RichText::new("새 단축키를 누르세요…").color(ui.visuals().strong_text_color()));
    } else if let Some(fixed) = action.fixed_display() {
        cell(ui, egui::RichText::new(fixed).monospace());
    } else {
        let text = egui::RichText::new(shortcuts.get(action).display()).monospace();
        let text = if shortcuts.is_default(action) { text } else { text.strong() };
        cell(ui, text);
    }

    {
        // 고정된 기능은 수정 아이콘조차 보이지 않는다(md 확정). 자리는 그대로 잡아 둔다.
        let ui = &mut cell_at(ui, widths.actions);
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
    }
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
///
/// **머리글(경로)은 스크롤 밖에 둔다** — 내려도 어느 파일을 보고 있는지 자리를 지켜야 한다.
fn log_tab(ui: &mut egui::Ui) {
    let Some(path) = crate::crash_log::log_path() else {
        ui.label("로그 파일 위치를 알 수 없습니다.");
        return;
    };
    let text = match std::fs::read_to_string(&path) {
        Ok(text) if !text.trim().is_empty() => text,
        Ok(_) => {
            ui.label("아직 기록된 것이 없습니다.");
            return;
        }
        Err(err) => {
            ui.label(format!("로그 파일을 읽지 못했습니다: {err}"));
            return;
        }
    };
    ui.label(egui::RichText::new(format!("{} (마지막 {LOG_TAIL_LINES}줄)", path.display())));
    ui.add_space(4.0);

    // 끝에서부터 보여 준다 — 방금 일어난 일이 맨 아래에 있다.
    let tail: Vec<&str> = text.lines().rev().take(LOG_TAIL_LINES).collect();
    let shown: String = tail.into_iter().rev().collect::<Vec<_>>().join("\n");
    egui::Frame::none()
        .fill(ui.visuals().extreme_bg_color)
        .inner_margin(egui::Margin::same(8.0))
        .rounding(4.0)
        .show(ui, |ui| {
            // 머리글을 뺀 나머지를 그대로 쓴다 — 바깥에서 이미 높이를 못박아 두었다.
            ui.set_min_size(ui.available_size());
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                // 글자 크기는 툴바와 같게 — 로그는 읽으라고 띄우는 것이다.
                let size = egui::TextStyle::Button.resolve(ui.style()).size;
                ui.add(egui::Label::new(egui::RichText::new(shown).monospace().size(size)).wrap());
            });
        });
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

/// 설정 창의 자리 잡기(2026-10-03 리포트).
#[cfg(test)]
mod layout_tests {
    use crate::shortcuts::{Action, Category, Shortcuts};

    /// 갈래마다 표를 따로 그려도 **칸 너비가 같아야** 한다. 전에는 수정 아이콘이 없는 '일반'만
    /// 좁았다(실측 186 대 259) — `allocate_ui_with_layout`이 요청한 폭이 아니라 내용 폭만큼만
    /// 자리를 잡기 때문이었다.
    #[test]
    fn every_group_uses_the_same_column_widths() {
        let ctx = egui::Context::default();
        crate::fonts::install_fonts(&ctx);
        crate::fonts::install_style(&ctx);
        let mut shortcuts = Shortcuts::default();
        let mut editor = super::ShortcutEditor::default();
        let mut widths: Vec<f32> = Vec::new();

        // 첫 프레임은 글꼴 측정이 아직이라 두 번 돌린다.
        for pass in 0..2 {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1400.0, 900.0))),
                ..Default::default()
            };
            widths.clear();
            let _ = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let columns = super::ColumnWidths::measure(ui, &shortcuts);
                    for (index, category) in Category::ALL.iter().enumerate() {
                        let before = ui.min_rect().width();
                        egui::Grid::new(("g", index)).num_columns(3).show(ui, |ui| {
                            for action in Action::ALL.iter().filter(|a| a.category() == *category) {
                                super::shortcut_row(ui, *action, &mut shortcuts, &mut editor, columns);
                            }
                        });
                        widths.push(ui.min_rect().width().max(before));
                    }
                });
            });
            let _ = pass;
        }
        let first = widths[0];
        for (index, width) in widths.iter().enumerate() {
            assert!((width - first).abs() < 0.5, "{index}번째 갈래의 폭이 {width}로 어긋난다(기준 {first})");
        }
    }

    /// 설정 창 하나를 헤드리스로 띄워 두고 프레임을 돌리는 도구.
    struct Harness {
        ctx: egui::Context,
        tab: super::Tab,
        colors: super::Colors,
        shortcuts: Shortcuts,
        editor: super::ShortcutEditor,
        pin: super::EdgePin,
        last: super::Outcome,
        time: f64,
    }

    impl Harness {
        fn new(tab: super::Tab) -> Self {
            let ctx = egui::Context::default();
            crate::fonts::install_fonts(&ctx);
            crate::fonts::install_style(&ctx);
            let mut harness = Self {
                ctx,
                tab,
                colors: super::Colors::default(),
                shortcuts: Shortcuts::default(),
                editor: super::ShortcutEditor::default(),
                pin: super::EdgePin::default(),
                last: super::Outcome::default(),
                time: 0.0,
            };
            // 글꼴 측정과 창 크기가 자리 잡을 때까지 몇 프레임 돌린다.
            for _ in 0..4 {
                harness.frame(Vec::new());
            }
            harness
        }

        /// 앱과 같은 순서로 한 프레임: 입력 손질(`raw_input_hook`) → 창 그리기.
        fn frame(&mut self, events: Vec<egui::Event>) {
            self.time += 1.0 / 60.0;
            let mut input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1400.0, 1000.0))),
                time: Some(self.time),
                events,
                ..Default::default()
            };
            self.pin.clamp_pointer(&self.ctx, &mut input);
            let mut last = self.last;
            let _ = self.ctx.run(input, |ctx| {
                last = super::window(
                    ctx,
                    &mut self.tab,
                    &mut self.colors,
                    &mut self.shortcuts,
                    &mut self.editor,
                    &mut self.pin,
                );
            });
            self.last = last;
        }

        fn press(&mut self, at: egui::Pos2) {
            self.frame(vec![egui::Event::PointerMoved(at)]);
            self.frame(vec![egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            }]);
        }

        fn drag_to(&mut self, to: egui::Pos2) {
            self.frame(vec![egui::Event::PointerMoved(to)]);
        }

        fn release(&mut self, at: egui::Pos2) {
            self.frame(vec![egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }]);
        }
    }

    const TABS: [super::Tab; 3] = [super::Tab::Colors, super::Tab::Shortcuts, super::Tab::Log];

    /// 버튼 줄은 구분선과 창 아래 테두리 사이 한가운데에 있어야 한다(2026-10-03 요청).
    #[test]
    fn button_row_sits_midway_between_the_line_and_the_window_bottom() {
        for tab in TABS {
            let harness = Harness::new(tab);
            let o = harness.last;
            let above = o.row.top() - o.line_y;
            let below = o.outer.bottom() - o.row.bottom();
            eprintln!("{tab:?}: 구분선→버튼 {above}, 버튼→창 아래 {below}, 줄 높이 {}", o.row.height());
            assert!((above - below).abs() < 1.0, "{tab:?}: 위 {above} 대 아래 {below}");
        }
    }

    /// 최소 크기까지 줄인 뒤에도 왼쪽·위쪽 변(과 그 꼭짓점)을 계속 끌면, **반대쪽 변은 제자리에
    /// 있어야** 한다. 전에는 창 전체가 끄는 방향으로 밀려갔다(2026-10-03 리포트).
    #[test]
    fn left_and_top_edges_keep_the_opposite_edge() {
        // (이름, 잡을 자리, 끌고 갈 방향)
        type Grip = fn(egui::Rect) -> egui::Pos2;
        let grips: [(&str, Grip, egui::Vec2); 4] = [
            ("왼쪽 변", |r| egui::pos2(r.left(), r.center().y), egui::vec2(1.0, 0.0)),
            ("위쪽 변", |r| egui::pos2(r.center().x, r.top()), egui::vec2(0.0, 1.0)),
            ("왼쪽 위 꼭짓점", |r| r.left_top(), egui::vec2(1.0, 1.0)),
            ("왼쪽 아래 꼭짓점", |r| r.left_bottom(), egui::vec2(1.0, -1.0)),
        ];
        for tab in TABS {
            for (name, grip, direction) in grips {
                let mut harness = Harness::new(tab);
                let start = harness.last.outer;
                let from = grip(start);
                harness.press(from);
                // 창 크기보다 훨씬 멀리, 여러 번에 나눠 끈다.
                let mut to = from;
                for _ in 0..30 {
                    to += direction * 40.0;
                    harness.drag_to(to);
                }
                let end = harness.last.outer;
                harness.release(to);
                eprintln!("{tab:?} {name}: {start:?} → {end:?}");
                assert!(end.width() < start.width() || direction.x == 0.0, "{tab:?} {name}: 폭이 줄지 않았다");
                assert!(end.height() < start.height() || direction.y == 0.0, "{tab:?} {name}: 높이가 줄지 않았다");
                if direction.x > 0.0 {
                    assert!((end.right() - start.right()).abs() < 1.0, "{tab:?} {name}: 오른쪽 변이 {}만큼 밀렸다", end.right() - start.right());
                }
                if direction.y > 0.0 {
                    assert!((end.bottom() - start.bottom()).abs() < 1.0, "{tab:?} {name}: 아래 변이 {}만큼 밀렸다", end.bottom() - start.bottom());
                }
                if direction.y < 0.0 {
                    assert!((end.top() - start.top()).abs() < 1.0, "{tab:?} {name}: 위 변이 {}만큼 밀렸다", end.top() - start.top());
                }
            }
        }
    }
}
