use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, AtomicUsize, Ordering};

use crate::arch::acpi::{self, AcpiPlatformInfo};
use crate::arch::apic::{self, IOAPIC_IRQ_VECTOR_BASE, IOAPIC_IRQ_VECTOR_COUNT};
use crate::drivers::registry::{self, DeviceBus, DriverStatus};
use crate::hardware::HardwareDiscovery;
use crate::klog::{self, KLogLevel};
use crate::task::kthread;
use crate::task::preempt::PreemptMutex;

pub const MAX_IOREG_NODES: usize = 32;
pub const MAX_HANDLERS_PER_VECTOR: usize = 4;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum IOServiceClass {
    IOPlatformExpert,
    IOACPIPlatformDevice,
    IOAPICController,
    IOPCIDevice,
    IOEthernetController,
    IOBlockStorageDevice,
    IOAudioDevice,
    IOHIDDevice,
    IOGraphicsDevice,
    IOInterruptEventSource,
    IOWorkLoop,
}

impl IOServiceClass {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::IOPlatformExpert => "IOPlatformExpert",
            Self::IOACPIPlatformDevice => "IOACPIPlatformDevice",
            Self::IOAPICController => "IOAPICController",
            Self::IOPCIDevice => "IOPCIDevice",
            Self::IOEthernetController => "IOEthernetController",
            Self::IOBlockStorageDevice => "IOBlockStorageDevice",
            Self::IOAudioDevice => "IOAudioDevice",
            Self::IOHIDDevice => "IOHIDDevice",
            Self::IOGraphicsDevice => "IOGraphicsDevice",
            Self::IOInterruptEventSource => "IOInterruptEventSource",
            Self::IOWorkLoop => "IOWorkLoop",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum WorkLoopKind {
    Network = 0,
    Audio = 1,
    Storage = 2,
}

#[derive(Clone)]
pub struct IORegEntryView {
    pub id: usize,
    pub depth: u8,
    pub name: &'static str,
    pub class_name: &'static str,
    pub status: &'static str,
    pub bus_info: String,
    pub gsi: Option<u32>,
    pub vector: Option<u8>,
    pub irq_count: u64,
    pub workloop: &'static str,
}

#[derive(Clone, Copy)]
struct IORegNode {
    valid: bool,
    depth: u8,
    name: &'static str,
    class: IOServiceClass,
    status: &'static str,
    bus_info: [u8; 48],
    bus_info_len: u8,
    gsi: Option<u32>,
    vector: Option<u8>,
    irq_vec_slot: Option<usize>,
    workloop: &'static str,
}

impl IORegNode {
    const EMPTY: Self = Self {
        valid: false,
        depth: 0,
        name: "",
        class: IOServiceClass::IOPlatformExpert,
        status: "Unknown",
        bus_info: [0; 48],
        bus_info_len: 0,
        gsi: None,
        vector: None,
        irq_vec_slot: None,
        workloop: "",
    };

    fn set_bus_info(&mut self, s: &str) {
        let bytes = s.as_bytes();
        let len = bytes.len().min(self.bus_info.len());
        self.bus_info[..len].copy_from_slice(&bytes[..len]);
        self.bus_info_len = len as u8;
    }

    fn bus_info_str(&self) -> &str {
        core::str::from_utf8(&self.bus_info[..self.bus_info_len as usize]).unwrap_or("")
    }
}

struct IORegistryState {
    nodes: [IORegNode; MAX_IOREG_NODES],
    count: usize,
}

impl IORegistryState {
    const fn new() -> Self {
        Self {
            nodes: [IORegNode::EMPTY; MAX_IOREG_NODES],
            count: 0,
        }
    }

    fn clear(&mut self) {
        for n in &mut self.nodes {
            *n = IORegNode::EMPTY;
        }
        self.count = 0;
    }

