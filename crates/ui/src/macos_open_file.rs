//! Finder 더블클릭/"다음으로 열기"로 PDF를 열 때 파일 경로를 받기 위한 macOS 전용 코드.
//!
//! winit(이 앱이 쓰는 GUI 백엔드)은 이 macOS 전용 Apple Event(문서 열기,
//! `application:openURLs:`)를 전혀 지원하지 않는다. 처음엔 앱 코드에서 직접
//! `NSAppleEventManager`에 핸들러를 등록해봤지만, macOS는 콜드 스타트(앱이 안 떠
//! 있을 때 더블클릭)의 문서 열기 이벤트를 `NSApplication`의 `finishLaunching` 시퀀스
//! 도중 — 즉 `applicationDidFinishLaunching:` 알림이 뜨기도 전에 — 델리게이트로 동기
//! 호출한다. 그 시점은 전부 `eframe::run_native()` 내부(winit이 NSApplication을 만들고
//! 실행하는 코드)이고 우리 크레이트의 코드는 그보다 항상 늦게 실행되므로, 어떤 시점에
//! 등록하든 앱 코드만으로는 이 콜드 스타트 이벤트를 절대 따라잡을 수 없다(실측 확인 —
//! `open -a`로 이미 실행 중인 인스턴스에 파일을 다시 열면 받아지지만, 새로 실행할 때는
//! 항상 놓침). 그래서 이 프로젝트가 이미 쓰고 있는 winit 포크(한글 IME 수정,
//! pdf_viewer_spec.md §7 참고)에 `application:openURLs:`를 직접 구현해 넣는 방식으로
//! 해결했다 — `winit::platform::macos::take_opened_files()`가 그 결과를 꺼내온다.

use std::path::PathBuf;

use winit::platform::macos::take_opened_files as winit_take_opened_files;

/// 지금까지 Finder가 열어달라고 요청한(아직 처리 안 한) 파일들을 꺼내면서 비운다.
/// 앱이 이미 실행 중일 때 다른 PDF를 다시 "열기"해도 macOS는 새 프로세스를 띄우는
/// 대신 실행 중인 인스턴스로 같은 이벤트를 보내므로, 시작 시 1회성이 아니라 매 프레임
/// 폴링돼야 한다(app.rs의 update 루프에서 호출).
pub fn take_pending_files() -> Vec<PathBuf> {
    winit_take_opened_files()
}
