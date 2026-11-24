use crate::console::Console;
use crate::graphics::Color;

pub mod driver;
pub mod device_manager;
pub mod inspect;
pub mod resources;
pub mod validation;
pub mod query;
pub mod cli;
pub mod ide;

const PCI_CONFIG_ADDRESS: u16 = 0xCF8;
const PCI_CONFIG_DATA: u16 = 0xCFC;

pub const MAX_PCI_DEVICES: usize = 128;
const MAX_BARS: usize = 6;

const WHITE: Color = (235, 235, 240);
const GREEN: Color = (40, 180, 120);
const GRAY: Color = (140, 140, 150);
const YELLOW: Color = (220, 190, 90);
const RED: Color = (220, 90, 90);

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PciBarKind {
    Io,
    Memory32,
    MemoryBelow1M,
    Memory64,
    Reserved,
}

#[derive(Clone, Copy)]
pub struct PciBar {
    pub index: u8,
    pub kind: PciBarKind,
    pub base: u64,
    pub size: u64,
    pub prefetchable: bool,
    pub valid: bool,
    pub size_known: bool,
}

impl PciBar {
    pub const EMPTY: Self = Self {
        index: 0,
        kind: PciBarKind::Reserved,
        base: 0,
        size: 0,
        prefetchable: false,
        valid: false,
        size_known: false,
    };
}

#[derive(Clone, Copy)]
pub struct PciDevice {
    pub bus: u8,
    pub device: u8,
    pub function: u8,

    pub vendor_id: u16,
    pub device_id: u16,

    pub class_code: u8,
    pub subclass: u8,
    pub programming_interface: u8,

    pub header_type: u8,

    pub bars: [PciBar; MAX_BARS],
    pub bar_count: usize,
}

impl PciDevice {
    pub const EMPTY: Self = Self {
        bus: 0,
        device: 0,
        function: 0,

        vendor_id: 0,
        device_id: 0,

        class_code: 0,
        subclass: 0,
        programming_interface: 0,

        header_type: 0,

        bars: [PciBar::EMPTY; MAX_BARS],
        bar_count: 0,
    };
}

pub struct PciInventory {
    pub devices: [PciDevice; MAX_PCI_DEVICES],
    pub count: usize,
    pub overflow: usize,
}

static mut PCI_INVENTORY: PciInventory =
    PciInventory::new();

impl PciInventory {
    pub const fn new() -> Self {
        Self {
            devices: [PciDevice::EMPTY; MAX_PCI_DEVICES],
            count: 0,
            overflow: 0,
        }
    }

    fn add(&mut self, device: PciDevice) {
        if self.count < MAX_PCI_DEVICES {
            self.devices[self.count] = device;
            self.count += 1;
        } else {
            self.overflow += 1;
        }
    }

    pub fn get(&self, index: usize) -> Option<&PciDevice> {
        if index < self.count {
            Some(&self.devices[index])
        } else {
            None
        }
    }
}

// ============================================================
// PCI CONFIGURATION SPACE
// ============================================================

fn config_address(
    bus: u8,
    device: u8,
    function: u8,
    offset: u8,
) -> u32 {
    0x8000_0000u32
        | ((bus as u32) << 16)
        | ((device as u32) << 11)
        | ((function as u32) << 8)
        | ((offset as u32) & 0xFC)
}

unsafe fn read_config_u32(
    bus: u8,
    device: u8,
    function: u8,
    offset: u8,
) -> u32 {
    let address = config_address(
        bus,
        device,
        function,
        offset,
    );

    unsafe {
        core::arch::asm!(
            "out dx, eax",
            in("dx") PCI_CONFIG_ADDRESS,
            in("eax") address,
            options(nomem, nostack, preserves_flags),
        );
    }

    let value: u32;

    unsafe {
        core::arch::asm!(
            "in eax, dx",
            in("dx") PCI_CONFIG_DATA,
            out("eax") value,
            options(nomem, nostack, preserves_flags),
        );
    }

    value
}

