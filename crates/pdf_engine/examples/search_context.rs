//! 검색 결과 문맥 진단 — 실제 PDF에서 검색어를 찾아 "앞 문맥 [일치] 뒤 문맥"을 출력한다.
//! 결과 사각형으로 되짚은 문자 범위가 검색어 자리와 정확히 맞는지(이웃 글자가 섞이거나
//! 잘리지 않는지) 눈으로 확인하는 용도.
//!
//! 사용법: cargo run --release --example search_context -p pdf_engine -- <pdfium_dylib> <pdf> <검색어> [최대 출력 수]

use pdf_engine::PdfEngine;
use std::path::PathBuf;
use std::time::Instant;

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let lib_path = PathBuf::from(args.next().expect("1: pdfium dylib 경로"));
    let pdf_path = PathBuf::from(args.next().expect("2: PDF 경로"));
    let query = args.next().expect("3: 검색어");
    let limit: usize = args.next().map(|s| s.parse()).transpose()?.unwrap_or(12);

    let engine = PdfEngine::new_with_library_path(&lib_path)?;
    let document = engine.open_document(&pdf_path)?;

    let started = Instant::now();
    let matches = pdf_engine::search::search_document(&document, &query);
    println!(
        "'{query}' — {}건, 문서 전체 {:.0?} (문맥 추출 포함)",
        matches.len(),
        started.elapsed()
    );

    let mut mismatched = 0;
    for m in &matches {
        let same = m
            .matched_text
            .chars()
            .filter(|c| !c.is_whitespace())
            .flat_map(char::to_lowercase)
            .eq(query.chars().filter(|c| !c.is_whitespace()).flat_map(char::to_lowercase));
        if !same {
            mismatched += 1;
        }
    }
    println!("일치 문자열이 검색어와 다른 결과: {mismatched}건\n");

    for m in matches.iter().take(limit) {
        println!("p{:<4} {}[{}]{}", m.page, m.context_before, m.matched_text, m.context_after);
    }
    Ok(())
}
