//! 진단용: PDF마다 사전 점검 결과와, 페이지별 해석 결과(보이지 않는 텍스트 표시 연산자 수,
//! 해석 실패 페이지)를 출력한다. 실제 파일에서 토크나이저·해석기가 버티는지 확인하는 용도.
//!
//! 사용: cargo run -p pdf_ocr --example survey -- <파일.pdf>...

use lopdf::{Document, ObjectId};
use pdf_ocr::content::interp::{interpret_page, Context, Problem, Source, Visitor};
use pdf_ocr::content::lexer::Operation;
use pdf_ocr::preflight::Preflight;
use std::time::Instant;

#[derive(Default)]
struct Counter {
    shows: usize,
    invisible: usize,
    in_forms: usize,
    problems: Vec<Problem>,
}

impl Visitor for Counter {
    fn operation(&mut self, c: &Context, _index: usize, op: &Operation) {
        if matches!(op.operator.as_slice(), b"Tj" | b"TJ" | b"'" | b"\"") {
            self.shows += 1;
            if c.state.text.render_mode == 3 {
                self.invisible += 1;
                if matches!(c.source, Source::Form(_)) {
                    self.in_forms += 1;
                }
            }
        }
    }
    fn problem(&mut self, _source: Source, problem: &Problem) {
        self.problems.push(problem.clone());
    }
}

fn main() {
    for path in std::env::args().skip(1) {
        let started = Instant::now();
        let raw = match std::fs::read(&path) {
            Ok(raw) => raw,
            Err(e) => {
                println!("{path}: 읽기 실패 {e}");
                continue;
            }
        };
        let doc = match Document::load_mem(&raw) {
            Ok(doc) => doc,
            Err(e) => {
                println!("{path}: lopdf 로드 실패 {e}");
                continue;
            }
        };
        let preflight = Preflight::inspect(&doc, &raw);
        let pages: Vec<ObjectId> = doc.get_pages().into_values().collect();
        let (mut shows, mut invisible, mut in_forms, mut failed, mut form_problems) = (0, 0, 0, Vec::new(), 0);
        for (i, page) in pages.iter().enumerate() {
            let mut counter = Counter::default();
            match interpret_page(&doc, *page, &mut counter) {
                Ok(()) => {
                    shows += counter.shows;
                    invisible += counter.invisible;
                    in_forms += counter.in_forms;
                    form_problems += counter.problems.len();
                }
                Err(problem) => failed.push((i + 1, problem)),
            }
        }
        println!(
            "{path}\n  {preflight:?}\n  표시 연산자 {shows}, Tr3 {invisible}(Form 안 {in_forms}), 해석 실패 페이지 {}, Form 문제 {form_problems}, {:.2}s",
            failed.len(),
            started.elapsed().as_secs_f64()
        );
        for (page, problem) in failed.iter().take(5) {
            println!("    p.{page}: {problem:?}");
        }
    }
}