unsafe fn write_config_u32(
    bus: u8,
    device: u8,
    function: u8,
    offset: u8,
    value: u32,
) {
    let address = config_address(
        bus,
        device,
        function,
        offset,
    );

    unsafe {
        core::arch::asm!(
            "out dx, eax",
            in("dx") PCI_CONFIG_ADDRESS,
            in("eax") address,
            options(nomem, nostack, preserves_flags),
        );

        core::arch::asm!(
            "out dx, eax",
            in("dx") PCI_CONFIG_DATA,
            in("eax") value,
            options(nomem, nostack, preserves_flags),
        );
    }
}

unsafe fn read_config_u16(
    bus: u8,
    device: u8,
    function: u8,
    offset: u8,
) -> u16 {
    let address = config_address(
        bus,
        device,
        function,
        offset,
    );

    unsafe {
        core::arch::asm!(
            "out dx, eax",
            in("dx") PCI_CONFIG_ADDRESS,
            in("eax") address,
            options(nomem, nostack, preserves_flags),
        );
    }

    let value: u16;

    unsafe {
        core::arch::asm!(
            "in ax, dx",
            in("dx") (PCI_CONFIG_DATA + (offset as u16 & 2)),
            out("ax") value,
            options(nomem, nostack, preserves_flags),
        );
    }

    value
}

unsafe fn write_config_u16(
    bus: u8,
    device: u8,
    function: u8,
    offset: u8,
    value: u16,
) {
    let address = config_address(
        bus,
        device,
        function,
        offset,
    );

    unsafe {
        core::arch::asm!(
            "out dx, eax",
            in("dx") PCI_CONFIG_ADDRESS,
            in("eax") address,
            options(nomem, nostack, preserves_flags),
        );

        core::arch::asm!(
            "out dx, ax",
            in("dx") (PCI_CONFIG_DATA + (offset as u16 & 2)),
            in("ax") value,
            options(nomem, nostack, preserves_flags),
        );
    }
}

// ============================================================
// BAR SIZING
// ============================================================

fn probe_bar_size(
    bus: u8,
    device: u8,
    function: u8,
    offset: u8,
    kind: PciBarKind,
    original_low: u32,
    original_high: Option<u32>,
) -> Option<u64> {
    let mask = match kind {
        PciBarKind::Io => {
            unsafe {
                write_config_u32(
                    bus,
                    device,
                    function,
                    offset,
                    u32::MAX,
                );
            }

            let mask = unsafe {
                read_config_u32(
                    bus,
                    device,
                    function,
                    offset,
                )
            };

            unsafe {
                write_config_u32(
                    bus,
                    device,
                    function,
                    offset,
                    original_low,
                );
            }

            let mask = mask & 0xFFFF_FFFC;

            if mask == 0 {
                return None;
            }

            (!mask).wrapping_add(1) as u64
        }

        PciBarKind::Memory32 => {
            unsafe {
                write_config_u32(
                    bus,
                    device,
                    function,
                    offset,
                    u32::MAX,
                );
            }

            let mask = unsafe {
                read_config_u32(
                    bus,
                    device,
                    function,
                    offset,
                )
            };

            unsafe {
                write_config_u32(
                    bus,
                    device,
                    function,
                    offset,
                    original_low,
                );
            }

            let mask = mask & 0xFFFF_FFF0;

            if mask == 0 {
                return None;
            }

            (!mask).wrapping_add(1) as u64
        }

        PciBarKind::MemoryBelow1M => {
            unsafe {
                write_config_u32(
                    bus,
                    device,
                    function,
                    offset,
                    u32::MAX,
                );
            }

            let mask = unsafe {
                read_config_u32(
                    bus,
                    device,
                    function,
                    offset,
                )
            };

            unsafe {
                write_config_u32(
                    bus,
                    device,
                    function,
                    offset,
                    original_low,
                );
            }

            let mask = mask & 0x000F_FFF0;

            if mask == 0 {
                return None;
            }

            ((!mask) & 0x000F_FFFF)
                .wrapping_add(1) as u64
        }

        PciBarKind::Memory64 => {
            let original_high = original_high?;

            unsafe {
                write_config_u32(
                    bus,
                    device,
                    function,
                    offset,
                    u32::MAX,
                );

                write_config_u32(
                    bus,
                    device,
                    function,
                    offset + 4,
                    u32::MAX,
                );
            }

            let mask_low = unsafe {
                read_config_u32(
                    bus,
                    device,
                    function,
                    offset,
                )
            };

            let mask_high = unsafe {
                read_config_u32(
                    bus,
                    device,
                    function,
                    offset + 4,
                )
            };

            // Restore both halves before calculating the size.
            unsafe {
                write_config_u32(
                    bus,
                    device,
                    function,
                    offset + 4,
                    original_high,
                );

                write_config_u32(
                    bus,
                    device,
                    function,
                    offset,
                    original_low,
                );
            }

            let mask =
                ((mask_high as u64) << 32)
                | ((mask_low & 0xFFFF_FFF0) as u64);

            if mask == 0 {
                return None;
            }

            (!mask).wrapping_add(1)
        }

        PciBarKind::Reserved => return None,
    };

    if mask == 0 || !mask.is_power_of_two() {
        return None;
    }

    Some(mask)
}

