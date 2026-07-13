//! 실제 PDF 파일에 북마크를 써서 저장한 뒤, `lopdf`로 다시 열어 우리가 쓴 내용이 정확히
//! 돌아오는지 검증한다. 안전 프로토콜(임시 파일에만 쓰고 원본은 건드리지 않음) 자체도 함께
//! 확인한다.

use bookmark::BookmarkNode;
use pdf_outline_writer::write_bookmarks_incremental;
use std::path::Path;
use tempfile::tempdir;

fn sample_tree() -> Vec<BookmarkNode> {
    let mut child = BookmarkNode::new("1.1절", 2);
    child.children.push(BookmarkNode::new("1.1.1항", 3));
    let mut root1 = BookmarkNode::new("1장 서론", 1);
    root1.children.push(child);
    root1.children.push(BookmarkNode::new("1.2절 한글 제목 테스트", 4));

    let root2 = BookmarkNode::new("2장 결론", 5);

    vec![root1, root2]
}

/// lopdf의 get_toc()로 저장 결과를 읽어 depth/title/page가 원래 트리와 일치하는지 확인.
fn assert_toc_matches(pdf_path: &Path, expected_flat: &[(usize, &str, usize)]) {
    let doc = lopdf::Document::load(pdf_path).expect("저장된 PDF를 lopdf로 다시 열지 못함");
    let toc = doc.get_toc().expect("get_toc 실패 — outline이 없거나 손상됨");
    let actual: Vec<(usize, String, usize)> = toc
        .toc
        .iter()
        .map(|t| (t.level, t.title.clone(), t.page))
        .collect();
    let expected: Vec<(usize, String, usize)> = expected_flat
        .iter()
        .map(|(l, t, p)| (*l, t.to_string(), *p))
        .collect();
    assert_eq!(actual, expected);
}

#[test]
fn write_bookmarks_to_real_sample_pdf_and_reread() {
    // BZR001088_01.pdf는 24페이지짜리라 sample_tree()가 쓰는 페이지 번호(1~5)가 전부
    // 실존 범위 안에 있음. KKZ000160_01.pdf(1페이지짜리)로는 이 테스트를 할 수 없다 —
    // 존재하지 않는 페이지 번호는 안전하게 첫 페이지로 폴백되는 게 의도된 동작이라
    // (add_nodes의 페이지-없음 폴백 로직 참고), 1페이지 문서에서는 전부 1로 뭉개져 보인다.
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../pdf-samples/BZR001088_01.pdf");
    assert!(source.exists(), "샘플 PDF가 없음: {:?}", source);

    let original_bytes = std::fs::read(&source).unwrap();

    let dir = tempdir().unwrap();
    let out_path = dir.path().join("with_bookmarks.pdf");

    let tree = sample_tree();
    write_bookmarks_incremental(&source, &out_path, &tree).unwrap();

    // 원본은 한 바이트도 안 건드렸는지 확인 (안전 프로토콜의 핵심)
    let after_bytes = std::fs::read(&source).unwrap();
    assert_eq!(original_bytes, after_bytes, "원본 파일이 수정됨 — 절대 안 됨");

    // lopdf의 get_toc()는 depth-first 평탄화 결과를 준다 (level은 1-based)
    assert_toc_matches(
        &out_path,
        &[
            (1, "1장 서론", 1),
            (2, "1.1절", 2),
            (3, "1.1.1항", 3),
            (2, "1.2절 한글 제목 테스트", 4),
            (1, "2장 결론", 5),
        ],
    );
}

#[test]
fn out_of_range_page_falls_back_to_first_page() {
    // KKZ000160_01.pdf는 1페이지짜리 — 존재하지 않는 페이지를 가리키는 북마크는
    // 항목을 통째로 누락시키는 대신 첫 페이지로 안전하게 대체되어야 한다.
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../pdf-samples/KKZ000160_01.pdf");

    let dir = tempdir().unwrap();
    let out_path = dir.path().join("out_of_range.pdf");

    let tree = vec![BookmarkNode::new("존재하지 않는 5페이지", 5)];
    write_bookmarks_incremental(&source, &out_path, &tree).unwrap();

    assert_toc_matches(&out_path, &[(1, "존재하지 않는 5페이지", 1)]);
}

#[test]
fn write_empty_bookmarks_removes_outline() {
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../pdf-samples/KKZ000160_01.pdf");

    let dir = tempdir().unwrap();
    let out_path = dir.path().join("no_bookmarks.pdf");

    write_bookmarks_incremental(&source, &out_path, &[]).unwrap();

    let doc = lopdf::Document::load(&out_path).unwrap();
    let result = doc.get_toc();
    assert!(result.is_err(), "빈 북마크 트리 저장 후에도 outline이 남아있음");
}

