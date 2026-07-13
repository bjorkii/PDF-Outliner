//! Sumatra급 속도를 목표로 한 PDF 뷰어 진입점.
//! GUI: egui + eframe(wgpu 백엔드) — immediate-mode, 콜드 스타트 최소화 목적으로 선택.
//! 이 크레이트는 macOS/Windows 타겟이며, 실제 렌더링에는 앱 번들에 동봉된 pdfium 동적
//! 라이브러리가 필요하다(런타임 바인딩 방식).

mod app;
mod autosave;
mod fonts;
mod toolbar;
mod viewer_panel;
mod sidebar;

use app::PdfViewerApp;

fn main() -> eframe::Result<()> {
    env_logger::init();

    // Finder "연결 프로그램" 더블클릭이나 CLI에서 파일 경로를 인자로 넘기면 시작 시 바로 연다.
    let initial_file = std::env::args().nth(1).map(std::path::PathBuf::from);

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1200.0, 800.0])
            .with_min_inner_size([600.0, 400.0])
            .with_title("PDF Outliner"),
        // wgpu 백엔드 - 콜드 스타트/렌더링 속도 우선
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };

    eframe::run_native(
        "PDF Outliner",
        native_options,
        Box::new(move |cc| {
            fonts::install_korean_font(&cc.egui_ctx);
            let mut app = PdfViewerApp::new(cc);
            // CLI 인자(더블클릭으로 특정 파일 열기)가 명시적으로 주어졌으면 그게 우선,
            // 없으면 지난 실행에서 열려있던 파일을 자동으로 이어서 연다.
            let path_to_open = initial_file.or_else(|| app.last_opened_file.take());
            if let Some(path) = path_to_open {
                app.request_open_file(path);
            }
            Ok(Box::new(app))
        }),
    )
}
