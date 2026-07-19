//! 폴더 일괄 북마크 적용(2026-07-19 예약 작업): 폴더 하나 + 북마크 csv/xlsx 하나를 골라
//! 폴더 안의 모든 PDF를 재귀 순회하며 파일명이 매칭되는 북마크를 각 PDF에 저장한다.
//! 원본은 `x.pdf.backup`으로 rename해 보존하고, 결과는 원래 이름으로 만들어진다.
//!
//! 이 파이프라인에는 렌더링이 없다 — 행 파싱(import_export) + 트리 구성(bookmark) +
//! lopdf 증분 쓰기(pdf_outline_writer)만으로 완결되므로 pdfium 없이도 동작한다(§7의
//! pdfium 스레드 제약과 무관). 다만 엔진이 살아있으면 파일마다 저장 결과를 pdfium으로
//! 재오픈해 파싱 검증한다(단일 파일 저장 `save_bookmarks_to_pdf`와 같은 안전 관례).
//!
//! egui 즉시모드 특성상 한 번에 다 돌리면 UI가 멎으므로 프레임당 1파일씩 처리한다
//! (`poll` — §7 poll_search_job과 같은 프레임 분할 패턴). 진행 상황은 뷰어 영역을
//! 로그 뷰로 전환해 실시간으로 보여주고(스펙 1순위안), 끝나면 통계와 함께 대상 폴더
//! 루트에 타임스탬프가 붙은 UTF-8 `.log` 파일을 남긴다.

use bookmark::BookmarkRow;
use pdf_engine::PdfEngine;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use unicode_normalization::UnicodeNormalization;

/// 파일 하나의 처리 결과. Skip 계열은 파일을 전혀 건드리지 않았음을 뜻한다.
pub enum FileOutcome {
    Success { bookmark_count: usize },
    /// csv/xlsx에 이 파일명과 일치하는 행이 없음.
    SkippedNoRows,
    /// `x.pdf.backup`이 이미 존재 — 이전 실행의 흔적으로 보고 건드리지 않는다
    /// (덮어쓰면 진짜 원본 백업이 유실될 수 있음).
    SkippedBackupExists,
    /// 현재 앱에 열려 있는 파일 — rename하면 문서 핸들/파일 감시(macOS inode 추적)와
    /// 충돌하므로 제외한다. 닫고 다시 실행하면 처리된다.
    SkippedOpenFile,
    Failed(String),
}

impl FileOutcome {
    fn label(&self) -> &'static str {
        match self {
            FileOutcome::Success { .. } => "성공",
            FileOutcome::Failed(_) => "실패",
            _ => "건너뜀",
        }
    }

    fn detail(&self) -> String {
        match self {
            FileOutcome::Success { bookmark_count } => format!("북마크 {bookmark_count}개 저장"),
            FileOutcome::SkippedNoRows => "일치하는 '파일명' 행 없음".to_string(),
            FileOutcome::SkippedBackupExists => ".backup 파일이 이미 있음(이전 실행 흔적)".to_string(),
            FileOutcome::SkippedOpenFile => "현재 앱에 열려 있는 파일".to_string(),
            FileOutcome::Failed(reason) => reason.clone(),
        }
    }
}

pub struct LogEntry {
    /// 대상 폴더 기준 상대 경로(NFC, 표시/로그 파일용).
    pub rel_path: String,
    pub outcome: FileOutcome,
}

pub enum JobPhase {
    /// 대상 파일 수를 보여주고 시작/취소를 기다리는 중.
    AwaitingConfirmation,
    Running,
    Finished,
}

pub struct Stats {
    pub total: usize,
    pub success: usize,
    pub failed: usize,
    pub skipped: usize,
}

