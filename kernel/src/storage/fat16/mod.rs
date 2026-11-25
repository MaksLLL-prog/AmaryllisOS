
use crate::storage::block::{
    BlockDevice,
    BlockDeviceError,
};

use crate::storage::partition::{
    self,
    FatBpb,
    FatType,
    MbrPartition,
    PartitionStatus,
};

const SECTOR_SIZE: usize = 512;
const ROOT_ENTRY_SIZE: usize = 32;
const FAT16_ENTRY_SIZE: usize = 2;

const FAT16_EOC_MIN: u16 = 0xFFF8;
const FAT16_BAD_CLUSTER: u16 = 0xFFF7;
const FAT16_RESERVED_MIN: u16 = 0xFFF0;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Fat16Error {
    NoController,
    DeviceNotReady,
    UnsupportedSectorSize,
    NoFat16Partition,
    InvalidPartition,
    InvalidFilesystem,
    InvalidCluster,
    InvalidFatEntry,
    EndOfChain,
    UnexpectedEndOfChain,
    FileNotFound,
    InvalidFileName,
    OffsetOutOfRange,
    DiskReadError,
}

impl From<BlockDeviceError> for Fat16Error {
    fn from(error: BlockDeviceError) -> Self {
        match error {
            BlockDeviceError::DeviceNotReady => Self::DeviceNotReady,
            BlockDeviceError::InvalidSectorSize => Self::UnsupportedSectorSize,
            _ => Self::DiskReadError,
        }
    }
}

#[derive(Clone, Copy)]
pub struct Fat16File {
    pub name: [u8; 11],
    pub attributes: u8,
    pub first_cluster: u16,
    pub size: u32,
}

impl Fat16File {
    pub const EMPTY: Self = Self {
        name: [0; 11],
        attributes: 0,
        first_cluster: 0,
        size: 0,
    };

    pub fn is_directory(&self) -> bool {
        self.attributes & 0x10 != 0
    }

    pub fn is_read_only(&self) -> bool {
        self.attributes & 0x01 != 0
    }

    pub fn is_hidden(&self) -> bool {
        self.attributes & 0x02 != 0
    }

    pub fn is_empty(&self) -> bool {
        self.name[0] == 0
    }
}

/// FAT16 filesystem volume backed by any sector-based block device.
pub struct Fat16Volume<D: BlockDevice> {
    device: D,

    pub partition: MbrPartition,
    pub bpb: FatBpb,

    root_start_sector: u32,
    data_start_sector: u32,
}

impl<D: BlockDevice> Fat16Volume<D> {
    // ========================================================
    // MOUNT
    // ========================================================

    pub fn mount(mut device: D) -> Result<Self, Fat16Error> {
        if device.sector_size() != SECTOR_SIZE as u32 {
            return Err(Fat16Error::UnsupportedSectorSize);
        }

        let mut mbr_sector = [0u8; SECTOR_SIZE];

        device.read_sector(0, &mut mbr_sector)?;

        let mbr = partition::parse_mbr(
            &mbr_sector,
            device.sector_count(),
        )
        .map_err(|_| Fat16Error::InvalidPartition)?;

        for partition_index in 0..mbr.partitions.len() {
            let partition = mbr.partitions[partition_index];

            if partition.status != PartitionStatus::Valid {
                continue;
            }

            if partition::is_extended_partition(
                partition.partition_type,
            ) || partition.partition_type == 0xEE {
                continue;
            }

            let mut boot_sector = [0u8; SECTOR_SIZE];

            device.read_sector(
                partition.start_lba as u64,
                &mut boot_sector,
            )?;

            let bpb = match partition::parse_fat_bpb(
                &boot_sector,
                partition.sector_count as u64,
            ) {
                Ok(bpb) => bpb,
                Err(_) => continue,
            };

            if bpb.fat_type != FatType::Fat16 {
                continue;
            }

            let root_start =
                bpb.reserved_sectors as u64
                + bpb.fat_count as u64
                    * bpb.fat_size_sectors as u64;

            let data_start = bpb.first_data_sector as u64;

            if root_start > u32::MAX as u64
                || data_start > u32::MAX as u64
            {
                continue;
            }

            return Ok(Self {
                device,
                partition,
                bpb,
                root_start_sector: root_start as u32,
                data_start_sector: data_start as u32,
            });
        }

        Err(Fat16Error::NoFat16Partition)
    }

    pub fn device(&self) -> &D {
        &self.device
    }

    pub fn device_mut(&mut self) -> &mut D {
        &mut self.device
    }

