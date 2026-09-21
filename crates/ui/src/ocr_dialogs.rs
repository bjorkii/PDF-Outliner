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

/// 내보내기 옵션 대화상자 상태. 형식은 메뉴에서 고른다.
pub struct ExportDialog {
    pub format: ExportFormat,
    pub invisible_only: bool,
    pub txt_page_labels: bool,
    pub txt_crlf: bool,
    pub txt_form_feed: bool,
}

impl ExportDialog {
    pub fn new(format: ExportFormat) -> Self {
        Self { format, invisible_only: true, txt_page_labels: true, txt_crlf: false, txt_form_feed: false }
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
        Self { title, worker, temp_output, phase, stage: None, kind }
    }

    fn finish(&mut self, text: String) {
        self.worker = None;
        self.temp_output = None;
        self.phase = JobPhase::Finished(text);
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
pub fn poll(app: &mut PdfViewerApp) {
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
        app.ocr_removal_needs_save = true;
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

fn show_needs_save_dialog(ctx: &egui::Context, app: &mut PdfViewerApp) {
    if !app.ocr_removal_needs_save {
        return;
    }
    let mut action = None;
    egui::Window::new("OCR 전체 삭제")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .show(ctx, |ui| {
            ui.label("저장하지 않은 북마크 변경사항이 있습니다.");
            ui.label("OCR 삭제는 파일을 새로 쓰므로 북마크를 먼저 PDF에 저장해야 합니다.");
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
            app.ocr_removal_needs_save = false;
            if app.save_bookmarks_to_pdf() {
                request_removal(ctx, app);
            }
        }
        Some(false) => app.ocr_removal_needs_save = false,
        None => {}
    }
}

fn backup_path(pdf: &Path) -> PathBuf {
    let mut name = pdf.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    name.push(".backup");
    pdf.with_file_name(name)
}

fn show_removal_confirm(ctx: &egui::Context, app: &mut PdfViewerApp) {
    let Some(confirm) = app.ocr_removal_confirm.as_mut() else { return };
    let a = &confirm.analysis;
    let mut action = None;
    egui::Window::new("OCR 전체 삭제")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .show(ctx, |ui| {
            ui.set_max_width(460.0);
            let removed = a.counts.removed();
            if removed == 0 {
                ui.label("지울 보이지 않는 텍스트가 없습니다.");
            } else {
                ui.label(format!(
                    "{}쪽 중 {}쪽에서 보이지 않는 텍스트 {}건을 지웁니다.",
                    a.pages, a.pages_with_hidden_text, removed
                ));
                let mut kinds = Vec::new();
                if a.counts.invisible_mode > 0 {
                    kinds.push(format!("보이지 않게 그린 텍스트(OCR) {}", a.counts.invisible_mode));
                }
                if a.counts.zero_size > 0 {
                    kinds.push(format!("크기 0 텍스트 {}", a.counts.zero_size));
                }
                if a.counts.transparent > 0 {
                    kinds.push(format!("완전 투명 텍스트 {}", a.counts.transparent));
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
            if a.counts.clip_only_kept > 0 {
                ui.weak(format!(
                    "클리핑에 쓰이는 보이지 않는 텍스트 {}건은 화면에 영향을 줄 수 있어 남깁니다.",
                    a.counts.clip_only_kept
                ));
            }

            ui.add_space(6.0);
            if a.signed {
                ui.colored_label(ui.visuals().warn_fg_color, "디지털 서명된 PDF입니다. 파일을 새로 쓰면 서명이 무효가 됩니다.");
                ui.checkbox(&mut confirm.signature_ack, "서명이 무효가 되는 것을 이해했습니다");
            }
            if a.tagged {
                ui.weak("태그(접근성 구조)가 있는 PDF입니다. 지운 텍스트를 가리키던 태그가 비게 될 수 있습니다.");
            }
            if let Some(pdfa) = &a.pdfa {
                ui.weak(format!("PDF/A-{pdfa} 선언은 유지하지만 규격 준수는 보장하지 않습니다. 외부 검증(veraPDF 등)을 권합니다."));
            }
            if a.incremental_updates > 0 {
                ui.weak(format!(
                    "파일 끝에 쌓인 옛 수정본 {}개가 함께 정리됩니다(예전 내용은 더 이상 복원할 수 없음).",
                    a.incremental_updates
                ));
            }
            if a.linearized {
                ui.weak("빠른 웹 보기(선형화)가 해제됩니다.");
            }
            if removed > 0 {
                let backup = backup_path(&confirm.pdf);
                let name = crate::app::display_filename(&backup);
                if backup.exists() {
                    ui.weak(format!("이미 있는 백업({name})은 그대로 둡니다(더 이전 원본일 수 있음)."));
                } else {
                    ui.weak(format!("원본은 {name}(으)로 보존합니다."));
                }
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
        return Ok("지울 보이지 않는 텍스트가 없습니다. 파일을 바꾸지 않았습니다.".to_string());
    }
    let temp = temp.ok_or("결과 파일 경로가 없습니다.")?;
    let backup = backup_path(pdf);
    let backup_note = if backup.exists() {
        format!("기존 백업 유지: {}", crate::app::display_filename(&backup))
    } else {
        if let Err(err) = std::fs::copy(pdf, &backup) {
            let _ = std::fs::remove_file(temp);
            return Err(format!("원본 백업 실패({err}) — 원본은 바뀌지 않았습니다."));
        }
        format!("원본 백업: {}", crate::app::display_filename(&backup))
    };
    if let Err(err) = std::fs::rename(temp, pdf) {
        let _ = std::fs::remove_file(temp);
        return Err(format!("파일 교체 실패({err}) — 원본은 바뀌지 않았습니다."));
    }
    if app.current_file.as_deref() == Some(pdf) {
        app.reload_current_document();
    }
    app.status_message = Some("OCR 텍스트를 지웠습니다.".to_string());
    Ok(describe_removal(report, &backup_note))
}

fn describe_removal(r: &RemovalReport, backup_note: &str) -> String {
    let a = &r.analysis;
    let mut lines = vec![
        format!("바뀐 페이지: {}쪽 / 전체 {}쪽", r.pages_changed, a.pages),
        format!(
            "지운 텍스트: {}건(보이지 않게 그림 {}, 크기 0 {}, 투명 {})",
            a.counts.removed(),
            a.counts.invisible_mode,
            a.counts.zero_size,
            a.counts.transparent
        ),
        format!("파일 크기: {} → {}", human_size(r.size_before), human_size(r.size_after)),
    ];
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
    if a.counts.clip_only_kept > 0 {
        lines.push(format!("남긴 클리핑용 보이지 않는 텍스트: {}건", a.counts.clip_only_kept));
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
    show_job_window(ctx, app);
}

fn show_export_dialog(ctx: &egui::Context, app: &mut PdfViewerApp) {
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
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .show(ctx, |ui| {
            ui.label("내보낼 텍스트");
            ui.radio_value(&mut dialog.invisible_only, true, "보이지 않는 텍스트만(OCR 레이어)");
            ui.radio_value(&mut dialog.invisible_only, false, "페이지의 모든 텍스트");
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
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("내보내기…").clicked() {
                    start = true;
                }
                if ui.button("취소").clicked() {
                    close = true;
                }
            });
        });

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
        if let Some(output) = file_dialog.save_file() {
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
    let mut close = false;
    egui::Window::new(job.title.clone())
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
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
    if close {
        if let Some(mut job) = app.ocr_job.take() {
            job.cancel();
        }
    }
}
