use alloc::string::String;
use alloc::vec::Vec;

use crate::storage::block::{BlockDevice, BlockDeviceError};
use crate::storage::partition::{
    self,
    FatBpb,
    FatType,
    MbrPartition,
    PartitionStatus,
};

const SECTOR_SIZE: usize = 512;
const DIR_ENTRY_SIZE: usize = 32;
const ENTRIES_PER_SECTOR: usize = SECTOR_SIZE / DIR_ENTRY_SIZE;
const FAT32_ENTRIES_PER_SECTOR: usize = SECTOR_SIZE / 4;

const FAT32_MASK: u32 = 0x0FFF_FFFF;
const FAT32_FREE: u32 = 0x0000_0000;
const FAT32_BAD_CLUSTER: u32 = 0x0FFF_FFF7;
const FAT32_EOC_MIN: u32 = 0x0FFF_FFF8;
const FAT32_EOC_MARK: u32 = 0x0FFF_FFFF;

const ATTR_READ_ONLY: u8 = 0x01;
const ATTR_HIDDEN: u8 = 0x02;
const ATTR_SYSTEM: u8 = 0x04;
const ATTR_VOLUME_ID: u8 = 0x08;
const ATTR_DIRECTORY: u8 = 0x10;
const ATTR_ARCHIVE: u8 = 0x20;
const ATTR_LFN: u8 = 0x0F;

const FSINFO_LEAD_SIG: u32 = 0x4161_5252;
const FSINFO_STRUCT_SIG: u32 = 0x6141_7272;
const FSINFO_TRAIL_SIG: u32 = 0xAA55_0000;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Fat32Error {
    DeviceNotReady,
    UnsupportedSectorSize,
    NoFat32Partition,
    InvalidPartition,
    InvalidFilesystem,
    InvalidCluster,
    InvalidFatEntry,
    UnexpectedEndOfChain,
    NoFreeClusters,
    FileNotFound,
    AlreadyExists,
    InvalidFileName,
    IsADirectory,
    NotADirectory,
    DirectoryNotEmpty,
    ReadOnlyEntry,
    OffsetOutOfRange,
    DiskIoError,
}

impl From<BlockDeviceError> for Fat32Error {
    fn from(error: BlockDeviceError) -> Self {
        match error {
            BlockDeviceError::DeviceNotReady => Self::DeviceNotReady,
            BlockDeviceError::InvalidSectorSize => Self::UnsupportedSectorSize,
            _ => Self::DiskIoError,
        }
    }
}

#[derive(Clone)]
pub struct Fat32DirEntry {
    pub name: String,
    pub short_name: [u8; 11],
    pub attributes: u8,
    pub first_cluster: u32,
    pub size: u32,
    pub dir_cluster: u32,
    pub entry_slot: usize,
    pub lfn_count: usize,
}

impl Fat32DirEntry {
    pub fn is_directory(&self) -> bool {
        self.attributes & ATTR_DIRECTORY != 0
    }

    pub fn is_read_only(&self) -> bool {
        self.attributes & ATTR_READ_ONLY != 0
    }

    pub fn is_hidden(&self) -> bool {
        self.attributes & ATTR_HIDDEN != 0
    }

    pub fn is_system(&self) -> bool {
        self.attributes & ATTR_SYSTEM != 0
    }
}

#[derive(Clone, Copy)]
struct SlotLocation {
    cluster: u32,
    sector_in_cluster: u8,
    entry_in_sector: u8,
}

/// Read-write FAT32 volume backed by any sector-based `BlockDevice`.
pub struct Fat32Volume<D: BlockDevice> {
    device: D,
    pub partition: MbrPartition,
    pub bpb: FatBpb,
    data_start_sector: u32,
    fs_info_valid: bool,
    free_cluster_hint: Option<u32>,
    next_free_hint: u32,
}

impl<D: BlockDevice> Fat32Volume<D> {
    // ========================================================
    // MOUNT
    // ========================================================

    pub fn mount(mut device: D) -> Result<Self, Fat32Error> {
        if device.sector_size() != SECTOR_SIZE as u32 {
            return Err(Fat32Error::UnsupportedSectorSize);
        }

        let mut sector0 = [0u8; SECTOR_SIZE];
        device.read_sector(0, &mut sector0)?;

        if let Ok(mbr) = partition::parse_mbr(&sector0, device.sector_count()) {
            for index in 0..mbr.partitions.len() {
                let partition = mbr.partitions[index];
                if partition.status != PartitionStatus::Valid {
                    continue;
                }
                if partition::is_extended_partition(partition.partition_type)
                    || partition.partition_type == 0xEE
                {
                    continue;
                }

                let mut boot_sector = [0u8; SECTOR_SIZE];
                if device
                    .read_sector(partition.start_lba as u64, &mut boot_sector)
                    .is_err()
                {
                    continue;
                }

                if let Ok(bpb) =
                    partition::parse_fat_bpb(&boot_sector, partition.sector_count as u64)
                {
                    if bpb.fat_type == FatType::Fat32 {
                        return Self::finish_mount(device, partition, bpb);
                    }
                }
            }
        }

        // Fallback: unpartitioned FAT32 volume starting at LBA 0.
        if let Ok(bpb) = partition::parse_fat_bpb(&sector0, device.sector_count()) {
            if bpb.fat_type == FatType::Fat32 {
                let sector_count =
                    u32::try_from(device.sector_count()).unwrap_or(u32::MAX);
                let partition = MbrPartition {
                    index: 1,
                    bootable: false,
                    partition_type: 0x0C,
                    start_lba: 0,
                    sector_count,
                    end_lba_exclusive: sector_count as u64,
                    status: PartitionStatus::Valid,
                };
                return Self::finish_mount(device, partition, bpb);
            }
        }

        Err(Fat32Error::NoFat32Partition)
    }

    fn finish_mount(
        mut device: D,
        partition: MbrPartition,
        bpb: FatBpb,
    ) -> Result<Self, Fat32Error> {
        let data_start_sector = bpb.first_data_sector;
        let mut fs_info_valid = false;
        let mut free_cluster_hint = None;
        let mut next_free_hint = 2u32;

        if bpb.fs_info_sector > 0 && bpb.fs_info_sector < bpb.reserved_sectors {
            let abs_lba = partition.start_lba as u64 + bpb.fs_info_sector as u64;
            let mut fs_info = [0u8; SECTOR_SIZE];
            if device.read_sector(abs_lba, &mut fs_info).is_ok() {
                let lead = read_u32_le(&fs_info, 0);
                let struct_sig = read_u32_le(&fs_info, 484);
                let trail = read_u32_le(&fs_info, 508);

                if lead == FSINFO_LEAD_SIG
                    && struct_sig == FSINFO_STRUCT_SIG
                    && trail == FSINFO_TRAIL_SIG
                {
                    fs_info_valid = true;

                    let free_count = read_u32_le(&fs_info, 488);
                    if free_count != 0xFFFF_FFFF && free_count <= bpb.cluster_count {
                        free_cluster_hint = Some(free_count);
                    }

                    let next_free = read_u32_le(&fs_info, 492);
                    if next_free >= 2 && next_free < bpb.cluster_count + 2 {
                        next_free_hint = next_free;
                    }
                }
            }
        }

        Ok(Self {
            device,
            partition,
            bpb,
            data_start_sector,
            fs_info_valid,
            free_cluster_hint,
            next_free_hint,
        })
    }

