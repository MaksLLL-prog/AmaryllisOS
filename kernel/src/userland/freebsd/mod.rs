pub mod kqueue;
pub mod signal;
pub mod sysctl;

use alloc::string::String;
use alloc::vec;
use core::arch::asm;

use crate::arch::rtc;
use crate::interrupts::timer;
use crate::memory::vmm::PhysicalMemoryMapping;
use crate::memory::PhysicalFrameAllocator;
use crate::storage::vfs::{self, VfsError, VfsNodeKind};
use crate::userland::ipc;
use crate::userland::sched::{self, FdEntry, UserTaskState, WaitReason};
use crate::userland::syscall::{
    self, build_resume_ctx_from_syscall, copy_from_user_bytes_in, copy_str_from_user_in,
    copy_to_user_bytes_in, validate_user_slice_in, SyscallFrame, UserStat,
};

pub const ELFOSABI_SYSV: u8 = 0;
pub const ELFOSABI_FREEBSD: u8 = 9;
pub const PT_NOTE: u32 = 4;
pub const PT_PHDR: u32 = 6;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum ProcessAbi {
    Amaryllis = 0,
    FreeBsd = 9,
}

impl ProcessAbi {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Amaryllis => "Amaryllis",
            Self::FreeBsd => "FreeBSD",
        }
    }
}

// FreeBSD ELF Auxiliary Vector (AT_*) keys
pub const AT_NULL: u64 = 0;
pub const AT_IGNORE: u64 = 1;
pub const AT_EXECFD: u64 = 2;
pub const AT_PHDR: u64 = 3;
pub const AT_PHENT: u64 = 4;
pub const AT_PHNUM: u64 = 5;
pub const AT_PAGESZ: u64 = 6;
pub const AT_BASE: u64 = 7;
pub const AT_FLAGS: u64 = 8;
pub const AT_ENTRY: u64 = 9;
pub const AT_EXECPATH: u64 = 15;
pub const AT_OSRELDATE: u64 = 18;
pub const AT_NCPUS: u64 = 19;
pub const AT_STACKPROT: u64 = 23;

// FreeBSD x86_64 Syscall Numbers
pub const SYS_FBSD_EXIT: u64 = 1;
pub const SYS_FBSD_FORK: u64 = 2;
pub const SYS_FBSD_READ: u64 = 3;
pub const SYS_FBSD_WRITE: u64 = 4;
pub const SYS_FBSD_OPEN: u64 = 5;
pub const SYS_FBSD_CLOSE: u64 = 6;
pub const SYS_FBSD_WAIT4: u64 = 7;
pub const SYS_FBSD_UNLINK: u64 = 10;
pub const SYS_FBSD_GETPID: u64 = 20;
pub const SYS_FBSD_GETUID: u64 = 24;
pub const SYS_FBSD_GETEUID: u64 = 25;
pub const SYS_FBSD_KILL: u64 = 37;
pub const SYS_FBSD_GETPPID: u64 = 39;
pub const SYS_FBSD_PIPE: u64 = 42;
pub const SYS_FBSD_GETEGID: u64 = 43;
pub const SYS_FBSD_GETGID: u64 = 47;
pub const SYS_FBSD_SIGALTSTACK: u64 = 53;
pub const SYS_FBSD_IOCTL: u64 = 54;
pub const SYS_FBSD_MUNMAP: u64 = 73;
pub const SYS_FBSD_FCNTL: u64 = 92;
pub const SYS_FBSD_GETTIMEOFDAY: u64 = 116;
pub const SYS_FBSD_MKDIR: u64 = 136;
pub const SYS_FBSD_SYSARCH: u64 = 165;
pub const SYS_FBSD_STAT: u64 = 188;
pub const SYS_FBSD_FSTAT: u64 = 189;
pub const SYS_FBSD_SYSCTL: u64 = 202;
pub const SYS_FBSD_CLOCK_GETTIME: u64 = 232;
pub const SYS_FBSD_NANOSLEEP: u64 = 240;
pub const SYS_FBSD_ISSETUGID: u64 = 253;
pub const SYS_FBSD_GETDENTS: u64 = 272;
pub const SYS_FBSD_MACH_PORT_ALLOCATE: u64 = 330;
pub const SYS_FBSD_MACH_PORT_DEALLOCATE: u64 = 331;
pub const SYS_FBSD_MACH_MSG: u64 = 332;
pub const SYS_FBSD_BOOTSTRAP_REGISTER: u64 = 333;
pub const SYS_FBSD_BOOTSTRAP_LOOKUP: u64 = 334;
pub const SYS_FBSD_SIGPROCMASK: u64 = 340;
pub const SYS_FBSD_KQUEUE: u64 = 362;
pub const SYS_FBSD_KEVENT: u64 = 363;
pub const SYS_FBSD_SIGACTION: u64 = 416;
pub const SYS_FBSD_SIGRETURN: u64 = 417;
pub const SYS_FBSD_THR_EXIT: u64 = 431;
pub const SYS_FBSD_THR_SELF: u64 = 432;
pub const SYS_FBSD_THR_KILL: u64 = 433;
pub const SYS_FBSD_UMTX_OP: u64 = 454;
pub const SYS_FBSD_THR_NEW: u64 = 455;
pub const SYS_FBSD_MMAP: u64 = 477;
pub const SYS_FBSD_LSEEK: u64 = 478;
pub const SYS_FBSD_PIPE2: u64 = 542;

