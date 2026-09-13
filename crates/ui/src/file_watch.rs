//! 열린 파일의 바깥 변경 감지와 자동 다시 열기(라이브 업데이트) — SumatraPDF 방식(2026-09-14).
//!
//! SumatraPDF(`src/base/FileWatcher.cpp`, `SumatraPDF.cpp` `ReloadDocument`/
//! `AutoReloadFileStillChanging`, `Canvas.cpp` `kAutoReloadTimerID`)에서 가져온 정책:
//! - **파일이 아니라 폴더를 감시**하고, 파일명이 같은 추가·수정·이름 변경(새 이름) 이벤트를 받는다
//!   — 편집기가 임시 파일에 쓴 뒤 원본 자리로 옮기는 "원자적 저장"도 잡기 위해서다. 파일을 읽기만
//!   한 이벤트(Access)는 무시한다(우리 pdfium이 파일을 여는 것도 여기 걸린다).
//! - 이벤트가 오면 곧바로 열지 않는다. 마지막 이벤트 뒤 `RELOAD_DELAY`만큼 조용해지면 크기·수정
//!   시각을 보고, **직전 확인과 다르면 아직 쓰는 중**이라 보고 한 번 더 기다린다(첫 확인은 비교
//!   대상이 없어 항상 기다림 — LaTeX가 절반 쓴 파일을 열어 "페이지 없음"이 뜨던 문제의 해법).
//!   계속 바뀌는 파일도 `MAX_WAIT`가 지나면 그냥 연다.
//! - **자기 저장은 무시**한다(SumatraPDF `ignoreNextAutoReload`) — 여기서는 저장 직후의 파일
//!   상태를 "알고 있는 상태"로 기록해, 확인 시점의 상태가 그것과 같으면 다시 열지 않는다. 같은
//!   폴더의 다른 파일 때문에 온 이벤트도 같은 방식으로 걸러진다.
//! - 자동 다시 열기가 실패하면(쓰는 중이라 깨진 파일) 복구·암호 입력을 시도하지 않고 기존 문서를
//!   유지한 채 다음 변경을 기다린다(app.rs `reload_current_document`).
//!
//! 상수 값도 SumatraPDF와 같다(`SumatraPDF.h` `kAutoReloadDelayInMs = 500`,
//! `kAutoReloadMaxWaitMs = 5000`).

use std::ffi::OsStr;
use std::path::Path;
use std::time::{Duration, Instant, SystemTime};
use unicode_normalization::UnicodeNormalization;

/// 마지막 변경 이벤트 뒤 이만큼 조용해지면 파일 상태를 확인한다(SumatraPDF `kAutoReloadDelayInMs`).
pub const RELOAD_DELAY: Duration = Duration::from_millis(500);
/// 계속 바뀌는 파일(로그처럼 이어 쓰는)도 첫 이벤트 뒤 이 시간이 지나면 그냥 다시 연다
/// (SumatraPDF `kAutoReloadMaxWaitMs`).
pub const MAX_WAIT: Duration = Duration::from_secs(5);

/// 변경 판정에 쓰는 파일 상태(크기·수정 시각).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileState {
    len: u64,
    modified: Option<SystemTime>,
}

