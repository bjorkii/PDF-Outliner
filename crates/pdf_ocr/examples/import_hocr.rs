//! 진단용: hOCR을 PDF에 텍스트 레이어로 넣는다(분류·중복 제거·검증 없이, 기존 보이지 않는 텍스트는
//! 모두 지우고 덮어쓴다). 앱은 작업 프로세스가 분류·중복 제거·검증까지 한다.
//!
//! 사용: cargo run --release -p pdf_ocr --example import_hocr -- <입력.pdf> <입력.hocr> <출력.pdf>

use lopdf::Document;
use pdf_ocr::geometry::PageFrame;
use pdf_ocr::remove::Applied;

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let (input, hocr, output) = (args.next().expect("PDF"), args.next().expect("hOCR"), args.next().expect("출력"));
    let mut doc = Document::load(&input)?;
    let compact = pdf_ocr::save::uses_object_streams(&doc);
    let (pages, report) = pdf_ocr::hocr::parse(&std::fs::read_to_string(&hocr)?)?;
    println!("hOCR {}쪽, {report:?}", pages.len());
    let mut applied = Applied::default();
    pdf_ocr::insert::strip_own_layers(&mut doc, None, &mut applied)?;
    let plan = pdf_ocr::remove::plan(&doc);
    pdf_ocr::remove::apply(&mut doc, &plan, &mut applied)?;
    let page_ids: Vec<_> = doc.get_pages().into_values().collect();
    let mut targets = Vec::new();
    for (i, (page, &page_id)) in pages.iter().zip(&page_ids).enumerate() {
        let frame = PageFrame::from_page(&doc, page_id)?;
        match pdf_ocr::import::layer_lines(page, &frame) {
            Ok(lines) => targets.push((i, page_id, frame, lines)),
            Err(e) => println!("  p.{}: {e}", i + 1),
        }
    }
    let now = chrono::Local::now().fixed_offset();
    pdf_ocr::insert::insert_layers(&mut doc, &targets, &pdf_ocr::save::format_pdf_date(now), &mut applied)?;
    pdf_ocr::save::save_rewritten(&mut doc, output.as_ref(), now, compact)?;
    println!("넣은 페이지 {}", targets.len());
    Ok(())
}
