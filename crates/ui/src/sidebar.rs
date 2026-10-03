use crate::app::PdfViewerApp;
use bookmark::{move_node, parent_of, BookmarkNode, DropPosition};
use egui::{Id, Sense};
use std::collections::HashSet;
use uuid::Uuid;

/// 드래그 중인 노드 id와, 현재 hover 중인 대상 위에서의 드롭 위치.
#[derive(Default, Clone)]
pub struct DragState {
    pub dragging: Option<Uuid>,
    pub hover_target: Option<(Uuid, DropPosition)>,
    /// 인라인 이름 수정 중인 노드 id와 편집 버퍼. "+"로 새로 추가했거나, 이미 선택된
    /// 항목을 한 번 더 클릭했을 때 진입한다.
    pub editing: Option<(Uuid, String)>,
    /// 접혀있는(자식이 안 보이는) 노드 id 집합. 트리 데이터 자체가 아니라 순수 표시 상태라
    /// BookmarkNode나 CSV/PDF outline 스키마에는 저장하지 않는다.
    pub collapsed: HashSet<Uuid>,
    /// 새로 만든 항목이라 이번 프레임에 편집 텍스트필드로 포커스를 옮겨야 하는지.
    pub focus_editing: bool,
    /// 화살표 키 순회로 선택이 방금 바뀌었으니, 다음 프레임에 그 행이 스크롤 영역
    /// 밖(위/아래)이면 부드럽게 중앙으로 스크롤해 달라는 1회성 요청(2026-07-17 요청).
    /// 클릭 선택에는 세우지 않는다 — 클릭된 행은 정의상 이미 화면 안에 있다.
    pub scroll_selected_into_view: bool,
    /// 이번 프레임에 **뷰어를 눌렀는가**(예약 10). 제목 편집 중에 뷰어를 눌러 포커스를 잃은
    /// 경우와 그 밖의 경우를 가르는 데 쓴다. 매 프레임 새로 계산해 넣으므로 기억된 값이 남아도
    /// 곧바로 덮인다 — 다른 필드와 달리 프레임을 넘겨 이어지는 상태가 아니다.
    pub pressed_in_viewer: bool,
    /// `scroll_to_me` 호출 직후 이 시각까지는 매 프레임 강제로 다시 그리게 한다. egui는
    /// 기본적으로 입력 이벤트가 있을 때만 다시 그리는 즉시모드라(§7 문서 검색 폴링과 같은
    /// 사정), scroll_to_me가 세운 애니메이션 목표가 있어도 사용자가 마우스를 안 움직이면
    /// 프레임이 불규칙하게만 진행돼 "부드럽게"가 아니라 "덜컹거리며" 움직인다(실측 리포트,
    /// 2026-07-18) — 애니메이션 지속시간(egui 기본 0.1~0.3초) 동안 강제로 매 프레임을
    /// 깨워야 실제로 매끄럽게 보인다.
    pub scroll_animation_until: Option<std::time::Instant>,
}

/// 북마크 탭 머리 줄(추가·삭제·Undo·Redo)의 높이(pt).
///
/// 아이콘 버튼은 19pt(글자 줄 17pt + 위아래 1pt씩)이므로 그보다 낮출 수 없다. 이 값이면 아이콘
/// 잉크 위아래로 **잉크 높이의 30%쯤**이 남는다(`header_padding_tests`). 처음 36pt였고 아이콘으로
/// 바꾼 뒤 26pt로 줄였지만 그래도 46%가 남아 휑했다(2026-10-03 리포트).
const HEADER_HEIGHT: f32 = 22.0;

/// 재귀 전체에 걸쳐 누적되는 결과. 재귀 호출마다 지역 변수를 새로 선언하면 하위 노드의
/// 클릭이 상위 호출로 전파되지 않고 버려지는 버그가 생기므로, 하나의 구조체를 `&mut`로
/// 재귀 전체에 그대로 넘긴다.
#[derive(Default)]
struct RenderOutcome {
    jump_page: Option<u32>,
    selected: Option<Uuid>,
    dirty: bool,
    /// 확정된 제목 변경 (노드 id, 새 제목) — 되돌리기 기록을 먼저 남겨야 해서 호출 측에서 적용한다.
    rename: Option<(Uuid, String)>,
}

/// 엑셀 시트 탭이나 브라우저 탭처럼 보이는 탭 줄(2026-10-01 요청).
///
/// **고른 탭만 아래 테두리를 끊어 본문과 이어지게 그린다.** 이것이 눌린 단추가 아니라 탭으로
/// 읽히게 하는 핵심이다. `selectable_label`은 배경색만 바꾸므로 단추처럼 보였다. egui에 탭 위젯은
/// 없어서 직접 그린다 — 칸을 잡고, 안 고른 탭 → 아래 가로줄 → 고른 탭 순으로 얹으면 고른 탭의
/// 바탕이 그 아래 가로줄을 덮어 본문과 이어진다.
fn tab_bar(ui: &mut egui::Ui, current: &mut crate::app::SidebarTab) {
    use crate::app::SidebarTab;

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

    let tabs = [
        (SidebarTab::Bookmarks, crate::icons::TAB_BOOKMARKS, "북마크"),
        (SidebarTab::Thumbnails, crate::icons::TAB_THUMBNAILS, "썸네일"),
    ];
    let (strip, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), HEIGHT), Sense::hover());
    if !ui.is_rect_visible(strip) {
        return;
    }

    let visuals = ui.visuals().clone();
    let border = visuals.widgets.noninteractive.bg_stroke.color;
    let font = egui::TextStyle::Button.resolve(ui.style());
    let rounding = egui::Rounding { nw: RADIUS, ne: RADIUS, sw: 0.0, se: 0.0 };

    // 칸을 먼저 다 잡아 둔다. 그려야 할 순서와 자리를 정하는 순서가 다르기 때문이다.
    let mut placed = Vec::with_capacity(tabs.len());
    let mut x = strip.left() + LEFT;
    for (tab, icon, label) in tabs {
        // 아이콘과 글자를 한 줄로 재어 둔다 — 탭 폭을 그 합으로 잡는다.
        let icon_font = egui::FontId::new(ICON_SIZE, egui::FontFamily::Name(crate::fonts::ICON_FAMILY.into()));
        let icon_galley = ui.painter().layout_no_wrap(icon.to_string(), icon_font, egui::Color32::PLACEHOLDER);
        let galley = ui.painter().layout_no_wrap(label.to_string(), font.clone(), egui::Color32::PLACEHOLDER);
        let inner = icon_galley.size().x + ICON_GAP + galley.size().x;
        let rect = egui::Rect::from_min_size(egui::pos2(x, strip.top()), egui::vec2(inner + PAD_X * 2.0, HEIGHT));
        x = rect.right() + GAP;
        let response = ui.interact(rect, ui.id().with(("sidebar_tab", label)), Sense::click());
        if response.clicked() {
            *current = tab;
        }
        placed.push((tab, icon_galley, galley, rect, response));
    }

    let painter = ui.painter().clone();
    let draw = |tab: &SidebarTab,
                icon_galley: &std::sync::Arc<egui::Galley>,
                galley: &std::sync::Arc<egui::Galley>,
                rect: egui::Rect,
                hovered: bool| {
        let active = *current == *tab;
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
        painter.galley(
            egui::pos2(left, rect.center().y - icon_galley.size().y / 2.0),
            icon_galley.clone(),
            color,
        );
        painter.galley(
            egui::pos2(left + icon_galley.size().x + ICON_GAP, rect.center().y - galley.size().y / 2.0),
            galley.clone(),
            color,
        );
    };

    for (tab, icon_galley, galley, rect, response) in &placed {
        if *current != *tab {
            draw(tab, icon_galley, galley, *rect, response.hovered());
        }
    }
    painter.line_segment(
        [egui::pos2(strip.left(), strip.bottom()), egui::pos2(strip.right(), strip.bottom())],
        egui::Stroke::new(1.0_f32, border),
    );
    for (tab, icon_galley, galley, rect, response) in &placed {
        if *current == *tab {
            draw(tab, icon_galley, galley, *rect, response.hovered());
        }
    }
}

