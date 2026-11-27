
mod pic;
pub mod timer;

use core::arch::asm;

use lazy_static::lazy_static;
use x86_64::registers::control::Cr2;
use x86_64::structures::idt::{
    InterruptDescriptorTable,
    InterruptStackFrame,
    PageFaultErrorCode,
};

lazy_static! {
    static ref IDT: InterruptDescriptorTable = {
        let mut idt = InterruptDescriptorTable::new();

        idt.divide_error.set_handler_fn(divide_error_handler);
        idt.debug.set_handler_fn(debug_handler);
        idt.non_maskable_interrupt
            .set_handler_fn(non_maskable_interrupt_handler);
        idt.breakpoint.set_handler_fn(breakpoint_handler);
        idt.overflow.set_handler_fn(overflow_handler);
        idt.bound_range_exceeded
            .set_handler_fn(bound_range_exceeded_handler);
        idt.invalid_opcode.set_handler_fn(invalid_opcode_handler);
        idt.device_not_available
            .set_handler_fn(device_not_available_handler);

        unsafe {
            idt.double_fault
                .set_handler_fn(double_fault_handler)
                .set_stack_index(crate::arch::gdt::DOUBLE_FAULT_IST_INDEX);
        }

        idt.invalid_tss.set_handler_fn(invalid_tss_handler);
        idt.segment_not_present
            .set_handler_fn(segment_not_present_handler);
        idt.stack_segment_fault
            .set_handler_fn(stack_segment_fault_handler);
        idt.general_protection_fault
            .set_handler_fn(general_protection_fault_handler);
        idt.page_fault.set_handler_fn(page_fault_handler);
        idt.x87_floating_point
            .set_handler_fn(x87_floating_point_handler);
        idt.alignment_check
            .set_handler_fn(alignment_check_handler);
        idt.machine_check.set_handler_fn(machine_check_handler);
        idt.simd_floating_point
            .set_handler_fn(simd_floating_point_handler);

        // IRQ0: 1000 Hz PIT timer (with Ring 3 preemption support).
        unsafe {
            idt[32].set_handler_addr(x86_64::VirtAddr::new(
                crate::userland::syscall::timer_irq0_entry as *const () as u64,
            ));
        }

        // IRQ1: keyboard.
        idt[33].set_handler_fn(keyboard_handler);

        // Local APIC Periodic Timer IRQ (vector 0x40 = 64).
        unsafe {
            idt[crate::arch::apic::APIC_TIMER_VECTOR as u8].set_handler_addr(
                x86_64::VirtAddr::new(
                    crate::userland::syscall::apic_timer_irq_entry as *const () as u64,
                ),
            );
        }

        // IRQ7: master PIC spurious interrupt guard.
        idt[39].set_handler_fn(spurious_irq7_handler);

        // IRQ12: PS/2 mouse.
        idt[44].set_handler_fn(mouse_handler);

        // IRQ15: slave PIC spurious interrupt guard.
        idt[47].set_handler_fn(spurious_irq15_handler);

        // IOAPIC PCI / Platform IRQ vectors (0x50..0x57) -> IOKit Top-Half
        idt[crate::arch::apic::IOAPIC_IRQ_VECTOR_BASE as u8]
            .set_handler_fn(ioapic_irq_50_handler);
        idt[crate::arch::apic::IOAPIC_IRQ_VECTOR_BASE as u8 + 1]
            .set_handler_fn(ioapic_irq_51_handler);
        idt[crate::arch::apic::IOAPIC_IRQ_VECTOR_BASE as u8 + 2]
            .set_handler_fn(ioapic_irq_52_handler);
        idt[crate::arch::apic::IOAPIC_IRQ_VECTOR_BASE as u8 + 3]
            .set_handler_fn(ioapic_irq_53_handler);
        idt[crate::arch::apic::IOAPIC_IRQ_VECTOR_BASE as u8 + 4]
            .set_handler_fn(ioapic_irq_54_handler);
        idt[crate::arch::apic::IOAPIC_IRQ_VECTOR_BASE as u8 + 5]
            .set_handler_fn(ioapic_irq_55_handler);
        idt[crate::arch::apic::IOAPIC_IRQ_VECTOR_BASE as u8 + 6]
            .set_handler_fn(ioapic_irq_56_handler);
        idt[crate::arch::apic::IOAPIC_IRQ_VECTOR_BASE as u8 + 7]
            .set_handler_fn(ioapic_irq_57_handler);

        // SMP Inter-Processor Interrupts (IPIs)
        idt[crate::arch::apic::IPI_TLB_SHOOTDOWN_VECTOR as u8]
            .set_handler_fn(ipi_tlb_shootdown_handler);
        idt[crate::arch::apic::IPI_RESCHEDULE_VECTOR as u8]
            .set_handler_fn(ipi_reschedule_handler);

        idt
    };
}