pub struct BatchImportJob {
    pub folder: PathBuf,
    /// 폴더에서 자동 인식된 북마크 파일들 — **읽은 순서대로**(xlsx 전부 → csv 전부,
    /// 각각 경로순. 2026-07-19 사용자 지정 우선순위). 별도 선택 다이얼로그는 없다.
    pub sheet_files: Vec<PathBuf>,
    /// 재귀 수집된 처리 대상 PDF 목록(경로순 정렬).
    pub files: Vec<PathBuf>,
    /// NFC 정규화한 파일명 → 그 파일의 행들('순서' 정렬 완료). 한 csv/xlsx에 여러
    /// 문서의 행이 섞여 있는 스키마(2026-07-19)를 그대로 받아 파일명별로 골라둔 것.
    /// 여러 북마크 파일이 같은 PDF의 행을 담고 있으면 **먼저 읽은 파일이 이긴다**
    /// (뒤의 것은 건너뛰고 setup_notes에 기록).
    rows_by_filename: HashMap<String, Vec<BookmarkRow>>,
    /// 준비 단계에서 생긴 안내(읽기 실패한 북마크 파일, 중복으로 건너뛴 행 등) —
    /// 확인 화면과 .log 파일에 그대로 실린다.
    pub setup_notes: Vec<String>,
    /// 확인 화면에 보여줄, 매칭 행이 있는 파일 수(prepare 시 1회 계산).
    pub matched_count: usize,
    pub next_index: usize,
    pub log: Vec<LogEntry>,
    pub phase: JobPhase,
    /// 완료 후 로그 파일 경로 안내(또는 저장 실패 사유).
    pub log_file_note: Option<String>,
    /// 배치를 시작한 시점에 앱에 열려 있던 (PDF 경로, 보던 페이지) — 완료 화면의
    /// "되돌아가기" 버튼이 이 자리로 복귀시킨다(2026-07-19 요청). 열린 문서가 없었으면
    /// None이고 버튼은 "닫기"로 표시된다. `start_batch_import`(app.rs)가 채운다.
    pub return_to: Option<(PathBuf, u32)>,
    started_at: chrono::DateTime<chrono::Local>,
}

impl BatchImportJob {
    pub fn stats(&self) -> Stats {
        let mut stats = Stats { total: self.log.len(), success: 0, failed: 0, skipped: 0 };
        for entry in &self.log {
            match entry.outcome {
                FileOutcome::Success { .. } => stats.success += 1,
                FileOutcome::Failed(_) => stats.failed += 1,
                _ => stats.skipped += 1,
            }
        }
        stats
    }

    pub fn is_running(&self) -> bool {
        matches!(self.phase, JobPhase::Running)
    }
}

