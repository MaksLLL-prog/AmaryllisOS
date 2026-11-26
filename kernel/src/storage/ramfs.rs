use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

use super::vfs::{FileSystem, VfsDirEntry, VfsError, VfsNodeKind, VfsVolumeInfo};

const RAMFS_MAX_BYTES: usize = 8 * 1024 * 1024; // 8 MiB soft cap

enum RamNode {
    File(Vec<u8>),
    Dir(BTreeMap<String, RamNode>),
}

pub struct RamFs {
    root: RamNode,
    initialized: bool,
    total_file_bytes: usize,
}

impl RamFs {
    pub const fn new() -> Self {
        Self {
            root: RamNode::Dir(BTreeMap::new()),
            initialized: false,
            total_file_bytes: 0,
        }
    }

    pub fn ensure_initialized(&mut self) {
        if self.initialized {
            return;
        }
        self.initialized = true;
        let _ = self.create_dir("/home");
        let _ = self.create_dir("/tmp");
        let _ = self.write_file(
            "/README.txt",
            b"Welcome to AmaryllisOS!\n\nRoot '/' uses writable RAMFS.\nPhysical IDE disks (FAT32/FAT16) are mounted at '/disk'.\nExternal Rust apps can be compiled via 'amaryllis-sdk/'.\n\nRing 3 Demos (double-click in Files or run in Terminal):\n  /counter.elf   Preemptive multitasking worker\n  /gui_demo.elf  External Ring 3 GUI window\n  /net_demo.elf  Ring 3 Ping & DNS syscall test\n  /hello.elf     Minimal Ring 3 console output\n\nShortcuts:\n  Alt+Space  Spotlight Launcher\n  Ctrl+S     Save in Text Editor\n  Ctrl+C/V   Copy / Paste across Editor & Terminal\n",
        );
        let _ = self.write_file(
            "/home/notes.txt",
            b"Amaryllis Notes\n---------------\nEdit and save me with Ctrl+S!\n",
        );
        let _ = self.write_file("/hello.elf", &crate::userland::demo::build_hello_elf());
        let _ = self.write_file("/counter.elf", &crate::userland::demo::build_counter_elf());
        let _ = self.write_file("/gui_demo.elf", &crate::userland::demo::build_gui_demo_elf());
        let _ = self.write_file("/net_demo.elf", &crate::userland::demo::build_net_demo_elf());
        let _ = self.write_file("/fault.elf", &crate::userland::demo::build_fault_elf());
        let _ = self.write_file("/fork_demo.elf", &crate::userland::demo::build_fork_demo_elf());
        let _ = self.write_file("/mach_ipc_demo.elf", &crate::userland::demo::build_mach_ipc_demo_elf());
        let _ = self.create_dir("/home/amaryllis");
        let _ = self.create_dir("/home/wallpapers");
        let _ = self.write_file("/home/wallpapers/nebula_glow.ppm", &build_sample_nebula_ppm());
        let _ = self.write_file("/home/wallpapers/slate_grid.bmp", &build_sample_grid_bmp());
    }

    fn navigate_dir_mut<'a>(&'a mut self, abs_path: &str) -> Result<&'a mut BTreeMap<String, RamNode>, VfsError> {
        let mut cur = match &mut self.root {
            RamNode::Dir(map) => map,
            RamNode::File(_) => return Err(VfsError::NotADirectory),
        };

        for seg in abs_path.split('/').filter(|s| !s.is_empty()) {
            let next = cur.get_mut(seg).ok_or(VfsError::NotFound)?;
            cur = match next {
                RamNode::Dir(map) => map,
                RamNode::File(_) => return Err(VfsError::NotADirectory),
            };
        }
        Ok(cur)
    }

    fn navigate_dir<'a>(&'a self, abs_path: &str) -> Result<&'a BTreeMap<String, RamNode>, VfsError> {
        let mut cur = match &self.root {
            RamNode::Dir(map) => map,
            RamNode::File(_) => return Err(VfsError::NotADirectory),
        };

        for seg in abs_path.split('/').filter(|s| !s.is_empty()) {
            let next = cur.get(seg).ok_or(VfsError::NotFound)?;
            cur = match next {
                RamNode::Dir(map) => map,
                RamNode::File(_) => return Err(VfsError::NotADirectory),
            };
        }
        Ok(cur)
    }

    fn resolve_node<'a>(&'a self, abs_path: &str) -> Result<&'a RamNode, VfsError> {
        if abs_path == "/" {
            return Ok(&self.root);
        }
        let (parent_path, leaf) = split_parent(abs_path)?;
        let parent = self.navigate_dir(parent_path)?;
        parent.get(leaf).ok_or(VfsError::NotFound)
    }
}

