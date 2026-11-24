use crate::console::Console;
use crate::graphics::Color;
use crate::hardware::{
    IdeChannelDescriptor,
    IdeChannelMode,
    IdeControllerDescriptor,
};

use super::{PciBarKind, PciDevice};

const WHITE: Color = (235, 235, 240);
const GREEN: Color = (40, 180, 120);
const GRAY: Color = (140, 140, 150);
const YELLOW: Color = (220, 190, 90);
const RED: Color = (220, 90, 90);

pub type IdeChannel = IdeChannelDescriptor;
pub type IdeControllerInfo = IdeControllerDescriptor;

// ============================================================
// PCI BAR HELPERS
// ============================================================

fn find_io_bar(device: &PciDevice, index: u8) -> Option<u16> {
    for bar_index in 0..device.bar_count {
        let bar = device.bars[bar_index];

        if bar.index == index
            && bar.valid
            && bar.kind == PciBarKind::Io
            && bar.base != 0
            && bar.base <= u16::MAX as u64
        {
            return Some(bar.base as u16);
        }
    }

    None
}

// ============================================================
// CHANNEL CONFIGURATION
// ============================================================

fn create_channel(
    native: bool,
    command_bar: u8,
    control_bar: u8,
    legacy_command: u16,
    legacy_control: u16,
    device: &PciDevice,
) -> IdeChannelDescriptor {
    if !native {
        return IdeChannelDescriptor {
            mode: IdeChannelMode::Compatibility,
            command_base: legacy_command,
            control_base: legacy_control,
            bus_master_base: None,
            valid: true,
        };
    }

    let command = find_io_bar(device, command_bar);
    let control = find_io_bar(device, control_bar);

    match (command, control) {
        (Some(command_base), Some(control_base)) => IdeChannelDescriptor {
            mode: IdeChannelMode::Native,
            command_base,
            control_base: control_base.saturating_add(2),
            bus_master_base: None,
            valid: true,
        },
        _ => IdeChannelDescriptor {
            mode: IdeChannelMode::Native,
            command_base: 0,
            control_base: 0,
            bus_master_base: None,
            valid: false,
        },
    }
}

// ============================================================
// IDE CONTROLLER INSPECTION
// ============================================================

pub fn inspect(device: &PciDevice) -> Option<IdeControllerDescriptor> {
    if device.class_code != 0x01 || device.subclass != 0x01 {
        return None;
    }

    let interface = device.programming_interface;

    let primary_native = interface & 0x01 != 0;
    let secondary_native = interface & 0x04 != 0;

    let mut primary = create_channel(primary_native, 0, 1, 0x1F0, 0x3F6, device);
    let mut secondary = create_channel(secondary_native, 2, 3, 0x170, 0x376, device);

    let bus_master = find_io_bar(device, 4);

    let bus_master_supported = interface & 0x80 != 0 && bus_master.is_some();

    if let Some(base) = bus_master {
        if primary.valid {
            primary.bus_master_base = Some(base);
        }

        if secondary.valid {
            secondary.bus_master_base = Some(base.saturating_add(8));
        }
    }

    Some(IdeControllerDescriptor {
        bus: device.bus,
        device: device.device,
        function: device.function,
        vendor_id: device.vendor_id,
        device_id: device.device_id,
        primary,
        secondary,
        bus_master_supported,
    })
}

// ============================================================
// IDE DISPLAY
// ============================================================

pub fn display(console: &mut Console<'_>, info: &IdeControllerDescriptor) {
    console.write("\nIDE Controller\n", GREEN);
    console.write("------------------------------\n", GRAY);

    console.write("PCI address: ", GRAY);
    write_hex8(console, info.bus);
    console.write(":", GRAY);
    write_hex8(console, info.device);
    console.write(".", GRAY);
    write_hex8(console, info.function);
    console.write("\n", WHITE);

    console.write("Vendor: ", GRAY);
    write_hex16(console, info.vendor_id);
    console.write("  Device: ", GRAY);
    write_hex16(console, info.device_id);
    console.write("\n", WHITE);

    display_channel(console, "Primary", &info.primary);
    display_channel(console, "Secondary", &info.secondary);

    console.write("\nBus Master IDE: ", GRAY);

    if info.bus_master_supported {
        console.write("SUPPORTED\n", GREEN);
    } else {
        console.write("NOT AVAILABLE\n", YELLOW);
    }
}

fn display_channel(
    console: &mut Console<'_>,
    name: &str,
    channel: &IdeChannelDescriptor,
) {
    console.write("\n", WHITE);
    console.write(name, GREEN);
    console.write(" Channel\n", GREEN);

    console.write("  Mode: ", GRAY);
    console.write(channel.mode.name(), WHITE);
    console.write("\n", WHITE);

    if !channel.valid {
        console.write("  Configuration: INVALID\n", RED);
        return;
    }

    console.write("  Command base: ", GRAY);
    write_hex16(console, channel.command_base);
    console.write("\n", WHITE);

    console.write("  Control base: ", GRAY);
    write_hex16(console, channel.control_base);
    console.write("\n", WHITE);

    console.write("  Bus Master base: ", GRAY);

    if let Some(base) = channel.bus_master_base {
        write_hex16(console, base);
        console.write("\n", WHITE);
    } else {
        console.write("NOT AVAILABLE\n", YELLOW);
    }

    console.write("  Configuration: ", GRAY);
    console.write("VALID\n", GREEN);
}

// ============================================================
// NUMBER FORMATTING
// ============================================================

fn write_hex8(console: &mut Console<'_>, value: u8) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";

    let bytes = [HEX[(value >> 4) as usize], HEX[(value & 0x0F) as usize]];

    if let Ok(text) = core::str::from_utf8(&bytes) {
        console.write(text, WHITE);
    }
}

fn write_hex16(console: &mut Console<'_>, value: u16) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";

    let mut buffer = [0u8; 4];

    for index in 0..4 {
        let shift = (3 - index) * 4;
        buffer[index] = HEX[((value >> shift) & 0x0F) as usize];
    }

    console.write("0x", GRAY);

    if let Ok(text) = core::str::from_utf8(&buffer) {
        console.write(text, WHITE);
    }
}
