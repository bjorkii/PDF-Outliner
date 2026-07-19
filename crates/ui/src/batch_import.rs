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
    pub sheet_path: PathBuf,
    /// 재귀 수집된 처리 대상 PDF 목록(경로순 정렬).
    pub files: Vec<PathBuf>,
    /// NFC 정규화한 파일명 → 그 파일의 행들('순서' 정렬 완료). 한 csv/xlsx에 여러
    /// 문서의 행이 섞여 있는 스키마(2026-07-19)를 그대로 받아 파일명별로 골라둔 것.
    rows_by_filename: HashMap<String, Vec<BookmarkRow>>,
    /// 확인 화면에 보여줄, 매칭 행이 있는 파일 수(prepare 시 1회 계산).
    pub matched_count: usize,
    pub next_index: usize,
    pub log: Vec<LogEntry>,
    pub phase: JobPhase,
    /// 완료 후 로그 파일 경로 안내(또는 저장 실패 사유).
    pub log_file_note: Option<String>,
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

/// 폴더/시트를 받아 잡을 준비한다(아직 아무 파일도 건드리지 않음 — 확인 대기 상태).
/// csv/xlsx 파싱 실패나 빈 폴더 등은 Err 문자열로 돌려 호출측이 status_message로 안내한다.
pub fn prepare_job(folder: PathBuf, sheet_path: PathBuf) -> Result<BatchImportJob, String> {
    let is_xlsx = sheet_path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("xlsx"));
    let rows = if is_xlsx {
        import_export::import_xlsx(&sheet_path).map_err(|e| format!("Excel 읽기 실패: {e}"))?
    } else {
        import_export::import_csv(&sheet_path, None).map_err(|e| format!("CSV 읽기 실패: {e}"))?
    };
    if rows.is_empty() {
        return Err("북마크 파일에 행이 없습니다.".to_string());
    }

    // '파일명'을 NFC로 정규화해 그룹핑 — macOS 디스크의 NFD 파일명(display_filename도
    // NFC로 맞춰 비교)과 Excel에서 입력한 조합형 한글이 어긋나지 않게 한다.
    let mut rows_by_filename: HashMap<String, Vec<BookmarkRow>> = HashMap::new();
    for row in rows {
        let key: String = row.filename.nfc().collect();
        rows_by_filename.entry(key).or_default().push(row);
    }
    for group in rows_by_filename.values_mut() {
        // 단일 파일 import(prepare_imported_rows)와 같은 규칙: '순서' 기준 stable 정렬.
        group.sort_by_key(|r| r.order);
    }

    let mut files = Vec::new();
    collect_pdfs(&folder, &mut files);
    files.sort();
    if files.is_empty() {
        return Err("선택한 폴더(하위 폴더 포함)에서 PDF를 찾지 못했습니다.".to_string());
    }

    let matched_count = files
        .iter()
        .filter(|f| rows_by_filename.contains_key(&crate::app::display_filename(f)))
        .count();

    Ok(BatchImportJob {
        folder,
        sheet_path,
        files,
        rows_by_filename,
        matched_count,
        next_index: 0,
        log: Vec::new(),
        phase: JobPhase::AwaitingConfirmation,
        log_file_note: None,
        started_at: chrono::Local::now(),
    })
}

/// `.pdf`(대소문자 무시)만 재귀 수집. `x.pdf.backup`은 확장자가 backup이라 자연히
/// 제외된다. 심볼릭 링크 디렉토리는 따라가지 않는다(순환 방지 — entry의 file_type은
/// 링크를 해석하지 않으므로 is_dir가 false).
fn collect_pdfs(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if file_type.is_dir() {
            collect_pdfs(&path, out);
        } else if file_type.is_file()
            && path.extension().is_some_and(|e| e.eq_ignore_ascii_case("pdf"))
        {
            out.push(path);
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
    text.push_str(&format!("북마크 파일: {}\n", job.sheet_path.to_string_lossy().nfc().collect::<String>()));
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
    ui.label(format!(
        "북마크 파일: {}",
        crate::app::display_filename(&job.sheet_path)
    ));
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
            if ui.button("닫기").clicked() {
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
        app.batch_import = None;
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

        let mut job = prepare_job(root.to_path_buf(), csv).unwrap();
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

        let mut job = prepare_job(root.to_path_buf(), csv).unwrap();
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
