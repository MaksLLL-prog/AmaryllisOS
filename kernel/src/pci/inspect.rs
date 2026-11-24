
use crate::console::Console;
use crate::graphics::Color;

use super::{PciBar, PciBarKind, PciDevice};

use super::driver::{
    self,
    BindingState,
    DeviceBinding,
    DriverLifecycle,
};

const WHITE: Color = (235, 235, 240);
const GREEN: Color = (40, 180, 120);
const GRAY: Color = (140, 140, 150);
const YELLOW: Color = (220, 190, 90);
const RED: Color = (220, 90, 90);

#[derive(Clone, Copy, PartialEq, Eq)]
enum DisplayMode {
    List,
    Verbose,
    Drivers,
    Count,
}

#[derive(Clone, Copy)]
enum DeviceFilter {
    None,
    Address { bus: u8, device: u8, function: u8 },
    Id { vendor: u16, device: u16 },
}

pub fn execute(console: &mut Console<'_>, arguments: &str) {
    let mut mode = DisplayMode::List;
    let mut filter = DeviceFilter::None;

    let mut parts = arguments.split_whitespace();

    while let Some(option) = parts.next() {
        match option {
            "-v" => mode = DisplayMode::Verbose,
            "--drivers" => mode = DisplayMode::Drivers,
            "--count" => mode = DisplayMode::Count,

            "--help" => {
                show_help(console);
                return;
            }

            "-s" => {
                let Some(value) = parts.next() else {
                    usage_error(console, "Missing PCI address after -s");
                    return;
                };

                let Some((bus, device, function)) = parse_address(value) else {
                    usage_error(console, "Invalid address. Expected BB:DD.F");
                    return;
                };

                filter = DeviceFilter::Address {
                    bus,
                    device,
                    function,
                };
            }

            "-d" => {
                let Some(value) = parts.next() else {
                    usage_error(console, "Missing IDs after -d");
                    return;
                };

                let Some((vendor, device)) = parse_ids(value) else {
                    usage_error(console, "Invalid IDs. Expected VVVV:DDDD");
                    return;
                };

                filter = DeviceFilter::Id { vendor, device };
            }

            _ => {
                usage_error(console, "Unknown option");
                return;
            }
        }
    }

    let total = driver::binding_count();
    let mut found = 0usize;

    if mode != DisplayMode::Count {
        console.write("\nPCI Device Manager\n", GREEN);
        console.write("------------------------------\n", GRAY);
    }

    for index in 0..total {
        let Some(device) = super::get_device(index) else {
            continue;
        };

        if !matches_filter(&device, filter) {
            continue;
        }

        found += 1;

        if mode == DisplayMode::Count {
            continue;
        }

        let binding = driver::find_binding(
            device.bus,
            device.device,
            device.function,
        );

        match mode {
            DisplayMode::List => show_list_entry(console, &device, binding),
            DisplayMode::Verbose => show_verbose(console, &device, binding),
            DisplayMode::Drivers => show_driver_entry(console, &device, binding),
            DisplayMode::Count => {}
        }
    }

    match mode {
        DisplayMode::Count => {
            console.write("PCI devices: ", GREEN);
            write_decimal(console, found as u64);
            console.write("\n", WHITE);
        }

        _ if found == 0 => {
            console.write("No matching PCI devices found.\n", YELLOW);
        }

        _ => {
            console.write("\nMatching devices: ", GRAY);
            write_decimal(console, found as u64);
            console.write("\n", WHITE);
        }
    }
}

fn matches_filter(device: &PciDevice, filter: DeviceFilter) -> bool {
    match filter {
        DeviceFilter::None => true,

        DeviceFilter::Address {
            bus,
            device: target_device,
            function,
        } => {
            device.bus == bus
                && device.device == target_device
                && device.function == function
        }

        DeviceFilter::Id { vendor, device: id } => {
            device.vendor_id == vendor && device.device_id == id
        }
    }
}

// ============================================================
// LIST OUTPUT
// ============================================================

