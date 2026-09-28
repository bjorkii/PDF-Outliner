//! OCR 메뉴의 대화상자 — 내보내기 옵션, 전체 삭제 확인, 진행률·취소, 결과 리포트(설계 문서 7장).
//!
//! 실제 작업은 `ocr_worker` 프로세스가 한다. 여기서는 옵션을 모아 작업을 띄우고, 매 프레임
//! 이벤트를 받아 진행 상황을 보여 준다.
//!
//! 전체 삭제 흐름: (저장 안 한 북마크가 있으면 먼저 저장) → 분석 작업 → 확인 창(형태별 개수,
//! 건너뛸 페이지, 서명·태그·PDF/A 경고) → 삭제 작업(임시 파일에 쓰고 검증) → 이 모듈이 원본을
//! `.backup`으로 복사하고 임시 파일로 교체한 뒤 문서를 다시 연다(보던 페이지 유지). OCR 작업은
//! 파일 단위 파괴적 작업이라 앱의 Undo 대상이 아니고, 되돌리기는 백업으로 한다.

use crate::app::PdfViewerApp;
use crate::ocr_worker::{
    Event, ExportFormat, ExportJob, ExportReport, Job, RemovalAnalysis, RemovalReport, WorkerHandle, WorkerPoll,
};
use std::path::{Path, PathBuf};

/// 창에 다시 포커스를 준다. 네이티브 파일·폴더 대화상자를 띄우거나 보조 프로세스가 도는 동안
/// macOS에서 앱 창이 키 포커스를 잃고, 그 상태로는 파일을 끌어다 놓아도 열리지 않는다(다른 앱에
/// 갔다 돌아와야 동작) — 사용자 리포트 2026-09-23.
fn focus_window(ctx: &egui::Context) {
    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
}

/// 내보내기 옵션 대화상자 상태. 형식은 메뉴에서 고른다.
pub struct ExportDialog {
    pub format: ExportFormat,
    pub invisible_only: bool,
    pub txt_page_labels: bool,
    pub txt_crlf: bool,
    pub txt_form_feed: bool,
    /// 이 문서에 어떤 종류의 텍스트가 있는지 — **확인이 끝나기 전에는 `None`**이다.
    /// 결과에 따라 의미 없는 선택지를 회색으로 잠근다(→ `show_export_dialog`).
    pub text_kinds: Option<TextKinds>,
    /// 확인이 끝나기 전에 "내보내기…"를 누른 경우 — 답이 오면 그때 시작한다(→ `show_export_dialog`).
    pub start_pending: bool,
}

impl ExportDialog {
    pub fn new(format: ExportFormat) -> Self {
        Self {
            format,
            invisible_only: true,
            txt_page_labels: true,
            txt_crlf: false,
            txt_form_feed: false,
            text_kinds: None,
            start_pending: false,
        }
    }
}

/// 이 문서에 있는 텍스트의 종류. 내보내기 선택지 중 **결과가 비거나 서로 같아지는 것**을 가려내는
/// 데 쓴다(2026-09-27 요청 — "켜도 달라지는 것이 없는 선택지는 보여 주지 않는다").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextKinds {
    pub visible: bool,
    pub invisible: bool,
}

impl TextKinds {
    /// 확인에 실패했을 때 쓰는 값 — 둘 다 있다고 보아 아무 선택지도 잠그지 않는다.
    /// 확인만 없는 것이고 내보내기 자체는 되므로, 모른다는 이유로 막지는 않는다.
    const UNKNOWN: Self = Self { visible: true, invisible: true };
}

/// 메뉴 "hOCR/txt로 내보내기…" — 옵션 창을 **즉시** 띄우고, 보이는 텍스트가 있는지는 뒤에서
/// 확인한다(2026-09-27 사용자 선택).
///
/// 확인을 먼저 끝내고 창을 띄우면 큰 파일에서 메뉴가 먹먹해진다. 반대로 확인 없이 두면 결과가
/// 같은 선택을 열어 둔 채로 보여 주게 된다. 그래서 창은 바로 띄우고, 답이 오면 그때 라디오를
/// 잠근다 — 답이 오기 전에 사용자가 골라 진행해도 결과는 어차피 같으므로 문제가 없다.
pub fn request_export(ctx: &egui::Context, app: &mut PdfViewerApp, format: ExportFormat) {
    app.ocr_export_dialog = Some(ExportDialog::new(format));
    finish_probe(app);
    if let Some(pdf) = app.current_file.clone() {
        // 실패하면 확인만 없는 것이고(라디오는 열린 채로 남는다) 내보내기 자체는 되므로 조용히 넘긴다.
        app.ocr_export_probe = WorkerHandle::spawn(&Job::ProbeTextKinds { pdf }, ctx).ok();
    }
}

/// 확인 작업을 놓는다 — `kill`이 `wait`까지 해서 이미 끝난 프로세스도 거둔다(좀비 방지).
fn finish_probe(app: &mut PdfViewerApp) {
    if let Some(mut probe) = app.ocr_export_probe.take() {
        probe.kill();
    }
}

/// 조용한 확인의 결과를 받아 옵션 창에 채운다. 창이 닫혔으면 확인도 그만둔다.
fn poll_export_probe(app: &mut PdfViewerApp) {
    if app.ocr_export_dialog.is_none() {
        finish_probe(app);
        return;
    }
    // 답은 한 번뿐이라 한 프레임에 한 줄만 읽는다(끝 이벤트든 실패든 바로 확인을 놓는다).
    let Some(probe) = app.ocr_export_probe.as_ref() else { return };
    match probe.poll() {
        WorkerPoll::Empty => {}
        WorkerPoll::Event(Event::TextKinds { visible, invisible }) => {
            if let Some(dialog) = app.ocr_export_dialog.as_mut() {
                dialog.text_kinds = Some(TextKinds { visible, invisible });
                // 한쪽만 있으면 그쪽으로 굳힌다 — 빈 파일이 나오는 선택으로 시작하지 않게.
                if visible != invisible {
                    dialog.invisible_only = invisible;
                }
            }
            finish_probe(app);
        }
        // 다른 이벤트나 실패 — 라디오는 열어 두고, 기다리던 "내보내기…"는 풀어 준다.
        WorkerPoll::Event(_) | WorkerPoll::Closed => {
            if let Some(dialog) = app.ocr_export_dialog.as_mut() {
                dialog.text_kinds = Some(TextKinds::UNKNOWN);
            }
            finish_probe(app);
        }
    }
}

enum JobPhase {
    Running { done: usize, total: usize },
    Finished(String),
    Failed(String),
}

enum JobKind {
    Export { describe: Box<dyn Fn(&ExportReport) -> String> },
    AnalyzeRemoval { pdf: PathBuf },
    Remove { pdf: PathBuf },
    AnalyzeImport { pdf: PathBuf, files: Vec<PathBuf> },
    Batch,
    /// `skipped`는 넣을 대상에서 미리 빠진 페이지의 이유별 개수(작업 프로세스는 이유를 모른다 —
    /// 창에서 정한 것이다). 결과 창에도 남겨야 실행 뒤에 확인할 수 있다.
    Import { pdf: PathBuf, skipped: String },
}

/// 북마크를 먼저 저장해야 시작할 수 있는 OCR 작업.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingOcr {
    Remove,
    Import,
}

/// 실행 중이거나 끝난 OCR 작업(창을 닫을 때까지 유지).
pub struct OcrJob {
    title: String,
    worker: Option<WorkerHandle>,
    /// 취소·실패 시 지울 임시 파일.
    temp_output: Option<PathBuf>,
    phase: JobPhase,
    /// 작업 프로세스가 알려 준 현재 단계.
    stage: Option<String>,
    kind: JobKind,
    /// 결과 창에서 "보기"로 뷰어에 표시할 자리(가져오기 검증 결과).
    marks: Vec<ProblemMark>,
}

/// 전체 삭제 확인 창 상태.
pub struct RemovalConfirm {
    pdf: PathBuf,
    analysis: RemovalAnalysis,
    /// 서명 무효화에 동의함(서명된 파일에서만 필요).
    signature_ack: bool,
}

impl OcrJob {
    pub fn is_running(&self) -> bool {
        matches!(self.phase, JobPhase::Running { .. })
    }

    fn cancel(&mut self) {
        if let Some(mut worker) = self.worker.take() {
            worker.kill();
        }
        if let Some(temp) = self.temp_output.take() {
            let _ = std::fs::remove_file(temp);
        }
    }

    fn spawn(ctx: &egui::Context, title: String, job: &Job, kind: JobKind, temp_output: Option<PathBuf>) -> Self {
        let (worker, phase) = match WorkerHandle::spawn(job, ctx) {
            Ok(worker) => (Some(worker), JobPhase::Running { done: 0, total: 0 }),
            Err(err) => (None, JobPhase::Failed(format!("작업 프로세스를 시작하지 못했습니다: {err}"))),
        };
        Self { title, worker, temp_output, phase, stage: None, kind, marks: Vec::new() }
    }

