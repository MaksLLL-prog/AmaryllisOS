use core::arch::asm;
use core::ptr::{read_volatile, write_volatile};
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use spin::Mutex;
use x86_64::instructions::interrupts::without_interrupts;
use x86_64::registers::model_specific::Msr;
use x86_64::structures::paging::PageTableFlags;
use x86_64::VirtAddr;

use crate::interrupts::timer;
use crate::klog::{self, KLogLevel};
use crate::memory::vmm::{PhysicalMemoryMapping, PAGE_SIZE};
use crate::memory::PhysicalFrameAllocator;

use super::acpi::{AcpiPlatformInfo, MAX_MADT_CPUS, MAX_MADT_IOAPICS};
use super::percpu;

pub const APIC_TIMER_VECTOR: u8 = 0x40;
pub const IOAPIC_IRQ_VECTOR_BASE: u8 = 0x50;
pub const IOAPIC_IRQ_VECTOR_COUNT: usize = 8;
pub const IPI_TLB_SHOOTDOWN_VECTOR: u8 = 0xF0;
pub const IPI_RESCHEDULE_VECTOR: u8 = 0xF1;

const IA32_APIC_BASE_MSR: u32 = 0x1B;
const LAPIC_MMIO_VIRT_BASE: u64 = 0x0000_5300_0000_0000;
const IOAPIC_MMIO_VIRT_BASE: u64 = 0x0000_5310_0000_0000;

const LAPIC_REG_ID: usize = 0x020;
const LAPIC_REG_VERSION: usize = 0x030;
const LAPIC_REG_TPR: usize = 0x080;
const LAPIC_REG_EOI: usize = 0x0B0;
const LAPIC_REG_SVR: usize = 0x0F0;
const LAPIC_REG_ICR_LO: usize = 0x300;
const LAPIC_REG_ICR_HI: usize = 0x310;
const LAPIC_REG_LVT_TIMER: usize = 0x320;
const LAPIC_REG_LVT_LINT0: usize = 0x350;
const LAPIC_REG_TIMER_INIT: usize = 0x380;
const LAPIC_REG_TIMER_CUR: usize = 0x390;
const LAPIC_REG_TIMER_DIV: usize = 0x3E0;

const LAPIC_TIMER_MODE_PERIODIC: u32 = 1 << 17;
const LAPIC_LVT_MASK: u32 = 1 << 16;

const ICR_DELIVERY_PENDING: u32 = 1 << 12;
const ICR_LEVEL_ASSERT: u32 = 1 << 14;
const ICR_TRIGGER_LEVEL: u32 = 1 << 15;
const ICR_DELIVERY_INIT: u32 = 5 << 8;
const ICR_DELIVERY_STARTUP: u32 = 6 << 8;
const ICR_DEST_ALL_EXCL_SELF: u32 = 3 << 18;

#[derive(Clone, Copy)]
pub struct IoApicRuntimeInfo {
    pub id: u8,
    pub phys_addr: u32,
    pub virt_addr: u64,
    pub gsi_base: u32,
    pub version: u8,
    pub max_redirs: u8,
    pub mapped: bool,
}

impl IoApicRuntimeInfo {
    pub const EMPTY: Self = Self {
        id: 0,
        phys_addr: 0,
        virt_addr: 0,
        gsi_base: 0,
        version: 0,
        max_redirs: 0,
        mapped: false,
    };
}

#[derive(Clone, Copy)]
pub struct ApicSmpSnapshot {
    pub apic_supported: bool,
    pub x2apic_supported: bool,
    pub lapic_enabled: bool,
    pub lapic_phys: u64,
    pub lapic_virt: u64,
    pub bsp_apic_id: u32,
    pub lapic_version: u8,
    pub lapic_max_lvt: u8,
    pub svr_value: u32,
    pub detected_cores: usize,
    pub enabled_cores: usize,
    pub ioapics: [IoApicRuntimeInfo; MAX_MADT_IOAPICS],
    pub ioapic_count: usize,
    pub cpu_vendor: [u8; 12],
    pub cpu_brand: [u8; 48],
    pub cpu_brand_len: usize,
    pub timer_active: bool,
    pub timer_hz: u32,
    pub timer_ticks_per_ms: u32,
    pub timer_init_count: u32,
}