pub fn init() {
    unsafe {
        asm!("cli", options(nomem, nostack));
    }

    IDT.load();

    pic::initialize();
    timer::initialize();

    pic::enable_timer_and_keyboard();

    unsafe {
        asm!("sti", options(nomem, nostack));
    }
}

pub fn load_idt_for_ap() {
    IDT.load();
}

pub fn enable_mouse_irq() {
    pic::enable_mouse();
}

#[inline(always)]
unsafe fn ensure_kernel_segments() {
    asm!(
        "cld",
        "mov ax, 0x10",
        "mov ds, ax",
        "mov es, ax",
        out("ax") _,
        options(nomem, nostack)
    );
}

fn exception_name(vector: u8) -> &'static str {
    match vector {
        0 => "#DE Divide Error",
        1 => "#DB Debug",
        2 => "NMI",
        3 => "#BP Breakpoint",
        4 => "#OF Overflow",
        5 => "#BR Bound Range",
        6 => "#UD Invalid Opcode",
        7 => "#NM Device Not Avail",
        8 => "#DF Double Fault",
        10 => "#TS Invalid TSS",
        11 => "#NP Segment Not Present",
        12 => "#SS Stack Fault",
        13 => "#GP General Protection",
        14 => "#PF Page Fault",
        16 => "#MF x87 FP Fault",
        17 => "#AC Alignment Check",
        18 => "#MC Machine Check",
        19 => "#XM SIMD FP Fault",
        _ => "CPU Exception",
    }
}

fn handle_exception_or_halt(
    vector: u8,
    error_code: u64,
    cr2: u64,
    stack_frame: &InterruptStackFrame,
) -> ! {
    unsafe {
        ensure_kernel_segments();
    }

    let from_ring3 = (stack_frame.code_segment.0 & 3) == 3;

    if from_ring3 && crate::userland::syscall::is_user_execution_active() {
        unsafe {
            crate::userland::syscall::abort_from_user_fault(
                vector,
                error_code,
                stack_frame.instruction_pointer.as_u64(),
                stack_frame.stack_pointer.as_u64(),
                cr2,
            );
        }
    }

    use core::fmt::Write;
    let cpu = crate::arch::percpu::current_cpu_id();
    let rip = stack_frame.instruction_pointer.as_u64();
    let rsp = stack_frame.stack_pointer.as_u64();
    let cs = stack_frame.code_segment.0;
    let rflags = stack_frame.cpu_flags.bits();

    let mut l1 = crate::serial::FixedBuf::<96>::new();
    let mut l2 = crate::serial::FixedBuf::<96>::new();
    let _ = write!(
        l1,
        "vec={} ({}) err=0x{:x} cpu={}",
        vector,
        exception_name(vector),
        error_code,
        cpu
    );
    let _ = write!(
        l2,
        "rip=0x{:x} rsp=0x{:x} cr2=0x{:x} cs=0x{:x} rf=0x{:x}",
        rip, rsp, cr2, cs, rflags
    );
    crate::serial::panic_banner("KERNEL CPU EXCEPTION", l1.as_str(), l2.as_str());

    loop {
        unsafe {
            asm!("cli", "hlt", options(nomem, nostack));
        }
    }
}

extern "x86-interrupt" fn breakpoint_handler(
    _stack_frame: InterruptStackFrame,
) {
}

extern "x86-interrupt" fn divide_error_handler(
    stack_frame: InterruptStackFrame,
) {
    handle_exception_or_halt(0, 0, 0, &stack_frame);
}

extern "x86-interrupt" fn debug_handler(
    stack_frame: InterruptStackFrame,
) {
    handle_exception_or_halt(1, 0, 0, &stack_frame);
}

extern "x86-interrupt" fn non_maskable_interrupt_handler(
    stack_frame: InterruptStackFrame,
) {
    handle_exception_or_halt(2, 0, 0, &stack_frame);
}

extern "x86-interrupt" fn overflow_handler(
    stack_frame: InterruptStackFrame,
) {
    handle_exception_or_halt(4, 0, 0, &stack_frame);
}

extern "x86-interrupt" fn bound_range_exceeded_handler(
    stack_frame: InterruptStackFrame,
) {
    handle_exception_or_halt(5, 0, 0, &stack_frame);
}

extern "x86-interrupt" fn invalid_opcode_handler(
    stack_frame: InterruptStackFrame,
) {
    handle_exception_or_halt(6, 0, 0, &stack_frame);
}

