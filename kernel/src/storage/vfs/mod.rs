use alloc::string::String;
use alloc::vec::Vec;

use crate::task::preempt::PreemptMutex;

use crate::drivers::ahci::{self, AhciBlockDevice};
use crate::drivers::nvme::{self, NvmeBlockDevice};
use crate::drivers::virtio_blk::{self, VirtioBlockDevice};
use crate::storage::block::{BlockDevice, BlockDeviceError};
use crate::storage::cache::{CacheBackendKind, CachedBlockDevice};
use crate::storage::fat16::{Fat16Error, Fat16File, Fat16Volume};
use crate::storage::fat32::{Fat32DirEntry, Fat32Error, Fat32Volume};
use crate::storage::ide::IdeBlockDevice;

pub enum SystemBlockDevice {
    Virtio(VirtioBlockDevice),
    Ahci(AhciBlockDevice),
    Nvme(NvmeBlockDevice),
    Ide(IdeBlockDevice),
}

impl BlockDevice for SystemBlockDevice {
    fn sector_size(&self) -> u32 {
        match self {
            Self::Virtio(d) => d.sector_size(),
            Self::Ahci(d) => d.sector_size(),
            Self::Nvme(d) => d.sector_size(),
            Self::Ide(d) => d.sector_size(),
        }
    }

    fn sector_count(&self) -> u64 {
        match self {
            Self::Virtio(d) => d.sector_count(),
            Self::Ahci(d) => d.sector_count(),
            Self::Nvme(d) => d.sector_count(),
            Self::Ide(d) => d.sector_count(),
        }
    }

    fn max_transfer_sectors(&self) -> usize {
        match self {
            Self::Virtio(d) => d.max_transfer_sectors(),
            Self::Ahci(d) => d.max_transfer_sectors(),
            Self::Nvme(d) => d.max_transfer_sectors(),
            Self::Ide(d) => d.max_transfer_sectors(),
        }
    }

    fn read_sector(&mut self, lba: u64, buffer: &mut [u8]) -> Result<(), BlockDeviceError> {
        match self {
            Self::Virtio(d) => d.read_sector(lba, buffer),
            Self::Ahci(d) => d.read_sector(lba, buffer),
            Self::Nvme(d) => d.read_sector(lba, buffer),
            Self::Ide(d) => d.read_sector(lba, buffer),
        }
    }

    fn write_sector(&mut self, lba: u64, buffer: &[u8]) -> Result<(), BlockDeviceError> {
        match self {
            Self::Virtio(d) => d.write_sector(lba, buffer),
            Self::Ahci(d) => d.write_sector(lba, buffer),
            Self::Nvme(d) => d.write_sector(lba, buffer),
            Self::Ide(d) => d.write_sector(lba, buffer),
        }
    }

    fn read_sectors(
        &mut self,
        start_lba: u64,
        sector_count: usize,
        buffer: &mut [u8],
    ) -> Result<(), BlockDeviceError> {
        match self {
            Self::Virtio(d) => d.read_sectors(start_lba, sector_count, buffer),
            Self::Ahci(d) => d.read_sectors(start_lba, sector_count, buffer),
            Self::Nvme(d) => d.read_sectors(start_lba, sector_count, buffer),
            Self::Ide(d) => d.read_sectors(start_lba, sector_count, buffer),
        }
    }

    fn write_sectors(
        &mut self,
        start_lba: u64,
        sector_count: usize,
        buffer: &[u8],
    ) -> Result<(), BlockDeviceError> {
        match self {
            Self::Virtio(d) => d.write_sectors(start_lba, sector_count, buffer),
            Self::Ahci(d) => d.write_sectors(start_lba, sector_count, buffer),
            Self::Nvme(d) => d.write_sectors(start_lba, sector_count, buffer),
            Self::Ide(d) => d.write_sectors(start_lba, sector_count, buffer),
        }
    }

