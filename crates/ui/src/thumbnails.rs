//! 사이드바의 썸네일 탭(예약 7) — 쪽 미리보기를 세로로 늘어놓고 눌러서 이동한다.
//!
//! **보이는 것만 렌더링한다.** 수백 쪽 문서에서 전부 미리 그리면 시작이 멈춘다. 지금 화면에 걸리는
//! 줄만 골라 그 쪽을 렌더 워커에 맡기고, 나머지는 자리만 잡아 둔다.
//!
//! **줄 높이는 쪽마다 다르다.** 처음에는 `ScrollArea::show_rows`(높이가 일정해야 한다)로 문서의 중위
//! 비율 하나를 모든 줄에 썼는데, 가로 판형 쪽이 끼면 그 줄만 그림 위아래가 휑하게 남아 앞 쪽과의
//! 간격이 벌어져 보였다(2026-10-01 리포트). 판형과 상관없이 간격을 일정하게 두려면 줄 높이가 그림
//! 높이를 따라가야 하므로, `show_viewport`로 **직접 가상화**한다. 쪽마다의 높이를 미리 재어 누적
//! 합(`Layout::tops`)으로 두고, 화면에 걸리는 범위만 이분 탐색해 그린다.
//!
//! 극단적인 판형은 높이를 막아 둔다. DTFA00006.pdf 2쪽 같은 책등(216 × 3663 pt, 17:1)을 비율대로
//! 그리면 한 줄이 화면을 몇 배나 넘는다. 중위 비율의 `IRREGULAR_CAP`배를 넘지 않게 맞춰 넣으면
//! 책등은 얇고 조금 긴 띠로 보인다.
//!
//! 렌더는 뷰어와 **같은 보조 프로세스**를 쓴다(`app::request_page_texture`). 따로 두지 않는 이유는
//! pdfium이 스레드 안전하지 않아 어차피 프로세스를 나눠야 하고, 이미 있는 것을 쓰면 요청 폐기
//! (epoch)와 실패 처리가 그대로 따라오기 때문이다.
//!
//! 뷰어 텍스처와 **캐시를 나눠 둔다**. `page_textures`는 쪽마다 배율 하나만 들고 있어서, 썸네일을
//! 거기 넣으면 보고 있던 큰 텍스처를 밀어낸다.
//!
//! **렌더 폭은 보여 줄 폭과 화면 배율에서 계단식으로 정한다**(2026-10-01 "많이 흐리다"). 처음에는
//! 208px로 못박아 두고 늘려 그렸는데, Retina에서 1pt는 물리 2px이라 300pt까지 넓히면 세 배로 늘어나
//! 흐려졌다. 그렇다고 필요한 폭을 그대로 쓰면 사이드바를 끌 때마다 다시 그리게 되므로, 몇 단계로
//! 끊어 그 중 가장 작은 충분한 값을 쓴다. 대신 한 장이 무거워지므로 캐시 상한을 장수가 아니라
//! **메모리 예산**으로 둔다 — 폭이 커지면 담아 두는 장수가 저절로 줄어든다.
//!
//! **캐시에서 빠진 텍스처는 한 프레임 뒤에 놓아준다**(2026-09-30 크래시). egui-wgpu는 한 프레임을
//! 올리기 → 그리기 → 해제 → GPU 제출 순으로 처리하므로, 이번 프레임에 그린 텍스처를 같은 프레임
//! 안에서 놓으면 제출 시점에 `Texture ... has been destroyed`로 패닉한다. `texture_cache` 모듈이
//! 같은 이유로 `retired`를 두고 있고, 여기도 같은 방식을 쓴다.
//!
//! **버릴 쪽은 `current_page`가 아니라 지금 보고 있는 줄을 기준으로 고른다.** 처음에는 현재 쪽에서
//! 먼 것부터 버렸는데, 목록만 멀리 스크롤하면 **화면에 보이는 쪽이 곧 가장 먼 쪽**이라 방금 그린
//! 텍스처를 그 프레임에 바로 버렸다. 위의 패닉이 난 경로가 이것이다.