impl FileState {
    pub fn read(path: &Path) -> Option<Self> {
        let meta = std::fs::metadata(path).ok()?;
        Some(Self {
            len: meta.len(),
            modified: meta.modified().ok(),
        })
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum ReloadDecision {
    /// 대기 중인 변경 없음.
    Idle,
    /// 아직 확인할 때가 아니거나 쓰는 중 — 잠시 뒤 다시 확인.
    Wait,
    /// 변경이 우리와 무관(내용 그대로·파일 없음) — 대기 해제.
    Skip,
    /// 다시 연다.
    Reload,
}

#[derive(Debug, Default)]
pub struct ReloadScheduler {
    first_event: Option<Instant>,
    last_event: Option<Instant>,
    /// 직전 확인 때의 상태 — 다음 확인과 같아야 "쓰기가 끝났다"고 본다.
    last_snapshot: Option<FileState>,
}

impl ReloadScheduler {
    /// 감시 중인 파일에 변경 이벤트가 왔다.
    pub fn note_change(&mut self, now: Instant) {
        self.first_event.get_or_insert(now);
        self.last_event = Some(now);
    }

    /// 매 프레임 호출. `current`는 지금 파일 상태, `known`은 우리가 마지막으로 열거나 저장한 상태.
    pub fn tick(&mut self, now: Instant, current: Option<FileState>, known: Option<&FileState>) -> ReloadDecision {
        let (Some(first), Some(last)) = (self.first_event, self.last_event) else {
            return ReloadDecision::Idle;
        };
        if now.duration_since(last) < RELOAD_DELAY {
            return ReloadDecision::Wait;
        }
        let Some(current) = current else {
            // 파일이 없다(이름 변경·삭제) — 이름 변경은 app.rs가 따로 따라간다.
            self.reset();
            return ReloadDecision::Skip;
        };
        if known == Some(&current) {
            // 내용이 그대로 — 같은 폴더의 다른 파일, 또는 우리가 방금 저장한 결과.
            self.reset();
            return ReloadDecision::Skip;
        }
        if now.duration_since(first) >= MAX_WAIT {
            self.reset();
            return ReloadDecision::Reload;
        }
        if self.last_snapshot.as_ref() != Some(&current) {
            // 직전 확인과 다르다(첫 확인 포함) — 누군가 아직 쓰는 중일 수 있으니 한 번 더 기다린다.
            self.last_snapshot = Some(current);
            self.last_event = Some(now);
            return ReloadDecision::Wait;
        }
        self.reset();
        ReloadDecision::Reload
    }

    fn reset(&mut self) {
        *self = Self::default();
    }
}

/// 이 감시 이벤트가 `file_name` 파일의 내용 변경일 수 있는지. 경로가 없는 이벤트는 판단할 수
/// 없어 변경 가능성으로 취급한다(실제 판정은 파일 상태 비교가 한다). macOS는 파일명을 NFD로
/// 돌려줄 수 있어 NFC로 맞춰 비교한다(§7 "한글 파일명 자소 분리").
pub fn event_touches_file(event: &notify::Event, file_name: &OsStr) -> bool {
    if matches!(event.kind, notify::EventKind::Access(_)) {
        return false;
    }
    let wanted: String = file_name.to_string_lossy().nfc().collect();
    event.paths.is_empty()
        || event.paths.iter().any(|path| {
            path.file_name()
                .is_some_and(|name| name.to_string_lossy().nfc().collect::<String>() == wanted)
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify::event::{AccessKind, CreateKind, ModifyKind};
    use notify::{Event, EventKind};
    use std::path::PathBuf;

    fn state(len: u64) -> FileState {
        FileState {
            len,
            modified: Some(SystemTime::UNIX_EPOCH + Duration::from_secs(len)),
        }
    }

    #[test]
    fn nothing_pending_is_idle() {
        let mut scheduler = ReloadScheduler::default();
        assert_eq!(scheduler.tick(Instant::now(), Some(state(1)), None), ReloadDecision::Idle);
    }

    /// 이벤트 직후엔 기다리고, 조용해진 뒤 첫 확인도 기다리며, 두 번 연속 같으면 다시 연다.
    #[test]
    fn reloads_only_after_file_stops_changing() {
        let start = Instant::now();
        let known = state(100);
        let mut scheduler = ReloadScheduler::default();
        scheduler.note_change(start);

        assert_eq!(scheduler.tick(start, Some(state(150)), Some(&known)), ReloadDecision::Wait);
        let t1 = start + RELOAD_DELAY;
        assert_eq!(scheduler.tick(t1, Some(state(150)), Some(&known)), ReloadDecision::Wait);
        // 아직 쓰는 중(크기 변함) → 다시 기다림
        let t2 = t1 + RELOAD_DELAY;
        assert_eq!(scheduler.tick(t2, Some(state(200)), Some(&known)), ReloadDecision::Wait);
        let t3 = t2 + RELOAD_DELAY;
        assert_eq!(scheduler.tick(t3, Some(state(200)), Some(&known)), ReloadDecision::Reload);
        assert_eq!(scheduler.tick(t3, Some(state(200)), Some(&known)), ReloadDecision::Idle);
    }

    /// 자기 저장·다른 파일 이벤트: 상태가 알고 있는 것과 같으면 다시 열지 않는다.
    #[test]
    fn unchanged_content_is_skipped() {
        let start = Instant::now();
        let known = state(100);
        let mut scheduler = ReloadScheduler::default();
        scheduler.note_change(start);
        let later = start + RELOAD_DELAY;
        assert_eq!(scheduler.tick(later, Some(state(100)), Some(&known)), ReloadDecision::Skip);
        assert_eq!(scheduler.tick(later, Some(state(100)), Some(&known)), ReloadDecision::Idle);
    }

    #[test]
    fn missing_file_is_skipped() {
        let start = Instant::now();
        let mut scheduler = ReloadScheduler::default();
        scheduler.note_change(start);
        assert_eq!(scheduler.tick(start + RELOAD_DELAY, None, Some(&state(1))), ReloadDecision::Skip);
    }

    /// 계속 바뀌는 파일도 최대 대기 시간이 지나면 다시 연다.
    #[test]
    fn keeps_changing_file_reloads_after_max_wait() {
        let start = Instant::now();
        let known = state(1);
        let mut scheduler = ReloadScheduler::default();
        scheduler.note_change(start);
        let mut now = start;
        let mut size = 2;
        loop {
            now += RELOAD_DELAY;
            size += 1;
            match scheduler.tick(now, Some(state(size)), Some(&known)) {
                ReloadDecision::Wait => assert!(now.duration_since(start) < MAX_WAIT + RELOAD_DELAY),
                ReloadDecision::Reload => break,
                other => panic!("unexpected {other:?}"),
            }
        }
        assert!(now.duration_since(start) >= MAX_WAIT);
    }

    /// 실제 파일 시스템 감시(macOS FSEvents 등)가 두 가지 저장 방식 모두에서 이 파일의 변경
    /// 이벤트를 주는지 — (1) 제자리에 다시 쓰기, (2) 임시 파일에 쓰고 원본 자리로 옮기기(원자적
    /// 저장, 편집기·우리 북마크 저장 방식). 알림 지연 때문에 느리고 환경 영향을 받아 기본 제외:
    /// `cargo test -p ui --bins -- --ignored real_watcher`
    #[test]
    #[ignore]
    fn real_watcher_reports_in_place_and_atomic_saves() {
        use notify::Watcher;

        let dir = tempfile::tempdir().unwrap();
        // tempdir 경로가 /var → /private/var 심볼릭 링크를 거쳐 FSEvents가 실제 경로로 알려도
        // 파일명만 비교하므로 무관하다.
        let target = dir.path().join("문서.pdf");
        std::fs::write(&target, b"v1").unwrap();

        let (tx, rx) = std::sync::mpsc::channel();
        let mut watcher = notify::recommended_watcher(move |res| {
            let _ = tx.send(res);
        })
        .unwrap();
        watcher
            .watch(dir.path(), notify::RecursiveMode::NonRecursive)
            .unwrap();
        std::thread::sleep(Duration::from_millis(300));

        let name = target.file_name().unwrap().to_os_string();
        let wait_for_touch = |label: &str| {
            let deadline = Instant::now() + Duration::from_secs(5);
            while let Some(left) = deadline.checked_duration_since(Instant::now()) {
                match rx.recv_timeout(left) {
                    Ok(Ok(event)) if event_touches_file(&event, &name) => return,
                    Ok(_) => continue,
                    Err(_) => break,
                }
            }
            panic!("{label}: 5초 안에 이 파일의 변경 이벤트가 오지 않음");
        };

        std::fs::write(&target, b"v2 in place").unwrap();
        wait_for_touch("제자리 쓰기");
        while rx.try_recv().is_ok() {}

        let temp = dir.path().join("문서.pdf.tmp");
        std::fs::write(&temp, b"v3 atomic").unwrap();
        std::fs::rename(&temp, &target).unwrap();
        wait_for_touch("임시 파일 → 원본 자리로 옮기기");
    }

    #[test]
    fn event_filter_matches_file_name_and_ignores_reads() {
        let name = OsStr::new("의료.pdf");
        let modify = Event::new(EventKind::Modify(ModifyKind::Any)).add_path(PathBuf::from("/docs/의료.pdf"));
        assert!(event_touches_file(&modify, name));

        // macOS가 돌려주는 NFD 파일명도 같은 파일로 본다.
        let nfd: String = "의료.pdf".nfd().collect();
        let created = Event::new(EventKind::Create(CreateKind::File)).add_path(PathBuf::from(format!("/docs/{nfd}")));
        assert!(event_touches_file(&created, name));

        let other = Event::new(EventKind::Modify(ModifyKind::Any)).add_path(PathBuf::from("/docs/other.pdf"));
        assert!(!event_touches_file(&other, name));

        let read = Event::new(EventKind::Access(AccessKind::Any)).add_path(PathBuf::from("/docs/의료.pdf"));
        assert!(!event_touches_file(&read, name));

        assert!(event_touches_file(&Event::new(EventKind::Any), name));
    }
}