// ============================================================
// BAR READING
// ============================================================

fn read_bars(
    bus: u8,
    device: u8,
    function: u8,
    header_type: u8,
) -> ([PciBar; MAX_BARS], usize) {
    let mut bars = [PciBar::EMPTY; MAX_BARS];

    let header_layout = header_type & 0x7F;

    let register_limit = match header_layout {
        0 => 6,
        1 => 2,
        2 => 1,
        _ => 0,
    };

    if register_limit == 0 {
        return (bars, 0);
    }

    // Preserve the original PCI Command register.
    let original_command = unsafe {
        read_config_u16(bus, device, function, 0x04)
    };

    // Disable I/O and memory decoding while probing BARs.
    unsafe {
        write_config_u16(
            bus,
            device,
            function,
            0x04,
            original_command & !0x0003,
        );
    }

    let mut index = 0usize;
    let mut count = 0usize;

    while index < register_limit {
        let offset = 0x10 + (index as u8 * 4);

        let low = unsafe {
            read_config_u32(
                bus,
                device,
                function,
                offset,
            )
        };

        if low == 0 {
            index += 1;
            continue;
        }

        let (kind, base, prefetchable, high) =
            if low & 1 != 0 {
                (
                    PciBarKind::Io,
                    (low & !0x03) as u64,
                    false,
                    None,
                )
            } else {
                let memory_type = (low >> 1) & 0x03;
                let prefetchable = low & 0x08 != 0;

                match memory_type {
                    0 => (
                        PciBarKind::Memory32,
                        (low & !0x0F) as u64,
                        prefetchable,
                        None,
                    ),

                    1 => (
                        PciBarKind::MemoryBelow1M,
                        (low & 0x000F_FFF0) as u64,
                        prefetchable,
                        None,
                    ),

                    2 if index + 1 < register_limit => {
                        let high = unsafe {
                            read_config_u32(
                                bus,
                                device,
                                function,
                                offset + 4,
                            )
                        };

                        (
                            PciBarKind::Memory64,
                            ((high as u64) << 32)
                                | ((low & !0x0F) as u64),
                            prefetchable,
                            Some(high),
                        )
                    }

                    _ => (
                        PciBarKind::Reserved,
                        (low & !0x0F) as u64,
                        prefetchable,
                        None,
                    ),
                }
            };

        let size = probe_bar_size(
            bus,
            device,
            function,
            offset,
            kind,
            low,
            high,
        );

        bars[count] = PciBar {
            index: index as u8,
            kind,
            base,
            size: size.unwrap_or(0),
            prefetchable,
            valid: true,
            size_known: size.is_some(),
        };

        count += 1;

        if kind == PciBarKind::Memory64 {
            index += 2;
        } else {
            index += 1;
        }
    }

    // Restore the original Command register.
    unsafe {
        write_config_u16(
            bus,
            device,
            function,
            0x04,
            original_command,
        );
    }

    (bars, count)
}