impl ApicSmpSnapshot {
    pub const EMPTY: Self = Self {
        apic_supported: false,
        x2apic_supported: false,
        lapic_enabled: false,
        lapic_phys: 0,
        lapic_virt: 0,
        bsp_apic_id: 0,
        lapic_version: 0,
        lapic_max_lvt: 0,
        svr_value: 0,
        detected_cores: 1,
        enabled_cores: 1,
        ioapics: [IoApicRuntimeInfo::EMPTY; MAX_MADT_IOAPICS],
        ioapic_count: 0,
        cpu_vendor: [0; 12],
        cpu_brand: [0; 48],
        cpu_brand_len: 0,
        timer_active: false,
        timer_hz: 0,
        timer_ticks_per_ms: 0,
        timer_init_count: 0,
    };

    pub fn vendor_str(&self) -> &str {
        core::str::from_utf8(&self.cpu_vendor).unwrap_or("GenuineCPU")
    }

    pub fn brand_str(&self) -> &str {
        core::str::from_utf8(&self.cpu_brand[..self.cpu_brand_len])
            .unwrap_or("x86_64 Processor")
            .trim()
    }
}

static APIC_SMP_STATE: Mutex<ApicSmpSnapshot> = Mutex::new(ApicSmpSnapshot::EMPTY);
static LAPIC_VIRT_FAST: AtomicU64 = AtomicU64::new(0);
static LAPIC_ENABLED_FAST: AtomicBool = AtomicBool::new(false);
static APIC_TIMER_ACTIVE: AtomicBool = AtomicBool::new(false);
static APIC_TIMER_HZ_FAST: AtomicU32 = AtomicU32::new(0);
static APIC_TICKS_PER_MS: AtomicU32 = AtomicU32::new(0);
static APIC_INIT_COUNT_FAST: AtomicU32 = AtomicU32::new(0);
static IOAPIC_LOCK: Mutex<()> = Mutex::new(());
static IOAPIC_FAST_COUNT: core::sync::atomic::AtomicUsize =
    core::sync::atomic::AtomicUsize::new(0);
static IOAPIC_FAST_VIRT: [AtomicU64; MAX_MADT_IOAPICS] =
    [const { AtomicU64::new(0) }; MAX_MADT_IOAPICS];
static IOAPIC_FAST_GSI_BASE: [AtomicU32; MAX_MADT_IOAPICS] =
    [const { AtomicU32::new(0) }; MAX_MADT_IOAPICS];
static IOAPIC_FAST_MAX_REDIRS: [core::sync::atomic::AtomicU8; MAX_MADT_IOAPICS] =
    [const { core::sync::atomic::AtomicU8::new(0) }; MAX_MADT_IOAPICS];

#[inline]
unsafe fn lapic_read(virt_base: u64, offset: usize) -> u32 {
    unsafe { read_volatile((virt_base + offset as u64) as *const u32) }
}

#[inline]
unsafe fn lapic_write(virt_base: u64, offset: usize, val: u32) {
    unsafe { write_volatile((virt_base + offset as u64) as *mut u32, val) }
}

#[inline]
unsafe fn ioapic_read(virt_base: u64, reg: u32) -> u32 {
    let ioregsel = virt_base as *mut u32;
    let iowin = (virt_base + 0x10) as *const u32;
    unsafe {
        write_volatile(ioregsel, reg);
        read_volatile(iowin)
    }
}

#[inline]
unsafe fn ioapic_write(virt_base: u64, reg: u32, val: u32) {
    let ioregsel = virt_base as *mut u32;
    let iowin = (virt_base + 0x10) as *mut u32;
    unsafe {
        write_volatile(ioregsel, reg);
        write_volatile(iowin, val);
    }
}

