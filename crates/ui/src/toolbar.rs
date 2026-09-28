use crate::app::{FitMode, PdfViewerApp, ViewportState};

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

/// 북마크·OCR 메뉴의 가로폭 — 가장 긴 항목("Excel에서 가져오기…", "hOCR로 내보내기…")이
/// 한 줄에 들어가는 만큼만(2026-09-23: 기본 220pt는 너무 넓다는 지적).
const MENU_WIDTH: f32 = 186.0;

/// 파일 메뉴의 가로폭 — 최근 파일 목록(파일명 + 상위 폴더 경로 두 줄)에 맞춘다.
const RECENT_FILE_WIDTH: f32 = 320.0;

/// 아이콘 버튼 한 변 크기 — 옆의 ➖/➕ 텍스트 버튼과 높이가 어울리게 잡은 값.
const ICON_BUTTON_SIZE: f32 = 22.0;

/// 커스텀 벡터 아이콘을 그리는 버튼. `egui::Button`은 텍스트(WidgetText)만 받고 임의
/// 도형을 못 그리므로, 사이드바 폴드 아이콘(sidebar.rs)과 같은 방식으로 직접 rect를
/// 할당하고 그 위에 수동으로 그린다 — `ui.style().interact(&response)`로 실제 버튼과
/// 동일한 hover/눌림 배경·테두리를 얻어 다른 버튼들과 시각적으로 어울리게 한다.
fn icon_button(
    ui: &mut egui::Ui,
    draw: impl FnOnce(&egui::Painter, egui::Rect, egui::Color32),
) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ICON_BUTTON_SIZE, ICON_BUTTON_SIZE), egui::Sense::click());
    if ui.is_rect_visible(rect) {
        let visuals = ui.style().interact(&response);
        ui.painter()
            .rect(rect, visuals.rounding, visuals.weak_bg_fill, visuals.bg_stroke);
        draw(ui.painter(), rect.shrink(4.0), visuals.fg_stroke.color);
    }
    response
}

/// "폭 맞춤"(페이지 너비를 뷰어 폭에 맞춤) — 문서 사각형 좌우로
/// 바깥을 향한 화살표를 그려 "폭에 맞춘다"는 뜻을 표현한다. flat/simple 스타일
/// (2026-07-18 요청).
fn draw_fit_width_icon(painter: &egui::Painter, rect: egui::Rect, color: egui::Color32) {
    let stroke = egui::Stroke::new(1.3_f32, color);
    let page = egui::Rect::from_center_size(rect.center(), egui::vec2(rect.width() * 0.42, rect.height()));
    painter.rect_stroke(page, 1.0, stroke);

    let mid_y = rect.center().y;
    let arrow_len = rect.width() * 0.26;
    let head = 2.6;
    for sign in [-1.0_f32, 1.0] {
        let start_x = if sign < 0.0 { page.left() } else { page.right() };
        let tip = egui::pos2(start_x + sign * arrow_len, mid_y);
        painter.line_segment([egui::pos2(start_x, mid_y), tip], stroke);
        painter.line_segment([tip, tip + egui::vec2(-sign * head, -head)], stroke);
        painter.line_segment([tip, tip + egui::vec2(-sign * head, head)], stroke);
    }
}

/// "쪽 맞춤"(페이지 전체가 뷰어 안에 들어오게 맞춤) — 카메라 뷰파인더처럼 네 모서리에
/// 꺾쇠를 그리고 그 안에 작은 페이지 사각형을 둬 "전체가 프레임 안에 들어온다"는 뜻을
/// 표현한다. flat/simple 스타일(2026-07-18 요청).
fn draw_fit_page_icon(painter: &egui::Painter, rect: egui::Rect, color: egui::Color32) {
    let stroke = egui::Stroke::new(1.3_f32, color);
    let arm = rect.width().min(rect.height()) * 0.34;
    let corners: [(egui::Pos2, egui::Vec2, egui::Vec2); 4] = [
        (rect.left_top(), egui::vec2(arm, 0.0), egui::vec2(0.0, arm)),
        (rect.right_top(), egui::vec2(-arm, 0.0), egui::vec2(0.0, arm)),
        (rect.left_bottom(), egui::vec2(arm, 0.0), egui::vec2(0.0, -arm)),
        (rect.right_bottom(), egui::vec2(-arm, 0.0), egui::vec2(0.0, -arm)),
    ];
    for (corner, dx, dy) in corners {
        painter.line_segment([corner, corner + dx], stroke);
        painter.line_segment([corner, corner + dy], stroke);
    }
    let page = rect.shrink2(egui::vec2(rect.width() * 0.24, rect.height() * 0.14));
    painter.rect_stroke(page, 1.0, egui::Stroke::new(1.1_f32, color));
}

