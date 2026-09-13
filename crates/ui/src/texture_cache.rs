//! 페이지 텍스처 캐시 — 캐시에서 빠진 텍스처를 **다음 프레임 시작 때** 해제한다.
//!
//! egui-wgpu는 한 프레임을 텍스처 올리기 → 그리기 → 해제(`free_texture`) → GPU 제출 순서로
//! 처리한다(egui-wgpu 0.29.1 `winit.rs` `paint_and_update_textures`). 그래서 이번 프레임에
//! 그린 텍스처가 같은 프레임 안에서 마지막 참조를 잃으면, 제출 시점에 wgpu가 "Texture ...
//! has been destroyed"로 패닉한다 — release는 `panic = "abort"`라 앱이 그대로 꺼진다
//! (2026-09-14 실사용 크래시, `panic.log`/macOS 크래시 리포트로 확인). 교체·범위 밖 정리·문서
//! 교체로 빠진 텍스처는 여기 `retired`에 한 프레임 더 붙잡아 두었다가, 그 프레임이 제출된 뒤인
//! 다음 `update` 맨 앞(`begin_frame`)에서 놓아준다.
//!
//! 캐시 변화와 그린 텍스처는 `crate::trace`에 기록된다 — 크래시가 다시 나면 panic.log에서
//! 텍스처 번호로 경로를 추적할 수 있다.

use crate::trace;
use std::collections::HashMap;

#[derive(Default)]
pub struct PageTextureCache {
    /// 페이지 번호 → (텍스처, 요청한 target_width).
    pages: HashMap<u32, (egui::TextureHandle, i32)>,
    /// 쪽 단위 보기에서 마지막으로 그린 텍스처 — 페이지를 넘긴 직후 새 결과가 오기 전 잠깐
    /// 그대로 보여주는 데 쓴다(viewer_panel `PAGE_SWITCH_GRACE_SECS`).
    shown: Option<egui::TextureHandle>,
    /// 이번 프레임에 캐시에서 빠진 텍스처 — 다음 프레임 `begin_frame`에서 해제.
    retired: Vec<egui::TextureHandle>,
    /// 직전 프레임에 그린 텍스처(기록용 — 바뀔 때만 trace에 남긴다).
    painted: Vec<egui::TextureId>,
}

impl PageTextureCache {
    /// 매 프레임 `update` 맨 앞(어떤 그리기보다 먼저)에 호출 — 지난 프레임에 빠진 텍스처를
    /// 이제 놓아준다. 지난 프레임은 이미 GPU에 제출됐으므로 안전하다.
    pub fn begin_frame(&mut self, ctx: &egui::Context) {
        trace::next_frame();
        if self.retired.is_empty() {
            return;
        }
        let ids: Vec<egui::TextureId> = self.retired.iter().map(egui::TextureHandle::id).collect();
        self.retired.clear();
        // 핸들을 놓아도 다른 곳(예: 직전 화면)이 들고 있으면 egui는 아직 해제하지 않는다.
        let tex_manager = ctx.tex_manager();
        let tex_manager = tex_manager.read();
        let states: Vec<String> = ids
            .iter()
            .map(|id| {
                let state = if tex_manager.meta(*id).is_some() { "아직 참조됨" } else { "해제" };
                format!("{id:?}({state})")
            })
            .collect();
        trace::record(format_args!("퇴역 텍스처 반납: {}", states.join(", ")));
    }

    pub fn get(&self, page: u32) -> Option<&(egui::TextureHandle, i32)> {
        self.pages.get(&page)
    }

    /// 캐시된 텍스처를 렌더링할 때 요청한 폭.
    pub fn width(&self, page: u32) -> Option<i32> {
        self.pages.get(&page).map(|(_, width)| *width)
    }