/// 지금 누르고 있는(또는 방금 뗀) 곳이 뷰어 안인가.
///
/// 사이드바는 뷰어보다 먼저 그려지므로 지난 프레임에 재어 둔 자리를 쓴다(`app::viewer_rect`).
/// **누르는 동작만 본다** — Tab처럼 키로 포커스를 옮긴 것은 예전대로 편집을 끝내야 한다.
fn pressed_in_viewer(ctx: &egui::Context, viewer: Option<egui::Rect>) -> bool {
    let Some(rect) = viewer else { return false };
    ctx.input(|i| {
        let pressing = i.pointer.any_down() || i.pointer.any_released();
        let at = i.pointer.press_origin().or_else(|| i.pointer.latest_pos());
        pressing && at.is_some_and(|pos| rect.contains(pos))
    })
}

pub fn show(ctx: &egui::Context, app: &mut PdfViewerApp) {
    let panel_response = egui::SidePanel::left("bookmarks_sidebar")
        .resizable(true)
        .default_width(240.0)
        .min_width(90.0)
        .show(ctx, |ui| {
            // 탭 줄 — 북마크와 썸네일을 오간다(예약 7). 아래 헤더의 +/-/Undo/Redo는 북마크에만
            // 쓰는 것이라 탭이 그 위에 있어야 말이 된다. 썸네일 탭이면 여기서 끝낸다 — 북마크
            // 쪽의 드래그 상태를 읽지도 쓰지도 않아야 그 상태가 어긋나지 않는다.
            ui.add_space(6.0);
            tab_bar(ui, &mut app.sidebar_tab);
            if app.sidebar_tab == crate::app::SidebarTab::Thumbnails {
                ui.add_space(8.0);
                crate::thumbnails::show(ui, app);
                return;
            }

            let drag_id = Id::new("bookmark_drag_state");
            let mut drag_state = ctx
                .data_mut(|d| d.get_temp::<DragState>(drag_id))
                .unwrap_or_default();
            drag_state.pressed_in_viewer = pressed_in_viewer(ctx, app.viewer_rect);

            // Cmd+B 등 외부(app.rs 전역 단축키)에서 걸어둔 "추가해줘" 요청 처리.
            // DragState(편집 포커스 상태)는 이 파일 안에서만 관리되므로, app.rs는 플래그만
            // 세워두고 실제 처리는 여기서 "+" 버튼과 동일한 로직으로 한다.
            if app.request_add_bookmark {
                app.request_add_bookmark = false;
                add_new_bookmark(app, &mut drag_state);
            }

            // 헤더: +/-/Undo/Redo 버튼을 헤더 영역 안에서 가로/세로 모두 중앙 정렬
            // ("북마크" 제목 텍스트는 자리만 차지해서 제거 — 2026-07-17 요청).
            // 가로 중앙: egui는 single-pass immediate mode라 그리기 전엔 콘텐츠 폭을 알 수
            // 없으므로, 지난 프레임에 측정해둔 버튼 그룹 폭으로 오프셋을 계산한다(첫
            // 프레임만 왼쪽 치우침, 다음 프레임부터 중앙 — 실사용에선 안 보임).
            // 세로 중앙: 고정 높이 rect를 잡고 그 안에 Align::Center 가로 레이아웃 child를
            // 만든다 — 예전처럼 ui.horizontal을 그냥 쓰면 행 높이가 버튼 높이에 딱 맞아
            // 헤더 영역 위쪽에 붙어 보인다는 피드백.
            // **머리 줄 둘레의 간격을 없앤다.**
            //
            // 눈에 보이는 여백은 머리 줄 *안쪽*이 아니라 바깥에서 온다. 앞서 머리 줄 높이만
            // 줄였더니 안쪽은 31%가 됐는데 화면에서는 그대로였다 — 실측으로 위 15.2pt(잉크
            // 높이의 112%), 아래 10.2pt(75%)였고, 전역 줄 간격 6pt와 탭 아래 띄움 8pt,
            // 그리고 머리 줄이 36pt이던 시절의 균형 보정 3pt가 그 정체였다(2026-10-03).
            //
            // 머리 줄 안에 이미 아이콘 위아래로 여백이 있으므로, 둘레는 0으로 두고 그 안쪽
            // 여백만 보이게 한다. 구분선을 지난 뒤 원래 간격으로 되돌린다.
            let outer_spacing = std::mem::replace(&mut ui.spacing_mut().item_spacing.y, 0.0);
            let header_height = HEADER_HEIGHT;
            let (header_rect, _) = ui.allocate_exact_size(
                egui::vec2(ui.available_width(), header_height),
                Sense::hover(),
            );
            let buttons_width_id = Id::new("bm_header_buttons_width");
            let known_width: f32 = ctx
                .data_mut(|d| d.get_temp(buttons_width_id))
                .unwrap_or(0.0);
            let mut header_ui = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(header_rect)
                    .layout(egui::Layout::left_to_right(egui::Align::Center)),
            );
            {
                let ui = &mut header_ui;
                ui.add_space(((header_rect.width() - known_width) / 2.0).max(0.0));
                let buttons_start_x = ui.cursor().min.x;

                if crate::icons::button(
                    ui,
                    crate::icons::ADD,
                    "추가 (Cmd+B). 선택된 항목의 하위에, 선택이 없으면 최상위에 넣습니다.",
                )
                .clicked()
                {
                    add_new_bookmark(app, &mut drag_state);
                }

                let delete_enabled = app.selected_bookmark.is_some();
                if crate::icons::button_enabled(ui, delete_enabled, crate::icons::REMOVE, "삭제 (Delete)")
                    .clicked()
                {
                    app.delete_selected_bookmark();
                }

                let undo_enabled = !app.bookmark_undo_stack.is_empty();
                if crate::icons::button_enabled(ui, undo_enabled, crate::icons::UNDO, "실행취소 (Cmd+Z)")
                    .clicked()
                {
                    app.undo_bookmarks();
                }

                let redo_enabled = !app.bookmark_redo_stack.is_empty();
                if crate::icons::button_enabled(ui, redo_enabled, crate::icons::REDO, "다시 실행 (Cmd+Shift+Z)")
                    .clicked()
                {
                    app.redo_bookmarks();
                }

                let measured = ui.min_rect().max.x - buttons_start_x;
                ctx.data_mut(|d| d.insert_temp(buttons_width_id, measured));
            }
            ui.separator();
            ui.spacing_mut().item_spacing.y = outer_spacing;

            let mut outcome = RenderOutcome::default();
            let current_selected = app.selected_bookmark;
            // 1회성 플래그라 여기서 바로 소비(false로 되돌림) — 앱 시작 시 마지막으로 보던
            // 페이지를 복원했을 때만 세워짐(main.rs 참고), 이후 일반 페이지 이동에는 적용 안 함.
            // 선택은 페이지 이동 때마다 활성 북마크로 자동 동기화되므로(set_current_page)
            // 복원 직후의 selected_bookmark가 곧 그 페이지의 활성 북마크다.
            let scroll_to_active_once = std::mem::take(&mut app.scroll_sidebar_to_active_once);
            if scroll_to_active_once {
                // 접혀있는 조상 밑에 있으면 애초에 안 그려져서 스크롤이 무의미하다 —
                // add_new_bookmark와 같은 패턴으로 미리 펼쳐둔다.
                if let Some(id) = current_selected {
                    for ancestor in ancestors_of(&app.bookmarks, id) {
                        drag_state.collapsed.remove(&ancestor);
                    }
                }
            }

            // 페이지가 바뀐 프레임(app.rs의 set_current_page/note_visible_page_during_scroll):
            // 사이드바가 다른 곳으로 스크롤돼 활성 북마크가 안 보이는 상태면 부드럽게 중앙으로
            // 되돌린다(2026-07-18 요청). scroll_selected_into_view는 "화면 밖일 때만" 스크롤
            // 하므로, 이미 보이는 항목은 건드리지 않아 화살표 페이지 연타 중에 들썩이지 않는다.
            if std::mem::take(&mut app.sidebar_reveal_selected_once) {
                if let Some(id) = current_selected {
                    for ancestor in ancestors_of(&app.bookmarks, id) {
                        drag_state.collapsed.remove(&ancestor);
                    }
                    drag_state.scroll_selected_into_view = true;
                }
            }

            // auto_shrink를 꺼야 스크롤 영역이 사이드바 폭을 다 쓴다. egui의 기본값은
            // 가로·세로 모두 "내용에 맞춰 줄이기"라, 북마크 제목이 짧은 문서(예: 전부
            // "0017" 같은 쪽번호)에서는 영역이 글자 폭까지 쪼그라들어 스크롤바가 글자
            // 바로 옆에 붙고 오른쪽이 텅 비어 보였다(사용자 리포트, 2026-09-27).
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                // 북마크 트리 글자를 기본(본문 12.5pt)보다 두 단계(2pt) 키운다(2026-09-15 요청).
                // 이 스크롤 영역 안에만 적용 — 위쪽 +/-/Undo/Redo 버튼은 그대로. 접기 아이콘
                // (Small)도 같은 비율로 키워 글자와 어울리게 한다. 행 높이는 interact_size.y(18pt)
                // 기준이라 14.5pt 글자도 그대로 들어간다.
                for style in [egui::TextStyle::Body, egui::TextStyle::Button, egui::TextStyle::Small] {
                    if let Some(font) = ui.style_mut().text_styles.get_mut(&style) {
                        font.size += 2.0;
                    }
                }
                render_nodes(
                    ui,
                    &mut app.bookmarks,
                    &mut drag_state,
                    current_selected,
                    app.selection_is_explicit,
                    scroll_to_active_once,
                    app.colors.bookmark_selection.stroke(),
                    &mut outcome,
                );
                // 트리가 패널을 꽉 채우면 새로 추가된 항목(항상 형제 중 맨 끝 근처에 생김)이
                // 스크롤 영역 맨 아래 경계에 딱 붙어 다음 "+"/Cmd+B를 누르기 전까지 시야에서
                // 잘려 보이는 느낌을 준다 — 여유 공간을 좀 둬서 항상 마지막 항목 아래가
                // 눈에 들어오게 한다.
                ui.add_space(48.0);
            });

            // 마우스를 뗀 시점에 실제 드래그 재구성 적용
            if ui.input(|i| i.pointer.any_released()) {
                if let (Some(moving), Some((target, pos))) =
                    (drag_state.dragging, drag_state.hover_target)
                {
                    if moving != target {
                        app.push_bookmark_undo_snapshot();
                        if move_node(&mut app.bookmarks, moving, target, pos).is_ok() {
                            outcome.dirty = true;
                        } else {
                            app.bookmark_undo_stack.pop_back();
                        }
                    }
                }
                drag_state.dragging = None;
                drag_state.hover_target = None;
            }

            // 선택된 북마크 기준 화살표 키 네비게이션 + F2 이름 편집. 텍스트 편집 중이거나
            // 다른 위젯이 키보드를 쓰고 있을 때, 그리고 포커스가 뷰어 쪽일 때는(app.rs의
            // FocusArea 참고 — Tab 또는 뷰어 클릭으로 옮겨감) 가로채지 않는다.
            if !ctx.wants_keyboard_input()
                && drag_state.editing.is_none()
                && app.focus_area == crate::app::FocusArea::Sidebar
            {
                if let Some(selected) = current_selected {
                    // F2 — 선택된 항목을 곧바로 이름 편집 모드로. 더블클릭/재클릭과 동일한
                    // 진입점이지만 키보드만으로 접근 가능하게 하는 게 목적.
                    if ctx.input(|i| i.key_pressed(egui::Key::F2)) {
                        if let Some(title) = find_title(&app.bookmarks, selected) {
                            drag_state.editing = Some((selected, title));
                            drag_state.focus_editing = true;
                        }
                    }

                    // (Enter로 선택 항목 페이지 재확인하던 기능은 제거 — 상/하 화살표
                    // 이동이 곧바로 페이지를 넘기므로 선택과 페이지가 항상 동기화된다,
                    // 2026-07-17 사용자 확정.)

                    let mut visible = Vec::new();
                    flatten_visible(&app.bookmarks, &drag_state.collapsed, &mut visible);
                    if let Some(pos) = visible.iter().position(|id| *id == selected) {
                        ctx.input(|i| {
                            // 상/하 화살표 = 형제/전체 탐색. 클릭과 마찬가지로 이동한 항목의
                            // 페이지를 곧바로 뷰어에 보여준다(2026-07-17 추가 요구사항 — 예전엔
                            // Enter를 따로 눌러야 페이지가 따라왔음).
                            if i.key_pressed(egui::Key::ArrowDown) && pos + 1 < visible.len() {
                                let target = visible[pos + 1];
                                outcome.selected = Some(target);
                                outcome.jump_page = find_page(&app.bookmarks, target);
                                drag_state.scroll_selected_into_view = true;
                            }
                            if i.key_pressed(egui::Key::ArrowUp) && pos > 0 {
                                let target = visible[pos - 1];
                                outcome.selected = Some(target);
                                outcome.jump_page = find_page(&app.bookmarks, target);
                                drag_state.scroll_selected_into_view = true;
                            }
                            // 좌/우 화살표는 선택된 항목 자체가 자식을 가지면 그 항목을,
                            // 아니면(리프 노드) 그 부모를 접고/편다 — "선택된 항목이 속한
                            // 레벨"을 조작한다는 요구사항(예전 동작 그대로, 2026-07-17 복원 —
                            // 포커스가 사이드바일 때만 여기로 오므로 뷰어 페이지 이동과 더 이상
                            // 겹치지 않는다).
                            let fold_target = if has_children(&app.bookmarks, selected) {
                                Some(selected)
                            } else {
                                parent_of(&app.bookmarks, selected)
                            };
                            if let Some(target) = fold_target {
                                if i.key_pressed(egui::Key::ArrowLeft) {
                                    drag_state.collapsed.insert(target);
                                    // 리프 노드가 선택된 상태에서 그 부모를 접은 경우,
                                    // 선택된 노드 자신은 화면에서 사라진다 — 선택을 부모로
                                    // 옮겨야 다음 화살표 키 입력이 계속 먹힌다("포커스 상실"
                                    // 버그). 자기 자신을 접은 경우(target == selected)는
                                    // 여전히 화면에 보이니 선택을 그대로 둔다.
                                    if target != selected {
                                        outcome.selected = Some(target);
                                        outcome.jump_page = find_page(&app.bookmarks, target);
                                        drag_state.scroll_selected_into_view = true;
                                    }
                                }
                                if i.key_pressed(egui::Key::ArrowRight) {
                                    drag_state.collapsed.remove(&target);
                                }
                            }
                        });
                    }
                }
            }

            // scroll_to_me 애니메이션이 아직 진행 중일 시각이면 강제로 다음 프레임을 깨워
            // 매끄럽게 움직이게 한다(DragState::scroll_animation_until 문서 참고) — 다
            // 지났으면 더 깨울 필요 없으니 필드를 비운다.
            if let Some(deadline) = drag_state.scroll_animation_until {
                if std::time::Instant::now() < deadline {
                    ctx.request_repaint();
                } else {
                    drag_state.scroll_animation_until = None;
                }
            }
            ctx.data_mut(|d| d.insert_temp(drag_id, drag_state));

            if let Some(page) = outcome.jump_page {
                app.go_to_page(page);
            }
            if let Some(selected) = outcome.selected {
                // go_to_page(위 jump_page 처리)가 자동 동기화로 selected_bookmark를 앵커
                // 북마크로 덮어썼을 수 있으므로, 사용자가 직접 고른 노드가 반드시 그 뒤에
                // 다시 쓰여야 한다(같은 페이지에 북마크 여러 개인 경우 실제로 갈라짐).
                app.selected_bookmark = Some(selected);
                app.selection_is_explicit = true;
                // 클릭이든 화살표 키 탐색이든, 북마크 선택이 바뀌면 포커스는 사이드바다
                // (이미 사이드바 포커스였던 화살표 키 경로에는 no-op, 뷰어 포커스 상태에서
                // 북마크를 클릭한 경우엔 실제로 되돌림 — app::FocusArea 문서 참고).
                app.focus_area = crate::app::FocusArea::Sidebar;
            }
            // 제목 변경은 되돌리기 기록을 먼저 남기고 적용한다(그리기 중에는 app을 다시
            // 빌릴 수 없어 호출 측으로 올려 보낸 것).
            if let Some((id, title)) = outcome.rename {
                app.push_bookmark_undo_snapshot();
                if set_title(&mut app.bookmarks, id, title) {
                    app.bookmarks_dirty = true;
                } else {
                    app.bookmark_undo_stack.pop_back();
                }
            }
            if outcome.dirty {
                app.bookmarks_dirty = true;
            }
        });

    // 레이어는 PanelResizeLine(패널 위·팝업 아래) — 예전엔 Foreground라 같은 층의 툴바 "최근 파일"
    // 드롭다운보다 나중에 그려져 목록 위에 테두리가 겹쳤다(2026-09-15 피드백).
    // 포커스가 사이드바일 때 패널 둘레에 테두리를 그려 "지금 화살표 키가 북마크
    // 탐색으로 동작한다"는 걸 시각적으로 알려준다(Tab/클릭으로 전환 — FocusArea 문서
    // 참고). 처음엔 패널 닫기 전에 ui.painter()로 그렸는데, 패널 안의 painter는 패널
    // 내용 영역으로 클리핑돼서 위/아래 변이 잘리고 좌/우 변만 보였다(사용자 리포트,
    // 2026-07-17) — 패널이 닫힌 뒤 클리핑 없는 Foreground 레이어 painter로 패널 전체
    // rect에 그려야 네 변이 다 나온다. 스트로크가 화면 경계에서 잘리지 않게 절반
    // 폭만큼 안쪽으로 줄인다.
    if app.focus_area == crate::app::FocusArea::Sidebar {
        let panel_rect = panel_response.response.rect;
        let stroke = egui::Stroke::new(2.0_f32, app.colors.focus_border.stroke());
        ctx.layer_painter(egui::LayerId::new(
            egui::Order::PanelResizeLine,
            Id::new("sidebar_focus_border"),
        ))
        .rect_stroke(panel_rect.shrink(1.0), 2.0_f32, stroke);
    }
}

