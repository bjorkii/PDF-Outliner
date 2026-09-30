//! 사이드바의 썸네일 탭(예약 7) — 쪽 미리보기를 세로로 늘어놓고 눌러서 이동한다.
//!
//! **보이는 것만 렌더링한다.** 수백 쪽 문서에서 전부 미리 그리면 시작이 멈춘다. egui의
//! `ScrollArea::show_rows`가 지금 화면에 걸리는 줄 번호만 알려 주므로, 그 줄의 쪽만 렌더 워커에
//! 맡기고 나머지는 자리만 잡아 둔다. 스크롤하면 그때그때 채워진다.
//!
//! 렌더는 뷰어와 **같은 보조 프로세스**를 쓴다(`app::request_page_texture`). 따로 두지 않는 이유는
//! pdfium이 스레드 안전하지 않아 어차피 프로세스를 나눠야 하고, 이미 있는 것을 쓰면 요청 폐기
//! (epoch)와 실패 처리가 그대로 따라오기 때문이다.
//!
//! 뷰어 텍스처와 **캐시를 나눠 둔다**. `page_textures`는 쪽마다 배율 하나만 들고 있어서, 썸네일을
//! 거기 넣으면 보고 있던 큰 텍스처를 밀어낸다.

use crate::app::PdfViewerApp;
use std::collections::{HashMap, HashSet};

/// 썸네일 렌더 폭(px). 사이드바를 넓혀도 이 폭으로 한 번만 그려 두고 늘려 보여 준다.
pub const THUMB_WIDTH: i32 = 160;

/// 캐시에 담아 둘 쪽 수. 넘으면 지금 보이는 자리에서 먼 것부터 버린다.
const CACHE_LIMIT: usize = 240;

/// 줄 하나에서 그림 아래 쪽 번호가 차지하는 높이.
const LABEL_HEIGHT: f32 = 18.0;
/// 줄 사이 여백.
const ROW_GAP: f32 = 10.0;

#[derive(Default)]
pub struct Thumbnails {
    cache: HashMap<u32, egui::TextureHandle>,
    /// 렌더를 맡겨 두고 기다리는 쪽 — 응답이 썸네일 것인지 가리는 데도 쓴다.
    inflight: HashSet<u32>,
}

impl Thumbnails {
    /// 문서가 바뀌면 전부 버린다.
    pub fn clear(&mut self) {
        self.cache.clear();
        self.inflight.clear();
    }

    /// 이 쪽의 렌더 결과를 기다리는 중인가. 워커 응답을 썸네일로 받을지 가리는 데 쓴다.
    pub fn is_waiting(&self, page: u32) -> bool {
        self.inflight.contains(&page)
    }

    pub fn insert(&mut self, page: u32, texture: egui::TextureHandle) {
        self.inflight.remove(&page);
        self.cache.insert(page, texture);
    }

    /// 렌더가 실패했거나 버려졌을 때 — 다음에 다시 맡길 수 있게 표시만 지운다.
    pub fn give_up(&mut self, page: u32) {
        self.inflight.remove(&page);
    }

    /// 캐시가 너무 커지면 지금 보는 자리에서 먼 쪽부터 버린다.
    fn trim(&mut self, around: u32) {
        if self.cache.len() <= CACHE_LIMIT {
            return;
        }
        let mut pages: Vec<u32> = self.cache.keys().copied().collect();
        pages.sort_by_key(|page| page.abs_diff(around));
        for page in pages.into_iter().skip(CACHE_LIMIT) {
            self.cache.remove(&page);
        }
    }
}

/// 한 줄(썸네일 하나)의 높이. 쪽 크기에 따라 그림 높이가 달라지므로 문서에서 재어 쓴다.
fn row_height(app: &PdfViewerApp, width: f32) -> f32 {
    let size = app.page_size_pt(app.current_page.max(1));
    let ratio = if size.x > 0.0 { (size.y / size.x) as f32 } else { 1.414 };
    width * ratio + LABEL_HEIGHT + ROW_GAP
}

