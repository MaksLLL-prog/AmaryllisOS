use crate::console::Console;
use crate::graphics::Color;
use crate::storage::ata::{AtaDeviceType, AtaProbeStatus};
use crate::storage::block::BlockDevice;
use crate::storage::ide::{self, IdeBlockDevice};
use crate::storage::partition::{self, PartitionStatus};
use crate::storage::vfs::{self, MountedBackend, VfsError, VfsNodeKind};

const WHITE: Color = (235, 235, 240);
const GREEN: Color = (40, 180, 120);
const GRAY: Color = (140, 140, 150);
const YELLOW: Color = (220, 190, 90);
const RED: Color = (220, 90, 90);

pub fn cmd_ls(console: &mut Console<'_>, arguments: &str) {
    let target = first_arg(arguments).unwrap_or(".");

    let result = vfs::with_vfs(|vfs_state| {
        let abs = vfs_state.resolve_path(target)?;
        let entries = vfs_state.list_dir(&abs)?;
        Ok((abs, entries))
    });

    let (abs_path, entries) = match result {
        Ok(data) => data,
        Err(error) => {
            print_vfs_error(console, "ls", error);
            return;
        }
    };

    console.write("Directory: ", GREEN);
    console.write(&abs_path, WHITE);
    console.write("\n", WHITE);
    console.write("TYPE   SIZE       CLUSTER    NAME\n", GRAY);
    console.write("--------------------------------------------------\n", GRAY);

    let mut files = 0u64;
    let mut dirs = 0u64;

    for entry in &entries {
        if entry.kind == VfsNodeKind::Directory {
            console.write("<DIR>  ", YELLOW);
            console.write("         ", WHITE);
            dirs += 1;
        } else {
            console.write("FILE   ", GREEN);
            write_padded_u64(console, entry.size as u64, 9);
            files += 1;
        }

        console.write("  ", WHITE);
        write_padded_u64(console, entry.first_cluster as u64, 9);
        console.write("  ", WHITE);
        console.write(
            &entry.name,
            if entry.kind == VfsNodeKind::Directory {
                YELLOW
            } else {
                WHITE
            },
        );
        console.write("\n", WHITE);
    }

    console.write("\nTotal: ", GRAY);
    write_decimal(console, files);
    console.write(" files, ", GRAY);
    write_decimal(console, dirs);
    console.write(" directories\n", GRAY);
}

pub fn cmd_cd(console: &mut Console<'_>, arguments: &str) {
    let target = first_arg(arguments).unwrap_or("/");

    let result = vfs::with_vfs(|vfs_state| vfs_state.change_dir(target));

    match result {
        Ok(_) => {}
        Err(error) => print_vfs_error(console, "cd", error),
    }
}

pub fn cmd_pwd(console: &mut Console<'_>) {
    let cwd = vfs::current_working_directory();
    console.write(&cwd, WHITE);
    console.write("\n", WHITE);
}

pub fn cmd_cat(console: &mut Console<'_>, arguments: &str) {
    let Some(target) = first_arg(arguments) else {
        console.write("Usage: cat <path>\n", YELLOW);
        return;
    };

    let stat = match vfs::with_vfs(|vfs_state| vfs_state.stat(target)) {
        Ok(stat) => stat,
        Err(error) => {
            print_vfs_error(console, "cat", error);
            return;
        }
    };

    if stat.kind == VfsNodeKind::Directory {
        print_vfs_error(console, "cat", VfsError::IsADirectory);
        return;
    }

    if stat.size == 0 {
        return;
    }

    const CHUNK_SIZE: usize = 512;
    const MAX_DISPLAY_BYTES: u32 = 16 * 1024;
    let mut offset = 0u32;
    let limit = core::cmp::min(stat.size, MAX_DISPLAY_BYTES);
    let mut buffer = [0u8; CHUNK_SIZE];

    while offset < limit {
        let to_read = core::cmp::min(CHUNK_SIZE, (limit - offset) as usize);
        let read = match vfs::with_vfs(|vfs_state| {
            vfs_state.read_file_chunk(target, offset, &mut buffer[..to_read])
        }) {
            Ok(n) => n,
            Err(error) => {
                print_vfs_error(console, "cat", error);
                return;
            }
        };

        if read == 0 {
            break;
        }

        print_text_bytes(console, &buffer[..read]);
        offset += read as u32;
    }

    console.write("\n", WHITE);

    if stat.size > MAX_DISPLAY_BYTES {
        console.write("[cat output truncated at 16 KiB; total size: ", YELLOW);
        write_decimal(console, stat.size as u64);
        console.write(" bytes]\n", YELLOW);
    }
}