    fn flush(&mut self) -> Result<(), BlockDeviceError> {
        match self {
            Self::Virtio(d) => d.flush(),
            Self::Ahci(d) => d.flush(),
            Self::Nvme(d) => d.flush(),
            Self::Ide(d) => d.flush(),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum MountedBackend {
    VirtioBlk {
        max_dma_sectors: usize,
    },
    AhciSata {
        port: u8,
        max_dma_sectors: usize,
    },
    Nvme {
        max_dma_sectors: usize,
        lba_size: u32,
    },
    IdeAta {
        controller: usize,
        secondary: bool,
        slave: bool,
    },
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum VfsNodeKind {
    File,
    Directory,
}

#[derive(Clone)]
pub struct VfsDirEntry {
    pub name: String,
    pub kind: VfsNodeKind,
    pub size: u32,
    pub attributes: u8,
    pub first_cluster: u32,
}

#[derive(Clone, Copy)]
pub struct VfsVolumeInfo {
    pub fs_type: &'static str,
    pub read_only: bool,
    pub sector_size: u32,
    pub disk_sectors: u64,
    pub partition_index: u8,
    pub partition_start_lba: u32,
    pub partition_sectors: u32,
    pub sectors_per_cluster: u8,
    pub cluster_count: u32,
    pub free_clusters: Option<u32>,
    pub total_bytes: u64,
    pub free_bytes: Option<u64>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum VfsError {
    NotMounted,
    NoDisk,
    UnsupportedFilesystem,
    ReadOnlyFilesystem,
    NotFound,
    AlreadyExists,
    InvalidPath,
    InvalidName,
    IsADirectory,
    NotADirectory,
    DirectoryNotEmpty,
    NoSpace,
    PermissionDenied,
    OffsetOutOfRange,
    IoError,
}

impl VfsError {
    pub const fn message(self) -> &'static str {
        match self {
            Self::NotMounted => "Filesystem is not mounted",
            Self::NoDisk => "No storage device available",
            Self::UnsupportedFilesystem => "No supported FAT32/FAT16 partition found",
            Self::ReadOnlyFilesystem => "Operation not supported on read-only FAT16 volume",
            Self::NotFound => "No such file or directory",
            Self::AlreadyExists => "File or directory already exists",
            Self::InvalidPath => "Invalid path",
            Self::InvalidName => "Invalid file or directory name",
            Self::IsADirectory => "Target is a directory",
            Self::NotADirectory => "Target is not a directory",
            Self::DirectoryNotEmpty => "Directory is not empty",
            Self::NoSpace => "No free clusters left on volume",
            Self::PermissionDenied => "Entry is read-only or protected",
            Self::OffsetOutOfRange => "Read offset is out of range",
            Self::IoError => "Storage device I/O error",
        }
    }
}

impl From<Fat32Error> for VfsError {
    fn from(error: Fat32Error) -> Self {
        match error {
            Fat32Error::DeviceNotReady => Self::NoDisk,
            Fat32Error::NoFat32Partition | Fat32Error::UnsupportedSectorSize => {
                Self::UnsupportedFilesystem
            }
            Fat32Error::FileNotFound => Self::NotFound,
            Fat32Error::AlreadyExists => Self::AlreadyExists,
            Fat32Error::InvalidFileName => Self::InvalidName,
            Fat32Error::IsADirectory => Self::IsADirectory,
            Fat32Error::NotADirectory => Self::NotADirectory,
            Fat32Error::DirectoryNotEmpty => Self::DirectoryNotEmpty,
            Fat32Error::NoFreeClusters => Self::NoSpace,
            Fat32Error::ReadOnlyEntry => Self::PermissionDenied,
            Fat32Error::OffsetOutOfRange => Self::OffsetOutOfRange,
            _ => Self::IoError,
        }
    }
}

impl From<Fat16Error> for VfsError {
    fn from(error: Fat16Error) -> Self {
        match error {
            Fat16Error::NoController | Fat16Error::DeviceNotReady => Self::NoDisk,
            Fat16Error::NoFat16Partition | Fat16Error::UnsupportedSectorSize => {
                Self::UnsupportedFilesystem
            }
            Fat16Error::FileNotFound => Self::NotFound,
            Fat16Error::InvalidFileName => Self::InvalidName,
            Fat16Error::OffsetOutOfRange => Self::OffsetOutOfRange,
            _ => Self::IoError,
        }
    }
}

pub trait FileSystem {
    fn fs_name(&self) -> &'static str;
    fn is_read_only(&self) -> bool;
    fn volume_info(&mut self) -> Result<VfsVolumeInfo, VfsError>;
    fn list_dir(&mut self, abs_path: &str) -> Result<Vec<VfsDirEntry>, VfsError>;
    fn stat(&mut self, abs_path: &str) -> Result<VfsDirEntry, VfsError>;
    fn read_file_chunk(
        &mut self,
        abs_path: &str,
        offset: u32,
        buffer: &mut [u8],
    ) -> Result<usize, VfsError>;
    fn read_file(&mut self, abs_path: &str) -> Result<Vec<u8>, VfsError>;
    fn write_file(&mut self, abs_path: &str, data: &[u8]) -> Result<(), VfsError>;
    fn create_dir(&mut self, abs_path: &str) -> Result<(), VfsError>;
    fn remove(&mut self, abs_path: &str) -> Result<(), VfsError>;
}

// ============================================================
// FAT32 FILESYSTEM IMPLEMENTATION
// ============================================================

impl<D: BlockDevice> Fat32Volume<D> {
    fn resolve_cluster(&mut self, abs_path: &str) -> Result<u32, VfsError> {
        let mut current = self.root_cluster();

        for segment in abs_path.split('/').filter(|s| !s.is_empty()) {
            if segment == "." {
                continue;
            }
            if segment == ".." {
                if current == self.root_cluster() {
                    continue;
                }
                if let Some(dotdot) = self.find_entry(current, "..")? {
                    current = if dotdot.first_cluster < 2 {
                        self.root_cluster()
                    } else {
                        dotdot.first_cluster
                    };
                }
                continue;
            }

            let entry = self
                .find_entry(current, segment)?
                .ok_or(VfsError::NotFound)?;

            if !entry.is_directory() {
                return Err(VfsError::NotADirectory);
            }

            current = if entry.first_cluster < 2 {
                self.root_cluster()
            } else {
                entry.first_cluster
            };
        }

        Ok(current)
    }

    fn resolve_entry(&mut self, abs_path: &str) -> Result<Fat32DirEntry, VfsError> {
        let (parent_path, leaf) = split_parent_leaf(abs_path)?;
        let parent_cluster = self.resolve_cluster(parent_path)?;
        let entry = self
            .find_entry(parent_cluster, leaf)?
            .ok_or(VfsError::NotFound)?;
        Ok(entry)
    }
}

impl<D: BlockDevice> FileSystem for Fat32Volume<D> {
    fn fs_name(&self) -> &'static str {
        "FAT32"
    }

    fn is_read_only(&self) -> bool {
        false
    }

    fn volume_info(&mut self) -> Result<VfsVolumeInfo, VfsError> {
        let free_clusters = self.free_clusters_count().ok();
        let cluster_bytes =
            self.bpb.sectors_per_cluster as u64 * self.bpb.bytes_per_sector as u64;
        let total_bytes = self.bpb.cluster_count as u64 * cluster_bytes;
        let free_bytes = free_clusters.map(|free| free as u64 * cluster_bytes);

        Ok(VfsVolumeInfo {
            fs_type: "FAT32",
            read_only: false,
            sector_size: self.device().sector_size(),
            disk_sectors: self.device().sector_count(),
            partition_index: self.partition.index,
            partition_start_lba: self.partition.start_lba,
            partition_sectors: self.partition.sector_count,
            sectors_per_cluster: self.bpb.sectors_per_cluster,
            cluster_count: self.bpb.cluster_count,
            free_clusters,
            total_bytes,
            free_bytes,
        })
    }

    fn list_dir(&mut self, abs_path: &str) -> Result<Vec<VfsDirEntry>, VfsError> {
        let cluster = self.resolve_cluster(abs_path)?;
        let entries = Fat32Volume::list_dir(self, cluster)?;

        let mut out = Vec::with_capacity(entries.len());
        for entry in entries {
            out.push(VfsDirEntry {
                name: entry.name.clone(),
                kind: if entry.is_directory() {
                    VfsNodeKind::Directory
                } else {
                    VfsNodeKind::File
                },
                size: entry.size,
                attributes: entry.attributes,
                first_cluster: entry.first_cluster,
            });
        }
        Ok(out)
    }

    fn stat(&mut self, abs_path: &str) -> Result<VfsDirEntry, VfsError> {
        if abs_path == "/" {
            return Ok(VfsDirEntry {
                name: String::from("/"),
                kind: VfsNodeKind::Directory,
                size: 0,
                attributes: 0x10,
                first_cluster: self.root_cluster(),
            });
        }

        let entry = self.resolve_entry(abs_path)?;
        let is_dir = entry.is_directory();
        Ok(VfsDirEntry {
            name: entry.name,
            kind: if is_dir {
                VfsNodeKind::Directory
            } else {
                VfsNodeKind::File
            },
            size: entry.size,
            attributes: entry.attributes,
            first_cluster: entry.first_cluster,
        })
    }

    fn read_file_chunk(
        &mut self,
        abs_path: &str,
        offset: u32,
        buffer: &mut [u8],
    ) -> Result<usize, VfsError> {
        let entry = self.resolve_entry(abs_path)?;
        if entry.is_directory() {
            return Err(VfsError::IsADirectory);
        }
        let read = self.read_file_range(&entry, offset, buffer)?;
        Ok(read)
    }

    fn read_file(&mut self, abs_path: &str) -> Result<Vec<u8>, VfsError> {
        let entry = self.resolve_entry(abs_path)?;
        let data = Fat32Volume::read_file(self, &entry)?;
        Ok(data)
    }

    fn write_file(&mut self, abs_path: &str, data: &[u8]) -> Result<(), VfsError> {
        let (parent_path, leaf) = split_parent_leaf(abs_path)?;
        let parent_cluster = self.resolve_cluster(parent_path)?;
        Fat32Volume::write_file(self, parent_cluster, leaf, data)?;
        Ok(())
    }

    fn create_dir(&mut self, abs_path: &str) -> Result<(), VfsError> {
        let (parent_path, leaf) = split_parent_leaf(abs_path)?;
        let parent_cluster = self.resolve_cluster(parent_path)?;
        Fat32Volume::create_dir(self, parent_cluster, leaf)?;
        Ok(())
    }

    fn remove(&mut self, abs_path: &str) -> Result<(), VfsError> {
        let (parent_path, leaf) = split_parent_leaf(abs_path)?;
        let parent_cluster = self.resolve_cluster(parent_path)?;
        Fat32Volume::remove_entry(self, parent_cluster, leaf)?;
        Ok(())
    }
}

// ============================================================
// FAT16 FILESYSTEM ADAPTER (READ-ONLY ROOT SUPPORT)
// ============================================================

impl<D: BlockDevice> FileSystem for Fat16Volume<D> {
    fn fs_name(&self) -> &'static str {
        "FAT16"
    }

    fn is_read_only(&self) -> bool {
        true
    }

    fn volume_info(&mut self) -> Result<VfsVolumeInfo, VfsError> {
        let cluster_bytes =
            self.bpb.sectors_per_cluster as u64 * self.bpb.bytes_per_sector as u64;
        let total_bytes = self.bpb.cluster_count as u64 * cluster_bytes;

        Ok(VfsVolumeInfo {
            fs_type: "FAT16",
            read_only: true,
            sector_size: self.device().sector_size(),
            disk_sectors: self.device().sector_count(),
            partition_index: self.partition.index,
            partition_start_lba: self.partition.start_lba,
            partition_sectors: self.partition.sector_count,
            sectors_per_cluster: self.bpb.sectors_per_cluster,
            cluster_count: self.bpb.cluster_count,
            free_clusters: None,
            total_bytes,
            free_bytes: None,
        })
    }

    fn list_dir(&mut self, abs_path: &str) -> Result<Vec<VfsDirEntry>, VfsError> {
        if abs_path != "/" {
            return Err(VfsError::NotADirectory);
        }

        let mut entries = Vec::new();
        for index in 0..self.root_entry_count() {
            if self.root_entry_is_end(index)? {
                break;
            }
            let file = self.read_root_entry(index)?;
            if file.is_empty() {
                continue;
            }

            entries.push(VfsDirEntry {
                name: format_fat16_name(&file.name),
                kind: if file.is_directory() {
                    VfsNodeKind::Directory
                } else {
                    VfsNodeKind::File
                },
                size: file.size,
                attributes: file.attributes,
                first_cluster: file.first_cluster as u32,
            });
        }

        Ok(entries)
    }

    fn stat(&mut self, abs_path: &str) -> Result<VfsDirEntry, VfsError> {
        if abs_path == "/" {
            return Ok(VfsDirEntry {
                name: String::from("/"),
                kind: VfsNodeKind::Directory,
                size: 0,
                attributes: 0x10,
                first_cluster: 0,
            });
        }

        let file = find_fat16_by_path(self, abs_path)?;
        Ok(VfsDirEntry {
            name: format_fat16_name(&file.name),
            kind: if file.is_directory() {
                VfsNodeKind::Directory
            } else {
                VfsNodeKind::File
            },
            size: file.size,
            attributes: file.attributes,
            first_cluster: file.first_cluster as u32,
        })
    }

    fn read_file_chunk(
        &mut self,
        abs_path: &str,
        offset: u32,
        buffer: &mut [u8],
    ) -> Result<usize, VfsError> {
        let file = find_fat16_by_path(self, abs_path)?;
        if file.is_directory() {
            return Err(VfsError::IsADirectory);
        }
        let read = self.read_file_range(&file, offset, buffer)?;
        Ok(read)
    }

    fn read_file(&mut self, abs_path: &str) -> Result<Vec<u8>, VfsError> {
        let file = find_fat16_by_path(self, abs_path)?;
        if file.is_directory() {
            return Err(VfsError::IsADirectory);
        }
        let mut data = Vec::new();
        if file.size == 0 {
            return Ok(data);
        }
        data.resize(file.size as usize, 0);
        let read = self.read_file_range(&file, 0, &mut data)?;
        data.truncate(read);
        Ok(data)
    }

    fn write_file(&mut self, _abs_path: &str, _data: &[u8]) -> Result<(), VfsError> {
        Err(VfsError::ReadOnlyFilesystem)
    }

    fn create_dir(&mut self, _abs_path: &str) -> Result<(), VfsError> {
        Err(VfsError::ReadOnlyFilesystem)
    }

    fn remove(&mut self, _abs_path: &str) -> Result<(), VfsError> {
        Err(VfsError::ReadOnlyFilesystem)
    }
}

fn find_fat16_by_path<D: BlockDevice>(
    volume: &mut Fat16Volume<D>,
    abs_path: &str,
) -> Result<Fat16File, VfsError> {
    let trimmed = abs_path.trim_start_matches('/');
    if trimmed.is_empty() || trimmed.contains('/') {
        return Err(VfsError::NotFound);
    }
    let short_name = parse_fat16_8_3(trimmed)?;
    let file = volume.find_root_file(&short_name)?;
    Ok(file)
}

fn format_fat16_name(raw: &[u8; 11]) -> String {
    let mut name = String::new();
    let mut base_end = 8;
    while base_end > 0 && raw[base_end - 1] == b' ' {
        base_end -= 1;
    }
    for &b in &raw[..base_end] {
        name.push(b as char);
    }

    let mut ext_end = 3;
    while ext_end > 0 && raw[8 + ext_end - 1] == b' ' {
        ext_end -= 1;
    }
    if ext_end > 0 {
        name.push('.');
        for &b in &raw[8..8 + ext_end] {
            name.push(b as char);
        }
    }
    name
}

fn parse_fat16_8_3(name: &str) -> Result<[u8; 11], VfsError> {
    let mut result = [b' '; 11];
    let mut parts = name.split('.');

    let base = parts.next().unwrap_or("");
    let ext = parts.next().unwrap_or("");

    if base.is_empty() || base.len() > 8 || ext.len() > 3 || parts.next().is_some() {
        return Err(VfsError::InvalidName);
    }

    for (i, b) in base.bytes().enumerate() {
        if !b.is_ascii_alphanumeric() && !matches!(b, b'_' | b'-' | b'~' | b'$') {
            return Err(VfsError::InvalidName);
        }
        result[i] = b.to_ascii_uppercase();
    }

    for (i, b) in ext.bytes().enumerate() {
        if !b.is_ascii_alphanumeric() {
            return Err(VfsError::InvalidName);
        }
        result[8 + i] = b.to_ascii_uppercase();
    }

    Ok(result)
}

// ============================================================
// MOUNTED VOLUME & GLOBAL KERNEL VFS
// ============================================================

pub enum MountedVolume<D: BlockDevice> {
    Fat32(Fat32Volume<D>),
    Fat16(Fat16Volume<D>),
}

impl<D: BlockDevice> MountedVolume<D> {
    pub fn sync(&mut self) -> Result<(), VfsError> {
        match self {
            Self::Fat32(vol) => vol.flush().map_err(VfsError::from),
            Self::Fat16(vol) => vol.flush().map_err(VfsError::from),
        }
    }

    pub fn device(&self) -> &D {
        match self {
            Self::Fat32(vol) => vol.device(),
            Self::Fat16(vol) => vol.device(),
        }
    }

    pub fn device_mut(&mut self) -> &mut D {
        match self {
            Self::Fat32(vol) => vol.device_mut(),
            Self::Fat16(vol) => vol.device_mut(),
        }
    }
}

impl<D: BlockDevice> FileSystem for MountedVolume<D> {
    fn fs_name(&self) -> &'static str {
        match self {
            Self::Fat32(vol) => FileSystem::fs_name(vol),
            Self::Fat16(vol) => FileSystem::fs_name(vol),
        }
    }

    fn is_read_only(&self) -> bool {
        match self {
            Self::Fat32(vol) => FileSystem::is_read_only(vol),
            Self::Fat16(vol) => FileSystem::is_read_only(vol),
        }
    }

    fn volume_info(&mut self) -> Result<VfsVolumeInfo, VfsError> {
        match self {
            Self::Fat32(vol) => FileSystem::volume_info(vol),
            Self::Fat16(vol) => FileSystem::volume_info(vol),
        }
    }

    fn list_dir(&mut self, abs_path: &str) -> Result<Vec<VfsDirEntry>, VfsError> {
        match self {
            Self::Fat32(vol) => FileSystem::list_dir(vol, abs_path),
            Self::Fat16(vol) => FileSystem::list_dir(vol, abs_path),
        }
    }

    fn stat(&mut self, abs_path: &str) -> Result<VfsDirEntry, VfsError> {
        match self {
            Self::Fat32(vol) => FileSystem::stat(vol, abs_path),
            Self::Fat16(vol) => FileSystem::stat(vol, abs_path),
        }
    }

    fn read_file_chunk(
        &mut self,
        abs_path: &str,
        offset: u32,
        buffer: &mut [u8],
    ) -> Result<usize, VfsError> {
        match self {
            Self::Fat32(vol) => FileSystem::read_file_chunk(vol, abs_path, offset, buffer),
            Self::Fat16(vol) => FileSystem::read_file_chunk(vol, abs_path, offset, buffer),
        }
    }

    fn read_file(&mut self, abs_path: &str) -> Result<Vec<u8>, VfsError> {
        match self {
            Self::Fat32(vol) => FileSystem::read_file(vol, abs_path),
            Self::Fat16(vol) => FileSystem::read_file(vol, abs_path),
        }
    }

    fn write_file(&mut self, abs_path: &str, data: &[u8]) -> Result<(), VfsError> {
        match self {
            Self::Fat32(vol) => FileSystem::write_file(vol, abs_path, data),
            Self::Fat16(vol) => FileSystem::write_file(vol, abs_path, data),
        }
    }

    fn create_dir(&mut self, abs_path: &str) -> Result<(), VfsError> {
        match self {
            Self::Fat32(vol) => FileSystem::create_dir(vol, abs_path),
            Self::Fat16(vol) => FileSystem::create_dir(vol, abs_path),
        }
    }

    fn remove(&mut self, abs_path: &str) -> Result<(), VfsError> {
        match self {
            Self::Fat32(vol) => FileSystem::remove(vol, abs_path),
            Self::Fat16(vol) => FileSystem::remove(vol, abs_path),
        }
    }
}

use crate::storage::ramfs::RamFs;

enum DiskSubPath<'a> {
    RootRamFs,
    DiskMount(&'a str),
}

fn split_mount_target(abs_path: &str) -> DiskSubPath<'_> {
    if abs_path == "/disk" {
        DiskSubPath::DiskMount("/")
    } else if let Some(rest) = abs_path.strip_prefix("/disk/") {
        if rest.is_empty() {
            DiskSubPath::DiskMount("/")
        } else {
            // abs_path[5..] begins with '/'
            DiskSubPath::DiskMount(&abs_path[5..])
        }
    } else {
        DiskSubPath::RootRamFs
    }
}