use crate::app::PdfViewerApp;
use std::collections::HashMap;

/// 그림을 보여 줄 수 있는 폭의 범위(pt). 사이드바를 넓히면 위쪽 한계까지 따라 커진다.
const MIN_DISPLAY_WIDTH: f32 = 60.0;
const MAX_DISPLAY_WIDTH: f32 = 300.0;

/// 실제로 그려 둘 수 있는 폭(px)의 단계. 필요한 폭 이상인 것 중 가장 작은 것을 쓴다. 사이드바를
/// 끄는 동안 폭이 조금 달라졌다고 매번 다시 그리지 않게 하려고 계단으로 끊는다.
const RENDER_STEPS: [i32; 5] = [160, 224, 320, 448, 640];

/// 썸네일 텍스처에 내줄 메모리(바이트). 넘으면 지금 보고 있는 줄에서 먼 것부터 버린다.
///
/// 장수가 아니라 크기로 재는 이유: 렌더 폭이 160px일 때와 640px일 때 한 장의 무게가 16배 차이 난다.
/// 같은 장수를 담으면 넓게 쓸 때 메모리가 터지고, 좁게 쓸 때는 쓸데없이 적게 담는다.
const CACHE_BUDGET_BYTES: usize = 48 << 20;

/// 쪽 크기를 알 수 없을 때 쓰는 세로/가로 비율(A4).
const DEFAULT_RATIO: f32 = 1.414;

/// 한 줄의 높이는 문서 중위 판형의 이 배수를 넘지 않는다. 책등처럼 극단적인 쪽이 화면을 독차지하지
/// 않게 막는 장치다.
const IRREGULAR_CAP: f32 = 1.6;

/// 그림 아랫변과 쪽 번호 사이(pt). 판형과 상관없이 늘 이만큼이다.
const LABEL_GAP: f32 = 7.0;
/// 쪽 번호 한 줄의 높이.
const LABEL_HEIGHT: f32 = 16.0;
/// 줄 사이 여백. 줄 높이가 그림 높이를 따라가므로 이 값이 곧 눈에 보이는 간격이다.
const ROW_GAP: f32 = 14.0;

/// 선택 표시 — macOS 미리보기의 썸네일 표시를 따랐다. 테두리를 두르는 것이 아니라 **쪽 번호까지
/// 함께 감싸는 둥근 판**을 깔고 그 위에 종이와 번호를 얹는다. 종이 둘레에 이만큼 색이 보인다.
const SELECT_PAD: f32 = 4.0;
/// 종이와 선택 판의 모서리 둥글기.
const CORNER: f32 = 5.0;
const SELECT_CORNER: f32 = 8.0;

/// 캐시 한 칸 — 텍스처와 그것을 그릴 때 쓴 폭.
struct Thumb {
    texture: egui::TextureHandle,
    width: i32,
}

impl Thumb {
    fn bytes(&self) -> usize {
        let [w, h] = self.texture.size();
        w * h * 4
    }
}

/// 폭 하나에 대해 미리 재어 둔 줄 배치. 쪽마다 그림 높이가 다르므로 누적 합을 들고 다닌다.
struct Layout {
    /// 이 배치를 만든 그림 폭. 폭이 달라지면 다시 잰다.
    width: f32,
    /// 쪽별 그림 크기.
    images: Vec<egui::Vec2>,
    /// 쪽별 줄의 윗변(내용 좌표). 길이는 쪽 수 + 1이고 마지막 값이 전체 높이다.
    tops: Vec<f32>,
}

