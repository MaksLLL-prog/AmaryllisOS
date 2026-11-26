
use bootloader_api::info::{MemoryRegion, MemoryRegionKind};

use super::bitmap::FrameBitmap;
use super::bootstrap::BootstrapPmm;
use super::dynamic::DynamicPmm;
use super::metadata::PmmMetadata;
use super::region::PmmRegion;

pub fn run_bootstrap_tests() -> bool {
    let regions = [
        MemoryRegion {
            start: 0x80000,
            end: 0x100000,
            kind: MemoryRegionKind::Bootloader,
        },
        MemoryRegion {
            start: 0x100000,
            end: 0x104000,
            kind: MemoryRegionKind::Usable,
        },
        MemoryRegion {
            start: 0x104000,
            end: 0x108000,
            kind: MemoryRegionKind::Bootloader,
        },
        MemoryRegion {
            start: 0x108000,
            end: 0x10C000,
            kind: MemoryRegionKind::Usable,
        },
    ];

    let mut pmm = BootstrapPmm::new(&regions);

    let a = match pmm.allocate_frame() {
        Some(frame) => frame,
        None => return false,
    };

    let b = match pmm.allocate_frame() {
        Some(frame) => frame,
        None => return false,
    };

    let c = match pmm.allocate_frame() {
        Some(frame) => frame,
        None => return false,
    };

    let d = match pmm.allocate_frame() {
        Some(frame) => frame,
        None => return false,
    };

    if a != 0x100000
        || b != 0x101000
        || c != 0x102000
        || d != 0x103000
    {
        return false;
    }

    if !pmm.owns_frame(a)
        || !pmm.owns_frame(b)
        || !pmm.owns_frame(c)
        || !pmm.owns_frame(d)
    {
        return false;
    }

    if pmm.allocated_frames() != 4 {
        return false;
    }

    if !pmm.deallocate_frame(b) {
        return false;
    }

    if pmm.owns_frame(b) || pmm.allocated_frames() != 3 {
        return false;
    }

    if pmm.deallocate_frame(b) {
        return false;
    }

    let reused = match pmm.allocate_frame() {
        Some(frame) => frame,
        None => return false,
    };

    if reused != b || !pmm.owns_frame(reused) {
        return false;
    }

    if pmm.deallocate_frame(reused + 1)
        || pmm.deallocate_frame(0x200000)
    {
        return false;
    }

    let mut snapshot = [0u64; 4];

    let copied = match pmm.copy_allocated_frames(&mut snapshot) {
        Some(count) => count,
        None => return false,
    };

    if copied != 4 {
        return false;
    }

    for i in 0..copied {
        for j in (i + 1)..copied {
            if snapshot[i] == snapshot[j] {
                return false;
            }
        }
    }

    pmm.total_allocations() == 5
        && pmm.total_deallocations() == 1
        && pmm.check_integrity()
}

pub fn run_dynamic_tests() -> bool {
    let mut regions = [
        match PmmRegion::new(0x10000000, 4, 0) {
            Some(region) => region,
            None => return false,
        },
        match PmmRegion::new(0x20000000, 4, 4) {
            Some(region) => region,
            None => return false,
        },
    ];

    let mut bitmap_storage = [0u8; 1];
    let mut refcounts = [0u16; 8];

    let bitmap = match FrameBitmap::new(&mut bitmap_storage, 8) {
        Some(bitmap) => bitmap,
        None => return false,
    };

    let bootstrap_allocations = [
        0x10000000,
        0x20001000,
    ];

    let mut pmm = match DynamicPmm::new(
        &mut regions,
        bitmap,
        &mut refcounts[..],
        &bootstrap_allocations,
    ) {
        Some(pmm) => pmm,
        None => return false,
    };

    if !pmm.owns_frame(0x10000000)
        || !pmm.owns_frame(0x20001000)
        || pmm.frame_refcount(0x10000000) != 1
    {
        return false;
    }

    if pmm.allocated_frames() != 2
        || pmm.free_frames() != 6
        || pmm.total_frames() != 8
    {
        return false;
    }

    let mut allocated = [0u64; 6];

    for slot in allocated.iter_mut() {
        let frame = match pmm.allocate_frame() {
            Some(frame) => frame,
            None => return false,
        };

        if bootstrap_allocations.contains(&frame) {
            return false;
        }

        *slot = frame;
    }

    if pmm.allocated_frames() != 8
        || pmm.free_frames() != 0
    {
        return false;
    }

    if pmm.allocate_frame().is_some() {
        return false;
    }

    for &frame in bootstrap_allocations.iter() {
        if !pmm.owns_frame(frame) {
            return false;
        }
    }

    for &frame in allocated.iter() {
        if !pmm.owns_frame(frame) {
            return false;
        }
    }

    let released = allocated[0];

    // Verify CoW reference counting keeps frame alive until last ref drops.
    if !pmm.retain_frame(released) || pmm.frame_refcount(released) != 2 {
        return false;
    }
    if !pmm.deallocate_frame(released)
        || !pmm.owns_frame(released)
        || pmm.frame_refcount(released) != 1
        || pmm.allocated_frames() != 8
    {
        return false;
    }

    if !pmm.deallocate_frame(released) {
        return false;
    }

    if pmm.owns_frame(released)
        || pmm.frame_refcount(released) != 0
        || pmm.allocated_frames() != 7
        || pmm.free_frames() != 1
    {
        return false;
    }

    if pmm.deallocate_frame(released)
        || pmm.deallocate_frame(released + 1)
    {
        return false;
    }

    let reused = match pmm.allocate_frame() {
        Some(frame) => frame,
        None => return false,
    };

    if reused != released || !pmm.owns_frame(reused) || pmm.frame_refcount(reused) != 1 {
        return false;
    }

    if pmm.deallocate_frame(0x30000000) {
        return false;
    }

    pmm.allocated_frames() == 8
        && pmm.free_frames() == 0
        && pmm.total_allocations() == 9
        && pmm.total_deallocations() == 1
        && pmm.check_integrity()
}

