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
//!
//! **줄 높이는 문서의 중위 비율로 한 번만 정하고, 그림은 쪽마다 제 비율로 그 안에 맞춰 넣는다**
//! (2026-09-30 리포트). 처음에는 *현재 쪽*의 비율로 모든 줄의 높이를 잡았는데, DTFA00006.pdf
//! 2쪽처럼 책등(216 × 3663 pt, 17:1)이 섞여 있으면 그 쪽을 고른 순간 48줄 전부가 2400px로 늘어나
//! 목록이 무너졌다. `ScrollArea::show_rows`는 줄 높이가 일정해야 쓸 수 있으므로, 높이는 중위
//! 비율로 못박고, 판형이 다른 쪽은 그 칸 안에 늘이지 않고 맞춰 넣는다 — 책등은 얇은 띠로 보인다.

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

/// 쪽 크기를 알 수 없을 때 쓰는 세로/가로 비율(A4).
const DEFAULT_RATIO: f32 = 1.414;

#[derive(Default)]
pub struct Thumbnails {
    cache: HashMap<u32, egui::TextureHandle>,
    /// 렌더를 맡겨 두고 기다리는 쪽 — 응답이 썸네일 것인지 가리는 데도 쓴다.
    inflight: HashSet<u32>,
    /// 줄 높이를 정하는 문서 대표 비율. 문서마다 한 번만 재고 `clear`로 버린다.
    row_ratio: Option<f32>,
}

impl Thumbnails {
    /// 문서가 바뀌면 전부 버린다.
    pub fn clear(&mut self) {
        self.cache.clear();
        self.inflight.clear();
        self.row_ratio = None;
    }

    /// 줄 높이를 정하는 대표 비율. **평균이 아니라 중위값**을 쓴다 — 책등이나 접지처럼 판형이 크게
    /// 다른 쪽이 한둘 끼어도 평균은 끌려가지만 중위값은 흔들리지 않는다.
    fn row_ratio(&mut self, sizes: &[egui::Vec2]) -> f32 {
        *self.row_ratio.get_or_insert_with(|| median_ratio(sizes))
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

/// 쪽들의 세로/가로 비율 중위값.
fn median_ratio(sizes: &[egui::Vec2]) -> f32 {
    let mut ratios: Vec<f32> =
        sizes.iter().filter(|size| size.x > 0.0 && size.y > 0.0).map(|size| size.y / size.x).collect();
    if ratios.is_empty() {
        return DEFAULT_RATIO;
    }
    ratios.sort_by(|a, b| a.total_cmp(b));
    ratios[ratios.len() / 2]
}

/// 한 쪽의 세로/가로 비율.
fn page_ratio(size: egui::Vec2) -> f32 {
    if size.x > 0.0 && size.y > 0.0 { size.y / size.x } else { DEFAULT_RATIO }
}

/// `box_size` 안에 `ratio` 비율을 **늘이지 않고** 맞춰 넣은 크기.
fn fit(box_size: egui::Vec2, ratio: f32) -> egui::Vec2 {
    if box_size.x * ratio <= box_size.y {
        egui::vec2(box_size.x, box_size.x * ratio)
    } else {
        egui::vec2(box_size.y / ratio, box_size.y)
    }
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
    let box_height = width * app.thumbnails.row_ratio(&app.page_sizes);
    let height = box_height + LABEL_HEIGHT + ROW_GAP;
    let mut go_to: Option<u32> = None;

    egui::ScrollArea::vertical().auto_shrink([false, false]).show_rows(
        ui,
        height,
        pages as usize,
        |ui, rows| {
            for index in rows {
                let page = index as u32 + 1;
                if let Some(target) = row(ui, app, page, width, box_height) {
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

/// 줄 하나. 눌렸으면 그 쪽 번호를 돌려준다. `box_height`는 그림이 들어갈 칸의 높이이고, 줄 전체
/// 높이는 거기에 쪽 번호 자리와 여백을 더한 것이다.
fn row(ui: &mut egui::Ui, app: &mut PdfViewerApp, page: u32, width: f32, box_height: f32) -> Option<u32> {
    let row_height = box_height + LABEL_HEIGHT + ROW_GAP;
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), row_height), egui::Sense::click());
    if !ui.is_rect_visible(rect) {
        return None;
    }
    // 칸은 모든 줄이 같고, 그림은 그 안에 제 비율로 맞춰 가운데 놓는다.
    let box_rect =
        egui::Rect::from_min_size(egui::pos2(rect.center().x - width / 2.0, rect.top()), egui::vec2(width, box_height));
    let fitted = fit(box_rect.size(), page_ratio(app.page_size_pt(page)));
    let image_rect = egui::Rect::from_center_size(box_rect.center(), fitted);

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

    // 쪽 번호는 그림 아래가 아니라 칸 아래에 붙인다 — 판형이 달라도 줄마다 같은 높이에 오도록.
    let label_pos = egui::pos2(rect.center().x, box_rect.bottom() + LABEL_HEIGHT / 2.0);
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

    /// 책등처럼 판형이 크게 다른 쪽이 섞여도 줄 높이가 흔들리지 않아야 한다 — DTFA00006.pdf
    /// 2쪽(216 × 3663 pt)을 고르자 목록 전체가 무너졌던 일(2026-09-30)의 회귀 시험.
    #[test]
    fn a_book_spine_page_does_not_change_the_row_height() {
        let a4 = egui::vec2(2480.0, 3520.0);
        let spine = egui::vec2(216.0, 3663.0);
        let mut sizes = vec![a4; 47];
        sizes.insert(1, spine);

        let ratio = median_ratio(&sizes);
        assert!((ratio - a4.y / a4.x).abs() < 1e-6, "중위 비율이 본문 판형이어야 한다: {ratio}");
        // 평균을 썼다면 책등 하나에 끌려갔을 것이다.
        let mean: f32 = sizes.iter().map(|size| size.y / size.x).sum::<f32>() / sizes.len() as f32;
        assert!(mean > ratio * 1.2, "평균은 실제로 끌려간다({mean} vs {ratio})");
    }

    /// 칸보다 세로로 긴 쪽은 늘이지 않고 폭을 줄여 넣는다 — 책등이 칸을 뚫고 나오면 안 된다.
    #[test]
    fn a_tall_page_is_fitted_inside_the_box_instead_of_stretched() {
        let box_size = egui::vec2(144.0, 204.0);
        let fitted = fit(box_size, 3663.0 / 216.0);
        assert!(fitted.y <= box_size.y + 1e-3, "칸 높이를 넘었다: {fitted:?}");
        assert!(fitted.x > 0.0 && fitted.x < box_size.x, "폭이 줄어야 한다: {fitted:?}");

        // 칸 비율과 같은 쪽은 칸을 꽉 채운다.
        let snug = fit(box_size, box_size.y / box_size.x);
        assert!((snug.x - box_size.x).abs() < 1e-3 && (snug.y - box_size.y).abs() < 1e-3, "{snug:?}");
    }

    /// 쪽 크기를 모르면 A4 비율로 버틴다(문서를 아직 재지 못한 첫 프레임).
    #[test]
    fn unknown_page_sizes_fall_back_to_a4() {
        assert_eq!(median_ratio(&[]), DEFAULT_RATIO);
        assert_eq!(page_ratio(egui::vec2(0.0, 100.0)), DEFAULT_RATIO);
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