impl Layout {
    fn build(width: f32, sizes: &[egui::Vec2], pages: u32, median: f32) -> Self {
        let cap = width * median * IRREGULAR_CAP;
        let mut images = Vec::with_capacity(pages as usize);
        let mut tops = Vec::with_capacity(pages as usize + 1);
        let mut y = 0.0;
        for index in 0..pages as usize {
            let size = sizes.get(index).copied().unwrap_or_default();
            let image = fit(egui::vec2(width, cap), page_ratio(size));
            tops.push(y);
            y += SELECT_PAD + image.y + LABEL_GAP + LABEL_HEIGHT + ROW_GAP;
            images.push(image);
        }
        tops.push(y);
        Self { width, images, tops }
    }

    fn pages(&self) -> usize {
        self.images.len()
    }

    fn total(&self) -> f32 {
        self.tops.last().copied().unwrap_or(0.0)
    }

    fn height_of(&self, index: usize) -> f32 {
        self.tops[index + 1] - self.tops[index]
    }

    /// 내용 좌표 `y`에 걸리는 줄 번호(0부터).
    fn index_at(&self, y: f32) -> usize {
        let last = self.pages().saturating_sub(1);
        match self.tops.binary_search_by(|top| top.total_cmp(&y)) {
            Ok(index) => index.min(last),
            Err(index) => index.saturating_sub(1).min(last),
        }
    }
}

#[derive(Default)]
pub struct Thumbnails {
    cache: HashMap<u32, Thumb>,
    /// 렌더를 맡겨 두고 기다리는 쪽 → 맡긴 폭. 워커 응답이 썸네일 것인지 가리는 데도 쓴다.
    inflight: HashMap<u32, i32>,
    /// 줄 높이를 정하는 문서 대표 비율. 문서마다 한 번만 재고 `clear`로 버린다.
    row_ratio: Option<f32>,
    /// 지금 폭에 맞춰 재어 둔 줄 배치.
    layout: Option<Layout>,
    /// 이번 프레임에 캐시에서 빠진 텍스처 — 다음 프레임 `begin_frame`에서 놓아준다.
    retired: Vec<egui::TextureHandle>,
    /// 직전 프레임의 스크롤 위치와, 그때 화면에 들였던 쪽.
    last_offset: f32,
    last_page: Option<u32>,
}

impl Thumbnails {
    /// 매 프레임 `update` 맨 앞(어떤 그리기보다 먼저)에 호출 — 지난 프레임에 빠진 텍스처를 이제
    /// 놓아준다. 지난 프레임은 이미 GPU에 제출됐으므로 안전하다.
    pub fn begin_frame(&mut self) {
        if self.retired.is_empty() {
            return;
        }
        let ids: Vec<egui::TextureId> = self.retired.iter().map(egui::TextureHandle::id).collect();
        self.retired.clear();
        crate::trace::record(format_args!("썸네일 퇴역 반납: {ids:?}"));
    }

    /// 문서가 바뀌면 전부 버린다. 그리던 중일 수 있으므로 텍스처는 곧장 놓지 않는다.
    pub fn clear(&mut self) {
        self.retired.extend(self.cache.drain().map(|(_, thumb)| thumb.texture));
        self.inflight.clear();
        self.row_ratio = None;
        self.layout = None;
        self.last_offset = 0.0;
        self.last_page = None;
    }

    /// 이 쪽을 이 폭으로 맡겨 두고 기다리는 중인가. 워커 응답을 썸네일로 받을지 가리는 데 쓴다.
    pub fn is_waiting(&self, page: u32, width: i32) -> bool {
        self.inflight.get(&page) == Some(&width)
    }

    pub fn insert(&mut self, page: u32, texture: egui::TextureHandle, width: i32) {
        self.inflight.remove(&page);
        // 같은 쪽을 다시 그린 경우, 밀려난 것도 곧장 놓지 않는다(`trim` 주석과 같은 이유).
        if let Some(old) = self.cache.insert(page, Thumb { texture, width }) {
            self.retired.push(old.texture);
        }
    }

    /// 렌더가 실패했거나 버려졌을 때 — 다음에 다시 맡길 수 있게 표시만 지운다.
    pub fn give_up(&mut self, page: u32, width: i32) {
        if self.inflight.get(&page) == Some(&width) {
            self.inflight.remove(&page);
        }
    }

