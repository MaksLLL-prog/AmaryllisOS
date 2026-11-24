use crate::console::Console;
use crate::graphics::Color;
use crate::storage::ata::{
    self,
    AtaChannelDevices,
    AtaDevice,
    AtaDeviceType,
    AtaIoError,
    AtaProbeStatus,
};
use crate::storage::fat16::{Fat16Error, Fat16File, Fat16Volume};
use crate::storage::ide::{
    self,
    IdeBlockDevice,
};

const WHITE: Color = (235, 235, 240);
const GREEN: Color = (40, 180, 120);
const GRAY: Color = (140, 140, 150);
const YELLOW: Color = (220, 190, 90);
const RED: Color = (220, 90, 90);

const HEX: &[u8; 16] = b"0123456789ABCDEF";

pub fn execute(console: &mut Console<'_>, arguments: &str) {
    let mut parts = arguments.split_whitespace();

    match parts.next() {
        Some("devices") => display_devices(console),
        Some("partitions") => display_partitions(console),
        Some("ls") => display_fat16_root(console),

        Some("cat") => {
            let Some(filename) = parts.next() else {
                console.write("Usage: ata cat <FILENAME>\n", YELLOW);
                return;
            };

            display_fat16_file(console, filename);
        }

        Some("fileinfo") => {
            let Some(filename) = parts.next() else {
                console.write("Usage: ata fileinfo <FILENAME>\n", YELLOW);
                return;
            };

            display_fat16_file_info(console, filename);
        }

        Some("extract") => {
            let Some(filename) = parts.next() else {
                console.write("Usage: ata extract <FILE> <OFFSET> <LENGTH>\n", YELLOW);
                return;
            };

            let Some(offset) = parts.next().and_then(parse_number) else {
                console.write("Invalid offset.\n", RED);
                return;
            };

            let Some(length) = parts.next().and_then(parse_number) else {
                console.write("Invalid length.\n", RED);
                return;
            };

            display_fat16_extract(console, filename, offset, length);
        }

        Some("read") => {
            let Some(value) = parts.next() else {
                console.write("Usage: ata read <LBA>\n", YELLOW);
                return;
            };

            let Some(lba) = parse_number(value) else {
                console.write("Invalid LBA value.\n", RED);
                return;
            };

            read_sector(console, lba);
        }

        Some("help") | None => display_help(console),

        Some(command) => {
            console.write("Unknown ATA command: ", RED);
            console.write(command, WHITE);
            console.write("\nUse: ata help\n", GRAY);
        }
    }
}

fn display_help(console: &mut Console<'_>) {
    console.write("\nATA Commands:\n", GREEN);
    console.write("  ata devices       List detected ATA devices\n", WHITE);
    console.write("  ata read <LBA>    Read one 512-byte sector\n", WHITE);
    console.write("  ata partitions    Show MBR partition table\n", WHITE);
    console.write("  ata ls            List FAT16 root directory\n", WHITE);
    console.write("  ata cat <FILE>    Read a FAT16 file\n", WHITE);
    console.write("  ata fileinfo <FILE> Inspect a FAT16 file\n", WHITE);
    console.write("  ata extract <FILE> <OFFSET> <LENGTH>\n", WHITE);
    console.write("  ata help          Show this help\n", WHITE);
}

fn display_devices(console: &mut Console<'_>) {
    let count = ide::controller_count();

    console.write("\nIDE Runtime Devices:\n", GREEN);

    if count == 0 {
        console.write("No IDE controllers registered.\n", YELLOW);
        return;
    }

    for index in 0..count {
        let Some(controller) = ide::get_controller(index) else {
            continue;
        };

        console.write("\nController ", GREEN);
        write_decimal(console, index as u64);
        console.write("\n", WHITE);

        console.write("  PCI address: ", GRAY);
        write_hex_byte(console, controller.info.bus);
        console.write(":", WHITE);
        write_hex_byte(console, controller.info.device);
        console.write(".", WHITE);
        write_hex_byte(console, controller.info.function);
        console.write("\n", WHITE);

        display_channel_devices(console, "Primary", &controller.primary_devices);
        display_channel_devices(console, "Secondary", &controller.secondary_devices);
    }

    console.write("\nRuntime controllers: ", GRAY);
    write_decimal(console, count as u64);
    console.write("\n", WHITE);

    console.write("Runtime overflow: ", GRAY);
    write_decimal(console, ide::controller_overflow() as u64);
    console.write("\n", WHITE);
}

