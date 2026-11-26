
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
    Dynamic(DynamicPmm<Vec<PmmRegion>, Vec<u8>, Vec<u16>>),
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
        match &mut self.backend {
            AllocatorBackend::Bootstrap(allocator) => {
                allocator.allocate_frame()
            }

            AllocatorBackend::Dynamic(allocator) => {
                allocator.allocate_frame()
            }
        }
    }

    pub fn deallocate_frame(&mut self, frame: u64) -> bool {
        match &mut self.backend {
            AllocatorBackend::Bootstrap(allocator) => {
                allocator.deallocate_frame(frame)
            }

            AllocatorBackend::Dynamic(allocator) => {
                allocator.deallocate_frame(frame)
            }
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

    pub fn retain_frame(&mut self, frame: u64) -> bool {
        match &mut self.backend {
            AllocatorBackend::Bootstrap(_) => false,
            AllocatorBackend::Dynamic(allocator) => allocator.retain_frame(frame),
        }
    }

    pub fn frame_refcount(&self, frame: u64) -> usize {
        match &self.backend {
            AllocatorBackend::Bootstrap(allocator) => usize::from(allocator.owns_frame(frame)),
            AllocatorBackend::Dynamic(allocator) => allocator.frame_refcount(frame),
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

        let mut refcounts = Vec::<u16>::new();
        if refcounts.try_reserve_exact(metadata.total_frames).is_err() {
            return false;
        }
        refcounts.resize(metadata.total_frames, 0);

        // Construct the new backend locally.
        // The current Bootstrap backend is still untouched.
        let dynamic = match DynamicPmm::new_with_stats(
            regions,
            bitmap,
            refcounts,
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

#[inline]
pub fn inc_frame_ref(allocator: &mut PhysicalFrameAllocator<'_>, frame: u64) -> bool {
    allocator.retain_frame(frame)
}

#[inline]
pub fn frame_refcount(allocator: &PhysicalFrameAllocator<'_>, frame: u64) -> usize {
    allocator.frame_refcount(frame)
}