    fn finish(&mut self, text: String) {
        self.worker = None;
        self.temp_output = None;
        self.phase = JobPhase::Finished(text);
    }

    fn is_finished(&self) -> bool {
        !matches!(self.phase, JobPhase::Running { .. })
    }

    fn fail(&mut self, message: String) {
        self.worker = None;
        if let Some(temp) = self.temp_output.take() {
            let _ = std::fs::remove_file(temp);
        }
        self.phase = JobPhase::Failed(message);
    }
}

/// 매 프레임: 작업 프로세스의 이벤트를 받아 상태를 갱신한다.
pub fn poll(ctx: &egui::Context, app: &mut PdfViewerApp) {
    poll_export_probe(app);
    let was_running = app.ocr_job.as_ref().is_some_and(|j| !j.is_finished());
    poll_events(app);
    // 작업이 끝난 프레임에 창 포커스를 되찾는다(보조 프로세스가 가져간 포커스).
    if was_running && app.ocr_job.as_ref().is_some_and(|j| j.is_finished()) {
        focus_window(ctx);
    }
}

fn poll_events(app: &mut PdfViewerApp) {
    loop {
        let Some(job) = app.ocr_job.as_mut() else { return };
        let Some(worker) = job.worker.as_ref() else { return };
        match worker.poll() {
            WorkerPoll::Empty => return,
            WorkerPoll::Event(Event::Stage(stage)) => {
                job.stage = Some(stage);
                job.phase = JobPhase::Running { done: 0, total: 0 };
            }
            WorkerPoll::Event(Event::Progress { done, total }) => job.phase = JobPhase::Running { done, total },
            WorkerPoll::Event(Event::ExportDone(report)) => {
                let text = match &job.kind {
                    JobKind::Export { describe } => describe(&report),
                    _ => String::new(),
                };
                job.finish(text); // 결과 파일로 이미 옮겨짐
            }
            WorkerPoll::Event(Event::RemovalAnalysis(analysis)) => {
                let JobKind::AnalyzeRemoval { pdf } = &job.kind else { return };
                let pdf = pdf.clone();
                app.ocr_job = None;
                app.ocr_removal_confirm = Some(RemovalConfirm { pdf, analysis, signature_ack: false });
            }
            WorkerPoll::Event(Event::RemovalDone(report)) => {
                let JobKind::Remove { pdf } = &job.kind else { return };
                let pdf = pdf.clone();
                let temp = job.temp_output.take();
                job.worker = None;
                let outcome = finish_removal(app, &pdf, temp.as_deref(), &report);
                if let Some(job) = app.ocr_job.as_mut() {
                    match outcome {
                        Ok(text) => job.finish(text),
                        Err(message) => job.fail(message),
                    }
                }
            }
            WorkerPoll::Event(Event::BatchDone(report)) => {
                job.finish(describe_batch(&report));
                app.status_message = Some(format!("폴더 일괄 OCR 삭제: {}개 파일 바꿈", report.changed.len()));
            }
            WorkerPoll::Event(Event::ImportAnalysis(analysis)) => {
                let JobKind::AnalyzeImport { pdf, files } = &job.kind else { return };
                let dialog = ImportDialog::new(pdf.clone(), files.clone(), analysis);
                app.ocr_job = None;
                app.ocr_import_dialog = Some(dialog);
            }
            WorkerPoll::Event(Event::ImportDone(report)) => {
                let JobKind::Import { pdf, skipped } = &job.kind else { return };
                let (pdf, skipped) = (pdf.clone(), skipped.clone());
                let temp = job.temp_output.take();
                job.worker = None;
                let outcome = if report.nothing_to_do {
                    if let Some(temp) = &temp {
                        let _ = std::fs::remove_file(temp);
                    }
                    Ok(describe_import(&report, &skipped, "파일을 바꾸지 않았습니다."))
                } else {
                    swap_in_result(app, &pdf, temp.as_deref(), "OCR 텍스트를 가져왔습니다.", "OCR을 가져올 수 없습니다")
                        .map(|backup_note| describe_import(&report, &skipped, &backup_note))
                };
                if let Some(job) = app.ocr_job.as_mut() {
                    match outcome {
                        Ok(text) => {
                            job.finish(text);
                            job.marks = report.marks;
                        }
                        Err(message) => job.fail(message),
                    }
                }
            }
            // 내보내기 옵션 창의 조용한 확인은 작업 창과 별개 경로다(`poll_export_probe`).
            WorkerPoll::Event(Event::TextKinds { .. }) => {}
            WorkerPoll::Event(Event::Failed(message)) => job.fail(message),
            // 끝 이벤트 없이 출력이 끝남 — 작업 프로세스가 죽었다.
            WorkerPoll::Closed => job.fail("작업 프로세스가 예기치 않게 끝났습니다(panic.log 확인).".to_string()),
        }
    }
}

// ---------------------------------------------------------------- 전체 삭제

/// 메뉴 "전체 삭제…". 저장 안 한 북마크가 있으면 먼저 저장을 묻는다(OCR 작업은 디스크의 파일을
/// 다시 쓰므로, 저장 안 한 편집은 그 결과 위에 따로 저장해야 해 순서가 꼬인다).
pub fn request_removal(ctx: &egui::Context, app: &mut PdfViewerApp) {
    if app.bookmarks_dirty {
        app.ocr_needs_save = Some(PendingOcr::Remove);
        return;
    }
    let Some(pdf) = app.current_file.clone() else { return };
    if !pdf.exists() {
        app.status_message = Some("원본 PDF를 찾을 수 없습니다(이름이 바뀌었거나 이동/삭제됨).".to_string());
        return;
    }
    let job = Job::AnalyzeRemoval { pdf: pdf.clone() };
    app.ocr_job = Some(OcrJob::spawn(ctx, "OCR 전체 삭제 — 분석".to_string(), &job, JobKind::AnalyzeRemoval { pdf }, None));
}

/// 폴더 일괄 삭제 결과 문장.
fn describe_batch(r: &crate::ocr_worker::BatchReport) -> String {
    let mut lines = vec![format!(
        "PDF {}개 중 바꾼 파일 {}개, 지울 것이 없던 파일 {}개, 건너뛴 파일 {}개, 실패 {}개",
        r.total,
        r.changed.len(),
        r.unchanged.len(),
        r.skipped.len(),
        r.failed.len()
    )];
    for (name, pages, removed) in r.changed.iter().take(30) {
        lines.push(format!("  {name}: {pages}쪽에서 {removed}건 지움"));
    }
    if r.changed.len() > 30 {
        lines.push(format!("  … 외 {}개", r.changed.len() - 30));
    }
    for (name, reason) in r.skipped.iter().take(10) {
        lines.push(format!("  건너뜀 {name}: {reason}"));
    }
    for (name, reason) in r.failed.iter().take(10) {
        lines.push(format!("  실패 {name}: {reason}"));
    }
    lines.push("원본은 파일마다 '파일명.pdf-실행시각.backup'으로 보존했습니다.".to_string());
    if let Some(log) = &r.log {
        lines.push(format!("로그(CSV): {log}"));
    }
    lines.join("
")
}

/// 메뉴 "폴더 일괄 삭제…" — 폴더를 고르면 바로 시작한다(파일마다 백업 후 교체).
pub fn request_folder_removal(ctx: &egui::Context, app: &mut PdfViewerApp) {
    let folder = rfd::FileDialog::new().set_title("OCR을 지울 PDF 폴더 선택(하위 폴더 포함)").pick_folder();
    focus_window(ctx);
    let Some(folder) = folder else { return };
    let job = Job::RemoveFolder {
        folder,
        // 열려 있는 파일은 건드리지 않는다(문서 핸들·파일 감시와 충돌).
        skip: app.current_file.clone().into_iter().collect(),
    };
    app.ocr_job = Some(OcrJob::spawn(ctx, "OCR 폴더 일괄 삭제".to_string(), &job, JobKind::Batch, None));
}

fn show_needs_save_dialog(ctx: &egui::Context, app: &mut PdfViewerApp) {
    let Some(pending) = app.ocr_needs_save else { return };
    if crate::app::escape_to_close(ctx) {
        app.ocr_needs_save = None; // Esc = 취소
        return;
    }
    let (title, what) = match pending {
        PendingOcr::Remove => ("OCR 전체 삭제", "OCR 삭제"),
        PendingOcr::Import => ("OCR 가져오기", "OCR 가져오기"),
    };
    let mut action = None;
    egui::Window::new(title)
        .collapsible(false)
        .resizable(false)
        // 끌어서 옮길 수 있게 anchor 대신 pivot + default_pos를 쓴다(2026-09-28 요청).
        // anchor를 주면 egui가 매 프레임 위치를 다시 고정해 드래그가 먹지 않는다 —
        // 처음 뜰 때만 화면 가운데에 놓고, 그 뒤 위치는 egui가 창 id로 기억한다.
        .pivot(egui::Align2::CENTER_CENTER)
        .default_pos(ctx.screen_rect().center())
        .show(ctx, |ui| {
            ui.label("저장하지 않은 북마크 변경사항이 있습니다.");
            ui.label(format!("{what}는 파일을 새로 쓰므로 북마크를 먼저 PDF에 저장해야 합니다."));
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("저장 후 계속").clicked() {
                    action = Some(true);
                }
                if ui.button("취소").clicked() {
                    action = Some(false);
                }
            });
        });
    match action {
        Some(true) => {
            app.ocr_needs_save = None;
            if app.save_bookmarks_to_pdf() {
                match pending {
                    PendingOcr::Remove => request_removal(ctx, app),
                    PendingOcr::Import => request_import(ctx, app),
                }
            }
        }
        Some(false) => app.ocr_needs_save = None,
        None => {}
    }
}