fn display_channel_devices(
    console: &mut Console<'_>,
    name: &str,
    devices: &AtaChannelDevices,
) {
    console.write("\n  ", GREEN);
    console.write(name, GREEN);
    console.write(" Channel:\n", GREEN);

    display_device(console, "Master", &devices.master);
    display_device(console, "Slave", &devices.slave);
}

fn display_device(console: &mut Console<'_>, name: &str, device: &AtaDevice) {
    console.write("    ", GRAY);
    console.write(name, GRAY);
    console.write(": ", GRAY);

    match device.status {
        AtaProbeStatus::Present => {
            match device.device_type {
                Some(AtaDeviceType::Ata) => console.write("ATA\n", GREEN),
                Some(AtaDeviceType::Atapi) => console.write("ATAPI\n", GREEN),
                None => console.write("UNKNOWN\n", YELLOW),
            }

            display_string(console, "Model", &device.model);
            display_string(console, "Serial", &device.serial);
            display_string(console, "Firmware", &device.firmware);

            if device.device_type == Some(AtaDeviceType::Ata) {
                display_capabilities(console, device);
            }
        }
        AtaProbeStatus::Absent => console.write("ABSENT\n", YELLOW),
        AtaProbeStatus::Unsupported => console.write("UNSUPPORTED\n", YELLOW),
        AtaProbeStatus::Timeout => console.write("TIMEOUT\n", RED),
        AtaProbeStatus::DeviceError => console.write("DEVICE ERROR\n", RED),
    }
}

fn display_capabilities(console: &mut Console<'_>, device: &AtaDevice) {
    console.write("      LBA: ", GRAY);
    display_supported(console, device.lba_supported);

    console.write("      LBA48: ", GRAY);
    display_supported(console, device.lba48_supported);

    console.write("      LBA28 sectors: ", GRAY);
    write_decimal(console, device.sectors_lba28);
    console.write("\n", WHITE);

    console.write("      LBA48 sectors: ", GRAY);
    write_decimal(console, device.sectors_lba48);
    console.write("\n", WHITE);

    console.write("      Total sectors: ", GRAY);
    write_decimal(console, device.capacity_sectors);
    console.write("\n", WHITE);

    console.write("      Sector size: ", GRAY);
    write_decimal(console, device.logical_sector_size as u64);
    console.write(" bytes\n", WHITE);

    let capacity_bytes = device
        .capacity_sectors
        .saturating_mul(device.logical_sector_size as u64);

    console.write("      Capacity: ", GRAY);
    write_capacity(console, capacity_bytes);
    console.write("\n", WHITE);
}

fn display_supported(console: &mut Console<'_>, supported: bool) {
    if supported {
        console.write("YES\n", GREEN);
    } else {
        console.write("NO\n", YELLOW);
    }
}