/// `scroll_to_me` 호출 직후마다 부르는 헬퍼 — 애니메이션 지속시간(egui 기본 최대 0.3초)
/// 동안 매 프레임 강제로 다시 그리도록 마감 시각을 세운다(DragState::scroll_animation_until
/// 문서 참고).
fn request_scroll_animation_repaint(drag_state: &mut DragState) {
    drag_state.scroll_animation_until =
        Some(std::time::Instant::now() + std::time::Duration::from_millis(400));
}

/// "+"버튼과 Cmd+B가 공유하는 로직: 선택된 항목의 자식(없으면 최상위)으로 새 북마크를
/// 추가하고, 조상 노드를 펼쳐서 보이게 한 뒤, 곧바로 이름 편집 모드로 들어간다.
///
/// 제목 초기값은 클립보드의 텍스트다 — 뷰어에서 장·절 제목을 드래그해 복사(Cmd+C)한 뒤
/// Cmd+B를 누르면 그대로 제목이 된다. 편집 필드는 전체 선택 상태로 열리므로 원치 않으면
/// 바로 타이핑해 덮어쓰면 된다. 클립보드가 비었거나 텍스트가 아니면 "새 북마크".
/// 노드 자체도 같은 제목으로 만들어서, Esc로 편집을 취소해도 보이던 제목이 남는다.
fn add_new_bookmark(app: &mut PdfViewerApp, drag_state: &mut DragState) {
    let title = arboard::Clipboard::new()
        .and_then(|mut clipboard| clipboard.get_text())
        .ok()
        .and_then(|text| bookmark_title_from_clipboard(&text))
        .unwrap_or_else(|| "새 북마크".to_string());
    let new_id = app.add_bookmark_under_selection(&title);
    for ancestor in ancestors_of(&app.bookmarks, new_id) {
        drag_state.collapsed.remove(&ancestor);
    }
    drag_state.editing = Some((new_id, title));
    drag_state.focus_editing = true;
}

