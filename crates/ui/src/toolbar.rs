use crate::app::{PdfViewerApp, ViewportState};

pub fn show(ctx: &egui::Context, app: &mut PdfViewerApp) {
    egui::TopBottomPanel::top("toolbar").show(ctx, |ui| {
        ui.horizontal(|ui| {
            if ui.button("파일 열기").clicked() {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("PDF", &["pdf"])
                    .pick_file()
                {
                    app.request_open_file(path);
                }
            }

            let save_button = ui.add_enabled(app.bookmarks_dirty, egui::Button::new("저장"));
            if save_button.on_hover_text("PDF에 북마크 저장").clicked() {
                app.save_bookmarks_to_pdf();
            }

            ui.separator();

            ui.menu_button("북마크 내보내기", |ui| {
                if ui.button("CSV로 내보내기").clicked() {
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("CSV", &["csv"])
                        .set_file_name("bookmarks.csv")
                        .save_file()
                    {
                        app.export_bookmarks_csv(path);
                    }
                    ui.close_menu();
                }
                if ui.button("Excel로 내보내기").clicked() {
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("Excel", &["xlsx"])
                        .set_file_name("bookmarks.xlsx")
                        .save_file()
                    {
                        app.export_bookmarks_xlsx(path);
                    }
                    ui.close_menu();
                }
            });

            ui.menu_button("북마크 가져오기", |ui| {
                if ui.button("CSV에서 가져오기").clicked() {
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("CSV", &["csv"])
                        .pick_file()
                    {
                        app.import_bookmarks_csv(path);
                    }
                    ui.close_menu();
                }
                if ui.button("Excel에서 가져오기").clicked() {
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("Excel", &["xlsx"])
                        .pick_file()
                    {
                        app.import_bookmarks_xlsx(path);
                    }
                    ui.close_menu();
                }
            });

            ui.separator();

            // 트랙패드 핀치/마우스 휠 줌과 별개로, 비전문 사용자를 위한 명시적 버튼 병행 배치
            if ui.button("➖").on_hover_text("축소").clicked() {
                app.viewport.zoom_by(0.8);
            }
            ui.label(format!("{:.0}%", app.viewport.zoom * 100.0));
            if ui.button("➕").on_hover_text("확대").clicked() {
                app.viewport.zoom_by(1.25);
            }
            if ui.button("100%").clicked() {
                app.viewport.zoom = 1.0;
            }

            ui.separator();

            if ui.button("◀").on_hover_text("이전 페이지").clicked() {
                let prev = app.current_page.saturating_sub(1).max(1);
                app.go_to_page(prev);
            }

            // "현재쪽" 입력창 폭을 "전체쪽" 숫자의 자릿수에 맞춘다 — 예전엔 50px 고정이라
            // 총 페이지가 한 자릿수여도 입력창만 과도하게 넓어 보였다.
            let digits = app.total_pages.max(1).to_string().len().max(1) as f32;
            let field_width = digits * 8.0 + 16.0;
            let response = ui.add(
                egui::TextEdit::singleline(&mut app.page_number_input).desired_width(field_width),
            );
            if response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                if let Ok(page) = app.page_number_input.trim().parse::<u32>() {
                    app.go_to_page(page);
                } else {
                    // 파싱 실패 시 현재 페이지 값으로 되돌림
                    app.page_number_input = app.current_page.to_string();
                }
            }
            ui.label(format!("/ {}", app.total_pages));

            if ui.button("▶").on_hover_text("다음 페이지").clicked() {
                let next = (app.current_page + 1).min(app.total_pages.max(1));
                app.go_to_page(next);
            }

            if let Some(message) = &app.status_message {
                ui.separator();
                ui.label(message);
            }

            // 검색 UI를 메인바 제일 오른쪽에 고정한다. 남은 가로 공간을 이 하위 레이아웃이
            // 통째로 차지한 뒤 오른쪽에서 왼쪽 방향으로 채워나가므로(egui의
            // Layout::right_to_left 관례), 이 스코프 안에서 먼저 추가한 위젯이 가장
            // 오른쪽에 온다 — 그래서 눈에 보이는 순서([검색창][🔍][◀][N/M][▶])와는
            // 반대로 ▶부터 추가한다.
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let has_results = !app.search_matches.is_empty();
                let searching = app.search_running.is_some();

                let next_response = ui
                    .add_enabled(has_results, egui::Button::new("▶"))
                    .on_hover_text("다음 결과 (Enter)");
                if next_response.clicked() {
                    app.search_next();
                }
                // 검색이 막 끝나 결과가 나오면 포커스를 이 버튼으로 옮겨준다 — 그래야
                // 검색창에 남아있던 포커스가 없어져서(요청사항: 검색 버튼을 누르면 포커스가
                // 검색창에서 사라지도록) 이후 Enter가 검색창 재검색이 아니라 이 버튼의
                // 클릭으로 해석된다(egui는 Sense::click 위젯이 포커스를 가진 상태에서
                // Enter/Space를 누르면 클릭으로 처리한다).
                if app.request_focus_next_result {
                    next_response.request_focus();
                    app.request_focus_next_result = false;
                }
                if has_results {
                    ui.label(format!(
                        "{} / {}",
                        app.search_current_index + 1,
                        app.search_matches.len()
                    ));
                }
                if ui
                    .add_enabled(has_results, egui::Button::new("◀"))
                    .on_hover_text("이전 결과")
                    .clicked()
                {
                    app.search_previous();
                }
                if searching {
                    ui.spinner();
                }
                if ui
                    .add_enabled(!searching, egui::Button::new("🔍"))
                    .on_hover_text("검색 실행 (Enter)")
                    .clicked()
                {
                    app.execute_search();
                }

                let search_field_id = egui::Id::new("pdf_search_field");
                let search_response = ui.add(
                    egui::TextEdit::singleline(&mut app.search_query)
                        .id(search_field_id)
                        .hint_text("검색어")
                        .desired_width(160.0),
                );
                if app.request_focus_search {
                    ui.memory_mut(|m| m.request_focus(search_field_id));
                    app.request_focus_search = false;
                }
                // 검색창에 포커스가 있는 동안의 Enter는 항상 "새로 검색"이다 — 결과를
                // 순회하던 중이라도 다른 검색어를 입력하고 Enter를 누르면 그 새 검색어로
                // 다시 검색해야 한다(예전엔 has_results를 봐서 "다음 결과로 이동"으로
                // 잘못 처리했었음 — 이전 검색어의 결과를 계속 순회하는 버그였음).
                if search_response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    app.execute_search();
                }
            });
        });
    });
}

/// Ctrl+휠(Windows 마우스 휠 줌 관례) 처리. viewer_panel에서 스크롤 이벤트 처리 시 호출.
pub fn handle_scroll_zoom(ctx: &egui::Context, viewport: &mut ViewportState) {
    let ctrl_held = ctx.input(|i| i.modifiers.ctrl);
    if !ctrl_held {
        return;
    }
    let scroll_delta = ctx.input(|i| i.smooth_scroll_delta.y);
    if scroll_delta != 0.0 {
        let factor = 1.0 + (scroll_delta * 0.001);
        viewport.zoom_by(factor);
    }
}