/// "쪽 단위 보기"(현재 모드 표시용) — 페이지 한 장. flat/simple 스타일.
fn draw_single_page_icon(painter: &egui::Painter, rect: egui::Rect, color: egui::Color32) {
    let stroke = egui::Stroke::new(1.3_f32, color);
    let page = egui::Rect::from_center_size(
        rect.center(),
        egui::vec2(rect.width() * 0.62, rect.height() * 0.92),
    );
    painter.rect_stroke(page, 1.0, stroke);
}

/// "연속 스크롤 보기"(현재 모드 표시용) — 세로로 이어지는 페이지 두 장(위/아래는 화면
/// 밖으로 이어지는 느낌으로 개방). flat/simple 스타일 — 사용자 확인 완료(2026-07-18).
fn draw_continuous_icon(painter: &egui::Painter, rect: egui::Rect, color: egui::Color32) {
    let stroke = egui::Stroke::new(1.3_f32, color);
    let page_w = rect.width() * 0.62;
    let left = rect.center().x - page_w / 2.0;
    let right = rect.center().x + page_w / 2.0;
    let gap = rect.height() * 0.12;
    let mid = rect.center().y;
    // 위 페이지: 상단 열림(위로 계속 이어짐을 암시) — 좌/우 변 + 아래 변만.
    let top_bottom = mid - gap / 2.0;
    painter.line_segment([egui::pos2(left, rect.top()), egui::pos2(left, top_bottom)], stroke);
    painter.line_segment([egui::pos2(right, rect.top()), egui::pos2(right, top_bottom)], stroke);
    painter.line_segment([egui::pos2(left, top_bottom), egui::pos2(right, top_bottom)], stroke);
    // 아래 페이지: 하단 열림 — 좌/우 변 + 위 변만.
    let bottom_top = mid + gap / 2.0;
    painter.line_segment([egui::pos2(left, bottom_top), egui::pos2(left, rect.bottom())], stroke);
    painter.line_segment([egui::pos2(right, bottom_top), egui::pos2(right, rect.bottom())], stroke);
    painter.line_segment([egui::pos2(left, bottom_top), egui::pos2(right, bottom_top)], stroke);
}

/// 한 번에 하나만 열리게 하는 열쇠 — 열려 있는 메뉴의 `id`를 담는다. 메뉴마다 따로
/// "열림" 플래그를 두면, 옆 메뉴 버튼으로 마우스를 옮겨도 그 버튼이 앞 메뉴 팝업의 판정
/// 영역(아래 `inside`의 여유분) 안이라 앞 메뉴가 닫히지 않는다(사용자 리포트, 2026-09-23).
fn active_menu_id() -> egui::Id {
    egui::Id::new("hover_menu_active")
}

