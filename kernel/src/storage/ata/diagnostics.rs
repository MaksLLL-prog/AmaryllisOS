use crate::console::Console;
use crate::graphics::Color;
use crate::hardware::IdeChannelDescriptor;

use super::{
    probe_channel,
    AtaChannelDevices,
    AtaDevice,
    AtaDeviceType,
    AtaProbeStatus,
};

const WHITE: Color = (235, 235, 240);
const GREEN: Color = (40, 180, 120);
const GRAY: Color = (140, 140, 150);
const YELLOW: Color = (220, 190, 90);
const RED: Color = (220, 90, 90);

pub fn display_results(
    console: &mut Console<'_>,
    primary: &IdeChannelDescriptor,
    secondary: &IdeChannelDescriptor,
) -> (AtaChannelDevices, AtaChannelDevices) {
    console.write("\nATA Device Detection:\n", GREEN);

    let primary_devices = probe_channel(primary);
    let secondary_devices = probe_channel(secondary);

    display_channel_results(console, "Primary", &primary_devices);
    display_channel_results(console, "Secondary", &secondary_devices);

    let detected = count_present(&primary_devices) + count_present(&secondary_devices);

    console.write("\nATA Summary:\n", GREEN);
    console.write("  Detected devices: ", GRAY);
    write_number(console, detected);
    console.write("\n", WHITE);

    console.write("  Detection status: ", GRAY);

    if detected > 0 {
        console.write("COMPLETED\n", GREEN);
    } else {
        console.write("NO DEVICES FOUND\n", YELLOW);
    }

    (primary_devices, secondary_devices)
}

pub fn count_present(devices: &AtaChannelDevices) -> usize {
    let mut count = 0;

    if devices.master.status == AtaProbeStatus::Present {
        count += 1;
    }

    if devices.slave.status == AtaProbeStatus::Present {
        count += 1;
    }

    count
}

pub fn display_channel_results(
    console: &mut Console<'_>,
    name: &str,
    devices: &AtaChannelDevices,
) {
    console.write("\n", WHITE);
    console.write(name, GREEN);
    console.write(" Channel:\n", GREEN);

    display_device(console, "Master", &devices.master);
    display_device(console, "Slave", &devices.slave);
}

pub fn display_device(
    console: &mut Console<'_>,
    name: &str,
    device: &AtaDevice,
) {
    console.write("  ", WHITE);
    console.write(name, GRAY);
    console.write(": ", GRAY);

    match device.status {
        AtaProbeStatus::Present => {
            console.write("PRESENT", GREEN);

            if let Some(device_type) = device.device_type {
                console.write(" (", WHITE);

                match device_type {
                    AtaDeviceType::Ata => console.write("ATA", WHITE),
                    AtaDeviceType::Atapi => console.write("ATAPI", WHITE),
                }

                console.write(")", WHITE);
            }

            console.write("\n", WHITE);

            display_string(console, "Model", &device.model);
            display_string(console, "Serial", &device.serial);
            display_string(console, "Firmware", &device.firmware);

            if device.device_type == Some(AtaDeviceType::Ata) {
                console.write("    LBA: ", GRAY);
                if device.lba_supported {
                    console.write("SUPPORTED", GREEN);
                } else {
                    console.write("NOT SUPPORTED", YELLOW);
                }
                console.write("\n", WHITE);

                console.write("    LBA48: ", GRAY);
                if device.lba48_supported {
                    console.write("SUPPORTED", GREEN);
                } else {
                    console.write("NOT SUPPORTED", YELLOW);
                }
                console.write("\n", WHITE);

                console.write("    Sectors: ", GRAY);
                write_number(console, device.capacity_sectors as usize);
                console.write("\n", WHITE);

                console.write("    Sector size: ", GRAY);
                write_number(console, device.logical_sector_size as usize);
                console.write(" bytes\n", WHITE);

                let capacity_bytes = device
                    .capacity_sectors
                    .saturating_mul(device.logical_sector_size as u64);

                console.write("    Capacity: ", GRAY);
                write_capacity(console, capacity_bytes);
                console.write("\n", WHITE);
            }
        }

        AtaProbeStatus::Absent => console.write("ABSENT\n", YELLOW),
        AtaProbeStatus::Unsupported => console.write("UNSUPPORTED\n", YELLOW),
        AtaProbeStatus::Timeout => console.write("TIMEOUT\n", RED),
        AtaProbeStatus::DeviceError => console.write("DEVICE ERROR\n", RED),
    }
}

fn display_string(console: &mut Console<'_>, label: &str, value: &[u8]) {
    console.write("    ", WHITE);
    console.write(label, GRAY);
    console.write(": ", GRAY);

    let mut end = value.len();

    while end > 0 {
        let byte = value[end - 1];

        if byte == 0 || byte == b' ' {
            end -= 1;
        } else {
            break;
        }
    }

    if end == 0 {
        console.write("UNKNOWN", YELLOW);
    } else if let Ok(text) = core::str::from_utf8(&value[..end]) {
        console.write(text, WHITE);
    } else {
        console.write("INVALID STRING", YELLOW);
    }

    console.write("\n", WHITE);
}

fn write_number(console: &mut Console<'_>, mut number: usize) {
    if number == 0 {
        console.write("0", WHITE);
        return;
    }

    let mut buffer = [0u8; 20];
    let mut index = buffer.len();

    while number > 0 {
        index -= 1;
        buffer[index] = b'0' + (number % 10) as u8;
        number /= 10;
    }

    if let Ok(text) = core::str::from_utf8(&buffer[index..]) {
        console.write(text, WHITE);
    }
}

fn write_capacity(console: &mut Console<'_>, bytes: u64) {
    const MB: u64 = 1024 * 1024;
    const GB: u64 = 1024 * MB;
    const TB: u64 = 1024 * GB;

    if bytes >= TB {
        write_scaled(console, bytes, TB);
        console.write(" TiB", WHITE);
    } else if bytes >= GB {
        write_scaled(console, bytes, GB);
        console.write(" GiB", WHITE);
    } else if bytes >= MB {
        write_scaled(console, bytes, MB);
        console.write(" MiB", WHITE);
    } else {
        write_number(console, bytes as usize);
        console.write(" bytes", WHITE);
    }
}

fn write_scaled(console: &mut Console<'_>, bytes: u64, unit: u64) {
    let whole = bytes / unit;
    let fraction = ((bytes % unit) * 100) / unit;

    write_number(console, whole as usize);
    console.write(".", WHITE);

    if fraction < 10 {
        console.write("0", WHITE);
    }

    write_number(console, fraction as usize);
}