// ============================================================
// DEVICE READING
// ============================================================

fn read_device(
    bus: u8,
    device: u8,
    function: u8,
) -> Option<PciDevice> {
    let id = unsafe {
        read_config_u32(bus, device, function, 0x00)
    };

    let vendor_id = (id & 0xFFFF) as u16;
    let device_id = (id >> 16) as u16;

    if vendor_id == 0xFFFF {
        return None;
    }

    let class_info = unsafe {
        read_config_u32(bus, device, function, 0x08)
    };

    let header_info = unsafe {
        read_config_u32(bus, device, function, 0x0C)
    };

    let header_type = ((header_info >> 16) & 0xFF) as u8;

    let (bars, bar_count) = read_bars(
        bus,
        device,
        function,
        header_type,
    );

    Some(PciDevice {
        bus,
        device,
        function,

        vendor_id,
        device_id,

        class_code: (class_info >> 24) as u8,
        subclass: (class_info >> 16) as u8,
        programming_interface: (class_info >> 8) as u8,

        header_type,

        bars,
        bar_count,
    })
}

// ============================================================
// PCI ENUMERATION
// ============================================================

use crate::hardware::{
    HardwareDiscovery,
    PciAhciDescriptor,
    PciAudioDescriptor,
    PciNetworkDescriptor,
    PciVirtioBlkDescriptor,
};

fn enable_pci_device(device: &PciDevice, io: bool, mem: bool, bus_master: bool) -> u8 {
    let mut cmd = unsafe {
        read_config_u16(device.bus, device.device, device.function, 0x04)
    };
    if io {
        cmd |= 1 << 0;
    }
    if mem {
        cmd |= 1 << 1;
    }
    if bus_master {
        cmd |= 1 << 2;
    }
    // Clear Interrupt Disable bit (bit 10).
    cmd &= !(1 << 10);

    unsafe {
        write_config_u16(device.bus, device.device, device.function, 0x04, cmd);
    }

    let irq_reg = unsafe {
        read_config_u32(device.bus, device.device, device.function, 0x3C)
    };
    (irq_reg & 0xFF) as u8
}

fn inspect_network_device(device: &PciDevice) -> Option<PciNetworkDescriptor> {
    if device.vendor_id != 0x8086 {
        return None;
    }
    if !matches!(device.device_id, 0x100E | 0x100F | 0x10D3 | 0x1004 | 0x1019) {
        return None;
    }

    let mut mmio_base = 0u64;
    let mut mmio_size = 0u64;
    let mut io_base = 0u16;

    for i in 0..device.bar_count {
        let bar = device.bars[i];
        if !bar.valid || bar.base == 0 {
            continue;
        }
        match bar.kind {
            PciBarKind::Memory32 | PciBarKind::Memory64 => {
                if mmio_base == 0 {
                    mmio_base = bar.base;
                    mmio_size = if bar.size_known && bar.size > 0 {
                        bar.size
                    } else {
                        0x20000
                    };
                }
            }
            PciBarKind::Io => {
                if io_base == 0 {
                    io_base = bar.base as u16;
                }
            }
            _ => {}
        }
    }

    if mmio_base == 0 {
        return None;
    }

    let irq_line = enable_pci_device(device, io_base != 0, true, true);

    Some(PciNetworkDescriptor {
        bus: device.bus,
        device: device.device,
        function: device.function,
        vendor_id: device.vendor_id,
        device_id: device.device_id,
        irq_line,
        mmio_base,
        mmio_size,
        io_base,
        valid: true,
    })
}

