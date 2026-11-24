use alloc::vec::Vec;
use core::arch::asm;
use core::ptr::write_bytes;
use core::sync::atomic::{fence, AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use spin::Mutex;
use x86_64::structures::paging::PageTableFlags;
use x86_64::VirtAddr;

use crate::interrupts::timer;
use crate::klog::{self, KLogLevel};
use crate::memory::vmm::{self, PhysicalMemoryMapping, PAGE_SIZE};
use crate::memory::PhysicalFrameAllocator;

use super::acpi::AcpiPlatformInfo;
use super::apic::{self, IPI_RESCHEDULE_VECTOR, IPI_TLB_SHOOTDOWN_VECTOR};
use super::gdt;
use super::percpu::{self, MAX_SMP_CPUS};

const TRAMPOLINE_PHYS: u64 = 0x8000;
const TRAMPOLINE_SIPI_VEC: u8 = 0x08;
const AP_BOOT_STACK_SIZE: usize = 32 * 1024;
const AP_SYSCALL_STACK_SIZE: usize = 16 * 1024;

// Mailbox offsets inside the 4 KiB page at 0x8000:
// 0x8F00: GDT pointer (6B)
// 0x8F08: GDT entries (5 * 8 = 40B -> 0x8F30)
// 0x8F40: pml4_phys (u32)
// 0x8F44: efer_mask (u32)
// 0x8F48: ap_stack_top (u64)
// 0x8F50: ap_cpu_id (u32)
// 0x8F58: ap_rust_entry (u64)
const OFF_GDT_PTR: usize = 0x0F00;
const OFF_GDT_ENTRIES: usize = 0x0F08;
const OFF_PML4_PHYS: usize = 0x0F40;
const OFF_EFER_MASK: usize = 0x0F44;
const OFF_AP_STACK: usize = 0x0F48;
const OFF_AP_CPU_ID: usize = 0x0F50;
const OFF_AP_ENTRY: usize = 0x0F58;

// 16-bit real mode entry @ 0x8000
#[rustfmt::skip]
const TRAMPOLINE_16: &[u8] = &[
    0xFA,                         // cli
    0xFC,                         // cld
    0x31, 0xC0,                   // xor ax, ax
    0x8E, 0xD8,                   // mov ds, ax
    0x8E, 0xC0,                   // mov es, ax
    0x8E, 0xD0,                   // mov ss, ax
    0x0F, 0x01, 0x16, 0x00, 0x8F, // lgdt [0x8F00]
    0x66, 0x0F, 0x20, 0xC0,       // mov eax, cr0
    0x66, 0x83, 0xC8, 0x01,       // or eax, 1
    0x66, 0x0F, 0x22, 0xC0,       // mov cr0, eax
    0xEA, 0x30, 0x80, 0x08, 0x00, // ljmp 0x08:0x8030
];

// 32-bit protected mode stub @ 0x8030
#[rustfmt::skip]
const TRAMPOLINE_32: &[u8] = &[
    0x66, 0xB8, 0x10, 0x00,             // mov ax, 0x10
    0x8E, 0xD8,                         // mov ds, ax
    0x8E, 0xC0,                         // mov es, ax
    0x8E, 0xD0,                         // mov ss, ax
    0x8E, 0xE0,                         // mov fs, ax
    0x8E, 0xE8,                         // mov gs, ax
    0x0F, 0x20, 0xE0,                   // mov eax, cr4
    0x83, 0xC8, 0x20,                   // or eax, 0x20 (PAE)
    0x0F, 0x22, 0xE0,                   // mov cr4, eax
    0xA1, 0x40, 0x8F, 0x00, 0x00,       // mov eax, [0x8F40] (pml4_phys)
    0x0F, 0x22, 0xD8,                   // mov cr3, eax
    0xB9, 0x80, 0x00, 0x00, 0xC0,       // mov ecx, 0xC0000080 (IA32_EFER)
    0x0F, 0x32,                         // rdmsr
    0x0B, 0x05, 0x44, 0x8F, 0x00, 0x00, // or eax, [0x8F44] (LME | SCE | NXE?)
    0x0F, 0x30,                         // wrmsr
    0x0F, 0x20, 0xC0,                   // mov eax, cr0
    0x0D, 0x00, 0x00, 0x01, 0x80,       // or eax, 0x80010000 (PG | WP)
    0x0F, 0x22, 0xC0,                   // mov cr0, eax
    0xEA, 0x90, 0x80, 0x00, 0x00, 0x18, 0x00, // ljmp 0x18:0x8090
];

// 64-bit long mode stub @ 0x8090
#[rustfmt::skip]
const TRAMPOLINE_64: &[u8] = &[
    0x66, 0xB8, 0x20, 0x00,                   // mov ax, 0x20
    0x8E, 0xD8,                               // mov ds, ax
    0x8E, 0xC0,                               // mov es, ax
    0x8E, 0xD0,                               // mov ss, ax
    0x66, 0x31, 0xC0,                         // xor ax, ax
    0x8E, 0xE0,                               // mov fs, ax
    0x8E, 0xE8,                               // mov gs, ax
    0x48, 0x8B, 0x24, 0x25, 0x48, 0x8F, 0x00, 0x00, // mov rsp, [0x8F48]
    0x8B, 0x3C, 0x25, 0x50, 0x8F, 0x00, 0x00, // mov edi, [0x8F50] (cpu_id)
    0x31, 0xED,                               // xor ebp, ebp
    0x48, 0x8B, 0x04, 0x25, 0x58, 0x8F, 0x00, 0x00, // mov rax, [0x8F58]
    0xFF, 0xE0,                               // jmp rax
];

static SMP_INITIALIZED: AtomicBool = AtomicBool::new(false);
static AP_BOOT_ACK: AtomicU32 = AtomicU32::new(0);
static AP_CORES_BOOTED: AtomicUsize = AtomicUsize::new(0);

static TLB_SHOOTDOWN_ADDR: AtomicU64 = AtomicU64::new(0);
static TLB_SHOOTDOWN_ACKS: AtomicUsize = AtomicUsize::new(0);
static TLB_SHOOTDOWN_COUNT: AtomicU64 = AtomicU64::new(0);
static TLB_SHOOTDOWN_LOCK: Mutex<()> = Mutex::new(());

#[derive(Clone, Copy)]
pub struct SmpSummary {
    pub initialized: bool,
    pub online_cpus: usize,
    pub ap_booted: usize,
    pub tlb_shootdowns: u64,
}

fn delay_ms_pit_or_spin(ms: u64) {
    let start = timer::uptime_ms();
    let mut spins = 0u32;
    while timer::uptime_ms().saturating_sub(start) < ms && spins < 5_000_000 {
        spins += 1;
        core::hint::spin_loop();
    }
}

extern "C" fn ap_rust_entry(cpu_id: u32) -> ! {
    let cid = cpu_id as usize;
    unsafe {
        gdt::init_ap(cid);
    }
    crate::interrupts::load_idt_for_ap();
    let apic_id = apic::enable_ap_lapic_and_timer();

    let syscall_stack: &'static [u8] =
        alloc::boxed::Box::leak(alloc::vec![0u8; AP_SYSCALL_STACK_SIZE].into_boxed_slice());
    let syscall_top = ((syscall_stack.as_ptr() as u64) + AP_SYSCALL_STACK_SIZE as u64) & !0xF;
    percpu::init_ap(cid, apic_id, syscall_top);

    AP_BOOT_ACK.store(cpu_id, Ordering::Release);

    loop {
        crate::task::kthread::step_workers_on_cpu(cid, 4, 4);
        unsafe {
            asm!("sti", "hlt", options(nomem, nostack));
        }
    }
}

/// Wakes secondary Application Processors (APs) listed in ACPI MADT using
/// the x86 INIT-SIPI-SIPI protocol and a real->protected->long-mode trampoline at `0x8000`.
pub fn init_ap_cores(
    acpi: &AcpiPlatformInfo,
    mapping: &PhysicalMemoryMapping,
    allocator: &mut PhysicalFrameAllocator<'_>,
) -> SmpSummary {
    if SMP_INITIALIZED.swap(true, Ordering::AcqRel) {
        return summary();
    }

    let snap = apic::snapshot();
    if !snap.lapic_enabled || snap.lapic_virt == 0 || acpi.cpu_count <= 1 {
        return summary();
    }

    let mut target_apics: Vec<u32> = Vec::new();
    for i in 0..acpi.cpu_count.min(super::acpi::MAX_MADT_CPUS) {
        let cpu = acpi.cpus[i];
        let apic_id = cpu.apic_id as u32;
        if (cpu.enabled || cpu.online_capable)
            && apic_id != snap.bsp_apic_id
            && !target_apics.contains(&apic_id)
        {
            target_apics.push(apic_id);
            if target_apics.len() + 1 >= MAX_SMP_CPUS {
                break;
            }
        }
    }

    if target_apics.is_empty() {
        return summary();
    }

    let Some(tramp_virt) = mapping.translate_frame(TRAMPOLINE_PHYS) else {
        klog::log_warn("smp", "Cannot translate 0x8000 trampoline phys via HHDM");
        return summary();
    };
    let tramp_ptr = tramp_virt.as_u64() as *mut u8;

    let tramp_vaddr = VirtAddr::new(TRAMPOLINE_PHYS);
    let was_identity_mapped = mapping.translate_virtual_address(tramp_vaddr).is_some();
    if !was_identity_mapped {
        let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE;
        if mapping
            .map_page(allocator, tramp_vaddr, TRAMPOLINE_PHYS, flags)
            .is_err()
        {
            klog::log_warn("smp", "Failed to identity-map 0x8000 AP trampoline");
            return summary();
        }
    }

    gdt::prepare_ap_descriptors();

    let pml4_phys = vmm::active_level_4_table_address();
    if pml4_phys > u32::MAX as u64 {
        klog::log_warn("smp", "Kernel PML4 > 4GiB unsupported by 32-bit AP stub");
        if !was_identity_mapped {
            let _ = mapping.unmap_page(tramp_vaddr);
        }
        return summary();
    }

    let mut efer_mask: u32 = (1 << 8) | (1 << 0); // LME | SCE
    if crate::userland::syscall::nxe_enabled() {
        efer_mask |= 1 << 11; // NXE
    }

    unsafe {
        write_bytes(tramp_ptr, 0, PAGE_SIZE as usize);
        core::ptr::copy_nonoverlapping(TRAMPOLINE_16.as_ptr(), tramp_ptr, TRAMPOLINE_16.len());
        core::ptr::copy_nonoverlapping(
            TRAMPOLINE_32.as_ptr(),
            tramp_ptr.add(0x30),
            TRAMPOLINE_32.len(),
        );
        core::ptr::copy_nonoverlapping(
            TRAMPOLINE_64.as_ptr(),
            tramp_ptr.add(0x90),
            TRAMPOLINE_64.len(),
        );

        // GDTR at 0x8F00: limit=39 (5 descriptors), base=0x8F08
        let gdtr = tramp_ptr.add(OFF_GDT_PTR);
        (gdtr as *mut u16).write_unaligned((5 * 8 - 1) as u16);
        (gdtr.add(2) as *mut u32).write_unaligned((TRAMPOLINE_PHYS + OFF_GDT_ENTRIES as u64) as u32);

        let gdt_entries = tramp_ptr.add(OFF_GDT_ENTRIES) as *mut u64;
        gdt_entries.add(0).write(0x0000_0000_0000_0000); // 0x00 Null
        gdt_entries.add(1).write(0x00CF_9A00_0000_FFFF); // 0x08 32-bit Code
        gdt_entries.add(2).write(0x00CF_9200_0000_FFFF); // 0x10 32-bit Data
        gdt_entries.add(3).write(0x00AF_9A00_0000_FFFF); // 0x18 64-bit Code
        gdt_entries.add(4).write(0x00AF_9200_0000_FFFF); // 0x20 64-bit Data

        (tramp_ptr.add(OFF_PML4_PHYS) as *mut u32).write(pml4_phys as u32);
        (tramp_ptr.add(OFF_EFER_MASK) as *mut u32).write(efer_mask);
        (tramp_ptr.add(OFF_AP_ENTRY) as *mut u64).write(ap_rust_entry as *const () as u64);
    }

    let mut booted = 0usize;
    for (idx, &apic_id) in target_apics.iter().enumerate() {
        let cpu_id = idx + 1;
        let stack: &'static [u8] =
            alloc::boxed::Box::leak(alloc::vec![0u8; AP_BOOT_STACK_SIZE].into_boxed_slice());
        // Simulate SysV `call` return-address alignment (`RSP % 16 == 8` at function entry before `jmp rax`).
        let stack_top = (((stack.as_ptr() as u64) + AP_BOOT_STACK_SIZE as u64) & !0xF) - 8;

        percpu::register_apic_cpu(apic_id, cpu_id);
        AP_BOOT_ACK.store(0, Ordering::Release);

        unsafe {
            (tramp_ptr.add(OFF_AP_STACK) as *mut u64).write(stack_top);
            (tramp_ptr.add(OFF_AP_CPU_ID) as *mut u32).write(cpu_id as u32);
        }
        fence(Ordering::SeqCst);

        unsafe {
            apic::send_init_ipi(apic_id);
        }
        delay_ms_pit_or_spin(10);

        for _attempt in 0..2 {
            unsafe {
                apic::send_startup_ipi(apic_id, TRAMPOLINE_SIPI_VEC);
            }
            for _ in 0..40 {
                if AP_BOOT_ACK.load(Ordering::Acquire) == cpu_id as u32 {
                    break;
                }
                delay_ms_pit_or_spin(1);
            }
            if AP_BOOT_ACK.load(Ordering::Acquire) == cpu_id as u32 {
                break;
            }
        }

        if AP_BOOT_ACK.load(Ordering::Acquire) == cpu_id as u32 {
            booted += 1;
        } else {
            klog::log_fmt(
                KLogLevel::Warn,
                "smp",
                format_args!("AP cpu={} apic={} failed SIPI handshake", cpu_id, apic_id),
            );
        }
    }

    AP_CORES_BOOTED.store(booted, Ordering::Release);

    if !was_identity_mapped {
        let _ = mapping.unmap_page(tramp_vaddr);
    }

    klog::log_fmt(
        KLogLevel::Info,
        "smp",
        format_args!(
            "SMP bring-up complete: online={} CPUs (BSP + {} APs)",
            percpu::online_cpu_count(),
            booted
        ),
    );
    summary()
}

/// Broadcasts a synchronous TLB shootdown IPI (`0xF0`) to all other online CPUs
/// whenever a virtual page mapping is invalidated across cores.
pub fn tlb_shootdown_page(vaddr: u64) {
    let online = percpu::online_cpu_count();
    if online <= 1 {
        return;
    }

    // Avoid deadlock if invoked from an IRQ-disabled context
    let rflags: u64;
    unsafe {
        asm!("pushfq; pop {}", out(reg) rflags, options(nomem, preserves_flags));
    }
    if (rflags & (1 << 9)) == 0 {
        return;
    }

    let Some(_guard) = TLB_SHOOTDOWN_LOCK.try_lock() else {
        return;
    };

    let expected = online.saturating_sub(1);
    TLB_SHOOTDOWN_ADDR.store(vaddr, Ordering::Release);
    TLB_SHOOTDOWN_ACKS.store(0, Ordering::Release);
    fence(Ordering::SeqCst);

    apic::broadcast_ipi_excluding_self(IPI_TLB_SHOOTDOWN_VECTOR);

    let mut spins = 0u32;
    while TLB_SHOOTDOWN_ACKS.load(Ordering::Acquire) < expected && spins < 2_000_000 {
        spins += 1;
        core::hint::spin_loop();
    }
    TLB_SHOOTDOWN_COUNT.fetch_add(1, Ordering::Relaxed);
}

#[inline]
pub fn on_tlb_shootdown_ipi() {
    let addr = TLB_SHOOTDOWN_ADDR.load(Ordering::Acquire);
    if addr == u64::MAX {
        unsafe {
            let val: u64;
            asm!("mov {}, cr3", out(reg) val, options(nomem, nostack));
            asm!("mov cr3, {}", in(reg) val, options(nostack));
        }
    } else {
        unsafe {
            asm!("invlpg [{}]", in(reg) addr, options(nostack, preserves_flags));
        }
    }
    let cpu = percpu::current_cpu_id();
    percpu::record_ipi(cpu);
    percpu::record_tlb_shootdown(cpu);
    TLB_SHOOTDOWN_ACKS.fetch_add(1, Ordering::Release);
    apic::send_lapic_eoi();
}

pub fn broadcast_reschedule_ipi() {
    if percpu::online_cpu_count() > 1 {
        apic::broadcast_ipi_excluding_self(IPI_RESCHEDULE_VECTOR);
    }
}

#[inline]
pub fn on_reschedule_ipi() {
    let cpu = percpu::current_cpu_id();
    percpu::record_ipi(cpu);
    crate::task::preempt::set_need_resched_on_cpu(cpu);
    apic::send_lapic_eoi();
}

pub fn summary() -> SmpSummary {
    SmpSummary {
        initialized: SMP_INITIALIZED.load(Ordering::Acquire),
        online_cpus: percpu::online_cpu_count(),
        ap_booted: AP_CORES_BOOTED.load(Ordering::Acquire),
        tlb_shootdowns: TLB_SHOOTDOWN_COUNT.load(Ordering::Relaxed),
    }
}