/// 폴더 하나만 받아 잡을 준비한다(아직 아무 파일도 건드리지 않음 — 확인 대기 상태).
/// 북마크 파일은 따로 묻지 않고 폴더 안(하위 폴더 포함)의 모든 xlsx/csv를 자동 인식해
/// **xlsx 전부 → csv 전부** 순으로 읽는다(2026-07-19 사용자 지정). 같은 PDF의 행이
/// 여러 북마크 파일에 있으면 먼저 읽은 파일이 이기고, 뒤의 것은 건너뛰어 기록만 남긴다.
/// PDF나 북마크 파일이 하나도 없으면 Err(호출측이 status_message로 안내).
pub fn prepare_job(folder: PathBuf) -> Result<BatchImportJob, String> {
    let mut files = Vec::new();
    let mut sheet_files = Vec::new();
    collect_files(&folder, &mut files, &mut sheet_files);
    files.sort();
    if files.is_empty() {
        return Err("선택한 폴더(하위 폴더 포함)에서 PDF를 찾지 못했습니다.".to_string());
    }
    if sheet_files.is_empty() {
        return Err("폴더(하위 폴더 포함)에서 북마크 파일(xlsx/csv)을 찾지 못했습니다.".to_string());
    }
    // 우선순위: xlsx 전부 → csv 전부, 각각 경로순(사전순) — sort key의 bool이
    // xlsx=false < csv=true 로 정렬돼 xlsx가 앞에 온다.
    sheet_files.sort_by_key(|p| (!is_ext(p, "xlsx"), p.clone()));

    let mut rows_by_filename: HashMap<String, Vec<BookmarkRow>> = HashMap::new();
    // 각 PDF 파일명을 어느 북마크 파일이 선점했는지(중복 안내문에 쓸 정보).
    let mut source_of: HashMap<String, PathBuf> = HashMap::new();
    let mut setup_notes = Vec::new();

    for sheet in &sheet_files {
        let parsed = if is_ext(sheet, "xlsx") {
            import_export::import_xlsx(sheet)
        } else {
            import_export::import_csv(sheet, None)
        };
        let rows = match parsed {
            Ok(rows) => rows,
            Err(err) => {
                // 스키마가 다른 무관한 엑셀/CSV가 섞여 있을 수 있다 — 전체를 중단하지
                // 않고 그 파일만 건너뛰며 알린다.
                setup_notes.push(format!(
                    "[경고] 북마크 파일 '{}' 읽기 실패 — 무시함: {err}",
                    rel_display(sheet, &folder)
                ));
                continue;
            }
        };

        // '파일명'을 NFC로 정규화해 이 북마크 파일 안에서 먼저 그룹핑 — macOS 디스크의
        // NFD 파일명(display_filename도 NFC로 맞춰 비교)과 Excel에서 입력한 조합형
        // 한글이 어긋나지 않게 한다. BTreeMap이라 중복 안내문 순서도 결정적이다.
        let mut per_file: std::collections::BTreeMap<String, Vec<BookmarkRow>> =
            std::collections::BTreeMap::new();
        for row in rows {
            let key: String = row.filename.nfc().collect();
            per_file.entry(key).or_default().push(row);
        }

        for (name, mut group) in per_file {
            if let Some(first_source) = source_of.get(&name) {
                // 이미 앞선(우선순위 높은) 북마크 파일이 이 PDF의 행을 제공함 — 중복은
                // 적용하지 않고 건너뛴다(2026-07-19 사용자 지정).
                setup_notes.push(format!(
                    "[중복] '{}'의 '{name}' 행 {}개 건너뜀 — 먼저 읽은 '{}'의 행을 적용",
                    rel_display(sheet, &folder),
                    group.len(),
                    rel_display(first_source, &folder)
                ));
                continue;
            }
            // 단일 파일 import(prepare_imported_rows)와 같은 규칙: '순서' 기준 stable 정렬.
            group.sort_by_key(|r| r.order);
            rows_by_filename.insert(name.clone(), group);
            source_of.insert(name, sheet.clone());
        }
    }

    let matched_count = files
        .iter()
        .filter(|f| rows_by_filename.contains_key(&crate::app::display_filename(f)))
        .count();

    Ok(BatchImportJob {
        folder,
        sheet_files,
        files,
        rows_by_filename,
        setup_notes,
        matched_count,
        next_index: 0,
        log: Vec::new(),
        phase: JobPhase::AwaitingConfirmation,
        log_file_note: None,
        return_to: None,
        started_at: chrono::Local::now(),
    })
}

fn is_ext(path: &Path, ext: &str) -> bool {
    path.extension().is_some_and(|e| e.eq_ignore_ascii_case(ext))
}

/// 처리 대상 `.pdf`와 북마크 파일 `.xlsx`/`.csv`(전부 대소문자 무시)를 한 번의 재귀
/// 순회로 수집. `x.pdf.backup`은 확장자가 backup이라 자연히 제외된다. 숨김 파일과
/// Excel이 열려 있는 동안 만드는 잠금 파일(`~$*.xlsx`)은 건너뛴다. 심볼릭 링크
/// 디렉토리는 따라가지 않는다(순환 방지 — entry의 file_type은 링크를 해석하지 않음).
fn collect_files(dir: &Path, pdfs: &mut Vec<PathBuf>, sheets: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if file_type.is_dir() {
            collect_files(&path, pdfs, sheets);
        } else if file_type.is_file() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with('.') || name.starts_with("~$") {
                continue;
            }
            if is_ext(&path, "pdf") {
                pdfs.push(path);
            } else if is_ext(&path, "xlsx") || is_ext(&path, "csv") {
                sheets.push(path);
            }
        }
    }
}

/// 프레임당 1파일 처리(즉시모드 UI가 멎지 않게 — 모듈 문서 참고). Running이 아니면
/// 아무 일도 안 한다. 마지막 파일까지 끝나면 통계/로그 파일을 쓰고 Finished로 전환.
pub fn poll(job: &mut BatchImportJob, engine: Option<&PdfEngine>, open_file: Option<&Path>, ctx: &egui::Context) {
    if !job.is_running() {
        return;
    }

    if let Some(file) = job.files.get(job.next_index).cloned() {
        job.next_index += 1;
        let outcome = process_one(&file, &job.rows_by_filename, engine, open_file);
        job.log.push(LogEntry {
            rel_path: rel_display(&file, &job.folder),
            outcome,
        });
    }

    if job.next_index >= job.files.len() {
        finish(job);
    }
    // 다음 파일 처리(또는 완료 화면 갱신)를 위해 즉시 다음 프레임을 요청.
    ctx.request_repaint();
}

