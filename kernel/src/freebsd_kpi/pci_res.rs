use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, AtomicU8, AtomicUsize, Ordering};

use crate::arch::acpi;
use crate::drivers::iokit;
use crate::memory::vmm::PhysicalMemoryMapping;
use crate::memory::PhysicalFrameAllocator;
use crate::pci::{self, PciBarKind};
use crate::task::preempt::PreemptMutex;

use super::bus_space::{self, BusSpaceHandle};
use super::newbus::DeviceT;
use super::KpiError;

pub const SYS_RES_IRQ: u32 = 1;
pub const SYS_RES_MEMORY: u32 = 3;
pub const SYS_RES_IOPORT: u32 = 4;

pub const MAX_KPI_INTR_SLOTS: usize = 16;

#[derive(Clone, Copy, Debug)]
pub struct Resource {
    pub kind: u32,
    pub rid: u8,
    pub phys_start: u64,
    pub size: u64,
    pub bus_handle: Option<BusSpaceHandle>,
    pub irq_line: Option<u8>,
}

#[derive(Clone, Copy)]
pub struct KpiIntrRouteSnapshot {
    pub slot: usize,
    pub name: &'static str,
    pub irq_line: u8,
    pub gsi: u32,
    pub vector: u8,
    pub calls: u64,
}