fn read_cpu_identity(snap: &mut ApicSmpSnapshot) {
    let leaf0 = core::arch::x86_64::__cpuid(0);
    snap.cpu_vendor[0..4].copy_from_slice(&leaf0.ebx.to_le_bytes());
    snap.cpu_vendor[4..8].copy_from_slice(&leaf0.edx.to_le_bytes());
    snap.cpu_vendor[8..12].copy_from_slice(&leaf0.ecx.to_le_bytes());

    if leaf0.eax >= 1 {
        let leaf1 = core::arch::x86_64::__cpuid(1);
        snap.apic_supported = (leaf1.edx & (1 << 9)) != 0;
        snap.x2apic_supported = (leaf1.ecx & (1 << 21)) != 0;
        snap.bsp_apic_id = (leaf1.ebx >> 24) & 0xFF;
    }

    let max_ext = core::arch::x86_64::__cpuid(0x8000_0000).eax;
    if max_ext >= 0x8000_0004 {
        let mut raw = [0u8; 48];
        for i in 0..3u32 {
            let r = core::arch::x86_64::__cpuid(0x8000_0002 + i);
            let base = (i as usize) * 16;
            raw[base..base + 4].copy_from_slice(&r.eax.to_le_bytes());
            raw[base + 4..base + 8].copy_from_slice(&r.ebx.to_le_bytes());
            raw[base + 8..base + 12].copy_from_slice(&r.ecx.to_le_bytes());
            raw[base + 12..base + 16].copy_from_slice(&r.edx.to_le_bytes());
        }
        let trimmed = raw
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(raw.len());
        snap.cpu_brand[..trimmed].copy_from_slice(&raw[..trimmed]);
        snap.cpu_brand_len = trimmed;
    }
}

