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
}

/// 재귀 전체에 걸쳐 누적되는 결과. 재귀 호출마다 지역 변수를 새로 선언하면 하위 노드의
/// 클릭이 상위 호출로 전파되지 않고 버려지는 버그가 생기므로, 하나의 구조체를 `&mut`로
/// 재귀 전체에 그대로 넘긴다.
#[derive(Default)]
struct RenderOutcome {
    jump_page: Option<u32>,
    selected: Option<Uuid>,
    dirty: bool,
}

pub fn show(ctx: &egui::Context, app: &mut PdfViewerApp) {
    let panel_response = egui::SidePanel::left("bookmarks_sidebar")
        .resizable(true)
        .default_width(240.0)
        .min_width(90.0)
        .show(ctx, |ui| {
            let drag_id = Id::new("bookmark_drag_state");
            let mut drag_state = ctx
                .data_mut(|d| d.get_temp::<DragState>(drag_id))
                .unwrap_or_default();

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
            let header_height = 36.0;
            // 아래쪽엔 item_spacing + 구분선 자체 패딩(합계 ~6pt)이 붙는데 위쪽 패널
            // 마진은 ~1pt뿐이라, 36pt 밴드 정중앙에 놓아도 버튼이 위 테두리 쪽으로
            // 치우쳐 보인다(스크린샷 픽셀 실측: 위 24px vs 아래 31px @2x) — 그 차이만큼
            // 밴드를 내려 균형을 맞춘다(3pt 적용 후 재실측 29px vs 30px).
            ui.add_space(3.0);
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

                if ui
                    .button("+")
                    .on_hover_text("추가 (선택된 항목의 하위, 없으면 최상위) — Cmd+B")
                    .clicked()
                {
                    add_new_bookmark(app, &mut drag_state);
                }

                let delete_enabled = app.selected_bookmark.is_some();
                if ui
                    .add_enabled(delete_enabled, egui::Button::new("-"))
                    .on_hover_text("삭제 (Delete)")
                    .clicked()
                {
                    app.delete_selected_bookmark();
                }

                let undo_enabled = !app.bookmark_undo_stack.is_empty();
                if ui
                    .add_enabled(undo_enabled, egui::Button::new("Undo"))
                    .on_hover_text("실행취소 (Cmd+Z)")
                    .clicked()
                {
                    app.undo_bookmarks();
                }

                let redo_enabled = !app.bookmark_redo_stack.is_empty();
                if ui
                    .add_enabled(redo_enabled, egui::Button::new("Redo"))
                    .on_hover_text("다시 실행 (Cmd+Shift+Z)")
                    .clicked()
                {
                    app.redo_bookmarks();
                }

                let measured = ui.min_rect().max.x - buttons_start_x;
                ctx.data_mut(|d| d.insert_temp(buttons_width_id, measured));
            }
            ui.separator();

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

            egui::ScrollArea::vertical().show(ui, |ui| {
                render_nodes(
                    ui,
                    &mut app.bookmarks,
                    &mut drag_state,
                    current_selected,
                    app.selection_is_explicit,
                    scroll_to_active_once,
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
                            }
                            if i.key_pressed(egui::Key::ArrowUp) && pos > 0 {
                                let target = visible[pos - 1];
                                outcome.selected = Some(target);
                                outcome.jump_page = find_page(&app.bookmarks, target);
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
            if outcome.dirty {
                app.bookmarks_dirty = true;
            }
        });

    // 포커스가 사이드바일 때 패널 둘레에 테두리를 그려 "지금 화살표 키가 북마크
    // 탐색으로 동작한다"는 걸 시각적으로 알려준다(Tab/클릭으로 전환 — FocusArea 문서
    // 참고). 처음엔 패널 닫기 전에 ui.painter()로 그렸는데, 패널 안의 painter는 패널
    // 내용 영역으로 클리핑돼서 위/아래 변이 잘리고 좌/우 변만 보였다(사용자 리포트,
    // 2026-07-17) — 패널이 닫힌 뒤 클리핑 없는 Foreground 레이어 painter로 패널 전체
    // rect에 그려야 네 변이 다 나온다. 스트로크가 화면 경계에서 잘리지 않게 절반
    // 폭만큼 안쪽으로 줄인다.
    if app.focus_area == crate::app::FocusArea::Sidebar {
        let panel_rect = panel_response.response.rect;
        let stroke = egui::Stroke::new(2.0_f32, ctx.style().visuals.selection.bg_fill);
        ctx.layer_painter(egui::LayerId::new(
            egui::Order::Foreground,
            Id::new("sidebar_focus_border"),
        ))
        .rect_stroke(panel_rect.shrink(1.0), 2.0_f32, stroke);
    }
}

/// "+"버튼과 Cmd+B가 공유하는 로직: 선택된 항목의 자식(없으면 최상위)으로 새 북마크를
/// 추가하고, 조상 노드를 펼쳐서 보이게 한 뒤, 곧바로 이름 편집 모드로 들어간다.
fn add_new_bookmark(app: &mut PdfViewerApp, drag_state: &mut DragState) {
    let new_id = app.add_bookmark_under_selection();
    for ancestor in ancestors_of(&app.bookmarks, new_id) {
        drag_state.collapsed.remove(&ancestor);
    }
    drag_state.editing = Some((new_id, "새 북마크".to_string()));
    drag_state.focus_editing = true;
}

fn render_nodes(
    ui: &mut egui::Ui,
    nodes: &mut Vec<BookmarkNode>,
    drag_state: &mut DragState,
    current_selected: Option<Uuid>,
    selection_is_explicit: bool,
    scroll_to_active_once: bool,
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
                let edit_response = ui.add(
                    egui::TextEdit::singleline(buffer)
                        .desired_width(ui.available_width())
                        .id(edit_id),
                );
                if drag_state.focus_editing {
                    ui.memory_mut(|m| m.request_focus(edit_id));
                    // 새로 추가한 항목(또는 F2/재클릭으로 편집 시작한 항목)이 스크롤 영역
                    // 밖에 있을 수 있으니 편집 필드가 보이는 위치까지 스크롤한다 — 사이드바가
                    // 꽉 찬 상태에서 Cmd+B로 추가하면 새 항목이 안 보이던 문제.
                    edit_response.scroll_to_me(Some(egui::Align::Center));
                    // 텍스트 전체를 선택 상태로 둬서, 새로 만든 placeholder("새 북마크")나
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
                    // Enter뿐 아니라 다른 곳을 클릭해 포커스를 잃어도 커밋한다(Finder식 관례).
                    let trimmed = buffer.trim();
                    if !trimmed.is_empty() {
                        node.title = trimmed.to_string();
                        outcome.dirty = true;
                    }
                    drag_state.editing = None;
                }
            } else {
                // .selectable(false) 핵심: 기본값(true)이면 egui가 Label을 "선택 가능한
                // 텍스트"로 취급해서 드래그 제스처를 자체 텍스트 선택 UI(마치 영역을
                // 지정하는 사각형처럼 보이는 것)가 가로채 버린다 — 우리가 원하는 "항목을
                // 드래그해서 재정렬" 동작과 충돌해서, 실제로는 텍스트 선택 박스가
                // 늘어나는 것처럼 보이고 정작 재정렬용 hover_target은 갱신되지 않는
                // 버그가 있었다. false로 꺼야 Sense::click_and_drag()가 온전히 우리 것.
                // (하이라이트는 아래 is_selected 오버레이 하나로 통일 — 예전의 "현재 페이지
                // 활성 북마크" 회색 배경 강조는 선택이 페이지와 자동 동기화되면서 중복이라
                // 제거, 2026-07-17 사용자 확정. Frame의 inner_margin은 행 간격 유지용.)
                let label_response = egui::Frame::none()
                    .inner_margin(egui::Margin::symmetric(3.0, 1.0))
                    .show(ui, |ui| {
                        ui.add(
                            egui::Label::new(node.title.clone())
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
                }

                if is_selected {
                    ui.painter().rect_filled(
                        label_response.rect.expand(2.0),
                        2.0,
                        ui.visuals().selection.bg_fill.gamma_multiply(0.5),
                    );
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