fn inspect_audio_device(device: &PciDevice) -> Option<PciAudioDescriptor> {
    if device.vendor_id != 0x8086 {
        return None;
    }
    if device.device_id != 0x2415
        && !(device.class_code == 0x04 && device.subclass == 0x01)
    {
        return None;
    }

    let mut nambar_io = 0u16;
    let mut nabmbar_io = 0u16;

    for i in 0..device.bar_count {
        let bar = device.bars[i];
        if !bar.valid || bar.kind != PciBarKind::Io || bar.base == 0 {
            continue;
        }
        if bar.index == 0 {
            nambar_io = bar.base as u16;
        } else if bar.index == 1 {
            nabmbar_io = bar.base as u16;
        }
    }

    if nambar_io == 0 || nabmbar_io == 0 {
        return None;
    }

    let irq_line = enable_pci_device(device, true, false, true);

    Some(PciAudioDescriptor {
        bus: device.bus,
        device: device.device,
        function: device.function,
        vendor_id: device.vendor_id,
        device_id: device.device_id,
        irq_line,
        nambar_io,
        nabmbar_io,
        valid: true,
    })
}

fn inspect_ahci_device(device: &PciDevice) -> Option<PciAhciDescriptor> {
    if !(device.class_code == 0x01 && device.subclass == 0x06) {
        return None;
    }

    let mut abar_phys = 0u64;
    let mut abar_size = 0u64;
    // AHCI spec places ABAR in BAR5, falling back to first valid MMIO BAR
    for i in 0..device.bar_count {
        let bar = device.bars[i];
        if !bar.valid || bar.base == 0 {
            continue;
        }
        if matches!(bar.kind, PciBarKind::Memory32 | PciBarKind::Memory64) {
            if bar.index == 5 || abar_phys == 0 {
                abar_phys = bar.base;
                abar_size = if bar.size_known && bar.size > 0 {
                    bar.size
                } else {
                    0x2000
                };
            }
        }
    }

    if abar_phys == 0 {
        return None;
    }

    let irq_line = enable_pci_device(device, true, true, true);
    Some(PciAhciDescriptor {
        bus: device.bus,
        device: device.device,
        function: device.function,
        vendor_id: device.vendor_id,
        device_id: device.device_id,
        irq_line,
        abar_phys,
        abar_size,
        valid: true,
    })
}

fn inspect_virtio_blk_device(device: &PciDevice) -> Option<PciVirtioBlkDescriptor> {
    if device.vendor_id != 0x1AF4 || !matches!(device.device_id, 0x1001 | 0x1042) {
        return None;
    }

    let mut io_base = 0u16;
    let mut mmio_base = 0u64;
    let mut mmio_size = 0u64;
    for i in 0..device.bar_count {
        let bar = device.bars[i];
        if !bar.valid || bar.base == 0 {
            continue;
        }
        match bar.kind {
            PciBarKind::Io if io_base == 0 => io_base = bar.base as u16,
            PciBarKind::Memory32 | PciBarKind::Memory64 => {
                if mmio_base == 0 || bar.index == 4 {
                    mmio_base = bar.base;
                    mmio_size = if bar.size_known && bar.size > 0 {
                        bar.size
                    } else {
                        0x4000
                    };
                }
            }
            _ => {}
        }
    }

    if io_base == 0 && mmio_base == 0 {
        return None;
    }

    let irq_line = enable_pci_device(device, io_base != 0, mmio_base != 0, true);
    Some(PciVirtioBlkDescriptor {
        bus: device.bus,
        device: device.device,
        function: device.function,
        vendor_id: device.vendor_id,
        device_id: device.device_id,
        irq_line,
        io_base,
        mmio_base,
        mmio_size,
        is_modern: device.device_id == 0x1042 || io_base == 0,
        valid: true,
    })
}

