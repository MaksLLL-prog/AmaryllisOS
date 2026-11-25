
use alloc::vec::Vec;

use bootloader_api::info::{MemoryRegion, MemoryRegionKind};

use core::alloc::Layout;
use core::ptr::NonNull;

use pmm::bitmap::FrameBitmap;
use pmm::bootstrap::{BootstrapPmm, BOOTSTRAP_CAPACITY};
use pmm::dynamic::DynamicPmm;
use pmm::metadata::PmmMetadata;
use pmm::region::PmmRegion;

pub const FRAME_SIZE: u64 = 4096;

use core::sync::atomic::{AtomicU16, AtomicUsize, Ordering};

static FRAME_REFCOUNTS_PTR: AtomicUsize = AtomicUsize::new(0);
static FRAME_REFCOUNTS_LEN: AtomicUsize = AtomicUsize::new(0);

fn init_frame_refcounts(max_phys_addr: u64, active_frames: &[u64]) {
    if FRAME_REFCOUNTS_PTR.load(Ordering::Acquire) != 0 {
        return;
    }
    let page_count = ((max_phys_addr.saturating_add(FRAME_SIZE - 1)) / FRAME_SIZE) as usize;
    if page_count == 0 {
        return;
    }
    let mut vec = Vec::<AtomicU16>::new();
    if vec.try_reserve_exact(page_count).is_err() {
        return;
    }
    for _ in 0..page_count {
        vec.push(AtomicU16::new(0));
    }
    for &f in active_frames {
        let idx = (f / FRAME_SIZE) as usize;
        if idx < page_count {
            vec[idx].store(1, Ordering::Relaxed);
        }
    }
    let boxed = vec.into_boxed_slice();
    let len = boxed.len();
    let ptr = alloc::boxed::Box::into_raw(boxed) as *mut AtomicU16 as usize;
    FRAME_REFCOUNTS_LEN.store(len, Ordering::Release);
    FRAME_REFCOUNTS_PTR.store(ptr, Ordering::Release);
}

#[inline]
fn refcount_slot(frame: u64) -> Option<&'static AtomicU16> {
    let ptr = FRAME_REFCOUNTS_PTR.load(Ordering::Acquire) as *const AtomicU16;
    let len = FRAME_REFCOUNTS_LEN.load(Ordering::Acquire);
    if ptr.is_null() {
        return None;
    }
    let idx = (frame / FRAME_SIZE) as usize;
    if idx >= len {
        return None;
    }
    Some(unsafe { &*ptr.add(idx) })
}

/// Increments the physical frame reference count (used when sharing a frame via CoW/SHM).
#[inline]
pub fn inc_frame_ref(frame: u64) -> u16 {
    if let Some(slot) = refcount_slot(frame) {
        let prev = slot.try_update(Ordering::AcqRel, Ordering::Acquire, |v| {
            if v == 0 { Some(2) } else { v.checked_add(1) }
        }).unwrap_or(1);
        prev.saturating_add(1)
    } else {
        1
    }
}

/// Decrements the physical frame reference count and returns the new count.
#[inline]
pub fn dec_frame_ref(frame: u64) -> u16 {
    if let Some(slot) = refcount_slot(frame) {
        let prev = slot.try_update(Ordering::AcqRel, Ordering::Acquire, |v| {
            if v > 0 { Some(v - 1) } else { Some(0) }
        }).unwrap_or(0);
        prev.saturating_sub(1)
    } else {
        0
    }
}

/// Returns the current reference count of a physical frame (`1` if unshared or pre-migration).
#[inline]
pub fn frame_ref_count(frame: u64) -> u16 {
    if let Some(slot) = refcount_slot(frame) {
        let v = slot.load(Ordering::Acquire);
        if v == 0 { 1 } else { v }
    } else {
        1
    }
}

#[inline]
fn set_frame_ref_one(frame: u64) {
    if let Some(slot) = refcount_slot(frame) {
        slot.store(1, Ordering::Release);
    }
}


pub mod vmm;
pub mod heap;
pub mod address_space;
pub mod cr3;
pub mod diagnostics;
pub mod pmm;

// ============================================================
// PHYSICAL MEMORY STATISTICS
// ============================================================

#[derive(Debug, Clone, Copy)]
pub struct MemoryStats {
    pub total_regions: usize,
    pub usable_regions: usize,
    pub reserved_regions: usize,
    pub mapped_bytes: u64,
    pub usable_bytes: u64,
}

impl MemoryStats {
    pub fn usable_megabytes(&self) -> u64 {
        self.usable_bytes / 1024 / 1024
    }

    pub fn mapped_megabytes(&self) -> u64 {
        self.mapped_bytes / 1024 / 1024
    }
}