    pub fn flush(&mut self) -> Result<(), Fat16Error> {
        self.device.flush().map_err(Fat16Error::from)
    }

    // ========================================================
    // DEVICE ACCESS
    // ========================================================

    fn read_partition_sectors(
        &mut self,
        start_relative_lba: u32,
        sector_count: usize,
        buffer: &mut [u8],
    ) -> Result<(), Fat16Error> {
        if sector_count == 0 {
            return Ok(());
        }
        let end_rel = (start_relative_lba as u64)
            .checked_add(sector_count as u64)
            .ok_or(Fat16Error::InvalidPartition)?;
        if end_rel > self.bpb.total_sectors as u64 {
            return Err(Fat16Error::InvalidPartition);
        }

        let absolute_lba =
            self.partition.start_lba as u64
            + start_relative_lba as u64;
        let abs_end = absolute_lba
            .checked_add(sector_count as u64)
            .ok_or(Fat16Error::InvalidPartition)?;

        if abs_end > self.device.sector_count() {
            return Err(Fat16Error::InvalidPartition);
        }

        self.device.read_sectors(
            absolute_lba,
            sector_count,
            buffer,
        )?;

        Ok(())
    }

    fn read_partition_sector(
        &mut self,
        relative_lba: u32,
        buffer: &mut [u8; SECTOR_SIZE],
    ) -> Result<(), Fat16Error> {
        self.read_partition_sectors(relative_lba, 1, buffer)
    }

    // ========================================================
    // ROOT DIRECTORY
    // ========================================================

    pub fn root_entry_count(&self) -> u16 {
        self.bpb.root_entry_count
    }

    pub fn root_entry_is_end(
        &mut self,
        index: u16,
    ) -> Result<bool, Fat16Error> {
        let entry = self.read_root_entry_raw(index)?;
        Ok(entry[0] == 0x00)
    }

    fn read_root_entry_raw(
        &mut self,
        index: u16,
    ) -> Result<[u8; ROOT_ENTRY_SIZE], Fat16Error> {
        if index >= self.bpb.root_entry_count {
            return Err(Fat16Error::InvalidFileName);
        }

        let byte_offset =
            index as u32 * ROOT_ENTRY_SIZE as u32;

        let sector_offset =
            byte_offset / SECTOR_SIZE as u32;

        let entry_offset =
            (byte_offset % SECTOR_SIZE as u32) as usize;

        let relative_lba = self.root_start_sector
            .checked_add(sector_offset)
            .ok_or(Fat16Error::InvalidPartition)?;

        let mut sector = [0u8; SECTOR_SIZE];

        self.read_partition_sector(
            relative_lba,
            &mut sector,
        )?;

        let mut raw = [0u8; ROOT_ENTRY_SIZE];

        raw.copy_from_slice(
            &sector[
                entry_offset..entry_offset + ROOT_ENTRY_SIZE
            ],
        );

        Ok(raw)
    }

    pub fn read_root_entry(
        &mut self,
        index: u16,
    ) -> Result<Fat16File, Fat16Error> {
        let entry = self.read_root_entry_raw(index)?;

        let first_byte = entry[0];

        if first_byte == 0x00 || first_byte == 0xE5 {
            return Ok(Fat16File::EMPTY);
        }

        let attributes = entry[11];

        if attributes == 0x0F || attributes & 0x08 != 0 {
            return Ok(Fat16File::EMPTY);
        }

        let mut name = [0u8; 11];
        name.copy_from_slice(&entry[0..11]);

        Ok(Fat16File {
            name,
            attributes,
            first_cluster: read_u16_le(&entry, 26),
            size: read_u32_le(&entry, 28),
        })
    }

    pub fn find_root_file(
        &mut self,
        name: &[u8; 11],
    ) -> Result<Fat16File, Fat16Error> {
        for index in 0..self.bpb.root_entry_count {
            let raw = self.read_root_entry_raw(index)?;

            if raw[0] == 0x00 {
                break;
            }

            if raw[0] == 0xE5 {
                continue;
            }

            let attributes = raw[11];

            if attributes == 0x0F || attributes & 0x08 != 0 {
                continue;
            }

            let mut entry_name = [0u8; 11];
            entry_name.copy_from_slice(&raw[0..11]);

            if entry_name == *name {
                return Ok(Fat16File {
                    name: entry_name,
                    attributes,
                    first_cluster: read_u16_le(&raw, 26),
                    size: read_u32_le(&raw, 28),
                });
            }
        }

        Err(Fat16Error::FileNotFound)
    }

    // ========================================================
    // FAT TABLE
    // ========================================================

