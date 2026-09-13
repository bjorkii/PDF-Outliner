use crate::app::{PdfViewerApp, ViewportState};
use crate::toolbar::handle_scroll_zoom;
use egui::Sense;
use pdf_engine::links::LinkTarget;
use pdf_engine::selection::TextSelectionRange;
use pdfium_render::prelude::*;

/// 줌이 멎은 뒤 쪽 단위 모드가 재렌더링하기까지 기다리는 시간(초). 배율 표시와 화면상
/// 크기는 즉시 바뀌고(기존 텍스처를 목표 크기로 늘려 그림) pdfium 렌더만 미룬다.
const ZOOM_RENDER_DEBOUNCE_SECS: f64 = 0.12;
/// 연속 스크롤 모드의 원해상도 업그레이드 디바운스(초). 보이는 페이지가 여러 장이라
/// 업그레이드 비용이 커서 쪽 단위보다 조금 더 기다린다.
const CONTINUOUS_RESCALE_DEBOUNCE_SECS: f64 = 0.2;
/// 연속 스크롤 모드의 페이지 사이 간격(pt). 배율과 무관한 상수라 스크롤 보정을 비율
/// 곱셈으로 하면 안 되는 이유가 된다(`anchor_at` 참고).
const PAGE_GAP: f32 = 8.0;
/// 쪽 단위 보기에서 페이지를 넘긴 직후 새 페이지 렌더 결과가 아직 없을 때, 직전 화면을 그대로
/// 두는 시간(초). 동기 렌더링 시절 "렌더가 끝날 때까지 이전 화면이 남아 있던" 모습과 같게 해
/// 흰 화면 깜빡임을 피한다. 이보다 오래 걸리면 흰 페이지로 자리만 잡는다.
const PAGE_SWITCH_GRACE_SECS: f64 = 0.3;
/// 앞뒤 페이지 미리 렌더링의 텍스처 크기 상한(픽셀 수, RGBA 약 96MB). 고배율에서 이웃까지
/// 원해상도로 만들면 한 장에 수백 MB라, 넘기는 순간 보여줄 만큼만 만들고 넘긴 뒤 원해상도를
/// 다시 요청한다.
const PREFETCH_MAX_PIXELS: f32 = 24_000_000.0;

/// 이웃 페이지를 미리 렌더링할 폭 — 현재 배율 폭을 넘지 않고, 픽셀 수 상한 안에서.
fn prefetch_width(target_width: i32, aspect: f32) -> i32 {
    let cap = (PREFETCH_MAX_PIXELS / aspect.max(0.01)).sqrt();
    (target_width as f32).min(cap).max(50.0) as i32
}

/// 검색 결과 `index`의 페이지 번호와, 그 검색어(줄바꿈으로 나뉘면 사각형들을 합친 영역) 중심의
/// 페이지 안 위치(페이지 좌상단 기준, 화면 pt). 페이지 좌표→화면 좌표 변환은 뷰어의 검색
/// 하이라이트(`draw_search_highlight`)와 같은 pdfium 변환이라 /Rotate 페이지에서도 하이라이트와
/// 같은 자리를 가리킨다.
fn search_match_center(
    app: &PdfViewerApp,
    index: usize,
    target_width: i32,
    pixels_per_point: f32,
) -> Option<(u32, egui::Vec2)> {
    let search_match = app.search_matches.get(index)?;
    let (left, right, bottom, top) = search_match.rects.iter().fold(
        (f32::MAX, f32::MIN, f32::MAX, f32::MIN),
        |(left, right, bottom, top), rect| {
            (
                left.min(rect.left().value),
                right.max(rect.right().value),
                bottom.min(rect.bottom().value),
                top.max(rect.top().value),
            )
        },
    );
    if left > right || bottom > top {
        return None;
    }
    let document = app.document.as_ref()?;
    let page = document
        .pages()
        .get((search_match.page - 1) as PdfPageIndex)
        .ok()?;
    let config = PdfRenderConfig::new().set_target_width(target_width);
    let (px, py) = page
        .points_to_pixels(
            PdfPoints::new((left + right) / 2.0),
            PdfPoints::new((bottom + top) / 2.0),
            &config,
        )
        .ok()?;
    Some((
        search_match.page,
        egui::vec2(px as f32, py as f32) / pixels_per_point.max(0.01),
    ))
}

/// 쪽 단위 보기: 페이지 안의 점(페이지 좌상단 기준 pt)이 화면 중앙에 오게 하는 팬 오프셋.
/// 화면상 페이지 중심 = 패널 중심 + 팬이므로, 점의 화면 위치 = 패널 중심 + 팬 − 페이지/2 + 점.
fn pan_to_center(page_size: egui::Vec2, point: egui::Vec2) -> egui::Vec2 {
    page_size / 2.0 - point
}

/// 연속 스크롤: 페이지 상단 y에 있는 페이지 안의 점(높이 point_y)이 화면 세로 중앙에 오는 스크롤 오프셋.
fn scroll_offset_to_center(page_top: f32, point_y: f32, view_height: f32) -> f32 {
    (page_top + point_y - view_height / 2.0).max(0.0)
}

/// 연속 스크롤: 페이지 안의 점(가로 point_x)이 화면 가로 중앙에 오는 가로 이동. 페이지가 패널보다
/// 좁으면 움직일 수 없으므로 0, 넓으면 페이지 가장자리를 넘지 않게 제한한다.
fn pan_x_to_center(page_width: f32, point_x: f32, view_width: f32) -> f32 {
    let max_pan = ((page_width - view_width) / 2.0).max(0.0);
    (page_width / 2.0 - point_x).clamp(-max_pan, max_pan)
}

fn page_aspect_of(page_aspects: &[f32], page: u32) -> f32 {
    page_aspects
        .get((page as usize).saturating_sub(1))
        .copied()
        .unwrap_or(1.414)
}

