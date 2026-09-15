use bookmark::BookmarkNode;
use egui::Key;
use pdf_engine::PdfEngine;
use pdfium_render::prelude::*;
use crate::render_worker::{RenderRequest, RenderWorker, WorkerEvent};
use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};
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

/// 방향키를 사이드바(북마크 탐색)와 뷰어(페이지 이동) 중 어디로 보낼지 결정하는 상태.
/// 선택된 북마크가 있는지 여부와는 별개다(2026-07-17 재설계 — 이전엔 `selected_bookmark`
/// 유무로 방향키를 분기했는데, 그러면 "북마크를 클릭한 뒤 화살표로 페이지를 넘겨도 선택은
/// 그대로 유지하고 싶다"는 요구를 표현할 방법이 없었음). Tab으로 전환(app.rs), 뷰어 클릭
/// 시 Viewer로(viewer_panel.rs), 북마크 클릭 시 Sidebar로(sidebar.rs) 바뀐다.
///
/// `SearchResults`(검색 결과 목록, 2026-09-14)는 Tab 순환 체인 밖에 있다 — Ctrl/Cmd+F·
/// 검색창·목록 클릭으로만 들어오고, 거기서 Tab을 누르면 들어오기 직전 영역
/// (`PdfViewerApp::focus_before_search`)으로 돌아간다. 그래서 Viewer↔Sidebar 전환은 그대로다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusArea {
    Sidebar,
    Viewer,
    SearchResults,
}

impl ViewportState {
    pub const MIN_ZOOM: f32 = 0.25;
    pub const MAX_ZOOM: f32 = 8.0;

    /// 확대/축소 버튼용 고정 배율 단계표 — SumatraPDF `DisplayModel.cpp`의
    /// `defaultZoomLevels[]` 중 MIN_ZOOM~MAX_ZOOM 구간과 같은 값. 비율 곱셈(×1.25)은 배율이
    /// 커질수록 한 걸음의 절대 변화량이 급격히 커져 "훅 튀고", 100%·200% 같은 값에 정확히
    /// 멈추지도 않는다. 핀치/Ctrl+휠은 연속 동작이라 이 표를 쓰지 않고 `zoom_by`로 직접
    /// 곱한다(표를 쓰면 한 제스처에 여러 칸씩 건너뛴다).
    pub const ZOOM_STEPS: [f32; 13] = [
        0.25, 0.3333, 0.5, 0.6667, 0.75, 1.0, 1.25, 1.5, 2.0, 3.0, 4.0, 6.0, 8.0,
    ];

    pub fn zoom_by(&mut self, factor: f32) {
        self.zoom = (self.zoom * factor).clamp(Self::MIN_ZOOM, Self::MAX_ZOOM);
    }

    /// 단계표에서 현재 배율보다 큰 첫 값으로. 표에 없는 배율(핀치 후 등)이면 바로 위 단계에 멈춘다.
    pub fn zoom_in(&mut self) {
        self.zoom = Self::ZOOM_STEPS
            .iter()
            .copied()
            .find(|&step| step > self.zoom + 0.001)
            .unwrap_or(Self::MAX_ZOOM);
    }

