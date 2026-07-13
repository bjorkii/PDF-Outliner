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
