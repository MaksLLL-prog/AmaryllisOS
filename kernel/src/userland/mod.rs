pub mod demo;
pub mod elf;
pub mod freebsd;
pub mod ipc;
pub mod runner;
pub mod sched;
pub mod syscall;

pub fn init_fpu_sse() {
    unsafe {
        let mut cr0: u64;
        core::arch::asm!("mov {}, cr0", out(reg) cr0, options(nomem, nostack, preserves_flags));
        // Clear EM (bit 2) and TS (bit 3); set MP (bit 1) and NE (bit 5).
        cr0 &= !((1u64 << 2) | (1u64 << 3));
        cr0 |= (1u64 << 1) | (1u64 << 5);
        core::arch::asm!("mov cr0, {}", in(reg) cr0, options(nomem, nostack, preserves_flags));
        core::arch::asm!("fninit", options(nomem, nostack, preserves_flags));

        let features = core::arch::x86_64::__cpuid(1);
        // EDX bit 24 = FXSR, bit 25 = SSE
        if (features.edx & ((1 << 24) | (1 << 25))) == ((1 << 24) | (1 << 25)) {
            let mut cr4: u64;
            core::arch::asm!("mov {}, cr4", out(reg) cr4, options(nomem, nostack, preserves_flags));
            // Set OSFXSR (bit 9) and OSXMMEXCPT (bit 10)
            cr4 |= (1u64 << 9) | (1u64 << 10);
            core::arch::asm!("mov cr4, {}", in(reg) cr4, options(nomem, nostack, preserves_flags));
            let mxcsr: u32 = 0x1F80;
            core::arch::asm!("ldmxcsr [{}]", in(reg) &mxcsr, options(nostack, preserves_flags));
        }
    }
}

pub fn init() {
    init_fpu_sse();
    syscall::init();
}
