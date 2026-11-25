use alloc::format;
use alloc::string::String;
use alloc::vec;
use core::arch::{asm, global_asm};
use core::ptr::{addr_of, copy_nonoverlapping};
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use x86_64::registers::control::{Cr3, Cr3Flags};
use x86_64::registers::model_specific::Msr;
use x86_64::structures::paging::{PageTableFlags, PhysFrame, Size4KiB};
use x86_64::{PhysAddr, VirtAddr};

use crate::console::Console;
use crate::graphics::Color;
use crate::interrupts::timer;
use crate::memory::vmm::{PhysicalMemoryMapping, PAGE_SIZE};
use crate::memory::PhysicalFrameAllocator;
use crate::storage::vfs::{self, VfsError, VfsNodeKind};
use crate::task::kthread::{self, NetJobRequest};

use super::ipc;
use super::sched::{
    self, FdEntry, FxSaveArea, NetWaitKind, OpenSocket, UserCpuContext, UserTaskState, WaitReason,
};

const WHITE: Color = (235, 235, 240);
const RED: Color = (220, 90, 90);

// Linux x86_64-compatible syscall numbers + AmaryllisOS POSIX/VFS/GUI/Net extensions
pub const SYS_READ: u64 = 0;
pub const SYS_WRITE: u64 = 1;
pub const SYS_OPEN: u64 = 2;
pub const SYS_CLOSE: u64 = 3;
pub const SYS_STAT: u64 = 4;
pub const SYS_FSTAT: u64 = 5;
pub const SYS_LSEEK: u64 = 8;
pub const SYS_MMAP: u64 = 9;
pub const SYS_MUNMAP: u64 = 11;
pub const SYS_BRK: u64 = 12;
pub const SYS_PIPE: u64 = 22;
pub const SYS_YIELD: u64 = 24;
pub const SYS_SLEEP_MS: u64 = 35;
pub const SYS_GETPID: u64 = 39;
pub const SYS_SOCKET: u64 = 41;
pub const SYS_CONNECT: u64 = 42;
pub const SYS_SEND: u64 = 44;
pub const SYS_RECV: u64 = 45;
pub const SYS_BIND: u64 = 49;
pub const SYS_EXIT: u64 = 60;
pub const SYS_GETDENTS: u64 = 78;
pub const SYS_MKDIR: u64 = 83;
pub const SYS_UNLINK: u64 = 87;
pub const SYS_UPTIME_MS: u64 = 96;
pub const SYS_BEEP: u64 = 200;
pub const SYS_NET_PING: u64 = 210;
pub const SYS_DNS_RESOLVE: u64 = 211;
pub const SYS_HTTP_GET: u64 = 212;
pub const SYS_PROC_COUNT: u64 = 220;
pub const SYS_CREATE_WINDOW: u64 = 300;
pub const SYS_WINDOW_BLIT: u64 = 301;
pub const SYS_POLL_GUI_EVENT: u64 = 302;
pub const SYS_CREATE_SHM_WINDOW: u64 = 303;
pub const SYS_COMMIT_SHM_WINDOW: u64 = 304;
pub const SYS_SPAWN_ELF: u64 = 310;
pub const SYS_SHM_CREATE: u64 = 320;
pub const SYS_SHM_MAP: u64 = 321;
pub const SYS_SHM_UNMAP: u64 = 322;

const ENOENT: i64 = -2;
const EIO: i64 = -5;
const EBADF: i64 = -9;
const EAGAIN: i64 = -11;
const ENOMEM: i64 = -12;
const EFAULT: i64 = -14;
const EEXIST: i64 = -17;
const ENOTDIR: i64 = -20;
const EISDIR: i64 = -21;
const EINVAL: i64 = -22;
const EMFILE: i64 = -24;
const EROFS: i64 = -30;
const EPIPE: i64 = -32;
const ENOSYS: i64 = -38;
const ETIMEDOUT: i64 = -110;
const ECONNREFUSED: i64 = -111;

const O_ACCMODE: u32 = 3;
const O_RDONLY: u32 = 0;
const O_WRONLY: u32 = 1;
const O_RDWR: u32 = 2;
const O_CREAT: u32 = 0x40;
const O_TRUNC: u32 = 0x200;
const O_APPEND: u32 = 0x400;
const O_DIRECTORY: u32 = 0x10000;

const IA32_EFER_MSR: u32 = 0xC000_0080;
const IA32_STAR_MSR: u32 = 0xC000_0081;
const IA32_LSTAR_MSR: u32 = 0xC000_0082;
const IA32_FMASK_MSR: u32 = 0xC000_0084;

const EFER_SCE: u64 = 1 << 0;
const EFER_NXE: u64 = 1 << 11;

const SYSCALL_STACK_SIZE: usize = 32 * 1024;

#[repr(align(16))]
struct AlignedSyscallStack([u8; SYSCALL_STACK_SIZE]);

static mut SYSCALL_KERNEL_STACK: AlignedSyscallStack =
    AlignedSyscallStack([0; SYSCALL_STACK_SIZE]);

#[no_mangle]
static mut SYSCALL_STACK_TOP: u64 = 0;
#[no_mangle]
static mut USER_SAVED_RSP: u64 = 0;
#[no_mangle]
static mut SAVED_KERNEL_RSP: u64 = 0;
#[no_mangle]
static mut ACTIVE_KERNEL_PML4: u64 = 0;
#[no_mangle]
static mut ACTIVE_USER_PML4: u64 = 0;

static SYSCALL_INITIALIZED: AtomicBool = AtomicBool::new(false);
static NXE_SUPPORTED_AND_ENABLED: AtomicBool = AtomicBool::new(false);
static USER_EXECUTION_ACTIVE: AtomicBool = AtomicBool::new(false);

static mut ACTIVE_CONSOLE_PTR: *mut Console<'static> = core::ptr::null_mut();
static mut ACTIVE_MAPPING_PTR: *const PhysicalMemoryMapping = core::ptr::null();
static mut ACTIVE_ALLOCATOR_PTR: *mut PhysicalFrameAllocator<'static> = core::ptr::null_mut();
static mut ACTIVE_PID: usize = 0;
static mut ACTIVE_CR3_FLAGS: u64 = 0;
static SYSCALL_COUNT: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy)]
pub struct UserFaultInfo {
    pub vector: u8,
    pub error_code: u64,
    pub rip: u64,
    pub rsp: u64,
    pub cr2: u64,
}