/// 북마크 제목으로 쓸 수 있는 최대 글자 수 — 본문을 통째로 복사해 둔 채 추가해도 사이드바가
/// 한 문단짜리 항목으로 뒤덮이지 않게 자른다.
const MAX_CLIPBOARD_TITLE_CHARS: usize = 200;

/// 클립보드 텍스트를 한 줄 제목으로 정리한다. PDF에서 복사한 텍스트는 줄바꿈·탭·연속
/// 공백이 섞여 있기 흔하므로 공백 하나로 합친다. 쓸 내용이 없으면 None.
fn bookmark_title_from_clipboard(text: &str) -> Option<String> {
    let title = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if title.is_empty() {
        return None;
    }
    Some(title.chars().take(MAX_CLIPBOARD_TITLE_CHARS).collect())
}

fn render_nodes(
    ui: &mut egui::Ui,
    nodes: &mut Vec<BookmarkNode>,
    drag_state: &mut DragState,
    current_selected: Option<Uuid>,
    selection_is_explicit: bool,
    scroll_to_active_once: bool,
    selection_color: egui::Color32,
    outcome: &mut RenderOutcome,
) {
    let mut delete_id: Option<Uuid> = None;

    for node in nodes.iter_mut() {
        let is_editing = drag_state.editing.as_ref().is_some_and(|(id, _)| *id == node.id);
        let is_selected = current_selected == Some(node.id);
        let has_children = !node.children.is_empty();
        let is_collapsed = drag_state.collapsed.contains(&node.id);

        let row_response = ui.horizontal(|ui| {
            // 접기/펼치기 화살표(자식 있는 노드만). 없는 노드는 자리만 맞춰서 정렬을 맞춘다.
            // add_sized(Button)로 폭을 고정해봤지만 여전히 자식 있는 행이 없는 행보다 더
            // 들여쓰기되는 정렬 어긋남이 있었다 — egui의 centered_and_justified 레이아웃은
            // 버튼의 "요청한" 크기가 아니라 내부 콘텐츠가 실제로 차지한 min_rect 크기만큼만
            // 부모 커서를 전진시키기 때문에(egui ui.rs의 allocate_new_ui_dyn 참고), 작은
            // 아이콘 글리프 하나만 든 Button의 실제 폭이 18.0과 미묘하게 달라지면 그만큼
            // 어긋난다. allocate_exact_size로 폭을 직접 못박고 그 rect 안에 글리프만
            // 수동으로 그리면 두 경우가 항상 정확히 같은 폭을 차지한다.
            let toggle_size = egui::vec2(18.0, ui.spacing().interact_size.y);
            let (toggle_rect, toggle_response) = ui.allocate_exact_size(
                toggle_size,
                if has_children { Sense::click() } else { Sense::hover() },
            );
            if has_children {
                let icon = if is_collapsed { ">" } else { "v" };
                ui.painter().text(
                    toggle_rect.center(),
                    egui::Align2::CENTER_CENTER,
                    icon,
                    egui::TextStyle::Small.resolve(ui.style()),
                    ui.visuals().text_color(),
                );
                if toggle_response.clicked() {
                    if is_collapsed {
                        drag_state.collapsed.remove(&node.id);
                    } else {
                        drag_state.collapsed.insert(node.id);
                    }
                }
            }

            if is_editing {
                let (_, buffer) = drag_state.editing.as_mut().unwrap();
                let buffer_len_chars = buffer.chars().count();
                let edit_id = Id::new(("bm_edit", node.id));
                // **여러 줄 칸을 쓰되 Enter는 확정에 남긴다**(2026-10-01 요청). 한 줄 칸은 긴 제목이
                // 옆으로 흘러 나가 편집이 어렵다. `return_key(None)`으로 줄바꿈 삽입을 꺼 두면
                // 아래 `enter_pressed`가 예전처럼 확정을 맡는다. `desired_rows(1)`이라 짧은 제목은
                // 한 줄 높이 그대로이고, 길어지면 그만큼 칸이 자란다.
                let edit_response = ui.add(
                    egui::TextEdit::multiline(buffer)
                        .desired_width(ui.available_width())
                        .desired_rows(1)
                        .return_key(None)
                        .id(edit_id),
                );
                if drag_state.focus_editing {
                    ui.memory_mut(|m| m.request_focus(edit_id));
                    // 새로 추가한 항목(또는 F2/재클릭으로 편집 시작한 항목)이 스크롤 영역
                    // 밖에 있을 수 있으니 편집 필드가 보이는 위치까지 스크롤한다 — 사이드바가
                    // 꽉 찬 상태에서 Cmd+B로 추가하면 새 항목이 안 보이던 문제.
                    edit_response.scroll_to_me(Some(egui::Align::Center));
                    // `buffer`가 drag_state.editing을 통째로 빌리고 있어 여기서 함수
                    // 호출(&mut drag_state 전체)을 못 하므로, 서로 겹치지 않는 필드는
                    // 직접 대입해 우회한다(request_scroll_animation_repaint와 동일 내용).
                    drag_state.scroll_animation_until =
                        Some(std::time::Instant::now() + std::time::Duration::from_millis(400));
                    // 텍스트 전체를 선택 상태로 둬서, 새로 만든 항목의 초기 제목(클립보드 내용 또는 "새 북마크")이나
                    // F2/재클릭으로 연 기존 제목을 바로 타이핑해서 덮어쓸 수 있게 한다 —
                    // request_focus만으로는 커서만 옮겨갈 뿐 선택은 안 돼서 매번 수동으로
                    // 전체 선택(Cmd+A)해야 했다.
                    if let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), edit_id) {
                        let range = egui::text::CCursorRange::two(
                            egui::text::CCursor::new(0),
                            egui::text::CCursor::new(buffer_len_chars),
                        );
                        state.cursor.set_char_range(Some(range));
                        egui::TextEdit::store_state(ui.ctx(), edit_id, state);
                    }
                    drag_state.focus_editing = false;
                }

                let enter_pressed = ui.input(|i| i.key_pressed(egui::Key::Enter));
                let escape_pressed = ui.input(|i| i.key_pressed(egui::Key::Escape));

                if escape_pressed {
                    // 편집 취소 — 입력한 내용을 버리고 원래 제목 유지
                    drag_state.editing = None;
                } else if enter_pressed || edit_response.lost_focus() {
                    // **뷰어를 눌러서 포커스가 옮겨 간 것이라면 끝내지 않는다**(예약 10). 다른
                    // 쪽을 확인해 가며 제목을 적는 경우에 대응한다. Enter·Esc, 그리고 뷰어 밖을
                    // 누른 경우에는 예전대로 끝난다.
                    if !enter_pressed && drag_state.pressed_in_viewer {
                        ui.memory_mut(|m| m.request_focus(edit_id));
                    } else {
                    // Enter뿐 아니라 다른 곳을 클릭해 포커스를 잃어도 커밋한다(Finder식 관례).
                    //
                    // **제목이 그대로면 아무것도 하지 않는다.** 전에는 내용이 같아도 변경으로
                    // 표시해서, F2로 열어 제목만 복사하고 나와도 "저장하지 않은 북마크 변경사항이
                    // 있습니다"가 떴다(2026-09-28 리포트 — 사용자는 사이드카에 옛 수정이 남은
                    // 것으로 의심했다). 되돌리기 기록도 이때 남긴다 — 그러지 않으면 제목 변경만
                    // Undo로 되돌릴 수 없었다.
                    let trimmed = buffer.trim();
                    if !trimmed.is_empty() && trimmed != node.title {
                        let new_title = trimmed.to_string();
                        outcome.rename = Some((node.id, new_title));
                    }
                    drag_state.editing = None;
                    }
                }
            } else {
                // .selectable(false) 핵심: 기본값(true)이면 egui가 Label을 "선택 가능한
                // 텍스트"로 취급해서 드래그 제스처를 자체 텍스트 선택 UI(마치 영역을
                // 지정하는 사각형처럼 보이는 것)가 가로채 버린다 — 우리가 원하는 "항목을
                // 드래그해서 재정렬" 동작과 충돌해서, 실제로는 텍스트 선택 박스가
                // 늘어나는 것처럼 보이고 정작 재정렬용 hover_target은 갱신되지 않는
                // 버그가 있었다. false로 꺼야 Sense::click_and_drag()가 온전히 우리 것.
                // 선택 하이라이트는 SIDEBAR_ACCENT 배경 + 흰 글자(2026-07-17 사용자 확정)
                // — 배경이 텍스트보다 먼저 칠해져야 흰 글자가 먹히므로 rect_filled 오버레이가
                // 아니라 Frame fill로 그린다(§7에 기록된 함정과 동일한 이유).
                let title_text = egui::RichText::new(node.title.clone());
                let title_text = if is_selected {
                    title_text.color(egui::Color32::WHITE)
                } else {
                    title_text
                };
                let label_response = egui::Frame::none()
                    .fill(if is_selected {
                        selection_color
                    } else {
                        egui::Color32::TRANSPARENT
                    })
                    .rounding(2.0)
                    .inner_margin(egui::Margin::symmetric(3.0, 1.0))
                    .show(ui, |ui| {
                        ui.add(
                            egui::Label::new(title_text)
                                .wrap()
                                .selectable(false)
                                .sense(Sense::click_and_drag()),
                        )
                    })
                    .inner;

                // 앱 시작 시 마지막으로 보던 페이지를 복원한 직후 한 번만: 선택(=그 페이지의
                // 활성 북마크, set_current_page의 자동 동기화)이 사이드바에서 보이는
                // 위치까지 스크롤한다(scroll_sidebar_to_active_once, main.rs 참고) —
                // 안 그러면 트리가 길 때 강조된 항목이 스크롤 밖에 있어도 알 방법이 없다.
                if is_selected && scroll_to_active_once {
                    label_response.scroll_to_me(Some(egui::Align::Center));
                    request_scroll_animation_repaint(drag_state);
                }

                // 화살표 키 순회로 선택이 방금 바뀐 경우(1회성 플래그): 새 선택 행이
                // 스크롤 영역의 보이는 범위를 벗어났으면 수직 중앙으로 스크롤한다
                // (2026-07-17 요청 — 예: 화면 맨 윗줄이 선택된 상태에서 ↑를 누르면 바로
                // 위 항목이 중앙으로 스르르 내려옴). scroll_to_me는 egui 0.29의
                // style.scroll_animation 기본값(거리 기반 0.1~0.3초)으로 이미 부드럽게
                // 애니메이션된다. 화면 안에 멀쩡히 보이면 스크롤하지 않는다(시점 튐 방지).
                if is_selected && drag_state.scroll_selected_into_view {
                    drag_state.scroll_selected_into_view = false;
                    let clip = ui.clip_rect();
                    let rect = label_response.rect;
                    if rect.top() < clip.top() || rect.bottom() > clip.bottom() {
                        label_response.scroll_to_me(Some(egui::Align::Center));
                        request_scroll_animation_repaint(drag_state);
                    }
                }

                if label_response.clicked() {
                    // "이미 선택된 항목 재클릭 = 이름 편집"은 사용자가 직접 고른
                    // 선택(selection_is_explicit)일 때만 — 페이지 이동만으로 자동 선택된
                    // 항목(set_current_page 참고)을 처음 클릭했는데 곧바로 편집 모드로
                    // 들어가면 당황스럽다. 자동 선택 항목의 첫 클릭은 일반 선택으로 처리
                    // (outcome 적용 시 explicit로 승격되므로 두 번째 클릭부터 편집).
                    if is_selected && selection_is_explicit {
                        drag_state.editing = Some((node.id, node.title.clone()));
                        drag_state.focus_editing = true;
                    } else {
                        outcome.selected = Some(node.id);
                        outcome.jump_page = Some(node.page);
                    }
                }
                label_response.context_menu(|ui| {
                    if ui.button("이름 바꾸기").clicked() {
                        drag_state.editing = Some((node.id, node.title.clone()));
                        drag_state.focus_editing = true;
                        ui.close_menu();
                    }
                    if ui.button("삭제").clicked() {
                        delete_id = Some(node.id);
                        ui.close_menu();
                    }
                });

                if label_response.drag_started() {
                    drag_state.dragging = Some(node.id);
                }
                if let Some(dragging) = drag_state.dragging {
                    // hovered()가 아니라 contains_pointer()를 써야 한다: egui 문서에
                    // 명시돼 있듯, 다른 위젯이 드래그 중일 때는 hovered()가 그 위젯 외에는
                    // 전부 false를 반환한다("In contrast to contains_pointer, this will be
                    // false whenever some other widget is being dragged" — response.rs).
                    // 그래서 드래그 중엔 대상 행의 hover_target이 절대 갱신되지 않아
                    // 삽입선/드롭 위치 표시가 아예 안 뜨는 버그가 있었다. contains_pointer()는
                    // 바로 이 "드래그 중 드롭 타겟 표시" 용도로 문서에 명시된 대안이다.
                    if dragging != node.id && label_response.contains_pointer() {
                        let pos_in_row = ui
                            .ctx()
                            .pointer_hover_pos()
                            .map(|p| (p.y - label_response.rect.top()) / label_response.rect.height().max(1.0))
                            .unwrap_or(0.5);
                        let drop_pos = if pos_in_row < 0.25 {
                            DropPosition::Before
                        } else if pos_in_row > 0.75 {
                            DropPosition::After
                        } else {
                            DropPosition::Inside
                        };
                        drag_state.hover_target = Some((node.id, drop_pos));
                    }
                }
            }
        });

        // 드래그 중인 항목 자체를 반투명하게 표시해 "지금 이게 들려서 옮겨지고 있다"는
        // 느낌을 준다(실제로 마우스를 따라다니는 고스트까지는 아니지만, 최소한 정적인
        // "선택 박스처럼 안 보이게"는 확실히 함).
        if drag_state.dragging == Some(node.id) {
            ui.painter().rect_filled(
                row_response.response.rect,
                2.0,
                ui.visuals().selection.bg_fill.gamma_multiply(0.25),
            );
        }

        // 드래그 대상 표시: Before/After는 삽입 위치를 나타내는 가로선, Inside는
        // "이 노드 안으로 들어감"을 나타내는 테두리 — Acrobat류 뷰어의 관례를 따른다.
        if !is_editing {
            if let Some((target_id, position)) = drag_state.hover_target {
                if target_id == node.id {
                    let rect = row_response.response.rect;
                    let painter = ui.painter();
                    let color = ui.visuals().selection.bg_fill;
                    match position {
                        DropPosition::Before => {
                            painter.hline(rect.x_range(), rect.top(), egui::Stroke::new(2.5_f32, color));
                        }
                        DropPosition::After => {
                            painter.hline(rect.x_range(), rect.bottom(), egui::Stroke::new(2.5_f32, color));
                        }
                        DropPosition::Inside => {
                            painter.rect_stroke(rect, 2.0, egui::Stroke::new(2.0_f32, color));
                        }
                    }
                }
            }
        }

        if has_children && !is_collapsed {
            ui.indent(("bm_children", node.id), |ui| {
                render_nodes(
                    ui,
                    &mut node.children,
                    drag_state,
                    current_selected,
                    selection_is_explicit,
                    scroll_to_active_once,
                    selection_color,
                    outcome,
                );
            });
        }

        // 기본 item_spacing(약 4pt)만으로는 두 줄 이상으로 줄바꿈되는 제목이 많을 때
        // 항목 경계가 잘 안 보인다는 피드백(2026-07-16) — 항목마다 약간의 여백을 더해
        // 시각적으로 구분되게 한다. 너무 벌리면 한 화면에 보이는 항목 수가 줄어 오히려
        // 활용성이 떨어지므로 적당히(3pt 추가, 기본과 합쳐 총 ~7pt)만 늘린다.
        ui.add_space(3.0);
    }

    if let Some(id) = delete_id {
        nodes.retain(|n| n.id != id);
        outcome.dirty = true;
    }
}

