//! 진단용: OCR 전체 삭제(표준 모드)를 적용해 새 파일로 저장한다(검증 없이 — 앱은 작업 프로세스가
//! 렌더·텍스트 비교까지 한다).
//!
//! 사용: cargo run --release -p pdf_ocr --example remove_ocr -- <입력.pdf> <출력.pdf>

use lopdf::Document;
use pdf_ocr::remove::{apply, plan, PageStatus};
use pdf_ocr::save::save_rewritten;
use std::time::Instant;

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let input = args.next().expect("입력 PDF");
    let output = args.next().expect("출력 PDF");
    let started = Instant::now();
    let mut doc = Document::load(&input)?;
    let compact = pdf_ocr::save::uses_object_streams(&doc);
    let plan = plan(&doc);
    let totals = plan.totals();
    let skipped: Vec<_> = plan
        .pages
        .iter()
        .filter_map(|p| match &p.status {
            PageStatus::Skipped(reason) => Some((p.number, reason.clone())),
            _ => None,
        })
        .collect();
    println!("계획: {totals:?}, 건너뜀 {}쪽 {:?}", skipped.len(), skipped.iter().take(5).collect::<Vec<_>>());
    for page in plan.pages.iter().filter(|p| !p.notes.is_empty()).take(5) {
        println!("  p.{} {:?}", page.number, page.notes);
    }
    if !plan.has_changes() {
        println!("지울 텍스트 없음 — 저장하지 않음");
        return Ok(());
    }
    apply(&mut doc, &plan)?;
    save_rewritten(&mut doc, output.as_ref(), chrono::Local::now().fixed_offset(), compact)?;
    println!(
        "{:.2}s, 크기 {} → {} bytes",
        started.elapsed().as_secs_f64(),
        std::fs::metadata(&input)?.len(),
        std::fs::metadata(&output)?.len()
    );
    Ok(())
}
