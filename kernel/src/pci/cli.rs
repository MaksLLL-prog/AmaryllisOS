
use crate::console::Console;
use crate::graphics::Color;

use super::resources::{self, PciResource};
use super::{PciBar, PciBarKind};

const WHITE: Color = (235, 235, 240);
const GREEN: Color = (40, 180, 120);
const GRAY: Color = (140, 140, 150);
const YELLOW: Color = (220, 190, 90);
const RED: Color = (220, 90, 90);

#[derive(Clone, Copy)]
enum ResourceFilter {
    All,
    Memory,
    Io,
}

pub fn execute(console: &mut Console<'_>, arguments: &str) {
    let mut parts = arguments.split_whitespace();

    match parts.next() {
        Some("resources") => {
            show_resources(console, ResourceFilter::All);
        }
        Some("memory") => {
            show_resources(console, ResourceFilter::Memory);
        }
        Some("io") => {
            show_resources(console, ResourceFilter::Io);
        }
        Some("validate") => {
            show_validation(console);
        }
        Some("find") => {
            match parts.next() {
                Some(address) if parts.next().is_none() => {
                    find_device(console, address);
                }
                _ => {
                    console.write("Usage: pci find BB:DD.F\n", YELLOW);
                }
            }
        }

        Some("init") => {
    match parts.next() {
        Some(address) if parts.next().is_none() => {
            initialize_device(console, address);
        }
        _ => {
            console.write("Usage: pci init BB:DD.F\n", YELLOW);
        }
    }
}

        Some("help") | None => {
            show_help(console);
        }
        Some(command) => {
            console.write("Unknown PCI command: ", RED);
            console.write(command, WHITE);
            console.write("\nUse 'pci help'.\n", GRAY);
        }
    }
}

// ============================================================
// RESOURCE LIST
// ============================================================

fn show_resources(
    console: &mut Console<'_>,
    filter: ResourceFilter,
) {
    console.write("\nPCI Resource Manager\n", GREEN);
    console.write("------------------------------\n", GRAY);

    let total = resources::count();
    let mut displayed = 0usize;

    for index in 0..total {
        let Some(resource) = resources::get(index) else {
            continue;
        };

        let include = match filter {
            ResourceFilter::All => true,
            ResourceFilter::Memory => resource.is_memory(),
            ResourceFilter::Io => resource.is_io(),
        };

        if !include {
            continue;
        }

        display_resource(console, &resource);
        displayed += 1;
    }

    console.write("\nDisplayed resources: ", GRAY);
    write_number(console, displayed as u64);
    console.write("\n", WHITE);

    if displayed == 0 {
        console.write("No matching resources found.\n", YELLOW);
    }
}

fn display_resource(
    console: &mut Console<'_>,
    resource: &PciResource,
) {
    console.write("\nPCI ", GREEN);
    write_hex8(console, resource.bus);
    console.write(":", GRAY);
    write_hex8(console, resource.device);
    console.write(".", GRAY);
    write_hex8(console, resource.function);

    console.write("  BAR", WHITE);
    write_number(console, resource.bar.index as u64);
    console.write("\n", WHITE);

    console.write("  Vendor:    ", GRAY);
    write_hex16(console, resource.vendor_id);
    console.write("\n", WHITE);

    console.write("  Device:    ", GRAY);
    write_hex16(console, resource.device_id);
    console.write("\n", WHITE);

    console.write("  Type:      ", GRAY);
    write_bar_kind(console, resource.bar.kind);
    console.write("\n", WHITE);

    console.write("  Base:      ", GRAY);
    write_hex64(console, resource.bar.base);
    console.write("\n", WHITE);

    console.write("  Size:      ", GRAY);

    if resource.bar.size_known {
        write_number(console, resource.bar.size);
        console.write(" bytes\n", WHITE);
    } else {
        console.write("Unknown\n", YELLOW);
    }

    console.write("  Valid:     ", GRAY);

    if resource.bar.valid {
        console.write("YES", GREEN);
    } else {
        console.write("NO", RED);
    }

    console.write("\n", WHITE);

    if resource.bar.prefetchable {
        console.write("  Prefetch:  YES\n", GREEN);
    }
}

