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
    egui::SidePanel::left("bookmarks_sidebar")
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

            ui.horizontal(|ui| {
                ui.heading("북마크");
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
            });
            ui.separator();

            let mut outcome = RenderOutcome::default();
            let current_selected = app.selected_bookmark;

            egui::ScrollArea::vertical().show(ui, |ui| {
                render_nodes(ui, &mut app.bookmarks, &mut drag_state, current_selected, &mut outcome);
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
            // 다른 위젯이 키보드를 쓰고 있을 때는 가로채지 않는다.
            if !ctx.wants_keyboard_input() && drag_state.editing.is_none() {
                if let Some(selected) = current_selected {
                    // F2 — 선택된 항목을 곧바로 이름 편집 모드로. 더블클릭/재클릭과 동일한
                    // 진입점이지만 키보드만으로 접근 가능하게 하는 게 목적.
                    if ctx.input(|i| i.key_pressed(egui::Key::F2)) {
                        if let Some(title) = find_title(&app.bookmarks, selected) {
                            drag_state.editing = Some((selected, title));
                            drag_state.focus_editing = true;
                        }
                    }

                    let mut visible = Vec::new();
                    flatten_visible(&app.bookmarks, &drag_state.collapsed, &mut visible);
                    if let Some(pos) = visible.iter().position(|id| *id == selected) {
                        ctx.input(|i| {
                            if i.key_pressed(egui::Key::ArrowDown) && pos + 1 < visible.len() {
                                outcome.selected = Some(visible[pos + 1]);
                            }
                            if i.key_pressed(egui::Key::ArrowUp) && pos > 0 {
                                outcome.selected = Some(visible[pos - 1]);
                            }
                            // 좌/우 화살표는 선택된 항목 자체가 자식을 가지면 그 항목을,
                            // 아니면(리프 노드) 그 부모를 접고/편다 — "선택된 항목이 속한
                            // 레벨"을 조작한다는 요구사항. 예전엔 선택된 항목 자신이 자식을
                            // 가질 때만 작동해서 리프 노드에서는 아무 반응이 없었다.
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
                app.selected_bookmark = Some(selected);
            }
            if outcome.dirty {
                app.bookmarks_dirty = true;
            }
        });
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
            // 폭을 반드시 add_sized로 고정해야 한다 — Button은 프레임 유무/텍스트 크기에 따라
            // 실제 폭이 미묘하게 달라져서, 자식 있는 행과 없는 행의 라벨 시작 x좌표가
            // 안 맞는(자식 있는 쪽이 왼쪽으로 살짝 밀리는) 정렬 어긋남이 있었다.
            let toggle_size = egui::vec2(18.0, ui.spacing().interact_size.y);
            if has_children {
                let icon = if is_collapsed { ">" } else { "v" };
                if ui
                    .add_sized(toggle_size, egui::Button::new(icon).small().frame(false))
                    .clicked()
                {
                    if is_collapsed {
                        drag_state.collapsed.remove(&node.id);
                    } else {
                        drag_state.collapsed.insert(node.id);
                    }
                }
            } else {
                ui.add_space(toggle_size.x);
            }

            if is_editing {
                let (_, buffer) = drag_state.editing.as_mut().unwrap();
                let edit_id = Id::new(("bm_edit", node.id));
                let edit_response = ui.add(
                    egui::TextEdit::singleline(buffer)
                        .desired_width(ui.available_width())
                        .id(edit_id),
                );
                if drag_state.focus_editing {
                    ui.memory_mut(|m| m.request_focus(edit_id));
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
                let label = egui::Label::new(node.title.clone())
                    .wrap()
                    .selectable(false)
                    .sense(Sense::click_and_drag());
                let label_response = ui.add(label);

                if is_selected {
                    ui.painter().rect_filled(
                        label_response.rect.expand(2.0),
                        2.0,
                        ui.visuals().selection.bg_fill.gamma_multiply(0.5),
                    );
                }

                if label_response.clicked() {
                    if is_selected {
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
                render_nodes(ui, &mut node.children, drag_state, current_selected, outcome);
            });
        }
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
