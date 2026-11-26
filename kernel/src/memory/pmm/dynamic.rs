
use super::bitmap::FrameBitmap;
use super::region::PmmRegion;

use crate::memory::FRAME_SIZE;

pub struct DynamicPmm<R, S, C>
where
    R: AsRef<[PmmRegion]> + AsMut<[PmmRegion]>,
    S: AsRef<[u8]> + AsMut<[u8]>,
    C: AsRef<[u16]> + AsMut<[u16]>,
{
    regions: R,
    bitmap: FrameBitmap<S>,
    refcounts: C,

    region_hint: usize,

    allocated_count: usize,
    total_frames: usize,

    total_allocations: u64,
    total_deallocations: u64,
}

impl<R, S, C> DynamicPmm<R, S, C>
where
    R: AsRef<[PmmRegion]> + AsMut<[PmmRegion]>,
    S: AsRef<[u8]> + AsMut<[u8]>,
    C: AsRef<[u16]> + AsMut<[u16]>,
{
    /// Creates a Dynamic PMM and imports Bootstrap allocations.
    ///
    /// Historical counters start from the imported allocation count.
    /// Use `new_with_stats` when migrating a live Bootstrap PMM.
    pub fn new(
        regions: R,
        bitmap: FrameBitmap<S>,
        refcounts: C,
        bootstrap_allocations: &[u64],
    ) -> Option<Self> {
        Self::new_with_stats(
            regions,
            bitmap,
            refcounts,
            bootstrap_allocations,
            bootstrap_allocations.len() as u64,
            0,
        )
    }

    /// Creates a Dynamic PMM while preserving historical statistics.
    pub fn new_with_stats(
        regions: R,
        mut bitmap: FrameBitmap<S>,
        mut refcounts: C,
        bootstrap_allocations: &[u64],
        total_allocations: u64,
        total_deallocations: u64,
    ) -> Option<Self> {
        let mut expected_offset = 0usize;
        let mut previous_end = None;

        for region in regions.as_ref() {
            if region.frame_count == 0
                || region.start_address % FRAME_SIZE != 0
                || region.bitmap_offset != expected_offset
                || region.next_hint >= region.frame_count
            {
                return None;
            }

            let end = region.end_address()?;

            if let Some(previous) = previous_end {
                if region.start_address < previous {
                    return None;
                }
            }

            expected_offset =
                expected_offset.checked_add(region.frame_count)?;

            previous_end = Some(end);
        }

        if expected_offset != bitmap.frame_count()
            || refcounts.as_ref().len() != expected_offset
            || bootstrap_allocations.len() > expected_offset
            || total_allocations < bootstrap_allocations.len() as u64
            || total_deallocations > total_allocations
        {
            return None;
        }

        // Validate all imported frames before modifying the bitmap.
        for (index, &frame) in bootstrap_allocations.iter().enumerate() {
            if frame % FRAME_SIZE != 0 {
                return None;
            }

            let mut found = false;

            for region in regions.as_ref() {
                if let Some(frame_index) = region.frame_index(frame) {
                    if region.bitmap_index(frame_index).is_some() {
                        found = true;
                        break;
                    }
                }
            }

            if !found || bootstrap_allocations[..index].contains(&frame) {
                return None;
            }
        }

        bitmap.clear();
        refcounts.as_mut().fill(0);

        for &frame in bootstrap_allocations {
            let mut marked = false;

            for region in regions.as_ref() {
                if let Some(frame_index) = region.frame_index(frame) {
                    let bitmap_index = region.bitmap_index(frame_index)?;

                    if !bitmap.mark_allocated(bitmap_index) {
                        bitmap.clear();
                        refcounts.as_mut().fill(0);
                        return None;
                    }
                    refcounts.as_mut()[bitmap_index] = 1;

                    marked = true;
                    break;
                }
            }

            if !marked {
                bitmap.clear();
                refcounts.as_mut().fill(0);
                return None;
            }
        }

        Some(Self {
            regions,
            bitmap,
            refcounts,
            region_hint: 0,
            allocated_count: bootstrap_allocations.len(),
            total_frames: expected_offset,
            total_allocations,
            total_deallocations,
        })
    }

    pub fn allocate_frame(&mut self) -> Option<u64> {
        if self.allocated_count >= self.total_frames {
            return None;
        }

        let region_count = self.regions.as_ref().len();

        if region_count == 0 {
            return None;
        }

        for step in 0..region_count {
            let region_index = (self.region_hint + step) % region_count;
            let region = &mut self.regions.as_mut()[region_index];

            let count = region.frame_count;

            if count == 0 {
                continue;
            }

            let start = region.next_hint.min(count - 1);
            let mut frame_index = start;

            for _ in 0..count {
                let bitmap_index = region.bitmap_index(frame_index)?;

                if !self.bitmap.is_allocated(bitmap_index) {
                    let frame = region.frame_address(frame_index)?;

                    if !self.bitmap.mark_allocated(bitmap_index) {
                        return None;
                    }
                    self.refcounts.as_mut()[bitmap_index] = 1;

                    region.next_hint = if frame_index + 1 == count {
                        0
                    } else {
                        frame_index + 1
                    };

                    self.region_hint = region_index;
                    self.allocated_count += 1;
                    self.total_allocations =
                        self.total_allocations.saturating_add(1);

                    return Some(frame);
                }

                frame_index = if frame_index + 1 == count {
                    0
                } else {
                    frame_index + 1
                };
            }
        }

        None
    }

    pub fn retain_frame(&mut self, frame: u64) -> bool {
        if frame % FRAME_SIZE != 0 {
            return false;
        }

        for region in self.regions.as_ref() {
            let Some(frame_index) = region.frame_index(frame) else {
                continue;
            };
            let Some(bitmap_index) = region.bitmap_index(frame_index) else {
                return false;
            };
            if !self.bitmap.is_allocated(bitmap_index) {
                return false;
            }
            let rc = &mut self.refcounts.as_mut()[bitmap_index];
            if *rc == 0 || *rc == u16::MAX {
                return false;
            }
            *rc += 1;
            return true;
        }

        false
    }

    pub fn frame_refcount(&self, frame: u64) -> usize {
        if frame % FRAME_SIZE != 0 {
            return 0;
        }

        for region in self.regions.as_ref() {
            let Some(frame_index) = region.frame_index(frame) else {
                continue;
            };
            let Some(bitmap_index) = region.bitmap_index(frame_index) else {
                return 0;
            };
            if !self.bitmap.is_allocated(bitmap_index) {
                return 0;
            }
            return self.refcounts.as_ref()[bitmap_index] as usize;
        }

        0
    }

    pub fn deallocate_frame(&mut self, frame: u64) -> bool {
        if frame % FRAME_SIZE != 0 || self.allocated_count == 0 {
            return false;
        }

        for (region_index, region) in
            self.regions.as_mut().iter_mut().enumerate()
        {
            let frame_index = match region.frame_index(frame) {
                Some(index) => index,
                None => continue,
            };

            let bitmap_index = match region.bitmap_index(frame_index) {
                Some(index) => index,
                None => return false,
            };

            if !self.bitmap.is_allocated(bitmap_index) {
                return false;
            }

            let rc = self.refcounts.as_ref()[bitmap_index];
            if rc > 1 {
                self.refcounts.as_mut()[bitmap_index] = rc - 1;
                return true;
            }

            self.refcounts.as_mut()[bitmap_index] = 0;

            if !self.bitmap.mark_free(bitmap_index) {
                return false;
            }

            self.allocated_count -= 1;

            if frame_index < region.next_hint {
                region.next_hint = frame_index;
            }

            self.region_hint = region_index;
            self.total_deallocations =
                self.total_deallocations.saturating_add(1);

            return true;
        }

        false
    }

    pub fn owns_frame(&self, frame: u64) -> bool {
        if frame % FRAME_SIZE != 0 {
            return false;
        }

        for region in self.regions.as_ref() {
            if let Some(frame_index) = region.frame_index(frame) {
                if let Some(bitmap_index) = region.bitmap_index(frame_index) {
                    return self.bitmap.is_allocated(bitmap_index);
                }

                return false;
            }
        }

        false
    }

    pub fn allocated_frames(&self) -> usize {
        self.allocated_count
    }

    pub fn free_frames(&self) -> usize {
        self.total_frames.saturating_sub(self.allocated_count)
    }

    pub fn total_frames(&self) -> usize {
        self.total_frames
    }

    pub fn total_allocations(&self) -> u64 {
        self.total_allocations
    }

    pub fn total_deallocations(&self) -> u64 {
        self.total_deallocations
    }

    pub fn check_integrity(&self) -> bool {
        if !self.bitmap.check_integrity()
            || self.allocated_count > self.total_frames
            || self.bitmap.frame_count() != self.total_frames
            || self.refcounts.as_ref().len() != self.total_frames
            || self.total_allocations < self.allocated_count as u64
            || self.total_deallocations > self.total_allocations
        {
            return false;
        }

        let mut expected_offset = 0usize;
        let mut previous_end = None;
        let mut actual_allocated = 0usize;

        for region in self.regions.as_ref() {
            if region.frame_count == 0
                || region.start_address % FRAME_SIZE != 0
                || region.bitmap_offset != expected_offset
                || region.next_hint >= region.frame_count
            {
                return false;
            }

            let end = match region.end_address() {
                Some(value) => value,
                None => return false,
            };

            if let Some(previous) = previous_end {
                if region.start_address < previous {
                    return false;
                }
            }

            expected_offset = match
                expected_offset.checked_add(region.frame_count)
            {
                Some(value) => value,
                None => return false,
            };

            previous_end = Some(end);

            for index in 0..region.frame_count {
                let bitmap_index = match region.bitmap_index(index) {
                    Some(value) => value,
                    None => return false,
                };

                let rc = self.refcounts.as_ref()[bitmap_index];
                if self.bitmap.is_allocated(bitmap_index) {
                    if rc == 0 {
                        return false;
                    }
                    actual_allocated += 1;
                } else if rc != 0 {
                    return false;
                }
            }
        }

        expected_offset == self.total_frames
            && actual_allocated == self.allocated_count
    }
}