    fn push(
        &mut self,
        depth: u8,
        name: &'static str,
        class: IOServiceClass,
        status: &'static str,
        bus_info: &str,
        gsi: Option<u32>,
        vector: Option<u8>,
        workloop: &'static str,
    ) {
        if self.count >= MAX_IOREG_NODES {
            return;
        }
        let mut node = IORegNode::EMPTY;
        node.valid = true;
        node.depth = depth;
        node.name = name;
        node.class = class;
        node.status = status;
        node.set_bus_info(bus_info);
        node.gsi = gsi;
        node.vector = vector;
        node.irq_vec_slot = vector.and_then(|v| {
            if v >= IOAPIC_IRQ_VECTOR_BASE
                && (v as usize) < (IOAPIC_IRQ_VECTOR_BASE as usize + IOAPIC_IRQ_VECTOR_COUNT)
            {
                Some((v - IOAPIC_IRQ_VECTOR_BASE) as usize)
            } else {
                None
            }
        });
        node.workloop = workloop;
        self.nodes[self.count] = node;
        self.count += 1;
    }
}

struct IrqVectorSlot {
    active: AtomicBool,
    gsi: AtomicU32,
    vector: AtomicU8,
    irq_count: AtomicU64,
    unacked_streak: AtomicU32,
    polarity_flipped: AtomicBool,
    masked_by_guard: AtomicBool,
    handler_ptrs: [AtomicUsize; MAX_HANDLERS_PER_VECTOR],
}

impl IrqVectorSlot {
    const fn new() -> Self {
        Self {
            active: AtomicBool::new(false),
            gsi: AtomicU32::new(0),
            vector: AtomicU8::new(0),
            irq_count: AtomicU64::new(0),
            unacked_streak: AtomicU32::new(0),
            polarity_flipped: AtomicBool::new(false),
            masked_by_guard: AtomicBool::new(false),
            handler_ptrs: [const { AtomicUsize::new(0) }; MAX_HANDLERS_PER_VECTOR],
        }
    }
}

static IOREGISTRY: PreemptMutex<IORegistryState> = PreemptMutex::new(IORegistryState::new());
static IRQ_SLOTS: [IrqVectorSlot; IOAPIC_IRQ_VECTOR_COUNT] =
    [const { IrqVectorSlot::new() }; IOAPIC_IRQ_VECTOR_COUNT];
static TOTAL_IOAPIC_IRQS: AtomicU64 = AtomicU64::new(0);
static WORKLOOPS_STARTED: AtomicBool = AtomicBool::new(false);
static WORKLOOP_AUDIO_SIGNALS: AtomicU64 = AtomicU64::new(0);
static WORKLOOP_BLK_SIGNALS: AtomicU64 = AtomicU64::new(0);
static WORKLOOP_NET_SIGNALS: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Default)]
pub struct IOKitStats {
    pub total_ioapic_irqs: u64,
    pub routed_vectors: usize,
    pub workloops_active: bool,
    pub net_signals: u64,
    pub audio_signals: u64,
    pub blk_signals: u64,
}

/// Attaches a lock-free top-half hardware IRQ handler for `irq_line` onto an IOAPIC vector
/// (`0x50..0x57`), sharing vectors transparently if multiple PCI devices share the same GSI.
/// Returns `Some((gsi, vector))` on success.
pub fn attach_pci_interrupt(
    acpi: &AcpiPlatformInfo,
    irq_line: u8,
    top_half: fn() -> bool,
) -> Option<(u32, u8)> {
    if irq_line == 0 || irq_line == 0xFF {
        return None;
    }
    let apic_snap = apic::snapshot();
    if !apic_snap.lapic_enabled || apic_snap.ioapic_count == 0 {
        return None;
    }

    let (gsi, level_triggered, active_low) = apic::resolve_pci_or_isa_gsi(acpi, irq_line);
    let fn_addr = top_half as *const () as usize;

    // Reuse an already allocated vector slot if this GSI is shared across PCI devices.
    for (slot_idx, slot) in IRQ_SLOTS.iter().enumerate() {
        if slot.active.load(Ordering::Acquire) && slot.gsi.load(Ordering::Acquire) == gsi {
            for h in &slot.handler_ptrs {
                let cur = h.load(Ordering::Acquire);
                if cur == fn_addr {
                    return Some((gsi, IOAPIC_IRQ_VECTOR_BASE + slot_idx as u8));
                }
                if cur == 0
                    && h.compare_exchange(0, fn_addr, Ordering::AcqRel, Ordering::Acquire)
                        .is_ok()
                {
                    return Some((gsi, IOAPIC_IRQ_VECTOR_BASE + slot_idx as u8));
                }
            }
            return Some((gsi, IOAPIC_IRQ_VECTOR_BASE + slot_idx as u8));
        }
    }

    // Allocate a new IOAPIC vector slot in 0x50..0x57.
    for (slot_idx, slot) in IRQ_SLOTS.iter().enumerate() {
        if slot
            .active
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            let vector = IOAPIC_IRQ_VECTOR_BASE + slot_idx as u8;
            slot.gsi.store(gsi, Ordering::Release);
            slot.vector.store(vector, Ordering::Release);
            slot.unacked_streak.store(0, Ordering::Release);
            slot.polarity_flipped.store(false, Ordering::Release);
            slot.masked_by_guard.store(false, Ordering::Release);
            slot.handler_ptrs[0].store(fn_addr, Ordering::Release);

            if apic::route_ioapic_irq(gsi, vector, apic_snap.bsp_apic_id, level_triggered, active_low) {
                return Some((gsi, vector));
            } else {
                slot.handler_ptrs[0].store(0, Ordering::Release);
                slot.active.store(false, Ordering::Release);
                return None;
            }
        }
    }

    None
}