fn show_list_entry(
    console: &mut Console<'_>,
    device: &PciDevice,
    binding: Option<DeviceBinding>,
) {
    write_address(console, device);
    console.write("  ", WHITE);

    write_hex16(console, device.vendor_id);
    console.write(":", GRAY);
    write_hex16(console, device.device_id);

    console.write("  ", WHITE);
    write_class_name(console, device.class_code);

    if let Some(binding) = binding {
        console.write("  ", WHITE);
        write_binding_state(console, binding.state);

        if let Some(name) = binding.driver_name {
            console.write("  ", GRAY);
            console.write(name, WHITE);
        }
    }

    console.write("\n", WHITE);
}

// ============================================================
// VERBOSE OUTPUT
// ============================================================

fn show_verbose(
    console: &mut Console<'_>,
    device: &PciDevice,
    binding: Option<DeviceBinding>,
) {
    console.write("\nDevice ", GREEN);
    write_address(console, device);
    console.write("\n", WHITE);

    console.write("  Vendor ID:              ", GRAY);
    write_hex16(console, device.vendor_id);
    console.write("\n", WHITE);

    console.write("  Device ID:              ", GRAY);
    write_hex16(console, device.device_id);
    console.write("\n", WHITE);

    console.write("  Class:                  ", GRAY);
    write_hex8(console, device.class_code);
    console.write("  ", WHITE);
    write_class_name(console, device.class_code);
    console.write("\n", WHITE);

    console.write("  Subclass:               ", GRAY);
    write_hex8(console, device.subclass);
    console.write("\n", WHITE);

    console.write("  Programming Interface:  ", GRAY);
    write_hex8(console, device.programming_interface);
    console.write("\n", WHITE);

    console.write("  Header Type:            ", GRAY);
    write_hex8(console, device.header_type);
    console.write("\n", WHITE);

    console.write("\n  Driver information:\n", GREEN);

    match binding {
        Some(binding) => {
            console.write("    Driver:       ", GRAY);

            if let Some(name) = binding.driver_name {
                console.write(name, WHITE);
            } else {
                console.write("None", YELLOW);
            }

            console.write("\n", WHITE);

            console.write("    Priority:     ", GRAY);
            write_decimal(console, binding.priority as u64);
            console.write("\n", WHITE);

            console.write("    Binding:      ", GRAY);
            write_binding_state(console, binding.state);
            console.write("\n", WHITE);

            console.write("    Lifecycle:    ", GRAY);
            write_lifecycle(console, binding.lifecycle);
            console.write("\n", WHITE);
        }

        None => {
            console.write("    No binding record\n", YELLOW);
        }
    }

    console.write("\n  BAR resources:\n", GREEN);

    if device.bar_count == 0 {
        console.write("    No active BAR resources reported\n", YELLOW);
    } else {
        for index in 0..device.bar_count {
            let bar = device.bars[index];

            if bar.valid {
                show_bar(console, &bar);
            }
        }
    }

    console.write("\n", WHITE);
}

fn show_bar(console: &mut Console<'_>, bar: &PciBar) {
    console.write("    BAR", GRAY);
    write_decimal(console, bar.index as u64);
    console.write(": ", GRAY);

    match bar.kind {
        PciBarKind::Io => console.write("I/O PORT", WHITE),
        PciBarKind::Memory32 => console.write("MEMORY 32-BIT", WHITE),
        PciBarKind::MemoryBelow1M => {
            console.write("MEMORY BELOW 1 MB", WHITE)
        }
        PciBarKind::Memory64 => console.write("MEMORY 64-BIT", WHITE),
        PciBarKind::Reserved => console.write("RESERVED", YELLOW),
    }

    console.write("\n      Base:          ", GRAY);
    write_hex64(console, bar.base);
    console.write("\n", WHITE);

    console.write("      Address width: ", GRAY);

    match bar.kind {
        PciBarKind::Memory64 => console.write("64-bit\n", GREEN),
        PciBarKind::Memory32 | PciBarKind::MemoryBelow1M => {
            console.write("32-bit\n", WHITE)
        }
        PciBarKind::Io => console.write("I/O\n", WHITE),
        PciBarKind::Reserved => console.write("Unknown\n", YELLOW),
    }

    console.write("      Prefetchable:  ", GRAY);

    if bar.prefetchable {
        console.write("YES\n", GREEN);
    } else {
        console.write("NO\n", WHITE);
    }

    console.write("      Size:          ", GRAY);

    if bar.size_known {
        write_hex64(console, bar.size);
        console.write(" (", GRAY);
        write_decimal(console, bar.size);
        console.write(" bytes)\n", GRAY);
    } else {
        console.write("UNKNOWN\n", YELLOW);
    }
}