#[inline(never)]
pub fn init(
    acpi: &AcpiPlatformInfo,
    mapping: &PhysicalMemoryMapping,
    allocator: &mut PhysicalFrameAllocator<'_>,
) -> ApicSmpSnapshot {
    let mut snap = ApicSmpSnapshot::EMPTY;
    read_cpu_identity(&mut snap);

    if acpi.cpu_count > 0 {
        snap.detected_cores = acpi.cpu_count.min(MAX_MADT_CPUS);
        snap.enabled_cores = acpi.enabled_cpu_count.max(1);
    }

    let msr_val = unsafe { Msr::new(IA32_APIC_BASE_MSR).read() };
    let msr_phys = msr_val & 0xFFFF_F000;
    let lapic_phys = if acpi.lapic_phys != 0 {
        acpi.lapic_phys & !(PAGE_SIZE - 1)
    } else if msr_phys != 0 {
        msr_phys
    } else {
        0xFEE0_0000
    };
    snap.lapic_phys = lapic_phys;

    let mmio_flags = PageTableFlags::PRESENT
        | PageTableFlags::WRITABLE
        | PageTableFlags::NO_CACHE
        | PageTableFlags::WRITE_THROUGH
        | PageTableFlags::NO_EXECUTE;

    if snap.apic_supported && lapic_phys != 0 {
        // Ensure Local APIC global enable bit (bit 11) in IA32_APIC_BASE MSR
        if (msr_val & (1 << 11)) == 0 {
            unsafe {
                Msr::new(IA32_APIC_BASE_MSR).write(msr_val | (1 << 11));
            }
        }

        let lapic_virt = VirtAddr::new(LAPIC_MMIO_VIRT_BASE);
        let mapped_ok = if mapping.translate_virtual_address(lapic_virt).is_some() {
            true
        } else {
            mapping
                .map_page(allocator, lapic_virt, lapic_phys, mmio_flags)
                .is_ok()
        };

        if mapped_ok {
            snap.lapic_virt = LAPIC_MMIO_VIRT_BASE;
            unsafe {
                // Enable software LAPIC bit (bit 8) in Spurious Interrupt Vector Register (0xFF vector)
                // while keeping dual-8259 PIC active for existing PIT/PS2 routing.
                let svr = lapic_read(LAPIC_MMIO_VIRT_BASE, LAPIC_REG_SVR);
                let new_svr = (svr & !0xFF) | 0x1FF;
                lapic_write(LAPIC_MMIO_VIRT_BASE, LAPIC_REG_SVR, new_svr);

                // Ensure BSP LINT0 Virtual Wire Mode (ExtINT) is unmasked so 8259 PIC IRQs
                // (PIT IRQ0, PS/2 Keyboard IRQ1, PS/2 Mouse IRQ12) continue reaching the CPU
                // regardless of whether MADT PCAT_COMPAT is reported.
                let lvt_lint0 = lapic_read(LAPIC_MMIO_VIRT_BASE, LAPIC_REG_LVT_LINT0);
                if (lvt_lint0 & 0x700) == 0x700 {
                    if (lvt_lint0 & LAPIC_LVT_MASK) != 0 {
                        lapic_write(
                            LAPIC_MMIO_VIRT_BASE,
                            LAPIC_REG_LVT_LINT0,
                            lvt_lint0 & !LAPIC_LVT_MASK,
                        );
                    }
                } else if (lvt_lint0 & 0x700) == 0 && (lvt_lint0 & LAPIC_LVT_MASK) != 0 {
                    lapic_write(LAPIC_MMIO_VIRT_BASE, LAPIC_REG_LVT_LINT0, 0x0000_0700);
                }

                // Clear Task Priority to 0 (allow all priorities) and mask LAPIC timer until calibrated.
                lapic_write(LAPIC_MMIO_VIRT_BASE, LAPIC_REG_TPR, 0);
                let lvt_timer = lapic_read(LAPIC_MMIO_VIRT_BASE, LAPIC_REG_LVT_TIMER);
                lapic_write(LAPIC_MMIO_VIRT_BASE, LAPIC_REG_LVT_TIMER, lvt_timer | (1 << 16));

                let _ = lapic_read(LAPIC_MMIO_VIRT_BASE, LAPIC_REG_TIMER_CUR);
                let _ = lapic_read(LAPIC_MMIO_VIRT_BASE, LAPIC_REG_TIMER_INIT);
                let _ = lapic_read(LAPIC_MMIO_VIRT_BASE, LAPIC_REG_TIMER_DIV);

                let id_reg = lapic_read(LAPIC_MMIO_VIRT_BASE, LAPIC_REG_ID);
                let ver_reg = lapic_read(LAPIC_MMIO_VIRT_BASE, LAPIC_REG_VERSION);
                snap.bsp_apic_id = (id_reg >> 24) & 0xFF;
                snap.lapic_version = (ver_reg & 0xFF) as u8;
                snap.lapic_max_lvt = (((ver_reg >> 16) & 0xFF) + 1) as u8;
                snap.svr_value = lapic_read(LAPIC_MMIO_VIRT_BASE, LAPIC_REG_SVR);
                snap.lapic_enabled = (snap.svr_value & (1 << 8)) != 0;
            }
        }
    }

    percpu::set_apic_id(snap.bsp_apic_id);

    // Map and inspect all MADT IOAPIC controllers
    for idx in 0..acpi.ioapic_count.min(MAX_MADT_IOAPICS) {
        let entry = acpi.ioapics[idx];
        let vaddr = IOAPIC_MMIO_VIRT_BASE + (idx as u64) * PAGE_SIZE;
        let virt = VirtAddr::new(vaddr);
        let phys = (entry.phys_addr as u64) & !(PAGE_SIZE - 1);
        let mapped = if phys == 0 {
            false
        } else if mapping.translate_virtual_address(virt).is_some() {
            true
        } else {
            mapping.map_page(allocator, virt, phys, mmio_flags).is_ok()
        };

        let mut rt = IoApicRuntimeInfo {
            id: entry.ioapic_id,
            phys_addr: entry.phys_addr,
            virt_addr: vaddr,
            gsi_base: entry.gsi_base,
            version: 0,
            max_redirs: 0,
            mapped,
        };

        if mapped {
            unsafe {
                let ver = ioapic_read(vaddr, 0x01);
                rt.version = (ver & 0xFF) as u8;
                rt.max_redirs = (((ver >> 16) & 0xFF) + 1) as u8;
            }
        }

        snap.ioapics[snap.ioapic_count] = rt;
        snap.ioapic_count += 1;
    }

    for i in 0..snap.ioapic_count {
        let io = snap.ioapics[i];
        IOAPIC_FAST_VIRT[i].store(if io.mapped { io.virt_addr } else { 0 }, Ordering::Release);
        IOAPIC_FAST_GSI_BASE[i].store(io.gsi_base, Ordering::Release);
        IOAPIC_FAST_MAX_REDIRS[i].store(io.max_redirs, Ordering::Release);
    }
    IOAPIC_FAST_COUNT.store(snap.ioapic_count, Ordering::Release);

    LAPIC_VIRT_FAST.store(snap.lapic_virt, Ordering::Release);
    LAPIC_ENABLED_FAST.store(snap.lapic_enabled, Ordering::Release);
    *APIC_SMP_STATE.lock() = snap;
    klog::log_fmt(
        KLogLevel::Info,
        "apic",
        format_args!(
            "LAPIC enabled={} bsp_id={} cores={} ioapics={}",
            snap.lapic_enabled, snap.bsp_apic_id, snap.detected_cores, snap.ioapic_count
        ),
    );
    crate::serial::log_fmt(
        "apic",
        format_args!(
            "enabled={} bsp_id={} detected_cores={} ioapics={} lapic_phys=0x{:x}",
            snap.lapic_enabled,
            snap.bsp_apic_id,
            snap.detected_cores,
            snap.ioapic_count,
            snap.lapic_phys
        ),
    );
    snap
}

