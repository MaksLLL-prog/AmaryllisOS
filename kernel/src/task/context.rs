
use core::arch::global_asm;
use core::ptr;

global_asm!(
    ".global task_context_switch",
    "task_context_switch:",

    // Save callee-saved registers.
    "push rbp",
    "push rbx",
    "push r12",
    "push r13",
    "push r14",
    "push r15",

    // Save current stack pointer.
    "mov [rdi], rsp",

    // Load next stack pointer.
    "mov rsp, rsi",

    // Restore registers.
    "pop r15",
    "pop r14",
    "pop r13",
    "pop r12",
    "pop rbx",
    "pop rbp",

    // Resume execution.
    "ret",
);

unsafe extern "C" {
    fn task_context_switch(
        old_rsp: *mut u64,
        new_rsp: u64,
    );
}

// Stack pointer of the kernel.
static mut KERNEL_RSP: u64 = 0;

// Pointer to the currently running task's saved RSP.
static mut CURRENT_TASK_RSP: *mut u64 = ptr::null_mut();

// Temporary compatibility storage for the old one-task API.
static mut LEGACY_TASK_RSP: u64 = 0;

/// Run or resume a task using its own saved RSP.
pub fn run_task(task_rsp: *mut u64) {
    if task_rsp.is_null() {
        return;
    }

    unsafe {
        CURRENT_TASK_RSP = task_rsp;

        let new_rsp = ptr::read(task_rsp);

        task_context_switch(
            ptr::addr_of_mut!(KERNEL_RSP),
            new_rsp,
        );
    }
}

/// Start the old demonstration task.
pub fn start_task(initial_rsp: u64) {
    unsafe {
        LEGACY_TASK_RSP = initial_rsp;
        run_task(ptr::addr_of_mut!(LEGACY_TASK_RSP));
    }
}

/// Resume the old demonstration task.
pub fn resume_task() {
    unsafe {
        run_task(ptr::addr_of_mut!(LEGACY_TASK_RSP));
    }
}

/// Return control to the kernel.
pub fn yield_to_kernel() {
    unsafe {
        let task_rsp = CURRENT_TASK_RSP;

        if task_rsp.is_null() {
            return;
        }

        task_context_switch(
            task_rsp,
            ptr::read(ptr::addr_of!(KERNEL_RSP)),
        );
    }
}