    /// 단계표에서 현재 배율보다 작은 첫 값으로.
    pub fn zoom_out(&mut self) {
        self.zoom = Self::ZOOM_STEPS
            .iter()
            .rev()
            .copied()
            .find(|&step| step < self.zoom - 0.001)
            .unwrap_or(Self::MIN_ZOOM);
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
    /// 페이지가 바뀔 때마다 그 페이지의 활성 북마크로 자동 동기화된다(`set_current_page`,
    /// 2026-07-17 재설계 — "순회 중인 북마크가 곧 선택된 북마크") — 사이드바 하이라이트도
    /// 이 값 하나만 따라간다.
    pub selected_bookmark: Option<Uuid>,
    /// `selected_bookmark`가 사용자의 명시적 조작(사이드바 클릭/화살표 탐색)으로 정해진
    /// 것인지, 페이지 이동에 따라 자동 동기화된 것인지 구분한다. 자동 선택된 항목을
    /// 처음 클릭했을 때 "이미 선택된 항목 재클릭 = 이름 편집"으로 오인되는 것을 막고
    /// (sidebar.rs), Cmd+B의 "선택 없으면 페이지 순서 삽입" 관례(2026-07-14 스펙)를
    /// 자동 선택이 망가뜨리지 않게 한다(`add_bookmark_under_selection`).
    pub selection_is_explicit: bool,
    /// 방향키를 사이드바/뷰어 중 어디로 보낼지 — `FocusArea` 문서 참고. 기본값은 Viewer
    /// (문서를 막 열었을 때 화살표 키로 곧바로 페이지를 넘길 수 있어야 하므로).
    pub focus_area: FocusArea,
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
    /// `selection`이 속한 페이지 번호 — 쪽 단위 모드에선 항상 `current_page`와 같지만,
    /// 연속 스크롤 모드는 선택한 뒤 다른 페이지로 스크롤해도 선택이 유지되므로(2026-07-18
    /// 요청 — "연속 모드에서도 텍스트 선택/복사가 상식적으로 되어야 함") `current_page`와
    /// 갈라질 수 있어 별도로 기억해야 한다.
    pub selection_page: Option<u32>,
    pub selection_drag_start_index: Option<i32>,

    /// 툴바 검색창에 입력 중인 검색어.
    pub search_query: String,
    /// 마지막으로 완료된 검색의 결과(문서 전체, 페이지 순). 새로 문서를 열면 비운다.
    pub search_matches: Vec<pdf_engine::search::SearchMatch>,
    /// `search_matches` 안에서 현재 보고 있는 항목의 0-based 인덱스.
    pub search_current_index: usize,
    /// 진행 중인 검색. `poll_search_job`이 매 프레임 정해진 페이지 수만큼만 진행시키고
    /// (PDFium은 스레드 안전하지 않아 백그라운드 스레드로 못 돌린다 — `pdf_engine::search`
    /// 모듈 문서 참고), 다 끝나면 결과를 반영하고 비운다.
    pub search_running: Option<pdf_engine::search::IncrementalSearch>,
    /// 검색을 실행했지만 일치하는 결과가 없어 "일치하는 결과가 없습니다" 알림을 띄운 상태.
    pub search_no_results: bool,
    /// 검색 결과 사이드바를 보여줄지(검색 실행·Ctrl/Cmd+F·검색창 클릭 시 열리고 ×로 닫힘).
    pub search_panel_open: bool,
    /// 검색 결과를 메인 창에 붙이지 않고 별도 창(항상 위)으로 띄울지 — 패널의 핀 버튼으로 전환.
    pub search_panel_detached: bool,
    /// 결과 목록 "페이지" 컬럼 폭(pt) — 머리글의 컬럼 경계를 드래그해 조절.
    pub search_page_column_width: f32,
    /// 검색 결과 목록으로 포커스가 들어오기 직전의 영역(뷰어/북마크) — 목록에서 Tab을 누르면
    /// 여기로 돌아간다(`FocusArea` 문서).
    pub focus_before_search: FocusArea,
    /// 선택된 결과 행이 목록에서 안 보이면 보이게 스크롤해 달라는 1회성 요청.
    pub search_scroll_to_current_once: bool,
    /// 이 검색 결과(인덱스)의 위치를 뷰어 화면 중앙에 맞춰 달라는 1회성 요청 — 실제 좌표
    /// 계산은 배율·패널 크기를 아는 viewer_panel이 소비하며 한다(2026-09-14 요청).
    pub search_center_request: Option<usize>,
    /// 검색이 진행되는 동안 사용자가 결과를 직접 골랐는지 — 그렇다면 검색이 끝났을 때
    /// "현재 페이지 이후 첫 결과"로 선택을 옮기지 않는다.
    search_user_selected: bool,
    /// Ctrl/Cmd+F 등으로 검색창에 포커스를 옮겨야 하는지 — 실제 포커스 이동은 toolbar.rs가
    /// 검색창을 그리는 시점에 처리한다(sidebar.rs의 focus_editing과 같은 패턴).
    pub request_focus_search: bool,
    /// 검색이 결과와 함께 끝나 포커스를 "다음 결과"(▶) 버튼으로 옮겨야 하는지 — 검색창에
    /// 포커스가 남아있으면 다음 Enter가 재검색으로 해석돼야 하므로, 결과가 나온 뒤엔
    /// 검색창에서 포커스를 치워둔다.
    pub request_focus_next_result: bool,

    /// pdfium 바인딩. 라이브러리 로드에 실패하면 None으로 두고 뷰어는 안내 메시지만 표시.
    pub engine: Option<PdfEngine>,
    /// PdfEngine이 Pdfium을 `'static`으로 리크해 보관하므로 (crates/pdf_engine/src/lib.rs 참고)
    /// PdfDocument도 자기참조 구조체 없이 여기 필드로 직접 저장할 수 있다.
    pub document: Option<PdfDocument<'static>>,
    /// 쪽 단위 보기에서 지금 보고 있는 페이지와, 그 페이지로 바뀐 시각(`InputState::time`).
    pub single_view_page: Option<u32>,
    pub single_view_switched_at: f64,

    /// GPU가 허용하는 한 변 최대 텍스처 크기(픽셀). 첫 프레임에 wgpu 디바이스에서 조회.
    /// 고배율 줌에서 이 한도를 넘는 텍스처를 만들면 wgpu validation 패닉으로 앱이 죽는다
    /// (Apple Silicon Retina에서 실측 — §7 "고배율 줌 크래시" 참고).
    pub max_texture_side: Option<u32>,
    /// 현재 페이지의 높이/폭 비율(PDF 포인트 기준, 렌더링 시 갱신). viewer_panel이
    /// GPU 텍스처 한도에 맞춰 줌 상한을 역산할 때 사용.
    pub page_aspect: Option<f32>,
    /// 이번 프레임에 페이지 이미지가 그려진 화면 좌표(rect). 클릭 좌표 변환에 사용.
    pub image_rect: Option<egui::Rect>,

    /// 쪽 단위 보기(false) / 연속 스크롤 보기(true) — 단축키 'C'로 전환(2026-07-18 추가).
    /// 연속 스크롤 모드는 현재 검색 하이라이트/텍스트 선택/링크 클릭을 지원하지 않는다
    /// (범위 밖 — 필요하면 'C'로 쪽 단위 모드로 돌아가서 사용).
    pub continuous_scroll: bool,
    /// 문서를 열 때 한 번 계산해두는 페이지별 높이/폭 비율(`compute_page_aspects`) —
    /// 연속 스크롤 모드가 전체 페이지를 렌더링하지 않고도 각 페이지가 차지할 세로 공간을
    /// 미리 알아야 스크롤 총 높이/가상화 범위를 계산할 수 있어서 필요하다.
    pub page_aspects: Vec<f32>,
    /// 렌더링된 페이지 텍스처 캐시 — 두 보기 모드가 함께 쓴다. 연속 스크롤은 보이는 범위
    /// (+여유분), 쪽 단위는 현재 페이지와 앞뒤 한 쪽만 남기도록 매 프레임 정리한다. 빠진
    /// 텍스처는 다음 프레임에 해제된다(`texture_cache` 모듈 문서 — wgpu 크래시 방지).
    pub page_textures: crate::texture_cache::PageTextureCache,
    /// 연속 스크롤 모드에서 "이 페이지로 스크롤해줘"라는 1회성 요청(북마크 클릭/검색
    /// 이동/페이지 번호 입력 등 명시적 이동에서 세워짐, `set_current_page` 참고) — 스크롤
    /// 위치로 현재 페이지를 그냥 추적만 하는 경우(`note_visible_page_during_scroll`)엔
    /// 세우지 않는다. 안 그러면 사용자가 스크롤하는 동안 계속 원래 자리로 끌려간다.
    pub scroll_to_page_once: Option<u32>,
    /// 툴바 "쪽 맞춤" 버튼이 세우는 1회성 요청 — 실제 계산은 그 프레임의 패널 높이를 아는
    /// viewer_panel.rs에서 처리한다(page_aspect처럼 렌더링 시점에만 정확한 값이라 toolbar.rs
    /// 자체에서는 계산할 수 없음).
    pub request_fit_page: bool,
    /// 연속 스크롤 모드가 직전 프레임에 레이아웃한 페이지 폭(pt). 0.0 = 아직 없음.
    /// 줌/창 크기 변화로 폭이 바뀐 프레임을 감지하는 데 쓴다 — (1) 전체 레이아웃 높이가
    /// 변하므로 스크롤 오프셋을 재조정해야 보던 위치가 유지된다(안 하면 확대=앞쪽, 축소=뒤쪽
    /// 페이지로 점프 — 2026-07-18 리포트). 보정은 비율 곱셈이 아니라 앵커 재계산으로 한다
    /// (viewer_panel::anchor_at 참고). 재렌더링 디바운스는 `zoom_changed_at`이 담당한다.
    pub continuous_last_page_width: f32,
    /// 연속 스크롤 보기의 가로 이동(pt, +면 페이지가 오른쪽으로). 확대로 페이지가 패널보다
    /// 넓을 때만 의미가 있고 매 프레임 그 범위로 제한된다 — 트랙패드 좌우 스와이프와 검색
    /// 결과 중앙 맞춤이 움직인다.
    pub continuous_pan_x: f32,
    /// 연속 스크롤 보기에서 우클릭 드래그로 이번 프레임에 쌓인 세로 이동(pt) — 세로 위치는
    /// ScrollArea 오프셋이라 드래그를 처리하는 시점(ScrollArea 안)엔 바꿀 수 없어, 다음 프레임에
    /// ScrollArea를 만들기 전에 오프셋에서 뺀다.
    pub continuous_drag_scroll: f32,
    /// 직전 프레임의 렌더 목표 폭(물리 픽셀 — 줌·패널 폭·화면 배율 반영). 0 = 아직 없음.
    pub last_target_width: i32,
    /// `last_target_width`가 마지막으로 바뀐 시각(`egui::InputState::time`, 초). 핀치/휠
    /// 줌이 이어지는 동안은 기존 텍스처를 목표 크기로 늘려 즉시 보여주고, 배율이 일정 시간
    /// 멎은 뒤에만 pdfium 재렌더링을 한다(viewer_panel의 디바운스 상수 참고). pdfium 렌더는
    /// 메인 스레드에서 동기로 돌고 중간에 끊을 수 없으므로, 곧 버려질 중간 배율의 렌더를
    /// 애초에 시작하지 않는 것이 "최신 배율 하나만 렌더"를 얻는 방법이다.
    pub zoom_changed_at: f64,
    /// 렌더링 전용 보조 프로세스(`crate::render_worker`). 띄우지 못했거나(환경변수
    /// `PDF_RENDER_WORKER=0` 포함) 도중에 죽으면 None — 그때는 UI 스레드에서 직접 렌더링하는
    /// 기존 방식으로 대체한다(`request_page_texture`).
    render_worker: Option<RenderWorker>,
    /// 보조 프로세스에 연 문서의 세대 번호 — 문서를 열 때마다 증가. 다른 세대의 응답은 버린다.
    render_generation: u64,
    /// 요청에 붙이는 화면 번호 — 보이는 화면(모드·페이지 범위·배율)이 바뀔 때 증가해, 보조
    /// 프로세스가 대기열의 옛 요청을 렌더링하지 않고 버리게 한다(`set_render_view`).
    render_epoch: u64,
    render_view_key: Option<(bool, u32, u32, i32)>,
    /// 보조 프로세스에 보내고 아직 결과가 안 온 (페이지, target_width) — 중복 요청 방지.
    render_inflight: HashSet<(u32, i32)>,
    /// 렌더링에 실패한 (페이지, target_width) — 매 프레임 다시 요청하지 않게 기억해 둔다.
    render_failed: HashSet<(u32, i32)>,

    /// 다른 문서를 열려고 했는데 현재 북마크에 저장 안 된 변경사항이 있어 확인을 기다리는 중.
    pub pending_open_path: Option<PathBuf>,

    /// 시작 시 감지된, 이전 세션이 비정상 종료돼 복구 가능한 자동저장 스냅샷이 있으면
    /// 여기 담겨 사용자에게 복구 여부를 물어보는 대화상자로 이어진다(`autosave` 모듈).
    pub pending_recovery: Option<crate::autosave::RecoverableSession>,

    /// 저장 안 된 북마크 변경사항이 있는 채로 창을 닫으려(Cmd+Q, 창 닫기 버튼 등) 해서
    /// 종료를 일단 취소하고 확인창을 띄운 상태 — 다른 문서를 열 때의 저장 확인과 동일한
    /// 관례(`show_unsaved_changes_dialog`)를 종료 시에도 적용한다.
    pub quit_confirmation_pending: bool,

    /// 이전 실행 종료 시점에 열려있던 파일 경로(eframe Storage에서 복원). `main.rs`가
    /// 시작 시 CLI 인자가 없으면 이 값으로 자동으로 연 뒤 소비(take)한다.
    pub last_opened_file: Option<PathBuf>,
    /// 이전 실행 종료 시점에 보고 있던 페이지 번호. `last_opened_file`과 짝을 이뤄 저장되며,
    /// `main.rs`가 자동 재오픈(CLI 인자 없이 이전 세션 파일을 이어서 열 때) 직후 이 페이지로
    /// 이동한다 — CLI 인자로 명시적으로 다른 파일을 열 때는 쓰지 않는다(그 파일과 무관한
    /// 페이지 번호이므로).
    pub last_opened_page: Option<u32>,

    /// 최근에 연 파일 경로 — 최신순, 중복 없이 최대 10개(eframe Storage에서 복원).
    /// 툴바 "파일 열기" 버튼에 마우스를 올리면 드롭다운으로 보여준다(`toolbar.rs`).
    pub recent_files: Vec<PathBuf>,

    /// 마지막으로 창에 실제로 적용한 제목. 매 프레임 같은 값을 다시 보내지 않기 위한 캐시.
    last_window_title: Option<String>,

    /// 직전 프레임에 키보드 포커스를 가졌던 위젯 id. 텍스트 필드 A→B "직행" 포커스 전환을
    /// 감지해 한글 IME 조합 잔여물을 정리하기 위한 것(`guard_ime_across_focus_change`,
    /// §7 "한글 IME" 참고 — 포크한 winit의 discardMarkedText 패치와 한 세트).
    prev_focused_widget: Option<egui::Id>,

    /// 앱 시작 시 마지막으로 보던 페이지를 복원한 직후 한 번만 세우는 플래그 — 사이드바가
    /// 그 페이지에 해당하는(또는 가장 가까운 이전) 북마크로 자동 스크롤한다(`main.rs`에서
    /// 설정, `sidebar.rs`가 소비). 이후 일반 페이지 이동에서는 스크롤하지 않는다 — 사용자
    /// 요청 범위가 "앱 재실행 시"로 한정됨(2026-07-14).
    pub scroll_sidebar_to_active_once: bool,

    /// 페이지가 실제로 바뀐 프레임에 세우는 1회성 플래그 — 사이드바가 이를 받아, 자동
    /// 동기화된 선택 북마크가 스크롤 영역 밖에 있으면(사용자가 사이드바를 딴 데로 스크롤해
    /// 둔 경우) 부드럽게 중앙으로 다시 보이게 한다(2026-07-18 요청). 위
    /// `scroll_sidebar_to_active_once`(무조건 중앙 스크롤)와 달리, 이미 보이는 항목은
    /// 건드리지 않아 화살표로 페이지를 연달아 넘길 때 사이드바가 계속 들썩이지 않는다
    /// (`sidebar.rs`의 `DragState::scroll_selected_into_view`로 이어짐 — 같은 검사 로직).
    pub sidebar_reveal_selected_once: bool,

    /// 열린 파일이 든 폴더의 감시 — 두 가지에 쓴다. (1) 내용이 바깥에서 바뀌면 보던 화면을
    /// 유지한 채 다시 연다(라이브 업데이트, `crate::file_watch` 모듈 문서 — 모든 플랫폼).
    /// (2) 같은 폴더 안에서 이름이 바뀌면(Finder rename 등) `current_file`이 새 이름을 따라간다
    /// — macOS 전용(inode를 신뢰할 수 있는 유닉스 계열만). 다른 폴더로 옮겨졌거나 이 감시가 놓친
    /// 경우는 저장 시점에 `save_as_requested`로 이어진다(§7 "열린 파일 외부 변경 추적" 참고).
    file_watcher: Option<notify::RecommendedWatcher>,
    file_watch_rx: Option<std::sync::mpsc::Receiver<notify::Result<notify::Event>>>,
    #[cfg(target_os = "macos")]
    watched_file_inode: Option<u64>,
    /// 라이브 업데이트 판정(마지막 이벤트 후 대기·쓰기 완료 확인).
    reload_scheduler: crate::file_watch::ReloadScheduler,
    /// 우리가 마지막으로 열거나 저장한 파일 상태 — 확인 시점 상태가 이것과 같으면 다시 열지
    /// 않는다(자기 저장·같은 폴더의 다른 파일 변경을 걸러냄).
    known_file_state: Option<crate::file_watch::FileState>,
    /// 파일이 바뀌어 검색을 다시 돌리는 중 — 끝나도 화면을 결과 위치로 옮기지 않고 포커스도
    /// 건드리지 않는다(`start_search`).
    search_keep_view: bool,

    /// `save_bookmarks_to_pdf`가 원본 파일을 못 찾았을 때 세우는 플래그 — `toolbar.rs`가
    /// 이 값을 보고 저장 대화상자를 띄운 뒤 `save_bookmarks_as`를 호출한다("다른 이름으로
    /// 저장" 플로우, §7 "열린 파일 외부 변경 추적" 참고).
    pub save_as_requested: bool,

    /// 폴더 일괄 북마크 적용 잡(`batch_import` 모듈). Some인 동안 뷰어 영역이 확인/진행/
    /// 로그 화면으로 전환되고, 매 프레임 `batch_import::poll`이 1파일씩 진행시킨다.
    pub batch_import: Option<crate::batch_import::BatchImportJob>,

    pub status_message: Option<String>,
}

const LAST_OPENED_FILE_KEY: &str = "last_opened_file";
const LAST_OPENED_PAGE_KEY: &str = "last_opened_page";
const RECENT_FILES_KEY: &str = "recent_files";
const RECENT_FILES_MAX: usize = 10;

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
        let last_opened_page = cc
            .storage
            .and_then(|s| s.get_string(LAST_OPENED_PAGE_KEY))
            .and_then(|s| s.parse::<u32>().ok());
        let recent_files = cc
            .storage
            .and_then(|s| s.get_string(RECENT_FILES_KEY))
            .and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok())
            .map(|paths| paths.into_iter().map(PathBuf::from).collect())
            .unwrap_or_default();

        // 이번 세션이 자동저장 파일을 건드리기 전에 먼저 확인해야 이전 세션의 흔적을
        // 덮어쓰지 않는다.
        let pending_recovery = crate::autosave::check_for_crash_recovery();

        let render_worker_enabled = std::env::var("PDF_RENDER_WORKER").map_or(true, |value| value != "0");
        let render_worker = if engine.is_some() && render_worker_enabled {
            match RenderWorker::spawn(&cc.egui_ctx) {
                Ok(worker) => Some(worker),
                Err(err) => {
                    eprintln!("렌더링 보조 프로세스를 띄우지 못해 기본 방식으로 렌더링합니다: {err}");
                    None
                }
            }
        } else {
            None
        };

        Self {
            current_file: None,
            current_page: 1,
            total_pages: 0,
            page_number_input: "1".to_string(),
            viewport: ViewportState::default(),
            bookmarks: Vec::new(),
            bookmarks_dirty: false,
            selected_bookmark: None,
            selection_is_explicit: false,
            focus_area: FocusArea::Viewer,
            bookmark_undo_stack: VecDeque::new(),
            bookmark_redo_stack: VecDeque::new(),
            request_add_bookmark: false,
            page_back_history: Vec::new(),
            page_forward_history: Vec::new(),
            selection: None,
            selection_page: None,
            selection_drag_start_index: None,
            search_query: String::new(),
            search_matches: Vec::new(),
            search_current_index: 0,
            search_running: None,
            search_no_results: false,
            search_panel_open: false,
            search_panel_detached: false,
            search_page_column_width: 56.0,
            focus_before_search: FocusArea::Viewer,
            search_scroll_to_current_once: false,
            search_center_request: None,
            search_user_selected: false,
            request_focus_search: false,
            request_focus_next_result: false,
            engine,
            document: None,
            single_view_page: None,
            single_view_switched_at: 0.0,
            max_texture_side: None,
            page_aspect: None,
            image_rect: None,
            continuous_scroll: false,
            page_aspects: Vec::new(),
            page_textures: Default::default(),
            scroll_to_page_once: None,
            request_fit_page: false,
            continuous_last_page_width: 0.0,
            continuous_pan_x: 0.0,
            continuous_drag_scroll: 0.0,
            last_target_width: 0,
            zoom_changed_at: 0.0,
            render_worker,
            render_generation: 0,
            render_epoch: 0,
            render_view_key: None,
            render_inflight: HashSet::new(),
            render_failed: HashSet::new(),
            pending_open_path: None,
            pending_recovery,
            quit_confirmation_pending: false,
            last_opened_file,
            last_opened_page,
            recent_files,
            scroll_sidebar_to_active_once: false,
            sidebar_reveal_selected_once: false,
            file_watcher: None,
            file_watch_rx: None,
            #[cfg(target_os = "macos")]
            watched_file_inode: None,
            reload_scheduler: Default::default(),
            known_file_state: None,
            search_keep_view: false,
            save_as_requested: false,
            batch_import: None,
            last_window_title: None,
            prev_focused_widget: None,
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

    /// 툴바 "파일 열기" 호버 드롭다운(`toolbar.rs`)에서 최근 파일을 선택했을 때 쓴다.
    /// 그 사이 파일이 삭제/이동됐을 수 있으니 열기 전에 존재를 확인하고, 없으면 안내
    /// 메시지를 띄운 뒤 목록에서 빼버린다(있으면 평소처럼 열고, `open_file_now`가
    /// `remember_recent_file`로 맨 앞에 다시 끌어올린다).
    pub fn open_recent_file(&mut self, path: PathBuf) {
        if !path.exists() {
            let name = display_filename(&path);
            self.status_message = Some(format!("파일을 찾을 수 없습니다: {name}"));
            self.recent_files.retain(|p| p != &path);
            return;
        }
        self.request_open_file(path);
    }

    /// `recent_files`에 최신순으로 기록한다 — 이미 있던 항목이면 앞으로 끌어올리고
    /// (중복 없음), 10개를 넘으면 뒤에서부터 잘라낸다. 툴바 "파일 열기" 버튼 호버
    /// 드롭다운(`toolbar.rs`)이 이 목록을 보여준다.
    fn remember_recent_file(&mut self, path: &Path) {
        self.recent_files.retain(|p| p != path);
        self.recent_files.insert(0, path.to_path_buf());
        self.recent_files.truncate(RECENT_FILES_MAX);
    }

    /// 지금 연 파일의 부모 폴더를 감시 시작하고, 라이브 업데이트·이름 변경 추적의 기준(파일
    /// 상태, macOS는 inode)을 기록한다. 파일을 새로 열 때마다 이전 감시는 watcher를 새 값으로
    /// 덮으면서 자동으로 정리된다(Drop).
    fn start_watching_current_file(&mut self) {
        self.file_watcher = None;
        self.file_watch_rx = None;

        let Some(path) = self.current_file.clone() else {
            return;
        };
        self.refresh_watch_baseline(&path);
        let Some(parent) = path.parent() else {
            return;
        };

        let (tx, rx) = std::sync::mpsc::channel();
        let Ok(mut watcher) = notify::recommended_watcher(move |res| {
            let _ = tx.send(res);
        }) else {
            return;
        };
        if notify::Watcher::watch(&mut watcher, parent, notify::RecursiveMode::NonRecursive).is_ok() {
            self.file_watcher = Some(watcher);
            self.file_watch_rx = Some(rx);
        }
    }

    /// 파일 상태를 "알고 있는 상태"로 기록한다 — 파일을 열었을 때, 그리고 **우리가 파일을 바꾼
    /// 직후**(북마크 저장). 그래야 자동 다시 열기가 자기 저장을 바깥 변경으로 오인하지 않는다
    /// (SumatraPDF `ignoreNextAutoReload`와 같은 역할). 저장은 임시 파일을 원본 자리로 옮기는
    /// 방식이라 inode도 바뀌므로 이름 변경 추적 기준(macOS)도 함께 갱신한다.
    fn refresh_watch_baseline(&mut self, path: &Path) {
        self.known_file_state = crate::file_watch::FileState::read(path);
        self.reload_scheduler = Default::default();
        #[cfg(target_os = "macos")]
        {
            use std::os::unix::fs::MetadataExt;
            self.watched_file_inode = std::fs::metadata(path).ok().map(|meta| meta.ino());
        }
    }

    /// 매 프레임 호출 — 감시 이벤트를 비우고 (1) 현재 파일의 내용 변경이면 라이브 업데이트를
    /// 예약·판정하고(`file_watch` 모듈), (2) macOS에서 현재 파일이 그 이름으로 더 이상 없으면
    /// 이름 변경을 따라간다.
    fn poll_file_watch(&mut self, ctx: &egui::Context) {
        let Some(rx) = &self.file_watch_rx else {
            return;
        };
        // egui는 기본적으로 입력 이벤트가 있을 때만 다시 그리는 즉시모드라(§7 "문서 전체
        // 텍스트 검색"의 poll_search_job과 같은 사정), 사용자가 다른 프로그램에서 파일을
        // 바꾸고 이 앱 창은 그대로 idle 상태로 둔 채 마우스도 안 움직이면 update() 자체가
        // 한동안 안 불려서 이 폴링도 멈춰버린다 — 감시 중인 동안은 주기적으로 강제 리페인트를
        // 요청해 폴링이 계속 돌아가게 한다(실측: 이 요청 없이는 창이 idle일 때 파일명 변경
        // 감지가 사용자가 창을 클릭/스크롤하기 전까지 멈춰 있었음, 2026-07-16).
        ctx.request_repaint_after(std::time::Duration::from_millis(500));

        let Some(path) = self.current_file.clone() else {
            return;
        };
        let file_name = path.file_name().map(std::ffi::OsStr::to_os_string);
        let mut dir_changed = false;
        let mut file_touched = false;
        while let Ok(result) = rx.try_recv() {
            dir_changed = true;
            // 오류 이벤트는 무엇이 바뀌었는지 알 수 없어 변경 가능성으로 두고, 실제 판정은
            // 파일 상태 비교에 맡긴다.
            file_touched |= match (&result, &file_name) {
                (Ok(event), Some(name)) => crate::file_watch::event_touches_file(event, name),
                _ => true,
            };
        }
        let now = std::time::Instant::now();
        if file_touched {
            self.reload_scheduler.note_change(now);
        }

        #[cfg(target_os = "macos")]
        if dir_changed && !path.exists() {
            self.follow_renamed_file(&path);
            return;
        }
        #[cfg(not(target_os = "macos"))]
        let _ = dir_changed;

        let current = crate::file_watch::FileState::read(&path);
        match self
            .reload_scheduler
            .tick(now, current, self.known_file_state.as_ref())
        {
            crate::file_watch::ReloadDecision::Reload => self.reload_current_document(),
            crate::file_watch::ReloadDecision::Wait => {
                ctx.request_repaint_after(crate::file_watch::RELOAD_DELAY);
            }
            crate::file_watch::ReloadDecision::Idle | crate::file_watch::ReloadDecision::Skip => {}
        }
    }

    /// 열린 파일이 바깥에서 바뀌었을 때 보던 화면을 유지한 채 다시 연다(라이브 업데이트,
    /// 2026-09-14 요청). 유지: 페이지 번호·배율·팬·연속 스크롤 위치·보기 모드. 보던 페이지가
    /// 없어졌으면(페이지 수가 줄어) 첫 페이지로. 검색 모드였다면 같은 검색어로 목록을 갱신하되
    /// 화면은 옮기지 않는다. 저장 안 한 북마크 편집이 있으면 파일의 북마크로 덮어쓰지 않는다.
    /// 열기에 실패하면(아직 쓰는 중이라 깨진 파일 등) 기존 문서를 그대로 두고 다음 변경을
    /// 기다린다 — SumatraPDF와 같이 복구·암호 입력은 시도하지 않는다.
    fn reload_current_document(&mut self) {
        let Some(path) = self.current_file.clone() else {
            return;
        };
        let Some(engine) = &self.engine else {
            return;
        };
        let document = match engine.open_document(&path) {
            Ok(document) => document,
            Err(err) => {
                crate::trace::record(format_args!("파일 다시 열기 실패(다음 변경 때 재시도): {err}"));
                self.status_message =
                    Some("파일이 바뀌었지만 아직 열 수 없습니다 — 저장이 끝나면 다시 불러옵니다.".to_string());
                return;
            }
        };

        let new_total = document.pages().len() as u32;
        let keep_bookmark_edits = self.bookmarks_dirty;
        if !keep_bookmark_edits {
            self.bookmarks = pdf_engine::outline::read_bookmarks(&document);
        }
        self.document = Some(document);
        self.total_pages = new_total;
        self.compute_page_aspects();
        // 새 렌더가 올 때까지 기존 이미지를 계속 보여준다(하얗게 깜빡이지 않게).
        self.page_textures.invalidate_all();
        self.notify_render_worker_document(&path);
        self.selection = None;
        self.selection_page = None;
        self.selection_drag_start_index = None;
        self.page_back_history.retain(|&page| page <= new_total);
        self.page_forward_history.retain(|&page| page <= new_total);

        // 보던 화면 유지 — 페이지가 남아 있으면 번호·배율·팬·스크롤을 건드리지 않는다.
        let page_gone = self.current_page > new_total.max(1);
        if page_gone {
            self.set_current_page(1);
        } else {
            self.page_number_input = self.current_page.to_string();
            self.page_aspect = self
                .page_aspects
                .get((self.current_page as usize).saturating_sub(1))
                .copied();
            if !keep_bookmark_edits {
                self.selected_bookmark =
                    bookmark::active_bookmark_for_page(&self.bookmarks, self.current_page);
                self.selection_is_explicit = false;
            }
        }

        // 검색 모드였다면 바뀐 내용으로 목록을 갱신한다(화면·포커스는 그대로).
        let query = self.search_query.trim().to_string();
        let had_search = !self.search_matches.is_empty() || self.search_running.is_some();
        if had_search && !query.is_empty() {
            self.start_search(query, true);
        }

        self.refresh_watch_baseline(&path);
        crate::trace::record(format_args!(
            "파일 다시 열기: {new_total}쪽, 보던 페이지 {}{}",
            self.current_page,
            if page_gone { " (없어져 첫 페이지로)" } else { "" }
        ));
        let mut message = "파일이 바뀌어 다시 불러왔습니다".to_string();
        if page_gone {
            message.push_str(" — 보던 페이지가 없어져 첫 페이지로 이동");
        }
        if keep_bookmark_edits {
            message.push_str(" — 저장 안 한 북마크 편집은 그대로 유지");
        }
        self.status_message = Some(message);
    }

    /// macOS — 현재 파일이 그 이름으로 더 이상 없으면(이름이 바뀐 것으로 추정) 기억해 둔
    /// inode로 같은 폴더 안을 뒤져 새 이름을 찾는다. 찾으면 `current_file`/`recent_files`를
    /// 조용히 갱신 — 사용자 입장에서는 저장이 계속 정상 동작하는 것처럼 보인다(요청사항:
    /// seamless하게 대처).
    #[cfg(target_os = "macos")]
    fn follow_renamed_file(&mut self, old_path: &Path) {
        use std::os::unix::fs::MetadataExt;

        let old_path = old_path.to_path_buf();
        let Some(inode) = self.watched_file_inode else {
            return;
        };
        let Some(parent) = old_path.parent() else {
            return;
        };

        let Ok(entries) = std::fs::read_dir(parent) else {
            return;
        };
        for entry in entries.flatten() {
            let Ok(meta) = entry.metadata() else { continue };
            if meta.ino() != inode {
                continue;
            }
            let new_path = entry.path();
            let new_name = display_filename(&new_path);
            self.status_message = Some(format!("파일 이름이 바뀌어 자동으로 따라갔습니다: {new_name}"));
            if let Some(pos) = self.recent_files.iter().position(|p| p == &old_path) {
                self.recent_files[pos] = new_path.clone();
            }
            self.current_file = Some(new_path);
            break;
        }
    }

    /// 매 프레임 호출 — Finder 더블클릭/"다음으로 열기"로 받은 파일이 있으면 연다
    /// (`macos_open_file.rs` 참고, winit이 이 Apple Event를 지원하지 않아 별도 등록한
    /// `NSAppleEventManager` 핸들러가 채워두는 큐를 폴링). 앱이 이미 떠 있는 상태에서
    /// 또 다른 PDF를 열어도(새 프로세스가 아니라) 같은 인스턴스로 이벤트가 다시
    /// 오므로 시작 시 1회성이 아니라 계속 폴링해야 한다.
    #[cfg(target_os = "macos")]
    fn poll_macos_open_file_events(&mut self, ctx: &egui::Context) {
        let files = crate::macos_open_file::take_pending_files();
        let Some(path) = files.into_iter().next() else {
            return;
        };
        self.request_open_file(path);
        ctx.request_repaint();
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
                self.compute_page_aspects();
                self.page_textures.clear();
                self.notify_render_worker_document(&path);
                self.scroll_to_page_once = None;
                self.continuous_last_page_width = 0.0;
                self.continuous_pan_x = 0.0;
                self.search_center_request = None;
                self.remember_recent_file(&path);
                self.current_file = Some(path);
                self.start_watching_current_file();
                self.selection = None;
                self.selection_page = None;
                self.selection_drag_start_index = None;
                // 이전 문서의 검색 결과는 새 문서에서 의미가 없다.
                self.search_matches.clear();
                self.search_current_index = 0;
                self.search_running = None;
                self.search_no_results = false;
                self.close_search_panel();
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

    /// 크래시 복구 대화상자에서 "복구"를 선택했을 때: 그 세션에서 열려있던 문서를 열고,
    /// PDF 자체 outline 대신 자동저장에 남아있던(저장되지 않았던) 북마크 트리로 덮어쓴다.
    /// 아직 PDF에 저장된 건 아니므로 dirty로 표시해 사용자가 "저장"을 눌러야 확정된다.
    pub fn accept_recovery(&mut self) {
        let Some(session) = self.pending_recovery.take() else {
            return;
        };
        self.request_open_file(session.file_path);
        self.bookmarks = session.bookmarks;
        self.bookmarks_dirty = true;
    }

    /// "무시"를 선택했을 때: 복구하지 않고, 다음 실행에도 다시 묻지 않도록 자동저장 흔적을
    /// 즉시 정상 종료 상태로 표시해둔다.
    pub fn dismiss_recovery(&mut self) {
        if let Some(session) = self.pending_recovery.take() {
            crate::autosave::record(Some(&session.file_path), &[], false);
        }
    }

    /// 종료 확인창에서 "저장"을 선택했을 때: 저장에 성공하면 취소해뒀던 종료를 다시
    /// 요청해 실제로 창을 닫는다. 실패하면(에러는 save_bookmarks_to_pdf가 status_message에
    /// 남김) 확인창을 그대로 띄운 채로 둬 사용자가 다시 시도하거나 취소할 수 있게 한다.
    pub fn confirm_save_then_quit(&mut self, ctx: &egui::Context) {
        if self.save_bookmarks_to_pdf() {
            self.quit_confirmation_pending = false;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    /// "저장하지 않음"을 선택했을 때: 편집을 버리고 그대로 종료를 재요청한다.
    pub fn discard_and_quit(&mut self, ctx: &egui::Context) {
        self.bookmarks_dirty = false;
        self.quit_confirmation_pending = false;
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }

    /// "취소"를 선택했을 때: 종료 자체를 그만두고 앱을 계속 쓴다.
    pub fn cancel_quit(&mut self) {
        self.quit_confirmation_pending = false;
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

        if !path.exists() {
            // 이름이 바뀌었거나(같은 폴더 안이면 poll_file_rename이 보통 따라잡지만
            // 놓쳤을 수 있음) 다른 폴더로 이동/삭제된 것으로 보임 — lopdf 기반 쓰기는
            // 이 경로의 실제 파일을 다시 읽어야 하는데 원본이 없으니 더 진행할 수 없다.
            // "다른 이름으로 저장" 플로우로 유도한다(save_as_requested를 세우면
            // toolbar.rs가 저장 대화상자를 띄우고 `save_bookmarks_as`를 부른다).
            self.status_message = Some(
                "원본 파일을 찾을 수 없습니다(이름이 바뀌었거나 이동/삭제됨) — 다른 이름으로 저장해주세요."
                    .to_string(),
            );
            self.save_as_requested = true;
            return false;
        }

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
                // 우리가 방금 바꾼 결과를 기준으로 삼아, 이 저장이 일으킨 감시 이벤트로 문서를
                // 다시 열지 않게 한다(라이브 업데이트 — refresh_watch_baseline 문서).
                self.refresh_watch_baseline(&path);
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

    /// "다른 이름으로 저장" — `save_bookmarks_to_pdf`가 원본을 못 찾아 `save_as_requested`를
    /// 세우면 `toolbar.rs`가 저장 대화상자로 받은 새 경로를 여기로 넘긴다. lopdf 기반
    /// outline 쓰기는 디스크의 실제 파일을 다시 읽어야 하는데 원본이 사라졌으니, pdfium이
    /// 메모리에 이미 들고 있는 문서(원본 경로가 사라져도 이 세션 안에서는 여전히 유효 —
    /// 열 때 내용을 다 읽어들인 뒤라 디스크 경로와 독립적, `Document::save_to_file` 참고)를
    /// 이 새 위치에 먼저 내보낸 뒤, 그 새 파일을 대상으로 평소처럼 북마크를 쓴다.
    pub fn save_bookmarks_as(&mut self, new_path: PathBuf) -> bool {
        let Some(document) = self.document.as_ref() else {
            self.status_message = Some("저장할 문서가 열려있지 않습니다.".to_string());
            return false;
        };
        if let Err(err) = document.save_to_file(&new_path) {
            self.status_message = Some(format!("파일 내보내기 실패: {err}"));
            return false;
        }

        self.current_file = Some(new_path.clone());
        self.remember_recent_file(&new_path);
        self.start_watching_current_file();
        self.save_bookmarks_to_pdf()
    }

    /// 북마크 트리를 flat row로 펴서 파일명 컬럼에 채울 이름을 정한다.
    /// 열려있는 PDF가 있으면 그 파일명을, 없으면 빈 문자열을 쓴다.
    fn current_filename_for_export(&self) -> String {
        self.current_file
            .as_deref()
            .map(display_filename)
            .unwrap_or_default()
    }

    /// 내보내기 저장 대화상자의 기본 파일명 — 열린 파일명을 붙여 "bookmark-<원본이름>.<확장자>"
    /// (예: test.pdf → bookmark-test.xlsx, 2026-07-19 요청). 열린 파일이 없으면 예전
    /// 기본값 "bookmarks.<확장자>"로 폴백.
    pub(crate) fn export_default_filename(&self, ext: &str) -> String {
        let name = self.current_filename_for_export();
        let stem = std::path::Path::new(&name)
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        if stem.is_empty() {
            format!("bookmarks.{ext}")
        } else {
            format!("bookmark-{stem}.{ext}")
        }
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
            Ok(rows) => self.apply_imported_rows(rows, "CSV"),
            Err(err) => self.status_message = Some(format!("CSV 가져오기 실패: {err}")),
        }
    }

    pub fn import_bookmarks_xlsx(&mut self, path: PathBuf) {
        match import_export::import_xlsx(&path) {
            Ok(rows) => self.apply_imported_rows(rows, "Excel"),
            Err(err) => self.status_message = Some(format!("Excel 가져오기 실패: {err}")),
        }
    }

    /// 폴더 일괄 북마크 적용 시작 — 폴더만 받으면 안의 xlsx/csv를 자동 인식해 잡을
    /// 준비하고(파싱/PDF 수집까지만, 파일은 안 건드림) 뷰어 영역이 확인 화면으로
    /// 전환된다. 실제 처리는 거기서 "시작"을 눌러야 진행된다(`batch_import` 모듈 참고).
    pub fn start_batch_import(&mut self, folder: PathBuf) {
        match crate::batch_import::prepare_job(folder) {
            Ok(mut job) => {
                // 완료 화면의 "되돌아가기"가 복귀할 자리 — 배치 직전에 보던 문서/페이지.
                job.return_to = self.current_file.clone().map(|p| (p, self.current_page));
                self.batch_import = Some(job);
            }
            Err(msg) => self.status_message = Some(format!("폴더 일괄 적용 취소: {msg}")),
        }
    }

    /// 매 프레임 일괄 처리 잡을 1파일씩 진행 — 즉시모드 UI를 살리는 프레임 분할 패턴
    /// (poll_search_job과 동일한 이유). 잡을 잠시 꺼내(take) 처리하는 이유는 engine/
    /// current_file을 동시에 빌려야 해서(부분 borrow 회피).
    fn poll_batch_import(&mut self, ctx: &egui::Context) {
        let Some(mut job) = self.batch_import.take() else {
            return;
        };
        crate::batch_import::poll(&mut job, self.engine.as_ref(), self.current_file.as_deref(), ctx);
        self.batch_import = Some(job);
    }

    /// CSV/Excel import 공통 정책(2026-07-19): '파일명' 컬럼이 현재 열린 파일과 일치하는
    /// 행만 받아들이고, '순서' 컬럼 기준으로 정렬한 뒤 트리를 만든다(행이 뒤섞여 있어도
    /// 안전 — `bookmark::prepare_imported_rows`). 일치하는 행이 하나도 없으면 기존
    /// 북마크를 건드리지 않고 안내만 한다(파일을 잘못 고른 경우 데이터가 조용히 사라지는
    /// 사고 방지).
    fn apply_imported_rows(&mut self, rows: Vec<bookmark::BookmarkRow>, format_label: &str) {
        let total = rows.len();
        let current_name = self.current_filename_for_export();
        let (kept, skipped) = bookmark::prepare_imported_rows(rows, &current_name);
        if kept.is_empty() {
            self.status_message = Some(format!(
                "{format_label} 가져오기 취소: '파일명' 컬럼이 현재 파일({current_name})과 일치하는 행이 없습니다({total}행 검사)."
            ));
            return;
        }
        self.push_bookmark_undo_snapshot();
        self.bookmarks = bookmark::build_tree(&kept);
        self.bookmarks_dirty = true;
        self.status_message = Some(if skipped > 0 {
            format!(
                "{format_label}에서 북마크 {}개를 가져왔습니다 (파일명 불일치 {skipped}행 제외).",
                kept.len()
            )
        } else {
            format!("{format_label}에서 북마크 {}개를 가져왔습니다.", kept.len())
        });
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

    /// "+"버튼: 현재 선택된 북마크가 있으면 그 자식으로(단, 그 자식들 사이에서 페이지 순서에
    /// 맞는 위치에), 없으면 페이지 순서상 직전 북마크와 같은 레벨(형제)에 새 항목을 추가하고
    /// 즉시 이름 편집 상태로 선택한다(사이드바 쪽에서 편집 모드 진입을 이어서 처리).
    /// `insert_node_by_page`(무조건 끝에 붙이던 `insert_node`를 대체) 참고.
    pub fn add_bookmark_under_selection(&mut self, title: &str) -> Uuid {
        self.push_bookmark_undo_snapshot();
        let new_node = BookmarkNode::new(title, self.current_page);
        let new_id = new_node.id;
        // 자동 동기화로 잡힌 선택(selection_is_explicit=false)은 "선택 없음"으로 취급 —
        // 페이지 이동만 해도 선택이 거의 항상 잡혀 있게 된 뒤로(set_current_page 참고),
        // 이걸 부모로 쓰면 "선택 없으면 직전 북마크와 같은 레벨에 페이지 순서 삽입"
        // 관례(2026-07-14 스펙)가 사실상 사라져버린다. 사용자가 직접 클릭/화살표로
        // 고른 선택만 "이 노드의 자식으로" 관례를 탄다.
        let parent = self
            .selected_bookmark
            .filter(|_| self.selection_is_explicit);
        bookmark::insert_node_by_page(&mut self.bookmarks, parent, new_node);
        self.selected_bookmark = Some(new_id);
        self.selection_is_explicit = true;
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
            // 삭제 후 이어지는 화살표 탐색/재클릭 이름수정이 예전처럼 동작하도록,
            // 넘겨받은 선택도 명시적 선택으로 취급한다.
            self.selection_is_explicit = next_selection.is_some();
            self.bookmarks_dirty = true;
        } else {
            // 아무것도 지워지지 않았으면 방금 남긴 스냅샷도 의미가 없으니 되돌린다.
            self.bookmark_undo_stack.pop_back();
        }
    }

    /// 페이지 텍스처를 준비한다. 보조 프로세스가 있으면 요청만 보내고 곧바로 돌아온다 —
    /// 결과는 `poll_render_worker`가 다음 프레임 이후 `page_textures`에 넣는다. 없으면 기존처럼
    /// 이 자리에서 동기로 렌더링해 넣는다(그만큼 UI 스레드가 멈춘다).
    ///
    /// 고배율 줌에서 요청 폭 그대로 렌더하면 텍스처가 GPU 최대 크기를 넘어 wgpu validation
    /// 패닉으로 앱이 죽는다(Apple Silicon Retina에서 800% 줌 크래시 실측, Metal 한도 16384 —
    /// §7 참고). 렌더 해상도만 한도 안으로 줄이고 캐시 키에는 요청값(target_width)을 그대로
    /// 써서 매 프레임 재요청을 막는다. 히트테스트/하이라이트(viewer_panel)는 화면 rect 대비
    /// 비율로 좌표를 환산하므로 실제 텍스처 해상도가 요청과 달라도 일치가 유지된다.
    pub(crate) fn request_page_texture(&mut self, ctx: &egui::Context, page: u32, target_width: i32) {
        if self.render_failed.contains(&(page, target_width)) {
            return;
        }
        if self.render_worker.is_some() {
            if self.render_inflight.contains(&(page, target_width)) {
                return;
            }
            let aspect = self
                .page_aspects
                .get((page as usize).saturating_sub(1))
                .copied()
                .unwrap_or(1.414);
            let request = RenderRequest {
                generation: self.render_generation,
                epoch: self.render_epoch,
                page,
                target_width,
                render_width: clamped_render_width(
                    target_width,
                    1.0,
                    aspect,
                    self.max_texture_side.unwrap_or(8192),
                ),
            };
            let sent = self.render_worker.as_mut().map(|worker| worker.render(request));
            match sent {
                Some(Ok(())) => {
                    self.render_inflight.insert((page, target_width));
                    return;
                }
                Some(Err(err)) => self.stop_render_worker(&format!("요청 전송 실패: {err}")),
                None => {}
            }
        }
        match self.render_page_texture(ctx, page, target_width) {
            Ok(texture) => {
                self.page_textures.insert(page, texture, target_width);
            }
            Err(err) => {
                self.render_failed.insert((page, target_width));
                self.status_message = Some(format!("렌더링 실패: {err}"));
            }
        }
    }

    /// 렌더링이 보조 프로세스에서 비동기로 도는지. 아니면(대체 경로) 호출부가 동기 렌더링의
    /// 버벅임 완화책(반해상도·프레임당 1장 업그레이드 등)을 쓴다.
    pub(crate) fn render_worker_active(&self) -> bool {
        self.render_worker.is_some()
    }

    /// 지금 보이는 화면을 알린다. 이전과 다르면 epoch를 올려 보조 프로세스가 대기열의 옛
    /// 요청을 렌더링하지 않고 버리게 한다. key = (연속 스크롤 여부, 첫 페이지, 끝 페이지, 배율 폭).
    pub(crate) fn set_render_view(&mut self, key: (bool, u32, u32, i32)) {
        if self.render_view_key != Some(key) {
            self.render_view_key = Some(key);
            self.render_epoch += 1;
            let (continuous, first, last, width) = key;
            crate::trace::record(format_args!(
                "화면 전환 epoch {}: {} p{first}..p{last} w{width}",
                self.render_epoch,
                if continuous { "연속" } else { "쪽 단위" }
            ));
        }
    }

    /// 새 문서를 연 직후 — 보조 프로세스도 같은 파일을 열게 한다.
    fn notify_render_worker_document(&mut self, path: &Path) {
        self.render_generation += 1;
        self.render_inflight.clear();
        self.render_failed.clear();
        self.render_view_key = None;
        let generation = self.render_generation;
        let result = self
            .render_worker
            .as_mut()
            .map(|worker| worker.open_document(generation, path));
        if let Some(Err(err)) = result {
            self.stop_render_worker(&format!("문서 전달 실패: {err}"));
        }
    }

    /// 보조 프로세스를 내려놓고 동기 렌더링으로 대체한다. 요청 중이던 것은 다음 프레임에
    /// 뷰어가 다시 요청하면서 동기로 렌더링된다.
    fn stop_render_worker(&mut self, reason: &str) {
        if self.render_worker.take().is_some() {
            self.render_inflight.clear();
            eprintln!("렌더링 보조 프로세스 중단: {reason}");
            crate::trace::record(format_args!("렌더링 보조 프로세스 중단: {reason}"));
            self.status_message =
                Some("렌더링 보조 프로세스가 중단되어 기본 렌더링 방식으로 전환했습니다.".to_string());
        }
    }

    /// 매 프레임 뷰어보다 먼저 호출 — 보조 프로세스가 끝낸 렌더를 `page_textures`에 반영한다.
    /// 그리기 전에 교체해야 옛 텍스처 해제가 안전하다(`update`의 순서 주석).
    fn poll_render_worker(&mut self, ctx: &egui::Context) {
        loop {
            let Some(event) = self.render_worker.as_ref().and_then(RenderWorker::try_event) else {
                break;
            };
            match event {
                WorkerEvent::Rendered {
                    generation,
                    page,
                    target_width,
                    image,
                } => {
                    if generation != self.render_generation {
                        continue;
                    }
                    self.render_inflight.remove(&(page, target_width));
                    // 늦게 도착한 옛 배율 결과가 이미 들어온 현재 배율 텍스처를 덮어쓰지 않게
                    // 한다 — 그런 결과는 텍스처로 만들지도 않는다.
                    let current = self.last_target_width;
                    let keep_existing =
                        target_width != current && self.page_textures.width(page) == Some(current);
                    crate::trace::record(format_args!(
                        "렌더 결과 도착 p{page} w{target_width} (현재 배율 w{current}){}",
                        if keep_existing { " — 현재 배율 텍스처가 있어 버림" } else { "" }
                    ));
                    if !keep_existing {
                        // 텍스처 등록은 메인 스레드에서만 한다(무거운 RGBA 변환은 응답 읽기
                        // 스레드가 이미 끝냄) — 텍스처 생명주기를 프레임 흐름 안에 가둔다.
                        let texture = ctx.load_texture(
                            format!("pdf_page_{page}_{target_width}"),
                            image,
                            egui::TextureOptions::LINEAR,
                        );
                        self.page_textures.insert(page, texture, target_width);
                    }
                }
                WorkerEvent::Finished {
                    generation,
                    page,
                    target_width,
                    error,
                } => {
                    if generation != self.render_generation {
                        continue;
                    }
                    self.render_inflight.remove(&(page, target_width));
                    if let Some(message) = error {
                        crate::trace::record(format_args!(
                            "렌더 실패 p{page} w{target_width}: {message}"
                        ));
                        self.render_failed.insert((page, target_width));
                        self.status_message = Some(format!("렌더링 실패: {message}"));
                    }
                }
                WorkerEvent::Died => {
                    self.stop_render_worker("보조 프로세스가 종료됨");
                    break;
                }
            }
        }
    }

    /// 선택된 range의 텍스트를 클립보드로 복사한다(Cmd+C/Ctrl+C, 우클릭 메뉴에서도 호출).
    pub(crate) fn copy_selection_to_clipboard(&mut self) {
        let (Some(document), Some(range)) = (self.document.as_ref(), self.selection) else {
            return;
        };
        // 연속 스크롤 모드에서는 선택한 뒤 다른 페이지로 스크롤할 수 있어 current_page와
        // 선택이 속한 페이지가 다를 수 있다 — selection_page가 진실의 원천이다.
        let selection_page = self.selection_page.unwrap_or(self.current_page);
        let Ok(page) = document
            .pages()
            .get((selection_page - 1) as PdfPageIndex)
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

    /// 검색 실행(돋보기 버튼/검색창 Enter). 문서 전체를 페이지마다 훑어야 해서 한 번에
    /// 끝내면 무거울 수 있는 작업이지만, **PDFium은 스레드 안전하지 않아 백그라운드
    /// 스레드로 돌릴 수 없다**(`pdf_engine::search` 모듈 문서 참고 — 실제로 그렇게 했다가
    /// 검색 버튼을 누르는 즉시 세그폴트가 나는 걸 재현·확인함). 대신
    /// `pdf_engine::search::IncrementalSearch`로 여러 프레임에 나눠 메인 스레드에서만
    /// 진행시킨다 — `poll_search_job`이 매 프레임 이어간다. 이미 진행 중인 검색이 있으면
    /// 무시해 중복 실행을 막는다.
    pub fn execute_search(&mut self) {
        let query = self.search_query.trim().to_string();
        if query.is_empty() || self.search_running.is_some() || self.document.is_none() {
            return;
        }

        // 검색을 실행하면 결과 목록이 포커스를 갖는다(검색어를 지워 검색 모드가 풀리며 포커스가
        // 직전 영역으로 돌아간 뒤 다시 검색하는 경우 포함).
        self.focus_search_results();
        self.start_search(query, false);
    }

    /// 검색을 (다시) 시작한다. `keep_view`는 파일이 바뀌어 목록만 갱신하는 경우(라이브 업데이트)
    /// — 끝났을 때 화면을 결과 위치로 옮기지 않고, 포커스도 건드리지 않으며, 결과가 없어져도
    /// "일치하는 결과가 없습니다" 알림 대신 목록만 비운다.
    fn start_search(&mut self, query: String, keep_view: bool) {
        self.search_matches.clear();
        self.search_current_index = 0;
        self.search_no_results = false;
        self.search_user_selected = false;
        self.search_center_request = None;
        self.search_keep_view = keep_view;
        self.search_running = Some(pdf_engine::search::IncrementalSearch::new(
            query,
            self.total_pages,
        ));
    }

    /// 매 프레임 호출. 진행 중인 검색이 있으면 이번 프레임 몫만큼 진행시키고, 다 끝났으면
    /// 결과를 반영한다.
    ///
    /// egui는 기본적으로 입력 이벤트가 있을 때만 다시 그리는 즉시모드라, 검색이 끝난
    /// 순간을 마우스/키보드 입력 없이도 알아채려면 검색이 진행 중인 동안 매 프레임
    /// `request_repaint()`를 걸어둬야 한다 — 안 그러면 사용자가 뭔가 조작하기 전까지
    /// 다음 배치가 진행되지 않고 멈춘 것처럼 보인다.
    pub fn poll_search_job(&mut self, ctx: &egui::Context) {
        if self.search_running.is_none() {
            return;
        }

        // 한 프레임에 훑을 페이지 수. 페이지 하나 텍스트 검색에 몇 ms 정도 걸릴 수 있어
        // (실측: 360p 문서 전체 스캔에 release 빌드로 약 2초) 너무 크면 그 프레임이
        // 버벅이고, 너무 작으면 전체 완료까지 프레임이 과도하게 많이 필요하다 — 8이
        // 적당한 절충.
        const PAGES_PER_FRAME: usize = 8;

        let Some(document) = self.document.as_ref() else {
            // 검색 중에 문서가 사라졌으면(이론상 open_file_now가 먼저 비우지만 방어적으로)
            // 더 진행할 수 없으니 포기한다.
            self.search_running = None;
            return;
        };
        let search = self
            .search_running
            .as_mut()
            .expect("search_running checked Some above");
        let finished = search.step(document, PAGES_PER_FRAME);
        // 검색이 끝나기 전에도 찾은 만큼 결과 목록에 흘려 보여준다(페이지 순서 유지).
        let new_matches = search.take_new_matches();
        self.search_matches.extend(new_matches);

        ctx.request_repaint();

        if !finished {
            return;
        }

        self.search_running = None;
        let keep_view = std::mem::take(&mut self.search_keep_view);
        if self.search_matches.is_empty() {
            // 파일이 바뀌어 다시 검색한 경우엔 알림창을 띄우지 않고 목록만 비운다.
            self.search_no_results = !keep_view;
            return;
        }
        if keep_view {
            // 라이브 업데이트로 목록만 갱신 — 화면·포커스는 그대로 두고, 목록 선택만 보던
            // 페이지 근처(같거나 이후 첫 결과)로 맞춘다.
            self.search_current_index = self
                .search_matches
                .iter()
                .position(|m| m.page >= self.current_page)
                .unwrap_or(0);
            self.search_scroll_to_current_once = true;
            return;
        }

        // 현재 보고 있는 페이지와 같거나 그 이후에 있는 첫 결과부터 보여준다 — 훑어보던
        // 위치에서 "다음"을 찾는 일반적인 찾기 동작과 맞추기 위함. 그런 결과가 없으면
        // (현재 페이지 이후로는 없다는 뜻) 문서의 첫 결과로 되돌아간다. 검색 도중 사용자가
        // 목록에서 이미 결과를 골랐다면 그 선택을 존중한다.
        if !self.search_user_selected {
            let start = self
                .search_matches
                .iter()
                .position(|m| m.page >= self.current_page)
                .unwrap_or(0);
            self.jump_to_search_match(start);
        }
        // 검색창에서 포커스를 치워 "다음 결과" 버튼으로 옮긴다 — 검색창에 포커스가
        // 남아있으면 다음 Enter가 재검색으로 해석돼버려서 결과를 순회할 수 없다.
        self.request_focus_next_result = true;
    }

    fn jump_to_search_match(&mut self, index: usize) {
        let Some(m) = self.search_matches.get(index) else {
            return;
        };
        self.search_current_index = index;
        self.search_scroll_to_current_once = true;
        // go_to_page가 팬 오프셋을 0으로 되돌린 뒤, 뷰어가 이번 프레임에 결과 위치로 다시 맞춘다.
        self.search_center_request = Some(index);
        self.go_to_page(m.page);
        // 검색 결과를 순회할 때마다 그 페이지의 활성 북마크(go_to_page가 이미
        // set_current_page를 통해 selected_bookmark로 동기화해둠)가 사이드바에서 실제로
        // 보이는 위치까지 스크롤되게 한다 — 안 그러면 하이라이트/선택 상태는 바뀌어도
        // 사이드바가 스크롤 밖에 있으면 사용자가 알아챌 방법이 없다(2026-07-18 요청).
        self.scroll_sidebar_to_active_once = true;
    }

    /// 결과 목록에서 항목을 고른다(클릭·위/아래 화살표) — 그 페이지로 이동한다.
    pub(crate) fn select_search_match(&mut self, index: usize) {
        if index < self.search_matches.len() {
            self.search_user_selected = true;
            self.jump_to_search_match(index);
        }
    }

    /// 결과 목록 위/아래 화살표 — 끝에서는 멈춘다(▶/◀ 버튼과 달리 순환하지 않음).
    pub(crate) fn step_search_selection(&mut self, forward: bool) {
        let len = self.search_matches.len();
        if len == 0 {
            return;
        }
        let current = self.search_current_index.min(len - 1);
        if forward && current + 1 < len {
            self.select_search_match(current + 1);
        } else if !forward && current > 0 {
            self.select_search_match(current - 1);
        }
    }

    /// 검색 결과 목록에 포커스를 준다(Ctrl/Cmd+F, 검색창·목록 클릭). 들어오기 직전 영역을
    /// 기억해 두어 목록에서 Tab을 누르면 그리로 돌아가게 한다(`FocusArea` 문서).
    pub(crate) fn focus_search_results(&mut self) {
        if self.focus_area != FocusArea::SearchResults {
            self.focus_before_search = self.focus_area;
        }
        self.focus_area = FocusArea::SearchResults;
        self.search_panel_open = true;
    }

    /// 검색 모드를 끝낸다(결과 패널의 ×, 또는 검색창을 비움) — 진행 중인 검색·결과 목록·뷰어
    /// 하이라이트를 모두 지우고 결과 패널을 닫는다(2026-09-14 요청: 패널만 닫혀서 하이라이트가
    /// 계속 남아 있었음).
    pub(crate) fn clear_search(&mut self) {
        self.search_running = None;
        self.search_matches.clear();
        self.search_current_index = 0;
        self.search_no_results = false;
        self.search_user_selected = false;
        self.search_center_request = None;
        self.close_search_panel();
    }

    /// 검색 결과 패널을 닫는다. 목록에 포커스가 있었다면 직전 영역으로 돌려준다.
    pub(crate) fn close_search_panel(&mut self) {
        self.search_panel_open = false;
        if self.focus_area == FocusArea::SearchResults {
            self.focus_area = self.focus_before_search;
        }
    }

    /// 다음 결과로 이동(검색창 Enter, ▶ 버튼). 마지막 결과에서는 처음으로 순환한다.
    pub fn search_next(&mut self) {
        if self.search_matches.is_empty() {
            return;
        }
        let next = (self.search_current_index + 1) % self.search_matches.len();
        self.jump_to_search_match(next);
    }

    /// 이전 결과로 이동(◀ 버튼). 첫 결과에서는 마지막으로 순환한다.
    pub fn search_previous(&mut self) {
        if self.search_matches.is_empty() {
            return;
        }
        let len = self.search_matches.len();
        let previous = (self.search_current_index + len - 1) % len;
        self.jump_to_search_match(previous);
    }

    /// "일치하는 결과가 없습니다" 알림을 닫는다(Enter 또는 닫기 버튼) — 닫힌 뒤에는 다시
    /// 검색창에 포커스를 돌려줘야 하므로 그 요청 플래그도 같이 세운다.
    pub fn dismiss_search_no_results(&mut self) {
        self.search_no_results = false;
        self.request_focus_search = true;
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
        // 페이지가 실제로 바뀔 때만: 사이드바가 밀려나 있으면 활성 북마크를 다시 보이게
        // 한다(sidebar_reveal_selected_once 문서 참고). clamp로 같은 페이지에 머무는
        // 경우(예: 1페이지에서 ← 연타)엔 세우지 않아 사이드바가 공연히 움직이지 않는다.
        if clamped != self.current_page {
            self.sidebar_reveal_selected_once = true;
        }
        self.current_page = clamped;
        self.page_number_input = clamped.to_string();
        // 페이지 전환 시 확대/스크롤 상태 초기화 (Sumatra 관례와 동일)
        self.viewport.pan_offset = egui::Vec2::ZERO;
        self.selection = None;
        self.selection_page = None;
        self.selection_drag_start_index = None;
        // 페이지가 바뀌면 사이드바 선택도 그 페이지의 활성 북마크로 자동 동기화한다
        // (2026-07-17 재설계 — "순회 중인 북마크가 곧 선택된 북마크", 최초 구동 시에도
        // 마지막 페이지 복원 경로가 여기를 지나므로 Tab 직후 화살표 탐색이 바로 된다).
        // 사이드바 클릭/화살표 탐색은 이 함수가 불린 "뒤에" 자기가 고른 노드로
        // selected_bookmark를 다시 덮어쓰므로(sidebar.rs의 outcome 적용 순서) 같은
        // 페이지에 북마크가 여러 개여도 명시적 선택이 항상 이긴다.
        self.selected_bookmark = bookmark::active_bookmark_for_page(&self.bookmarks, clamped);
        self.selection_is_explicit = false;
        // page_aspect는 원래 render_current_page(쪽 단위 모드)가 렌더링 시점에 채웠는데,
        // 연속 스크롤 모드는 그 함수를 안 타므로 여기서도 미리 계산해둔 page_aspects에서
        // 동기화한다 — GPU 텍스처 한도 줌 상한/쪽 맞춤 계산이 모드와 무관하게 항상 정확한
        // 값을 보게 하기 위함.
        self.page_aspect = self.page_aspects.get((clamped as usize).saturating_sub(1)).copied();
        // 연속 스크롤 모드에서는 "페이지 이동"이 뷰어 스크롤 위치를 직접 바꾸는 게 아니라
        // 이 1회성 요청을 세우는 것뿐 — 실제 스크롤은 viewer_panel.rs가 그 페이지의 rect로
        // scroll_to_rect를 부르면서 소비한다.
        if self.continuous_scroll {
            self.scroll_to_page_once = Some(clamped);
        }
    }

    /// 쪽 단위 ↔ 연속 스크롤 보기 전환('C' 키와 툴바 모드 토글 버튼이 공유).
    /// 연속 스크롤로 들어갈 때 지금 보던 페이지로 스크롤해야 한다 — 안 그러면 스크롤
    /// 영역의 예전(또는 초기 0) 위치가 그대로 남아 있어서 엉뚱한 페이지로 "점프"한
    /// 것처럼 보인다(2026-07-18 리포트).
    pub(crate) fn toggle_continuous_scroll(&mut self) {
        self.continuous_scroll = !self.continuous_scroll;
        if self.continuous_scroll {
            self.scroll_to_page_once = Some(self.current_page);
        }
    }

    /// 연속 스크롤 모드에서 화면에 보이는 페이지로 `current_page`만 조용히 따라가게 한다
    /// (viewer_panel.rs가 매 프레임 호출). `set_current_page`와 달리 페이지 이동
    /// 히스토리·텍스트 선택·팬 오프셋·`scroll_to_page_once`는 건드리지 않는다 — 스크롤
    /// 한 번에 여러 페이지를 훑고 지나가는 건 "의도적 이동"이 아니라서, 매번 히스토리에
    /// 쌓거나 선택을 지우면 스크롤하는 동안 계속 방해가 된다.
    pub(crate) fn note_visible_page_during_scroll(&mut self, page: u32) {
        let clamped = page.clamp(1, self.total_pages.max(1));
        if clamped == self.current_page {
            return;
        }
        self.current_page = clamped;
        self.page_number_input = clamped.to_string();
        self.selected_bookmark = bookmark::active_bookmark_for_page(&self.bookmarks, clamped);
        self.selection_is_explicit = false;
        self.page_aspect = self.page_aspects.get((clamped as usize).saturating_sub(1)).copied();
        // 연속 스크롤로 페이지 경계를 넘을 때도 사이드바가 밀려나 있으면 활성 북마크를
        // 다시 보이게 한다(뷰어를 스크롤하는 중엔 사이드바를 동시에 스크롤할 수 없으므로
        // 사용자 조작과 싸울 일 없음).
        self.sidebar_reveal_selected_once = true;
    }

    /// 문서를 열 때 한 번, 전체 페이지의 높이/폭 비율을 미리 읽어둔다(렌더링 없이 페이지
    /// 크기 메타데이터만 조회 — 수백 페이지 문서에서도 비용이 작다). 연속 스크롤 모드가
    /// 아직 렌더링하지 않은 페이지의 세로 공간을 미리 알아야 스크롤 총 높이/가상화 범위를
    /// 계산할 수 있어서 필요하다.
    fn compute_page_aspects(&mut self) {
        self.page_aspects.clear();
        let Some(document) = &self.document else { return };
        for i in 0..self.total_pages {
            let aspect = document
                .pages()
                .get(i as PdfPageIndex)
                .map(|page| page.height().value / page.width().value.max(1.0))
                .unwrap_or(1.0);
            self.page_aspects.push(aspect);
        }
    }

    /// 임의 페이지를 지정한 target_width로 UI 스레드에서 동기로 렌더링해 텍스처로 반환한다 —
    /// 보조 프로세스가 없을 때의 대체 경로(`request_page_texture`).
    pub(crate) fn render_page_texture(
        &self,
        ctx: &egui::Context,
        page_number: u32,
        target_width: i32,
    ) -> anyhow::Result<egui::TextureHandle> {
        use anyhow::Context as _;
        let document = self.document.as_ref().context("열린 문서가 없음")?;
        let page = document
            .pages()
            .get((page_number - 1) as PdfPageIndex)
            .context("페이지 조회 실패")?;
        let render_width = clamped_render_width(
            target_width,
            page.width().value,
            page.height().value,
            self.max_texture_side.unwrap_or(8192),
        );
        let config = PdfRenderConfig::new().set_target_width(render_width);
        let bitmap = page.render_with_config(&config).context("페이지 렌더링 실패")?;
        let width = bitmap.width() as usize;
        let height = bitmap.height() as usize;
        let rgba = bitmap.as_rgba_bytes();
        let color_image = egui::ColorImage::from_rgba_unmultiplied([width, height], &rgba);
        Ok(ctx.load_texture(
            format!("pdf_page_continuous_{page_number}"),
            color_image,
            egui::TextureOptions::LINEAR,
        ))
    }

    fn handle_page_navigation_keys(&mut self, ctx: &egui::Context) {
        // Tab(사이드바<->뷰어 포커스 전환)은 여기가 아니라 raw_input_hook에서 처리한다 —
        // 여기서 key_pressed(Tab)를 봐도 egui가 같은 프레임 시작 시점에 이미 그 Tab으로
        // 위젯 포커스 순회를 시작해버려("파일 열기" 버튼 등으로 포커스가 끼어드는 실측
        // 리포트, 2026-07-17) 전환이 오염된다. raw_input_hook 쪽 주석 참고.

        // 텍스트필드 등에 포커스가 있을 때는 방향키를 가로채지 않는다.
        if !ctx.wants_keyboard_input() {
            // 좌/우 화살표는 뷰어가 포커스일 때만 페이지 이동 — 사이드바가 포커스면 좌/우
            // 화살표는 sidebar.rs가 트리 접기/펼치기로 쓴다(예전 동작 그대로).
            if self.focus_area == FocusArea::Viewer {
                ctx.input(|i| {
                    if i.key_pressed(Key::ArrowRight) {
                        self.go_to_page_delta(1);
                    }
                    if i.key_pressed(Key::ArrowLeft) {
                        self.go_to_page_delta(-1);
                    }
                });
            }

            // 검색 결과 목록이 포커스일 때 위/아래 = 결과 선택 이동(뷰어가 그 페이지로 따라감).
            // 분리 창에 OS 포커스가 있을 때의 키는 그 창이 따로 처리한다(search_panel).
            if self.focus_area == FocusArea::SearchResults {
                let (down, up) =
                    ctx.input(|i| (i.key_pressed(Key::ArrowDown), i.key_pressed(Key::ArrowUp)));
                if down || up {
                    self.step_search_selection(down);
                }
            }

            // C — 쪽 단위/연속 스크롤 보기 전환(2026-07-18). 다른 조합키가 전혀 없는 순수
            // "C" 키만 본다 — Cmd+C(복사)는 egui-winit이 raw Key::C 자체를 안 만들고
            // `Event::Copy`로 바꿔치기해서(위 copy_pressed 참고) 원래 안 겹치지만, Windows
            // Ctrl+C는 플랫폼에 따라 raw 키가 같이 올 수도 있어 modifiers가 하나라도 있으면
            // 무시하도록 방어적으로 막는다.
            if ctx.input(|i| i.key_pressed(Key::C) && i.modifiers.is_none()) {
                self.toggle_continuous_scroll();
            }
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
            // 항목 자체를 지워버리는 사고 방지). 사이드바가 포커스일 때만 동작 —
            // 페이지 이동만 해도 선택이 자동 동기화로 거의 항상 잡혀 있으므로
            // (set_current_page 참고), 뷰어를 보다가 무심코 누른 Delete가 엉뚱한
            // 북마크를 지우는 사고를 막아야 한다(2026-07-17).
            if self.focus_area == FocusArea::Sidebar
                && self.selected_bookmark.is_some()
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

        // Cmd+F(맥)/Ctrl+F(윈도우) — 검색창으로 포커스 이동. 어디에 포커스가 있든(예:
        // 사이드바 이름 편집 중이 아닌 한) 항상 가로채야 브라우저의 찾기 단축키처럼
        // 동작한다 — wants_keyboard_input() 게이트 밖에 둔 이유.
        if ctx.input(|i| i.modifiers.command && i.key_pressed(Key::F)) {
            self.request_focus_search = true;
            self.focus_search_results();
        }

        // Cmd+S(맥)/Ctrl+S(윈도우) — PDF에 북마크 저장. "저장" 버튼과 동일한 동작이며,
        // 다른 텍스트 필드에 포커스가 있어도(Cmd+C/Cmd+F와 같은 이유로) 항상 동작해야
        // 하므로 게이트 밖에 둔다. 저장할 변경사항이 없으면(bookmarks_dirty == false)
        // 아무 일도 안 한다 — 툴바 "저장" 버튼의 활성/비활성 조건과 동일하게.
        if self.bookmarks_dirty && ctx.input(|i| i.modifiers.command && i.key_pressed(Key::S)) {
            self.save_bookmarks_to_pdf();
        }

        // "일치하는 결과가 없습니다" 알림이 떠 있는 동안의 Enter는 그 알림을 닫는 데 쓴다
        // — 검색창이 여전히 포커스를 쥐고 있어 wants_keyboard_input()이 true인 상태라도
        // 반드시 동작해야 하므로 게이트 밖에서 확인한다.
        if self.search_no_results && ctx.input(|i| i.key_pressed(Key::Enter)) {
            self.dismiss_search_no_results();
        }
    }

    /// 저장 안 된 북마크 변경사항이 있는 채로 창을 닫으려 하면(닫기 버튼, Cmd+Q 등)
    /// 일단 종료 자체를 취소하고 확인창을 띄운다. eframe 문서에 명시된 관례대로
    /// `close_requested()`가 참인 프레임에 `ViewportCommand::CancelClose`를 보내지
    /// 않으면 이번 프레임이 끝난 뒤 그대로 종료돼버리므로, 반드시 같은 프레임 안에서
    /// 응답해야 한다. 변경사항이 없으면 아무것도 안 해서 정상적으로 닫히게 둔다.
    fn handle_close_request(&mut self, ctx: &egui::Context) {
        if ctx.input(|i| i.viewport().close_requested()) && self.bookmarks_dirty {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.quit_confirmation_pending = true;
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
    /// 한글 IME 자소 유출 워크어라운드(macOS 전용, §7 "한글 IME" 참고).
    ///
    /// 텍스트 위젯 A에서 B로 포커스가 "직행"으로 넘어가면 egui-winit은 IME allowed 값을
    /// 계속 true로 유지해(`ime.is_some()`이 한 프레임도 false가 안 됨) winit이 OS 입력기의
    /// 조합 상태를 정리할 기회가 없고, A에서 조합 중이던 마지막 자소가 B의 첫 입력에 섞여
    /// 나온다. 모든 위젯을 그린 뒤(포커스 확정 후) A→B 전환이 감지되면 같은 프레임 안에서
    /// `IMEAllowed(false)`→`(true)`를 연속으로 보내 조합만 버린다 — 두 명령이 연달아
    /// 처리되므로 이전 v2 워크어라운드와 달리 "IME 꺼진 프레임"이 생기지 않는다.
    ///
    /// 전제: `set_ime_allowed(false)`가 OS input context의 `discardMarkedText()`까지
    /// 불러줘야 실제로 정리된다 — 순정 winit 0.30.13은 자기 내부 버퍼만 비우기 때문에
    /// 워크스페이스 `Cargo.toml`의 `[patch.crates-io]`로 포크한 winit을 쓴다.
    /// B가 텍스트 위젯이 아니면(ime 출력 None) egui-winit이 어차피 allowed를 false로
    /// 내리면서(패치 덕에 discard 포함) 정리되므로 여기선 건드리지 않는다.
    fn guard_ime_across_focus_change(&mut self, ctx: &egui::Context) {
        let focused = ctx.memory(|m| m.focused());
        if cfg!(target_os = "macos") {
            let ime_active = ctx.output_mut(|o| o.ime.is_some());
            if let (Some(prev), Some(cur)) = (self.prev_focused_widget, focused) {
                if prev != cur && ime_active {
                    ctx.send_viewport_cmd(egui::ViewportCommand::IMEAllowed(false));
                    ctx.send_viewport_cmd(egui::ViewportCommand::IMEAllowed(true));
                }
            }
        }
        self.prev_focused_widget = focused;
    }

    /// last_window_title로 값이 안 바뀌었으면 매 프레임 viewport 명령을 안 보내게 막는다.
    fn update_window_title(&mut self, ctx: &egui::Context) {
        // 버전은 빌드 시 넣은 태그(build.rs `embed_version`) — 예: "PDF Outliner v0.2.1 - 문서.pdf"
        // (2026-09-15 요청).
        let app_name = concat!("PDF Outliner ", env!("PDF_OUTLINER_VERSION"));
        let title = match &self.current_file {
            Some(path) => format!("{app_name} - {}", display_filename(path)),
            None => app_name.to_string(),
        };

        if self.last_window_title.as_deref() != Some(title.as_str()) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.last_window_title = Some(title);
        }
    }
}

/// 사람이 읽는 문자열로 화면에 표시하기 전에 NFC(조합형)로 정규화한다. macOS(APFS/HFS+)는
/// 한글 등 분해 가능한 유니코드를 파일/폴더명에 NFD(분해형)로 저장하는데, 그대로 그리면
/// 자소가 낱개로 갈라져 보인다(예: 최근 파일 목록의 긴 한글 파일명에서 재현됨, 사용자
/// 리포트 2026-07-16) — 대부분의 폰트/텍스트 셰이핑이 조합형을 전제로 하기 때문. 실제
/// 파일 경로(`PathBuf`) 자체는 OS가 준 그대로 유지해야 하므로(파일 I/O·비교에 영향 없게)
/// 이 함수는 표시 직전에 뽑아낸 `String`에만 적용한다. Windows(NTFS)는 이미 대개 NFC라
/// 사실상 no-op — 플랫폼 분기 없이 항상 적용해도 안전하다.
pub(crate) fn display_filename(path: &std::path::Path) -> String {
    use unicode_normalization::UnicodeNormalization;
    let raw = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string_lossy().to_string());
    raw.nfc().collect()
}

/// 요청된 렌더 폭을, 결과 텍스처의 가로·세로가 모두 GPU 한도(`max_texture_side`) 안에
/// 들도록 페이지 종횡비를 반영해 줄인다. 세로형 페이지는 높이가 먼저 한도에 걸리므로
/// 폭만 검사해서는 부족하다. 한도가 비정상적으로 크게 보고되는 GPU에서도 거대 텍스처
/// 할당(수 GB)을 피하기 위해 16384로 상한을 둔다.
fn clamped_render_width(
    target_width: i32,
    page_width_pt: f32,
    page_height_pt: f32,
    max_texture_side: u32,
) -> i32 {
    let max_side = max_texture_side.min(16384) as f64;
    let aspect = if page_width_pt > 0.0 {
        page_height_pt as f64 / page_width_pt as f64
    } else {
        1.0
    };

    let mut width = target_width as f64;
    if width * aspect > max_side {
        width = max_side / aspect;
    }
    width = width.min(max_side);
    (width.floor() as i32).max(50)
}

/// pdfium 동적 라이브러리를 찾아 엔진을 초기화한다.
/// 우선순위: (1) 실행 파일 기준 배포 번들 동봉 경로 → (2) PDFIUM_DYLIB_PATH 환경변수(개발용 오버라이드)
/// → (3) 이 머신에 Homebrew(ocrmypdf 의존성)로 이미 설치된 libpdfium.dylib(개발 편의 폴백).
pub(crate) fn create_engine() -> Option<PdfEngine> {
    let mut candidates: Vec<PathBuf> = Vec::new();

    // 배포 번들 레이아웃: macOS는 `PDF Outliner.app/Contents/MacOS/PDF-Outliner` 기준
    // `../Frameworks/libpdfium.dylib`, Windows/Linux는 실행 파일과 같은 디렉토리.
    if let Ok(exe) = std::env::current_exe() {
        if let Some(exe_dir) = exe.parent() {
            if cfg!(target_os = "macos") {
                candidates.push(exe_dir.join("../Frameworks/libpdfium.dylib"));
            } else if cfg!(target_os = "windows") {
                candidates.push(exe_dir.join("pdfium.dll"));
            } else {
                candidates.push(exe_dir.join("libpdfium.so"));
            }
        }
    }

    candidates.extend(std::env::var("PDFIUM_DYLIB_PATH").ok().map(PathBuf::from));

    candidates.push(PathBuf::from(
        "/opt/homebrew/Cellar/ocrmypdf/17.8.0/libexec/lib/python3.14/site-packages/pypdfium2_raw/libpdfium.dylib",
    ));

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

/// "이전 세션이 비정상 종료된 것으로 보입니다" 복구 확인창. 시작 시
/// `autosave::check_for_crash_recovery()`로 감지된 게 있을 때만(`pending_recovery`가
/// `Some`일 때) 뜬다.
fn show_crash_recovery_dialog(ctx: &egui::Context, app: &mut PdfViewerApp) {
    let Some(session) = &app.pending_recovery else {
        return;
    };

    let file_name = display_filename(&session.file_path);
    let saved_at = session
        .saved_at
        .with_timezone(&chrono::Local)
        .format("%Y-%m-%d %H:%M:%S");

    egui::Window::new("이전 세션 복구")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .show(ctx, |ui| {
            ui.label("이전 실행이 비정상 종료된 것으로 보입니다.");
            ui.label(format!("문서: {file_name}"));
            ui.label(format!("마지막 자동저장: {saved_at}"));
            ui.label("저장되지 않았던 북마크 편집을 복구하시겠습니까?");
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("복구").clicked() {
                    app.accept_recovery();
                }
                if ui.button("무시").clicked() {
                    app.dismiss_recovery();
                }
            });
        });
}

/// "저장하시겠습니까?" 종료 확인창. 저장 안 된 북마크 변경사항이 있는 채로 앱을
/// 종료하려 했을 때만(`quit_confirmation_pending`) 뜬다 — 새 문서를 열 때의
/// `show_unsaved_changes_dialog`와 같은 문구/구성.
fn show_quit_confirmation_dialog(ctx: &egui::Context, app: &mut PdfViewerApp) {
    if !app.quit_confirmation_pending {
        return;
    }

    egui::Window::new("변경사항 저장")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .show(ctx, |ui| {
            ui.label("앱을 종료하면 기존 북마크 변경사항이 유실됩니다.");
            ui.label("기존 내용을 저장하시겠습니까?");
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("저장").clicked() {
                    app.confirm_save_then_quit(ctx);
                }
                if ui.button("저장하지 않음").clicked() {
                    app.discard_and_quit(ctx);
                }
                if ui.button("취소").clicked() {
                    app.cancel_quit();
                }
            });
        });
}

/// "일치하는 결과가 없습니다" 검색 결과 없음 알림. Enter(app.rs 전역 처리) 또는 이 창의
/// "확인" 버튼으로 닫을 수 있고, 닫히면 검색창에 다시 포커스가 돌아간다.
fn show_search_no_results_dialog(ctx: &egui::Context, app: &mut PdfViewerApp) {
    if !app.search_no_results {
        return;
    }

    egui::Window::new("검색")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .show(ctx, |ui| {
            ui.label("일치하는 결과가 없습니다.");
            ui.add_space(8.0);
            if ui.button("확인").clicked() {
                app.dismiss_search_no_results();
            }
        });
}

impl eframe::App for PdfViewerApp {
    /// Tab 키를 egui보다 먼저 가로채 사이드바<->뷰어 포커스 전환(FocusArea) 전용으로
    /// 쓴다. update() 안에서 `key_pressed(Tab)`를 보는 방식은 안 된다 — egui의 포커스
    /// 시스템(`Memory::begin_pass`)이 프레임 시작 시점에 raw 이벤트에서 Tab을 읽어
    /// 위젯 포커스 순회를 시작해버리므로("파일 열기" 버튼 등으로 포커스가 끼어드는 실측
    /// 리포트, 2026-07-17), 그 전에 raw input에서 Tab 이벤트 자체를 제거해야 한다.
    ///
    /// Tab 이벤트는 텍스트 편집 중이든 아니든 **항상** 걷어낸다 — 처음엔 편집 중
    /// (wants_keyboard_input)일 때 TextEdit의 표준 동작을 살리려고 통과시켰는데, egui의
    /// "표준 동작"이 바로 위젯 포커스 순회여서 편집 필드에서 Tab을 누르는 순간 포커스가
    /// 다음 위젯(버튼/북마크)으로 옮겨가고, 그 뒤로는 Tab 연타로 모든 위젯을 순회하는
    /// 상태에 빠졌다(사용자 리포트, 2026-07-17). 이 앱에서 Tab의 의미는 사이드바<->뷰어
    /// 전환 딱 하나다: 텍스트 입력 중엔 아무 일도 하지 않고, 그 외엔 전환.
    fn raw_input_hook(&mut self, ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        let mut toggle = false;
        raw_input.events.retain(|event| {
            if let egui::Event::Key {
                key: Key::Tab,
                pressed,
                repeat,
                modifiers,
                ..
            } = event
            {
                if modifiers.is_none() || modifiers.shift_only() {
                    if *pressed && !*repeat {
                        toggle = true;
                    }
                    return false; // egui 포커스 순회로 새어나가지 않게 이벤트 제거
                }
            }
            true
        });
        // 텍스트 입력 중(북마크 이름 편집, 검색창, 페이지 번호 입력)의 Tab은 무시 —
        // 이벤트는 위에서 이미 걷어냈으므로 위젯 순회도 일어나지 않는다.
        if ctx.wants_keyboard_input() {
            return;
        }
        if toggle {
            self.focus_area = match self.focus_area {
                FocusArea::Sidebar => FocusArea::Viewer,
                FocusArea::Viewer => FocusArea::Sidebar,
                // 검색 결과 목록은 순환 체인 밖 — 들어오기 직전 영역으로 돌아간다.
                FocusArea::SearchResults => self.focus_before_search,
            };
            // 사이드바로 들어왔는데 선택이 없으면(현재 페이지보다 앞선 북마크가 하나도
            // 없어 자동 동기화가 None을 남긴 경우) 화살표 탐색의 출발점이 없어 키가
            // 죽는다 — 첫 북마크를 잡아준다.
            if self.focus_area == FocusArea::Sidebar && self.selected_bookmark.is_none() {
                self.selected_bookmark = self.bookmarks.first().map(|n| n.id);
                self.selection_is_explicit = false;
            }
        }
    }

    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        // 어떤 그리기보다 먼저 — 지난 프레임에 캐시에서 빠진 텍스처를 이제 해제한다
        // (texture_cache 모듈 문서: 같은 프레임에 그리고 해제하면 wgpu 패닉).
        self.page_textures.begin_frame(ctx);

        if self.max_texture_side.is_none() {
            self.max_texture_side = frame
                .wgpu_render_state()
                .map(|rs| rs.device.limits().max_texture_dimension_2d);
        }

        self.update_window_title(ctx);
        self.handle_page_navigation_keys(ctx);
        self.handle_dropped_files(ctx);
        self.handle_close_request(ctx);
        self.poll_search_job(ctx);
        self.poll_render_worker(ctx);
        self.poll_batch_import(ctx);
        self.poll_file_watch(ctx);
        #[cfg(target_os = "macos")]
        self.poll_macos_open_file_events(ctx);

        crate::toolbar::show(ctx, self);
        crate::sidebar::show(ctx, self);
        // 오른쪽 검색 결과 패널 — 뷰어(CentralPanel)보다 먼저 공간을 차지해야 한다.
        crate::search_panel::show_docked(ctx, self);
        // viewer_panel보다 먼저 그려야 한다 — 실측된 크래시 원인: "저장"/"저장하지 않음"이
        // 문서를 교체하면서 page_textures의 텍스처를 드롭하는데, 이게 viewer_panel::show *뒤에*
        // 일어나면 이번 프레임에 이미 그 텍스처를 참조하는 draw call이 큐에 들어간 상태에서
        // 텍스처가 파괴돼버려 wgpu가 프레임 제출 시 "Texture ... has been destroyed"
        // Validation Error로 패닉한다(실제 터미널 출력으로 확인, wgpu_core.rs:2314).
        // 툴바 "파일 열기"/드래그앤드롭은 전부 viewer_panel보다 먼저 실행돼서 이 문제가
        // 없었다 — 이 확인창만 유일하게 viewer_panel *뒤에* 있어서 걸렸다. 순서를 앞으로
        // 옮기면, 문서 교체가 일어나도 그 프레임의 viewer_panel::show는 이미 None이 된
        // page_textures를 보고 흰 페이지 자리만 그리게 되어 안전하다.
        show_unsaved_changes_dialog(ctx, self);
        show_crash_recovery_dialog(ctx, self);
        show_quit_confirmation_dialog(ctx, self);
        show_search_no_results_dialog(ctx, self);
        crate::viewer_panel::show(ctx, self);
        // 분리된 검색 결과 창(항상 위) — 별도 OS 창이라 메인 창 레이아웃과 무관.
        crate::search_panel::show_detached(ctx, self);

        // 모든 위젯을 그린 뒤에 호출해야 이번 프레임에 확정된 포커스/IME 출력을 본다.
        self.guard_ime_across_focus_change(ctx);
    }

    /// eframe이 주기적으로(`auto_save_interval`)/종료 시 호출.
    /// - 마지막으로 열었던 파일 경로는 항상 저장(다음 실행 자동 재오픈용).
    /// - 저장되지 않은 북마크 편집이 있으면 크래시 복구용 자동저장 스냅샷도 갱신한다
    ///   (`autosave` 모듈 — PDF 자체 저장과는 별개).
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        if let Some(path) = &self.current_file {
            storage.set_string(LAST_OPENED_FILE_KEY, path.to_string_lossy().to_string());
            storage.set_string(LAST_OPENED_PAGE_KEY, self.current_page.to_string());
        }
        let recent_as_strings: Vec<String> = self
            .recent_files
            .iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect();
        if let Ok(json) = serde_json::to_string(&recent_as_strings) {
            storage.set_string(RECENT_FILES_KEY, json);
        }
        crate::autosave::record(self.current_file.as_deref(), &self.bookmarks, self.bookmarks_dirty);
    }

    /// eframe이 `save()` 직후, 정상 종료 시 딱 한 번 호출 — 저장되지 않은 편집이 남아
    /// 있었더라도(사용자가 "저장 안 함"을 선택했거나 그냥 종료한 경우 포함) 이건 크래시가
    /// 아니라 정상 종료이므로 `clean_exit: true`로 명시해 다음 실행에 복구 프롬프트가
    /// 뜨지 않게 한다.
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        crate::autosave::record(self.current_file.as_deref(), &self.bookmarks, false);
    }

    /// 크래시 복구 자동저장 주기 — 사용자 요청대로 1분.
    fn auto_save_interval(&self) -> std::time::Duration {
        std::time::Duration::from_secs(60)
    }
}