pub fn scan(console: &mut Console<'_>) -> HardwareDiscovery {
    // Используем заранее выделенную статическую память.
    // Не создаём большой PciInventory на стеке.
    let inventory_ptr =
        core::ptr::addr_of_mut!(PCI_INVENTORY);

    let inventory = unsafe {
        &mut *inventory_ptr
    };

    // Подготавливаем инвентарь к сканированию.
    inventory.count = 0;
    inventory.overflow = 0;
    let mut discovery = HardwareDiscovery::EMPTY;

    console.write(
        "\nPCI Bus initialization:\n",
        GREEN,
    );

    console.write(
        "  Configuration mechanism: ",
        GRAY,
    );

    console.write(
        "PCI Mechanism #1\n",
        WHITE,
    );

    console.write(
        "  Scanning PCI buses...\n",
        GRAY,
    );

    for bus in 0u16..=255 {
        for device in 0u8..32 {
            let first = match read_device(
                bus as u8,
                device,
                0,
            ) {
                Some(found) => found,
                None => continue,
            };

            let multifunction =
                first.header_type & 0x80 != 0;

            inventory.add(first);

            let function_limit =
                if multifunction { 8 } else { 1 };

            for function in 1..function_limit {
                if let Some(found) = read_device(
                    bus as u8,
                    device,
                    function,
                ) {
                    inventory.add(found);
                }
            }
        }
    }

display_inventory(console, inventory);

driver::match_devices(console, inventory);
// ============================================================
// IDE CONTROLLER DETECTION
// ============================================================
console.write("\nIDE Controller Detection:\n", GREEN);

let mut ide_controllers_found = 0usize;

for index in 0..inventory.count {
    let device = &inventory.devices[index];

    if let Some(info) = ide::inspect(device) {
        ide_controllers_found += 1;

        ide::display(console, &info);

        if !discovery.ide_controllers.push(info) {
            console.write(
                "  IDE discovery list is full!\n",
                RED,
            );
        }
    }

    if !discovery.network.valid {
        if let Some(net_info) = inspect_network_device(device) {
            discovery.network = net_info;
        }
    }

    if !discovery.audio.valid {
        if let Some(audio_info) = inspect_audio_device(device) {
            discovery.audio = audio_info;
        }
    }

    if !discovery.ahci.valid {
        if let Some(ahci_info) = inspect_ahci_device(device) {
            discovery.ahci = ahci_info;
        }
    }

    if !discovery.virtio_blk.valid {
        if let Some(vblk_info) = inspect_virtio_blk_device(device) {
            discovery.virtio_blk = vblk_info;
        }
    }
}

console.write("\nIDE Controller Summary:\n", GREEN);

console.write("  Detected controllers: ", GRAY);
write_decimal(console, ide_controllers_found as u64);
console.write("\n", WHITE);

console.write("  Discovered slots: ", GRAY);
write_decimal(
    console,
    discovery.ide_controllers.count as u64,
);
console.write("\n", WHITE);

console.write("  Discovery overflow: ", GRAY);
write_decimal(
    console,
    discovery.ide_controllers.overflow as u64,
);
console.write("\n", WHITE);

if ide_controllers_found > 0 {
    console.write("  IDE subsystem: ", GRAY);
    console.write("DETECTED\n", GREEN);
} else {
    console.write("  IDE subsystem: ", GRAY);
    console.write("NOT FOUND\n", YELLOW);
}

console.write("\nNetwork, Audio & Modern Storage PCI Detection:\n", GREEN);
if discovery.network.valid {
    console.write("  Intel e1000 NIC: ", GRAY);
    console.write("DETECTED (MMIO ", GREEN);
    write_hex64(console, discovery.network.mmio_base);
    console.write(")\n", GREEN);
} else {
    console.write("  Intel e1000 NIC: ", GRAY);
    console.write("NOT FOUND\n", YELLOW);
}

if discovery.audio.valid {
    console.write("  Intel AC'97 Audio: ", GRAY);
    console.write("DETECTED (NAMBAR ", GREEN);
    write_hex16(console, discovery.audio.nambar_io);
    console.write(", NABMBAR ", GREEN);
    write_hex16(console, discovery.audio.nabmbar_io);
    console.write(")\n", GREEN);
} else {
    console.write("  Intel AC'97 Audio: ", GRAY);
    console.write("NOT FOUND\n", YELLOW);
}

if discovery.ahci.valid {
    console.write("  AHCI SATA HBA:     ", GRAY);
    console.write("DETECTED (ABAR ", GREEN);
    write_hex64(console, discovery.ahci.abar_phys);
    console.write(")\n", GREEN);
} else {
    console.write("  AHCI SATA HBA:     ", GRAY);
    console.write("NOT FOUND\n", YELLOW);
}

if discovery.virtio_blk.valid {
    console.write("  VirtIO-Blk PCI:    ", GRAY);
    console.write("DETECTED\n", GREEN);
} else {
    console.write("  VirtIO-Blk PCI:    ", GRAY);
    console.write("NOT FOUND\n", YELLOW);
}

// Build the PCI BAR resource registry.
let resource_stats = resources::rebuild();
let validation_stats = validation::validate();

console.write("\nPCI Resource Validation:\n", GREEN);

console.write("  Checked resources: ", GRAY);
write_decimal(console, validation_stats.checked_resources as u64);
console.write("\n", WHITE);

console.write("  Invalid resources: ", GRAY);
write_decimal(console, validation_stats.invalid_resources as u64);
console.write("\n", WHITE);

console.write("  Unknown sizes: ", GRAY);
write_decimal(console, validation_stats.unknown_sizes as u64);
console.write("\n", WHITE);

console.write("  Invalid sizes: ", GRAY);
write_decimal(console, validation_stats.invalid_sizes as u64);
console.write("\n", WHITE);

console.write("  Misaligned addresses: ", GRAY);
write_decimal(console, validation_stats.misaligned_addresses as u64);
console.write("\n", WHITE);

console.write("  Address overflows: ", GRAY);
write_decimal(console, validation_stats.address_overflows as u64);
console.write("\n", WHITE);

console.write("  Out of range: ", GRAY);
write_decimal(console, validation_stats.out_of_range as u64);
console.write("\n", WHITE);

console.write("  Overlapping pairs: ", GRAY);
write_decimal(console, validation_stats.overlapping_pairs as u64);
console.write("\n", WHITE);

if validation_stats.invalid_resources == 0
    && validation_stats.overlapping_pairs == 0
{
    console.write("  Validation: ", GRAY);
    console.write("PASSED\n", GREEN);
} else {
    console.write("  Validation: ", GRAY);
    console.write("WARNINGS FOUND\n", YELLOW);
}

console.write("\nPCI Resource Registry:\n", GREEN);

console.write("  Registered resources: ", GRAY);
write_decimal(
    console,
    resource_stats.resources_registered as u64,
);
console.write("\n", WHITE);

console.write("  Unknown resource sizes: ", GRAY);
write_decimal(
    console,
    resource_stats.unknown_sizes as u64,
);
console.write("\n", WHITE);

console.write("  Registry overflow: ", GRAY);
write_decimal(
    console,
    resource_stats.overflow as u64,
);
console.write("\n", WHITE);

console.write("  Status: ", GRAY);
console.write("READY\n", GREEN);

discovery
}

