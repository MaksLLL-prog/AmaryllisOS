/// Standard sector size used by block devices.
pub const DEFAULT_SECTOR_SIZE: u32 = 512;

/// Errors returned by block-device operations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockDeviceError {
    InvalidBufferSize,
    InvalidSectorSize,
    SectorOutOfRange,
    DeviceNotReady,
    UnsupportedOperation,
    Timeout,
    IoError,
}

/// Common interface for sector-based storage devices.
pub trait BlockDevice {
    /// Returns the logical sector size in bytes.
    fn sector_size(&self) -> u32;

    /// Returns the total number of addressable sectors.
    fn sector_count(&self) -> u64;

    /// Maximum contiguous sectors that can be transferred in a single hardware command.
    fn max_transfer_sectors(&self) -> usize {
        1
    }

    /// Reads exactly one logical sector.
    fn read_sector(
        &mut self,
        lba: u64,
        buffer: &mut [u8],
    ) -> Result<(), BlockDeviceError>;

    /// Writes exactly one logical sector.
    fn write_sector(
        &mut self,
        _lba: u64,
        _buffer: &[u8],
    ) -> Result<(), BlockDeviceError> {
        Err(BlockDeviceError::UnsupportedOperation)
    }

    /// Reads `sector_count` contiguous sectors starting at `start_lba`.
    fn read_sectors(
        &mut self,
        start_lba: u64,
        sector_count: usize,
        buffer: &mut [u8],
    ) -> Result<(), BlockDeviceError> {
        if sector_count == 0 {
            return if buffer.is_empty() {
                Ok(())
            } else {
                Err(BlockDeviceError::InvalidBufferSize)
            };
        }
        let sec_size = self.sector_size() as usize;
        if sec_size == 0 {
            return Err(BlockDeviceError::InvalidSectorSize);
        }
        let expected = sector_count
            .checked_mul(sec_size)
            .ok_or(BlockDeviceError::InvalidBufferSize)?;
        if buffer.len() != expected {
            return Err(BlockDeviceError::InvalidBufferSize);
        }
        let total = self.sector_count();
        if total > 0 {
            let end = start_lba
                .checked_add(sector_count as u64)
                .ok_or(BlockDeviceError::SectorOutOfRange)?;
            if end > total {
                return Err(BlockDeviceError::SectorOutOfRange);
            }
        }
        for i in 0..sector_count {
            let off = i * sec_size;
            self.read_sector(start_lba + i as u64, &mut buffer[off..off + sec_size])?;
        }
        Ok(())
    }

    /// Writes `sector_count` contiguous sectors starting at `start_lba`.
    fn write_sectors(
        &mut self,
        start_lba: u64,
        sector_count: usize,
        buffer: &[u8],
    ) -> Result<(), BlockDeviceError> {
        if sector_count == 0 {
            return if buffer.is_empty() {
                Ok(())
            } else {
                Err(BlockDeviceError::InvalidBufferSize)
            };
        }
        let sec_size = self.sector_size() as usize;
        if sec_size == 0 {
            return Err(BlockDeviceError::InvalidSectorSize);
        }
        let expected = sector_count
            .checked_mul(sec_size)
            .ok_or(BlockDeviceError::InvalidBufferSize)?;
        if buffer.len() != expected {
            return Err(BlockDeviceError::InvalidBufferSize);
        }
        let total = self.sector_count();
        if total > 0 {
            let end = start_lba
                .checked_add(sector_count as u64)
                .ok_or(BlockDeviceError::SectorOutOfRange)?;
            if end > total {
                return Err(BlockDeviceError::SectorOutOfRange);
            }
        }
        for i in 0..sector_count {
            let off = i * sec_size;
            self.write_sector(start_lba + i as u64, &buffer[off..off + sec_size])?;
        }
        Ok(())
    }

    /// Flushes volatile device write caches to persistent media.
    fn flush(&mut self) -> Result<(), BlockDeviceError> {
        Ok(())
    }
}
