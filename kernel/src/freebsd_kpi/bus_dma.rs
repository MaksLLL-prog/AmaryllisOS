use alloc::vec::Vec;
use core::ptr::write_bytes;
use core::sync::atomic::{fence, AtomicU64, Ordering};

use crate::memory::vmm::{PhysicalMemoryMapping, PAGE_SIZE};
use crate::memory::PhysicalFrameAllocator;
use crate::task::preempt::PreemptMutex;

use super::KpiError;

pub const BUS_DMA_WAITOK: u32 = 0x00;
pub const BUS_DMA_NOWAIT: u32 = 0x01;
pub const BUS_DMA_ZERO: u32 = 0x04;

pub const BUS_DMASYNC_PREREAD: u32 = 0x01;
pub const BUS_DMASYNC_POSTREAD: u32 = 0x02;
pub const BUS_DMASYNC_PREWRITE: u32 = 0x04;
pub const BUS_DMASYNC_POSTWRITE: u32 = 0x08;

static DMA_TAG_ALLOCS: AtomicU64 = AtomicU64::new(0);
static DMA_ACTIVE_PAGES: AtomicU64 = AtomicU64::new(0);
static DMA_SYNC_OPS: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy)]
pub struct DmaTagSnapshot {
    pub name: &'static str,
    pub alignment: u64,
    pub maxsize: usize,
    pub nsegments: usize,
    pub allocated_pages: usize,
}

static DMA_TAG_REGISTRY: PreemptMutex<Vec<DmaTagSnapshot>> = PreemptMutex::new(Vec::new());

#[derive(Clone, Copy, Debug)]
pub struct BusDmaSegment {
    pub ds_addr: u64,
    pub ds_len: usize,
}

#[derive(Clone, Copy, Debug)]
pub struct BusDmaTag {
    pub name: &'static str,
    pub alignment: u64,
    pub lowaddr: u64,
    pub highaddr: u64,
    pub maxsize: usize,
    pub nsegments: usize,
    pub maxsegsz: usize,
}

impl BusDmaTag {
    pub const EMPTY: Self = Self {
        name: "",
        alignment: PAGE_SIZE,
        lowaddr: u64::MAX,
        highaddr: u64::MAX,
        maxsize: PAGE_SIZE as usize,
        nsegments: 1,
        maxsegsz: PAGE_SIZE as usize,
    };
}

pub struct BusDmaMap {
    pub phys_addrs: Vec<u64>,
    pub virt_addrs: Vec<u64>,
    pub total_size: usize,
}

impl BusDmaMap {
    pub const fn empty() -> Self {
        Self {
            phys_addrs: Vec::new(),
            virt_addrs: Vec::new(),
            total_size: 0,
        }
    }

    #[inline]
    pub fn phys_addr(&self) -> u64 {
        self.phys_addrs.first().copied().unwrap_or(0)
    }

    #[inline]
    pub fn virt_addr(&self) -> u64 {
        self.virt_addrs.first().copied().unwrap_or(0)
    }

    #[inline]
    pub fn page_count(&self) -> usize {
        self.phys_addrs.len()
    }
}

pub fn bus_dma_tag_create(
    name: &'static str,
    alignment: u64,
    lowaddr: u64,
    highaddr: u64,
    maxsize: usize,
    nsegments: usize,
    maxsegsz: usize,
) -> BusDmaTag {
    let tag = BusDmaTag {
        name,
        alignment: alignment.max(1),
        lowaddr,
        highaddr,
        maxsize: maxsize.max(1),
        nsegments: nsegments.max(1),
        maxsegsz: maxsegsz.max(1),
    };
    let mut reg = DMA_TAG_REGISTRY.lock();
    if reg.len() < 64 {
        reg.push(DmaTagSnapshot {
            name: tag.name,
            alignment: tag.alignment,
            maxsize: tag.maxsize,
            nsegments: tag.nsegments,
            allocated_pages: 0,
        });
    }
    tag
}