pub fn cmd_write(console: &mut Console<'_>, arguments: &str) {
    let Some((path, raw_content)) = split_first_arg(arguments) else {
        console.write("Usage: write <path> <text...>\n", YELLOW);
        return;
    };

    let content = unquote_text(raw_content);

    let result = vfs::with_vfs(|vfs_state| vfs_state.write_file(path, content.as_bytes()));

    match result {
        Ok(()) => {
            console.write("Wrote ", GREEN);
            write_decimal(console, content.len() as u64);
            console.write(" bytes to ", WHITE);
            console.write(path, WHITE);
            console.write("\n", WHITE);
        }
        Err(error) => print_vfs_error(console, "write", error),
    }
}

pub fn cmd_mkdir(console: &mut Console<'_>, arguments: &str) {
    let Some(path) = first_arg(arguments) else {
        console.write("Usage: mkdir <path>\n", YELLOW);
        return;
    };

    let result = vfs::with_vfs(|vfs_state| vfs_state.create_dir(path));

    match result {
        Ok(()) => {
            console.write("Created directory: ", GREEN);
            console.write(path, WHITE);
            console.write("\n", WHITE);
        }
        Err(error) => print_vfs_error(console, "mkdir", error),
    }
}

pub fn cmd_rm(console: &mut Console<'_>, arguments: &str) {
    let Some(path) = first_arg(arguments) else {
        console.write("Usage: rm <path>\n", YELLOW);
        return;
    };

    let result = vfs::with_vfs(|vfs_state| vfs_state.remove(path));

    match result {
        Ok(()) => {
            console.write("Removed: ", GREEN);
            console.write(path, WHITE);
            console.write("\n", WHITE);
        }
        Err(error) => print_vfs_error(console, "rm", error),
    }
}

pub fn cmd_sync(console: &mut Console<'_>) {
    match vfs::sync() {
        Ok(()) => {
            let stats = crate::storage::cache::stats_snapshot();
            console.write("VFS Page Cache flushed (", GREEN);
            write_decimal(console, stats.flushes);
            console.write(" flushes, ", GRAY);
            write_decimal(console, stats.writeback_sectors);
            console.write(" sectors written, ", GRAY);
            write_decimal(console, stats.dirty_pages as u64);
            console.write(" dirty)\n", GRAY);
        }
        Err(err) => print_vfs_error(console, "sync", err),
    }
}

pub fn cmd_mount(console: &mut Console<'_>, arguments: &str) {
    let target = first_arg(arguments).unwrap_or("auto");

    let res = match target {
        "auto" => vfs::with_vfs(|v| v.mount_auto()),
        "nvme" => vfs::with_vfs(|v| v.mount_nvme()),
        "virtio" => vfs::with_vfs(|v| v.mount_virtio()),
        "ahci" => vfs::with_vfs(|v| v.mount_ahci()),
        "usb" => vfs::with_vfs(|v| v.mount_usb()),
        "sd" | "sdhci" => vfs::with_vfs(|v| v.mount_sd()),
        "pm" | "0" => vfs::with_vfs(|v| v.mount_slot(0, false, false)),
        "ps" | "1" => vfs::with_vfs(|v| v.mount_slot(0, false, true)),
        "sm" | "2" => vfs::with_vfs(|v| v.mount_slot(0, true, false)),
        "ss" | "3" => vfs::with_vfs(|v| v.mount_slot(0, true, true)),
        _ => {
            console.write("Usage: mount [auto|nvme|virtio|ahci|usb|sd|pm|ps|sm|ss]\n", YELLOW);
            return;
        }
    };

    match res {
        Ok(()) => {
            console.write("Mounted volume successfully.\n", GREEN);
            cmd_diskinfo(console);
        }
        Err(err) => print_vfs_error(console, "mount", err),
    }
}