/// 올려놓기만 해도 열리는 메뉴(클릭도 됨). 항목을 고르면 닫힌다.
///
/// 마우스가 버튼에서 팝업으로 넘어가는 순간 어느 쪽도 hover가 아닌 프레임이 끼면 메뉴가
/// 닫혀 버리므로(2026-07-16 최근 파일 목록에서 겪음), 위젯별 hover 대신 포인터 좌표가
/// "버튼 ∪ 팝업"(여유분 포함) 안에 있는지로 판정한다. 여유분은 아래·옆으로만 준다 —
/// 위로도 주면 툴바의 다른 메뉴 버튼까지 판정 영역에 들어간다.
///
/// `width`는 팝업의 가로폭(포인트). 항목은 이 폭을 꽉 채우고, 글자는 왼쪽에 붙는다.
fn hover_menu<R>(
    ui: &mut egui::Ui,
    id: &str,
    label: &str,
    enabled: bool,
    width: f32,
    contents: impl FnOnce(&mut egui::Ui) -> R,
) -> Option<R> {
    let active: Option<String> = ui.ctx().data(|d| d.get_temp(active_menu_id())).unwrap_or_default();
    let mut open = active.as_deref() == Some(id);
    let button = ui.add_enabled(enabled, egui::Button::new(label));
    if enabled && (button.hovered() || button.clicked()) {
        open = true;
    }
    if !enabled {
        open = false;
    }

    let mut result = None;
    if open {
        let area = egui::Area::new(egui::Id::new(("hover_menu_area", id)))
            .fixed_pos(button.rect.left_bottom())
            .order(egui::Order::Foreground)
            .show(ui.ctx(), |ui| {
                egui::Frame::popup(ui.style())
                    .show(ui, |ui| {
                        ui.set_width(width);
                        ui.spacing_mut().item_spacing.y = 5.0;
                        // 세로로 쌓되 각 항목이 폭을 꽉 채우게(justified) — 그래야 hover
                        // 강조가 줄 전체에 걸린다. 글자는 왼쪽 정렬.
                        ui.with_layout(egui::Layout::top_down_justified(egui::Align::LEFT), contents).inner
                    })
                    .inner
            });
        let popup = area.response.rect;
        let reach = egui::Rect::from_min_max(
            popup.min - egui::vec2(10.0, 2.0),
            popup.max + egui::vec2(10.0, 10.0),
        );
        let pointer = ui.ctx().input(|i| (i.pointer.hover_pos(), i.pointer.any_click()));
        let inside =
            pointer.0.is_some_and(|pos| button.rect.expand(6.0).contains(pos) || reach.contains(pos));
        let clicked_item = pointer.1 && pointer.0.is_some_and(|pos| popup.contains(pos));
        if !inside || clicked_item {
            open = false;
        }
        result = Some(area.inner);
    }
    ui.ctx().data_mut(|d| {
        let current: Option<String> = d.get_temp(active_menu_id()).unwrap_or_default();
        if open {
            d.insert_temp(active_menu_id(), Some(id.to_string()));
        } else if current.as_deref() == Some(id) {
            d.insert_temp(active_menu_id(), None::<String>);
        }
    });
    result
}

/// 메뉴 한 줄. 비활성이면 회색으로 두고 이유를 툴팁에 적는다.
///
/// 버튼(`egui::Button`)이 아니라 `SelectableLabel`을 쓴다 — 버튼은 평소에도 배경 상자를
/// 그려서 항목이 모두 음영 박스로 보이고, 그게 읽기를 방해한다는 지적(2026-09-23).
/// SelectableLabel은 마우스를 올린 줄에만 배경을 칠한다.
fn menu_item(ui: &mut egui::Ui, label: &str, enabled: bool, tooltip: &str) -> bool {
    let response = ui.add_enabled(enabled, egui::SelectableLabel::new(false, label));
    let response = if tooltip.is_empty() { response } else { response.on_hover_text(tooltip) };
    response.clicked()
}