    pub fn next_cluster(
        &mut self,
        cluster: u16,
    ) -> Result<u16, Fat16Error> {
        if cluster < 2
            || cluster as u32 >= self.bpb.cluster_count + 2
        {
            return Err(Fat16Error::InvalidCluster);
        }

        let fat_byte_offset =
            cluster as u32 * FAT16_ENTRY_SIZE as u32;

        let fat_sector_offset =
            fat_byte_offset / SECTOR_SIZE as u32;

        let entry_offset =
            (fat_byte_offset % SECTOR_SIZE as u32) as usize;

        if fat_sector_offset >= self.bpb.fat_size_sectors {
            return Err(Fat16Error::InvalidFatEntry);
        }

        let fat_start = self.bpb.reserved_sectors as u32;

        let relative_lba = fat_start
            .checked_add(fat_sector_offset)
            .ok_or(Fat16Error::InvalidFatEntry)?;

        let mut sector = [0u8; SECTOR_SIZE];

        self.read_partition_sector(
            relative_lba,
            &mut sector,
        )?;

        let next = read_u16_le(&sector, entry_offset);

        if next == 0
            || next == 1
            || next == FAT16_BAD_CLUSTER
            || (next >= FAT16_RESERVED_MIN
                && next < FAT16_EOC_MIN)
        {
            return Err(Fat16Error::InvalidFatEntry);
        }

        if next >= 2
            && next < FAT16_EOC_MIN
            && next as u32 >= self.bpb.cluster_count + 2
        {
            return Err(Fat16Error::InvalidCluster);
        }

        Ok(next)
    }

    // ========================================================
    // CLUSTER ACCESS
    // ========================================================

    fn cluster_sector(
        &self,
        cluster: u16,
        sector_index: u8,
    ) -> Result<u32, Fat16Error> {
        if cluster < 2
            || cluster as u32 >= self.bpb.cluster_count + 2
            || sector_index >= self.bpb.sectors_per_cluster
        {
            return Err(Fat16Error::InvalidCluster);
        }

        let cluster_offset =
            (cluster as u64 - 2)
            * self.bpb.sectors_per_cluster as u64;

        let relative_lba =
            self.data_start_sector as u64
            + cluster_offset
            + sector_index as u64;

        if relative_lba >= self.bpb.total_sectors as u64
            || relative_lba > u32::MAX as u64
        {
            return Err(Fat16Error::InvalidPartition);
        }

        Ok(relative_lba as u32)
    }

    // ========================================================
    // FILE DATA
    // ========================================================