/// 확인 창에 보여 줄 백업 안내. 실제 이름은 작업할 때 정해지므로 형식만 알려 준다.
const BACKUP_NOTE: &str = "원본은 같은 자리에 '파일명.pdf-실행시각.backup'으로 보존합니다(실행마다 따로 남습니다).";

fn show_removal_confirm(ctx: &egui::Context, app: &mut PdfViewerApp) {
    if app.ocr_removal_confirm.is_some() && crate::app::escape_to_close(ctx) {
        app.ocr_removal_confirm = None;
        return;
    }
    let Some(confirm) = app.ocr_removal_confirm.as_mut() else { return };
    let a = &confirm.analysis;
    let mut action = None;
    egui::Window::new("OCR 전체 삭제")
        .collapsible(false)
        .resizable(false)
        // 끌어서 옮길 수 있게 anchor 대신 pivot + default_pos를 쓴다(2026-09-28 요청).
        // anchor를 주면 egui가 매 프레임 위치를 다시 고정해 드래그가 먹지 않는다 —
        // 처음 뜰 때만 화면 가운데에 놓고, 그 뒤 위치는 egui가 창 id로 기억한다.
        .pivot(egui::Align2::CENTER_CENTER)
        .default_pos(ctx.screen_rect().center())
        .show(ctx, |ui| {
            ui.set_max_width(460.0);
            let removed = a.counts.removed() + a.own_layer_pages;
            if removed == 0 {
                ui.label("지울 보이지 않는 텍스트가 없습니다.");
            } else {
                ui.label(format!("{}쪽 중 {}쪽에서 보이지 않는 텍스트를 지웁니다.", a.pages, a.pages_with_hidden_text));
                let mut kinds = Vec::new();
                if a.own_layer_pages > 0 {
                    kinds.push(format!("이 앱이 넣은 OCR 레이어 {}쪽", a.own_layer_pages));
                }
                if !a.counts.0.is_empty() {
                    kinds.push(a.counts.describe());
                }
                ui.weak(kinds.join(" · "));
                ui.weak("지운 뒤 페이지마다 원본과 화면·보이는 텍스트를 비교해, 달라진 페이지는 원래대로 되돌립니다.");
            }
            if !a.skipped.is_empty() {
                ui.add_space(6.0);
                ui.label(format!("처리할 수 없어 그대로 둘 페이지: {}쪽", a.skipped.len()));
                for (page, reason) in a.skipped.iter().take(5) {
                    ui.weak(format!("  p.{page}: {reason}"));
                }
            }
            // OCR 텍스트 기준이 "이미지 영역 안이거나 걸친 3 Tr"이라, 같은 자리에 그렇게 넣은
            // 다른 텍스트(워터마크 자리표시자, 접근성용 텍스트 레이어)는 OCR이 만든 것과 구분할
            // 방법이 없다. 함께 지워진다는 사실을 알려 준다(2026-09-28 요청).
            if removed > 0 {
                ui.add_space(6.0);
                ui.colored_label(
                    ui.visuals().warn_fg_color,
                    "이 앱에서 추가하지 않은 대체텍스트, 워터마크가 있을 경우 함께 삭제될 수 있습니다.",
                );
            }

            ui.add_space(6.0);
            if a.signed {
                ui.colored_label(ui.visuals().warn_fg_color, "디지털 서명된 PDF입니다. 파일을 새로 쓰면 서명이 무효가 됩니다.");
                ui.checkbox(&mut confirm.signature_ack, "서명이 무효가 되는 것을 이해했습니다");
            }
            if a.tagged {
                ui.weak(format!(
                    "태그(접근성 구조)가 있는 PDF입니다. 태그 껍데기는 남겨 구조는 유지하지만, 내용이 비는 태그가 {}개 생깁니다.",
                    a.empty_tags
                ));
            }
            if let Some(pdfa) = &a.pdfa {
                ui.weak(format!("PDF/A-{pdfa} 선언은 유지하지만 규격 준수는 보장하지 않습니다. 외부 검증(veraPDF 등)을 권합니다."));
            }
            if a.linearized {
                ui.weak("빠른 웹 보기(선형화)가 해제됩니다.");
            }
            if removed > 0 {
                ui.weak(BACKUP_NOTE);
            }

            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if removed > 0 {
                    let allowed = !a.signed || confirm.signature_ack;
                    if ui.add_enabled(allowed, egui::Button::new("삭제")).clicked() {
                        action = Some(true);
                    }
                    if ui.button("취소").clicked() {
                        action = Some(false);
                    }
                } else if ui.button("닫기").clicked() {
                    action = Some(false);
                }
            });
        });

    match action {
        Some(true) => {
            let Some(confirm) = app.ocr_removal_confirm.take() else { return };
            let temp = confirm.pdf.with_extension("ocr_tmp.pdf");
            let job = Job::Remove { pdf: confirm.pdf.clone(), temp_output: temp.clone() };
            app.ocr_job = Some(OcrJob::spawn(
                ctx,
                "OCR 전체 삭제".to_string(),
                &job,
                JobKind::Remove { pdf: confirm.pdf },
                Some(temp),
            ));
        }
        Some(false) => app.ocr_removal_confirm = None,
        None => {}
    }
}

/// 검증을 통과한 임시 파일로 원본을 바꾼다: 백업 → 원자적 rename → 다시 열기. 리포트 문장 또는
/// 실패 이유(원본은 그대로).
fn finish_removal(app: &mut PdfViewerApp, pdf: &Path, temp: Option<&Path>, report: &RemovalReport) -> Result<String, String> {
    if report.nothing_to_do {
        if report.rolled_back.is_empty() {
            return Ok("지울 보이지 않는 텍스트가 없습니다. 파일을 바꾸지 않았습니다.".to_string());
        }
        return Ok(format!(
            "지울 수 있는 것이 없었습니다 — 지워 본 {}쪽 모두 화면이나 보이는 텍스트가 달라져 되돌렸습니다(파일을 바꾸지 않음).\n{}",
            report.rolled_back.len(),
            describe_removal(report, "원본 그대로")
        ));
    }
    let backup_note = swap_in_result(app, pdf, temp, "OCR 텍스트를 지웠습니다.", "OCR을 삭제할 수 없습니다")?;
    Ok(describe_removal(report, &backup_note))
}

/// 원본을 `.backup`으로 복사(이미 있으면 유지)하고 임시 파일로 바꾼 뒤 다시 연다. 백업 안내 문장
/// 또는 실패 이유(원본은 그대로).
///
/// `action`은 실패 문장에 들어갈 작업 이름(예: "OCR을 삭제할 수 없습니다") — 파일이 다른 앱에
/// 열려 있어 쓰기가 막히는 경우를 그 자리에 맞게 설명하기 위해서다(2026-09-27 요청).
fn swap_in_result(
    app: &mut PdfViewerApp,
    pdf: &Path,
    temp: Option<&Path>,
    status: &str,
    action: &str,
) -> Result<String, String> {
    let temp = temp.ok_or("결과 파일 경로가 없습니다.")?;
    // 쓰기가 권한 문제로 막히는 가장 흔한 원인은 그 파일을 다른 앱이 붙잡고 있는 것이다
    // (Windows는 열린 파일의 교체를 막는다). 원인을 짚어 주지 않으면 사용자가 손쓸 데가 없다.
    let write_blocked = |err: &std::io::Error| -> Option<String> {
        matches!(err.kind(), std::io::ErrorKind::PermissionDenied)
            .then(|| format!("파일이 다른 앱에서 열려 있어서 {action}. 원본은 바뀌지 않았습니다."))
    };
    // 이름에 시각이 들어가므로 늘 새 백업을 만든다 — 이전 백업은 그대로 남는다.
    let backup = crate::app::backup_path(pdf, &crate::app::backup_stamp());
    if let Err(err) = std::fs::copy(pdf, &backup) {
        let _ = std::fs::remove_file(temp);
        return Err(write_blocked(&err)
            .unwrap_or_else(|| format!("원본 백업 실패({err}) — 원본은 바뀌지 않았습니다.")));
    }
    let backup_note = format!("원본 백업: {}", crate::app::display_filename(&backup));
    if let Err(err) = std::fs::rename(temp, pdf) {
        let _ = std::fs::remove_file(temp);
        return Err(write_blocked(&err)
            .unwrap_or_else(|| format!("파일 교체 실패({err}) — 원본은 바뀌지 않았습니다.")));
    }
    if app.current_file.as_deref() == Some(pdf) {
        app.reload_current_document();
    }
    app.status_message = Some(status.to_string());
    Ok(backup_note)
}