#[derive(Debug, Clone, Copy)]
pub enum UserTrapOutcome {
    Exited {
        exit_code: i64,
        syscalls_served: u64,
    },
    Faulted {
        fault: UserFaultInfo,
        syscalls_served: u64,
    },
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct SockAddrIn {
    pub ip: [u8; 4],
    pub port: u16,
    pub pad: u16,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct UserStat {
    pub size: u64,
    pub kind: u32,       // 1 = File, 2 = Directory
    pub attributes: u32,
    pub flags: u32,      // bit 0 = read_only
    pub reserved: u32,
}

#[repr(C)]
pub struct SyscallFrame {
    pub r15: u64,
    pub r14: u64,
    pub r13: u64,
    pub r12: u64,
    pub rbp: u64,
    pub rbx: u64,
    pub rax: u64,
    pub rdi: u64,
    pub rsi: u64,
    pub rdx: u64,
    pub r10: u64,
    pub r8: u64,
    pub r9: u64,
    pub user_rip: u64,
    pub user_rflags: u64,
    pub user_rsp: u64,
}

global_asm!(
    ".global syscall_entry",
    "syscall_entry:",
    "swapgs",
    "mov gs:[8], rsp",
    "mov rsp, gs:[0]",
    "push qword ptr gs:[8]",
    "push r11",
    "push rcx",
    "push r9",
    "push r8",
    "push r10",
    "push rdx",
    "push rsi",
    "push rdi",
    "push rax",
    "push rbx",
    "push rbp",
    "push r12",
    "push r13",
    "push r14",
    "push r15",
    "cld",
    "mov ax, 0x10",
    "mov ds, ax",
    "mov es, ax",
    "mov rdi, rsp",
    "call rust_syscall_dispatch",
    "pop r15",
    "pop r14",
    "pop r13",
    "pop r12",
    "pop rbp",
    "pop rbx",
    "pop rax",
    "pop rdi",
    "pop rsi",
    "pop rdx",
    "pop r10",
    "pop r8",
    "pop r9",
    "pop rcx",
    "pop r11",
    "push rax",
    "mov ax, 0x1B",
    "mov ds, ax",
    "mov es, ax",
    "pop rax",
    "pop rsp",
    "swapgs",
    "sysretq",
    "",
    ".global timer_irq0_entry",
    "timer_irq0_entry:",
    "push rdi",
    "push rsi",
    "push rdx",
    "push rcx",
    "push rax",
    "push r8",
    "push r9",
    "push r10",
    "push r11",
    "push rbx",
    "push rbp",
    "push r12",
    "push r13",
    "push r14",
    "push r15",
    "cld",
    "mov ax, 0x10",
    "mov ds, ax",
    "mov es, ax",
    "mov rdi, rsp",
    "call rust_timer_irq_dispatch",
    "test rax, rax",
    "jnz user_exit_return_trampoline",
    "test qword ptr [rsp + 128], 3",
    "jz 1f",
    "mov ax, 0x1B",
    "mov ds, ax",
    "mov es, ax",
    "1:",
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
    ".global apic_timer_irq_entry",
    "apic_timer_irq_entry:",
    "push rdi",
    "push rsi",
    "push rdx",
    "push rcx",
    "push rax",
    "push r8",
    "push r9",
    "push r10",
    "push r11",
    "push rbx",
    "push rbp",
    "push r12",
    "push r13",
    "push r14",
    "push r15",
    "cld",
    "mov ax, 0x10",
    "mov ds, ax",
    "mov es, ax",
    "mov rdi, rsp",
    "call rust_apic_timer_irq_dispatch",
    "cmp rax, 1",
    "je user_exit_return_trampoline",
    "cmp rax, 2",
    "je kthread_exit_preempt_trampoline",
    "test qword ptr [rsp + 128], 3",
    "jz 2f",
    "mov ax, 0x1B",
    "mov ds, ax",
    "mov es, ax",
    "2:",
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
    ".global resume_user_context_asm",
    // rdi = *const UserCpuContext (20 qwords = 160 bytes)
    "resume_user_context_asm:",
    "push rbx",
    "push rbp",
    "push r12",
    "push r13",
    "push r14",
    "push r15",
    "mov [rip + SAVED_KERNEL_RSP], rsp",
    "cli",
    "sub rsp, 160",
    "mov rsi, rdi",
    "mov rdi, rsp",
    "mov rcx, 20",
    "cld",
    "rep movsq",
    "mov rax, [rip + ACTIVE_USER_PML4]",
    "mov cr3, rax",
    "mov ax, [rsp + 152]",
    "mov ds, ax",
    "mov es, ax",
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
    ".global user_exit_return_trampoline",
    "user_exit_return_trampoline:",
    "cli",
    "cld",
    "mov rax, [rip + ACTIVE_KERNEL_PML4]",
    "mov cr3, rax",
    "mov ax, 0x10",
    "mov ds, ax",
    "mov es, ax",
    "mov rsp, [rip + SAVED_KERNEL_RSP]",
    "call rust_trampoline_normalize_gs",
    "pop r15",
    "pop r14",
    "pop r13",
    "pop r12",
    "pop rbp",
    "pop rbx",
    "ret",
);

unsafe extern "C" {
    fn syscall_entry();
    pub fn timer_irq0_entry();
    pub fn apic_timer_irq_entry();
    fn resume_user_context_asm(ctx: *const UserCpuContext);
    fn user_exit_return_trampoline() -> !;
}

#[no_mangle]
extern "C" fn rust_trampoline_normalize_gs() {
    crate::arch::percpu::swapgs_if_needed_on_fault();
}

pub fn init() {
    if SYSCALL_INITIALIZED.load(Ordering::Acquire) {
        return;
    }

    unsafe {
        let base = addr_of!(SYSCALL_KERNEL_STACK) as u64;
        let top = (base + SYSCALL_STACK_SIZE as u64) & !0xF;
        SYSCALL_STACK_TOP = top;
        crate::arch::percpu::init_bsp(top);
    }

    let nxe_supported = {
        let max_ext = core::arch::x86_64::__cpuid(0x8000_0000).eax;
        if max_ext >= 0x8000_0001 {
            let ext = core::arch::x86_64::__cpuid(0x8000_0001);
            (ext.edx & (1 << 20)) != 0
        } else {
            false
        }
    };

    unsafe {
        let mut efer_msr = Msr::new(IA32_EFER_MSR);
        let mut efer = efer_msr.read();
        efer |= EFER_SCE;
        if nxe_supported {
            efer |= EFER_NXE;
        }
        efer_msr.write(efer);
        NXE_SUPPORTED_AND_ENABLED.store(nxe_supported, Ordering::Release);

        let selectors = crate::arch::gdt::selectors();
        let star_val =
            ((selectors.kernel_data.0 as u64) << 48) | ((selectors.kernel_code.0 as u64) << 32);
        Msr::new(IA32_STAR_MSR).write(star_val);

        Msr::new(IA32_LSTAR_MSR).write(syscall_entry as *const () as u64);
        Msr::new(IA32_FMASK_MSR).write(0x0004_0700);
    }

    SYSCALL_INITIALIZED.store(true, Ordering::Release);
}

pub fn nxe_enabled() -> bool {
    NXE_SUPPORTED_AND_ENABLED.load(Ordering::Acquire)
}

pub fn is_user_execution_active() -> bool {
    USER_EXECUTION_ACTIVE.load(Ordering::Acquire)
}

pub unsafe fn exit_quantum_trampoline() -> ! {
    unsafe {
        user_exit_return_trampoline();
    }
}

pub unsafe fn abort_from_user_fault(
    vector: u8,
    error_code: u64,
    rip: u64,
    rsp: u64,
    cr2: u64,
) -> ! {
    unsafe {
        asm!(
            "cli",
            "cld",
            "mov ax, 0x10",
            "mov ds, ax",
            "mov es, ax",
            out("ax") _,
            options(nomem, nostack)
        );
        let kernel_pml4 = ACTIVE_KERNEL_PML4;
        let flags_bits = ACTIVE_CR3_FLAGS;
        if kernel_pml4 != 0 {
            switch_cr3_raw(kernel_pml4, flags_bits);
        }
        sched::abort_current_fault(UserFaultInfo {
            vector,
            error_code,
            rip,
            rsp,
            cr2,
        });
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum UserPfResolution {
    Resolved,
    GuardPageHit,
    Invalid,
}

pub fn try_handle_user_page_fault(cr2: u64, error_code: u64) -> UserPfResolution {
    if !USER_EXECUTION_ACTIVE.load(Ordering::Acquire) {
        return UserPfResolution::Invalid;
    }
    let (kernel_pml4, user_pml4, flags_bits) = unsafe {
        (ACTIVE_KERNEL_PML4, ACTIVE_USER_PML4, ACTIVE_CR3_FLAGS)
    };
    let mapping_opt = unsafe {
        if ACTIVE_MAPPING_PTR.is_null() {
            None
        } else {
            Some(&*ACTIVE_MAPPING_PTR)
        }
    };
    let allocator_opt = unsafe {
        if ACTIVE_ALLOCATOR_PTR.is_null() {
            None
        } else {
            Some(&mut *ACTIVE_ALLOCATOR_PTR)
        }
    };
    let (Some(mapping), Some(allocator)) = (mapping_opt, allocator_opt) else {
        return UserPfResolution::Invalid;
    };

    if kernel_pml4 != 0 {
        unsafe {
            switch_cr3_raw(kernel_pml4, flags_bits);
        }
    }

    let outcome = sched::try_resolve_user_page_fault(cr2, error_code, mapping, allocator);

    if user_pml4 != 0 && matches!(outcome, sched::FaultResolveOutcome::Resolved) {
        unsafe {
            switch_cr3_raw(user_pml4, flags_bits);
        }
    }

    match outcome {
        sched::FaultResolveOutcome::Resolved => UserPfResolution::Resolved,
        sched::FaultResolveOutcome::GuardPageHit => UserPfResolution::GuardPageHit,
        sched::FaultResolveOutcome::NotHandled => UserPfResolution::Invalid,
    }
}

#[no_mangle]
extern "C" fn rust_apic_timer_irq_dispatch(ctx: *mut UserCpuContext) -> u64 {
    crate::arch::apic::send_lapic_eoi();
    let cpu = crate::arch::percpu::current_cpu_id();
    crate::arch::percpu::record_timer_irq(cpu);
    crate::klog::record_apic_timer_irq();
    if ctx.is_null() {
        return 0;
    }
    let now_ms = if cpu == 0 { timer::on_lapic_tick() } else { timer::uptime_ms() };
    let cs = unsafe { (*ctx).cs };
    if (cs & 3) == 3 && cpu == 0 && USER_EXECUTION_ACTIVE.load(Ordering::Acquire) {
        return sched::on_timer_irq_preempt(ctx, now_ms);
    }
    if (cs & 3) == 0 && kthread::is_in_kthread() {
        return kthread::on_apic_irq_preempt(ctx, now_ms);
    }
    0
}

#[no_mangle]
extern "C" fn rust_timer_irq_dispatch(ctx: *mut UserCpuContext) -> u64 {
    let now_ms = timer::on_irq0_tick();
    if ctx.is_null() || crate::arch::apic::is_timer_active() {
        return 0;
    }
    let cs = unsafe { (*ctx).cs };
    if (cs & 3) == 3 && USER_EXECUTION_ACTIVE.load(Ordering::Acquire) {
        return sched::on_timer_irq_preempt(ctx, now_ms);
    }
    0
}

#[inline]
fn fxsr_supported() -> bool {
    (core::arch::x86_64::__cpuid(1).edx & (1 << 24)) != 0
}

pub unsafe fn run_user_quantum(
    console: &mut Console<'_>,
    mapping: &PhysicalMemoryMapping,
    allocator: &mut PhysicalFrameAllocator<'_>,
    pid: usize,
    user_pml4: u64,
    ctx: &UserCpuContext,
    user_fx: &mut FxSaveArea,
) {
    if !SYSCALL_INITIALIZED.load(Ordering::Acquire) {
        init();
    }

    let (kernel_frame, cr3_flags) = Cr3::read();
    let kernel_pml4 = kernel_frame.start_address().as_u64();

    struct ScopeGuard;
    impl Drop for ScopeGuard {
        fn drop(&mut self) {
            USER_EXECUTION_ACTIVE.store(false, Ordering::Release);
            unsafe {
                ACTIVE_CONSOLE_PTR = core::ptr::null_mut();
                ACTIVE_MAPPING_PTR = core::ptr::null();
                ACTIVE_ALLOCATOR_PTR = core::ptr::null_mut();
                ACTIVE_PID = 0;
            }
        }
    }

    unsafe {
        ACTIVE_KERNEL_PML4 = kernel_pml4;
        ACTIVE_USER_PML4 = user_pml4;
        ACTIVE_CR3_FLAGS = cr3_flags.bits();
        crate::arch::percpu::set_cr3_pair(kernel_pml4, user_pml4, cr3_flags.bits());
        ACTIVE_CONSOLE_PTR = (console as *mut Console<'_>).cast::<Console<'static>>();
        ACTIVE_MAPPING_PTR = mapping as *const PhysicalMemoryMapping;
        ACTIVE_ALLOCATOR_PTR =
            (allocator as *mut PhysicalFrameAllocator<'_>).cast::<PhysicalFrameAllocator<'static>>();
        ACTIVE_PID = pid;
        SYSCALL_COUNT.store(0, Ordering::Release);
    }

    let _guard = ScopeGuard;
    USER_EXECUTION_ACTIVE.store(true, Ordering::Release);

    let has_fx = fxsr_supported();
    let mut kernel_fx = FxSaveArea::new_clean();

    unsafe {
        if has_fx {
            asm!("fxsave64 [{}]", in(reg) kernel_fx.0.as_mut_ptr(), options(nostack));
            asm!("fxrstor64 [{}]", in(reg) user_fx.0.as_ptr(), options(nostack));
        }
        resume_user_context_asm(ctx as *const UserCpuContext);
        if has_fx {
            asm!("fxsave64 [{}]", in(reg) user_fx.0.as_mut_ptr(), options(nostack));
            asm!("fxrstor64 [{}]", in(reg) kernel_fx.0.as_ptr(), options(nostack));
        }
    }

    USER_EXECUTION_ACTIVE.store(false, Ordering::Release);
    unsafe {
        asm!("sti", options(nomem, nostack));
    }
}

#[inline]
unsafe fn switch_cr3_raw(pml4: u64, flags_bits: u64) {
    if let Ok(phys) = PhysAddr::try_new(pml4) {
        if let Ok(frame) = PhysFrame::<Size4KiB>::from_start_address(phys) {
            let flags = Cr3Flags::from_bits_truncate(flags_bits);
            unsafe {
                Cr3::write(frame, flags);
            }
        }
    }
}

fn validate_user_slice(
    mapping: &PhysicalMemoryMapping,
    allocator: Option<&mut PhysicalFrameAllocator<'_>>,
    user_pml4: u64,
    user_ptr: u64,
    len: usize,
    require_writable: bool,
) -> Result<(), i64> {
    if len == 0 {
        return Ok(());
    }
    if user_ptr == 0 {
        crate::klog::record_copy_user_fault();
        return Err(EFAULT);
    }

    let Some(end) = user_ptr.checked_add(len as u64) else {
        crate::klog::record_copy_user_fault();
        return Err(EFAULT);
    };
    if end > crate::userland::elf::USER_ADDR_LIMIT {
        crate::klog::record_copy_user_fault();
        crate::klog::log_fmt(
            crate::klog::KLogLevel::Warn,
            "uaccess",
            format_args!("Rejected kernel-space ptr=0x{:x} len={}", user_ptr, len),
        );
        return Err(EFAULT);
    }

    let first_page = user_ptr & !(PAGE_SIZE - 1);
    let last_page = (end - 1) & !(PAGE_SIZE - 1);

    let mut page = first_page;
    while page <= last_page {
        let p4 = ((page >> 39) & 0x1FF) as usize;
        if crate::memory::vmm::is_shared_kernel_entry(p4) {
            crate::klog::record_copy_user_fault();
            return Err(EFAULT);
        }
        page = page.saturating_add(PAGE_SIZE);
    }

    if sched::ensure_user_pages_committed(
        user_pml4,
        first_page,
        last_page,
        require_writable,
        mapping,
        allocator,
    )
    .is_err()
    {
        crate::klog::record_copy_user_fault();
        crate::klog::log_fmt(
            crate::klog::KLogLevel::Warn,
            "uaccess",
            format_args!(
                "copy_{}_user fault ptr=0x{:x} len={}",
                if require_writable { "to" } else { "from" },
                user_ptr,
                len
            ),
        );
        return Err(EFAULT);
    }

    Ok(())
}

fn copy_from_user_bytes(
    mapping: &PhysicalMemoryMapping,
    allocator: Option<&mut PhysicalFrameAllocator<'_>>,
    user_pml4: u64,
    user_ptr: u64,
    out: &mut [u8],
) -> Result<(), i64> {
    validate_user_slice(mapping, allocator, user_pml4, user_ptr, out.len(), false)?;

    let mut copied = 0usize;
    while copied < out.len() {
        let cur_vaddr = user_ptr + copied as u64;
        let page_offset = (cur_vaddr & (PAGE_SIZE - 1)) as usize;
        let step = (out.len() - copied).min(PAGE_SIZE as usize - page_offset);

        let virt = VirtAddr::new(cur_vaddr);
        let trans = mapping
            .translate_virtual_address_in(user_pml4, virt)
            .ok_or(EFAULT)?;
        let src_virt = mapping.translate(trans.physical_address).ok_or(EFAULT)?;

        unsafe {
            copy_nonoverlapping(src_virt.as_u64() as *const u8, out.as_mut_ptr().add(copied), step);
        }

        copied += step;
    }

    Ok(())
}

pub(crate) fn copy_to_user_bytes_in(
    mapping: &PhysicalMemoryMapping,
    allocator: Option<&mut PhysicalFrameAllocator<'_>>,
    user_pml4: u64,
    user_ptr: u64,
    src: &[u8],
) -> Result<(), i64> {
    copy_to_user_bytes(mapping, allocator, user_pml4, user_ptr, src)
}

fn copy_to_user_bytes(
    mapping: &PhysicalMemoryMapping,
    allocator: Option<&mut PhysicalFrameAllocator<'_>>,
    user_pml4: u64,
    user_ptr: u64,
    src: &[u8],
) -> Result<(), i64> {
    validate_user_slice(mapping, allocator, user_pml4, user_ptr, src.len(), true)?;

    let mut copied = 0usize;
    while copied < src.len() {
        let cur_vaddr = user_ptr + copied as u64;
        let page_offset = (cur_vaddr & (PAGE_SIZE - 1)) as usize;
        let step = (src.len() - copied).min(PAGE_SIZE as usize - page_offset);

        let virt = VirtAddr::new(cur_vaddr);
        let trans = mapping
            .translate_virtual_address_in(user_pml4, virt)
            .ok_or(EFAULT)?;
        let dst_virt = mapping.translate(trans.physical_address).ok_or(EFAULT)?;

        unsafe {
            copy_nonoverlapping(src.as_ptr().add(copied), dst_virt.as_u64() as *mut u8, step);
        }

        copied += step;
    }

    Ok(())
}

#[allow(dead_code)]
fn copy_str_from_user(
    mapping: &PhysicalMemoryMapping,
    allocator: Option<&mut PhysicalFrameAllocator<'_>>,
    user_pml4: u64,
    user_ptr: u64,
    max_len: usize,
) -> Result<String, i64> {
    let cap = max_len.min(256);
    if cap == 0 || user_ptr == 0 {
        return Err(EINVAL);
    }
    let mut buf = vec![0u8; cap];
    copy_from_user_bytes(mapping, allocator, user_pml4, user_ptr, &mut buf)?;
    let nul_pos = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    let s = core::str::from_utf8(&buf[..nul_pos]).map_err(|_| EINVAL)?;
    Ok(String::from(s))
}

fn vfs_err_to_errno(err: VfsError) -> i64 {
    match err {
        VfsError::NotFound => ENOENT,
        VfsError::AlreadyExists => EEXIST,
        VfsError::IsADirectory => EISDIR,
        VfsError::NotADirectory => ENOTDIR,
        VfsError::ReadOnlyFilesystem | VfsError::PermissionDenied => EROFS,
        VfsError::InvalidPath | VfsError::InvalidName | VfsError::OffsetOutOfRange => EINVAL,
        VfsError::NoSpace => ENOMEM,
        _ => EIO,
    }
}

fn read_user_path(
    mapping: &PhysicalMemoryMapping,
    allocator: Option<&mut PhysicalFrameAllocator<'_>>,
    user_pml4: u64,
    ptr: u64,
    len: usize,
) -> Result<String, i64> {
    if len == 0 || len > 240 {
        return Err(EINVAL);
    }
    let mut buf = [0u8; 240];
    copy_from_user_bytes(mapping, allocator, user_pml4, ptr, &mut buf[..len])?;
    let s = core::str::from_utf8(&buf[..len]).map_err(|_| EINVAL)?;
    Ok(String::from(s.trim()))
}

fn read_user_sockaddr(
    mapping: &PhysicalMemoryMapping,
    allocator: Option<&mut PhysicalFrameAllocator<'_>>,
    user_pml4: u64,
    ptr: u64,
) -> Result<SockAddrIn, i64> {
    let mut buf = [0u8; core::mem::size_of::<SockAddrIn>()];
    copy_from_user_bytes(mapping, allocator, user_pml4, ptr, &mut buf)?;
    Ok(unsafe { core::ptr::read_unaligned(buf.as_ptr() as *const SockAddrIn) })
}

fn write_user_sockaddr(
    mapping: &PhysicalMemoryMapping,
    allocator: Option<&mut PhysicalFrameAllocator<'_>>,
    user_pml4: u64,
    ptr: u64,
    addr: SockAddrIn,
) -> Result<(), i64> {
    let bytes = unsafe {
        core::slice::from_raw_parts(
            (&addr as *const SockAddrIn) as *const u8,
            core::mem::size_of::<SockAddrIn>(),
        )
    };
    copy_to_user_bytes(mapping, allocator, user_pml4, ptr, bytes)
}

fn read_console_line(console: &mut Console<'_>, out: &mut [u8]) -> usize {
    if out.is_empty() {
        return 0;
    }

    let mut len = 0usize;
    let mut shift = false;

    loop {
        if let Some(scancode) = crate::input::read_scancode() {
            if scancode == 0x2A || scancode == 0x36 {
                shift = true;
                continue;
            }
            if scancode == 0xAA || scancode == 0xB6 {
                shift = false;
                continue;
            }
            if scancode & 0x80 != 0 {
                continue;
            }

            if scancode == 0x0E {
                if len > 0 {
                    len -= 1;
                    console.backspace();
                }
                continue;
            }

            if scancode == 0x1C {
                console.write("\n", WHITE);
                if len < out.len() {
                    out[len] = b'\n';
                    len += 1;
                }
                break;
            }

            if let Some(ch) = crate::input::scancode_to_char(scancode, shift) {
                if ch.is_ascii() && !ch.is_ascii_control() && len < out.len() {
                    let b = ch as u8;
                    out[len] = b;
                    len += 1;
                    let s = [b];
                    if let Ok(txt) = core::str::from_utf8(&s) {
                        console.write(txt, WHITE);
                    }
                    if len == out.len() {
                        break;
                    }
                }
            }
        } else {
            unsafe {
                asm!("sti", "hlt", "cli", options(nomem, nostack));
            }
        }
    }

    len
}

#[inline]
fn build_resume_ctx_from_syscall(frame: &SyscallFrame, ret_val: i64) -> UserCpuContext {
    let selectors = crate::arch::gdt::selectors();
    UserCpuContext {
        r15: frame.r15,
        r14: frame.r14,
        r13: frame.r13,
        r12: frame.r12,
        rbp: frame.rbp,
        rbx: frame.rbx,
        r11: frame.user_rflags,
        r10: frame.r10,
        r9: frame.r9,
        r8: frame.r8,
        rax: ret_val as u64,
        rcx: frame.user_rip,
        rdx: frame.rdx,
        rsi: frame.rsi,
        rdi: frame.rdi,
        rip: frame.user_rip,
        cs: selectors.user_code.0 as u64,
        rflags: frame.user_rflags,
        rsp: frame.user_rsp,
        ss: selectors.user_data.0 as u64,
    }
}

#[no_mangle]
extern "C" fn rust_syscall_dispatch(frame: &mut SyscallFrame) {
    let (kernel_pml4, user_pml4, flags_bits) = unsafe {
        (ACTIVE_KERNEL_PML4, ACTIVE_USER_PML4, ACTIVE_CR3_FLAGS)
    };

    unsafe {
        switch_cr3_raw(kernel_pml4, flags_bits);
    }

    SYSCALL_COUNT.fetch_add(1, Ordering::Relaxed);
    sched::record_current_syscall();

    if frame.user_rip == 0 || frame.user_rip >= crate::userland::elf::USER_ADDR_LIMIT {
        unsafe {
            abort_from_user_fault(13, 0, frame.user_rip, frame.user_rsp, 0);
        }
    }

    frame.user_rflags = (frame.user_rflags & 0x0020_0CD5) | 0x0202;

    let mapping_opt = unsafe {
        if ACTIVE_MAPPING_PTR.is_null() {
            None
        } else {
            Some(&*ACTIVE_MAPPING_PTR)
        }
    };

    let mut console_opt = unsafe {
        if ACTIVE_CONSOLE_PTR.is_null() {
            None
        } else {
            Some(&mut *ACTIVE_CONSOLE_PTR)
        }
    };

    let mut allocator_opt = unsafe {
        if ACTIVE_ALLOCATOR_PTR.is_null() {
            None
        } else {
            Some(&mut *ACTIVE_ALLOCATOR_PTR)
        }
    };

    let syscall_num = frame.rax;
    let arg1 = frame.rdi;
    let arg2 = frame.rsi;
    let arg3 = frame.rdx;
    let arg4 = frame.r10;

    let ret: i64 = match syscall_num {
        SYS_EXIT => unsafe {
            sched::finish_current_from_syscall(arg1 as i64);
        },

        SYS_GETPID => unsafe { ACTIVE_PID as i64 },

        SYS_YIELD => {
            let ctx = build_resume_ctx_from_syscall(frame, 0);
            unsafe {
                sched::suspend_current_from_syscall(ctx, UserTaskState::Ready);
            }
        }

        SYS_SLEEP_MS => {
            let ms = arg1.min(30_000);
            if ms > 0 {
                let ctx = build_resume_ctx_from_syscall(frame, 0);
                let wake_ms = timer::uptime_ms().saturating_add(ms);
                unsafe {
                    sched::suspend_current_from_syscall(
                        ctx,
                        UserTaskState::Waiting(WaitReason::Sleep { wake_ms }),
                    );
                }
            }
            0
        }

        SYS_UPTIME_MS => timer::uptime_ms() as i64,

        SYS_PROC_COUNT => sched::active_count() as i64,

        SYS_BEEP => {
            let freq = (arg1 as u32).clamp(60, 12_000);
            let dur = (arg2 as u32).clamp(20, 3_000);
            crate::audio::mixer::play_tone_async(freq, dur);
            0
        }

        SYS_BRK => {
            if let (Some(mapping), Some(allocator)) = (mapping_opt, allocator_opt) {
                sched::sys_brk_current(arg1, mapping, allocator)
            } else {
                ENOMEM
            }
        }

        SYS_MMAP => {
            let len = arg2 as usize;
            let prot = arg3;
            if let (Some(mapping), Some(allocator)) = (mapping_opt, allocator_opt) {
                sched::sys_mmap_current(len, prot, mapping, allocator)
            } else {
                ENOMEM
            }
        }

        SYS_MUNMAP => {
            let addr = arg1;
            let len = arg2 as usize;
            if let (Some(mapping), Some(allocator)) = (mapping_opt, allocator_opt) {
                sched::sys_munmap_current(addr, len, mapping, allocator)
            } else {
                EINVAL
            }
        }

        SYS_PIPE => {
            let fds_ptr = arg1;
            if let Some(mapping) = mapping_opt {
                if validate_user_slice(mapping, allocator_opt.as_deref_mut(), user_pml4, fds_ptr, 8, true).is_err() {
                    EFAULT
                } else {
                    match ipc::create_pipe() {
                        Ok(pipe_id) => {
                            if let Some(rfd) = sched::alloc_current_fd(FdEntry::PipeRead { pipe_id }) {
                                if let Some(wfd) = sched::alloc_current_fd(FdEntry::PipeWrite { pipe_id }) {
                                    let mut raw = [0u8; 8];
                                    raw[0..4].copy_from_slice(&(rfd as i32).to_le_bytes());
                                    raw[4..8].copy_from_slice(&(wfd as i32).to_le_bytes());
                                    if copy_to_user_bytes(mapping, allocator_opt.as_deref_mut(), user_pml4, fds_ptr, &raw).is_err() {
                                        let _ = sched::take_current_fd(rfd);
                                        let _ = sched::take_current_fd(wfd);
                                        ipc::close_pipe_end(pipe_id, false);
                                        ipc::close_pipe_end(pipe_id, true);
                                        EFAULT
                                    } else {
                                        0
                                    }
                                } else {
                                    let _ = sched::take_current_fd(rfd);
                                    ipc::close_pipe_end(pipe_id, false);
                                    ipc::close_pipe_end(pipe_id, true);
                                    EMFILE
                                }
                            } else {
                                ipc::close_pipe_end(pipe_id, false);
                                ipc::close_pipe_end(pipe_id, true);
                                EMFILE
                            }
                        }
                        Err(e) => e,
                    }
                }
            } else {
                EINVAL
            }
        }

        SYS_OPEN => {
            if let Some(mapping) = mapping_opt {
                match read_user_path(mapping, allocator_opt.as_deref_mut(), user_pml4, arg1, arg2 as usize) {
                    Ok(raw_path) => {
                        let flags = arg3 as u32;
                        let acc = flags & O_ACCMODE;
                        let readable = acc == O_RDONLY || acc == O_RDWR;
                        let writable = acc == O_WRONLY || acc == O_RDWR;
                        let create = (flags & O_CREAT) != 0;
                        let trunc = (flags & O_TRUNC) != 0;
                        let append = (flags & O_APPEND) != 0;
                        let want_dir = (flags & O_DIRECTORY) != 0;

                        let vfs_res: Result<FdEntry, i64> = vfs::with_vfs(|s| {
                            let abs = s.resolve_path(&raw_path).map_err(vfs_err_to_errno)?;
                            match s.stat(&abs) {
                                Ok(st) if st.kind == VfsNodeKind::Directory => {
                                    if writable {
                                        return Err(EISDIR);
                                    }
                                    let entries = s.list_dir(&abs).map_err(vfs_err_to_errno)?;
                                    Ok(FdEntry::Dir {
                                        abs_path: abs,
                                        entries,
                                        cursor: 0,
                                    })
                                }
                                Ok(st) => {
                                    if want_dir {
                                        return Err(ENOTDIR);
                                    }
                                    if writable && trunc {
                                        s.write_file(&abs, &[]).map_err(vfs_err_to_errno)?;
                                    }
                                    let start_off = if append && !trunc { st.size } else { 0 };
                                    Ok(FdEntry::File {
                                        abs_path: abs,
                                        offset: start_off,
                                        readable,
                                        writable,
                                        append,
                                    })
                                }
                                Err(VfsError::NotFound) if create && !want_dir => {
                                    s.touch_or_truncate(&abs, true, false)
                                        .map_err(vfs_err_to_errno)?;
                                    Ok(FdEntry::File {
                                        abs_path: abs,
                                        offset: 0,
                                        readable,
                                        writable,
                                        append,
                                    })
                                }
                                Err(e) => Err(vfs_err_to_errno(e)),
                            }
                        });

                        match vfs_res {
                            Ok(entry) => match sched::alloc_current_fd(entry) {
                                Some(fd) => fd as i64,
                                None => EMFILE,
                            },
                            Err(e) => e,
                        }
                    }
                    Err(e) => e,
                }
            } else {
                EINVAL
            }
        }

        SYS_STAT => {
            if let Some(mapping) = mapping_opt {
                match read_user_path(mapping, allocator_opt.as_deref_mut(), user_pml4, arg1, arg2 as usize) {
                    Ok(path) => match vfs::with_vfs(|s| s.stat(&path)) {
                        Ok(st) => {
                            let ro = path == "/disk"
                                && vfs::with_vfs(|s| {
                                    s.volume_info_for(&path).map(|v| v.read_only).unwrap_or(false)
                                });
                            let ustat = UserStat {
                                size: st.size as u64,
                                kind: if st.kind == VfsNodeKind::Directory { 2 } else { 1 },
                                attributes: st.attributes as u32,
                                flags: if ro { 1 } else { 0 },
                                reserved: 0,
                            };
                            let bytes = unsafe {
                                core::slice::from_raw_parts(
                                    (&ustat as *const UserStat) as *const u8,
                                    core::mem::size_of::<UserStat>(),
                                )
                            };
                            if copy_to_user_bytes(mapping, allocator_opt.as_deref_mut(), user_pml4, arg3, bytes).is_err() {
                                EFAULT
                            } else {
                                0
                            }
                        }
                        Err(e) => vfs_err_to_errno(e),
                    },
                    Err(e) => e,
                }
            } else {
                EINVAL
            }
        }

        SYS_FSTAT => {
            if let Some(mapping) = mapping_opt {
                let fd = arg1 as usize;
                let entry_info = sched::with_current_fd_mut(fd, |entry| match entry {
                    FdEntry::File { abs_path, .. } => Ok((abs_path.clone(), false)),
                    FdEntry::Dir { abs_path, .. } => Ok((abs_path.clone(), true)),
                    FdEntry::Socket(_) | FdEntry::PipeRead { .. } | FdEntry::PipeWrite { .. } => {
                        Err(EINVAL)
                    }
                });
                match entry_info {
                    Some(Ok((path, _))) => match vfs::with_vfs(|s| s.stat(&path)) {
                        Ok(st) => {
                            let ustat = UserStat {
                                size: st.size as u64,
                                kind: if st.kind == VfsNodeKind::Directory { 2 } else { 1 },
                                attributes: st.attributes as u32,
                                flags: 0,
                                reserved: 0,
                            };
                            let bytes = unsafe {
                                core::slice::from_raw_parts(
                                    (&ustat as *const UserStat) as *const u8,
                                    core::mem::size_of::<UserStat>(),
                                )
                            };
                            if copy_to_user_bytes(mapping, allocator_opt.as_deref_mut(), user_pml4, arg2, bytes).is_err() {
                                EFAULT
                            } else {
                                0
                            }
                        }
                        Err(e) => vfs_err_to_errno(e),
                    },
                    Some(Err(e)) => e,
                    None => EBADF,
                }
            } else {
                EINVAL
            }
        }

        SYS_LSEEK => {
            let fd = arg1 as usize;
            let offset = arg2 as i64;
            let whence = arg3;
            match sched::with_current_fd_mut(fd, |entry| match entry {
                FdEntry::File {
                    abs_path,
                    offset: cur,
                    ..
                } => {
                    let size = vfs::with_vfs(|s| s.stat(abs_path).map(|st| st.size).unwrap_or(0));
                    let base: i64 = match whence {
                        0 => 0,            // SEEK_SET
                        1 => *cur as i64,  // SEEK_CUR
                        2 => size as i64,  // SEEK_END
                        _ => return Err(EINVAL),
                    };
                    let next = base.checked_add(offset).ok_or(EINVAL)?;
                    if next < 0 || next > u32::MAX as i64 {
                        return Err(EINVAL);
                    }
                    *cur = next as u32;
                    Ok(next)
                }
                FdEntry::Dir { cursor, entries, .. } => {
                    if whence == 0 && offset >= 0 {
                        *cursor = (offset as usize).min(entries.len());
                        Ok(*cursor as i64)
                    } else {
                        Err(EINVAL)
                    }
                }
                FdEntry::Socket(_) | FdEntry::PipeRead { .. } | FdEntry::PipeWrite { .. } => {
                    Err(EINVAL)
                }
            }) {
                Some(Ok(pos)) => pos,
                Some(Err(e)) => e,
                None => EBADF,
            }
        }

        SYS_GETDENTS => {
            let fd = arg1 as usize;
            let name_ptr = arg2;
            let name_cap = (arg3 as usize).min(240);
            let out_stat_ptr = arg4;
            if let Some(mapping) = mapping_opt {
                if name_cap == 0 {
                    EINVAL
                } else if validate_user_slice(mapping, allocator_opt.as_deref_mut(), user_pml4, name_ptr, name_cap, true).is_err()
                {
                    EFAULT
                } else {
                    let next = sched::with_current_fd_mut(fd, |entry| match entry {
                        FdEntry::Dir {
                            entries, cursor, ..
                        } => {
                            if *cursor >= entries.len() {
                                Ok(None)
                            } else {
                                let item = entries[*cursor].clone();
                                *cursor += 1;
                                Ok(Some(item))
                            }
                        }
                        _ => Err(ENOTDIR),
                    });

                    match next {
                        Some(Ok(None)) => 0,
                        Some(Ok(Some(item))) => {
                            let bytes = item.name.as_bytes();
                            let copy_len = bytes.len().min(name_cap);
                            if copy_to_user_bytes(mapping, allocator_opt.as_deref_mut(), user_pml4, name_ptr, &bytes[..copy_len])
                                .is_err()
                            {
                                EFAULT
                            } else {
                                let stat_res = if out_stat_ptr != 0 {
                                    let ustat = UserStat {
                                        size: item.size as u64,
                                        kind: if item.kind == VfsNodeKind::Directory {
                                            2
                                        } else {
                                            1
                                        },
                                        attributes: item.attributes as u32,
                                        flags: 0,
                                        reserved: 0,
                                    };
                                    let sbytes = unsafe {
                                        core::slice::from_raw_parts(
                                            (&ustat as *const UserStat) as *const u8,
                                            core::mem::size_of::<UserStat>(),
                                        )
                                    };
                                    copy_to_user_bytes(mapping, allocator_opt.as_deref_mut(), user_pml4, out_stat_ptr, sbytes)
                                } else {
                                    Ok(())
                                };
                                if stat_res.is_err() {
                                    EFAULT
                                } else {
                                    copy_len as i64
                                }
                            }
                        }
                        Some(Err(e)) => e,
                        None => EBADF,
                    }
                }
            } else {
                EINVAL
            }
        }

        SYS_MKDIR => {
            if let Some(mapping) = mapping_opt {
                match read_user_path(mapping, allocator_opt.as_deref_mut(), user_pml4, arg1, arg2 as usize) {
                    Ok(path) => match vfs::with_vfs(|s| s.create_dir(&path)) {
                        Ok(()) => 0,
                        Err(e) => vfs_err_to_errno(e),
                    },
                    Err(e) => e,
                }
            } else {
                EINVAL
            }
        }

        SYS_UNLINK => {
            if let Some(mapping) = mapping_opt {
                match read_user_path(mapping, allocator_opt.as_deref_mut(), user_pml4, arg1, arg2 as usize) {
                    Ok(path) => match vfs::with_vfs(|s| s.remove(&path)) {
                        Ok(()) => 0,
                        Err(e) => vfs_err_to_errno(e),
                    },
                    Err(e) => e,
                }
            } else {
                EINVAL
            }
        }

        SYS_WRITE => {
            let fd = arg1;
            let buf_ptr = arg2;
            let len = arg3 as usize;

            if len == 0 {
                0
            } else if fd == 1 || fd == 2 {
                if let Some(mapping) = mapping_opt {
                    let capped = len.min(16 * 1024);
                    let mut written = 0usize;
                    let mut tmp = [0u8; 256];
                    let mut fault = false;

                    while written < capped {
                        let step = (capped - written).min(tmp.len());
                        if copy_from_user_bytes(
                            mapping,
                            allocator_opt.as_deref_mut(),
                            user_pml4,
                            buf_ptr + written as u64,
                            &mut tmp[..step],
                        )
                        .is_err()
                        {
                            fault = true;
                            break;
                        }

                        if crate::graphics::wm::is_desktop_active() {
                            sched::push_current_stdout(sanitize_user_text(&tmp[..step]));
                        } else if let Some(console) = console_opt.as_mut() {
                            let color = if fd == 2 { RED } else { WHITE };
                            write_user_bytes_to_console(console, &tmp[..step], color);
                        }
                        written += step;
                    }

                    if fault && written == 0 {
                        EFAULT
                    } else {
                        written as i64
                    }
                } else {
                    EINVAL
                }
            } else if let Some(mapping) = mapping_opt {
                enum WriteTarget {
                    Tcp(usize),
                    Pipe(usize),
                    File(String, u32, bool),
                    Err(i64),
                }
                let target = sched::with_current_fd_mut(fd as usize, |entry| match entry {
                    FdEntry::Socket(OpenSocket::Tcp { handle }) => WriteTarget::Tcp(*handle),
                    FdEntry::Socket(_) | FdEntry::PipeRead { .. } => WriteTarget::Err(EINVAL),
                    FdEntry::PipeWrite { pipe_id } => WriteTarget::Pipe(*pipe_id),
                    FdEntry::File {
                        abs_path,
                        offset,
                        writable,
                        append,
                        ..
                    } => {
                        if !*writable {
                            WriteTarget::Err(EBADF)
                        } else {
                            WriteTarget::File(abs_path.clone(), *offset, *append)
                        }
                    }
                    FdEntry::Dir { .. } => WriteTarget::Err(EISDIR),
                });

                match target {
                    Some(WriteTarget::Pipe(pipe_id)) => {
                        let capped = len.min(ipc::PIPE_BUF_SIZE);
                        let mut kbuf = vec![0u8; capped];
                        if copy_from_user_bytes(mapping, allocator_opt.as_deref_mut(), user_pml4, buf_ptr, &mut kbuf).is_err() {
                            EFAULT
                        } else {
                            match ipc::write_pipe(pipe_id, &kbuf) {
                                Ok(n) => n as i64,
                                Err(e) => e,
                            }
                        }
                    }
                    Some(WriteTarget::Tcp(handle)) => {
                        let capped = len.min(8192);
                        let mut kbuf = vec![0u8; capped];
                        if copy_from_user_bytes(mapping, allocator_opt.as_deref_mut(), user_pml4, buf_ptr, &mut kbuf).is_err() {
                            EFAULT
                        } else {
                            unsafe { asm!("sti", options(nomem, nostack)); }
                            let res = crate::net::tcp::send_data(handle, &kbuf);
                            unsafe { asm!("cli", options(nomem, nostack)); }
                            match res {
                                Ok(n) => n as i64,
                                Err(_) => EIO,
                            }
                        }
                    }
                    Some(WriteTarget::File(abs_path, mut off, append)) => {
                        let capped = len.min(64 * 1024);
                        let mut kbuf = vec![0u8; capped];
                        if copy_from_user_bytes(mapping, allocator_opt.as_deref_mut(), user_pml4, buf_ptr, &mut kbuf).is_err() {
                            EFAULT
                        } else {
                            let res = vfs::with_vfs(|s| {
                                if append {
                                    if let Ok(st) = s.stat(&abs_path) {
                                        off = st.size;
                                    }
                                }
                                s.write_file_at(&abs_path, off, &kbuf)
                            });
                            match res {
                                Ok(n) => {
                                    sched::with_current_fd_mut(fd as usize, |entry| {
                                        if let FdEntry::File { offset, .. } = entry {
                                            *offset = off.saturating_add(n as u32);
                                        }
                                    });
                                    n as i64
                                }
                                Err(e) => vfs_err_to_errno(e),
                            }
                        }
                    }
                    Some(WriteTarget::Err(e)) => e,
                    None => EBADF,
                }
            } else {
                EINVAL
            }
        }

        SYS_READ => {
            let fd = arg1;
            let buf_ptr = arg2;
            let count = arg3 as usize;

            if count == 0 {
                0
            } else if fd == 0 {
                if let (Some(mapping), Some(console)) = (mapping_opt, console_opt) {
                    if crate::graphics::wm::is_desktop_active() {
                        EAGAIN
                    } else {
                        let mut kbuf = [0u8; 256];
                        let max_read = count.min(kbuf.len());
                        if validate_user_slice(mapping, allocator_opt.as_deref_mut(), user_pml4, buf_ptr, max_read, true).is_err() {
                            EFAULT
                        } else {
                            let actual = read_console_line(console, &mut kbuf[..max_read]);
                            if copy_to_user_bytes(mapping, allocator_opt.as_deref_mut(), user_pml4, buf_ptr, &kbuf[..actual]).is_err() {
                                EFAULT
                            } else {
                                actual as i64
                            }
                        }
                    }
                } else {
                    EINVAL
                }
            } else if let Some(mapping) = mapping_opt {
                enum ReadTarget {
                    Sock(OpenSocket),
                    Pipe(usize),
                    File(String, u32),
                    Err(i64),
                }
                let target = sched::with_current_fd_mut(fd as usize, |entry| match entry {
                    FdEntry::Socket(sock) => ReadTarget::Sock(*sock),
                    FdEntry::PipeRead { pipe_id } => ReadTarget::Pipe(*pipe_id),
                    FdEntry::PipeWrite { .. } => ReadTarget::Err(EBADF),
                    FdEntry::File {
                        abs_path,
                        offset,
                        readable,
                        ..
                    } => {
                        if !*readable {
                            ReadTarget::Err(EBADF)
                        } else {
                            ReadTarget::File(abs_path.clone(), *offset)
                        }
                    }
                    FdEntry::Dir { .. } => ReadTarget::Err(EISDIR),
                });

                match target {
                    Some(ReadTarget::Pipe(pipe_id)) => {
                        let max_read = count.min(ipc::PIPE_BUF_SIZE);
                        if validate_user_slice(mapping, allocator_opt.as_deref_mut(), user_pml4, buf_ptr, max_read, true).is_err() {
                            EFAULT
                        } else {
                            let mut kbuf = vec![0u8; max_read];
                            match ipc::read_pipe(pipe_id, &mut kbuf) {
                                Ok(n) => {
                                    if n > 0
                                        && copy_to_user_bytes(mapping, allocator_opt.as_deref_mut(), user_pml4, buf_ptr, &kbuf[..n])
                                            .is_err()
                                    {
                                        EFAULT
                                    } else {
                                        n as i64
                                    }
                                }
                                Err(-11) => {
                                    let ctx = build_resume_ctx_from_syscall(frame, 0);
                                    let deadline_ms = timer::uptime_ms().saturating_add(2000);
                                    unsafe {
                                        sched::suspend_current_from_syscall(
                                            ctx,
                                            UserTaskState::Waiting(WaitReason::PipeRead {
                                                pipe_id,
                                                buf_ptr,
                                                cap: max_read,
                                                deadline_ms,
                                            }),
                                        );
                                    }
                                }
                                Err(e) => e,
                            }
                        }
                    }
                    Some(ReadTarget::File(abs_path, off)) => {
                        let max_read = count.min(32 * 1024);
                        if validate_user_slice(mapping, allocator_opt.as_deref_mut(), user_pml4, buf_ptr, max_read, true).is_err() {
                            EFAULT
                        } else {
                            let mut kbuf = vec![0u8; max_read];
                            let res = vfs::with_vfs(|s| s.read_file_chunk(&abs_path, off, &mut kbuf));
                            match res {
                                Ok(n) => {
                                    if n > 0
                                        && copy_to_user_bytes(mapping, allocator_opt.as_deref_mut(), user_pml4, buf_ptr, &kbuf[..n])
                                            .is_err()
                                    {
                                        EFAULT
                                    } else {
                                        sched::with_current_fd_mut(fd as usize, |entry| {
                                            if let FdEntry::File { offset, .. } = entry {
                                                *offset = off.saturating_add(n as u32);
                                            }
                                        });
                                        n as i64
                                    }
                                }
                                Err(VfsError::OffsetOutOfRange) => 0, // EOF
                                Err(e) => vfs_err_to_errno(e),
                            }
                        }
                    }
                    Some(ReadTarget::Sock(sock)) => {
                        let max_read = count.min(2048);
                        if validate_user_slice(mapping, allocator_opt.as_deref_mut(), user_pml4, buf_ptr, max_read, true).is_err() {
                            EFAULT
                        } else {
                            let mut kbuf = vec![0u8; max_read];
                            match sock {
                                OpenSocket::Tcp { handle } => {
                                    unsafe { asm!("sti", options(nomem, nostack)); }
                                    let res = crate::net::tcp::recv_data(handle, &mut kbuf);
                                    unsafe { asm!("cli", options(nomem, nostack)); }
                                    match res {
                                        Ok(0) => EAGAIN,
                                        Ok(n) => {
                                            if copy_to_user_bytes(mapping, allocator_opt.as_deref_mut(), user_pml4, buf_ptr, &kbuf[..n]).is_err() {
                                                EFAULT
                                            } else {
                                                n as i64
                                            }
                                        }
                                        Err(_) => 0,
                                    }
                                }
                                OpenSocket::Udp { handle } => {
                                    unsafe { asm!("sti", options(nomem, nostack)); }
                                    let pkt = crate::net::udp::recv_socket(handle, &mut kbuf);
                                    unsafe { asm!("cli", options(nomem, nostack)); }
                                    if let Some((_ip, _port, n)) = pkt {
                                        if copy_to_user_bytes(mapping, allocator_opt.as_deref_mut(), user_pml4, buf_ptr, &kbuf[..n]).is_err() {
                                            EFAULT
                                        } else {
                                            n as i64
                                        }
                                    } else {
                                        EAGAIN
                                    }
                                }
                                OpenSocket::TcpUnconnected => -107,
                            }
                        }
                    }
                    Some(ReadTarget::Err(e)) => e,
                    None => EBADF,
                }
            } else {
                EINVAL
            }
        }

        SYS_CLOSE => {
            if let Some(entry) = sched::take_current_fd(arg1 as usize) {
                match entry {
                    FdEntry::Socket(sock) => match sock {
                        OpenSocket::Tcp { handle } => crate::net::tcp::close_socket(handle),
                        OpenSocket::Udp { handle } => crate::net::udp::free_socket(handle),
                        OpenSocket::TcpUnconnected => {}
                    },
                    FdEntry::PipeRead { pipe_id } => ipc::close_pipe_end(pipe_id, false),
                    FdEntry::PipeWrite { pipe_id } => ipc::close_pipe_end(pipe_id, true),
                    FdEntry::File { .. } | FdEntry::Dir { .. } => {}
                }
                0
            } else {
                EBADF
            }
        }

        SYS_SOCKET => {
            let domain = arg1;
            let sock_type = arg2;
            if domain != 2 {
                EINVAL
            } else if sock_type == 1 {
                match sched::alloc_current_fd(FdEntry::Socket(OpenSocket::TcpUnconnected)) {
                    Some(fd) => fd as i64,
                    None => EMFILE,
                }
            } else if sock_type == 2 {
                match crate::net::udp::alloc_socket() {
                    Ok((handle, _port)) => {
                        match sched::alloc_current_fd(FdEntry::Socket(OpenSocket::Udp { handle })) {
                            Some(fd) => fd as i64,
                            None => {
                                crate::net::udp::free_socket(handle);
                                EMFILE
                            }
                        }
                    }
                    Err(_) => EMFILE,
                }
            } else {
                EINVAL
            }
        }

        SYS_BIND => {
            let fd = arg1 as usize;
            let port = arg2 as u16;
            match sched::get_current_fd_socket(fd) {
                Some(OpenSocket::Udp { handle }) => {
                    if crate::net::udp::set_bound_port(handle, port).is_ok() {
                        0
                    } else {
                        EINVAL
                    }
                }
                _ => EBADF,
            }
        }

        SYS_CONNECT => {
            let fd = arg1 as usize;
            let addr_ptr = arg2;
            if let Some(mapping) = mapping_opt {
                match sched::get_current_fd_socket(fd) {
                    Some(OpenSocket::TcpUnconnected) => {
                        match read_user_sockaddr(mapping, allocator_opt.as_deref_mut(), user_pml4, addr_ptr) {
                            Ok(addr) => {
                                match crate::net::tcp::open_and_start_connect(addr.ip, addr.port) {
                                    Ok(handle) => {
                                        let ctx = build_resume_ctx_from_syscall(frame, 0);
                                        let deadline_ms = timer::uptime_ms().saturating_add(4000);
                                        unsafe {
                                            sched::suspend_current_from_syscall(
                                                ctx,
                                                UserTaskState::Waiting(WaitReason::TcpConnect {
                                                    fd,
                                                    handle,
                                                    deadline_ms,
                                                }),
                                            );
                                        }
                                    }
                                    Err(_) => ECONNREFUSED,
                                }
                            }
                            Err(e) => e,
                        }
                    }
                    Some(_) => EINVAL,
                    None => EBADF,
                }
            } else {
                EINVAL
            }
        }

        SYS_SEND => {
            let fd = arg1 as usize;
            let buf_ptr = arg2;
            let len = (arg3 as usize).min(1472);
            let addr_ptr = arg4;
            if let Some(mapping) = mapping_opt {
                let mut kbuf = vec![0u8; len];
                if copy_from_user_bytes(mapping, allocator_opt.as_deref_mut(), user_pml4, buf_ptr, &mut kbuf).is_err() {
                    EFAULT
                } else {
                    match sched::get_current_fd_socket(fd) {
                        Some(OpenSocket::Tcp { handle }) => {
                            unsafe { asm!("sti", options(nomem, nostack)); }
                            let res = crate::net::tcp::send_data(handle, &kbuf);
                            unsafe { asm!("cli", options(nomem, nostack)); }
                            match res {
                                Ok(n) => n as i64,
                                Err(_) => EIO,
                            }
                        }
                        Some(OpenSocket::Udp { handle }) => {
                            if addr_ptr == 0 {
                                EINVAL
                            } else {
                                match read_user_sockaddr(mapping, allocator_opt.as_deref_mut(), user_pml4, addr_ptr) {
                                    Ok(addr) => {
                                        let src_port =
                                            crate::net::udp::get_bound_port(handle).unwrap_or(52000);
                                        unsafe { asm!("sti", options(nomem, nostack)); }
                                        let res = crate::net::udp::send_datagram(
                                            src_port, addr.ip, addr.port, &kbuf,
                                        );
                                        unsafe { asm!("cli", options(nomem, nostack)); }
                                        if res.is_ok() { len as i64 } else { EIO }
                                    }
                                    Err(e) => e,
                                }
                            }
                        }
                        Some(OpenSocket::TcpUnconnected) => -107,
                        None => EBADF,
                    }
                }
            } else {
                EINVAL
            }
        }

        SYS_RECV => {
            let fd = arg1 as usize;
            let buf_ptr = arg2;
            let count = (arg3 as usize).min(2048);
            let addr_out_ptr = arg4;
            if let Some(mapping) = mapping_opt {
                if validate_user_slice(mapping, allocator_opt.as_deref_mut(), user_pml4, buf_ptr, count, true).is_err() {
                    EFAULT
                } else {
                    let mut kbuf = vec![0u8; count];
                    match sched::get_current_fd_socket(fd) {
                        Some(OpenSocket::Tcp { handle }) => {
                            unsafe { asm!("sti", options(nomem, nostack)); }
                            let res = crate::net::tcp::recv_data(handle, &mut kbuf);
                            unsafe { asm!("cli", options(nomem, nostack)); }
                            match res {
                                Ok(0) => EAGAIN,
                                Ok(n) => {
                                    if copy_to_user_bytes(mapping, allocator_opt.as_deref_mut(), user_pml4, buf_ptr, &kbuf[..n]).is_err() {
                                        EFAULT
                                    } else {
                                        n as i64
                                    }
                                }
                                Err(_) => 0,
                            }
                        }
                        Some(OpenSocket::Udp { handle }) => {
                            unsafe { asm!("sti", options(nomem, nostack)); }
                            let pkt = crate::net::udp::recv_socket(handle, &mut kbuf);
                            unsafe { asm!("cli", options(nomem, nostack)); }
                            if let Some((ip, port, n)) = pkt {
                                if copy_to_user_bytes(mapping, allocator_opt.as_deref_mut(), user_pml4, buf_ptr, &kbuf[..n]).is_err() {
                                    EFAULT
                                } else if addr_out_ptr != 0
                                    && write_user_sockaddr(
                                        mapping,
                                        allocator_opt.as_deref_mut(),
                                        user_pml4,
                                        addr_out_ptr,
                                        SockAddrIn { ip, port, pad: 0 },
                                    )
                                    .is_err()
                                {
                                    EFAULT
                                } else {
                                    n as i64
                                }
                            } else {
                                EAGAIN
                            }
                        }
                        Some(OpenSocket::TcpUnconnected) => -107,
                        None => EBADF,
                    }
                }
            } else {
                EINVAL
            }
        }

        SYS_NET_PING => {
            let packed_ip = (arg1 as u32).to_be_bytes();
            let seq = (arg2 as u16).max(1);
            let timeout_ms = arg3.clamp(100, 3000);
            let job_id = kthread::submit_net_job(NetJobRequest::Ping {
                target_ip: packed_ip,
                seq,
                timeout_ms,
            });
            let ctx = build_resume_ctx_from_syscall(frame, 0);
            let deadline_ms = timer::uptime_ms().saturating_add(timeout_ms + 300);
            unsafe {
                sched::suspend_current_from_syscall(
                    ctx,
                    UserTaskState::Waiting(WaitReason::NetJob {
                        job_id,
                        kind: NetWaitKind::Ping,
                        out_ptr: 0,
                        out_cap: 0,
                        deadline_ms,
                    }),
                );
            }
        }

        SYS_DNS_RESOLVE => {
            let host_ptr = arg1;
            let host_len = (arg2 as usize).min(128);
            let out_ip_ptr = arg3;
            if let Some(mapping) = mapping_opt {
                if host_len == 0 {
                    EINVAL
                } else if validate_user_slice(mapping, allocator_opt.as_deref_mut(), user_pml4, out_ip_ptr, 4, true).is_err() {
                    EFAULT
                } else {
                    let mut hbuf = [0u8; 128];
                    if copy_from_user_bytes(mapping, allocator_opt.as_deref_mut(), user_pml4, host_ptr, &mut hbuf[..host_len]).is_err() {
                        EFAULT
                    } else if let Ok(host) = core::str::from_utf8(&hbuf[..host_len]) {
                        let job_id = kthread::submit_net_job(NetJobRequest::DnsResolve {
                            host: String::from(host),
                        });
                        let ctx = build_resume_ctx_from_syscall(frame, 0);
                        let deadline_ms = timer::uptime_ms().saturating_add(3000);
                        unsafe {
                            sched::suspend_current_from_syscall(
                                ctx,
                                UserTaskState::Waiting(WaitReason::NetJob {
                                    job_id,
                                    kind: NetWaitKind::DnsResolve,
                                    out_ptr: out_ip_ptr,
                                    out_cap: 4,
                                    deadline_ms,
                                }),
                            );
                        }
                    } else {
                        EINVAL
                    }
                }
            } else {
                EINVAL
            }
        }

        SYS_HTTP_GET => {
            let url_ptr = arg1;
            let url_len = (arg2 as usize).min(256);
            let out_ptr = arg3;
            let out_cap = (arg4 as usize).min(16 * 1024);
            if let Some(mapping) = mapping_opt {
                if url_len == 0 || out_cap == 0 {
                    EINVAL
                } else if validate_user_slice(mapping, allocator_opt.as_deref_mut(), user_pml4, out_ptr, out_cap, true).is_err() {
                    EFAULT
                } else {
                    let mut ubuf = [0u8; 256];
                    if copy_from_user_bytes(mapping, allocator_opt.as_deref_mut(), user_pml4, url_ptr, &mut ubuf[..url_len]).is_err() {
                        EFAULT
                    } else if let Ok(url) = core::str::from_utf8(&ubuf[..url_len]) {
                        let job_id = kthread::submit_net_job(NetJobRequest::HttpGet {
                            url: String::from(url),
                            max_bytes: out_cap,
                        });
                        let ctx = build_resume_ctx_from_syscall(frame, 0);
                        let deadline_ms = timer::uptime_ms().saturating_add(12_000);
                        unsafe {
                            sched::suspend_current_from_syscall(
                                ctx,
                                UserTaskState::Waiting(WaitReason::NetJob {
                                    job_id,
                                    kind: NetWaitKind::HttpGet,
                                    out_ptr,
                                    out_cap,
                                    deadline_ms,
                                }),
                            );
                        }
                    } else {
                        EINVAL
                    }
                }
            } else {
                EINVAL
            }
        }

        SYS_CREATE_WINDOW => {
            let title_ptr = arg1;
            let title_len = (arg2 as usize).min(64);
            let packed = arg3;
            let w = ((packed >> 16) & 0xFFFF) as usize;
            let h = (packed & 0xFFFF) as usize;
            let owner_pid = unsafe { ACTIVE_PID };

            if let Some(mapping) = mapping_opt {
                let mut tbuf = [0u8; 64];
                if title_len > 0
                    && copy_from_user_bytes(mapping, allocator_opt.as_deref_mut(), user_pml4, title_ptr, &mut tbuf[..title_len]).is_err()
                {
                    EFAULT
                } else {
                    let title = core::str::from_utf8(&tbuf[..title_len]).unwrap_or("Ring3 App");
                    match crate::graphics::wm::create_external_window_from_syscall(owner_pid, title, w, h) {
                        Ok(id) => {
                            sched::mark_current_gui_window();
                            id as i64
                        }
                        Err(e) => e,
                    }
                }
            } else {
                EINVAL
            }
        }

        SYS_WINDOW_BLIT => {
            let win_id = arg1;
            let pixels_ptr = arg2;
            let packed = arg3;
            let w = ((packed >> 16) & 0xFFFF) as usize;
            let h = (packed & 0xFFFF) as usize;

            if w == 0 || h == 0 || w > 1280 || h > 800 {
                EINVAL
            } else if let Some(mapping) = mapping_opt {
                let pixel_count = w * h;
                let byte_len = pixel_count * 4;
                let mut kbuf = vec![0u32; pixel_count];
                let byte_slice = unsafe {
                    core::slice::from_raw_parts_mut(kbuf.as_mut_ptr() as *mut u8, byte_len)
                };
                if copy_from_user_bytes(mapping, allocator_opt.as_deref_mut(), user_pml4, pixels_ptr, byte_slice).is_err() {
                    EFAULT
                } else {
                    match crate::graphics::wm::blit_external_window_from_syscall(win_id, &kbuf, w, h) {
                        Ok(()) => 0,
                        Err(e) => e,
                    }
                }
            } else {
                EINVAL
            }
        }

        SYS_POLL_GUI_EVENT => {
            let win_id = arg1;
            let evt_ptr = arg2;
            let wait_ms = arg3.min(500);

            if let Some(mapping) = mapping_opt {
                if validate_user_slice(
                    mapping,
                    allocator_opt.as_deref_mut(),
                    user_pml4,
                    evt_ptr,
                    core::mem::size_of::<crate::graphics::wm::UserGuiEvent>(),
                    true,
                )
                .is_err()
                {
                    EFAULT
                } else {
                    let evt = crate::graphics::wm::pop_external_event_from_syscall(win_id);
                    let evt_bytes = unsafe {
                        core::slice::from_raw_parts(
                            (&evt as *const crate::graphics::wm::UserGuiEvent) as *const u8,
                            core::mem::size_of::<crate::graphics::wm::UserGuiEvent>(),
                        )
                    };
                    if copy_to_user_bytes(mapping, allocator_opt.as_deref_mut(), user_pml4, evt_ptr, evt_bytes).is_err() {
                        EFAULT
                    } else if evt.kind != 0 {
                        1
                    } else if wait_ms > 0 {
                        let ctx = build_resume_ctx_from_syscall(frame, 0);
                        let deadline_ms = timer::uptime_ms().saturating_add(wait_ms.min(16));
                        unsafe {
                            sched::suspend_current_from_syscall(
                                ctx,
                                UserTaskState::Waiting(WaitReason::GuiEvent {
                                    win_id,
                                    evt_ptr,
                                    deadline_ms,
                                }),
                            );
                        }
                    } else {
                        0
                    }
                }
            } else {
                EINVAL
            }
        }

        SYS_CREATE_SHM_WINDOW => {
            let title_ptr = arg1;
            let title_len = (arg2 as usize).min(64);
            let packed = arg3;
            let out_info_ptr = arg4;
            let w = ((packed >> 16) & 0xFFFF) as usize;
            let h = (packed & 0xFFFF) as usize;
            let cw = w.clamp(160, 1280);
            let ch = h.clamp(120, 800);
            let owner_pid = unsafe { ACTIVE_PID };

            if mapping_opt.is_some() && allocator_opt.is_some() {
                let mapping = mapping_opt.unwrap();
                let info_size = core::mem::size_of::<crate::graphics::wm::UserShmWindowInfo>();
                let mut tbuf = [0u8; 64];
                if validate_user_slice(
                    mapping,
                    allocator_opt.as_deref_mut(),
                    user_pml4,
                    out_info_ptr,
                    info_size,
                    true,
                )
                .is_err()
                {
                    EFAULT
                } else if title_len > 0
                    && copy_from_user_bytes(
                        mapping,
                        allocator_opt.as_deref_mut(),
                        user_pml4,
                        title_ptr,
                        &mut tbuf[..title_len],
                    )
                    .is_err()
                {
                    EFAULT
                } else {
                    let title = core::str::from_utf8(&tbuf[..title_len]).unwrap_or("SHM App");
                    let fb_bytes = cw * ch * 4;
                    let shm_res = {
                        let allocator = allocator_opt.as_deref_mut().unwrap();
                        ipc::create_or_open_shm("", fb_bytes, mapping, allocator)
                    };
                    match shm_res {
                        Ok((shm_id, aligned_size)) => {
                            match crate::graphics::wm::create_external_shm_window_from_syscall(
                                owner_pid, title, cw, ch, shm_id,
                            ) {
                                Ok((win_id, final_w, final_h)) => {
                                    let vaddr = {
                                        let allocator = allocator_opt.as_deref_mut().unwrap();
                                        sched::sys_shm_map_current(shm_id, true, mapping, allocator)
                                    };
                                    if vaddr < 0 {
                                        let freed = crate::graphics::wm::close_external_windows_for_pid(
                                            owner_pid,
                                        );
                                        let allocator = allocator_opt.as_deref_mut().unwrap();
                                        for sid in freed {
                                            ipc::release_shm(sid, allocator);
                                        }
                                        vaddr
                                    } else {
                                        sched::mark_current_gui_window();
                                        let info = crate::graphics::wm::UserShmWindowInfo {
                                            win_id,
                                            shm_id,
                                            vaddr: vaddr as u64,
                                            size_bytes: aligned_size as u64,
                                            width: final_w as u32,
                                            height: final_h as u32,
                                        };
                                        let bytes = unsafe {
                                            core::slice::from_raw_parts(
                                                (&info as *const crate::graphics::wm::UserShmWindowInfo)
                                                    as *const u8,
                                                info_size,
                                            )
                                        };
                                        if copy_to_user_bytes(
                                            mapping,
                                            allocator_opt.as_deref_mut(),
                                            user_pml4,
                                            out_info_ptr,
                                            bytes,
                                        )
                                        .is_err()
                                        {
                                            EFAULT
                                        } else {
                                            win_id as i64
                                        }
                                    }
                                }
                                Err(e) => {
                                    let allocator = allocator_opt.as_deref_mut().unwrap();
                                    ipc::release_shm(shm_id, allocator);
                                    e
                                }
                            }
                        }
                        Err(e) => e,
                    }
                }
            } else {
                EINVAL
            }
        }

        SYS_COMMIT_SHM_WINDOW => {
            let win_id = arg1;
            match crate::graphics::wm::commit_external_shm_window_from_syscall(win_id) {
                Ok(()) => 0,
                Err(e) => e,
            }
        }

        SYS_SHM_CREATE => {
            let name_ptr = arg1;
            let name_len = (arg2 as usize).min(48);
            let req_size = arg3 as usize;
            if mapping_opt.is_some() && allocator_opt.is_some() {
                let mapping = mapping_opt.unwrap();
                let mut nbuf = [0u8; 48];
                if name_len > 0
                    && copy_from_user_bytes(
                        mapping,
                        allocator_opt.as_deref_mut(),
                        user_pml4,
                        name_ptr,
                        &mut nbuf[..name_len],
                    )
                    .is_err()
                {
                    EFAULT
                } else if let Ok(name) = core::str::from_utf8(&nbuf[..name_len]) {
                    let allocator = allocator_opt.as_deref_mut().unwrap();
                    match ipc::create_or_open_shm(name, req_size, mapping, allocator) {
                        Ok((shm_id, _)) => {
                            sched::record_current_shm_owner(shm_id);
                            shm_id as i64
                        }
                        Err(e) => e,
                    }
                } else {
                    EINVAL
                }
            } else {
                EINVAL
            }
        }

        SYS_SHM_MAP => {
            let shm_id = arg1;
            let writable = arg2 != 0;
            if let (Some(mapping), Some(allocator)) = (mapping_opt, allocator_opt) {
                sched::sys_shm_map_current(shm_id, writable, mapping, allocator)
            } else {
                EINVAL
            }
        }

        SYS_SHM_UNMAP => {
            let vaddr = arg1;
            if let (Some(mapping), Some(allocator)) = (mapping_opt, allocator_opt) {
                sched::sys_shm_unmap_current(vaddr, mapping, allocator)
            } else {
                EINVAL
            }
        }

        SYS_SPAWN_ELF => {
            let path_ptr = arg1;
            let path_len = (arg2 as usize).min(240);
            if let Some(mapping) = mapping_opt {
                if path_len == 0 {
                    EINVAL
                } else {
                    let mut pbuf = [0u8; 240];
                    if copy_from_user_bytes(mapping, allocator_opt.as_deref_mut(), user_pml4, path_ptr, &mut pbuf[..path_len]).is_err() {
                        EFAULT
                    } else if let Ok(cmdline) = core::str::from_utf8(&pbuf[..path_len]) {
                        match sched::queue_spawn_from_user(cmdline) {
                            Ok(()) => 0,
                            Err(e) => e,
                        }
                    } else {
                        EINVAL
                    }
                }
            } else {
                EINVAL
            }
        }

        _ => ENOSYS,
    };

    frame.rax = ret as u64;

    unsafe {
        switch_cr3_raw(user_pml4, flags_bits);
    }
}

fn sanitize_user_text(bytes: &[u8]) -> &str {
    core::str::from_utf8(bytes).unwrap_or("[binary data]\n")
}

fn write_user_bytes_to_console(console: &mut Console<'_>, bytes: &[u8], color: Color) {
    let mut start = 0usize;
    while start < bytes.len() {
        let mut end = start;
        while end < bytes.len() {
            let b = bytes[end];
            if b == b'\n' || b == b'\t' || (32..=126).contains(&b) {
                end += 1;
            } else {
                break;
            }
        }

        if end > start {
            if let Ok(text) = core::str::from_utf8(&bytes[start..end]) {
                console.write(text, color);
            }
        }

        if end < bytes.len() {
            if bytes[end] == b'\r' {
                end += 1;
            } else {
                console.write(".", color);
                end += 1;
            }
        }
        start = end;
    }
}

pub fn format_syscall_fault_note(fault: &UserFaultInfo) -> alloc::string::String {
    format!(
        "vec={} rip=0x{:x} rsp=0x{:x} cr2=0x{:x}",
        fault.vector, fault.rip, fault.rsp, fault.cr2
    )
}
