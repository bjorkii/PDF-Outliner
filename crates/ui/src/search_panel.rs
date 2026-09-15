//! 검색 결과 사이드바 — 검색하면 메인 창 오른쪽에 결과 목록(페이지 | 결과)을 보여준다
//! (2026-09-14 요청).
//!
//! - 핀 버튼: 별도 창(항상 위)으로 분리, 다시 누르거나 분리 창을 닫으면 재도킹. 패널·창 모두
//!   크기를 자유롭게 조절할 수 있다.
//! - "페이지" 컬럼 폭은 머리글의 컬럼 경계를 드래그해 조절한다(나머지는 "결과" 컬럼).
//! - 결과 컬럼: 일치 문자열(하이라이트)과 그 앞뒤 문맥. 항목은 문서 순서(페이지 순).
//! - 포커스(`FocusArea::SearchResults`): Ctrl/Cmd+F·검색창 클릭·목록 클릭으로 들어오고,
//!   위/아래 화살표로 선택 이동(뷰어가 그 페이지로 따라감), Tab이면 들어오기 직전 영역(뷰어/
//!   북마크 사이드바)으로 돌아간다. 뷰어↔북마크 Tab 전환 체인은 그대로다.

use crate::app::{FocusArea, PdfViewerApp};
use egui::text::{LayoutJob, TextWrapping};
use egui::{Align, Align2, Color32, FontId, Id, Key, Layout, Sense, TextFormat};
use pdf_engine::search::SearchMatch;

const ROW_HEIGHT: f32 = 22.0;
const HEADER_HEIGHT: f32 = 22.0;
/// 결과 컬럼에서 일치 문자열 앞에 보여줄 문맥 글자 수 — 길면 컬럼 폭 안에서 일치 문자열이
/// 오른쪽으로 밀려나 안 보이게 된다.
const BEFORE_CONTEXT_SHOWN: usize = 12;
/// 북마크 사이드바와 같은 강조색(sidebar.rs `SIDEBAR_ACCENT`, 2026-07-17 사용자 확정).
const ACCENT: Color32 = Color32::from_rgb(0x69, 0x17, 0x8A);
/// 결과 목록 안 일치 문자열 배경 — 뷰어의 검색 하이라이트(노란색)와 같은 계열.
const MATCH_BACKGROUND: Color32 = Color32::from_rgb(255, 213, 79);
const MIN_PAGE_COLUMN_WIDTH: f32 = 32.0;
const MIN_RESULT_COLUMN_WIDTH: f32 = 80.0;
const SCROLL_SALT: &str = "search_results_rows";

/// 도킹 상태의 결과 패널. 뷰어(CentralPanel)보다 먼저 호출해야 오른쪽 공간을 차지한다.
pub fn show_docked(ctx: &egui::Context, app: &mut PdfViewerApp) {
    if !app.search_panel_open || app.search_panel_detached {
        return;
    }
    let panel = egui::SidePanel::right("search_results_panel")
        .resizable(true)
        .default_width(320.0)
        .min_width(200.0)
        .show(ctx, |ui| contents(ui, app, false));

    // 포커스 테두리 — 북마크 사이드바와 같은 방식(패널 밖 레이어에 그려야 네 변이 잘리지 않는다,
    // sidebar.rs 참고). 레이어는 PanelResizeLine(패널 위·팝업 아래)이라 드롭다운·메뉴를 덮지 않는다.
    if app.focus_area == FocusArea::SearchResults {
        ctx.layer_painter(egui::LayerId::new(
            egui::Order::PanelResizeLine,
            Id::new("search_panel_focus_border"),
        ))
        .rect_stroke(
            panel.response.rect.shrink(1.0),
            2.0_f32,
            egui::Stroke::new(2.0_f32, ACCENT),
        );
    }
}

/// 분리 상태의 결과 창(항상 위). 뷰어를 그린 뒤에 호출한다.
pub fn show_detached(ctx: &egui::Context, app: &mut PdfViewerApp) {
    if !app.search_panel_open || !app.search_panel_detached {
        return;
    }
    let builder = egui::ViewportBuilder::default()
        .with_title("검색 결과")
        .with_inner_size([420.0, 640.0])
        .with_min_inner_size([240.0, 200.0])
        .with_always_on_top();
    ctx.show_viewport_immediate(
        egui::ViewportId::from_hash_of("search_results_window"),
        builder,
        |ctx, class| {
            if class == egui::ViewportClass::Embedded {
                // 백엔드가 별도 OS 창을 못 띄우면 메인 창 안의 떠 있는 창으로 대신한다.
                let mut open = true;
                egui::Window::new("검색 결과")
                    .open(&mut open)
                    .default_size([420.0, 640.0])
                    .show(ctx, |ui| contents(ui, app, true));
                if !open {
                    app.search_panel_detached = false;
                }
                return;
            }

            egui::CentralPanel::default().show(ctx, |ui| contents(ui, app, true));

            // 분리 창을 닫으면 재도킹한다(결과는 그대로).
            if ctx.input(|i| i.viewport().close_requested()) {
                app.search_panel_detached = false;
            }
            // 분리 창에 OS 포커스가 있으면 키 입력은 메인 창이 아니라 이 창으로 온다.
            if ctx.input(|i| i.viewport().focused.unwrap_or(false)) {
                handle_detached_keys(ctx, app);
            }
        },
    );
}