extern "x86-interrupt" fn device_not_available_handler(
    stack_frame: InterruptStackFrame,
) {
    handle_exception_or_halt(7, 0, 0, &stack_frame);
}

extern "x86-interrupt" fn double_fault_handler(
    stack_frame: InterruptStackFrame,
    error_code: u64,
) -> ! {
    unsafe {
        ensure_kernel_segments();
    }
    use core::fmt::Write;
    let cr2 = Cr2::read_raw();
    let rip = stack_frame.instruction_pointer.as_u64();
    let rsp = stack_frame.stack_pointer.as_u64();
    let cs = stack_frame.code_segment.0;

    let mut l1 = crate::serial::FixedBuf::<96>::new();
    let mut l2 = crate::serial::FixedBuf::<96>::new();
    let _ = write!(
        l1,
        "#DF DOUBLE FAULT err=0x{:x} cs=0x{:x}",
        error_code, cs
    );
    let _ = write!(l2, "rip=0x{:x} rsp=0x{:x} cr2=0x{:x}", rip, rsp, cr2);
    crate::serial::panic_banner("KERNEL DOUBLE FAULT", l1.as_str(), l2.as_str());

    loop {
        unsafe {
            asm!("cli", "hlt", options(nomem, nostack));
        }
    }
}

extern "x86-interrupt" fn invalid_tss_handler(
    stack_frame: InterruptStackFrame,
    error_code: u64,
) {
    handle_exception_or_halt(10, error_code, 0, &stack_frame);
}

extern "x86-interrupt" fn segment_not_present_handler(
    stack_frame: InterruptStackFrame,
    error_code: u64,
) {
    handle_exception_or_halt(11, error_code, 0, &stack_frame);
}

extern "x86-interrupt" fn stack_segment_fault_handler(
    stack_frame: InterruptStackFrame,
    error_code: u64,
) {
    handle_exception_or_halt(12, error_code, 0, &stack_frame);
}

extern "x86-interrupt" fn general_protection_fault_handler(
    stack_frame: InterruptStackFrame,
    error_code: u64,
) {
    handle_exception_or_halt(13, error_code, 0, &stack_frame);
}

extern "x86-interrupt" fn page_fault_handler(
    stack_frame: InterruptStackFrame,
    error_code: PageFaultErrorCode,
) {
    unsafe {
        ensure_kernel_segments();
    }
    let cr2 = Cr2::read_raw();
    let from_ring3 = (stack_frame.code_segment.0 & 3) == 3;

    if from_ring3 && crate::userland::syscall::is_user_execution_active() {
        match crate::userland::syscall::try_handle_user_page_fault(cr2, error_code.bits()) {
            crate::userland::syscall::UserPfResolution::Resolved => {
                unsafe {
                    asm!(
                        "mov ax, 0x1B",
                        "mov ds, ax",
                        "mov es, ax",
                        out("ax") _,
                        options(nomem, nostack)
                    );
                }
                return;
            }
            crate::userland::syscall::UserPfResolution::GuardPageHit => {
                unsafe {
                    crate::userland::syscall::abort_from_user_fault(
                        14,
                        error_code.bits(),
                        stack_frame.instruction_pointer.as_u64(),
                        stack_frame.stack_pointer.as_u64(),
                        cr2,
                    );
                }
            }
            crate::userland::syscall::UserPfResolution::Invalid => {
                crate::klog::record_pf_user_kill();
                crate::klog::log_fmt(
                    crate::klog::KLogLevel::Fault,
                    "pf",
                    format_args!(
                        "Ring3 #PF kill cr2=0x{:x} rip=0x{:x} err=0x{:x}",
                        cr2,
                        stack_frame.instruction_pointer.as_u64(),
                        error_code.bits()
                    ),
                );
            }
        }
    } else if !from_ring3 {
        if let Some((kid, kname)) = crate::task::kthread::is_guard_page_addr(cr2) {
            crate::klog::record_pf_guard_fault();
            crate::klog::log_fmt(
                crate::klog::KLogLevel::Fault,
                "pf",
                format_args!(
                    "KThread #{} '{}' stack guard hit cr2=0x{:x} rip=0x{:x}",
                    kid,
                    kname,
                    cr2,
                    stack_frame.instruction_pointer.as_u64()
                ),
            );
            if crate::task::kthread::is_in_kthread() {
                unsafe {
                    crate::task::kthread::abort_current_guard_fault(
                        cr2,
                        stack_frame.instruction_pointer.as_u64(),
                    );
                }
            }
        } else {
            crate::klog::log_fmt(
                crate::klog::KLogLevel::Fault,
                "pf",
                format_args!(
                    "Kernel #PF cr2=0x{:x} rip=0x{:x} err=0x{:x}",
                    cr2,
                    stack_frame.instruction_pointer.as_u64(),
                    error_code.bits()
                ),
            );
        }
    }

    handle_exception_or_halt(14, error_code.bits(), cr2, &stack_frame);
}