impl FileSystem for RamFs {
    fn fs_name(&self) -> &'static str {
        "RAMFS"
    }

    fn is_read_only(&self) -> bool {
        false
    }

    fn volume_info(&mut self) -> Result<VfsVolumeInfo, VfsError> {
        self.ensure_initialized();
        let used = self.total_file_bytes as u64;
        let cap = RAMFS_MAX_BYTES as u64;
        let free = cap.saturating_sub(used);
        Ok(VfsVolumeInfo {
            fs_type: "RAMFS",
            read_only: false,
            sector_size: 512,
            disk_sectors: cap / 512,
            partition_index: 0,
            partition_start_lba: 0,
            partition_sectors: (cap / 512) as u32,
            sectors_per_cluster: 8,
            cluster_count: (cap / 4096) as u32,
            free_clusters: Some((free / 4096) as u32),
            total_bytes: cap,
            free_bytes: Some(free),
        })
    }

    fn list_dir(&mut self, abs_path: &str) -> Result<Vec<VfsDirEntry>, VfsError> {
        self.ensure_initialized();
        let dir = self.navigate_dir(abs_path)?;
        let mut out = Vec::with_capacity(dir.len());
        for (name, node) in dir.iter() {
            match node {
                RamNode::Dir(_) => out.push(VfsDirEntry {
                    name: name.clone(),
                    kind: VfsNodeKind::Directory,
                    size: 0,
                    attributes: 0x10,
                    first_cluster: 0,
                }),
                RamNode::File(data) => out.push(VfsDirEntry {
                    name: name.clone(),
                    kind: VfsNodeKind::File,
                    size: data.len() as u32,
                    attributes: 0x20,
                    first_cluster: 0,
                }),
            }
        }
        Ok(out)
    }

    fn stat(&mut self, abs_path: &str) -> Result<VfsDirEntry, VfsError> {
        self.ensure_initialized();
        if abs_path == "/" {
            return Ok(VfsDirEntry {
                name: String::from("/"),
                kind: VfsNodeKind::Directory,
                size: 0,
                attributes: 0x10,
                first_cluster: 0,
            });
        }
        let (_, leaf) = split_parent(abs_path)?;
        let node = self.resolve_node(abs_path)?;
        match node {
            RamNode::Dir(_) => Ok(VfsDirEntry {
                name: String::from(leaf),
                kind: VfsNodeKind::Directory,
                size: 0,
                attributes: 0x10,
                first_cluster: 0,
            }),
            RamNode::File(data) => Ok(VfsDirEntry {
                name: String::from(leaf),
                kind: VfsNodeKind::File,
                size: data.len() as u32,
                attributes: 0x20,
                first_cluster: 0,
            }),
        }
    }

    fn read_file_chunk(
        &mut self,
        abs_path: &str,
        offset: u32,
        buffer: &mut [u8],
    ) -> Result<usize, VfsError> {
        self.ensure_initialized();
        let node = self.resolve_node(abs_path)?;
        let data = match node {
            RamNode::File(d) => d,
            RamNode::Dir(_) => return Err(VfsError::IsADirectory),
        };
        let off = offset as usize;
        if off > data.len() {
            return Err(VfsError::OffsetOutOfRange);
        }
        let avail = data.len() - off;
        let n = avail.min(buffer.len());
        buffer[..n].copy_from_slice(&data[off..off + n]);
        Ok(n)
    }

    fn read_file(&mut self, abs_path: &str) -> Result<Vec<u8>, VfsError> {
        self.ensure_initialized();
        let node = self.resolve_node(abs_path)?;
        match node {
            RamNode::File(d) => Ok(d.clone()),
            RamNode::Dir(_) => Err(VfsError::IsADirectory),
        }
    }

    fn write_file(&mut self, abs_path: &str, data: &[u8]) -> Result<(), VfsError> {
        self.ensure_initialized();
        let (parent_path, leaf) = split_parent(abs_path)?;
        validate_name(leaf)?;

        let old_len = match self.resolve_node(abs_path) {
            Ok(RamNode::Dir(_)) => return Err(VfsError::IsADirectory),
            Ok(RamNode::File(existing)) => existing.len(),
            Err(VfsError::NotFound) => 0,
            Err(e) => return Err(e),
        };

        let projected = self
            .total_file_bytes
            .saturating_sub(old_len)
            .saturating_add(data.len());
        if projected > RAMFS_MAX_BYTES {
            return Err(VfsError::NoSpace);
        }

        let parent = self.navigate_dir_mut(parent_path)?;
        parent.insert(String::from(leaf), RamNode::File(data.to_vec()));
        self.total_file_bytes = projected;
        Ok(())
    }

    fn create_dir(&mut self, abs_path: &str) -> Result<(), VfsError> {
        self.ensure_initialized();
        let (parent_path, leaf) = split_parent(abs_path)?;
        validate_name(leaf)?;
        let parent = self.navigate_dir_mut(parent_path)?;
        if parent.contains_key(leaf) {
            return Err(VfsError::AlreadyExists);
        }
        parent.insert(String::from(leaf), RamNode::Dir(BTreeMap::new()));
        Ok(())
    }

    fn remove(&mut self, abs_path: &str) -> Result<(), VfsError> {
        self.ensure_initialized();
        let (parent_path, leaf) = split_parent(abs_path)?;
        let parent = self.navigate_dir_mut(parent_path)?;
        match parent.get(leaf) {
            None => return Err(VfsError::NotFound),
            Some(RamNode::Dir(map)) if !map.is_empty() => {
                return Err(VfsError::DirectoryNotEmpty);
            }
            _ => {}
        }
        if let Some(RamNode::File(old)) = parent.remove(leaf) {
            self.total_file_bytes = self.total_file_bytes.saturating_sub(old.len());
        }
        Ok(())
    }
}

