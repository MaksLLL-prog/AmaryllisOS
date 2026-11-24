
pub struct FrameBitmap<S> {
    storage: S,
    storage_len: usize,
    frame_count: usize,
}

impl<S> FrameBitmap<S>
where
    S: AsRef<[u8]> + AsMut<[u8]>,
{
    pub fn new(
        storage: S,
        frame_count: usize,
    ) -> Option<Self> {
        let required_bytes = Self::required_bytes(frame_count)?;

        if storage.as_ref().len() < required_bytes {
            return None;
        }

        Some(Self {
            storage,
            storage_len: required_bytes,
            frame_count,
        })
    }

    pub fn required_bytes(frame_count: usize) -> Option<usize> {
        frame_count.checked_add(7).map(|value| value / 8)
    }

    pub fn frame_count(&self) -> usize {
        self.frame_count
    }

    pub fn storage_size(&self) -> usize {
        self.storage_len
    }

    pub fn clear(&mut self) {
        self.storage.as_mut()[..self.storage_len].fill(0);
    }

    pub fn is_allocated(&self, frame_index: usize) -> bool {
        if frame_index >= self.frame_count {
            return false;
        }

        let byte_index = frame_index / 8;
        let bit_index = frame_index % 8;

        (self.storage.as_ref()[byte_index] & (1 << bit_index)) != 0
    }

    pub fn mark_allocated(&mut self, frame_index: usize) -> bool {
        if frame_index >= self.frame_count {
            return false;
        }

        let byte_index = frame_index / 8;
        let bit_index = frame_index % 8;
        let mask = 1u8 << bit_index;

        let storage = &mut self.storage.as_mut()[..self.storage_len];

        if (storage[byte_index] & mask) != 0 {
            return false;
        }

        storage[byte_index] |= mask;
        true
    }

    pub fn mark_free(&mut self, frame_index: usize) -> bool {
        if frame_index >= self.frame_count {
            return false;
        }

        let byte_index = frame_index / 8;
        let bit_index = frame_index % 8;
        let mask = 1u8 << bit_index;

        let storage = &mut self.storage.as_mut()[..self.storage_len];

        if (storage[byte_index] & mask) == 0 {
            return false;
        }

        storage[byte_index] &= !mask;
        true
    }

    pub fn find_free(&self, start_hint: usize) -> Option<usize> {
        if self.frame_count == 0 {
            return None;
        }

        let start = start_hint.min(self.frame_count);

        for index in start..self.frame_count {
            if !self.is_allocated(index) {
                return Some(index);
            }
        }

        for index in 0..start {
            if !self.is_allocated(index) {
                return Some(index);
            }
        }

        None
    }

    pub fn allocated_count(&self) -> usize {
        let mut count = 0usize;

        for index in 0..self.frame_count {
            if self.is_allocated(index) {
                count += 1;
            }
        }

        count
    }

    pub fn free_count(&self) -> usize {
        self.frame_count - self.allocated_count()
    }

    pub fn check_integrity(&self) -> bool {
        let required_bytes = match Self::required_bytes(self.frame_count) {
            Some(value) => value,
            None => return false,
        };

        if self.storage_len != required_bytes
            || self.storage.as_ref().len() < self.storage_len
        {
            return false;
        }

        let remainder = self.frame_count % 8;

        if remainder != 0 && self.storage_len > 0 {
            let last_byte = self.storage.as_ref()[self.storage_len - 1];
            let valid_mask = (1u8 << remainder) - 1;

            if (last_byte & !valid_mask) != 0 {
                return false;
            }
        }

        true
    }
}
