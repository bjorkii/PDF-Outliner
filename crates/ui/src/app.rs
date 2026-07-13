use bookmark::BookmarkNode;
use egui::Key;
use pdf_engine::PdfEngine;
use pdfium_render::prelude::*;
use std::collections::VecDeque;
use std::path::PathBuf;
use uuid::Uuid;

/// 뷰포트(스크롤+줌) 상태. 확대 시 drag 탐색을 위해 오프셋을 별도로 관리한다.
#[derive(Debug, Clone, Copy)]
pub struct ViewportState {
    pub zoom: f32,
    pub pan_offset: egui::Vec2,
}

impl Default for ViewportState {
    fn default() -> Self {
        Self {
            zoom: 1.0,
            pan_offset: egui::Vec2::ZERO,
        }
    }
}

impl ViewportState {
    pub const MIN_ZOOM: f32 = 0.25;
    pub const MAX_ZOOM: f32 = 8.0;

    pub fn zoom_by(&mut self, factor: f32) {
        self.zoom = (self.zoom * factor).clamp(Self::MIN_ZOOM, Self::MAX_ZOOM);
    }

    /// 페이지 경계를 벗어나지 않도록 pan 오프셋을 clamp한다.
    /// page_size/viewport_size는 현재 줌이 적용된 화면 픽셀 기준.
    pub fn clamp_pan(&mut self, page_size: egui::Vec2, viewport_size: egui::Vec2) {
        let max_x = ((page_size.x - viewport_size.x) / 2.0).max(0.0);
        let max_y = ((page_size.y - viewport_size.y) / 2.0).max(0.0);
        self.pan_offset.x = self.pan_offset.x.clamp(-max_x, max_x);
        self.pan_offset.y = self.pan_offset.y.clamp(-max_y, max_y);
    }
}

pub struct PdfViewerApp {
    pub current_file: Option<PathBuf>,
    pub current_page: u32, // 1-based
    pub total_pages: u32,
    pub page_number_input: String,

    pub viewport: ViewportState,

    /// 사이드바 북마크 트리 (문서별). 드래그 재구성은 bookmark::move_node로 처리.
    pub bookmarks: Vec<BookmarkNode>,
    /// 마지막 로드/저장 이후 북마크가 바뀌었는지. 다른 문서를 열 때 이 값을 보고
    /// "저장하시겠습니까?" 확인창을 띄울지 결정한다.
    pub bookmarks_dirty: bool,
    /// 사이드바에서 현재 선택된 북마크. 클릭 시 뷰어 페이지 이동 + 이 값 설정, "+"는 이
    /// 노드의 자식으로 추가, "-"는 이 노드를 삭제하는 데 쓰인다.
    pub selected_bookmark: Option<Uuid>,
    /// 북마크 변경 실행취소(Cmd+Z) 스택 — 변경 직전 스냅샷을 최대 20개까지 보관.
    pub bookmark_undo_stack: VecDeque<Vec<BookmarkNode>>,
    /// 다시 실행(Cmd+Shift+Z) 스택. undo할 때 채워지고, 새 편집이 생기면 비워진다
    /// (표준 undo/redo 관례 — redo 이후 다른 편집을 하면 그 redo 기록은 의미가 없어짐).
    pub bookmark_redo_stack: VecDeque<Vec<BookmarkNode>>,
    /// Cmd+B 전역 단축키가 세워두는 요청 플래그. 실제 처리(및 편집 포커스 이동)는
    /// sidebar.rs가 담당하는 DragState가 필요해서 여기서는 플래그만 세운다.
    pub request_add_bookmark: bool,

    /// 웹브라우저 뒤로/앞으로가기(Cmd+[/Cmd+])처럼 페이지 이동 히스토리를 순회하기 위한
    /// 스택. `go_to_page`로 페이지가 바뀔 때마다(북마크 클릭, 링크 클릭, 방향키, 페이지
    /// 번호 입력 등 경로 무관) 현재 페이지가 back 스택에 쌓이고 forward 스택은 비워진다.
    /// 문서를 새로 열면 둘 다 초기화한다(다른 문서의 페이지 번호는 의미가 없으므로).
    pub page_back_history: Vec<u32>,
    pub page_forward_history: Vec<u32>,