pub fn analyze(regions: &[MemoryRegion]) -> MemoryStats {
    let mut stats = MemoryStats {
        total_regions: 0,
        usable_regions: 0,
        reserved_regions: 0,
        mapped_bytes: 0,
        usable_bytes: 0,
    };

    for region in regions {
        let size = region.end.saturating_sub(region.start);

        stats.total_regions += 1;
        stats.mapped_bytes = stats.mapped_bytes.saturating_add(size);

        match region.kind {
            MemoryRegionKind::Usable => {
                stats.usable_regions += 1;
                stats.usable_bytes =
                    stats.usable_bytes.saturating_add(size);
            }
            _ => {
                stats.reserved_regions += 1;
            }
        }
    }

    stats
}

// ============================================================
// HEAP MEMORY API
// ============================================================

#[derive(Debug, Clone, Copy)]
pub struct HeapStats {
    pub initialized: bool,
    pub total_bytes: usize,
    pub used_bytes: usize,
    pub free_bytes: usize,
    pub free_blocks: usize,
    pub largest_free_block: usize,
}

pub fn allocate(layout: Layout) -> Option<NonNull<u8>> {
    heap::GLOBAL_ALLOCATOR.allocate(layout)
}

/// # Safety
///
/// `pointer` must be a live allocation returned by this
/// allocator and must not have been deallocated before.
pub unsafe fn deallocate(pointer: NonNull<u8>) -> bool {
    unsafe {
        heap::GLOBAL_ALLOCATOR.deallocate(pointer)
    }
}

pub fn heap_stats() -> HeapStats {
    let initialized = heap::GLOBAL_ALLOCATOR.is_initialized();
    let total_bytes = heap::GLOBAL_ALLOCATOR.total_size();
    let free_bytes = heap::GLOBAL_ALLOCATOR.free_bytes();

    HeapStats {
        initialized,
        total_bytes,
        used_bytes: total_bytes.saturating_sub(free_bytes),
        free_bytes,
        free_blocks: heap::GLOBAL_ALLOCATOR.free_blocks(),
        largest_free_block:
            heap::GLOBAL_ALLOCATOR.largest_free_block(),
    }
}

pub fn check_heap_integrity() -> bool {
    heap::GLOBAL_ALLOCATOR.check_integrity()
}

// ============================================================
// PHYSICAL FRAME ALLOCATOR BACKEND
// ============================================================

enum AllocatorBackend<'a> {
    Bootstrap(BootstrapPmm<'a>),
    Dynamic(DynamicPmm<Vec<PmmRegion>, Vec<u8>>),
}

// ============================================================
// PHYSICAL FRAME ALLOCATOR
// ============================================================

pub struct PhysicalFrameAllocator<'a> {
    memory_regions: &'a [MemoryRegion],
    backend: AllocatorBackend<'a>,
}

impl<'a> PhysicalFrameAllocator<'a> {
    /// Creates a Bootstrap PMM.
    ///
    /// This constructor does not allocate from the heap.
    pub const fn new(regions: &'a [MemoryRegion]) -> Self {
        Self {
            memory_regions: regions,
            backend: AllocatorBackend::Bootstrap(
                BootstrapPmm::new(regions),
            ),
        }
    }

    // --------------------------------------------------------
    // ALLOCATION
    // --------------------------------------------------------

    pub fn allocate_frame(&mut self) -> Option<u64> {
        let res = match &mut self.backend {
            AllocatorBackend::Bootstrap(allocator) => allocator.allocate_frame(),
            AllocatorBackend::Dynamic(allocator) => allocator.allocate_frame(),
        };
        if let Some(frame) = res {
            set_frame_ref_one(frame);
        }
        res
    }

    pub fn deallocate_frame(&mut self, frame: u64) -> bool {
        if dec_frame_ref(frame) > 0 {
            return true;
        }
        match &mut self.backend {
            AllocatorBackend::Bootstrap(allocator) => allocator.deallocate_frame(frame),
            AllocatorBackend::Dynamic(allocator) => allocator.deallocate_frame(frame),
        }
    }

    pub fn owns_frame(&self, frame: u64) -> bool {
        match &self.backend {
            AllocatorBackend::Bootstrap(allocator) => {
                allocator.owns_frame(frame)
            }

            AllocatorBackend::Dynamic(allocator) => {
                allocator.owns_frame(frame)
            }
        }
    }

    // --------------------------------------------------------
    // STATISTICS
    // --------------------------------------------------------

    pub fn allocated_frames(&self) -> usize {
        match &self.backend {
            AllocatorBackend::Bootstrap(allocator) => {
                allocator.allocated_frames()
            }

            AllocatorBackend::Dynamic(allocator) => {
                allocator.allocated_frames()
            }
        }
    }

    /// Bootstrap returns cached freed frames.
    /// Dynamic returns all currently free managed frames.
    pub fn free_frames(&self) -> usize {
        match &self.backend {
            AllocatorBackend::Bootstrap(allocator) => {
                allocator.cached_free_frames()
            }

            AllocatorBackend::Dynamic(allocator) => {
                allocator.free_frames()
            }
        }
    }

