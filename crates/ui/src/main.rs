// Windows에서 콘솔 서브시스템(기본값) 대신 GUI 서브시스템으로 빌드 — 없으면 실행 시
// 빈 콘솔 창이 같이 뜬다(v0.1.2 배포본에서 사용자 확인). 대가로 Windows에선
// println!/env_logger 콘솔 출력이 안 보이게 되지만 최종 사용자에겐 무관.
#![cfg_attr(windows, windows_subsystem = "windows")]

//! Sumatra급 속도를 목표로 한 PDF 뷰어 진입점.
//! GUI: egui + eframe(wgpu 백엔드) — immediate-mode, 콜드 스타트 최소화 목적으로 선택.
//! 이 크레이트는 macOS/Windows 타겟이며, 실제 렌더링에는 앱 번들에 동봉된 pdfium 동적
//! 라이브러리가 필요하다(런타임 바인딩 방식).

mod app;
mod autosave;
mod fonts;
#[cfg(target_os = "macos")]
mod macos_open_file;
mod toolbar;
mod viewer_panel;
mod sidebar;

use app::PdfViewerApp;

fn main() -> eframe::Result<()> {
    env_logger::init();

    // CLI에서 파일 경로를 인자로 넘기면 시작 시 바로 연다(Windows "연결 프로그램"은
    // 이 방식으로 파일 경로를 argv에 넘겨준다 — macOS는 argv가 아니라 Apple Event로
    // 알려주므로 macos_open_file.rs가 별도로 처리한다).
    let initial_file = std::env::args().nth(1).map(std::path::PathBuf::from);

    // ViewportBuilder에 아이콘을 안 주면 eframe이 자기 기본 아이콘(egui 로고, "e" 모양)을
    // 런타임에 Dock/Cmd+Tab용으로 강제 설정해버린다(eframe-0.29.1의
    // native/epi_integration.rs `load_default_egui_icon` + native/app_icon.rs
    // `AppTitleIconSetter` — macOS에서 실행 중인 NSApplication 아이콘을 직접 갈아치움).
    // Finder 아이콘(.icns, 앱이 안 떠 있어도 보임)과는 별개 경로라 번들에 .icns를 넣는 것만
    // 으로는 Dock/Cmd+Tab이 안 고쳐진다 — 우리 아이콘을 직접 줘야 한다.
    let icon = eframe::icon_data::from_png_bytes(include_bytes!(
        "../../../assets/icon/icon-master-1024.png"
    ))
    .expect("assets/icon/icon-master-1024.png 로드 실패");

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1200.0, 800.0])
            .with_min_inner_size([600.0, 400.0])
            .with_title("PDF Outliner")
            .with_icon(icon),
        // wgpu 백엔드 - 콜드 스타트/렌더링 속도 우선
        renderer: eframe::Renderer::Wgpu,
        wgpu_options: eframe::egui_wgpu::WgpuConfiguration {
            // egui-wgpu 기본 device_descriptor는 max_texture_dimension_2d를 8192로
            // 하드코딩한다 — Retina에서 줌 ~312%면 세로형 페이지 텍스처 높이가 이 한도에
            // 걸려 더 선명하게 확대할 수 없다(§7 "고배율 줌 크래시" 참고). GPU가 실제로
            // 지원하는 한도(Apple Silicon Metal은 16384)로 디바이스를 연다.
            // 이 클로저의 나머지 부분은 egui-wgpu 0.29.1 기본 구현을 그대로 복사한 것.
            device_descriptor: std::sync::Arc::new(|adapter| {
                use eframe::wgpu;
                let base_limits = if adapter.get_info().backend == wgpu::Backend::Gl {
                    wgpu::Limits::downlevel_webgl2_defaults()
                } else {
                    wgpu::Limits::default()
                };
                wgpu::DeviceDescriptor {
                    label: Some("egui wgpu device"),
                    required_features: wgpu::Features::default(),
                    required_limits: wgpu::Limits {
                        max_texture_dimension_2d: adapter.limits().max_texture_dimension_2d,
                        ..base_limits
                    },
                    memory_hints: wgpu::MemoryHints::default(),
                }
            }),
            ..Default::default()
        },
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
            let is_auto_reopen = initial_file.is_none();
            let path_to_open = initial_file.or_else(|| app.last_opened_file.take());
            if let Some(path) = path_to_open {
                app.request_open_file(path);
                // 이전 세션을 이어서 여는 경우에만 마지막으로 보던 페이지로 이동한다
                // (open_file_now가 내부적으로 1페이지로 리셋하므로 그 뒤에 덮어써야 함) —
                // CLI 인자로 명시적으로 다른 파일을 열 때는 그 페이지 번호가 무의미하다.
                if is_auto_reopen {
                    if let Some(page) = app.last_opened_page {
                        app.go_to_page(page);
                        app.scroll_sidebar_to_active_once = true;
                    }
                }
            }
            Ok(Box::new(app))
        }),
    )
}
