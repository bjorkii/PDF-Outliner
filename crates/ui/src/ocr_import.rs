//! OCR 가져오기 작업(작업 프로세스 쪽, 설계 문서 6장) — `ocr_worker`가 부른다.
//!
//! 분석([`analyze`]): hOCR을 읽고, PDF 페이지마다 분류 신호(디지털 텍스트, 기존 보이지 않는 텍스트,
//! 앱이 넣은 레이어, 큰 이미지 비율, 디지털 텍스트 손상 의심)를 모은다. 파일은 쓰지 않는다. 페이지
//! 대응(시작 페이지)과 페이지별 삽입·덮어쓰기 결정은 UI가 이 결과로 정해 [`ImportJob`]에 담는다.
//!
//! 실행([`run`]): hOCR 좌표를 페이지 좌표로 바꾸고(가로세로 비율이 맞지 않는 페이지는 건너뜀),
//! 디지털 텍스트와 겹치는 단어를 빼고(문자 단위 중복 제거), 덮어쓸 페이지의 기존 보이지 않는
//! 텍스트를 지운 뒤(삭제 기능과 같은 규칙) 텍스트 레이어를 넣고 전체 재작성한다. 그 뒤 페이지마다
//! 원본과 화면·보이는 텍스트를 비교하고 넣은 글자가 그대로 추출되는지 확인해, 실패한 페이지는 삭제와
//! 삽입을 함께 되돌린다.

use crate::ocr_worker::{open_error_message, open_for_edit, verify_pages, Event};
use pdf_engine::{text_layer, PdfEngine};
use pdf_ocr::geometry::PageFrame;
use pdf_ocr::hocr::{parse, HocrPage};
use pdf_ocr::import::{classify_page, dedupe, layer_lines, looks_damaged, only_in_margins, DigitalChar, PageClass, PageSignals};
use pdf_ocr::lopdf::{Document, ObjectId};
use pdf_ocr::remove::{Applied, PageStatus};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use unicode_normalization::UnicodeNormalization;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PageKind {
    NoText,
    ExistingOcrOnly,
    ScanWithExtras,
    Digital,
    Undetermined,
}