    pub fn device(&self) -> &D {
        &self.device
    }

    pub fn device_mut(&mut self) -> &mut D {
        &mut self.device
    }

    pub fn root_cluster(&self) -> u32 {
        self.bpb.root_cluster
    }

    pub fn max_cluster(&self) -> u32 {
        self.bpb.cluster_count.saturating_add(2)
    }

    pub fn cluster_size_bytes(&self) -> usize {
        self.bpb.sectors_per_cluster as usize * SECTOR_SIZE
    }

    pub fn flush(&mut self) -> Result<(), Fat32Error> {
        self.device.flush().map_err(Fat32Error::from)
    }

    // ========================================================
    // PARTITION SECTOR ACCESS
    // ========================================================

    fn read_partition_sectors(
        &mut self,
        start_relative_lba: u32,
        sector_count: usize,
        buffer: &mut [u8],
    ) -> Result<(), Fat32Error> {
        if sector_count == 0 {
            return Ok(());
        }
        let end_rel = (start_relative_lba as u64)
            .checked_add(sector_count as u64)
            .ok_or(Fat32Error::InvalidPartition)?;
        if end_rel > self.bpb.total_sectors as u64 {
            return Err(Fat32Error::InvalidPartition);
        }

        let absolute_lba = self.partition.start_lba as u64 + start_relative_lba as u64;
        let abs_end = absolute_lba
            .checked_add(sector_count as u64)
            .ok_or(Fat32Error::InvalidPartition)?;
        if abs_end > self.device.sector_count() {
            return Err(Fat32Error::InvalidPartition);
        }

        self.device
            .read_sectors(absolute_lba, sector_count, buffer)?;
        Ok(())
    }

    fn write_partition_sectors(
        &mut self,
        start_relative_lba: u32,
        sector_count: usize,
        buffer: &[u8],
    ) -> Result<(), Fat32Error> {
        if sector_count == 0 {
            return Ok(());
        }
        let end_rel = (start_relative_lba as u64)
            .checked_add(sector_count as u64)
            .ok_or(Fat32Error::InvalidPartition)?;
        if end_rel > self.bpb.total_sectors as u64 {
            return Err(Fat32Error::InvalidPartition);
        }

        let absolute_lba = self.partition.start_lba as u64 + start_relative_lba as u64;
        let abs_end = absolute_lba
            .checked_add(sector_count as u64)
            .ok_or(Fat32Error::InvalidPartition)?;
        if abs_end > self.device.sector_count() {
            return Err(Fat32Error::InvalidPartition);
        }

        self.device
            .write_sectors(absolute_lba, sector_count, buffer)?;
        Ok(())
    }

    fn read_partition_sector(
        &mut self,
        relative_lba: u32,
        buffer: &mut [u8; SECTOR_SIZE],
    ) -> Result<(), Fat32Error> {
        self.read_partition_sectors(relative_lba, 1, buffer)
    }

    fn write_partition_sector(
        &mut self,
        relative_lba: u32,
        buffer: &[u8; SECTOR_SIZE],
    ) -> Result<(), Fat32Error> {
        self.write_partition_sectors(relative_lba, 1, buffer)
    }

    fn sync_fs_info(&mut self) -> Result<(), Fat32Error> {
        if !self.fs_info_valid {
            return Ok(());
        }

        let rel_lba = self.bpb.fs_info_sector as u32;
        let mut fs_info = [0u8; SECTOR_SIZE];
        self.read_partition_sector(rel_lba, &mut fs_info)?;

        let free_val = self.free_cluster_hint.unwrap_or(0xFFFF_FFFF);
        write_u32_le(&mut fs_info, 488, free_val);
        write_u32_le(&mut fs_info, 492, self.next_free_hint);

        self.write_partition_sector(rel_lba, &fs_info)
    }

    // ========================================================
    // FAT32 CLUSTER TABLE OPERATIONS
    // ========================================================

    fn validate_cluster(&self, cluster: u32) -> Result<(), Fat32Error> {
        if cluster < 2 || cluster >= self.max_cluster() {
            return Err(Fat32Error::InvalidCluster);
        }
        Ok(())
    }

    pub fn read_fat_entry(&mut self, cluster: u32) -> Result<u32, Fat32Error> {
        self.validate_cluster(cluster)?;

        let byte_offset = cluster as u64 * 4;
        let sector_offset = (byte_offset / SECTOR_SIZE as u64) as u32;
        let entry_offset = (byte_offset % SECTOR_SIZE as u64) as usize;

        if sector_offset >= self.bpb.fat_size_sectors {
            return Err(Fat32Error::InvalidFatEntry);
        }

        let relative_lba = self
            .bpb
            .reserved_sectors as u32
            + sector_offset;

        let mut sector = [0u8; SECTOR_SIZE];
        self.read_partition_sector(relative_lba, &mut sector)?;

        Ok(read_u32_le(&sector, entry_offset) & FAT32_MASK)
    }

    pub fn write_fat_entry(
        &mut self,
        cluster: u32,
        value: u32,
    ) -> Result<(), Fat32Error> {
        self.validate_cluster(cluster)?;

        let byte_offset = cluster as u64 * 4;
        let sector_offset = (byte_offset / SECTOR_SIZE as u64) as u32;
        let entry_offset = (byte_offset % SECTOR_SIZE as u64) as usize;

        if sector_offset >= self.bpb.fat_size_sectors {
            return Err(Fat32Error::InvalidFatEntry);
        }

        let masked = value & FAT32_MASK;
        let mut sector = [0u8; SECTOR_SIZE];

        for fat_idx in 0..self.bpb.fat_count {
            let fat_base = self.bpb.reserved_sectors as u32
                + (fat_idx as u32) * self.bpb.fat_size_sectors;
            let relative_lba = fat_base + sector_offset;

            self.read_partition_sector(relative_lba, &mut sector)?;
            let existing = read_u32_le(&sector, entry_offset);
            let updated = (existing & 0xF000_0000) | masked;
            write_u32_le(&mut sector, entry_offset, updated);
            self.write_partition_sector(relative_lba, &sector)?;
        }

        Ok(())
    }