pub fn show(ctx: &egui::Context, app: &mut PdfViewerApp) {
    egui::TopBottomPanel::top("toolbar").show(ctx, |ui| {
        ui.horizontal(|ui| {
            let has_file = app.current_file.is_some();
            let ocr_busy = app.ocr_job.as_ref().is_some_and(|j| j.is_running());
            let m = modifier_label();

            // ---- 파일
            hover_menu(ui, "file", "파일", true, RECENT_FILE_WIDTH, |ui| {
                if menu_item(ui, "파일 열기…", true, "") {
                    if let Some(path) = rfd::FileDialog::new().add_filter("PDF", &["pdf"]).pick_file() {
                        ui.ctx().send_viewport_cmd(egui::ViewportCommand::Focus);
                        app.request_open_file(path);
                    }
                }
                if menu_item(ui, &format!("저장  ({m}+S)"), app.bookmarks_dirty, "북마크를 PDF에 저장") {
                    app.save_bookmarks_to_pdf();
                }
                if menu_item(ui, "파일명 변경…  (F2)", has_file, "열려 있는 파일의 이름을 바꿉니다") {
                    app.begin_rename();
                }
                if !app.recent_files.is_empty() {
                    ui.separator();
                    ui.weak("최근 파일");
                    recent_file_items(ui, app);
                }
            });

            // ---- 북마크
            hover_menu(ui, "bookmark", "북마크", true, MENU_WIDTH, |ui| {
                if menu_item(ui, "CSV로 내보내기…", true, "") {
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("CSV", &["csv"])
                        .set_file_name(app.export_default_filename("csv"))
                        .save_file()
                    {
                        app.export_bookmarks_csv(path);
                    }
                }
                if menu_item(ui, "Excel로 내보내기…", true, "") {
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("Excel", &["xlsx"])
                        .set_file_name(app.export_default_filename("xlsx"))
                        .save_file()
                    {
                        app.export_bookmarks_xlsx(path);
                    }
                }
                ui.separator();
                if menu_item(ui, "CSV에서 가져오기…", true, "") {
                    if let Some(path) = rfd::FileDialog::new().add_filter("CSV", &["csv"]).pick_file() {
                        app.import_bookmarks_csv(path);
                    }
                }
                if menu_item(ui, "Excel에서 가져오기…", true, "") {
                    if let Some(path) = rfd::FileDialog::new().add_filter("Excel", &["xlsx"]).pick_file() {
                        app.import_bookmarks_xlsx(path);
                    }
                }
                // 폴더 일괄 적용(2026-07-19) — 폴더만 고르면 안의 xlsx/csv를 자동 인식한다
                // (폴더 → 파일 다이얼로그를 연달아 띄웠더니 macOS 패널에 제목이 안 보여
                // "잘못 뜬 파일 선택"으로 오인됐던 이력, batch_import::prepare_job 참고).
                let batch_running = app.batch_import.as_ref().is_some_and(|j| j.is_running());
                if menu_item(
                    ui,
                    "폴더 일괄 적용…",
                    !batch_running,
                    "폴더 안 모든 PDF에 북마크 파일(CSV/Excel)을 적용 — 원본은 .backup으로 보존",
                ) {
                    if let Some(folder) =
                        rfd::FileDialog::new().set_title("일괄 적용할 PDF 폴더 선택").pick_folder()
                    {
                        ui.ctx().send_viewport_cmd(egui::ViewportCommand::Focus);
                        app.start_batch_import(folder);
                    }
                }
                ui.separator();
                if menu_item(ui, "전체 삭제", !app.bookmarks.is_empty(), "이 문서의 북마크를 모두 지웁니다(되돌리기 가능)") {
                    app.clear_bookmarks_pending = true;
                }
            });

            // ---- OCR (planning/ocr_feature_considerations.md)
            hover_menu(ui, "ocr", "OCR", !ocr_busy, MENU_WIDTH, |ui| {
                if menu_item(ui, "전체 삭제…", has_file, "보이지 않는 텍스트(OCR 레이어)를 지웁니다 — 원본은 .backup으로 보존") {
                    crate::ocr_dialogs::request_removal(ui.ctx(), app);
                }
                if menu_item(ui, "폴더 일괄 삭제…", true, "고른 폴더(하위 폴더 포함)의 모든 PDF에서 지웁니다 — 결과는 CSV 로그") {
                    crate::ocr_dialogs::request_folder_removal(ui.ctx(), app);
                }
                ui.separator();
                if menu_item(ui, "가져오기…", has_file, "hOCR 파일의 텍스트를 보이지 않는 텍스트 레이어로 넣습니다") {
                    crate::ocr_dialogs::request_import(ui.ctx(), app);
                }
                use crate::ocr_worker::ExportFormat;
                if menu_item(ui, "hOCR로 내보내기…", has_file, "단어 위치가 담긴 hOCR — 가져오기에 다시 쓸 수 있음") {
                    crate::ocr_dialogs::request_export(ui.ctx(), app, ExportFormat::Hocr);
                }
                if menu_item(ui, "txt로 내보내기…", has_file, "페이지 번호 표기가 붙은 텍스트") {
                    crate::ocr_dialogs::request_export(ui.ctx(), app, ExportFormat::Txt);
                }
            });

            // save_bookmarks_to_pdf가 원본을 못 찾으면(이름변경/이동/삭제) 세우는 플래그를 받아
            // "다른 이름으로 저장" 대화상자를 띄운다 — 파일 다이얼로그는 관례상 여기서 열고,
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

            // 창을 좁히면 툴바의 묶음들이 서로 겹쳐 검색창이 페이지 번호 위를 덮고 넘김
            // 버튼이 반쯤 보이는 상태가 됐다(사용자 리포트, 2026-09-23). 남은 폭을 재서
            // 들어가지 않는 묶음은 그리지 않고, 오른쪽 끝 » 메뉴 안으로 접는다.
            let plan = fit_groups(ui, app);

            if plan.zoom {
                zoom_group(ui, app);
                ui.separator();
            }
            if plan.pages {
                page_group(ui, app);
            }

            // 검색 UI를 메인바 제일 오른쪽에 고정한다. 남은 가로 공간을 이 하위 레이아웃이
            // 통째로 차지한 뒤 오른쪽에서 왼쪽 방향으로 채워나가므로(egui의
            // Layout::right_to_left 관례), 이 스코프 안에서 먼저 추가한 위젯이 가장
            // 오른쪽에 온다 — 그래서 눈에 보이는 순서([검색창][🔍][◀][N/M][▶])와는
            // 반대로 ▶부터 추가한다.
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if !plan.fits_all() {
                    overflow_menu(ui, app, plan);
                    ui.separator();
                }
                // 단축키 안내 — 툴바 맨 오른쪽(2026-09-15 요청). on_hover_ui는 egui 기본 지연
                // (tooltip_delay 0.5초 + 마우스가 멈출 때까지 대기)이 있어 1~2초 뒤에야 떴으므로,
                // 마우스가 올라와 있는 동안 show_tooltip_ui를 직접 불러 즉시 띄운다. 앱 전체
                // tooltip_delay는 건드리지 않아 다른 버튼 툴팁은 그대로다.
                let shortcuts = ui.add(egui::Label::new("단축키").sense(egui::Sense::hover()));
                if shortcuts.hovered() {
                    shortcuts.show_tooltip_ui(show_shortcut_help);
                }
                ui.separator();
                if plan.search {
                    search_group(ui, app);
                }
            });
        });
    });
}