#[derive(Clone, Copy)]
enum ProbeTarget {
    Virtio,
    Ahci(u8),
    Nvme,
    Ide(usize, bool, bool),
}

fn open_probe_target(
    target: ProbeTarget,
) -> Option<(
    CachedBlockDevice<SystemBlockDevice>,
    MountedBackend,
    Option<(usize, bool, bool)>,
)> {
    match target {
        ProbeTarget::Virtio => {
            let snap = virtio_blk::snapshot();
            if !snap.initialized || snap.sector_count == 0 {
                return None;
            }
            let dev = SystemBlockDevice::Virtio(VirtioBlockDevice);
            let max_dma = dev.max_transfer_sectors();
            Some((
                CachedBlockDevice::new(dev, CacheBackendKind::VirtioBlk),
                MountedBackend::VirtioBlk {
                    max_dma_sectors: max_dma,
                },
                None,
            ))
        }
        ProbeTarget::Ahci(port) => {
            let snap = ahci::snapshot();
            if !snap.initialized || snap.sector_count == 0 || snap.active_port != Some(port) {
                return None;
            }
            let dev = SystemBlockDevice::Ahci(AhciBlockDevice);
            let max_dma = dev.max_transfer_sectors();
            Some((
                CachedBlockDevice::new(dev, CacheBackendKind::AhciSata),
                MountedBackend::AhciSata {
                    port,
                    max_dma_sectors: max_dma,
                },
                None,
            ))
        }
        ProbeTarget::Nvme => {
            let snap = nvme::snapshot();
            if !snap.initialized || snap.sector_count_512 == 0 {
                return None;
            }
            let dev = SystemBlockDevice::Nvme(NvmeBlockDevice);
            let max_dma = dev.max_transfer_sectors();
            Some((
                CachedBlockDevice::new(dev, CacheBackendKind::Nvme),
                MountedBackend::Nvme {
                    max_dma_sectors: max_dma,
                    lba_size: snap.lba_size,
                },
                None,
            ))
        }
        ProbeTarget::Ide(ctrl, sec, slv) => {
            let ide_dev = IdeBlockDevice::from_slot(ctrl, sec, slv).ok()?;
            let dev = SystemBlockDevice::Ide(ide_dev);
            Some((
                CachedBlockDevice::new(dev, CacheBackendKind::IdeAtaPio),
                MountedBackend::IdeAta {
                    controller: ctrl,
                    secondary: sec,
                    slave: slv,
                },
                Some((ctrl, sec, slv)),
            ))
        }
    }
}

