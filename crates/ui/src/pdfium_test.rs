//! 시험에서만 쓰는 pdfium 공유 잠금·엔진·샘플 경로.
//!
//! pdfium은 스레드 안전하지 않다(설계 메모 §7). `cargo test`는 시험을 병렬로 돌리므로, 문서를 여는
//! 시험은 이 잠금으로 줄을 세우고 **엔진도 하나만 만들어 나눠 쓴다**. 시험마다 새로 만들면 여러
//! 스레드가 라이브러리를 동시에 초기화해 진짜로 죽는다(2026-09-30에 겪음: SIGSEGV).
//!
//! **모듈마다 따로 두면 안 된다.** 잠금이 둘이면 서로를 막지 못해 병렬로 열리는 길이 그대로 남는다.
//! 그래서 한곳에 모아 두고 모든 모듈이 이것을 쓴다.

/// 문서를 여는 시험을 줄 세우는 잠금. 먼저 잠근 시험이 끝날 때까지 기다린다.
pub fn lock() -> std::sync::MutexGuard<'static, ()> {
    static PDFIUM: std::sync::Mutex<()> = std::sync::Mutex::new(());
    PDFIUM.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// 시험 전체가 나눠 쓰는 엔진. 라이브러리를 못 찾으면 `None`이고, 그때 시험은 조용히 지나간다.
pub fn engine() -> Option<pdf_engine::PdfEngine> {
    static ENGINE: std::sync::OnceLock<Option<pdf_engine::PdfEngine>> = std::sync::OnceLock::new();
    *ENGINE.get_or_init(crate::app::create_engine)
}

/// 샘플 문서 경로. `pdf-samples/`는 저장소에 없으므로, 없으면 시험을 건너뛴다.
pub fn sample(name: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../pdf-samples").join(name)
}