fn handle_detached_keys(ctx: &egui::Context, app: &mut PdfViewerApp) {
    if ctx.wants_keyboard_input() {
        return;
    }
    let (down, up, tab) = ctx.input(|i| {
        (
            i.key_pressed(Key::ArrowDown),
            i.key_pressed(Key::ArrowUp),
            i.key_pressed(Key::Tab),
        )
    });
    if down || up {
        app.focus_search_results();
        app.step_search_selection(down);
    }
    if tab {
        // 들어오기 직전 영역(뷰어/북마크)으로 돌아가며 메인 창에 OS 포커스를 넘긴다.
        if app.focus_area == FocusArea::SearchResults {
            app.focus_area = app.focus_before_search;
        }
        ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Focus);
    }
}

fn contents(ui: &mut egui::Ui, app: &mut PdfViewerApp, detached: bool) {
    ui.horizontal(|ui| {
        ui.strong("검색 결과");
        if let Some((done, total)) = app.search_running.as_ref().map(|search| search.progress()) {
            ui.spinner();
            ui.weak(format!("{}건 · {done}/{total}쪽", app.search_matches.len()));
        } else if !app.search_matches.is_empty() {
            ui.weak(format!("{}건", app.search_matches.len()));
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            // × = 검색 모드 해제(결과·뷰어 하이라이트·검색어까지 지우고 패널을 닫음).
            if header_icon_button(ui, false, draw_close_icon)
                .on_hover_text("검색 끝내기 (결과·하이라이트 지움)")
                .clicked()
            {
                app.search_query.clear();
                app.clear_search();
            }
            let hint = if detached { "메인 창에 다시 붙이기" } else { "별도 창으로 분리 (항상 위)" };
            if header_icon_button(ui, detached, draw_pin_icon)
                .on_hover_text(hint)
                .clicked()
            {
                app.search_panel_detached = !detached;
            }
        });
    });
    ui.separator();

    if app.search_matches.is_empty() {
        ui.add_space(8.0);
        let message = if app.search_running.is_some() {
            "검색 중…"
        } else if app.search_query.trim().is_empty() {
            "검색어를 입력하고 Enter를 누르세요"
        } else {
            "결과 없음"
        };
        ui.weak(message);
        return;
    }

    // 머리글 + 컬럼 경계(드래그로 "페이지" 컬럼 폭 조절)
    let total_width = ui.available_width();
    let max_page_width = (total_width - MIN_RESULT_COLUMN_WIDTH).max(MIN_PAGE_COLUMN_WIDTH);
    let page_width = app
        .search_page_column_width
        .clamp(MIN_PAGE_COLUMN_WIDTH, max_page_width);
    let (header_rect, _) =
        ui.allocate_exact_size(egui::vec2(total_width, HEADER_HEIGHT), Sense::hover());
    let font = egui::TextStyle::Body.resolve(ui.style());
    let painter = ui.painter().clone();
    let header_color = ui.visuals().strong_text_color();
    let line_color = ui.visuals().widgets.noninteractive.bg_stroke.color;
    painter.text(
        egui::pos2(header_rect.left() + page_width / 2.0, header_rect.center().y),
        Align2::CENTER_CENTER,
        "페이지",
        font.clone(),
        header_color,
    );
    painter.text(
        egui::pos2(header_rect.left() + page_width + 8.0, header_rect.center().y),
        Align2::LEFT_CENTER,
        "결과",
        font.clone(),
        header_color,
    );
    let divider_x = header_rect.left() + page_width;
    painter.line_segment(
        [
            egui::pos2(divider_x, header_rect.top() + 3.0),
            egui::pos2(divider_x, header_rect.bottom() - 3.0),
        ],
        egui::Stroke::new(1.0_f32, line_color),
    );
    painter.line_segment(
        [header_rect.left_bottom(), header_rect.right_bottom()],
        egui::Stroke::new(1.0_f32, line_color),
    );
    let divider = ui.interact(
        egui::Rect::from_center_size(
            egui::pos2(divider_x, header_rect.center().y),
            egui::vec2(8.0, HEADER_HEIGHT),
        ),
        ui.id().with("search_column_divider"),
        Sense::drag(),
    );
    if divider.hovered() || divider.dragged() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
    }
    app.search_page_column_width = if divider.dragged() {
        (page_width + divider.drag_delta().x).clamp(MIN_PAGE_COLUMN_WIDTH, max_page_width)
    } else {
        page_width
    };

    // 결과 행 — 보이는 행만 그린다(결과가 수천 건이어도 가볍게).
    ui.spacing_mut().item_spacing.y = 0.0;
    let mut scroll = egui::ScrollArea::vertical()
        .id_salt(SCROLL_SALT)
        .auto_shrink([false, false]);
    if std::mem::take(&mut app.search_scroll_to_current_once) {
        let scroll_id = ui.make_persistent_id(Id::new(SCROLL_SALT));
        let offset = egui::scroll_area::State::load(ui.ctx(), scroll_id).map_or(0.0, |state| state.offset.y);
        if let Some(target) =
            offset_to_reveal(app.search_current_index, ROW_HEIGHT, offset, ui.available_height())
        {
            scroll = scroll.vertical_scroll_offset(target);
        }
    }

    let focused = app.focus_area == FocusArea::SearchResults;
    let mut clicked = None;
    scroll.show_rows(ui, ROW_HEIGHT, app.search_matches.len(), |ui, rows| {
        for index in rows {
            let selected = index == app.search_current_index;
            let response = result_row(ui, &app.search_matches[index], selected, focused, page_width, &font);
            if response.clicked() {
                clicked = Some(index);
            }
        }
    });
    if let Some(index) = clicked {
        app.focus_search_results();
        app.select_search_match(index);
    }
}