pub fn show(ctx: &egui::Context, app: &mut PdfViewerApp) {
    handle_scroll_zoom(ctx, &mut app.viewport);

    egui::CentralPanel::default().show(ctx, |ui| {
        // 폴더 일괄 북마크 적용이 진행 중(확인 대기/처리/완료)이면 뷰어 대신 그 화면을
        // 그린다 — 실시간 로그를 뷰어 영역으로 전환해 보여주는 스펙 1순위안.
        if app.batch_import.is_some() {
            crate::batch_import::show_panel(ui, app);
            return;
        }

        if app.document.is_none() {
            ui.centered_and_justified(|ui| {
                ui.label("PDF 파일을 열어주세요 (파일 열기 버튼 또는 드래그 앤 드롭)");
            });
            return;
        }

        // 핀치 줌: egui-winit 0.29.1이 macOS WindowEvent::PinchGesture를 내부적으로 이미
        // zoom_delta로 변환해준다(소스로 직접 확인함) — 별도 raw winit 이벤트 후킹 불필요.
        // 쪽 단위/연속 스크롤 두 모드 모두에서 그대로 확대/축소로 쓴다.
        let zoom_delta = ctx.input(|i| i.zoom_delta());
        if zoom_delta != 1.0 {
            app.viewport.zoom_by(zoom_delta);
        }

        // 트랙패드 두 손가락 스와이프 = 패닝(스크롤). Ctrl+스크롤은 확대/축소로 이미 쓰고
        // 있으니(toolbar::handle_scroll_zoom) 그 조합일 때는 패닝에서 제외한다. 연속
        // 스크롤 모드는 egui::ScrollArea가 스크롤 자체를 관리하므로 이 pan_offset 로직은
        // 쪽 단위 모드 전용이다.
        if !app.continuous_scroll && !ctx.input(|i| i.modifiers.ctrl) {
            let scroll_delta = ctx.input(|i| i.smooth_scroll_delta);
            if scroll_delta != egui::Vec2::ZERO {
                app.viewport.pan_offset += scroll_delta;
            }
        }

        let available = ui.available_size();
        // TextureHandle::size_vec2()는 텍스처의 실제 픽셀 크기를 반환하고(포인트로 나뉘지
        // 않음), egui의 Rect 크기는 포인트 단위다. pixels_per_point로 보정하지 않으면
        // Retina(2x) 화면에서 렌더링이 흐릿하게 나온다 — target_width를 물리 픽셀 기준으로
        // 렌더링하고, 화면에 그릴 때는 다시 포인트로 나눠 배치한다.
        let pixels_per_point = ctx.pixels_per_point();

        // GPU 텍스처 한도를 넘는 배율은 그 해상도로 렌더링 자체가 불가능하므로(§7 "고배율
        // 줌 크래시") 줌 값을 여기서 상한에 멈춘다 — 툴바 % 표시도 viewport.zoom을 그대로
        // 보여주므로 함께 멈춘다. 한도 초과분을 흐릿하게 스케일업해서 보여주는 방안은
        // 사용자가 기각(2026-07-14). 세로형 페이지는 높이가 먼저 한도에 걸리므로 페이지
        // 종횡비(page_aspect)를 반영해 허용 가능한 최대 렌더 폭을 역산한다.
        // 주의: 이 상한은 고정 %가 아니다 — %는 "패널 폭 대비 배율"이라 창이 좁거나
        // 사이드바가 넓으면 같은 800%라도 텍스처가 작아져 상한에 안 걸릴 수 있다
        // (실측: 기본 창에서는 세로형 A4급이 ~647%에서 멈추지만, 패널이 ~734pt 이하면
        // 800% 전체가 합법). 지켜지는 불변식은 "텍스처 ≤ GPU 한도" 하나다.
        if let (Some(aspect), Some(max_side)) = (app.page_aspect, app.max_texture_side) {
            let max_side = max_side.min(16384) as f32;
            let max_width_px = if aspect > 1.0 { max_side / aspect } else { max_side };
            let max_zoom = max_width_px / (available.x * pixels_per_point).max(1.0);
            if app.viewport.zoom > max_zoom {
                app.viewport.zoom = max_zoom.max(ViewportState::MIN_ZOOM);
            }
        }

        // 툴바 "쪽 맞춤" 버튼 요청 처리 — 그 프레임의 패널 크기를 아는 여기서만 정확히
        // 계산할 수 있다(app::request_fit_page 문서 참고). 폭 맞춤(zoom=1.0)이 이미
        // "페이지 폭 == 패널 폭"이므로, 높이도 패널 안에 들어오도록 필요하면 그보다 더
        // 축소한다(이미 다 들어오면 그대로 폭 맞춤 유지 — min(1.0, ...)).
        if std::mem::take(&mut app.request_fit_page) {
            if let Some(aspect) = app.page_aspect {
                let fit_zoom = (available.y / (available.x * aspect).max(1.0))
                    .clamp(ViewportState::MIN_ZOOM, ViewportState::MAX_ZOOM);
                app.viewport.zoom = fit_zoom;
            }
        }

        let target_width =
            ((available.x * app.viewport.zoom * pixels_per_point).round() as i32).max(50);

        // 배율(또는 패널 폭)이 바뀐 시각 — 두 모드 모두 이 시각 기준으로 재렌더링을
        // 디바운스한다(app::zoom_changed_at 문서 참고). 첫 프레임은 변화로 치지 않는다.
        if target_width != app.last_target_width {
            if app.last_target_width != 0 {
                app.zoom_changed_at = ctx.input(|i| i.time);
            }
            app.last_target_width = target_width;
        }

        if app.continuous_scroll {
            show_continuous(ctx, app, ui, available, target_width);
        } else {
            show_single_page(ctx, app, ui, available, pixels_per_point, target_width);
        }
    });
}

