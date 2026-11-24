use alloc::collections::VecDeque;
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::arch::{asm, global_asm};
use core::ptr::write_bytes;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use x86_64::structures::paging::PageTableFlags;
use x86_64::VirtAddr;

use crate::arch::percpu::{self, MAX_SMP_CPUS};
use crate::interrupts::timer;
use crate::klog::{self, KLogLevel};
use crate::memory::vmm::{PhysicalMemoryMapping, PAGE_SIZE};
use crate::memory::PhysicalFrameAllocator;
use crate::net::{dhcp, dns, http, icmp};
use crate::task::preempt::{self, PreemptMutex};
use crate::userland::sched::UserCpuContext;

pub const KTHREAD_STACK_SIZE: usize = 64 * 1024;
pub const KTHREAD_GUARD_SIZE: usize = PAGE_SIZE as usize;
pub const KTHREAD_SLOT_STRIDE: u64 = (KTHREAD_STACK_SIZE + KTHREAD_GUARD_SIZE) as u64;
/// Virtual address range inside PML4 index 3 (`SHARED_KERNEL_PML4_MASK`),
/// with a permanently unmapped 4 KiB guard page at the bottom of each 64 KiB stack slot.
pub const KTHREAD_STACK_VIRT_BASE: u64 = 0x0000_0180_0000_0000;
pub const MAX_KTHREADS: usize = 12;
pub const KTHREAD_QUANTUM_MS: u64 = 4;

pub const WAKE_BIT_NET: u64 = 1 << 0;
pub const WAKE_BIT_AUDIO: u64 = 1 << 1;
pub const WAKE_BIT_BLK: u64 = 1 << 2;

static mut KTHREAD_SAVED_KERNEL_RSP: [u64; MAX_SMP_CPUS] = [0; MAX_SMP_CPUS];

#[no_mangle]
extern "C" fn rust_kthread_current_saved_krsp() -> u64 {
    let cpu = percpu::current_cpu_id() % MAX_SMP_CPUS;
    unsafe { KTHREAD_SAVED_KERNEL_RSP[cpu] }
}

global_asm!(
    ".global kthread_switch",
    // rdi = old_rsp_ptr (*mut u64), rsi = new_rsp (u64), rdx = krsp_slot (*mut u64)
    "kthread_switch:",
    "push rbp",
    "push rbx",
    "push r12",
    "push r13",
    "push r14",
    "push r15",
    "mov [rdi], rsp",
    "mov [rdx], rsp",
    "mov rsp, rsi",
    "pop r15",
    "pop r14",
    "pop r13",
    "pop r12",
    "pop rbx",
    "pop rbp",
    "ret",
    "",
    ".global kthread_resume_irq_asm",
    // rdi = *mut u64 (old kernel rsp), rsi = *const UserCpuContext (20 qwords = 160 bytes), rdx = krsp_slot (*mut u64)
    "kthread_resume_irq_asm:",
    "push rbp",
    "push rbx",
    "push r12",
    "push r13",
    "push r14",
    "push r15",
    "mov [rdi], rsp",
    "mov [rdx], rsp",
    "cli",
    "sub rsp, 160",
    "mov rdi, rsp",
    "mov rcx, 20",
    "cld",
    "rep movsq",
    "pop r15",
    "pop r14",
    "pop r13",
    "pop r12",
    "pop rbp",
    "pop rbx",
    "pop r11",
    "pop r10",
    "pop r9",
    "pop r8",
    "pop rax",
    "pop rcx",
    "pop rdx",
    "pop rsi",
    "pop rdi",
    "iretq",
    "",
    ".global kthread_exit_preempt_trampoline",
    "kthread_exit_preempt_trampoline:",
    "cli",
    "cld",
    "mov ax, 0x10",
    "mov ds, ax",
    "mov es, ax",
    "and rsp, -16",
    "call rust_kthread_current_saved_krsp",
    "mov rsp, rax",
    "pop r15",
    "pop r14",
    "pop r13",
    "pop r12",
    "pop rbx",
    "pop rbp",
    "ret",
);

