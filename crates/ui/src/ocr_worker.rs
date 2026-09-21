//! OCR 작업 전용 보조 프로세스 — 같은 실행 파일을 `--ocr-worker`로 띄워 작업 하나를 맡긴다.
//!
//! 프로세스로 분리하는 이유(`render_worker`와 같은 이유에 더해):
//! - pdfium은 스레드 안전하지 않아 UI 스레드 밖의 스레드에서 부를 수 없다.
//! - 수천 쪽 문서의 텍스트 추출·구조 편집은 오래 걸리고 메모리를 많이 쓴다. 따로 두면 UI가 멈추지
//!   않고, 작업이 죽어도 앱은 살아 있다.
//! - 취소는 프로세스를 끝내는 것으로 충분하다. 결과는 임시 파일에만 쓰고 끝에서 교체하므로,
//!   도중에 끝내도 대상 파일은 그대로다(남은 임시 파일은 UI 쪽이 지운다).
//!
//! 한 프로세스가 작업 하나만 처리한다. 통신은 렌더 워커와 달리 JSON 줄 단위다 — 비트맵 같은 큰
//! 데이터가 오가지 않아 이진 프레임이 필요 없다. 요청은 stdin에 JSON 하나를 쓰고 닫는다. 응답은
//! stdout에 이벤트마다 한 줄씩 쓴다. **작업 프로세스 코드 경로에서 stdout에 다른 것을 쓰면
//! 프로토콜이 깨진다**(로그는 stderr로).

