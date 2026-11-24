//! Standard I/O streams (`stdin`, `stdout`, `stderr`) and formatted `print!`/`println!` support.

use core::fmt;

use crate::syscall::{syscall3, SYS_READ, SYS_WRITE};

pub const STDIN: u64 = 0;
pub const STDOUT: u64 = 1;
pub const STDERR: u64 = 2;

pub fn write_fd(fd: u64, bytes: &[u8]) -> isize {
    if bytes.is_empty() {
        return 0;
    }
    unsafe { syscall3(SYS_WRITE, fd, bytes.as_ptr() as u64, bytes.len() as u64) as isize }
}

pub fn read_fd(fd: u64, buf: &mut [u8]) -> isize {
    if buf.is_empty() {
        return 0;
    }
    unsafe { syscall3(SYS_READ, fd, buf.as_mut_ptr() as u64, buf.len() as u64) as isize }
}

#[inline]
pub fn print_str(s: &str) {
    let _ = write_fd(STDOUT, s.as_bytes());
}

#[inline]
pub fn eprint_str(s: &str) {
    let _ = write_fd(STDERR, s.as_bytes());
}

#[inline]
pub fn read_stdin(buf: &mut [u8]) -> isize {
    read_fd(STDIN, buf)
}

/// Buffered stack writer that minimizes per-chunk `SYS_WRITE` traps during formatting.
pub struct BufferedFdWriter {
    fd: u64,
    buf: [u8; 512],
    pos: usize,
}

impl BufferedFdWriter {
    pub const fn new(fd: u64) -> Self {
        Self {
            fd,
            buf: [0; 512],
            pos: 0,
        }
    }

    pub fn flush(&mut self) {
        if self.pos > 0 {
            let _ = write_fd(self.fd, &self.buf[..self.pos]);
            self.pos = 0;
        }
    }
}

impl fmt::Write for BufferedFdWriter {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for &b in s.as_bytes() {
            if self.pos == self.buf.len() {
                self.flush();
            }
            self.buf[self.pos] = b;
            self.pos += 1;
        }
        Ok(())
    }
}

impl Drop for BufferedFdWriter {
    fn drop(&mut self) {
        self.flush();
    }
}

#[doc(hidden)]
pub fn _print(args: fmt::Arguments<'_>) {
    let mut w = BufferedFdWriter::new(STDOUT);
    let _ = fmt::write(&mut w, args);
    w.flush();
}

#[doc(hidden)]
pub fn _eprint(args: fmt::Arguments<'_>) {
    let mut w = BufferedFdWriter::new(STDERR);
    let _ = fmt::write(&mut w, args);
    w.flush();
}