// ---------------------------------------------------------------- 가져오기

use crate::ocr_import::{ImportAnalysis, ImportJob, ImportReport, PageKind, PdfPageInfo, ProblemMark};

/// 메뉴 "가져오기…": (북마크 저장 확인) → hOCR 파일 선택(여러 개면 파일 이름 자연 정렬) → 분석 작업.
pub fn request_import(ctx: &egui::Context, app: &mut PdfViewerApp) {
    if app.bookmarks_dirty {
        app.ocr_needs_save = Some(PendingOcr::Import);
        return;
    }
    let Some(pdf) = app.current_file.clone() else { return };
    if !pdf.exists() {
        app.status_message = Some("원본 PDF를 찾을 수 없습니다(이름이 바뀌었거나 이동/삭제됨).".to_string());
        return;
    }
    let files = rfd::FileDialog::new()
        .set_title("가져올 hOCR 파일 선택(페이지별 파일이면 여러 개)")
        .add_filter("hOCR", &["hocr", "html", "htm", "xhtml"])
        .pick_files();
    focus_window(ctx);
    let Some(mut files) = files else { return };
    files.sort_by(|a, b| {
        let name = |p: &PathBuf| crate::app::display_filename(p);
        pdf_ocr::hocr::parse::natural_cmp(&name(a), &name(b))
    });
    let job = Job::AnalyzeImport { pdf: pdf.clone(), hocr_files: files.clone() };
    app.ocr_job = Some(OcrJob::spawn(ctx, "OCR 가져오기 — 분석".to_string(), &job, JobKind::AnalyzeImport { pdf, files }, None));
}

/// 한 페이지를 어떻게 할지. 넣지 않는 페이지는 이유별 개수로만 알린다 — 페이지 목록과 모드
/// 라디오를 없애고 범위 입력만 남겼다(2026-09-28 개편).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PagePlan {
    Insert { overwrite: bool },
    Skip(SkipReason),
}

/// 넣지 않는 이유. 창에 적는 순서가 이 순서다(`Ord`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum SkipReason {
    /// 스캔이 없는 디지털 페이지 — OCR을 넣을 자리가 아니다.
    Digital,
    /// 스캔 위 본문 자리에 디지털 텍스트가 있어 어느 쪽인지 가릴 수 없다.
    Undetermined,
    /// 이미 OCR이 있는데 덮어쓰기를 껐다.
    Existing,
    /// hOCR과 가로세로 비율이 맞지 않는다(회전된 스캔이거나 다른 파일의 hOCR).
    Aspect,
}

fn skip_label(reason: SkipReason) -> &'static str {
    match reason {
        SkipReason::Digital => "디지털 페이지",
        SkipReason::Undetermined => "판단 불가",
        SkipReason::Existing => "이미 OCR이 있음",
        SkipReason::Aspect => "가로세로 비율 불일치",
    }
}

/// 이유별 건너뛸 쪽수를 한 줄로 — "디지털 페이지 3쪽 · 판단 불가 1쪽".
fn describe_skips(skipped: &std::collections::BTreeMap<SkipReason, usize>) -> String {
    skipped.iter().map(|(reason, n)| format!("{} {n}쪽", skip_label(*reason))).collect::<Vec<_>>().join(" · ")
}

/// 방금 만진 칸 — 네 칸을 서로 맞출 때 기준을 정한다(→ [`ImportDialog::sync`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Field {
    HocrFirst,
    HocrLast,
    PdfFirst,
    PdfLast,
}

fn has_existing(info: &PdfPageInfo) -> bool {
    info.existing_ocr || info.own_layer
}

/// 가져오기 설정 창 상태.
///
/// 페이지 대응은 **네 칸(hOCR 시작·끝 → PDF 시작·끝)** 으로 받는다(2026-09-28 개편). 네 칸은 서로
/// 묶여 있다 — 두 범위의 쪽수가 같아야 대응이 되므로, 한 칸을 고치면 나머지를 맞춘다([`Self::sync`]).
/// 칸은 늘 채워져 있어(빈 칸이 없어) "한쪽만 입력한 경우"는 생기지 않는다.
pub struct ImportDialog {
    pdf: PathBuf,
    files: Vec<PathBuf>,
    analysis: ImportAnalysis,
    /// 가져올 hOCR 범위(1부터, 양끝 포함).
    hocr_first: usize,
    hocr_last: usize,
    /// 그것을 놓을 PDF 범위(1부터, 양끝 포함). 쪽수는 hOCR 범위와 항상 같다.
    pdf_first: usize,
    pdf_last: usize,
    /// 이미 OCR이 있는 페이지를 덮어쓴다. 끄면 그 페이지는 **건너뛴다** — 지우지 않고 넣으면
    /// 같은 자리에 OCR이 두 겹으로 남는다.
    overwrite_existing: bool,
    signature_ack: bool,
}

impl ImportDialog {
    fn new(pdf: PathBuf, files: Vec<PathBuf>, analysis: ImportAnalysis) -> Self {
        let mut dialog = Self {
            hocr_first: 1,
            hocr_last: analysis.hocr_pages.len().max(1),
            pdf_first: 1,
            pdf_last: analysis.pdf_pages.len().max(1),
            overwrite_existing: true,
            signature_ack: false,
            pdf,
            files,
            analysis,
        };
        // 기본값은 "hOCR 전체를 PDF 1쪽부터" — 짧은 쪽 문서의 끝에서 멈춘다.
        dialog.sync(Field::HocrLast, 0);
        dialog
    }

    fn hocr_count(&self) -> usize {
        self.analysis.hocr_pages.len().max(1)
    }

    fn pdf_count(&self) -> usize {
        self.analysis.pdf_pages.len().max(1)
    }

    /// 대응되는 쪽수.
    fn count(&self) -> usize {
        self.pdf_last + 1 - self.pdf_first
    }

    /// 네 칸을 서로 맞춘다 — 방금 만진 칸을 살리고 나머지를 계산한다.
    ///
    /// - **시작 칸**을 옮긴 것은 "범위를 그대로 옮긴다"는 뜻이므로 쪽수(`count_before`)를 지킨다.
    /// - **끝 칸**을 옮긴 것은 "범위를 늘이거나 줄인다"는 뜻이므로 그 칸으로 쪽수를 다시 잡고,
    ///   반대쪽 범위는 시작 칸을 그대로 둔 채 같은 쪽수를 따른다.
    /// - 어느 쪽도 문서 끝을 넘지 않는 쪽수로 줄인다. 그래서 PDF 시작을 문서 끝 가까이 옮기면
    ///   쪽수가 함께 줄어든다(되돌려도 줄어든 쪽수는 그대로다 — 끝 칸으로 다시 늘리면 된다).
    fn sync(&mut self, edited: Field, count_before: usize) {
        let (hocr_n, pdf_n) = (self.hocr_count(), self.pdf_count());
        self.hocr_first = self.hocr_first.clamp(1, hocr_n);
        self.hocr_last = self.hocr_last.clamp(1, hocr_n);
        self.pdf_first = self.pdf_first.clamp(1, pdf_n);
        self.pdf_last = self.pdf_last.clamp(1, pdf_n);
        let count = match edited {
            Field::HocrFirst | Field::PdfFirst => count_before,
            // 끝을 시작보다 앞으로 끌어내렸으면 시작을 함께 당긴다(방금 만진 칸을 살린다).
            Field::HocrLast => {
                self.hocr_first = self.hocr_first.min(self.hocr_last);
                self.hocr_last + 1 - self.hocr_first
            }
            Field::PdfLast => {
                self.pdf_first = self.pdf_first.min(self.pdf_last);
                self.pdf_last + 1 - self.pdf_first
            }
        };
        let count = count.clamp(1, (hocr_n + 1 - self.hocr_first).min(pdf_n + 1 - self.pdf_first));
        self.hocr_last = self.hocr_first + count - 1;
        self.pdf_last = self.pdf_first + count - 1;
    }

    /// 대응되는 (PDF 페이지 0부터, hOCR 페이지 0부터).
    fn mapped(&self) -> Vec<(usize, usize)> {
        (0..self.count())
            .map(|j| (self.pdf_first - 1 + j, self.hocr_first - 1 + j))
            .filter(|(index, k)| {
                *index < self.analysis.pdf_pages.len() && *k < self.analysis.hocr_pages.len()
            })
            .collect()
    }

    fn aspect_ok(&self, index: usize, k: usize) -> bool {
        let (pw, ph) = self.analysis.pdf_pages[index].size;
        let (hw, hh) = self.analysis.hocr_pages[k].size;
        if pw <= 0.0 || ph <= 0.0 || hw <= 0.0 || hh <= 0.0 {
            return false;
        }
        ((pw / ph) / (hw / hh) - 1.0).abs() <= pdf_ocr::import::THRESHOLDS.aspect_tolerance
    }

