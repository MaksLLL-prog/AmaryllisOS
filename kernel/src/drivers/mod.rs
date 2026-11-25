pub mod ac97;
pub mod ahci;
pub mod e1000;
pub mod iokit;
pub mod registry;
pub mod virtio_blk;

use core::fmt::Write;

use bootloader_api::info::FrameBufferInfo;

use crate::console::Console;
use crate::graphics::Color;
use crate::hardware::HardwareDiscovery;
use crate::input;
use crate::memory::PhysicalFrameAllocator;
use crate::memory::vmm::PhysicalMemoryMapping;
use crate::storage::{ide, vfs};

use registry::{DeviceBus, DeviceClass, DriverStatus};

const GREEN: Color = (40, 180, 120);
const WHITE: Color = (235, 235, 240);
const GRAY: Color = (140, 140, 150);
const YELLOW: Color = (220, 190, 90);

struct FixedStr<const N: usize> {
    buf: [u8; N],
    len: usize,
}

impl<const N: usize> FixedStr<N> {
    const fn new() -> Self {
        Self {
            buf: [0; N],
            len: 0,
        }
    }

    fn as_str(&self) -> &str {
        core::str::from_utf8(&self.buf[..self.len]).unwrap_or("")
    }
}

impl<const N: usize> Write for FixedStr<N> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let bytes = s.as_bytes();
        let avail = N.saturating_sub(self.len);
        let n = bytes.len().min(avail);
        self.buf[self.len..self.len + n].copy_from_slice(&bytes[..n]);
        self.len += n;
        Ok(())
    }
}