/// 해당 노드의 제목을 바꾼다. 찾았으면 `true`.
fn set_title(nodes: &mut [bookmark::BookmarkNode], id: Uuid, title: String) -> bool {
    for node in nodes {
        if node.id == id {
            node.title = title;
            return true;
        }
        if set_title(&mut node.children, id, title.clone()) {
            return true;
        }
    }
    false
}

/// 접힌 노드의 자식은 제외하고, 화면에 실제로 보이는 순서대로 id를 나열한다.
/// 화살표 키 네비게이션(위/아래)이 이 순서를 따라간다.
fn flatten_visible(nodes: &[BookmarkNode], collapsed: &HashSet<Uuid>, out: &mut Vec<Uuid>) {
    for n in nodes {
        out.push(n.id);
        if !n.children.is_empty() && !collapsed.contains(&n.id) {
            flatten_visible(&n.children, collapsed, out);
        }
    }
}

fn find_title(nodes: &[BookmarkNode], id: Uuid) -> Option<String> {
    for n in nodes {
        if n.id == id {
            return Some(n.title.clone());
        }
        if let Some(title) = find_title(&n.children, id) {
            return Some(title);
        }
    }
    None
}

fn find_page(nodes: &[BookmarkNode], id: Uuid) -> Option<u32> {
    for n in nodes {
        if n.id == id {
            return Some(n.page);
        }
        if let Some(page) = find_page(&n.children, id) {
            return Some(page);
        }
    }
    None
}