fn read_sector(console: &mut Console<'_>, lba: u64) {
    console.write("\nATA Sector Read:\n", GREEN);

    let Some(controller) = ide::get_controller(0) else {
        console.write("No IDE controller available.\n", RED);
        return;
    };

    let device = &controller.primary_devices.master;

    if device.status != AtaProbeStatus::Present {
        console.write("Primary Master is not present.\n", RED);
        return;
    }

    if device.device_type != Some(AtaDeviceType::Ata) {
        console.write("Primary Master is not an ATA disk.\n", YELLOW);
        return;
    }

    if device.logical_sector_size != 512 {
        console.write("Unsupported logical sector size.\n", RED);
        return;
    }

    if !device.lba_supported {
        console.write("Device does not support LBA.\n", RED);
        return;
    }

    if lba >= device.capacity_sectors {
        console.write("LBA is outside the disk capacity.\n", RED);
        return;
    }

    let mut buffer = [0u8; 512];

    console.write("  Device: ", GRAY);
    write_ata_string(console, &device.model);
    console.write("\n  LBA: ", GRAY);
    write_decimal(console, lba);
    console.write("\n  Reading...\n", YELLOW);

    match ata::read_sector(&controller.info.primary, device, false, lba, &mut buffer) {
        Ok(()) => {
            console.write("  Read successful.\n", GREEN);
            display_hex_dump(console, &buffer);

            console.write("\nMBR signature: ", GRAY);
            if buffer[510] == 0x55 && buffer[511] == 0xAA {
                console.write("VALID (55 AA)\n", GREEN);
            } else {
                console.write("NOT FOUND\n", YELLOW);
            }
        }
        Err(error) => {
            console.write("  Read failed: ", RED);
            display_io_error(console, error);
            console.write("\n", WHITE);
        }
    }
}