/// Lock-free Top-Half hardware IRQ dispatcher invoked from IDT `0x50..0x57`.
#[inline]
pub fn handle_hardware_irq(vector: u8) {
    if vector < IOAPIC_IRQ_VECTOR_BASE {
        return;
    }
    let idx = (vector - IOAPIC_IRQ_VECTOR_BASE) as usize;
    if idx >= IOAPIC_IRQ_VECTOR_COUNT {
        return;
    }

    TOTAL_IOAPIC_IRQS.fetch_add(1, Ordering::Relaxed);
    let slot = &IRQ_SLOTS[idx];
    slot.irq_count.fetch_add(1, Ordering::Relaxed);

    let mut any_acked = false;
    for h in &slot.handler_ptrs {
        let addr = h.load(Ordering::Acquire);
        if addr != 0 {
            let func: fn() -> bool = unsafe { core::mem::transmute(addr) };
            if func() {
                any_acked = true;
            }
        }
    }

    if any_acked {
        slot.unacked_streak.store(0, Ordering::Relaxed);
    } else {
        let streak = slot.unacked_streak.fetch_add(1, Ordering::Relaxed) + 1;
        let gsi = slot.gsi.load(Ordering::Relaxed);
        if streak == 16 && !slot.polarity_flipped.swap(true, Ordering::AcqRel) {
            if let Some(new_low) = apic::flip_ioapic_polarity_irqsafe(gsi) {
                klog::log_fmt(
                    KLogLevel::Warn,
                    "ioapic",
                    format_args!(
                        "Auto-flipped polarity on GSI {} (vec 0x{:02x}) -> active_{}",
                        gsi,
                        vector,
                        if new_low { "low" } else { "high" }
                    ),
                );
            }
        } else if streak >= 64 && !slot.masked_by_guard.swap(true, Ordering::AcqRel) {
            if apic::mask_ioapic_irq_irqsafe(gsi) {
                klog::log_fmt(
                    KLogLevel::Warn,
                    "ioapic",
                    format_args!(
                        "Masked runaway unacked GSI {} (vec 0x{:02x}) to protect IRQ12/CPU",
                        gsi, vector
                    ),
                );
            }
        }
    }
}

/// Lock-free IRQ-safe signal waking the corresponding IOKit `IOWorkLoop` kernel thread.
#[inline]
pub fn signal_workloop(kind: WorkLoopKind) {
    match kind {
        WorkLoopKind::Network => {
            WORKLOOP_NET_SIGNALS.fetch_add(1, Ordering::Relaxed);
            kthread::signal_worker_wake(kthread::WAKE_BIT_NET);
        }
        WorkLoopKind::Audio => {
            WORKLOOP_AUDIO_SIGNALS.fetch_add(1, Ordering::Relaxed);
            kthread::signal_worker_wake(kthread::WAKE_BIT_AUDIO);
        }
        WorkLoopKind::Storage => {
            WORKLOOP_BLK_SIGNALS.fetch_add(1, Ordering::Relaxed);
            kthread::signal_worker_wake(kthread::WAKE_BIT_BLK);
        }
    }
}