pub struct KernelVfs {
    ramfs: RamFs,
    volume: Option<MountedVolume<CachedBlockDevice<SystemBlockDevice>>>,
    mounted_slot: Option<(usize, bool, bool)>,
    mounted_backend: Option<MountedBackend>,
    disk_probed: bool,
    cwd: String,
}

impl KernelVfs {
    pub const fn new() -> Self {
        Self {
            ramfs: RamFs::new(),
            volume: None,
            mounted_slot: None,
            mounted_backend: None,
            disk_probed: false,
            cwd: String::new(),
        }
    }

    pub fn cwd(&self) -> &str {
        if self.cwd.is_empty() {
            "/"
        } else {
            &self.cwd
        }
    }

    pub fn mounted_slot(&self) -> Option<(usize, bool, bool)> {
        self.mounted_slot
    }

    pub fn mounted_backend(&self) -> Option<MountedBackend> {
        self.mounted_backend
    }

    pub fn is_mounted(&self) -> bool {
        true
    }

    pub fn is_disk_mounted(&self) -> bool {
        self.volume.is_some()
    }

    pub fn sync(&mut self) -> Result<(), VfsError> {
        if let Some(ref mut vol) = self.volume {
            vol.sync()?;
        }
        Ok(())
    }

    fn install_volume(
        &mut self,
        mut volume: MountedVolume<CachedBlockDevice<SystemBlockDevice>>,
        backend: MountedBackend,
        slot: Option<(usize, bool, bool)>,
    ) {
        volume.device_mut().activate_telemetry();
        self.volume = Some(volume);
        self.mounted_backend = Some(backend);
        self.mounted_slot = slot;
        if self.cwd.is_empty() {
            self.cwd = String::from("/");
        }
    }