/// 파일 하나 처리: (1) 원본을 `x.pdf.backup`으로 rename, (2) 그 백업을 소스로 lopdf
/// 증분 쓰기를 원래 이름에 수행(IncrementalDocument는 원본 바이트 전체 + 새 리비전을
/// 기록하므로 결과 파일이 그 자체로 완전하다 — 별도 복사 단계 불필요), (3) 가능하면
/// pdfium으로 재오픈 검증. 실패 시 백업을 원래 이름으로 되돌려 원본을 보존한다.
fn process_one(
    file: &Path,
    rows_by_filename: &HashMap<String, Vec<BookmarkRow>>,
    engine: Option<&PdfEngine>,
    open_file: Option<&Path>,
) -> FileOutcome {
    let name = crate::app::display_filename(file);
    let Some(rows) = rows_by_filename.get(&name) else {
        return FileOutcome::SkippedNoRows;
    };

    if open_file.is_some_and(|open| is_same_file(open, file)) {
        return FileOutcome::SkippedOpenFile;
    }

    let backup = backup_path(file);
    if backup.exists() {
        return FileOutcome::SkippedBackupExists;
    }

    let tree = bookmark::build_tree(rows);

    if let Err(err) = std::fs::rename(file, &backup) {
        return FileOutcome::Failed(format!("백업 rename 실패: {err}"));
    }

    if let Err(err) = pdf_outline_writer::write_bookmarks_incremental(&backup, file, &tree) {
        restore_backup(&backup, file);
        return FileOutcome::Failed(format!("북마크 쓰기 실패(원본 복원됨): {err}"));
    }

    // 검증: pdfium이 있으면 결과 파일을 다시 열어 파싱되는지 확인(단일 파일 저장과 같은
    // 관례). 실패하면 결과를 버리고 백업을 원래 이름으로 되돌린다.
    if let Some(engine) = engine {
        let parses_ok = engine
            .open_document(file)
            .map(|doc| !doc.pages().is_empty())
            .unwrap_or(false);
        if !parses_ok {
            restore_backup(&backup, file);
            return FileOutcome::Failed("저장 결과 검증 실패(원본 복원됨)".to_string());
        }
    }

    FileOutcome::Success { bookmark_count: rows.len() }
}

/// `x.pdf` → `x.pdf.backup` (확장자 교체가 아니라 덧붙임 — 원래 이름을 그대로 품는다).
fn backup_path(file: &Path) -> PathBuf {
    let mut os = file.as_os_str().to_owned();
    os.push(".backup");
    PathBuf::from(os)
}

/// 실패한 결과 파일(있다면)을 치우고 백업을 원래 이름으로 되돌린다.
fn restore_backup(backup: &Path, original: &Path) {
    let _ = std::fs::remove_file(original);
    let _ = std::fs::rename(backup, original);
}

/// 경로가 실제로 같은 파일을 가리키는지 — 심볼릭 링크/상대 경로 차이를 흡수하기 위해
/// canonicalize로 비교하고, 실패하면(파일이 없는 등) 문자열 비교로 폴백.
fn is_same_file(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(ca), Ok(cb)) => ca == cb,
        _ => a == b,
    }
}

fn rel_display(path: &Path, folder: &Path) -> String {
    let rel = path.strip_prefix(folder).unwrap_or(path);
    rel.to_string_lossy().nfc().collect()
}

fn stats_line(stats: &Stats) -> String {
    format!(
        "전체 {}개 중 {}개 성공, {}개 실패, {}개 건너뜀",
        stats.total, stats.success, stats.failed, stats.skipped
    )
}