fn format_pci_bus(bus: &DeviceBus) -> String {
    match *bus {
        DeviceBus::Pci {
            bus,
            device,
            function,
            vendor_id,
            device_id,
        } => format!(
            "PCI {:02}:{:02}.{} ({:04X}:{:04X})",
            bus, device, function, vendor_id, device_id
        ),
        DeviceBus::Ps2 => String::from("ISA / i8042"),
        DeviceBus::Framebuffer => String::from("LinearFB"),
        DeviceBus::System => String::from("SystemBus"),
    }
}

/// Populates the XNU-inspired `IORegistry` tree and binds IOAPIC IRQ routing for active PCI drivers.
pub fn init_io_services(discovery: &HardwareDiscovery) {
    let acpi_info = acpi::snapshot();
    let apic_snap = apic::snapshot();

    let net_route = if discovery.network.valid && crate::drivers::e1000::is_ready() {
        attach_pci_interrupt(
            &acpi_info,
            discovery.network.irq_line,
            crate::drivers::e1000::irq_top_half,
        )
    } else {
        None
    };

    let audio_route = if discovery.audio.valid && crate::drivers::ac97::is_ready() {
        attach_pci_interrupt(
            &acpi_info,
            discovery.audio.irq_line,
            crate::drivers::ac97::irq_top_half,
        )
    } else {
        None
    };

    let virtio_route = if discovery.virtio_blk.valid
        && crate::drivers::virtio_blk::snapshot().initialized
    {
        attach_pci_interrupt(
            &acpi_info,
            discovery.virtio_blk.irq_line,
            crate::drivers::virtio_blk::irq_top_half,
        )
    } else {
        None
    };

    let ahci_route = if discovery.ahci.valid && crate::drivers::ahci::snapshot().initialized {
        attach_pci_interrupt(
            &acpi_info,
            discovery.ahci.irq_line,
            crate::drivers::ahci::irq_top_half,
        )
    } else {
        None
    };

    let nvme_snap = crate::drivers::nvme::snapshot();
    let nvme_route = if nvme_snap.initialized {
        nvme_snap.irq_gsi.zip(nvme_snap.irq_vector)
    } else {
        None
    };

    let mut reg = IOREGISTRY.lock();
    reg.clear();

    reg.push(
        0,
        "AmaryllisPlatform",
        IOServiceClass::IOPlatformExpert,
        "Online",
        apic_snap.vendor_str(),
        None,
        None,
        "",
    );

    reg.push(
        1,
        "IOACPIPlatformExpert",
        IOServiceClass::IOACPIPlatformDevice,
        if acpi_info.discovered { "Active" } else { "Absent" },
        &format!(
            "{} cpus={} ioapic={}",
            if acpi_info.using_xsdt { "XSDT" } else { "RSDT" },
            acpi_info.cpu_count,
            acpi_info.ioapic_count
        ),
        None,
        None,
        "",
    );

    for i in 0..apic_snap.ioapic_count {
        let io = apic_snap.ioapics[i];
        reg.push(
            2,
            "IOAPICController",
            IOServiceClass::IOAPICController,
            if io.mapped { "Active" } else { "Unmapped" },
            &format!("id={} gsi={}..{}", io.id, io.gsi_base, io.gsi_base + io.max_redirs as u32),
            Some(io.gsi_base),
            None,
            "",
        );
    }

    reg.push(
        1,
        "IOPCIHostBridge",
        IOServiceClass::IOPCIDevice,
        "Active",
        &format!("ECAM=0x{:x}", acpi_info.mcfg_ecam_base),
        None,
        None,
        "",
    );

    let total_devs = registry::count();
    for i in 0..total_devs {
        let Some(dev) = registry::get(i) else {
            continue;
        };
        if dev.status == DriverStatus::NotPresent {
            continue;
        }
        let class = match dev.class {
            registry::DeviceClass::Network => IOServiceClass::IOEthernetController,
            registry::DeviceClass::Storage => IOServiceClass::IOBlockStorageDevice,
            registry::DeviceClass::Audio => IOServiceClass::IOAudioDevice,
            registry::DeviceClass::Keyboard | registry::DeviceClass::Mouse => {
                IOServiceClass::IOHIDDevice
            }
            registry::DeviceClass::Framebuffer => IOServiceClass::IOGraphicsDevice,
            registry::DeviceClass::Timer | registry::DeviceClass::Controller => continue,
        };

        let (route, workloop) = match dev.name {
            "Intel e1000" => (net_route, "knet_worker"),
            "Intel AC'97" => (audio_route, "kiokit_audio"),
            "VirtIO-Blk" => (virtio_route, "kiokit_blk"),
            "AHCI SATA" => (ahci_route, "kiokit_blk"),
            "NVMe" => (nvme_route, "kiokit_blk"),
            _ => (None, ""),
        };

        reg.push(
            2,
            dev.name,
            class,
            dev.status.name(),
            &format_pci_bus(&dev.bus),
            route.map(|r| r.0),
            route.map(|r| r.1),
            workloop,
        );

        if let Some((gsi, vec)) = route {
            reg.push(
                3,
                "IOInterruptEventSource",
                IOServiceClass::IOInterruptEventSource,
                "Routed",
                &format!("IOAPIC GSI {} -> IDT 0x{:02x}", gsi, vec),
                Some(gsi),
                Some(vec),
                workloop,
            );
        }

        if !workloop.is_empty() {
            reg.push(
                3,
                workloop,
                IOServiceClass::IOWorkLoop,
                "Armed",
                "Preemptive KThread WorkLoop",
                route.map(|r| r.0),
                route.map(|r| r.1),
                workloop,
            );
        }
    }

    klog::log_fmt(
        KLogLevel::Info,
        "iokit",
        format_args!(
            "IORegistry built ({} nodes); IOAPIC IRQ routes net={:?} audio={:?} blk={:?}",
            reg.count, net_route, audio_route, virtio_route.or(ahci_route).or(nvme_route)
        ),
    );
}

