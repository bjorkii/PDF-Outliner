use pdf_engine::PdfEngine;
use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let lib_path = PathBuf::from(args.next().expect("1: pdfium dylib 경로"));
    let pdf_path = PathBuf::from(args.next().expect("2: PDF 경로"));

    let engine = PdfEngine::new_with_library_path(&lib_path)?;
    let doc = engine.open_document(&pdf_path)?;
    let bookmarks = pdf_engine::outline::read_bookmarks(&doc);

    println!("{}: 최상위 북마크 {}개", pdf_path.display(), bookmarks.len());
    fn print_tree(nodes: &[bookmark::BookmarkNode], depth: usize) {
        for n in nodes {
            println!("{}- {} (p.{})", "  ".repeat(depth), n.title, n.page);
            print_tree(&n.children, depth + 1);
        }
    }
    print_tree(&bookmarks, 1);

    Ok(())
}