pub fn cmd_diskinfo(console: &mut Console<'_>) {
    console.write("\nStorage & Filesystem Info\n", GREEN);
    console.write("--------------------------------------------------\n", GRAY);

    let controllers = ide::controller_count();
    console.write("IDE controllers: ", GRAY);
    write_decimal(console, controllers as u64);
    console.write("\n", WHITE);

    for ctrl_idx in 0..controllers {
        let Some(ctrl) = ide::get_controller(ctrl_idx) else {
            continue;
        };

        for (chan_name, secondary, devs) in [
            ("Primary", false, &ctrl.primary_devices),
            ("Secondary", true, &ctrl.secondary_devices),
        ] {
            for (role, slave, dev) in [("Master", false, &devs.master), ("Slave", true, &devs.slave)]
            {
                if dev.status != AtaProbeStatus::Present {
                    continue;
                }

                console.write("\n[IDE ", GREEN);
                write_decimal(console, ctrl_idx as u64);
                console.write("] ", GREEN);
                console.write(chan_name, WHITE);
                console.write(" ", WHITE);
                console.write(role, WHITE);
                console.write(" (", GRAY);
                match dev.device_type {
                    Some(AtaDeviceType::Ata) => console.write("ATA", GREEN),
                    Some(AtaDeviceType::Atapi) => console.write("ATAPI", YELLOW),
                    None => console.write("UNKNOWN", GRAY),
                }
                console.write(")\n", GRAY);

                console.write("  Model:       ", GRAY);
                write_trimmed_ascii(console, &dev.model);
                console.write("\n", WHITE);

                console.write("  Firmware:    ", GRAY);
                write_trimmed_ascii(console, &dev.firmware);
                console.write("\n", WHITE);

                if dev.device_type == Some(AtaDeviceType::Ata) {
                    let bytes = dev
                        .capacity_sectors
                        .saturating_mul(dev.logical_sector_size as u64);
                    console.write("  Geometry:    ", GRAY);
                    write_decimal(console, dev.capacity_sectors);
                    console.write(" sectors x ", GRAY);
                    write_decimal(console, dev.logical_sector_size as u64);
                    console.write(" B (", GRAY);
                    write_capacity(console, bytes);
                    console.write(")\n", GRAY);

                    if let Ok(mut blk) = IdeBlockDevice::from_slot(ctrl_idx, secondary, slave) {
                        display_mbr_summary(console, &mut blk);
                    }
                }
            }
        }
    }

    console.write("\nVFS Mount Topology\n", GREEN);
    console.write("--------------------------------------------------\n", GRAY);
    console.write("  Root /:      ", GRAY);
    console.write("RAMFS (read-write, 8 MiB soft cap)\n", GREEN);
    console.write("  Disk /disk:  ", GRAY);

    let mount_info = vfs::with_vfs(|vfs_state| {
        let cwd = alloc::string::String::from(vfs_state.cwd());
        let backend = vfs_state.mounted_backend();
        let disk_mounted = vfs_state.is_disk_mounted();
        let info = if disk_mounted {
            vfs_state.volume_info_for("/disk")
        } else {
            Err(VfsError::NotMounted)
        };
        (cwd, backend, info, disk_mounted)
    });

    if !mount_info.3 {
        console.write("Not attached (no FAT32/FAT16 volume)\n", YELLOW);
    } else {
        console.write("Mounted\n", GREEN);
    }

    match mount_info.2 {
        Ok(info) => {
            console.write("  Filesystem:  ", GRAY);
            console.write(info.fs_type, GREEN);
            console.write(
                if info.read_only {
                    " (read-only)\n"
                } else {
                    " (read-write)\n"
                },
                if info.read_only { YELLOW } else { GREEN },
            );

            match mount_info.1 {
                Some(MountedBackend::VirtioBlk { max_dma_sectors }) => {
                    console.write("  Backend:     ", GRAY);
                    console.write("VirtIO-Blk PCI Split-Queue DMA (max ", GREEN);
                    write_decimal(console, max_dma_sectors as u64);
                    console.write(" sectors/batch)\n", GRAY);
                }
                Some(MountedBackend::AhciSata { port, max_dma_sectors }) => {
                    console.write("  Backend:     ", GRAY);
                    console.write("AHCI SATA DMA Port ", GREEN);
                    write_decimal(console, port as u64);
                    console.write(" (max ", GRAY);
                    write_decimal(console, max_dma_sectors as u64);
                    console.write(" sectors/batch)\n", GRAY);
                }
                Some(MountedBackend::Nvme { max_dma_sectors, lba_size }) => {
                    console.write("  Backend:     ", GRAY);
                    console.write("NVMe PCI Express DMA (FreeBSD KPI, ", GREEN);
                    write_decimal(console, lba_size as u64);
                    console.write("B LBA, max ", GRAY);
                    write_decimal(console, max_dma_sectors as u64);
                    console.write(" sectors/batch)\n", GRAY);
                }
                Some(MountedBackend::UsbMass { max_dma_sectors }) => {
                    console.write("  Backend:     ", GRAY);
                    console.write("USB Mass Storage xHCI BOT (max ", GREEN);
                    write_decimal(console, max_dma_sectors as u64);
                    console.write(" sectors/batch)\n", GRAY);
                }
                Some(MountedBackend::Sdhci { max_dma_sectors, sdhc }) => {
                    console.write("  Backend:     ", GRAY);
                    console.write(if sdhc { "SDHCI SDHC v2 DMA (max " } else { "SDHCI SDSC DMA (max " }, GREEN);
                    write_decimal(console, max_dma_sectors as u64);
                    console.write(" sectors/batch)\n", GRAY);
                }
                Some(MountedBackend::IdeAta { controller, secondary, slave }) => {
                    console.write("  Backend:     ", GRAY);
                    console.write("IDE", WHITE);
                    write_decimal(console, controller as u64);
                    console.write(if secondary { " Secondary " } else { " Primary " }, WHITE);
                    console.write(if slave { "Slave (ATA PIO)\n" } else { "Master (ATA PIO)\n" }, WHITE);
                }
                None => {}
            }

            console.write("  Partition:   #", GRAY);
            write_decimal(console, info.partition_index as u64);
            console.write(" (LBA ", GRAY);
            write_decimal(console, info.partition_start_lba as u64);
            console.write(", ", GRAY);
            write_decimal(console, info.partition_sectors as u64);
            console.write(" sectors)\n", GRAY);

            console.write("  Cluster size:", GRAY);
            console.write(" ", WHITE);
            write_decimal(console, info.sectors_per_cluster as u64 * info.sector_size as u64);
            console.write(" bytes (", GRAY);
            write_decimal(console, info.sectors_per_cluster as u64);
            console.write(" sectors)\n", GRAY);

            console.write("  Clusters:    ", GRAY);
            write_decimal(console, info.cluster_count as u64);
            if let Some(free) = info.free_clusters {
                console.write(" total, ", GRAY);
                write_decimal(console, free as u64);
                console.write(" free", GREEN);
            }
            console.write("\n", WHITE);

            console.write("  Total space: ", GRAY);
            write_capacity(console, info.total_bytes);
            console.write("\n", WHITE);

            if let Some(free_bytes) = info.free_bytes {
                console.write("  Free space:  ", GRAY);
                write_capacity(console, free_bytes);
                console.write("\n", WHITE);
            }

            console.write("  Current dir: ", GRAY);
            console.write(&mount_info.0, WHITE);
            console.write("\n", WHITE);

            let cache = crate::storage::cache::stats_snapshot();
            let total_ops = cache.hits + cache.misses;
            let hit_pct = if total_ops > 0 {
                (cache.hits * 100) / total_ops
            } else {
                0
            };
            console.write("  Page Cache:  ", GRAY);
            write_decimal(console, (cache.cached_pages * 4) as u64);
            console.write(" / ", GRAY);
            write_decimal(console, (cache.max_pages * 4) as u64);
            console.write(" KiB (", WHITE);
            write_decimal(console, cache.dirty_pages as u64);
            console.write(" dirty)  Hits: ", GRAY);
            write_decimal(console, hit_pct);
            console.write("% (", GREEN);
            write_decimal(console, cache.hits);
            console.write("h / ", GRAY);
            write_decimal(console, cache.misses);
            console.write("m)  RA: ", GRAY);
            write_decimal(console, cache.readahead_pages);
            console.write("p\n", GRAY);
        }
        Err(error) => {
            console.write("  Status:      ", GRAY);
            console.write(error.message(), YELLOW);
            console.write("\n", WHITE);
        }
    }
}

