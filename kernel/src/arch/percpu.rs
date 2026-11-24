use core::arch::asm;
use core::ptr::{addr_of, addr_of_mut};
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, AtomicUsize, Ordering};

use x86_64::registers::model_specific::Msr;

pub const MAX_SMP_CPUS: usize = 16;

pub const IA32_GS_BASE_MSR: u32 = 0xC000_0101;
pub const IA32_KERNEL_GS_BASE_MSR: u32 = 0xC000_0102;

/// Per-CPU kernel descriptor swapped in on `SYSCALL` via `swapgs`.
/// Layout offsets MUST match inline assembly in `userland/syscall.rs`:
/// - `0`:  `kernel_rsp` (`SYSCALL_STACK_TOP`)
/// - `8`:  `user_rsp_scratch`
/// - `16`: `kernel_cr3`
/// - `24`: `user_cr3`
/// - `32`: `cr3_flags`
#[repr(C, align(16))]
#[derive(Clone, Copy)]
pub struct PerCpuData {
    pub kernel_rsp: u64,
    pub user_rsp_scratch: u64,
    pub kernel_cr3: u64,
    pub user_cr3: u64,
    pub cr3_flags: u64,
    pub cpu_id: u32,
    pub apic_id: u32,
    pub syscall_gs_active: u64,
}

impl PerCpuData {
    pub const EMPTY: Self = Self {
        kernel_rsp: 0,
        user_rsp_scratch: 0,
        kernel_cr3: 0,
        user_cr3: 0,
        cr3_flags: 0,
        cpu_id: 0,
        apic_id: 0,
        syscall_gs_active: 0,
    };
}

struct PerCpuAtomics {
    online: AtomicBool,
    apic_id: AtomicU32,
    timer_irqs: AtomicU64,
    ipi_count: AtomicU64,
    tlb_shootdowns: AtomicU64,
    kthread_slices: AtomicU64,
}

impl PerCpuAtomics {
    const fn new() -> Self {
        Self {
            online: AtomicBool::new(false),
            apic_id: AtomicU32::new(0),
            timer_irqs: AtomicU64::new(0),
            ipi_count: AtomicU64::new(0),
            tlb_shootdowns: AtomicU64::new(0),
            kthread_slices: AtomicU64::new(0),
        }
    }
}

#[derive(Clone, Copy)]
pub struct PerCpuRuntimeInfo {
    pub cpu_id: usize,
    pub apic_id: u32,
    pub online: bool,
    pub timer_irqs: u64,
    pub ipi_count: u64,
    pub tlb_shootdowns: u64,
    pub kthread_slices: u64,
}

static mut PER_CPU: [PerCpuData; MAX_SMP_CPUS] = [PerCpuData::EMPTY; MAX_SMP_CPUS];
static APIC_TO_CPU: [AtomicU8; 256] = [const { AtomicU8::new(0) }; 256];
static PER_CPU_METRICS: [PerCpuAtomics; MAX_SMP_CPUS] =
    [const { PerCpuAtomics::new() }; MAX_SMP_CPUS];
static ONLINE_CPU_COUNT: AtomicUsize = AtomicUsize::new(1);

#[inline]
pub fn register_apic_cpu(apic_id: u32, cpu_id: usize) {
    let clamped_cpu = cpu_id.min(MAX_SMP_CPUS - 1);
    APIC_TO_CPU[(apic_id & 0xFF) as usize].store(clamped_cpu as u8, Ordering::Release);
    PER_CPU_METRICS[clamped_cpu]
        .apic_id
        .store(apic_id, Ordering::Release);
}

#[inline]
pub fn current_cpu_id() -> usize {
    if ONLINE_CPU_COUNT.load(Ordering::Relaxed) <= 1 {
        return 0;
    }
    let apic_id = crate::arch::apic::current_apic_id_fast();
    let mapped = APIC_TO_CPU[(apic_id & 0xFF) as usize].load(Ordering::Relaxed) as usize;
    if mapped < MAX_SMP_CPUS {
        mapped
    } else {
        0
    }
}

#[inline]
pub fn online_cpu_count() -> usize {
    ONLINE_CPU_COUNT.load(Ordering::Acquire).max(1)
}

pub fn init_bsp(syscall_stack_top: u64) {
    unsafe {
        PER_CPU[0].kernel_rsp = syscall_stack_top;
        PER_CPU[0].user_rsp_scratch = 0;
        PER_CPU[0].cpu_id = 0;
        PER_CPU[0].syscall_gs_active = 0;

        // Ring 0 executes with GS.base = 0; IA32_KERNEL_GS_BASE points to PER_CPU[0]
        // so Ring 3 <-> Ring 0 `swapgs` swaps cleanly into kernel per-CPU scratch.
        Msr::new(IA32_GS_BASE_MSR).write(0);
        Msr::new(IA32_KERNEL_GS_BASE_MSR).write(addr_of_mut!(PER_CPU[0]) as u64);
    }
    PER_CPU_METRICS[0].online.store(true, Ordering::Release);
    ONLINE_CPU_COUNT.store(1, Ordering::Release);
}