/// Calibrates the Local APIC timer against the 1000 Hz PIT clock and starts
/// periodic preemption interrupts on `APIC_TIMER_VECTOR` (0x40).
pub fn calibrate_and_enable_timer(target_hz: u32) -> bool {
    if !LAPIC_ENABLED_FAST.load(Ordering::Acquire) {
        return false;
    }
    let lapic_virt = LAPIC_VIRT_FAST.load(Ordering::Acquire);
    if lapic_virt == 0 {
        return false;
    }

    let hz = target_hz.clamp(100, 2000);

    // Measure LAPIC timer frequency over a 20 ms PIT window with divide-by-16.
    let window_ms: u64 = 20;
    let t_start = timer::uptime_ms();
    unsafe {
        lapic_write(lapic_virt, LAPIC_REG_TIMER_DIV, 0x03); // divide by 16
        lapic_write(
            lapic_virt,
            LAPIC_REG_LVT_TIMER,
            LAPIC_LVT_MASK | (APIC_TIMER_VECTOR as u32),
        );
        lapic_write(lapic_virt, LAPIC_REG_TIMER_INIT, 0xFFFF_FFFF);
    }

    let mut ticks_elapsed: u32 = 0;
    let mut spins: u32 = 0;
    while timer::uptime_ms().saturating_sub(t_start) < window_ms && spins < 20_000_000 {
        spins += 1;
        core::hint::spin_loop();
    }

    let actual_ms = timer::uptime_ms().saturating_sub(t_start);
    let cur = unsafe { lapic_read(lapic_virt, LAPIC_REG_TIMER_CUR) };
    if cur > 0 && cur < 0xFFFF_FFFF {
        ticks_elapsed = 0xFFFF_FFFF - cur;
    }

    let ticks_per_ms = if actual_ms > 0 && ticks_elapsed > 100 {
        (ticks_elapsed as u64 / actual_ms).clamp(1_000, 50_000_000) as u32
    } else {
        62_500
    };

    let init_count = ((ticks_per_ms as u64 * 1000) / hz as u64).clamp(2_000, 50_000_000) as u32;

    unsafe {
        lapic_write(lapic_virt, LAPIC_REG_TIMER_DIV, 0x03); // divide by 16
        lapic_write(
            lapic_virt,
            LAPIC_REG_LVT_TIMER,
            LAPIC_TIMER_MODE_PERIODIC | (APIC_TIMER_VECTOR as u32),
        );
        lapic_write(lapic_virt, LAPIC_REG_TIMER_INIT, init_count);
    }

    APIC_TICKS_PER_MS.store(ticks_per_ms, Ordering::Release);
    APIC_INIT_COUNT_FAST.store(init_count, Ordering::Release);
    APIC_TIMER_HZ_FAST.store(hz, Ordering::Release);
    APIC_TIMER_ACTIVE.store(true, Ordering::Release);

    {
        let mut st = APIC_SMP_STATE.lock();
        st.timer_active = true;
        st.timer_hz = hz;
        st.timer_ticks_per_ms = ticks_per_ms;
        st.timer_init_count = init_count;
    }

    klog::log_fmt(
        KLogLevel::Info,
        "apic",
        format_args!(
            "Periodic LAPIC timer armed: {}Hz vec=0x{:02x} cal={}t/ms init={}",
            hz, APIC_TIMER_VECTOR, ticks_per_ms, init_count
        ),
    );
    crate::serial::log_fmt(
        "apic",
        format_args!(
            "timer armed {}Hz vec=0x{:02x} cal={}t/ms actual_ms={} init={}",
            hz, APIC_TIMER_VECTOR, ticks_per_ms, actual_ms, init_count
        ),
    );
    true
}

