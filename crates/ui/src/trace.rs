//! 최근 동작 기록(링 버퍼) — 패닉이 나면 `panic.log`에 함께 남긴다(main.rs `install_panic_log`).
//!
//! 텍스처 크래시처럼 문제가 드러난 곳(GPU 제출)과 원인이 생긴 곳(텍스처 해제)이 다른 버그는
//! 백트레이스만으로 경로를 알 수 없다. 텍스처 생성·교체·해제·그리기와 화면 전환을 프레임
//! 번호와 함께 최근 `CAPACITY`개만 남겨, 패닉 메시지의 텍스처 번호(`Managed(N)`)와 대조한다.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError, TryLockError};

const CAPACITY: usize = 3000;

static FRAME: AtomicU64 = AtomicU64::new(0);
static EVENTS: Mutex<VecDeque<String>> = Mutex::new(VecDeque::new());

/// 프레임(egui 패스) 시작 — 이후 기록에 붙는 번호를 올린다.
pub fn next_frame() {
    FRAME.fetch_add(1, Ordering::Relaxed);
}

pub fn record(message: std::fmt::Arguments<'_>) {
    let line = format!("f{} {message}", FRAME.load(Ordering::Relaxed));
    let mut events = EVENTS.lock().unwrap_or_else(PoisonError::into_inner);
    if events.len() >= CAPACITY {
        events.pop_front();
    }
    events.push_back(line);
}

/// 기록 전체(오래된 것부터).
#[cfg_attr(not(test), allow(dead_code))]
pub fn dump() -> String {
    dump_recent(usize::MAX)
}

/// 최근 `max_lines`줄(오래된 것부터). 패닉 훅에서 부르므로 잠금을 기다리지 않는다 — 기록
/// 도중 같은 스레드에서 패닉이 났다면 잠금이 이미 잡혀 있을 수 있다.
pub fn dump_recent(max_lines: usize) -> String {
    let events = match EVENTS.try_lock() {
        Ok(events) => events,
        Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
        Err(TryLockError::WouldBlock) => return "(기록이 잠겨 있어 생략)\n".to_string(),
    };
    let skip = events.len().saturating_sub(max_lines);
    let mut out = String::new();
    for line in events.iter().skip(skip) {
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// 기록은 프로세스 전역이라, 기록 내용을 확인하는 테스트끼리 병렬로 돌면 서로의 줄을 밀어낸다
/// (링 버퍼 넘침 테스트가 수천 줄을 쓰는 동안 다른 테스트의 기록이 사라짐) — 순서대로 돌게 잠근다.
#[cfg(test)]
pub static TEST_LOCK: Mutex<()> = Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_only_the_most_recent_events() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
        for i in 0..CAPACITY + 50 {
            record(format_args!("trace-test {i}"));
        }
        let dump = dump();
        assert!(dump.lines().count() <= CAPACITY);
        // 다른 테스트가 동시에 기록할 수 있어 마지막 줄 대신 포함 여부로 확인한다.
        assert!(dump.contains(&format!("trace-test {}", CAPACITY + 49)));
        assert!(!dump.contains("trace-test 0\n"));
    }

    #[test]
    fn dump_recent_keeps_only_the_tail() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
        for i in 0..20 {
            record(format_args!("tail-test {i}"));
        }
        let tail = dump_recent(5);
        assert_eq!(tail.lines().count(), 5);
        assert!(tail.ends_with("tail-test 19\n"));
        assert!(!tail.contains("tail-test 14\n"));
    }
}