pub fn run_metadata_tests() -> bool {
    // 1. Normalization, alignment and bitmap size.
    let regions = [
        MemoryRegion {
            start: 0x80000,
            end: 0x100000,
            kind: MemoryRegionKind::Bootloader,
        },
        MemoryRegion {
            start: 0x100001,
            end: 0x105000,
            kind: MemoryRegionKind::Usable,
        },
        MemoryRegion {
            start: 0x108000,
            end: 0x10B000,
            kind: MemoryRegionKind::Usable,
        },
    ];

    let metadata = match PmmMetadata::calculate(&regions) {
        Some(value) => value,
        None => return false,
    };

if metadata.region_count != 2
    || metadata.total_frames != 7
    || metadata.bitmap_bytes != 1
    {
        return false;
    }

    // 2. Build normalized regions and verify offsets.
    let mut output = [
        match PmmRegion::new(0x200000, 1, 0) {
            Some(value) => value,
            None => return false,
        },
        match PmmRegion::new(0x300000, 1, 1) {
            Some(value) => value,
            None => return false,
        },
    ];

    let count = match metadata.build_regions(&regions, &mut output) {
        Some(value) => value,
        None => return false,
    };

if count != 2
    || output[0].start_address != 0x101000
    || output[0].frame_count != 4
    || output[0].bitmap_offset != 0
    || output[1].start_address != 0x108000
    || output[1].frame_count != 3
    || output[1].bitmap_offset != 4
    {
        return false;
    }

    // 3. Insufficient output buffer must be rejected.
    let mut small_output = [
        match PmmRegion::new(0x400000, 1, 0) {
            Some(value) => value,
            None => return false,
        },
    ];

    if metadata.build_regions(&regions, &mut small_output).is_some() {
        return false;
    }

    // 4. Overlapping usable regions must be rejected.
    let overlapping = [
        MemoryRegion {
            start: 0x100000,
            end: 0x104000,
            kind: MemoryRegionKind::Usable,
        },
        MemoryRegion {
            start: 0x102000,
            end: 0x106000,
            kind: MemoryRegionKind::Usable,
        },
    ];

    if PmmMetadata::calculate(&overlapping).is_some() {
        return false;
    }

    // 5. Empty and non-usable regions produce empty metadata.
    let empty = [
        MemoryRegion {
            start: 0x100000,
            end: 0x100000,
            kind: MemoryRegionKind::Usable,
        },
        MemoryRegion {
            start: 0x200000,
            end: 0x300000,
            kind: MemoryRegionKind::Bootloader,
        },
    ];

    let empty_metadata = match PmmMetadata::calculate(&empty) {
        Some(value) => value,
        None => return false,
    };

    if empty_metadata.region_count != 0
        || empty_metadata.total_frames != 0
        || empty_metadata.bitmap_bytes != 0
    {
        return false;
    }

    // 6. Reversed memory ranges must be rejected.
    let malformed = [
        MemoryRegion {
            start: 0x200000,
            end: 0x100000,
            kind: MemoryRegionKind::Usable,
        },
    ];

    if PmmMetadata::calculate(&malformed).is_some() {
        return false;
    }

    true
}
