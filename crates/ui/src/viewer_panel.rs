use crate::app::{PdfViewerApp, ViewportState};
use crate::toolbar::handle_scroll_zoom;
use egui::Sense;
use pdf_engine::links::LinkTarget;
use pdf_engine::selection::TextSelectionRange;
use pdfium_render::prelude::*;

pub fn show(ctx: &egui::Context, app: &mut PdfViewerApp) {
    handle_scroll_zoom(ctx, &mut app.viewport);

    egui::CentralPanel::default().show(ctx, |ui| {
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

        if app.continuous_scroll {
            show_continuous(ctx, app, ui, available, target_width);
        } else {
            show_single_page(ctx, app, ui, available, pixels_per_point, target_width);
        }
    });
}

/// 쪽 단위 보기(기본 모드) — 한 번에 페이지 하나만 렌더링해 보여준다. 검색 결과
/// 하이라이트는 아직 이 모드에서만 지원한다(연속 스크롤 모드는 텍스트 선택/링크 클릭까지는
/// 지원하지만 검색 하이라이트는 범위 밖 — `show_continuous` 문서 참고).
fn show_single_page(
    ctx: &egui::Context,
    app: &mut PdfViewerApp,
    ui: &mut egui::Ui,
    available: egui::Vec2,
    pixels_per_point: f32,
    target_width: i32,
) {
    {
        if app.rendered_for != Some((app.current_page, target_width)) {
            if let Err(err) = app.render_current_page(ctx, target_width) {
                app.status_message = Some(format!("렌더링 실패: {err}"));
            }
        }

        let (rect, response) = ui.allocate_exact_size(available, Sense::click_and_drag());

        let Some(texture) = app.page_texture.clone() else {
            app.image_rect = None;
            return;
        };

        let tex_size = texture.size_vec2() / pixels_per_point;
        app.viewport.clamp_pan(tex_size, available);

        let image_rect =
            egui::Rect::from_center_size(rect.center() + app.viewport.pan_offset, tex_size);

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
                app.viewport.clamp_pan(tex_size, available);
            }
        }
        if response.drag_stopped() {
            app.selection_drag_start_index = None;
        }

        ui.painter().image(
            texture.id(),
            image_rect,
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            egui::Color32::WHITE,
        );

        draw_selection_highlight(ui, app, image_rect, target_width, app.current_page);
        draw_search_highlight(ui, app, image_rect, target_width);

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
/// 페이지로 넘어가면 그 프레임은 무시 — `app.selection_page`가 앵커 페이지 기준). **범위
/// 밖**: 검색 결과 하이라이트는 아직 이 모드에서 안 보인다(선택과 달리 요청받지 않음 —
/// 필요하면 'C'로 쪽 단위 모드로). 트랙패드 두 손가락 드래그로 확대해 패널보다 넓어지면
/// 페이지가 가로 중앙 정렬된 채 양옆이 잘린다(가로 팬 미지원).
fn show_continuous(
    ctx: &egui::Context,
    app: &mut PdfViewerApp,
    ui: &mut egui::Ui,
    available: egui::Vec2,
    target_width: i32,
) {
    const PAGE_GAP: f32 = 8.0;

    let total_pages = app.total_pages.max(1) as usize;
    // 줌을 반영해야 한다 — 예전엔 `available.x`를 그대로 써서 배율과 무관하게 항상 폭
    // 맞춤으로 보이고, 확대/축소해도 텍스처(target_width, 줌 반영됨)만 해상도가 바뀌고
    // 화면에 그리는 크기는 그대로라 확대 시 흐릿해 보이는 문제가 있었다(2026-07-18 리포트
    // — "배율이 어떻든 되어야 함", "강제확대한 것처럼 sharpness가 떨어짐"). 쪽 단위 모드와
    // 동일하게 "줌 1.0 == 페이지 폭이 패널 폭과 같다"는 의미를 유지한다.
    let page_width_pts = available.x * app.viewport.zoom;

    // 각 페이지의 화면상 높이(pt)와 누적 y 오프셋(페이지 사이 간격 포함)을 미리 계산한다
    // — page_aspects(문서를 열 때 1회 계산, app.rs 참고)가 있으면 그 페이지의 실제
    // 비율을, 아직 없으면 A4 비슷한 기본값으로 대체한다.
    let mut offsets = Vec::with_capacity(total_pages);
    let mut heights = Vec::with_capacity(total_pages);
    let mut cursor = 0.0_f32;
    for i in 0..total_pages {
        let aspect = app.page_aspects.get(i).copied().unwrap_or(1.414);
        let height = page_width_pts * aspect;
        offsets.push(cursor);
        heights.push(height);
        cursor += height + PAGE_GAP;
    }
    let total_height = (cursor - PAGE_GAP).max(0.0);

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

    // 페이지 폭이 직전 프레임과 달라졌는지(줌/창 크기 변화). 두 가지에 쓰인다 —
    // (1) 스크롤 오프셋 비율 재조정: 전체 레이아웃이 폭에 비례해 커지고 작아지므로
    //     오프셋을 그대로 두면 같은 y가 다른 페이지를 가리켜 확대=앞쪽/축소=뒤쪽으로
    //     점프한다(2026-07-18 리포트). 폭 비율만큼 오프셋을 곱해 보던 위치를 유지한다.
    // (2) 재렌더링 스로틀: 핀치 줌 중 매 프레임 pdfium 재렌더링하면 심하게 버벅이므로,
    //     폭이 변하는 동안은 기존 텍스처를 늘려 그리고 줌이 멎은 다음 프레임에 한 번만
    //     선명하게 재렌더링한다(app::continuous_last_page_width 문서 참고).
    let last_width = app.continuous_last_page_width;
    let width_changed = last_width > 0.0 && (page_width_pts - last_width).abs() > 0.5;
    app.continuous_last_page_width = page_width_pts;

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
            override_offset = Some(state.offset.y * (page_width_pts / last_width));
        }
    }

    // 스크롤이 진행 중인지(손가락 스크롤 이벤트 또는 관성 스크롤 감속 중). 페이지 경계에서
    // 새 페이지를 원해상도로 동기 렌더링하면 그 프레임이 길어져 스크롤이 한 번 "덜컹"하는
    // 문제(2026-07-18 리포트)의 완화책: 스크롤 중엔 반해상도(픽셀 1/4)로 빠르게 렌더링해
    // 프레임 시간을 줄이고, 멎은 뒤에 프레임당 1장씩 원해상도로 다시 그린다(정지 상태에서
    // 여러 장을 한 프레임에 업그레이드하면 그때 또 덜컹하므로 분할). PDFium은 메인 스레드
    // 제약(§7)이 있어 백그라운드 렌더링으로는 풀 수 없다.
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
                outer_left + (available.x - page_width_pts) / 2.0,
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

            // 보이는 범위(+한 페이지 여유) 밖의 텍스처는 버려서 큰 문서에서도 메모리를
            // 무한정 쓰지 않게 한다 — 드롭되는 즉시 egui 텍스처 매니저가 GPU 메모리도 해제.
            let keep_lo = first_visible.saturating_sub(1);
            let keep_hi = (last_visible + 1).min(total_pages.saturating_sub(1));
            app.continuous_textures.retain(|&page_number, _| {
                let idx = (page_number as usize).saturating_sub(1);
                idx >= keep_lo && idx <= keep_hi
            });

            // 정지 상태에서의 원해상도 업그레이드는 프레임당 1장(아래 재렌더링 정책 참고).
            let mut upgraded_this_frame = false;
            for i in first_visible..=last_visible {
                let page_number = (i + 1) as u32;
                let page_rect = egui::Rect::from_min_size(
                    egui::pos2(0.0, offsets[i]),
                    egui::vec2(page_width_pts, heights[i]),
                )
                .translate(origin.to_vec2());

                // 재렌더링 정책(위 width_changed/scrolling 주석 참고):
                // - 텍스처가 아예 없는 페이지: 즉시 렌더링(빈 화면 방지) — 단 스크롤 중엔
                //   반해상도로 빠르게(페이지 경계 덜컹 완화), 정지 상태면 원해상도로.
                // - 해상도가 안 맞는 캐시(반해상도 잔재/줌 변경): 늘려서 그리다가, 줌도
                //   스크롤도 멎은 뒤 프레임당 1장씩만 원해상도로 업그레이드(여러 장을 한
                //   프레임에 하면 그때 또 덜컹하므로 분할 — upgraded_this_frame).
                let cached_width = app.continuous_textures.get(&page_number).map(|(_, w)| *w);
                let (needs_render, render_width) = match cached_width {
                    None => (
                        true,
                        if scrolling { scroll_render_width } else { target_width },
                    ),
                    Some(w) => (
                        w != target_width && !width_changed && !scrolling && !upgraded_this_frame,
                        target_width,
                    ),
                };
                if needs_render {
                    if cached_width.is_some() {
                        upgraded_this_frame = true;
                    }
                    if let Ok(texture) = app.render_page_texture(ctx, page_number, render_width) {
                        app.continuous_textures
                            .insert(page_number, (texture, render_width));
                    }
                    // 업그레이드가 남아 있을 수 있으니 다음 프레임을 강제로 깨운다 —
                    // egui는 입력이 없으면 리페인트하지 않아 마지막 스크롤 후 업그레이드가
                    // 다음 마우스 조작까지 멈춰 보일 수 있다(§7의 즉시모드 함정과 동일).
                    ctx.request_repaint();
                }

                if let Some((texture, _)) = app.continuous_textures.get(&page_number) {
                    ui.painter().image(
                        texture.id(),
                        page_rect,
                        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                        egui::Color32::WHITE,
                    );
                }

                draw_selection_highlight(ui, app, page_rect, target_width, page_number);
            }

            // (북마크 클릭/검색 이동 등 명시적 페이지 이동(scroll_to_page_once)은
            // ScrollArea를 만들기 전에 vertical_scroll_offset으로 소비된다 — 위 참고.
            // scroll_to_rect 방식은 (a) 화면/content 좌표 혼동으로 오프셋이 중복 가산돼
            // 문서 끝으로 붙어버리는 버그를 만들었고, (b) 고쳐도 애니메이션이 중간
            // 페이지들을 스쳐 지나가는 게 보여서 즉시 점프 방식으로 교체했다.)
        });
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
) {
    if app.search_matches.is_empty() {
        return;
    }
    let Some(document) = app.document.as_ref() else {
        return;
    };
    let Ok(page) = document
        .pages()
        .get((app.current_page - 1) as PdfPageIndex)
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

    let current_fill = egui::Color32::from_rgba_unmultiplied(255, 165, 0, 70);
    let current_stroke = egui::Color32::from_rgb(255, 140, 0);
    let other_fill = egui::Color32::from_rgba_unmultiplied(255, 235, 59, 60);
    let other_stroke = egui::Color32::from_rgb(255, 213, 79);

    let painter = ui.painter();
    for (index, m) in app.search_matches.iter().enumerate() {
        if m.page != app.current_page {
            continue;
        }
        let (fill, stroke) = if index == app.search_current_index {
            (current_fill, current_stroke)
        } else {
            (other_fill, other_stroke)
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
                painter.rect_filled(screen_rect, 2.0, fill);
                painter.rect_stroke(screen_rect, 2.0, egui::Stroke::new(2.0_f32, stroke));
            }
        }
    }
}