    fn mount_probe_target(&mut self, target: ProbeTarget) -> Result<(), VfsError> {
        self.ramfs.ensure_initialized();
        self.disk_probed = true;
        if let Some(ref mut existing) = self.volume {
            let _ = existing.sync();
        }

        let Some((cached, backend, slot)) = open_probe_target(target) else {
            return Err(VfsError::NoDisk);
        };

        if let Ok(fat32) = Fat32Volume::mount(cached) {
            self.install_volume(MountedVolume::Fat32(fat32), backend, slot);
            return Ok(());
        }

        let Some((cached, backend, slot)) = open_probe_target(target) else {
            return Err(VfsError::NoDisk);
        };

        if let Ok(fat16) = Fat16Volume::mount(cached) {
            self.install_volume(MountedVolume::Fat16(fat16), backend, slot);
            return Ok(());
        }

        Err(VfsError::UnsupportedFilesystem)
    }

    pub fn mount_auto(&mut self) -> Result<(), VfsError> {
        self.ramfs.ensure_initialized();
        self.disk_probed = true;
        if let Some(ref mut existing) = self.volume {
            let _ = existing.sync();
        }

        let mut candidates: Vec<ProbeTarget> = Vec::new();

        let v_snap = virtio_blk::snapshot();
        if v_snap.initialized && v_snap.sector_count > 0 {
            candidates.push(ProbeTarget::Virtio);
        }

        let a_snap = ahci::snapshot();
        if a_snap.initialized && a_snap.sector_count > 0 {
            if let Some(port) = a_snap.active_port {
                candidates.push(ProbeTarget::Ahci(port));
            }
        }

        let n_snap = nvme::snapshot();
        if n_snap.initialized && n_snap.sector_count_512 > 0 {
            candidates.push(ProbeTarget::Nvme);
        }

        let count = crate::storage::ide::controller_count();
        for controller_index in 0..count {
            for secondary in [false, true] {
                for slave in [false, true] {
                    if IdeBlockDevice::from_slot(controller_index, secondary, slave).is_ok() {
                        candidates.push(ProbeTarget::Ide(controller_index, secondary, slave));
                    }
                }
            }
        }

        if candidates.is_empty() {
            return Err(VfsError::NoDisk);
        }

        // Pass 1: Prefer writable FAT32 volumes across fastest DMA controllers first (VirtIO -> AHCI -> NVMe -> IDE).
        for &target in &candidates {
            let Some((cached, backend, slot)) = open_probe_target(target) else {
                continue;
            };
            if let Ok(fat32) = Fat32Volume::mount(cached) {
                self.install_volume(MountedVolume::Fat32(fat32), backend, slot);
                return Ok(());
            }
        }

        // Pass 2: Fall back to read-only FAT16 if no FAT32 volume exists anywhere.
        for &target in &candidates {
            let Some((cached, backend, slot)) = open_probe_target(target) else {
                continue;
            };
            if let Ok(fat16) = Fat16Volume::mount(cached) {
                self.install_volume(MountedVolume::Fat16(fat16), backend, slot);
                return Ok(());
            }
        }

        Err(VfsError::UnsupportedFilesystem)
    }

