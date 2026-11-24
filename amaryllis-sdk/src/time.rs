//! Timekeeping and sleep syscalls backed by AmaryllisOS 1000 Hz PIT timer.

use crate::syscall::{syscall0, syscall1, SYS_SLEEP_MS, SYS_UPTIME_MS};

/// Returns kernel monotonic uptime in milliseconds since boot.
#[inline]
pub fn uptime_ms() -> u64 {
    let v = unsafe { syscall0(SYS_UPTIME_MS) };
    if v < 0 {
        0
    } else {
        v as u64
    }
}

/// Suspends the current Ring 3 process for at least `ms` milliseconds while
/// yielding the CPU to the Desktop compositor and other runnable tasks.
#[inline]
pub fn sleep_ms(ms: u64) {
    if ms == 0 {
        return;
    }
    let _ = unsafe { syscall1(SYS_SLEEP_MS, ms) };
}
