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
    /// 폴더 안 모든 PDF에서 OCR 삭제 — 파일마다 백업하고 바꾼다(설계 문서 7장 폴더 일괄 처리).
    RemoveFolder { folder: PathBuf, skip: Vec<PathBuf> },
    /// hOCR 가져오기 전 분석(hOCR 파싱 + 페이지 분류). 파일을 쓰지 않는다.
    AnalyzeImport { pdf: PathBuf, hocr_files: Vec<PathBuf> },
    /// hOCR 가져오기 — 결과를 `temp_output`에 쓰고 검증까지 한다.
    Import(crate::ocr_import::ImportJob),
    /// 이 문서에 보이는 텍스트와 보이지 않는 텍스트가 각각 있는지만 확인한다. 파일을 쓰지 않고,
    /// 진행률도 내보내지 않는다 — 내보내기 옵션 창이 떠 있는 동안 조용히 돈다(→ `Event::TextKinds`).
    ProbeTextKinds { pdf: PathBuf },
}

/// 형태별 개수 — (형태 이름, 개수).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RemovalCounts(pub Vec<(String, usize)>);

impl RemovalCounts {
    pub fn removed(&self) -> usize {
        self.0.iter().map(|(_, n)| n).sum()
    }

    // 형태별 개수를 문장으로 잇던 `describe`는 없앴다(2026-09-28 결정) — 지울 대상이 OCR
    // 텍스트 하나뿐이라 형태를 나눌 것이 없고, 글자 수는 사용자가 관심 가질 정보가 아니다.
}

impl From<pdf_ocr::remove::KindCounts> for RemovalCounts {
    fn from(c: pdf_ocr::remove::KindCounts) -> Self {
        Self(c.iter().map(|(kind, n)| (kind.label().to_string(), n)).collect())
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RemovalAnalysis {
    pub pages: usize,
    /// 지울 것이 있는 페이지 수(앱이 넣은 레이어 포함).
    pub pages_with_hidden_text: usize,
    /// 앱이 넣은 OCR 레이어가 있는 페이지 수(구조째 떼어 낸다).
    pub own_layer_pages: usize,
    pub counts: RemovalCounts,
    /// 이 모드에서는 지우지 않고 보고만 하는 형태(적극 모드로 바꾸면 지울 수 있는 것).
    pub reported: RemovalCounts,
    /// (페이지 번호, 이유) — 건드리지 않을 페이지.
    pub skipped: Vec<(usize, String)>,
    pub notes: Vec<(usize, String)>,
    pub signed: bool,
    pub tagged: bool,
    /// 지우고 나면 내용이 비게 되는 태그 수(태그 PDF일 때).
    pub empty_tags: usize,
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
    /// 적극 모드로 돌렸는지.
    /// 쓰이지 않게 되어 목록에서 뺀 레이어 수.
    pub pruned_layers: usize,
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
    /// 내보낼 쪽 범위(1부터, 양끝 포함). 전체면 `(1, 마지막 쪽)`이다.
    pub first: usize,
    pub last: usize,
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
    /// 실제로 내보낸 쪽 수(범위를 골랐으면 그 범위의 쪽 수).
    pub pages: usize,
    /// 문서 전체가 아니라 일부만 내보냈을 때의 범위(1부터, 양끝 포함).
    pub range: Option<(usize, usize)>,
    pub pages_with_text: usize,
    pub words: usize,
    /// 높이가 비정상적으로 커서 보정한 글자 수(`pdf_ocr::layout` 문서).
    pub clamped_chars: usize,
    /// (1부터 센 페이지 번호, 이유) — 빈 페이지로 기록했다.
    pub failed_pages: Vec<(usize, String)>,
}

/// 폴더 일괄 삭제 결과.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BatchReport {
    pub total: usize,
    /// CSV 로그용 파일별 기록(요약은 아래 네 목록에서 만든다).
    pub entries: Vec<BatchEntry>,
    /// 바꾼 파일과 (바뀐 페이지 수, 지운 건수).
    pub changed: Vec<(String, usize, usize)>,
    /// 지울 것이 없던 파일.
    pub unchanged: Vec<String>,
    /// (파일, 이유) — 건드리지 않음.
    pub skipped: Vec<(String, String)>,
    /// (파일, 이유) — 실패(원본 그대로).
    pub failed: Vec<(String, String)>,
    /// 남긴 로그(CSV) 경로.
    pub log: Option<String>,
}

/// 폴더 일괄 삭제에서 파일 하나의 결과 — CSV 열 구성이 그대로 이 구조다(2026-09-27 요청).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BatchEntry {
    /// 고른 폴더 기준 상위 폴더(바로 아래 파일이면 빈 문자열).
    pub folder: String,
    pub name: String,
    /// 지울 보이지 않는 텍스트가 있었는지. 열어 보지 못한 파일(건너뜀·실패)은 None.
    pub had_ocr: Option<bool>,
    /// 성공 / 변경 없음 / 건너뜀 / 실패.
    pub outcome: String,
    /// 이 파일 처리를 마친 시각 `yyyy/mm/dd-hh:mm:ss`.
    pub finished_at: String,
    pub note: String,
}