    pub fn mount_virtio(&mut self) -> Result<(), VfsError> {
        self.mount_probe_target(ProbeTarget::Virtio)
    }

    pub fn mount_ahci(&mut self) -> Result<(), VfsError> {
        let port = ahci::snapshot()
            .active_port
            .ok_or(VfsError::NoDisk)?;
        self.mount_probe_target(ProbeTarget::Ahci(port))
    }

    pub fn mount_nvme(&mut self) -> Result<(), VfsError> {
        self.mount_probe_target(ProbeTarget::Nvme)
    }

    pub fn mount_slot(
        &mut self,
        controller_index: usize,
        secondary: bool,
        slave: bool,
    ) -> Result<(), VfsError> {
        self.mount_probe_target(ProbeTarget::Ide(controller_index, secondary, slave))
    }

    pub fn ensure_disk_mounted(
        &mut self,
    ) -> Result<&mut MountedVolume<CachedBlockDevice<SystemBlockDevice>>, VfsError> {
        self.ramfs.ensure_initialized();
        if self.volume.is_none() && !self.disk_probed {
            let _ = self.mount_auto();
        }
        self.volume.as_mut().ok_or(VfsError::NotMounted)
    }

    pub fn resolve_path(&self, path: &str) -> Result<String, VfsError> {
        normalize_path(self.cwd(), path)
    }

    pub fn list_dir(&mut self, path: &str) -> Result<Vec<VfsDirEntry>, VfsError> {
        self.ramfs.ensure_initialized();
        if !self.disk_probed && self.volume.is_none() {
            let _ = self.mount_auto();
        }
        let abs = self.resolve_path(path)?;
        match split_mount_target(&abs) {
            DiskSubPath::DiskMount(sub) => self.ensure_disk_mounted()?.list_dir(sub),
            DiskSubPath::RootRamFs => {
                let mut entries = self.ramfs.list_dir(&abs)?;
                if abs == "/" && !entries.iter().any(|e| e.name == "disk") {
                    entries.insert(
                        0,
                        VfsDirEntry {
                            name: String::from("disk"),
                            kind: VfsNodeKind::Directory,
                            size: 0,
                            attributes: 0x10,
                            first_cluster: 0,
                        },
                    );
                }
                Ok(entries)
            }
        }
    }

    pub fn change_dir(&mut self, path: &str) -> Result<String, VfsError> {
        let target = if path.trim().is_empty() { "/" } else { path };
        let abs = self.resolve_path(target)?;
        let stat = self.stat(&abs)?;
        if stat.kind != VfsNodeKind::Directory {
            return Err(VfsError::NotADirectory);
        }
        self.cwd = abs.clone();
        Ok(abs)
    }

    pub fn stat(&mut self, path: &str) -> Result<VfsDirEntry, VfsError> {
        self.ramfs.ensure_initialized();
        if !self.disk_probed && self.volume.is_none() {
            let _ = self.mount_auto();
        }
        let abs = self.resolve_path(path)?;
        if abs == "/disk" {
            return Ok(VfsDirEntry {
                name: String::from("disk"),
                kind: VfsNodeKind::Directory,
                size: 0,
                attributes: 0x10,
                first_cluster: 0,
            });
        }
        match split_mount_target(&abs) {
            DiskSubPath::DiskMount(sub) => self.ensure_disk_mounted()?.stat(sub),
            DiskSubPath::RootRamFs => self.ramfs.stat(&abs),
        }
    }