// ============================================================
// DRIVER OUTPUT
// ============================================================

fn show_driver_entry(
    console: &mut Console<'_>,
    device: &PciDevice,
    binding: Option<DeviceBinding>,
) {
    write_address(console, device);
    console.write("  ", WHITE);

    write_hex16(console, device.vendor_id);
    console.write(":", GRAY);
    write_hex16(console, device.device_id);

    console.write("\n  Driver: ", GRAY);

    match binding {
        Some(binding) => {
            if let Some(name) = binding.driver_name {
                console.write(name, WHITE);
            } else {
                console.write("None", YELLOW);
            }

            console.write("\n  Priority: ", GRAY);
            write_decimal(console, binding.priority as u64);

            console.write("\n  Binding: ", GRAY);
            write_binding_state(console, binding.state);

            console.write("\n  Lifecycle: ", GRAY);
            write_lifecycle(console, binding.lifecycle);

            console.write("\n\n", WHITE);
        }

        None => {
            console.write("No binding record\n\n", YELLOW);
        }
    }
}

// ============================================================
// HELP
// ============================================================

fn show_help(console: &mut Console<'_>) {
    console.write("\nPCI Device Manager\n", GREEN);
    console.write("------------------------------\n", GRAY);

    console.write("lspci                  List PCI devices\n", WHITE);
    console.write("lspci -v               Detailed device and BAR information\n", WHITE);
    console.write("lspci --drivers        Show driver bindings\n", WHITE);
    console.write("lspci --count          Count matching devices\n", WHITE);
    console.write("lspci -s BB:DD.F       Search by PCI address\n", WHITE);
    console.write("lspci -d VVVV:DDDD     Search by Vendor/Device ID\n", WHITE);
    console.write("lspci --help           Show this help\n", WHITE);

    console.write("\nExamples:\n", GREEN);
    console.write("  lspci -s 00:01.0\n", GRAY);
    console.write("  lspci -v -s 00:01.0\n", GRAY);
    console.write("  lspci -d 1234:5678\n", GRAY);
}

fn usage_error(console: &mut Console<'_>, message: &str) {
    console.write("lspci: ", RED);
    console.write(message, WHITE);
    console.write("\nUse 'lspci --help'.\n", GRAY);
}

// ============================================================
// PARSING
// ============================================================

fn parse_address(value: &str) -> Option<(u8, u8, u8)> {
    let (bus_text, rest) = value.split_once(':')?;
    let (device_text, function_text) = rest.split_once('.')?;

    let bus = parse_hex(bus_text)?;
    let device = parse_hex(device_text)?;
    let function = parse_hex(function_text)?;

    if bus > 0xFF || device >= 32 || function >= 8 {
        return None;
    }

    Some((bus as u8, device as u8, function as u8))
}

fn parse_ids(value: &str) -> Option<(u16, u16)> {
    let (vendor_text, device_text) = value.split_once(':')?;

    let vendor = parse_hex(vendor_text)?;
    let device = parse_hex(device_text)?;

    if vendor > 0xFFFF || device > 0xFFFF {
        return None;
    }

    Some((vendor as u16, device as u16))
}

fn parse_hex(value: &str) -> Option<u32> {
    let value = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
        .unwrap_or(value);

    if value.is_empty() {
        return None;
    }

    let mut result = 0u32;

    for byte in value.bytes() {
        let digit = match byte {
            b'0'..=b'9' => (byte - b'0') as u32,
            b'a'..=b'f' => (byte - b'a' + 10) as u32,
            b'A'..=b'F' => (byte - b'A' + 10) as u32,
            _ => return None,
        };

        result = result.checked_mul(16)?.checked_add(digit)?;
    }

    Some(result)
}