/// 쪽 단위 보기(기본 모드) — 한 번에 페이지 하나만 렌더링해 보여준다.
fn show_single_page(
    ctx: &egui::Context,
    app: &mut PdfViewerApp,
    ui: &mut egui::Ui,
    available: egui::Vec2,
    pixels_per_point: f32,
    target_width: i32,
) {
    {
        let (rect, response) = ui.allocate_exact_size(available, Sense::click_and_drag());

        // 링크 클릭으로 이번 프레임 중간에 current_page가 바뀔 수 있어, 렌더 요청·그리기는
        // 프레임 시작 시점의 페이지로 일관되게 한다(새 페이지는 다음 프레임부터).
        let page_number = app.current_page;
        let now = ctx.input(|i| i.time);
        let waited = now - app.zoom_changed_at;
        let zoom_settling = waited < ZOOM_RENDER_DEBOUNCE_SECS;
        if app.single_view_page != Some(page_number) {
            app.single_view_page = Some(page_number);
            app.single_view_switched_at = now;
        }

        // 보조 프로세스 대기열의 기준 — 페이지나 배율이 바뀌면 옛 요청은 렌더링되지 않고 버려진다.
        app.set_render_view((false, page_number, page_number, target_width));
        // 현재 페이지와 앞뒤 한 쪽 텍스처만 남긴다(넘기는 순간 보여줄 이웃). 그리기 전이라 해제 안전.
        app.page_textures
            .retain(|page| page + 1 >= page_number && page <= page_number + 1);

        // 렌더 요청. 보조 프로세스가 있으면 요청만 보내고 결과는 다음 프레임 이후 도착한다
        // (app::request_page_texture) — 그동안 기존 텍스처를 늘려 보여주므로 UI가 멈추지 않는다.
        let cached_width = app.page_textures.width(page_number);
        match cached_width {
            None => app.request_page_texture(ctx, page_number, target_width),
            // 같은 페이지의 배율만 바뀐 경우: 기존 텍스처를 목표 크기로 늘려 보여주다가, 배율이
            // 디바운스 시간 동안 멎은 뒤에 요청한다 — 핀치 중 스쳐 가는 중간 배율 렌더로
            // 대기열을 채우지 않기 위해.
            Some(width) if width != target_width => {
                if zoom_settling {
                    // 입력이 없으면 egui가 리페인트하지 않으므로, 줌이 멎은 뒤 요청이 다음
                    // 마우스 조작까지 밀리지 않게 깨울 시각을 예약한다.
                    ctx.request_repaint_after(std::time::Duration::from_secs_f64(
                        ZOOM_RENDER_DEBOUNCE_SECS - waited,
                    ));
                } else {
                    app.request_page_texture(ctx, page_number, target_width);
                }
            }
            // 현재 페이지가 준비됐으면 앞뒤 페이지를 미리 렌더링해 둔다 — 보조 프로세스가 있을
            // 때만(UI 스레드에서 동기로 하면 그만큼 멈추므로).
            Some(_) => {
                if app.render_worker_active() && !zoom_settling {
                    for neighbor in [page_number + 1, page_number.saturating_sub(1)] {
                        if neighbor == 0 || neighbor > app.total_pages {
                            continue;
                        }
                        let wanted =
                            prefetch_width(target_width, page_aspect_of(&app.page_aspects, neighbor));
                        let have = app.page_textures.width(neighbor);
                        if have.map_or(true, |width| width < wanted) {
                            app.request_page_texture(ctx, neighbor, wanted);
                        }
                    }
                }
            }
        }

        // 화면상 페이지 크기(pt) — 렌더 결과와 무관하게 페이지 종횡비로 정한다. 기다리는 동안
        // 옛 텍스처가 이 크기로 늘어나 보이다가 결과가 오면 같은 자리에서 선명하게 교체된다.
        // 히트테스트/하이라이트는 image_rect.width()와 target_width의 비율로 좌표를 환산하므로
        // 텍스처 해상도와 무관하게 화면과 일치한다.
        let display_width = target_width as f32 / pixels_per_point;
        let page_size = egui::vec2(
            display_width,
            display_width * page_aspect_of(&app.page_aspects, page_number),
        );
        // 검색 결과를 골랐으면 그 검색어 위치가 화면 중앙에 오게 팬을 맞춘다(페이지 경계를
        // 넘지 않게 바로 아래 clamp_pan이 제한). 결과 선택이 go_to_page로 현재 페이지를 이미
        // 옮겨 두므로 같은 프레임에 소비된다.
        if let Some(index) = app.search_center_request.take() {
            if let Some((match_page, point)) =
                search_match_center(app, index, target_width, pixels_per_point)
            {
                if match_page == page_number {
                    app.viewport.pan_offset = pan_to_center(page_size, point);
                }
            }
        }
        app.viewport.clamp_pan(page_size, available);

        let image_rect =
            egui::Rect::from_center_size(rect.center() + app.viewport.pan_offset, page_size);

        // 마우스가 링크 위에 있으면 손가락(Pointer) 커서, 문자 위에 있으면 텍스트
        // 커서(I-beam)로 바꿔 각각 클릭/선택 가능함을 알려준다. 링크가 텍스트 위에 겹쳐
        // 있는 경우가 흔하므로(예: 밑줄 그어진 하이퍼링크) 링크를 먼저 확인한다.
        // interact_pointer_pos()는 버튼이 눌려있을 때만 값이 있어 호버만으로는 커서가
        // 안 바뀌는 문제가 있었다 — hover_pos()는 버튼 상태와 무관하게 항상 갱신된다.
        if let Some(pos) = response.hover_pos() {
            if link_target_at_screen_pos(app, pos, image_rect, target_width, app.current_page).is_some() {
                ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
            } else if char_index_at_screen_pos(app, pos, image_rect, target_width, app.current_page).is_some() {
                ctx.set_cursor_icon(egui::CursorIcon::Text);
            }
        }

        // 우클릭 시 복사 메뉴. 텍스트 선택 상태(app.selection)가 있을 때만 의미가 있지만,
        // 메뉴 자체는 항상 띄우고 선택이 없으면 버튼을 비활성화해 상태를 알 수 있게 한다.
        response.context_menu(|ui| {
            if ui
                .add_enabled(app.selection.is_some(), egui::Button::new("복사"))
                .clicked()
            {
                app.copy_selection_to_clipboard();
                ui.close_menu();
            }
        });

        // 뷰어를 클릭하면 포커스가 뷰어로 옮겨간다 — 그래야 화살표 키가 페이지 이동으로
        // 쓰인다(사이드바가 포커스인 동안은 화살표가 트리 탐색용, app::FocusArea 참고).
        // 사이드바 선택 자체는 건드리지 않는다 — 어떤 북마크가 "선택돼 있었는지"는 포커스와
        // 별개 상태라 뷰어를 봐도 그대로 유지된다.
        if response.clicked() {
            app.focus_area = crate::app::FocusArea::Viewer;

            // 텍스트 선택이 있는 상태에서 뷰어를 클릭하면 선택 해제(2026-07-18 요청) —
            // 일반 텍스트 편집기/뷰어의 관례. 드래그로 새 선택을 시작할 때는 clicked()가
            // 아니라 drag_started() 경로라 여기 안 옴(거기서도 어차피 선택을 새로 잡음).
            // 우클릭 복사 메뉴는 secondary 클릭이라 clicked()(primary 전용)에 안 걸림 —
            // 선택을 유지한 채 메뉴를 띄울 수 있다.
            app.selection = None;
            app.selection_page = None;

            // 클릭한 위치가 문서 내 링크(주석)라면 그 대상으로 이동/열기한다 — 문서 내
            // 다른 페이지를 가리키면 뷰어에서 바로 이동, 외부 URI(웹 링크 등)면 시스템
            // 기본 브라우저로 연다.
            if let Some(pos) = response.interact_pointer_pos() {
                match link_target_at_screen_pos(app, pos, image_rect, target_width, app.current_page) {
                    Some(LinkTarget::Page(page)) => app.go_to_page(page),
                    Some(LinkTarget::Uri(url)) => app.open_external_link(&url),
                    None => {}
                }
            }
        }

        // 확대 시 drag 탐색: 텍스트 선택 드래그가 아닐 때만 pan으로 처리.
        // (텍스트 선택은 문자 인덱스가 있을 때만 활성화되므로, 문서에 텍스트 레이어가
        // 없는 페이지나 클릭이 문자에 닿지 않은 경우 자연히 pan으로 동작한다.)
        let hit_char = response
            .interact_pointer_pos()
            .and_then(|pos| char_index_at_screen_pos(app, pos, image_rect, target_width, app.current_page));

        if response.drag_started() {
            app.selection_drag_start_index = hit_char;
            app.selection = None;
            app.selection_page = hit_char.map(|_| app.current_page);
        } else if response.dragged() {
            if let Some(start) = app.selection_drag_start_index {
                if let Some(pos) = response.interact_pointer_pos() {
                    let current = char_index_at_screen_pos(app, pos, image_rect, target_width, app.current_page)
                        .or(hit_char);
                    if let Some(current) = current {
                        app.selection = Some(TextSelectionRange::from_anchors(start, current));
                        app.selection_page = Some(app.current_page);
                    }
                }
            } else {
                // 문자 위에서 드래그가 시작되지 않았으면 화면 이동(pan)으로 처리.
                app.viewport.pan_offset += response.drag_delta();
                app.viewport.clamp_pan(page_size, available);
            }
        }
        if response.drag_stopped() {
            app.selection_drag_start_index = None;
        }

        // 새 페이지 결과가 아직 없으면 잠깐(PAGE_SWITCH_GRACE_SECS) 직전에 그린 화면을 그대로
        // 두고, 그래도 없으면 흰 페이지로 자리만 잡는다.
        let fresh = app.page_textures.get(page_number).map(|(texture, _)| texture.clone());
        let texture = match fresh {
            Some(texture) => {
                app.page_textures.set_shown(&texture);
                Some(texture)
            }
            None => {
                let since_switch = now - app.single_view_switched_at;
                if since_switch < PAGE_SWITCH_GRACE_SECS {
                    ctx.request_repaint_after(std::time::Duration::from_secs_f64(
                        PAGE_SWITCH_GRACE_SECS - since_switch,
                    ));
                    app.page_textures.shown().cloned()
                } else {
                    None
                }
            }
        };
        app.page_textures
            .note_painted(texture.as_ref().map(egui::TextureHandle::id).as_slice());
        match texture {
            Some(texture) => {
                ui.painter().image(
                    texture.id(),
                    image_rect,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
            }
            None => {
                ui.painter().rect_filled(image_rect, 0.0, egui::Color32::WHITE);
            }
        }

        draw_selection_highlight(ui, app, image_rect, target_width, app.current_page);
        draw_search_highlight(ui, app, image_rect, target_width, page_number);

        app.image_rect = Some(image_rect);

        // macOS 트랙패드 핀치 제스처는 eframe 기본 추상화 밖 -> raw winit
        // WindowEvent::PinchGesture 후킹이 필요 (별도 platform integration 모듈에서 처리 예정)
    }
}

/// 연속 스크롤 보기 — 모든 페이지를 세로로 이어 붙여 스크롤한다(단축키 'C'로 전환). 문서
/// 전체를 한꺼번에 렌더링하면 수백 페이지 문서에서 감당이 안 되므로, `egui::ScrollArea`의
/// `show_viewport`로 실제로 화면(+위아래 여유분)에 들어오는 페이지만 pdfium으로 렌더링해
/// 텍스처로 캐싱하고, 그 범위를 벗어난 텍스처는 매 프레임 정리한다(가상화).
///
/// 텍스트 선택/복사, 문서 내 링크 클릭은 쪽 단위 모드와 동일하게 지원한다(2026-07-18
/// 요청 — "상식적으로 되어야 한다"). 선택은 한 페이지 안에서만 이어진다(드래그가 다른
/// 페이지로 넘어가면 그 프레임은 무시 — `app.selection_page`가 앵커 페이지 기준). 검색 결과
/// 하이라이트도 쪽 단위 모드와 똑같이 보인다(2026-09-14). 확대해 페이지가 패널보다 넓어지면
/// 트랙패드 좌우 스와이프로 가로 이동하고(`app.continuous_pan_x`), 검색 결과를 고르면 그
/// 검색어가 가로·세로 모두 화면 중앙에 오게 맞춘다.
fn show_continuous(
    ctx: &egui::Context,
    app: &mut PdfViewerApp,
    ui: &mut egui::Ui,
    available: egui::Vec2,
    target_width: i32,
) {
    let total_pages = app.total_pages.max(1) as usize;
    // 줌을 반영해야 한다 — 예전엔 `available.x`를 그대로 써서 배율과 무관하게 항상 폭
    // 맞춤으로 보이고, 확대/축소해도 텍스처(target_width, 줌 반영됨)만 해상도가 바뀌고
    // 화면에 그리는 크기는 그대로라 확대 시 흐릿해 보이는 문제가 있었다(2026-07-18 리포트
    // — "배율이 어떻든 되어야 함", "강제확대한 것처럼 sharpness가 떨어짐"). 쪽 단위 모드와
    // 동일하게 "줌 1.0 == 페이지 폭이 패널 폭과 같다"는 의미를 유지한다.
    let page_width_pts = available.x * app.viewport.zoom;

    let (offsets, heights, total_height) =
        continuous_layout(&app.page_aspects, total_pages, page_width_pts);

    // 가로 이동 — 확대로 페이지가 패널보다 넓을 때만 가능. 트랙패드 좌우 스와이프(Ctrl+휠
    // 줌과 겹치지 않게 Ctrl 제외)로 움직이고, 세로 스크롤은 ScrollArea가 맡는다.
    let max_pan_x = ((page_width_pts - available.x) / 2.0).max(0.0);
    let horizontal_swipe = ctx.input(|i| {
        if i.modifiers.ctrl {
            0.0
        } else {
            i.smooth_scroll_delta.x
        }
    });
    app.continuous_pan_x = (app.continuous_pan_x + horizontal_swipe).clamp(-max_pan_x, max_pan_x);

    // 화면 좌표 → (페이지 번호, 그 페이지의 화면 rect). 클릭/드래그/호버 히트테스트가 전부
    // 이 하나로 통일된다 — 가상화 범위와 무관하게(오프셋 계산은 항상 전체 페이지에 대해
    // 이미 돼 있으므로) 문서 어디를 가리켜도 정확하다.
    let page_at = |screen_pos: egui::Pos2, origin: egui::Pos2| -> Option<(u32, egui::Rect)> {
        let content_x = screen_pos.x - origin.x;
        let content_y = screen_pos.y - origin.y;
        if content_x < 0.0 || content_x > page_width_pts {
            return None;
        }
        for i in 0..total_pages {
            let top = offsets[i];
            let bottom = top + heights[i];
            if content_y >= top && content_y <= bottom {
                let rect = egui::Rect::from_min_size(
                    egui::pos2(0.0, top),
                    egui::vec2(page_width_pts, heights[i]),
                )
                .translate(origin.to_vec2());
                return Some(((i + 1) as u32, rect));
            }
        }
        None
    };

    // 페이지 폭이 직전 프레임과 달라졌는지(줌/창 크기 변화). 전체 레이아웃이 폭에 따라
    // 커지고 작아지므로 오프셋을 그대로 두면 같은 y가 다른 페이지를 가리켜 확대=앞쪽/
    // 축소=뒤쪽으로 점프한다(2026-07-18 리포트) — 아래에서 앵커로 보정한다.
    let last_width = app.continuous_last_page_width;
    let width_changed = last_width > 0.0 && (page_width_pts - last_width).abs() > 0.5;
    app.continuous_last_page_width = page_width_pts;

    // 재렌더링 디바운스: 핀치 줌 중 pdfium 재렌더링을 하면 심하게 버벅이므로, 배율이 바뀐
    // 뒤 일정 시간 동안은 기존 텍스처를 늘려 그리고, 멎은 뒤에야 원해상도로 업그레이드한다.
    // (예전엔 "폭이 바뀐 바로 그 프레임"만 건너뛰어서, 핀치 도중 한 프레임만 쉬어도
    // 중간 배율로 렌더링이 시작돼 곧 버려졌다.)
    let since_zoom = ctx.input(|i| i.time) - app.zoom_changed_at;
    let zoom_settling = since_zoom < CONTINUOUS_RESCALE_DEBOUNCE_SECS;
    if zoom_settling {
        ctx.request_repaint_after(std::time::Duration::from_secs_f64(
            CONTINUOUS_RESCALE_DEBOUNCE_SECS - since_zoom,
        ));
    }

    // 스크롤 오프셋을 프레임 시작 전에 직접 지정해야 하는 두 경우를 계산한다.
    // vertical_scroll_offset은 이번 프레임 그리기 "전에" 적용되므로 scroll_to_rect처럼
    // 애니메이션으로 중간 페이지들을 경유하는 모습이 보이지 않는다('C' 진입 시 다른
    // 페이지를 스쳤다가 돌아오던 증상의 해결책, 2026-07-18 리포트).
    let scroll_id = ui.make_persistent_id(egui::Id::new("continuous_scroll_area"));
    let scroll_state = egui::containers::scroll_area::State::load(ui.ctx(), scroll_id);
    let mut override_offset: Option<f32> = None;
    if let Some(target_page) = app.scroll_to_page_once.take() {
        // 북마크 클릭/검색 이동/페이지 입력/'C' 진입 — 그 페이지 상단으로 즉시 이동.
        let idx = (target_page as usize).saturating_sub(1);
        if let Some(&top) = offsets.get(idx) {
            override_offset = Some(top);
        }
    } else if width_changed {
        if let Some(state) = &scroll_state {
            // 비율 곱셈(offset × 새폭/옛폭)으로 보정하면 안 된다 — 전체 높이는
            // Σ(페이지 높이) + PAGE_GAP × 간격 수인데, 배율과 무관한 뒤쪽 상수항에도 비율이
            // 곱해져 `간격 × 위쪽 페이지 수 × (비율 − 1)`만큼 어긋난다(뒤쪽 페이지일수록
            // 크게). 대신 뷰포트 중앙이 가리키던 "페이지 i의 f% 지점"을 옛 레이아웃에서
            // 구하고 새 레이아웃에서 다시 계산한다 — 재계산이라 오차가 섞이지 않는다.
            let (old_offsets, old_heights, _) =
                continuous_layout(&app.page_aspects, total_pages, last_width);
            let half_view = available.y / 2.0;
            let anchor = anchor_at(&old_offsets, &old_heights, state.offset.y + half_view);
            override_offset =
                Some((y_for_anchor(&offsets, &heights, anchor) - half_view).max(0.0));
        }
    }

    // 검색 결과를 골랐으면 페이지 상단 대신 그 검색어 위치가 화면 중앙에 오게 한다 — 세로는
    // 스크롤 오프셋, 가로는 continuous_pan_x(패널보다 넓게 확대된 경우만 움직임).
    if let Some(index) = app.search_center_request.take() {
        if let Some((match_page, point)) =
            search_match_center(app, index, target_width, ctx.pixels_per_point())
        {
            let idx = (match_page as usize).saturating_sub(1);
            if let Some(&page_top) = offsets.get(idx) {
                override_offset = Some(scroll_offset_to_center(page_top, point.y, available.y));
                app.continuous_pan_x =
                    pan_x_to_center(page_width_pts, point.x, available.x);
            }
        }
    }

    // 스크롤이 진행 중인지(손가락 스크롤 이벤트 또는 관성 스크롤 감속 중). 페이지 경계에서
    // 새 페이지를 원해상도로 동기 렌더링하면 그 프레임이 길어져 스크롤이 한 번 "덜컹"하는
    // 문제(2026-07-18 리포트)의 완화책: 스크롤 중엔 반해상도(픽셀 1/4)로 빠르게 렌더링해
    // 프레임 시간을 줄이고, 멎은 뒤에 프레임당 1장씩 원해상도로 다시 그린다(정지 상태에서
    // 여러 장을 한 프레임에 업그레이드하면 그때 또 덜컹하므로 분할). 렌더링 보조 프로세스
    // (render_worker)가 있으면 렌더링이 UI 스레드를 막지 않으므로 이 완화책은 쓰지 않고,
    // 보조 프로세스가 없을 때의 대체 경로에서만 쓴다.
    let scrolling = scroll_state.as_ref().is_some_and(|s| s.velocity().y.abs() > 50.0)
        || ctx.input(|i| i.smooth_scroll_delta.y != 0.0);
    let scroll_render_width = (target_width / 2).max(400).min(target_width);

    // 쪽 단위 모드와 픽셀 단위로 같은 가로 중앙 위치를 쓰기 위해, ScrollArea에 들어가기
    // 전에 패널 기준 왼쪽 끝을 잡아둔다 — 안쪽 clip 폭 기반으로 계산했더니 쪽 단위 대비
    // 8pt 오른쪽으로 치우쳤음(스크린샷 픽셀 실측, 2026-07-18 리포트). 쪽 단위 모드의
    // `Rect::from_center_size(rect.center(), ...)`와 동일하게 "패널 전체 폭의 중앙"을
    // 기준으로 삼는다(페이지가 패널보다 넓으면 좌우 대칭으로 넘침 — 이것도 쪽 단위와 동일).
    let outer_left = ui.available_rect_before_wrap().min.x;

    let mut scroll_area = egui::ScrollArea::vertical()
        .id_salt("continuous_scroll_area")
        .auto_shrink([false, false]);
    if let Some(offset) = override_offset {
        scroll_area = scroll_area.vertical_scroll_offset(offset);
    }
    scroll_area
        .show_viewport(ui, |ui, viewport| {
            ui.set_width(page_width_pts);
            ui.set_height(total_height);

            // 가로 중앙 정렬 — 쪽 단위 모드와 동일하게 "패널 전체 폭"(outer_left +
            // available.x) 기준으로 중앙을 계산한다(안쪽 clip 폭 기준으로 했더니 8pt
            // 오른쪽으로 치우침 — 위 outer_left 주석 참고). 이 x를 origin에 접어 넣어
            // 히트테스트(page_at)/클릭 영역/그리기가 전부 같은 좌표를 쓰게 한다.
            let origin = egui::pos2(
                outer_left + (available.x - page_width_pts) / 2.0 + app.continuous_pan_x,
                ui.max_rect().min.y,
            );

            // 전체 문서 영역 하나에 클릭+드래그를 건다 — 페이지별로 따로 Response를 만들지
            // 않고 이 하나로 클릭(포커스/링크)·드래그(텍스트 선택)를 전부 처리한다.
            let full_rect = egui::Rect::from_min_size(origin, egui::vec2(page_width_pts, total_height));
            let full_response = ui.interact(
                full_rect,
                ui.id().with("continuous_interact"),
                Sense::click_and_drag(),
            );

            if let Some(pos) = full_response.hover_pos() {
                if let Some((page_number, page_rect)) = page_at(pos, origin) {
                    if link_target_at_screen_pos(app, pos, page_rect, target_width, page_number).is_some() {
                        ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
                    } else if char_index_at_screen_pos(app, pos, page_rect, target_width, page_number)
                        .is_some()
                    {
                        ctx.set_cursor_icon(egui::CursorIcon::Text);
                    }
                }
            }

            full_response.context_menu(|ui| {
                if ui
                    .add_enabled(app.selection.is_some(), egui::Button::new("복사"))
                    .clicked()
                {
                    app.copy_selection_to_clipboard();
                    ui.close_menu();
                }
            });

            if full_response.clicked() {
                app.focus_area = crate::app::FocusArea::Viewer;
                // 클릭 시 텍스트 선택 해제 — 쪽 단위 모드와 동일한 관례(2026-07-18 요청).
                app.selection = None;
                app.selection_page = None;
                if let Some(pos) = full_response.interact_pointer_pos() {
                    if let Some((page_number, page_rect)) = page_at(pos, origin) {
                        match link_target_at_screen_pos(app, pos, page_rect, target_width, page_number) {
                            Some(LinkTarget::Page(page)) => app.go_to_page(page),
                            Some(LinkTarget::Uri(url)) => app.open_external_link(&url),
                            None => {}
                        }
                    }
                }
            }

            // 텍스트 선택 — 드래그가 시작된 페이지(앵커, app.selection_page)를 벗어나면
            // 그 프레임은 갱신하지 않고 무시한다(한 페이지 안에서만 선택 — 문서 상단 docs
            // 참고).
            if full_response.drag_started() {
                let hit = full_response
                    .interact_pointer_pos()
                    .and_then(|pos| page_at(pos, origin));
                app.selection = None;
                app.selection_page = None;
                app.selection_drag_start_index = None;
                if let Some((page_number, page_rect)) = hit {
                    if let Some(pos) = full_response.interact_pointer_pos() {
                        if let Some(idx) =
                            char_index_at_screen_pos(app, pos, page_rect, target_width, page_number)
                        {
                            app.selection_drag_start_index = Some(idx);
                            app.selection_page = Some(page_number);
                        }
                    }
                }
            } else if full_response.dragged() {
                if let (Some(start), Some(anchor_page)) =
                    (app.selection_drag_start_index, app.selection_page)
                {
                    if let Some(pos) = full_response.interact_pointer_pos() {
                        if let Some((page_number, page_rect)) = page_at(pos, origin) {
                            if page_number == anchor_page {
                                if let Some(current) = char_index_at_screen_pos(
                                    app,
                                    pos,
                                    page_rect,
                                    target_width,
                                    page_number,
                                ) {
                                    app.selection =
                                        Some(TextSelectionRange::from_anchors(start, current));
                                }
                            }
                        }
                    }
                }
            }
            if full_response.drag_stopped() && app.selection.is_none() {
                // 문자 위에서 시작 못 한 드래그(빈 여백 등) — 앵커 페이지도 정리.
                app.selection_page = None;
            }

            // `viewport`는 콘텐츠 자체의 좌표계(스크롤 안 했으면 0에서 시작) — 실제 화면
            // clip 영역과는 다른 좌표계라 `ui.is_rect_visible()`로는 이 범위를 정확히 알 수
            // 없다(egui 문서: "the relative view of the content"). 위아래로 페이지 하나
            // 폭 정도 여유를 둬서 스크롤 도중 팝인이 덜 보이게 미리 렌더링한다.
            let buffer = page_width_pts.max(200.0);
            let visible_top = (viewport.min.y - buffer).max(0.0);
            let visible_bottom = viewport.max.y + buffer;

            let mut first_visible: Option<usize> = None;
            let mut last_visible = 0usize;
            for i in 0..total_pages {
                let top = offsets[i];
                let bottom = top + heights[i];
                if bottom >= visible_top && top <= visible_bottom {
                    first_visible.get_or_insert(i);
                    last_visible = i;
                }
            }
            let first_visible = first_visible.unwrap_or(0);

            // 지금 뷰포트 중앙에 가장 가까운 페이지를 "현재 페이지"로 추적 — 사이드바 선택
            // 동기화/창 제목/페이지 번호 입력창이 스크롤을 따라간다(note_visible_page_during_scroll
            // 문서 참고 — 일반 페이지 이동과 달리 히스토리/선택 상태는 안 건드림).
            let center_y = (viewport.min.y + viewport.max.y) / 2.0;
            let mut tracked_page = first_visible;
            let mut best_dist = f32::MAX;
            for i in first_visible..=last_visible {
                let mid = offsets[i] + heights[i] / 2.0;
                let dist = (mid - center_y).abs();
                if dist < best_dist {
                    best_dist = dist;
                    tracked_page = i;
                }
            }
            app.note_visible_page_during_scroll((tracked_page + 1) as u32);

            // 보조 프로세스 대기열의 기준 — 보이는 페이지 범위나 배율이 바뀌면 옛 요청은 버려진다.
            app.set_render_view((
                true,
                (first_visible + 1) as u32,
                (last_visible + 1) as u32,
                target_width,
            ));

            // 보이는 범위(+한 페이지 여유) 밖의 텍스처는 버려서 큰 문서에서도 메모리를
            // 무한정 쓰지 않게 한다 — 드롭되는 즉시 egui 텍스처 매니저가 GPU 메모리도 해제.
            let keep_lo = first_visible.saturating_sub(1);
            let keep_hi = (last_visible + 1).min(total_pages.saturating_sub(1));
            app.page_textures.retain(|page_number| {
                let idx = (page_number as usize).saturating_sub(1);
                idx >= keep_lo && idx <= keep_hi
            });

            // 재렌더링 정책:
            // - 보조 프로세스가 있으면(render_worker) 렌더링이 UI를 막지 않으므로, 필요한
            //   페이지를 화면 중앙에 가까운 순으로 곧바로 요청한다(배율이 바뀌는 중에만 기다림).
            // - 없으면(대체 경로, 위 scrolling 주석): 텍스처가 없는 페이지는 스크롤/줌 중엔
            //   반해상도로 빠르게, 해상도가 안 맞는 캐시는 줌도 스크롤도 멎은 뒤 프레임당 1장씩만
            //   원해상도로 업그레이드(여러 장을 한 프레임에 하면 그때 또 덜컹하므로 분할).
            let async_render = app.render_worker_active();
            let mut render_order: Vec<usize> = (first_visible..=last_visible).collect();
            render_order.sort_by(|&a, &b| {
                let distance = |i: usize| (offsets[i] + heights[i] / 2.0 - center_y).abs();
                distance(a).total_cmp(&distance(b))
            });
            let mut upgraded_this_frame = false;
            for i in render_order {
                let page_number = (i + 1) as u32;
                let cached_width = app.page_textures.width(page_number);
                let (needs_render, render_width) = match cached_width {
                    None => (
                        true,
                        if !async_render && (scrolling || zoom_settling) {
                            scroll_render_width
                        } else {
                            target_width
                        },
                    ),
                    Some(w) => (
                        w != target_width
                            && !zoom_settling
                            && (async_render || (!scrolling && !upgraded_this_frame)),
                        target_width,
                    ),
                };
                if needs_render {
                    if !async_render {
                        if cached_width.is_some() {
                            upgraded_this_frame = true;
                        }
                        // 업그레이드가 남아 있을 수 있으니 다음 프레임을 강제로 깨운다 —
                        // egui는 입력이 없으면 리페인트하지 않아 마지막 스크롤 후 업그레이드가
                        // 다음 마우스 조작까지 멈춰 보일 수 있다(§7의 즉시모드 함정과 동일).
                        // (보조 프로세스 경로는 결과가 도착할 때 응답 스레드가 깨운다.)
                        ctx.request_repaint();
                    }
                    app.request_page_texture(ctx, page_number, render_width);
                }
            }

            let mut painted_ids = Vec::new();
            for i in first_visible..=last_visible {
                let page_number = (i + 1) as u32;
                let page_rect = egui::Rect::from_min_size(
                    egui::pos2(0.0, offsets[i]),
                    egui::vec2(page_width_pts, heights[i]),
                )
                .translate(origin.to_vec2());

                // 렌더 결과를 기다리는 페이지는 흰 페이지로 자리만 잡아 둔다.
                match app.page_textures.get(page_number) {
                    Some((texture, _)) => {
                        painted_ids.push(texture.id());
                        ui.painter().image(
                            texture.id(),
                            page_rect,
                            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                            egui::Color32::WHITE,
                        );
                    }
                    None => {
                        ui.painter().rect_filled(page_rect, 0.0, egui::Color32::WHITE);
                    }
                }

                draw_selection_highlight(ui, app, page_rect, target_width, page_number);
                draw_search_highlight(ui, app, page_rect, target_width, page_number);
            }
            app.page_textures.note_painted(&painted_ids);

            // (북마크 클릭/검색 이동 등 명시적 페이지 이동(scroll_to_page_once)은
            // ScrollArea를 만들기 전에 vertical_scroll_offset으로 소비된다 — 위 참고.
            // scroll_to_rect 방식은 (a) 화면/content 좌표 혼동으로 오프셋이 중복 가산돼
            // 문서 끝으로 붙어버리는 버그를 만들었고, (b) 고쳐도 애니메이션이 중간
            // 페이지들을 스쳐 지나가는 게 보여서 즉시 점프 방식으로 교체했다.)
        });
}

/// 연속 스크롤 레이아웃 — 페이지별 상단 y·높이(pt)와 전체 높이. 크기를 이 한 식으로만
/// 정하므로 그리기·히트테스트·가상화 범위·스크롤 보정이 서로 어긋날 수 없다.
/// page_aspects(문서를 열 때 1회 계산, app.rs 참고)가 아직 없으면 A4 비슷한 기본값을 쓴다.
fn continuous_layout(
    page_aspects: &[f32],
    total_pages: usize,
    page_width: f32,
) -> (Vec<f32>, Vec<f32>, f32) {
    let mut offsets = Vec::with_capacity(total_pages);
    let mut heights = Vec::with_capacity(total_pages);
    let mut cursor = 0.0_f32;
    for i in 0..total_pages {
        let height = page_width * page_aspects.get(i).copied().unwrap_or(1.414);
        offsets.push(cursor);
        heights.push(height);
        cursor += height + PAGE_GAP;
    }
    (offsets, heights, (cursor - PAGE_GAP).max(0.0))
}

/// 배율과 무관한 스크롤 기준점 — "몇 번째(0-based) 페이지의 몇 % 지점".
#[derive(Debug, Clone, Copy, PartialEq)]
struct ScrollAnchor {
    page: usize,
    fraction: f32,
}

/// 레이아웃 y → 앵커. 페이지 사이 간격에 있는 y는 바로 위 페이지의 아래 끝(100%)으로
/// 붙인다 — 간격은 배율과 무관하게 고정이라 비율로 표현할 수 없어서다(최대 PAGE_GAP만큼
/// 한 번 움직일 뿐, 반복 줌에서 쌓이지 않는다).
fn anchor_at(offsets: &[f32], heights: &[f32], y: f32) -> ScrollAnchor {
    let page = offsets.partition_point(|&top| top <= y).saturating_sub(1);
    let (top, height) = (
        offsets.get(page).copied().unwrap_or(0.0),
        heights.get(page).copied().unwrap_or(0.0),
    );
    let fraction = if height > 0.0 {
        ((y - top) / height).clamp(0.0, 1.0)
    } else {
        0.0
    };
    ScrollAnchor { page, fraction }
}

/// 앵커 → 주어진 레이아웃에서의 y.
fn y_for_anchor(offsets: &[f32], heights: &[f32], anchor: ScrollAnchor) -> f32 {
    match (offsets.get(anchor.page), heights.get(anchor.page)) {
        (Some(top), Some(height)) => top + height * anchor.fraction,
        _ => 0.0,
    }
}

/// 화면 좌표(스크린 픽셀) → 렌더링에 쓰인 PdfRenderConfig 기준 비트맵 픽셀 → PDF 포인트 →
/// 그 위치의 링크(있다면). char_index_at_screen_pos와 동일한 변환 과정을 거치므로
/// 화면에 보이는 링크 영역과 클릭 판정이 어긋나지 않는다.
fn link_target_at_screen_pos(
    app: &PdfViewerApp,
    screen_pos: egui::Pos2,
    image_rect: egui::Rect,
    target_width: i32,
    page_number: u32,
) -> Option<LinkTarget> {
    if !image_rect.contains(screen_pos) {
        return None;
    }
    let document = app.document.as_ref()?;
    let page = document
        .pages()
        .get((page_number - 1) as PdfPageIndex)
        .ok()?;

    let config = PdfRenderConfig::new().set_target_width(target_width);
    let scale = image_rect.width() / target_width as f32;
    let pixel_x = ((screen_pos.x - image_rect.left()) / scale) as i32;
    let pixel_y = ((screen_pos.y - image_rect.top()) / scale) as i32;

    let (x, y) = page.pixels_to_points(pixel_x, pixel_y, &config).ok()?;
    pdf_engine::links::link_target_at_point(&page, x, y)
}

/// 화면 좌표(스크린 픽셀) → 렌더링에 쓰인 PdfRenderConfig 기준 비트맵 픽셀 → PDF 포인트 →
/// 문자 인덱스. 렌더링과 히트테스트가 동일한 target_width 기반 PdfRenderConfig를 쓰기 때문에
/// 화면에 보이는 문자와 클릭 판정이 어긋나지 않는다.
fn char_index_at_screen_pos(
    app: &PdfViewerApp,
    screen_pos: egui::Pos2,
    image_rect: egui::Rect,
    target_width: i32,
    page_number: u32,
) -> Option<i32> {
    if !image_rect.contains(screen_pos) {
        return None;
    }
    let document = app.document.as_ref()?;
    let page = document
        .pages()
        .get((page_number - 1) as PdfPageIndex)
        .ok()?;
    let text_page = page.text().ok()?;

    let config = PdfRenderConfig::new().set_target_width(target_width);
    let scale = image_rect.width() / target_width as f32;
    let pixel_x = ((screen_pos.x - image_rect.left()) / scale) as i32;
    let pixel_y = ((screen_pos.y - image_rect.top()) / scale) as i32;

    let (x, y) = page.pixels_to_points(pixel_x, pixel_y, &config).ok()?;
    let tolerance = PdfPoints::new(6.0);
    pdf_engine::selection::char_index_at_point(&text_page, x, y, tolerance, tolerance)
}

/// 선택 영역을 문자별 quad로 그린다(스큐/세로쓰기에도 정확히 따라가도록 축정렬 사각형으로
/// 뭉뚱그리지 않는다 — pdf_engine::skew 설계 참고).
fn draw_selection_highlight(
    ui: &egui::Ui,
    app: &PdfViewerApp,
    image_rect: egui::Rect,
    target_width: i32,
    page_number: u32,
) {
    let Some(range) = app.selection else { return };
    // 선택이 이 페이지의 것이 아니면(연속 스크롤 모드에서 다른 페이지에 선택이 있는 채로
    // 스크롤한 경우) 그리지 않는다 — app::selection_page 문서 참고.
    if app.selection_page != Some(page_number) {
        return;
    }
    let Some(document) = app.document.as_ref() else {
        return;
    };
    let Ok(page) = document
        .pages()
        .get((page_number - 1) as PdfPageIndex)
    else {
        return;
    };
    let Ok(text_page) = page.text() else { return };
    let Ok(quads) = pdf_engine::selection::selection_quads(&text_page, range) else {
        return;
    };

    let config = PdfRenderConfig::new().set_target_width(target_width);
    let scale = image_rect.width() / target_width as f32;

    let to_screen = |point: (f32, f32)| -> Option<egui::Pos2> {
        let (px, py) = page
            .points_to_pixels(PdfPoints::new(point.0), PdfPoints::new(point.1), &config)
            .ok()?;
        Some(egui::pos2(
            image_rect.left() + px as f32 * scale,
            image_rect.top() + py as f32 * scale,
        ))
    };

    let painter = ui.painter();
    for quad in quads {
        if let (Some(a), Some(b), Some(c), Some(d)) = (
            to_screen(quad.top_left),
            to_screen(quad.top_right),
            to_screen(quad.bottom_right),
            to_screen(quad.bottom_left),
        ) {
            painter.add(egui::Shape::convex_polygon(
                vec![a, b, c, d],
                egui::Color32::from_rgba_unmultiplied(80, 150, 255, 90),
                egui::Stroke::NONE,
            ));
        }
    }
}

/// 검색 결과에 bounding box를 그린다. 텍스트 선택 하이라이트와 달리 문자별 quad를 우리가
/// 계산하지 않고, pdfium이 이미 계산해 둔 병합된 사각형(`SearchMatch::rects`,
/// `pdf_engine::search` 참고)을 그대로 화면 좌표로 변환만 해서 쓴다 — 검색 결과 강조는
/// 스큐 보정이 필요 없는 일반적인 사각형 하이라이트라 이 편이 더 간단하고 정확하다.
///
/// 현재 페이지에 있는 모든 일치 항목을 노란색으로 표시하되, 지금 순회 중인 항목만 주황색
/// (텍스트 선택 하이라이트의 파란색과도 구별됨)으로 돋보이게 한다 — 브라우저 찾기 기능의
/// 일반적인 관례(전체는 옅게, 현재는 진하게)와 같다.
fn draw_search_highlight(
    ui: &egui::Ui,
    app: &PdfViewerApp,
    image_rect: egui::Rect,
    target_width: i32,
    page_number: u32,
) {
    // 결과는 페이지 순이라 이 페이지의 첫 결과를 이진 탐색으로 찾는다 — 결과가 수천 건이어도
    // 보이는 페이지마다 전체를 훑지 않게.
    let first = app.search_matches.partition_point(|m| m.page < page_number);
    if app.search_matches.get(first).map(|m| m.page) != Some(page_number) {
        return;
    }
    let Some(document) = app.document.as_ref() else {
        return;
    };
    let Ok(page) = document
        .pages()
        .get((page_number - 1) as PdfPageIndex)
    else {
        return;
    };

    let config = PdfRenderConfig::new().set_target_width(target_width);
    let scale = image_rect.width() / target_width as f32;

    let to_screen = |point: (f32, f32)| -> Option<egui::Pos2> {
        let (px, py) = page
            .points_to_pixels(PdfPoints::new(point.0), PdfPoints::new(point.1), &config)
            .ok()?;
        Some(egui::pos2(
            image_rect.left() + px as f32 * scale,
            image_rect.top() + py as f32 * scale,
        ))
    };

    // 지금 선택된 결과만 진한 주황색으로 채워 도드라지게 하고, 같은 페이지의 나머지 일치는
    // 테두리만 그린다 — 반투명 배경을 칠하면 글자 위에 색이 덮여 원문이 흐려 보인다는
    // 피드백(2026-09-14). 페이지 텍스처가 불투명 이미지라 "글자 아래에만" 칠할 방법이 없어서
    // 채움을 빼는 것이 원문 선명도를 지키는 유일한 방법이다.
    let current_fill = egui::Color32::from_rgba_unmultiplied(255, 140, 0, 110);
    let current_stroke = egui::Color32::from_rgb(230, 90, 0);
    let other_fill = egui::Color32::TRANSPARENT;
    let other_stroke = egui::Color32::from_rgb(245, 166, 35);

    let painter = ui.painter();
    for (offset, m) in app.search_matches[first..].iter().enumerate() {
        if m.page != page_number {
            break;
        }
        let index = first + offset;
        let (fill, stroke, stroke_width) = if index == app.search_current_index {
            (current_fill, current_stroke, 2.5_f32)
        } else {
            (other_fill, other_stroke, 1.8_f32)
        };

        for rect in &m.rects {
            if let (Some(top_left), Some(bottom_right)) = (
                to_screen((rect.left().value, rect.top().value)),
                to_screen((rect.right().value, rect.bottom().value)),
            ) {
                // pdfium이 계산해 병합해준 rect는 글자의 advance(펜 이동) 기준이라 실제
                // 잉크 영역보다 좁은 경우가 있어, 특히 첫 글자 왼쪽이 하이라이트 밖으로
                // 튀어나와 보인다는 리포트(2026-07-18) — 사방으로 살짝 넓혀서 시각적으로
                // 보이는 글자를 확실히 덮게 한다.
                let screen_rect = egui::Rect::from_two_pos(top_left, bottom_right).expand(2.0);
                if fill != egui::Color32::TRANSPARENT {
                    painter.rect_filled(screen_rect, 2.0, fill);
                }
                painter.rect_stroke(screen_rect, 2.0, egui::Stroke::new(stroke_width, stroke));
            }
        }
    }
}

#[cfg(test)]
mod scroll_anchor_tests {
    use super::{anchor_at, continuous_layout, y_for_anchor, ScrollAnchor, PAGE_GAP};

    const PAGES: usize = 200;
    const HALF_VIEW: f32 = 400.0;

    /// 세로형/가로형이 섞인 문서 — 페이지마다 높이가 달라도 성립해야 한다.
    fn aspects() -> Vec<f32> {
        (0..PAGES).map(|i| if i % 3 == 0 { 0.773 } else { 1.414 }).collect()
    }

    /// 폭 `from` 레이아웃의 스크롤 오프셋을 폭 `to` 레이아웃으로 옮긴다(show_continuous와 같은 절차).
    fn rezoom(aspects: &[f32], scroll: f32, from: f32, to: f32) -> f32 {
        let (old_offsets, old_heights, _) = continuous_layout(aspects, PAGES, from);
        let (new_offsets, new_heights, _) = continuous_layout(aspects, PAGES, to);
        let anchor = anchor_at(&old_offsets, &old_heights, scroll + HALF_VIEW);
        (y_for_anchor(&new_offsets, &new_heights, anchor) - HALF_VIEW).max(0.0)
    }

    #[test]
    fn anchor_roundtrips_within_layout() {
        let aspects = aspects();
        let (offsets, heights, _) = continuous_layout(&aspects, PAGES, 700.0);
        let y = offsets[120] + heights[120] * 0.37;
        let anchor = anchor_at(&offsets, &heights, y);
        assert_eq!(anchor.page, 120);
        assert!((anchor.fraction - 0.37).abs() < 1e-3);
        assert!((y_for_anchor(&offsets, &heights, anchor) - y).abs() < 0.05);
    }

    /// 한 번 확대해도 뷰포트 중앙은 같은 페이지의 같은 % 지점에 머문다.
    #[test]
    fn zoom_keeps_center_on_same_spot() {
        let aspects = aspects();
        let (offsets, heights, _) = continuous_layout(&aspects, PAGES, 700.0);
        let scroll = offsets[150] + heights[150] * 0.5 - HALF_VIEW;
        let new_scroll = rezoom(&aspects, scroll, 700.0, 875.0);

        let (new_offsets, new_heights, _) = continuous_layout(&aspects, PAGES, 875.0);
        let anchor = anchor_at(&new_offsets, &new_heights, new_scroll + HALF_VIEW);
        assert_eq!(anchor.page, 150);
        assert!((anchor.fraction - 0.5).abs() < 1e-3);
    }

    /// 예전 방식(오프셋 × 폭비)은 고정 간격에도 비율이 곱해져 뒤쪽 페이지에서 크게 어긋난다
    /// — 앵커 방식을 쓰는 이유를 고정해 둔다.
    #[test]
    fn ratio_scaling_misplaces_by_gap_error() {
        let aspects = aspects();
        let (offsets, heights, _) = continuous_layout(&aspects, PAGES, 700.0);
        let scroll = offsets[150] + heights[150] * 0.5 - HALF_VIEW;
        let ratio = 875.0 / 700.0;
        // 기준점(뷰포트 중앙)은 같게 두고 보정 방식만 비교한다.
        let naive = (scroll + HALF_VIEW) * ratio - HALF_VIEW;
        let anchored = rezoom(&aspects, scroll, 700.0, 875.0);
        let expected_error = PAGE_GAP * 150.0 * (ratio - 1.0);
        assert!(((naive - anchored).abs() - expected_error).abs() < expected_error * 0.1);
    }

    /// 확대/축소를 여러 번 왕복해도 원래 위치로 돌아온다.
    #[test]
    fn repeated_zoom_roundtrip_does_not_drift() {
        let aspects = aspects();
        let widths = [700.0, 875.0, 1050.0, 1400.0, 1050.0, 875.0, 700.0];
        let (offsets, heights, _) = continuous_layout(&aspects, PAGES, widths[0]);
        let start = offsets[120] + heights[120] * 0.37 - HALF_VIEW;

        let mut scroll = start;
        for pair in widths.windows(2) {
            scroll = rezoom(&aspects, scroll, pair[0], pair[1]);
        }
        assert!((scroll - start).abs() < 0.5, "drifted {} pt", scroll - start);
    }

    /// 페이지 사이 간격에 있는 y는 위 페이지 아래 끝에 붙는다.
    #[test]
    fn gap_snaps_to_page_above() {
        let aspects = aspects();
        let (offsets, heights, _) = continuous_layout(&aspects, PAGES, 700.0);
        let y = offsets[5] + heights[5] + PAGE_GAP / 2.0;
        assert_eq!(anchor_at(&offsets, &heights, y), ScrollAnchor { page: 5, fraction: 1.0 });
    }
}

#[cfg(test)]
mod search_center_tests {
    use super::{pan_to_center, pan_x_to_center, scroll_offset_to_center};

    /// 쪽 단위: 팬을 적용하면 검색어 점이 정확히 패널 중앙에 온다(viewer의 image_rect 식 그대로 재현).
    #[test]
    fn single_page_pan_puts_point_at_panel_center() {
        let panel_center = egui::pos2(500.0, 400.0);
        let page_size = egui::vec2(2400.0, 3400.0);
        let point = egui::vec2(1800.0, 2900.0);
        let pan = pan_to_center(page_size, point);
        let image_rect = egui::Rect::from_center_size(panel_center + pan, page_size);
        let on_screen = image_rect.min + point;
        assert!((on_screen - panel_center).length() < 1e-3);
    }

    #[test]
    fn continuous_scroll_centers_vertically_but_not_above_document_top() {
        assert_eq!(scroll_offset_to_center(5000.0, 300.0, 800.0), 4900.0);
        assert_eq!(scroll_offset_to_center(0.0, 100.0, 800.0), 0.0);
    }

    #[test]
    fn continuous_pan_x_centers_within_page_edges() {
        // 2000pt 페이지, 800pt 패널 → 좌우 최대 600pt 이동.
        assert_eq!(pan_x_to_center(2000.0, 1000.0, 800.0), 0.0);
        assert_eq!(pan_x_to_center(2000.0, 1300.0, 800.0), -300.0);
        // 페이지 가장자리 근처는 가장자리를 넘지 않게 제한.
        assert_eq!(pan_x_to_center(2000.0, 1950.0, 800.0), -600.0);
        // 페이지가 패널보다 좁으면 움직이지 않는다.
        assert_eq!(pan_x_to_center(600.0, 50.0, 800.0), 0.0);
    }
}

#[cfg(test)]
mod prefetch_tests {
    use super::{prefetch_width, PREFETCH_MAX_PIXELS};

    /// 저배율에서는 현재 배율 그대로 미리 렌더링한다.
    #[test]
    fn low_zoom_prefetches_at_current_width() {
        assert_eq!(prefetch_width(1800, 1.414), 1800);
    }

    /// 고배율에서는 픽셀 수 상한 안으로 줄인다 — 세로로 긴 페이지일수록 더 좁게.
    #[test]
    fn high_zoom_is_capped_by_pixel_budget() {
        for aspect in [1.414_f32, 9.2] {
            let width = prefetch_width(11_000, aspect);
            assert!(width < 11_000);
            let pixels = width as f32 * width as f32 * aspect;
            assert!(pixels <= PREFETCH_MAX_PIXELS * 1.001, "aspect {aspect}: {pixels}");
        }
        assert!(prefetch_width(11_000, 9.2) < prefetch_width(11_000, 1.414));
    }
}