fn display_partitions(console: &mut Console<'_>) {
    console.write("\nMBR Partition Table:\n", GREEN);

    let Some(controller) = ide::get_controller(0) else {
        console.write("No IDE controller available.\n", RED);
        return;
    };

    let device = &controller.primary_devices.master;

    if device.status != AtaProbeStatus::Present
        || device.device_type != Some(AtaDeviceType::Ata)
    {
        console.write("Primary Master ATA disk unavailable.\n", RED);
        return;
    }

    if device.logical_sector_size != 512 || !device.lba_supported {
        console.write("Unsupported disk configuration.\n", RED);
        return;
    }

    let mut sector = [0u8; 512];

    if let Err(error) = ata::read_sector(&controller.info.primary, device, false, 0, &mut sector) {
        console.write("Unable to read MBR: ", RED);
        display_io_error(console, error);
        console.write("\n", WHITE);
        return;
    }

    let table = match crate::storage::partition::parse_mbr(&sector, device.capacity_sectors) {
        Ok(table) => table,
        Err(_) => {
            console.write("Invalid MBR signature.\n", RED);
            return;
        }
    };

    console.write("MBR signature: VALID (55 AA)\n", GREEN);

    if table.protective_gpt {
        console.write("Protective GPT partition detected.\n", YELLOW);
    }

    if table.count == 0 {
        console.write("No partitions found.\n", YELLOW);
        return;
    }

    for partition in table.partitions.iter() {
        if partition.status == crate::storage::partition::PartitionStatus::Empty {
            continue;
        }

        console.write("\nPartition ", GREEN);
        write_decimal(console, partition.index as u64);
        console.write("\n", WHITE);

        console.write("  Type: ", GRAY);
        write_hex_byte(console, partition.partition_type);
        console.write(" (", WHITE);
        console.write(
            crate::storage::partition::partition_type_name(partition.partition_type),
            WHITE,
        );
        console.write(")\n", WHITE);

        console.write("  Bootable: ", GRAY);
        if partition.bootable {
            console.write("YES\n", GREEN);
        } else {
            console.write("NO\n", WHITE);
        }

        console.write("  Start LBA: ", GRAY);
        write_decimal(console, partition.start_lba as u64);
        console.write("\n", WHITE);

        console.write("  Sector count: ", GRAY);
        write_decimal(console, partition.sector_count as u64);
        console.write("\n", WHITE);

        console.write("  End LBA (exclusive): ", GRAY);
        write_decimal(console, partition.end_lba_exclusive);
        console.write("\n", WHITE);

        if partition.status != crate::storage::partition::PartitionStatus::Valid {
            console.write("  Status: INVALID\n", RED);
            continue;
        }

        console.write("  Status: VALID\n", GREEN);

        if crate::storage::partition::is_extended_partition(partition.partition_type)
            || partition.partition_type == 0xEE
        {
            console.write("  Filesystem: Extended/GPT container\n", YELLOW);
            continue;
        }

        let mut vbr = [0u8; 512];

        match ata::read_sector(
            &controller.info.primary,
            device,
            false,
            partition.start_lba as u64,
            &mut vbr,
        ) {
            Ok(()) => {}
            Err(error) => {
                console.write("  Boot sector read failed: ", RED);
                display_io_error(console, error);
                console.write("\n", WHITE);
                continue;
            }
        }

        match crate::storage::partition::parse_fat_bpb(&vbr, partition.sector_count as u64) {
            Ok(fat) => {
                console.write("  Filesystem: ", GREEN);
                console.write(fat.fat_type.name(), GREEN);
                console.write("\n", WHITE);

                console.write("  Bytes per sector: ", GRAY);
                write_decimal(console, fat.bytes_per_sector as u64);
                console.write("\n", WHITE);

                console.write("  Sectors per cluster: ", GRAY);
                write_decimal(console, fat.sectors_per_cluster as u64);
                console.write("\n", WHITE);

                console.write("  FAT count: ", GRAY);
                write_decimal(console, fat.fat_count as u64);
                console.write("\n", WHITE);

                console.write("  FAT size: ", GRAY);
                write_decimal(console, fat.fat_size_sectors as u64);
                console.write("\n", WHITE);

                console.write("  Cluster count: ", GRAY);
                write_decimal(console, fat.cluster_count as u64);
                console.write("\n", WHITE);

                console.write("  First data sector: ", GRAY);
                write_decimal(console, fat.first_data_sector as u64);
                console.write("\n", WHITE);

                console.write("  BPB validation: PASSED\n", GREEN);

                if matches!(
                    partition.partition_type,
                    0x01 | 0x04 | 0x06 | 0x0E | 0x0B | 0x0C
                ) {
                    if crate::storage::partition::partition_type_matches_fat(
                        partition.partition_type,
                        fat.fat_type,
                    ) {
                        console.write("  MBR/BPB consistency: MATCH\n", GREEN);
                    } else {
                        console.write("  WARNING: MBR/BPB type mismatch!\n", RED);
                    }
                }
            }

            Err(_) => {
                console.write("  Filesystem: FAT not recognized\n", YELLOW);
                console.write("  BPB validation: FAILED\n", RED);
            }
        }
    }

    console.write("\nTotal partitions: ", GRAY);
    write_decimal(console, table.count as u64);
    console.write("\n", WHITE);
}

fn mount_fat16(console: &mut Console<'_>) -> Option<Fat16Volume<IdeBlockDevice>> {
    let device = match IdeBlockDevice::primary(0, false) {
        Ok(device) => device,
        Err(_) => {
            console.write("FAT16 mount failed: ", RED);
            display_fat16_error(console, Fat16Error::DeviceNotReady);
            console.write("\n", WHITE);
            return None;
        }
    };

    match Fat16Volume::mount(device) {
        Ok(volume) => Some(volume),
        Err(error) => {
            console.write("FAT16 mount failed: ", RED);
            display_fat16_error(console, error);
            console.write("\n", WHITE);
            None
        }
    }
}