static INTR_ACTIVE: [AtomicU8; MAX_KPI_INTR_SLOTS] = [const { AtomicU8::new(0) }; MAX_KPI_INTR_SLOTS];
static INTR_IRQ_LINE: [AtomicU8; MAX_KPI_INTR_SLOTS] = [const { AtomicU8::new(0) }; MAX_KPI_INTR_SLOTS];
static INTR_GSI: [AtomicU64; MAX_KPI_INTR_SLOTS] = [const { AtomicU64::new(0) }; MAX_KPI_INTR_SLOTS];
static INTR_VECTOR: [AtomicU8; MAX_KPI_INTR_SLOTS] = [const { AtomicU8::new(0) }; MAX_KPI_INTR_SLOTS];
static INTR_HANDLER: [AtomicUsize; MAX_KPI_INTR_SLOTS] = [const { AtomicUsize::new(0) }; MAX_KPI_INTR_SLOTS];
static INTR_ARG: [AtomicUsize; MAX_KPI_INTR_SLOTS] = [const { AtomicUsize::new(0) }; MAX_KPI_INTR_SLOTS];
static INTR_CALLS: [AtomicU64; MAX_KPI_INTR_SLOTS] = [const { AtomicU64::new(0) }; MAX_KPI_INTR_SLOTS];
static INTR_NAMES: PreemptMutex<[&'static str; MAX_KPI_INTR_SLOTS]> =
    PreemptMutex::new([""; MAX_KPI_INTR_SLOTS]);

#[inline]
fn dispatch_kpi_intr(slot: usize) -> bool {
    let fn_addr = INTR_HANDLER[slot].load(Ordering::Acquire);
    if fn_addr == 0 {
        return false;
    }
    let arg = INTR_ARG[slot].load(Ordering::Acquire);
    let func: fn(usize) -> bool = unsafe { core::mem::transmute(fn_addr) };
    let handled = func(arg);
    if handled {
        INTR_CALLS[slot].fetch_add(1, Ordering::Relaxed);
    }
    handled
}

fn kpi_intr_trampoline_0() -> bool { dispatch_kpi_intr(0) }
fn kpi_intr_trampoline_1() -> bool { dispatch_kpi_intr(1) }
fn kpi_intr_trampoline_2() -> bool { dispatch_kpi_intr(2) }
fn kpi_intr_trampoline_3() -> bool { dispatch_kpi_intr(3) }
fn kpi_intr_trampoline_4() -> bool { dispatch_kpi_intr(4) }
fn kpi_intr_trampoline_5() -> bool { dispatch_kpi_intr(5) }
fn kpi_intr_trampoline_6() -> bool { dispatch_kpi_intr(6) }
fn kpi_intr_trampoline_7() -> bool { dispatch_kpi_intr(7) }
fn kpi_intr_trampoline_8() -> bool { dispatch_kpi_intr(8) }
fn kpi_intr_trampoline_9() -> bool { dispatch_kpi_intr(9) }
fn kpi_intr_trampoline_10() -> bool { dispatch_kpi_intr(10) }
fn kpi_intr_trampoline_11() -> bool { dispatch_kpi_intr(11) }
fn kpi_intr_trampoline_12() -> bool { dispatch_kpi_intr(12) }
fn kpi_intr_trampoline_13() -> bool { dispatch_kpi_intr(13) }
fn kpi_intr_trampoline_14() -> bool { dispatch_kpi_intr(14) }
fn kpi_intr_trampoline_15() -> bool { dispatch_kpi_intr(15) }

const TRAMPOLINES: [fn() -> bool; MAX_KPI_INTR_SLOTS] = [
    kpi_intr_trampoline_0,
    kpi_intr_trampoline_1,
    kpi_intr_trampoline_2,
    kpi_intr_trampoline_3,
    kpi_intr_trampoline_4,
    kpi_intr_trampoline_5,
    kpi_intr_trampoline_6,
    kpi_intr_trampoline_7,
    kpi_intr_trampoline_8,
    kpi_intr_trampoline_9,
    kpi_intr_trampoline_10,
    kpi_intr_trampoline_11,
    kpi_intr_trampoline_12,
    kpi_intr_trampoline_13,
    kpi_intr_trampoline_14,
    kpi_intr_trampoline_15,
];

#[inline]
pub fn pci_read_config(dev: &DeviceT, reg: u8, width: usize) -> u32 {
    match width {
        1 => pci::read_cfg_u8(dev.bus, dev.device, dev.function, reg) as u32,
        2 => pci::read_cfg_u16(dev.bus, dev.device, dev.function, reg) as u32,
        _ => pci::read_cfg_u32(dev.bus, dev.device, dev.function, reg),
    }
}

#[inline]
pub fn pci_write_config(dev: &DeviceT, reg: u8, val: u32, width: usize) {
    match width {
        2 => pci::write_cfg_u16(dev.bus, dev.device, dev.function, reg, val as u16),
        4 => pci::write_cfg_u32(dev.bus, dev.device, dev.function, reg, val),
        _ => {}
    }
}

#[inline]
pub fn pci_enable_busmaster(dev: &DeviceT, io: bool, mem: bool) -> u8 {
    pci::enable_bus_device(dev.bus, dev.device, dev.function, io, mem, true)
}

pub fn bus_alloc_resource_any(
    dev: &DeviceT,
    kind: u32,
    rid: u8,
    mapping: Option<&PhysicalMemoryMapping>,
    allocator: &mut PhysicalFrameAllocator<'_>,
) -> Result<Resource, KpiError> {
    let pci_dev = pci::find_pci_device(dev.bus, dev.device, dev.function).ok_or(KpiError::NoDev)?;

    match kind {
        SYS_RES_IRQ => {
            let line = (pci_read_config(dev, 0x3C, 4) & 0xFF) as u8;
            if line == 0 || line == 0xFF {
                return Err(KpiError::NoEnt);
            }
            Ok(Resource {
                kind,
                rid,
                phys_start: line as u64,
                size: 1,
                bus_handle: None,
                irq_line: Some(line),
            })
        }
        SYS_RES_MEMORY => {
            for i in 0..pci_dev.bar_count {
                let bar = pci_dev.bars[i];
                if !bar.valid || bar.base == 0 || bar.index != rid {
                    continue;
                }
                if matches!(bar.kind, PciBarKind::Memory32 | PciBarKind::Memory64) {
                    let size = if bar.size_known && bar.size > 0 {
                        bar.size.min(1024 * 1024)
                    } else {
                        0x4000
                    };
                    let map = mapping.ok_or(KpiError::NoMem)?;
                    let handle = bus_space::map_mmio(bar.base, size, map, allocator)?;
                    return Ok(Resource {
                        kind,
                        rid,
                        phys_start: bar.base,
                        size,
                        bus_handle: Some(handle),
                        irq_line: None,
                    });
                }
            }
            Err(KpiError::NoEnt)
        }
        SYS_RES_IOPORT => {
            for i in 0..pci_dev.bar_count {
                let bar = pci_dev.bars[i];
                if !bar.valid || bar.base == 0 || bar.index != rid {
                    continue;
                }
                if bar.kind == PciBarKind::Io {
                    let size = if bar.size_known && bar.size > 0 {
                        bar.size
                    } else {
                        0x40
                    };
                    let handle = BusSpaceHandle::from_io_port(bar.base as u16, size);
                    return Ok(Resource {
                        kind,
                        rid,
                        phys_start: bar.base,
                        size,
                        bus_handle: Some(handle),
                        irq_line: None,
                    });
                }
            }
            Err(KpiError::NoEnt)
        }
        _ => Err(KpiError::InvalidArg),
    }
}

/// Attaches a FreeBSD KPI interrupt handler onto IOAPIC via `iokit::attach_pci_interrupt`.
pub fn bus_setup_intr(
    irq_line: u8,
    name: &'static str,
    handler: fn(usize) -> bool,
    arg: usize,
) -> Result<(u32, u8), KpiError> {
    let fn_addr = handler as *const () as usize;
    let acpi_info = acpi::snapshot();

    for slot in 0..MAX_KPI_INTR_SLOTS {
        if INTR_ACTIVE[slot]
            .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            INTR_HANDLER[slot].store(fn_addr, Ordering::Release);
            INTR_ARG[slot].store(arg, Ordering::Release);
            INTR_IRQ_LINE[slot].store(irq_line, Ordering::Release);
            INTR_NAMES.lock()[slot] = name;

            if let Some((gsi, vector)) =
                iokit::attach_pci_interrupt(&acpi_info, irq_line, TRAMPOLINES[slot])
            {
                INTR_GSI[slot].store(gsi as u64, Ordering::Release);
                INTR_VECTOR[slot].store(vector, Ordering::Release);
                return Ok((gsi, vector));
            } else {
                INTR_HANDLER[slot].store(0, Ordering::Release);
                INTR_ACTIVE[slot].store(0, Ordering::Release);
                return Err(KpiError::Io);
            }
        }
    }
    Err(KpiError::NoMem)
}

pub fn snapshot_intr_routes() -> Vec<KpiIntrRouteSnapshot> {
    let names = INTR_NAMES.lock();
    let mut out = Vec::new();
    for slot in 0..MAX_KPI_INTR_SLOTS {
        if INTR_ACTIVE[slot].load(Ordering::Acquire) != 0 {
            out.push(KpiIntrRouteSnapshot {
                slot,
                name: names[slot],
                irq_line: INTR_IRQ_LINE[slot].load(Ordering::Relaxed),
                gsi: INTR_GSI[slot].load(Ordering::Relaxed) as u32,
                vector: INTR_VECTOR[slot].load(Ordering::Relaxed),
                calls: INTR_CALLS[slot].load(Ordering::Relaxed),
            });
        }
    }
    out
}
