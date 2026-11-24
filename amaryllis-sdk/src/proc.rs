//! Preemptive Ring 3 process control & ELF64 spawning syscalls.

use crate::syscall::{
    syscall0, syscall1, syscall2, SYS_EXIT, SYS_GETPID, SYS_PROC_COUNT, SYS_SPAWN_ELF, SYS_YIELD,
};

/// Returns the PID of the calling Ring 3 process.
#[inline]
pub fn getpid() -> usize {
    let pid = unsafe { syscall0(SYS_GETPID) };
    if pid < 0 {
        0
    } else {
        pid as usize
    }
}

/// Voluntarily yields the remaining preemptive time slice to the scheduler.
#[inline]
pub fn yield_now() {
    let _ = unsafe { syscall0(SYS_YIELD) };
}

/// Terminates the calling Ring 3 process cleanly and returns `code` to the OS.
#[inline]
pub fn exit(code: i32) -> ! {
    unsafe {
        let _ = syscall1(SYS_EXIT, code as i64 as u64);
    }
    loop {
        core::hint::spin_loop();
    }
}

/// Spawns another ELF64 executable asynchronously from RAMFS (`/`) or `/disk` FAT32.
/// Accepts either a plain VFS path (`"/counter.elf"`) or path + arguments (`"/app.elf arg1 arg2"`).
#[inline]
pub fn spawn(cmdline: &str) -> Result<(), i64> {
    if cmdline.is_empty() {
        return Err(-22);
    }
    let res = unsafe { syscall2(SYS_SPAWN_ELF, cmdline.as_ptr() as u64, cmdline.len() as u64) };
    if res >= 0 {
        Ok(())
    } else {
        Err(res)
    }
}

/// Alias for `spawn` emphasizing command-line argument support (`argv`).
#[inline]
pub fn spawn_with_args(cmdline: &str) -> Result<(), i64> {
    spawn(cmdline)
}

/// Returns the total number of currently registered Ring 3 processes.
#[inline]
pub fn active_count() -> usize {
    let n = unsafe { syscall0(SYS_PROC_COUNT) };
    if n < 0 {
        0
    } else {
        n as usize
    }
}