/// UI가 실제로 밟을 안전 프로토콜의 핵심 전제: lopdf로 쓴 결과를 **pdfium이 정상적으로
/// 다시 읽을 수 있는지**. 이게 실패하면 "저장 후 pdfium으로 재검증"이라는 안전장치 자체가
/// 성립하지 않으므로, 이 테스트가 이 크레이트에서 가장 중요하다.
///
/// pdfium 바인딩은 프로세스당 한 번만 초기화할 수 있는데, `cargo test`는 기본적으로 한
/// 테스트 바이너리 안의 모든 테스트를 여러 스레드로 병렬 실행한다 — 각 테스트 함수가 각자
/// `new_with_library_path`를 부르면 두 번째부터 `PdfiumLibraryBindingsAlreadyInitialized`로
/// 실패한다. `OnceLock`으로 딱 한 번만 초기화해 모든 테스트가 같은 `PdfEngine`을 공유한다
/// (`PdfEngine`은 `&'static Pdfium` 포인터 하나뿐이라 Copy 가능 — pdf_engine/src/lib.rs 참고).
fn shared_engine() -> pdf_engine::PdfEngine {
    static ENGINE: std::sync::OnceLock<pdf_engine::PdfEngine> = std::sync::OnceLock::new();
    *ENGINE.get_or_init(|| {
        let path = std::env::var("PDFIUM_DYLIB_PATH").map(std::path::PathBuf::from).unwrap_or_else(|_| {
            std::path::PathBuf::from(
                "/opt/homebrew/Cellar/ocrmypdf/17.8.0/libexec/lib/python3.14/site-packages/pypdfium2_raw/libpdfium.dylib",
            )
        });
        pdf_engine::PdfEngine::new_with_library_path(&path).expect("pdfium 라이브러리 로드 실패")
    })
}

#[test]
fn pdfium_can_reread_what_lopdf_wrote() {
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../pdf-samples/BZR001088_01.pdf");

    let dir = tempdir().unwrap();
    let out_path = dir.path().join("pdfium_reread.pdf");

    let tree = sample_tree();
    write_bookmarks_incremental(&source, &out_path, &tree).unwrap();

    let engine = shared_engine();

    // 원본과 페이지 수가 같아야 함 (콘텐츠는 안 건드렸으니 당연히 같아야 하지만,
    // 손상됐다면 여기서 파싱 자체가 실패하거나 페이지 수가 달라질 것)
    let original_doc = engine.open_document(&source).expect("원본을 pdfium으로 열지 못함");
    let original_pages = original_doc.pages().len();

    let written_doc = engine
        .open_document(&out_path)
        .expect("lopdf로 저장한 파일을 pdfium이 열지 못함 — 손상 가능성");
    assert_eq!(written_doc.pages().len(), original_pages, "저장 후 페이지 수가 달라짐");

    let read_back = pdf_engine::outline::read_bookmarks(&written_doc);
    assert_eq!(read_back.len(), tree.len());
    assert_eq!(read_back[0].title, "1장 서론");
    assert_eq!(read_back[0].page, 1);
    assert_eq!(read_back[0].children.len(), 2);
    assert_eq!(read_back[0].children[0].title, "1.1절");
    assert_eq!(read_back[0].children[0].children[0].title, "1.1.1항");
    assert_eq!(read_back[1].title, "2장 결론");
    assert_eq!(read_back[1].page, 5);
}