fn display_mbr_summary(console: &mut Console<'_>, disk: &mut IdeBlockDevice) {
    let mut mbr_sector = [0u8; 512];
    if disk.read_sector(0, &mut mbr_sector).is_err() {
        console.write("  MBR:         read error\n", RED);
        return;
    }

    match partition::parse_mbr(&mbr_sector, disk.sector_count()) {
        Ok(table) => {
            if table.count == 0 {
                console.write("  MBR:         valid, no partitions\n", YELLOW);
                return;
            }
            for part in table.partitions.iter() {
                if part.status == PartitionStatus::Empty {
                    continue;
                }
                console.write("  Part #", GRAY);
                write_decimal(console, part.index as u64);
                console.write(":     ", GRAY);
                console.write(partition::partition_type_name(part.partition_type), WHITE);
                console.write("  start=", GRAY);
                write_decimal(console, part.start_lba as u64);
                console.write("  sectors=", GRAY);
                write_decimal(console, part.sector_count as u64);
                console.write("\n", WHITE);
            }
        }
        Err(_) => {
            console.write("  MBR:         no MBR signature (raw/superfloppy?)\n", YELLOW);
        }
    }
}

// ============================================================
// ARGUMENT PARSING & FORMATTING HELPERS
// ============================================================

fn first_arg(input: &str) -> Option<&str> {
    split_first_arg(input).map(|(arg, _)| arg)
}

