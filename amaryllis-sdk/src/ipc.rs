//! Inter-Process Communication (POSIX Pipes & Zero-Copy Shared Memory) for AmaryllisOS Ring 3.

use crate::syscall::{
    syscall1, syscall2, syscall3, SYS_CLOSE, SYS_PIPE, SYS_READ, SYS_SHM_CREATE, SYS_SHM_MAP,
    SYS_SHM_UNMAP, SYS_WRITE,
};

pub struct PipeReader {
    fd: i32,
}

pub struct PipeWriter {
    fd: i32,
}

impl PipeReader {
    #[inline]
    pub const fn raw_fd(&self) -> i32 {
        self.fd
    }

    pub fn read(&self, buf: &mut [u8]) -> Result<usize, i64> {
        let res = unsafe {
            syscall3(
                SYS_READ,
                self.fd as u64,
                buf.as_mut_ptr() as u64,
                buf.len() as u64,
            )
        };
        if res >= 0 {
            Ok(res as usize)
        } else {
            Err(res)
        }
    }
}

impl PipeWriter {
    #[inline]
    pub const fn raw_fd(&self) -> i32 {
        self.fd
    }

    pub fn write(&self, data: &[u8]) -> Result<usize, i64> {
        let res = unsafe {
            syscall3(
                SYS_WRITE,
                self.fd as u64,
                data.as_ptr() as u64,
                data.len() as u64,
            )
        };
        if res >= 0 {
            Ok(res as usize)
        } else {
            Err(res)
        }
    }
}

impl Drop for PipeReader {
    fn drop(&mut self) {
        if self.fd >= 0 {
            let _ = unsafe { syscall1(SYS_CLOSE, self.fd as u64) };
        }
    }
}

impl Drop for PipeWriter {
    fn drop(&mut self) {
        if self.fd >= 0 {
            let _ = unsafe { syscall1(SYS_CLOSE, self.fd as u64) };
        }
    }
}

/// Creates a kernel-buffered POSIX pipe `(PipeReader, PipeWriter)`.
pub fn pipe() -> Result<(PipeReader, PipeWriter), i64> {
    let mut fds = [-1i32; 2];
    let res = unsafe { syscall1(SYS_PIPE, fds.as_mut_ptr() as u64) };
    if res < 0 {
        return Err(res);
    }
    Ok((PipeReader { fd: fds[0] }, PipeWriter { fd: fds[1] }))
}

/// A mapped Shared Memory (`SHM`) region backed by shared physical frames in the kernel.
pub struct SharedMem {
    id: u64,
    vaddr: u64,
    size_bytes: usize,
    writable: bool,
}

impl SharedMem {
    /// Creates or opens a named shared memory region and maps it into the caller's address space.
    pub fn create_or_open(name: &str, size_bytes: usize, writable: bool) -> Result<Self, i64> {
        let id_res = unsafe {
            syscall3(
                SYS_SHM_CREATE,
                name.as_ptr() as u64,
                name.len() as u64,
                size_bytes as u64,
            )
        };
        if id_res < 0 {
            return Err(id_res);
        }
        let id = id_res as u64;
        let vaddr_res = unsafe { syscall2(SYS_SHM_MAP, id, if writable { 1 } else { 0 }) };
        if vaddr_res < 0 {
            return Err(vaddr_res);
        }
        let aligned = ((size_bytes.max(1) + 4095) / 4096) * 4096;
        Ok(Self {
            id,
            vaddr: vaddr_res as u64,
            size_bytes: aligned,
            writable,
        })
    }

    /// Maps an existing SHM object by numeric ID.
    pub fn map_id(id: u64, size_bytes: usize, writable: bool) -> Result<Self, i64> {
        let vaddr_res = unsafe { syscall2(SYS_SHM_MAP, id, if writable { 1 } else { 0 }) };
        if vaddr_res < 0 {
            return Err(vaddr_res);
        }
        let aligned = ((size_bytes.max(1) + 4095) / 4096) * 4096;
        Ok(Self {
            id,
            vaddr: vaddr_res as u64,
            size_bytes: aligned,
            writable,
        })
    }

    #[inline]
    pub const fn id(&self) -> u64 {
        self.id
    }

    #[inline]
    pub const fn size(&self) -> usize {
        self.size_bytes
    }

    pub fn as_bytes(&self) -> &[u8] {
        unsafe { core::slice::from_raw_parts(self.vaddr as *const u8, self.size_bytes) }
    }

    pub fn as_bytes_mut(&mut self) -> Option<&mut [u8]> {
        if !self.writable {
            return None;
        }
        Some(unsafe { core::slice::from_raw_parts_mut(self.vaddr as *mut u8, self.size_bytes) })
    }
}

impl Drop for SharedMem {
    fn drop(&mut self) {
        if self.vaddr != 0 {
            let _ = unsafe { syscall1(SYS_SHM_UNMAP, self.vaddr) };
            self.vaddr = 0;
        }
    }
}
