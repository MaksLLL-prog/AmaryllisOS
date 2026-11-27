pub mod ac97;
pub mod acpi_pm;
pub mod ahci;
pub mod e1000;
pub mod hda;
pub mod iokit;
pub mod nvme;
pub mod re_nic;
pub mod registry;
pub mod sdhci;
pub mod uefi_gop;
pub mod usb;
pub mod virtio_blk;

use core::fmt::Write;

use bootloader_api::info::FrameBufferInfo;

use crate::console::Console;
use crate::graphics::Color;
use crate::hardware::HardwareDiscovery;
use crate::input;
use crate::memory::vmm::PhysicalMemoryMapping;
use crate::memory::PhysicalFrameAllocator;
use crate::storage::{ide, vfs};

use registry::{AudioDevice, DeviceBus, DeviceClass, DriverStatus};

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

// ============================================================
// UNIFIED NETWORK FACADE (Intel e1000 / Realtek re/rl)
// ============================================================

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ActiveNetDriver {
    None,
    E1000,
    Realtek,
}

#[derive(Clone, Copy)]
pub struct UnifiedNetStats {
    pub driver: ActiveNetDriver,
    pub driver_name: &'static str,
    pub initialized: bool,
    pub link_up: bool,
    pub mac: [u8; 6],
    pub mmio_phys: u64,
    pub io_base: u16,
    pub irq_line: u8,
    pub tx_packets: u64,
    pub rx_packets: u64,
    pub tx_bytes: u64,
    pub rx_bytes: u64,
    pub tx_errors: u64,
}

impl UnifiedNetStats {
    pub const EMPTY: Self = Self {
        driver: ActiveNetDriver::None,
        driver_name: "None",
        initialized: false,
        link_up: false,
        mac: [0; 6],
        mmio_phys: 0,
        io_base: 0,
        irq_line: 0,
        tx_packets: 0,
        rx_packets: 0,
        tx_bytes: 0,
        rx_bytes: 0,
        tx_errors: 0,
    };
}

pub fn net_is_ready() -> bool {
    e1000::is_ready() || re_nic::is_ready()
}

pub fn net_driver_label() -> &'static str {
    if e1000::is_ready() {
        "Intel e1000"
    } else if re_nic::is_ready() {
        if re_nic::stats().is_rtl8139 {
            "Realtek RTL8139"
        } else {
            "Realtek RTL8168/8169"
        }
    } else {
        "None"
    }
}

pub fn net_stats() -> UnifiedNetStats {
    if e1000::is_ready() {
        let st = e1000::stats();
        UnifiedNetStats {
            driver: ActiveNetDriver::E1000,
            driver_name: "Intel e1000 / e1000e",
            initialized: st.initialized,
            link_up: st.link_up,
            mac: st.mac,
            mmio_phys: st.mmio_phys,
            io_base: 0,
            irq_line: st.irq_line,
            tx_packets: st.tx_packets,
            rx_packets: st.rx_packets,
            tx_bytes: st.tx_bytes,
            rx_bytes: st.rx_bytes,
            tx_errors: st.tx_errors,
        }
    } else if re_nic::is_ready() {
        let st = re_nic::stats();
        UnifiedNetStats {
            driver: ActiveNetDriver::Realtek,
            driver_name: if st.is_rtl8139 {
                "Realtek RTL8139 (FreeBSD rl)"
            } else {
                "Realtek RTL8168/8169 (FreeBSD re)"
            },
            initialized: st.initialized,
            link_up: st.link_up,
            mac: st.mac,
            mmio_phys: st.mmio_phys,
            io_base: st.io_base,
            irq_line: st.irq_line,
            tx_packets: st.tx_packets,
            rx_packets: st.rx_packets,
            tx_bytes: st.tx_bytes,
            rx_bytes: st.rx_bytes,
            tx_errors: st.tx_errors,
        }
    } else {
        UnifiedNetStats::EMPTY
    }
}

pub fn net_send_frame(frame: &[u8]) -> Result<(), &'static str> {
    if e1000::is_ready() {
        e1000::send_frame(frame)
    } else if re_nic::is_ready() {
        re_nic::send_frame(frame)
    } else {
        Err("No active network interface driver")
    }
}

pub fn net_poll_frame(out: &mut [u8]) -> Option<usize> {
    if e1000::is_ready() {
        e1000::poll_frame(out)
    } else if re_nic::is_ready() {
        re_nic::poll_frame(out)
    } else {
        None
    }
}