fn kiokit_audio_workloop_main() {
    loop {
        let _active = crate::audio::mixer::tick();
        kthread::sleep_current_ms(120);
    }
}

fn kiokit_blk_workloop_main() {
    loop {
        let dirty = crate::storage::cache::stats_snapshot().dirty_pages;
        if dirty >= 16 {
            let _ = crate::storage::vfs::sync();
        }
        kthread::sleep_current_ms(2000);
    }
}

/// Spawns IOKit `IOWorkLoop` kernel worker threads (`kiokit_audio`, `kiokit_blk`, alongside `knet_worker`).
pub fn start_workloops() {
    if WORKLOOPS_STARTED.swap(true, Ordering::AcqRel) {
        return;
    }
    kthread::ensure_system_workers();
    let _ = kthread::spawn_kthread("kiokit_audio", kiokit_audio_workloop_main);
    let _ = kthread::spawn_kthread("kiokit_blk", kiokit_blk_workloop_main);
    klog::log_info("iokit", "IOKit IOWorkLoop kthreads active (net/audio/blk)");
}

pub fn snapshot_ioregistry() -> Vec<IORegEntryView> {
    let reg = IOREGISTRY.lock();
    let mut out = Vec::with_capacity(reg.count);
    for idx in 0..reg.count {
        let n = &reg.nodes[idx];
        if !n.valid {
            continue;
        }
        let irq_count = n
            .irq_vec_slot
            .map(|s| IRQ_SLOTS[s].irq_count.load(Ordering::Relaxed))
            .unwrap_or(0);
        out.push(IORegEntryView {
            id: idx,
            depth: n.depth,
            name: n.name,
            class_name: n.class.as_str(),
            status: n.status,
            bus_info: String::from(n.bus_info_str()),
            gsi: n.gsi,
            vector: n.vector,
            irq_count,
            workloop: n.workloop,
        });
    }
    out
}

pub fn stats() -> IOKitStats {
    let routed_vectors = IRQ_SLOTS
        .iter()
        .filter(|s| s.active.load(Ordering::Acquire))
        .count();
    IOKitStats {
        total_ioapic_irqs: TOTAL_IOAPIC_IRQS.load(Ordering::Relaxed),
        routed_vectors,
        workloops_active: WORKLOOPS_STARTED.load(Ordering::Acquire),
        net_signals: WORKLOOP_NET_SIGNALS.load(Ordering::Relaxed),
        audio_signals: WORKLOOP_AUDIO_SIGNALS.load(Ordering::Relaxed),
        blk_signals: WORKLOOP_BLK_SIGNALS.load(Ordering::Relaxed),
    }
}
