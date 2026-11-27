use crate::interrupts::timer;
use crate::memory::vmm::PhysicalMemoryMapping;
use crate::memory::PhysicalFrameAllocator;

pub const ELFOSABI_NONE: u8 = 0;
pub const ELFOSABI_LINUX: u8 = 3;
pub const ELFOSABI_FREEBSD: u8 = 9;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ProcessAbi {
    Amaryllis,
    FreeBsd,
}

impl ProcessAbi {
    #[inline]
    pub const fn from_e_ident(osabi: u8) -> Self {
        match osabi {
            ELFOSABI_FREEBSD => Self::FreeBsd,
            _ => Self::Amaryllis,
        }
    }
}

pub const FBSD_SYS_EXIT: u64 = 1;
pub const FBSD_SYS_FORK: u64 = 2;
pub const FBSD_SYS_READ: u64 = 3;
pub const FBSD_SYS_WRITE: u64 = 4;
pub const FBSD_SYS_OPEN: u64 = 5;
pub const FBSD_SYS_CLOSE: u64 = 6;
pub const FBSD_SYS_WAIT4: u64 = 7;
pub const FBSD_SYS_GETPID: u64 = 20;
pub const FBSD_SYS_GETUID: u64 = 24;
pub const FBSD_SYS_GETEUID: u64 = 25;
pub const FBSD_SYS_GETEGID: u64 = 43;
pub const FBSD_SYS_GETGID: u64 = 47;
pub const FBSD_SYS_IOCTL: u64 = 54;
pub const FBSD_SYS_MUNMAP: u64 = 73;
pub const FBSD_SYS_MPROTECT: u64 = 74;
pub const FBSD_SYS_FCNTL: u64 = 92;
pub const FBSD_SYS_GETTIMEOFDAY: u64 = 116;
pub const FBSD_SYS_SYSCTL: u64 = 202;
pub const FBSD_SYS_CLOCK_GETTIME: u64 = 232;
pub const FBSD_SYS_ISSETUGID: u64 = 253;
pub const FBSD_SYS_SIGPROCMASK: u64 = 340;
pub const FBSD_SYS_SIGACTION: u64 = 416;
pub const FBSD_SYS_MMAP: u64 = 477;
pub const FBSD_SYS_LSEEK: u64 = 478;
pub const FBSD_SYS_FSTAT: u64 = 551;
pub const FBSD_SYS_GETDENTS: u64 = 554;