fn parse_fat_filename(filename: &str) -> Result<[u8; 11], Fat16Error> {
    let mut result = [b' '; 11];
    let mut parts = filename.split('.');

    let base = parts.next().unwrap_or("");
    let extension = parts.next().unwrap_or("");

    if base.is_empty() || base.len() > 8 || extension.len() > 3 || parts.next().is_some() {
        return Err(Fat16Error::InvalidFileName);
    }

    for (index, byte) in base.bytes().enumerate() {
        if !byte.is_ascii_alphanumeric() && !matches!(byte, b'_' | b'-' | b'~' | b'$') {
            return Err(Fat16Error::InvalidFileName);
        }

        result[index] = byte.to_ascii_uppercase();
    }

    for (index, byte) in extension.bytes().enumerate() {
        if !byte.is_ascii_alphanumeric() {
            return Err(Fat16Error::InvalidFileName);
        }

        result[8 + index] = byte.to_ascii_uppercase();
    }

    Ok(result)
}

fn find_fat16_file(
    console: &mut Console<'_>,
    volume: &mut Fat16Volume<IdeBlockDevice>,
    filename: &str,
) -> Option<Fat16File> {
    let name = match parse_fat_filename(filename) {
        Ok(name) => name,
        Err(error) => {
            console.write("Invalid filename: ", RED);
            display_fat16_error(console, error);
            console.write("\n", WHITE);
            return None;
        }
    };

    match volume.find_root_file(&name) {
        Ok(file) => Some(file),
        Err(error) => {
            console.write("File lookup failed: ", RED);
            display_fat16_error(console, error);
            console.write("\n", WHITE);
            None
        }
    }
}

fn display_fat16_root(console: &mut Console<'_>) {
    console.write("\nFAT16 Root Directory:\n", GREEN);

    let Some(mut volume) = mount_fat16(console) else {
        return;
    };

    console.write("Filesystem: FAT16\n", GREEN);
    console.write("Root entries: ", GRAY);
    write_decimal(console, volume.root_entry_count() as u64);
    console.write("\n\n", WHITE);

    console.write("TYPE        NAME                 SIZE\n", GREEN);
    console.write("----------------------------------------\n", GRAY);

    let mut files = 0usize;
    let mut directories = 0usize;

    for index in 0..volume.root_entry_count() {
        let is_end = match volume.root_entry_is_end(index) {
            Ok(value) => value,
            Err(error) => {
                console.write("Directory read failed: ", RED);
                display_fat16_error(console, error);
                console.write("\n", WHITE);
                return;
            }
        };

        if is_end {
            break;
        }

        let file = match volume.read_root_entry(index) {
            Ok(file) => file,
            Err(error) => {
                console.write("Directory read failed: ", RED);
                display_fat16_error(console, error);
                console.write("\n", WHITE);
                return;
            }
        };

        if file.is_empty() {
            continue;
        }

        if file.is_directory() {
            console.write("DIRECTORY   ", YELLOW);
            directories += 1;
        } else {
            console.write("FILE        ", GREEN);
            files += 1;
        }

        display_fat16_filename(console, &file.name);

        let name_length = file
            .name
            .iter()
            .take(8)
            .filter(|byte| **byte != b' ')
            .count()
            + if file.name[8..].iter().any(|byte| *byte != b' ') {
                1 + file.name[8..]
                    .iter()
                    .take_while(|byte| **byte != b' ')
                    .count()
            } else {
                0
            };

        let padding = 21usize.saturating_sub(name_length);

        for _ in 0..padding {
            console.write(" ", WHITE);
        }

        if file.is_directory() {
            console.write("<DIR>", YELLOW);
        } else {
            write_decimal(console, file.size as u64);
        }

        console.write("\n", WHITE);
    }

    console.write("\nDirectory summary:\n", GREEN);

    console.write("  Files: ", GRAY);
    write_decimal(console, files as u64);
    console.write("\n", WHITE);

    console.write("  Directories: ", GRAY);
    write_decimal(console, directories as u64);
    console.write("\n", WHITE);
}

