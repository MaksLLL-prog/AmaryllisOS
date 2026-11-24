//! VFS filesystem APIs for Ring 3 applications (`/`, `/tmp`, `/disk` FAT32).

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use crate::syscall::{
    syscall1, syscall2, syscall3, syscall4, EINVAL, SYS_CLOSE, SYS_FSTAT, SYS_GETDENTS, SYS_LSEEK,
    SYS_MKDIR, SYS_OPEN, SYS_READ, SYS_STAT, SYS_UNLINK, SYS_WRITE,
};

pub const O_RDONLY: u32 = 0;
pub const O_WRONLY: u32 = 1;
pub const O_RDWR: u32 = 2;
pub const O_CREAT: u32 = 0x40;
pub const O_TRUNC: u32 = 0x200;
pub const O_APPEND: u32 = 0x400;
pub const O_DIRECTORY: u32 = 0x10000;

pub const SEEK_SET: u64 = 0;
pub const SEEK_CUR: u64 = 1;
pub const SEEK_END: u64 = 2;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct Metadata {
    pub size: u64,
    pub kind: u32,
    pub attributes: u32,
    pub flags: u32,
    pub reserved: u32,
}

impl Metadata {
    #[inline]
    pub fn is_file(&self) -> bool {
        self.kind == 1
    }

    #[inline]
    pub fn is_dir(&self) -> bool {
        self.kind == 2
    }

    #[inline]
    pub fn len(&self) -> u64 {
        self.size
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.size == 0
    }

    #[inline]
    pub fn is_read_only(&self) -> bool {
        (self.flags & 1) != 0
    }
}

#[derive(Clone, Copy, Debug)]
pub enum SeekFrom {
    Start(u64),
    End(i64),
    Current(i64),
}

#[derive(Clone, Copy, Debug)]
pub struct OpenOptions {
    read: bool,
    write: bool,
    create: bool,
    truncate: bool,
    append: bool,
}

impl OpenOptions {
    pub const fn new() -> Self {
        Self {
            read: false,
            write: false,
            create: false,
            truncate: false,
            append: false,
        }
    }

    pub fn read(&mut self, read: bool) -> &mut Self {
        self.read = read;
        self
    }

    pub fn write(&mut self, write: bool) -> &mut Self {
        self.write = write;
        self
    }

    pub fn create(&mut self, create: bool) -> &mut Self {
        self.create = create;
        self
    }

    pub fn truncate(&mut self, truncate: bool) -> &mut Self {
        self.truncate = truncate;
        self
    }

    pub fn append(&mut self, append: bool) -> &mut Self {
        self.append = append;
        self
    }

    pub fn open(&self, path: &str) -> Result<File, i64> {
        if path.is_empty() {
            return Err(EINVAL);
        }
        let mut flags = match (self.read, self.write) {
            (true, false) => O_RDONLY,
            (false, true) => O_WRONLY,
            (true, true) => O_RDWR,
            (false, false) => return Err(EINVAL),
        };
        if self.create {
            flags |= O_CREAT;
        }
        if self.truncate {
            flags |= O_TRUNC;
        }
        if self.append {
            flags |= O_APPEND;
        }
        let fd = unsafe { syscall3(SYS_OPEN, path.as_ptr() as u64, path.len() as u64, flags as u64) };
        if fd < 0 {
            Err(fd)
        } else {
            Ok(File { fd })
        }
    }
}

pub struct File {
    fd: i64,
}

impl File {
    pub fn open(path: &str) -> Result<Self, i64> {
        OpenOptions::new().read(true).open(path)
    }

    pub fn create(path: &str) -> Result<Self, i64> {
        OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)
    }

    pub fn read(&mut self, buf: &mut [u8]) -> Result<usize, i64> {
        if buf.is_empty() {
            return Ok(0);
        }
        let n = unsafe { syscall3(SYS_READ, self.fd as u64, buf.as_mut_ptr() as u64, buf.len() as u64) };
        if n < 0 {
            Err(n)
        } else {
            Ok(n as usize)
        }
    }

    pub fn read_to_end(&mut self, out: &mut Vec<u8>) -> Result<usize, i64> {
        let mut total = 0usize;
        let mut chunk = [0u8; 4096];
        loop {
            let n = self.read(&mut chunk)?;
            if n == 0 {
                break;
            }
            out.extend_from_slice(&chunk[..n]);
            total += n;
        }
        Ok(total)
    }

    pub fn write(&mut self, buf: &[u8]) -> Result<usize, i64> {
        if buf.is_empty() {
            return Ok(0);
        }
        let n = unsafe { syscall3(SYS_WRITE, self.fd as u64, buf.as_ptr() as u64, buf.len() as u64) };
        if n < 0 {
            Err(n)
        } else {
            Ok(n as usize)
        }
    }

    pub fn write_all(&mut self, mut buf: &[u8]) -> Result<(), i64> {
        while !buf.is_empty() {
            let n = self.write(buf)?;
            if n == 0 {
                return Err(crate::syscall::EIO);
            }
            buf = &buf[n..];
        }
        Ok(())
    }

    pub fn seek(&mut self, pos: SeekFrom) -> Result<u64, i64> {
        let (whence, off) = match pos {
            SeekFrom::Start(v) => (SEEK_SET, v as i64),
            SeekFrom::Current(v) => (SEEK_CUR, v),
            SeekFrom::End(v) => (SEEK_END, v),
        };
        let res = unsafe { syscall3(SYS_LSEEK, self.fd as u64, off as u64, whence) };
        if res < 0 {
            Err(res)
        } else {
            Ok(res as u64)
        }
    }

    pub fn metadata(&self) -> Result<Metadata, i64> {
        let mut st = Metadata::default();
        let res = unsafe { syscall2(SYS_FSTAT, self.fd as u64, (&mut st as *mut Metadata) as u64) };
        if res < 0 {
            Err(res)
        } else {
            Ok(st)
        }
    }
}