    /// 줄 높이를 정하는 대표 비율. **평균이 아니라 중위값**을 쓴다 — 책등이나 접지처럼 판형이 크게
    /// 다른 쪽이 한둘 끼어도 평균은 끌려가지만 중위값은 흔들리지 않는다.
    fn row_ratio(&mut self, sizes: &[egui::Vec2]) -> f32 {
        *self.row_ratio.get_or_insert_with(|| median_ratio(sizes))
    }

    /// 폭이 달라졌으면 줄 배치를 다시 잰다.
    ///
    /// 다시 쟀으면 `last_page`를 지운다. 같은 스크롤 위치가 **다른 쪽을 가리키게** 되므로, 보고 있던
    /// 쪽이 화면 밖으로 밀려난다(2026-10-01 "사이드바 폭을 조정하면 사라져"). 지워 두면 바로 다음
    /// `scroll_to_show`가 그 쪽을 다시 화면에 들인다.
    fn ensure_layout(&mut self, width: f32, sizes: &[egui::Vec2], pages: u32) {
        let median = self.row_ratio(sizes);
        let stale = match &self.layout {
            Some(layout) => (layout.width - width).abs() > 0.5 || layout.pages() != pages as usize,
            None => true,
        };
        if stale {
            self.layout = Some(Layout::build(width, sizes, pages, median));
            self.last_page = None;
        }
    }

    /// 담아 둔 텍스처가 쓰는 메모리.
    fn bytes(&self) -> usize {
        self.cache.values().map(Thumb::bytes).sum()
    }

    /// 예산을 넘으면 **지금 보고 있는 줄**에서 먼 쪽부터 버린다. `around`에 현재 쪽을 넣으면 안
    /// 된다 — 목록만 멀리 스크롤했을 때 방금 그린 것부터 버리게 된다(모듈 문서).
    ///
    /// 뺀 텍스처는 `retired`에 한 프레임 붙잡아 둔다. 여기서 바로 놓으면 이번 프레임에 그린
    /// 텍스처가 GPU 제출 전에 사라져 wgpu가 패닉한다.
    fn trim(&mut self, around: u32) {
        let mut total = self.bytes();
        if total <= CACHE_BUDGET_BYTES {
            return;
        }
        let mut pages: Vec<u32> = self.cache.keys().copied().collect();
        // 먼 것부터 본다.
        pages.sort_by_key(|page| std::cmp::Reverse(page.abs_diff(around)));
        let mut dropped = 0usize;
        for page in pages {
            if total <= CACHE_BUDGET_BYTES {
                break;
            }
            if let Some(thumb) = self.cache.remove(&page) {
                total -= thumb.bytes();
                self.retired.push(thumb.texture);
                dropped += 1;
            }
        }
        crate::trace::record(format_args!(
            "썸네일 정리(기준 p{around}): {dropped}장 퇴역, 남은 {} KiB",
            total / 1024
        ));
    }