fn display_fat16_filename(console: &mut Console<'_>, name: &[u8; 11]) {
    let mut output = [0u8; 13];
    let mut length = 0usize;

    for byte in name.iter().take(8) {
        if *byte == b' ' {
            break;
        }

        output[length] = byte.to_ascii_uppercase();
        length += 1;
    }

    let mut extension_length = 0usize;

    for byte in name.iter().skip(8).take(3) {
        if *byte == b' ' {
            break;
        }

        extension_length += 1;
    }

    if extension_length > 0 {
        output[length] = b'.';
        length += 1;

        for byte in name.iter().skip(8).take(extension_length) {
            output[length] = byte.to_ascii_uppercase();
            length += 1;
        }
    }

    if let Ok(text) = core::str::from_utf8(&output[..length]) {
        console.write(text, WHITE);
    }
}

fn display_fat16_file(console: &mut Console<'_>, filename: &str) {
    console.write("\nFAT16 File Reader:\n", GREEN);

    let Some(mut volume) = mount_fat16(console) else {
        return;
    };

    let Some(file) = find_fat16_file(console, &mut volume, filename) else {
        return;
    };

    if file.is_directory() {
        console.write("Requested entry is a directory.\n", YELLOW);
        return;
    }

    console.write("File: ", GREEN);
    console.write(filename, WHITE);
    console.write("\nSize: ", GRAY);
    write_decimal(console, file.size as u64);
    console.write(" bytes\n", WHITE);

    if file.size == 0 {
        console.write("File is empty.\n", YELLOW);
        return;
    }

    const PREVIEW_SIZE: usize = 1024;
    let mut buffer = [0u8; PREVIEW_SIZE];

    let count = match volume.read_file_range(&file, 0, &mut buffer) {
        Ok(count) => count,
        Err(error) => {
            console.write("File read failed: ", RED);
            display_fat16_error(console, error);
            console.write("\n", WHITE);
            return;
        }
    };

    console.write("\nContents preview:\n", GREEN);

    for byte in buffer.iter().take(count) {
        if *byte == b'\n' || *byte == b'\r' || *byte == b'\t' || (32..=126).contains(byte) {
            let character = [*byte];
            console.write(
                core::str::from_utf8(&character).unwrap_or("."),
                WHITE,
            );
        } else {
            console.write(".", GRAY);
        }
    }

    console.write("\n\nRead complete: ", GREEN);
    write_decimal(console, count as u64);
    console.write(" bytes\n", WHITE);

    if count < file.size as usize {
        console.write("Preview truncated.\n", YELLOW);
    }
}

fn display_fat16_file_info(console: &mut Console<'_>, filename: &str) {
    console.write("\nAmaryllis File Inspector\n", GREEN);

    let Some(mut volume) = mount_fat16(console) else {
        return;
    };

    let Some(file) = find_fat16_file(console, &mut volume, filename) else {
        return;
    };

    console.write("\nFile: ", GREEN);
    console.write(filename, WHITE);

    console.write("\nSize: ", GRAY);
    write_decimal(console, file.size as u64);
    console.write(" bytes\n", WHITE);

    console.write("Starting cluster: ", GRAY);
    write_decimal(console, file.first_cluster as u64);
    console.write("\n", WHITE);

    console.write("Attributes: ", GRAY);
    write_hex_byte(console, file.attributes);
    console.write("\n", WHITE);

    if file.is_directory() {
        console.write("Type: Directory\n", YELLOW);
        return;
    }

    if file.size == 0 {
        console.write("Format: EMPTY FILE\n", YELLOW);
        return;
    }

    let mut header = [0u8; 32];

    let count = match volume.read_file_range(&file, 0, &mut header) {
        Ok(count) => count,
        Err(error) => {
            console.write("Header read failed: ", RED);
            display_fat16_error(console, error);
            console.write("\n", WHITE);
            return;
        }
    };

    let format = if header.starts_with(b"\x7FELF") {
        "ELF executable"
    } else if header.starts_with(b"MZ") {
        "DOS/Windows executable (PE candidate)"
    } else if header.starts_with(b"\x89PNG\r\n\x1A\n") {
        "PNG image"
    } else if header.starts_with(b"\xFF\xD8\xFF") {
        "JPEG image"
    } else if header.starts_with(b"GIF87a") || header.starts_with(b"GIF89a") {
        "GIF image"
    } else if header.starts_with(b"%PDF-") {
        "PDF document"
    } else if header.starts_with(b"PK\x03\x04") {
        "ZIP archive"
    } else if header.starts_with(b"\x1F\x8B") {
        "GZIP archive"
    } else if header.starts_with(b"SQLite format 3\0") {
        "SQLite database"
    } else {
        "Unknown / text data"
    };

    console.write("\nDetected format: ", GREEN);
    console.write(format, WHITE);
    console.write("\n\nHeader:\n", GREEN);

    for (index, byte) in header.iter().take(count).enumerate() {
        write_hex_byte(console, *byte);
        console.write(" ", WHITE);

        if index == 15 {
            console.write("\n", WHITE);
        }
    }

    console.write("\n\nInspection complete.\n", GREEN);
}

