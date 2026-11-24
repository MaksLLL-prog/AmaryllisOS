//! Ring 3 `#[global_allocator]` backed by `SYS_BRK` free-list heap + `SYS_MMAP`/`SYS_MUNMAP`.

use core::alloc::{GlobalAlloc, Layout};
use core::cell::UnsafeCell;
use core::ptr::{self, null_mut};
use core::sync::atomic::{AtomicBool, Ordering};

use crate::syscall::{syscall1, syscall2, syscall4, SYS_BRK, SYS_MMAP, SYS_MUNMAP};

const HEADER_SIZE: usize = 32;
const MIN_SPLIT_PAYLOAD: usize = 32;
const BRK_CHUNK_SIZE: usize = 64 * 1024;
const MMAP_THRESHOLD: usize = 64 * 1024;
const USER_MMAP_BASE: u64 = 0x0000_0040_0000_0000;
const MMAP_MAGIC: u64 = 0x414D_4152_4D4D_4150; // "AMARMAP"

#[repr(C, align(16))]
struct BlockHeader {
    size: usize,
    free: usize,
    next: *mut BlockHeader,
    _pad: usize,
}

#[repr(C)]
struct MmapHeader {
    magic: u64,
    base: u64,
    map_len: usize,
    _pad: usize,
}

struct HeapInner {
    head: *mut BlockHeader,
    tail: *mut BlockHeader,
    brk_start: usize,
    brk_end: usize,
    mmap_bytes: usize,
}

impl HeapInner {
    const fn new() -> Self {
        Self {
            head: null_mut(),
            tail: null_mut(),
            brk_start: 0,
            brk_end: 0,
            mmap_bytes: 0,
        }
    }
}

pub struct AmaryllisAllocator {
    lock: AtomicBool,
    inner: UnsafeCell<HeapInner>,
}

unsafe impl Sync for AmaryllisAllocator {}

struct SpinGuard<'a> {
    lock: &'a AtomicBool,
}

impl<'a> Drop for SpinGuard<'a> {
    fn drop(&mut self) {
        self.lock.store(false, Ordering::Release);
    }
}

#[inline]
const fn align_up(val: usize, align: usize) -> usize {
    (val + align - 1) & !(align - 1)
}

impl AmaryllisAllocator {
    pub const fn new() -> Self {
        Self {
            lock: AtomicBool::new(false),
            inner: UnsafeCell::new(HeapInner::new()),
        }
    }

    fn acquire(&self) -> SpinGuard<'_> {
        while self
            .lock
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            core::hint::spin_loop();
        }
        SpinGuard { lock: &self.lock }
    }

    unsafe fn alloc_mmap(&self, layout: Layout) -> *mut u8 {
        let hdr_size = core::mem::size_of::<MmapHeader>();
        let align = layout.align().max(16);
        let Some(raw_need) = layout
            .size()
            .checked_add(align)
            .and_then(|s| s.checked_add(hdr_size))
        else {
            return null_mut();
        };
        let map_len = align_up(raw_need, 4096);
        // prot = PROT_READ(1) | PROT_WRITE(2), flags = MAP_PRIVATE(2) | MAP_ANONYMOUS(0x20)
        let res = unsafe { syscall4(SYS_MMAP, 0, map_len as u64, 3, 0x22) };
        if res <= 0 {
            return null_mut();
        }
        let base = res as usize;
        let data_addr = align_up(base + hdr_size, align);
        let hdr_ptr = (data_addr - hdr_size) as *mut MmapHeader;
        unsafe {
            hdr_ptr.write(MmapHeader {
                magic: MMAP_MAGIC,
                base: base as u64,
                map_len,
                _pad: 0,
            });
        }
        let _g = self.acquire();
        unsafe {
            (*self.inner.get()).mmap_bytes += map_len;
        }
        data_addr as *mut u8
    }

    unsafe fn grow_brk(inner: &mut HeapInner, min_payload: usize) -> *mut BlockHeader {
        if inner.brk_end == 0 {
            let cur = unsafe { syscall1(SYS_BRK, 0) };
            if cur <= 0 {
                return null_mut();
            }
            let aligned = align_up(cur as usize, 16);
            if aligned != cur as usize {
                let adj = unsafe { syscall1(SYS_BRK, aligned as u64) };
                if adj < aligned as i64 {
                    return null_mut();
                }
            }
            inner.brk_start = aligned;
            inner.brk_end = aligned;
        }

        let need_total = align_up(HEADER_SIZE + min_payload, 16);
        let grow = need_total.max(BRK_CHUNK_SIZE);
        let target_end = inner.brk_end.saturating_add(grow);
        let res = unsafe { syscall1(SYS_BRK, target_end as u64) };
        if res < target_end as i64 {
            return null_mut();
        }

        let block_addr = inner.brk_end;
        let payload_size = grow - HEADER_SIZE;
        inner.brk_end = target_end;

        let blk = block_addr as *mut BlockHeader;
        unsafe {
            blk.write(BlockHeader {
                size: payload_size,
                free: 1,
                next: null_mut(),
                _pad: 0,
            });
        }

        if inner.head.is_null() {
            inner.head = blk;
            inner.tail = blk;
        } else {
            let tail = inner.tail;
            unsafe {
                if (*tail).free != 0 && (tail as usize) + HEADER_SIZE + (*tail).size == block_addr {
                    (*tail).size += grow;
                    return tail;
                }
                (*tail).next = blk;
            }
            inner.tail = blk;
        }
        blk
    }

    unsafe fn coalesce_forward(head: *mut BlockHeader) {
        let mut cur = head;
        while !cur.is_null() {
            unsafe {
                let nxt = (*cur).next;
                if (*cur).free != 0
                    && !nxt.is_null()
                    && (*nxt).free != 0
                    && (cur as usize) + HEADER_SIZE + (*cur).size == (nxt as usize)
                {
                    (*cur).size += HEADER_SIZE + (*nxt).size;
                    (*cur).next = (*nxt).next;
                    continue;
                }
                cur = nxt;
            }
        }
    }
}