#[inline]
pub const fn map_to_native_syscall(fbsd_num: u64) -> Option<u64> {
    use super::syscall::*;
    match fbsd_num {
        FBSD_SYS_EXIT => Some(SYS_EXIT),
        FBSD_SYS_FORK => Some(SYS_FORK),
        FBSD_SYS_READ => Some(SYS_READ),
        FBSD_SYS_WRITE => Some(SYS_WRITE),
        FBSD_SYS_OPEN => Some(SYS_OPEN),
        FBSD_SYS_CLOSE => Some(SYS_CLOSE),
        FBSD_SYS_WAIT4 => Some(SYS_WAITPID),
        FBSD_SYS_GETPID => Some(SYS_GETPID),
        FBSD_SYS_MUNMAP => Some(SYS_MUNMAP),
        FBSD_SYS_MMAP => Some(SYS_MMAP),
        FBSD_SYS_LSEEK => Some(SYS_LSEEK),
        FBSD_SYS_FSTAT => Some(SYS_FSTAT),
        FBSD_SYS_GETDENTS => Some(SYS_GETDENTS),
        _ => None,
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
struct FbsdWinsize { ws_row: u16, ws_col: u16, ws_xpixel: u16, ws_ypixel: u16 }

#[repr(C)]
#[derive(Clone, Copy)]
struct FbsdTimespec { tv_sec: i64, tv_nsec: i64 }

pub fn handle_freebsd_specific_syscall(
    num: u64, arg1: u64, arg2: u64, arg3: u64, arg4: u64,
    user_pml4: u64, mapping: &PhysicalMemoryMapping, allocator: Option<&mut PhysicalFrameAllocator<'_>>,
) -> Option<i64> {
    match num {
        FBSD_SYS_GETUID | FBSD_SYS_GETEUID | FBSD_SYS_GETGID | FBSD_SYS_GETEGID

        | FBSD_SYS_ISSETUGID | FBSD_SYS_MPROTECT | FBSD_SYS_SIGPROCMASK | FBSD_SYS_SIGACTION | FBSD_SYS_FCNTL => Some(0),
        FBSD_SYS_IOCTL => {
            if arg2 == 0x4008_7468 && arg1 <= 2 && arg3 != 0 {
                let ws = FbsdWinsize { ws_row: 36, ws_col: 120, ws_xpixel: 1280, ws_ypixel: 720 };
                let raw = unsafe { core::slice::from_raw_parts((&ws as *const FbsdWinsize) as *const u8, 8) };
                Some(super::syscall::copy_to_user_bytes_in(mapping, allocator, user_pml4, arg3, raw).map(|_| 0).unwrap_or(-14))
            } else { Some(-25) }
        }
        FBSD_SYS_CLOCK_GETTIME => {
            if arg2 == 0 { return Some(-14); }
            let ms = timer::uptime_ms();
            let ts = FbsdTimespec { tv_sec: (ms / 1000) as i64, tv_nsec: ((ms % 1000) * 1_000_000) as i64 };
            let raw = unsafe { core::slice::from_raw_parts((&ts as *const FbsdTimespec) as *const u8, 16) };
            Some(super::syscall::copy_to_user_bytes_in(mapping, allocator, user_pml4, arg2, raw).map(|_| 0).unwrap_or(-14))
        }
        FBSD_SYS_GETTIMEOFDAY => {
            if arg1 == 0 { return Some(0); }
            let ms = timer::uptime_ms();
            let tv = FbsdTimespec { tv_sec: 1_760_000_000 + (ms / 1000) as i64, tv_nsec: ((ms % 1000) * 1000) as i64 };
            let raw = unsafe { core::slice::from_raw_parts((&tv as *const FbsdTimespec) as *const u8, 16) };
            Some(super::syscall::copy_to_user_bytes_in(mapping, allocator, user_pml4, arg1, raw).map(|_| 0).unwrap_or(-14))
        }
        FBSD_SYS_SYSCTL => Some(handle_sysctl(arg1, arg2 as usize, arg3, arg4, user_pml4, mapping, allocator)),
        _ => None,
    }
}

fn handle_sysctl(
    name_ptr: u64, namelen: usize, oldp: u64, oldlenp: u64,
    user_pml4: u64, mapping: &PhysicalMemoryMapping, mut allocator: Option<&mut PhysicalFrameAllocator<'_>>,
) -> i64 {
    if name_ptr == 0 || !(2..=8).contains(&namelen) { return -22; }
    let mut mib_bytes = [0u8; 32];
    if super::syscall::copy_from_user_bytes_in(mapping, allocator.as_deref_mut(), user_pml4, name_ptr, &mut mib_bytes[..namelen * 4]).is_err() { return -14; }
    let m0 = i32::from_le_bytes(mib_bytes[0..4].try_into().unwrap());
    let m1 = i32::from_le_bytes(mib_bytes[4..8].try_into().unwrap());
    let mut ibuf = [0u8; 8];
    let payload: &[u8] = match (m0, m1) {
        (1, 1) => b"FreeBSD\0",
        (1, 2) => b"14.2-ANU\0",
        (1, 4) => b"Amaryllis ANU 0.11.0 (FreeBSD ABI)\0",
        (1, 10) => b"amaryllis\0",
        (6, 1) => b"amd64\0",
        (6, 2) => b"ANU x86_64 SMP\0",
        (6, 3) => { ibuf[..4].copy_from_slice(&4i32.to_le_bytes()); &ibuf[..4] }
        (1, 7) | (6, 7) => { ibuf[..4].copy_from_slice(&4096i32.to_le_bytes()); &ibuf[..4] }
        _ => return -2,
    };
    if oldlenp != 0 {
        let mut lb = [0u8; 8];
        if super::syscall::copy_from_user_bytes_in(mapping, allocator.as_deref_mut(), user_pml4, oldlenp, &mut lb).is_err() { return -14; }
        let cap = usize::from_le_bytes(lb);
        let n = payload.len().min(cap);
        if oldp != 0 && n > 0 && super::syscall::copy_to_user_bytes_in(mapping, allocator.as_deref_mut(), user_pml4, oldp, &payload[..n]).is_err() { return -14; }
        if super::syscall::copy_to_user_bytes_in(mapping, allocator, user_pml4, oldlenp, &payload.len().to_le_bytes()).is_err() { return -14; }
    }
    0
}