    /// Reads a file range into the supplied buffer.
    /// Returns the number of bytes actually read.
    pub fn read_file_range(
        &mut self,
        file: &Fat16File,
        offset: u32,
        output: &mut [u8],
    ) -> Result<usize, Fat16Error> {
        if file.is_directory() {
            return Err(Fat16Error::InvalidFileName);
        }

        if offset > file.size {
            return Err(Fat16Error::OffsetOutOfRange);
        }

        if output.is_empty() || offset == file.size {
            return Ok(0);
        }

        if file.first_cluster < 2 {
            return Err(Fat16Error::InvalidCluster);
        }

        let available = (file.size - offset) as usize;

        let target_length = core::cmp::min(
            output.len(),
            available,
        );

        let cluster_bytes =
            self.bpb.sectors_per_cluster as u64
            * SECTOR_SIZE as u64;

        if cluster_bytes == 0 {
            return Err(Fat16Error::InvalidFilesystem);
        }

        let mut current_cluster = file.first_cluster;

        let mut clusters_to_skip =
            offset as u64 / cluster_bytes;

        let mut intra_cluster_offset =
            offset as u64 % cluster_bytes;

        let mut traversed = 1u32;

        while clusters_to_skip > 0 {
            if traversed >= self.bpb.cluster_count {
                return Err(Fat16Error::InvalidCluster);
            }

            let next = self.next_cluster(current_cluster)?;

            if next >= FAT16_EOC_MIN {
                return Err(Fat16Error::UnexpectedEndOfChain);
            }

            current_cluster = next;
            clusters_to_skip -= 1;
            traversed += 1;
        }

        let spc = self.bpb.sectors_per_cluster as usize;
        let mut written = 0usize;

        while written < target_length {
            let first_sector = (intra_cluster_offset / SECTOR_SIZE as u64) as usize;
            let byte_in_sector = (intra_cluster_offset % SECTOR_SIZE as u64) as usize;

            if byte_in_sector != 0 {
                let mut sector = [0u8; SECTOR_SIZE];
                let relative_lba = self.cluster_sector(
                    current_cluster,
                    first_sector as u8,
                )?;
                self.read_partition_sector(
                    relative_lba,
                    &mut sector,
                )?;

                let remaining = target_length - written;
                let count = core::cmp::min(
                    SECTOR_SIZE - byte_in_sector,
                    remaining,
                );
                output[written..written + count]
                    .copy_from_slice(&sector[byte_in_sector..byte_in_sector + count]);

                written += count;
                intra_cluster_offset += count as u64;

                if written >= target_length {
                    break;
                }
                if intra_cluster_offset >= cluster_bytes {
                    if traversed >= self.bpb.cluster_count {
                        return Err(Fat16Error::InvalidCluster);
                    }
                    let next = self.next_cluster(current_cluster)?;
                    if next >= FAT16_EOC_MIN {
                        return Err(Fat16Error::UnexpectedEndOfChain);
                    }
                    current_cluster = next;
                    traversed += 1;
                    intra_cluster_offset = 0;
                }
                continue;
            }

            let rem_bytes = target_length - written;
            let needed_sectors = (rem_bytes + SECTOR_SIZE - 1) / SECTOR_SIZE;
            let rem_in_cur = spc.saturating_sub(first_sector);
            if rem_in_cur == 0 {
                return Err(Fat16Error::InvalidCluster);
            }

            let start_rel_lba = self.cluster_sector(current_cluster, first_sector as u8)?;
            let mut batch_sectors = core::cmp::min(rem_in_cur, needed_sectors).min(128);
            let mut tail_cluster = current_cluster;
            let mut reached_cluster_end = first_sector + batch_sectors == spc;
            let mut prefetched_next: Option<u16> = None;

            if reached_cluster_end && needed_sectors > batch_sectors && batch_sectors + spc <= 128 {
                while batch_sectors < needed_sectors && batch_sectors + spc <= 128 {
                    if traversed >= self.bpb.cluster_count {
                        return Err(Fat16Error::InvalidCluster);
                    }
                    let next = self.next_cluster(tail_cluster)?;
                    if next >= FAT16_EOC_MIN {
                        return Err(Fat16Error::UnexpectedEndOfChain);
                    }
                    if next == tail_cluster + 1 {
                        tail_cluster = next;
                        traversed += 1;
                        let take = core::cmp::min(spc, needed_sectors - batch_sectors);
                        batch_sectors += take;
                        reached_cluster_end = take == spc;
                    } else {
                        prefetched_next = Some(next);
                        break;
                    }
                }
            }

            let whole_sectors = core::cmp::min(rem_bytes / SECTOR_SIZE, batch_sectors);
            if whole_sectors > 0 {
                let whole_bytes = whole_sectors * SECTOR_SIZE;
                self.read_partition_sectors(
                    start_rel_lba,
                    whole_sectors,
                    &mut output[written..written + whole_bytes],
                )?;
                written += whole_bytes;
            }

            if written < target_length && whole_sectors < batch_sectors {
                let mut tail_sector = [0u8; SECTOR_SIZE];
                self.read_partition_sector(start_rel_lba + whole_sectors as u32, &mut tail_sector)?;
                let tail_bytes = core::cmp::min(SECTOR_SIZE, target_length - written);
                output[written..written + tail_bytes].copy_from_slice(&tail_sector[..tail_bytes]);
                written += tail_bytes;
            }

            if written >= target_length {
                break;
            }

            if !reached_cluster_end {
                intra_cluster_offset += (batch_sectors * SECTOR_SIZE) as u64;
            } else {
                intra_cluster_offset = 0;
                if let Some(nc) = prefetched_next {
                    current_cluster = nc;
                    traversed += 1;
                } else {
                    if traversed >= self.bpb.cluster_count {
                        return Err(Fat16Error::InvalidCluster);
                    }
                    let next = self.next_cluster(tail_cluster)?;
                    if next >= FAT16_EOC_MIN {
                        return Err(Fat16Error::UnexpectedEndOfChain);
                    }
                    current_cluster = next;
                    traversed += 1;
                }
            }
        }

        Ok(written)
    }
}

// ============================================================
// LITTLE-ENDIAN HELPERS
// ============================================================

fn read_u16_le(
    buffer: &[u8],
    offset: usize,
) -> u16 {
    (buffer[offset] as u16)
        | ((buffer[offset + 1] as u16) << 8)
}

fn read_u32_le(
    buffer: &[u8],
    offset: usize,
) -> u32 {
    (buffer[offset] as u32)
        | ((buffer[offset + 1] as u32) << 8)
        | ((buffer[offset + 2] as u32) << 16)
        | ((buffer[offset + 3] as u32) << 24)
}