#[cfg(test)]
mod render_width_tests {
    use super::clamped_render_width;

    /// A4 세로형(612×792pt) 기준. 한도 안이면 요청 폭 그대로.
    #[test]
    fn within_limit_is_unchanged() {
        assert_eq!(clamped_render_width(2000, 612.0, 792.0, 16384), 2000);
    }

    /// 세로형 페이지는 높이가 먼저 한도에 걸린다 — 800% 줌 Retina에서 실측된 크래시
    /// 시나리오(요청 폭 16000 → 높이 약 20700 > 16384).
    #[test]
    fn portrait_page_is_height_bound() {
        let w = clamped_render_width(16000, 612.0, 792.0, 16384);
        assert!(w < 16000);
        let h = (w as f64 * 792.0 / 612.0).ceil() as i32;
        assert!(h <= 16384, "height {h} exceeds limit");
    }

    /// 가로형 페이지는 폭이 먼저 걸린다.
    #[test]
    fn landscape_page_is_width_bound() {
        let w = clamped_render_width(20000, 792.0, 612.0, 16384);
        assert_eq!(w, 16384);
    }

    /// GPU가 한도를 비정상적으로 크게 보고해도 16384 상한(거대 텍스처 할당 방지).
    #[test]
    fn sanity_cap_applies() {
        let w = clamped_render_width(40000, 612.0, 612.0, 1_000_000);
        assert_eq!(w, 16384);
    }