    /// 페이지 하나의 결정.
    fn plan(&self, index: usize, k: usize) -> PagePlan {
        if !self.aspect_ok(index, k) {
            return PagePlan::Skip(SkipReason::Aspect);
        }
        let info = &self.analysis.pdf_pages[index];
        match info.kind {
            // 스캔이 없는 페이지에 OCR을 넣을 자리는 없다.
            PageKind::Digital => return PagePlan::Skip(SkipReason::Digital),
            // 스캔 위 본문 자리에 디지털 텍스트가 있는 페이지 — 넣으면 글자가 두 겹이 된다.
            // 예전에는 사용자가 목록에서 골랐지만, 고를 근거가 없어 건너뛰고 개수만 알리기로
            // 했다(2026-09-28 결정).
            PageKind::Undetermined => return PagePlan::Skip(SkipReason::Undetermined),
            _ => {}
        }
        if has_existing(info) {
            if !self.overwrite_existing {
                return PagePlan::Skip(SkipReason::Existing);
            }
            return PagePlan::Insert { overwrite: true };
        }
        PagePlan::Insert { overwrite: false }
    }

    /// 최종 결정: (넣을 페이지, 덮어쓸 페이지, 이유별 건너뛸 쪽수) — 페이지는 0부터.
    fn selection(&self) -> (Vec<usize>, Vec<usize>, std::collections::BTreeMap<SkipReason, usize>) {
        let (mut insert, mut overwrite, mut skipped) = (Vec::new(), Vec::new(), std::collections::BTreeMap::new());
        for (index, k) in self.mapped() {
            match self.plan(index, k) {
                PagePlan::Insert { overwrite: ow } => {
                    insert.push(index);
                    if ow {
                        overwrite.push(index);
                    }
                }
                PagePlan::Skip(reason) => *skipped.entry(reason).or_insert(0) += 1,
            }
        }
        (insert, overwrite, skipped)
    }
}

fn show_import_dialog(ctx: &egui::Context, app: &mut PdfViewerApp) {
    if app.ocr_import_dialog.is_some() && crate::app::escape_to_close(ctx) {
        app.ocr_import_dialog = None;
        return;
    }
    let Some(dialog) = app.ocr_import_dialog.as_mut() else { return };
    let mut action = None;
    egui::Window::new("OCR 가져오기")
        .collapsible(false)
        .resizable(false)
        // 끌어서 옮길 수 있게 anchor 대신 pivot + default_pos를 쓴다(2026-09-28 요청).
        // anchor를 주면 egui가 매 프레임 위치를 다시 고정해 드래그가 먹지 않는다 —
        // 처음 뜰 때만 화면 가운데에 놓고, 그 뒤 위치는 egui가 창 id로 기억한다.
        .pivot(egui::Align2::CENTER_CENTER)
        .default_pos(ctx.screen_rect().center())
        .show(ctx, |ui| {
            ui.set_max_width(520.0);
            let (hocr_count, pdf_count) = (dialog.hocr_count(), dialog.pdf_count());
            {
                let a = &dialog.analysis;
                ui.label(format!(
                    "hOCR {}쪽(파일 {}개{}) → PDF {}쪽",
                    a.hocr_pages.len(),
                    dialog.files.len(),
                    if a.ordered_by_ppageno { ", ppageno 순서" } else { "" },
                    a.pdf_pages.len()
                ));
                // 주어를 밝힌다 — 빠진 항목은 hOCR 쪽 이야기다(2026-09-28 요청).
                if a.dropped_items + a.empty_words > 0 {
                    ui.weak(format!(
                        "hOCR에서 위치 정보가 없거나 비어 있어 뺀 항목: {}개",
                        a.dropped_items + a.empty_words
                    ));
                }
                if a.hocr_pages.len() != a.pdf_pages.len() {
                    ui.colored_label(ui.visuals().warn_fg_color, "hOCR과 PDF의 페이지 수가 다릅니다.");
                }
                // 같은 페이지의 hOCR을 여러 개 골랐을 때 — 중복이 페이지 하나씩 차지해 뒤의 대응이
                // 통째로 밀리므로, 조용히 넘기지 않고 짚어 준다(2026-09-27 요청).
                if !a.duplicate_page_numbers.is_empty() {
                    ui.colored_label(
                        ui.visuals().error_fg_color,
                        format!(
                            "같은 페이지의 hOCR을 여러 개 고른 것으로 보입니다 — 겹치는 페이지: {}. \
                             이대로 넣으면 뒤쪽 대응이 한 칸씩 밀립니다. 파일 선택을 다시 확인하세요.",
                            summarize_pages(&a.duplicate_page_numbers)
                        ),
                    );
                }
            }

            // 페이지 범위: hOCR 어디부터 어디까지를 PDF 어디부터 어디까지에 넣을지. 네 칸은 서로
            // 묶여 있어(쪽수가 같아야 한다) 한 칸을 고치면 나머지가 따라온다(→ `ImportDialog::sync`).
            ui.add_space(6.0);
            let count_before = dialog.count();
            let mut edited = None;
            ui.horizontal(|ui| {
                ui.label("hOCR");
                if ui.add(egui::DragValue::new(&mut dialog.hocr_first).range(1..=hocr_count)).changed() {
                    edited = Some(Field::HocrFirst);
                }
                ui.label("~");
                if ui.add(egui::DragValue::new(&mut dialog.hocr_last).range(1..=hocr_count)).changed() {
                    edited = Some(Field::HocrLast);
                }
                ui.label("쪽   →   PDF");
                if ui.add(egui::DragValue::new(&mut dialog.pdf_first).range(1..=pdf_count)).changed() {
                    edited = Some(Field::PdfFirst);
                }
                ui.label("~");
                if ui.add(egui::DragValue::new(&mut dialog.pdf_last).range(1..=pdf_count)).changed() {
                    edited = Some(Field::PdfLast);
                }
                ui.label("쪽");
            });
            if let Some(field) = edited {
                dialog.sync(field, count_before);
            }
            ui.weak(format!("대응되는 쪽수: {}쪽", dialog.count()));

            let mapped = dialog.mapped();
            let bad_aspect: Vec<usize> = mapped.iter().filter(|(i, k)| !dialog.aspect_ok(*i, *k)).map(|(i, _)| i + 1).collect();
            if !bad_aspect.is_empty() {
                // 대응되는 **모든** 페이지가 어긋나면 회전이나 한두 쪽의 문제가 아니라 아예 다른
                // PDF의 hOCR일 가능성이 크다 — 그때는 다른 문장으로 말한다(2026-09-27 요청).
                if bad_aspect.len() == mapped.len() {
                    ui.colored_label(
                        ui.visuals().error_fg_color,
                        "이 PDF의 hOCR이 아닌 것 같습니다 — 대응되는 모든 페이지의 가로세로 비율이 맞지 않습니다.",
                    );
                } else {
                    ui.colored_label(
                        ui.visuals().warn_fg_color,
                        format!(
                            "가로세로 비율이 맞지 않아 건너뛸 페이지 {}쪽(회전된 스캔이거나 다른 파일의 hOCR일 수 있음): {}",
                            bad_aspect.len(),
                            summarize_pages(&bad_aspect)
                        ),
                    );
                }
            }

            // 페이지 분류별 개수는 적지 않는다(2026-09-28 결정) — 분류 이름을 알아도 사용자가
            // 할 일이 없다. 대신 아래에 "빠지는 페이지"만 이유별로 센다.
            ui.add_space(6.0);
            ui.checkbox(&mut dialog.overwrite_existing, "OCR이 있는 페이지는 덮어씌움").on_hover_text(
                "보이지 않는 텍스트가 이미 있는 페이지는 그것을 지우고 새 OCR을 넣습니다. \
                 끄면 그 페이지는 건너뜁니다 — 지우지 않고 넣으면 OCR이 두 겹으로 남습니다.",
            );

            {
                let a = &dialog.analysis;
                if a.signed {
                    ui.colored_label(ui.visuals().warn_fg_color, "디지털 서명된 PDF입니다. 파일을 새로 쓰면 서명이 무효가 됩니다.");
                    ui.checkbox(&mut dialog.signature_ack, "서명이 무효가 되는 것을 이해했습니다");
                }
                if a.tagged {
                    ui.weak("태그(접근성 구조)가 있는 PDF입니다. 넣는 텍스트는 태그 구조에 들어가지 않습니다.");
                }
                if let Some(pdfa) = &a.pdfa {
                    ui.weak(format!("PDF/A-{pdfa} 선언은 유지하지만 규격 준수는 보장하지 않습니다. 외부 검증(veraPDF 등)을 권합니다."));
                }
            }

            let (insert, overwrite, skipped) = dialog.selection();
            ui.add_space(6.0);
            ui.label(format!(
                "넣을 페이지 {}쪽(그중 기존 OCR 덮어쓰기 {}쪽)",
                insert.len(),
                overwrite.len()
            ));
            // 페이지 목록이 없으니(2026-09-28 개편) 빠지는 페이지는 이유별 개수로 알린다 —
            // 조용히 빠지면 사용자가 알 길이 없다.
            if !skipped.is_empty() {
                ui.weak(format!("건너뛸 페이지 — {}", describe_skips(&skipped)));
            }
            ui.weak(BACKUP_NOTE);
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let allowed = !insert.is_empty() && (!dialog.analysis.signed || dialog.signature_ack);
                if ui.add_enabled(allowed, egui::Button::new("가져오기")).clicked() {
                    action = Some(true);
                }
                if ui.button("취소").clicked() {
                    action = Some(false);
                }
            });
        });

    match action {
        Some(true) => {
            let Some(dialog) = app.ocr_import_dialog.take() else { return };
            let (insert_pages, overwrite_pages, skipped) = dialog.selection();
            let temp = dialog.pdf.with_extension("ocr_tmp.pdf");
            let job = ImportJob {
                pdf: dialog.pdf.clone(),
                hocr_files: dialog.files.clone(),
                temp_output: temp.clone(),
                hocr_first: dialog.hocr_first,
                hocr_last: dialog.hocr_last,
                pdf_first: dialog.pdf_first,
                insert_pages,
                overwrite_pages,
            };
            app.ocr_job = Some(OcrJob::spawn(
                ctx,
                "OCR 가져오기".to_string(),
                &Job::Import(job),
                JobKind::Import { pdf: dialog.pdf, skipped: describe_skips(&skipped) },
                Some(temp),
            ));
        }
        Some(false) => app.ocr_import_dialog = None,
        None => {}
    }
}

