//! Raw x86_64 `SYSCALL` numbers and inline-assembly ABI wrappers for AmaryllisOS Ring 3.

use core::arch::asm;

pub const SYS_READ: u64 = 0;
pub const SYS_WRITE: u64 = 1;
pub const SYS_OPEN: u64 = 2;
pub const SYS_CLOSE: u64 = 3;
pub const SYS_STAT: u64 = 4;
pub const SYS_FSTAT: u64 = 5;
pub const SYS_LSEEK: u64 = 8;
pub const SYS_MMAP: u64 = 9;
pub const SYS_MUNMAP: u64 = 11;
pub const SYS_BRK: u64 = 12;
pub const SYS_PIPE: u64 = 22;
pub const SYS_YIELD: u64 = 24;
pub const SYS_SLEEP_MS: u64 = 35;
pub const SYS_GETPID: u64 = 39;
pub const SYS_SOCKET: u64 = 41;
pub const SYS_CONNECT: u64 = 42;
pub const SYS_SEND: u64 = 44;
pub const SYS_RECV: u64 = 45;
pub const SYS_BIND: u64 = 49;
pub const SYS_FORK: u64 = 57;
pub const SYS_EXIT: u64 = 60;
pub const SYS_WAITPID: u64 = 61;
pub const SYS_GETDENTS: u64 = 78;
pub const SYS_MKDIR: u64 = 83;
pub const SYS_UNLINK: u64 = 87;
pub const SYS_UPTIME_MS: u64 = 96;
pub const SYS_BEEP: u64 = 200;
pub const SYS_NET_PING: u64 = 210;
pub const SYS_DNS_RESOLVE: u64 = 211;
pub const SYS_HTTP_GET: u64 = 212;
pub const SYS_PROC_COUNT: u64 = 220;
pub const SYS_CREATE_WINDOW: u64 = 300;
pub const SYS_WINDOW_BLIT: u64 = 301;
pub const SYS_POLL_GUI_EVENT: u64 = 302;
pub const SYS_CREATE_SHM_WINDOW: u64 = 303;
pub const SYS_COMMIT_SHM_WINDOW: u64 = 304;
pub const SYS_SPAWN_ELF: u64 = 310;
pub const SYS_SHM_CREATE: u64 = 320;
pub const SYS_SHM_MAP: u64 = 321;
pub const SYS_SHM_UNMAP: u64 = 322;
pub const SYS_MACH_PORT_ALLOCATE: u64 = 330;
pub const SYS_MACH_PORT_DEALLOCATE: u64 = 331;
pub const SYS_MACH_MSG: u64 = 332;
pub const SYS_BOOTSTRAP_REGISTER: u64 = 333;
pub const SYS_BOOTSTRAP_LOOKUP: u64 = 334;

pub const ENOENT: i64 = -2;
pub const EIO: i64 = -5;
pub const EBADF: i64 = -9;
pub const EAGAIN: i64 = -11;
pub const ENOMEM: i64 = -12;
pub const EFAULT: i64 = -14;
pub const EEXIST: i64 = -17;
pub const ENOTDIR: i64 = -20;
pub const EISDIR: i64 = -21;
pub const EINVAL: i64 = -22;
pub const EMFILE: i64 = -24;
pub const EROFS: i64 = -30;
pub const EPIPE: i64 = -32;
pub const ENOSYS: i64 = -38;
pub const ETIMEDOUT: i64 = -110;
pub const ECONNREFUSED: i64 = -111;

#[inline(always)]
pub unsafe fn syscall0(num: u64) -> i64 {
    let ret: i64;
    unsafe {
        asm!(
            "syscall",
            inlateout("rax") num => ret,
            out("rcx") _,
            out("r11") _,
            options(nostack)
        );
    }
    ret
}

#[inline(always)]
pub unsafe fn syscall1(num: u64, arg1: u64) -> i64 {
    let ret: i64;
    unsafe {
        asm!(
            "syscall",
            inlateout("rax") num => ret,
            in("rdi") arg1,
            out("rcx") _,
            out("r11") _,
            options(nostack)
        );
    }
    ret
}

#[inline(always)]
pub unsafe fn syscall2(num: u64, arg1: u64, arg2: u64) -> i64 {
    let ret: i64;
    unsafe {
        asm!(
            "syscall",
            inlateout("rax") num => ret,
            in("rdi") arg1,
            in("rsi") arg2,
            out("rcx") _,
            out("r11") _,
            options(nostack)
        );
    }
    ret
}

#[inline(always)]
pub unsafe fn syscall3(num: u64, arg1: u64, arg2: u64, arg3: u64) -> i64 {
    let ret: i64;
    unsafe {
        asm!(
            "syscall",
            inlateout("rax") num => ret,
            in("rdi") arg1,
            in("rsi") arg2,
            in("rdx") arg3,
            out("rcx") _,
            out("r11") _,
            options(nostack)
        );
    }
    ret
}

#[inline(always)]
pub unsafe fn syscall4(num: u64, arg1: u64, arg2: u64, arg3: u64, arg4: u64) -> i64 {
    let ret: i64;
    unsafe {
        asm!(
            "syscall",
            inlateout("rax") num => ret,
            in("rdi") arg1,
            in("rsi") arg2,
            in("rdx") arg3,
            in("r10") arg4,
            out("rcx") _,
            out("r11") _,
            options(nostack)
        );
    }
    ret
}

#[inline(always)]
pub unsafe fn syscall5(
    num: u64,
    arg1: u64,
    arg2: u64,
    arg3: u64,
    arg4: u64,
    arg5: u64,
) -> i64 {
    let ret: i64;
    unsafe {
        asm!(
            "syscall",
            inlateout("rax") num => ret,
            in("rdi") arg1,
            in("rsi") arg2,
            in("rdx") arg3,
            in("r10") arg4,
            in("r8") arg5,
            out("rcx") _,
            out("r11") _,
            options(nostack)
        );
    }
    ret
}

#[inline(always)]
pub unsafe fn syscall6(
    num: u64,
    arg1: u64,
    arg2: u64,
    arg3: u64,
    arg4: u64,
    arg5: u64,
    arg6: u64,
) -> i64 {
    let ret: i64;
    unsafe {
        asm!(
            "syscall",
            inlateout("rax") num => ret,
            in("rdi") arg1,
            in("rsi") arg2,
            in("rdx") arg3,
            in("r10") arg4,
            in("r8") arg5,
            in("r9") arg6,
            out("rcx") _,
            out("r11") _,
            options(nostack)
        );
    }
    ret
}