pub fn init_ap(cpu_id: usize, apic_id: u32, syscall_stack_top: u64) {
    if cpu_id == 0 || cpu_id >= MAX_SMP_CPUS {
        return;
    }
    register_apic_cpu(apic_id, cpu_id);
    unsafe {
        PER_CPU[cpu_id].kernel_rsp = syscall_stack_top;
        PER_CPU[cpu_id].user_rsp_scratch = 0;
        PER_CPU[cpu_id].cpu_id = cpu_id as u32;
        PER_CPU[cpu_id].apic_id = apic_id;
        PER_CPU[cpu_id].syscall_gs_active = 0;

        Msr::new(IA32_GS_BASE_MSR).write(0);
        Msr::new(IA32_KERNEL_GS_BASE_MSR).write(addr_of_mut!(PER_CPU[cpu_id]) as u64);
    }
    if !PER_CPU_METRICS[cpu_id].online.swap(true, Ordering::AcqRel) {
        ONLINE_CPU_COUNT.fetch_add(1, Ordering::AcqRel);
    }
}

#[inline]
pub fn set_cr3_pair(kernel_cr3: u64, user_cr3: u64, cr3_flags: u64) {
    let cpu = current_cpu_id();
    unsafe {
        PER_CPU[cpu].kernel_cr3 = kernel_cr3;
        PER_CPU[cpu].user_cr3 = user_cr3;
        PER_CPU[cpu].cr3_flags = cr3_flags;
    }
}

#[inline]
pub fn set_apic_id(apic_id: u32) {
    register_apic_cpu(apic_id, 0);
    unsafe {
        PER_CPU[0].apic_id = apic_id;
    }
    PER_CPU_METRICS[0].online.store(true, Ordering::Release);
}

#[inline]
pub fn record_timer_irq(cpu_id: usize) {
    if cpu_id < MAX_SMP_CPUS {
        PER_CPU_METRICS[cpu_id]
            .timer_irqs
            .fetch_add(1, Ordering::Relaxed);
    }
}

#[inline]
pub fn record_ipi(cpu_id: usize) {
    if cpu_id < MAX_SMP_CPUS {
        PER_CPU_METRICS[cpu_id]
            .ipi_count
            .fetch_add(1, Ordering::Relaxed);
    }
}

#[inline]
pub fn record_tlb_shootdown(cpu_id: usize) {
    if cpu_id < MAX_SMP_CPUS {
        PER_CPU_METRICS[cpu_id]
            .tlb_shootdowns
            .fetch_add(1, Ordering::Relaxed);
    }
}

#[inline]
pub fn record_kthread_slice(cpu_id: usize) {
    if cpu_id < MAX_SMP_CPUS {
        PER_CPU_METRICS[cpu_id]
            .kthread_slices
            .fetch_add(1, Ordering::Relaxed);
    }
}

#[inline]
pub fn read_msr(msr: u32) -> u64 {
    unsafe { Msr::new(msr).read() }
}

pub fn swapgs_if_needed_on_fault() {
    let gs_base = read_msr(IA32_GS_BASE_MSR);
    let cpu = current_cpu_id();
    let expected_ptr = unsafe { addr_of!(PER_CPU[cpu]) as u64 };
    if gs_base == expected_ptr {
        unsafe {
            asm!("swapgs", options(nomem, nostack));
        }
    }
}

pub fn bsp_snapshot() -> PerCpuData {
    unsafe { PER_CPU[0] }
}

pub fn snapshot_all() -> [PerCpuRuntimeInfo; MAX_SMP_CPUS] {
    let mut out = [PerCpuRuntimeInfo {
        cpu_id: 0,
        apic_id: 0,
        online: false,
        timer_irqs: 0,
        ipi_count: 0,
        tlb_shootdowns: 0,
        kthread_slices: 0,
    }; MAX_SMP_CPUS];

    for i in 0..MAX_SMP_CPUS {
        let m = &PER_CPU_METRICS[i];
        out[i] = PerCpuRuntimeInfo {
            cpu_id: i,
            apic_id: m.apic_id.load(Ordering::Relaxed),
            online: m.online.load(Ordering::Acquire),
            timer_irqs: m.timer_irqs.load(Ordering::Relaxed),
            ipi_count: m.ipi_count.load(Ordering::Relaxed),
            tlb_shootdowns: m.tlb_shootdowns.load(Ordering::Relaxed),
            kthread_slices: m.kthread_slices.load(Ordering::Relaxed),
        };
    }
    out
}
