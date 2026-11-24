
use crate::memory::FRAME_SIZE;

#[derive(Clone, Copy, Debug)]
pub struct PmmRegion {
    pub start_address: u64,
    pub frame_count: usize,
    pub bitmap_offset: usize,
    pub next_hint: usize,
}

impl PmmRegion {
    pub fn new(
        start_address: u64,
        frame_count: usize,
        bitmap_offset: usize,
    ) -> Option<Self> {
        if start_address % FRAME_SIZE != 0 {
            return None;
        }

        if frame_count == 0 {
            return None;
        }

        let byte_length = (frame_count as u64)
            .checked_mul(FRAME_SIZE)?;

        start_address.checked_add(byte_length)?;

        Some(Self {
            start_address,
            frame_count,
            bitmap_offset,
            next_hint: 0,
        })
    }

    pub fn end_address(&self) -> Option<u64> {
        (self.frame_count as u64)
            .checked_mul(FRAME_SIZE)?
            .checked_add(self.start_address)
    }

    pub fn contains_frame(&self, frame: u64) -> bool {
        if frame < self.start_address
            || frame % FRAME_SIZE != 0
        {
            return false;
        }

        match self.end_address() {
            Some(end) => frame < end,
            None => false,
        }
    }

    pub fn frame_index(&self, frame: u64) -> Option<usize> {
        if !self.contains_frame(frame) {
            return None;
        }

        let index = (frame - self.start_address) / FRAME_SIZE;

        usize::try_from(index).ok()
    }

    pub fn frame_address(&self, index: usize) -> Option<u64> {
        if index >= self.frame_count {
            return None;
        }

        self.start_address.checked_add(
            (index as u64).checked_mul(FRAME_SIZE)?
        )
    }

    pub fn bitmap_index(&self, frame_index: usize) -> Option<usize> {
        if frame_index >= self.frame_count {
            return None;
        }

        self.bitmap_offset.checked_add(frame_index)
    }
}