    pub fn next_cluster(&mut self, cluster: u32) -> Result<Option<u32>, Fat32Error> {
        let entry = self.read_fat_entry(cluster)?;

        if entry == FAT32_FREE || entry == 1 || entry == FAT32_BAD_CLUSTER {
            return Err(Fat32Error::InvalidFatEntry);
        }

        if entry >= FAT32_EOC_MIN {
            return Ok(None);
        }

        self.validate_cluster(entry)?;
        Ok(Some(entry))
    }

    pub fn free_clusters_count(&mut self) -> Result<u32, Fat32Error> {
        if let Some(cached) = self.free_cluster_hint {
            return Ok(cached);
        }

        let mut free = 0u32;
        let max_cluster = self.max_cluster();
        let mut cluster = 2u32;
        let mut sector = [0u8; SECTOR_SIZE];

        while cluster < max_cluster {
            let fat_sector_idx = (cluster as usize * 4) / SECTOR_SIZE;
            if fat_sector_idx as u32 >= self.bpb.fat_size_sectors {
                break;
            }

            let rel_lba = self.bpb.reserved_sectors as u32 + fat_sector_idx as u32;
            self.read_partition_sector(rel_lba, &mut sector)?;

            let start_in_sector = (cluster as usize) % FAT32_ENTRIES_PER_SECTOR;
            for idx in start_in_sector..FAT32_ENTRIES_PER_SECTOR {
                if cluster >= max_cluster {
                    break;
                }
                let val = read_u32_le(&sector, idx * 4) & FAT32_MASK;
                if val == FAT32_FREE {
                    free = free.saturating_add(1);
                }
                cluster += 1;
            }
        }

        self.free_cluster_hint = Some(free);
        Ok(free)
    }

    fn allocate_cluster(&mut self, zero_init: bool) -> Result<u32, Fat32Error> {
        let max_cluster = self.max_cluster();
        if max_cluster <= 2 {
            return Err(Fat32Error::NoFreeClusters);
        }

        let start = if self.next_free_hint >= 2 && self.next_free_hint < max_cluster {
            self.next_free_hint
        } else {
            2
        };

        let found = self
            .scan_free_cluster_range(start, max_cluster)?
            .or(self.scan_free_cluster_range(2, start)?)
            .ok_or(Fat32Error::NoFreeClusters)?;

        self.write_fat_entry(found, FAT32_EOC_MARK)?;

        self.next_free_hint = if found + 1 < max_cluster {
            found + 1
        } else {
            2
        };

        if let Some(count) = self.free_cluster_hint.as_mut() {
            *count = count.saturating_sub(1);
        }

        let _ = self.sync_fs_info();

        if zero_init {
            self.zero_cluster(found)?;
        }

        Ok(found)
    }

    fn scan_free_cluster_range(
        &mut self,
        start: u32,
        end_exclusive: u32,
    ) -> Result<Option<u32>, Fat32Error> {
        if start >= end_exclusive {
            return Ok(None);
        }

        let mut cluster = start;
        let mut sector = [0u8; SECTOR_SIZE];

        while cluster < end_exclusive {
            let fat_sector_idx = (cluster as usize * 4) / SECTOR_SIZE;
            if fat_sector_idx as u32 >= self.bpb.fat_size_sectors {
                break;
            }

            let rel_lba = self.bpb.reserved_sectors as u32 + fat_sector_idx as u32;
            self.read_partition_sector(rel_lba, &mut sector)?;

            let start_in_sector = (cluster as usize) % FAT32_ENTRIES_PER_SECTOR;
            for idx in start_in_sector..FAT32_ENTRIES_PER_SECTOR {
                if cluster >= end_exclusive {
                    break;
                }
                let val = read_u32_le(&sector, idx * 4) & FAT32_MASK;
                if val == FAT32_FREE {
                    return Ok(Some(cluster));
                }
                cluster += 1;
            }
        }

        Ok(None)
    }

    fn free_chain(&mut self, start_cluster: u32) -> Result<(), Fat32Error> {
        if start_cluster < 2 {
            return Ok(());
        }

        let mut current = start_cluster;
        let mut steps = 0u32;

        while steps < self.bpb.cluster_count {
            if current < 2 || current >= self.max_cluster() {
                break;
            }

            let next = self.read_fat_entry(current)?;
            self.write_fat_entry(current, FAT32_FREE)?;

            if let Some(count) = self.free_cluster_hint.as_mut() {
                *count = count.saturating_add(1);
            }
            if current < self.next_free_hint {
                self.next_free_hint = current;
            }

            if next == FAT32_FREE || next == FAT32_BAD_CLUSTER || next >= FAT32_EOC_MIN {
                break;
            }

            current = next;
            steps += 1;
        }

        let _ = self.sync_fs_info();
        Ok(())
    }

    fn cluster_sector(
        &self,
        cluster: u32,
        sector_index: u8,
    ) -> Result<u32, Fat32Error> {
        self.validate_cluster(cluster)?;

        if sector_index >= self.bpb.sectors_per_cluster {
            return Err(Fat32Error::InvalidCluster);
        }

        let cluster_offset =
            (cluster as u64 - 2) * self.bpb.sectors_per_cluster as u64;
        let relative_lba =
            self.data_start_sector as u64 + cluster_offset + sector_index as u64;

        if relative_lba >= self.bpb.total_sectors as u64 || relative_lba > u32::MAX as u64 {
            return Err(Fat32Error::InvalidPartition);
        }

        Ok(relative_lba as u32)
    }

    fn zero_cluster(&mut self, cluster: u32) -> Result<(), Fat32Error> {
        let spc = self.bpb.sectors_per_cluster as usize;
        if spc == 0 {
            return Ok(());
        }
        let start_lba = self.cluster_sector(cluster, 0)?;
        let zeros = alloc::vec![0u8; spc * SECTOR_SIZE];
        self.write_partition_sectors(start_lba, spc, &zeros)
    }

    fn collect_cluster_chain(&mut self, start_cluster: u32) -> Result<Vec<u32>, Fat32Error> {
        self.validate_cluster(start_cluster)?;
        let mut chain = Vec::new();
        let mut current = Some(start_cluster);

        while let Some(cluster) = current {
            if chain.len() as u32 >= self.bpb.cluster_count {
                return Err(Fat32Error::InvalidCluster);
            }
            chain.push(cluster);
            current = self.next_cluster(cluster)?;
        }

        Ok(chain)
    }