// FreeBSD positive errnos (sys/errno.h)
pub const FBSD_EPERM: u64 = 1;
pub const FBSD_ENOENT: u64 = 2;
pub const FBSD_ESRCH: u64 = 3;
pub const FBSD_EINTR: u64 = 4;
pub const FBSD_EIO: u64 = 5;
pub const FBSD_EBADF: u64 = 9;
pub const FBSD_ECHILD: u64 = 10;
pub const FBSD_ENOMEM: u64 = 12;
pub const FBSD_EACCES: u64 = 13;
pub const FBSD_EFAULT: u64 = 14;
pub const FBSD_EEXIST: u64 = 17;
pub const FBSD_ENOTDIR: u64 = 20;
pub const FBSD_EISDIR: u64 = 21;
pub const FBSD_EINVAL: u64 = 22;
pub const FBSD_EMFILE: u64 = 24;
pub const FBSD_EROFS: u64 = 30;
pub const FBSD_EPIPE: u64 = 32;
pub const FBSD_EAGAIN: u64 = 35;
pub const FBSD_ENOTCONN: u64 = 57;
pub const FBSD_ETIMEDOUT: u64 = 60;
pub const FBSD_ECONNREFUSED: u64 = 61;
pub const FBSD_ENOSYS: u64 = 78;

// FreeBSD open(2) flags
const FBSD_O_ACCMODE: u32 = 0x0003;
const FBSD_O_RDONLY: u32 = 0x0000;
const FBSD_O_WRONLY: u32 = 0x0001;
const FBSD_O_RDWR: u32 = 0x0002;
const FBSD_O_APPEND: u32 = 0x0008;
const FBSD_O_CREAT: u32 = 0x0200;
const FBSD_O_TRUNC: u32 = 0x0400;
const FBSD_O_DIRECTORY: u32 = 0x0002_0000;

// sysarch(2) AMD64 subcommands
const AMD64_GET_FSBASE: u64 = 128;
const AMD64_SET_FSBASE: u64 = 129;
const AMD64_GET_GSBASE: u64 = 130;
const AMD64_SET_GSBASE: u64 = 131;