fn result_row(
    ui: &mut egui::Ui,
    search_match: &SearchMatch,
    selected: bool,
    focused: bool,
    page_width: f32,
    font: &FontId,
) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), ROW_HEIGHT), Sense::click());
    if !ui.is_rect_visible(rect) {
        return response;
    }
    let visuals = ui.visuals();
    // 선택 행: 목록에 포커스가 있으면 북마크 사이드바와 같은 강조색 + 흰 글자, 없으면 옅게.
    let (background, text_color) = if selected && focused {
        (Some(ACCENT), Color32::WHITE)
    } else if selected {
        (Some(ACCENT.gamma_multiply(0.3)), visuals.strong_text_color())
    } else if response.hovered() {
        (Some(visuals.widgets.hovered.weak_bg_fill), visuals.text_color())
    } else {
        (None, visuals.text_color())
    };

    let painter = ui.painter_at(rect);
    if let Some(background) = background {
        painter.rect_filled(rect, 0.0, background);
    }
    painter.text(
        egui::pos2(rect.left() + page_width / 2.0, rect.center().y),
        Align2::CENTER_CENTER,
        search_match.page.to_string(),
        font.clone(),
        text_color,
    );

    let result_rect = egui::Rect::from_min_max(
        egui::pos2(rect.left() + page_width + 8.0, rect.top()),
        rect.right_bottom(),
    );
    if result_rect.width() > 0.0 {
        let job = result_layout(search_match, font, text_color, result_rect.width() - 4.0);
        let galley = ui.fonts(|fonts| fonts.layout_job(job));
        painter.with_clip_rect(result_rect).galley(
            egui::pos2(result_rect.left(), rect.center().y - galley.size().y / 2.0),
            galley,
            text_color,
        );
    }
    response
}

/// "…앞 문맥 [일치 문자열] 뒤 문맥" 한 줄 — 컬럼 폭을 넘으면 끝을 "…"로 줄인다.
fn result_layout(search_match: &SearchMatch, font: &FontId, text_color: Color32, max_width: f32) -> LayoutJob {
    let mut job = LayoutJob {
        wrap: TextWrapping {
            max_width: max_width.max(1.0),
            max_rows: 1,
            break_anywhere: true,
            overflow_character: Some('…'),
        },
        ..Default::default()
    };
    let plain = TextFormat {
        font_id: font.clone(),
        color: text_color,
        ..Default::default()
    };
    job.append(&tail_chars(&search_match.context_before, BEFORE_CONTEXT_SHOWN), 0.0, plain.clone());
    job.append(
        &search_match.matched_text,
        0.0,
        TextFormat {
            font_id: font.clone(),
            color: Color32::BLACK,
            background: MATCH_BACKGROUND,
            ..Default::default()
        },
    );
    job.append(&search_match.context_after, 0.0, plain);
    job
}

/// 뒤쪽 `count`자만 남기고, 잘랐으면 앞에 "…"를 붙인다.
fn tail_chars(text: &str, count: usize) -> String {
    let len = text.chars().count();
    if len <= count {
        text.to_string()
    } else {
        format!("…{}", text.chars().skip(len - count).collect::<String>())
    }
}