/// 완료 처리: 통계를 집계하고 대상 폴더 루트에 UTF-8 `.log` 파일을 남긴다. 파일명에
/// 시작 시각 타임스탬프를 넣어 재실행이 이전 로그를 덮어쓰지 않게 한다.
fn finish(job: &mut BatchImportJob) {
    job.phase = JobPhase::Finished;

    let stats = job.stats();
    let mut text = String::new();
    text.push_str("PDF Outliner — 폴더 일괄 북마크 적용 로그\n");
    text.push_str(&format!("실행 시각: {}\n", job.started_at.format("%Y-%m-%d %H:%M:%S")));
    text.push_str(&format!("대상 폴더: {}\n", job.folder.to_string_lossy().nfc().collect::<String>()));
    for sheet in &job.sheet_files {
        text.push_str(&format!("북마크 파일(읽은 순서): {}\n", rel_display(sheet, &job.folder)));
    }
    for note in &job.setup_notes {
        text.push_str(note);
        text.push('\n');
    }
    text.push_str("----\n");
    for entry in &job.log {
        text.push_str(&format!("[{}] {} — {}\n", entry.outcome.label(), entry.rel_path, entry.outcome.detail()));
    }
    text.push_str("----\n");
    text.push_str(&stats_line(&stats));
    text.push('\n');

    let log_name = format!("bookmark-import-{}.log", job.started_at.format("%Y%m%d-%H%M%S"));
    let log_path = job.folder.join(log_name);
    job.log_file_note = Some(match std::fs::write(&log_path, text) {
        Ok(()) => format!("로그 파일: {}", log_path.to_string_lossy().nfc().collect::<String>()),
        Err(err) => format!("로그 파일 저장 실패: {err}"),
    });
}

