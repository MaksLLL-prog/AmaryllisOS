
use bootloader_api::info::{MemoryRegion, MemoryRegionKind};

use crate::console::Console;
use crate::graphics::Color;

const WHITE: Color = (235, 235, 240);
const GRAY: Color = (140, 140, 150);
const GREEN: Color = (40, 180, 120);
const YELLOW: Color = (220, 190, 90);

fn write_hex(console: &mut Console<'_>, value: u64) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";

    console.write("0x", WHITE);

    let mut started = false;

    for shift in (0..16).rev() {
        let digit = ((value >> (shift * 4)) & 0xF) as usize;

        if digit != 0 || started || shift == 0 {
            started = true;

            let character = [HEX[digit]];

            console.write(
                core::str::from_utf8(&character).unwrap_or("?"),
                WHITE,
            );
        }
    }
}

fn write_number(console: &mut Console<'_>, mut value: u64) {
    let mut buffer = [0u8; 20];
    let mut position = buffer.len();

    if value == 0 {
        console.write("0", WHITE);
        return;
    }

    while value > 0 {
        position -= 1;
        buffer[position] = b'0' + (value % 10) as u8;
        value /= 10;
    }

    let text = core::str::from_utf8(&buffer[position..])
        .unwrap_or("?");

    console.write(text, WHITE);
}

fn write_region_kind(
    console: &mut Console<'_>,
    kind: MemoryRegionKind,
) {
    match kind {
        MemoryRegionKind::Usable => {
            console.write("Usable", GREEN);
        }
        MemoryRegionKind::Bootloader => {
            console.write("Bootloader", YELLOW);
        }
        MemoryRegionKind::UnknownUefi(value) => {
            console.write("Unknown UEFI (", YELLOW);
            write_number(console, value as u64);
            console.write(")", YELLOW);
        }
        MemoryRegionKind::UnknownBios(value) => {
            console.write("Unknown BIOS (", YELLOW);
            write_number(console, value as u64);
            console.write(")", YELLOW);
        }
        _ => {
            console.write("Other", GRAY);
        }
    }
}

pub fn display_memory_map(
    console: &mut Console<'_>,
    regions: &[MemoryRegion],
) {
    console.write("\nPhysical memory map:\n", GREEN);

    console.write(
        "  #   START              END                SIZE KiB    TYPE\n",
        GRAY,
    );

    for (index, region) in regions.iter().enumerate() {
        let size = region.end.saturating_sub(region.start);
        let size_kib = size / 1024;

        console.write("  ", GRAY);
        write_number(console, index as u64);
        console.write("   ", GRAY);

        write_hex(console, region.start);
        console.write("   ", GRAY);

        write_hex(console, region.end);
        console.write("   ", GRAY);

        write_number(console, size_kib);
        console.write("       ", GRAY);

        write_region_kind(console, region.kind);

        console.write("\n", WHITE);
    }
}
