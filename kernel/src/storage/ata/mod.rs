pub mod diagnostics;

use core::arch::asm;

use crate::hardware::{IdeChannelDescriptor, IdeChannelMode};

const REG_DATA: u16 = 0;
const REG_SECTOR_COUNT: u16 = 2;
const REG_LBA_LOW: u16 = 3;
const REG_LBA_MID: u16 = 4;
const REG_LBA_HIGH: u16 = 5;
const REG_DEVICE_HEAD: u16 = 6;
const REG_STATUS: u16 = 7;
const REG_COMMAND: u16 = 7;

const STATUS_ERR: u8 = 1 << 0;
const STATUS_DRQ: u8 = 1 << 3;
const STATUS_DF: u8 = 1 << 5;
const STATUS_BSY: u8 = 1 << 7;

const COMMAND_READ_SECTORS: u8 = 0x20;
const COMMAND_READ_SECTORS_EXT: u8 = 0x24;
const COMMAND_WRITE_SECTORS: u8 = 0x30;
const COMMAND_WRITE_SECTORS_EXT: u8 = 0x34;
const COMMAND_CACHE_FLUSH: u8 = 0xE7;
const COMMAND_CACHE_FLUSH_EXT: u8 = 0xEA;
const COMMAND_IDENTIFY: u8 = 0xEC;
const COMMAND_IDENTIFY_PACKET: u8 = 0xA1;

const MAX_LBA28: u64 = 0x0FFF_FFFF;
const MAX_LBA48: u64 = 0x0000_FFFF_FFFF_FFFF;