// ============================================================
// UNIFIED AUDIO FACADE (Intel HDA / Intel AC'97)
// ============================================================

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ActiveAudioDriver {
    None,
    Ac97,
    IntelHda,
}

#[derive(Clone, Copy)]
pub struct UnifiedAudioStats {
    pub driver: ActiveAudioDriver,
    pub driver_name: &'static str,
    pub initialized: bool,
    pub sample_rate: u32,
    pub volume_percent: u8,
    pub playing: bool,
    pub buffers_played: u64,
    pub irq_line: u8,
    pub vra_supported: bool,
    pub nambar: u16,
    pub nabmbar: u16,
    pub mmio_phys: u64,
}

impl UnifiedAudioStats {
    pub const EMPTY: Self = Self {
        driver: ActiveAudioDriver::None,
        driver_name: "None",
        initialized: false,
        sample_rate: 48_000,
        volume_percent: 80,
        playing: false,
        buffers_played: 0,
        irq_line: 0,
        vra_supported: false,
        nambar: 0,
        nabmbar: 0,
        mmio_phys: 0,
    };
}

pub fn audio_is_ready() -> bool {
    hda::is_ready() || ac97::is_ready()
}

pub fn audio_driver_label() -> &'static str {
    if hda::is_ready() {
        "Intel HDA"
    } else if ac97::is_ready() {
        "Intel AC'97"
    } else {
        "PC Speaker"
    }
}

pub fn audio_stats() -> UnifiedAudioStats {
    if hda::is_ready() {
        let st = hda::stats();
        UnifiedAudioStats {
            driver: ActiveAudioDriver::IntelHda,
            driver_name: "Intel High Definition Audio (snd_hda)",
            initialized: st.initialized,
            sample_rate: st.sample_rate,
            volume_percent: st.volume_percent,
            playing: st.playing,
            buffers_played: st.buffers_played,
            irq_line: st.irq_line,
            vra_supported: false,
            nambar: 0,
            nabmbar: 0,
            mmio_phys: st.mmio_phys,
        }
    } else if ac97::is_ready() {
        let st = ac97::stats();
        UnifiedAudioStats {
            driver: ActiveAudioDriver::Ac97,
            driver_name: "Intel AC'97 (ICH 8086:2415)",
            initialized: st.initialized,
            sample_rate: st.sample_rate,
            volume_percent: st.volume_percent,
            playing: st.playing,
            buffers_played: st.buffers_played,
            irq_line: st.irq_line,
            vra_supported: st.vra_supported,
            nambar: st.nambar,
            nabmbar: st.nabmbar,
            mmio_phys: 0,
        }
    } else {
        UnifiedAudioStats::EMPTY
    }
}

pub fn audio_begin_playback(rate_hz: u32) -> u32 {
    if hda::is_ready() {
        hda::begin_playback(rate_hz)
    } else if ac97::is_ready() {
        ac97::AC97_DRIVER.lock().begin_playback(rate_hz)
    } else {
        48_000
    }
}

pub fn audio_queue_pcm_s16le(pcm_s16: &[i16]) -> Result<(), &'static str> {
    if hda::is_ready() {
        hda::queue_pcm_s16le(pcm_s16)
    } else if ac97::is_ready() {
        ac97::AC97_DRIVER.lock().queue_pcm_s16le(pcm_s16)
    } else {
        Err("No active hardware audio driver")
    }
}

pub fn audio_finish_playback() {
    if hda::is_ready() {
        hda::finish_playback();
    } else if ac97::is_ready() {
        ac97::AC97_DRIVER.lock().finish_playback();
    }
}

pub fn audio_set_volume(percent: u8) {
    if hda::is_ready() {
        hda::set_volume(percent);
    } else if ac97::is_ready() {
        ac97::AC97_DRIVER.lock().set_volume(percent);
    }
}

pub fn audio_stop() {
    if hda::is_ready() {
        hda::stop();
    } else if ac97::is_ready() {
        ac97::AC97_DRIVER.lock().stop();
    }
}

pub fn audio_try_stop() -> bool {
    if hda::is_ready() {
        hda::try_stop()
    } else if ac97::is_ready() {
        if let Some(mut dev) = ac97::AC97_DRIVER.try_lock() {
            dev.stop();
            true
        } else {
            false
        }
    } else {
        false
    }
}