/// 썸네일 탭 본문.
pub fn show(ui: &mut egui::Ui, app: &mut PdfViewerApp) {
    let Some(document) = app.document.as_ref() else {
        ui.weak("열린 문서가 없습니다.");
        return;
    };
    let pages = document.pages().len() as u32;
    if pages == 0 {
        return;
    }

    // 그림 폭은 사이드바 폭에 맞추되, 렌더는 늘 THUMB_WIDTH로 한 번만 한다.
    let width = (ui.available_width() - 16.0).clamp(60.0, 240.0);
    let height = row_height(app, width);
    let mut go_to: Option<u32> = None;

    egui::ScrollArea::vertical().auto_shrink([false, false]).show_rows(
        ui,
        height,
        pages as usize,
        |ui, rows| {
            for index in rows {
                let page = index as u32 + 1;
                if let Some(target) = row(ui, app, page, width, height) {
                    go_to = Some(target);
                }
            }
        },
    );

    if let Some(page) = go_to {
        app.focus_area = crate::app::FocusArea::Sidebar;
        app.go_to_page(page);
    }
    app.thumbnails.trim(app.current_page);
}

/// 줄 하나. 눌렸으면 그 쪽 번호를 돌려준다.
fn row(ui: &mut egui::Ui, app: &mut PdfViewerApp, page: u32, width: f32, height: f32) -> Option<u32> {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(ui.available_width(), height), egui::Sense::click());
    if !ui.is_rect_visible(rect) {
        return None;
    }
    let image_rect = egui::Rect::from_min_size(
        egui::pos2(rect.center().x - width / 2.0, rect.top()),
        egui::vec2(width, height - LABEL_HEIGHT - ROW_GAP),
    );

    let is_current = page == app.current_page;
    match app.thumbnails.cache.get(&page) {
        Some(texture) => {
            ui.painter().image(
                texture.id(),
                image_rect,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        }
        None => {
            // 아직 없으면 흰 자리만 잡아 두고 렌더를 맡긴다. 자리를 잡아 두어야 스크롤 막대
            // 길이가 흔들리지 않는다.
            ui.painter().rect_filled(image_rect, 1.0, egui::Color32::from_gray(245));
            if !app.thumbnails.inflight.contains(&page) {
                app.thumbnails.inflight.insert(page);
                app.request_page_texture(ui.ctx(), page, THUMB_WIDTH);
            }
        }
    }

    let accent = app.colors.bookmark_selection.stroke();
    let stroke = if is_current {
        egui::Stroke::new(2.0_f32, accent)
    } else if response.hovered() {
        egui::Stroke::new(1.0_f32, accent.gamma_multiply(0.5))
    } else {
        egui::Stroke::new(1.0_f32, ui.visuals().widgets.noninteractive.bg_stroke.color)
    };
    ui.painter().rect_stroke(image_rect, 1.0, stroke);

    let label_pos = egui::pos2(rect.center().x, image_rect.bottom() + LABEL_HEIGHT / 2.0);
    ui.painter().text(
        label_pos,
        egui::Align2::CENTER_CENTER,
        page.to_string(),
        egui::FontId::proportional(12.0),
        if is_current { accent } else { ui.visuals().text_color() },
    );

    response.clicked().then_some(page)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 캐시가 상한을 넘으면 지금 보는 자리에서 먼 쪽부터 버린다 — 가까운 쪽은 남아야 스크롤이
    /// 매끄럽다.
    #[test]
    fn trimming_keeps_the_pages_around_the_current_one() {
        let ctx = egui::Context::default();
        let mut thumbs = Thumbnails::default();
        let image = egui::ColorImage::new([1, 1], egui::Color32::WHITE);
        for page in 1..=(CACHE_LIMIT as u32 + 40) {
            let texture = ctx.load_texture(format!("t{page}"), image.clone(), egui::TextureOptions::LINEAR);
            thumbs.insert(page, texture);
        }
        let around = CACHE_LIMIT as u32 / 2;
        thumbs.trim(around);

        assert_eq!(thumbs.cache.len(), CACHE_LIMIT);
        assert!(thumbs.cache.contains_key(&around), "보고 있는 쪽이 남아야 한다");
        assert!(thumbs.cache.contains_key(&(around + 1)));
        assert!(!thumbs.cache.contains_key(&(CACHE_LIMIT as u32 + 40)), "가장 먼 쪽이 버려져야 한다");
    }

    /// 기다리는 중인 쪽은 두 번 맡기지 않는다.
    #[test]
    fn a_page_is_only_requested_once() {
        let mut thumbs = Thumbnails::default();
        assert!(!thumbs.is_waiting(7));
        thumbs.inflight.insert(7);
        assert!(thumbs.is_waiting(7));
        // 응답이 오면 표시가 지워진다.
        thumbs.give_up(7);
        assert!(!thumbs.is_waiting(7));
    }
}