/// 페이지 번호 목록을 "1-3, 7, 9-10"처럼 줄인다(최대 10구간).
fn summarize_pages(pages: &[usize]) -> String {
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    for &p in pages {
        match ranges.last_mut() {
            Some((_, end)) if *end + 1 == p => *end = p,
            _ => ranges.push((p, p)),
        }
    }
    let mut parts: Vec<String> =
        ranges.iter().take(10).map(|(a, b)| if a == b { a.to_string() } else { format!("{a}-{b}") }).collect();
    if ranges.len() > 10 {
        parts.push("…".to_string());
    }
    parts.join(", ")
}

fn describe_import(r: &ImportReport, skipped_by_plan: &str, backup_note: &str) -> String {
    let mut lines = Vec::new();
    if !skipped_by_plan.is_empty() {
        lines.push(format!("대상에서 뺀 페이지 — {skipped_by_plan}"));
    }
    if r.nothing_to_do {
        lines.push("넣을 단어가 있는 페이지가 없습니다.".to_string());
    } else {
        lines.push(format!("넣은 페이지: {}쪽, 단어 {}개", r.pages_inserted, r.words_inserted));
        if r.pages_overwritten > 0 {
            lines.push(format!("기존 OCR을 지우고 넣은 페이지: {}쪽", r.pages_overwritten));
        }
        lines.push(format!("파일 크기: {} → {}", human_size(r.size_before), human_size(r.size_after)));
    }
    if r.dedupe_same + r.dedupe_conflicts + r.dedupe_kept_over_damaged > 0 {
        lines.push(format!(
            "중복 제거: 디지털 텍스트와 같아 뺀 단어 {}, 내용이 달라 디지털을 우선한 단어 {}, 디지털 손상 의심으로 남긴 단어 {}",
            r.dedupe_same, r.dedupe_conflicts, r.dedupe_kept_over_damaged
        ));
        for (page, ocr, digital) in r.dedupe_samples.iter().take(5) {
            lines.push(format!("  p.{page}: OCR \"{ocr}\" ↔ 디지털 \"{digital}\""));
        }
    }
    if !r.skipped.is_empty() {
        lines.push(format!("넣지 못한 페이지: {}쪽", r.skipped.len()));
        lines.extend(r.skipped.iter().take(20).map(|(p, why)| format!("  p.{p}: {why}")));
    }
    if !r.rolled_back.is_empty() {
        lines.push(format!("검증에서 원본과 달라 되돌린 페이지: {}쪽", r.rolled_back.len()));
        lines.extend(r.rolled_back.iter().take(20).map(|(p, why)| format!("  p.{p}: {why}")));
    }
    let warnings = r.marks.iter().filter(|m| !m.rolled_back).count();
    if warnings > 0 {
        lines.push(format!(
            "확인 권장: {warnings}쪽 — 같은 글자가 겹친 자리가 있어 뷰어에서 한 글자로 추출됩니다(아래 목록의 '보기')"
        ));
    }
    if !r.also_reverted.is_empty() {
        lines.push(format!(
            "같은 Form을 써서 함께 원래대로 둔 페이지: {}",
            r.also_reverted.iter().map(|p| p.to_string()).collect::<Vec<_>>().join(", ")
        ));
    }
    lines.push(backup_note.to_string());
    lines.join("\n")
}

fn describe_removal(r: &RemovalReport, backup_note: &str) -> String {
    let a = &r.analysis;
    let mut lines = vec![
        format!("바뀐 페이지: {}쪽 / 전체 {}쪽", r.pages_changed, a.pages),
        format!(
            "지운 텍스트: {}건({}){}",
            a.counts.removed(),
            a.counts.describe(),
            if a.own_layer_pages > 0 { format!(" + 이 앱이 넣은 레이어 {}쪽", a.own_layer_pages) } else { String::new() }
        ),
        format!("파일 크기: {} → {}", human_size(r.size_before), human_size(r.size_after)),
    ];
    if r.analysis.empty_tags > 0 {
        lines.push(format!("내용이 비게 된 태그: {}개(구조는 유지)", r.analysis.empty_tags));
    }
    if r.pruned_layers > 0 {
        lines.push(format!("빈 레이어 정리: {}개", r.pruned_layers));
    }
    if !r.rolled_back.is_empty() {
        lines.push(format!("검증에서 원본과 달라 되돌린 페이지: {}쪽", r.rolled_back.len()));
        lines.extend(r.rolled_back.iter().take(20).map(|(p, why)| format!("  p.{p}: {why}")));
    }
    if !r.also_reverted.is_empty() {
        lines.push(format!(
            "같은 Form을 써서 함께 원래대로 둔 페이지: {}",
            r.also_reverted.iter().map(|p| p.to_string()).collect::<Vec<_>>().join(", ")
        ));
    }
    if !a.skipped.is_empty() {
        lines.push(format!("처리할 수 없어 그대로 둔 페이지: {}쪽", a.skipped.len()));
        lines.extend(a.skipped.iter().take(20).map(|(p, why)| format!("  p.{p}: {why}")));
    }
    for (page, note) in a.notes.iter().take(10) {
        lines.push(format!("참고 p.{page}: {note}"));
    }
    lines.push(backup_note.to_string());
    lines.join("\n")
}

fn human_size(bytes: u64) -> String {
    if bytes >= 1 << 20 {
        format!("{:.1}MB", bytes as f64 / (1 << 20) as f64)
    } else {
        format!("{:.0}KB", bytes as f64 / 1024.0)
    }
}

pub fn show(ctx: &egui::Context, app: &mut PdfViewerApp) {
    show_export_dialog(ctx, app);
    show_needs_save_dialog(ctx, app);
    show_removal_confirm(ctx, app);
    show_import_dialog(ctx, app);
    show_job_window(ctx, app);
}