    /// 텍스트 선택 상태: 좌표가 아니라 인덱스로 관리 (pdf_engine::selection 설계 참고)
    pub selection: Option<pdf_engine::selection::TextSelectionRange>,
    pub selection_drag_start_index: Option<i32>,

    /// pdfium 바인딩. 라이브러리 로드에 실패하면 None으로 두고 뷰어는 안내 메시지만 표시.
    pub engine: Option<PdfEngine>,
    /// PdfEngine이 Pdfium을 `'static`으로 리크해 보관하므로 (crates/pdf_engine/src/lib.rs 참고)
    /// PdfDocument도 자기참조 구조체 없이 여기 필드로 직접 저장할 수 있다.
    pub document: Option<PdfDocument<'static>>,
    pub page_texture: Option<egui::TextureHandle>,
    /// (렌더링된 페이지, 렌더링에 사용한 target width) — 동일하면 재렌더링 생략.
    pub rendered_for: Option<(u32, i32)>,
    /// 마지막 렌더링에 사용한 target width. 클릭 좌표→PDF 포인트 변환 시 동일한
    /// PdfRenderConfig를 재구성하기 위해 필요(PdfRenderConfig는 Clone을 구현하지 않음).
    pub render_target_width: Option<i32>,
    /// 이번 프레임에 페이지 이미지가 그려진 화면 좌표(rect). 클릭 좌표 변환에 사용.
    pub image_rect: Option<egui::Rect>,

    /// 다른 문서를 열려고 했는데 현재 북마크에 저장 안 된 변경사항이 있어 확인을 기다리는 중.
    pub pending_open_path: Option<PathBuf>,

    /// 이전 실행 종료 시점에 열려있던 파일 경로(eframe Storage에서 복원). `main.rs`가
    /// 시작 시 CLI 인자가 없으면 이 값으로 자동으로 연 뒤 소비(take)한다.
    pub last_opened_file: Option<PathBuf>,

    /// 마지막으로 창에 실제로 적용한 제목. 매 프레임 같은 값을 다시 보내지 않기 위한 캐시.
    last_window_title: Option<String>,

    pub status_message: Option<String>,
}

const LAST_OPENED_FILE_KEY: &str = "last_opened_file";