/// 좁은 창에서 접힌 묶음을 모아 보여 주는 버튼의 라벨.
const OVERFLOW_LABEL: &str = "»";
/// » 메뉴의 가로폭 — 가장 넓은 묶음(검색)이 들어가는 만큼.
const OVERFLOW_WIDTH: f32 = 340.0;
/// 검색어 입력칸 폭.
const SEARCH_FIELD_WIDTH: f32 = 160.0;

/// 이번 프레임에 툴바에 직접 그릴 묶음. 나머지는 » 메뉴로 접힌다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Plan {
    zoom: bool,
    pages: bool,
    search: bool,
}

impl Plan {
    fn fits_all(&self) -> bool {
        self.zoom && self.pages && self.search
    }
}

/// 남은 가로폭에 맞춰 어느 묶음을 그릴지 정한다. 다 들어가면 » 버튼 자리는 잡지 않는다.
/// 자리가 모자라면 **페이지 이동 → 확대/축소 → 검색** 순으로 남긴다(좁은 창에서 가장
/// 아쉬운 것이 페이지 이동이라는 판단).
fn fit_groups(ui: &egui::Ui, app: &PdfViewerApp) -> Plan {
    let separator = separator_width(ui);
    let (zoom, pages, search) = (zoom_width(ui) + separator, page_width(ui, app), search_width(ui));
    // 단축키 라벨과 그 왼쪽 구분선은 항상 남긴다(작고, 접을 곳도 마땅치 않다).
    let mut budget = ui.available_width() - label_width(ui, "단축키") - separator;
    if budget >= zoom + pages + search {
        return Plan { zoom: true, pages: true, search: true };
    }
    budget -= button_width(ui, OVERFLOW_LABEL) + separator;
    let mut plan = Plan { zoom: false, pages: false, search: false };
    for (width, slot) in [(pages, &mut plan.pages), (zoom, &mut plan.zoom), (search, &mut plan.search)] {
        if budget >= width {
            budget -= width;
            *slot = true;
        }
    }
    plan
}

fn separator_width(ui: &egui::Ui) -> f32 {
    // egui::Separator가 가로 레이아웃에서 잡는 폭(기본 spacing 6pt) + 항목 간격.
    6.0 + ui.spacing().item_spacing.x
}

fn text_width(ui: &egui::Ui, text: &str, style: egui::TextStyle) -> f32 {
    let font = style.resolve(ui.style());
    ui.fonts(|fonts| fonts.layout_no_wrap(text.to_owned(), font, egui::Color32::WHITE).size().x)
}