    pub fn total_allocations(&self) -> u64 {
        match &self.backend {
            AllocatorBackend::Bootstrap(allocator) => {
                allocator.total_allocations()
            }

            AllocatorBackend::Dynamic(allocator) => {
                allocator.total_allocations()
            }
        }
    }

    pub fn total_deallocations(&self) -> u64 {
        match &self.backend {
            AllocatorBackend::Bootstrap(allocator) => {
                allocator.total_deallocations()
            }

            AllocatorBackend::Dynamic(allocator) => {
                allocator.total_deallocations()
            }
        }
    }

    pub fn available_slots(&self) -> usize {
        match &self.backend {
            AllocatorBackend::Bootstrap(allocator) => {
                allocator.available_slots()
            }

            AllocatorBackend::Dynamic(allocator) => {
                allocator.free_frames()
            }
        }
    }

    // --------------------------------------------------------
    // BACKEND INFORMATION
    // --------------------------------------------------------

    pub fn is_dynamic(&self) -> bool {
        matches!(
            self.backend,
            AllocatorBackend::Dynamic(_)
        )
    }

    pub fn is_bootstrap(&self) -> bool {
        matches!(
            self.backend,
            AllocatorBackend::Bootstrap(_)
        )
    }

    // --------------------------------------------------------
    // BOOTSTRAP -> DYNAMIC MIGRATION
    // --------------------------------------------------------

    /// Migrates Bootstrap PMM to Dynamic PMM.
    ///
    /// Requirements:
    /// - Global heap must already be initialized.
    /// - Bootstrap allocator must pass its integrity check.
    ///
    /// The migration is transactional:
    /// the Bootstrap backend is retained until the new
    /// Dynamic backend has been fully constructed and verified.
    pub fn migrate_to_dynamic(&mut self) -> bool {
        if !heap::GLOBAL_ALLOCATOR.is_initialized() {
            return false;
        }

        if self.is_dynamic() {
            return true;
        }

        let bootstrap = match &self.backend {
            AllocatorBackend::Bootstrap(allocator) => allocator,
            AllocatorBackend::Dynamic(_) => return true,
        };

        if !bootstrap.check_integrity() {
            return false;
        }

        // Snapshot active allocations and historical statistics.
        let mut active_frames = [0u64; BOOTSTRAP_CAPACITY];

        let active_count =
            match bootstrap.copy_allocated_frames(&mut active_frames) {
                Some(count) => count,
                None => return false,
            };

        let total_allocations = bootstrap.total_allocations();
        let total_deallocations = bootstrap.total_deallocations();

        if total_allocations < active_count as u64
            || total_deallocations > total_allocations
        {
            return false;
        }

        // Calculate metadata without heap allocation.
        let metadata = match PmmMetadata::calculate(self.memory_regions) {
            Some(metadata) => metadata,
            None => return false,
        };

        if metadata.region_count == 0
            || metadata.total_frames == 0
        {
            return false;
        }

        // Build the dynamic region array.
        let regions = match metadata.build_regions_vec(self.memory_regions) {
            Some(regions) => regions,
            None => return false,
        };

        // Allocate bitmap storage without panicking on OOM.
        let mut bitmap_storage = Vec::<u8>::new();

        if bitmap_storage
            .try_reserve_exact(metadata.bitmap_bytes)
            .is_err()
        {
            return false;
        }

        // Capacity has already been reserved.
        bitmap_storage.resize(metadata.bitmap_bytes, 0);

        let bitmap = match FrameBitmap::new(
            bitmap_storage,
            metadata.total_frames,
        ) {
            Some(bitmap) => bitmap,
            None => return false,
        };

        // Construct the new backend locally.
        // The current Bootstrap backend is still untouched.
        let dynamic = match DynamicPmm::new_with_stats(
            regions,
            bitmap,
            &active_frames[..active_count],
            total_allocations,
            total_deallocations,
        ) {
            Some(dynamic) => dynamic,
            None => return false,
        };

        // Validate the completed Dynamic PMM.
        if !dynamic.check_integrity() {
            return false;
        }

        if dynamic.allocated_frames() != active_count {
            return false;
        }

        // Verify every previously active frame survived migration.
        for &frame in &active_frames[..active_count] {
            if !dynamic.owns_frame(frame) {
                return false;
            }
        }

        let max_phys = self.memory_regions.iter().map(|r| r.end).max().unwrap_or(0);
        init_frame_refcounts(max_phys, &active_frames[..active_count]);

        // Commit: replace Bootstrap only after all checks pass.
        self.backend = AllocatorBackend::Dynamic(dynamic);

        true
    }

    // --------------------------------------------------------
    // INTEGRITY
    // --------------------------------------------------------

    pub fn check_integrity(&self) -> bool {
        match &self.backend {
            AllocatorBackend::Bootstrap(allocator) => {
                allocator.check_integrity()
            }

            AllocatorBackend::Dynamic(allocator) => {
                allocator.check_integrity()
            }
        }
    }
}