/// Called on each Application Processor right after jumping to long mode:
/// enables its Local APIC (via IA32_APIC_BASE + SVR) and returns its hardware APIC ID
/// BEFORE arming its periodic timer so `percpu::init_ap` can initialize first.
pub fn enable_ap_lapic() -> u32 {
    let msr_val = unsafe { Msr::new(IA32_APIC_BASE_MSR).read() };
    if (msr_val & (1 << 11)) == 0 {
        unsafe {
            Msr::new(IA32_APIC_BASE_MSR).write(msr_val | (1 << 11));
        }
    }

    let lapic_virt = LAPIC_VIRT_FAST.load(Ordering::Acquire);
    if lapic_virt == 0 {
        return 0;
    }

    unsafe {
        let svr = lapic_read(lapic_virt, LAPIC_REG_SVR);
        lapic_write(lapic_virt, LAPIC_REG_SVR, (svr & !0xFF) | 0x1FF);
        lapic_write(lapic_virt, LAPIC_REG_TPR, 0);
        let lvt_timer = lapic_read(lapic_virt, LAPIC_REG_LVT_TIMER);
        lapic_write(lapic_virt, LAPIC_REG_LVT_TIMER, lvt_timer | LAPIC_LVT_MASK);
        (lapic_read(lapic_virt, LAPIC_REG_ID) >> 24) & 0xFF
    }
}

/// Arms the AP's per-core periodic LAPIC timer using the BSP-calibrated initial count.
pub fn arm_ap_timer() {
    let lapic_virt = LAPIC_VIRT_FAST.load(Ordering::Acquire);
    if lapic_virt == 0 {
        return;
    }
    let init_count = APIC_INIT_COUNT_FAST.load(Ordering::Acquire);
    if APIC_TIMER_ACTIVE.load(Ordering::Acquire) && init_count > 0 {
        unsafe {
            lapic_write(lapic_virt, LAPIC_REG_TIMER_DIV, 0x03);
            lapic_write(
                lapic_virt,
                LAPIC_REG_LVT_TIMER,
                LAPIC_TIMER_MODE_PERIODIC | (APIC_TIMER_VECTOR as u32),
            );
            lapic_write(lapic_virt, LAPIC_REG_TIMER_INIT, init_count);
        }
    }
}

pub fn enable_ap_lapic_and_timer() -> u32 {
    let apic_id = enable_ap_lapic();
    arm_ap_timer();
    apic_id
}

#[inline]
pub fn current_apic_id_fast() -> u32 {
    if !LAPIC_ENABLED_FAST.load(Ordering::Relaxed) {
        return 0;
    }
    let lapic_virt = LAPIC_VIRT_FAST.load(Ordering::Relaxed);
    if lapic_virt == 0 {
        return 0;
    }
    unsafe { (lapic_read(lapic_virt, LAPIC_REG_ID) >> 24) & 0xFF }
}

#[inline]
pub fn lapic_virt() -> u64 {
    LAPIC_VIRT_FAST.load(Ordering::Acquire)
}

#[inline]
fn wait_icr_idle(lapic_virt: u64) {
    for _ in 0..100_000 {
        let lo = unsafe { lapic_read(lapic_virt, LAPIC_REG_ICR_LO) };
        if (lo & ICR_DELIVERY_PENDING) == 0 {
            break;
        }
        core::hint::spin_loop();
    }
}

pub unsafe fn send_init_ipi(dest_apic_id: u32) {
    let lapic_virt = LAPIC_VIRT_FAST.load(Ordering::Acquire);
    if lapic_virt == 0 {
        return;
    }
    without_interrupts(|| {
        wait_icr_idle(lapic_virt);
        unsafe {
            lapic_write(lapic_virt, LAPIC_REG_ICR_HI, (dest_apic_id & 0xFF) << 24);
            lapic_write(
                lapic_virt,
                LAPIC_REG_ICR_LO,
                ICR_DELIVERY_INIT | ICR_LEVEL_ASSERT | ICR_TRIGGER_LEVEL,
            );
            wait_icr_idle(lapic_virt);
            lapic_write(lapic_virt, LAPIC_REG_ICR_HI, (dest_apic_id & 0xFF) << 24);
            lapic_write(
                lapic_virt,
                LAPIC_REG_ICR_LO,
                ICR_DELIVERY_INIT | ICR_TRIGGER_LEVEL,
            );
            wait_icr_idle(lapic_virt);
        }
    });
}

pub unsafe fn send_startup_ipi(dest_apic_id: u32, start_page_vector: u8) {
    let lapic_virt = LAPIC_VIRT_FAST.load(Ordering::Acquire);
    if lapic_virt == 0 {
        return;
    }
    without_interrupts(|| {
        wait_icr_idle(lapic_virt);
        unsafe {
            lapic_write(lapic_virt, LAPIC_REG_ICR_HI, (dest_apic_id & 0xFF) << 24);
            lapic_write(
                lapic_virt,
                LAPIC_REG_ICR_LO,
                ICR_DELIVERY_STARTUP | (start_page_vector as u32),
            );
            wait_icr_idle(lapic_virt);
        }
    });
}

