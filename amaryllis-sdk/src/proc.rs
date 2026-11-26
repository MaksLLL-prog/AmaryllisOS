//! Preemptive Ring 3 process control & ELF64 spawning syscalls.

use crate::syscall::{
    syscall0, syscall1, syscall2, syscall3, SYS_EXIT, SYS_FORK, SYS_GETPID, SYS_PROC_COUNT,
    SYS_SPAWN_ELF, SYS_WAITPID, SYS_YIELD,
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

/// Forks the current Ring 3 process with Copy-on-Write (`CoW`) page tables.
///
/// Returns `Ok(0)` in the child process and `Ok(child_pid)` in the parent process.
#[inline]
pub fn fork() -> Result<usize, i64> {
    let res = unsafe { syscall0(SYS_FORK) };
    if res >= 0 {
        Ok(res as usize)
    } else {
        Err(res)
    }
}

/// Waits for a child process (`pid <= 0` waits for any child) to terminate.
///
/// Returns `Ok((child_pid, exit_code))` on success.
#[inline]
pub fn waitpid(pid: isize) -> Result<(usize, i32), i64> {
    let mut status: i32 = 0;
    let res = unsafe { syscall3(SYS_WAITPID, pid as i64 as u64, &mut status as *mut i32 as u64, 0) };
    if res >= 0 {
        Ok((res as usize, status))
    } else {
        Err(res)
    }
}

/// Non-blocking `waitpid(..., WNOHANG)`. Returns `Ok(Some((child_pid, exit_code)))` if reaped,
/// `Ok(None)` if matching child is still running, or `Err(errno)` (e.g. `-10` `ECHILD`).
#[inline]
pub fn try_waitpid(pid: isize) -> Result<Option<(usize, i32)>, i64> {
    let mut status: i32 = 0;
    let res = unsafe { syscall3(SYS_WAITPID, pid as i64 as u64, &mut status as *mut i32 as u64, 1) };
    if res > 0 {
        Ok(Some((res as usize, status)))
    } else if res == 0 {
        Ok(None)
    } else {
        Err(res)
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