impl PdfViewerApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let engine = create_engine();
        let status_message = if engine.is_none() {
            Some("pdfium 라이브러리를 찾지 못했습니다. PDF를 열 수 없습니다.".to_string())
        } else {
            None
        };

        let last_opened_file = cc
            .storage
            .and_then(|s| s.get_string(LAST_OPENED_FILE_KEY))
            .map(PathBuf::from);

        Self {
            current_file: None,
            current_page: 1,
            total_pages: 0,
            page_number_input: "1".to_string(),
            viewport: ViewportState::default(),
            bookmarks: Vec::new(),
            bookmarks_dirty: false,
            selected_bookmark: None,
            bookmark_undo_stack: VecDeque::new(),
            bookmark_redo_stack: VecDeque::new(),
            request_add_bookmark: false,
            page_back_history: Vec::new(),
            page_forward_history: Vec::new(),
            selection: None,
            selection_drag_start_index: None,
            engine,
            document: None,
            page_texture: None,
            rendered_for: None,
            render_target_width: None,
            image_rect: None,
            pending_open_path: None,
            last_opened_file,
            last_window_title: None,
            status_message,
        }
    }

    /// 새 PDF를 연다. 현재 북마크에 저장 안 된 변경사항이 있으면 즉시 열지 않고 확인창을
    /// 띄운다(pending_open_path에 대기시킴) — 실제로 여는 동작은 `open_file_now`가 한다.
    pub fn request_open_file(&mut self, path: PathBuf) {
        if self.bookmarks_dirty {
            self.pending_open_path = Some(path);
        } else {
            self.open_file_now(path);
        }
    }

    fn open_file_now(&mut self, path: PathBuf) {
        let Some(engine) = &self.engine else {
            self.status_message = Some("pdfium 엔진이 초기화되지 않았습니다.".to_string());
            return;
        };

        match engine.open_document(&path) {
            Ok(document) => {
                self.total_pages = document.pages().len() as u32;
                // 문서 자체에 내장된 북마크(목차)를 읽어 사이드바 초기 상태로 사용한다.
                self.bookmarks = pdf_engine::outline::read_bookmarks(&document);
                self.bookmarks_dirty = false;
                self.selected_bookmark = None;
                self.bookmark_undo_stack.clear();
                self.page_back_history.clear();
                self.page_forward_history.clear();
                self.document = Some(document);
                self.current_file = Some(path);
                self.page_texture = None;
                self.rendered_for = None;
                self.selection = None;
                self.selection_drag_start_index = None;
                self.status_message = None;
                self.go_to_page(1);
            }
            Err(err) => {
                self.status_message = Some(format!("PDF 열기 실패: {err}"));
            }
        }
    }

    /// "저장" 확인창에서 저장을 선택했을 때: 현재 문서에 북마크를 쓴 뒤, 성공하면 대기 중인
    /// 문서를 연다. 실패하면 대기 상태를 유지해 사용자가 다시 시도하거나 취소할 수 있게 한다.
    pub fn confirm_save_then_open_pending(&mut self) {
        if self.save_bookmarks_to_pdf() {
            if let Some(path) = self.pending_open_path.take() {
                self.open_file_now(path);
            }
        }
    }

    pub fn discard_and_open_pending(&mut self) {
        self.bookmarks_dirty = false;
        if let Some(path) = self.pending_open_path.take() {
            self.open_file_now(path);
        }
    }

    pub fn cancel_pending_open(&mut self) {
        self.pending_open_path = None;
    }

    /// 현재 문서에 북마크 트리를 저장한다(PDF 자체의 outline을 갱신).
    /// pdfium은 outline 쓰기 API가 없어 lopdf(pdf_outline_writer)로 별도 처리한다.
    ///
    /// 안전 프로토콜: 원본을 직접 덮어쓰지 않는다. 임시 파일에 먼저 쓴 뒤, pdfium으로
    /// 다시 열어 파싱이 정상인지(페이지 수 일치) 검증하고 나서야 원자적으로 원본과
    /// 교체한다. 검증 실패 시 원본은 그대로 두고 임시 파일만 정리한다.
    pub fn save_bookmarks_to_pdf(&mut self) -> bool {
        let Some(path) = self.current_file.clone() else {
            self.status_message = Some("저장할 문서가 열려있지 않습니다.".to_string());
            return false;
        };
        let Some(engine) = &self.engine else {
            self.status_message = Some("pdfium 엔진이 초기화되지 않았습니다.".to_string());
            return false;
        };

        let temp_path = path.with_extension("bookmarks_tmp.pdf");

        if let Err(err) =
            pdf_outline_writer::write_bookmarks_incremental(&path, &temp_path, &self.bookmarks)
        {
            let _ = std::fs::remove_file(&temp_path);
            self.status_message = Some(format!("북마크 저장 실패: {err}"));
            return false;
        }

        // 검증: 임시 파일을 pdfium으로 다시 열어 페이지 수가 원본과 같은지 확인.
        let verify_result = engine
            .open_document(&temp_path)
            .map(|doc| doc.pages().len());
        let expected_pages = self.total_pages as PdfPageIndex;

        match verify_result {
            Ok(pages) if pages == expected_pages => {
                if let Err(err) = std::fs::rename(&temp_path, &path) {
                    let _ = std::fs::remove_file(&temp_path);
                    self.status_message = Some(format!("저장된 파일 교체 실패: {err}"));
                    return false;
                }
                // 파일이 바뀌었으니 문서 핸들을 새로 연다(콘텐츠는 그대로지만 일관성을 위해).
                if let Ok(reopened) = engine.open_document(&path) {
                    self.document = Some(reopened);
                }
                self.bookmarks_dirty = false;
                self.status_message = Some("PDF에 북마크를 저장했습니다.".to_string());
                true
            }
            Ok(_) => {
                let _ = std::fs::remove_file(&temp_path);
                self.status_message =
                    Some("북마크 저장 검증 실패(페이지 수 불일치) — 원본은 보존됐습니다.".to_string());
                false
            }
            Err(err) => {
                let _ = std::fs::remove_file(&temp_path);
                self.status_message =
                    Some(format!("북마크 저장 검증 실패({err}) — 원본은 보존됐습니다."));
                false
            }
        }
    }

    /// 북마크 트리를 flat row로 펴서 파일명 컬럼에 채울 이름을 정한다.
    /// 열려있는 PDF가 있으면 그 파일명을, 없으면 빈 문자열을 쓴다.
    fn current_filename_for_export(&self) -> String {
        self.current_file
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default()
    }

    pub fn export_bookmarks_csv(&mut self, path: PathBuf) {
        let rows = bookmark::flatten_tree(&self.bookmarks, &self.current_filename_for_export());
        match import_export::export_csv(&rows, &path) {
            Ok(()) => self.status_message = Some(format!("CSV로 내보냈습니다: {:?}", path)),
            Err(err) => self.status_message = Some(format!("CSV 내보내기 실패: {err}")),
        }
    }

    pub fn export_bookmarks_xlsx(&mut self, path: PathBuf) {
        let rows = bookmark::flatten_tree(&self.bookmarks, &self.current_filename_for_export());
        match import_export::export_xlsx(&rows, &path) {
            Ok(()) => self.status_message = Some(format!("Excel로 내보냈습니다: {:?}", path)),
            Err(err) => self.status_message = Some(format!("Excel 내보내기 실패: {err}")),
        }
    }

    pub fn import_bookmarks_csv(&mut self, path: PathBuf) {
        match import_export::import_csv(&path, None) {
            Ok(rows) => {
                self.bookmarks = bookmark::build_tree(&rows);
                self.bookmarks_dirty = true;
                self.status_message = Some(format!("CSV에서 북마크 {}개를 가져왔습니다.", rows.len()));
            }
            Err(err) => self.status_message = Some(format!("CSV 가져오기 실패: {err}")),
        }
    }

    pub fn import_bookmarks_xlsx(&mut self, path: PathBuf) {
        match import_export::import_xlsx(&path) {
            Ok(rows) => {
                self.bookmarks = bookmark::build_tree(&rows);
                self.bookmarks_dirty = true;
                self.status_message = Some(format!("Excel에서 북마크 {}개를 가져왔습니다.", rows.len()));
            }
            Err(err) => self.status_message = Some(format!("Excel 가져오기 실패: {err}")),
        }
    }

    const BOOKMARK_UNDO_LIMIT: usize = 20;

    /// 북마크를 바꾸는 조작(추가/이름수정/삭제/드래그이동) 직전에 반드시 호출해서
    /// 현재 상태를 실행취소 스택에 남긴다.
    pub fn push_bookmark_undo_snapshot(&mut self) {
        if self.bookmark_undo_stack.len() >= Self::BOOKMARK_UNDO_LIMIT {
            self.bookmark_undo_stack.pop_front();
        }
        self.bookmark_undo_stack.push_back(self.bookmarks.clone());
        // 새 편집이 생기면 이전 redo 기록은 더 이상 유효하지 않다(표준 undo/redo 관례).
        self.bookmark_redo_stack.clear();
    }

    /// Cmd+Z(맥)/Ctrl+Z(윈도우) 또는 실행취소 버튼. 스택이 비어있으면 아무 일도 안 한다.
    pub fn undo_bookmarks(&mut self) {
        if let Some(prev) = self.bookmark_undo_stack.pop_back() {
            if self.bookmark_redo_stack.len() >= Self::BOOKMARK_UNDO_LIMIT {
                self.bookmark_redo_stack.pop_front();
            }
            self.bookmark_redo_stack.push_back(self.bookmarks.clone());
            self.bookmarks = prev;
            self.selected_bookmark = None;
            self.bookmarks_dirty = true;
            self.status_message = Some("북마크 변경을 실행취소했습니다.".to_string());
        }
    }

    /// Cmd+Shift+Z(맥)/Ctrl+Y(윈도우 관례도 있지만 여기선 Shift+Z로 통일) 또는 다시 실행 버튼.
    pub fn redo_bookmarks(&mut self) {
        if let Some(next) = self.bookmark_redo_stack.pop_back() {
            self.bookmark_undo_stack.push_back(self.bookmarks.clone());
            self.bookmarks = next;
            self.selected_bookmark = None;
            self.bookmarks_dirty = true;
            self.status_message = Some("북마크 변경을 다시 실행했습니다.".to_string());
        }
    }

    /// "+"버튼: 현재 선택된 북마크가 있으면 그 자식으로, 없으면 최상위에 새 항목을 추가하고
    /// 즉시 이름 편집 상태로 선택한다(사이드바 쪽에서 편집 모드 진입을 이어서 처리).
    pub fn add_bookmark_under_selection(&mut self) -> Uuid {
        self.push_bookmark_undo_snapshot();
        let new_node = BookmarkNode::new("새 북마크", self.current_page);
        let new_id = new_node.id;
        bookmark::insert_node(&mut self.bookmarks, self.selected_bookmark, new_node);
        self.selected_bookmark = Some(new_id);
        self.bookmarks_dirty = true;
        new_id
    }

    /// "-"버튼: 현재 선택된 북마크(하위 트리 포함)를 삭제한다.
    /// 삭제 후 선택이 완전히 풀리면 화살표 키로 트리를 계속 탐색할 수 없게 되므로,
    /// 삭제 "전에" 다음 형제 → 이전 형제 → 부모 순으로 다음 선택 대상을 미리 정해둔다.
    pub fn delete_selected_bookmark(&mut self) {
        let Some(id) = self.selected_bookmark else {
            return;
        };
        let next_selection = bookmark::sibling_or_parent_after_removal(&self.bookmarks, id);
        self.push_bookmark_undo_snapshot();
        if bookmark::remove_node(&mut self.bookmarks, id).is_some() {
            self.selected_bookmark = next_selection;
            self.bookmarks_dirty = true;
        } else {
            // 아무것도 지워지지 않았으면 방금 남긴 스냅샷도 의미가 없으니 되돌린다.
            self.bookmark_undo_stack.pop_back();
        }
    }

    /// 현재 페이지를 주어진 target width(픽셀, 줌 반영됨)로 렌더링해 텍스처로 업로드한다.
    /// 페이지/줌이 바뀌지 않았으면 viewer_panel에서 호출하지 않으므로 매 프레임 재렌더링되지 않는다.
    pub fn render_current_page(
        &mut self,
        ctx: &egui::Context,
        target_width: i32,
    ) -> anyhow::Result<()> {
        use anyhow::Context as _;

        let document = self.document.as_ref().context("열린 문서가 없음")?;
        let page = document
            .pages()
            .get((self.current_page - 1) as PdfPageIndex)
            .context("페이지 조회 실패")?;

        let config = PdfRenderConfig::new().set_target_width(target_width);
        let bitmap = page
            .render_with_config(&config)
            .context("페이지 렌더링 실패")?;

        let width = bitmap.width() as usize;
        let height = bitmap.height() as usize;
        let rgba = bitmap.as_rgba_bytes();
        let color_image = egui::ColorImage::from_rgba_unmultiplied([width, height], &rgba);
        let texture = ctx.load_texture("pdf_page", color_image, egui::TextureOptions::LINEAR);

        self.page_texture = Some(texture);
        self.rendered_for = Some((self.current_page, target_width));
        self.render_target_width = Some(target_width);
        Ok(())
    }

    /// 선택된 range의 텍스트를 클립보드로 복사한다(Cmd+C/Ctrl+C, 우클릭 메뉴에서도 호출).
    pub(crate) fn copy_selection_to_clipboard(&mut self) {
        let (Some(document), Some(range)) = (self.document.as_ref(), self.selection) else {
            return;
        };
        let Ok(page) = document
            .pages()
            .get((self.current_page - 1) as PdfPageIndex)
        else {
            return;
        };
        let Ok(text_page) = page.text() else {
            return;
        };
        match pdf_engine::selection::extract_text(&text_page, range) {
            Ok(text) if !text.is_empty() => {
                if let Ok(mut clipboard) = arboard::Clipboard::new() {
                    if clipboard.set_text(text).is_ok() {
                        self.status_message = Some("선택한 텍스트를 복사했습니다.".to_string());
                    }
                }
            }
            Ok(_) => {}
            Err(err) => {
                self.status_message = Some(format!("텍스트 추출 실패: {err}"));
            }
        }
    }

    /// 문서 내 링크가 가리키는 외부 URI를 시스템 기본 브라우저로 연다.
    pub fn open_external_link(&mut self, url: &str) {
        if let Err(err) = open::that(url) {
            self.status_message = Some(format!("링크 열기 실패: {err}"));
        }
    }

    /// 페이지 이동. 실제 페이지가 바뀌는 경우 현재 페이지를 뒤로가기 히스토리에 쌓고
    /// 앞으로가기 히스토리는 비운다(표준 브라우저 히스토리 관례 — 새로 이동하면 그 시점
    /// 이후의 "앞으로" 기록은 더 이상 의미가 없음). 히스토리 자체를 순회하는
    /// `navigate_back`/`navigate_forward`는 이 함수가 아니라 `set_current_page`를 직접
    /// 써서 순회 중에 히스토리가 다시 쌓이는 걸 방지한다.
    pub fn go_to_page(&mut self, page: u32) {
        let clamped = page.clamp(1, self.total_pages.max(1));
        if clamped != self.current_page {
            self.page_back_history.push(self.current_page);
            self.page_forward_history.clear();
        }
        self.set_current_page(clamped);
    }

    /// Cmd+[ — 웹브라우저 뒤로가기. 히스토리가 비어있으면 아무 일도 안 한다.
    pub fn navigate_back(&mut self) {
        if let Some(prev) = self.page_back_history.pop() {
            self.page_forward_history.push(self.current_page);
            self.set_current_page(prev);
        }
    }

    /// Cmd+] — 웹브라우저 앞으로가기. 히스토리가 비어있으면 아무 일도 안 한다.
    pub fn navigate_forward(&mut self) {
        if let Some(next) = self.page_forward_history.pop() {
            self.page_back_history.push(self.current_page);
            self.set_current_page(next);
        }
    }

    fn set_current_page(&mut self, page: u32) {
        let clamped = page.clamp(1, self.total_pages.max(1));
        self.current_page = clamped;
        self.page_number_input = clamped.to_string();
        // 페이지 전환 시 확대/스크롤 상태 초기화 (Sumatra 관례와 동일)
        self.viewport.pan_offset = egui::Vec2::ZERO;
        self.selection = None;
        self.selection_drag_start_index = None;
    }

    fn handle_page_navigation_keys(&mut self, ctx: &egui::Context) {
        // 텍스트필드 등에 포커스가 있을 때는 방향키를 페이지 이동으로 가로채지 않는다.
        // 사이드바에 선택된 북마크가 있을 때도 마찬가지로 가로채지 않는다 — 그때는
        // 좌/우 화살표가 트리 접기/펼치기(sidebar.rs)로 쓰이기 때문에 페이지 이동과 겹치면
        // 안 된다. 뷰어 패널을 클릭하면 선택이 해제되어 다시 페이지 이동으로 돌아온다.
        if !ctx.wants_keyboard_input() && self.selected_bookmark.is_none() {
            ctx.input(|i| {
                if i.key_pressed(Key::ArrowRight) {
                    self.go_to_page_delta(1);
                }
                if i.key_pressed(Key::ArrowLeft) {
                    self.go_to_page_delta(-1);
                }
            });
        }

        // Cmd+Z(실행취소)/Cmd+Shift+Z(다시 실행). Cmd+C와 달리 egui-winit이 별도 세맨틱
        // 이벤트로 가로채지 않아서 raw 키 체크로 충분하다(직접 확인함). shift 여부로
        // 분기해야 한다 — 안 그러면 Cmd+Shift+Z를 눌러도 "command && key_pressed(Z)"가
        // 참이라 매번 undo만 실행되고 redo는 절대 못 탄다.
        if !ctx.wants_keyboard_input() {
            ctx.input(|i| {
                if i.modifiers.command && i.key_pressed(Key::Z) {
                    if i.modifiers.shift {
                        self.redo_bookmarks();
                    } else {
                        self.undo_bookmarks();
                    }
                }
            });

            // Cmd+B — 선택된 항목의 자식(없으면 최상위)으로 북마크 추가. 실제 처리는
            // sidebar.rs가 담당(편집 포커스 이동까지 이어져야 해서) — 여기선 요청만 세운다.
            if ctx.input(|i| i.modifiers.command && i.key_pressed(Key::B)) {
                self.request_add_bookmark = true;
            }

            // Cmd+[ / Cmd+] — 웹브라우저 뒤로/앞으로가기처럼 페이지 이동 히스토리를
            // 순회한다(북마크 클릭, 문서 내 링크 클릭, 방향키, 페이지 번호 입력 등으로
            // 쌓인 히스토리 — go_to_page 참고). 사이드바 선택 여부와 무관하게 항상
            // 동작한다(Cmd+Z/Cmd+B와 같은 스코프).
            if ctx.input(|i| i.modifiers.command && i.key_pressed(Key::OpenBracket)) {
                self.navigate_back();
            }
            if ctx.input(|i| i.modifiers.command && i.key_pressed(Key::CloseBracket)) {
                self.navigate_forward();
            }

            // Delete/Backspace — 선택된 북마크 삭제. 사이드바 텍스트 편집 중엔
            // wants_keyboard_input()이 true라 여기까지 안 온다(편집 중 백스페이스가
            // 항목 자체를 지워버리는 사고 방지).
            if self.selected_bookmark.is_some()
                && ctx.input(|i| i.key_pressed(Key::Delete) || i.key_pressed(Key::Backspace))
            {
                self.delete_selected_bookmark();
            }
        }

        // Cmd+C(맥)/Ctrl+C(윈도우)는 페이지 이동 방향키와 별개로 항상 확인한다.
        // 예전엔 위 wants_keyboard_input() 얼리 리턴 안쪽에 같이 있어서, 페이지 번호
        // 입력창 등 어떤 텍스트 위젯이라도 포커스를 쥐고 있으면 복사가 조용히 안 먹는
        // 버그가 있었다 — 텍스트 선택 복사는 그런 포커스 상태와 무관하게 동작해야 한다.
        //
        // 그 수정만으로는 여전히 안 됐음: egui-winit이 Cmd+C를 감지하면 raw `Key::C`
        // 키 이벤트 자체를 만들지 않고 `egui::Event::Copy`로 바꿔치기한 뒤 그대로
        // return해버린다(egui-winit 0.29.1 lib.rs의 is_copy_command 분기 참고). 즉
        // `i.key_pressed(Key::C)`는 Cmd+C에서 절대 true가 될 수 없는 조건이었음 —
        // modifiers.command 체크 자체가 무의미했다. `Event::Copy`를 직접 봐야 한다.
        let copy_pressed = ctx.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Copy)));
        if copy_pressed {
            self.copy_selection_to_clipboard();
        }
    }

    /// OS 파일 탐색기에서 PDF를 창으로 드래그 앤 드롭했을 때 처리.
    fn handle_dropped_files(&mut self, ctx: &egui::Context) {
        let dropped_path = ctx.input(|i| {
            i.raw
                .dropped_files
                .first()
                .and_then(|f| f.path.clone())
        });
        if let Some(path) = dropped_path {
            self.request_open_file(path);
        }
    }

    fn go_to_page_delta(&mut self, delta: i32) {
        let next = (self.current_page as i32 + delta).max(1) as u32;
        self.go_to_page(next);
    }

    /// 창 헤더에 "PDF Outliner - 파일명" 형식으로 현재 파일명을 보여준다.
    /// last_window_title로 값이 안 바뀌었으면 매 프레임 viewport 명령을 안 보내게 막는다.
    fn update_window_title(&mut self, ctx: &egui::Context) {
        let title = match &self.current_file {
            Some(path) => {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| path.to_string_lossy().to_string());
                format!("PDF Outliner - {name}")
            }
            None => "PDF Outliner".to_string(),
        };

        if self.last_window_title.as_deref() != Some(title.as_str()) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.last_window_title = Some(title);
        }
    }
}