    /// 보고 있는 쪽이 바뀌었으면 그 줄이 화면에 들어오도록 새 스크롤 위치를 돌려준다. 이미 다
    /// 보이면 `None`이고, 벗어났으면 **세로 가운데**로 끌어온다(북마크 사이드바와 같은 방식).
    fn scroll_to_show(&mut self, page: u32, viewport: f32) -> Option<f32> {
        let layout = self.layout.as_ref()?;
        if self.last_page == Some(page) || layout.pages() == 0 {
            return None;
        }
        self.last_page = Some(page);
        let index = (page as usize).saturating_sub(1).min(layout.pages() - 1);
        let (top, height) = (layout.tops[index], layout.height_of(index));
        if top >= self.last_offset && top + height <= self.last_offset + viewport {
            return None;
        }
        let wanted = top + height / 2.0 - viewport / 2.0;
        Some(wanted.clamp(0.0, (layout.total() - viewport).max(0.0)))
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

/// 이 폭(pt)으로 이 화면 배율에 보여 주려면 몇 px로 그려 두어야 하는가.
fn render_width_for(display_width: f32, pixels_per_point: f32) -> i32 {
    let needed = (display_width * pixels_per_point).ceil() as i32;
    let last = RENDER_STEPS[RENDER_STEPS.len() - 1];
    RENDER_STEPS.into_iter().find(|step| *step >= needed).unwrap_or(last)
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

    let width = (ui.available_width() - 16.0).clamp(MIN_DISPLAY_WIDTH, MAX_DISPLAY_WIDTH);
    let render_width = render_width_for(width, ui.ctx().pixels_per_point());
    app.thumbnails.ensure_layout(width, &app.page_sizes, pages);
    let scroll_to = app.thumbnails.scroll_to_show(app.current_page, ui.available_height());

    let mut go_to: Option<u32> = None;
    // 캐시 정리의 기준점. 현재 쪽이 아니라 지금 화면에 보이는 줄의 한가운데다.
    let mut visible_center = app.current_page;

    let mut area = egui::ScrollArea::vertical().auto_shrink([false, false]);
    if let Some(offset) = scroll_to {
        area = area.vertical_scroll_offset(offset);
    }
    let output = area.show_viewport(ui, |ui, viewport| {
        // 전체 높이를 먼저 잡아 두어야 스크롤 막대 길이가 맞는다. 줄은 그 안 절대 좌표에 그린다.
        let total = app.thumbnails.layout.as_ref().map_or(0.0, Layout::total);
        let (content, _) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), total), egui::Sense::hover());

        // 그릴 줄의 자리를 먼저 뽑아 캐시 빌림을 끝낸다 — 아래에서 `app`을 다시 빌려야 한다.
        let plan: Vec<(usize, f32, egui::Vec2)> = match app.thumbnails.layout.as_ref() {
            Some(layout) if layout.pages() > 0 => {
                let first = layout.index_at(viewport.min.y);
                let last = layout.index_at(viewport.max.y);
                visible_center = (first + last) as u32 / 2 + 1;
                (first..=last).map(|index| (index, layout.tops[index], layout.images[index])).collect()
            }
            _ => Vec::new(),
        };
        for (index, top, image) in plan {
            if let Some(target) = row(ui, app, index, content, top, image, render_width) {
                go_to = Some(target);
            }
        }
    });
    app.thumbnails.last_offset = output.state.offset.y;

    if let Some(page) = go_to {
        app.focus_area = crate::app::FocusArea::Sidebar;
        app.go_to_page(page);
        // 클릭한 줄은 이미 보이므로 스크롤을 건드리지 않는다.
        app.thumbnails.last_page = Some(app.current_page);
    }
    app.thumbnails.trim(visible_center.clamp(1, pages));
}