fn button_width(ui: &egui::Ui, text: &str) -> f32 {
    text_width(ui, text, egui::TextStyle::Button) + 2.0 * ui.spacing().button_padding.x
}

fn label_width(ui: &egui::Ui, text: &str) -> f32 {
    text_width(ui, text, egui::TextStyle::Body)
}

fn zoom_width(ui: &egui::Ui) -> f32 {
    button_width(ui, "➖") + button_width(ui, "➕")
        + label_width(ui, "1000%")
        + ICON_BUTTON_SIZE * 2.0
        + ui.spacing().item_spacing.x * 5.0
}

fn page_width(ui: &egui::Ui, app: &PdfViewerApp) -> f32 {
    button_width(ui, "◀")
        + button_width(ui, "▶")
        + page_field_width(ui, app.total_pages)
        + ui.spacing().button_padding.x * 2.0
        + label_width(ui, &format!("/ {}", app.total_pages))
        + ui.spacing().item_spacing.x * 4.0
}

fn search_width(ui: &egui::Ui) -> f32 {
    button_width(ui, "▶") + button_width(ui, "◀") + button_width(ui, "🔍")
        + label_width(ui, "999 / 999")
        + SEARCH_FIELD_WIDTH
        + ui.spacing().item_spacing.x * 5.0
}

/// "현재쪽" 입력창 폭 — 숫자 3자리가 잘리지 않게(천 쪽 이상 문서는 그 자릿수만큼).
/// 실제 글꼴의 숫자 폭으로 계산한다(2026-09-14 요청).
fn page_field_width(ui: &egui::Ui, total_pages: u32) -> f32 {
    let digits = total_pages.max(1).to_string().len().max(3) as f32;
    let font_id = egui::TextStyle::Body.resolve(ui.style());
    let digit_width = ui.fonts(|fonts| fonts.glyph_width(&font_id, '0'));
    (digit_width * digits).ceil() + 2.0
}

/// 확대/축소와 보기 모드. 트랙패드 핀치·마우스 휠 줌과 별개로, 비전문 사용자를 위한
/// 명시적 버튼 병행 배치. 버튼은 고정 단계표로 움직인다(ViewportState::ZOOM_STEPS 참고).
fn zoom_group(ui: &mut egui::Ui, app: &mut PdfViewerApp) {
    if ui.button("➖").on_hover_text("축소").clicked() {
        app.viewport.zoom_out();
    }
    ui.label(format!("{:.0}%", app.viewport.zoom * 100.0))
        .on_hover_text("100% = 페이지 실제 크기 (PDF 1pt = 화면 1pt)");
    if ui.button("➕").on_hover_text("확대").clicked() {
        app.viewport.zoom_in();
    }
    // 쪽 맞춤/폭 맞춤 통합 토글(2026-07-18 요청) — 아이콘은 "누르면 무엇이 되는지"를
    // 보여준다. 판정은 현재 줌 값이 아니라 맞춤 모드로 한다(2026-09-27) — 모드가 생기기
    // 전에는 "줌이 100%인가"로 갈랐는데, 배율이 실제 크기 기준이 되면서 100%와 폭 맞춤이
    // 더는 같은 뜻이 아니다. 수동 줌 상태에서는 쪽 맞춤으로 돌아가는 길을 권한다.
    if app.viewport.fit == FitMode::Page {
        if icon_button(ui, draw_fit_width_icon)
            .on_hover_text("폭 맞춤 — 페이지 폭을 뷰어 폭에")
            .clicked()
        {
            app.viewport.fit = FitMode::Width;
        }
    } else if icon_button(ui, draw_fit_page_icon)
        .on_hover_text("쪽 맞춤 — 페이지 전체가 보이게")
        .clicked()
    {
        app.viewport.fit = FitMode::Page;
    }

    // 쪽 단위/연속 스크롤 모드 토글('C'와 동일 동작, 2026-07-18 요청) — 이쪽은
    // 위와 달리 "현재 모드"를 아이콘으로 보여준다(사용자 명세: 연속 모드에서는
    // 연속 스크롤 아이콘, 다시 누르거나 C를 누르면 쪽 단위 아이콘으로 변경).
    let mode_response = if app.continuous_scroll {
        icon_button(ui, draw_continuous_icon).on_hover_text("연속 스크롤 보기 중 — 누르면 쪽 단위 (C)")
    } else {
        icon_button(ui, draw_single_page_icon).on_hover_text("쪽 단위 보기 중 — 누르면 연속 스크롤 (C)")
    };
    if mode_response.clicked() {
        app.toggle_continuous_scroll();
    }
}