impl Drop for File {
    fn drop(&mut self) {
        if self.fd >= 3 {
            let _ = unsafe { syscall1(SYS_CLOSE, self.fd as u64) };
        }
    }
}

#[derive(Clone, Debug)]
pub struct DirEntry {
    pub name: String,
    pub metadata: Metadata,
}

pub struct ReadDir {
    fd: i64,
}

impl Iterator for ReadDir {
    type Item = Result<DirEntry, i64>;

    fn next(&mut self) -> Option<Self::Item> {
        let mut name_buf = [0u8; 240];
        let mut st = Metadata::default();
        let n = unsafe {
            syscall4(
                SYS_GETDENTS,
                self.fd as u64,
                name_buf.as_mut_ptr() as u64,
                name_buf.len() as u64,
                (&mut st as *mut Metadata) as u64,
            )
        };
        if n == 0 {
            None
        } else if n < 0 {
            Some(Err(n))
        } else {
            let len = (n as usize).min(name_buf.len());
            let name = String::from(core::str::from_utf8(&name_buf[..len]).unwrap_or("?"));
            Some(Ok(DirEntry { name, metadata: st }))
        }
    }
}

impl Drop for ReadDir {
    fn drop(&mut self) {
        if self.fd >= 3 {
            let _ = unsafe { syscall1(SYS_CLOSE, self.fd as u64) };
        }
    }
}

pub fn metadata(path: &str) -> Result<Metadata, i64> {
    if path.is_empty() {
        return Err(EINVAL);
    }
    let mut st = Metadata::default();
    let res = unsafe {
        syscall3(
            SYS_STAT,
            path.as_ptr() as u64,
            path.len() as u64,
            (&mut st as *mut Metadata) as u64,
        )
    };
    if res < 0 {
        Err(res)
    } else {
        Ok(st)
    }
}

pub fn read(path: &str) -> Result<Vec<u8>, i64> {
    let mut f = File::open(path)?;
    let cap = f.metadata().map(|m| m.size as usize).unwrap_or(0).min(1024 * 1024);
    let mut buf = Vec::with_capacity(cap);
    f.read_to_end(&mut buf)?;
    Ok(buf)
}

pub fn read_to_string(path: &str) -> Result<String, i64> {
    let bytes = read(path)?;
    String::from_utf8(bytes).map_err(|_| EINVAL)
}

pub fn write(path: &str, data: &[u8]) -> Result<(), i64> {
    let mut f = File::create(path)?;
    f.write_all(data)
}

pub fn read_dir(path: &str) -> Result<ReadDir, i64> {
    if path.is_empty() {
        return Err(EINVAL);
    }
    let flags = O_RDONLY | O_DIRECTORY;
    let fd = unsafe { syscall3(SYS_OPEN, path.as_ptr() as u64, path.len() as u64, flags as u64) };
    if fd < 0 {
        Err(fd)
    } else {
        Ok(ReadDir { fd })
    }
}

pub fn create_dir(path: &str) -> Result<(), i64> {
    if path.is_empty() {
        return Err(EINVAL);
    }
    let res = unsafe { syscall2(SYS_MKDIR, path.as_ptr() as u64, path.len() as u64) };
    if res < 0 {
        Err(res)
    } else {
        Ok(())
    }
}

pub fn remove_file(path: &str) -> Result<(), i64> {
    if path.is_empty() {
        return Err(EINVAL);
    }
    let res = unsafe { syscall2(SYS_UNLINK, path.as_ptr() as u64, path.len() as u64) };
    if res < 0 {
        Err(res)
    } else {
        Ok(())
    }
}
