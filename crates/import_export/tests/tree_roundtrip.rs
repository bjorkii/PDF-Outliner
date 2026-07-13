//! `flatten_tree`/`build_tree`(bookmark 크레이트)와 `export_csv`/`import_csv`,
//! `export_xlsx`/`import_xlsx`(이 크레이트)는 각자 단위 테스트가 있지만, 실제 UI의
//! "북마크 내보내기/가져오기" 버튼이 밟는 전체 경로(트리 -> flat row -> 파일 -> flat row
//! -> 트리)를 이어서 검증한 적은 없었다. id는 가져오기 시 새로 생성되므로 제외하고
//! title/page/children 구조만 비교한다.

use bookmark::{build_tree, flatten_tree, BookmarkNode};
use import_export::{export_csv, export_xlsx, import_csv, import_xlsx};
use tempfile::tempdir;

fn sample_tree() -> Vec<BookmarkNode> {
    let mut ch1 = BookmarkNode::new("1.1절", 2);
    ch1.children.push(BookmarkNode::new("1.1.1항", 3));
    let mut root1 = BookmarkNode::new("1장", 1);
    root1.children.push(ch1);
    root1.children.push(BookmarkNode::new("1.2절", 5));

    let root2 = BookmarkNode::new("2장 결론", 10);

    vec![root1, root2]
}

/// id를 무시하고 title/page/children 구조만 비교.
fn assert_same_shape(a: &[BookmarkNode], b: &[BookmarkNode]) {
    assert_eq!(a.len(), b.len(), "노드 개수가 다름");
    for (x, y) in a.iter().zip(b.iter()) {
        assert_eq!(x.title, y.title, "title 불일치");
        assert_eq!(x.page, y.page, "page 불일치");
        assert_same_shape(&x.children, &y.children);
    }
}

#[test]
fn csv_roundtrip_preserves_tree_shape() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("bookmarks.csv");

    let original = sample_tree();
    let rows = flatten_tree(&original, "문서.pdf");
    export_csv(&rows, &path).unwrap();

    let imported_rows = import_csv(&path, None).unwrap();
    let reconstructed = build_tree(&imported_rows);

    assert_same_shape(&original, &reconstructed);
}

#[test]
fn xlsx_roundtrip_preserves_tree_shape() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("bookmarks.xlsx");

    let original = sample_tree();
    let rows = flatten_tree(&original, "문서.pdf");
    export_xlsx(&rows, &path).unwrap();

    let imported_rows = import_xlsx(&path).unwrap();
    let reconstructed = build_tree(&imported_rows);

    assert_same_shape(&original, &reconstructed);
}