// ============================================================
// DEVICE RESOURCE SEARCH
// ============================================================

fn find_device(
    console: &mut Console<'_>,
    address: &str,
) {
    let Some((bus, device, function)) = parse_pci_address(address) else {
        console.write("Invalid PCI address.\n", RED);
        console.write("Expected format: BB:DD.F (example: 00:01.0)\n", GRAY);
        return;
    };

    let query = super::query::ResourceQuery::by_address(
        bus,
        device,
        function,
    );

    if super::query::find_first(&query).is_none() {
        console.write("No resources found for PCI ", YELLOW);
        write_hex8(console, bus);
        console.write(":", GRAY);
        write_hex8(console, device);
        console.write(".", GRAY);
        write_hex8(console, function);
        console.write("\n", WHITE);
        return;
    }

    console.write("\nResources for PCI ", GREEN);
    write_hex8(console, bus);
    console.write(":", GRAY);
    write_hex8(console, device);
    console.write(".", GRAY);
    write_hex8(console, function);
    console.write("\n", WHITE);

    let mut found = 0usize;

    for bar_index in 0..6 {
        if let Some(resource) = resources::find_bar(
            bus,
            device,
            function,
            bar_index,
        ) {
            display_resource(console, &resource);
            found += 1;
        }
    }

    console.write("\nTotal: ", GRAY);
    write_number(console, found as u64);
    console.write(" resources\n", WHITE);
}

fn parse_pci_address(address: &str) -> Option<(u8, u8, u8)> {
    let (bus_part, rest) = address.split_once(':')?;
    let (device_part, function_part) = rest.split_once('.')?;

    if bus_part.is_empty()
        || device_part.is_empty()
        || function_part.is_empty()
    {
        return None;
    }

    let bus = u8::from_str_radix(bus_part, 16).ok()?;
    let device = u8::from_str_radix(device_part, 16).ok()?;
    let function = u8::from_str_radix(function_part, 16).ok()?;

    if device > 31 || function > 7 {
        return None;
    }

    Some((bus, device, function))
}

// ============================================================
// DRIVER INITIALIZATION
// ============================================================

fn initialize_device(
    console: &mut Console<'_>,
    address: &str,
) {
    let Some((bus, device, function)) = parse_pci_address(address) else {
        console.write("Invalid PCI address.\n", RED);
        console.write("Expected format: BB:DD.F\n", GRAY);
        return;
    };

    console.write("\nPCI Driver Initialization\n", GREEN);
    console.write("------------------------------\n", GRAY);

    console.write("Device: ", GRAY);
    write_hex8(console, bus);
    console.write(":", GRAY);
    write_hex8(console, device);
    console.write(".", GRAY);
    write_hex8(console, function);
    console.write("\n", WHITE);

    match super::driver::initialize_driver(
        bus,
        device,
        function,
    ) {
        Ok(super::driver::DriverInitStatus::Success) => {
            console.write("Initialization: SUCCESS\n", GREEN);
        }

        Ok(super::driver::DriverInitStatus::NotImplemented) => {
            console.write(
                "Initialization: NOT IMPLEMENTED\n",
                YELLOW,
            );
        }

        Ok(super::driver::DriverInitStatus::HardwareError) => {
            console.write(
                "Initialization: HARDWARE ERROR\n",
                RED,
            );
        }

        Err(error) => {
            console.write("Initialization failed: ", RED);
            console.write(error.name(), WHITE);
            console.write("\n", WHITE);
        }
    }
}

// ============================================================
// RESOURCE VALIDATION
// ============================================================