/// 선택 행이 보이는 범위 밖이면 그 행이 보이게 할 스크롤 오프셋, 이미 보이면 None.
fn offset_to_reveal(row: usize, row_height: f32, offset: f32, view_height: f32) -> Option<f32> {
    let top = row as f32 * row_height;
    let bottom = top + row_height;
    if top < offset {
        Some(top)
    } else if bottom > offset + view_height {
        Some((bottom - view_height).max(0.0))
    } else {
        None
    }
}

/// 머리글 아이콘 버튼 한 변(pt) — 핀과 ×를 같은 크기로 맞춘다(2026-09-14 요청).
const HEADER_ICON_SIZE: f32 = 18.0;

/// 머리글 아이콘 버튼. `active`면 눌린(선택된) 배경으로 그린다. 툴바 아이콘 버튼과 같은
/// 방식(직접 rect를 잡고 벡터로 그림 — toolbar.rs `icon_button` 참고).
fn header_icon_button(
    ui: &mut egui::Ui,
    active: bool,
    draw: impl FnOnce(&egui::Painter, egui::Rect, Color32, bool),
) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(HEADER_ICON_SIZE, HEADER_ICON_SIZE), Sense::click());
    if ui.is_rect_visible(rect) {
        let visuals = ui.style().interact_selectable(&response, active);
        ui.painter()
            .rect(rect, visuals.rounding, visuals.weak_bg_fill, visuals.bg_stroke);
        draw(ui.painter(), rect.shrink(4.0), visuals.fg_stroke.color, active);
    }
    response
}

/// 세로로 선 압정 — 위 머리(가로로 넓은 캡), 몸통, 받침(넓은 가로선), 아래로 곧은 바늘.
/// 예전의 "기울어진 큰 원 + 대각선 바늘"은 돋보기(검색)로 착각된다는 피드백(2026-09-14).
/// 분리 상태(`active`)면 머리·몸통을 채워 "꽂혀 있음"을 표시한다.
fn draw_pin_icon(painter: &egui::Painter, area: egui::Rect, color: Color32, active: bool) {
    let stroke = egui::Stroke::new(1.3_f32, color);
    let (center_x, width, height) = (area.center().x, area.width(), area.height());
    let cap = egui::Rect::from_min_max(
        egui::pos2(center_x - width * 0.26, area.top()),
        egui::pos2(center_x + width * 0.26, area.top() + height * 0.16),
    );
    let body = egui::Rect::from_min_max(
        egui::pos2(center_x - width * 0.14, cap.bottom()),
        egui::pos2(center_x + width * 0.14, area.top() + height * 0.52),
    );
    if active {
        painter.rect_filled(cap, 0.5, color);
        painter.rect_filled(body, 0.0, color);
    } else {
        painter.rect_stroke(cap, 0.5, stroke);
        painter.rect_stroke(body, 0.0, stroke);
    }
    let collar_y = body.bottom();
    painter.line_segment(
        [
            egui::pos2(center_x - width * 0.36, collar_y),
            egui::pos2(center_x + width * 0.36, collar_y),
        ],
        stroke,
    );
    painter.line_segment(
        [egui::pos2(center_x, collar_y), egui::pos2(center_x, area.bottom())],
        stroke,
    );
}

/// × — 핀과 같은 크기 영역에 벡터로 그린다(글자 ×는 글꼴 크기를 따라가 핀과 크기가 어긋났음).
fn draw_close_icon(painter: &egui::Painter, area: egui::Rect, color: Color32, _active: bool) {
    let stroke = egui::Stroke::new(1.3_f32, color);
    let area = area.shrink(1.0);
    painter.line_segment([area.left_top(), area.right_bottom()], stroke);
    painter.line_segment([area.right_top(), area.left_bottom()], stroke);
}

#[cfg(test)]
mod tests {
    use super::{offset_to_reveal, tail_chars};

    #[test]
    fn tail_chars_keeps_the_end_near_the_match() {
        assert_eq!(tail_chars("짧은", 12), "짧은");
        assert_eq!(tail_chars("가나다라마바사아자차카타파하", 4), "…카타파하");
    }

    #[test]
    fn reveal_scrolls_only_when_row_is_out_of_view() {
        // 22pt 행, 화면 높이 220pt(10행), 현재 오프셋 0.
        assert_eq!(offset_to_reveal(3, 22.0, 0.0, 220.0), None);
        assert_eq!(offset_to_reveal(12, 22.0, 0.0, 220.0), Some(13.0 * 22.0 - 220.0));
        assert_eq!(offset_to_reveal(2, 22.0, 100.0, 220.0), Some(44.0));
    }
}