fn show_export_dialog(ctx: &egui::Context, app: &mut PdfViewerApp) {
    if app.ocr_export_dialog.is_some() && crate::app::escape_to_close(ctx) {
        app.ocr_export_dialog = None;
        return;
    }
    let Some(dialog) = app.ocr_export_dialog.as_mut() else { return };
    let (title, extension) = match dialog.format {
        ExportFormat::Hocr => ("OCR 텍스트를 hOCR로 내보내기", "hocr"),
        ExportFormat::Txt => ("OCR 텍스트를 txt로 내보내기", "txt"),
    };
    let mut close = false;
    let mut start = false;
    egui::Window::new(title)
        .collapsible(false)
        .resizable(false)
        // 끌어서 옮길 수 있게 anchor 대신 pivot + default_pos를 쓴다(2026-09-28 요청).
        // anchor를 주면 egui가 매 프레임 위치를 다시 고정해 드래그가 먹지 않는다 —
        // 처음 뜰 때만 화면 가운데에 놓고, 그 뒤 위치는 egui가 창 id로 기억한다.
        .pivot(egui::Align2::CENTER_CENTER)
        .default_pos(ctx.screen_rect().center())
        .show(ctx, |ui| {
            // 내보낼 텍스트가 아예 없으면 **선택지를 모두 감춘다**(2026-09-27 요청) — 고를 것이
            // 없는 화면에 잠긴 위젯만 늘어놓아 봐야 읽을 거리만 늘어난다.
            let kinds = dialog.text_kinds;
            let nothing_to_export = kinds.is_some_and(|k| !k.visible && !k.invisible);
            if nothing_to_export {
                ui.colored_label(ui.visuals().warn_fg_color, "이 문서에는 내보낼 텍스트가 없습니다.");
            } else {
                ui.label("내보낼 텍스트");
                // 의미 없는 선택지는 잠근다. 확인이 끝나기 전(`None`)에는 둘 다 열어 둔다 —
                // 그동안 사용자가 골라 진행해도 결과는 달라지지 않는다.
                //
                //          보이는 텍스트   보이지 않는 텍스트   →  선택지
                //  섞여 있음      O              O             둘 다 열림
                //  순수 스캔+OCR  ×              O             둘 다 잠금(결과가 같다)
                //  순수 디지털    O              ×             "보이지 않는 텍스트만" 잠금(빈 파일)
                //  텍스트 없음    ×              ×             위에서 이미 감췄다
                let allow = |wanted: bool| kinds.is_none_or(|k| if wanted { k.invisible } else { k.visible });
                let same_either_way = kinds.is_some_and(|k| !k.visible);
                ui.add_enabled_ui(!same_either_way && allow(true), |ui| {
                    ui.radio_value(&mut dialog.invisible_only, true, "보이지 않는 텍스트만(OCR 레이어)");
                });
                ui.add_enabled_ui(!same_either_way && allow(false), |ui| {
                    ui.radio_value(&mut dialog.invisible_only, false, "페이지의 모든 텍스트");
                });
                if same_either_way {
                    ui.weak("이 문서에는 보이는 텍스트가 없어 어느 쪽을 골라도 결과가 같습니다.");
                } else if kinds.is_some_and(|k| !k.invisible) {
                    ui.weak("이 문서에는 보이지 않는 텍스트(OCR 레이어)가 없습니다.");
                }
                if dialog.format == ExportFormat::Txt {
                    ui.add_space(6.0);
                    ui.checkbox(&mut dialog.txt_page_labels, "페이지 레이블 함께 표기")
                        .on_hover_text("PDF의 페이지 레이블이 물리 번호와 다르면 === [p. 12 | xii] === 처럼 함께 적습니다.");
                    ui.checkbox(&mut dialog.txt_crlf, "줄바꿈을 CRLF로(Windows 호환)");
                    ui.checkbox(&mut dialog.txt_form_feed, "페이지 사이에 폼피드 넣기(pdftotext 호환)");
                    ui.add_space(4.0);
                    ui.weak("txt에는 위치 정보가 없어 OCR 가져오기에 쓸 수 없습니다.");
                } else {
                    ui.add_space(4.0);
                    ui.weak("좌표는 화면에 보이는 페이지 기준(회전 반영), 300 DPI로 환산합니다.");
                }
            }
            ui.add_space(8.0);
            // 확인이 끝나기 전에 눌렀으면 **누른 것을 기억해 두고 답을 기다린다**(2026-09-27 요청).
            // 그냥 진행시키면 안 된다 — 기본값이 "보이지 않는 텍스트만"이라, 순수 디지털 문서에서
            // 빨리 누르면 빈 파일이 나온다. 버튼을 처음부터 잠그는 쪽은 택하지 않았다(창을 바로
            // 띄운 취지가 없어진다) — 기다리는 동안 txt 옵션은 계속 만질 수 있다.
            ui.horizontal(|ui| {
                if dialog.start_pending {
                    ui.spinner();
                    ui.label("텍스트 종류 확인 중… 끝나면 이어서 진행합니다");
                } else if ui
                    .add_enabled(!nothing_to_export, egui::Button::new("내보내기…"))
                    .clicked()
                {
                    if dialog.text_kinds.is_some() {
                        start = true;
                    } else {
                        dialog.start_pending = true;
                    }
                }
                if ui.button("취소").clicked() {
                    close = true;
                }
            });
        });

    // 기다리던 클릭 — 확인이 끝났으니 이제 시작한다. 그 사이 선택지가 잠기면서 선택이 바뀌었을 수
    // 있는데, 그게 바로 이 가드가 막으려던 것이다(빈 파일).
    if let Some(dialog) = app.ocr_export_dialog.as_mut().filter(|_| !close) {
        if dialog.start_pending && dialog.text_kinds.is_some() {
            dialog.start_pending = false;
            start = true;
        }
    }

    if start {
        let default_name = app
            .current_file
            .as_deref()
            .and_then(Path::file_stem)
            .map(|s| format!("{}.{extension}", crate::app::display_filename(Path::new(s))))
            .unwrap_or_else(|| format!("ocr.{extension}"));
        let mut file_dialog = rfd::FileDialog::new().set_file_name(&default_name);
        file_dialog = match extension {
            "hocr" => file_dialog.add_filter("hOCR", &["hocr", "html"]),
            _ => file_dialog.add_filter("텍스트", &["txt"]),
        };
        let output = file_dialog.save_file();
        focus_window(ctx);
        if let Some(output) = output {
            start_export(ctx, app, output);
        }
        // 저장 대화상자를 취소하면 옵션 창을 그대로 둔다.
    } else if close {
        app.ocr_export_dialog = None;
    }
}

fn start_export(ctx: &egui::Context, app: &mut PdfViewerApp, output: PathBuf) {
    let Some(dialog) = app.ocr_export_dialog.take() else { return };
    let Some(pdf) = app.current_file.clone() else { return };
    if !pdf.exists() {
        app.status_message = Some("원본 PDF를 찾을 수 없습니다(이름이 바뀌었거나 이동/삭제됨).".to_string());
        return;
    }
    let temp_output = partial_path(&output);
    let job = ExportJob {
        pdf,
        output: output.clone(),
        temp_output: temp_output.clone(),
        format: dialog.format,
        invisible_only: dialog.invisible_only,
        txt_crlf: dialog.txt_crlf,
        txt_form_feed: dialog.txt_form_feed,
        txt_page_labels: dialog.txt_page_labels,
    };
    let format_label = match dialog.format {
        ExportFormat::Hocr => "hOCR",
        ExportFormat::Txt => "txt",
    };
    let source_label = if dialog.invisible_only { "보이지 않는 텍스트만" } else { "모든 텍스트" };
    let invisible_only = dialog.invisible_only;
    let output_label = output.display().to_string();
    let describe = move |r: &ExportReport| describe_export(r, format_label, source_label, invisible_only, &output_label);

    app.ocr_job = Some(OcrJob::spawn(
        ctx,
        format!("OCR 텍스트 내보내기({format_label})"),
        &Job::Export(job),
        JobKind::Export { describe: Box::new(describe) },
        Some(temp_output),
    ));
}

/// `결과.hocr` → `결과.hocr.partial`(같은 폴더라 끝의 rename이 원자적이다).
fn partial_path(output: &Path) -> PathBuf {
    let mut name = output.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    name.push(".partial");
    output.with_file_name(name)
}

fn describe_export(r: &ExportReport, format: &str, source: &str, invisible_only: bool, output: &str) -> String {
    let mut lines = vec![
        format!("형식: {format} ({source})"),
        format!("페이지: {}쪽 중 텍스트 있음 {}쪽, 단어 {}개", r.pages, r.pages_with_text, r.words),
    ];
    if r.clamped_chars > 0 {
        lines.push(format!("글자 높이가 비정상적으로 커서 보정한 글자: {}개", r.clamped_chars));
    }
    if !r.failed_pages.is_empty() {
        lines.push(format!("읽지 못해 빈 페이지로 기록한 페이지: {}쪽", r.failed_pages.len()));
        for (page, reason) in r.failed_pages.iter().take(20) {
            lines.push(format!("  p.{page}: {reason}"));
        }
        if r.failed_pages.len() > 20 {
            lines.push(format!("  … 외 {}쪽", r.failed_pages.len() - 20));
        }
    }
    if r.pages_with_text == 0 && invisible_only {
        lines.push("보이지 않는 텍스트가 없습니다. '페이지의 모든 텍스트'로 다시 내보내 보세요.".to_string());
    }
    lines.push(format!("저장 위치: {output}"));
    lines.join("\n")
}

