use crate::app::{PdfViewerApp, ViewportState};

/// 단축키 표시용 modifier 이름 — egui의 `Modifiers::command`는 이미 맥/윈도우를
/// 알아서 Cmd/Ctrl로 구분해 처리하므로(키 입력 검사 쪽은 손댈 게 없음) 여기선
/// 툴팁 문구에 보여줄 라벨만 OS별로 다르게 고른다.
fn modifier_label() -> &'static str {
    if cfg!(target_os = "macos") {
        "Cmd"
    } else {
        "Ctrl"
    }
}

pub fn show(ctx: &egui::Context, app: &mut PdfViewerApp) {
    egui::TopBottomPanel::top("toolbar").show(ctx, |ui| {
        ui.horizontal(|ui| {
            let open_button_response = ui.button("파일 열기");
            if open_button_response.clicked() {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("PDF", &["pdf"])
                    .pick_file()
                {
                    app.request_open_file(path);
                }
            }
            show_recent_files_dropdown(ui, app, &open_button_response);

            let save_button = ui.add_enabled(app.bookmarks_dirty, egui::Button::new("저장"));
            if save_button.on_hover_text("PDF에 북마크 저장").clicked() {
                app.save_bookmarks_to_pdf();
            }

            // save_bookmarks_to_pdf가 원본을 못 찾으면(이름변경/이동/삭제) 여기서 세운
            // save_as_requested를 받아 "다른 이름으로 저장" 대화상자를 띄운다 — 파일
            // 다이얼로그는 관례상 toolbar.rs에서 열고(다른 내보내기/가져오기 버튼과 동일),
            // 실제 쓰기는 app.rs의 save_bookmarks_as가 담당.
            if app.save_as_requested {
                app.save_as_requested = false;
                let default_name = app
                    .current_file
                    .as_ref()
                    .map(|p| crate::app::display_filename(p))
                    .unwrap_or_else(|| "document.pdf".to_string());
                if let Some(new_path) = rfd::FileDialog::new()
                    .add_filter("PDF", &["pdf"])
                    .set_file_name(&default_name)
                    .save_file()
                {
                    app.save_bookmarks_as(new_path);
                }
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

            // 정보성 툴팁이라 on_hover_ui로 충분 — egui 툴팁은 원래 마우스가 벗어나면
            // 자동으로 닫힌다(사용자가 요청한 "hover 종료되면 창도 꺼지게"가 기본 동작).
            // \t(tab)만으로는 실제 탭 스톱 정렬이 안 되고 각 줄의 키 텍스트 길이만큼
            // 들쑥날쑥해진다(egui는 tab을 "고정폭만큼 더 전진"으로만 처리, 열 정렬 개념이
            // 없음) — 사용자가 원한 표 형태 정렬은 egui::Grid로 컬럼 자체를 나눠야 나온다.
            let m = modifier_label();
            ui.label("단축키").on_hover_ui(|ui| {
                egui::Grid::new("shortcut_help_grid")
                    .num_columns(2)
                    .spacing([16.0, 8.0])
                    .show(ui, |ui| {
                        for (key, desc) in [
                            (format!("{m}+B"), "북마크 추가"),
                            ("F2".to_string(), "북마크 수정"),
                            ("Delete".to_string(), "북마크 삭제"),
                            (format!("{m}+S"), "북마크 저장"),
                            (format!("{m}+F"), "내용 검색"),
                            (format!("{m}+["), "이전 화면"),
                            (format!("{m}+]"), "다음 화면"),
                            ("Tab".to_string(), "북마크↔뷰어"),
                        ] {
                            ui.label(key);
                            ui.label(desc);
                            ui.end_row();
                        }
                    });
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
                // .truncate()가 잘렸을 때(egui::Label의 elided 처리) 전체 텍스트를 hover
                // 툴팁으로 자동으로 붙여준다(egui-0.29.1 label.rs 253행) — 별도로
                // on_hover_text를 또 붙이면 툴팁이 위아래 두 개로 겹쳐 보인다(사용자 리포트,
                // 2026-07-16). .truncate()만으로 충분.
                ui.add(egui::Label::new(message).truncate());
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

/// "파일 열기" 버튼에 마우스를 올리면(클릭 아님) 최근 연 파일 목록을 드롭다운으로 보여준다.
/// egui의 `on_hover_ui`/`on_hover_text`는 클릭이 안 되는 순수 정보성 툴팁이라(내용 위로
/// 마우스가 가면 클릭 이벤트가 그 아래 위젯으로 새지 않고 그냥 사라짐) 이 목적엔 못 쓰고,
/// `egui::Area`로 직접 떠 있는 패널을 만들어 "버튼 또는 이 패널 위에 마우스가 있는 동안"만
/// 열려있게 수동으로 상태를 관리한다(`ctx.data_mut`의 임시 저장 — sidebar.rs의
/// `DragState` 패턴과 동일한 이유: 프레임을 넘어 지속되는 상태가 필요해서).
fn show_recent_files_dropdown(ui: &mut egui::Ui, app: &mut PdfViewerApp, button: &egui::Response) {
    let menu_id = egui::Id::new("recent_files_dropdown_open");
    let mut open = ui.ctx().data(|d| d.get_temp::<bool>(menu_id)).unwrap_or(false);

    if button.hovered() {
        open = true;
    }

    if open && !app.recent_files.is_empty() {
        let mut path_to_open: Option<std::path::PathBuf> = None;
        let dropdown_width = button.rect.width().max(320.0);
        let area_response = egui::Area::new(egui::Id::new("recent_files_dropdown_area"))
            .fixed_pos(button.rect.left_bottom())
            .order(egui::Order::Foreground)
            .show(ui.ctx(), |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.set_max_width(dropdown_width);
                    for path in &app.recent_files {
                        let filename = crate::app::display_filename(path);
                        // 파일명은 위 줄에 이미 있으니 아래 줄엔 상위 폴더 경로만(파일명
                        // 중복 표시하지 않음) — 루트 바로 아래 파일 등 부모가 없으면 생략.
                        // 폴더명에도 한글이 있을 수 있어 파일명과 마찬가지로 NFC 정규화
                        // 필요(§7 "한글 파일명 자소 분리" 참고).
                        use unicode_normalization::UnicodeNormalization;
                        let dir_only = path
                            .parent()
                            .map(|p| p.to_string_lossy().nfc().collect::<String>())
                            .filter(|s| !s.is_empty());

                        // 파일명 + 경로를 한 버튼 안에 두 줄로 — 경로가 길면 줄바꿈되고,
                        // 그만큼 다음 항목과의 간격도 자연히 넓어진다(요청사항). 스타일이
                        // 서로 다른 두 줄을 한 버튼에 넣으려면 egui::Button이 받는 단순
                        // WidgetText로는 안 되고 LayoutJob으로 섹션별 폰트/색을 지정해야 한다.
                        let mut job = egui::text::LayoutJob::default();
                        job.wrap.max_width = dropdown_width;
                        job.append(
                            &filename,
                            0.0,
                            egui::TextFormat {
                                font_id: egui::FontId::proportional(14.0),
                                color: ui.visuals().text_color(),
                                ..Default::default()
                            },
                        );
                        if let Some(dir_only) = dir_only {
                            job.append(
                                &format!("\n{dir_only}"),
                                0.0,
                                egui::TextFormat {
                                    font_id: egui::FontId::proportional(11.0),
                                    color: ui.visuals().weak_text_color(),
                                    ..Default::default()
                                },
                            );
                        }

                        if ui.button(job).clicked() {
                            path_to_open = Some(path.clone());
                        }
                    }
                });
            })
            .response;

        if let Some(path) = path_to_open {
            app.open_recent_file(path);
            open = false;
        } else {
            // button.hovered()/area_response.hovered()만 보면, 마우스가 버튼에서 팝업
            // 쪽으로 이동하는 도중(둘 사이 미세한 틈, 팝업 프레임 안쪽 여백 등) 어느 쪽도
            // true가 안 되는 프레임이 낄 수 있어 클릭하기도 전에 목록이 닫혀버렸다(사용자
            // 리포트, 2026-07-16). 위젯 하나하나의 hover 판정 대신, 실제 포인터 좌표가
            // "버튼 rect 확장본 ∪ 팝업 rect 확장본" 안에 있는지를 직접 검사해 그 틈을 흡수한다.
            let hovering = ui.ctx().input(|i| i.pointer.hover_pos()).is_some_and(|pos| {
                button.rect.expand(6.0).contains(pos) || area_response.rect.expand(10.0).contains(pos)
            });
            if !hovering {
                open = false;
            }
        }
    } else {
        open = false;
    }

    ui.ctx().data_mut(|d| d.insert_temp(menu_id, open));
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