impl From<PageClass> for PageKind {
    fn from(c: PageClass) -> Self {
        match c {
            PageClass::NoText => PageKind::NoText,
            PageClass::ExistingOcrOnly => PageKind::ExistingOcrOnly,
            PageClass::ScanWithExtras => PageKind::ScanWithExtras,
            PageClass::Digital => PageKind::Digital,
            PageClass::Undetermined => PageKind::Undetermined,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PdfPageInfo {
    pub kind: PageKind,
    pub digital_chars: usize,
    /// 보이지 않는 텍스트(다른 도구의 OCR 등)가 있음.
    pub existing_ocr: bool,
    /// 이 앱이 넣은 레이어가 있음.
    pub own_layer: bool,
    pub damaged_digital: bool,
    pub image_coverage: f64,
    /// 표시 프레임 크기(pt).
    pub size: (f64, f64),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HocrPageInfo {
    /// 픽셀 크기.
    pub size: (f64, f64),
    pub words: usize,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ImportAnalysis {
    pub pdf_pages: Vec<PdfPageInfo>,
    /// 대응 순서대로 정렬된 hOCR 페이지.
    pub hocr_pages: Vec<HocrPageInfo>,
    /// hOCR 페이지 순서를 `ppageno`로 정했는지(아니면 파일·등장 순서).
    pub ordered_by_ppageno: bool,
    pub dropped_items: usize,
    pub empty_words: usize,
    pub signed: bool,
    pub tagged: bool,
    pub pdfa: Option<String>,
    pub incremental_updates: usize,
    pub linearized: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportJob {
    pub pdf: PathBuf,
    pub hocr_files: Vec<PathBuf>,
    pub temp_output: PathBuf,
    /// hOCR 첫 페이지를 대응시킬 PDF 페이지(1부터).
    pub start_page: usize,
    /// 넣을 PDF 페이지(0부터).
    pub insert_pages: Vec<usize>,
    /// 넣기 전에 기존 보이지 않는 텍스트를 지울 페이지(0부터, `insert_pages`의 부분집합).
    pub overwrite_pages: Vec<usize>,
    pub dedupe: bool,
}

/// 검증에서 문제가 된 자리 — 리포트의 "보기"가 뷰어에 표시한다.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProblemMark {
    /// 1부터 센 페이지 번호.
    pub page: usize,
    /// 사용자 공간 `[left, bottom, right, top]`.
    pub rects: Vec<[f64; 4]>,
    /// 그 자리의 단어(hOCR 텍스트).
    pub words: Vec<String>,
    /// 무엇이 문제인지(리포트 문장).
    pub note: String,
    /// 이 문제로 페이지를 되돌렸는지(아니면 확인 권장).
    pub rolled_back: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ImportReport {
    pub pages_inserted: usize,
    pub words_inserted: usize,
    pub pages_overwritten: usize,
    /// (페이지 번호, 이유) — 넣지 못한 페이지(비율 불일치, 넣을 단어 없음 등).
    pub skipped: Vec<(usize, String)>,
    pub dedupe_same: usize,
    pub dedupe_conflicts: usize,
    pub dedupe_kept_over_damaged: usize,
    /// 내용이 달랐던 예 (페이지, OCR, 디지털).
    pub dedupe_samples: Vec<(usize, String, String)>,
    pub rolled_back: Vec<(usize, String)>,
    /// 넣은 글자가 기대대로 추출되지 않은 자리(되돌린 페이지 + 확인 권장).
    pub marks: Vec<ProblemMark>,
    pub also_reverted: Vec<usize>,
    pub size_before: u64,
    pub size_after: u64,
    pub nothing_to_do: bool,
}

/// hOCR 파일들을 읽어 대응 순서의 페이지 목록으로. (페이지들, ppageno로 정렬했는지, 버린 항목, 빈 단어)
fn load_hocr(files: &[PathBuf]) -> anyhow::Result<(Vec<HocrPage>, bool, usize, usize)> {
    let mut pages = Vec::new();
    let (mut dropped, mut empty) = (0, 0);
    for file in files {
        let bytes = std::fs::read(file).map_err(|e| anyhow::anyhow!("hOCR 파일을 읽을 수 없음({}): {e}", file.display()))?;
        let html = String::from_utf8_lossy(&bytes);
        let (mut parsed, report) = parse(&html).map_err(|e| anyhow::anyhow!("{}: {e}", file.display()))?;
        dropped += report.dropped_items;
        empty += report.empty_words;
        pages.append(&mut parsed);
    }
    // 모든 페이지에 서로 다른 ppageno가 있으면 그 순서를, 아니면 파일·등장 순서를 쓴다
    // (Tesseract의 페이지별 파일은 모두 ppageno 0이라 순서 정보가 없다).
    let numbers: Option<Vec<usize>> = pages.iter().map(|p| p.ppageno).collect();
    let by_ppageno = numbers.is_some_and(|n| n.iter().collect::<BTreeSet<_>>().len() == n.len() && pages.len() > 1);
    if by_ppageno {
        pages.sort_by_key(|p| p.ppageno);
    }
    Ok((pages, by_ppageno, dropped, empty))
}

/// 넣은 레이어가 그대로 추출되는지 페이지마다 확인한다(`skip` 페이지 제외). 빠진 글자와 그 자리를
/// 찾고(`pdf_ocr::import::missing_positions`), 빠진 글자가 모두 "같은 글자가 겹친 자리"로 설명되면
/// 확인 권장, 아니면 되돌릴 문제로 표시한다.
///
/// 겹친 같은 글자: pdfium은 같은 글자가 가까운 자리에 두 번 그려지면(굵게 보이려고 겹쳐 찍는
/// 경우로 보고) 하나로 합쳐 추출한다. 글자 단위로 쪼개진 hOCR에서 OCR 엔진이 같은 글자의 상자를
/// 겹쳐 낸 경우가 실제로 있었다(2026-09-22, 122·123·145쪽에서 한 글자씩). 파일에는 두 글자 모두
/// 들어 있고 검색에도 영향이 거의 없어 페이지 전체를 되돌리지 않는다.
fn check_layers(
    engine: PdfEngine,
    result: &Path,
    targets: &[(usize, ObjectId, PageFrame, Vec<pdf_ocr::insert::LayerLine>)],
    skip: &BTreeSet<usize>,
) -> Vec<ProblemMark> {
    let Ok(document) = engine.open_document(result) else { return Vec::new() };
    let mut marks = Vec::new();
    for (index, _, frame, lines) in targets.iter().filter(|t| !skip.contains(&t.0)) {
        let Ok(page) = document.pages().get(*index as i32) else { continue };
        let Ok(extracted) = pdf_engine::verify::chars_in_font(&page, pdf_ocr::glyphless::FONT_NAME) else { continue };
        // 넣은 순서의 글자와 그 글자 칸(표시 프레임) — Tz로 단어 폭을 글자 수만큼 고르게 나눴다.
        let mut expected: Vec<(char, usize, pdf_ocr::layout::DRect)> = Vec::new();
        let words: Vec<(&pdf_ocr::insert::LayerWord, f64)> =
            lines.iter().flat_map(|l| l.words.iter().map(move |w| (w, l.angle))).collect();
        for (word_index, (word, angle)) in words.iter().enumerate() {
            let chars: Vec<char> = word.text.nfc().collect();
            for (i, c) in chars.iter().enumerate() {
                if !c.is_whitespace() {
                    let cell = char_cell(&word.rect, i, chars.len(), *angle);
                    expected.push((pdf_engine::verify::fold_mirrored(*c), word_index, cell));
                }
            }
        }
        let center = |r: &pdf_ocr::layout::DRect| ((r.x0 + r.x1) / 2.0, (r.y0 + r.y1) / 2.0);
        let wanted: Vec<(char, (f64, f64))> = expected.iter().map(|e| (e.0, center(&e.2))).collect();
        let got: Vec<(char, (f64, f64))> =
            extracted.iter().map(|(c, b)| (*c, center(&frame.user_rect_to_display(*b)))).collect();
        let missing = pdf_ocr::import::missing_positions(&wanted, &got);
        if missing.is_empty() {
            continue;
        }
        let explained = missing.iter().all(|&m| {
            let (c, _, cell) = &expected[m];
            expected.iter().enumerate().any(|(k, (other, _, other_cell))| k != m && other == c && close_duplicate(cell, other_cell))
        });
        let mut word_ids: Vec<usize> = missing.iter().map(|&m| expected[m].1).collect();
        word_ids.dedup();
        let to_user = |r: &pdf_ocr::layout::DRect| {
            let (a, b) = (frame.display_to_user(r.x0, r.y0), frame.display_to_user(r.x1, r.y1));
            [a.0.min(b.0), a.1.min(b.1), a.0.max(b.0), a.1.max(b.1)]
        };
        let missing_text: String = missing.iter().map(|&m| expected[m].0).collect();
        marks.push(ProblemMark {
            page: index + 1,
            rects: word_ids.iter().map(|&w| to_user(&words[w].0.rect)).collect(),
            words: word_ids.iter().map(|&w| words[w].0.text.clone()).collect(),
            note: if explained {
                format!("같은 글자가 겹친 자리 {}곳 — 뷰어에서 하나로 합쳐 추출됨(\"{missing_text}\"), 페이지는 유지", missing.len())
            } else {
                format!("넣은 글자 {}자가 추출되지 않음(\"{missing_text}\") — 페이지를 원래대로 되돌림", missing.len())
            },
            rolled_back: !explained,
        });
    }
    marks
}

/// 단어 칸을 읽는 방향으로 `n`등분한 `i`번째 글자 칸.
fn char_cell(r: &pdf_ocr::layout::DRect, i: usize, n: usize, angle: f64) -> pdf_ocr::layout::DRect {
    let (a, b) = (i as f64 / n as f64, (i + 1) as f64 / n as f64);
    let quarter = (angle.rem_euclid(360.0) / 90.0).round() as i32 % 4;
    let (w, h) = (r.width(), r.height());
    match quarter {
        1 => pdf_ocr::layout::DRect { y0: r.y1 - b * h, y1: r.y1 - a * h, ..*r }, // 아래→위
        2 => pdf_ocr::layout::DRect { x0: r.x1 - b * w, x1: r.x1 - a * w, ..*r }, // 오른쪽→왼쪽
        3 => pdf_ocr::layout::DRect { y0: r.y0 + a * h, y1: r.y0 + b * h, ..*r }, // 위→아래
        _ => pdf_ocr::layout::DRect { x0: r.x0 + a * w, x1: r.x0 + b * w, ..*r },
    }
}

/// 같은 글자 두 칸이 pdfium이 하나로 합칠 만큼 가까운지 — 칸 중심 거리가 글자 크기(칸의 긴 변)의
/// 0.6배 이하. pdfium은 박스 겹침이 아니라 글자 크기 대비 거리로 겹쳐 찍은 글자를 판정한다(맞닿은
/// '1' 두 개도 합쳐졌다, 122쪽).
fn close_duplicate(a: &pdf_ocr::layout::DRect, b: &pdf_ocr::layout::DRect) -> bool {
    let (ax, ay) = ((a.x0 + a.x1) / 2.0, (a.y0 + a.y1) / 2.0);
    let (bx, by) = ((b.x0 + b.x1) / 2.0, (b.y0 + b.y1) / 2.0);
    let size = a.width().max(a.height()).max(b.width().max(b.height()));
    (ax - bx).hypot(ay - by) <= size * 0.6
}

/// 페이지의 보이는 디지털 글자(표시 프레임 좌표).
fn digital_chars(page: &pdfium_render::prelude::PdfPage, frame: &PageFrame) -> Vec<DigitalChar> {
    text_layer::page_chars(page)
        .unwrap_or_default()
        .into_iter()
        .filter(|c| !c.invisible && !c.generated && !c.ch.is_whitespace())
        .map(|c| DigitalChar { ch: c.ch, rect: frame.user_rect_to_display(c.bounds) })
        .collect()
}

pub fn analyze(
    engine: PdfEngine,
    pdf: &Path,
    hocr_files: &[PathBuf],
    emit: &mut dyn FnMut(Event),
) -> anyhow::Result<ImportAnalysis> {
    emit(Event::Stage("hOCR 읽는 중".to_string()));
    let (hocr, ordered_by_ppageno, dropped_items, empty_words) = load_hocr(hocr_files)?;
    emit(Event::Stage("페이지 분석 중".to_string()));
    let (mut doc, preflight, _) = open_for_edit(engine, pdf)?;
    let own: BTreeSet<usize> = pdf_ocr::insert::pages_with_own_layer(&doc).into_iter().collect();
    pdf_ocr::insert::strip_own_layers(&mut doc, None, &mut Applied::default())?;
    let removal = pdf_ocr::remove::plan(&doc);
    let document = engine.open_document(pdf).map_err(open_error_message)?;
    let page_ids: Vec<ObjectId> = doc.get_pages().into_values().collect();

    let mut pdf_pages = Vec::with_capacity(page_ids.len());
    for (index, &page_id) in page_ids.iter().enumerate() {
        let frame = PageFrame::from_page(&doc, page_id)?;
        let size = frame.display_size();
        let chars = document.pages().get(index as i32).map(|p| digital_chars(&p, &frame)).unwrap_or_default();
        let plan_page = &removal.pages[index];
        let existing_ocr = plan_page.counts.removed() > 0;
        let damaged = looks_damaged(&chars);
        let coverage = pdf_ocr::import::image_coverage(&doc, page_id, &frame);
        let signals = PageSignals {
            digital_chars: chars.len(),
            digital_only_in_margins: only_in_margins(&chars, size),
            has_invisible_text: existing_ocr || own.contains(&index),
            image_coverage: coverage,
            damaged_digital: damaged,
        };
        pdf_pages.push(PdfPageInfo {
            kind: classify_page(&signals).into(),
            digital_chars: chars.len(),
            existing_ocr,
            own_layer: own.contains(&index),
            damaged_digital: damaged,
            image_coverage: coverage,
            size,
        });
        emit(Event::Progress { done: index + 1, total: page_ids.len() });
    }
    Ok(ImportAnalysis {
        pdf_pages,
        hocr_pages: hocr
            .iter()
            .map(|p| HocrPageInfo {
                size: (p.bbox.width(), p.bbox.height()),
                words: p.lines.iter().map(|l| l.words.len()).sum(),
            })
            .collect(),
        ordered_by_ppageno,
        dropped_items,
        empty_words,
        signed: preflight.signed,
        tagged: preflight.tagged,
        pdfa: preflight.pdfa,
        incremental_updates: preflight.incremental_updates,
        linearized: preflight.linearized,
    })
}

pub fn run(engine: PdfEngine, job: &ImportJob, emit: &mut dyn FnMut(Event)) -> anyhow::Result<ImportReport> {
    use anyhow::bail;
    emit(Event::Stage("hOCR 읽는 중".to_string()));
    let (hocr, ..) = load_hocr(&job.hocr_files)?;
    let (mut doc, _, compact) = open_for_edit(engine, &job.pdf)?;
    let size_before = std::fs::metadata(&job.pdf).map(|m| m.len()).unwrap_or(0);
    let mut report = ImportReport { size_before, ..Default::default() };
    let page_ids: Vec<ObjectId> = doc.get_pages().into_values().collect();
    let insert: BTreeSet<usize> = job.insert_pages.iter().copied().collect();
    let overwrite: BTreeSet<usize> = job.overwrite_pages.iter().copied().filter(|p| insert.contains(p)).collect();

    // 1. 넣을 줄 준비(원본 문서 기준 — 중복 제거는 지우기 전의 디지털 텍스트와 비교한다).
    emit(Event::Stage("단어 배치·중복 제거 중".to_string()));
    let document = engine.open_document(&job.pdf).map_err(open_error_message)?;
    let mut targets = Vec::new();
    for (k, page) in hocr.iter().enumerate() {
        let Some(index) = (job.start_page + k).checked_sub(1).filter(|i| *i < page_ids.len()) else { continue };
        if !insert.contains(&index) {
            continue;
        }
        let frame = PageFrame::from_page(&doc, page_ids[index])?;
        let mut lines = match layer_lines(page, &frame) {
            Ok(lines) => lines,
            Err(err) => {
                report.skipped.push((index + 1, format!("{err:#}")));
                continue;
            }
        };
        if job.dedupe {
            let chars = document.pages().get(index as i32).map(|p| digital_chars(&p, &frame)).unwrap_or_default();
            let stats = dedupe(&mut lines, &chars, looks_damaged(&chars));
            report.dedupe_same += stats.same;
            report.dedupe_conflicts += stats.conflicts;
            report.dedupe_kept_over_damaged += stats.kept_over_damaged;
            for (ocr, digital) in stats.samples {
                if report.dedupe_samples.len() < 10 {
                    report.dedupe_samples.push((index + 1, ocr, digital));
                }
            }
        }
        if lines.is_empty() {
            report.skipped.push((index + 1, "넣을 단어가 없음".to_string()));
            continue;
        }
        targets.push((index, page_ids[index], frame, lines));
        emit(Event::Progress { done: k + 1, total: hocr.len() });
    }
    drop(document);
    let target_pages: BTreeSet<usize> = targets.iter().map(|t| t.0).collect();
    if targets.is_empty() {
        report.nothing_to_do = true;
        return Ok(report);
    }

    // 2. 덮어쓸 페이지의 기존 보이지 않는 텍스트 삭제(넣을 페이지만).
    emit(Event::Stage("기존 OCR 삭제·삽입 중".to_string()));
    let mut applied = Applied::default();
    let overwrite: BTreeSet<usize> = overwrite.intersection(&target_pages).copied().collect();
    if !overwrite.is_empty() {
        pdf_ocr::insert::strip_own_layers(&mut doc, Some(&overwrite), &mut applied)?;
        let plan = pdf_ocr::remove::plan_for(&doc, Some(&overwrite));
        for page in plan.pages.iter().filter(|p| overwrite.contains(&(p.number - 1))) {
            if let PageStatus::Skipped(reason) = &page.status {
                report.skipped.push((page.number, format!("기존 OCR을 지울 수 없어 건너뜀: {reason}")));
            }
        }
        pdf_ocr::remove::apply(&mut doc, &plan, &mut applied)?;
        // 기존 OCR을 못 지운 페이지에는 넣지 않는다(텍스트 중복).
        let blocked: BTreeSet<usize> = plan
            .pages
            .iter()
            .filter(|p| matches!(p.status, PageStatus::Skipped(_)) && overwrite.contains(&(p.number - 1)))
            .map(|p| p.number - 1)
            .collect();
        targets.retain(|t| !blocked.contains(&t.0));
        report.pages_overwritten = overwrite.len() - blocked.len();
    }

    // 3. 삽입과 저장.
    let now = chrono::Local::now().fixed_offset();
    pdf_ocr::insert::insert_layers(&mut doc, &targets, &pdf_ocr::save::format_pdf_date(now), &mut applied)?;
    emit(Event::Stage("저장 중".to_string()));
    pdf_ocr::save::save_rewritten(&mut doc, &job.temp_output, now, compact)?;

    // 4. 검증과 되돌리기.
    emit(Event::Stage("검증 중(원본과 화면·텍스트 비교)".to_string()));
    let changed: Vec<usize> = applied.changed_pages().into_iter().collect();
    let mut failures = verify_pages(engine, &job.pdf, &job.temp_output, &changed, &HashMap::new(), emit)?;
    let already: BTreeSet<usize> = failures.iter().map(|(i, _)| *i).collect();
    for mark in check_layers(engine, &job.temp_output, &targets, &already) {
        if mark.rolled_back {
            failures.push((mark.page - 1, mark.note.clone()));
        }
        report.marks.push(mark);
    }
    let mut reverted = BTreeSet::new();
    if !failures.is_empty() {
        let failed: BTreeSet<usize> = failures.iter().map(|(i, _)| *i).collect();
        let also = applied.rollback(&mut doc, &failed);
        report.rolled_back = failures.iter().map(|(i, reason)| (i + 1, reason.clone())).collect();
        report.also_reverted = also.iter().map(|i| i + 1).collect();
        reverted.extend(failed);
        reverted.extend(also);
        emit(Event::Stage("되돌린 페이지 반영해 다시 저장 중".to_string()));
        pdf_ocr::save::save_rewritten(&mut doc, &job.temp_output, now, compact)?;
        let again: Vec<usize> = reverted.iter().copied().collect();
        let still = verify_pages(engine, &job.pdf, &job.temp_output, &again, &HashMap::new(), emit)?;
        if let Some((page, reason)) = still.first() {
            bail!("되돌린 {}쪽이 원본과 같지 않습니다({reason}). 원본을 건드리지 않았습니다.", page + 1);
        }
    }
    let kept: Vec<_> = targets.iter().filter(|t| !reverted.contains(&t.0)).collect();
    report.pages_inserted = kept.len();
    report.words_inserted = kept.iter().map(|t| t.3.iter().map(|l| l.words.len()).sum::<usize>()).sum();
    report.size_after = std::fs::metadata(&job.temp_output).map(|m| m.len()).unwrap_or(0);
    // 결과 파일 구조 확인.
    let reopened = Document::load(&job.temp_output).map_err(|e| anyhow::anyhow!("결과 파일을 다시 읽지 못함: {e}"))?;
    if reopened.get_pages().len() != page_ids.len() {
        bail!("결과 파일의 페이지 수가 원본과 다릅니다. 원본을 건드리지 않았습니다.");
    }
    Ok(report)
}