fn split_parent(abs_path: &str) -> Result<(&str, &str), VfsError> {
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

fn validate_name(name: &str) -> Result<(), VfsError> {
    if name.is_empty() || name == "." || name == ".." || name.contains('/') || name.contains('\0') {
        return Err(VfsError::InvalidName);
    }
    Ok(())
}

fn build_sample_nebula_ppm() -> Vec<u8> {
    const W: usize = 160;
    const H: usize = 90;
    let header = b"P6\n160 90\n255\n";
    let mut out = Vec::with_capacity(header.len() + W * H * 3);
    out.extend_from_slice(header);
    for y in 0..H {
        for x in 0..W {
            let dx = x as i32 - 104;
            let dy = y as i32 - 38;
            let d2 = dx * dx + dy * dy;
            let glow = (2200 - d2).max(0) / 16;
            let r = (14 + (x * 48) / W as usize + (glow as usize).min(160)).min(255) as u8;
            let g = (12 + (y * 28) / H as usize + ((glow as usize) / 3).min(90)).min(255) as u8;
            let b = (36 + ((W - x) * 64) / W as usize + (glow as usize).min(180)).min(255) as u8;
            out.push(r);
            out.push(g);
            out.push(b);
        }
    }
    out
}

fn build_sample_grid_bmp() -> Vec<u8> {
    const W: u32 = 160;
    const H: u32 = 90;
    let pixel_bytes = W * H * 3;
    let file_size = 54 + pixel_bytes;
    let mut out = Vec::with_capacity(file_size as usize);
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&file_size.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&54u32.to_le_bytes());
    // DIB BITMAPINFOHEADER (40 bytes)
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&(W as i32).to_le_bytes());
    out.extend_from_slice(&(H as i32).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&24u16.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&pixel_bytes.to_le_bytes());
    out.extend_from_slice(&2835u32.to_le_bytes());
    out.extend_from_slice(&2835u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());

    for y in 0..H {
        for x in 0..W {
            let is_line = (x % 16 == 0) || (y % 16 == 0);
            let b = if is_line { 64 } else { (22 + (y * 24) / H) as u8 };
            let g = if is_line { 48 } else { (16 + (x * 16) / W) as u8 };
            let r = if is_line { 96 } else { (18 + (x * 26) / W) as u8 };
            // BMP stores BGR
            out.push(b);
            out.push(g);
            out.push(r);
        }
    }
    out
}