    pub fn read_file_chunk(
        &mut self,
        path: &str,
        offset: u32,
        buffer: &mut [u8],
    ) -> Result<usize, VfsError> {
        self.ramfs.ensure_initialized();
        let abs = self.resolve_path(path)?;
        if abs == "/disk" {
            return Err(VfsError::IsADirectory);
        }
        match split_mount_target(&abs) {
            DiskSubPath::DiskMount(sub) => self.ensure_disk_mounted()?.read_file_chunk(sub, offset, buffer),
            DiskSubPath::RootRamFs => self.ramfs.read_file_chunk(&abs, offset, buffer),
        }
    }

    pub fn read_file(&mut self, path: &str) -> Result<Vec<u8>, VfsError> {
        self.ramfs.ensure_initialized();
        let abs = self.resolve_path(path)?;
        if abs == "/disk" {
            return Err(VfsError::IsADirectory);
        }
        match split_mount_target(&abs) {
            DiskSubPath::DiskMount(sub) => self.ensure_disk_mounted()?.read_file(sub),
            DiskSubPath::RootRamFs => self.ramfs.read_file(&abs),
        }
    }

    pub fn write_file(&mut self, path: &str, data: &[u8]) -> Result<(), VfsError> {
        self.ramfs.ensure_initialized();
        let abs = self.resolve_path(path)?;
        if abs == "/disk" {
            return Err(VfsError::IsADirectory);
        }
        match split_mount_target(&abs) {
            DiskSubPath::DiskMount(sub) => self.ensure_disk_mounted()?.write_file(sub, data),
            DiskSubPath::RootRamFs => self.ramfs.write_file(&abs, data),
        }
    }

    pub fn touch_or_truncate(
        &mut self,
        path: &str,
        create: bool,
        truncate: bool,
    ) -> Result<(), VfsError> {
        match self.stat(path) {
            Ok(st) => {
                if st.kind == VfsNodeKind::Directory {
                    return Err(VfsError::IsADirectory);
                }
                if truncate {
                    self.write_file(path, &[])?;
                }
                Ok(())
            }
            Err(VfsError::NotFound) if create => self.write_file(path, &[]),
            Err(e) => Err(e),
        }
    }

    pub fn write_file_at(
        &mut self,
        path: &str,
        offset: u32,
        data: &[u8],
    ) -> Result<usize, VfsError> {
        const MAX_USER_WRITE_FILE: usize = 2 * 1024 * 1024;
        if data.is_empty() {
            return Ok(0);
        }
        let end = (offset as usize)
            .checked_add(data.len())
            .ok_or(VfsError::NoSpace)?;
        if end > MAX_USER_WRITE_FILE {
            return Err(VfsError::NoSpace);
        }

        let mut buf = match self.read_file(path) {
            Ok(existing) => existing,
            Err(VfsError::NotFound) => Vec::new(),
            Err(e) => return Err(e),
        };
        if offset == 0 && data.len() >= buf.len() {
            self.write_file(path, data)?;
            return Ok(data.len());
        }
        if buf.len() < end {
            buf.resize(end, 0);
        }
        buf[offset as usize..end].copy_from_slice(data);
        self.write_file(path, &buf)?;
        Ok(data.len())
    }

    pub fn create_dir(&mut self, path: &str) -> Result<(), VfsError> {
        self.ramfs.ensure_initialized();
        let abs = self.resolve_path(path)?;
        if abs == "/disk" {
            return Err(VfsError::AlreadyExists);
        }
        match split_mount_target(&abs) {
            DiskSubPath::DiskMount(sub) => self.ensure_disk_mounted()?.create_dir(sub),
            DiskSubPath::RootRamFs => self.ramfs.create_dir(&abs),
        }
    }

    pub fn remove(&mut self, path: &str) -> Result<(), VfsError> {
        self.ramfs.ensure_initialized();
        let abs = self.resolve_path(path)?;
        if abs == "/" || abs == "/disk" {
            return Err(VfsError::PermissionDenied);
        }
        match split_mount_target(&abs) {
            DiskSubPath::DiskMount(sub) => self.ensure_disk_mounted()?.remove(sub),
            DiskSubPath::RootRamFs => self.ramfs.remove(&abs),
        }
    }

    pub fn remove_recursive(&mut self, path: &str) -> Result<(), VfsError> {
        self.ramfs.ensure_initialized();
        let abs = self.resolve_path(path)?;
        self.remove_recursive_inner(&abs, 0)
    }

    fn remove_recursive_inner(&mut self, abs_path: &str, depth: usize) -> Result<(), VfsError> {
        if depth > 16 {
            return Err(VfsError::InvalidPath);
        }
        if abs_path == "/" || abs_path == "/disk" {
            return Err(VfsError::PermissionDenied);
        }
        let st = self.stat(abs_path)?;
        if st.kind == VfsNodeKind::Directory {
            let entries = self.list_dir(abs_path)?;
            for entry in entries {
                if entry.name == "." || entry.name == ".." {
                    continue;
                }
                let child = join_abs_child(abs_path, &entry.name);
                self.remove_recursive_inner(&child, depth + 1)?;
            }
        }
        self.remove(abs_path)
    }

    pub fn copy_path(&mut self, src: &str, dst: &str) -> Result<(), VfsError> {
        self.ramfs.ensure_initialized();
        let abs_src = self.resolve_path(src)?;
        let abs_dst = self.resolve_path(dst)?;

        if abs_src == "/" || abs_src == "/disk" || abs_dst == "/" || abs_dst == "/disk" {
            return Err(VfsError::InvalidPath);
        }
        if abs_src == abs_dst {
            return Err(VfsError::AlreadyExists);
        }

        let src_stat = self.stat(&abs_src)?;
        if src_stat.kind == VfsNodeKind::Directory && is_descendant_path(&abs_dst, &abs_src) {
            return Err(VfsError::InvalidPath);
        }
        if self.stat(&abs_dst).is_ok() {
            return Err(VfsError::AlreadyExists);
        }

        self.copy_recursive_inner(&abs_src, &abs_dst, 0)
    }