    fn slot_to_location(
        &self,
        chain: &[u32],
        slot: usize,
    ) -> Option<SlotLocation> {
        let slots_per_cluster =
            self.bpb.sectors_per_cluster as usize * ENTRIES_PER_SECTOR;
        let cluster_idx = slot / slots_per_cluster;
        if cluster_idx >= chain.len() {
            return None;
        }
        let rem = slot % slots_per_cluster;
        let sector_in_cluster = (rem / ENTRIES_PER_SECTOR) as u8;
        let entry_in_sector = (rem % ENTRIES_PER_SECTOR) as u8;

        Some(SlotLocation {
            cluster: chain[cluster_idx],
            sector_in_cluster,
            entry_in_sector,
        })
    }

    fn read_slot_raw(&mut self, loc: SlotLocation) -> Result<[u8; DIR_ENTRY_SIZE], Fat32Error> {
        let rel_lba = self.cluster_sector(loc.cluster, loc.sector_in_cluster)?;
        let mut sector = [0u8; SECTOR_SIZE];
        self.read_partition_sector(rel_lba, &mut sector)?;

        let offset = loc.entry_in_sector as usize * DIR_ENTRY_SIZE;
        let mut raw = [0u8; DIR_ENTRY_SIZE];
        raw.copy_from_slice(&sector[offset..offset + DIR_ENTRY_SIZE]);
        Ok(raw)
    }

    fn write_slot_raw(
        &mut self,
        loc: SlotLocation,
        raw: &[u8; DIR_ENTRY_SIZE],
    ) -> Result<(), Fat32Error> {
        let rel_lba = self.cluster_sector(loc.cluster, loc.sector_in_cluster)?;
        let mut sector = [0u8; SECTOR_SIZE];
        self.read_partition_sector(rel_lba, &mut sector)?;

        let offset = loc.entry_in_sector as usize * DIR_ENTRY_SIZE;
        sector[offset..offset + DIR_ENTRY_SIZE].copy_from_slice(raw);
        self.write_partition_sector(rel_lba, &sector)
    }

    // ========================================================
    // DIRECTORY READING (WITH FULL LFN DECODING)
    // ========================================================

    pub fn list_dir(&mut self, dir_cluster: u32) -> Result<Vec<Fat32DirEntry>, Fat32Error> {
        let target_cluster = if dir_cluster == 0 {
            self.bpb.root_cluster
        } else {
            dir_cluster
        };

        let chain = self.collect_cluster_chain(target_cluster)?;
        let mut entries = Vec::new();

        let mut lfn_utf16 = [0u16; 260];
        let mut lfn_valid = false;
        let mut lfn_expected_seq = 0u8;
        let mut lfn_checksum = 0u8;
        let mut lfn_count = 0usize;
        let mut lfn_max_slot = 0usize;

        let mut slot_index = 0usize;
        let spc = self.bpb.sectors_per_cluster as usize;
        let mut cluster_buf = alloc::vec![0u8; spc * SECTOR_SIZE];

        'outer: for &cluster in &chain {
            let cluster_lba = self.cluster_sector(cluster, 0)?;
            self.read_partition_sectors(cluster_lba, spc, &mut cluster_buf)?;

            for sector_idx in 0..spc {
                let sec_off = sector_idx * SECTOR_SIZE;
                let sector = &cluster_buf[sec_off..sec_off + SECTOR_SIZE];

                for entry_idx in 0..ENTRIES_PER_SECTOR {
                    let offset = entry_idx * DIR_ENTRY_SIZE;
                    let raw = &sector[offset..offset + DIR_ENTRY_SIZE];
                    let first = raw[0];

                    if first == 0x00 {
                        break 'outer;
                    }

                    if first == 0xE5 {
                        lfn_valid = false;
                        lfn_count = 0;
                        slot_index += 1;
                        continue;
                    }

                    let attr = raw[11];

                    if attr == ATTR_LFN {
                        let ord = raw[0];
                        let seq = ord & 0x1F;
                        let is_last = (ord & 0x40) != 0;
                        let checksum = raw[13];

                        if seq == 0 || seq > 20 || raw[12] != 0x00 {
                            lfn_valid = false;
                            lfn_count = 0;
                            slot_index += 1;
                            continue;
                        }

                        if is_last {
                            lfn_utf16 = [0u16; 260];
                            lfn_valid = true;
                            lfn_checksum = checksum;
                            lfn_count = 1;
                            lfn_max_slot = seq as usize * 13;
                        } else if !lfn_valid || seq != lfn_expected_seq || checksum != lfn_checksum {
                            lfn_valid = false;
                            lfn_count = 0;
                            slot_index += 1;
                            continue;
                        } else {
                            lfn_count += 1;
                        }

                        let base = (seq as usize - 1) * 13;
                        extract_lfn_chars(raw, &mut lfn_utf16[base..base + 13]);
                        lfn_expected_seq = seq - 1;
                        slot_index += 1;
                        continue;
                    }

                    if attr & ATTR_VOLUME_ID != 0 {
                        lfn_valid = false;
                        lfn_count = 0;
                        slot_index += 1;
                        continue;
                    }

                    let mut short_name = [0u8; 11];
                    short_name.copy_from_slice(&raw[0..11]);

                    let checksum = sfn_checksum(&short_name);
                    let (name, used_lfn_count) =
                        if lfn_valid && lfn_expected_seq == 0 && lfn_checksum == checksum {
                            let decoded = decode_utf16_lfn(&lfn_utf16[..lfn_max_slot]);
                            if !decoded.is_empty() {
                                (decoded, lfn_count)
                            } else {
                                (format_short_name(&short_name, raw[12]), 0)
                            }
                        } else {
                            (format_short_name(&short_name, raw[12]), 0)
                        };

                    lfn_valid = false;
                    lfn_count = 0;

                    let hi = read_u16_le(raw, 20) as u32;
                    let lo = read_u16_le(raw, 26) as u32;
                    let first_cluster = (hi << 16) | lo;
                    let size = read_u32_le(raw, 28);

                    entries.push(Fat32DirEntry {
                        name,
                        short_name,
                        attributes: attr,
                        first_cluster,
                        size,
                        dir_cluster: target_cluster,
                        entry_slot: slot_index,
                        lfn_count: used_lfn_count,
                    });

                    slot_index += 1;
                }
            }
        }