const POLL_LIMIT: usize = 1_000_000;
const IDENTIFY_WORDS: usize = 256;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum AtaDeviceType {
    Ata,
    Atapi,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum AtaProbeStatus {
    Present,
    Absent,
    Unsupported,
    Timeout,
    DeviceError,
}

#[derive(Clone, Copy)]
pub struct AtaDevice {
    pub status: AtaProbeStatus,
    pub device_type: Option<AtaDeviceType>,

    pub model: [u8; 40],
    pub serial: [u8; 20],
    pub firmware: [u8; 8],

    pub sectors_lba28: u64,
    pub sectors_lba48: u64,
    pub capacity_sectors: u64,

    pub logical_sector_size: u32,
    pub lba_supported: bool,
    pub lba48_supported: bool,

    pub signature_mid: u8,
    pub signature_high: u8,
}

impl AtaDevice {
    pub const EMPTY: Self = Self {
        status: AtaProbeStatus::Absent,
        device_type: None,

        model: [0; 40],
        serial: [0; 20],
        firmware: [0; 8],

        sectors_lba28: 0,
        sectors_lba48: 0,
        capacity_sectors: 0,

        logical_sector_size: 512,
        lba_supported: false,
        lba48_supported: false,

        signature_mid: 0,
        signature_high: 0,
    };
}

#[derive(Clone, Copy)]
pub struct AtaChannelDevices {
    pub master: AtaDevice,
    pub slave: AtaDevice,
}

impl AtaChannelDevices {
    pub const EMPTY: Self = Self {
        master: AtaDevice::EMPTY,
        slave: AtaDevice::EMPTY,
    };
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum AtaIoError {
    InvalidChannel,
    DeviceNotReady,
    UnsupportedDevice,
    LbaOutOfRange,
    Timeout,
    DeviceError,
}

pub type AtaReadError = AtaIoError;

// ============================================================
// PORT I/O
// ============================================================

fn inb(port: u16) -> u8 {
    let value: u8;

    unsafe {
        asm!(
            "in al, dx",
            in("dx") port,
            out("al") value,
            options(nomem, nostack, preserves_flags),
        );
    }

    value
}

fn outb(port: u16, value: u8) {
    unsafe {
        asm!(
            "out dx, al",
            in("dx") port,
            in("al") value,
            options(nomem, nostack, preserves_flags),
        );
    }
}

fn inw(port: u16) -> u16 {
    let value: u16;

    unsafe {
        asm!(
            "in ax, dx",
            in("dx") port,
            out("ax") value,
            options(nomem, nostack, preserves_flags),
        );
    }

    value
}

fn outw(port: u16, value: u16) {
    unsafe {
        asm!(
            "out dx, ax",
            in("dx") port,
            in("ax") value,
            options(nomem, nostack, preserves_flags),
        );
    }
}

fn io_delay(control_port: u16) {
    let _ = inb(control_port);
    let _ = inb(control_port);
    let _ = inb(control_port);
    let _ = inb(control_port);
}

// ============================================================
// ATA STATUS POLLING
// ============================================================

fn wait_not_busy(status_port: u16) -> Result<u8, AtaProbeStatus> {
    for _ in 0..POLL_LIMIT {
        let status = inb(status_port);

        if status == 0 || status == 0xFF {
            return Err(AtaProbeStatus::Absent);
        }

        if status & STATUS_BSY == 0 {
            return Ok(status);
        }

        core::hint::spin_loop();
    }

    Err(AtaProbeStatus::Timeout)
}

fn wait_data_ready(status_port: u16) -> Result<(), AtaProbeStatus> {
    for _ in 0..POLL_LIMIT {
        let status = inb(status_port);

        if status == 0 || status == 0xFF {
            return Err(AtaProbeStatus::Absent);
        }

        if status & STATUS_BSY != 0 {
            core::hint::spin_loop();
            continue;
        }

        if status & STATUS_ERR != 0 || status & STATUS_DF != 0 {
            return Err(AtaProbeStatus::DeviceError);
        }

        if status & STATUS_DRQ != 0 {
            return Ok(());
        }

        core::hint::spin_loop();
    }

    Err(AtaProbeStatus::Timeout)
}

fn map_poll_error(error: AtaProbeStatus) -> AtaIoError {
    match error {
        AtaProbeStatus::Timeout => AtaIoError::Timeout,
        AtaProbeStatus::Absent | AtaProbeStatus::Unsupported => AtaIoError::DeviceNotReady,
        AtaProbeStatus::DeviceError | AtaProbeStatus::Present => AtaIoError::DeviceError,
    }
}

// ============================================================
// IDENTIFY PARSER
// ============================================================

fn parse_ata_string(
    words: &[u16; IDENTIFY_WORDS],
    start: usize,
    output: &mut [u8],
) {
    for index in 0..output.len() / 2 {
        let word = words[start + index];

        output[index * 2] = (word >> 8) as u8;
        output[index * 2 + 1] = word as u8;
    }

    let mut first = 0;
    let mut last = output.len();

    while first < last && (output[first] == b' ' || output[first] == 0) {
        first += 1;
    }

    while last > first && (output[last - 1] == b' ' || output[last - 1] == 0) {
        last -= 1;
    }

    if first > 0 {
        output.copy_within(first..last, 0);
    }

    let new_length = last - first;

    for byte in &mut output[new_length..] {
        *byte = 0;
    }
}

fn parse_identify(words: &[u16; IDENTIFY_WORDS]) -> AtaDevice {
    let mut device = AtaDevice::EMPTY;

    parse_ata_string(words, 10, &mut device.serial);
    parse_ata_string(words, 23, &mut device.firmware);
    parse_ata_string(words, 27, &mut device.model);

    // Word 49 bit 9: LBA supported.
    device.lba_supported = words[49] & (1 << 9) != 0;

    // LBA28 capacity: words 60-61.
    device.sectors_lba28 = (words[60] as u64) | ((words[61] as u64) << 16);

    // Word 83 bit 10: 48-bit LBA supported.
    let word83_valid = words[83] & (1 << 14) != 0 && words[83] & (1 << 15) == 0;
    device.lba48_supported = word83_valid && words[83] & (1 << 10) != 0;

    // LBA48 capacity: words 100-103.
    if device.lba48_supported {
        device.sectors_lba48 = (words[100] as u64)
            | ((words[101] as u64) << 16)
            | ((words[102] as u64) << 32)
            | ((words[103] as u64) << 48);
    }

    device.capacity_sectors = if device.lba48_supported && device.sectors_lba48 > 0 {
        device.sectors_lba48
    } else {
        device.sectors_lba28
    };

    device.logical_sector_size = 512;

    let word106 = words[106];
    let word106_valid = word106 & (1 << 14) != 0 && word106 & (1 << 15) == 0;

    if word106_valid && word106 & (1 << 12) != 0 {
        let words_per_sector = (words[117] as u32) | ((words[118] as u32) << 16);

        if words_per_sector > 0 && words_per_sector <= u32::MAX / 2 {
            device.logical_sector_size = words_per_sector * 2;
        }
    }

    device.status = AtaProbeStatus::Present;
    device.device_type = Some(AtaDeviceType::Ata);

    device
}

fn read_identify_words(data_port: u16) -> [u16; IDENTIFY_WORDS] {
    let mut words = [0u16; IDENTIFY_WORDS];

    for word in &mut words {
        *word = inw(data_port);
    }

    words
}

// ============================================================
// DEVICE & CHANNEL PROBING
// ============================================================

pub fn probe_device(
    channel: &IdeChannelDescriptor,
    slave: bool,
) -> AtaDevice {
    if !channel.valid
        || channel.mode == IdeChannelMode::Unsupported
        || channel.command_base == 0
        || channel.control_base == 0
    {
        return AtaDevice {
            status: AtaProbeStatus::Unsupported,
            ..AtaDevice::EMPTY
        };
    }

    let command = channel.command_base;
    let control = channel.control_base;

    outb(command + REG_DEVICE_HEAD, if slave { 0xB0 } else { 0xA0 });
    io_delay(control);

    let initial_status = inb(command + REG_STATUS);

    if initial_status == 0 || initial_status == 0xFF {
        return AtaDevice::EMPTY;
    }

    outb(command + REG_SECTOR_COUNT, 0);
    outb(command + REG_LBA_LOW, 0);
    outb(command + REG_LBA_MID, 0);
    outb(command + REG_LBA_HIGH, 0);
    outb(command + REG_COMMAND, COMMAND_IDENTIFY);

    let status = match wait_not_busy(command + REG_STATUS) {
        Ok(status) => status,
        Err(error) => {
            return AtaDevice {
                status: error,
                ..AtaDevice::EMPTY
            };
        }
    };

    let mut device_type = AtaDeviceType::Ata;
    let mut signature_mid = 0;
    let mut signature_high = 0;

    if status & STATUS_ERR != 0 {
        signature_mid = inb(command + REG_LBA_MID);
        signature_high = inb(command + REG_LBA_HIGH);

        let is_atapi = (signature_mid == 0x14 && signature_high == 0xEB)
            || (signature_mid == 0x69 && signature_high == 0x96);

        if !is_atapi {
            return AtaDevice {
                status: AtaProbeStatus::DeviceError,
                signature_mid,
                signature_high,
                ..AtaDevice::EMPTY
            };
        }

        device_type = AtaDeviceType::Atapi;
        outb(command + REG_COMMAND, COMMAND_IDENTIFY_PACKET);

        if let Err(error) = wait_not_busy(command + REG_STATUS) {
            return AtaDevice {
                status: error,
                signature_mid,
                signature_high,
                ..AtaDevice::EMPTY
            };
        }
    }

    if let Err(error) = wait_data_ready(command + REG_STATUS) {
        return AtaDevice {
            status: error,
            signature_mid,
            signature_high,
            ..AtaDevice::EMPTY
        };
    }

    let words = read_identify_words(command + REG_DATA);
    let mut device = parse_identify(&words);

    device.device_type = Some(device_type);
    device.signature_mid = signature_mid;
    device.signature_high = signature_high;

    device
}

pub fn probe_channel(channel: &IdeChannelDescriptor) -> AtaChannelDevices {
    if !channel.valid {
        return AtaChannelDevices::EMPTY;
    }

    AtaChannelDevices {
        master: probe_device(channel, false),
        slave: probe_device(channel, true),
    }
}

// ============================================================
// COMMON VALIDATION & TASK FILE SETUP
// ============================================================

fn validate_ata_io_range(
    channel: &IdeChannelDescriptor,
    device: &AtaDevice,
    lba: u64,
    sector_count: usize,
) -> Result<bool, AtaIoError> {
    if !channel.valid
        || channel.mode == IdeChannelMode::Unsupported
        || channel.command_base == 0
        || channel.control_base == 0
    {
        return Err(AtaIoError::InvalidChannel);
    }

    if device.status != AtaProbeStatus::Present {
        return Err(AtaIoError::DeviceNotReady);
    }

    if device.device_type != Some(AtaDeviceType::Ata) || !device.lba_supported {
        return Err(AtaIoError::UnsupportedDevice);
    }

    let end_lba = lba
        .checked_add(sector_count as u64)
        .ok_or(AtaIoError::LbaOutOfRange)?;

    if device.capacity_sectors > 0 && end_lba > device.capacity_sectors {
        return Err(AtaIoError::LbaOutOfRange);
    }

    let last_lba = if sector_count == 0 { lba } else { end_lba - 1 };
    if last_lba <= MAX_LBA28 && sector_count <= 128 {
        Ok(false)
    } else if device.lba48_supported && last_lba <= MAX_LBA48 {
        Ok(true)
    } else {
        Err(AtaIoError::LbaOutOfRange)
    }
}

fn prepare_multi_sector_command(
    channel: &IdeChannelDescriptor,
    slave: bool,
    lba: u64,
    count: u16,
    use_lba48: bool,
) -> Result<(), AtaIoError> {
    let command = channel.command_base;
    let control = channel.control_base;

    if use_lba48 {
        let device_head = 0x40 | ((slave as u8) << 4);
        outb(command + REG_DEVICE_HEAD, device_head);
        io_delay(control);
        wait_not_busy(command + REG_STATUS).map_err(map_poll_error)?;

        // High bytes first (LBA48 task file).
        outb(command + REG_SECTOR_COUNT, ((count >> 8) & 0xFF) as u8);
        outb(command + REG_LBA_LOW, ((lba >> 24) & 0xFF) as u8);
        outb(command + REG_LBA_MID, ((lba >> 32) & 0xFF) as u8);
        outb(command + REG_LBA_HIGH, ((lba >> 40) & 0xFF) as u8);

        // Low bytes.
        outb(command + REG_SECTOR_COUNT, (count & 0xFF) as u8);
        outb(command + REG_LBA_LOW, (lba & 0xFF) as u8);
        outb(command + REG_LBA_MID, ((lba >> 8) & 0xFF) as u8);
        outb(command + REG_LBA_HIGH, ((lba >> 16) & 0xFF) as u8);
    } else {
        let device_head = 0xE0 | ((slave as u8) << 4) | (((lba >> 24) as u8) & 0x0F);
        outb(command + REG_DEVICE_HEAD, device_head);
        io_delay(control);
        wait_not_busy(command + REG_STATUS).map_err(map_poll_error)?;

        outb(command + REG_SECTOR_COUNT, (count & 0xFF) as u8);
        outb(command + REG_LBA_LOW, (lba & 0xFF) as u8);
        outb(command + REG_LBA_MID, ((lba >> 8) & 0xFF) as u8);
        outb(command + REG_LBA_HIGH, ((lba >> 16) & 0xFF) as u8);
    }

    Ok(())
}

// ============================================================
// ATA PIO READ & WRITE (LBA28 / LBA48)
// ============================================================

pub fn read_sector(
    channel: &IdeChannelDescriptor,
    device: &AtaDevice,
    slave: bool,
    lba: u64,
    buffer: &mut [u8; 512],
) -> Result<(), AtaIoError> {
    read_sectors(channel, device, slave, lba, 1, buffer)
}

pub fn read_sectors(
    channel: &IdeChannelDescriptor,
    device: &AtaDevice,
    slave: bool,
    start_lba: u64,
    sector_count: usize,
    buffer: &mut [u8],
) -> Result<(), AtaIoError> {
    if sector_count == 0 {
        return Ok(());
    }
    if buffer.len() != sector_count * 512 {
        return Err(AtaIoError::DeviceError);
    }

    let command = channel.command_base;
    let mut done = 0usize;
    while done < sector_count {
        let chunk = (sector_count - done).min(64);
        let lba = start_lba + done as u64;
        let use_lba48 = validate_ata_io_range(channel, device, lba, chunk)?;

        prepare_multi_sector_command(channel, slave, lba, chunk as u16, use_lba48)?;

        outb(
            command + REG_COMMAND,
            if use_lba48 {
                COMMAND_READ_SECTORS_EXT
            } else {
                COMMAND_READ_SECTORS
            },
        );

        for s in 0..chunk {
            wait_data_ready(command + REG_STATUS).map_err(|err| match err {
                AtaProbeStatus::Timeout => AtaIoError::Timeout,
                _ => AtaIoError::DeviceError,
            })?;

            let base = (done + s) * 512;
            for index in 0..256 {
                let word = inw(command + REG_DATA);
                buffer[base + index * 2] = word as u8;
                buffer[base + index * 2 + 1] = (word >> 8) as u8;
            }
        }

        let final_status = wait_not_busy(command + REG_STATUS).map_err(|err| match err {
            AtaProbeStatus::Timeout => AtaIoError::Timeout,
            _ => AtaIoError::DeviceError,
        })?;

        if final_status & STATUS_ERR != 0 || final_status & STATUS_DF != 0 {
            return Err(AtaIoError::DeviceError);
        }

        done += chunk;
    }

    Ok(())
}

pub fn write_sector(
    channel: &IdeChannelDescriptor,
    device: &AtaDevice,
    slave: bool,
    lba: u64,
    buffer: &[u8; 512],
) -> Result<(), AtaIoError> {
    write_sectors(channel, device, slave, lba, 1, buffer)?;
    flush_cache(channel, device, slave)
}

pub fn write_sectors(
    channel: &IdeChannelDescriptor,
    device: &AtaDevice,
    slave: bool,
    start_lba: u64,
    sector_count: usize,
    buffer: &[u8],
) -> Result<(), AtaIoError> {
    if sector_count == 0 {
        return Ok(());
    }
    if buffer.len() != sector_count * 512 {
        return Err(AtaIoError::DeviceError);
    }

    let command = channel.command_base;
    let control = channel.control_base;
    let mut done = 0usize;

    while done < sector_count {
        let chunk = (sector_count - done).min(64);
        let lba = start_lba + done as u64;
        let use_lba48 = validate_ata_io_range(channel, device, lba, chunk)?;

        prepare_multi_sector_command(channel, slave, lba, chunk as u16, use_lba48)?;

        outb(
            command + REG_COMMAND,
            if use_lba48 {
                COMMAND_WRITE_SECTORS_EXT
            } else {
                COMMAND_WRITE_SECTORS
            },
        );

        for s in 0..chunk {
            wait_data_ready(command + REG_STATUS).map_err(|err| match err {
                AtaProbeStatus::Timeout => AtaIoError::Timeout,
                _ => AtaIoError::DeviceError,
            })?;

            let base = (done + s) * 512;
            for index in 0..256 {
                let word =
                    (buffer[base + index * 2] as u16) | ((buffer[base + index * 2 + 1] as u16) << 8);
                outw(command + REG_DATA, word);
            }
            io_delay(control);
        }

        let final_status = wait_not_busy(command + REG_STATUS).map_err(|err| match err {
            AtaProbeStatus::Timeout => AtaIoError::Timeout,
            _ => AtaIoError::DeviceError,
        })?;

        if final_status & STATUS_ERR != 0 || final_status & STATUS_DF != 0 {
            return Err(AtaIoError::DeviceError);
        }

        done += chunk;
    }

    Ok(())
}

pub fn flush_cache(
    channel: &IdeChannelDescriptor,
    device: &AtaDevice,
    slave: bool,
) -> Result<(), AtaIoError> {
    if !channel.valid
        || channel.mode == IdeChannelMode::Unsupported
        || channel.command_base == 0
        || channel.control_base == 0
    {
        return Err(AtaIoError::InvalidChannel);
    }

    if device.status != AtaProbeStatus::Present {
        return Err(AtaIoError::DeviceNotReady);
    }

    let command = channel.command_base;
    let control = channel.control_base;

    outb(command + REG_DEVICE_HEAD, if slave { 0xB0 } else { 0xA0 });
    io_delay(control);

    wait_not_busy(command + REG_STATUS).map_err(map_poll_error)?;

    outb(
        command + REG_COMMAND,
        if device.lba48_supported {
            COMMAND_CACHE_FLUSH_EXT
        } else {
            COMMAND_CACHE_FLUSH
        },
    );

    let status = wait_not_busy(command + REG_STATUS).map_err(|err| match err {
        AtaProbeStatus::Timeout => AtaIoError::Timeout,
        _ => AtaIoError::DeviceError,
    })?;

    if status & STATUS_ERR != 0 || status & STATUS_DF != 0 {
        return Err(AtaIoError::DeviceError);
    }

    Ok(())
}