fn display_fat16_extract(
    console: &mut Console<'_>,
    filename: &str,
    offset: u64,
    length: u64,
) {
    console.write("\nAmaryllis File Extractor\n", GREEN);

    if length == 0 || length > 1024 {
        console.write("Length must be between 1 and 1024 bytes.\n", RED);
        return;
    }

    if offset > u32::MAX as u64 {
        console.write("Offset exceeds supported range.\n", RED);
        return;
    }

    let Some(mut volume) = mount_fat16(console) else {
        return;
    };

    let Some(file) = find_fat16_file(console, &mut volume, filename) else {
        return;
    };

    if file.is_directory() {
        console.write("Cannot extract a directory.\n", RED);
        return;
    }

    if offset >= file.size as u64 {
        console.write("Offset is outside the file.\n", RED);
        return;
    }

    let mut buffer = [0u8; 1024];

    let count = match volume.read_file_range(&file, offset as u32, &mut buffer[..length as usize]) {
        Ok(count) => count,
        Err(error) => {
            console.write("Extraction failed: ", RED);
            display_fat16_error(console, error);
            console.write("\n", WHITE);
            return;
        }
    };

    console.write("\nFile: ", GREEN);
    console.write(filename, WHITE);
    console.write("\nOffset: ", GRAY);
    write_decimal(console, offset);
    console.write("\nBytes read: ", GRAY);
    write_decimal(console, count as u64);
    console.write("\n\n", WHITE);

    display_hex_dump_slice(console, &buffer[..count]);

    console.write("\nExtraction complete.\n", GREEN);
}

fn display_fat16_error(console: &mut Console<'_>, error: Fat16Error) {
    match error {
        Fat16Error::NoController => console.write("No IDE controller", RED),
        Fat16Error::DeviceNotReady => console.write("ATA device not ready", RED),
        Fat16Error::UnsupportedSectorSize => console.write("Unsupported sector size", RED),
        Fat16Error::NoFat16Partition => console.write("No FAT16 partition found", RED),
        Fat16Error::InvalidPartition => console.write("Invalid partition", RED),
        Fat16Error::InvalidFilesystem => console.write("Invalid filesystem", RED),
        Fat16Error::InvalidCluster => console.write("Invalid cluster", RED),
        Fat16Error::InvalidFatEntry => console.write("Invalid FAT entry", RED),
        Fat16Error::EndOfChain => console.write("End of cluster chain", YELLOW),
        Fat16Error::UnexpectedEndOfChain => console.write("Unexpected end of cluster chain", RED),
        Fat16Error::FileNotFound => console.write("File not found", RED),
        Fat16Error::InvalidFileName => console.write("Invalid filename", RED),
        Fat16Error::OffsetOutOfRange => console.write("Offset out of range", RED),
        Fat16Error::DiskReadError => console.write("Disk read error", RED),
    }
}