        Ok(entries)
    }

    pub fn find_entry(
        &mut self,
        dir_cluster: u32,
        name: &str,
    ) -> Result<Option<Fat32DirEntry>, Fat32Error> {
        let entries = self.list_dir(dir_cluster)?;
        for entry in entries {
            if eq_ignore_ascii_case(&entry.name, name) {
                return Ok(Some(entry));
            }
        }
        Ok(None)
    }

    // ========================================================
    // FILE READING
    // ========================================================

    pub fn read_file_range(
        &mut self,
        file: &Fat32DirEntry,
        offset: u32,
        output: &mut [u8],
    ) -> Result<usize, Fat32Error> {
        if file.is_directory() {
            return Err(Fat32Error::IsADirectory);
        }

        if offset > file.size {
            return Err(Fat32Error::OffsetOutOfRange);
        }

        if output.is_empty() || offset == file.size || file.size == 0 {
            return Ok(0);
        }

        if file.first_cluster < 2 {
            return Err(Fat32Error::InvalidCluster);
        }

        let available = (file.size - offset) as usize;
        let target_length = core::cmp::min(output.len(), available);
        let cluster_bytes = self.bpb.sectors_per_cluster as u64 * SECTOR_SIZE as u64;

        if cluster_bytes == 0 {
            return Err(Fat32Error::InvalidFilesystem);
        }

        let mut current_cluster = file.first_cluster;
        let mut clusters_to_skip = offset as u64 / cluster_bytes;
        let mut intra_cluster_offset = offset as u64 % cluster_bytes;
        let mut traversed = 1u32;

        while clusters_to_skip > 0 {
            if traversed >= self.bpb.cluster_count {
                return Err(Fat32Error::InvalidCluster);
            }
            current_cluster = self
                .next_cluster(current_cluster)?
                .ok_or(Fat32Error::UnexpectedEndOfChain)?;
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
                let rel_lba = self.cluster_sector(current_cluster, first_sector as u8)?;
                self.read_partition_sector(rel_lba, &mut sector)?;

                let remaining = target_length - written;
                let count = core::cmp::min(SECTOR_SIZE - byte_in_sector, remaining);
                output[written..written + count]
                    .copy_from_slice(&sector[byte_in_sector..byte_in_sector + count]);
                written += count;
                intra_cluster_offset += count as u64;

                if written >= target_length {
                    break;
                }
                if intra_cluster_offset >= cluster_bytes {
                    if traversed >= self.bpb.cluster_count {
                        return Err(Fat32Error::InvalidCluster);
                    }
                    current_cluster = self
                        .next_cluster(current_cluster)?
                        .ok_or(Fat32Error::UnexpectedEndOfChain)?;
                    traversed += 1;
                    intra_cluster_offset = 0;
                }
                continue;
            }

            let rem_bytes = target_length - written;
            let needed_sectors = (rem_bytes + SECTOR_SIZE - 1) / SECTOR_SIZE;
            let rem_in_cur = spc.saturating_sub(first_sector);
            if rem_in_cur == 0 {
                return Err(Fat32Error::InvalidCluster);
            }

            let start_rel_lba = self.cluster_sector(current_cluster, first_sector as u8)?;
            let mut batch_sectors = core::cmp::min(rem_in_cur, needed_sectors).min(128);
            let mut tail_cluster = current_cluster;
            let mut reached_cluster_end = first_sector + batch_sectors == spc;
            let mut prefetched_next: Option<u32> = None;

            if reached_cluster_end && needed_sectors > batch_sectors && batch_sectors + spc <= 128 {
                while batch_sectors < needed_sectors && batch_sectors + spc <= 128 {
                    if traversed >= self.bpb.cluster_count {
                        return Err(Fat32Error::InvalidCluster);
                    }
                    let next_opt = self.next_cluster(tail_cluster)?;
                    let Some(next_c) = next_opt else {
                        return Err(Fat32Error::UnexpectedEndOfChain);
                    };
                    if next_c == tail_cluster + 1 {
                        tail_cluster = next_c;
                        traversed += 1;
                        let take = core::cmp::min(spc, needed_sectors - batch_sectors);
                        batch_sectors += take;
                        reached_cluster_end = take == spc;
                    } else {
                        prefetched_next = Some(next_c);
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
                        return Err(Fat32Error::InvalidCluster);
                    }
                    current_cluster = self
                        .next_cluster(tail_cluster)?
                        .ok_or(Fat32Error::UnexpectedEndOfChain)?;
                    traversed += 1;
                }
            }
        }

        Ok(written)
    }

    pub fn read_file(&mut self, file: &Fat32DirEntry) -> Result<Vec<u8>, Fat32Error> {
        if file.is_directory() {
            return Err(Fat32Error::IsADirectory);
        }

        let mut data = Vec::new();
        if file.size == 0 {
            return Ok(data);
        }

        data.resize(file.size as usize, 0);
        let read = self.read_file_range(file, 0, &mut data)?;
        data.truncate(read);
        Ok(data)
    }

    // ========================================================
    // WRITING FILES, MKDIR & REMOVE
    // ========================================================

    fn write_data_clusters(&mut self, data: &[u8]) -> Result<u32, Fat32Error> {
        if data.is_empty() {
            return Ok(0);
        }

        let spc = self.bpb.sectors_per_cluster as usize;
        let cluster_bytes = self.cluster_size_bytes();
        let needed_clusters = (data.len() + cluster_bytes - 1) / cluster_bytes;

        let mut first_cluster = 0u32;
        let mut prev_cluster = 0u32;
        let mut written = 0usize;
        let mut cluster_buf = alloc::vec![0u8; cluster_bytes];

        for _ in 0..needed_clusters {
            let new_cluster = match self.allocate_cluster(false) {
                Ok(c) => c,
                Err(err) => {
                    if first_cluster >= 2 {
                        let _ = self.free_chain(first_cluster);
                    }
                    return Err(err);
                }
            };

            if first_cluster == 0 {
                first_cluster = new_cluster;
            } else if let Err(err) = self.write_fat_entry(prev_cluster, new_cluster) {
                let _ = self.free_chain(first_cluster);
                return Err(err);
            }
            prev_cluster = new_cluster;

            cluster_buf.fill(0);
            if written < data.len() {
                let count = core::cmp::min(cluster_bytes, data.len() - written);
                cluster_buf[..count].copy_from_slice(&data[written..written + count]);
                written += count;
            }
            let start_lba = self.cluster_sector(new_cluster, 0)?;
            if let Err(err) = self.write_partition_sectors(start_lba, spc, &cluster_buf) {
                let _ = self.free_chain(first_cluster);
                return Err(err);
            }
        }

        self.device.flush()?;
        Ok(first_cluster)
    }

    pub fn write_file(
        &mut self,
        dir_cluster: u32,
        name: &str,
        data: &[u8],
    ) -> Result<Fat32DirEntry, Fat32Error> {
        validate_entry_name(name)?;

        let target_dir = if dir_cluster == 0 {
            self.bpb.root_cluster
        } else {
            dir_cluster
        };

        if let Some(existing) = self.find_entry(target_dir, name)? {
            if existing.is_directory() {
                return Err(Fat32Error::IsADirectory);
            }
            if existing.is_read_only() {
                return Err(Fat32Error::ReadOnlyEntry);
            }

            let new_first_cluster = self.write_data_clusters(data)?;

            if existing.first_cluster >= 2 {
                self.free_chain(existing.first_cluster)?;
            }

            let chain = self.collect_cluster_chain(target_dir)?;
            let loc = self
                .slot_to_location(&chain, existing.entry_slot)
                .ok_or(Fat32Error::InvalidFilesystem)?;
            let mut raw = self.read_slot_raw(loc)?;

            write_u16_le(&mut raw, 20, ((new_first_cluster >> 16) & 0xFFFF) as u16);
            write_u16_le(&mut raw, 26, (new_first_cluster & 0xFFFF) as u16);
            write_u32_le(&mut raw, 28, data.len() as u32);
            raw[11] |= ATTR_ARCHIVE;

            self.write_slot_raw(loc, &raw)?;
            self.device.flush()?;

            return Ok(Fat32DirEntry {
                first_cluster: new_first_cluster,
                size: data.len() as u32,
                attributes: raw[11],
                ..existing
            });
        }

        let first_cluster = self.write_data_clusters(data)?;
        match self.insert_dir_entry(target_dir, name, ATTR_ARCHIVE, first_cluster, data.len() as u32)
        {
            Ok(entry) => {
                self.device.flush()?;
                Ok(entry)
            }
            Err(err) => {
                if first_cluster >= 2 {
                    let _ = self.free_chain(first_cluster);
                }
                Err(err)
            }
        }
    }

    pub fn create_dir(
        &mut self,
        parent_cluster: u32,
        name: &str,
    ) -> Result<Fat32DirEntry, Fat32Error> {
        validate_entry_name(name)?;

        let parent = if parent_cluster == 0 {
            self.bpb.root_cluster
        } else {
            parent_cluster
        };

        if self.find_entry(parent, name)?.is_some() {
            return Err(Fat32Error::AlreadyExists);
        }

        let new_cluster = self.allocate_cluster(true)?;

        // Write "." and ".." entries in sector 0 of new directory cluster.
        let mut sector = [0u8; SECTOR_SIZE];
        let mut dot_name = [b' '; 11];
        dot_name[0] = b'.';
        build_sfn_raw(&mut sector[0..32], &dot_name, ATTR_DIRECTORY, new_cluster, 0);

        let mut dotdot_name = [b' '; 11];
        dotdot_name[0] = b'.';
        dotdot_name[1] = b'.';
        let parent_for_dotdot = if parent == self.bpb.root_cluster {
            0
        } else {
            parent
        };
        build_sfn_raw(
            &mut sector[32..64],
            &dotdot_name,
            ATTR_DIRECTORY,
            parent_for_dotdot,
            0,
        );

        let first_lba = self.cluster_sector(new_cluster, 0)?;
        if let Err(err) = self.write_partition_sector(first_lba, &sector) {
            let _ = self.free_chain(new_cluster);
            return Err(err);
        }

        match self.insert_dir_entry(parent, name, ATTR_DIRECTORY, new_cluster, 0) {
            Ok(entry) => {
                self.device.flush()?;
                Ok(entry)
            }
            Err(err) => {
                let _ = self.free_chain(new_cluster);
                Err(err)
            }
        }
    }

    pub fn remove_entry(
        &mut self,
        dir_cluster: u32,
        name: &str,
    ) -> Result<(), Fat32Error> {
        if name == "." || name == ".." {
            return Err(Fat32Error::InvalidFileName);
        }

        let target_dir = if dir_cluster == 0 {
            self.bpb.root_cluster
        } else {
            dir_cluster
        };

        let entry = self
            .find_entry(target_dir, name)?
            .ok_or(Fat32Error::FileNotFound)?;

        if entry.is_read_only() || entry.is_system() {
            return Err(Fat32Error::ReadOnlyEntry);
        }

        if entry.is_directory() {
            let children = self.list_dir(entry.first_cluster)?;
            let has_children = children
                .iter()
                .any(|child| child.name != "." && child.name != "..");
            if has_children {
                return Err(Fat32Error::DirectoryNotEmpty);
            }
        }

        let chain = self.collect_cluster_chain(target_dir)?;
        let start_slot = entry.entry_slot.saturating_sub(entry.lfn_count);

        for slot in start_slot..=entry.entry_slot {
            if let Some(loc) = self.slot_to_location(&chain, slot) {
                let mut raw = self.read_slot_raw(loc)?;
                raw[0] = 0xE5;
                self.write_slot_raw(loc, &raw)?;
            }
        }

        if entry.first_cluster >= 2 {
            self.free_chain(entry.first_cluster)?;
        }

        self.device.flush()?;
        Ok(())
    }

    // ========================================================
    // DIRECTORY SLOT ALLOCATION & LFN + 8.3 CREATION
    // ========================================================

    fn insert_dir_entry(
        &mut self,
        dir_cluster: u32,
        name: &str,
        attributes: u8,
        first_cluster: u32,
        size: u32,
    ) -> Result<Fat32DirEntry, Fat32Error> {
        let existing_entries = self.list_dir(dir_cluster)?;

        let utf16: Vec<u16> = name.encode_utf16().collect();
        if utf16.is_empty() || utf16.len() > 255 {
            return Err(Fat32Error::InvalidFileName);
        }

        let (short_name, needs_lfn) = generate_unique_short_name(name, &existing_entries);
        let lfn_count = if needs_lfn {
            (utf16.len() + 12) / 13
        } else {
            0
        };

        let total_slots_needed = lfn_count + 1;
        let start_slot = self.find_or_extend_dir_slots(dir_cluster, total_slots_needed)?;
        let chain = self.collect_cluster_chain(dir_cluster)?;

        let checksum = sfn_checksum(&short_name);

        for lfn_idx in 0..lfn_count {
            let order_num = lfn_count - lfn_idx; // N down to 1
            let mut lfn_raw = [0u8; DIR_ENTRY_SIZE];
            let ord_byte = (order_num as u8) | if lfn_idx == 0 { 0x40 } else { 0x00 };
            let char_start = (order_num - 1) * 13;

            build_lfn_raw(&mut lfn_raw, ord_byte, checksum, &utf16, char_start);

            let loc = self
                .slot_to_location(&chain, start_slot + lfn_idx)
                .ok_or(Fat32Error::InvalidFilesystem)?;
            self.write_slot_raw(loc, &lfn_raw)?;
        }

        let mut sfn_raw = [0u8; DIR_ENTRY_SIZE];
        build_sfn_raw(&mut sfn_raw, &short_name, attributes, first_cluster, size);

        let sfn_slot = start_slot + lfn_count;
        let sfn_loc = self
            .slot_to_location(&chain, sfn_slot)
            .ok_or(Fat32Error::InvalidFilesystem)?;
        self.write_slot_raw(sfn_loc, &sfn_raw)?;

        Ok(Fat32DirEntry {
            name: String::from(name),
            short_name,
            attributes,
            first_cluster,
            size,
            dir_cluster,
            entry_slot: sfn_slot,
            lfn_count,
        })
    }

    fn find_or_extend_dir_slots(
        &mut self,
        dir_cluster: u32,
        needed: usize,
    ) -> Result<usize, Fat32Error> {
        let chain = self.collect_cluster_chain(dir_cluster)?;
        let slots_per_cluster =
            self.bpb.sectors_per_cluster as usize * ENTRIES_PER_SECTOR;
        let total_slots = chain.len() * slots_per_cluster;

        let mut run_start = 0usize;
        let mut run_len = 0usize;
        let spc = self.bpb.sectors_per_cluster as usize;
        let mut cluster_buf = alloc::vec![0u8; spc * SECTOR_SIZE];
        let mut slot_index = 0usize;

        for &cluster in &chain {
            let cluster_lba = self.cluster_sector(cluster, 0)?;
            self.read_partition_sectors(cluster_lba, spc, &mut cluster_buf)?;

            for sector_idx in 0..spc {
                let sec_off = sector_idx * SECTOR_SIZE;
                let sector = &cluster_buf[sec_off..sec_off + SECTOR_SIZE];

                for entry_idx in 0..ENTRIES_PER_SECTOR {
                    let first = sector[entry_idx * DIR_ENTRY_SIZE];

                    if first == 0x00 {
                        // All remaining slots from here to end of chain are free.
                        let remaining_in_chain = total_slots - slot_index;
                        if run_len + remaining_in_chain >= needed {
                            return Ok(if run_len > 0 { run_start } else { slot_index });
                        } else {
                            // Extend directory chain by allocating one cluster.
                            let last_cluster = *chain.last().ok_or(Fat32Error::InvalidCluster)?;
                            let new_cluster = self.allocate_cluster(true)?;
                            self.write_fat_entry(last_cluster, new_cluster)?;
                            return Ok(if run_len > 0 { run_start } else { slot_index });
                        }
                    } else if first == 0xE5 {
                        if run_len == 0 {
                            run_start = slot_index;
                        }
                        run_len += 1;
                        if run_len >= needed {
                            return Ok(run_start);
                        }
                    } else {
                        run_len = 0;
                    }

                    slot_index += 1;
                }
            }
        }

        // Current cluster chain had no trailing 0x00 and no run of `needed` 0xE5 slots.
        let last_cluster = *chain.last().ok_or(Fat32Error::InvalidCluster)?;
        let new_cluster = self.allocate_cluster(true)?;
        self.write_fat_entry(last_cluster, new_cluster)?;

        Ok(if run_len > 0 { run_start } else { total_slots })
    }
}