pub fn send_ipi(dest_apic_id: u32, vector: u8) {
    if !LAPIC_ENABLED_FAST.load(Ordering::Acquire) {
        return;
    }
    let lapic_virt = LAPIC_VIRT_FAST.load(Ordering::Acquire);
    if lapic_virt == 0 {
        return;
    }
    without_interrupts(|| {
        wait_icr_idle(lapic_virt);
        unsafe {
            lapic_write(lapic_virt, LAPIC_REG_ICR_HI, (dest_apic_id & 0xFF) << 24);
            lapic_write(lapic_virt, LAPIC_REG_ICR_LO, vector as u32);
        }
    });
}

pub fn broadcast_ipi_excluding_self(vector: u8) {
    if !LAPIC_ENABLED_FAST.load(Ordering::Acquire) {
        return;
    }
    let lapic_virt = LAPIC_VIRT_FAST.load(Ordering::Acquire);
    if lapic_virt == 0 {
        return;
    }
    without_interrupts(|| {
        wait_icr_idle(lapic_virt);
        unsafe {
            lapic_write(lapic_virt, LAPIC_REG_ICR_HI, 0);
            lapic_write(
                lapic_virt,
                LAPIC_REG_ICR_LO,
                ICR_DEST_ALL_EXCL_SELF | (vector as u32),
            );
        }
    });
}

/// Resolves an ISA/legacy IRQ line against ACPI MADT Interrupt Source Overrides (ISO),
/// returning `(gsi, level_triggered, active_low)`.
pub fn resolve_pci_or_isa_gsi(acpi: &AcpiPlatformInfo, irq_line: u8) -> (u32, bool, bool) {
    for i in 0..acpi.iso_count {
        let iso = acpi.isos[i];
        if iso.irq_source == irq_line {
            let polarity = iso.flags & 0x3;
            let trigger = (iso.flags >> 2) & 0x3;
            let active_low = polarity == 3;
            let level_triggered = trigger == 3;
            return (iso.gsi, level_triggered, active_low);
        }
    }
    // On QEMU (PIIX/i440fx and Q35) PCI INTx lines wired into IOAPIC without an explicit
    // MADT Interrupt Source Override are active-high level-triggered (matching MADT ISO 0x000D).
    // Setting active_low=true on active-high QEMU lines triggers an immediate level storm.
    if irq_line >= 5 {
        (irq_line as u32, true, false)
    } else {
        (irq_line as u32, false, false)
    }
}

/// Routes Global System Interrupt `gsi` on the matching IOAPIC controller onto IDT `vector`,
/// targeting Local APIC ID `dest_apic_id`.
pub fn route_ioapic_irq(
    gsi: u32,
    vector: u8,
    dest_apic_id: u32,
    level_triggered: bool,
    active_low: bool,
) -> bool {
    let snap = *APIC_SMP_STATE.lock();
    let mut target_virt = 0u64;
    let mut local_pin = 0u32;

    for i in 0..snap.ioapic_count {
        let io = snap.ioapics[i];
        if !io.mapped || io.virt_addr == 0 {
            continue;
        }
        let end_gsi = io.gsi_base.saturating_add(io.max_redirs as u32);
        if gsi >= io.gsi_base && gsi < end_gsi {
            target_virt = io.virt_addr;
            local_pin = gsi - io.gsi_base;
            break;
        }
    }

    if target_virt == 0 {
        return false;
    }

    let mut lo: u32 = vector as u32; // Fixed delivery (000), physical destination (0)
    if active_low {
        lo |= 1 << 13;
    }
    if level_triggered {
        lo |= 1 << 15;
    }
    let hi: u32 = (dest_apic_id & 0xFF) << 24;

    without_interrupts(|| {
        let _guard = IOAPIC_LOCK.lock();
        let reg_lo = 0x10 + local_pin * 2;
        let reg_hi = reg_lo + 1;
        unsafe {
            // Mask entry first while updating high/low halves
            ioapic_write(target_virt, reg_lo, 1 << 16);
            ioapic_write(target_virt, reg_hi, hi);
            ioapic_write(target_virt, reg_lo, lo);
        }
    });

    klog::log_fmt(
        KLogLevel::Info,
        "ioapic",
        format_args!(
            "Routed GSI {} (pin {}) -> vec 0x{:02x} dest_apic={} ({}/{})",
            gsi,
            local_pin,
            vector,
            dest_apic_id,
            if level_triggered { "level" } else { "edge" },
            if active_low { "low" } else { "high" }
        ),
    );
    true
}