/// Allocates `pages` 4 KiB frames backed by PMM and reachable via HHDM.
pub fn bus_dmamem_alloc(
    tag: &BusDmaTag,
    size: usize,
    flags: u32,
    mapping: &PhysicalMemoryMapping,
    allocator: &mut PhysicalFrameAllocator<'_>,
) -> Result<BusDmaMap, KpiError> {
    if size == 0 || size > tag.maxsize {
        return Err(KpiError::InvalidArg);
    }

    let pages = ((size + PAGE_SIZE as usize - 1) / PAGE_SIZE as usize).max(1);
    if pages > tag.nsegments && tag.maxsegsz <= PAGE_SIZE as usize {
        return Err(KpiError::NoMem);
    }

    let mut phys_addrs = Vec::with_capacity(pages);
    let mut virt_addrs = Vec::with_capacity(pages);

    for _ in 0..pages {
        let Some(frame) = allocator.allocate_frame() else {
            for p in phys_addrs {
                let _ = allocator.deallocate_frame(p);
            }
            return Err(KpiError::NoMem);
        };
        let Some(virt) = mapping.translate_frame(frame) else {
            let _ = allocator.deallocate_frame(frame);
            for p in phys_addrs {
                let _ = allocator.deallocate_frame(p);
            }
            return Err(KpiError::Io);
        };
        if frame > tag.lowaddr && tag.lowaddr != u64::MAX {
            // Note: QEMU x86_64 guest memory starts below 4G for early allocations, but honor check.
            if (frame + PAGE_SIZE - 1) > tag.highaddr {
                let _ = allocator.deallocate_frame(frame);
                for p in phys_addrs {
                    let _ = allocator.deallocate_frame(p);
                }
                return Err(KpiError::NoMem);
            }
        }
        if (flags & BUS_DMA_ZERO) != 0 {
            unsafe {
                write_bytes(virt.as_u64() as *mut u8, 0, PAGE_SIZE as usize);
            }
        }
        phys_addrs.push(frame);
        virt_addrs.push(virt.as_u64());
    }

    DMA_TAG_ALLOCS.fetch_add(1, Ordering::Relaxed);
    DMA_ACTIVE_PAGES.fetch_add(pages as u64, Ordering::Relaxed);

    let mut reg = DMA_TAG_REGISTRY.lock();
    if let Some(entry) = reg.iter_mut().rev().find(|e| e.name == tag.name) {
        entry.allocated_pages += pages;
    }

    Ok(BusDmaMap {
        phys_addrs,
        virt_addrs,
        total_size: size,
    })
}

pub fn bus_dmamem_free(map: &mut BusDmaMap, allocator: &mut PhysicalFrameAllocator<'_>) {
    let freed = map.phys_addrs.len();
    for p in map.phys_addrs.drain(..) {
        let _ = allocator.deallocate_frame(p);
    }
    map.virt_addrs.clear();
    map.total_size = 0;
    if freed > 0 {
        DMA_ACTIVE_PAGES.fetch_sub(freed as u64, Ordering::Relaxed);
    }
}

pub fn bus_dmamap_load(map: &BusDmaMap, byte_len: usize) -> Vec<BusDmaSegment> {
    let mut segs = Vec::with_capacity(map.phys_addrs.len());
    let mut remaining = byte_len.min(map.total_size);
    for &phys in &map.phys_addrs {
        if remaining == 0 {
            break;
        }
        let chunk = remaining.min(PAGE_SIZE as usize);
        segs.push(BusDmaSegment {
            ds_addr: phys,
            ds_len: chunk,
        });
        remaining -= chunk;
    }
    segs
}

#[inline]
pub fn bus_dmamap_sync(_op: u32) {
    fence(Ordering::SeqCst);
    DMA_SYNC_OPS.fetch_add(1, Ordering::Relaxed);
}

pub fn snapshot_dma_stats() -> (u64, u64, u64, Vec<DmaTagSnapshot>) {
    (
        DMA_TAG_ALLOCS.load(Ordering::Relaxed),
        DMA_ACTIVE_PAGES.load(Ordering::Relaxed),
        DMA_SYNC_OPS.load(Ordering::Relaxed),
        DMA_TAG_REGISTRY.lock().clone(),
    )
}
