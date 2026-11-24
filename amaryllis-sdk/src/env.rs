//! Command-line argument inspection (`argc` / `argv`) passed via SysV x86_64 stack/registers.

use alloc::string::String;
use alloc::vec::Vec;
use core::ptr::null;
use core::sync::atomic::{AtomicPtr, AtomicUsize, Ordering};

static ARGC: AtomicUsize = AtomicUsize::new(0);
static ARGV: AtomicPtr<*const u8> = AtomicPtr::new(null::<*const u8>() as *mut *const u8);

/// Called internally by the `entry!` startup trampoline before invoking `main()`.
#[doc(hidden)]
pub unsafe fn __init_args(argc: usize, argv: *const *const u8) {
    ARGC.store(argc.min(64), Ordering::Release);
    ARGV.store(argv as *mut *const u8, Ordering::Release);
}

pub struct Args {
    idx: usize,
    argc: usize,
    argv: *const *const u8,
}

impl Iterator for Args {
    type Item = &'static str;

    fn next(&mut self) -> Option<Self::Item> {
        if self.idx >= self.argc || self.argv.is_null() {
            return None;
        }
        let ptr = unsafe { *self.argv.add(self.idx) };
        self.idx += 1;
        if ptr.is_null() {
            return None;
        }
        let mut len = 0usize;
        while len < 1024 {
            let b = unsafe { *ptr.add(len) };
            if b == 0 {
                break;
            }
            len += 1;
        }
        let bytes = unsafe { core::slice::from_raw_parts(ptr, len) };
        Some(core::str::from_utf8(bytes).unwrap_or(""))
    }
}

/// Returns an iterator over command-line arguments (`argv[0..argc]`) as `&'static str`.
pub fn args() -> Args {
    Args {
        idx: 0,
        argc: ARGC.load(Ordering::Acquire),
        argv: ARGV.load(Ordering::Acquire) as *const *const u8,
    }
}

/// Collects all command-line arguments into an owned `Vec<String>`.
pub fn args_vec() -> Vec<String> {
    args().map(String::from).collect()
}