impl BatchEntry {
    fn new(folder: &Path, file: &Path) -> Self {
        let relative = file.strip_prefix(folder).unwrap_or(file);
        Self {
            folder: relative.parent().map(|p| p.to_string_lossy().to_string()).unwrap_or_default(),
            name: crate::app::display_filename(file),
            finished_at: chrono::Local::now().format("%Y/%m/%d-%H:%M:%S").to_string(),
            ..Default::default()
        }
    }

    fn finish(mut self, outcome: &str, had_ocr: Option<bool>, note: String) -> Self {
        self.outcome = outcome.to_string();
        self.had_ocr = had_ocr;
        self.note = note;
        // 시각은 "처리 완료" 시점이므로 여기서 다시 찍는다(파일 하나가 오래 걸릴 수 있다).
        self.finished_at = chrono::Local::now().format("%Y/%m/%d-%H:%M:%S").to_string();
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Event {
    /// 진행 중인 단계 이름(예: "검증 중").
    Stage(String),
    Progress { done: usize, total: usize },
    ExportDone(ExportReport),
    RemovalAnalysis(RemovalAnalysis),
    RemovalDone(RemovalReport),
    BatchDone(BatchReport),
    ImportAnalysis(crate::ocr_import::ImportAnalysis),
    ImportDone(crate::ocr_import::ImportReport),
    /// 이 문서에 어떤 종류의 텍스트가 있는지(→ `Job::ProbeTextKinds`). 둘 다 찾는 즉시 보낸다.
    TextKinds { visible: bool, invisible: bool },
    /// 쪽마다 어떤 텍스트가 있는지(→ `Job::ProbeTextKinds`, 전부 훑은 뒤). 내보내기 창이 "이
    /// 범위에는 OCR 텍스트가 없습니다"를 판단하는 데 쓴다. 길이는 쪽 수와 같다.
    PageTextMap { visible: Vec<bool>, invisible: Vec<bool> },
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
        Job::AnalyzeRemoval { pdf } => {
            analyze_removal(engine, pdf, pdf_ocr::remove::Mode::Standard, Action::Remove)
                .map(|prepared| Event::RemovalAnalysis(prepared.analysis))
        }
        Job::Remove { pdf, temp_output } => {
            run_removal(engine, pdf, temp_output, Action::Remove, &mut emit).map(Event::RemovalDone)
        }
        Job::RemoveFolder { folder, skip } => {
            run_folder_removal(engine, folder, skip, &mut emit).map(Event::BatchDone)
        }
        Job::AnalyzeImport { pdf, hocr_files } => {
            crate::ocr_import::analyze(engine, pdf, hocr_files, &mut emit).map(Event::ImportAnalysis)
        }
        Job::Import(job) => crate::ocr_import::run(engine, job, &mut emit).map(Event::ImportDone),
        Job::ProbeTextKinds { pdf } => probe_text_kinds(engine, pdf, &mut emit),
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
                Job::Import(job) => drop(std::fs::remove_file(&job.temp_output)),
                // 폴더 일괄은 파일마다 임시 파일을 스스로 정리한다. 나머지는 파일을 쓰지 않는다.
                Job::AnalyzeRemoval { .. }
                | Job::AnalyzeImport { .. }
                | Job::RemoveFolder { .. }
                | Job::ProbeTextKinds { .. } => {}
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

/// 이 문서에 (보이는 텍스트가 있는지, 보이지 않는 텍스트가 있는지).
///
/// 내보내기의 두 선택("보이지 않는 텍스트만" / "페이지의 모든 텍스트")이 각각 **무언가를 내놓는지**를
/// 그대로 답하는 질문이다 — `layout.rs`의 `TextSource`가 `InvisibleOnly`일 때 `c.invisible`로 거르기
/// 때문이다. 보이는 글자가 없으면 두 선택의 결과가 같고, 보이지 않는 글자가 없으면 "보이지 않는
/// 텍스트만"은 빈 파일을 만든다.
///
/// **내보내기와 같은 경로**(`text_layer::page_chars`의 invisible 플래그)를 일부러 쓴다. 다른 방법으로
/// 재면 회색 처리와 실제 내보내기 결과가 어긋날 수 있다.
///
/// 답을 **두 번에 나눠 보낸다**. 문서 전체에 어떤 텍스트가 있는지(`TextKinds`)는 둘 다 찾는 즉시
/// 보내 라디오를 바로 풀어 주고, 쪽별 목록(`PageTextMap`)은 끝까지 훑은 뒤에 보낸다. 쪽별 목록은
/// 페이지 범위를 고르는 데만 쓰이므로 조금 늦어도 되고, 라디오까지 늦추면 큰 문서에서 창이
/// 먹먹해진다(그래서 창을 먼저 띄우는 것이다).
fn probe_text_kinds(engine: PdfEngine, pdf: &Path, emit: &mut dyn FnMut(Event)) -> anyhow::Result<Event> {
    let document = engine.open_document(pdf).map_err(|e| open_error_message(e, Action::Export))?;
    let pages = document.pages();
    let mut visible_pages = Vec::with_capacity(pages.len() as usize);
    let mut invisible_pages = Vec::with_capacity(pages.len() as usize);
    let (mut visible, mut invisible) = (false, false);
    let mut announced = false;
    for page in pages.iter() {
        let (mut page_visible, mut page_invisible) = (false, false);
        if let Ok(chars) = pdf_engine::text_layer::page_chars(&page) {
            for c in chars.iter().filter(|c| !c.generated && !c.ch.is_whitespace()) {
                if c.invisible {
                    page_invisible = true;
                } else {
                    page_visible = true;
                }
                if page_visible && page_invisible {
                    break;
                }
            }
        }
        visible |= page_visible;
        invisible |= page_invisible;
        visible_pages.push(page_visible);
        invisible_pages.push(page_invisible);
        if visible && invisible && !announced {
            announced = true;
            emit(Event::TextKinds { visible, invisible });
        }
    }
    if !announced {
        emit(Event::TextKinds { visible, invisible });
    }
    Ok(Event::PageTextMap { visible: visible_pages, invisible: invisible_pages })
}

fn run_export(engine: PdfEngine, job: &ExportJob, emit: &mut dyn FnMut(Event)) -> anyhow::Result<ExportReport> {
    use anyhow::Context;
    let document = engine.open_document(&job.pdf).map_err(|e| open_error_message(e, Action::Export))?;
    let pages = document.pages();
    let page_count = pages.len() as usize;
    // 범위는 UI에서 이미 맞춰 오지만, 작업 프로세스는 스스로도 지킨다 — 문서가 그새 바뀌었을 수 있다.
    let last = job.last.clamp(1, page_count.max(1));
    let first = job.first.clamp(1, last);
    let total = last - first + 1;
    let partial = first > 1 || last < page_count;

    let file = std::fs::File::create(&job.temp_output)
        .with_context(|| format!("임시 파일을 만들 수 없음: {}", job.temp_output.display()))?;
    let writer = BufWriter::with_capacity(1 << 20, file);
    let mut output = match job.format {
        ExportFormat::Hocr => {
            let title = file_display_name(&job.pdf);
            let system = concat!("PDF-Outliner ", env!("PDF_OUTLINER_VERSION"));
            // 일부만 내보낼 때만 원본 쪽 범위를 머리에 적는다. `ppageno`는 규격대로 0부터 다시
            // 매기므로(hocr::write 모듈 문서), 원본 쪽 번호는 이 메타로만 남는다.
            let source = partial.then_some((first, last));
            Output::Hocr(HocrWriter::new(writer, HOCR_DPI, &title, system, source)?)
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

    let mut report =
        ExportReport { pages: total, range: partial.then_some((first, last)), ..Default::default() };
    for (done, (index, page)) in pages.iter().enumerate().skip(first - 1).take(total).enumerate() {
        // `number`는 **원본 쪽 번호**다. txt의 `=== [p. 12] ===`와 실패 보고가 이 값을 쓴다 —
        // 사람이 읽는 표시라 원본을 가리켜야 한다.
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
        emit(Event::Progress { done: done + 1, total });
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
    let scans = image_boxes(page);
    let chars: Vec<InputChar> = text_layer::page_chars(page)?
        .into_iter()
        .map(|c| InputChar {
            ch: c.ch,
            rect: upright.user_rect_to_display(c.bounds),
            baseline: c.origin.map(|(x, y)| upright.user_to_display(x, y).1),
            generated: c.generated,
            // **OCR 텍스트 = 이미지 영역 안이거나 걸친 `3 Tr`**(2026-09-28 확정). 삭제 쪽과 같은
            // 기준을 쓴다 — 여기서 정하는 값이 "보이지 않는 텍스트만" 내보내기의 대상이다.
            // 상자를 못 구했으면 페이지에 이미지가 있는지로 대신 본다(삭제 쪽과 같은 대비책).
            invisible: c.invisible_mode && on_scan(&c.bounds, &scans),
        })
        .collect();
    let (lines, stats) = build_lines(&chars, options);
    Ok((frame, frame.orient_lines(lines), stats.clamped_chars))
}

/// 페이지에 그려진 이미지들의 상자(사용자 공간 `[left, bottom, right, top]`).
fn image_boxes(page: &pdfium_render::prelude::PdfPage) -> Vec<[f64; 4]> {
    use pdfium_render::prelude::{PdfPageObjectCommon, PdfPageObjectsCommon};
    page.objects()
        .iter()
        .filter(|object| object.as_image_object().is_some())
        .filter_map(|object| object.bounds().ok())
        .map(|b| {
            [b.left().value as f64, b.bottom().value as f64, b.right().value as f64, b.top().value as f64]
        })
        .collect()
}

/// 글자 상자가 이미지 영역 안에 있거나 걸쳐 있는지. 경계가 닿는 것도 걸친 것으로 본다
/// (상자가 퇴화한 글자를 놓치지 않게 — `pdf_ocr::classify::overlaps_image`와 같은 기준).
fn on_scan(bounds: &[f64; 4], scans: &[[f64; 4]]) -> bool {
    if scans.is_empty() {
        return false;
    }
    let degenerate = bounds[0] >= bounds[2] || bounds[1] >= bounds[3];
    scans.iter().any(|s| {
        degenerate || (s[0] <= bounds[2] && bounds[0] <= s[2] && s[1] <= bounds[3] && bounds[1] <= s[3])
    })
}

// ---------------------------------------------------------------- OCR 전체 삭제

use pdf_ocr::lopdf::Document;
use pdf_ocr::remove::{PageStatus, RemovalPlan};

/// 삭제 준비가 끝난 상태: 앱이 넣은 레이어는 이미 떼어 냈고(`applied`에 기록) 나머지는 계획만 있다.
struct PreparedRemoval {
    analysis: RemovalAnalysis,
    doc: Document,
    plan: RemovalPlan,
    applied: pdf_ocr::remove::Applied,
    compact: bool,
}

/// 파일을 열어 사전 점검한다. 암호화·손상·페이지 수 불일치면 오류. (문서, 사전 점검, 압축 저장 여부)
pub(crate) fn open_for_edit(
    engine: PdfEngine,
    pdf: &Path,
    action: Action,
) -> anyhow::Result<(Document, pdf_ocr::preflight::Preflight, bool)> {
    use anyhow::{bail, Context};
    let raw = std::fs::read(pdf).with_context(|| format!("파일을 읽을 수 없음: {}", pdf.display()))?;
    // 구조를 못 읽는 것과 쪽 수가 어긋나는 것은 사용자에게 같은 사정이다 — 한 문구로 합쳤다
    // (2026-10-01 md 확정). 어느 쪽이었는지는 stderr로 가른다.
    let doc = Document::load_mem(&raw).map_err(|e| {
        eprintln!("PDF 구조를 읽지 못함: {e}");
        anyhow::anyhow!("PDF 구조가 손상된 것으로 확인되어 작업을 진행할 수 없습니다.")
    })?;
    let preflight = pdf_ocr::preflight::Preflight::inspect(&doc, &raw);
    drop(raw);
    if preflight.encrypted {
        // 빈 사용자 암호로 열리는 암호 PDF도 있어 pdfium보다 먼저 구조에서 막는다. 문구는 pdfium
        // 열기 실패와 같게 맞춘다 — 같은 상황을 두 가지로 말하지 않는다(2026-09-28 md 지적).
        return Err(password_protected(action));
    }
    let pdfium_pages = engine.open_document(pdf).map_err(|e| open_error_message(e, action))?.pages().len() as usize;
    if pdfium_pages != preflight.page_count {
        // 쪽 수가 왜 어긋났는지는 사용자가 어쩔 수 있는 일이 아니라 화면에 적지 않는다
        // (2026-10-01 지정). 진단이 필요할 때를 위해 수치는 stderr로 남긴다 — pdfium이 세는 쪽
        // 수와 PDF 페이지 트리가 주장하는 쪽 수다.
        eprintln!("쪽 수 불일치: pdfium {pdfium_pages}쪽, 페이지 트리 {}쪽", preflight.page_count);
        bail!("PDF 구조가 손상된 것으로 확인되어 작업을 진행할 수 없습니다.");
    }
    let compact = pdf_ocr::save::uses_object_streams(&doc) && !preflight.pdfa.as_deref().is_some_and(|p| p.starts_with('1'));
    Ok((doc, preflight, compact))
}

/// 사전 점검 + 앱 레이어 떼어 내기(메모리에서) + 삭제 계획.
fn analyze_removal(
    engine: PdfEngine,
    pdf: &Path,
    mode: pdf_ocr::remove::Mode,
    action: Action,
) -> anyhow::Result<PreparedRemoval> {
    let (mut doc, preflight, compact) = open_for_edit(engine, pdf, action)?;
    let mut applied = pdf_ocr::remove::Applied::default();
    let stripped = pdf_ocr::insert::strip_own_layers(&mut doc, None, &mut applied)?;
    let plan = pdf_ocr::remove::plan_for(&doc, None, mode);
    let mut analysis = RemovalAnalysis {
        pages: plan.pages.len(),
        own_layer_pages: stripped.len(),
        counts: plan.totals().into(),
        reported: plan.reported_totals().into(),
        signed: preflight.signed,
        tagged: preflight.tagged,
        empty_tags: plan.empty_tags(),
        pdfa: preflight.pdfa.clone(),
        incremental_updates: preflight.incremental_updates,
        linearized: preflight.linearized,
        ..Default::default()
    };
    for (index, page) in plan.pages.iter().enumerate() {
        match &page.status {
            PageStatus::Planned => analysis.pages_with_hidden_text += 1,
            PageStatus::Skipped(reason) => analysis.skipped.push((page.number, reason.clone())),
            PageStatus::Unchanged if stripped.contains(&index) => analysis.pages_with_hidden_text += 1,
            PageStatus::Unchanged => {}
        }
        analysis.notes.extend(page.notes.iter().map(|n| (page.number, n.clone())));
    }
    Ok(PreparedRemoval { analysis, doc, plan, applied, compact })
}

fn run_removal(
    engine: PdfEngine,
    pdf: &Path,
    temp_output: &Path,
    action: Action,
    emit: &mut dyn FnMut(Event),
) -> anyhow::Result<RemovalReport> {
    use anyhow::{bail, Context};
    use std::collections::BTreeSet;
    emit(Event::Stage("분석 중".to_string()));
    let mode = pdf_ocr::remove::Mode::Standard;
    let PreparedRemoval { analysis, mut doc, plan, mut applied, compact } = analyze_removal(engine, pdf, mode, action)?;
    let size_before = std::fs::metadata(pdf).map(|m| m.len()).unwrap_or(0);
    let mut report = RemovalReport { analysis, size_before, ..Default::default() };
    if !plan.has_changes() && report.analysis.own_layer_pages == 0 {
        report.nothing_to_do = true;
        return Ok(report);
    }

    emit(Event::Stage("삭제·저장 중".to_string()));
    pdf_ocr::remove::apply(&mut doc, &plan, &mut applied)?;
    report.pruned_layers = pdf_ocr::remove::prune_optional_content(&mut doc);
    let now = chrono::Local::now().fixed_offset();
    // 되돌릴 때 원본 콘텐츠 스트림이 필요하므로, 저장하며 버려질 객체를 미리 빼 둔다.
    let dropped = pdf_ocr::save::take_unreferenced(&mut doc);
    pdf_ocr::save::save_rewritten(&mut doc, temp_output, now, compact)?;

    let planned: Vec<usize> = applied.changed_pages().into_iter().collect();
    emit(Event::Stage("검증 중(원본과 화면·텍스트 비교)".to_string()));
    let no_layer = std::collections::HashMap::new();
    // 적극 모드는 화면만 비교한다 — 흰 글씨처럼 "화면엔 없지만 추출되는" 글자를 지우기 때문.
    let failures = verify_pages(engine, pdf, temp_output, &planned, &no_layer, true, emit)?;

    let mut reverted: BTreeSet<usize> = BTreeSet::new();
    if !failures.is_empty() {
        // 진단용(stderr) — 어떤 페이지가 왜 되돌려지는지.
        eprintln!(
            "ocr-worker: 검증 실패 {}쪽 — {:?}",
            failures.len(),
            failures.iter().take(10).map(|(i, why)| (i + 1, why.as_str())).collect::<Vec<_>>()
        );
        let failed: BTreeSet<usize> = failures.iter().map(|(i, _)| *i).collect();
        pdf_ocr::save::restore(&mut doc, dropped);
        let also = applied.rollback(&mut doc, &failed);
        report.rolled_back = failures.iter().map(|(i, reason)| (i + 1, reason.clone())).collect();
        report.also_reverted = also.iter().map(|i| i + 1).collect();
        reverted.extend(failed);
        reverted.extend(also);
        emit(Event::Stage("되돌린 페이지 반영해 다시 저장 중".to_string()));
        pdf_ocr::save::save_rewritten(&mut doc, temp_output, now, compact)?;
        let again: Vec<usize> = reverted.iter().copied().collect();
        let still = verify_pages(engine, pdf, temp_output, &again, &no_layer, true, emit)?;
        if !still.is_empty() {
            eprintln!(
                "ocr-worker: 되돌린 뒤에도 다른 페이지 {:?}",
                still.iter().take(10).map(|(i, why)| (i + 1, why.as_str())).collect::<Vec<_>>()
            );
        }
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
    if report.pages_changed == 0 {
        // 전부 되돌아갔다 — 결과가 원본과 같으므로 파일을 바꾸지 않는다.
        report.nothing_to_do = true;
        let _ = std::fs::remove_file(temp_output);
        return Ok(report);
    }
    report.size_after = std::fs::metadata(temp_output).map(|m| m.len()).unwrap_or(0);
    Ok(report)
}

/// 폴더(하위 폴더 포함) 안 모든 PDF에서 OCR을 지운다. 파일마다 `.backup`으로 보존하고 바꾸며,
/// 백업이 이미 있으면(이전 실행 흔적) 건드리지 않는다 — 폴더 북마크 일괄 적용과 같은 규칙
/// (`batch_import` 모듈). 앱에 열려 있는 파일도 건너뛴다. 결과는 폴더 루트에 CSV 로그로 남긴다.
fn run_folder_removal(
    engine: PdfEngine,
    folder: &Path,
    skip: &[PathBuf],
    emit: &mut dyn FnMut(Event),
) -> anyhow::Result<BatchReport> {
    let mut files = Vec::new();
    collect_pdfs(folder, &mut files);
    files.sort();
    // 진행률은 파일이 아니라 페이지 기준 — 파일마다 쪽수가 크게 달라서.
    emit(Event::Stage("페이지 수 세는 중".to_string()));
    let page_counts: Vec<usize> =
        files.iter().map(|f| engine.open_document(f).map(|d| d.pages().len() as usize).unwrap_or(1)).collect();
    let total_pages: usize = page_counts.iter().sum::<usize>().max(1);
    let mut done_pages = 0usize;

    let mut report = BatchReport { total: files.len(), ..Default::default() };
    let mut log = open_batch_log(folder);
    report.log = log.as_ref().map(|(path, _)| path.to_string_lossy().to_string());
    // 결과를 report에 담으면서 곧바로 CSV에도 한 줄 흘려 보낸다.
    macro_rules! record {
        ($entry:expr) => {{
            let entry = $entry;
            if let Some((_, file)) = log.as_mut() {
                append_batch_log(file, &entry);
            }
            report.entries.push(entry);
        }};
    }
    for (index, file) in files.iter().enumerate() {
        let name = file.strip_prefix(folder).unwrap_or(file).to_string_lossy().to_string();
        let entry = BatchEntry::new(folder, file);
        let file_pages = page_counts[index];
        emit(Event::Stage(format!("{name} ({}/{})", index + 1, files.len())));
        emit(Event::Progress { done: done_pages, total: total_pages });

        if skip.iter().any(|s| s == file) {
            let reason = "열려 있는 파일".to_string();
            record!(entry.finish("건너뜀", None, reason.clone()));
            report.skipped.push((name, reason));
            done_pages += file_pages;
            continue;
        }
        // 백업이 이미 있어도 건너뛰지 않는다 — 이름에 시각이 들어가 이전 백업을 덮어쓸 일이
        // 없기 때문이다(2026-09-28). 전에는 `문서.pdf.backup` 하나뿐이라 덮어쓰지 않으려고
        // 건너뛰었다. 다시 돌려도 지울 것이 없는 파일은 "변경 없음"으로 지나가므로 해가 없다.
        let backup = crate::app::backup_path(file, &crate::app::backup_stamp());
        let temp = file.with_extension("ocr_tmp.pdf");
        // 파일 안의 진행(검증 단계)을 이 파일 쪽수로 환산해 전체 진행률에 더한다.
        let base = done_pages;
        let forward = |event: Event| {
            if let Event::Progress { done, total } = event {
                let fraction = if total > 0 { done as f64 / total as f64 } else { 0.0 };
                emit(Event::Progress { done: base + (fraction * file_pages as f64) as usize, total: total_pages });
            }
        };
        let outcome = {
            // forward가 emit을 빌리고 있으므로 이 블록 안에서만 살려 둔다.
            let mut forward = forward;
            run_removal(engine, file, &temp, Action::BatchCell, &mut forward)
        };
        done_pages += file_pages;
        match outcome {
            Err(err) => {
                let _ = std::fs::remove_file(&temp);
                let reason = format!("{err:#}");
                record!(entry.finish("실패", None, reason.clone()));
                report.failed.push((name, reason));
            }
            Ok(result) if result.nothing_to_do => {
                // 지울 것이 없었다 = 보이지 않는 텍스트가 없었다(파일은 열어 봤으므로 단정할 수 있다).
                record!(entry.finish("변경 없음", Some(false), String::new()));
                report.unchanged.push(name);
            }
            Ok(result) => {
                if let Err(err) = std::fs::copy(file, &backup) {
                    let _ = std::fs::remove_file(&temp);
                    let reason = format!("백업 실패({err}). 원본 그대로");
                    record!(entry.finish("실패", Some(true), reason.clone()));
                    report.failed.push((name, reason));
                    continue;
                }
                if let Err(err) = std::fs::rename(&temp, file) {
                    let _ = std::fs::remove_file(&temp);
                    let reason = format!("교체 실패({err}). 원본 그대로");
                    record!(entry.finish("실패", Some(true), reason.clone()));
                    report.failed.push((name, reason));
                    continue;
                }
                let removed = result.analysis.counts.removed();
                record!(entry.finish("성공", Some(true), String::new()));
                report.changed.push((name, result.pages_changed, removed));
            }
        }
    }
    emit(Event::Progress { done: total_pages, total: total_pages });
    Ok(report)
}

fn collect_pdfs(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') || name.starts_with("~$") {
            continue;
        }
        match entry.file_type() {
            Ok(t) if t.is_dir() => collect_pdfs(&path, out),
            Ok(t) if t.is_file() && path.extension().is_some_and(|e| e.eq_ignore_ascii_case("pdf")) => out.push(path),
            _ => {}
        }
    }
}

/// 파일별 결과를 CSV로 남긴다(UTF-8 BOM — Excel이 한글을 제대로 읽게).
///
/// 열 구성은 사용자 지정(2026-09-27): 경로 · 파일명 · OCR 유무 · 실행결과 · 처리완료시간 · 비고.
/// 모든 열이 그 행의 **파일 하나**에 대한 값이고, 누계나 전체 집계는 넣지 않는다.
///
/// 예전 로그에 있던 `바뀐 페이지`·`지운 건수`는 뺐다 — 디버깅에나 쓸 값이고 사용자에게는
/// 필요하지 않다는 판단(2026-09-27). 규모는 결과 창 요약에 그대로 남아 있다.
///
/// 순서는 **처리한 순서**(폴더 탐색 순서)다. 결과별로 묶어 적던 예전 방식과 달리, 중간에 멈춘
/// 작업도 어디까지 됐는지 그대로 읽힌다.
/// CSV 로그를 열고 머리글을 쓴다. **파일 하나를 끝낼 때마다 한 줄씩 이어 쓴다**(2026-09-29 요청)
/// — 예전에는 작업이 다 끝난 뒤 한 번에 썼기 때문에, 도중에 취소하면 어디까지 처리됐는지 남는
/// 기록이 하나도 없었다.
fn open_batch_log(folder: &Path) -> Option<(PathBuf, std::fs::File)> {
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let path = folder.join(format!("ocr-remove-{stamp}.csv"));
    let mut file = std::fs::File::create(&path).ok()?;
    // UTF-8 BOM — Excel이 한글을 제대로 읽게.
    file.write_all("\u{feff}경로,파일명,OCR 유무,실행결과,처리완료시간,비고\n".as_bytes()).ok()?;
    Some((path, file))
}

/// 파일 하나의 결과를 CSV에 이어 쓰고 곧바로 디스크에 내린다(취소·크래시에도 남게).
fn append_batch_log(file: &mut std::fs::File, e: &BatchEntry) {
    let escape = |value: &str| format!("\"{}\"", value.replace('"', "\"\""));
    let had_ocr = match e.had_ocr {
        Some(true) => "있음",
        Some(false) => "없음",
        None => "알 수 없음",
    };
    let row = format!(
        "{},{},{had_ocr},{},{},{}\n",
        escape(&e.folder),
        escape(&e.name),
        e.outcome,
        e.finished_at,
        escape(&e.note),
    );
    let _ = file.write_all(row.as_bytes());
    let _ = file.flush();
}

/// 페이지들(0부터)을 원본과 비교해 실패한 (페이지, 이유) 목록. `expected_layer`에 든 페이지는
/// 앱이 넣은 텍스트 레이어의 글자가 기대한 글자(정렬된 목록)와 같은지도 확인한다.
pub(crate) fn verify_pages(
    engine: PdfEngine,
    original: &Path,
    result: &Path,
    pages: &[usize],
    expected_layer: &std::collections::HashMap<usize, Vec<char>>,
    check_visible_text: bool,
    emit: &mut dyn FnMut(Event),
) -> anyhow::Result<Vec<(usize, String)>> {
    use anyhow::Context;
    use pdf_engine::verify::{compare_with, snapshot};
    let before = engine.open_document(original).map_err(|e| open_error_message(e, Action::Remove))?;
    let after = engine.open_document(result).context("결과 파일을 pdfium으로 열지 못함")?;
    let mut failures = Vec::new();
    for (done, &index) in pages.iter().enumerate() {
        let outcome = (|| -> anyhow::Result<Result<(), String>> {
            let a = snapshot(&before.pages().get(index as i32)?)?;
            let after_page = after.pages().get(index as i32)?;
            let b = snapshot(&after_page)?;
            if let Err(reason) = compare_with(&a, &b, check_visible_text) {
                return Ok(Err(reason));
            }
            if let Some(expected) = expected_layer.get(&index) {
                let actual = pdf_engine::verify::sorted_chars_in_font(&after_page, pdf_ocr::glyphless::FONT_NAME)?;
                if &actual != expected {
                    return Ok(Err(format!(
                        "가져온 텍스트가 기대와 다르게 추출됨(기대 {}자, 추출 {}자)",
                        expected.len(),
                        actual.len()
                    )));
                }
            }
            Ok(Ok(()))
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

/// 실패 문장을 **자리에 맞게** 쓰기 위한 작업 이름(2026-09-27 요청).
///
/// 같은 실패가 두 자리에 나오는데 필요한 길이가 다르다. 폴더 일괄은 CSV `비고` **칸**에
/// 들어가므로 짧은 이름꼴이 좋고, 한 파일 작업은 결과 창 **본문**이라 그 한 줄이 설명의
/// 전부이므로 무엇을 못 했는지까지 말해야 한다.
#[derive(Clone, Copy)]
pub(crate) enum Action {
    /// 폴더 일괄 — 표 칸용 짧은 이름꼴.
    BatchCell,
    Remove,
    Import,
    Export,
}

impl Action {
    /// `None`이면 짧은 이름꼴을 쓴다.
    fn phrase(self) -> Option<&'static str> {
        match self {
            Action::BatchCell => None,
            Action::Remove => Some("OCR을 삭제할 수 없습니다"),
            Action::Import => Some("OCR을 가져올 수 없습니다"),
            Action::Export => Some("OCR 텍스트를 내보낼 수 없습니다"),
        }
    }
}

/// "암호로 보호된 파일" 안내 — 구조 점검과 pdfium 열기 실패가 같은 문구를 쓴다.
fn password_protected(action: Action) -> anyhow::Error {
    match action.phrase() {
        None => anyhow::anyhow!("암호로 보호된 파일"),
        Some(phrase) => anyhow::anyhow!("파일이 암호로 보호돼 있어서 {phrase}."),
    }
}

/// pdfium의 열기 오류를 사용자에게 보일 문장으로 바꾼다.
pub(crate) fn open_error_message(err: anyhow::Error, action: Action) -> anyhow::Error {
    use pdfium_render::prelude::{PdfiumError, PdfiumInternalError};
    // (짧은 이름꼴, 긴 문장의 "…해서" 부분)
    let (short, because) = match err.downcast_ref::<PdfiumError>() {
        Some(PdfiumError::PdfiumLibraryInternalError(PdfiumInternalError::PasswordError)) => {
            return password_protected(action)
        }
        Some(PdfiumError::PdfiumLibraryInternalError(PdfiumInternalError::FormatError)) => {
            ("형식이 손상된 파일", "파일 형식이 손상돼 있어서")
        }
        Some(PdfiumError::PdfiumLibraryInternalError(PdfiumInternalError::FileError)) => {
            ("읽을 수 없는 파일", "파일을 찾거나 읽을 수 없어서")
        }
        _ => return err,
    };
    match action.phrase() {
        None => anyhow::anyhow!(short),
        Some(phrase) => anyhow::anyhow!("{because} {phrase}."),
    }
}

fn file_display_name(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().nfc().collect()).unwrap_or_default()
}

#[cfg(test)]
mod export_range_tests {
    use super::*;
    use crate::pdfium_test;

    fn export(pdf: &Path, dir: &Path, format: ExportFormat, first: usize, last: usize) -> (String, ExportReport) {
        let engine = pdfium_test::engine().expect("엔진");
        let output = dir.join(if format == ExportFormat::Hocr { "out.hocr" } else { "out.txt" });
        let job = ExportJob {
            pdf: pdf.to_path_buf(),
            output: output.clone(),
            first,
            last,
            temp_output: dir.join("out.part"),
            format,
            invisible_only: false,
            txt_crlf: false,
            txt_form_feed: false,
            txt_page_labels: true,
        };
        let report = run_export(engine, &job, &mut |_| {}).expect("내보내기");
        (std::fs::read_to_string(&output).expect("결과 파일"), report)
    }

    /// 범위를 골라 내보내면 **그 쪽만** 나온다. hOCR의 `ppageno`는 0부터 다시 매기고 원본 범위는
    /// 메타로 남기며(hocr::write 모듈 문서), txt의 쪽 머리는 **원본 번호** 그대로다(2026-10-01 지정).
    #[test]
    fn a_ranged_export_writes_only_those_pages() {
        let _guard = pdfium_test::lock();
        if pdfium_test::engine().is_none() {
            return;
        }
        let pdf = pdfium_test::sample("BZR001088_01-mod.pdf");
        if !pdf.exists() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();

        let (hocr, report) = export(&pdf, dir.path(), ExportFormat::Hocr, 3, 10);
        assert_eq!(report.pages, 8, "8쪽만 내보내야 한다");
        assert_eq!(report.range, Some((3, 10)));
        assert_eq!(hocr.matches("class=\"ocr_page\"").count(), 8, "쪽 수가 맞지 않는다");
        assert!(hocr.contains("<meta name=\"ocr-source-pages\" content=\"3-10\"/>"), "원본 범위 메타가 없다");
        assert!(hocr.contains("ppageno 0;") && hocr.contains("ppageno 7;"), "ppageno는 0부터 7까지여야 한다");
        assert!(!hocr.contains("ppageno 8;"), "범위 밖 쪽이 들어갔다");

        let (txt, report) = export(&pdf, dir.path(), ExportFormat::Txt, 3, 10);
        assert_eq!(report.pages, 8);
        assert!(txt.contains("[p. 3]"), "txt 쪽 머리는 원본 번호여야 한다: {}", &txt[..txt.len().min(200)]);
        assert!(txt.contains("[p. 10]"));
        assert!(!txt.contains("[p. 2]") && !txt.contains("[p. 11]"), "범위 밖 쪽이 들어갔다");
        assert!(!txt.contains("[p. 1]"), "1쪽부터 다시 매기면 안 된다");
    }



    /// 전체를 내보내면 예전 그대로다 — 범위 메타도, 보고의 범위도 없다.
    #[test]
    fn a_full_export_is_unchanged() {
        let _guard = pdfium_test::lock();
        if pdfium_test::engine().is_none() {
            return;
        }
        let pdf = pdfium_test::sample("BZR001088_01-mod.pdf");
        if !pdf.exists() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();

        let (hocr, report) = export(&pdf, dir.path(), ExportFormat::Hocr, 1, 24);
        assert_eq!(report.pages, 24);
        assert_eq!(report.range, None, "전체면 범위를 적지 않는다");
        assert!(!hocr.contains("ocr-source-pages"));
        assert_eq!(hocr.matches("class=\"ocr_page\"").count(), 24);
    }
}