fn show_job_window(ctx: &egui::Context, app: &mut PdfViewerApp) {
    let Some(job) = app.ocr_job.as_mut() else { return };
    // 도는 중에는 Esc를 받지 않는다 — 긴 작업이 실수로 취소되지 않게. 끝난 뒤 결과 창에서만
    // 닫힌다(2026-09-27 요청).
    let mut close = job.is_finished() && crate::app::escape_to_close(ctx);
    let mut show_mark: Option<ProblemMark> = None;
    // 고정하지 않는다 — 결과의 "보기"로 페이지를 확인할 때 창을 옆으로 옮길 수 있게.
    egui::Window::new(job.title.clone())
        .collapsible(false)
        .resizable(false)
        .pivot(egui::Align2::CENTER_CENTER)
        .default_pos(ctx.screen_rect().center())
        .show(ctx, |ui| match &job.phase {
            JobPhase::Running { done, total } => {
                if let Some(stage) = &job.stage {
                    ui.label(stage);
                }
                let fraction = if *total > 0 { *done as f32 / *total as f32 } else { 0.0 };
                let text = if *total > 0 { format!("{done} / {total}쪽") } else { "시작하는 중…".to_string() };
                ui.add(egui::ProgressBar::new(fraction).text(text).desired_width(320.0));
                ui.add_space(8.0);
                if ui.button("취소").clicked() {
                    job.cancel();
                    close = true;
                }
            }
            JobPhase::Finished(report) | JobPhase::Failed(report) => {
                if matches!(job.phase, JobPhase::Failed(_)) {
                    ui.colored_label(ui.visuals().error_fg_color, "실패했습니다. 원본 PDF는 바뀌지 않았습니다.");
                    ui.add_space(4.0);
                }
                egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                    ui.add(egui::Label::new(report.as_str()).selectable(true));
                });
                if !job.marks.is_empty() {
                    ui.add_space(6.0);
                    ui.label("확인할 자리 — '보기'를 누르면 그 페이지에 빨간 테두리로 표시합니다(창은 끌어서 옮길 수 있음)");
                    egui::ScrollArea::vertical().id_salt("ocr_marks").max_height(160.0).show(ui, |ui| {
                        for mark in &job.marks {
                            ui.horizontal(|ui| {
                                if ui.small_button("보기").clicked() {
                                    show_mark = Some(mark.clone());
                                }
                                let color = if mark.rolled_back { ui.visuals().error_fg_color } else { ui.visuals().warn_fg_color };
                                ui.colored_label(color, format!("p.{}", mark.page));
                                ui.label(&mark.note);
                            });
                        }
                    });
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("복사").clicked() {
                        ui.output_mut(|o| o.copied_text = report.clone());
                    }
                    if ui.button("닫기").clicked() {
                        close = true;
                    }
                });
            }
        });
    if let Some(mark) = show_mark {
        let page = mark.page as u32;
        app.ocr_mark = Some(mark);
        app.go_to_page(page);
    }
    if close {
        app.ocr_mark = None;
        if let Some(mut job) = app.ocr_job.take() {
            job.cancel();
        }
    }
}

/// 가져오기 창의 페이지 범위 네 칸과 페이지별 결정(2026-09-28 개편).
#[cfg(test)]
mod import_range_tests {
    use super::*;
    use crate::ocr_import::{HocrPageInfo, PdfPageInfo};

    fn pdf_page(kind: PageKind, existing: bool) -> PdfPageInfo {
        PdfPageInfo {
            kind,
            digital_chars: 0,
            existing_ocr: existing,
            own_layer: false,
            damaged_digital: false,
            image_coverage: 0.95,
            size: (600.0, 800.0),
        }
    }

    /// hOCR과 PDF의 가로세로 비율을 같게 맞춘 분석 결과 — 비율 불일치로 빠지지 않게.
    fn dialog(pages: Vec<PdfPageInfo>, hocr: usize) -> ImportDialog {
        let analysis = ImportAnalysis {
            pdf_pages: pages,
            hocr_pages: (0..hocr).map(|_| HocrPageInfo { size: (1200.0, 1600.0), words: 3 }).collect(),
            ..Default::default()
        };
        ImportDialog::new(PathBuf::from("a.pdf"), vec![PathBuf::from("a.hocr")], analysis)
    }

    fn scans(n: usize) -> Vec<PdfPageInfo> {
        (0..n).map(|_| pdf_page(PageKind::NoText, false)).collect()
    }

    fn range(d: &ImportDialog) -> (usize, usize, usize, usize) {
        (d.hocr_first, d.hocr_last, d.pdf_first, d.pdf_last)
    }

    #[test]
    fn defaults_put_the_whole_hocr_from_the_first_page() {
        assert_eq!(range(&dialog(scans(10), 5)), (1, 5, 1, 5));
    }

    /// PDF가 더 짧으면 기본값이 그 끝에서 멈춘다("가능한 최종 페이지번호").
    #[test]
    fn defaults_stop_at_the_shorter_document() {
        assert_eq!(range(&dialog(scans(4), 10)), (1, 4, 1, 4));
    }

    /// 끝 칸을 만진 것은 "범위를 줄인다"는 뜻 — 반대쪽 끝이 따라온다.
    #[test]
    fn moving_an_end_changes_the_count_on_both_sides() {
        let mut d = dialog(scans(10), 10);
        let before = d.count();
        d.hocr_last = 4;
        d.sync(Field::HocrLast, before);
        assert_eq!(range(&d), (1, 4, 1, 4));
    }

    /// 시작 칸을 만진 것은 "범위를 옮긴다"는 뜻 — 쪽수를 지킨다.
    #[test]
    fn moving_a_start_keeps_the_count() {
        let mut d = dialog(scans(20), 10);
        let before = d.count();
        d.pdf_first = 6;
        d.sync(Field::PdfFirst, before);
        assert_eq!(range(&d), (1, 10, 6, 15));
    }

    /// 옮긴 범위가 문서 끝을 넘으면 쪽수를 줄인다 — 넘어간 쪽을 조용히 버리지 않는다.
    #[test]
    fn a_range_pushed_past_the_end_shrinks() {
        let mut d = dialog(scans(10), 10);
        let before = d.count();
        d.pdf_first = 8;
        d.sync(Field::PdfFirst, before);
        assert_eq!(range(&d), (1, 3, 8, 10));
    }

    #[test]
    fn moving_the_hocr_start_narrows_both_ranges() {
        let mut d = dialog(scans(10), 10);
        let before = d.count();
        d.hocr_first = 4;
        d.sync(Field::HocrFirst, before);
        assert_eq!(range(&d), (4, 10, 1, 7));
    }

    /// 끝을 시작보다 앞으로 끌어내리면 방금 만진 칸을 살리고 시작을 당긴다.
    #[test]
    fn dragging_an_end_below_its_start_pulls_the_start_down() {
        let mut d = dialog(scans(10), 10);
        let before = d.count();
        d.pdf_first = 6;
        d.sync(Field::PdfFirst, before);
        assert_eq!(range(&d), (1, 5, 6, 10));
        let before = d.count();
        d.pdf_last = 3;
        d.sync(Field::PdfLast, before);
        assert_eq!(range(&d), (1, 1, 3, 3));
    }

    /// 디지털 페이지와 판단 불가 페이지는 넣지 않고 이유별로 센다 — 목록으로 고르게 하지 않는다
    /// (2026-09-28 결정: 판단 불가 페이지는 고를 근거가 없다).
    #[test]
    fn digital_and_undetermined_pages_are_skipped_with_a_reason() {
        let pages = vec![
            pdf_page(PageKind::NoText, false),
            pdf_page(PageKind::Digital, false),
            pdf_page(PageKind::Undetermined, false),
            pdf_page(PageKind::ScanWithExtras, false),
        ];
        let (insert, overwrite, skipped) = dialog(pages, 4).selection();
        assert_eq!(insert, vec![0, 3]);
        assert!(overwrite.is_empty());
        assert_eq!(skipped.get(&SkipReason::Digital), Some(&1));
        assert_eq!(skipped.get(&SkipReason::Undetermined), Some(&1));
    }

    /// 이미 OCR이 있는 페이지는 덮어쓰거나 건너뛴다 — 지우지 않고 넣어 두 겹으로 남기지 않는다.
    #[test]
    fn existing_ocr_is_overwritten_or_skipped_but_never_stacked() {
        let pages = vec![pdf_page(PageKind::ExistingOcrOnly, true), pdf_page(PageKind::NoText, false)];
        let mut d = dialog(pages, 2);
        let (insert, overwrite, _) = d.selection();
        assert_eq!((insert, overwrite), (vec![0, 1], vec![0]));
        d.overwrite_existing = false;
        let (insert, overwrite, skipped) = d.selection();
        assert_eq!(insert, vec![1]);
        assert!(overwrite.is_empty());
        assert_eq!(skipped.get(&SkipReason::Existing), Some(&1));
    }

    /// 창이 정한 범위와 작업 프로세스의 대응이 같은 뜻이어야 한다.
    #[test]
    fn the_job_maps_the_same_pages_as_the_dialog() {
        let mut d = dialog(scans(6), 6);
        let before = d.count();
        d.hocr_first = 3;
        d.sync(Field::HocrFirst, before);
        assert_eq!(range(&d), (3, 6, 1, 4));
        let job = crate::ocr_import::ImportJob {
            pdf: PathBuf::from("a.pdf"),
            hocr_files: Vec::new(),
            temp_output: PathBuf::from("t.pdf"),
            hocr_first: d.hocr_first,
            hocr_last: d.hocr_last,
            pdf_first: d.pdf_first,
            insert_pages: Vec::new(),
            overwrite_pages: Vec::new(),
        };
        // 창의 대응 쌍(PDF 0부터, hOCR 0부터)과 작업 쪽 계산이 하나도 어긋나지 않아야 한다.
        for (index, k) in d.mapped() {
            assert_eq!(job.pdf_index_for(k), Some(index));
        }
        assert_eq!(job.pdf_index_for(0), None); // hOCR 1쪽 — 범위 밖
        assert_eq!(job.pdf_index_for(6), None); // hOCR 7쪽 — 문서 밖
    }
}