fn has_children(nodes: &[BookmarkNode], id: Uuid) -> bool {
    for n in nodes {
        if n.id == id {
            return !n.children.is_empty();
        }
        if has_children(&n.children, id) {
            return true;
        }
    }
    false
}

/// id 노드까지 내려가는 조상 id 목록(가까운 조상부터든 먼 조상부터든 순서는 상관없음 —
/// 호출부에서 전부 펼치는 데만 씀).
fn ancestors_of(nodes: &[BookmarkNode], id: Uuid) -> Vec<Uuid> {
    let mut path = Vec::new();
    find_ancestors(nodes, id, &mut path);
    path
}

fn find_ancestors(nodes: &[BookmarkNode], id: Uuid, path: &mut Vec<Uuid>) -> bool {
    for n in nodes {
        if n.id == id {
            return true;
        }
        path.push(n.id);
        if find_ancestors(&n.children, id, path) {
            return true;
        }
        path.pop();
    }
    false
}

#[cfg(test)]
mod clipboard_title_tests {
    use super::{bookmark_title_from_clipboard, MAX_CLIPBOARD_TITLE_CHARS};

    /// PDF에서 여러 줄에 걸쳐 복사한 제목은 한 줄로 합쳐진다.
    #[test]
    fn line_breaks_and_runs_of_spaces_collapse() {
        assert_eq!(
            bookmark_title_from_clipboard("  제3장\n  전시 의료\t지원 체계 \r\n").as_deref(),
            Some("제3장 전시 의료 지원 체계")
        );
    }