    /// 폭 0짜리 비정상 페이지에서도 패닉/0 반환 없이 동작.
    #[test]
    fn degenerate_page_size_is_safe() {
        let w = clamped_render_width(2000, 0.0, 792.0, 16384);
        assert!(w >= 50);
    }
}

#[cfg(test)]
mod zoom_step_tests {
    use super::ViewportState;

    fn viewport(zoom: f32) -> ViewportState {
        ViewportState {
            zoom,
            ..Default::default()
        }
    }

    #[test]
    fn table_spans_zoom_limits() {
        assert_eq!(ViewportState::ZOOM_STEPS[0], ViewportState::MIN_ZOOM);
        assert_eq!(*ViewportState::ZOOM_STEPS.last().unwrap(), ViewportState::MAX_ZOOM);
        assert!(ViewportState::ZOOM_STEPS.windows(2).all(|w| w[0] < w[1]));
    }

    /// 100%에서 한 칸씩 움직이면 표의 값에 정확히 멈춘다.
    #[test]
    fn steps_land_exactly_on_table_values() {
        let mut v = viewport(1.0);
        v.zoom_in();
        assert_eq!(v.zoom, 1.25);
        v.zoom_out();
        assert_eq!(v.zoom, 1.0);
        v.zoom_out();
        assert_eq!(v.zoom, 0.75);
    }

    /// 핀치로 표에 없는 배율이 됐으면 바로 옆 단계로 스냅한다(한 칸 건너뛰지 않음).
    #[test]
    fn off_table_zoom_snaps_to_neighbor() {
        let mut v = viewport(0.9);
        v.zoom_in();
        assert_eq!(v.zoom, 1.0);
        let mut v = viewport(0.9);
        v.zoom_out();
        assert_eq!(v.zoom, 0.75);
    }

    #[test]
    fn stays_within_limits() {
        let mut v = viewport(ViewportState::MAX_ZOOM);
        v.zoom_in();
        assert_eq!(v.zoom, ViewportState::MAX_ZOOM);
        let mut v = viewport(ViewportState::MIN_ZOOM);
        v.zoom_out();
        assert_eq!(v.zoom, ViewportState::MIN_ZOOM);
    }
}
