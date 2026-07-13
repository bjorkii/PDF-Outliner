//! 비정상 종료(크래시/강제종료/시스템 재부팅) 후 저장되지 않은 북마크 편집을 복구하기
//! 위한 자동저장. PDF 자체 저장("저장" 버튼, lopdf)과는 별개로, 편집 중인 상태를
//! 주기적으로 JSON 스냅샷 하나에 남긴다.
//!
//! **파일은 항상 고정된 경로 하나만 쓰고 매번 덮어쓴다** — 세션/문서마다 새 파일을
//! 만들지 않으므로 시간이 지나도 쌓이지 않는다.
//!
//! **`clean_exit` 플래그가 핵심**: 저장되지 않은 편집이 있을 때만 주기적으로
//! `false`로 갱신하고, 정상 종료 시에는(저장했든 안 했든, 어떤 경로로 종료하든) 항상
//! `true`로 남긴다. 다음 실행 때 파일에 `clean_exit == false`가 남아있다면, 그 사이
//! `save()`/`on_exit()`가 실행되지 못한 채(=비정상 종료) 편집이 끊겼다는 뜻이다.

use bookmark::BookmarkNode;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const APP_ID: &str = "PDF Outliner";
const FILE_NAME: &str = "autosave.json";

#[derive(Serialize, Deserialize)]
struct AutosaveState {
    clean_exit: bool,
    file_path: PathBuf,
    bookmarks: Vec<BookmarkNode>,
    saved_at: DateTime<Utc>,
}

/// 크래시 복구 대상으로 감지된 이전 세션의 스냅샷.
pub struct RecoverableSession {
    pub file_path: PathBuf,
    pub bookmarks: Vec<BookmarkNode>,
    pub saved_at: DateTime<Utc>,
}

/// `main.rs`의 `run_native("PDF Outliner", ...)`와 동일한 app_id를 써서, eframe이 이미
/// 자체 persistence(.ron)에 쓰는 것과 같은 디렉터리를 그대로 재사용한다(별도 의존성 없이).
fn autosave_path() -> Option<PathBuf> {
    eframe::storage_dir(APP_ID).map(|dir| dir.join(FILE_NAME))
}

/// 앱 시작 시 딱 한 번 호출. 이전 세션이 비정상 종료됐고(`clean_exit == false`) 복구할
/// 편집이 남아있으면 그 내용을 반환한다. 이 함수는 읽기만 하므로, 이번 세션의 `record()`
/// 호출보다 반드시 먼저 실행돼야 이전 세션의 흔적을 덮어쓰지 않는다.
pub fn check_for_crash_recovery() -> Option<RecoverableSession> {
    check_for_crash_recovery_at(&autosave_path()?)
}

/// 자동저장 스냅샷을 (있다면 고정 경로의 파일 하나에) 덮어쓴다. `dirty`가 `true`면
/// 저장되지 않은 편집이 있다는 뜻이라 `clean_exit: false`로 남기고, `false`면(이미
/// 저장했거나 애초에 편집이 없거나) `clean_exit: true`로 남긴다.
///
/// 정상 종료 시(`on_exit`)에는 실제 dirty 상태와 무관하게 `dirty: false`를 넘겨 호출해
/// "이 세션은 끝까지 정상적으로 종료됐다"를 명시적으로 표시한다 — 사용자가 "저장 안 함"을
/// 선택하고 종료한 경우까지 다음 실행에 크래시로 오인해 복구 프롬프트를 띄우면 안 되기
/// 때문이다.
pub fn record(current_file: Option<&Path>, bookmarks: &[BookmarkNode], dirty: bool) {
    let Some(path) = autosave_path() else { return };
    record_at(&path, current_file, bookmarks, dirty);
}

fn check_for_crash_recovery_at(path: &Path) -> Option<RecoverableSession> {
    let content = std::fs::read_to_string(path).ok()?;
    let state: AutosaveState = serde_json::from_str(&content).ok()?;

    if state.clean_exit {
        return None;
    }

    Some(RecoverableSession {
        file_path: state.file_path,
        bookmarks: state.bookmarks,
        saved_at: state.saved_at,
    })
}

fn record_at(path: &Path, current_file: Option<&Path>, bookmarks: &[BookmarkNode], dirty: bool) {
    let Some(current_file) = current_file else {
        return;
    };

    let state = AutosaveState {
        clean_exit: !dirty,
        file_path: current_file.to_path_buf(),
        bookmarks: bookmarks.to_vec(),
        saved_at: Utc::now(),
    };

    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string(&state) {
        let _ = std::fs::write(path, json);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_bookmarks() -> Vec<BookmarkNode> {
        vec![BookmarkNode::new("장 1", 3)]
    }

    #[test]
    fn dirty_record_is_recoverable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("autosave.json");
        let doc_path = PathBuf::from("/tmp/example.pdf");
        let bookmarks = sample_bookmarks();

        record_at(&path, Some(&doc_path), &bookmarks, true);

        let recovered = check_for_crash_recovery_at(&path).expect("dirty snapshot should be recoverable");
        assert_eq!(recovered.file_path, doc_path);
        assert_eq!(recovered.bookmarks, bookmarks);
    }

    #[test]
    fn clean_record_is_not_recoverable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("autosave.json");
        let doc_path = PathBuf::from("/tmp/example.pdf");

        // 크래시가 있었다고 가정하고 dirty로 한 번 기록한 뒤,
        record_at(&path, Some(&doc_path), &sample_bookmarks(), true);
        assert!(check_for_crash_recovery_at(&path).is_some());

        // 정상 종료(on_exit)를 흉내내 dirty: false로 다시 기록하면 더 이상 복구 대상이
        // 아니어야 한다 — "저장 안 함"으로 정상 종료한 경우까지 다음 실행에 복구
        // 프롬프트가 뜨면 안 되는 게 이 기능의 핵심 요구사항.
        record_at(&path, Some(&doc_path), &sample_bookmarks(), false);
        assert!(check_for_crash_recovery_at(&path).is_none());
    }

    #[test]
    fn missing_file_is_not_recoverable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("does_not_exist.json");
        assert!(check_for_crash_recovery_at(&path).is_none());
    }

    #[test]
    fn no_current_file_skips_write() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("autosave.json");

        record_at(&path, None, &sample_bookmarks(), true);

        assert!(!path.exists());
    }
}