// ============================================================
// LFN & 8.3 HELPERS
// ============================================================

fn extract_lfn_chars(raw: &[u8], out: &mut [u16]) {
    const OFFSETS: [usize; 13] = [1, 3, 5, 7, 9, 14, 16, 18, 20, 22, 24, 28, 30];
    for (idx, &offset) in OFFSETS.iter().enumerate() {
        out[idx] = (raw[offset] as u16) | ((raw[offset + 1] as u16) << 8);
    }
}

fn decode_utf16_lfn(units: &[u16]) -> String {
    let mut len = 0usize;
    while len < units.len() && units[len] != 0x0000 && units[len] != 0xFFFF {
        len += 1;
    }

    let mut result = String::new();
    for ch in core::char::decode_utf16(units[..len].iter().copied()) {
        match ch {
            Ok(c) => result.push(c),
            Err(_) => result.push('?'),
        }
    }
    result
}

fn format_short_name(short: &[u8; 11], nt_flags: u8) -> String {
    let lower_base = (nt_flags & 0x08) != 0;
    let lower_ext = (nt_flags & 0x10) != 0;

    let mut base_end = 8;
    while base_end > 0 && short[base_end - 1] == b' ' {
        base_end -= 1;
    }

    let mut ext_end = 3;
    while ext_end > 0 && short[8 + ext_end - 1] == b' ' {
        ext_end -= 1;
    }

    let mut out = String::new();
    for &b in &short[..base_end] {
        let ch = if lower_base {
            b.to_ascii_lowercase()
        } else {
            b
        };
        out.push(ch as char);
    }

    if ext_end > 0 {
        out.push('.');
        for &b in &short[8..8 + ext_end] {
            let ch = if lower_ext {
                b.to_ascii_lowercase()
            } else {
                b
            };
            out.push(ch as char);
        }
    }

    out
}