unsafe impl GlobalAlloc for AmaryllisAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if layout.size() == 0 {
            return layout.align() as *mut u8;
        }
        if layout.align() > 16 || layout.size() >= MMAP_THRESHOLD {
            return unsafe { self.alloc_mmap(layout) };
        }

        let req_size = align_up(layout.size().max(16), 16);
        {
            let _guard = self.acquire();
            let inner = unsafe { &mut *self.inner.get() };

            let mut cur = inner.head;
            while !cur.is_null() {
                unsafe {
                    if (*cur).free != 0 && (*cur).size >= req_size {
                        let rem = (*cur).size - req_size;
                        if rem >= HEADER_SIZE + MIN_SPLIT_PAYLOAD {
                            let next_blk = ((cur as usize) + HEADER_SIZE + req_size) as *mut BlockHeader;
                            next_blk.write(BlockHeader {
                                size: rem - HEADER_SIZE,
                                free: 1,
                                next: (*cur).next,
                                _pad: 0,
                            });
                            (*cur).size = req_size;
                            (*cur).next = next_blk;
                            if inner.tail == cur {
                                inner.tail = next_blk;
                            }
                        }
                        (*cur).free = 0;
                        return ((cur as usize) + HEADER_SIZE) as *mut u8;
                    }
                    cur = (*cur).next;
                }
            }

            let grown = unsafe { Self::grow_brk(inner, req_size) };
            if !grown.is_null() {
                unsafe {
                    let rem = (*grown).size.saturating_sub(req_size);
                    if rem >= HEADER_SIZE + MIN_SPLIT_PAYLOAD {
                        let next_blk = ((grown as usize) + HEADER_SIZE + req_size) as *mut BlockHeader;
                        next_blk.write(BlockHeader {
                            size: rem - HEADER_SIZE,
                            free: 1,
                            next: (*grown).next,
                            _pad: 0,
                        });
                        (*grown).size = req_size;
                        (*grown).next = next_blk;
                        if inner.tail == grown {
                            inner.tail = next_blk;
                        }
                    }
                    (*grown).free = 0;
                    return ((grown as usize) + HEADER_SIZE) as *mut u8;
                }
            }
        }

        // Fall back to SYS_MMAP if brk reached USER_HEAP_LIMIT
        unsafe { self.alloc_mmap(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if layout.size() == 0 || ptr.is_null() {
            return;
        }
        if (ptr as u64) >= USER_MMAP_BASE {
            let hdr_size = core::mem::size_of::<MmapHeader>();
            let hdr_ptr = (ptr as usize - hdr_size) as *const MmapHeader;
            let hdr = unsafe { hdr_ptr.read() };
            if hdr.magic == MMAP_MAGIC {
                {
                    let _guard = self.acquire();
                    let inner = unsafe { &mut *self.inner.get() };
                    inner.mmap_bytes = inner.mmap_bytes.saturating_sub(hdr.map_len);
                }
                let _ = unsafe { syscall2(SYS_MUNMAP, hdr.base, hdr.map_len as u64) };
            }
            return;
        }

        let _guard = self.acquire();
        let inner = unsafe { &mut *self.inner.get() };
        let blk = ((ptr as usize) - HEADER_SIZE) as *mut BlockHeader;
        unsafe {
            (*blk).free = 1;
            Self::coalesce_forward(inner.head);
            let mut tail = inner.head;
            while !tail.is_null() && !(*tail).next.is_null() {
                tail = (*tail).next;
            }
            inner.tail = tail;
        }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if ptr.is_null() {
            let Ok(nl) = Layout::from_size_align(new_size, layout.align()) else {
                return null_mut();
            };
            return unsafe { self.alloc(nl) };
        }
        if new_size == 0 {
            unsafe { self.dealloc(ptr, layout) };
            return layout.align() as *mut u8;
        }

        // Fast path for brk blocks that already fit the requested size
        if (ptr as u64) < USER_MMAP_BASE && layout.align() <= 16 {
            let blk = ((ptr as usize) - HEADER_SIZE) as *const BlockHeader;
            if unsafe { (*blk).size } >= new_size {
                return ptr;
            }
        }

        let Ok(nl) = Layout::from_size_align(new_size, layout.align()) else {
            return null_mut();
        };
        let new_ptr = unsafe { self.alloc(nl) };
        if !new_ptr.is_null() {
            let copy_len = layout.size().min(new_size);
            unsafe {
                ptr::copy_nonoverlapping(ptr, new_ptr, copy_len);
                self.dealloc(ptr, layout);
            }
        }
        new_ptr
    }
}

/// Returns `(brk_mapped_bytes, mmap_mapped_bytes, free_list_bytes)` for diagnostics.
pub fn heap_stats(alloc: &AmaryllisAllocator) -> (usize, usize, usize) {
    let _guard = alloc.acquire();
    let inner = unsafe { &*alloc.inner.get() };
    let brk_bytes = inner.brk_end.saturating_sub(inner.brk_start);
    let mut free_bytes = 0usize;
    let mut cur = inner.head;
    while !cur.is_null() {
        unsafe {
            if (*cur).free != 0 {
                free_bytes += (*cur).size;
            }
            cur = (*cur).next;
        }
    }
    (brk_bytes, inner.mmap_bytes, free_bytes)
}
