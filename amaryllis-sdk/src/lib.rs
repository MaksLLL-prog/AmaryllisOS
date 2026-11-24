#![no_std]
//! **AmaryllisOS Ring 3 Userland SDK (`amaryllis-sdk`)**
//!
//! Write standalone `#![no_std]` Rust applications with full `alloc` (`Vec`, `String`, `Box`,
//! `BTreeMap`), `fs` VFS file/directory I/O, `env::args()`, graphics windows, and TCP/UDP/HTTP
//! compiled to static x86_64 `ET_EXEC` ELF64 executables running preemptively in Ring 3.

pub extern crate alloc;

pub mod alloc_heap;
pub mod env;
pub mod fs;
pub mod gui;
pub mod io;
pub mod ipc;
pub mod net;
pub mod proc;
pub mod syscall;
pub mod time;

pub use alloc::boxed::Box;
pub use alloc::collections::BTreeMap;
pub use alloc::format;
pub use alloc::string::{String, ToString};
pub use alloc::vec;
pub use alloc::vec::Vec;

#[global_allocator]
static AMARYLLIS_HEAP: alloc_heap::AmaryllisAllocator = alloc_heap::AmaryllisAllocator::new();

/// Returns `(brk_mapped_bytes, mmap_mapped_bytes, free_list_bytes)` for the Ring 3 heap.
pub fn heap_stats() -> (usize, usize, usize) {
    alloc_heap::heap_stats(&AMARYLLIS_HEAP)
}

#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => {
        $crate::io::_print(core::format_args!($($arg)*))
    };
}

#[macro_export]
macro_rules! println {
    () => {
        $crate::io::print_str("\n")
    };
    ($($arg:tt)*) => {
        $crate::io::_print(core::format_args!("{}\n", core::format_args!($($arg)*)))
    };
}

#[macro_export]
macro_rules! eprintln {
    () => {
        $crate::io::eprint_str("\n")
    };
    ($($arg:tt)*) => {
        $crate::io::_eprint(core::format_args!("{}\n", core::format_args!($($arg)*)))
    };
}

/// Defines the `_start` ELF entry point (initializing SysV `argc`/`argv`) and minimal `#[panic_handler]`.
///
/// Expects a user function `fn main() -> i32`.
#[macro_export]
macro_rules! entry {
    ($main_fn:path) => {
        #[no_mangle]
        #[link_section = ".text._start"]
        pub extern "C" fn _start(argc: usize, argv: *const *const u8) -> ! {
            unsafe {
                $crate::env::__init_args(argc, argv);
            }
            let main: fn() -> i32 = $main_fn;
            let code = main();
            $crate::proc::exit(code);
        }

        #[panic_handler]
        fn panic(info: &core::panic::PanicInfo<'_>) -> ! {
            $crate::eprintln!("[PANIC PID={}] {}", $crate::proc::getpid(), info);
            $crate::proc::exit(101);
        }
    };
}