/// 뷰어 영역(CentralPanel 내부)에 그리는 일괄 처리 화면 — 잡이 존재하는 동안
/// viewer_panel::show가 문서 대신 이걸 그린다(스펙 1순위안: 뷰어 영역을 로그 뷰로 전환).
pub fn show_panel(ui: &mut egui::Ui, app: &mut crate::app::PdfViewerApp) {
    let Some(job) = app.batch_import.as_mut() else {
        return;
    };

    ui.heading("폴더 일괄 북마크 적용");
    ui.add_space(6.0);
    ui.label(format!("대상 폴더: {}", job.folder.to_string_lossy().nfc().collect::<String>()));
    // 자동 인식된 북마크 파일들 — 어떤 파일이 어떤 순서로 적용되는지 사용자가 확인
    // 화면에서 바로 볼 수 있어야 한다(선택 다이얼로그가 없으므로 여기가 유일한 안내).
    ui.label(format!("자동 인식된 북마크 파일 {}개 (xlsx → csv 순으로 적용):", job.sheet_files.len()));
    for sheet in &job.sheet_files {
        ui.label(format!("    • {}", rel_display(sheet, &job.folder)));
    }
    for note in &job.setup_notes {
        ui.colored_label(ui.visuals().warn_fg_color, note);
    }
    ui.add_space(6.0);

    let mut close = false;
    match job.phase {
        JobPhase::AwaitingConfirmation => {
            ui.label(format!(
                "발견된 PDF {}개 — 그중 파일명이 일치하는 행이 있는 파일 {}개",
                job.files.len(),
                job.matched_count
            ));
            ui.label("각 원본은 같은 자리에 '파일명.pdf.backup'으로 보존됩니다.");
            if job.matched_count == 0 {
                ui.colored_label(
                    ui.visuals().warn_fg_color,
                    "일치하는 파일명이 하나도 없습니다 — 시작하면 전부 건너뜁니다.",
                );
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("시작").clicked() {
                    job.started_at = chrono::Local::now();
                    job.phase = JobPhase::Running;
                }
                if ui.button("취소").clicked() {
                    close = true;
                }
            });
        }
        JobPhase::Running => {
            let done = job.next_index.min(job.files.len());
            ui.label(format!("처리 중… ({done}/{})", job.files.len()));
            ui.add(egui::ProgressBar::new(done as f32 / job.files.len().max(1) as f32).show_percentage());
        }
        JobPhase::Finished => {
            let stats = job.stats();
            ui.strong(format!("완료 — {}", stats_line(&stats)));
            if let Some(note) = &job.log_file_note {
                ui.label(note.clone());
            }
            ui.add_space(8.0);
            // 배치 전에 보던 문서가 있으면 그 자리로 복귀하는 버튼(2026-07-19 요청),
            // 없으면 그냥 닫기 — 실제 복귀 처리는 아래 close 블록에서.
            let label = if job.return_to.is_some() { "되돌아가기" } else { "닫기" };
            if ui.button(label).clicked() {
                close = true;
            }
        }
    }

    // 실시간 로그: 처리된 파일부터 차례로 쌓이고, 진행 중엔 바닥에 붙어 따라 내려간다.
    if !job.log.is_empty() {
        ui.add_space(8.0);
        ui.separator();
        egui::ScrollArea::vertical()
            .stick_to_bottom(job.is_running())
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for entry in &job.log {
                    let color = match entry.outcome {
                        FileOutcome::Success { .. } => egui::Color32::from_rgb(0x2e, 0x7d, 0x32),
                        FileOutcome::Failed(_) => ui.visuals().error_fg_color,
                        _ => ui.visuals().weak_text_color(),
                    };
                    ui.colored_label(
                        color,
                        format!("[{}] {} — {}", entry.outcome.label(), entry.rel_path, entry.outcome.detail()),
                    );
                }
            });
    }

    if close {
        let return_to = app.batch_import.take().and_then(|job| job.return_to);
        if let Some((path, page)) = return_to {
            // 열린 파일은 배치 대상에서 제외되므로(SkippedOpenFile) 문서는 보통 그대로
            // 열려 있다 — 그 경우 페이지만 복원하면 된다. 배치 중에 사용자가 다른
            // 문서를 열었거나 닫았으면 원래 파일을 다시 연다(북마크 변경사항이 있으면
            // request_open_file의 저장 확인 플로우를 그대로 탄다).
            if app.current_file.as_deref() != Some(path.as_path()) {
                app.request_open_file(path);
            }
            app.go_to_page(page);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backup_path_appends_suffix() {
        assert_eq!(
            backup_path(Path::new("/a/b/문서.pdf")),
            PathBuf::from("/a/b/문서.pdf.backup")
        );
    }

    /// NFD(맥 디스크 형태) 파일명과 NFC(엑셀 입력 형태) '파일명' 컬럼이 매칭되는지 —
    /// prepare_job의 그룹핑 키와 display_filename이 둘 다 NFC로 수렴해야 한다.
    #[test]
    fn nfd_filename_matches_nfc_rows() {
        let nfd: String = "한글.pdf".nfc().collect::<String>().nfd().collect();
        let key: String = nfd.nfc().collect();
        assert_eq!(key, "한글.pdf");
    }

    fn sample(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../../pdf-samples/{name}"))
    }

    /// 북마크 파일 자동 인식: 폴더에 xlsx/csv가 없으면 Err. 같은 PDF의 행이 xlsx와
    /// csv 양쪽에 있으면 xlsx가 이기고(우선순위 xlsx → csv), 뒤의 중복은 건너뛰며
    /// setup_notes에 기록된다.
    #[test]
    fn sheets_are_autodetected_xlsx_wins_over_csv() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::copy(sample("KKZ000160_01.pdf"), root.join("a.pdf")).unwrap();
        assert!(prepare_job(root.to_path_buf()).is_err(), "북마크 파일 없음 = Err");

        let xlsx_rows = vec![BookmarkRow {
            order: 1,
            filename: "a.pdf".to_string(),
            depth: 0,
            title: "엑셀에서 온 장".to_string(),
            page: 1,
        }];
        import_export::export_xlsx(&xlsx_rows, &root.join("우선.xlsx")).unwrap();
        std::fs::write(
            root.join("나중.csv"),
            "\u{FEFF}순서,파일명,계층,북마크명,페이지번호\n1,a.pdf,0,CSV에서 온 장,1\n2,a.pdf,1,CSV 절,1\n",
        )
        .unwrap();

        let job = prepare_job(root.to_path_buf()).unwrap();
        assert_eq!(job.sheet_files.len(), 2);
        assert!(is_ext(&job.sheet_files[0], "xlsx"), "xlsx가 먼저 와야 함");
        assert_eq!(job.matched_count, 1);
        // xlsx의 행(1개)이 채택되고, csv의 중복 행 2개는 건너뛴 기록이 남는다.
        assert_eq!(job.rows_by_filename["a.pdf"].len(), 1);
        assert_eq!(job.rows_by_filename["a.pdf"][0].title, "엑셀에서 온 장");
        assert!(
            job.setup_notes.iter().any(|n| n.contains("[중복]") && n.contains("나중.csv")),
            "{:?}",
            job.setup_notes
        );
    }

    /// 전체 파이프라인 실 구동: 하위 폴더 포함 재귀 수집 → 매칭 파일 처리(원본은
    /// .backup으로 rename, 결과는 원래 이름) → 손상 PDF는 실패 후 원본 복원 → 매칭 행
    /// 없는 파일은 건너뜀 → 폴더 루트에 타임스탬프 .log 파일 + 통계.
    #[test]
    fn end_to_end_batch_over_folder() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir(root.join("sub")).unwrap();
        std::fs::copy(sample("KKZ000160_01.pdf"), root.join("a.pdf")).unwrap();
        std::fs::copy(sample("KKZ000160_01.pdf"), root.join("sub/b.pdf")).unwrap();
        std::fs::copy(sample("corrupted.pdf"), root.join("bad.pdf")).unwrap();
        std::fs::copy(sample("KKZ000160_01.pdf"), root.join("norows.pdf")).unwrap();

        let csv = root.join("bookmarks.csv");
        std::fs::write(
            &csv,
            "\u{FEFF}순서,파일명,계층,북마크명,페이지번호\n\
             2,a.pdf,1,제1절,2\n\
             1,a.pdf,0,제1장,1\n\
             1,b.pdf,0,서론,1\n\
             1,bad.pdf,0,깨진파일,1\n",
        )
        .unwrap();

        let mut job = prepare_job(root.to_path_buf()).unwrap();
        assert_eq!(job.sheet_files, vec![csv]);
        assert_eq!(job.files.len(), 4);
        assert_eq!(job.matched_count, 3);

        job.phase = JobPhase::Running;
        let ctx = egui::Context::default();
        for _ in 0..10 {
            poll(&mut job, None, None, &ctx);
        }
        assert!(matches!(job.phase, JobPhase::Finished));

        let stats = job.stats();
        assert_eq!(stats.total, 4);
        assert_eq!(stats.success, 2);
        assert_eq!(stats.failed, 1);
        assert_eq!(stats.skipped, 1);

        // 성공 파일: 원본이 .backup으로 남고, 결과는 원래 이름 + 증분이 붙어 더 커야 한다.
        let orig_len = std::fs::metadata(sample("KKZ000160_01.pdf")).unwrap().len();
        for path in [root.join("a.pdf"), root.join("sub/b.pdf")] {
            assert!(backup_path(&path).exists(), "{path:?} 백업 없음");
            assert!(std::fs::metadata(&path).unwrap().len() > orig_len);
        }
        // 실패 파일(lopdf 파싱 불가): 원본이 제자리에 복원되고 백업은 남지 않는다.
        assert!(root.join("bad.pdf").exists());
        assert!(!backup_path(&root.join("bad.pdf")).exists());
        // 매칭 행 없는 파일: 아무것도 안 건드린다.
        assert!(!backup_path(&root.join("norows.pdf")).exists());

        // 로그 파일이 폴더 루트에 생기고 통계 줄을 담는다.
        let log_file = std::fs::read_dir(root)
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .find(|p| p.extension().is_some_and(|e| e == "log"))
            .expect("로그 파일 없음");
        let log_text = std::fs::read_to_string(log_file).unwrap();
        assert!(log_text.contains("전체 4개 중 2개 성공, 1개 실패, 1개 건너뜀"), "{log_text}");
    }

    /// 현재 앱에 열려 있는 파일은 rename 충돌을 피하려고 건너뛴다.
    #[test]
    fn open_file_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::copy(sample("KKZ000160_01.pdf"), root.join("a.pdf")).unwrap();
        let csv = root.join("bookmarks.csv");
        std::fs::write(&csv, "\u{FEFF}순서,파일명,계층,북마크명,페이지번호\n1,a.pdf,0,장,1\n").unwrap();

        let mut job = prepare_job(root.to_path_buf()).unwrap();
        job.phase = JobPhase::Running;
        let ctx = egui::Context::default();
        let open = root.join("a.pdf");
        for _ in 0..5 {
            poll(&mut job, None, Some(&open), &ctx);
        }
        let stats = job.stats();
        assert_eq!((stats.success, stats.skipped), (0, 1));
        assert!(!backup_path(&open).exists());
    }
}