/// 페이지 이동(◀ 현재쪽 / 전체쪽 ▶).
fn page_group(ui: &mut egui::Ui, app: &mut PdfViewerApp) {
    if ui.button("◀").on_hover_text("이전 페이지").clicked() {
        let prev = app.current_page.saturating_sub(1).max(1);
        app.go_to_page(prev);
    }

    let field_width = page_field_width(ui, app.total_pages);
    let response = ui.add(
        egui::TextEdit::singleline(&mut app.page_number_input)
            .desired_width(field_width)
            .horizontal_align(egui::Align::Center),
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
}

/// 검색 묶음. **오른쪽에서 왼쪽으로 쌓는 레이아웃 안에서 부르는 것을 전제로 한다** —
/// 먼저 추가한 위젯이 가장 오른쪽에 오므로, 눈에 보이는 순서([검색창][🔍][◀][N/M][▶])와
/// 반대로 ▶부터 추가한다. » 메뉴 안에서도 같은 레이아웃으로 불러 순서를 맞춘다.
fn search_group(ui: &mut egui::Ui, app: &mut PdfViewerApp) {
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
        ui.label(format!("{} / {}", app.search_current_index + 1, app.search_matches.len()));
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
            .desired_width(SEARCH_FIELD_WIDTH),
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
    // 검색어를 모두 지우면 검색 모드를 끝낸다 — 결과·뷰어 하이라이트·결과 패널까지
    // (2026-09-14 요청). 새 검색어를 입력하고 Enter를 누르면 다시 시작된다.
    let search_active =
        !app.search_matches.is_empty() || app.search_running.is_some() || app.search_panel_open;
    if search_response.changed() && app.search_query.trim().is_empty() && search_active {
        app.clear_search();
    } else if search_response.gained_focus() || search_response.clicked() {
        // 검색창을 클릭하면(Ctrl/Cmd+F는 app.rs에서) 검색 결과 목록이 포커스를 갖는다.
        app.focus_search_results();
    }
}

/// 접힌 묶음들을 담는 » 메뉴. 다른 메뉴와 같은 hover 동작이다.
fn overflow_menu(ui: &mut egui::Ui, app: &mut PdfViewerApp, plan: Plan) {
    hover_menu(ui, "overflow", OVERFLOW_LABEL, true, OVERFLOW_WIDTH, |ui| {
        if !plan.pages {
            ui.horizontal(|ui| page_group(ui, app));
        }
        if !plan.zoom {
            ui.horizontal(|ui| zoom_group(ui, app));
        }
        if !plan.search {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| search_group(ui, app));
        }
    })
    .map(|_| ())
    .unwrap_or_default();
}

/// 단축키 안내 툴팁 내용. \t(tab)만으로는 실제 탭 스톱 정렬이 안 되고 각 줄의 키 텍스트
/// 길이만큼 들쑥날쑥해진다(egui는 tab을 "고정폭만큼 더 전진"으로만 처리, 열 정렬 개념이 없음) —
/// 표 형태 정렬은 egui::Grid로 컬럼 자체를 나눠야 나온다.
fn show_shortcut_help(ui: &mut egui::Ui) {
    let m = modifier_label();
    egui::Grid::new("shortcut_help_grid")
        .num_columns(2)
        .spacing([16.0, 8.0])
        .show(ui, |ui| {
            for (key, desc) in [
                (format!("{m}+B"), "북마크 추가"),
                // 같은 키를 포커스 영역으로 나눠 쓴다(app.rs의 F2 처리 참고).
                ("F2".to_string(), "북마크 수정 (사이드바) / 파일명 변경 (그 외)"),
                ("Delete".to_string(), "북마크 삭제"),
                (format!("{m}+S"), "북마크 저장"),
                (format!("{m}+F"), "내용 검색"),
                ("↑ / ↓".to_string(), "검색 결과 이동 (검색 목록 포커스 시)"),
                (format!("{m}+["), "이전 화면"),
                (format!("{m}+]"), "다음 화면"),
                ("Tab".to_string(), "북마크↔뷰어 (검색 목록에서는 직전 영역으로)"),
                ("C".to_string(), "쪽 단위/연속 스크롤 전환"),
                ("우클릭 드래그".to_string(), "화면 이동"),
            ] {
                ui.label(key);
                ui.label(desc);
                ui.end_row();
            }
        });
}