pub fn audio_try_poll_halted() -> bool {
    if hda::is_ready() {
        hda::try_poll_halted().unwrap_or(false)
    } else if ac97::is_ready() {
        ac97::AC97_DRIVER
            .try_lock()
            .map(|mut d| d.poll_halted())
            .unwrap_or(false)
    } else {
        false
    }
}

pub fn audio_prepare_nonblocking_with_vol(rate_hz: u32) -> (u32, u8) {
    if hda::is_ready() {
        hda::prepare_nonblocking_with_vol(rate_hz)
    } else if ac97::is_ready() {
        let mut dev = ac97::AC97_DRIVER.lock();
        let rate = dev.prepare_nonblocking(rate_hz);
        (rate, dev.stats().volume_percent)
    } else {
        (48_000, 80)
    }
}

pub fn audio_try_prepare_and_free(rate_hz: u32) -> Option<(u32, usize)> {
    if hda::is_ready() {
        hda::try_prepare_nonblocking(rate_hz).map(|(r, free, _)| (r, free))
    } else if ac97::is_ready() {
        ac97::AC97_DRIVER.try_lock().map(|mut dev| {
            let r = dev.prepare_nonblocking(rate_hz);
            let free = dev.free_bdl_slots();
            (r, free)
        })
    } else {
        None
    }
}

pub fn audio_try_queue_one_chunk(pcm_s16: &[i16]) -> Option<Result<bool, &'static str>> {
    if hda::is_ready() {
        Some(hda::try_queue_one_chunk(pcm_s16))
    } else if ac97::is_ready() {
        Some(ac97::AC97_DRIVER.lock().try_queue_one_chunk(pcm_s16))
    } else {
        None
    }
}

// ============================================================
// UNIFIED DRIVER INITIALIZATION
// ============================================================

