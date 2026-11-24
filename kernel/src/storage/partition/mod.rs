
const MBR_SIGNATURE_OFFSET: usize = 510;
const PARTITION_TABLE_OFFSET: usize = 446;
const PARTITION_ENTRY_SIZE: usize = 16;
const PARTITION_COUNT: usize = 4;

const VBR_SIGNATURE_OFFSET: usize = 510;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum MbrError {
    InvalidSignature,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PartitionStatus {
    Empty,
    Valid,
    InvalidBootIndicator,
    InvalidRange,
    OutOfBounds,
}

#[derive(Clone, Copy)]
pub struct MbrPartition {
    pub index: u8,
    pub bootable: bool,
    pub partition_type: u8,
    pub start_lba: u32,
    pub sector_count: u32,
    pub end_lba_exclusive: u64,
    pub status: PartitionStatus,
}

impl MbrPartition {
    pub const EMPTY: Self = Self {
        index: 0,
        bootable: false,
        partition_type: 0,
        start_lba: 0,
        sector_count: 0,
        end_lba_exclusive: 0,
        status: PartitionStatus::Empty,
    };
}

#[derive(Clone, Copy)]
pub struct MbrPartitionTable {
    pub partitions: [MbrPartition; PARTITION_COUNT],
    pub count: usize,
    pub protective_gpt: bool,
}

impl MbrPartitionTable {
    pub const EMPTY: Self = Self {
        partitions: [MbrPartition::EMPTY; PARTITION_COUNT],
        count: 0,
        protective_gpt: false,
    };
}

// ============================================================
// MBR PARSER
// ============================================================

pub fn parse_mbr(
    sector: &[u8; 512],
    disk_capacity_sectors: u64,
) -> Result<MbrPartitionTable, MbrError> {
    if sector[MBR_SIGNATURE_OFFSET] != 0x55
        || sector[MBR_SIGNATURE_OFFSET + 1] != 0xAA
    {
        return Err(MbrError::InvalidSignature);
    }

    let mut table = MbrPartitionTable::EMPTY;

    for index in 0..PARTITION_COUNT {
        let offset =
            PARTITION_TABLE_OFFSET + index * PARTITION_ENTRY_SIZE;

        let boot_indicator = sector[offset];
        let partition_type = sector[offset + 4];

        let start_lba = read_u32_le(sector, offset + 8);
        let sector_count = read_u32_le(sector, offset + 12);

        if partition_type == 0 {
            continue;
        }

        let mut partition = MbrPartition {
            index: (index + 1) as u8,
            bootable: boot_indicator == 0x80,
            partition_type,
            start_lba,
            sector_count,
            end_lba_exclusive:
                start_lba as u64 + sector_count as u64,
            status: PartitionStatus::Valid,
        };

        table.count += 1;

        if boot_indicator != 0x00 && boot_indicator != 0x80 {
            partition.status = PartitionStatus::InvalidBootIndicator;
        } else if start_lba == 0 || sector_count == 0 {
            partition.status = PartitionStatus::InvalidRange;
        } else if partition.end_lba_exclusive > disk_capacity_sectors {
            partition.status = PartitionStatus::OutOfBounds;
        }

        if partition.partition_type == 0xEE {
            table.protective_gpt = true;
        }

        table.partitions[index] = partition;
    }

    Ok(table)
}

// ============================================================
// FAT FILESYSTEM TYPES
// ============================================================

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FatType {
    Fat12,
    Fat16,
    Fat32,
}

impl FatType {
    pub fn name(self) -> &'static str {
        match self {
            Self::Fat12 => "FAT12",
            Self::Fat16 => "FAT16",
            Self::Fat32 => "FAT32",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FatError {
    InvalidSignature,
    InvalidBytesPerSector,
    InvalidSectorsPerCluster,
    InvalidReservedSectors,
    InvalidFatCount,
    InvalidFatSize,
    InvalidTotalSectors,
    InvalidGeometry,
    InvalidClusterCount,
    InvalidRootCluster,
    PartitionTooSmall,
    FatTableTooSmall,
}

#[derive(Clone, Copy)]
pub struct FatBpb {
    pub fat_type: FatType,

    pub bytes_per_sector: u16,
    pub sectors_per_cluster: u8,
    pub reserved_sectors: u16,
    pub fat_count: u8,

    pub root_entry_count: u16,
    pub total_sectors: u32,

    pub fat_size_sectors: u32,
    pub root_directory_sectors: u32,

    pub first_data_sector: u32,
    pub data_sectors: u32,
    pub cluster_count: u32,

    pub root_cluster: u32,
    pub fs_info_sector: u16,
    pub backup_boot_sector: u16,

    pub media_descriptor: u8,
}

impl FatBpb {
    pub const EMPTY: Self = Self {
        fat_type: FatType::Fat12,

        bytes_per_sector: 0,
        sectors_per_cluster: 0,
        reserved_sectors: 0,
        fat_count: 0,

        root_entry_count: 0,
        total_sectors: 0,

        fat_size_sectors: 0,
        root_directory_sectors: 0,

        first_data_sector: 0,
        data_sectors: 0,
        cluster_count: 0,

        root_cluster: 0,
        fs_info_sector: 0,
        backup_boot_sector: 0,

        media_descriptor: 0,
    };
}

// ============================================================
// FAT BPB PARSER
// ============================================================

pub fn parse_fat_bpb(
    sector: &[u8; 512],
    partition_sectors: u64,
) -> Result<FatBpb, FatError> {
    // Verify the boot sector signature.
    if sector[VBR_SIGNATURE_OFFSET] != 0x55
        || sector[VBR_SIGNATURE_OFFSET + 1] != 0xAA
    {
        return Err(FatError::InvalidSignature);
    }

    let bytes_per_sector = read_u16_le(sector, 11);

if bytes_per_sector != 512 {
    return Err(FatError::InvalidBytesPerSector);
}

    let sectors_per_cluster = sector[13];

    if sectors_per_cluster == 0
        || !sectors_per_cluster.is_power_of_two()
        || sectors_per_cluster > 128
    {
        return Err(FatError::InvalidSectorsPerCluster);
    }

    let reserved_sectors = read_u16_le(sector, 14);

    if reserved_sectors == 0 {
        return Err(FatError::InvalidReservedSectors);
    }

    let fat_count = sector[16];

    if fat_count == 0 || fat_count > 4 {
        return Err(FatError::InvalidFatCount);
    }

    let root_entry_count = read_u16_le(sector, 17);

    let total_sectors_16 = read_u16_le(sector, 19) as u32;
    let total_sectors_32 = read_u32_le(sector, 32);

    let total_sectors = if total_sectors_16 != 0 {
        total_sectors_16
    } else {
        total_sectors_32
    };

    if total_sectors == 0 {
        return Err(FatError::InvalidTotalSectors);
    }

    if total_sectors as u64 > partition_sectors {
        return Err(FatError::PartitionTooSmall);
    }

    let fat_size_16 = read_u16_le(sector, 22) as u32;
    let fat_size_32 = read_u32_le(sector, 36);

    let fat_size_sectors = if fat_size_16 != 0 {
        fat_size_16
    } else {
        fat_size_32
    };

    if fat_size_sectors == 0 {
        return Err(FatError::InvalidFatSize);
    }

    // Calculate the fixed root directory size.
    let root_directory_bytes =
        root_entry_count as u64 * 32;

    let root_directory_sectors =
        (root_directory_bytes + bytes_per_sector as u64 - 1)
        / bytes_per_sector as u64;

    let root_directory_sectors = root_directory_sectors as u32;

    // Calculate the beginning of the data region.
    let fat_area_sectors =
        fat_count as u64 * fat_size_sectors as u64;

    let first_data_sector =
        reserved_sectors as u64
        + fat_area_sectors
        + root_directory_sectors as u64;

    if first_data_sector >= total_sectors as u64 {
        return Err(FatError::InvalidGeometry);
    }

    let data_sectors =
        total_sectors as u64 - first_data_sector;

    let cluster_count =
        data_sectors / sectors_per_cluster as u64;

    if cluster_count == 0 || cluster_count > u32::MAX as u64 {
        return Err(FatError::InvalidClusterCount);
    }

    // FAT type is determined by cluster thresholds or extended FAT32 BPB layout.
    let is_fat32_bpb = fat_size_16 == 0 && root_entry_count == 0 && fat_size_32 != 0;
    let fat_type = if cluster_count >= 65525 || (is_fat32_bpb && cluster_count >= 4085) {
        FatType::Fat32
    } else if cluster_count < 4085 {
        FatType::Fat12
    } else {
        FatType::Fat16
    };

    // Validate that the FAT table can store all cluster entries.
let fat_bytes =
    fat_size_sectors as u64 * bytes_per_sector as u64;

let fat_entry_capacity = match fat_type {
    FatType::Fat12 => (fat_bytes * 2) / 3,
    FatType::Fat16 => fat_bytes / 2,
    FatType::Fat32 => fat_bytes / 4,
};

let required_entries = cluster_count as u64 + 2;

if fat_entry_capacity < required_entries {
    return Err(FatError::FatTableTooSmall);
}

    // FAT32 requires a valid root cluster.
    let root_cluster = read_u32_le(sector, 44);

    if fat_type == FatType::Fat32 {
        if root_cluster < 2
            || root_cluster >= cluster_count as u32 + 2
        {
            return Err(FatError::InvalidRootCluster);
        }

        // FAT32 must use the extended FAT32 BPB layout.
        if fat_size_16 != 0 || root_entry_count != 0 {
            return Err(FatError::InvalidGeometry);
        }
    } else {
        // FAT12/FAT16 require a fixed root directory.
        if fat_size_16 == 0 || root_entry_count == 0 {
            return Err(FatError::InvalidGeometry);
        }
    }

    let fs_info_sector = read_u16_le(sector, 48);
    let backup_boot_sector = read_u16_le(sector, 50);

    Ok(FatBpb {
        fat_type,

        bytes_per_sector,
        sectors_per_cluster,
        reserved_sectors,
        fat_count,

        root_entry_count,
        total_sectors,

        fat_size_sectors,
        root_directory_sectors,

        first_data_sector: first_data_sector as u32,
        data_sectors: data_sectors as u32,
        cluster_count: cluster_count as u32,

        root_cluster,
        fs_info_sector,
        backup_boot_sector,

        media_descriptor: sector[21],
    })
}

// ============================================================
// LITTLE-ENDIAN READING
// ============================================================

fn read_u16_le(
    buffer: &[u8; 512],
    offset: usize,
) -> u16 {
    (buffer[offset] as u16)
        | ((buffer[offset + 1] as u16) << 8)
}

fn read_u32_le(
    buffer: &[u8; 512],
    offset: usize,
) -> u32 {
    (buffer[offset] as u32)
        | ((buffer[offset + 1] as u32) << 8)
        | ((buffer[offset + 2] as u32) << 16)
        | ((buffer[offset + 3] as u32) << 24)
}

// ============================================================
// PARTITION TYPE NAMES
// ============================================================

pub fn partition_type_name(partition_type: u8) -> &'static str {
    match partition_type {
        0x01 => "FAT12",
        0x04 | 0x06 | 0x0E => "FAT16",
        0x07 => "NTFS / exFAT / HPFS",
        0x0B | 0x0C => "FAT32",
        0x05 | 0x0F => "Extended",
        0x82 => "Linux Swap",
        0x83 => "Linux",
        0x85 => "Linux Extended",
        0xEF => "EFI System",
        0xEE => "GPT Protective",
        0xA5 => "FreeBSD",
        0xA6 => "OpenBSD",
        0xA9 => "NetBSD",
        0xAF => "Apple HFS/HFS+",
        0xFD => "Linux RAID",
        _ => "Unknown",
    }
}

pub fn is_extended_partition(partition_type: u8) -> bool {
    matches!(partition_type, 0x05 | 0x0F | 0x85)
}

pub fn partition_type_matches_fat(
    partition_type: u8,
    fat_type: FatType,
) -> bool {
    match partition_type {
        0x01 => fat_type == FatType::Fat12,

        0x04 | 0x06 | 0x0E => {
            fat_type == FatType::Fat16
        }

        0x0B | 0x0C => {
            fat_type == FatType::Fat32
        }

        _ => false,
    }
}