/// pdfium 동적 라이브러리를 찾아 엔진을 초기화한다.
/// 정식 배포판에서는 앱 번들 내 동봉 경로 하나만 시도하면 되지만, 개발 중에는 이 머신에
/// Homebrew(ocrmypdf 의존성)로 이미 설치된 libpdfium.dylib를 재사용해 실제 렌더링을 검증한다.
fn create_engine() -> Option<PdfEngine> {
    let candidates: Vec<PathBuf> = std::env::var("PDFIUM_DYLIB_PATH")
        .ok()
        .map(PathBuf::from)
        .into_iter()
        .chain([
            PathBuf::from("/opt/homebrew/Cellar/ocrmypdf/17.8.0/libexec/lib/python3.14/site-packages/pypdfium2_raw/libpdfium.dylib"),
        ])
        .collect();

    for candidate in candidates {
        if let Ok(engine) = PdfEngine::new_with_library_path(&candidate) {
            return Some(engine);
        }
    }
    None
}

/// "저장하시겠습니까?" 확인창. 다른 문서를 열려고 하는데 현재 북마크에 저장 안 된
/// 변경사항이 있을 때만 뜬다(pending_open_path가 Some일 때).
fn show_unsaved_changes_dialog(ctx: &egui::Context, app: &mut PdfViewerApp) {
    if app.pending_open_path.is_none() {
        return;
    }

    egui::Window::new("변경사항 저장")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .show(ctx, |ui| {
            ui.label("새 문서를 열면 기존 북마크 변경사항이 유실됩니다.");
            ui.label("기존 내용을 저장하시겠습니까?");
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("저장").clicked() {
                    app.confirm_save_then_open_pending();
                }
                if ui.button("저장하지 않음").clicked() {
                    app.discard_and_open_pending();
                }
                if ui.button("취소").clicked() {
                    app.cancel_pending_open();
                }
            });
        });
}