fn sfn_checksum(short_name: &[u8; 11]) -> u8 {
    let mut sum: u8 = 0;
    for &byte in short_name {
        sum = sum.rotate_right(1).wrapping_add(byte);
    }
    sum
}

fn build_lfn_raw(
    raw: &mut [u8; DIR_ENTRY_SIZE],
    ord: u8,
    checksum: u8,
    utf16: &[u16],
    char_start: usize,
) {
    const OFFSETS: [usize; 13] = [1, 3, 5, 7, 9, 14, 16, 18, 20, 22, 24, 28, 30];

    *raw = [0u8; DIR_ENTRY_SIZE];
    raw[0] = ord;
    raw[11] = ATTR_LFN;
    raw[12] = 0x00;
    raw[13] = checksum;
    raw[26] = 0x00;
    raw[27] = 0x00;

    for (idx, &offset) in OFFSETS.iter().enumerate() {
        let pos = char_start + idx;
        let unit = if pos < utf16.len() {
            utf16[pos]
        } else if pos == utf16.len() {
            0x0000
        } else {
            0xFFFF
        };
        raw[offset] = unit as u8;
        raw[offset + 1] = (unit >> 8) as u8;
    }
}

fn build_sfn_raw(
    raw: &mut [u8],
    short_name: &[u8; 11],
    attributes: u8,
    first_cluster: u32,
    size: u32,
) {
    for b in raw.iter_mut().take(DIR_ENTRY_SIZE) {
        *b = 0;
    }
    raw[0..11].copy_from_slice(short_name);
    raw[11] = attributes;
    raw[20] = ((first_cluster >> 16) & 0xFF) as u8;
    raw[21] = ((first_cluster >> 24) & 0xFF) as u8;
    raw[26] = (first_cluster & 0xFF) as u8;
    raw[27] = ((first_cluster >> 8) & 0xFF) as u8;
    raw[28] = (size & 0xFF) as u8;
    raw[29] = ((size >> 8) & 0xFF) as u8;
    raw[30] = ((size >> 16) & 0xFF) as u8;
    raw[31] = ((size >> 24) & 0xFF) as u8;
}

