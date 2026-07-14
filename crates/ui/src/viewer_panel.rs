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
        let zoom_delta = ctx.input(|i| i.zoom_delta());
        if zoom_delta != 1.0 {
            app.viewport.zoom_by(zoom_delta);
        }

        // 트랙패드 두 손가락 스와이프 = 패닝(스크롤). Ctrl+스크롤은 확대/축소로 이미 쓰고
        // 있으니(toolbar::handle_scroll_zoom) 그 조합일 때는 패닝에서 제외한다.
        if !ctx.input(|i| i.modifiers.ctrl) {
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

        let target_width =
            ((available.x * app.viewport.zoom * pixels_per_point).round() as i32).max(50);

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
            if link_target_at_screen_pos(app, pos, image_rect, target_width).is_some() {
                ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
            } else if char_index_at_screen_pos(app, pos, image_rect, target_width).is_some() {
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

        // 뷰어를 클릭하면 사이드바 북마크 선택을 해제한다 — 그래야 화살표 키가 다시
        // 페이지 이동으로 돌아온다(선택된 북마크가 있는 동안은 화살표가 트리 탐색용).
        if response.clicked() {
            app.selected_bookmark = None;

            // 클릭한 위치가 문서 내 링크(주석)라면 그 대상으로 이동/열기한다 — 문서 내
            // 다른 페이지를 가리키면 뷰어에서 바로 이동, 외부 URI(웹 링크 등)면 시스템
            // 기본 브라우저로 연다.
            if let Some(pos) = response.interact_pointer_pos() {
                match link_target_at_screen_pos(app, pos, image_rect, target_width) {
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
            .and_then(|pos| char_index_at_screen_pos(app, pos, image_rect, target_width));

        if response.drag_started() {
            app.selection_drag_start_index = hit_char;
            app.selection = None;
        } else if response.dragged() {
            if let Some(start) = app.selection_drag_start_index {
                if let Some(pos) = response.interact_pointer_pos() {
                    let current =
                        char_index_at_screen_pos(app, pos, image_rect, target_width).or(hit_char);
                    if let Some(current) = current {
                        app.selection = Some(TextSelectionRange::from_anchors(start, current));
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

        draw_selection_highlight(ui, app, image_rect, target_width);
        draw_search_highlight(ui, app, image_rect, target_width);

        app.image_rect = Some(image_rect);

        // macOS 트랙패드 핀치 제스처는 eframe 기본 추상화 밖 -> raw winit
        // WindowEvent::PinchGesture 후킹이 필요 (별도 platform integration 모듈에서 처리 예정)
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
) -> Option<LinkTarget> {
    if !image_rect.contains(screen_pos) {
        return None;
    }
    let document = app.document.as_ref()?;
    let page = document
        .pages()
        .get((app.current_page - 1) as PdfPageIndex)
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
) -> Option<i32> {
    if !image_rect.contains(screen_pos) {
        return None;
    }
    let document = app.document.as_ref()?;
    let page = document
        .pages()
        .get((app.current_page - 1) as PdfPageIndex)
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
) {
    let Some(range) = app.selection else { return };
    let Some(document) = app.document.as_ref() else {
        return;
    };
    let Ok(page) = document
        .pages()
        .get((app.current_page - 1) as PdfPageIndex)
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
                let screen_rect = egui::Rect::from_two_pos(top_left, bottom_right);
                painter.rect_filled(screen_rect, 2.0, fill);
                painter.rect_stroke(screen_rect, 2.0, egui::Stroke::new(2.0_f32, stroke));
            }
        }
    }
}