impl eframe::App for PdfViewerApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.update_window_title(ctx);
        self.handle_page_navigation_keys(ctx);
        self.handle_dropped_files(ctx);

        crate::toolbar::show(ctx, self);
        crate::sidebar::show(ctx, self);
        // viewer_panel보다 먼저 그려야 한다 — 실측된 크래시 원인: "저장"/"저장하지 않음"이
        // 문서를 교체하면서 self.page_texture를 드롭하는데, 이게 viewer_panel::show *뒤에*
        // 일어나면 이번 프레임에 이미 그 텍스처를 참조하는 draw call이 큐에 들어간 상태에서
        // 텍스처가 파괴돼버려 wgpu가 프레임 제출 시 "Texture ... has been destroyed"
        // Validation Error로 패닉한다(실제 터미널 출력으로 확인, wgpu_core.rs:2314).
        // 툴바 "파일 열기"/드래그앤드롭은 전부 viewer_panel보다 먼저 실행돼서 이 문제가
        // 없었다 — 이 확인창만 유일하게 viewer_panel *뒤에* 있어서 걸렸다. 순서를 앞으로
        // 옮기면, 문서 교체가 일어나도 그 프레임의 viewer_panel::show는 이미 None이 된
        // page_texture를 보고 그냥 아무 것도 안 그리게 되어 안전하다.
        show_unsaved_changes_dialog(ctx, self);
        crate::viewer_panel::show(ctx, self);
    }

    /// eframe이 주기적으로/종료 시 호출 — 다음 실행 때 마지막으로 열었던 파일을
    /// 자동으로 열기 위해 경로만 저장한다(북마크 내용 자체는 PDF에 저장하는 게 원칙이라
    /// 여기 별도로 안 담음).
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        if let Some(path) = &self.current_file {
            storage.set_string(LAST_OPENED_FILE_KEY, path.to_string_lossy().to_string());
        }
    }
}