    pub fn insert(&mut self, page: u32, texture: egui::TextureHandle, width: i32) {
        let id = texture.id();
        match self.pages.insert(page, (texture, width)) {
            Some((old, old_width)) => {
                trace::record(format_args!(
                    "캐시 교체 p{page}: {:?}(w{old_width}) → {id:?}(w{width})",
                    old.id()
                ));
                self.retired.push(old);
            }
            None => trace::record(format_args!("캐시 추가 p{page}: {id:?}(w{width})")),
        }
    }

    /// `keep`이 false인 페이지를 캐시에서 뺀다(텍스처 해제는 다음 프레임).
    pub fn retain(&mut self, mut keep: impl FnMut(u32) -> bool) {
        let removed: Vec<u32> = self.pages.keys().copied().filter(|&page| !keep(page)).collect();
        for page in removed {
            if let Some((texture, width)) = self.pages.remove(&page) {
                trace::record(format_args!("캐시 제외 p{page}: {:?}(w{width})", texture.id()));
                self.retired.push(texture);
            }
        }
    }

    pub fn shown(&self) -> Option<&egui::TextureHandle> {
        self.shown.as_ref()
    }

    pub fn set_shown(&mut self, texture: &egui::TextureHandle) {
        if self.shown.as_ref().map(egui::TextureHandle::id) == Some(texture.id()) {
            return;
        }
        if let Some(old) = self.shown.replace(texture.clone()) {
            trace::record(format_args!("직전 화면 교체: {:?} → {:?}", old.id(), texture.id()));
            self.retired.push(old);
        }
    }

    /// 열린 파일이 바깥에서 바뀌어 다시 열었을 때 — 텍스처는 새 렌더가 도착할 때까지 계속
    /// 보여주되(화면이 하얗게 깜빡이지 않게) 모두 다시 렌더링되도록 요청 폭 기록만 무효로 만든다.
    /// 뷰어는 "캐시 폭 ≠ 현재 배율 폭"이면 다시 요청하므로, 실제 폭(≥50)과 겹치지 않는 0을 쓴다.
    pub fn invalidate_all(&mut self) {
        trace::record(format_args!("캐시 무효화(파일 다시 열기): 페이지 {}장", self.pages.len()));
        for (_, width) in self.pages.values_mut() {
            *width = 0;
        }
    }

    /// 문서가 바뀌면 전부 뺀다(텍스처 해제는 다음 프레임).
    pub fn clear(&mut self) {
        trace::record(format_args!(
            "캐시 전체 비움: 페이지 {}장, 직전 화면 {}",
            self.pages.len(),
            u8::from(self.shown.is_some())
        ));
        self.retired.extend(self.pages.drain().map(|(_, (texture, _))| texture));
        self.retired.extend(self.shown.take());
    }