fn display_hex_dump(console: &mut Console<'_>, buffer: &[u8; 512]) {
    display_hex_dump_slice(console, buffer);
}

fn display_hex_dump_slice(console: &mut Console<'_>, buffer: &[u8]) {
    console.write(
        "Offset   Hexadecimal bytes                         ASCII\n",
        GREEN,
    );

    for (row, chunk) in buffer.chunks(16).enumerate() {
        write_hex_u32(console, (row * 16) as u32);
        console.write("   ", WHITE);

        for byte in chunk {
            write_hex_byte(console, *byte);
            console.write(" ", WHITE);
        }

        for _ in chunk.len()..16 {
            console.write("   ", WHITE);
        }

        console.write(" ", WHITE);

        for byte in chunk {
            if (32..=126).contains(byte) {
                let character = [*byte];
                console.write(
                    core::str::from_utf8(&character).unwrap_or("."),
                    WHITE,
                );
            } else {
                console.write(".", GRAY);
            }
        }

        console.write("\n", WHITE);
    }
}

fn display_io_error(console: &mut Console<'_>, error: AtaIoError) {
    match error {
        AtaIoError::InvalidChannel => console.write("Invalid channel", RED),
        AtaIoError::DeviceNotReady => console.write("Device not ready", RED),
        AtaIoError::UnsupportedDevice => console.write("Unsupported device", RED),
        AtaIoError::LbaOutOfRange => console.write("LBA out of range", RED),
        AtaIoError::Timeout => console.write("Timeout", RED),
        AtaIoError::DeviceError => console.write("Device error", RED),
    }
}

fn display_string(console: &mut Console<'_>, label: &str, value: &[u8]) {
    console.write("      ", WHITE);
    console.write(label, GRAY);
    console.write(": ", GRAY);
    write_ata_string(console, value);
    console.write("\n", WHITE);
}

fn write_ata_string(console: &mut Console<'_>, value: &[u8]) {
    let mut end = value.len();

    while end > 0 && (value[end - 1] == 0 || value[end - 1] == b' ') {
        end -= 1;
    }

    if end == 0 {
        console.write("UNKNOWN", YELLOW);
        return;
    }

    match core::str::from_utf8(&value[..end]) {
        Ok(text) => console.write(text, WHITE),
        Err(_) => console.write("INVALID STRING", YELLOW),
    }
}

fn write_decimal(console: &mut Console<'_>, mut value: u64) {
    let mut digits = [0u8; 20];
    let mut index = digits.len();

    if value == 0 {
        console.write("0", WHITE);
        return;
    }

    while value > 0 {
        index -= 1;
        digits[index] = b'0' + (value % 10) as u8;
        value /= 10;
    }

    let text = core::str::from_utf8(&digits[index..]).unwrap_or("0");
    console.write(text, WHITE);
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
    let fraction = ((bytes % unit) * 100) / unit;

    write_decimal(console, whole);
    console.write(".", WHITE);

    if fraction < 10 {
        console.write("0", WHITE);
    }

    write_decimal(console, fraction);
}

fn write_hex_byte(console: &mut Console<'_>, value: u8) {
    let chars = [
        HEX[(value >> 4) as usize],
        HEX[(value & 0x0F) as usize],
    ];

    console.write(core::str::from_utf8(&chars).unwrap_or("00"), WHITE);
}

fn write_hex_u32(console: &mut Console<'_>, value: u32) {
    for shift in (0..8).rev() {
        let digit = ((value >> (shift * 4)) & 0x0F) as usize;
        let character = [HEX[digit]];

        console.write(
            core::str::from_utf8(&character).unwrap_or("0"),
            WHITE,
        );
    }
}

fn parse_number(value: &str) -> Option<u64> {
    if let Some(hex) = value.strip_prefix("0x").or_else(|| value.strip_prefix("0X")) {
        u64::from_str_radix(hex, 16).ok()
    } else {
        value.parse::<u64>().ok()
    }
}