#[inline(never)]
pub fn init_all(
    console: &mut Console<'_>,
    discovery: &HardwareDiscovery,
    mapping: Option<&PhysicalMemoryMapping>,
    allocator: &mut PhysicalFrameAllocator<'_>,
    fb_info: &FrameBufferInfo,
) {
    console.write("\nUnified Driver Manager initialization:\n", GREEN);

    registry::DRIVER_REGISTRY.lock().clear();

    // 1. PIT System Timer
    registry::register_device(
        "PIT Timer",
        "8253/8254 PIT (100 Hz)",
        DeviceClass::Timer,
        DeviceBus::System,
        DriverStatus::Active,
        Some(0),
        0x40,
        0,
        "IRQ0 system tick source",
    );

    // 2. PS/2 Keyboard
    registry::register_device(
        "PS/2 Keyboard",
        "i8042 Keyboard Driver",
        DeviceClass::Keyboard,
        DeviceBus::Ps2,
        DriverStatus::Active,
        Some(1),
        0x60,
        0,
        "Scancode set 1",
    );

    // 3. PS/2 Mouse
    let mouse_status = if input::mouse::is_initialized() {
        DriverStatus::Active
    } else {
        DriverStatus::NotPresent
    };
    let mouse_details = if !input::mouse::is_initialized() {
        "PS/2 mouse inactive"
    } else if input::mouse::packet_size() == 4 {
        "4-byte wheel packet mode"
    } else {
        "3-byte standard packet mode"
    };
    registry::register_device(
        "PS/2 Mouse",
        "i8042 PS/2 Mouse",
        DeviceClass::Mouse,
        DeviceBus::Ps2,
        mouse_status,
        Some(12),
        0x60,
        0,
        mouse_details,
    );

    // 4. Linear Framebuffer Display
    let mut fb_details = FixedStr::<64>::new();
    let _ = write!(
        fb_details,
        "{}x{} {}bpp",
        fb_info.width,
        fb_info.height,
        fb_info.bytes_per_pixel * 8
    );
    registry::register_device(
        "Framebuffer",
        "Bootloader Linear FB",
        DeviceClass::Framebuffer,
        DeviceBus::Framebuffer,
        DriverStatus::Active,
        None,
        0,
        0,
        fb_details.as_str(),
    );

    // 5. Storage / ATA IDE + VFS
    let ide_count = ide::controller_count();
    let vfs_mounted = vfs::with_vfs(|state| state.is_mounted());
    let mut storage_details = FixedStr::<64>::new();
    let _ = write!(
        storage_details,
        "{} IDE ctrl(s), VFS: {}",
        ide_count,
        if vfs_mounted { "mounted" } else { "unmounted" }
    );
    let storage_status = if ide_count > 0 {
        DriverStatus::Active
    } else {
        DriverStatus::NotPresent
    };
    registry::register_device(
        "ATA/IDE Storage",
        "ATA PIO BlockDevice + VFS",
        DeviceClass::Storage,
        DeviceBus::System,
        storage_status,
        Some(14),
        0x1F0,
        0,
        storage_details.as_str(),
    );

    // 6. Intel e1000 Network Controller
    if discovery.network.valid {
        let net_bus = DeviceBus::Pci {
            bus: discovery.network.bus,
            device: discovery.network.device,
            function: discovery.network.function,
            vendor_id: discovery.network.vendor_id,
            device_id: discovery.network.device_id,
        };
        match e1000::init(&discovery.network, mapping, allocator) {
            Ok(mac) => {
                crate::net::set_mac(mac);
                let mut details = FixedStr::<64>::new();
                let _ = write!(
                    details,
                    "MAC {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
                    mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
                );
                registry::register_device(
                    "Intel e1000",
                    "e1000 MMIO/DMA NIC",
                    DeviceClass::Network,
                    net_bus,
                    DriverStatus::Active,
                    Some(discovery.network.irq_line),
                    discovery.network.io_base as u64,
                    discovery.network.mmio_base,
                    details.as_str(),
                );
                console.write("  [ OK ] ", GREEN);
                console.write("Intel e1000 NIC initialized (", WHITE);
                console.write(details.as_str(), GRAY);
                console.write(")\n", WHITE);
            }
            Err(err) => {
                registry::register_device(
                    "Intel e1000",
                    "e1000 MMIO/DMA NIC",
                    DeviceClass::Network,
                    net_bus,
                    DriverStatus::Failed,
                    Some(discovery.network.irq_line),
                    discovery.network.io_base as u64,
                    discovery.network.mmio_base,
                    err,
                );
                console.write("  [FAIL] Intel e1000 NIC: ", YELLOW);
                console.write(err, WHITE);
                console.write("\n", WHITE);
            }
        }
    } else {
        registry::register_device(
            "Intel e1000",
            "e1000 MMIO/DMA NIC",
            DeviceClass::Network,
            DeviceBus::System,
            DriverStatus::NotPresent,
            None,
            0,
            0,
            "Use QEMU -device e1000",
        );
    }

    // 7. Intel AC'97 Audio Controller
    if discovery.audio.valid {
        let audio_bus = DeviceBus::Pci {
            bus: discovery.audio.bus,
            device: discovery.audio.device,
            function: discovery.audio.function,
            vendor_id: discovery.audio.vendor_id,
            device_id: discovery.audio.device_id,
        };
        match ac97::init(&discovery.audio, mapping, allocator) {
            Ok(rate) => {
                let mut details = FixedStr::<64>::new();
                let _ = write!(details, "{} Hz PCM Out", rate);
                registry::register_device(
                    "Intel AC'97",
                    "AC'97 BDL/DMA Audio",
                    DeviceClass::Audio,
                    audio_bus,
                    DriverStatus::Active,
                    Some(discovery.audio.irq_line),
                    discovery.audio.nambar_io as u64,
                    0,
                    details.as_str(),
                );
                console.write("  [ OK ] ", GREEN);
                console.write("Intel AC'97 Audio initialized (", WHITE);
                console.write(details.as_str(), GRAY);
                console.write(")\n", WHITE);
            }
            Err(err) => {
                registry::register_device(
                    "Intel AC'97",
                    "AC'97 BDL/DMA Audio",
                    DeviceClass::Audio,
                    audio_bus,
                    DriverStatus::Failed,
                    Some(discovery.audio.irq_line),
                    discovery.audio.nambar_io as u64,
                    0,
                    err,
                );
                console.write("  [FAIL] Intel AC'97 Audio: ", YELLOW);
                console.write(err, WHITE);
                console.write("\n", WHITE);
            }
        }
    } else {
        registry::register_device(
            "Intel AC'97",
            "AC'97 BDL/DMA Audio",
            DeviceClass::Audio,
            DeviceBus::System,
            DriverStatus::NotPresent,
            None,
            0,
            0,
            "Use QEMU -device AC97",
        );
    }

    // 8. ACPI Platform & APIC/SMP Topology
    let acpi_info = crate::arch::acpi::snapshot();
    let apic_snap = crate::arch::apic::snapshot();
    let mut acpi_details = FixedStr::<64>::new();
    if acpi_info.discovered {
        let _ = write!(
            acpi_details,
            "{} tbl={} cpu={} ioapic={}",
            if acpi_info.using_xsdt { "XSDT" } else { "RSDT" },
            acpi_info.table_count,
            acpi_info.cpu_count,
            acpi_info.ioapic_count
        );
    } else {
        let _ = write!(acpi_details, "RSDP unavailable");
    }
    registry::register_device(
        "ACPI Platform",
        "ACPI MADT/FADT/MCFG/HPET",
        DeviceClass::Controller,
        DeviceBus::System,
        if acpi_info.discovered {
            DriverStatus::Active
        } else {
            DriverStatus::NotPresent
        },
        None,
        acpi_info.fadt_pm1a_cnt as u64,
        acpi_info.rsdp_phys,
        acpi_details.as_str(),
    );

    let mut apic_details = FixedStr::<64>::new();
    let _ = write!(
        apic_details,
        "BSP={} cores={} IOAPIC={}",
        apic_snap.bsp_apic_id, apic_snap.enabled_cores, apic_snap.ioapic_count
    );
    registry::register_device(
        "LAPIC / IOAPIC SMP",
        "x86_64 APIC Topology",
        DeviceClass::Controller,
        DeviceBus::System,
        if apic_snap.lapic_enabled {
            DriverStatus::Active
        } else {
            DriverStatus::NotPresent
        },
        None,
        0,
        apic_snap.lapic_phys,
        apic_details.as_str(),
    );

    // 9. AHCI SATA Controller
    if discovery.ahci.valid {
        let ahci_bus = DeviceBus::Pci {
            bus: discovery.ahci.bus,
            device: discovery.ahci.device,
            function: discovery.ahci.function,
            vendor_id: discovery.ahci.vendor_id,
            device_id: discovery.ahci.device_id,
        };
        match ahci::init(&discovery.ahci, mapping, allocator) {
            Ok(snap) => {
                let mut details = FixedStr::<64>::new();
                if let Some(p) = snap.active_port {
                    let _ = write!(details, "Port{} {} sect={}", p, snap.model_str(), snap.sector_count);
                } else {
                    let _ = write!(details, "v0x{:x} no SATA link", snap.version);
                }
                registry::register_device(
                    "AHCI SATA",
                    "AHCI HBA DMA",
                    DeviceClass::Storage,
                    ahci_bus,
                    DriverStatus::Active,
                    Some(discovery.ahci.irq_line),
                    0,
                    discovery.ahci.abar_phys,
                    details.as_str(),
                );
                console.write("  [ OK ] ", GREEN);
                console.write("AHCI SATA DMA initialized (", WHITE);
                console.write(details.as_str(), GRAY);
                console.write(")\n", WHITE);
            }
            Err(err) => {
                registry::register_device(
                    "AHCI SATA",
                    "AHCI HBA DMA",
                    DeviceClass::Storage,
                    ahci_bus,
                    DriverStatus::Failed,
                    Some(discovery.ahci.irq_line),
                    0,
                    discovery.ahci.abar_phys,
                    err,
                );
                console.write("  [FAIL] AHCI SATA: ", YELLOW);
                console.write(err, WHITE);
                console.write("\n", WHITE);
            }
        }
    } else {
        registry::register_device(
            "AHCI SATA",
            "AHCI HBA DMA",
            DeviceClass::Storage,
            DeviceBus::System,
            DriverStatus::NotPresent,
            None,
            0,
            0,
            "Use QEMU -device ahci,id=ahci",
        );
    }

    // 10. VirtIO-Blk PCI Controller
    if discovery.virtio_blk.valid {
        let v_bus = DeviceBus::Pci {
            bus: discovery.virtio_blk.bus,
            device: discovery.virtio_blk.device,
            function: discovery.virtio_blk.function,
            vendor_id: discovery.virtio_blk.vendor_id,
            device_id: discovery.virtio_blk.device_id,
        };
        match virtio_blk::init(&discovery.virtio_blk, mapping, allocator) {
            Ok(snap) => {
                let mut details = FixedStr::<64>::new();
                let _ = write!(details, "qsz={} sect={}", snap.queue_size, snap.sector_count);
                registry::register_device(
                    "VirtIO-Blk",
                    "VirtIO Split-Queue DMA",
                    DeviceClass::Storage,
                    v_bus,
                    DriverStatus::Active,
                    Some(discovery.virtio_blk.irq_line),
                    discovery.virtio_blk.io_base as u64,
                    discovery.virtio_blk.mmio_base,
                    details.as_str(),
                );
                console.write("  [ OK ] ", GREEN);
                console.write("VirtIO-Blk DMA initialized (", WHITE);
                console.write(details.as_str(), GRAY);
                console.write(")\n", WHITE);
            }
            Err(err) => {
                registry::register_device(
                    "VirtIO-Blk",
                    "VirtIO Split-Queue DMA",
                    DeviceClass::Storage,
                    v_bus,
                    DriverStatus::Failed,
                    Some(discovery.virtio_blk.irq_line),
                    discovery.virtio_blk.io_base as u64,
                    discovery.virtio_blk.mmio_base,
                    err,
                );
                console.write("  [WARN] VirtIO-Blk: ", YELLOW);
                console.write(err, WHITE);
                console.write("\n", WHITE);
            }
        }
    } else {
        registry::register_device(
            "VirtIO-Blk",
            "VirtIO Split-Queue DMA",
            DeviceClass::Storage,
            DeviceBus::System,
            DriverStatus::NotPresent,
            None,
            0,
            0,
            "Use QEMU -drive if=virtio",
        );
    }

    iokit::init_io_services(discovery);
    let io_st = iokit::stats();

    console.write("  [ OK ] ", GREEN);
    console.write("Unified driver registry & IORegistry ready (IOAPIC vecs=", WHITE);
    let mut vec_buf = FixedStr::<16>::new();
    let _ = write!(vec_buf, "{}", io_st.routed_vectors);
    console.write(vec_buf.as_str(), GREEN);
    console.write(")\n", WHITE);
}
