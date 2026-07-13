//! `PdfViewerApp`는 `eframe::CreationContext`가 필요해 테스트에서 직접 못 만들지만,
//! "저장/저장하지 않음" 확인창 버튼이 실제로 밟는 pdfium document 라이프사이클 시퀀스를
//! 그대로 재현해서, 화면 없이도 pdfium FFI 레벨에서 진짜로 크래시가 나는지 확인한다.
//!
//! 주의(첫 시도에서 얻은 교훈): pdfium 바인딩은 프로세스당 한 번만 초기화해야 한다.
//! 각 테스트가 저마다 `PdfEngine::new_with_library_path`를 새로 부르면, `cargo test`
//! 기본값인 병렬 실행(테스트마다 별도 스레드) 하에서 여러 스레드가 동시에 바인딩을
//! 초기화하려다 진짜로 크래시(SIGTRAP, 힙 손상 감지)가 난다 — 이건 테스트 설계 결함이지,
//! 실제 앱(엔진을 시작 시 딱 한 번, 메인 스레드에서만 만듦)에서는 있을 수 없는 상황이다.
//! `pdf_outline_writer`의 테스트와 동일하게 `OnceLock`으로 엔진을 하나만 만들어 공유한다.

use pdf_engine::PdfEngine;
use std::path::{Path, PathBuf};
use tempfile::tempdir;

fn shared_engine() -> PdfEngine {
    static ENGINE: std::sync::OnceLock<PdfEngine> = std::sync::OnceLock::new();
    *ENGINE.get_or_init(|| {
        let path = std::env::var("PDFIUM_DYLIB_PATH").map(PathBuf::from).unwrap_or_else(|_| {
            PathBuf::from(
                "/opt/homebrew/Cellar/ocrmypdf/17.8.0/libexec/lib/python3.14/site-packages/pypdfium2_raw/libpdfium.dylib",
            )
        });
        PdfEngine::new_with_library_path(&path).expect("pdfium 라이브러리 로드 실패")
    })
}

fn sample(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../../pdf-samples/{name}"))
}

/// "저장하지 않음" 경로 재현: 문서 A를 열어둔 채로 문서 B를 열어 재대입(`document = ...`).
#[test]
fn discard_path_reassignment() {
    let engine = shared_engine();
    let mut document = engine.open_document(&sample("BZB000877_01.pdf")).expect("A 열기 실패");
    println!("A 페이지 수: {}", document.pages().len());

    document = engine.open_document(&sample("KKZ000160_01.pdf")).expect("B 열기 실패");
    println!("B 페이지 수: {}", document.pages().len());
    println!("discard 경로 재현 완료 — 크래시 없음");
}

/// "저장" 경로 전체 재현: save_bookmarks_to_pdf()가 하는 시퀀스 그대로
/// (문서 A 열기 → 북마크 저장 → 임시파일 검증 오픈 → rename → 재오픈 → 대기 문서 B 열기).
#[test]
fn save_path_full_sequence() {
    let engine = shared_engine();

    let pristine_a = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../pdf-samples/embeddedoutline 복사본.pdf");
    let path_b = sample("KKZ000160_01.pdf");

    let work_dir = tempdir().unwrap();
    let path_a = work_dir.path().join("doc_a.pdf");
    std::fs::copy(&pristine_a, &path_a).unwrap();

    let mut document = engine.open_document(&path_a).expect("A 열기 실패");
    let expected_pages = document.pages().len();
    let bookmarks = pdf_engine::outline::read_bookmarks(&document);
    println!("A 페이지 수: {}, 북마크 최상위 개수: {}", expected_pages, bookmarks.len());

    let temp_path = path_a.with_extension("bookmarks_tmp.pdf");
    pdf_outline_writer::write_bookmarks_incremental(&path_a, &temp_path, &bookmarks)
        .expect("북마크 저장 실패");

    // 검증 오픈: 이 시점에 `document`(A)는 아직 살아있음 — save_bookmarks_to_pdf와 동일 조건.
    let verified_pages = engine.open_document(&temp_path).expect("임시파일 열기 실패").pages().len();
    assert_eq!(verified_pages, expected_pages);
    println!("검증 통과: {} 페이지", verified_pages);

    std::fs::rename(&temp_path, &path_a).expect("rename 실패");

    // 재오픈으로 재대입 — 이 시점에도 이전 `document`(A)는 아직 드롭 전.
    document = engine.open_document(&path_a).expect("재오픈 실패");
    println!("재오픈 성공, 페이지 수: {}", document.pages().len());

    // 대기 중이던 문서 B로 재대입.
    document = engine.open_document(&path_b).expect("B 열기 실패");
    println!("B 페이지 수: {}", document.pages().len());
    println!("save 경로 전체 재현 완료 — 크래시 없음");
}

/// 문서 세 개를 연달아 재대입 — 반복 전환에서도 문제없는지.
#[test]
fn repeated_reassignment_three_documents() {
    let engine = shared_engine();
    let mut document = engine.open_document(&sample("BZB000877_01.pdf")).expect("1 열기 실패");
    println!("문서1 페이지 수: {}", document.pages().len());
    document = engine.open_document(&sample("KKZ000160_01.pdf")).expect("2 열기 실패");
    println!("문서2 페이지 수: {}", document.pages().len());
    document = engine.open_document(&sample("BZR001088_01.pdf")).expect("3 열기 실패");
    println!("문서3 페이지 수: {}", document.pages().len());
    println!("반복 전환 완료 — 크래시 없음");
}