unsafe extern "C" {
    fn kthread_switch(old_rsp: *mut u64, new_rsp: u64, krsp_slot: *mut u64);
    fn kthread_resume_irq_asm(old_krsp: *mut u64, ctx: *const UserCpuContext, krsp_slot: *mut u64);
    fn kthread_exit_preempt_trampoline() -> !;
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum KThreadState {
    Ready,
    Running,
    Sleeping { wake_ms: u64 },
    Finished,
}

#[derive(Clone)]
pub struct KThreadSnapshot {
    pub id: usize,
    pub name: &'static str,
    pub state_label: &'static str,
    pub switches: u64,
    pub preempts: u64,
    pub cpu_ms: u64,
    pub guarded_stack: bool,
    pub stack_base: u64,
    pub last_cpu: usize,
    pub running_on_cpu: Option<usize>,
}

struct KThreadSlot {
    id: usize,
    name: &'static str,
    rsp: u64,
    _fallback_stack: Option<Vec<u8>>,
    guarded_stack: bool,
    stack_base: u64,
    was_irq_preempted: bool,
    preempt_ctx: UserCpuContext,
    entry: fn(),
    state: KThreadState,
    switches: u64,
    preempts: u64,
    cpu_ms: u64,
    running_on_cpu: Option<usize>,
    last_cpu: usize,
}

struct KThreadRuntime {
    slots: [Option<KThreadSlot>; MAX_KTHREADS],
    current_idx: [Option<usize>; MAX_SMP_CPUS],
    kernel_rsp: [u64; MAX_SMP_CPUS],
    rr_cursor: usize,
    next_id: usize,
    run_start_ms: [u64; MAX_SMP_CPUS],
    quantum_deadline_ms: [u64; MAX_SMP_CPUS],
    workers_bootstrapped: bool,
}

impl KThreadRuntime {
    const fn new() -> Self {
        Self {
            slots: [const { None }; MAX_KTHREADS],
            current_idx: [None; MAX_SMP_CPUS],
            kernel_rsp: [0; MAX_SMP_CPUS],
            rr_cursor: 0,
            next_id: 1,
            run_start_ms: [0; MAX_SMP_CPUS],
            quantum_deadline_ms: [0; MAX_SMP_CPUS],
            workers_bootstrapped: false,
        }
    }
}

static KTHREADS: PreemptMutex<KThreadRuntime> = PreemptMutex::new(KThreadRuntime::new());
static IN_KTHREAD: [AtomicBool; MAX_SMP_CPUS] = [const { AtomicBool::new(false) }; MAX_SMP_CPUS];
static GUARDED_STACKS_READY: AtomicBool = AtomicBool::new(false);
static TOTAL_KTHREAD_PREEMPTS: AtomicU64 = AtomicU64::new(0);
static PENDING_WAKE_BITS: AtomicU64 = AtomicU64::new(0);

#[inline]
fn cpu_slot() -> usize {
    percpu::current_cpu_id() % MAX_SMP_CPUS
}

#[inline]
fn interrupts_enabled() -> bool {
    let rflags: u64;
    unsafe {
        asm!(
            "pushfq",
            "pop {}",
            out(reg) rflags,
            options(nomem, preserves_flags)
        );
    }
    (rflags & (1 << 9)) != 0
}

#[inline]
pub fn slot_guard_addr(slot_idx: usize) -> u64 {
    KTHREAD_STACK_VIRT_BASE + (slot_idx as u64) * KTHREAD_SLOT_STRIDE
}

#[inline]
pub fn slot_stack_bottom(slot_idx: usize) -> u64 {
    slot_guard_addr(slot_idx) + KTHREAD_GUARD_SIZE as u64
}

#[inline]
pub fn slot_stack_top(slot_idx: usize) -> u64 {
    slot_stack_bottom(slot_idx) + KTHREAD_STACK_SIZE as u64
}

/// Allocates and maps `MAX_KTHREADS` 64 KiB kernel worker stacks inside PML4 index 3,
/// leaving a 4 KiB unmapped hardware guard page below each stack.
pub fn init_guarded_stacks(
    mapping: &PhysicalMemoryMapping,
    allocator: &mut PhysicalFrameAllocator<'_>,
) -> bool {
    if GUARDED_STACKS_READY.load(Ordering::Acquire) {
        return true;
    }

    let pages_per_stack = KTHREAD_STACK_SIZE / PAGE_SIZE as usize;
    let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::NO_EXECUTE;

    for slot_idx in 0..MAX_KTHREADS {
        let base = slot_stack_bottom(slot_idx);
        for p in 0..pages_per_stack {
            let vaddr = base + (p as u64) * PAGE_SIZE;
            let virt = VirtAddr::new(vaddr);
            if mapping.translate_virtual_address(virt).is_some() {
                continue;
            }
            let Some(frame) = allocator.allocate_frame() else {
                return false;
            };
            let Some(hhdm) = mapping.translate_frame(frame) else {
                let _ = allocator.deallocate_frame(frame);
                return false;
            };
            unsafe {
                write_bytes(hhdm.as_u64() as *mut u8, 0, PAGE_SIZE as usize);
            }
            if mapping.map_page(allocator, virt, frame, flags).is_err() {
                let _ = allocator.deallocate_frame(frame);
                return false;
            }
        }
    }

    GUARDED_STACKS_READY.store(true, Ordering::Release);
    klog::log_fmt(
        KLogLevel::Mm,
        "kthread",
        format_args!(
            "Guarded stacks armed: {} slots x {}KiB + 4KiB guard @ 0x{:x}",
            MAX_KTHREADS,
            KTHREAD_STACK_SIZE / 1024,
            KTHREAD_STACK_VIRT_BASE
        ),
    );
    true
}

pub fn are_guarded_stacks_active() -> bool {
    GUARDED_STACKS_READY.load(Ordering::Acquire)
}

pub fn total_preempt_count() -> u64 {
    TOTAL_KTHREAD_PREEMPTS.load(Ordering::Relaxed)
}

/// Lock-free IRQ-safe wakeup bit signal for `IOWorkLoop` worker kthreads.
#[inline]
pub fn signal_worker_wake(mask: u64) {
    PENDING_WAKE_BITS.fetch_or(mask, Ordering::Release);
    crate::arch::smp::broadcast_reschedule_ipi();
}

fn apply_pending_wakes(rt: &mut KThreadRuntime) {
    let bits = PENDING_WAKE_BITS.swap(0, Ordering::AcqRel);
    if bits == 0 {
        return;
    }
    for slot in rt.slots.iter_mut().flatten() {
        let match_wake = ((bits & WAKE_BIT_NET) != 0 && slot.name == "knet_worker")
            || ((bits & WAKE_BIT_AUDIO) != 0 && slot.name == "kiokit_audio")
            || ((bits & WAKE_BIT_BLK) != 0 && slot.name == "kiokit_blk");
        if match_wake && matches!(slot.state, KThreadState::Sleeping { .. }) {
            slot.state = KThreadState::Ready;
        }
    }
}

/// Checks whether a `#PF` address `cr2` belongs to one of the unmapped `kthread` guard pages.
pub fn is_guard_page_addr(cr2: u64) -> Option<(usize, &'static str)> {
    if !GUARDED_STACKS_READY.load(Ordering::Acquire) {
        return None;
    }
    for slot_idx in 0..MAX_KTHREADS {
        let guard_start = slot_guard_addr(slot_idx);
        let guard_end = guard_start + KTHREAD_GUARD_SIZE as u64;
        if cr2 >= guard_start && cr2 < guard_end {
            if let Some(rt) = KTHREADS.try_lock_irq() {
                if let Some(ref slot) = rt.slots[slot_idx] {
                    return Some((slot.id, slot.name));
                }
            }
            return Some((slot_idx, "kthread"));
        }
    }
    None
}

pub unsafe fn abort_current_guard_fault(cr2: u64, rip: u64) -> ! {
    unsafe {
        asm!("cli", "cld", options(nomem, nostack));
    }
    let cpu = cpu_slot();
    if let Some(mut rt) = KTHREADS.try_lock_irq() {
        if let Some(idx) = rt.current_idx[cpu] {
            if let Some(ref mut slot) = rt.slots[idx] {
                slot.state = KThreadState::Finished;
                slot.running_on_cpu = None;
                let summary = format!(
                    "kthread #{} '{}' (CPU{}) overflowed its 64 KiB stack into guard page 0x{:x}.",
                    slot.id, slot.name, cpu, cr2
                );
                let hint = format!(
                    "Worker terminated safely without crashing the desktop (RIP=0x{:x}).",
                    rip
                );
                let tag = format!("KTHREAD_GUARD_HIT id={}", slot.id);
                crate::graphics::alerts::report_system_error(
                    crate::graphics::alerts::AlertSeverity::CriticalFault,
                    crate::graphics::alerts::AlertSubsystem::Memory,
                    &tag,
                    "Kernel Thread Stack Guard Triggered",
                    &summary,
                    &hint,
                    true,
                );
            }
        }
    }
    IN_KTHREAD[cpu].store(false, Ordering::Release);
    preempt::clear_need_resched();
    unsafe {
        kthread_exit_preempt_trampoline();
    }
}

extern "C" fn kthread_entry_trampoline() -> ! {
    // Ensure interrupts are enabled when a newly launched kthread starts running
    // so APIC timer IRQs can preempt CPU-bound kernel threads cleanly on BSP and APs.
    unsafe {
        asm!("sti", options(nomem, nostack));
    }

    let cpu = cpu_slot();
    let entry_fn = {
        let rt = KTHREADS.lock();
        let idx = rt.current_idx[cpu].expect("kthread entry without active slot");
        rt.slots[idx].as_ref().expect("missing kthread slot").entry
    };

    entry_fn();

    let finished_name = {
        let mut rt = KTHREADS.lock();
        if let Some(idx) = rt.current_idx[cpu] {
            if let Some(ref mut slot) = rt.slots[idx] {
                slot.state = KThreadState::Finished;
                slot.running_on_cpu = None;
                Some((slot.id, slot.name))
            } else {
                None
            }
        } else {
            None
        }
    };

    if let Some((id, name)) = finished_name {
        klog::log_fmt(
            KLogLevel::Info,
            "kthread",
            format_args!("kthread #{} '{}' finished on CPU{}", id, name, cpu),
        );
    }

    yield_to_scheduler();
    loop {
        core::hint::spin_loop();
    }
}

#[inline]
pub fn is_in_kthread() -> bool {
    IN_KTHREAD[cpu_slot()].load(Ordering::Acquire)
}

pub fn spawn_kthread(name: &'static str, entry: fn()) -> Option<usize> {
    let guarded = GUARDED_STACKS_READY.load(Ordering::Acquire);
    let mut rt = KTHREADS.lock();
    let empty_idx = rt.slots.iter().position(|s| s.is_none())?;

    let (top, fallback_stack, stack_base) = if guarded {
        let base = slot_stack_bottom(empty_idx);
        let top = (slot_stack_top(empty_idx) as usize) & !0xF;
        (top, None, base)
    } else {
        let stack = vec![0u8; KTHREAD_STACK_SIZE];
        let base = stack.as_ptr() as u64;
        let top = (base as usize + KTHREAD_STACK_SIZE) & !0xF;
        (top, Some(stack), base)
    };

    let initial_rsp = top - 64;
    unsafe {
        let frame = initial_rsp as *mut u64;
        for i in 0..6 {
            frame.add(i).write(0);
        }
        frame.add(6).write(kthread_entry_trampoline as *const () as u64);
        frame.add(7).write(0);
    }

    let id = rt.next_id;
    rt.next_id += 1;

    rt.slots[empty_idx] = Some(KThreadSlot {
        id,
        name,
        rsp: initial_rsp as u64,
        _fallback_stack: fallback_stack,
        guarded_stack: guarded,
        stack_base,
        was_irq_preempted: false,
        preempt_ctx: UserCpuContext::default(),
        entry,
        state: KThreadState::Ready,
        switches: 0,
        preempts: 0,
        cpu_ms: 0,
        running_on_cpu: None,
        last_cpu: 0,
    });

    drop(rt);
    klog::log_fmt(
        KLogLevel::Info,
        "kthread",
        format_args!(
            "spawned #{} '{}' guarded={} stack=0x{:x}",
            id, name, guarded, stack_base
        ),
    );
    Some(id)
}

/// Called from APIC periodic timer IRQ (`0x40`, `IF = 0`) when interrupting Ring 0 kthread code
/// on any online CPU (`BSP` or `AP`).
pub fn on_apic_irq_preempt(ctx: *mut UserCpuContext, now_ms: u64) -> u64 {
    let cpu = cpu_slot();
    if !IN_KTHREAD[cpu].load(Ordering::Acquire) || ctx.is_null() {
        return 0;
    }

    if !preempt::is_preemptible() {
        preempt::set_need_resched();
        return 0;
    }

    let Some(mut rt) = KTHREADS.try_lock_irq() else {
        preempt::set_need_resched();
        return 0;
    };

    if now_ms < rt.quantum_deadline_ms[cpu] {
        return 0;
    }

    let Some(cur_idx) = rt.current_idx[cpu] else {
        return 0;
    };
    let start_ms = rt.run_start_ms[cpu];

    if let Some(ref mut slot) = rt.slots[cur_idx] {
        if slot.state == KThreadState::Running {
            slot.preempt_ctx = unsafe { *ctx };
            slot.preempt_ctx.rflags = (slot.preempt_ctx.rflags & 0x0020_0CD5) | 0x0202;
            slot.was_irq_preempted = true;
            slot.state = KThreadState::Ready;
            slot.running_on_cpu = None;
            slot.preempts = slot.preempts.saturating_add(1);
            slot.cpu_ms = slot.cpu_ms.saturating_add(now_ms.saturating_sub(start_ms).max(1));
            TOTAL_KTHREAD_PREEMPTS.fetch_add(1, Ordering::Relaxed);
            klog::record_kthread_preempt();
            preempt::clear_need_resched();
            return 2;
        }
    }

    0
}

/// Cooperatively yields from the running kernel worker back to this CPU's scheduler loop.
pub fn yield_to_scheduler() {
    let cpu = cpu_slot();
    if !IN_KTHREAD[cpu].load(Ordering::Acquire) {
        return;
    }

    preempt::clear_need_resched();
    let now = timer::uptime_ms();
    let (old_rsp_ptr, kernel_rsp) = {
        let mut rt = KTHREADS.lock();
        let Some(idx) = rt.current_idx[cpu] else {
            IN_KTHREAD[cpu].store(false, Ordering::Release);
            return;
        };
        let elapsed = now.saturating_sub(rt.run_start_ms[cpu]);
        let krsp = rt.kernel_rsp[cpu];
        let slot = rt.slots[idx].as_mut().expect("active kthread missing");
        if slot.state == KThreadState::Running {
            slot.state = KThreadState::Ready;
        }
        slot.running_on_cpu = None;
        slot.was_irq_preempted = false;
        slot.cpu_ms = slot.cpu_ms.saturating_add(elapsed);
        (&raw mut slot.rsp, krsp)
    };

    IN_KTHREAD[cpu].store(false, Ordering::Release);
    let krsp_slot = unsafe { (&raw mut KTHREAD_SAVED_KERNEL_RSP[cpu]) as *mut u64 };
    unsafe {
        kthread_switch(old_rsp_ptr, kernel_rsp, krsp_slot);
    }
}

/// Called from `timer::sleep_ms` when running inside a kernel worker thread so the caller CPU never stalls.
pub fn sleep_current_ms(ms: u64) {
    let cpu = cpu_slot();
    if !IN_KTHREAD[cpu].load(Ordering::Acquire) {
        return;
    }
    let wake_ms = timer::uptime_ms().saturating_add(ms);
    {
        let mut rt = KTHREADS.lock();
        if let Some(idx) = rt.current_idx[cpu] {
            if let Some(ref mut slot) = rt.slots[idx] {
                slot.state = KThreadState::Sleeping { wake_ms };
                slot.running_on_cpu = None;
            }
        }
    }
    yield_to_scheduler();
}

pub fn ensure_system_workers() {
    let need = {
        let mut rt = KTHREADS.lock();
        if rt.workers_bootstrapped {
            false
        } else {
            rt.workers_bootstrapped = true;
            true
        }
    };
    if need {
        let _ = spawn_kthread("knet_worker", net_worker_main);
    }
}

/// Steps up to `max_steps` ready kernel worker slices on the current CPU (`BSP` or `AP`).
#[inline]
pub fn step_workers(max_steps: usize, budget_ms: u64) -> usize {
    step_workers_on_cpu(percpu::current_cpu_id(), max_steps, budget_ms)
}

/// SMP work-stealing kthread scheduler loop callable simultaneously by BSP (`cpu_id=0`) and AP cores.
pub fn step_workers_on_cpu(cpu_id: usize, max_steps: usize, budget_ms: u64) -> usize {
    if cpu_id == 0 {
        ensure_system_workers();
    }

    let cpu = cpu_id % MAX_SMP_CPUS;
    let start_ms = timer::uptime_ms();
    let was_if = interrupts_enabled();
    let mut ran = 0usize;

    for _ in 0..max_steps {
        let now = timer::uptime_ms();
        if now.saturating_sub(start_ms) >= budget_ms {
            break;
        }

        enum PickMode {
            Cooperative {
                krsp_ptr: *mut u64,
                target_rsp: u64,
            },
            PreemptResume {
                krsp_ptr: *mut u64,
                ctx: UserCpuContext,
            },
        }

        let pick = {
            let mut rt = KTHREADS.lock();
            apply_pending_wakes(&mut rt);

            for slot in rt.slots.iter_mut().flatten() {
                if let KThreadState::Sleeping { wake_ms } = slot.state {
                    if now >= wake_ms {
                        slot.state = KThreadState::Ready;
                    }
                }
            }

            let start = rt.rr_cursor % MAX_KTHREADS;
            let mut chosen = None;
            for offset in 0..MAX_KTHREADS {
                let idx = (start + offset) % MAX_KTHREADS;
                if let Some(ref mut slot) = rt.slots[idx] {
                    if slot.state == KThreadState::Ready && slot.running_on_cpu.is_none() {
                        slot.state = KThreadState::Running;
                        slot.running_on_cpu = Some(cpu);
                        slot.last_cpu = cpu;
                        slot.switches = slot.switches.saturating_add(1);
                        let resume_irq = slot.was_irq_preempted;
                        let ctx = slot.preempt_ctx;
                        let target_rsp = slot.rsp;
                        slot.was_irq_preempted = false;

                        rt.rr_cursor = (idx + 1) % MAX_KTHREADS;
                        rt.current_idx[cpu] = Some(idx);
                        rt.run_start_ms[cpu] = now;
                        rt.quantum_deadline_ms[cpu] = now.saturating_add(KTHREAD_QUANTUM_MS);
                        chosen = Some(if resume_irq {
                            PickMode::PreemptResume {
                                krsp_ptr: &raw mut rt.kernel_rsp[cpu],
                                ctx,
                            }
                        } else {
                            PickMode::Cooperative {
                                krsp_ptr: &raw mut rt.kernel_rsp[cpu],
                                target_rsp,
                            }
                        });
                        break;
                    }
                }
            }
            chosen
        };

        let Some(mode) = pick else {
            break;
        };

        percpu::record_kthread_slice(cpu);
        preempt::clear_need_resched();
        IN_KTHREAD[cpu].store(true, Ordering::Release);
        let krsp_slot = unsafe { (&raw mut KTHREAD_SAVED_KERNEL_RSP[cpu]) as *mut u64 };
        match mode {
            PickMode::Cooperative { krsp_ptr, target_rsp } => unsafe {
                kthread_switch(krsp_ptr, target_rsp, krsp_slot);
            },
            PickMode::PreemptResume { krsp_ptr, ctx } => unsafe {
                kthread_resume_irq_asm(krsp_ptr, &ctx as *const UserCpuContext, krsp_slot);
            },
        }
        IN_KTHREAD[cpu].store(false, Ordering::Release);
        preempt::clear_need_resched();

        if was_if {
            unsafe {
                asm!("sti", options(nomem, nostack));
            }
        }

        {
            let mut rt = KTHREADS.lock();
            if let Some(done_idx) = rt.current_idx[cpu].take() {
                if let Some(ref mut slot) = rt.slots[done_idx] {
                    slot.running_on_cpu = None;
                    if slot.state == KThreadState::Finished {
                        rt.slots[done_idx] = None;
                    }
                }
            }
        }
        ran += 1;
    }

    ran
}

pub fn snapshot_kthreads() -> Vec<KThreadSnapshot> {
    let now = timer::uptime_ms();
    let rt = KTHREADS.lock();
    let mut out = Vec::new();
    for slot in rt.slots.iter().flatten() {
        let state_label = match slot.state {
            KThreadState::Running => "Running",
            KThreadState::Ready => "Ready",
            KThreadState::Sleeping { wake_ms } => {
                if now >= wake_ms {
                    "Waking"
                } else {
                    "Sleeping"
                }
            }
            KThreadState::Finished => "Finished",
        };
        out.push(KThreadSnapshot {
            id: slot.id,
            name: slot.name,
            state_label,
            switches: slot.switches,
            preempts: slot.preempts,
            cpu_ms: slot.cpu_ms,
            guarded_stack: slot.guarded_stack,
            stack_base: slot.stack_base,
            last_cpu: slot.last_cpu,
            running_on_cpu: slot.running_on_cpu,
        });
    }
    out
}

fn demo_cpu_worker_a() {
    let mut acc: u64 = 1;
    let end_ms = timer::uptime_ms().saturating_add(160);
    while timer::uptime_ms() < end_ms {
        acc = acc.wrapping_mul(6364136223846793005).wrapping_add(1);
        core::hint::black_box(acc);
    }
}

fn demo_cpu_worker_b() {
    let mut acc: u64 = 7;
    let end_ms = timer::uptime_ms().saturating_add(160);
    while timer::uptime_ms() < end_ms {
        acc = acc.rotate_left(7).wrapping_add(0x9E37);
        core::hint::black_box(acc);
    }
}

/// Spawns two pure CPU-bound kernel threads that never yield cooperatively,
/// proving hardware APIC timer preemption & SMP scheduling keep the system responsive.
pub fn spawn_preempt_demo_pair() -> (Option<usize>, Option<usize>) {
    let a = spawn_kthread("kcpu_demo_a", demo_cpu_worker_a);
    let b = spawn_kthread("kcpu_demo_b", demo_cpu_worker_b);
    crate::arch::smp::broadcast_reschedule_ipi();
    (a, b)
}

// ============================================================
// ASYNCHRONOUS KERNEL NETWORK JOBS (RUN ON `knet_worker` / IOAPIC IRQ)
// ============================================================

#[derive(Clone)]
pub enum NetJobRequest {
    Dhcp,
    Ping {
        target_ip: [u8; 4],
        seq: u16,
        timeout_ms: u64,
    },
    DnsResolve {
        host: String,
    },
    HttpGet {
        url: String,
        max_bytes: usize,
    },
}

#[derive(Clone)]
pub enum NetJobOutcome {
    Dhcp(Result<dhcp::DhcpLease, &'static str>),
    Ping(Result<icmp::PingReply, &'static str>),
    DnsResolve(Result<[u8; 4], &'static str>),
    HttpGet {
        result: Result<([u8; 4], usize, Vec<u8>), String>,
    },
}

struct PendingNetJob {
    id: u64,
    req: NetJobRequest,
}

static NEXT_NET_JOB_ID: AtomicU64 = AtomicU64::new(1);
static NET_JOB_QUEUE: PreemptMutex<VecDeque<PendingNetJob>> = PreemptMutex::new(VecDeque::new());
static NET_JOB_RESULTS: PreemptMutex<Vec<(u64, NetJobOutcome)>> = PreemptMutex::new(Vec::new());

pub fn submit_net_job(req: NetJobRequest) -> u64 {
    ensure_system_workers();
    let id = NEXT_NET_JOB_ID.fetch_add(1, Ordering::Relaxed);
    {
        let mut q = NET_JOB_QUEUE.lock();
        if q.len() >= 24 {
            q.pop_front();
        }
        q.push_back(PendingNetJob { id, req });
    }
    signal_worker_wake(WAKE_BIT_NET);
    id
}

pub fn take_net_job_result(job_id: u64) -> Option<NetJobOutcome> {
    let mut results = NET_JOB_RESULTS.lock();
    if let Some(pos) = results.iter().position(|(id, _)| *id == job_id) {
        Some(results.remove(pos).1)
    } else {
        None
    }
}

pub fn has_pending_net_jobs() -> bool {
    !NET_JOB_QUEUE.lock().is_empty()
}

fn net_worker_main() {
    loop {
        let next_job = NET_JOB_QUEUE.lock().pop_front();
        if let Some(job) = next_job {
            let outcome = match job.req {
                NetJobRequest::Dhcp => NetJobOutcome::Dhcp(dhcp::discover_and_configure()),
                NetJobRequest::Ping {
                    target_ip,
                    seq,
                    timeout_ms,
                } => NetJobOutcome::Ping(icmp::send_echo_and_wait(target_ip, 0x414D, seq, timeout_ms)),
                NetJobRequest::DnsResolve { host } => NetJobOutcome::DnsResolve(dns::resolve_a(&host)),
                NetJobRequest::HttpGet { url, max_bytes } => {
                    let mut body = Vec::new();
                    let cap = max_bytes.min(64 * 1024);
                    let res = http::http_get(&url, &mut |chunk| {
                        let avail = cap.saturating_sub(body.len());
                        let take = chunk.len().min(avail);
                        body.extend_from_slice(&chunk[..take]);
                    });
                    match res {
                        Ok((peer_ip, total)) => NetJobOutcome::HttpGet {
                            result: Ok((peer_ip, total, body)),
                        },
                        Err(e) => NetJobOutcome::HttpGet {
                            result: Err(format!("{}", e)),
                        },
                    }
                }
            };

            let mut results = NET_JOB_RESULTS.lock();
            if results.len() >= 32 {
                results.remove(0);
            }
            results.push((job.id, outcome));
            yield_to_scheduler();
        } else {
            // IOAPIC interrupt wakes knet_worker immediately when packets arrive;
            // long 100 ms fallback prevents busy polling while staying resilient without IOAPIC.
            let drained = crate::net::poll_rx();
            if drained > 0 {
                yield_to_scheduler();
            } else {
                sleep_current_ms(100);
            }
        }
    }
}