fn is_valid_sfn_char(b: u8) -> bool {
    b.is_ascii_alphanumeric()
        || matches!(
            b,
            b'$' | b'%' | b'\'' | b'-' | b'_' | b'@' | b'~' | b'!' | b'(' | b')' | b'{'
                | b'}' | b'^' | b'#' | b'&'
        )
}

fn validate_entry_name(name: &str) -> Result<(), Fat32Error> {
    if name.is_empty() || name == "." || name == ".." {
        return Err(Fat32Error::InvalidFileName);
    }

    if name.starts_with(' ') || name.ends_with(' ') || name.ends_with('.') {
        return Err(Fat32Error::InvalidFileName);
    }

    for ch in name.chars() {
        if ch == '\0'
            || (ch as u32) < 0x20
            || matches!(ch, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|')
        {
            return Err(Fat32Error::InvalidFileName);
        }
    }

    Ok(())
}

fn generate_unique_short_name(
    name: &str,
    existing: &[Fat32DirEntry],
) -> ([u8; 11], bool) {
    // Check if strictly valid uppercase 8.3 without needing LFN.
    if let Some(exact) = try_exact_sfn(name) {
        return (exact, false);
    }

    let mut clean_stem = [0u8; 8];
    let mut clean_stem_len = 0usize;
    let mut clean_ext = [0u8; 3];
    let mut clean_ext_len = 0usize;

    let trimmed = name.trim_start_matches('.');
    let (stem_str, ext_str) = match trimmed.rfind('.') {
        Some(pos) => (&trimmed[..pos], &trimmed[pos + 1..]),
        None => (trimmed, ""),
    };

    for ch in stem_str.chars() {
        if clean_stem_len >= 8 {
            break;
        }
        if ch.is_ascii() {
            let up = (ch as u8).to_ascii_uppercase();
            if is_valid_sfn_char(up) {
                clean_stem[clean_stem_len] = up;
                clean_stem_len += 1;
            } else if ch != ' ' && ch != '.' {
                clean_stem[clean_stem_len] = b'_';
                clean_stem_len += 1;
            }
        } else {
            clean_stem[clean_stem_len] = b'_';
            clean_stem_len += 1;
        }
    }

    if clean_stem_len == 0 {
        clean_stem[0] = b'_';
        clean_stem_len = 1;
    }

    for ch in ext_str.chars() {
        if clean_ext_len >= 3 {
            break;
        }
        if ch.is_ascii() {
            let up = (ch as u8).to_ascii_uppercase();
            if is_valid_sfn_char(up) {
                clean_ext[clean_ext_len] = up;
                clean_ext_len += 1;
            } else if ch != ' ' && ch != '.' {
                clean_ext[clean_ext_len] = b'_';
                clean_ext_len += 1;
            }
        } else {
            clean_ext[clean_ext_len] = b'_';
            clean_ext_len += 1;
        }
    }

    for suffix_num in 1usize..=9999 {
        let mut sfn = [b' '; 11];
        let mut digits = [0u8; 5];
        let mut digit_count = 0usize;
        let mut n = suffix_num;
        while n > 0 {
            digits[digit_count] = b'0' + (n % 10) as u8;
            digit_count += 1;
            n /= 10;
        }

        let max_prefix = 8usize.saturating_sub(1 + digit_count);
        let prefix_len = core::cmp::min(clean_stem_len, max_prefix);

        sfn[..prefix_len].copy_from_slice(&clean_stem[..prefix_len]);
        sfn[prefix_len] = b'~';
        for i in 0..digit_count {
            sfn[prefix_len + 1 + i] = digits[digit_count - 1 - i];
        }

        for i in 0..clean_ext_len {
            sfn[8 + i] = clean_ext[i];
        }

        if !existing.iter().any(|entry| entry.short_name == sfn) {
            return (sfn, true);
        }
    }

    let mut fallback = [b' '; 11];
    fallback[0] = b'F';
    fallback[1] = b'~';
    fallback[2] = b'1';
    (fallback, true)
}

fn try_exact_sfn(name: &str) -> Option<[u8; 11]> {
    if !name.is_ascii() {
        return None;
    }

    let mut dot_count = 0usize;
    for b in name.bytes() {
        if b == b'.' {
            dot_count += 1;
        }
    }
    if dot_count > 1 {
        return None;
    }

    let (base, ext) = match name.split_once('.') {
        Some((b, e)) => (b, e),
        None => (name, ""),
    };

    if base.is_empty() || base.len() > 8 || ext.len() > 3 {
        return None;
    }

    let mut sfn = [b' '; 11];
    for (idx, b) in base.bytes().enumerate() {
        if b.is_ascii_lowercase() || !is_valid_sfn_char(b) {
            return None;
        }
        sfn[idx] = b;
    }
    for (idx, b) in ext.bytes().enumerate() {
        if b.is_ascii_lowercase() || !is_valid_sfn_char(b) {
            return None;
        }
        sfn[8 + idx] = b;
    }

    Some(sfn)
}

fn eq_ignore_ascii_case(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

// ============================================================
// LITTLE-ENDIAN HELPERS
// ============================================================

fn read_u16_le(buffer: &[u8], offset: usize) -> u16 {
    (buffer[offset] as u16) | ((buffer[offset + 1] as u16) << 8)
}

fn read_u32_le(buffer: &[u8], offset: usize) -> u32 {
    (buffer[offset] as u32)
        | ((buffer[offset + 1] as u32) << 8)
        | ((buffer[offset + 2] as u32) << 16)
        | ((buffer[offset + 3] as u32) << 24)
}

fn write_u16_le(buffer: &mut [u8], offset: usize, value: u16) {
    buffer[offset] = value as u8;
    buffer[offset + 1] = (value >> 8) as u8;
}

fn write_u32_le(buffer: &mut [u8], offset: usize, value: u32) {
    buffer[offset] = value as u8;
    buffer[offset + 1] = (value >> 8) as u8;
    buffer[offset + 2] = (value >> 16) as u8;
    buffer[offset + 3] = (value >> 24) as u8;
}
