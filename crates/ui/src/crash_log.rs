//! 패닉 기록 파일(`panic.log`)과 그 보관 정책.
//!
//! release는 `panic = "abort"`라 어느 스레드에서든 패닉이 나면 앱이 즉시 꺼지고, .app으로
//! 실행하면 stderr가 어디에도 안 보인다 — 그래서 패닉 내용·백트레이스·최근 동작 기록
//! (`crate::trace`)을 파일로 남긴다(2026-09-14 텍스처 크래시 추적 때 도입).
//!
//! **보관 정책** — 파일이 끝없이 커지지 않게(2026-09-14 요청):
//! - 새 기록을 쓰기 전에 `panic.log`가 `MAX_LOG_BYTES` 이상이면 `panic.log.1`로 옮기고(이전
//!   `panic.log.1`은 덮어써 버림) 새 파일에 쓴다 → 디스크 사용량은 최대 약 2×`MAX_LOG_BYTES`.
//! - 동작 기록은 링 버퍼 전체가 아니라 최근 `TRACE_LINES`줄만 싣는다 — 크래시 원인 추적에는
//!   직전 몇 초면 충분하고, 한 건을 수십 KB로 묶어 둔다.
//! - 패닉은 드문 사건이라 기간 기준 삭제는 두지 않는다(크기 기준만으로 상한이 보장된다).

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// 이 크기를 넘으면 다음 기록 전에 `panic.log.1`로 넘긴다.
pub const MAX_LOG_BYTES: u64 = 1024 * 1024;
/// 한 건에 싣는 최근 동작 기록 줄 수.
pub const TRACE_LINES: usize = 400;

/// macOS `~/Library/Logs/PDF Outliner/panic.log`, Windows `%LOCALAPPDATA%\PDF Outliner\panic.log`.
pub fn log_path() -> Option<PathBuf> {
    if cfg!(target_os = "windows") {
        std::env::var_os("LOCALAPPDATA")
            .map(|dir| PathBuf::from(dir).join("PDF Outliner").join("panic.log"))
    } else {
        std::env::var_os("HOME").map(|home| PathBuf::from(home).join("Library/Logs/PDF Outliner/panic.log"))
    }
}

/// 패닉 훅을 설치한다. 기본 훅(터미널 출력)도 그대로 호출한다.
pub fn install(process: &'static str) {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if let Some(path) = log_path() {
            let unix_secs = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |elapsed| elapsed.as_secs());
            let entry = format!(
                "[unix {unix_secs}] {process} 스레드 '{}' 패닉: {info}\n{}\n\
                 --- 최근 동작 기록(최근 {TRACE_LINES}줄, 오래된 것부터, f = 프레임 번호) ---\n{}--- 기록 끝 ---\n\n",
                std::thread::current().name().unwrap_or("?"),
                std::backtrace::Backtrace::force_capture(),
                crate::trace::dump_recent(TRACE_LINES)
            );
            let _ = append_entry(&path, &entry, MAX_LOG_BYTES);
        }
        default_hook(info);
    }));
}

/// 파일이 `max_bytes` 이상이면 `<파일명>.1`로 옮긴(이전 백업은 덮어씀) 뒤 `entry`를 덧붙인다.
pub fn append_entry(path: &Path, entry: &str, max_bytes: u64) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    if fs::metadata(path).is_ok_and(|meta| meta.len() >= max_bytes) {
        fs::rename(path, backup_path(path))?;
    }
    let mut file = fs::OpenOptions::new().create(true).append(true).open(path)?;
    file.write_all(entry.as_bytes())
}

fn backup_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().map(|name| name.to_os_string()).unwrap_or_default();
    name.push(".1");
    path.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entries_accumulate_below_the_limit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("logs").join("panic.log");
        append_entry(&path, "first\n", 100).unwrap();
        append_entry(&path, "second\n", 100).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "first\nsecond\n");
        assert!(!backup_path(&path).exists());
    }

    /// 한도를 넘은 파일은 .1로 넘기고 새 파일에 쓴다. 다시 넘으면 이전 .1은 덮어써져 파일은
    /// 항상 최대 두 개다.
    #[test]
    fn oversized_log_rotates_to_single_backup() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("panic.log");
        let backup = backup_path(&path);
        assert_eq!(backup.file_name().unwrap(), "panic.log.1");

        append_entry(&path, &"a".repeat(20), 10).unwrap();
        append_entry(&path, "b\n", 10).unwrap();
        assert_eq!(fs::read_to_string(&backup).unwrap(), "a".repeat(20));
        assert_eq!(fs::read_to_string(&path).unwrap(), "b\n");

        append_entry(&path, &"c".repeat(20), 10).unwrap(); // 아직 2바이트라 교체 없이 덧붙음
        append_entry(&path, "d\n", 10).unwrap(); // 이제 22바이트 → 교체
        assert_eq!(fs::read_to_string(&backup).unwrap(), format!("b\n{}", "c".repeat(20)));
        assert_eq!(fs::read_to_string(&path).unwrap(), "d\n");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 2);
    }
}