    /// 이번 프레임에 그린 텍스처를 알린다 — 직전 프레임과 달라졌을 때만 기록한다.
    pub fn note_painted(&mut self, ids: &[egui::TextureId]) {
        if self.painted.as_slice() != ids {
            trace::record(format_args!("그림: {ids:?}"));
            self.painted = ids.to_vec();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texture(ctx: &egui::Context) -> egui::TextureHandle {
        ctx.load_texture(
            "test",
            egui::ColorImage::new([1, 1], egui::Color32::WHITE),
            egui::TextureOptions::LINEAR,
        )
    }

    /// egui 텍스처 매니저에 아직 할당돼 있는지 = 해제 요청이 아직 안 나갔는지.
    fn allocated(ctx: &egui::Context, id: egui::TextureId) -> bool {
        ctx.tex_manager().read().meta(id).is_some()
    }

    #[test]
    fn removed_page_texture_lives_until_next_frame() {
        let ctx = egui::Context::default();
        let mut cache = PageTextureCache::default();
        let tex = texture(&ctx);
        let id = tex.id();
        cache.insert(3, tex, 1800);

        cache.retain(|page| page != 3);
        assert!(cache.get(3).is_none());
        assert!(allocated(&ctx, id), "같은 프레임에 해제되면 wgpu 패닉");

        cache.begin_frame(&ctx);
        assert!(!allocated(&ctx, id));
    }

    #[test]
    fn replaced_texture_lives_until_next_frame() {
        let ctx = egui::Context::default();
        let mut cache = PageTextureCache::default();
        let old = texture(&ctx);
        let old_id = old.id();
        cache.insert(1, old, 900);
        cache.insert(1, texture(&ctx), 1800);

        assert_eq!(cache.width(1), Some(1800));
        assert!(allocated(&ctx, old_id));
        cache.begin_frame(&ctx);
        assert!(!allocated(&ctx, old_id));
    }

    #[test]
    fn shown_texture_is_retired_only_when_replaced() {
        let ctx = egui::Context::default();
        let mut cache = PageTextureCache::default();
        let first = texture(&ctx);
        let first_id = first.id();
        cache.insert(1, first.clone(), 900);
        cache.set_shown(&first);
        drop(first);

        // 같은 텍스처를 매 프레임 다시 알려도 퇴역 목록이 쌓이지 않는다.
        cache.set_shown(&cache.get(1).unwrap().0.clone());
        assert!(cache.retired.is_empty());

        // 페이지를 넘겨 캐시에서 빠져도 "직전 화면"으로 계속 살아 있다.
        cache.retain(|_| false);
        cache.begin_frame(&ctx);
        assert!(allocated(&ctx, first_id));
        assert_eq!(cache.shown().map(egui::TextureHandle::id), Some(first_id));

        cache.set_shown(&texture(&ctx));
        assert!(allocated(&ctx, first_id));
        cache.begin_frame(&ctx);
        assert!(!allocated(&ctx, first_id));
    }

    /// 파일 다시 열기: 텍스처는 그대로 보여주면서(해제 안 함) 다시 렌더링 대상이 된다.
    #[test]
    fn invalidate_keeps_textures_but_forces_rerender() {
        let ctx = egui::Context::default();
        let mut cache = PageTextureCache::default();
        let tex = texture(&ctx);
        let id = tex.id();
        cache.insert(2, tex, 1800);
        cache.invalidate_all();
        cache.begin_frame(&ctx);
        assert_eq!(cache.width(2), Some(0));
        assert!(cache.get(2).is_some() && allocated(&ctx, id));
    }

    #[test]
    fn clear_retires_everything_until_next_frame() {
        let ctx = egui::Context::default();
        let mut cache = PageTextureCache::default();
        let page = texture(&ctx);
        let shown = texture(&ctx);
        let (page_id, shown_id) = (page.id(), shown.id());
        cache.insert(5, page, 1000);
        cache.set_shown(&shown);
        drop(shown);

        cache.clear();
        assert!(cache.get(5).is_none() && cache.shown().is_none());
        assert!(allocated(&ctx, page_id) && allocated(&ctx, shown_id));
        cache.begin_frame(&ctx);
        assert!(!allocated(&ctx, page_id) && !allocated(&ctx, shown_id));
    }

    /// 캐시 변화가 텍스처 번호와 함께 동작 기록에 남는다.
    #[test]
    fn cache_changes_are_traced_with_texture_ids() {
        let _guard = trace::TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let ctx = egui::Context::default();
        let mut cache = PageTextureCache::default();
        let tex = texture(&ctx);
        let id = tex.id();
        cache.insert(77, tex, 1234);
        cache.note_painted(&[id]);
        cache.retain(|_| false);
        cache.begin_frame(&ctx);

        let dump = trace::dump();
        assert!(dump.contains(&format!("캐시 추가 p77: {id:?}(w1234)")));
        assert!(dump.contains(&format!("그림: [{id:?}]")));
        assert!(dump.contains(&format!("캐시 제외 p77: {id:?}(w1234)")));
        assert!(dump.contains(&format!("{id:?}(해제)")));
    }
}