#[inline(never)]
pub fn init_all(
    console: &mut Console<'_>,
    discovery: &HardwareDiscovery,
    mapping: Option<&PhysicalMemoryMapping>,
    allocator: &mut PhysicalFrameAllocator<'_>,
    fb_virt: u64,
    fb_size: usize,
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

    // 4. UEFI GOP + Linear Framebuffer Display (FreeBSD vt_efifb)
    let gop = uefi_gop::init(fb_virt, fb_size, fb_info, &discovery.display, mapping);
    let mut fb_details = FixedStr::<64>::new();
    let _ = write!(
        fb_details,
        "{}x{} {}bpp {}",
        gop.width,
        gop.height,
        gop.bits_per_pixel,
        gop.format_name()
    );
    let fb_bus = if discovery.display.valid {
        DeviceBus::Pci {
            bus: discovery.display.bus,
            device: discovery.display.device,
            function: discovery.display.function,
            vendor_id: discovery.display.vendor_id,
            device_id: discovery.display.device_id,
        }
    } else {
        DeviceBus::Framebuffer
    };
    registry::register_device(
        "UEFI GOP Framebuffer",
        "FreeBSD efifb / Linear FB",
        DeviceClass::Framebuffer,
        fb_bus,
        if gop.initialized {
            DriverStatus::Active
        } else {
            DriverStatus::Failed
        },
        None,
        0,
        gop.fb_phys,
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

    // 6a. Intel e1000 / e1000e Network Controller
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
                    "e1000/e1000e MMIO/DMA NIC",
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
                    "e1000/e1000e MMIO/DMA NIC",
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
            "e1000/e1000e MMIO/DMA NIC",
            DeviceClass::Network,
            DeviceBus::System,
            DriverStatus::NotPresent,
            None,
            0,
            0,
            "Use QEMU -device e1000",
        );
    }

    // 6b. Realtek Ethernet Controller (FreeBSD if_re / if_rl)
    if discovery.realtek_net.valid {
        let re_bus = DeviceBus::Pci {
            bus: discovery.realtek_net.bus,
            device: discovery.realtek_net.device,
            function: discovery.realtek_net.function,
            vendor_id: discovery.realtek_net.vendor_id,
            device_id: discovery.realtek_net.device_id,
        };
        match re_nic::init(&discovery.realtek_net, mapping, allocator) {
            Ok(_mac) => {
                let st = re_nic::stats();
                if !e1000::is_ready() {
                    crate::net::set_mac(st.mac);
                }
                let mut details = FixedStr::<64>::new();
                let _ = write!(
                    details,
                    "{} {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
                    if st.is_rtl8139 { "RTL8139" } else { "RTL8168" },
                    st.mac[0], st.mac[1], st.mac[2], st.mac[3], st.mac[4], st.mac[5]
                );
                registry::register_device(
                    "Realtek Ethernet",
                    "FreeBSD KPI re/rl NIC",
                    DeviceClass::Network,
                    re_bus,
                    DriverStatus::Active,
                    Some(st.irq_line),
                    st.io_base as u64,
                    st.mmio_phys,
                    details.as_str(),
                );
                console.write("  [ OK ] ", GREEN);
                console.write("Realtek Ethernet (FreeBSD KPI) initialized (", WHITE);
                console.write(details.as_str(), GRAY);
                console.write(")\n", WHITE);
            }
            Err(err) => {
                registry::register_device(
                    "Realtek Ethernet",
                    "FreeBSD KPI re/rl NIC",
                    DeviceClass::Network,
                    re_bus,
                    DriverStatus::Failed,
                    Some(discovery.realtek_net.irq_line),
                    discovery.realtek_net.io_base as u64,
                    discovery.realtek_net.mmio_base,
                    err,
                );
                console.write("  [WARN] Realtek Ethernet: ", YELLOW);
                console.write(err, WHITE);
                console.write("\n", WHITE);
            }
        }
    } else {
        registry::register_device(
            "Realtek Ethernet",
            "FreeBSD KPI re/rl NIC",
            DeviceClass::Network,
            DeviceBus::System,
            DriverStatus::NotPresent,
            None,
            0,
            0,
            "Use -device rtl8139",
        );
    }

    // 7a. Intel AC'97 Audio Controller
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

    // 7b. Intel High Definition Audio (FreeBSD snd_hda)
    if discovery.hda.valid {
        let hda_bus = DeviceBus::Pci {
            bus: discovery.hda.bus,
            device: discovery.hda.device,
            function: discovery.hda.function,
            vendor_id: discovery.hda.vendor_id,
            device_id: discovery.hda.device_id,
        };
        match hda::init(&discovery.hda, mapping, allocator) {
            Ok(st) => {
                let mut details = FixedStr::<64>::new();
                let _ = write!(
                    details,
                    "v{}.{} codec={} pin={} 48kHz",
                    st.version_major, st.version_minor, st.codec_addr, st.pin_nid
                );
                registry::register_device(
                    "Intel HDA",
                    "FreeBSD KPI snd_hda CORB/RIRB",
                    DeviceClass::Audio,
                    hda_bus,
                    DriverStatus::Active,
                    Some(st.irq_line),
                    0,
                    st.mmio_phys,
                    details.as_str(),
                );
                console.write("  [ OK ] ", GREEN);
                console.write("Intel HDA (FreeBSD KPI) initialized (", WHITE);
                console.write(details.as_str(), GRAY);
                console.write(")\n", WHITE);
            }
            Err(err) => {
                registry::register_device(
                    "Intel HDA",
                    "FreeBSD KPI snd_hda CORB/RIRB",
                    DeviceClass::Audio,
                    hda_bus,
                    DriverStatus::Failed,
                    Some(discovery.hda.irq_line),
                    0,
                    discovery.hda.mmio_base,
                    err,
                );
                console.write("  [WARN] Intel HDA: ", YELLOW);
                console.write(err, WHITE);
                console.write("\n", WHITE);
            }
        }
    } else {
        registry::register_device(
            "Intel HDA",
            "FreeBSD KPI snd_hda CORB/RIRB",
            DeviceClass::Audio,
            DeviceBus::System,
            DriverStatus::NotPresent,
            None,
            0,
            0,
            "Use -device intel-hda -device hda-duplex",
        );
    }

    // 8. ACPI Platform, AML Power/Battery/Thermal & APIC/SMP Topology
    let acpi_info = crate::arch::acpi::snapshot();
    let apic_snap = crate::arch::apic::snapshot();
    let pm_snap = acpi_pm::init(mapping);

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

    let mut pm_details = FixedStr::<64>::new();
    let _ = write!(
        pm_details,
        "AML={}B bat={} tz={} ec={}",
        pm_snap.dsdt_bytes,
        pm_snap.battery_count,
        pm_snap.thermal_zone_count,
        if pm_snap.ec_present { "Y" } else { "N" }
    );
    registry::register_device(
        "ACPI Power & Thermal",
        "FreeBSD acpi_pm/bat/tz/coretemp",
        DeviceClass::Power,
        DeviceBus::System,
        if pm_snap.initialized {
            DriverStatus::Active
        } else {
            DriverStatus::NotPresent
        },
        None,
        pm_snap.pm1a_cnt as u64,
        0,
        pm_details.as_str(),
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

    // 11. NVMe Controller (FreeBSD Driver KPI)
    if discovery.nvme.valid {
        let nvme_bus = DeviceBus::Pci {
            bus: discovery.nvme.bus,
            device: discovery.nvme.device,
            function: discovery.nvme.function,
            vendor_id: discovery.nvme.vendor_id,
            device_id: discovery.nvme.device_id,
        };
        match nvme::init(&discovery.nvme, mapping, allocator) {
            Ok(snap) => {
                let mut details = FixedStr::<64>::new();
                let _ = write!(
                    details,
                    "{} {}B sect={}",
                    snap.model_str(),
                    snap.lba_size,
                    snap.sector_count_512
                );
                registry::register_device(
                    "NVMe",
                    "FreeBSD KPI NVMe DMA",
                    DeviceClass::Storage,
                    nvme_bus,
                    DriverStatus::Active,
                    Some(discovery.nvme.irq_line),
                    0,
                    discovery.nvme.bar0_phys,
                    details.as_str(),
                );
                console.write("  [ OK ] ", GREEN);
                console.write("NVMe (FreeBSD KPI) initialized (", WHITE);
                console.write(details.as_str(), GRAY);
                console.write(")\n", WHITE);
            }
            Err(err) => {
                registry::register_device(
                    "NVMe",
                    "FreeBSD KPI NVMe DMA",
                    DeviceClass::Storage,
                    nvme_bus,
                    DriverStatus::Failed,
                    Some(discovery.nvme.irq_line),
                    0,
                    discovery.nvme.bar0_phys,
                    err,
                );
                console.write("  [WARN] NVMe (FreeBSD KPI): ", YELLOW);
                console.write(err, WHITE);
                console.write("\n", WHITE);
            }
        }
    } else {
        registry::register_device(
            "NVMe",
            "FreeBSD KPI NVMe DMA",
            DeviceClass::Storage,
            DeviceBus::System,
            DriverStatus::NotPresent,
            None,
            0,
            0,
            "Use QEMU -device nvme,serial=amaryllis",
        );
    }

    // 12. USB xHCI Host Controller + HID + Mass Storage (FreeBSD xhci)
    if discovery.xhci.valid {
        let xhci_bus = DeviceBus::Pci {
            bus: discovery.xhci.bus,
            device: discovery.xhci.device,
            function: discovery.xhci.function,
            vendor_id: discovery.xhci.vendor_id,
            device_id: discovery.xhci.device_id,
        };
        match usb::init(&discovery.xhci, mapping, allocator) {
            Ok(snap) => {
                let mut details = FixedStr::<64>::new();
                let _ = write!(
                    details,
                    "v0x{:x} ports={} devs={} kbd={} mse={}",
                    snap.hci_version,
                    snap.max_ports,
                    snap.enumerated_devices,
                    snap.hid_keyboard_slot,
                    snap.hid_mouse_slot
                );
                registry::register_device(
                    "USB xHCI",
                    "FreeBSD KPI xHCI + HID",
                    DeviceClass::Usb,
                    xhci_bus,
                    DriverStatus::Active,
                    Some(snap.irq_line),
                    0,
                    snap.mmio_phys,
                    details.as_str(),
                );
                console.write("  [ OK ] ", GREEN);
                console.write("USB xHCI (FreeBSD KPI) initialized (", WHITE);
                console.write(details.as_str(), GRAY);
                console.write(")\n", WHITE);

                if usb::has_mass_storage() {
                    let um = usb::umass_snapshot();
                    let mut um_det = FixedStr::<64>::new();
                    let _ = write!(
                        um_det,
                        "{} {} sect={}",
                        um.vendor_str(),
                        um.product_str(),
                        um.sector_count
                    );
                    registry::register_device(
                        "USB Mass Storage",
                        "USB BOT SCSI BlockDevice",
                        DeviceClass::Storage,
                        xhci_bus,
                        DriverStatus::Active,
                        None,
                        0,
                        0,
                        um_det.as_str(),
                    );
                }
            }
            Err(err) => {
                registry::register_device(
                    "USB xHCI",
                    "FreeBSD KPI xHCI + HID",
                    DeviceClass::Usb,
                    xhci_bus,
                    DriverStatus::Failed,
                    Some(discovery.xhci.irq_line),
                    0,
                    discovery.xhci.mmio_base,
                    err,
                );
                console.write("  [WARN] USB xHCI: ", YELLOW);
                console.write(err, WHITE);
                console.write("\n", WHITE);
            }
        }
    } else {
        registry::register_device(
            "USB xHCI",
            "FreeBSD KPI xHCI + HID",
            DeviceClass::Usb,
            DeviceBus::System,
            DriverStatus::NotPresent,
            None,
            0,
            0,
            "Use -device qemu-xhci -device usb-kbd -device usb-mouse",
        );
    }

    // 13. SDHCI / SD Card Reader (FreeBSD sdhci_pci / mmc / mmcsd)
    if discovery.sdhci.valid {
        let sd_bus = DeviceBus::Pci {
            bus: discovery.sdhci.bus,
            device: discovery.sdhci.device,
            function: discovery.sdhci.function,
            vendor_id: discovery.sdhci.vendor_id,
            device_id: discovery.sdhci.device_id,
        };
        match sdhci::init(&discovery.sdhci, mapping, allocator) {
            Ok(snap) => {
                let mut details = FixedStr::<64>::new();
                if snap.card_ready {
                    let _ = write!(details, "SD{} sect={}", if snap.sdhc_v2 { "HC" } else { "SC" }, snap.sector_count);
                } else if snap.card_present {
                    let _ = write!(details, "v0x{:x} card uninit", snap.host_version);
                } else {
                    let _ = write!(details, "v0x{:x} empty slot", snap.host_version);
                }
                registry::register_device(
                    "SDHCI Card Reader",
                    "FreeBSD KPI sdhci/mmcsd",
                    DeviceClass::CardReader,
                    sd_bus,
                    DriverStatus::Active,
                    Some(snap.irq_line),
                    0,
                    snap.bar0_phys,
                    details.as_str(),
                );
                console.write("  [ OK ] ", GREEN);
                console.write("SDHCI Card Reader (FreeBSD KPI) initialized (", WHITE);
                console.write(details.as_str(), GRAY);
                console.write(")\n", WHITE);
            }
            Err(err) => {
                registry::register_device(
                    "SDHCI Card Reader",
                    "FreeBSD KPI sdhci/mmcsd",
                    DeviceClass::CardReader,
                    sd_bus,
                    DriverStatus::Failed,
                    Some(discovery.sdhci.irq_line),
                    0,
                    discovery.sdhci.bar0_phys,
                    err,
                );
                console.write("  [WARN] SDHCI Card Reader: ", YELLOW);
                console.write(err, WHITE);
                console.write("\n", WHITE);
            }
        }
    } else {
        registry::register_device(
            "SDHCI Card Reader",
            "FreeBSD KPI sdhci/mmcsd",
            DeviceClass::CardReader,
            DeviceBus::System,
            DriverStatus::NotPresent,
            None,
            0,
            0,
            "Use -device sdhci-pci -device sd-card",
        );
    }

    crate::freebsd_kpi::taskqueue::ensure_taskqueue_worker();
    let kpi_report =
        crate::freebsd_kpi::validate_against_legacy_drivers(discovery, mapping, allocator);
    let kpi_ok = kpi_report.entries.iter().all(|e| e.passed);
    console.write(
        if kpi_ok { "  [ OK ] " } else { "  [WARN] " },
        if kpi_ok { GREEN } else { YELLOW },
    );
    console.write("FreeBSD Driver KPI cross-check (", WHITE);
    let mut kpi_buf = FixedStr::<16>::new();
    let _ = write!(kpi_buf, "{} checks", kpi_report.entries.len());
    console.write(kpi_buf.as_str(), GRAY);
    console.write(")\n", WHITE);

    iokit::init_io_services(discovery);
    let io_st = iokit::stats();

    console.write("  [ OK ] ", GREEN);
    console.write("Unified driver registry & IORegistry ready (IOAPIC vecs=", WHITE);
    let mut vec_buf = FixedStr::<16>::new();
    let _ = write!(vec_buf, "{}", io_st.routed_vectors);
    console.write(vec_buf.as_str(), GREEN);
    console.write(")\n", WHITE);
}
