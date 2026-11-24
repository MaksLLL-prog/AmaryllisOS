
use bootloader_api::info::{MemoryRegion, MemoryRegionKind};

use crate::memory::FRAME_SIZE;

const MIN_ALLOCATABLE_ADDRESS: u64 = 0x100000;
pub const BOOTSTRAP_CAPACITY: usize = 1024;

pub struct BootstrapPmm<'a> {
    regions: &'a [MemoryRegion],
    region_index: usize,
    next_frame: Option<u64>,

    free_stack: [u64; BOOTSTRAP_CAPACITY],
    free_count: usize,

    allocated_frames: [u64; BOOTSTRAP_CAPACITY],
    allocated_count: usize,

    total_allocations: u64,
    total_deallocations: u64,
}

impl<'a> BootstrapPmm<'a> {
    pub const fn new(regions: &'a [MemoryRegion]) -> Self {
        Self {
            regions,
            region_index: 0,
            next_frame: None,
            free_stack: [0; BOOTSTRAP_CAPACITY],
            free_count: 0,
            allocated_frames: [0; BOOTSTRAP_CAPACITY],
            allocated_count: 0,
            total_allocations: 0,
            total_deallocations: 0,
        }
    }

    pub fn allocate_frame(&mut self) -> Option<u64> {
        if self.allocated_count >= BOOTSTRAP_CAPACITY {
            return None;
        }

if self.free_count > 0 {
    let index = self.free_count - 1;
    let frame = self.free_stack[index];

    if !self.register_allocation(frame) {
        return None;
    }

    self.free_count = index;
    self.free_stack[index] = 0;

    return Some(frame);
}

        while self.region_index < self.regions.len() {
            let region = &self.regions[self.region_index];

            if region.kind != MemoryRegionKind::Usable {
                self.region_index += 1;
                self.next_frame = None;
                continue;
            }

            let start = region.start.max(MIN_ALLOCATABLE_ADDRESS);

            let frame = match self.next_frame {
                Some(value) => value,
                None => match align_up(start, FRAME_SIZE) {
                    Some(value) => value,
                    None => {
                        self.region_index += 1;
                        continue;
                    }
                },
            };

            let frame_end = match frame.checked_add(FRAME_SIZE) {
                Some(value) => value,
                None => {
                    self.region_index += 1;
                    self.next_frame = None;
                    continue;
                }
            };

            if frame_end > region.end {
                self.region_index += 1;
                self.next_frame = None;
                continue;
            }

            self.next_frame = frame.checked_add(FRAME_SIZE);

            if self.register_allocation(frame) {
                return Some(frame);
            }

            return None;
        }

        None
    }

    pub fn deallocate_frame(&mut self, frame: u64) -> bool {
        if frame % FRAME_SIZE != 0
            || !self.frame_is_usable(frame)
            || self.free_count >= BOOTSTRAP_CAPACITY
        {
            return false;
        }

        let index = match self.allocated_frames[..self.allocated_count]
            .iter()
            .position(|&allocated| allocated == frame)
        {
            Some(index) => index,
            None => return false,
        };

        self.allocated_count -= 1;
        self.allocated_frames[index] =
            self.allocated_frames[self.allocated_count];

        self.allocated_frames[self.allocated_count] = 0;

        self.free_stack[self.free_count] = frame;
        self.free_count += 1;

        self.total_deallocations =
            self.total_deallocations.saturating_add(1);

        true
    }

    pub fn owns_frame(&self, frame: u64) -> bool {
        if frame % FRAME_SIZE != 0 {
            return false;
        }

        self.allocated_frames[..self.allocated_count]
            .contains(&frame)
    }

    pub fn allocated_frames(&self) -> usize {
        self.allocated_count
    }

    pub fn cached_free_frames(&self) -> usize {
        self.free_count
    }

    pub fn total_allocations(&self) -> u64 {
        self.total_allocations
    }

    pub fn total_deallocations(&self) -> u64 {
        self.total_deallocations
    }

    pub fn available_slots(&self) -> usize {
        BOOTSTRAP_CAPACITY - self.allocated_count
    }

    /// Returns an allocated physical frame by index.
    pub fn allocated_frame_at(&self, index: usize) -> Option<u64> {
        self.allocated_frames.get(index)
            .filter(|_| index < self.allocated_count)
            .copied()
    }

    /// Copies all active allocations into a caller-provided buffer.
    /// Returns None if the buffer is too small.
    pub fn copy_allocated_frames(
        &self,
        output: &mut [u64],
    ) -> Option<usize> {
        if output.len() < self.allocated_count {
            return None;
        }

        output[..self.allocated_count]
            .copy_from_slice(&self.allocated_frames[..self.allocated_count]);

        Some(self.allocated_count)
    }

    pub fn check_integrity(&self) -> bool {
        if self.allocated_count > BOOTSTRAP_CAPACITY
            || self.free_count > BOOTSTRAP_CAPACITY
        {
            return false;
        }

        for &frame in &self.allocated_frames[..self.allocated_count] {
            if frame == 0
                || frame % FRAME_SIZE != 0
                || !self.frame_is_usable(frame)
            {
                return false;
            }
        }

        for &frame in &self.free_stack[..self.free_count] {
            if frame == 0
                || frame % FRAME_SIZE != 0
                || !self.frame_is_usable(frame)
            {
                return false;
            }

            if self.allocated_frames[..self.allocated_count]
                .contains(&frame)
            {
                return false;
            }
        }

        for i in 0..self.allocated_count {
            if self.allocated_frames[i + 1..self.allocated_count]
                .contains(&self.allocated_frames[i])
            {
                return false;
            }
        }

        for i in 0..self.free_count {
            if self.free_stack[i + 1..self.free_count]
                .contains(&self.free_stack[i])
            {
                return false;
            }
        }

        true
    }

    fn register_allocation(&mut self, frame: u64) -> bool {
        if self.allocated_count >= BOOTSTRAP_CAPACITY
            || !self.frame_is_usable(frame)
            || self.allocated_frames[..self.allocated_count]
                .contains(&frame)
        {
            return false;
        }

        self.allocated_frames[self.allocated_count] = frame;
        self.allocated_count += 1;

        self.total_allocations =
            self.total_allocations.saturating_add(1);

        true
    }

    fn frame_is_usable(&self, frame: u64) -> bool {
        if frame < MIN_ALLOCATABLE_ADDRESS
            || frame % FRAME_SIZE != 0
        {
            return false;
        }

        let frame_end = match frame.checked_add(FRAME_SIZE) {
            Some(value) => value,
            None => return false,
        };

        self.regions.iter().any(|region| {
            region.kind == MemoryRegionKind::Usable
                && frame >= region.start.max(MIN_ALLOCATABLE_ADDRESS)
                && frame_end <= region.end
        })
    }
}

fn align_up(value: u64, alignment: u64) -> Option<u64> {
    if alignment == 0 || !alignment.is_power_of_two() {
        return None;
    }

    value
        .checked_add(alignment - 1)
        .map(|aligned| aligned & !(alignment - 1))
}