// ============================================================
// FORMATTING
// ============================================================

fn write_address(console: &mut Console<'_>, device: &PciDevice) {
    console.write("PCI ", GREEN);
    write_hex8(console, device.bus);
    console.write(":", GRAY);
    write_hex8(console, device.device);
    console.write(".", GRAY);
    write_hex8(console, device.function);
}

fn write_class_name(console: &mut Console<'_>, class: u8) {
    let name = match class {
        0x00 => "Unclassified",
        0x01 => "Mass Storage",
        0x02 => "Network Controller",
        0x03 => "Display Controller",
        0x04 => "Multimedia",
        0x05 => "Memory Controller",
        0x06 => "Bridge",
        0x07 => "Communication",
        0x08 => "System Peripheral",
        0x09 => "Input Controller",
        0x0A => "Docking Station",
        0x0B => "Processor",
        0x0C => "Serial Bus",
        0x0D => "Wireless Controller",
        0x0E => "Intelligent Controller",
        0x0F => "Satellite Communication",
        0x10 => "Encryption Controller",
        0x11 => "Signal Processing",
        0x12 => "Processing Accelerator",
        0x13 => "Non-Essential Instrumentation",
        _ => "Unknown Device",
    };

    console.write(name, WHITE);
}

fn write_binding_state(
    console: &mut Console<'_>,
    state: BindingState,
) {
    match state {
        BindingState::Matched => console.write("MATCHED", GREEN),
        BindingState::ProbeRejected => console.write("PROBE REJECTED", RED),
        BindingState::NoDriver => console.write("NO DRIVER", YELLOW),
    }
}

fn write_lifecycle(
    console: &mut Console<'_>,
    lifecycle: DriverLifecycle,
) {
    match lifecycle {
        DriverLifecycle::Registered => console.write("REGISTERED", GRAY),
        DriverLifecycle::Matched => console.write("MATCHED", GREEN),
        DriverLifecycle::Initialized => console.write("INITIALIZED", GREEN),
        DriverLifecycle::Failed => console.write("FAILED", RED),
        DriverLifecycle::Initializing => console.write("INITIALIZING", YELLOW),
    }
}

fn write_decimal(console: &mut Console<'_>, mut value: u64) {
    if value == 0 {
        console.write("0", WHITE);
        return;
    }

    let mut buffer = [0u8; 20];
    let mut index = buffer.len();

    while value > 0 {
        index -= 1;
        buffer[index] = b'0' + (value % 10) as u8;
        value /= 10;
    }

    if let Ok(text) = core::str::from_utf8(&buffer[index..]) {
        console.write(text, WHITE);
    }
}

fn write_hex8(console: &mut Console<'_>, value: u8) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";

    let bytes = [
        HEX[((value >> 4) & 0xF) as usize],
        HEX[(value & 0xF) as usize],
    ];

    if let Ok(text) = core::str::from_utf8(&bytes) {
        console.write(text, WHITE);
    }
}

fn write_hex16(console: &mut Console<'_>, value: u16) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";

    console.write("0x", GRAY);

    let mut buffer = [0u8; 4];

    for index in 0..4 {
        let shift = (3 - index) * 4;
        buffer[index] = HEX[((value >> shift) & 0xF) as usize];
    }

    if let Ok(text) = core::str::from_utf8(&buffer) {
        console.write(text, WHITE);
    }
}

fn write_hex64(console: &mut Console<'_>, value: u64) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";

    console.write("0x", GRAY);

    let mut buffer = [0u8; 16];

    for index in 0..16 {
        let shift = (15 - index) * 4;
        buffer[index] = HEX[((value >> shift) & 0xF) as usize];
    }

    if let Ok(text) = core::str::from_utf8(&buffer) {
        console.write(text, WHITE);
    }
}