use pdf_engine::{text_layer, PdfEngine};
use pdf_ocr::geometry::{PageFrame, Rect};
use pdf_ocr::hocr::HocrWriter;
use pdf_ocr::layout::{build_lines, InputChar, LayoutOptions, Line, TextSource};
use pdf_ocr::txt::{TxtOptions, TxtWriter};
use serde::{Deserialize, Serialize};
use std::io::{self, BufRead, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use unicode_normalization::UnicodeNormalization;

/// 이 인자로 실행되면 창을 만들지 않고 OCR 작업 프로세스로 동작한다(main.rs).
pub const WORKER_FLAG: &str = "--ocr-worker";

/// hOCR 좌표 환산 해상도(가상 DPI).
pub const HOCR_DPI: f64 = 300.0;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Job {
    Export(ExportJob),
    /// OCR 전체 삭제 전 분석(사전 점검 + 삭제 계획). 파일을 쓰지 않는다.
    AnalyzeRemoval { pdf: PathBuf },
    /// OCR 전체 삭제 — 결과를 `temp_output`에 쓰고 검증까지 한다. 원본 교체는 UI가 한다.
    Remove { pdf: PathBuf, temp_output: PathBuf },
}

/// 삭제 대상 개수(형태별 표시 연산자 수).
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct RemovalCounts {
    pub invisible_mode: usize,
    pub zero_size: usize,
    pub transparent: usize,
    pub clip_only_kept: usize,
}

impl RemovalCounts {
    pub fn removed(&self) -> usize {
        self.invisible_mode + self.zero_size + self.transparent
    }
}

impl From<pdf_ocr::remove::KindCounts> for RemovalCounts {
    fn from(c: pdf_ocr::remove::KindCounts) -> Self {
        Self {
            invisible_mode: c.invisible_mode,
            zero_size: c.zero_size,
            transparent: c.transparent,
            clip_only_kept: c.clip_only_kept,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RemovalAnalysis {
    pub pages: usize,
    /// 지울 것이 있는 페이지 수.
    pub pages_with_hidden_text: usize,
    pub counts: RemovalCounts,
    /// (페이지 번호, 이유) — 건드리지 않을 페이지.
    pub skipped: Vec<(usize, String)>,
    pub notes: Vec<(usize, String)>,
    pub signed: bool,
    pub tagged: bool,
    pub pdfa: Option<String>,
    pub incremental_updates: usize,
    pub linearized: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RemovalReport {
    pub analysis: RemovalAnalysis,
    /// 실제로 바뀐 페이지 수(되돌린 페이지 제외).
    pub pages_changed: usize,
    /// (페이지 번호, 이유) — 검증에서 실패해 원래대로 되돌린 페이지.
    pub rolled_back: Vec<(usize, String)>,
    /// 공유 Form을 되돌리느라 함께 원래대로 돌아간 페이지.
    pub also_reverted: Vec<usize>,
    pub size_before: u64,
    pub size_after: u64,
    /// 결과 파일을 만들지 않음(지울 것이 없음).
    pub nothing_to_do: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExportFormat {
    Hocr,
    Txt,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportJob {
    pub pdf: PathBuf,
    pub output: PathBuf,
    /// 여기에 먼저 쓰고 끝나면 `output`으로 이름을 바꾼다.
    pub temp_output: PathBuf,
    pub format: ExportFormat,
    pub invisible_only: bool,
    pub txt_crlf: bool,
    pub txt_form_feed: bool,
    pub txt_page_labels: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExportReport {
    pub pages: usize,
    pub pages_with_text: usize,
    pub words: usize,
    /// 높이가 비정상적으로 커서 보정한 글자 수(`pdf_ocr::layout` 문서).
    pub clamped_chars: usize,
    /// (1부터 센 페이지 번호, 이유) — 빈 페이지로 기록했다.
    pub failed_pages: Vec<(usize, String)>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Event {
    /// 진행 중인 단계 이름(예: "검증 중").
    Stage(String),
    Progress { done: usize, total: usize },
    ExportDone(ExportReport),
    RemovalAnalysis(RemovalAnalysis),
    RemovalDone(RemovalReport),
    Failed(String),
}

// ---------------------------------------------------------------- UI 쪽

#[allow(clippy::large_enum_variant)] // 프레임당 몇 번만 오가는 값이라 크기가 문제 되지 않는다
pub enum WorkerPoll {
    Event(Event),
    Empty,
    /// 작업 프로세스의 출력이 끝남.
    Closed,
}

/// UI 프로세스가 들고 있는 작업 프로세스 핸들. 떨어뜨리면(drop) 프로세스를 끝낸다.
pub struct WorkerHandle {
    child: Child,
    events: Receiver<Event>,
}

impl WorkerHandle {
    pub fn spawn(job: &Job, ctx: &egui::Context) -> io::Result<Self> {
        let mut child = Command::new(std::env::current_exe()?)
            .arg(WORKER_FLAG)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()?;
        let mut stdin = child.stdin.take().ok_or_else(|| io::Error::other("stdin 파이프 없음"))?;
        let stdout = child.stdout.take().ok_or_else(|| io::Error::other("stdout 파이프 없음"))?;
        serde_json::to_writer(&mut stdin, job).map_err(io::Error::other)?;
        drop(stdin); // EOF — 작업 프로세스가 요청을 끝까지 읽는다.

        let (sender, events) = mpsc::channel();
        let ctx = ctx.clone();
        std::thread::Builder::new().name("ocr-worker-reader".to_string()).spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                match serde_json::from_str::<Event>(&line) {
                    Ok(event) => {
                        if sender.send(event).is_err() {
                            break;
                        }
                        ctx.request_repaint();
                    }
                    Err(err) => eprintln!("ocr-worker 응답 해석 실패: {err}: {line}"),
                }
            }
            ctx.request_repaint(); // 종료 감지
        })?;
        Ok(Self { child, events })
    }

    /// 받은 이벤트 하나. 읽기 스레드는 작업 프로세스의 stdout이 닫혀야 끝나므로, `Closed`는
    /// 보낸 이벤트를 모두 받은 뒤에만 나온다.
    pub fn poll(&self) -> WorkerPoll {
        match self.events.try_recv() {
            Ok(event) => WorkerPoll::Event(event),
            Err(mpsc::TryRecvError::Empty) => WorkerPoll::Empty,
            Err(mpsc::TryRecvError::Disconnected) => WorkerPoll::Closed,
        }
    }

    pub fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for WorkerHandle {
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(None)) {
            self.kill();
        }
    }
}

// ---------------------------------------------------------------- 작업 프로세스 쪽

pub fn run_worker_process() -> i32 {
    let mut input = String::new();
    if let Err(err) = io::stdin().read_to_string(&mut input) {
        eprintln!("ocr-worker: 요청 읽기 실패: {err}");
        return 1;
    }
    let mut output = io::stdout().lock();
    let mut emit = |event: Event| {
        // 한 줄에 이벤트 하나. 실패하면(UI가 먼저 끝남) 할 수 있는 일이 없다.
        if serde_json::to_writer(&mut output, &event).is_ok() {
            let _ = output.write_all(b"\n");
            let _ = output.flush();
        }
    };
    let job: Job = match serde_json::from_str(&input) {
        Ok(job) => job,
        Err(err) => {
            emit(Event::Failed(format!("작업 요청 해석 실패: {err}")));
            return 1;
        }
    };
    let Some(engine) = crate::app::create_engine() else {
        emit(Event::Failed("pdfium 라이브러리를 찾지 못했습니다.".to_string()));
        return 2;
    };
    let result = match &job {
        Job::Export(export) => run_export(engine, export, &mut emit).map(Event::ExportDone),
        Job::AnalyzeRemoval { pdf } => analyze_removal(engine, pdf).map(|(analysis, ..)| Event::RemovalAnalysis(analysis)),
        Job::Remove { pdf, temp_output } => run_removal(engine, pdf, temp_output, &mut emit).map(Event::RemovalDone),
    };
    match result {
        Ok(event) => {
            emit(event);
            0
        }
        Err(err) => {
            match &job {
                Job::Export(export) => drop(std::fs::remove_file(&export.temp_output)),
                Job::Remove { temp_output, .. } => drop(std::fs::remove_file(temp_output)),
                Job::AnalyzeRemoval { .. } => {}
            }
            emit(Event::Failed(format!("{err:#}")));
            1
        }
    }
}

enum Output<W: Write> {
    Hocr(HocrWriter<W>),
    Txt(TxtWriter<W>),
}

fn run_export(engine: PdfEngine, job: &ExportJob, emit: &mut dyn FnMut(Event)) -> anyhow::Result<ExportReport> {
    use anyhow::Context;
    let document = engine.open_document(&job.pdf).map_err(open_error_message)?;
    let pages = document.pages();
    let total = pages.len() as usize;

    let file = std::fs::File::create(&job.temp_output)
        .with_context(|| format!("임시 파일을 만들 수 없음: {}", job.temp_output.display()))?;
    let writer = BufWriter::with_capacity(1 << 20, file);
    let mut output = match job.format {
        ExportFormat::Hocr => {
            let title = file_display_name(&job.pdf);
            let system = concat!("PDF-Outliner ", env!("PDF_OUTLINER_VERSION"));
            Output::Hocr(HocrWriter::new(writer, HOCR_DPI, &title, system)?)
        }
        ExportFormat::Txt => Output::Txt(TxtWriter::new(
            writer,
            TxtOptions { crlf: job.txt_crlf, form_feed: job.txt_form_feed, page_labels: job.txt_page_labels },
        )),
    };
    let options = LayoutOptions {
        source: if job.invisible_only { TextSource::InvisibleOnly } else { TextSource::All },
        keep_format_chars: false,
    };

    let mut report = ExportReport { pages: total, ..Default::default() };
    for (index, page) in pages.iter().enumerate() {
        let number = index + 1;
        let (size, lines, label) = match extract_page(&page, &options) {
            Ok((frame, lines, clamped)) => {
                report.clamped_chars += clamped;
                (frame.display_size(), lines, text_layer::page_label(&page))
            }
            Err(err) => {
                report.failed_pages.push((number, format!("{err:#}")));
                let size = (page.width().value as f64, page.height().value as f64);
                (size, Vec::new(), text_layer::page_label(&page))
            }
        };
        if !lines.is_empty() {
            report.pages_with_text += 1;
            report.words += lines.iter().map(|l| l.words.len()).sum::<usize>();
        }
        match &mut output {
            Output::Hocr(w) => w.page(size, &lines)?,
            Output::Txt(w) => w.page(number, label.as_deref(), &lines)?,
        }
        emit(Event::Progress { done: number, total });
    }

    let mut writer = match output {
        Output::Hocr(w) => w.finish()?,
        Output::Txt(w) => w.finish()?,
    };
    writer.flush()?;
    writer
        .into_inner()
        .map_err(|e| e.into_error())?
        .sync_all()
        .context("임시 파일 기록 실패")?;
    std::fs::rename(&job.temp_output, &job.output)
        .with_context(|| format!("결과 파일로 옮기지 못함: {}", job.output.display()))?;
    Ok(report)
}

/// 페이지 하나: 표시 프레임, 줄 목록, 높이를 보정한 글자 수.
fn extract_page(page: &pdfium_render::prelude::PdfPage, options: &LayoutOptions) -> anyhow::Result<(PageFrame, Vec<Line>, usize)> {
    let page_box = text_layer::page_box(page)?;
    let [llx, lly, urx, ury] = page_box.crop;
    // pdfium은 /UserUnit을 반영하지 않는다 — 좌표만 맞으면 되므로 1로 둔다(가져오기는 hOCR
    // 페이지 크기로 배율을 다시 구하므로 위치는 그대로 맞는다).
    let frame = PageFrame { crop: Rect { llx, lly, urx, ury }, rotate: page_box.rotate, user_unit: 1.0 };
    // /Rotate로 돌린 스캔은 회전 전 프레임에서 글자가 바로 서 있다 — 줄은 거기서 구성한다.
    let upright = frame.upright();
    let chars: Vec<InputChar> = text_layer::page_chars(page)?
        .into_iter()
        .map(|c| InputChar {
            ch: c.ch,
            rect: upright.user_rect_to_display(c.bounds),
            baseline: c.origin.map(|(x, y)| upright.user_to_display(x, y).1),
            generated: c.generated,
            invisible: c.invisible,
        })
        .collect();
    let (lines, stats) = build_lines(&chars, options);
    Ok((frame, frame.orient_lines(lines), stats.clamped_chars))
}

// ---------------------------------------------------------------- OCR 전체 삭제

use pdf_ocr::lopdf::Document;
use pdf_ocr::remove::{PageStatus, RemovalPlan};

/// 사전 점검 + 삭제 계획. 암호화·손상·페이지 수 불일치면 오류. (분석, lopdf 문서, 계획, 압축 저장 여부)
fn analyze_removal(engine: PdfEngine, pdf: &Path) -> anyhow::Result<(RemovalAnalysis, Document, RemovalPlan, bool)> {
    use anyhow::{bail, Context};
    let raw = std::fs::read(pdf).with_context(|| format!("파일을 읽을 수 없음: {}", pdf.display()))?;
    let doc = Document::load_mem(&raw).map_err(|e| anyhow::anyhow!("PDF 구조를 읽지 못했습니다(손상 가능): {e}"))?;
    let preflight = pdf_ocr::preflight::Preflight::inspect(&doc, &raw);
    drop(raw);
    if preflight.encrypted {
        bail!("암호화된 PDF는 아직 지원하지 않습니다. 다시 저장하면 암호화가 풀리거나 바뀔 수 있어서입니다.");
    }
    let pdfium_pages = engine.open_document(pdf).map_err(open_error_message)?.pages().len() as usize;
    if pdfium_pages != preflight.page_count {
        bail!(
            "PDF 구조가 손상된 것으로 보입니다(페이지 수 불일치: 화면 {pdfium_pages}쪽, 구조 {}쪽). 원본을 건드리지 않았습니다.",
            preflight.page_count
        );
    }
    let compact = pdf_ocr::save::uses_object_streams(&doc) && !preflight.pdfa.as_deref().is_some_and(|p| p.starts_with('1'));
    let plan = pdf_ocr::remove::plan(&doc);
    let mut analysis = RemovalAnalysis {
        pages: plan.pages.len(),
        counts: plan.totals().into(),
        signed: preflight.signed,
        tagged: preflight.tagged,
        pdfa: preflight.pdfa.clone(),
        incremental_updates: preflight.incremental_updates,
        linearized: preflight.linearized,
        ..Default::default()
    };
    for page in &plan.pages {
        match &page.status {
            PageStatus::Planned => analysis.pages_with_hidden_text += 1,
            PageStatus::Skipped(reason) => analysis.skipped.push((page.number, reason.clone())),
            PageStatus::Unchanged => {}
        }
        analysis.notes.extend(page.notes.iter().map(|n| (page.number, n.clone())));
    }
    Ok((analysis, doc, plan, compact))
}

fn run_removal(engine: PdfEngine, pdf: &Path, temp_output: &Path, emit: &mut dyn FnMut(Event)) -> anyhow::Result<RemovalReport> {
    use anyhow::{bail, Context};
    use std::collections::BTreeSet;
    emit(Event::Stage("분석 중".to_string()));
    let (analysis, mut doc, plan, compact) = analyze_removal(engine, pdf)?;
    let size_before = std::fs::metadata(pdf).map(|m| m.len()).unwrap_or(0);
    let mut report = RemovalReport { analysis, size_before, ..Default::default() };
    if !plan.has_changes() {
        report.nothing_to_do = true;
        return Ok(report);
    }

    emit(Event::Stage("삭제·저장 중".to_string()));
    let mut applied = pdf_ocr::remove::apply(&mut doc, &plan)?;
    let now = chrono::Local::now().fixed_offset();
    pdf_ocr::save::save_rewritten(&mut doc, temp_output, now, compact)?;

    let planned: Vec<usize> =
        plan.pages.iter().enumerate().filter(|(_, p)| p.status == PageStatus::Planned).map(|(i, _)| i).collect();
    emit(Event::Stage("검증 중(원본과 화면·텍스트 비교)".to_string()));
    let failures = verify_pages(engine, pdf, temp_output, &planned, emit)?;

    let mut reverted: BTreeSet<usize> = BTreeSet::new();
    if !failures.is_empty() {
        let failed: BTreeSet<usize> = failures.iter().map(|(i, _)| *i).collect();
        let also = applied.rollback(&mut doc, &failed);
        report.rolled_back = failures.iter().map(|(i, reason)| (i + 1, reason.clone())).collect();
        report.also_reverted = also.iter().map(|i| i + 1).collect();
        reverted.extend(failed);
        reverted.extend(also);
        emit(Event::Stage("되돌린 페이지 반영해 다시 저장 중".to_string()));
        pdf_ocr::save::save_rewritten(&mut doc, temp_output, now, compact)?;
        let again: Vec<usize> = reverted.iter().copied().collect();
        let still = verify_pages(engine, pdf, temp_output, &again, emit)?;
        if let Some((page, reason)) = still.first() {
            bail!("되돌린 {}쪽이 원본과 같지 않습니다({reason}). 원본을 건드리지 않았습니다.", page + 1);
        }
    }

    // 결과 파일 전체 구조 확인: 두 라이브러리로 다시 열리고 페이지 수가 같아야 한다.
    let reopened = Document::load(temp_output).map_err(|e| anyhow::anyhow!("결과 파일을 다시 읽지 못함: {e}"))?;
    let pdfium_pages = engine.open_document(temp_output).context("결과 파일을 pdfium으로 열지 못함")?.pages().len() as usize;
    if reopened.get_pages().len() != report.analysis.pages || pdfium_pages != report.analysis.pages {
        bail!("결과 파일의 페이지 수가 원본과 다릅니다. 원본을 건드리지 않았습니다.");
    }
    report.pages_changed = planned.iter().filter(|i| !reverted.contains(i)).count();
    report.size_after = std::fs::metadata(temp_output).map(|m| m.len()).unwrap_or(0);
    Ok(report)
}

/// 페이지들(0부터)을 원본과 비교해 실패한 (페이지, 이유) 목록.
fn verify_pages(
    engine: PdfEngine,
    original: &Path,
    result: &Path,
    pages: &[usize],
    emit: &mut dyn FnMut(Event),
) -> anyhow::Result<Vec<(usize, String)>> {
    use anyhow::Context;
    use pdf_engine::verify::{compare, snapshot};
    let before = engine.open_document(original).map_err(open_error_message)?;
    let after = engine.open_document(result).context("결과 파일을 pdfium으로 열지 못함")?;
    let mut failures = Vec::new();
    for (done, &index) in pages.iter().enumerate() {
        let outcome = (|| -> anyhow::Result<Result<(), String>> {
            let a = snapshot(&before.pages().get(index as i32)?)?;
            let b = snapshot(&after.pages().get(index as i32)?)?;
            Ok(compare(&a, &b))
        })();
        match outcome {
            Ok(Ok(())) => {}
            Ok(Err(reason)) => failures.push((index, reason)),
            Err(err) => failures.push((index, format!("검증 실패({err:#})"))),
        }
        emit(Event::Progress { done: done + 1, total: pages.len() });
    }
    Ok(failures)
}

/// pdfium의 열기 오류를 사용자에게 보일 문장으로 바꾼다.
fn open_error_message(err: anyhow::Error) -> anyhow::Error {
    use pdfium_render::prelude::{PdfiumError, PdfiumInternalError};
    let message = match err.downcast_ref::<PdfiumError>() {
        Some(PdfiumError::PdfiumLibraryInternalError(PdfiumInternalError::PasswordError)) => {
            "암호로 보호된 PDF라 열 수 없습니다."
        }
        Some(PdfiumError::PdfiumLibraryInternalError(PdfiumInternalError::FormatError)) => {
            "PDF 형식이 손상되어 열 수 없습니다."
        }
        Some(PdfiumError::PdfiumLibraryInternalError(PdfiumInternalError::FileError)) => {
            "PDF 파일을 찾거나 읽을 수 없습니다."
        }
        _ => return err,
    };
    anyhow::anyhow!(message)
}

fn file_display_name(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().nfc().collect()).unwrap_or_default()
}