pub fn mask_ioapic_irq(gsi: u32) -> bool {
    let snap = *APIC_SMP_STATE.lock();
    for i in 0..snap.ioapic_count {
        let io = snap.ioapics[i];
        if !io.mapped || io.virt_addr == 0 {
            continue;
        }
        let end_gsi = io.gsi_base.saturating_add(io.max_redirs as u32);
        if gsi >= io.gsi_base && gsi < end_gsi {
            let reg_lo = 0x10 + (gsi - io.gsi_base) * 2;
            without_interrupts(|| {
                let _guard = IOAPIC_LOCK.lock();
                unsafe {
                    let cur = ioapic_read(io.virt_addr, reg_lo);
                    ioapic_write(io.virt_addr, reg_lo, cur | (1 << 16));
                }
            });
            return true;
        }
    }
    false
}

/// Lock-free / IRQ-safe polarity toggle used to recover automatically if a platform
/// reports the opposite PCI INTx polarity without an ACPI MADT override.
pub fn flip_ioapic_polarity_irqsafe(gsi: u32) -> Option<bool> {
    let count = IOAPIC_FAST_COUNT.load(Ordering::Acquire).min(MAX_MADT_IOAPICS);
    for i in 0..count {
        let virt = IOAPIC_FAST_VIRT[i].load(Ordering::Acquire);
        if virt == 0 {
            continue;
        }
        let base = IOAPIC_FAST_GSI_BASE[i].load(Ordering::Acquire);
        let max_r = IOAPIC_FAST_MAX_REDIRS[i].load(Ordering::Acquire) as u32;
        if gsi >= base && gsi < base.saturating_add(max_r) {
            let reg_lo = 0x10 + (gsi - base) * 2;
            let _guard = IOAPIC_LOCK.try_lock()?;
            unsafe {
                let cur = ioapic_read(virt, reg_lo);
                let toggled = cur ^ (1 << 13);
                ioapic_write(virt, reg_lo, toggled | (1 << 16));
                ioapic_write(virt, reg_lo, toggled);
                return Some((toggled & (1 << 13)) != 0);
            }
        }
    }
    None
}

/// Lock-free / IRQ-safe IOAPIC GSI masking guard to prevent runaway IRQ storms.
pub fn mask_ioapic_irq_irqsafe(gsi: u32) -> bool {
    let count = IOAPIC_FAST_COUNT.load(Ordering::Acquire).min(MAX_MADT_IOAPICS);
    for i in 0..count {
        let virt = IOAPIC_FAST_VIRT[i].load(Ordering::Acquire);
        if virt == 0 {
            continue;
        }
        let base = IOAPIC_FAST_GSI_BASE[i].load(Ordering::Acquire);
        let max_r = IOAPIC_FAST_MAX_REDIRS[i].load(Ordering::Acquire) as u32;
        if gsi >= base && gsi < base.saturating_add(max_r) {
            let reg_lo = 0x10 + (gsi - base) * 2;
            let Some(_guard) = IOAPIC_LOCK.try_lock() else {
                return false;
            };
            unsafe {
                let cur = ioapic_read(virt, reg_lo);
                ioapic_write(virt, reg_lo, cur | (1 << 16));
            }
            return true;
        }
    }
    false
}

#[inline]
pub fn is_timer_active() -> bool {
    APIC_TIMER_ACTIVE.load(Ordering::Acquire)
}

#[inline]
pub fn timer_hz() -> u32 {
    APIC_TIMER_HZ_FAST.load(Ordering::Acquire)
}

/// Lock-free Local APIC End-Of-Interrupt, safe to invoke from any IRQ context on any CPU.
#[inline]
pub fn send_lapic_eoi() {
    if LAPIC_ENABLED_FAST.load(Ordering::Acquire) {
        let lapic_virt = LAPIC_VIRT_FAST.load(Ordering::Acquire);
        if lapic_virt != 0 {
            unsafe {
                lapic_write(lapic_virt, LAPIC_REG_EOI, 0);
                asm!("nop", options(nomem, nostack));
            }
        }
    }
}

pub fn snapshot() -> ApicSmpSnapshot {
    *APIC_SMP_STATE.lock()
}