fn show_validation(console: &mut Console<'_>) {
    let stats = super::validation::validate();

    console.write("\nPCI Resource Validation\n", GREEN);
    console.write("------------------------------\n", GRAY);

    print_stat(console, "Checked resources:       ", stats.checked_resources);
    print_stat(console, "Invalid resources:       ", stats.invalid_resources);
    print_stat(console, "Unknown sizes:            ", stats.unknown_sizes);
    print_stat(console, "Invalid sizes:            ", stats.invalid_sizes);
    print_stat(console, "Misaligned addresses:    ", stats.misaligned_addresses);
    print_stat(console, "Address overflows:       ", stats.address_overflows);
    print_stat(console, "Out of range:             ", stats.out_of_range);
    print_stat(console, "Overlapping pairs:       ", stats.overlapping_pairs);

    console.write("\nValidation: ", GRAY);

    if stats.invalid_resources == 0
        && stats.invalid_sizes == 0
        && stats.misaligned_addresses == 0
        && stats.address_overflows == 0
        && stats.out_of_range == 0
        && stats.overlapping_pairs == 0
    {
        console.write("PASSED\n", GREEN);
    } else {
        console.write("WARNINGS FOUND\n", RED);
    }
}

fn print_stat(
    console: &mut Console<'_>,
    label: &str,
    value: usize,
) {
    console.write(label, GRAY);
    write_number(console, value as u64);
    console.write("\n", WHITE);
}

// ============================================================
// HELP
// ============================================================

fn show_help(console: &mut Console<'_>) {
    console.write("\nPCI Resource Manager\n", GREEN);
    console.write("------------------------------\n", GRAY);

    console.write("  pci resources       - List all BAR resources\n", WHITE);
    console.write("  pci memory          - List memory resources\n", WHITE);
    console.write("  pci io              - List I/O resources\n", WHITE);
    console.write("  pci find BB:DD.F    - Find device resources\n", WHITE);
    console.write("  pci init BB:DD.F    - Initialize device driver\n", WHITE);
    console.write("  pci validate        - Validate PCI resources\n", WHITE);
    console.write("  pci help            - Show this help\n", WHITE);

    console.write("\nExamples:\n", YELLOW);
    console.write("  pci init 00:01.0\n", GRAY);
    console.write("  pci find 00:01.0\n", GRAY);
    console.write("  pci memory\n", GRAY);
    console.write("  pci validate\n", GRAY);
}

// ============================================================
// FORMATTING
// ============================================================

fn write_bar_kind(
    console: &mut Console<'_>,
    kind: PciBarKind,
) {
    match kind {
        PciBarKind::Io => console.write("I/O", YELLOW),
        PciBarKind::Memory32 => console.write("Memory32", GREEN),
        PciBarKind::MemoryBelow1M => console.write("MemoryBelow1M", GREEN),
        PciBarKind::Memory64 => console.write("Memory64", GREEN),
        PciBarKind::Reserved => console.write("Reserved", RED),
    }
}

fn write_number(
    console: &mut Console<'_>,
    mut value: u64,
) {
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

fn write_hex8(
    console: &mut Console<'_>,
    value: u8,
) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";

    let bytes = [
        HEX[(value >> 4) as usize],
        HEX[(value & 0x0F) as usize],
    ];

    if let Ok(text) = core::str::from_utf8(&bytes) {
        console.write(text, WHITE);
    }
}

fn write_hex16(
    console: &mut Console<'_>,
    value: u16,
) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";

    let mut buffer = [0u8; 4];

    for index in 0..4 {
        let shift = (3 - index) * 4;
        buffer[index] = HEX[((value >> shift) & 0xF) as usize];
    }

    console.write("0x", GRAY);

    if let Ok(text) = core::str::from_utf8(&buffer) {
        console.write(text, WHITE);
    }
}

fn write_hex64(
    console: &mut Console<'_>,
    value: u64,
) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";

    console.write("0x", GRAY);

    for index in (0..16).rev() {
        let digit = ((value >> (index * 4)) & 0xF) as usize;
        let byte = [HEX[digit]];

        if let Ok(text) = core::str::from_utf8(&byte) {
            console.write(text, WHITE);
        }
    }
}