fn split_first_arg(input: &str) -> Option<(&str, &str)> {
    let trimmed = input.trim_start();
    if trimmed.is_empty() {
        return None;
    }

    if let Some(after_quote) = trimmed.strip_prefix('"') {
        if let Some(end_quote) = after_quote.find('"') {
            let arg = &after_quote[..end_quote];
            let rest = after_quote[end_quote + 1..].trim_start();
            return Some((arg, rest));
        }
    }

    match trimmed.find(char::is_whitespace) {
        Some(pos) => Some((&trimmed[..pos], trimmed[pos..].trim_start())),
        None => Some((trimmed, "")),
    }
}

fn unquote_text(text: &str) -> &str {
    let trimmed = text.trim_end();
    if trimmed.len() >= 2 && trimmed.starts_with('"') && trimmed.ends_with('"') {
        &trimmed[1..trimmed.len() - 1]
    } else {
        trimmed
    }
}

fn print_vfs_error(console: &mut Console<'_>, command: &str, error: VfsError) {
    console.write(command, RED);
    console.write(": ", RED);
    console.write(error.message(), RED);
    console.write("\n", WHITE);
}

fn print_text_bytes(console: &mut Console<'_>, bytes: &[u8]) {
    for &byte in bytes {
        if byte == b'\n' || byte == b'\t' || (32..=126).contains(&byte) {
            let ch = [byte];
            console.write(core::str::from_utf8(&ch).unwrap_or("."), WHITE);
        } else if byte == b'\r' {
            continue;
        } else {
            console.write(".", GRAY);
        }
    }
}

fn write_trimmed_ascii(console: &mut Console<'_>, bytes: &[u8]) {
    let mut end = bytes.len();
    while end > 0 && (bytes[end - 1] == 0 || bytes[end - 1] == b' ') {
        end -= 1;
    }
    if end == 0 {
        console.write("UNKNOWN", YELLOW);
    } else if let Ok(s) = core::str::from_utf8(&bytes[..end]) {
        console.write(s, WHITE);
    } else {
        console.write("INVALID", YELLOW);
    }
}

fn write_decimal(console: &mut Console<'_>, mut value: u64) {
    if value == 0 {
        console.write("0", WHITE);
        return;
    }

    let mut digits = [0u8; 20];
    let mut idx = digits.len();

    while value > 0 {
        idx -= 1;
        digits[idx] = b'0' + (value % 10) as u8;
        value /= 10;
    }

    if let Ok(s) = core::str::from_utf8(&digits[idx..]) {
        console.write(s, WHITE);
    }
}

fn write_padded_u64(console: &mut Console<'_>, value: u64, width: usize) {
    let mut digits = [0u8; 20];
    let mut idx = digits.len();

    if value == 0 {
        idx -= 1;
        digits[idx] = b'0';
    } else {
        let mut n = value;
        while n > 0 {
            idx -= 1;
            digits[idx] = b'0' + (n % 10) as u8;
            n /= 10;
        }
    }

    let len = digits.len() - idx;
    for _ in len..width {
        console.write(" ", WHITE);
    }
    if let Ok(s) = core::str::from_utf8(&digits[idx..]) {
        console.write(s, WHITE);
    }
}

fn write_capacity(console: &mut Console<'_>, bytes: u64) {
    const MIB: u64 = 1024 * 1024;
    const GIB: u64 = 1024 * MIB;
    const TIB: u64 = 1024 * GIB;

    if bytes >= TIB {
        write_scaled(console, bytes, TIB);
        console.write(" TiB", WHITE);
    } else if bytes >= GIB {
        write_scaled(console, bytes, GIB);
        console.write(" GiB", WHITE);
    } else if bytes >= MIB {
        write_scaled(console, bytes, MIB);
        console.write(" MiB", WHITE);
    } else {
        write_decimal(console, bytes);
        console.write(" bytes", WHITE);
    }
}

fn write_scaled(console: &mut Console<'_>, bytes: u64, unit: u64) {
    let whole = bytes / unit;
    let frac = ((bytes % unit) * 100) / unit;

    write_decimal(console, whole);
    console.write(".", WHITE);
    if frac < 10 {
        console.write("0", WHITE);
    }
    write_decimal(console, frac);
}