    fn copy_recursive_inner(
        &mut self,
        abs_src: &str,
        abs_dst: &str,
        depth: usize,
    ) -> Result<(), VfsError> {
        if depth > 16 {
            return Err(VfsError::InvalidPath);
        }
        let st = self.stat(abs_src)?;
        if st.kind == VfsNodeKind::File {
            let data = self.read_file(abs_src)?;
            self.write_file(abs_dst, &data)?;
            return Ok::<_, VfsError>(());
        }

        self.create_dir(abs_dst)?;
        let entries = self.list_dir(abs_src)?;
        for entry in entries {
            if entry.name == "." || entry.name == ".." {
                continue;
            }
            let child_src = join_abs_child(abs_src, &entry.name);
            let child_dst = join_abs_child(abs_dst, &entry.name);
            self.copy_recursive_inner(&child_src, &child_dst, depth + 1)?;
        }
        Ok::<_, VfsError>(())
    }

    pub fn rename_path(&mut self, src: &str, dst: &str) -> Result<(), VfsError> {
        self.ramfs.ensure_initialized();
        let abs_src = self.resolve_path(src)?;
        let abs_dst = self.resolve_path(dst)?;

        if abs_src == "/" || abs_src == "/disk" || abs_dst == "/" || abs_dst == "/disk" {
            return Err(VfsError::PermissionDenied);
        }
        if abs_src == abs_dst {
            return Ok::<_, VfsError>(());
        }

        let src_stat = self.stat(&abs_src)?;
        if src_stat.kind == VfsNodeKind::Directory && is_descendant_path(&abs_dst, &abs_src) {
            return Err(VfsError::InvalidPath);
        }
        if self.stat(&abs_dst).is_ok() {
            return Err(VfsError::AlreadyExists);
        }
        if let Ok(src_info) = self.volume_info_for(&abs_src) {
            if src_info.read_only {
                return Err(VfsError::ReadOnlyFilesystem);
            }
        }
        if let Ok(dst_info) = self.volume_info_for(&abs_dst) {
            if dst_info.read_only {
                return Err(VfsError::ReadOnlyFilesystem);
            }
        }

        self.copy_recursive_inner(&abs_src, &abs_dst, 0)?;
        self.remove_recursive_inner(&abs_src, 0)?;
        Ok::<_, VfsError>(())
    }

    pub fn volume_info_for(&mut self, path: &str) -> Result<VfsVolumeInfo, VfsError> {
        self.ramfs.ensure_initialized();
        if !self.disk_probed && self.volume.is_none() {
            let _ = self.mount_auto();
        }
        let abs = self.resolve_path(path)?;
        match split_mount_target(&abs) {
            DiskSubPath::DiskMount(_) => self.ensure_disk_mounted()?.volume_info(),
            DiskSubPath::RootRamFs => self.ramfs.volume_info(),
        }
    }

    pub fn volume_info(&mut self) -> Result<VfsVolumeInfo, VfsError> {
        self.ramfs.ensure_initialized();
        if !self.disk_probed && self.volume.is_none() {
            let _ = self.mount_auto();
        }
        if let Some(ref mut vol) = self.volume {
            vol.volume_info()
        } else {
            self.ramfs.volume_info()
        }
    }
}

fn join_abs_child(parent: &str, name: &str) -> String {
    let mut s = String::from(parent);
    if !s.ends_with('/') {
        s.push('/');
    }
    s.push_str(name);
    s
}

fn is_descendant_path(candidate: &str, ancestor: &str) -> bool {
    if candidate == ancestor {
        return true;
    }
    if ancestor == "/" {
        return true;
    }
    if let Some(rest) = candidate.strip_prefix(ancestor) {
        rest.starts_with('/')
    } else {
        false
    }
}

pub static KERNEL_VFS: PreemptMutex<KernelVfs> = PreemptMutex::new(KernelVfs::new());

pub fn with_vfs<F, R>(f: F) -> R
where
    F: FnOnce(&mut KernelVfs) -> R,
{
    let mut guard = KERNEL_VFS.lock();
    f(&mut guard)
}

pub fn sync() -> Result<(), VfsError> {
    with_vfs(|vfs| vfs.sync())
}

pub fn current_working_directory() -> String {
    let guard = KERNEL_VFS.lock();
    String::from(guard.cwd())
}

// ============================================================
// PATH NORMALIZATION HELPERS
// ============================================================

pub fn normalize_path(cwd: &str, input: &str) -> Result<String, VfsError> {
    let trimmed = input.trim();
    if trimmed.contains('\0') {
        return Err(VfsError::InvalidPath);
    }

    let mut stack: Vec<&str> = Vec::new();

    if !trimmed.starts_with('/') {
        for part in cwd.split('/').filter(|s| !s.is_empty()) {
            stack.push(part);
        }
    }

    let source = if trimmed.is_empty() { "." } else { trimmed };

    for part in source.split('/') {
        if part.is_empty() || part == "." {
            continue;
        } else if part == ".." {
            let _ = stack.pop();
        } else {
            stack.push(part);
        }
    }

    if stack.is_empty() {
        return Ok(String::from("/"));
    }

    let mut out = String::new();
    for segment in stack {
        out.push('/');
        out.push_str(segment);
    }
    Ok(out)
}

fn split_parent_leaf(abs_path: &str) -> Result<(&str, &str), VfsError> {
    let trimmed = abs_path.trim_end_matches('/');
    if trimmed.is_empty() || trimmed == "/" {
        return Err(VfsError::InvalidPath);
    }

    match trimmed.rfind('/') {
        Some(0) => Ok(("/", &trimmed[1..])),
        Some(pos) => Ok((&trimmed[..pos], &trimmed[pos + 1..])),
        None => Ok(("/", trimmed)),
    }
}