extern "x86-interrupt" fn x87_floating_point_handler(
    stack_frame: InterruptStackFrame,
) {
    handle_exception_or_halt(16, 0, 0, &stack_frame);
}

extern "x86-interrupt" fn alignment_check_handler(
    stack_frame: InterruptStackFrame,
    error_code: u64,
) {
    handle_exception_or_halt(17, error_code, 0, &stack_frame);
}

extern "x86-interrupt" fn machine_check_handler(
    stack_frame: InterruptStackFrame,
) -> ! {
    handle_exception_or_halt(18, 0, 0, &stack_frame);
}

extern "x86-interrupt" fn simd_floating_point_handler(
    stack_frame: InterruptStackFrame,
) {
    handle_exception_or_halt(19, 0, 0, &stack_frame);
}

extern "x86-interrupt" fn keyboard_handler(
    _stack_frame: InterruptStackFrame,
) {
    unsafe {
        asm!("cld", options(nomem, nostack));
    }
    crate::input::handle_interrupt();

    pic::send_eoi(1);
}

extern "x86-interrupt" fn spurious_irq7_handler(
    _stack_frame: InterruptStackFrame,
) {
    unsafe {
        asm!("cld", options(nomem, nostack));
    }
    pic::handle_irq7_or_spurious();
}

extern "x86-interrupt" fn mouse_handler(
    _stack_frame: InterruptStackFrame,
) {
    unsafe {
        asm!("cld", options(nomem, nostack));
    }
    crate::input::mouse::handle_interrupt();

    pic::send_eoi(12);
}

extern "x86-interrupt" fn spurious_irq15_handler(
    _stack_frame: InterruptStackFrame,
) {
    unsafe {
        asm!("cld", options(nomem, nostack));
    }
    pic::handle_irq15_or_spurious();
}

#[inline(always)]
fn dispatch_ioapic_vector(vector: u8) {
    unsafe {
        asm!("cld", options(nomem, nostack));
    }
    crate::drivers::iokit::handle_hardware_irq(vector);
    crate::arch::apic::send_lapic_eoi();
}

extern "x86-interrupt" fn ioapic_irq_50_handler(_stack_frame: InterruptStackFrame) {
    dispatch_ioapic_vector(crate::arch::apic::IOAPIC_IRQ_VECTOR_BASE);
}
extern "x86-interrupt" fn ioapic_irq_51_handler(_stack_frame: InterruptStackFrame) {
    dispatch_ioapic_vector(crate::arch::apic::IOAPIC_IRQ_VECTOR_BASE + 1);
}
extern "x86-interrupt" fn ioapic_irq_52_handler(_stack_frame: InterruptStackFrame) {
    dispatch_ioapic_vector(crate::arch::apic::IOAPIC_IRQ_VECTOR_BASE + 2);
}
extern "x86-interrupt" fn ioapic_irq_53_handler(_stack_frame: InterruptStackFrame) {
    dispatch_ioapic_vector(crate::arch::apic::IOAPIC_IRQ_VECTOR_BASE + 3);
}
extern "x86-interrupt" fn ioapic_irq_54_handler(_stack_frame: InterruptStackFrame) {
    dispatch_ioapic_vector(crate::arch::apic::IOAPIC_IRQ_VECTOR_BASE + 4);
}
extern "x86-interrupt" fn ioapic_irq_55_handler(_stack_frame: InterruptStackFrame) {
    dispatch_ioapic_vector(crate::arch::apic::IOAPIC_IRQ_VECTOR_BASE + 5);
}
extern "x86-interrupt" fn ioapic_irq_56_handler(_stack_frame: InterruptStackFrame) {
    dispatch_ioapic_vector(crate::arch::apic::IOAPIC_IRQ_VECTOR_BASE + 6);
}
extern "x86-interrupt" fn ioapic_irq_57_handler(_stack_frame: InterruptStackFrame) {
    dispatch_ioapic_vector(crate::arch::apic::IOAPIC_IRQ_VECTOR_BASE + 7);
}

extern "x86-interrupt" fn ipi_tlb_shootdown_handler(_stack_frame: InterruptStackFrame) {
    unsafe {
        asm!("cld", options(nomem, nostack));
    }
    crate::arch::smp::on_tlb_shootdown_ipi();
}

extern "x86-interrupt" fn ipi_reschedule_handler(_stack_frame: InterruptStackFrame) {
    unsafe {
        asm!("cld", options(nomem, nostack));
    }
    crate::arch::smp::on_reschedule_ipi();
}