// _umtx_op constants
pub const UMTX_OP_WAIT: u32 = 2;
pub const UMTX_OP_WAKE: u32 = 3;
pub const UMTX_OP_WAIT_UINT: u32 = 11;
pub const UMTX_OP_WAIT_UINT_PRIVATE: u32 = 15;
pub const UMTX_OP_WAKE_PRIVATE: u32 = 16;

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct TimeVal {
    pub tv_sec: i64,
    pub tv_usec: i64,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct TimeSpec {
    pub tv_sec: i64,
    pub tv_nsec: i64,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ThrParam {
    pub start_func: u64,
    pub arg: u64,
    pub stack_base: u64,
    pub stack_size: u64,
    pub tls_base: u64,
    pub tls_size: u64,
    pub child_tid: u64,
    pub parent_tid: u64,
    pub flags: i32,
    pub _pad: u32,
    pub rtp: u64,
}

#[inline]
pub fn linux_errno_to_freebsd(neg_err: i64) -> u64 {
    match -neg_err {
        1 => FBSD_EPERM,
        2 => FBSD_ENOENT,
        3 => FBSD_ESRCH,
        4 => FBSD_EINTR,
        5 => FBSD_EIO,
        9 => FBSD_EBADF,
        10 => FBSD_ECHILD,
        11 => FBSD_EAGAIN,
        12 => FBSD_ENOMEM,
        13 => FBSD_EACCES,
        14 => FBSD_EFAULT,
        17 => FBSD_EEXIST,
        20 => FBSD_ENOTDIR,
        21 => FBSD_EISDIR,
        22 => FBSD_EINVAL,
        24 => FBSD_EMFILE,
        30 => FBSD_EROFS,
        32 => FBSD_EPIPE,
        38 => FBSD_ENOSYS,
        107 => FBSD_ENOTCONN,
        110 => FBSD_ETIMEDOUT,
        111 => FBSD_ECONNREFUSED,
        other if other > 0 => other as u64,
        _ => FBSD_EINVAL,
    }
}

/// Applies ABI syscall return convention:
/// - Amaryllis: returns `raw_ret as u64`, clears `RFLAGS.CF`.
/// - FreeBSD: on `< 0`, sets `RFLAGS.CF = 1` and returns positive FreeBSD `errno`;
///            on `>= 0`, clears `RFLAGS.CF = 0` and returns value.
#[inline]
pub fn apply_abi_syscall_result(abi: ProcessAbi, raw_ret: i64, rflags: &mut u64) -> u64 {
    match abi {
        ProcessAbi::Amaryllis => {
            *rflags &= !1;
            raw_ret as u64
        }
        ProcessAbi::FreeBsd => {
            if raw_ret < 0 {
                *rflags |= 1;
                linux_errno_to_freebsd(raw_ret)
            } else {
                *rflags &= !1;
                raw_ret as u64
            }
        }
    }
}

fn vfs_err_to_errno(err: VfsError) -> i64 {
    match err {
        VfsError::NotFound => -2,
        VfsError::AlreadyExists => -17,
        VfsError::IsADirectory => -21,
        VfsError::NotADirectory => -20,
        VfsError::ReadOnlyFilesystem | VfsError::PermissionDenied => -30,
        VfsError::InvalidPath | VfsError::InvalidName | VfsError::OffsetOutOfRange => -22,
        VfsError::NoSpace => -12,
        _ => -5,
    }
}

fn current_unix_seconds_estimate() -> i64 {
    let dt = rtc::read_datetime();
    let y = (dt.year as i64).saturating_sub(1970);
    let approx_days = y * 365 + (y / 4) + ((dt.month as i64 - 1) * 30) + (dt.day as i64 - 1);
    approx_days * 86400 + (dt.hour as i64) * 3600 + (dt.minute as i64) * 60 + (dt.second as i64)
}

pub fn dispatch_freebsd_syscall(
    frame: &mut SyscallFrame,
    user_pml4: u64,
    console_opt: Option<&mut crate::console::Console<'static>>,
    mapping_opt: Option<&PhysicalMemoryMapping>,
    mut allocator_opt: Option<&mut PhysicalFrameAllocator<'static>>,
) -> i64 {
    let syscall_num = frame.rax;
    let arg1 = frame.rdi;
    let arg2 = frame.rsi;
    let arg3 = frame.rdx;
    let arg4 = frame.r10;
    let arg5 = frame.r8;
    let arg6 = frame.r9;

    match syscall_num {
        SYS_FBSD_EXIT => unsafe {
            sched::finish_current_from_syscall(arg1 as i64);
        },

        SYS_FBSD_FORK => {
            if let (Some(mapping), Some(allocator)) = (mapping_opt, allocator_opt) {
                let mut parent_ctx = build_resume_ctx_from_syscall(frame, 0);
                parent_ctx.rflags &= !1;
                sched::fork_current_process(parent_ctx, mapping, allocator)
            } else {
                -12
            }
        }

        SYS_FBSD_READ => {
            syscall::handle_sys_read(frame, user_pml4, console_opt, mapping_opt, allocator_opt)
        }

        SYS_FBSD_WRITE => {
            if arg1 == 1 && arg3 > 0 && arg3 <= 256 {
                if let Some(m) = mapping_opt {
                    let mut tmp = [0u8; 256];
                    let n = (arg3 as usize).min(256);
                    if copy_from_user_bytes_in(m, None, user_pml4, arg2, &mut tmp[..n]).is_ok() {
                        if let Ok(txt) = core::str::from_utf8(&tmp[..n]) {
                            crate::serial::write_str(txt);
                        }
                    }
                }
            }
            syscall::handle_sys_write(user_pml4, arg1, arg2, arg3, console_opt, mapping_opt, allocator_opt)
        }

        SYS_FBSD_OPEN => {
            let Some(mapping) = mapping_opt else { return -22; };
            let path = match copy_str_from_user_in(mapping, allocator_opt.as_deref_mut(), user_pml4, arg1, 240) {
                Ok(p) => p,
                Err(e) => return e,
            };
            let flags = arg2 as u32;
            let acc = flags & FBSD_O_ACCMODE;
            let readable = acc == FBSD_O_RDONLY || acc == FBSD_O_RDWR;
            let writable = acc == FBSD_O_WRONLY || acc == FBSD_O_RDWR;
            let create = (flags & FBSD_O_CREAT) != 0;
            let trunc = (flags & FBSD_O_TRUNC) != 0;
            let append = (flags & FBSD_O_APPEND) != 0;
            let want_dir = (flags & FBSD_O_DIRECTORY) != 0;

            let vfs_res: Result<FdEntry, i64> = vfs::with_vfs(|s| {
                let abs = s.resolve_path(&path).map_err(vfs_err_to_errno)?;
                match s.stat(&abs) {
                    Ok(st) if st.kind == VfsNodeKind::Directory => {
                        if writable {
                            return Err(-21);
                        }
                        let entries = s.list_dir(&abs).map_err(vfs_err_to_errno)?;
                        Ok(FdEntry::Dir {
                            abs_path: abs,
                            entries,
                            cursor: 0,
                        })
                    }
                    Ok(st) => {
                        if want_dir {
                            return Err(-20);
                        }
                        if writable && trunc {
                            s.write_file(&abs, &[]).map_err(vfs_err_to_errno)?;
                        }
                        let start_off = if append && !trunc { st.size } else { 0 };
                        Ok(FdEntry::File {
                            abs_path: abs,
                            offset: start_off,
                            readable,
                            writable,
                            append,
                        })
                    }
                    Err(VfsError::NotFound) if create && !want_dir => {
                        s.touch_or_truncate(&abs, true, false)
                            .map_err(vfs_err_to_errno)?;
                        Ok(FdEntry::File {
                            abs_path: abs,
                            offset: 0,
                            readable,
                            writable,
                            append,
                        })
                    }
                    Err(e) => Err(vfs_err_to_errno(e)),
                }
            });

            match vfs_res {
                Ok(entry) => match sched::alloc_current_fd(entry) {
                    Some(fd) => fd as i64,
                    None => -24,
                },
                Err(e) => e,
            }
        }

        SYS_FBSD_CLOSE => syscall::handle_sys_close(arg1),

        SYS_FBSD_WAIT4 => {
            let target_pid = (arg1 as i32) as isize;
            let status_ptr = arg2;
            let options = arg3;
            if let (Some(mapping), Some(allocator)) = (mapping_opt, allocator_opt) {
                match sched::sys_waitpid_current(target_pid, status_ptr, options, mapping, allocator) {
                    Ok(ret_val) => ret_val,
                    Err(wait_reason) => {
                        let ctx = build_resume_ctx_from_syscall(frame, 0);
                        unsafe {
                            sched::suspend_current_from_syscall(ctx, UserTaskState::Waiting(wait_reason));
                        }
                    }
                }
            } else {
                -22
            }
        }

        SYS_FBSD_UNLINK => {
            let Some(mapping) = mapping_opt else { return -22; };
            match copy_str_from_user_in(mapping, allocator_opt.as_deref_mut(), user_pml4, arg1, 240) {
                Ok(path) => match vfs::with_vfs(|s| s.remove(&path)) {
                    Ok(()) => 0,
                    Err(e) => vfs_err_to_errno(e),
                },
                Err(e) => e,
            }
        }

        SYS_FBSD_MKDIR => {
            let Some(mapping) = mapping_opt else { return -22; };
            match copy_str_from_user_in(mapping, allocator_opt.as_deref_mut(), user_pml4, arg1, 240) {
                Ok(path) => match vfs::with_vfs(|s| s.create_dir(&path)) {
                    Ok(()) => 0,
                    Err(e) => vfs_err_to_errno(e),
                },
                Err(e) => e,
            }
        }

        SYS_FBSD_GETPID => sched::current_tgid() as i64,
        SYS_FBSD_GETPPID => sched::current_parent_pid() as i64,
        SYS_FBSD_GETUID | SYS_FBSD_GETEUID | SYS_FBSD_GETGID | SYS_FBSD_GETEGID
        | SYS_FBSD_ISSETUGID => 0,

        SYS_FBSD_KILL => sched::sys_kill_current((arg1 as i32) as isize, arg2 as u32),

        SYS_FBSD_PIPE => {
            let pipe_id = match ipc::create_pipe() {
                Ok(id) => id,
                Err(e) => return e,
            };
            let Some(rfd) = sched::alloc_current_fd(FdEntry::PipeRead { pipe_id }) else {
                ipc::close_pipe_end(pipe_id, false);
                ipc::close_pipe_end(pipe_id, true);
                return -24;
            };
            let Some(wfd) = sched::alloc_current_fd(FdEntry::PipeWrite { pipe_id }) else {
                let _ = sched::take_current_fd(rfd);
                ipc::close_pipe_end(pipe_id, false);
                ipc::close_pipe_end(pipe_id, true);
                return -24;
            };
            if arg1 != 0 {
                if let Some(mapping) = mapping_opt {
                    let mut raw = [0u8; 8];
                    raw[0..4].copy_from_slice(&(rfd as i32).to_le_bytes());
                    raw[4..8].copy_from_slice(&(wfd as i32).to_le_bytes());
                    let _ = copy_to_user_bytes_in(mapping, allocator_opt.as_deref_mut(), user_pml4, arg1, &raw);
                }
            }
            frame.rdx = wfd as u64;
            rfd as i64
        }

        SYS_FBSD_PIPE2 => {
            let Some(mapping) = mapping_opt else { return -22; };
            if validate_user_slice_in(mapping, allocator_opt.as_deref_mut(), user_pml4, arg1, 8, true).is_err() {
                return -14;
            }
            let pipe_id = match ipc::create_pipe() {
                Ok(id) => id,
                Err(e) => return e,
            };
            let Some(rfd) = sched::alloc_current_fd(FdEntry::PipeRead { pipe_id }) else {
                ipc::close_pipe_end(pipe_id, false);
                ipc::close_pipe_end(pipe_id, true);
                return -24;
            };
            let Some(wfd) = sched::alloc_current_fd(FdEntry::PipeWrite { pipe_id }) else {
                let _ = sched::take_current_fd(rfd);
                ipc::close_pipe_end(pipe_id, false);
                ipc::close_pipe_end(pipe_id, true);
                return -24;
            };
            let mut raw = [0u8; 8];
            raw[0..4].copy_from_slice(&(rfd as i32).to_le_bytes());
            raw[4..8].copy_from_slice(&(wfd as i32).to_le_bytes());
            if copy_to_user_bytes_in(mapping, allocator_opt.as_deref_mut(), user_pml4, arg1, &raw).is_err() {
                let _ = sched::take_current_fd(rfd);
                let _ = sched::take_current_fd(wfd);
                ipc::close_pipe_end(pipe_id, false);
                ipc::close_pipe_end(pipe_id, true);
                return -14;
            }
            0
        }

        SYS_FBSD_MUNMAP => {
            if let (Some(mapping), Some(allocator)) = (mapping_opt, allocator_opt) {
                sched::sys_munmap_current(arg1, arg2 as usize, mapping, allocator)
            } else {
                -22
            }
        }

        SYS_FBSD_MMAP => {
            if let (Some(mapping), Some(allocator)) = (mapping_opt, allocator_opt) {
                sched::sys_mmap_current(arg2 as usize, arg3, mapping, allocator)
            } else {
                -12
            }
        }

        SYS_FBSD_LSEEK => {
            let fd = arg1 as usize;
            let offset = arg2 as i64;
            let whence = arg3;
            match sched::with_current_fd_mut(fd, |entry| match entry {
                FdEntry::File { abs_path, offset: cur, .. } => {
                    let size = vfs::with_vfs(|s| s.stat(abs_path).map(|st| st.size).unwrap_or(0));
                    let base: i64 = match whence {
                        0 => 0,
                        1 => *cur as i64,
                        2 => size as i64,
                        _ => return Err(-22),
                    };
                    let next = base.checked_add(offset).ok_or(-22)?;
                    if next < 0 || next > u32::MAX as i64 {
                        return Err(-22);
                    }
                    *cur = next as u32;
                    Ok(next)
                }
                _ => Err(-22),
            }) {
                Some(Ok(pos)) => pos,
                Some(Err(e)) => e,
                None => -9,
            }
        }

        SYS_FBSD_STAT => {
            let Some(mapping) = mapping_opt else { return -22; };
            let path = match copy_str_from_user_in(mapping, allocator_opt.as_deref_mut(), user_pml4, arg1, 240) {
                Ok(p) => p,
                Err(e) => return e,
            };
            match vfs::with_vfs(|s| s.stat(&path)) {
                Ok(st) => {
                    let ustat = UserStat {
                        size: st.size as u64,
                        kind: if st.kind == VfsNodeKind::Directory { 2 } else { 1 },
                        attributes: st.attributes as u32,
                        flags: 0,
                        reserved: 0,
                    };
                    let bytes = unsafe {
                        core::slice::from_raw_parts(
                            (&ustat as *const UserStat) as *const u8,
                            core::mem::size_of::<UserStat>(),
                        )
                    };
                    if copy_to_user_bytes_in(mapping, allocator_opt.as_deref_mut(), user_pml4, arg2, bytes).is_err() {
                        -14
                    } else {
                        0
                    }
                }
                Err(e) => vfs_err_to_errno(e),
            }
        }

        SYS_FBSD_FSTAT => {
            let Some(mapping) = mapping_opt else { return -22; };
            let fd = arg1 as usize;
            let info = sched::with_current_fd_mut(fd, |entry| match entry {
                FdEntry::File { abs_path, .. } => Ok(abs_path.clone()),
                FdEntry::Dir { abs_path, .. } => Ok(abs_path.clone()),
                _ => Err(-22),
            });
            match info {
                Some(Ok(path)) => match vfs::with_vfs(|s| s.stat(&path)) {
                    Ok(st) => {
                        let ustat = UserStat {
                            size: st.size as u64,
                            kind: if st.kind == VfsNodeKind::Directory { 2 } else { 1 },
                            attributes: st.attributes as u32,
                            flags: 0,
                            reserved: 0,
                        };
                        let bytes = unsafe {
                            core::slice::from_raw_parts(
                                (&ustat as *const UserStat) as *const u8,
                                core::mem::size_of::<UserStat>(),
                            )
                        };
                        if copy_to_user_bytes_in(mapping, allocator_opt.as_deref_mut(), user_pml4, arg2, bytes).is_err() {
                            -14
                        } else {
                            0
                        }
                    }
                    Err(e) => vfs_err_to_errno(e),
                },
                Some(Err(e)) => e,
                None => -9,
            }
        }

        SYS_FBSD_SYSCTL => {
            if let (Some(mapping), Some(allocator)) = (mapping_opt, allocator_opt) {
                sysctl::handle_sysctl(
                    user_pml4,
                    arg1,
                    arg2 as u32,
                    arg3,
                    arg4,
                    arg5,
                    arg6 as usize,
                    mapping,
                    allocator,
                )
            } else {
                -22
            }
        }

        SYS_FBSD_SYSARCH => {
            let cmd = arg1;
            let addr = arg2;
            match cmd {
                AMD64_SET_FSBASE => {
                    sched::set_current_fs_base(addr);
                    unsafe {
                        x86_64::registers::model_specific::Msr::new(0xC000_0100).write(addr);
                    }
                    0
                }
                AMD64_GET_FSBASE => {
                    let Some(mapping) = mapping_opt else { return -22; };
                    let fs = sched::get_current_fs_base();
                    if copy_to_user_bytes_in(
                        mapping,
                        allocator_opt.as_deref_mut(),
                        user_pml4,
                        addr,
                        &fs.to_le_bytes(),
                    )
                    .is_err()
                    {
                        -14
                    } else {
                        0
                    }
                }
                AMD64_SET_GSBASE => {
                    sched::set_current_gs_base(addr);
                    0
                }
                AMD64_GET_GSBASE => {
                    let Some(mapping) = mapping_opt else { return -22; };
                    let gs = sched::get_current_gs_base();
                    if copy_to_user_bytes_in(
                        mapping,
                        allocator_opt.as_deref_mut(),
                        user_pml4,
                        addr,
                        &gs.to_le_bytes(),
                    )
                    .is_err()
                    {
                        -14
                    } else {
                        0
                    }
                }
                _ => -22,
            }
        }

        SYS_FBSD_GETTIMEOFDAY => {
            let Some(mapping) = mapping_opt else { return -22; };
            if arg1 != 0 {
                let up_ms = timer::uptime_ms();
                let tv = TimeVal {
                    tv_sec: current_unix_seconds_estimate(),
                    tv_usec: ((up_ms % 1000) * 1000) as i64,
                };
                let bytes = unsafe {
                    core::slice::from_raw_parts(
                        (&tv as *const TimeVal) as *const u8,
                        core::mem::size_of::<TimeVal>(),
                    )
                };
                if copy_to_user_bytes_in(mapping, allocator_opt.as_deref_mut(), user_pml4, arg1, bytes).is_err() {
                    return -14;
                }
            }
            0
        }

        SYS_FBSD_CLOCK_GETTIME => {
            let Some(mapping) = mapping_opt else { return -22; };
            if arg2 == 0 {
                return -22;
            }
            let up_ms = timer::uptime_ms();
            let ts = if arg1 == 0 {
                TimeSpec {
                    tv_sec: current_unix_seconds_estimate(),
                    tv_nsec: ((up_ms % 1000) * 1_000_000) as i64,
                }
            } else {
                TimeSpec {
                    tv_sec: (up_ms / 1000) as i64,
                    tv_nsec: ((up_ms % 1000) * 1_000_000) as i64,
                }
            };
            let bytes = unsafe {
                core::slice::from_raw_parts(
                    (&ts as *const TimeSpec) as *const u8,
                    core::mem::size_of::<TimeSpec>(),
                )
            };
            if copy_to_user_bytes_in(mapping, allocator_opt.as_deref_mut(), user_pml4, arg2, bytes).is_err() {
                -14
            } else {
                0
            }
        }

        SYS_FBSD_NANOSLEEP => {
            let Some(mapping) = mapping_opt else { return -22; };
            let mut raw = [0u8; core::mem::size_of::<TimeSpec>()];
            if copy_from_user_bytes_in(mapping, allocator_opt.as_deref_mut(), user_pml4, arg1, &mut raw).is_err() {
                return -14;
            }
            let ts: TimeSpec = unsafe { core::ptr::read_unaligned(raw.as_ptr() as *const TimeSpec) };
            if ts.tv_sec < 0 || ts.tv_nsec < 0 || ts.tv_nsec >= 1_000_000_000 {
                return -22;
            }
            let total_ms = ((ts.tv_sec as u64).saturating_mul(1000))
                .saturating_add((ts.tv_nsec as u64) / 1_000_000)
                .min(30_000);
            if total_ms > 0 {
                let mut ctx = build_resume_ctx_from_syscall(frame, 0);
                ctx.rflags &= !1;
                let wake_ms = timer::uptime_ms().saturating_add(total_ms);
                unsafe {
                    sched::suspend_current_from_syscall(
                        ctx,
                        UserTaskState::Waiting(WaitReason::Sleep { wake_ms }),
                    );
                }
            }
            0
        }

        SYS_FBSD_KQUEUE => match kqueue::create_kqueue() {
            Ok(kq_id) => match sched::alloc_current_fd(FdEntry::Kqueue { kq_id }) {
                Some(fd) => fd as i64,
                None => {
                    kqueue::destroy_kqueue(kq_id);
                    -24
                }
            },
            Err(e) => e,
        },

        SYS_FBSD_KEVENT => {
            if let (Some(mapping), Some(allocator)) = (mapping_opt, allocator_opt) {
                match sched::sys_kevent_current(
                    arg1 as usize,
                    arg2,
                    arg3 as usize,
                    arg4,
                    arg5 as usize,
                    arg6,
                    mapping,
                    allocator,
                ) {
                    Ok(n) => n,
                    Err(wait_reason) => {
                        let ctx = build_resume_ctx_from_syscall(frame, 0);
                        unsafe {
                            sched::suspend_current_from_syscall(ctx, UserTaskState::Waiting(wait_reason));
                        }
                    }
                }
            } else {
                -22
            }
        }

        SYS_FBSD_SIGPROCMASK => {
            if let (Some(mapping), Some(allocator)) = (mapping_opt, allocator_opt) {
                sched::sys_sigprocmask_current(arg1 as i32, arg2, arg3, mapping, allocator)
            } else {
                -22
            }
        }

        SYS_FBSD_SIGACTION => {
            if let (Some(mapping), Some(allocator)) = (mapping_opt, allocator_opt) {
                sched::sys_sigaction_current(arg1 as u32, arg2, arg3, mapping, allocator)
            } else {
                -22
            }
        }

        SYS_FBSD_SIGRETURN => {
            if let (Some(mapping), Some(allocator)) = (mapping_opt, allocator_opt) {
                sched::sys_sigreturn_current(frame, arg1, mapping, allocator)
            } else {
                -22
            }
        }

        SYS_FBSD_SIGALTSTACK | SYS_FBSD_FCNTL | SYS_FBSD_IOCTL => 0,

        SYS_FBSD_THR_SELF => {
            let Some(mapping) = mapping_opt else { return -22; };
            let tid = sched::current_pid() as i64;
            if copy_to_user_bytes_in(
                mapping,
                allocator_opt.as_deref_mut(),
                user_pml4,
                arg1,
                &tid.to_le_bytes(),
            )
            .is_err()
            {
                -14
            } else {
                0
            }
        }

        SYS_FBSD_THR_EXIT => {
            if let (Some(mapping), Some(allocator)) = (mapping_opt, allocator_opt) {
                let _ = sched::clear_current_tid_on_exit(arg1, mapping, allocator);
            }
            unsafe {
                sched::finish_current_from_syscall(0);
            }
        }

        SYS_FBSD_THR_KILL => sched::sys_thr_kill_current(arg1 as i64, arg2 as u32),

        SYS_FBSD_THR_NEW => {
            if let (Some(mapping), Some(allocator)) = (mapping_opt, allocator_opt) {
                sched::sys_thr_new_current(arg1, arg2 as usize, mapping, allocator)
            } else {
                -22
            }
        }

        SYS_FBSD_UMTX_OP => {
            if let (Some(mapping), Some(allocator)) = (mapping_opt, allocator_opt) {
                match sched::sys_umtx_op_current(arg1, arg2 as u32, arg3, arg4, arg5, mapping, allocator) {
                    Ok(v) => v,
                    Err(wait_reason) => {
                        let ctx = build_resume_ctx_from_syscall(frame, 0);
                        unsafe {
                            sched::suspend_current_from_syscall(ctx, UserTaskState::Waiting(wait_reason));
                        }
                    }
                }
            } else {
                -22
            }
        }

        SYS_FBSD_MACH_PORT_ALLOCATE => sched::sys_mach_port_allocate_current(),
        SYS_FBSD_MACH_PORT_DEALLOCATE => {
            if let Some(allocator) = allocator_opt {
                sched::sys_mach_port_deallocate_current(arg1 as u32, allocator)
            } else {
                -22
            }
        }
        SYS_FBSD_MACH_MSG => {
            if let (Some(mapping), Some(allocator)) = (mapping_opt, allocator_opt) {
                match sched::sys_mach_msg_current(
                    arg1,
                    arg2 as u32,
                    arg3 as usize,
                    arg4 as usize,
                    arg5 as u32,
                    arg6,
                    mapping,
                    allocator,
                ) {
                    Ok(v) => v,
                    Err(wait_reason) => {
                        let ctx = build_resume_ctx_from_syscall(frame, 0);
                        unsafe {
                            sched::suspend_current_from_syscall(ctx, UserTaskState::Waiting(wait_reason));
                        }
                    }
                }
            } else {
                -22
            }
        }
        SYS_FBSD_BOOTSTRAP_REGISTER => {
            if let (Some(mapping), Some(allocator)) = (mapping_opt, allocator_opt) {
                let len = (arg2 as usize).min(64);
                let mut nbuf = [0u8; 64];
                if len == 0 {
                    -22
                } else if copy_from_user_bytes_in(mapping, Some(allocator), user_pml4, arg1, &mut nbuf[..len]).is_err() {
                    -14
                } else if let Ok(name) = core::str::from_utf8(&nbuf[..len]) {
                    sched::sys_bootstrap_register_current(name, arg3 as u32, allocator)
                } else {
                    -22
                }
            } else {
                -22
            }
        }
        SYS_FBSD_BOOTSTRAP_LOOKUP => {
            if let (Some(mapping), Some(allocator)) = (mapping_opt, allocator_opt) {
                let len = (arg2 as usize).min(64);
                let mut nbuf = [0u8; 64];
                if len == 0 {
                    -22
                } else if copy_from_user_bytes_in(mapping, Some(allocator), user_pml4, arg1, &mut nbuf[..len]).is_err() {
                    -14
                } else if let Ok(name) = core::str::from_utf8(&nbuf[..len]) {
                    sched::sys_bootstrap_lookup_current(name, allocator)
                } else {
                    -22
                }
            } else {
                -22
            }
        }

        _ => {
            let _ = (asm_nop_hint(), String::new(), vec![0u8; 0]);
            -38 // ENOSYS -> FBSD_ENOSYS (78) with CF=1
        }
    }
}

#[inline]
fn asm_nop_hint() {
    let _ = || unsafe { asm!("nop", options(nomem, nostack)) };
}