    #[test]
    fn blank_clipboard_falls_back() {
        assert_eq!(bookmark_title_from_clipboard(""), None);
        assert_eq!(bookmark_title_from_clipboard(" \n\t "), None);
    }

    /// 글자(char) 단위로 자르므로 한글 중간에서 바이트가 깨지지 않는다.
    #[test]
    fn long_text_is_truncated_by_chars() {
        let title = bookmark_title_from_clipboard(&"가".repeat(500)).unwrap();
        assert_eq!(title.chars().count(), MAX_CLIPBOARD_TITLE_CHARS);
    }
}

/// 북마크 제목 편집 중 뷰어를 눌러도 편집이 끝나지 않아야 한다(예약 10, 2026-10-01).
#[cfg(test)]
mod viewer_press_tests {
    use super::pressed_in_viewer;

    fn viewer() -> egui::Rect {
        egui::Rect::from_min_size(egui::pos2(200.0, 50.0), egui::vec2(600.0, 800.0))
    }

    /// 한 프레임을 흘려 넣고 판정 결과를 돌려준다.
    fn judge(events: Vec<egui::Event>, rect: Option<egui::Rect>) -> bool {
        let ctx = egui::Context::default();
        let input = egui::RawInput { events, ..Default::default() };
        let mut verdict = false;
        ctx.run(input, |ctx| verdict = pressed_in_viewer(ctx, rect));
        verdict
    }