// ============================================================
// DEVICE INFORMATION
// ============================================================

fn display_inventory(
    console: &mut Console<'_>,
    inventory: &PciInventory,
) {
    console.write(
        "\nDetected PCI devices:\n",
        GREEN,
    );

    if inventory.count == 0 {
        console.write(
            "  No PCI devices found\n",
            YELLOW,
        );

        return;
    }

    for index in 0..inventory.count {
        let device = inventory.devices[index];

        console.write("  PCI ", GRAY);

        write_decimal(console, device.bus as u64);
        console.write(":", GRAY);

        write_decimal(console, device.device as u64);
        console.write(".", GRAY);

        write_decimal(console, device.function as u64);

        console.write("  ", WHITE);

        write_hex16(console, device.vendor_id);
        console.write(":", GRAY);
        write_hex16(console, device.device_id);

        console.write("  ", WHITE);

        write_class_name(console, device.class_code);

        console.write("\n", WHITE);
    }

    console.write(
        "\nPCI BAR resources:\n",
        GREEN,
    );

    for device_index in 0..inventory.count {
        let device = &inventory.devices[device_index];

        if device.bar_count == 0 {
            continue;
        }

        console.write("  Device ", GRAY);

        write_decimal(console, device.bus as u64);
        console.write(":", GRAY);
        write_decimal(console, device.device as u64);
        console.write(".", GRAY);
        write_decimal(console, device.function as u64);

        console.write("\n", WHITE);

        for bar_index in 0..device.bar_count {
            let bar = device.bars[bar_index];

            if !bar.valid {
                continue;
            }

            console.write("    BAR", GRAY);
            write_decimal(console, bar.index as u64);
            console.write(": ", GRAY);

            match bar.kind {
                PciBarKind::Io => {
                    console.write("I/O Port  ", WHITE);
                }

                PciBarKind::Memory32 => {
                    console.write("Memory 32-bit  ", WHITE);
                }

                PciBarKind::MemoryBelow1M => {
                    console.write("Memory below 1 MB  ", WHITE);
                }

                PciBarKind::Memory64 => {
                    console.write("Memory 64-bit  ", WHITE);
                }

                PciBarKind::Reserved => {
                    console.write("Reserved  ", YELLOW);
                }
            }

            console.write("Base: ", GRAY);
            write_hex64(console, bar.base);

            if bar.prefetchable {
                console.write("  Prefetchable", GREEN);
            }

            if bar.size_known {
                console.write("  Size: ", GRAY);
                write_hex64(console, bar.size);
                console.write(" (", GRAY);
                write_decimal(console, bar.size);
                console.write(" bytes)", GRAY);
            } else {
                console.write("  Size: UNKNOWN", YELLOW);
            }

            console.write("\n", WHITE);
        }
    }

    console.write(
        "\nPCI statistics:\n",
        GREEN,
    );

    console.write("  Detected devices: ", GRAY);
    write_decimal(console, inventory.count as u64);
    console.write("\n", WHITE);

    console.write("  Inventory overflow: ", GRAY);
    write_decimal(console, inventory.overflow as u64);
    console.write("\n", WHITE);

    console.write("  BAR sizing: ", GRAY);
    console.write("COMPLETED\n", GREEN);

    console.write("  PCI scan: ", GRAY);
    console.write("SUCCESS\n", GREEN);
}

// ============================================================
// PCI CLASS NAMES
// ============================================================

fn write_class_name(
    console: &mut Console<'_>,
    class: u8,
) {
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

// ============================================================
// NUMBER FORMATTING
// ============================================================

fn write_decimal(
    console: &mut Console<'_>,
    mut number: u64,
) {
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

fn write_hex16(
    console: &mut Console<'_>,
    value: u16,
) {
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

fn write_hex64(
    console: &mut Console<'_>,
    value: u64,
) {
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

// ============================================================
// PCI INVENTORY ACCESS
// ============================================================

/// Returns a copy of a PCI device from the scanned inventory.
pub fn get_device(index: usize) -> Option<PciDevice> {
    let inventory_ptr = core::ptr::addr_of!(PCI_INVENTORY);

    unsafe {
        let inventory = &*inventory_ptr;

        if index < inventory.count {
            Some(inventory.devices[index])
        } else {
            None
        }
    }
}
