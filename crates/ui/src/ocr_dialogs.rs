//! OCR 메뉴의 대화상자 — 내보내기 옵션, 진행률·취소, 결과 리포트(설계 문서 7장).
//!
//! 실제 작업은 `ocr_worker` 프로세스가 한다. 여기서는 옵션을 모아 작업을 띄우고, 매 프레임
//! 이벤트를 받아 진행 상황을 보여 준다.

use crate::app::PdfViewerApp;
use crate::ocr_worker::{Event, ExportFormat, ExportJob, ExportReport, Job, WorkerHandle, WorkerPoll};
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

/// 실행 중이거나 끝난 OCR 작업(창을 닫을 때까지 유지).
pub struct OcrJob {
    title: String,
    worker: Option<WorkerHandle>,
    /// 취소·실패 시 지울 임시 파일.
    temp_output: Option<PathBuf>,
    phase: JobPhase,
    describe: Box<dyn Fn(&ExportReport) -> String>,
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
    let Some(job) = app.ocr_job.as_mut() else { return };
    while let Some(worker) = job.worker.as_ref() {
        match worker.poll() {
            WorkerPoll::Empty => break,
            WorkerPoll::Event(Event::Progress { done, total }) => job.phase = JobPhase::Running { done, total },
            WorkerPoll::Event(Event::ExportDone(report)) => {
                job.temp_output = None; // 이미 결과 파일로 옮겨짐
                job.worker = None;
                job.phase = JobPhase::Finished((job.describe)(&report));
            }
            WorkerPoll::Event(Event::Failed(message)) => job.fail(message),
            // 끝 이벤트 없이 출력이 끝남 — 작업 프로세스가 죽었다.
            WorkerPoll::Closed => job.fail("작업 프로세스가 예기치 않게 끝났습니다(panic.log 확인).".to_string()),
        }
    }
}

pub fn show(ctx: &egui::Context, app: &mut PdfViewerApp) {
    show_export_dialog(ctx, app);
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

    let (worker, phase) = match WorkerHandle::spawn(&Job::Export(job), ctx) {
        Ok(worker) => (Some(worker), JobPhase::Running { done: 0, total: 0 }),
        Err(err) => (None, JobPhase::Failed(format!("작업 프로세스를 시작하지 못했습니다: {err}"))),
    };
    app.ocr_job = Some(OcrJob {
        title: format!("OCR 텍스트 내보내기({format_label})"),
        worker,
        temp_output: Some(temp_output),
        phase,
        describe: Box::new(describe),
    });
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
