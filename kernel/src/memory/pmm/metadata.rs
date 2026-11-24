
use alloc::vec::Vec;

use bootloader_api::info::{MemoryRegion, MemoryRegionKind};

use crate::memory::FRAME_SIZE;

use super::region::PmmRegion;

const MIN_ALLOCATABLE_ADDRESS: u64 = 0x100000;

#[derive(Clone, Copy, Debug)]
pub struct PmmMetadata {
    pub region_count: usize,
    pub total_frames: usize,
    pub bitmap_bytes: usize,
}

#[derive(Clone, Copy)]
struct NormalizedRegion {
    start: u64,
    end: u64,
    frame_count: usize,
}

impl PmmMetadata {
    /// Calculates metadata without allocating memory.
    pub fn calculate(regions: &[MemoryRegion]) -> Option<Self> {
        let mut region_count = 0usize;
        let mut total_frames = 0usize;
        let mut previous_end = None;

        for region in regions {
            if region.kind != MemoryRegionKind::Usable {
                continue;
            }

            let normalized = match normalize_region(region)? {
                Some(value) => value,
                None => continue,
            };

            if let Some(end) = previous_end {
                if normalized.start < end {
                    return None;
                }
            }

            previous_end = Some(normalized.end);

            region_count = region_count.checked_add(1)?;
            total_frames =
                total_frames.checked_add(normalized.frame_count)?;
        }

        let bitmap_bytes = total_frames.checked_add(7)? / 8;

        Some(Self {
            region_count,
            total_frames,
            bitmap_bytes,
        })
    }

    /// Writes normalized regions into a caller-provided buffer.
    pub fn build_regions(
        &self,
        source: &[MemoryRegion],
        output: &mut [PmmRegion],
    ) -> Option<usize> {
        if output.len() < self.region_count {
            return None;
        }

        let mut output_index = 0usize;
        let mut bitmap_offset = 0usize;
        let mut previous_end = None;

        for region in source {
            if region.kind != MemoryRegionKind::Usable {
                continue;
            }

            let normalized = match normalize_region(region)? {
                Some(value) => value,
                None => continue,
            };

            if let Some(end) = previous_end {
                if normalized.start < end {
                    return None;
                }
            }

            previous_end = Some(normalized.end);

            if output_index >= self.region_count {
                return None;
            }

            output[output_index] = PmmRegion::new(
                normalized.start,
                normalized.frame_count,
                bitmap_offset,
            )?;

            bitmap_offset =
                bitmap_offset.checked_add(normalized.frame_count)?;

            output_index += 1;
        }

        if output_index != self.region_count
            || bitmap_offset != self.total_frames
        {
            return None;
        }

        Some(output_index)
    }

    /// Allocates and builds the region array using the initialized heap.
    ///
    /// Returns None if allocation or validation fails.
    pub fn build_regions_vec(
        &self,
        source: &[MemoryRegion],
    ) -> Option<Vec<PmmRegion>> {
        let mut output = Vec::new();

        output.try_reserve_exact(self.region_count).ok()?;

        let mut bitmap_offset = 0usize;
        let mut previous_end = None;

        for region in source {
            if region.kind != MemoryRegionKind::Usable {
                continue;
            }

            let normalized = match normalize_region(region)? {
                Some(value) => value,
                None => continue,
            };

            if let Some(end) = previous_end {
                if normalized.start < end {
                    return None;
                }
            }

            previous_end = Some(normalized.end);

            if output.len() >= self.region_count {
                return None;
            }

            let pmm_region = PmmRegion::new(
                normalized.start,
                normalized.frame_count,
                bitmap_offset,
            )?;

            bitmap_offset =
                bitmap_offset.checked_add(normalized.frame_count)?;

            output.push(pmm_region);
        }

        if output.len() != self.region_count
            || bitmap_offset != self.total_frames
        {
            return None;
        }

        Some(output)
    }
}

fn normalize_region(
    region: &MemoryRegion,
) -> Option<Option<NormalizedRegion>> {
    if region.end < region.start {
        return None;
    }

    let start = align_up(
        region.start.max(MIN_ALLOCATABLE_ADDRESS),
        FRAME_SIZE,
    )?;

    let end = region.end & !(FRAME_SIZE - 1);

    if end <= start {
        return Some(None);
    }

    let frame_count = usize::try_from(
        (end - start) / FRAME_SIZE,
    ).ok()?;

    if frame_count == 0 {
        return Some(None);
    }

    Some(Some(NormalizedRegion {
        start,
        end,
        frame_count,
    }))
}

fn align_up(value: u64, alignment: u64) -> Option<u64> {
    if alignment == 0 || !alignment.is_power_of_two() {
        return None;
    }

    value
        .checked_add(alignment - 1)
        .map(|aligned| aligned & !(alignment - 1))
}