/// 실제 사용자 워크플로우 전체를 재현: 이미 4단계 깊이의 실제 목차(outline)를 가진 진짜
/// 문서(embeddedoutline.pdf, 32페이지 한국현대사 자료 논문)를 pdfium으로 읽어 들이고,
/// 사용자가 편집하듯 항목을 하나 고치고 하나 추가한 뒤, lopdf로 저장하고, 다시 pdfium으로
/// 열어 편집 결과가 정확히 반영됐는지 확인한다.
///
/// `pdf-samples/embeddedoutline.pdf`는 사용자가 실제 앱으로 직접 삭제/저장 기능을 수동
/// 테스트하는 데도 쓰는 "살아있는" 파일이라(실제로 2026-07-13 사용자의 수동 삭제+저장
/// 테스트로 6개였던 장 중 일부가 지워진 채 저장된 걸 이 테스트가 우연히 잡아냈었음 —
/// 코드 버그가 아니라 저장 기능이 정확히 의도대로 동작한 증거였음), 이 자동화 테스트는
/// 반드시 `embeddedoutline 복사본.pdf`(사용자가 미리 가지고 있던, 아무도 안 건드린
/// 원본 백업, 2022년 파일)를 임시 디렉터리로 복사해 그 복사본만 사용한다 — 사용자의
/// 수동 테스트 상태와 절대 간섭하지 않도록.
#[test]
fn read_edit_save_reread_real_document_with_existing_outline() {
    let pristine_original = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../pdf-samples/embeddedoutline 복사본.pdf");
    assert!(pristine_original.exists(), "원본 백업 샘플 PDF가 없음: {:?}", pristine_original);

    let source_dir = tempdir().unwrap();
    let source = source_dir.path().join("embeddedoutline_test_copy.pdf");
    std::fs::copy(&pristine_original, &source).unwrap();

    let engine = shared_engine();

    // 1) 자동 로드: 문서를 열 때 사이드바를 채우는 것과 동일한 경로
    let original_doc = engine.open_document(&source).expect("원본 열기 실패");
    let mut tree = pdf_engine::outline::read_bookmarks(&original_doc);
    assert_eq!(tree.len(), 1, "최상위 북마크가 1개(대제목)여야 함");
    assert_eq!(tree[0].children.len(), 6, "대제목 아래 6개 장이 있어야 함(머리말~맺음말)");

    // 2) 사용자가 편집: 첫 장 제목을 고치고, 마지막에 새 장을 하나 추가
    tree[0].children[0].title = "1. 머리말 (수정됨)".to_string();
    tree[0]
        .children
        .push(bookmark::BookmarkNode::new("7. 새로 추가한 장", 32));

    // 3) 저장 (임시 파일에만 — 원본 확인)
    let original_bytes = std::fs::read(&source).unwrap();
    let dir = tempdir().unwrap();
    let out_path = dir.path().join("edited_outline.pdf");
    write_bookmarks_incremental(&source, &out_path, &tree).unwrap();
    assert_eq!(
        std::fs::read(&source).unwrap(),
        original_bytes,
        "원본 파일이 수정됨 — 절대 안 됨"
    );

    // 4) 재검증: pdfium으로 다시 열어 편집 결과 확인
    let written_doc = engine.open_document(&out_path).expect("저장한 파일을 pdfium이 못 엶");
    assert_eq!(
        written_doc.pages().len(),
        original_doc.pages().len(),
        "페이지 수가 달라짐 — 콘텐츠 손상 가능성"
    );

    let reread = pdf_engine::outline::read_bookmarks(&written_doc);
    assert_eq!(reread.len(), 1);
    assert_eq!(reread[0].children.len(), 7, "6개 기존 장 + 1개 새 장 = 7개");
    assert_eq!(reread[0].children[0].title, "1. 머리말 (수정됨)");
    assert_eq!(reread[0].children[6].title, "7. 새로 추가한 장");
    assert_eq!(reread[0].children[6].page, 32);

    // 편집하지 않은 깊은 하위 항목(4단계 깊이)도 그대로 보존됐는지 확인
    // "4. NARA 소장 한국현대사 관련 문서" > "1) 국문부의 문서분류체제" > "(1) 중앙분류문서철"
    let ch4 = &reread[0].children[3];
    assert_eq!(ch4.title, "4. NARA 소장 한국현대사 관련 문서");
    let sub1 = &ch4.children[0];
    assert_eq!(sub1.title, "1) 국문부의 문서분류체제");
    assert_eq!(sub1.children[0].title, "(1) 중앙분류문서철");
    assert_eq!(sub1.children[0].page, 8);
}

#[test]
fn multiple_saves_stay_readable_by_pdfium() {
    // 증분 저장을 여러 번 반복해도 pdfium이 여전히 정상 파싱하는지 확인
    // (실제 UI에서 "저장"을 여러 번 누르는 시나리오와 동일)
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../pdf-samples/KKZ000160_01.pdf");

    let dir = tempdir().unwrap();
    let step1 = dir.path().join("step1.pdf");
    let step2 = dir.path().join("step2.pdf");

    write_bookmarks_incremental(&source, &step1, &sample_tree()).unwrap();

    let mut tree2 = sample_tree();
    tree2.push(BookmarkNode::new("추가된 3장", 1));
    write_bookmarks_incremental(&step1, &step2, &tree2).unwrap();

    let doc = lopdf::Document::load(&step2).unwrap();
    let toc = doc.get_toc().unwrap();
    assert_eq!(toc.toc.len(), 6); // 기존 5개 + 새로 추가한 1개
}