/// 파일 메뉴 안의 "최근 파일" 목록 항목들. 팝업(Area)은 `hover_menu`가 이미 띄워 두었으므로
/// 여기서는 항목만 그린다. 항목은 파일명(위 줄) + 상위 폴더 경로(아래 줄) 두 줄짜리 버튼이다 —
/// 스타일이 다른 두 줄을 한 버튼에 넣으려면 egui::Button이 받는 WidgetText로는 안 되고
/// LayoutJob으로 섹션별 폰트·색을 지정해야 한다.
fn recent_file_items(ui: &mut egui::Ui, app: &mut PdfViewerApp) {
    // 폭은 파일 메뉴 전체 폭(RECENT_FILE_WIDTH)을 그대로 쓴다.
    let width = ui.available_width();
    // 항목 사이 간격을 기본(3pt)보다 조금 넓힌다 — 두 줄짜리 항목이 붙어 보여
    // 구분이 잘 안 된다는 피드백(2026-09-15).
    ui.spacing_mut().item_spacing.y = 7.0;
    // 경로 줄 색: weak_text_color는 너무 흐려 잘 안 읽힌다는 피드백(2026-09-15) —
    // 본문 글자색과 흐린 색의 중간. 둘 다 premultiplied라 성분 평균이 곧 선형 혼합.
    let path_color = {
        let (strong, weak) = (ui.visuals().text_color(), ui.visuals().weak_text_color());
        let mid = |a: u8, b: u8| ((a as u16 + b as u16) / 2) as u8;
        egui::Color32::from_rgba_premultiplied(
            mid(strong.r(), weak.r()),
            mid(strong.g(), weak.g()),
            mid(strong.b(), weak.b()),
            mid(strong.a(), weak.a()),
        )
    };

    let mut path_to_open: Option<std::path::PathBuf> = None;
    for path in &app.recent_files {
        let filename = crate::app::display_filename(path);
        // 파일명은 위 줄에 이미 있으니 아래 줄엔 상위 폴더 경로만(파일명 중복 표시하지
        // 않음) — 루트 바로 아래 파일 등 부모가 없으면 생략. 폴더명에도 한글이 있을 수
        // 있어 파일명과 마찬가지로 NFC 정규화 필요(§7 "한글 파일명 자소 분리" 참고).
        use unicode_normalization::UnicodeNormalization;
        let dir_only = path
            .parent()
            .map(|p| p.to_string_lossy().nfc().collect::<String>())
            .filter(|s| !s.is_empty());

        let mut job = egui::text::LayoutJob::default();
        job.wrap.max_width = width;
        // 글자 단위로 줄바꿈해 각 줄을 폭 끝까지 채운다 — 기본(단어 단위)은 띄어쓰기
        // 없는 긴 한글 경로 조각이 통째로 다음 줄로 넘어가 오른쪽 끝이 들쑥날쑥했음
        // (2026-09-15 피드백). egui엔 양쪽 정렬이 없어 이게 가장 고르게 맞추는 방법.
        job.wrap.break_anywhere = true;
        job.append(
            &filename,
            0.0,
            egui::TextFormat {
                font_id: egui::FontId::proportional(15.0),
                color: ui.visuals().text_color(),
                ..Default::default()
            },
        );
        if let Some(dir_only) = dir_only {
            job.append(
                &format!("\n{dir_only}"),
                0.0,
                egui::TextFormat {
                    font_id: egui::FontId::proportional(13.0),
                    color: path_color,
                    ..Default::default()
                },
            );
        }

        // 버튼이 아니라 SelectableLabel — 평소엔 배경 없이 글자만, 마우스를 올린 줄에만
        // 강조(2026-09-23 요청: 음영 박스가 목록 읽기를 방해한다).
        if ui.add(egui::SelectableLabel::new(false, job)).clicked() {
            path_to_open = Some(path.clone());
        }
    }

    if let Some(path) = path_to_open {
        app.open_recent_file(path);
    }
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