    fn press(pos: egui::Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::default(),
        }
    }

    /// 뷰어 안을 누르면 참 — 이때는 편집을 이어 간다.
    #[test]
    fn a_press_inside_the_viewer_counts() {
        let inside = viewer().center();
        assert!(judge(vec![egui::Event::PointerMoved(inside), press(inside, true)], Some(viewer())));
    }

    /// 사이드바·툴바 쪽을 누르면 거짓 — 예전대로 편집이 끝난다.
    #[test]
    fn a_press_outside_the_viewer_does_not_count() {
        let outside = egui::pos2(60.0, 300.0);
        assert!(!judge(vec![egui::Event::PointerMoved(outside), press(outside, true)], Some(viewer())));
    }

    /// 누르지 않고 마우스만 뷰어 위에 있으면 거짓 — Tab처럼 키로 포커스를 옮긴 경우를 가른다.
    #[test]
    fn merely_hovering_the_viewer_does_not_count() {
        let inside = viewer().center();
        assert!(!judge(vec![egui::Event::PointerMoved(inside)], Some(viewer())));
    }

    /// 아직 뷰어를 한 번도 그리지 않았으면(자리를 모르면) 거짓 — 모르는 것을 참으로 보지 않는다.
    #[test]
    fn an_unknown_viewer_rect_is_not_a_press() {
        let inside = viewer().center();
        assert!(!judge(vec![egui::Event::PointerMoved(inside), press(inside, true)], None));
    }
}

/// 북마크 탭 머리 줄의 여백(2026-10-03 리포트).
#[cfg(test)]
mod header_padding_tests {
    use super::HEADER_HEIGHT;

    /// 아이콘 위아래로 **눈에 보이는** 빈 자리가 잉크 높이의 30% 안팎이어야 한다.
    ///
    /// **머리 줄 안쪽만 재면 안 된다.** 처음에 그렇게 쟀다가 안쪽은 31%인데 화면은 그대로인
    /// 일이 있었다 — 여백의 대부분이 머리 줄 *바깥*(탭 아래 띄움, 전역 줄 간격, 옛 균형 보정)에서
    /// 왔기 때문이다. 그래서 **탭 아랫변에서 구분선까지** 실제 쌓이는 모양 그대로 잰다.
    #[test]
    fn the_space_around_the_icon_row_is_not_too_tall() {
        let ctx = egui::Context::default();
        crate::fonts::install_fonts(&ctx);
        crate::fonts::install_style(&ctx);
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(400.0, 600.0))),
            ..Default::default()
        };
        let (mut above, mut below) = (0.0_f32, 0.0_f32);
        let _ = ctx.run(input, |ctx| {
            egui::SidePanel::left("s").show(ctx, |ui| {
                // sidebar::show가 쌓는 순서 그대로.
                ui.add_space(6.0);
                let mut tab = crate::app::SidebarTab::Bookmarks;
                super::tab_bar(ui, &mut tab);
                let tab_bottom = ui.cursor().min.y;

                let outer = std::mem::replace(&mut ui.spacing_mut().item_spacing.y, 0.0);
                let (header, _) =
                    ui.allocate_exact_size(egui::vec2(ui.available_width(), HEADER_HEIGHT), egui::Sense::hover());
                let mut child = ui.new_child(
                    egui::UiBuilder::new().max_rect(header).layout(egui::Layout::left_to_right(egui::Align::Center)),
                );
                let button = crate::icons::button(&mut child, crate::icons::ADD, "").rect;
                let before_separator = ui.cursor().min.y;
                ui.separator();
                ui.spacing_mut().item_spacing.y = outer;

                // 글리프 잉크는 글자 줄 높이의 0.797배다(폰트 metrics: (856-40)/1024).
                let ink = crate::icons::SIZE * 816.0 / 1024.0;
                let ink_top = button.center().y - ink / 2.0;
                above = ink_top - tab_bottom;
                below = before_separator - (ink_top + ink);
            });
        });
        let ink = crate::icons::SIZE * 816.0 / 1024.0;
        for (name, gap) in [("위", above), ("아래", below)] {
            let ratio = gap / ink;
            assert!(
                (0.20..=0.40).contains(&ratio),
                "{name} 여백이 {gap:.1}pt — 잉크 높이의 {:.0}%다. 30% 안팎이어야 한다",
                ratio * 100.0
            );
        }
        assert!((above - below).abs() < 1.0, "위({above:.1})와 아래({below:.1})가 어긋난다");
    }
}