/// 줄 하나. 눌렸으면 그 쪽 번호를 돌려준다. 줄 높이가 그림 높이를 따라가므로 그림은 줄 맨 위에
/// 놓고 쪽 번호를 그 바로 밑에 찍으면 된다 — 판형이 섞여도 간격이 저절로 일정하다.
fn row(
    ui: &mut egui::Ui,
    app: &mut PdfViewerApp,
    index: usize,
    content: egui::Rect,
    top: f32,
    image: egui::Vec2,
    render_width: i32,
) -> Option<u32> {
    let page = index as u32 + 1;
    let rect = egui::Rect::from_min_size(
        egui::pos2(content.left(), content.top() + top),
        egui::vec2(content.width(), SELECT_PAD + image.y + LABEL_GAP + LABEL_HEIGHT + ROW_GAP),
    );
    if !ui.is_rect_visible(rect) {
        return None;
    }
    let response = ui.interact(rect, ui.id().with(("thumbnail", index)), egui::Sense::click());
    // 줄 맨 위 `SELECT_PAD`는 선택 판이 종이 위로 나오는 몫이다 — 비워 두지 않으면 앞 줄을 덮는다.
    let image_rect =
        egui::Rect::from_min_size(egui::pos2(rect.center().x - image.x / 2.0, rect.top() + SELECT_PAD), image);

    // 텍스처 상태만 꺼내 두고 캐시 빌림을 끝낸다.
    let cached: Option<(egui::TextureId, i32)> =
        app.thumbnails.cache.get(&page).map(|thumb| (thumb.texture.id(), thumb.width));

    let accent = app.colors.thumbnail_selection.stroke();
    let is_current = page == app.current_page;
    // 선택 판은 종이와 쪽 번호를 **함께** 감싼다. 종이 위아래옆으로 `SELECT_PAD`만큼 색이 보이고,
    // 번호 자리는 판 안쪽이라 번호가 그 색 위에 얹힌다.
    let plate = egui::Rect::from_min_max(
        egui::pos2(image_rect.left() - SELECT_PAD, image_rect.top() - SELECT_PAD),
        egui::pos2(image_rect.right() + SELECT_PAD, image_rect.bottom() + LABEL_GAP + LABEL_HEIGHT),
    );
    if is_current {
        ui.painter().rect_filled(plate, SELECT_CORNER, accent);
    } else if response.hovered() {
        // 고르면 어떻게 되는지 미리 보여 준다.
        ui.painter().rect_filled(plate, SELECT_CORNER, accent.gamma_multiply(0.18));
    } else {
        // 흰 종이가 배경에서 떠 보이게(미리보기 앱과 같은 인상). 선택 판 위에서는 탁해 보여서 뺀다.
        ui.painter()
            .rect_filled(image_rect.translate(egui::vec2(0.0, 1.5)), CORNER, egui::Color32::from_black_alpha(30));
    }

    match cached {
        Some((id, _)) => {
            ui.painter().image(
                id,
                image_rect,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        }
        None => {
            // 아직 없으면 흰 자리만 잡아 둔다. 자리를 잡아 두어야 스크롤 막대 길이가 흔들리지 않는다.
            ui.painter().rect_filled(image_rect, CORNER, egui::Color32::from_gray(245));
        }
    }

    // 없거나, 있어도 지금 폭에 모자라면 (다시) 맡긴다. 그동안 옛것을 늘려 보여 준다.
    let needs_render = cached.is_none_or(|(_, have)| have < render_width);
    if needs_render && !app.thumbnails.is_waiting(page, render_width) {
        app.thumbnails.inflight.insert(page, render_width);
        app.request_page_texture(ui.ctx(), page, render_width);
    }

    // 종이 가장자리. 선택 판 위에서는 판 색이 이미 테두리 노릇을 한다.
    if !is_current {
        ui.painter()
            .rect_stroke(image_rect, CORNER, egui::Stroke::new(1.0_f32, egui::Color32::from_black_alpha(40)));
    }

    // 쪽 번호는 그림 아랫변에서 늘 같은 거리에 찍는다. 고른 쪽은 선택 판 위에 얹히므로 흰 글자다.
    let label_pos = egui::pos2(rect.center().x, image_rect.bottom() + LABEL_GAP + LABEL_HEIGHT / 2.0);
    ui.painter().text(
        label_pos,
        egui::Align2::CENTER_CENTER,
        page.to_string(),
        egui::FontId::proportional(12.0),
        if is_current { app.colors.thumbnail_selection.on_stroke() } else { ui.visuals().text_color() },
    );

    response.clicked().then_some(page)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 쪽 하나에 텍스처를 채워 넣은 캐시.
    fn filled(ctx: &egui::Context, pages: u32, side: usize) -> Thumbnails {
        let mut thumbs = Thumbnails::default();
        let image = egui::ColorImage::new([side, side], egui::Color32::WHITE);
        for page in 1..=pages {
            let texture = ctx.load_texture(format!("t{page}"), image.clone(), egui::TextureOptions::LINEAR);
            thumbs.insert(page, texture, side as i32);
        }
        thumbs
    }

    /// 예산을 넘으면 지금 보는 자리에서 먼 쪽부터 버린다 — 가까운 쪽은 남아야 스크롤이 매끄럽다.
    #[test]
    fn trimming_keeps_the_pages_around_the_viewport() {
        let ctx = egui::Context::default();
        // 한 장 1MiB(512×512×4)로 60장 = 60MiB → 예산 48MiB를 넘긴다.
        let mut thumbs = filled(&ctx, 60, 512);
        assert!(thumbs.bytes() > CACHE_BUDGET_BYTES);
        thumbs.trim(30);

        assert!(thumbs.bytes() <= CACHE_BUDGET_BYTES, "예산 안으로 줄어야 한다");
        assert!(thumbs.cache.contains_key(&30), "보고 있는 쪽이 남아야 한다");
        assert!(!thumbs.cache.contains_key(&1), "가장 먼 쪽이 버려져야 한다");
    }

    /// 목록만 멀리 스크롤했을 때, **보고 있는 줄** 둘레가 남아야 한다. 현재 쪽을 기준으로 삼았을
    /// 때는 화면에 보이는 쪽이 곧 가장 먼 쪽이라 방금 그린 것부터 버려졌다(2026-09-30 크래시).
    #[test]
    fn trimming_keeps_what_is_on_screen_even_when_it_is_far_from_the_current_page() {
        let ctx = egui::Context::default();
        // 현재 쪽은 1쪽인데 목록은 맨 끝(58쪽 언저리)을 보고 있는 상황.
        let mut thumbs = filled(&ctx, 60, 512);
        thumbs.trim(58);

        for page in 56..=60 {
            assert!(thumbs.cache.contains_key(&page), "화면에 보이는 p{page}가 버려졌다");
        }
        assert!(!thumbs.cache.contains_key(&1), "멀리 있는 1쪽이 버려져야 한다");
    }

    /// 버린 텍스처는 그 프레임에 놓지 않고 다음 프레임까지 붙잡아 둔다 — 같은 프레임에 그리고
    /// 놓으면 wgpu가 제출 때 패닉한다(모듈 문서, 2026-09-30 크래시).
    #[test]
    fn dropped_textures_are_held_for_one_more_frame() {
        let ctx = egui::Context::default();
        let mut thumbs = filled(&ctx, 60, 512);
        thumbs.trim(30);
        assert!(!thumbs.retired.is_empty(), "뺀 텍스처를 붙잡아 두어야 한다");

        let held = thumbs.cache.len();
        thumbs.begin_frame();
        assert!(thumbs.retired.is_empty(), "다음 프레임에 놓아주어야 한다");

        // 문서를 닫을 때도 마찬가지다.
        thumbs.clear();
        assert!(thumbs.cache.is_empty());
        assert_eq!(thumbs.retired.len(), held, "비울 때도 곧장 놓으면 안 된다");
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

    /// 판형이 섞여도 썸네일 사이 간격이 같아야 한다(2026-10-01 리포트). 모든 줄에 같은 높이를
    /// 주면 가로 판형 쪽만 위아래가 휑하게 남아 앞 쪽과 벌어져 보인다. 줄 높이가 그림 높이를
    /// 따라가면 그림 아랫변에서 다음 그림 윗변까지가 늘 같아진다.
    #[test]
    fn the_gap_between_thumbnails_is_the_same_whatever_the_page_shape() {
        // 세로, 가로, 세로, 책등, 세로가 섞인 문서.
        let sizes = vec![
            egui::vec2(2480.0, 3520.0),
            egui::vec2(3520.0, 2480.0),
            egui::vec2(2480.0, 3520.0),
            egui::vec2(216.0, 3663.0),
            egui::vec2(2480.0, 3520.0),
        ];
        let median = median_ratio(&sizes);
        let layout = Layout::build(200.0, &sizes, sizes.len() as u32, median);

        let expected = SELECT_PAD + LABEL_GAP + LABEL_HEIGHT + ROW_GAP;
        for index in 0..sizes.len() {
            let gap = layout.height_of(index) - layout.images[index].y;
            assert!((gap - expected).abs() < 1e-3, "{index}번째 줄의 간격이 {gap}으로 어긋난다");
        }
        // 가로 판형 쪽은 실제로 낮아야 한다 — 세로와 같은 높이를 잡으면 앞 쪽과 벌어져 보인다.
        assert!(layout.height_of(1) < layout.height_of(0), "가로 판형이 세로와 같은 높이를 차지한다");
        // 책등은 막아 둔 높이까지만 차지한다.
        assert!(layout.images[3].y <= 200.0 * median * IRREGULAR_CAP + 1e-3, "책등이 화면을 독차지한다");
        assert!(layout.images[3].x < layout.images[0].x, "책등은 좁게 그려야 한다");
    }

    /// 화면 배율과 보여 줄 폭에 맞춰 렌더 폭을 계단으로 고른다.
    #[test]
    fn render_width_follows_the_display_size_in_steps() {
        // 1배 화면에서 좁게 쓰면 가장 작은 단계.
        assert_eq!(render_width_for(150.0, 1.0), 160);
        // Retina에서 기본 폭(224pt)이면 448px이 필요하다.
        assert_eq!(render_width_for(224.0, 2.0), 448);
        // 최대 폭에서도 마지막 단계를 넘지 않는다.
        assert_eq!(render_width_for(MAX_DISPLAY_WIDTH, 2.0), 640);
        assert_eq!(render_width_for(MAX_DISPLAY_WIDTH, 4.0), 640);
        // 폭이 조금 달라져도 같은 단계면 다시 그리지 않는다.
        assert_eq!(render_width_for(200.0, 2.0), render_width_for(224.0, 2.0));
    }

    /// 화면 밖으로 나간 쪽만, 그것도 세로 가운데로 끌어온다(북마크 사이드바와 같은 방식,
    /// 2026-10-01 요청). 사이드바 폭이 바뀌어 배치가 달라져도 다시 화면에 들여야 한다.
    #[test]
    fn scrolling_centres_a_page_that_went_out_of_view() {
        let sizes = vec![egui::vec2(100.0, 141.4); 50];
        let mut thumbs = Thumbnails::default();
        thumbs.ensure_layout(100.0, &sizes, 50);
        let row = thumbs.layout.as_ref().unwrap().height_of(0);
        let viewport = row * 4.0;
        let centre = |page: u32| row * (page - 1) as f32 + row / 2.0 - viewport / 2.0;
        // 줄 높이를 누적한 값과 곱으로 구한 값은 부동소수점 끝자리가 다를 수 있다.
        let near = |got: Option<f32>, want: f32| {
            let got = got.expect("스크롤해야 하는데 움직이지 않았다");
            assert!((got - want).abs() < 0.01, "{got} != {want}");
        };

        // 처음 열 때는 보고 있는 쪽을 가운데로 끌어온다.
        near(thumbs.scroll_to_show(20, viewport), centre(20));
        thumbs.last_offset = centre(20);

        // 같은 쪽을 다시 물어도, 화면 안에 다 보이는 이웃 쪽도 움직이지 않는다.
        assert_eq!(thumbs.scroll_to_show(20, viewport), None);
        assert_eq!(thumbs.scroll_to_show(21, viewport), None);
        // 벗어나면 가운데로.
        near(thumbs.scroll_to_show(40, viewport), centre(40));

        // 폭이 바뀌면 같은 스크롤 위치가 다른 쪽을 가리킨다 — 다시 맞출 수 있게 표시를 지운다.
        thumbs.ensure_layout(200.0, &sizes, 50);
        assert!(thumbs.last_page.is_none(), "폭이 바뀌면 보고 있던 쪽을 다시 화면에 들여야 한다");
    }
}
