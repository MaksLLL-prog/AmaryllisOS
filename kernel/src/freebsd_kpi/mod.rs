pub mod bus_dma;
pub mod bus_space;
pub mod newbus;
pub mod pci_res;
pub mod sync;
pub mod taskqueue;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use crate::hardware::HardwareDiscovery;
use crate::memory::vmm::PhysicalMemoryMapping;
use crate::memory::PhysicalFrameAllocator;
use crate::pci;
use crate::task::preempt::PreemptMutex;

pub use bus_dma::{
    bus_dma_tag_create, bus_dmamap_load, bus_dmamap_sync, bus_dmamem_alloc, bus_dmamem_free,
    BusDmaMap, BusDmaSegment, BusDmaTag, BUS_DMASYNC_POSTREAD, BUS_DMASYNC_POSTWRITE,
    BUS_DMASYNC_PREREAD, BUS_DMASYNC_PREWRITE, BUS_DMA_NOWAIT, BUS_DMA_WAITOK, BUS_DMA_ZERO,
};
pub use bus_space::{
    bus_space_barrier, bus_space_read_1, bus_space_read_2, bus_space_read_4, bus_space_read_8,
    bus_space_write_1, bus_space_write_2, bus_space_write_4, bus_space_write_8, map_mmio,
    BusSpaceHandle, BusSpaceTag,
};
pub use newbus::{
    device_add_pci_child, device_detach, device_probe_and_attach, snapshot_devices, with_device,
    with_device_mut, DeviceId, DeviceT, DriverT, NewbusState, BUS_PROBE_DEFAULT,
    BUS_PROBE_SPECIFIC, ENXIO,
};
pub use pci_res::{
    bus_alloc_resource_any, bus_setup_intr, pci_enable_busmaster, pci_read_config,
    pci_write_config, Resource, SYS_RES_IOPORT, SYS_RES_IRQ, SYS_RES_MEMORY,
};
pub use sync::{Mtx, MtxSpin, SxLock};
pub use taskqueue::{ensure_taskqueue_worker, Callout, Task, TaskQueue, KPI_TASKQUEUE};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum KpiError {
    NoMem,
    NoDev,
    NoEnt,
    InvalidArg,
    TimedOut,
    Io,
}

impl KpiError {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NoMem => "ENOMEM",
            Self::NoDev => "ENXIO",
            Self::NoEnt => "ENOENT",
            Self::InvalidArg => "EINVAL",
            Self::TimedOut => "ETIMEDOUT",
            Self::Io => "EIO",
        }
    }
}

#[derive(Clone)]
pub struct KpiValidationEntry {
    pub target: &'static str,
    pub passed: bool,
    pub summary: String,
}

#[derive(Clone, Default)]
pub struct KpiValidationReport {
    pub entries: Vec<KpiValidationEntry>,
}

static KPI_VALIDATION: PreemptMutex<KpiValidationReport> =
    PreemptMutex::new(KpiValidationReport { entries: Vec::new() });

/// Non-destructively cross-checks `freebsd_kpi` PCI config & MMIO `bus_space` against
/// AmaryllisOS's reference native `e1000` and `ahci` drivers (without altering device state).
pub fn validate_against_legacy_drivers(
    discovery: &HardwareDiscovery,
    mapping: Option<&PhysicalMemoryMapping>,
    allocator: &mut PhysicalFrameAllocator<'_>,
) -> KpiValidationReport {
    let mut report = KpiValidationReport::default();

    // 1. Validate against Intel e1000 NIC if discovered & active
    if discovery.network.valid && crate::drivers::e1000::is_ready() {
        let mut dev = DeviceT::EMPTY;
        dev.bus = discovery.network.bus;
        dev.device = discovery.network.device;
        dev.function = discovery.network.function;
        dev.vendor_id = discovery.network.vendor_id;
        dev.device_id = discovery.network.device_id;

        let id_dword = pci_read_config(&dev, 0x00, 4);
        let pci_vid = (id_dword & 0xFFFF) as u16;
        let pci_did = ((id_dword >> 16) & 0xFFFF) as u16;
        let nic_stats = crate::drivers::e1000::stats();

        let mmio_ok = if let Ok(res) =
            bus_alloc_resource_any(&dev, SYS_RES_MEMORY, 0, mapping, allocator)
        {
            if let Some(ref handle) = res.bus_handle {
                let kpi_status = bus_space_read_4(handle, 0x0008); // e1000 STATUS reg
                let kpi_link_up = (kpi_status & (1 << 1)) != 0;
                res.phys_start == nic_stats.mmio_phys && kpi_link_up == nic_stats.link_up
            } else {
                false
            }
        } else {
            false
        };

        report.entries.push(KpiValidationEntry {
            target: "e1000 (KPI cross-check)",
            passed: pci_vid == discovery.network.vendor_id
                && pci_did == discovery.network.device_id
                && mmio_ok,
            summary: format!(
                "PCI {:04x}:{:04x} BAR0=0x{:x} link_match={}",
                pci_vid, pci_did, nic_stats.mmio_phys, mmio_ok
            ),
        });
    }

    // 2. Validate against AHCI SATA HBA if discovered & active
    if discovery.ahci.valid {
        let ahci_snap = crate::drivers::ahci::snapshot();
        if ahci_snap.initialized {
            let mut dev = DeviceT::EMPTY;
            dev.bus = discovery.ahci.bus;
            dev.device = discovery.ahci.device;
            dev.function = discovery.ahci.function;
            dev.vendor_id = discovery.ahci.vendor_id;
            dev.device_id = discovery.ahci.device_id;

            let class_rev = pci_read_config(&dev, 0x08, 4);
            let class_ok = ((class_rev >> 24) & 0xFF) == 0x01 && ((class_rev >> 16) & 0xFF) == 0x06;
            let bar5_res = bus_alloc_resource_any(&dev, SYS_RES_MEMORY, 5, mapping, allocator)
                .or_else(|_| bus_alloc_resource_any(&dev, SYS_RES_MEMORY, 0, mapping, allocator));

            let mut mmio_ok = false;
            let mut kpi_vs = 0u32;
            if let Ok(res) = bar5_res {
                if let Some(ref handle) = res.bus_handle {
                    let cap = bus_space_read_4(handle, 0x00);
                    kpi_vs = bus_space_read_4(handle, 0x10);
                    mmio_ok = res.phys_start == ahci_snap.abar_phys
                        && cap == ahci_snap.cap
                        && kpi_vs == ahci_snap.version;
                }
            }

            let passed = class_ok && mmio_ok;
            report.entries.push(KpiValidationEntry {
                target: "AHCI (KPI cross-check)",
                passed,
                summary: format!(
                    "PCI {:02}:{:02}.{} class_ok={} CAP/VS=0x{:x} match={}",
                    dev.bus, dev.device, dev.function, class_ok, kpi_vs, mmio_ok
                ),
            });
        }
    }

    // 3. Also record Newbus PCI inventory coverage check
    let pci_count = (0..128)
        .filter_map(pci::get_device)
        .count();
    report.entries.push(KpiValidationEntry {
        target: "Newbus / PCI KPI",
        passed: pci_count > 0,
        summary: format!(
            "{} PCI devices visible, {} Newbus nodes, {} KPI IRQ routes",
            pci_count,
            newbus::snapshot_devices().len(),
            pci_res::snapshot_intr_routes().len()
        ),
    });

    *KPI_VALIDATION.lock() = report.clone();
    report
}

pub fn validation_report() -> KpiValidationReport {
    KPI_VALIDATION.lock().clone()
}
