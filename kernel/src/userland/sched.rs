use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::arch::asm;
use core::ptr::write_bytes;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use spin::Mutex;
use x86_64::instructions::interrupts::without_interrupts;
use x86_64::structures::paging::PageTableFlags;
use x86_64::VirtAddr;

use crate::console::Console;
use crate::graphics::Color;
use crate::interrupts::timer;
use crate::klog::{self, KLogLevel};
use crate::memory::vmm::{PhysicalMemoryMapping, COW_FLAG, PAGE_SIZE};
use crate::memory::PhysicalFrameAllocator;
use crate::process::{ProcessState, PROCESS_MANAGER};
use crate::storage::vfs::VfsDirEntry;
use crate::task::kthread::{self, NetJobOutcome};

use super::elf::{self, LoadedElfImage, USER_ADDR_LIMIT};
use super::freebsd::signal::{
    self, FreeBsdSigAction, FreeBsdSigSet, SavedSignalFrame, SignalDefault, SignalHandlerConfig,
    SignalState, SIGCHLD, SIGKILL, SIGTRAMP_CODE, SIG_BLOCK, SIG_DFL, SIG_IGN, SIG_SETMASK,
    SIG_UNBLOCK, USER_SIGTRAMP_ADDR,
};
use super::freebsd::{
    self,
    kqueue::{self, KEvent},
    ProcessAbi, ThrParam, UMTX_OP_WAIT, UMTX_OP_WAIT_UINT, UMTX_OP_WAIT_UINT_PRIVATE,
    UMTX_OP_WAKE, UMTX_OP_WAKE_PRIVATE,
};
use super::ipc::{
    self, MachKernelMsg, MachMsgHeader, MachMsgOolDesc, MachOolPayload, MachPortName,
    PortRightEntry, PortRightKind, MACH_MSGH_BITS_COMPLEX, MACH_MSGH_BITS_LOCAL_MASK,
    MACH_MSGH_BITS_REMOTE_MASK, MACH_MSG_TYPE_COPY_SEND, MACH_MSG_TYPE_MAKE_SEND,
    MACH_MSG_TYPE_MAKE_SEND_ONCE, MACH_MSG_TYPE_MOVE_SEND, MACH_MSG_TYPE_MOVE_SEND_ONCE,
    MACH_PORT_NULL, MACH_RCV_MSG, MACH_RCV_TIMEOUT, MACH_SEND_MSG,
};
use super::syscall::{self, SyscallFrame, UserFaultInfo, UserTrapOutcome};

pub const MAX_USER_PROCS: usize = 16;
pub const MAX_PROC_FDS: usize = 16;
pub const FD_OFFSET: usize = 3;
pub const DEFAULT_QUANTUM_MS: u64 = 8;

const USER_HEAP_LIMIT: u64 = 0x0000_003F_FFFF_0000;
const USER_MMAP_BASE: u64 = 0x0000_0040_0000_0000;
const USER_MMAP_LIMIT: u64 = 0x0000_0050_0000_0000;
const USER_SHM_BASE: u64 = 0x0000_0050_0000_0000;
const USER_SHM_LIMIT: u64 = 0x0000_0070_0000_0000;
const MAX_DYNAMIC_USER_BYTES: usize = 64 * 1024 * 1024;

#[repr(C, align(16))]
#[derive(Clone, Copy)]
pub struct FxSaveArea(pub [u8; 512]);

impl FxSaveArea {
    pub fn new_clean() -> Self {
        let mut area = Self([0; 512]);
        // Default x87 FCW (0x037F) at offset 0 and SSE MXCSR (0x1F80) at offset 24
        area.0[0] = 0x7F;
        area.0[1] = 0x03;
        area.0[24] = 0x80;
        area.0[25] = 0x1F;
        area
    }
}

impl Default for FxSaveArea {
    fn default() -> Self {
        Self::new_clean()
    }
}

const WHITE: Color = (235, 235, 240);
const GREEN: Color = (40, 180, 120);
const GRAY: Color = (140, 140, 150);
const RED: Color = (220, 90, 90);

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct UserCpuContext {
    pub r15: u64,    // 0
    pub r14: u64,    // 8
    pub r13: u64,    // 16
    pub r12: u64,    // 24
    pub rbp: u64,    // 32
    pub rbx: u64,    // 40
    pub r11: u64,    // 48
    pub r10: u64,    // 56
    pub r9: u64,     // 64
    pub r8: u64,     // 72
    pub rax: u64,    // 80
    pub rcx: u64,    // 88
    pub rdx: u64,    // 96
    pub rsi: u64,    // 104
    pub rdi: u64,    // 112
    pub rip: u64,    // 120
    pub cs: u64,     // 128
    pub rflags: u64, // 136
    pub rsp: u64,    // 144
    pub ss: u64,     // 152
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum VmRegionKind {
    ElfSegment,
    Stack,
    StackGuard,
    LazyHeap,
    LazyMmap,
    SharedMem,
}

#[derive(Clone, Copy)]
pub struct VmRegion {
    pub start: u64,
    pub end: u64,
    pub writable: bool,
    pub executable: bool,
    pub kind: VmRegionKind,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FaultResolveOutcome {
    Resolved,
    GuardPageHit,
    NotHandled,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum NetWaitKind {
    Ping,
    DnsResolve,
    HttpGet,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum WaitReason {
    Sleep {
        wake_ms: u64,
    },
    GuiEvent {
        win_id: u64,
        evt_ptr: u64,
        deadline_ms: u64,
    },
    TcpConnect {
        fd: usize,
        handle: usize,
        deadline_ms: u64,
    },
    NetJob {
        job_id: u64,
        kind: NetWaitKind,
        out_ptr: u64,
        out_cap: usize,
        deadline_ms: u64,
    },
    PipeRead {
        pipe_id: usize,
        buf_ptr: u64,
        cap: usize,
        deadline_ms: u64,
    },
    WaitPid {
        target_pid: isize,
        status_ptr: u64,
    },
    MachPortRecv {
        port_id: u64,
        rcv_name: MachPortName,
        msg_ptr: u64,
        rcv_cap: usize,
        deadline_ms: u64,
    },
    Kqueue {
        kq_id: usize,
        eventlist_ptr: u64,
        nevents: usize,
        deadline_ms: u64,
    },
    UmtxWait {
        pml4: u64,
        addr: u64,
        val: u64,
        is_u64: bool,
        deadline_ms: u64,
    },
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum UserTaskState {
    Ready,
    Running,
    Waiting(WaitReason),
    Exited(i64),
    Faulted(UserFaultInfo),
}

impl PartialEq for UserFaultInfo {
    fn eq(&self, other: &Self) -> bool {
        self.vector == other.vector
            && self.error_code == other.error_code
            && self.rip == other.rip
            && self.rsp == other.rsp
            && self.cr2 == other.cr2
    }
}
impl Eq for UserFaultInfo {}

#[derive(Clone, Copy)]
pub enum OpenSocket {
    TcpUnconnected,
    Tcp { handle: usize },
    Udp { handle: usize },
}

#[derive(Clone)]
pub enum FdEntry {
    Socket(OpenSocket),
    File {
        abs_path: String,
        offset: u32,
        readable: bool,
        writable: bool,
        append: bool,
    },
    Dir {
        abs_path: String,
        entries: Vec<VfsDirEntry>,
        cursor: usize,
    },
    PipeRead {
        pipe_id: usize,
    },
    PipeWrite {
        pipe_id: usize,
    },
    Kqueue {
        kq_id: usize,
    },
}

#[derive(Clone, Copy)]
struct ShmAttachment {
    shm_id: u64,
    vaddr: u64,
    page_count: usize,
}

#[derive(Clone)]
pub struct UserProcSnapshot {
    pub pid: usize,
    pub name: String,
    pub state_label: &'static str,
    pub cpu_ms: u64,
    pub vruntime_ms: u64,
    pub syscalls: u64,
    pub pages: usize,
    pub preempts: u64,
    pub lazy_faults: u64,
    pub vmas: usize,
}

#[derive(Clone)]
pub struct FinishedUserProcess {
    pub pid: usize,
    pub name: String,
    pub entry_point: u64,
    pub pages_mapped: usize,
    pub bytes_loaded: u64,
    pub outcome: UserTrapOutcome,
}

struct UserProcessSlot {
    pid: usize,
    tgid: usize,
    is_thread: bool,
    parent_pid: usize,
    abi: ProcessAbi,
    name: String,
    pml4: u64,
    ctx: UserCpuContext,
    fpu_state: FxSaveArea,
    fs_base: u64,
    gs_base: u64,
    clear_child_tid: u64,
    signals: SignalState,
    state: UserTaskState,
    cpu_ms: u64,
    vruntime_ms: u64,
    weight: u32,
    preempt_count: u64,
    lazy_faults: u64,
    syscalls_served: u64,
    entry_point: u64,
    pages_mapped: usize,
    bytes_loaded: u64,
    brk_base: u64,
    brk_current: u64,
    mmap_next: u64,
    shm_attach_next: u64,
    shm_attachments: Vec<ShmAttachment>,
    owned_shm_ids: Vec<u64>,
    dynamic_bytes: usize,
    vmas: Vec<VmRegion>,
    image: Option<LoadedElfImage>,
    fds: [Option<FdEntry>; MAX_PROC_FDS],
    port_rights: Vec<PortRightEntry>,
    next_port_name: u32,
    stdout_buf: String,
    has_gui_window: bool,
}

impl UserProcessSlot {
    fn find_vma(&self, addr: u64) -> Option<VmRegion> {
        self.vmas
            .iter()
            .copied()
            .find(|r| addr >= r.start && addr < r.end)
    }

    fn total_dynamic_vma_bytes(&self) -> usize {
        self.vmas
            .iter()
            .filter(|r| matches!(r.kind, VmRegionKind::LazyMmap | VmRegionKind::LazyHeap))
            .map(|r| (r.end.saturating_sub(r.start)) as usize)
            .sum()
    }

    fn remove_mmap_range(&mut self, start: u64, end: u64) {
        let mut updated = Vec::with_capacity(self.vmas.len());
        for region in self.vmas.iter().copied() {
            if region.kind != VmRegionKind::LazyMmap || region.end <= start || region.start >= end {
                updated.push(region);
                continue;
            }
            if region.start < start {
                updated.push(VmRegion {
                    start: region.start,
                    end: start,
                    ..region
                });
            }
            if region.end > end {
                updated.push(VmRegion {
                    start: end,
                    end: region.end,
                    ..region
                });
            }
        }
        self.vmas = updated;
    }

    fn alloc_port_right(
        &mut self,
        port_id: u64,
        right: PortRightKind,
        also_send: bool,
    ) -> MachPortName {
        let name = self.next_port_name.max(1);
        self.next_port_name = name.saturating_add(1).max(1);
        self.port_rights.push(PortRightEntry {
            name,
            port_id,
            right,
            also_send,
        });
        name
    }

    fn find_port_right(&self, name: MachPortName) -> Option<PortRightEntry> {
        self.port_rights.iter().copied().find(|r| r.name == name)
    }

    fn take_port_right(&mut self, name: MachPortName) -> Option<PortRightEntry> {
        let pos = self.port_rights.iter().position(|r| r.name == name)?;
        Some(self.port_rights.swap_remove(pos))
    }

    #[inline]
    fn charge_runtime(&mut self, elapsed_ms: u64) {
        self.cpu_ms = self.cpu_ms.saturating_add(elapsed_ms);
        let scaled = (elapsed_ms.saturating_mul(1024)) / (self.weight.max(1) as u64);
        self.vruntime_ms = self.vruntime_ms.saturating_add(scaled.max(1));
    }
}

struct UserScheduler {
    slots: [Option<Box<UserProcessSlot>>; MAX_USER_PROCS],
    current_slot: Option<usize>,
    rr_cursor: usize,
    next_tid: usize,
    quantum_start_ms: u64,
    quantum_deadline_ms: u64,
    pending_spawns: Vec<Vec<String>>,
    completed_for_sync: Vec<FinishedUserProcess>,
    exited_for_wait: Vec<(usize, usize, i64)>,
}

impl UserScheduler {
    const fn new() -> Self {
        Self {
            slots: [const { None }; MAX_USER_PROCS],
            current_slot: None,
            rr_cursor: 0,
            next_tid: 10_000,
            quantum_start_ms: 0,
            quantum_deadline_ms: 0,
            pending_spawns: Vec::new(),
            completed_for_sync: Vec::new(),
            exited_for_wait: Vec::new(),
        }
    }

    fn alloc_tid(&mut self) -> usize {
        loop {
            let candidate = self.next_tid;
            self.next_tid = self.next_tid.saturating_add(1).max(10_000);
            if self.find_slot_by_pid(candidate).is_none() {
                return candidate;
            }
        }
    }

    fn find_slot_by_pid(&self, pid: usize) -> Option<usize> {
        self.slots
            .iter()
            .position(|s| s.as_ref().map(|p| p.pid) == Some(pid))
    }

    fn find_slot_by_pml4(&self, pml4: u64) -> Option<usize> {
        self.slots
            .iter()
            .position(|s| s.as_ref().map(|p| p.pml4) == Some(pml4))
    }

    fn count_active(&self) -> usize {
        self.slots.iter().filter(|s| s.is_some()).count()
    }

    fn count_tgid(&self, tgid: usize) -> usize {
        self.slots
            .iter()
            .flatten()
            .filter(|p| p.tgid == tgid)
            .count()
    }

    fn min_runnable_vruntime(&self) -> u64 {
        self.slots
            .iter()
            .flatten()
            .filter(|p| matches!(p.state, UserTaskState::Ready | UserTaskState::Running))
            .map(|p| p.vruntime_ms)
            .min()
            .unwrap_or(0)
    }

    fn has_live_child_matching(&self, parent_pid: usize, target_pid: isize) -> bool {
        self.slots.iter().flatten().any(|c| {
            !c.is_thread
                && c.parent_pid == parent_pid
                && (target_pid <= 0 || c.pid == target_pid as usize)
        })
    }
}

static USER_SCHED: Mutex<UserScheduler> = Mutex::new(UserScheduler::new());
static QUANTUM_ACTIVE: AtomicBool = AtomicBool::new(false);
static LAST_DISPATCH_PID: AtomicU64 = AtomicU64::new(0);

#[inline]
pub fn is_quantum_active() -> bool {
    QUANTUM_ACTIVE.load(Ordering::Acquire)
}

pub fn active_count() -> usize {
    without_interrupts(|| USER_SCHED.lock().count_active())
}

pub fn is_process_alive(pid: usize) -> bool {
    without_interrupts(|| {
        let sched = USER_SCHED.lock();
        sched.slots.iter().flatten().any(|p| {
            (p.pid == pid || p.tgid == pid)
                && !matches!(p.state, UserTaskState::Exited(_) | UserTaskState::Faulted(_))
        })
    })
}

pub fn has_runnable_tasks() -> bool {
    without_interrupts(|| {
        let sched = USER_SCHED.lock();
        if !sched.pending_spawns.is_empty() {
            return true;
        }
        sched.slots.iter().flatten().any(|p| {
            matches!(
                p.state,
                UserTaskState::Ready | UserTaskState::Waiting(_) | UserTaskState::Running
            )
        })
    })
}

pub fn has_ready_now() -> bool {
    let now_ms = timer::uptime_ms();
    without_interrupts(|| {
        let sched = USER_SCHED.lock();
        if !sched.pending_spawns.is_empty() {
            return true;
        }
        sched.slots.iter().flatten().any(|p| {
            if p.signals.next_deliverable().is_some() {
                return true;
            }
            match p.state {
                UserTaskState::Ready => true,
                UserTaskState::Waiting(reason) => match reason {
                    WaitReason::Sleep { wake_ms } => now_ms >= wake_ms,
                    WaitReason::GuiEvent { deadline_ms, .. }
                    | WaitReason::TcpConnect { deadline_ms, .. }
                    | WaitReason::NetJob { deadline_ms, .. }
                    | WaitReason::PipeRead { deadline_ms, .. }
                    | WaitReason::UmtxWait { deadline_ms, .. } => now_ms >= deadline_ms,
                    WaitReason::Kqueue {
                        kq_id, deadline_ms, ..
                    } => now_ms >= deadline_ms || kqueue::has_ready_note(kq_id, now_ms),
                    WaitReason::WaitPid { target_pid, .. } => {
                        sched.exited_for_wait.iter().any(|&(ppid, cpid, _)| {
                            ppid == p.tgid && (target_pid <= 0 || cpid == target_pid as usize)
                        }) || !sched.has_live_child_matching(p.tgid, target_pid)
                    }
                    WaitReason::MachPortRecv {
                        port_id,
                        deadline_ms,
                        ..
                    } => {
                        now_ms >= deadline_ms
                            || ipc::mach_port_has_queued_msg(port_id)
                            || ipc::mach_port_is_dead(port_id)
                    }
                },
                UserTaskState::Exited(_) | UserTaskState::Faulted(_) => true,
                UserTaskState::Running => false,
            }
        })
    })
}

pub fn current_pid() -> usize {
    without_interrupts(|| {
        let sched = USER_SCHED.lock();
        sched
            .current_slot
            .and_then(|idx| sched.slots[idx].as_ref().map(|p| p.pid))
            .unwrap_or(0)
    })
}

pub fn current_tgid() -> usize {
    without_interrupts(|| {
        let sched = USER_SCHED.lock();
        sched
            .current_slot
            .and_then(|idx| sched.slots[idx].as_ref().map(|p| p.tgid))
            .unwrap_or(0)
    })
}

pub fn current_parent_pid() -> usize {
    without_interrupts(|| {
        let sched = USER_SCHED.lock();
        sched
            .current_slot
            .and_then(|idx| sched.slots[idx].as_ref().map(|p| p.parent_pid))
            .unwrap_or(0)
    })
}

pub fn current_abi() -> ProcessAbi {
    without_interrupts(|| {
        let sched = USER_SCHED.lock();
        sched
            .current_slot
            .and_then(|idx| sched.slots[idx].as_ref().map(|p| p.abi))
            .unwrap_or(ProcessAbi::Amaryllis)
    })
}

pub fn set_current_fs_base(addr: u64) {
    without_interrupts(|| {
        let mut sched = USER_SCHED.lock();
        if let Some(idx) = sched.current_slot {
            if let Some(ref mut proc) = sched.slots[idx] {
                proc.fs_base = addr;
            }
        }
    });
}

pub fn get_current_fs_base() -> u64 {
    without_interrupts(|| {
        let sched = USER_SCHED.lock();
        sched
            .current_slot
            .and_then(|idx| sched.slots[idx].as_ref().map(|p| p.fs_base))
            .unwrap_or(0)
    })
}

pub fn set_current_gs_base(addr: u64) {
    without_interrupts(|| {
        let mut sched = USER_SCHED.lock();
        if let Some(idx) = sched.current_slot {
            if let Some(ref mut proc) = sched.slots[idx] {
                proc.gs_base = addr;
            }
        }
    });
}

pub fn get_current_gs_base() -> u64 {
    without_interrupts(|| {
        let sched = USER_SCHED.lock();
        sched
            .current_slot
            .and_then(|idx| sched.slots[idx].as_ref().map(|p| p.gs_base))
            .unwrap_or(0)
    })
}

pub fn register_process(
    pid: usize,
    name: &str,
    pml4: u64,
    image: LoadedElfImage,
) -> Result<(), LoadedElfImage> {
    let selectors = crate::arch::gdt::selectors();
    let abi = image.abi;
    let (init_rdi, init_rsi) = match abi {
        ProcessAbi::FreeBsd => (image.user_rsp, 0u64),
        ProcessAbi::Amaryllis => (image.initial_argc, image.initial_argv),
    };
    let ctx = UserCpuContext {
        rip: image.entry_point,
        rsp: image.user_rsp,
        rdi: init_rdi,
        rsi: init_rsi,
        cs: selectors.user_code.0 as u64,
        ss: selectors.user_data.0 as u64,
        rflags: 0x0202,
        ..UserCpuContext::default()
    };

    let pages_mapped = image.mapped_pages.len();
    let bytes_loaded = image.total_bytes_loaded;
    let brk_base = image.brk_base;
    let entry_point = image.entry_point;

    let mut vmas = Vec::new();
    for &(start, end, writable, executable) in &image.load_regions {
        vmas.push(VmRegion {
            start,
            end,
            writable,
            executable,
            kind: VmRegionKind::ElfSegment,
        });
    }
    vmas.push(VmRegion {
        start: elf::USER_STACK_MAX_BOTTOM,
        end: elf::USER_STACK_TOP,
        writable: true,
        executable: false,
        kind: VmRegionKind::Stack,
    });
    vmas.push(VmRegion {
        start: elf::USER_STACK_GUARD_ADDR,
        end: elf::USER_STACK_MAX_BOTTOM,
        writable: false,
        executable: false,
        kind: VmRegionKind::StackGuard,
    });

    without_interrupts(|| {
        let mut sched = USER_SCHED.lock();
        let start_vr = sched.min_runnable_vruntime();
        if let Some(empty_idx) = sched.slots.iter().position(|s| s.is_none()) {
            sched.slots[empty_idx] = Some(Box::new(UserProcessSlot {
                pid,
                tgid: pid,
                is_thread: false,
                parent_pid: 0,
                abi,
                name: String::from(name),
                pml4,
                ctx,
                fpu_state: FxSaveArea::new_clean(),
                fs_base: 0,
                gs_base: 0,
                clear_child_tid: 0,
                signals: SignalState::new(),
                state: UserTaskState::Ready,
                cpu_ms: 0,
                vruntime_ms: start_vr,
                weight: 1024,
                preempt_count: 0,
                lazy_faults: 0,
                syscalls_served: 0,
                entry_point,
                pages_mapped,
                bytes_loaded,
                brk_base,
                brk_current: brk_base,
                mmap_next: USER_MMAP_BASE,
                shm_attach_next: USER_SHM_BASE,
                shm_attachments: Vec::new(),
                owned_shm_ids: Vec::new(),
                dynamic_bytes: 0,
                vmas,
                image: Some(image),
                fds: [const { None }; MAX_PROC_FDS],
                port_rights: Vec::new(),
                next_port_name: 1,
                stdout_buf: String::new(),
                has_gui_window: false,
            }));
            Ok(())
        } else {
            Err(image)
        }
    })
}

pub fn queue_spawn_from_user(cmdline: &str) -> Result<(), i64> {
    let tokens: Vec<String> = cmdline
        .split_whitespace()
        .take(16)
        .map(String::from)
        .collect();
    if tokens.is_empty() || tokens[0].len() > 120 {
        return Err(-22);
    }
    without_interrupts(|| {
        let mut sched = USER_SCHED.lock();
        if sched.pending_spawns.len() >= 4 {
            return Err(-11);
        }
        sched.pending_spawns.push(tokens);
        Ok(())
    })
}

pub fn push_current_stdout(text: &str) {
    if text.is_empty() {
        return;
    }
    without_interrupts(|| {
        let mut sched = USER_SCHED.lock();
        if let Some(idx) = sched.current_slot {
            if let Some(ref mut proc) = sched.slots[idx] {
                if proc.stdout_buf.len() < 8192 {
                    let take = text.len().min(8192 - proc.stdout_buf.len());
                    proc.stdout_buf.push_str(&text[..take]);
                }
            }
        }
    });
}

pub fn mark_current_gui_window() {
    without_interrupts(|| {
        let mut sched = USER_SCHED.lock();
        if let Some(idx) = sched.current_slot {
            if let Some(ref mut proc) = sched.slots[idx] {
                proc.has_gui_window = true;
            }
        }
    });
}

pub fn alloc_current_fd(entry: FdEntry) -> Option<usize> {
    without_interrupts(|| {
        let mut sched = USER_SCHED.lock();
        let idx = sched.current_slot?;
        let proc = sched.slots[idx].as_mut()?;
        let free = proc.fds.iter().position(|f| f.is_none())?;
        proc.fds[free] = Some(entry);
        Some(free + FD_OFFSET)
    })
}

pub fn with_current_fd_mut<F, R>(fd: usize, f: F) -> Option<R>
where
    F: FnOnce(&mut FdEntry) -> R,
{
    if fd < FD_OFFSET || fd >= FD_OFFSET + MAX_PROC_FDS {
        return None;
    }
    without_interrupts(|| {
        let mut sched = USER_SCHED.lock();
        let idx = sched.current_slot?;
        let proc = sched.slots[idx].as_mut()?;
        let entry = proc.fds[fd - FD_OFFSET].as_mut()?;
        Some(f(entry))
    })
}

pub fn get_current_fd_socket(fd: usize) -> Option<OpenSocket> {
    if fd < FD_OFFSET || fd >= FD_OFFSET + MAX_PROC_FDS {
        return None;
    }
    without_interrupts(|| {
        let sched = USER_SCHED.lock();
        let idx = sched.current_slot?;
        let proc = sched.slots[idx].as_ref()?;
        match proc.fds[fd - FD_OFFSET] {
            Some(FdEntry::Socket(sock)) => Some(sock),
            _ => None,
        }
    })
}

pub fn set_current_fd_socket(fd: usize, sock: OpenSocket) -> bool {
    if fd < FD_OFFSET || fd >= FD_OFFSET + MAX_PROC_FDS {
        return false;
    }
    without_interrupts(|| {
        let mut sched = USER_SCHED.lock();
        let Some(idx) = sched.current_slot else { return false };
        let Some(ref mut proc) = sched.slots[idx] else { return false };
        proc.fds[fd - FD_OFFSET] = Some(FdEntry::Socket(sock));
        true
    })
}

pub fn take_current_fd(fd: usize) -> Option<FdEntry> {
    if fd < FD_OFFSET || fd >= FD_OFFSET + MAX_PROC_FDS {
        return None;
    }
    without_interrupts(|| {
        let mut sched = USER_SCHED.lock();
        let idx = sched.current_slot?;
        let proc = sched.slots[idx].as_mut()?;
        proc.fds[fd - FD_OFFSET].take()
    })
}

fn map_zeroed_user_pages(
    image: &mut LoadedElfImage,
    pml4: u64,
    start_page: u64,
    page_count: usize,
    flags: PageTableFlags,
    mapping: &PhysicalMemoryMapping,
    allocator: &mut PhysicalFrameAllocator<'_>,
) -> Result<(), usize> {
    for i in 0..page_count {
        let vaddr = start_page + (i as u64) * PAGE_SIZE;
        let virt = VirtAddr::try_new(vaddr).map_err(|_| i)?;
        if mapping.translate_virtual_address_in(pml4, virt).is_some() {
            return Err(i);
        }
        let frame = allocator.allocate_frame().ok_or(i)?;
        let Some(hhdm) = mapping.translate_frame(frame) else {
            let _ = allocator.deallocate_frame(frame);
            return Err(i);
        };
        unsafe {
            write_bytes(hhdm.as_u64() as *mut u8, 0, PAGE_SIZE as usize);
        }
        if mapping.map_page_in(allocator, pml4, virt, frame, flags).is_err() {
            let _ = allocator.deallocate_frame(frame);
            return Err(i);
        }
        image.mapped_pages.push((vaddr, frame));
    }
    Ok(())
}

fn unmap_user_page_range(
    image: &mut LoadedElfImage,
    pml4: u64,
    start_page: u64,
    page_count: usize,
    mapping: &PhysicalMemoryMapping,
    allocator: &mut PhysicalFrameAllocator<'_>,
) -> usize {
    let mut freed = 0usize;
    for i in 0..page_count {
        let vaddr = start_page + (i as u64) * PAGE_SIZE;
        if let Some(pos) = image.mapped_pages.iter().position(|&(v, _)| v == vaddr) {
            let (_, recorded_frame) = image.mapped_pages.swap_remove(pos);
            let actual_frame = if let Ok(virt) = VirtAddr::try_new(vaddr) {
                mapping.unmap_page_in(pml4, virt).unwrap_or(recorded_frame)
            } else {
                recorded_frame
            };
            let _ = allocator.deallocate_frame(actual_frame);
            freed += 1;
        }
    }
    freed
}

fn commit_single_lazy_page_for_slot(
    slot_idx: usize,
    page_vaddr: u64,
    vma: VmRegion,
    mapping: &PhysicalMemoryMapping,
    allocator: &mut PhysicalFrameAllocator<'_>,
) -> bool {
    let setup = without_interrupts(|| {
        let mut sched = USER_SCHED.lock();
        let proc = sched.slots[slot_idx].as_mut()?;
        let pml4 = proc.pml4;
        let img = proc.image.take()?;
        Some((proc.pid, pml4, img))
    });

    let Some((_pid, pml4, mut img)) = setup else {
        return false;
    };

    let Ok(virt) = VirtAddr::try_new(page_vaddr) else {
        without_interrupts(|| {
            let mut sched = USER_SCHED.lock();
            if let Some(ref mut proc) = sched.slots[slot_idx] {
                proc.image = Some(img);
            }
        });
        return false;
    };

    if mapping.translate_virtual_address_in(pml4, virt).is_some() {
        without_interrupts(|| {
            let mut sched = USER_SCHED.lock();
            if let Some(ref mut proc) = sched.slots[slot_idx] {
                proc.image = Some(img);
            }
        });
        return true;
    }

    let mut flags = PageTableFlags::PRESENT | PageTableFlags::USER_ACCESSIBLE;
    if vma.writable {
        flags |= PageTableFlags::WRITABLE;
    }
    if syscall::nxe_enabled() && !vma.executable {
        flags |= PageTableFlags::NO_EXECUTE;
    }

    let ok = map_zeroed_user_pages(&mut img, pml4, page_vaddr, 1, flags, mapping, allocator).is_ok();
    let total_pages = img.mapped_pages.len();

    without_interrupts(|| {
        let mut sched = USER_SCHED.lock();
        if let Some(ref mut proc) = sched.slots[slot_idx] {
            if ok {
                proc.lazy_faults = proc.lazy_faults.saturating_add(1);
                proc.dynamic_bytes = proc.dynamic_bytes.saturating_add(PAGE_SIZE as usize);
                proc.pages_mapped = total_pages;
            }
            proc.image = Some(img);
        }
    });

    if ok {
        klog::record_pf_lazy_commit();
        unsafe {
            asm!("invlpg [{}]", in(reg) page_vaddr, options(nostack, preserves_flags));
        }
    }
    ok
}

/// Resolves a Ring 3 `#PF` against registered VMAs (Copy-on-Write write faults & lazy demand-paged
/// `mmap`/`stack` growth) or identifies guard-page stack overflow.
pub fn try_resolve_user_page_fault(
    cr2: u64,
    error_code: u64,
    mapping: &PhysicalMemoryMapping,
    allocator: &mut PhysicalFrameAllocator<'_>,
) -> FaultResolveOutcome {
    if cr2 == 0 || cr2 >= USER_ADDR_LIMIT {
        return FaultResolveOutcome::NotHandled;
    }

    let info = without_interrupts(|| {
        let sched = USER_SCHED.lock();
        let idx = sched.current_slot?;
        let proc = sched.slots[idx].as_ref()?;
        let vma = proc.find_vma(cr2);
        Some((idx, proc.pid, proc.pml4, vma))
    });

    let Some((slot_idx, pid, pml4, Some(vma))) = info else {
        return FaultResolveOutcome::NotHandled;
    };

    if vma.kind == VmRegionKind::StackGuard {
        klog::record_pf_guard_fault();
        klog::log_fmt(
            KLogLevel::Fault,
            "pf",
            format_args!(
                "PID={} GUARD_PAGE_HIT cr2=0x{:x} err=0x{:x}",
                pid, cr2, error_code
            ),
        );
        return FaultResolveOutcome::GuardPageHit;
    }

    let page_vaddr = cr2 & !(PAGE_SIZE - 1);

    // Present protection fault (bit 0 == 1): handle Copy-on-Write write faults on writable private VMAs.
    if (error_code & 0x1) != 0 {
        if (error_code & 0x2) != 0
            && (error_code & 0x10) == 0
            && vma.writable
            && vma.kind != VmRegionKind::SharedMem
        {
            if let Ok(copied) = mapping.resolve_cow_fault_in(pml4, page_vaddr, allocator) {
                if copied {
                    klog::record_pf_cow_copy();
                }
                return FaultResolveOutcome::Resolved;
            }
        }
        return FaultResolveOutcome::NotHandled;
    }

    if !matches!(
        vma.kind,
        VmRegionKind::LazyMmap | VmRegionKind::Stack | VmRegionKind::LazyHeap
    ) {
        return FaultResolveOutcome::NotHandled;
    }

    // Bit 1 == 1 means write access on a read-only VMA.
    if (error_code & 0x2) != 0 && !vma.writable {
        return FaultResolveOutcome::NotHandled;
    }
    // Bit 4 == 1 means instruction fetch on a non-executable VMA.
    if (error_code & 0x10) != 0 && !vma.executable {
        return FaultResolveOutcome::NotHandled;
    }

    if commit_single_lazy_page_for_slot(slot_idx, page_vaddr, vma, mapping, allocator) {
        FaultResolveOutcome::Resolved
    } else {
        FaultResolveOutcome::NotHandled
    }
}

/// Ensures every 4 KiB page in `[first_page ..= last_page]` inside `user_pml4` is mapped
/// and satisfies user/writable permissions, resolving CoW pages or demand-paging lazy VMAs if `allocator` is provided.
pub fn ensure_user_pages_committed(
    user_pml4: u64,
    first_page: u64,
    last_page: u64,
    require_writable: bool,
    mapping: &PhysicalMemoryMapping,
    mut allocator: Option<&mut PhysicalFrameAllocator<'_>>,
) -> Result<(), i64> {
    let mut page = first_page;
    while page <= last_page {
        let virt = VirtAddr::try_new(page).map_err(|_| -14)?;
        if let Some(trans) = mapping.translate_virtual_address_in(user_pml4, virt) {
            if !trans.flags.contains(PageTableFlags::USER_ACCESSIBLE) {
                return Err(-14);
            }
            if require_writable && !trans.flags.contains(PageTableFlags::WRITABLE) {
                if trans.flags.contains(COW_FLAG) {
                    let Some(alloc_ref) = allocator.as_deref_mut() else {
                        return Err(-14);
                    };
                    let writable_private = without_interrupts(|| {
                        let sched = USER_SCHED.lock();
                        let idx = sched.find_slot_by_pml4(user_pml4)?;
                        let proc = sched.slots[idx].as_ref()?;
                        let vma = proc.find_vma(page)?;
                        Some(vma.writable && vma.kind != VmRegionKind::SharedMem)
                    })
                    .unwrap_or(false);
                    if !writable_private {
                        return Err(-14);
                    }
                    match mapping.resolve_cow_fault_in(user_pml4, page, alloc_ref) {
                        Ok(copied) => {
                            if copied {
                                klog::record_pf_cow_copy();
                            }
                        }
                        Err(_) => return Err(-14),
                    }
                } else {
                    return Err(-14);
                }
            }
        } else {
            let Some(alloc_ref) = allocator.as_deref_mut() else {
                return Err(-14);
            };
            let candidate = without_interrupts(|| {
                let sched = USER_SCHED.lock();
                let idx = sched.find_slot_by_pml4(user_pml4)?;
                let proc = sched.slots[idx].as_ref()?;
                let vma = proc.find_vma(page)?;
                Some((idx, vma))
            });
            let Some((slot_idx, vma)) = candidate else {
                return Err(-14);
            };
            if vma.kind == VmRegionKind::StackGuard {
                return Err(-14);
            }
            if !matches!(
                vma.kind,
                VmRegionKind::LazyMmap | VmRegionKind::Stack | VmRegionKind::LazyHeap
            ) {
                return Err(-14);
            }
            if require_writable && !vma.writable {
                return Err(-14);
            }
            if !commit_single_lazy_page_for_slot(slot_idx, page, vma, mapping, alloc_ref) {
                return Err(-14);
            }
        }
        page = page.saturating_add(PAGE_SIZE);
    }
    Ok(())
}

pub fn sys_brk_current(
    requested: u64,
    mapping: &PhysicalMemoryMapping,
    allocator: &mut PhysicalFrameAllocator<'_>,
) -> i64 {
    let info = without_interrupts(|| {
        let mut sched = USER_SCHED.lock();
        let Some(idx) = sched.current_slot else { return None };
        let Some(ref mut proc) = sched.slots[idx] else { return None };
        let cur = proc.brk_current;
        let base = proc.brk_base;
        if requested == 0 || requested < base || requested >= USER_HEAP_LIMIT {
            return Some(Err(cur as i64));
        }
        let pml4 = proc.pml4;
        let img = proc.image.take();
        let dyn_bytes = proc.dynamic_bytes;
        Some(Ok((idx, pml4, base, cur, dyn_bytes, img)))
    });

    let Some(res) = info else { return -22 };
    let (idx, pml4, _base, old_brk, dyn_bytes, Some(mut img)) = (match res {
        Err(cur_ret) => return cur_ret,
        Ok(v) => v,
    }) else {
        return -22;
    };

    let old_top_page = (old_brk + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
    let new_top_page = (requested + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
    let mut final_brk = old_brk;
    let mut delta_dyn: isize = 0;

    if new_top_page > old_top_page {
        let add_pages = ((new_top_page - old_top_page) / PAGE_SIZE) as usize;
        let add_bytes = add_pages * PAGE_SIZE as usize;
        if dyn_bytes + add_bytes <= MAX_DYNAMIC_USER_BYTES {
            let mut flags =
                PageTableFlags::PRESENT | PageTableFlags::USER_ACCESSIBLE | PageTableFlags::WRITABLE;
            if syscall::nxe_enabled() {
                flags |= PageTableFlags::NO_EXECUTE;
            }
            match map_zeroed_user_pages(&mut img, pml4, old_top_page, add_pages, flags, mapping, allocator) {
                Ok(()) => {
                    final_brk = requested;
                    delta_dyn = add_bytes as isize;
                }
                Err(partial) => {
                    unmap_user_page_range(&mut img, pml4, old_top_page, partial, mapping, allocator);
                }
            }
        }
    } else if new_top_page < old_top_page {
        let rem_pages = ((old_top_page - new_top_page) / PAGE_SIZE) as usize;
        let freed = unmap_user_page_range(&mut img, pml4, new_top_page, rem_pages, mapping, allocator);
        final_brk = requested;
        delta_dyn = -((freed * PAGE_SIZE as usize) as isize);
    } else {
        final_brk = requested;
    }

    let total_pages = img.mapped_pages.len();
    without_interrupts(|| {
        let mut sched = USER_SCHED.lock();
        if let Some(ref mut proc) = sched.slots[idx] {
            proc.brk_current = final_brk;
            proc.pages_mapped = total_pages;
            proc.dynamic_bytes = (proc.dynamic_bytes as isize + delta_dyn).max(0) as usize;
            proc.image = Some(img);
        }
    });

    final_brk as i64
}

pub fn sys_mmap_current(
    len: usize,
    prot: u64,
    _mapping: &PhysicalMemoryMapping,
    _allocator: &mut PhysicalFrameAllocator<'_>,
) -> i64 {
    if len == 0 || len > 32 * 1024 * 1024 {
        return -22; // EINVAL
    }
    let pages = (len + PAGE_SIZE as usize - 1) / PAGE_SIZE as usize;
    let aligned_bytes = pages * PAGE_SIZE as usize;

    let res = without_interrupts(|| {
        let mut sched = USER_SCHED.lock();
        let Some(idx) = sched.current_slot else { return Err(-22) };
        let Some(proc) = sched.slots[idx].as_mut() else { return Err(-22) };
        if proc.dynamic_bytes + aligned_bytes > MAX_DYNAMIC_USER_BYTES
            || proc.total_dynamic_vma_bytes() + aligned_bytes > MAX_DYNAMIC_USER_BYTES
        {
            return Err(-12); // ENOMEM
        }
        let vaddr = proc.mmap_next;
        let Some(next) = vaddr.checked_add(aligned_bytes as u64) else {
            return Err(-12);
        };
        if next >= USER_MMAP_LIMIT || next >= USER_ADDR_LIMIT {
            return Err(-12);
        }
        proc.mmap_next = next;
        proc.vmas.push(VmRegion {
            start: vaddr,
            end: next,
            writable: (prot & 2) != 0,
            executable: (prot & 4) != 0,
            kind: VmRegionKind::LazyMmap,
        });
        Ok((proc.pid, vaddr))
    });

    match res {
        Ok((pid, vaddr)) => {
            klog::log_fmt(
                KLogLevel::Mm,
                "mmap",
                format_args!("PID={} lazy mmap 0x{:x} ({}p)", pid, vaddr, pages),
            );
            vaddr as i64
        }
        Err(e) => e,
    }
}

pub fn sys_munmap_current(
    addr: u64,
    len: usize,
    mapping: &PhysicalMemoryMapping,
    allocator: &mut PhysicalFrameAllocator<'_>,
) -> i64 {
    if addr < USER_MMAP_BASE || (addr & (PAGE_SIZE - 1)) != 0 || len == 0 {
        return -22;
    }
    let pages = (len + PAGE_SIZE as usize - 1) / PAGE_SIZE as usize;
    let end = addr.saturating_add((pages as u64) * PAGE_SIZE);

    let setup = without_interrupts(|| {
        let mut sched = USER_SCHED.lock();
        let idx = sched.current_slot?;
        let proc = sched.slots[idx].as_mut()?;
        let pml4 = proc.pml4;
        let img = proc.image.take()?;
        Some((idx, pml4, img))
    });

    let Some((idx, pml4, mut img)) = setup else { return -22 };
    let freed = unmap_user_page_range(&mut img, pml4, addr, pages, mapping, allocator);
    let freed_bytes = freed * PAGE_SIZE as usize;
    let total_pages = img.mapped_pages.len();

    without_interrupts(|| {
        let mut sched = USER_SCHED.lock();
        if let Some(ref mut proc) = sched.slots[idx] {
            proc.remove_mmap_range(addr, end);
            proc.dynamic_bytes = proc.dynamic_bytes.saturating_sub(freed_bytes);
            proc.pages_mapped = total_pages;
            proc.image = Some(img);
        }
    });
    0
}

pub fn record_current_shm_owner(shm_id: u64) {
    without_interrupts(|| {
        let mut sched = USER_SCHED.lock();
        if let Some(idx) = sched.current_slot {
            if let Some(ref mut proc) = sched.slots[idx] {
                if !proc.owned_shm_ids.contains(&shm_id) {
                    proc.owned_shm_ids.push(shm_id);
                }
            }
        }
    });
}

pub fn sys_shm_map_current(
    shm_id: u64,
    writable: bool,
    mapping: &PhysicalMemoryMapping,
    allocator: &mut PhysicalFrameAllocator<'_>,
) -> i64 {
    let Some(frames) = super::ipc::retain_shm_frames(shm_id) else {
        return -2; // ENOENT
    };
    let page_count = frames.len();
    if page_count == 0 {
        super::ipc::release_shm(shm_id, allocator);
        return -22;
    }
    let byte_len = (page_count as u64) * PAGE_SIZE;

    let setup = without_interrupts(|| {
        let mut sched = USER_SCHED.lock();
        let idx = sched.current_slot?;
        let proc = sched.slots[idx].as_mut()?;
        if let Some(existing) = proc.shm_attachments.iter().find(|a| a.shm_id == shm_id) {
            return Some(Ok((idx, proc.pml4, existing.vaddr, true)));
        }
        let vaddr = proc.shm_attach_next;
        let next = vaddr.checked_add(byte_len)?;
        if next >= USER_SHM_LIMIT || next >= USER_ADDR_LIMIT {
            return Some(Err(-12));
        }
        Some(Ok((idx, proc.pml4, vaddr, false)))
    });

    let Some(res) = setup else {
        super::ipc::release_shm(shm_id, allocator);
        return -22;
    };
    let (idx, pml4, vaddr, already_mapped) = match res {
        Ok(v) => v,
        Err(e) => {
            super::ipc::release_shm(shm_id, allocator);
            return e;
        }
    };

    if already_mapped {
        super::ipc::release_shm(shm_id, allocator);
        return vaddr as i64;
    }

    let mut flags = PageTableFlags::PRESENT | PageTableFlags::USER_ACCESSIBLE;
    if writable {
        flags |= PageTableFlags::WRITABLE;
    }
    if syscall::nxe_enabled() {
        flags |= PageTableFlags::NO_EXECUTE;
    }

    let mut mapped = 0usize;
    let mut map_failed = false;
    for (i, &frame) in frames.iter().enumerate() {
        let va = vaddr + (i as u64) * PAGE_SIZE;
        let Ok(virt) = VirtAddr::try_new(va) else {
            map_failed = true;
            break;
        };
        if mapping.map_page_in(allocator, pml4, virt, frame, flags).is_err() {
            map_failed = true;
            break;
        }
        mapped += 1;
    }

    if map_failed {
        for i in 0..mapped {
            if let Ok(virt) = VirtAddr::try_new(vaddr + (i as u64) * PAGE_SIZE) {
                let _ = mapping.unmap_page_in(pml4, virt);
            }
        }
        super::ipc::release_shm(shm_id, allocator);
        return -12;
    }

    without_interrupts(|| {
        let mut sched = USER_SCHED.lock();
        if let Some(ref mut proc) = sched.slots[idx] {
            proc.shm_attach_next = vaddr + byte_len;
            proc.shm_attachments.push(ShmAttachment {
                shm_id,
                vaddr,
                page_count,
            });
            proc.vmas.push(VmRegion {
                start: vaddr,
                end: vaddr + byte_len,
                writable,
                executable: false,
                kind: VmRegionKind::SharedMem,
            });
            proc.pages_mapped = proc.pages_mapped.saturating_add(page_count);
        }
    });
    vaddr as i64
}

pub fn sys_shm_unmap_current(
    vaddr: u64,
    mapping: &PhysicalMemoryMapping,
    allocator: &mut PhysicalFrameAllocator<'_>,
) -> i64 {
    let removed = without_interrupts(|| {
        let mut sched = USER_SCHED.lock();
        let idx = sched.current_slot?;
        let proc = sched.slots[idx].as_mut()?;
        let pos = proc.shm_attachments.iter().position(|a| a.vaddr == vaddr)?;
        let att = proc.shm_attachments.swap_remove(pos);
        proc.pages_mapped = proc.pages_mapped.saturating_sub(att.page_count);
        proc.vmas
            .retain(|r| !(r.kind == VmRegionKind::SharedMem && r.start == att.vaddr));
        Some((proc.pml4, att))
    });

    let Some((pml4, att)) = removed else {
        return -22;
    };

    for i in 0..att.page_count {
        if let Ok(virt) = VirtAddr::try_new(att.vaddr + (i as u64) * PAGE_SIZE) {
            let _ = mapping.unmap_page_in(pml4, virt);
        }
    }
    super::ipc::release_shm(att.shm_id, allocator);
    0
}

/// Called from `rust_apic_timer_irq_dispatch` (or PIT fallback `rust_timer_irq_dispatch`) inside IRQ (`IF = 0`).
/// NO heap allocations, NO desktop/process locks!
/// Returns `1` when preemption occurred and execution must trampoline out of Ring 3.
pub fn on_timer_irq_preempt(ctx: *mut UserCpuContext, now_ms: u64) -> u64 {
    if !QUANTUM_ACTIVE.load(Ordering::Acquire) || ctx.is_null() {
        return 0;
    }
    let Some(mut sched) = USER_SCHED.try_lock() else {
        return 0;
    };
    let Some(cur_idx) = sched.current_slot else {
        return 0;
    };
    if now_ms < sched.quantum_deadline_ms {
        return 0;
    }

    let start_ms = sched.quantum_start_ms;
    if let Some(ref mut proc) = sched.slots[cur_idx] {
        if proc.state == UserTaskState::Running {
            proc.ctx = unsafe { *ctx };
            proc.ctx.rflags = (proc.ctx.rflags & 0x0020_0CD5) | 0x0202;
            proc.state = UserTaskState::Ready;
            proc.preempt_count = proc.preempt_count.saturating_add(1);
            let elapsed = now_ms.saturating_sub(start_ms).max(1);
            proc.charge_runtime(elapsed);
            klog::record_user_preempt();
        }
    }
    1
}

pub fn record_current_syscall() {
    without_interrupts(|| {
        let mut sched = USER_SCHED.lock();
        if let Some(idx) = sched.current_slot {
            if let Some(ref mut proc) = sched.slots[idx] {
                proc.syscalls_served = proc.syscalls_served.saturating_add(1);
            }
        }
    });
}

/// Suspends current Ring 3 syscall caller with `saved_ctx` & `next_state`, then exits quantum.
pub unsafe fn suspend_current_from_syscall(
    saved_ctx: UserCpuContext,
    next_state: UserTaskState,
) -> ! {
    let now_ms = timer::uptime_ms();
    {
        let mut sched = USER_SCHED.lock();
        let start_ms = sched.quantum_start_ms;
        if let Some(idx) = sched.current_slot {
            if let Some(ref mut proc) = sched.slots[idx] {
                proc.ctx = saved_ctx;
                proc.state = next_state;
                let elapsed = now_ms.saturating_sub(start_ms);
                proc.charge_runtime(elapsed);
            }
        }
    }
    syscall::exit_quantum_trampoline();
}

pub unsafe fn finish_current_from_syscall(exit_code: i64) -> ! {
    let now_ms = timer::uptime_ms();
    {
        let mut sched = USER_SCHED.lock();
        let start_ms = sched.quantum_start_ms;
        if let Some(idx) = sched.current_slot {
            if let Some(ref mut proc) = sched.slots[idx] {
                proc.state = UserTaskState::Exited(exit_code);
                let elapsed = now_ms.saturating_sub(start_ms);
                proc.charge_runtime(elapsed);
            }
        }
    }
    syscall::exit_quantum_trampoline();
}

pub unsafe fn abort_current_fault(fault: UserFaultInfo) -> ! {
    let now_ms = timer::uptime_ms();
    {
        let mut sched = USER_SCHED.lock();
        let start_ms = sched.quantum_start_ms;
        if let Some(idx) = sched.current_slot {
            if let Some(ref mut proc) = sched.slots[idx] {
                proc.state = UserTaskState::Faulted(fault);
                let elapsed = now_ms.saturating_sub(start_ms);
                proc.charge_runtime(elapsed);
            }
        }
    }
    syscall::exit_quantum_trampoline();
}

/// Polls all non-blocking Ring 3 wait queues (`Sleep`, `GuiEvent`, `TcpConnect`, `NetJob`, `PipeRead`, `Kqueue`, `UmtxWait`)
/// without holding `USER_SCHED` across external locks.
pub fn poll_wait_queues(
    mapping: &PhysicalMemoryMapping,
    allocator: &mut PhysicalFrameAllocator<'_>,
) {
    let now_ms = timer::uptime_ms();
    let waiters: Vec<(usize, usize, usize, u64, WaitReason, bool)> = without_interrupts(|| {
        let sched = USER_SCHED.lock();
        let mut list = Vec::new();
        for (idx, slot) in sched.slots.iter().enumerate() {
            if let Some(ref proc) = slot {
                if let UserTaskState::Waiting(reason) = proc.state {
                    let sig_ready = proc
                        .signals
                        .next_deliverable()
                        .map(|s| proc.signals.is_interrupting_signal(s))
                        .unwrap_or(false);
                    list.push((idx, proc.pid, proc.tgid, proc.pml4, reason, sig_ready));
                }
            }
        }
        list
    });

    if waiters.is_empty() {
        return;
    }

    for (slot_idx, pid, tgid, pml4, reason, sig_ready) in waiters {
        let mut wake_ret: Option<i64> = None;
        let mut update_tcp_fd: Option<(usize, usize)> = None;

        if sig_ready {
            wake_ret = Some(-4); // EINTR
        } else {
            match reason {
                WaitReason::Sleep { wake_ms } => {
                    if now_ms >= wake_ms {
                        wake_ret = Some(0);
                    }
                }
                WaitReason::GuiEvent {
                    win_id,
                    evt_ptr,
                    deadline_ms,
                } => {
                    let evt = crate::graphics::wm::pop_external_event_from_syscall(win_id);
                    if evt.kind != 0 || now_ms >= deadline_ms {
                        let evt_bytes = unsafe {
                            core::slice::from_raw_parts(
                                (&evt as *const crate::graphics::wm::UserGuiEvent) as *const u8,
                                core::mem::size_of::<crate::graphics::wm::UserGuiEvent>(),
                            )
                        };
                        if syscall::copy_to_user_bytes_in(
                            mapping,
                            Some(allocator),
                            pml4,
                            evt_ptr,
                            evt_bytes,
                        )
                        .is_err()
                        {
                            wake_ret = Some(-14); // EFAULT
                        } else if evt.kind != 0 {
                            wake_ret = Some(1);
                        } else {
                            wake_ret = Some(0);
                        }
                    }
                }
                WaitReason::TcpConnect {
                    fd,
                    handle,
                    deadline_ms,
                } => match crate::net::tcp::poll_connect(handle) {
                    Ok(true) => {
                        update_tcp_fd = Some((fd, handle));
                        wake_ret = Some(0);
                    }
                    Ok(false) => {
                        if now_ms >= deadline_ms {
                            crate::net::tcp::close_socket(handle);
                            wake_ret = Some(-110); // ETIMEDOUT
                        }
                    }
                    Err(_) => {
                        crate::net::tcp::close_socket(handle);
                        wake_ret = Some(-111); // ECONNREFUSED
                    }
                },
                WaitReason::NetJob {
                    job_id,
                    kind,
                    out_ptr,
                    out_cap,
                    deadline_ms,
                } => {
                    if let Some(outcome) = kthread::take_net_job_result(job_id) {
                        let ret = match (kind, outcome) {
                            (NetWaitKind::Ping, NetJobOutcome::Ping(res)) => match res {
                                Ok(reply) => reply.rtt_ms as i64,
                                Err(_) => -110,
                            },
                            (NetWaitKind::DnsResolve, NetJobOutcome::DnsResolve(res)) => {
                                match res {
                                    Ok(ip) => {
                                        if syscall::copy_to_user_bytes_in(
                                            mapping,
                                            Some(allocator),
                                            pml4,
                                            out_ptr,
                                            &ip,
                                        )
                                        .is_err()
                                        {
                                            -14
                                        } else {
                                            0
                                        }
                                    }
                                    Err(_) => -110,
                                }
                            }
                            (NetWaitKind::HttpGet, NetJobOutcome::HttpGet { result }) => {
                                match result {
                                    Ok((_ip, _total, body)) => {
                                        let n = body.len().min(out_cap);
                                        if syscall::copy_to_user_bytes_in(
                                            mapping,
                                            Some(allocator),
                                            pml4,
                                            out_ptr,
                                            &body[..n],
                                        )
                                        .is_err()
                                        {
                                            -14
                                        } else {
                                            n as i64
                                        }
                                    }
                                    Err(_) => -110,
                                }
                            }
                            _ => -22,
                        };
                        wake_ret = Some(ret);
                    } else if now_ms >= deadline_ms {
                        wake_ret = Some(-110);
                    }
                }
                WaitReason::PipeRead {
                    pipe_id,
                    buf_ptr,
                    cap,
                    deadline_ms,
                } => {
                    let max_read = cap.min(4096);
                    let mut kbuf = alloc::vec![0u8; max_read];
                    match super::ipc::read_pipe(pipe_id, &mut kbuf) {
                        Ok(n) => {
                            if n > 0
                                && syscall::copy_to_user_bytes_in(
                                    mapping,
                                    Some(allocator),
                                    pml4,
                                    buf_ptr,
                                    &kbuf[..n],
                                )
                                .is_err()
                            {
                                wake_ret = Some(-14);
                            } else {
                                wake_ret = Some(n as i64);
                            }
                        }
                        Err(-11) => {
                            if now_ms >= deadline_ms {
                                wake_ret = Some(-11);
                            }
                        }
                        Err(e) => {
                            wake_ret = Some(e);
                        }
                    }
                }
                WaitReason::WaitPid {
                    target_pid,
                    status_ptr,
                } => {
                    let reaped = without_interrupts(|| {
                        let mut sched = USER_SCHED.lock();
                        if let Some(pos) = sched.exited_for_wait.iter().position(|&(ppid, cpid, _)| {
                            ppid == tgid && (target_pid <= 0 || cpid == target_pid as usize)
                        }) {
                            let (_ppid, cpid, code) = sched.exited_for_wait.swap_remove(pos);
                            Some(Ok((cpid, code)))
                        } else if !sched.has_live_child_matching(tgid, target_pid) {
                            Some(Err(-10)) // ECHILD
                        } else {
                            None
                        }
                    });
                    match reaped {
                        Some(Ok((cpid, code))) => {
                            if status_ptr != 0 {
                                let status_bytes = (code as i32).to_le_bytes();
                                if syscall::copy_to_user_bytes_in(
                                    mapping,
                                    Some(allocator),
                                    pml4,
                                    status_ptr,
                                    &status_bytes,
                                )
                                .is_err()
                                {
                                    wake_ret = Some(-14);
                                } else {
                                    wake_ret = Some(cpid as i64);
                                }
                            } else {
                                wake_ret = Some(cpid as i64);
                            }
                        }
                        Some(Err(e)) => {
                            wake_ret = Some(e);
                        }
                        None => {}
                    }
                }
                WaitReason::MachPortRecv {
                    port_id,
                    rcv_name,
                    msg_ptr,
                    rcv_cap,
                    deadline_ms,
                } => match ipc::mach_try_recv_msg(port_id) {
                    Ok(msg) => {
                        wake_ret = Some(deliver_mach_msg_to_slot(
                            slot_idx,
                            pid,
                            pml4,
                            port_id,
                            rcv_name,
                            msg,
                            msg_ptr,
                            rcv_cap,
                            mapping,
                            allocator,
                        ));
                    }
                    Err(-11) => {
                        if now_ms >= deadline_ms {
                            wake_ret = Some(-110); // ETIMEDOUT
                        }
                    }
                    Err(e) => {
                        wake_ret = Some(e);
                    }
                },
                WaitReason::Kqueue {
                    kq_id,
                    eventlist_ptr,
                    nevents,
                    deadline_ms,
                } => {
                    let owner_fds = without_interrupts(|| {
                        let sched = USER_SCHED.lock();
                        sched.slots[slot_idx]
                            .as_ref()
                            .map(|p| Box::new(p.fds.clone()))
                            .unwrap_or_else(|| Box::new([const { None }; MAX_PROC_FDS]))
                    });
                    match kqueue::poll_events(
                        kq_id,
                        owner_fds.as_slice(),
                        nevents,
                        now_ms,
                        &|target| is_process_alive(target),
                    ) {
                        Ok(events) if !events.is_empty() => {
                            let elem_sz = core::mem::size_of::<KEvent>();
                            let raw_bytes = unsafe {
                                core::slice::from_raw_parts(
                                    events.as_ptr() as *const u8,
                                    events.len() * elem_sz,
                                )
                            };
                            if syscall::copy_to_user_bytes_in(
                                mapping,
                                Some(allocator),
                                pml4,
                                eventlist_ptr,
                                raw_bytes,
                            )
                            .is_err()
                            {
                                wake_ret = Some(-14);
                            } else {
                                wake_ret = Some(events.len() as i64);
                            }
                        }
                        Ok(_) => {
                            if now_ms >= deadline_ms {
                                wake_ret = Some(0);
                            }
                        }
                        Err(e) => {
                            wake_ret = Some(e);
                        }
                    }
                }
                WaitReason::UmtxWait {
                    pml4: wait_pml4,
                    addr,
                    val,
                    is_u64,
                    deadline_ms,
                } => {
                    let mut raw = [0u8; 8];
                    let sz = if is_u64 { 8 } else { 4 };
                    if syscall::copy_from_user_bytes_in(
                        mapping,
                        Some(allocator),
                        wait_pml4,
                        addr,
                        &mut raw[..sz],
                    )
                    .is_err()
                    {
                        wake_ret = Some(-14);
                    } else {
                        let cur = if is_u64 {
                            u64::from_le_bytes(raw)
                        } else {
                            u32::from_le_bytes(raw[0..4].try_into().unwrap()) as u64
                        };
                        if cur != val {
                            wake_ret = Some(0);
                        } else if now_ms >= deadline_ms {
                            wake_ret = Some(-110);
                        }
                    }
                }
            }
        }

        if let Some(ret_val) = wake_ret {
            without_interrupts(|| {
                let mut sched = USER_SCHED.lock();
                let min_vr = sched.min_runnable_vruntime();
                if let Some(ref mut proc) = sched.slots[slot_idx] {
                    if proc.pid == pid && matches!(proc.state, UserTaskState::Waiting(_)) {
                        if let Some((fd, handle)) = update_tcp_fd {
                            if fd >= FD_OFFSET && fd < FD_OFFSET + MAX_PROC_FDS {
                                proc.fds[fd - FD_OFFSET] =
                                    Some(FdEntry::Socket(OpenSocket::Tcp { handle }));
                            }
                        }
                        proc.vruntime_ms = proc
                            .vruntime_ms
                            .max(min_vr.saturating_sub(DEFAULT_QUANTUM_MS * 2));
                        proc.ctx.rax =
                            freebsd::apply_abi_syscall_result(proc.abi, ret_val, &mut proc.ctx.rflags);
                        proc.state = UserTaskState::Ready;
                    }
                }
            });
        }
    }
}

/// Fair weighted virtual-runtime scheduler (CFS-style min-`vruntime_ms` with round-robin tie-break).
fn pick_next_ready_slot(sched: &mut UserScheduler) -> Option<usize> {
    let start = sched.rr_cursor % MAX_USER_PROCS;
    let mut best_idx: Option<usize> = None;
    let mut best_vr: u64 = u64::MAX;

    for offset in 0..MAX_USER_PROCS {
        let idx = (start + offset) % MAX_USER_PROCS;
        if let Some(ref proc) = sched.slots[idx] {
            if proc.state == UserTaskState::Ready && proc.vruntime_ms < best_vr {
                best_vr = proc.vruntime_ms;
                best_idx = Some(idx);
            }
        }
    }

    if let Some(idx) = best_idx {
        sched.rr_cursor = (idx + 1) % MAX_USER_PROCS;
    }
    best_idx
}

fn ensure_sigtramp_mapped_for_slot(
    slot_idx: usize,
    mapping: &PhysicalMemoryMapping,
    allocator: &mut PhysicalFrameAllocator<'_>,
) -> bool {
    let info = without_interrupts(|| {
        let mut sched = USER_SCHED.lock();
        let proc = sched.slots[slot_idx].as_mut()?;
        if proc.signals.sigtramp_mapped {
            return Some((true, proc.pml4, None));
        }
        let pml4 = proc.pml4;
        // Find thread-group leader or any sibling holding the LoadedElfImage
        let tgid = proc.tgid;
        let img = proc.image.take().or_else(|| {
            sched
                .slots
                .iter_mut()
                .flatten()
                .find(|s| s.tgid == tgid && s.image.is_some())
                .and_then(|s| s.image.take())
        });
        Some((false, pml4, img))
    });

    let Some((already, pml4, mut img_opt)) = info else {
        return false;
    };
    if already {
        return true;
    }

    let restore_img = |slot_idx: usize, img_opt: Option<LoadedElfImage>| {
        if let Some(img) = img_opt {
            without_interrupts(|| {
                let mut sched = USER_SCHED.lock();
                let tgid = sched.slots[slot_idx].as_ref().map(|p| p.tgid).unwrap_or(0);
                if let Some(ref mut proc) = sched.slots[slot_idx] {
                    if !proc.is_thread {
                        proc.image = Some(img);
                        return;
                    }
                }
                if let Some(owner) = sched
                    .slots
                    .iter_mut()
                    .flatten()
                    .find(|s| s.tgid == tgid && !s.is_thread)
                {
                    owner.image = Some(img);
                } else if let Some(ref mut proc) = sched.slots[slot_idx] {
                    proc.image = Some(img);
                }
            });
        }
    };

    let tramp_virt = VirtAddr::new(USER_SIGTRAMP_ADDR);
    if let Some(trans) = mapping.translate_virtual_address_in(pml4, tramp_virt) {
        let valid_rx = trans.flags.contains(PageTableFlags::USER_ACCESSIBLE)
            && !trans.flags.contains(PageTableFlags::NO_EXECUTE);
        if valid_rx {
            without_interrupts(|| {
                let mut sched = USER_SCHED.lock();
                if let Some(ref mut proc) = sched.slots[slot_idx] {
                    proc.signals.sigtramp_mapped = true;
                }
            });
            restore_img(slot_idx, img_opt.take());
            return true;
        }
        restore_img(slot_idx, img_opt.take());
        return false;
    }

    let Some(frame) = allocator.allocate_frame() else {
        restore_img(slot_idx, img_opt.take());
        return false;
    };
    let Some(hhdm) = mapping.translate_frame(frame) else {
        let _ = allocator.deallocate_frame(frame);
        restore_img(slot_idx, img_opt.take());
        return false;
    };
    unsafe {
        write_bytes(hhdm.as_u64() as *mut u8, 0, PAGE_SIZE as usize);
        core::ptr::copy_nonoverlapping(
            SIGTRAMP_CODE.as_ptr(),
            hhdm.as_u64() as *mut u8,
            SIGTRAMP_CODE.len(),
        );
    }

    let flags = PageTableFlags::PRESENT | PageTableFlags::USER_ACCESSIBLE;
    if mapping
        .map_page_in(allocator, pml4, tramp_virt, frame, flags)
        .is_err()
    {
        let _ = allocator.deallocate_frame(frame);
        restore_img(slot_idx, img_opt.take());
        return false;
    }

    without_interrupts(|| {
        let mut sched = USER_SCHED.lock();
        let tgid = sched.slots[slot_idx].as_ref().map(|p| p.tgid).unwrap_or(0);
        if let Some(mut img) = img_opt.take() {
            img.mapped_pages.push((USER_SIGTRAMP_ADDR, frame));
            let total_pages = img.mapped_pages.len();
            if let Some(owner) = sched
                .slots
                .iter_mut()
                .flatten()
                .find(|s| s.tgid == tgid && !s.is_thread)
            {
                owner.pages_mapped = total_pages;
                owner.image = Some(img);
            } else if let Some(ref mut proc) = sched.slots[slot_idx] {
                proc.pages_mapped = total_pages;
                proc.image = Some(img);
            }
        }
        if let Some(ref mut proc) = sched.slots[slot_idx] {
            proc.signals.sigtramp_mapped = true;
        }
    });
    true
}

fn try_deliver_pending_signal_slot(
    slot_idx: usize,
    mapping: &PhysicalMemoryMapping,
    allocator: &mut PhysicalFrameAllocator<'_>,
) -> bool {
    loop {
        let next = without_interrupts(|| {
            let mut sched = USER_SCHED.lock();
            let proc = sched.slots[slot_idx].as_mut()?;
            let sig = proc.signals.next_deliverable()?;
            proc.signals.clear_pending(sig);
            let cfg = proc.signals.actions[sig as usize];
            Some((proc.pml4, proc.ctx, proc.signals.blocked_mask, sig, cfg))
        });

        let Some((pml4, ctx, old_blocked, sig, cfg)) = next else {
            return true;
        };

        if sig == SIGKILL {
            without_interrupts(|| {
                let mut sched = USER_SCHED.lock();
                if let Some(ref mut proc) = sched.slots[slot_idx] {
                    proc.state = UserTaskState::Exited(-9);
                }
            });
            return false;
        }

        if cfg.handler == SIG_IGN {
            continue;
        }
        if cfg.handler == SIG_DFL {
            if signal::default_signal_action(sig) == SignalDefault::Terminate {
                without_interrupts(|| {
                    let mut sched = USER_SCHED.lock();
                    if let Some(ref mut proc) = sched.slots[slot_idx] {
                        proc.state = UserTaskState::Exited(-(sig as i64));
                    }
                });
                return false;
            }
            continue;
        }

        if !ensure_sigtramp_mapped_for_slot(slot_idx, mapping, allocator) {
            return true;
        }

        let frame_size = core::mem::size_of::<SavedSignalFrame>() as u64;
        // Align signal frame top below red zone, leaving ucontext right above return address so RSP+8 == uctx_ptr
        let frame_vaddr = ((ctx.rsp.saturating_sub(128)) & !0xF).saturating_sub(frame_size) & !0xF;
        let ret_addr_rsp = frame_vaddr.saturating_sub(8);

        let saved = SavedSignalFrame {
            r15: ctx.r15,
            r14: ctx.r14,
            r13: ctx.r13,
            r12: ctx.r12,
            rbp: ctx.rbp,
            rbx: ctx.rbx,
            r11: ctx.r11,
            r10: ctx.r10,
            r9: ctx.r9,
            r8: ctx.r8,
            rax: ctx.rax,
            rcx: ctx.rcx,
            rdx: ctx.rdx,
            rsi: ctx.rsi,
            rdi: ctx.rdi,
            rip: ctx.rip,
            rflags: ctx.rflags,
            rsp: ctx.rsp,
            old_blocked_mask: old_blocked,
            signo: sig,
            reserved: 0,
        };

        let frame_bytes = unsafe {
            core::slice::from_raw_parts(
                (&saved as *const SavedSignalFrame) as *const u8,
                frame_size as usize,
            )
        };
        if syscall::copy_to_user_bytes_in(
            mapping,
            Some(allocator),
            pml4,
            frame_vaddr,
            frame_bytes,
        )
        .is_err()
            || syscall::copy_to_user_bytes_in(
                mapping,
                Some(allocator),
                pml4,
                ret_addr_rsp,
                &USER_SIGTRAMP_ADDR.to_le_bytes(),
            )
            .is_err()
        {
            without_interrupts(|| {
                let mut sched = USER_SCHED.lock();
                if let Some(ref mut proc) = sched.slots[slot_idx] {
                    proc.state = UserTaskState::Exited(-(sig as i64));
                }
            });
            return false;
        }

        without_interrupts(|| {
            let mut sched = USER_SCHED.lock();
            if let Some(ref mut proc) = sched.slots[slot_idx] {
                proc.signals.blocked_mask |= cfg.mask | signal::sig_bit(sig);
                proc.ctx.rip = cfg.handler;
                proc.ctx.rsp = ret_addr_rsp;
                proc.ctx.rdi = sig as u64;
                proc.ctx.rsi = 0;
                proc.ctx.rdx = frame_vaddr;
                proc.ctx.rflags &= !1;
            }
        });
        return true;
    }
}

/// Runs up to one preemptive Ring 3 time slice (`DEFAULT_QUANTUM_MS`). Returns `true` if a task executed.
fn dispatch_one_quantum(
    console: &mut Console<'_>,
    mapping: &PhysicalMemoryMapping,
    allocator: &mut PhysicalFrameAllocator<'_>,
) -> bool {
    let picked_idx = without_interrupts(|| {
        let mut sched = USER_SCHED.lock();
        pick_next_ready_slot(&mut sched)
    });

    let Some(idx) = picked_idx else {
        return false;
    };

    if !try_deliver_pending_signal_slot(idx, mapping, allocator) {
        return false;
    }

    let picked = without_interrupts(|| {
        let mut sched = USER_SCHED.lock();
        let now_ms = timer::uptime_ms();
        sched.current_slot = Some(idx);
        sched.quantum_start_ms = now_ms;
        sched.quantum_deadline_ms = now_ms.saturating_add(DEFAULT_QUANTUM_MS);
        let proc = sched.slots[idx].as_mut()?;
        proc.state = UserTaskState::Running;
        LAST_DISPATCH_PID.store(proc.pid as u64, Ordering::Relaxed);
        Some((proc.pid, proc.tgid, proc.abi, proc.fs_base, proc.pml4, proc.ctx, proc.fpu_state))
    });

    let Some((pid, tgid, abi, fs_base, pml4, ctx, mut fpu_state)) = picked else {
        return false;
    };

    if pid == tgid {
        let mut pm = PROCESS_MANAGER.lock();
        pm.set_process_state(tgid, ProcessState::Running);
    }

    QUANTUM_ACTIVE.store(true, Ordering::Release);
    unsafe {
        syscall::run_user_quantum(
            console,
            mapping,
            allocator,
            pid,
            abi,
            fs_base,
            pml4,
            &ctx,
            &mut fpu_state,
        );
    }
    QUANTUM_ACTIVE.store(false, Ordering::Release);

    without_interrupts(|| {
        let mut sched = USER_SCHED.lock();
        if let Some(idx) = sched.current_slot.take() {
            if let Some(ref mut proc) = sched.slots[idx] {
                proc.fpu_state = fpu_state;
                if proc.state == UserTaskState::Running {
                    proc.state = UserTaskState::Ready;
                }
            }
        }
    });

    let new_pm_state = without_interrupts(|| {
        let sched = USER_SCHED.lock();
        sched
            .find_slot_by_pid(pid)
            .and_then(|idx| sched.slots[idx].as_ref())
            .filter(|p| p.pid == p.tgid)
            .map(|p| match p.state {
                UserTaskState::Waiting(_) => ProcessState::Blocked,
                _ => ProcessState::Ready,
            })
    });
    if let Some(st) = new_pm_state {
        let mut pm = PROCESS_MANAGER.lock();
        pm.set_process_state(tgid, st);
    }

    true
}

fn close_fd_list(fds: &mut [Option<FdEntry>]) {
    for slot in fds.iter_mut() {
        let Some(entry) = slot.take() else {
            continue;
        };
        match entry {
            FdEntry::Socket(sock) => match sock {
                OpenSocket::Tcp { handle } => crate::net::tcp::close_socket(handle),
                OpenSocket::Udp { handle } => crate::net::udp::free_socket(handle),
                OpenSocket::TcpUnconnected => {}
            },
            FdEntry::PipeRead { pipe_id } => super::ipc::close_pipe_end(pipe_id, false),
            FdEntry::PipeWrite { pipe_id } => super::ipc::close_pipe_end(pipe_id, true),
            FdEntry::Kqueue { kq_id } => kqueue::destroy_kqueue(kq_id),
            FdEntry::File { .. } | FdEntry::Dir { .. } => {}
        }
    }
}

fn cleanup_process_ipc_and_windows(
    pid: usize,
    pml4: u64,
    had_gui: bool,
    shm_attachments: Vec<ShmAttachment>,
    owned_shms: Vec<u64>,
    port_rights: Vec<PortRightEntry>,
    mapping: &PhysicalMemoryMapping,
    allocator: &mut PhysicalFrameAllocator<'_>,
) {
    for pr in port_rights {
        ipc::mach_release_port_id_right(pr.port_id, pr.right, pr.also_send, allocator);
    }
    for att in shm_attachments {
        for i in 0..att.page_count {
            if let Ok(virt) = VirtAddr::try_new(att.vaddr + (i as u64) * PAGE_SIZE) {
                let _ = mapping.unmap_page_in(pml4, virt);
            }
        }
        super::ipc::release_shm(att.shm_id, allocator);
    }
    for sid in owned_shms {
        super::ipc::release_shm(sid, allocator);
    }
    if had_gui && crate::graphics::wm::is_desktop_active() {
        let win_shms = crate::graphics::wm::close_external_windows_for_pid(pid);
        for sid in win_shms {
            super::ipc::release_shm(sid, allocator);
        }
    }
}

struct DeferredCleanup {
    tgid: usize,
    destroy_aspace: bool,
    pml4: u64,
    img_opt: Option<LoadedElfImage>,
    fds: [Option<FdEntry>; MAX_PROC_FDS],
    had_gui: bool,
    shm_atts: Vec<ShmAttachment>,
    owned_shms: Vec<u64>,
    port_rights: Vec<PortRightEntry>,
}

fn collect_finished_and_stdout(
    mapping: &PhysicalMemoryMapping,
    allocator: &mut PhysicalFrameAllocator<'_>,
) -> (Vec<(usize, String, String)>, Vec<FinishedUserProcess>) {
    let mut stdouts = Vec::new();
    let mut finished = Vec::new();
    let mut proc_exit_notes: Vec<(usize, i64)> = Vec::new();
    let mut deferred_cleanups: Vec<Box<DeferredCleanup>> = Vec::new();

    without_interrupts(|| {
        let mut sched = USER_SCHED.lock();
        let mut new_wait_entries: Vec<(usize, usize, i64)> = Vec::new();
        let mut dead_parent_pids: Vec<usize> = Vec::new();

        for idx in 0..MAX_USER_PROCS {
            let Some(ref mut proc) = sched.slots[idx] else {
                continue;
            };
            if !proc.stdout_buf.is_empty() {
                let text = core::mem::take(&mut proc.stdout_buf);
                stdouts.push((proc.pid, proc.name.clone(), text));
            }
            let exit_outcome = match proc.state {
                UserTaskState::Exited(code) => Some((true, code)),
                UserTaskState::Faulted(fault) => Some((false, fault.vector as i64)),
                _ => None,
            };
            let Some((is_clean_exit, raw_code)) = exit_outcome else {
                continue;
            };
            let mut proc_owned = sched.slots[idx].take().unwrap();
            let siblings_remain = sched.count_tgid(proc_owned.tgid) > 0;

            let mut img_to_free = proc_owned.image.take();
            let mut shms_to_free = core::mem::take(&mut proc_owned.shm_attachments);
            let mut owned_shms_to_free = core::mem::take(&mut proc_owned.owned_shm_ids);
            let mut ports_to_free = core::mem::take(&mut proc_owned.port_rights);
            let mut had_gui_to_free = proc_owned.has_gui_window;

            if siblings_remain {
                if let Some(survivor) = sched
                    .slots
                    .iter_mut()
                    .flatten()
                    .find(|s| s.tgid == proc_owned.tgid)
                {
                    if survivor.image.is_none() {
                        survivor.image = img_to_free.take();
                    }
                    if !shms_to_free.is_empty() && survivor.shm_attachments.is_empty() {
                        survivor.shm_attachments = core::mem::take(&mut shms_to_free);
                    }
                    if !owned_shms_to_free.is_empty() {
                        survivor.owned_shm_ids.extend(owned_shms_to_free.drain(..));
                    }
                    if !ports_to_free.is_empty() && survivor.port_rights.is_empty() {
                        survivor.port_rights = core::mem::take(&mut ports_to_free);
                    }
                    if had_gui_to_free {
                        survivor.has_gui_window = true;
                        had_gui_to_free = false;
                    }
                }
            } else {
                let report_code = if is_clean_exit { raw_code } else { -11 };
                if proc_owned.parent_pid != 0 {
                    new_wait_entries.push((proc_owned.parent_pid, proc_owned.tgid, report_code));
                }
                dead_parent_pids.push(proc_owned.tgid);
                proc_exit_notes.push((proc_owned.tgid, report_code));
            }

            let fds_owned = core::mem::replace(&mut proc_owned.fds, [const { None }; MAX_PROC_FDS]);
            deferred_cleanups.push(Box::new(DeferredCleanup {
                tgid: proc_owned.tgid,
                destroy_aspace: !siblings_remain,
                pml4: proc_owned.pml4,
                img_opt: img_to_free,
                fds: fds_owned,
                had_gui: had_gui_to_free,
                shm_atts: shms_to_free,
                owned_shms: owned_shms_to_free,
                port_rights: ports_to_free,
            }));

            if !siblings_remain {
                let outcome = match proc_owned.state {
                    UserTaskState::Exited(exit_code) => UserTrapOutcome::Exited {
                        exit_code,
                        syscalls_served: proc_owned.syscalls_served,
                    },
                    UserTaskState::Faulted(fault) => UserTrapOutcome::Faulted {
                        fault,
                        syscalls_served: proc_owned.syscalls_served,
                    },
                    _ => unreachable!(),
                };
                finished.push(FinishedUserProcess {
                    pid: proc_owned.tgid,
                    name: proc_owned.name,
                    entry_point: proc_owned.entry_point,
                    pages_mapped: proc_owned.pages_mapped,
                    bytes_loaded: proc_owned.bytes_loaded,
                    outcome,
                });
            }
        }

        if !dead_parent_pids.is_empty() {
            sched
                .exited_for_wait
                .retain(|&(ppid, _, _)| !dead_parent_pids.contains(&ppid));
        }
        for (ppid, cpid, code) in new_wait_entries {
            if let Some(p_idx) = sched
                .slots
                .iter()
                .position(|s| s.as_ref().map(|p| p.tgid) == Some(ppid))
            {
                sched.exited_for_wait.push((ppid, cpid, code));
                if let Some(ref mut parent_slot) = sched.slots[p_idx] {
                    parent_slot.signals.raise(SIGCHLD);
                }
            }
        }
    });

    for (pid, code) in proc_exit_notes {
        kqueue::notify_process_exit(pid, code);
    }

    for mut item in deferred_cleanups {
        close_fd_list(&mut item.fds);
        if item.destroy_aspace {
            cleanup_process_ipc_and_windows(
                item.tgid,
                item.pml4,
                item.had_gui,
                core::mem::take(&mut item.shm_atts),
                core::mem::take(&mut item.owned_shms),
                core::mem::take(&mut item.port_rights),
                mapping,
                allocator,
            );
            if let Some(mut img) = item.img_opt.take() {
                elf::cleanup_loaded_image(&mut img, item.pml4, mapping, allocator);
            }
            let mut pm = PROCESS_MANAGER.lock();
            unsafe {
                let _ = pm.terminate_process(item.tgid, mapping, allocator);
            }
            let _ = pm.reap_process(item.tgid);
        }
    }

    (stdouts, finished)
}

/// Steps the preemptive Ring 3 scheduler for Desktop / Kernel idle loop:
/// polls wait queues, steps kernel workers, drains `SYS_SPAWN_ELF`, runs one Ring 3 quantum,
/// streams stdout, and reaps finished tasks.
pub fn step_user_tasks(
    console: &mut Console<'_>,
    mapping: Option<&PhysicalMemoryMapping>,
    allocator: &mut PhysicalFrameAllocator<'_>,
) -> bool {
    let Some(mapping) = mapping else {
        return false;
    };

    // 1. Progress background kernel threads (network I/O) and resolve Ring 3 wait queues
    let _ = kthread::step_workers(2, 2);
    poll_wait_queues(mapping, allocator);

    // 2. Drain any pending SYS_SPAWN_ELF requests queued by userland processes
    let spawns: Vec<Vec<String>> = without_interrupts(|| {
        let mut sched = USER_SCHED.lock();
        core::mem::take(&mut sched.pending_spawns)
    });
    for argv in spawns {
        if let Some(path) = argv.first() {
            let arg_refs: Vec<&str> = argv.iter().map(String::as_str).collect();
            if let Err(err) =
                super::runner::spawn_elf_async_with_args(path, &arg_refs, Some(mapping), allocator)
            {
                if crate::graphics::wm::is_desktop_active() {
                    crate::graphics::wm::append_user_stdout(&format!(
                        "[spawn {} failed: {}]\n",
                        path,
                        err.message()
                    ));
                }
            }
        }
    }

    // 3. Dispatch one ready process quantum (<= 8 ms preemptive slice)
    let did_run = dispatch_one_quantum(console, mapping, allocator);

    // 4. Flush stdout + clean up any exited or faulted processes cleanly
    let (stdouts, finished) = collect_finished_and_stdout(mapping, allocator);

    let desktop_active = crate::graphics::wm::is_desktop_active();
    for (_pid, _name, text) in stdouts {
        if desktop_active {
            crate::graphics::wm::append_user_stdout(&text);
        } else {
            console.write(&text, WHITE);
        }
    }

    let mut sync_left = Vec::new();
    for rep in finished {
        match rep.outcome {
            UserTrapOutcome::Exited {
                exit_code,
                syscalls_served,
            } => {
                if desktop_active {
                    crate::graphics::wm::append_user_stdout(&format!(
                        "[PID {} '{}' exited code={} syscalls={}]\n",
                        rep.pid, rep.name, exit_code, syscalls_served
                    ));
                }
            }
            UserTrapOutcome::Faulted {
                fault,
                syscalls_served,
            } => {
                let pmm_ok = allocator.check_integrity();
                crate::graphics::alerts::report_user_fault(
                    rep.pid,
                    &rep.name,
                    fault,
                    syscalls_served,
                    pmm_ok,
                    desktop_active,
                );
                if desktop_active {
                    crate::graphics::wm::append_user_stdout(&format!(
                        "[PID {} '{}' KILLED vec={} rip=0x{:x} cr2=0x{:x}]\n",
                        rep.pid, rep.name, fault.vector, fault.rip, fault.cr2
                    ));
                }
            }
        }
        sync_left.push(rep);
    }

    if !sync_left.is_empty() && !desktop_active {
        without_interrupts(|| {
            let mut sched = USER_SCHED.lock();
            sched.completed_for_sync.extend(sync_left);
        });
    }

    did_run
}

/// Synchronous wait helper for CLI console `exec <path.elf>` outside Desktop WM.
pub fn wait_for_pid_sync(
    console: &mut Console<'_>,
    pid: usize,
    mapping: &PhysicalMemoryMapping,
    allocator: &mut PhysicalFrameAllocator<'_>,
) -> FinishedUserProcess {
    loop {
        let _ = step_user_tasks(console, Some(mapping), allocator);

        let found = without_interrupts(|| {
            let mut sched = USER_SCHED.lock();
            if let Some(pos) = sched.completed_for_sync.iter().position(|r| r.pid == pid) {
                Some(sched.completed_for_sync.remove(pos))
            } else {
                None
            }
        });
        if let Some(rep) = found {
            return rep;
        }

        if !has_ready_now() && !kthread::has_pending_net_jobs() {
            unsafe {
                asm!("sti", "hlt", options(nomem, nostack));
            }
        }
    }
}

pub fn kill_user_process(
    pid: usize,
    mapping: Option<&PhysicalMemoryMapping>,
    allocator: &mut PhysicalFrameAllocator<'_>,
) -> Result<(), &'static str> {
    let mapping = mapping.ok_or("Physical memory mapping is unavailable")?;
    let removed = without_interrupts(|| {
        let mut sched = USER_SCHED.lock();
        let idx = sched
            .find_slot_by_pid(pid)
            .or_else(|| sched.slots.iter().position(|s| s.as_ref().map(|p| p.tgid) == Some(pid)))?;
        let proc = sched.slots[idx].take()?;
        let tgid = proc.tgid;
        let mut extras = Vec::new();
        for s_idx in 0..MAX_USER_PROCS {
            if sched.slots[s_idx].as_ref().map(|p| p.tgid) == Some(tgid) {
                if let Some(extra) = sched.slots[s_idx].take() {
                    extras.push(extra);
                }
            }
        }
        sched.exited_for_wait.retain(|&(ppid, _, _)| ppid != tgid);
        if proc.parent_pid != 0 && sched.find_slot_by_pid(proc.parent_pid).is_some() {
            sched.exited_for_wait.push((proc.parent_pid, tgid, -9));
        }
        Some((proc, extras))
    });

    let Some((mut proc, extras)) = removed else {
        return Err("Ring 3 PID not found in active scheduler");
    };

    let tgid = proc.tgid;
    kqueue::notify_process_exit(tgid, -9);

    for mut extra in extras {
        close_fd_list(&mut extra.fds);
        if proc.image.is_none() {
            proc.image = extra.image;
        }
    }
    close_fd_list(&mut proc.fds);
    cleanup_process_ipc_and_windows(
        tgid,
        proc.pml4,
        proc.has_gui_window,
        proc.shm_attachments,
        proc.owned_shm_ids,
        proc.port_rights,
        mapping,
        allocator,
    );
    if let Some(mut img) = proc.image.take() {
        elf::cleanup_loaded_image(&mut img, proc.pml4, mapping, allocator);
    }
    let mut pm = PROCESS_MANAGER.lock();
    unsafe {
        let _ = pm.terminate_process(tgid, mapping, allocator);
    }
    let _ = pm.reap_process(tgid);

    if crate::graphics::wm::is_desktop_active() {
        crate::graphics::wm::append_user_stdout(&format!(
            "[PID {} '{}' killed by user]\n",
            tgid, proc.name
        ));
    }
    Ok(())
}

pub fn snapshot_processes() -> Vec<UserProcSnapshot> {
    let now_ms = timer::uptime_ms();
    without_interrupts(|| {
        let sched = USER_SCHED.lock();
        let mut out = Vec::new();
        for proc in sched.slots.iter().flatten() {
            let state_label = match proc.state {
                UserTaskState::Running => "Running",
                UserTaskState::Ready => "Ready",
                UserTaskState::Waiting(reason) => match reason {
                    WaitReason::Sleep { wake_ms } => {
                        if now_ms >= wake_ms {
                            "Waking"
                        } else {
                            "WaitSleep"
                        }
                    }
                    WaitReason::GuiEvent { .. } => "WaitGUI",
                    WaitReason::TcpConnect { .. } => "WaitTCP",
                    WaitReason::NetJob { .. } => "WaitNet",
                    WaitReason::PipeRead { .. } => "WaitPipe",
                    WaitReason::WaitPid { .. } => "WaitPid",
                    WaitReason::MachPortRecv { .. } => "WaitMach",
                    WaitReason::Kqueue { .. } => "WaitKev",
                    WaitReason::UmtxWait { .. } => "WaitUmtx",
                },
                UserTaskState::Exited(_) => "Exited",
                UserTaskState::Faulted(_) => "Faulted",
            };
            out.push(UserProcSnapshot {
                pid: proc.pid,
                name: proc.name.clone(),
                state_label,
                cpu_ms: proc.cpu_ms,
                vruntime_ms: proc.vruntime_ms,
                syscalls: proc.syscalls_served,
                pages: proc.pages_mapped,
                preempts: proc.preempt_count,
                lazy_faults: proc.lazy_faults,
                vmas: proc.vmas.len(),
            });
        }
        out
    })
}

pub fn print_cli_process_table(console: &mut Console<'_>) {
    let list = snapshot_processes();
    if list.is_empty() {
        return;
    }
    console.write("\nActive Ring 3 Scheduled Tasks:\n", GREEN);
    console.write(
        "PID   STATE      CPU(ms)  VRUNTM   PREEMPT  LAZY#PF  PAGES  NAME\n",
        GRAY,
    );
    for p in list {
        let line = format!(
            "{:<5} {:<10} {:<8} {:<8} {:<8} {:<8} {:<6} {}\n",
            p.pid, p.state_label, p.cpu_ms, p.vruntime_ms, p.preempts, p.lazy_faults, p.pages, p.name
        );
        console.write(&line, WHITE);
    }
}

pub fn dump_console_fault_tag(console: &mut Console<'_>, ok: bool) {
    console.write("Scheduler PMM: ", GRAY);
    console.write(if ok { "OK\n" } else { "LEAK\n" }, if ok { GREEN } else { RED });
}

struct ForkSnapshot {
    parent_pid: usize,
    parent_abi: ProcessAbi,
    parent_name: String,
    parent_pml4: u64,
    parent_fpu: FxSaveArea,
    parent_fs_base: u64,
    parent_gs_base: u64,
    child_signals: SignalState,
    parent_weight: u32,
    entry_point: u64,
    bytes_loaded: u64,
    brk_base: u64,
    brk_current: u64,
    mmap_next: u64,
    shm_attach_next: u64,
    shm_attachments: Vec<ShmAttachment>,
    dynamic_bytes: usize,
    vmas: Vec<VmRegion>,
    child_fds: [Option<FdEntry>; MAX_PROC_FDS],
    parent_port_rights: Vec<PortRightEntry>,
    next_port_name: u32,
    user_rsp: u64,
    initial_argc: u64,
    initial_argv: u64,
    auxv_ptr: u64,
    phdr_vaddr: u64,
    ph_ent: u16,
    ph_num: u16,
    osreldate: u32,
    load_regions: Vec<(u64, u64, bool, bool)>,
    load_segment_count: usize,
}

pub fn fork_current_process(
    parent_ctx: UserCpuContext,
    mapping: &PhysicalMemoryMapping,
    allocator: &mut PhysicalFrameAllocator<'_>,
) -> i64 {
    if active_count() >= MAX_USER_PROCS {
        return -11; // EAGAIN
    }

    let snapshot = without_interrupts(|| {
        let sched = USER_SCHED.lock();
        let idx = sched.current_slot?;
        let p = sched.slots[idx].as_ref()?;
        let tgid = p.tgid;
        let img = p.image.as_ref().or_else(|| {
            sched
                .slots
                .iter()
                .flatten()
                .find(|s| s.tgid == tgid && s.image.is_some())
                .and_then(|s| s.image.as_ref())
        })?;
        Some(Box::new(ForkSnapshot {
            parent_pid: tgid,
            parent_abi: p.abi,
            parent_name: p.name.clone(),
            parent_pml4: p.pml4,
            parent_fpu: p.fpu_state,
            parent_fs_base: p.fs_base,
            parent_gs_base: p.gs_base,
            child_signals: p.signals.clone_for_fork(),
            parent_weight: p.weight,
            entry_point: p.entry_point,
            bytes_loaded: p.bytes_loaded,
            brk_base: p.brk_base,
            brk_current: p.brk_current,
            mmap_next: p.mmap_next,
            shm_attach_next: p.shm_attach_next,
            shm_attachments: p.shm_attachments.clone(),
            dynamic_bytes: p.dynamic_bytes,
            vmas: p.vmas.clone(),
            child_fds: p.fds.clone(),
            parent_port_rights: p.port_rights.clone(),
            next_port_name: p.next_port_name,
            user_rsp: img.user_rsp,
            initial_argc: img.initial_argc,
            initial_argv: img.initial_argv,
            auxv_ptr: img.auxv_ptr,
            phdr_vaddr: img.phdr_vaddr,
            ph_ent: img.ph_ent,
            ph_num: img.ph_num,
            osreldate: img.osreldate,
            load_regions: img.load_regions.clone(),
            load_segment_count: img.load_segment_count,
        }))
    });

    let Some(mut snap) = snapshot else {
        return -22;
    };

    let parent_pid = snap.parent_pid;
    let parent_abi = snap.parent_abi;
    let parent_pml4 = snap.parent_pml4;

    let child_pid = {
        let mut pm = PROCESS_MANAGER.lock();
        match pm.create_process(mapping, allocator) {
            Ok(pid) => pid,
            Err(_) => return -12,
        }
    };

    let child_pml4 = {
        let mut pm = PROCESS_MANAGER.lock();
        let Some(p) = pm.get_process(child_pid) else {
            return -12;
        };
        let pml4 = p.address_space().pml4_physical_address();
        pm.set_process_state(child_pid, ProcessState::Ready);
        pml4
    };

    let shm_ranges: Vec<(u64, u64)> = snap
        .shm_attachments
        .iter()
        .map(|a| (a.vaddr, a.vaddr + (a.page_count as u64) * PAGE_SIZE))
        .collect();

    let mut cloned_pages =
        match mapping.clone_user_cow_tables(parent_pml4, child_pml4, allocator, &shm_ranges) {
            Ok(pages) => pages,
            Err(_) => {
                let mut pm = PROCESS_MANAGER.lock();
                unsafe {
                    let _ = pm.terminate_process(child_pid, mapping, allocator);
                }
                let _ = pm.reap_process(child_pid);
                return -12;
            }
        };

    // Re-attach shared memory regions directly without CoW duplication
    let mut attached_shms: Vec<ShmAttachment> = Vec::with_capacity(snap.shm_attachments.len());
    let mut shm_pages_mapped = 0usize;
    let mut shm_failed = false;

    for att in &snap.shm_attachments {
        let Some(frames) = ipc::retain_shm_frames(att.shm_id) else {
            shm_failed = true;
            break;
        };
        let writable = snap
            .vmas
            .iter()
            .find(|r| r.kind == VmRegionKind::SharedMem && r.start == att.vaddr)
            .map(|r| r.writable)
            .unwrap_or(true);
        let mut flags = PageTableFlags::PRESENT | PageTableFlags::USER_ACCESSIBLE;
        if writable {
            flags |= PageTableFlags::WRITABLE;
        }
        if syscall::nxe_enabled() {
            flags |= PageTableFlags::NO_EXECUTE;
        }

        let mut mapped_here = 0usize;
        for (i, &frame) in frames.iter().enumerate() {
            let va = att.vaddr + (i as u64) * PAGE_SIZE;
            let Ok(virt) = VirtAddr::try_new(va) else {
                shm_failed = true;
                break;
            };
            if mapping.map_page_in(allocator, child_pml4, virt, frame, flags).is_err() {
                shm_failed = true;
                break;
            }
            mapped_here += 1;
        }
        if shm_failed {
            for i in 0..mapped_here {
                if let Ok(virt) = VirtAddr::try_new(att.vaddr + (i as u64) * PAGE_SIZE) {
                    let _ = mapping.unmap_page_in(child_pml4, virt);
                }
            }
            ipc::release_shm(att.shm_id, allocator);
            break;
        }
        shm_pages_mapped += att.page_count;
        attached_shms.push(*att);
    }

    if shm_failed {
        for att in attached_shms {
            for i in 0..att.page_count {
                if let Ok(virt) = VirtAddr::try_new(att.vaddr + (i as u64) * PAGE_SIZE) {
                    let _ = mapping.unmap_page_in(child_pml4, virt);
                }
            }
            ipc::release_shm(att.shm_id, allocator);
        }
        for (va, fr) in cloned_pages.drain(..) {
            if let Ok(virt) = VirtAddr::try_new(va) {
                let _ = mapping.unmap_page_in(child_pml4, virt);
            }
            let _ = allocator.deallocate_frame(fr);
        }
        let mut pm = PROCESS_MANAGER.lock();
        unsafe {
            let _ = pm.terminate_process(child_pid, mapping, allocator);
        }
        let _ = pm.reap_process(child_pid);
        return -12;
    }

    // Inherit file descriptors: pipes retain refcount, sockets & kqueues are per-process and cleared
    for slot in &mut snap.child_fds {
        match slot {
            Some(FdEntry::PipeRead { pipe_id }) => ipc::retain_pipe_end(*pipe_id, false),
            Some(FdEntry::PipeWrite { pipe_id }) => ipc::retain_pipe_end(*pipe_id, true),
            Some(FdEntry::Socket(_)) | Some(FdEntry::Kqueue { .. }) => *slot = None,
            _ => {}
        }
    }

    // Inherit Mach IPC rights: Receive stays parent-only; Send (and also_send from allocate) is inherited
    let mut child_port_rights = Vec::new();
    for entry in &snap.parent_port_rights {
        match entry.right {
            PortRightKind::Receive => {
                if entry.also_send && ipc::mach_retain_right(entry.port_id, PortRightKind::Send, false)
                {
                    child_port_rights.push(PortRightEntry {
                        name: entry.name,
                        port_id: entry.port_id,
                        right: PortRightKind::Send,
                        also_send: false,
                    });
                }
            }
            PortRightKind::Send => {
                if ipc::mach_retain_right(entry.port_id, PortRightKind::Send, false) {
                    child_port_rights.push(PortRightEntry {
                        name: entry.name,
                        port_id: entry.port_id,
                        right: PortRightKind::Send,
                        also_send: false,
                    });
                }
            }
            PortRightKind::SendOnce => {}
        }
    }

    let mut child_ctx = parent_ctx;
    child_ctx.rax = 0;
    child_ctx.rdx = 1; // FreeBSD ABI fork convention: child has rdx=1, parent rdx=0
    child_ctx.rflags &= !1;

    let pages_mapped = cloned_pages.len() + shm_pages_mapped;
    let child_image = LoadedElfImage {
        entry_point: snap.entry_point,
        user_rsp: snap.user_rsp,
        brk_base: snap.brk_base,
        initial_argc: snap.initial_argc,
        initial_argv: snap.initial_argv,
        abi: parent_abi,
        auxv_ptr: snap.auxv_ptr,
        phdr_vaddr: snap.phdr_vaddr,
        ph_ent: snap.ph_ent,
        ph_num: snap.ph_num,
        osreldate: snap.osreldate,
        mapped_pages: cloned_pages,
        load_regions: snap.load_regions,
        load_segment_count: snap.load_segment_count,
        total_bytes_loaded: snap.bytes_loaded,
    };

    let mut child_slot = Box::new(UserProcessSlot {
        pid: child_pid,
        tgid: child_pid,
        is_thread: false,
        parent_pid,
        abi: parent_abi,
        name: format!("{}[fork]", snap.parent_name),
        pml4: child_pml4,
        ctx: child_ctx,
        fpu_state: snap.parent_fpu,
        fs_base: snap.parent_fs_base,
        gs_base: snap.parent_gs_base,
        clear_child_tid: 0,
        signals: snap.child_signals,
        state: UserTaskState::Ready,
        cpu_ms: 0,
        vruntime_ms: 0,
        weight: snap.parent_weight,
        preempt_count: 0,
        lazy_faults: 0,
        syscalls_served: 0,
        entry_point: snap.entry_point,
        pages_mapped,
        bytes_loaded: snap.bytes_loaded,
        brk_base: snap.brk_base,
        brk_current: snap.brk_current,
        mmap_next: snap.mmap_next,
        shm_attach_next: snap.shm_attach_next,
        shm_attachments: attached_shms,
        owned_shm_ids: Vec::new(),
        dynamic_bytes: snap.dynamic_bytes,
        vmas: snap.vmas,
        image: Some(child_image),
        fds: snap.child_fds,
        port_rights: child_port_rights,
        next_port_name: snap.next_port_name,
        stdout_buf: String::new(),
        has_gui_window: false,
    });

    let inserted = without_interrupts(|| {
        let mut sched = USER_SCHED.lock();
        let start_vr = sched.min_runnable_vruntime();
        let Some(empty_idx) = sched.slots.iter().position(|s| s.is_none()) else {
            return Err(child_slot);
        };
        child_slot.vruntime_ms = start_vr;
        sched.slots[empty_idx] = Some(child_slot);
        Ok(())
    });

    if let Err(mut returned_slot) = inserted {
        for pr in returned_slot.port_rights.drain(..) {
            ipc::mach_release_port_id_right(pr.port_id, pr.right, pr.also_send, allocator);
        }
        close_fd_list(&mut returned_slot.fds);
        for att in returned_slot.shm_attachments.drain(..) {
            for i in 0..att.page_count {
                if let Ok(virt) = VirtAddr::try_new(att.vaddr + (i as u64) * PAGE_SIZE) {
                    let _ = mapping.unmap_page_in(child_pml4, virt);
                }
            }
            ipc::release_shm(att.shm_id, allocator);
        }
        if let Some(mut returned_img) = returned_slot.image.take() {
            elf::cleanup_loaded_image(&mut returned_img, child_pml4, mapping, allocator);
        }
        let mut pm = PROCESS_MANAGER.lock();
        unsafe {
            let _ = pm.terminate_process(child_pid, mapping, allocator);
        }
        let _ = pm.reap_process(child_pid);
        return -11;
    }

    klog::log_fmt(
        KLogLevel::Mm,
        "fork",
        format_args!(
            "PID {} ({}) forked -> child PID {} ({} CoW pages)",
            parent_pid,
            parent_abi.name(),
            child_pid,
            pages_mapped
        ),
    );
    child_pid as i64
}

pub fn sys_waitpid_current(
    target_pid: isize,
    status_ptr: u64,
    options: u64,
    mapping: &PhysicalMemoryMapping,
    allocator: &mut PhysicalFrameAllocator<'_>,
) -> Result<i64, WaitReason> {
    enum PollResult {
        Reaped {
            pml4: u64,
            cpid: usize,
            code: i64,
        },
        Block,
        Err(i64),
    }

    let check = without_interrupts(|| {
        let mut sched = USER_SCHED.lock();
        let Some(idx) = sched.current_slot else {
            return PollResult::Err(-22);
        };
        let Some(ref proc) = sched.slots[idx] else {
            return PollResult::Err(-22);
        };
        let my_pid = proc.pid;
        let my_pml4 = proc.pml4;

        if let Some(pos) = sched.exited_for_wait.iter().position(|&(ppid, cpid, _)| {
            ppid == my_pid && (target_pid <= 0 || cpid == target_pid as usize)
        }) {
            let (_ppid, cpid, code) = sched.exited_for_wait.swap_remove(pos);
            return PollResult::Reaped {
                pml4: my_pml4,
                cpid,
                code,
            };
        }

        if !sched.has_live_child_matching(my_pid, target_pid) {
            return PollResult::Err(-10); // ECHILD
        }
        if (options & 1) != 0 {
            return PollResult::Err(0); // WNOHANG
        }
        PollResult::Block
    });

    match check {
        PollResult::Reaped { pml4, cpid, code } => {
            if status_ptr != 0 {
                let bytes = (code as i32).to_le_bytes();
                if syscall::copy_to_user_bytes_in(mapping, Some(allocator), pml4, status_ptr, &bytes)
                    .is_err()
                {
                    return Ok(-14);
                }
            }
            Ok(cpid as i64)
        }
        PollResult::Err(e) => Ok(e),
        PollResult::Block => Err(WaitReason::WaitPid {
            target_pid,
            status_ptr,
        }),
    }
}

pub fn sys_mach_port_allocate_current() -> i64 {
    let has_slot = without_interrupts(|| {
        let sched = USER_SCHED.lock();
        sched
            .current_slot
            .and_then(|idx| sched.slots[idx].as_ref())
            .is_some()
    });
    if !has_slot {
        return -22;
    }
    let Ok(port_id) = ipc::mach_port_allocate() else {
        return -12;
    };
    let res = without_interrupts(|| {
        let mut sched = USER_SCHED.lock();
        let Some(idx) = sched.current_slot else {
            return None;
        };
        let Some(ref mut proc) = sched.slots[idx] else {
            return None;
        };
        Some(proc.alloc_port_right(port_id, PortRightKind::Receive, true))
    });
    match res {
        Some(name) => name as i64,
        None => -22,
    }
}

pub fn sys_mach_port_deallocate_current(
    port_name: MachPortName,
    allocator: &mut PhysicalFrameAllocator<'_>,
) -> i64 {
    if port_name == MACH_PORT_NULL {
        return -22;
    }
    let taken = without_interrupts(|| {
        let mut sched = USER_SCHED.lock();
        let idx = sched.current_slot?;
        let proc = sched.slots[idx].as_mut()?;
        proc.take_port_right(port_name)
    });
    let Some(entry) = taken else {
        return -22; // EINVAL
    };
    ipc::mach_release_port_id_right(entry.port_id, entry.right, entry.also_send, allocator);
    0
}

pub fn sys_bootstrap_register_current(
    name: &str,
    port_name: MachPortName,
    allocator: &mut PhysicalFrameAllocator<'_>,
) -> i64 {
    let port_id = without_interrupts(|| {
        let sched = USER_SCHED.lock();
        let idx = sched.current_slot?;
        let proc = sched.slots[idx].as_ref()?;
        let entry = proc.find_port_right(port_name)?;
        match entry.right {
            PortRightKind::Send => Some(entry.port_id),
            PortRightKind::Receive if entry.also_send => Some(entry.port_id),
            _ => None,
        }
    });
    let Some(pid_port) = port_id else {
        return -22;
    };
    match ipc::bootstrap_register_port(name, pid_port, allocator) {
        Ok(()) => 0,
        Err(e) => e,
    }
}

pub fn sys_bootstrap_lookup_current(
    name: &str,
    allocator: &mut PhysicalFrameAllocator<'_>,
) -> i64 {
    let port_id = match ipc::bootstrap_lookup_port(name) {
        Ok(id) => id,
        Err(e) => return e,
    };
    let installed = without_interrupts(|| {
        let mut sched = USER_SCHED.lock();
        let idx = sched.current_slot?;
        let proc = sched.slots[idx].as_mut()?;
        Some(proc.alloc_port_right(port_id, PortRightKind::Send, false))
    });
    match installed {
        Some(name_handle) => name_handle as i64,
        None => {
            ipc::mach_release_port_id_right(port_id, PortRightKind::Send, false, allocator);
            -22
        }
    }
}

fn deliver_mach_msg_to_slot(
    slot_idx: usize,
    expected_pid: usize,
    pml4: u64,
    port_id: u64,
    rcv_name: MachPortName,
    mut msg: MachKernelMsg,
    msg_ptr: u64,
    rcv_cap: usize,
    mapping: &PhysicalMemoryMapping,
    allocator: &mut PhysicalFrameAllocator<'_>,
) -> i64 {
    let hdr_size = core::mem::size_of::<MachMsgHeader>();
    let ool_size = core::mem::size_of::<MachMsgOolDesc>();
    let has_ool = msg.ool.is_some();
    let total_needed = hdr_size + if has_ool { ool_size } else { 0 } + msg.inline_data.len();

    if rcv_cap < total_needed {
        if let Err(returned) = ipc::mach_requeue_front_msg(port_id, msg) {
            ipc::mach_drop_msg_resources(returned, allocator);
        }
        return -12;
    }

    if syscall::validate_user_slice_in(mapping, Some(allocator), pml4, msg_ptr, total_needed, true)
        .is_err()
    {
        ipc::mach_drop_msg_resources(msg, allocator);
        return -14;
    }

    let mut out_ool_desc = MachMsgOolDesc::default();
    if let Some(ool) = msg.ool.take() {
        let page_count = ool.frames.len();
        let aligned_bytes = page_count * (PAGE_SIZE as usize);
        let reservation = without_interrupts(|| {
            let mut sched = USER_SCHED.lock();
            let proc = sched.slots[slot_idx].as_mut()?;
            if proc.pid != expected_pid {
                return None;
            }
            if proc.dynamic_bytes + aligned_bytes > MAX_DYNAMIC_USER_BYTES {
                return None;
            }
            let base = proc.mmap_next;
            let next = base.checked_add(aligned_bytes as u64)?;
            if next >= USER_MMAP_LIMIT || next >= USER_ADDR_LIMIT {
                return None;
            }
            proc.mmap_next = next;
            Some(base)
        });

        let Some(map_base) = reservation else {
            for fr in ool.frames {
                let _ = allocator.deallocate_frame(fr);
            }
            if msg.reply_port_id != 0 {
                if let Some(rr) = msg.reply_right {
                    ipc::mach_release_port_id_right(msg.reply_port_id, rr, false, allocator);
                }
            }
            return -12;
        };

        let mut base_flags = PageTableFlags::PRESENT | PageTableFlags::USER_ACCESSIBLE;
        if syscall::nxe_enabled() {
            base_flags |= PageTableFlags::NO_EXECUTE;
        }

        let mut mapped_ok = 0usize;
        let mut map_err = false;
        for (i, &frame) in ool.frames.iter().enumerate() {
            let va = map_base + (i as u64) * PAGE_SIZE;
            let Ok(virt) = VirtAddr::try_new(va) else {
                map_err = true;
                break;
            };
            let flags = if allocator.frame_refcount(frame) > 1 {
                base_flags | COW_FLAG
            } else {
                base_flags | PageTableFlags::WRITABLE
            };
            if mapping.map_page_in(allocator, pml4, virt, frame, flags).is_err() {
                map_err = true;
                break;
            }
            mapped_ok += 1;
        }

        if map_err {
            for i in 0..mapped_ok {
                if let Ok(virt) = VirtAddr::try_new(map_base + (i as u64) * PAGE_SIZE) {
                    let _ = mapping.unmap_page_in(pml4, virt);
                }
            }
            for fr in ool.frames {
                let _ = allocator.deallocate_frame(fr);
            }
            if msg.reply_port_id != 0 {
                if let Some(rr) = msg.reply_right {
                    ipc::mach_release_port_id_right(msg.reply_port_id, rr, false, allocator);
                }
            }
            return -12;
        }

        let frames_len = ool.frames.len();
        without_interrupts(|| {
            let mut sched = USER_SCHED.lock();
            if let Some(ref mut proc) = sched.slots[slot_idx] {
                if proc.pid == expected_pid {
                    if let Some(ref mut img) = proc.image {
                        for (i, frame) in ool.frames.into_iter().enumerate() {
                            img.mapped_pages.push((map_base + (i as u64) * PAGE_SIZE, frame));
                        }
                        proc.pages_mapped = img.mapped_pages.len();
                    }
                    proc.dynamic_bytes = proc.dynamic_bytes.saturating_add(aligned_bytes);
                    proc.vmas.push(VmRegion {
                        start: map_base,
                        end: map_base + (frames_len as u64) * PAGE_SIZE,
                        writable: true,
                        executable: false,
                        kind: VmRegionKind::LazyMmap,
                    });
                }
            }
        });

        out_ool_desc = MachMsgOolDesc {
            addr: map_base,
            size: ool.size_bytes,
            deallocate: false,
            reserved: [0; 7],
        };
    }

    let local_reply_name = if msg.reply_port_id != 0 {
        if let Some(rr) = msg.reply_right {
            let name_opt = without_interrupts(|| {
                let mut sched = USER_SCHED.lock();
                let proc = sched.slots[slot_idx].as_mut()?;
                if proc.pid != expected_pid {
                    return None;
                }
                Some(proc.alloc_port_right(msg.reply_port_id, rr, false))
            });
            match name_opt {
                Some(n) => n,
                None => {
                    ipc::mach_release_port_id_right(msg.reply_port_id, rr, false, allocator);
                    MACH_PORT_NULL
                }
            }
        } else {
            MACH_PORT_NULL
        }
    } else {
        MACH_PORT_NULL
    };

    let out_hdr = MachMsgHeader {
        bits: msg.bits,
        size: total_needed as u32,
        remote_port: rcv_name,
        local_port: local_reply_name,
        id: msg.id,
        reserved: 0,
    };

    let mut wire = alloc::vec![0u8; total_needed];
    unsafe {
        let hdr_bytes = core::slice::from_raw_parts(
            (&out_hdr as *const MachMsgHeader) as *const u8,
            hdr_size,
        );
        wire[..hdr_size].copy_from_slice(hdr_bytes);
        let mut cursor = hdr_size;
        if has_ool {
            let ool_bytes = core::slice::from_raw_parts(
                (&out_ool_desc as *const MachMsgOolDesc) as *const u8,
                ool_size,
            );
            wire[cursor..cursor + ool_size].copy_from_slice(ool_bytes);
            cursor += ool_size;
        }
        if !msg.inline_data.is_empty() {
            wire[cursor..cursor + msg.inline_data.len()].copy_from_slice(&msg.inline_data);
        }
    }

    if syscall::copy_to_user_bytes_in(mapping, Some(allocator), pml4, msg_ptr, &wire).is_err() {
        return -14;
    }
    0
}

pub fn sys_mach_msg_current(
    msg_ptr: u64,
    option: u32,
    send_size: usize,
    rcv_size: usize,
    rcv_name: MachPortName,
    timeout_ms: u64,
    mapping: &PhysicalMemoryMapping,
    allocator: &mut PhysicalFrameAllocator<'_>,
) -> Result<i64, WaitReason> {
    let caller = without_interrupts(|| {
        let sched = USER_SCHED.lock();
        let idx = sched.current_slot?;
        let p = sched.slots[idx].as_ref()?;
        Some((idx, p.pid, p.pml4))
    });
    let Some((slot_idx, my_pid, pml4)) = caller else {
        return Ok(-22);
    };

    if (option & MACH_SEND_MSG) != 0 {
        let hdr_size = core::mem::size_of::<MachMsgHeader>();
        let ool_size = core::mem::size_of::<MachMsgOolDesc>();
        let max_wire = hdr_size + ool_size + ipc::MAX_MACH_INLINE_BYTES;
        if send_size < hdr_size || send_size > max_wire {
            return Ok(-22);
        }

        let mut wire = alloc::vec![0u8; send_size];
        if syscall::copy_from_user_bytes_in(mapping, Some(allocator), pml4, msg_ptr, &mut wire)
            .is_err()
        {
            return Ok(-14);
        }

        let hdr: MachMsgHeader =
            unsafe { core::ptr::read_unaligned(wire.as_ptr() as *const MachMsgHeader) };
        let declared = hdr.size as usize;
        if declared < hdr_size || declared > send_size {
            return Ok(-22);
        }

        let is_complex = (hdr.bits & MACH_MSGH_BITS_COMPLEX) != 0;
        let inline_offset = if is_complex {
            if declared < hdr_size + ool_size {
                return Ok(-22);
            }
            hdr_size + ool_size
        } else {
            hdr_size
        };
        let inline_len = declared - inline_offset;
        if inline_len > ipc::MAX_MACH_INLINE_BYTES {
            return Ok(-22);
        }

        // Resolve destination remote_port and local reply_port rights
        let remote_disp = hdr.bits & MACH_MSGH_BITS_REMOTE_MASK;
        let local_disp = (hdr.bits & MACH_MSGH_BITS_LOCAL_MASK) >> 8;

        let resolved = without_interrupts(|| {
            let sched = USER_SCHED.lock();
            let p = sched.slots[slot_idx].as_ref()?;
            let remote_entry = p.find_port_right(hdr.remote_port)?;
            let consume_remote_so = match remote_entry.right {
                PortRightKind::Send => remote_disp == MACH_MSG_TYPE_MOVE_SEND,
                PortRightKind::Receive if remote_entry.also_send => false,
                PortRightKind::SendOnce => {
                    remote_disp == 0 || remote_disp == MACH_MSG_TYPE_MOVE_SEND_ONCE
                }
                _ => return None,
            };

            let reply_info = if hdr.local_port != MACH_PORT_NULL {
                let le = p.find_port_right(hdr.local_port)?;
                let effective_disp = if local_disp == 0 {
                    match le.right {
                        PortRightKind::Receive => MACH_MSG_TYPE_MAKE_SEND_ONCE,
                        PortRightKind::Send => MACH_MSG_TYPE_COPY_SEND,
                        PortRightKind::SendOnce => MACH_MSG_TYPE_MOVE_SEND_ONCE,
                    }
                } else {
                    local_disp
                };
                match effective_disp {
                    MACH_MSG_TYPE_MAKE_SEND => {
                        if le.right != PortRightKind::Receive {
                            return None;
                        }
                        Some((le.port_id, PortRightKind::Send, false, false))
                    }
                    MACH_MSG_TYPE_MAKE_SEND_ONCE => {
                        if le.right != PortRightKind::Receive {
                            return None;
                        }
                        Some((le.port_id, PortRightKind::SendOnce, false, false))
                    }
                    MACH_MSG_TYPE_COPY_SEND => {
                        if !(le.right == PortRightKind::Send
                            || (le.right == PortRightKind::Receive && le.also_send))
                        {
                            return None;
                        }
                        Some((le.port_id, PortRightKind::Send, false, false))
                    }
                    MACH_MSG_TYPE_MOVE_SEND => {
                        if le.right != PortRightKind::Send {
                            return None;
                        }
                        Some((le.port_id, PortRightKind::Send, true, false))
                    }
                    MACH_MSG_TYPE_MOVE_SEND_ONCE => {
                        if le.right != PortRightKind::SendOnce {
                            return None;
                        }
                        Some((le.port_id, PortRightKind::SendOnce, false, true))
                    }
                    _ => return None,
                }
            } else {
                None
            };

            Some((remote_entry.port_id, remote_entry.right, consume_remote_so, reply_info))
        });

        let Some((target_port_id, remote_kind, remove_remote_after, reply_info)) = resolved else {
            return Ok(-22);
        };

        // Parse and zero-copy retain/unmap OOL pages if complex message
        let mut ool_payload: Option<MachOolPayload> = None;
        if is_complex {
            let desc_bytes = &wire[hdr_size..hdr_size + ool_size];
            let ool_addr = u64::from_le_bytes(desc_bytes[0..8].try_into().unwrap());
            let ool_len = u64::from_le_bytes(desc_bytes[8..16].try_into().unwrap());
            let deallocate = desc_bytes[16] != 0;

            if ool_len > 0 {
                if ool_addr == 0 || (ool_addr & (PAGE_SIZE - 1)) != 0 {
                    return Ok(-22);
                }
                let page_count = ((ool_len as usize) + PAGE_SIZE as usize - 1) / (PAGE_SIZE as usize);
                if page_count == 0 || page_count > ipc::MAX_MACH_OOL_FRAMES {
                    return Ok(-12);
                }
                let Some(ool_end) = ool_addr.checked_add((page_count as u64) * PAGE_SIZE) else {
                    return Ok(-22);
                };
                if ool_end > USER_ADDR_LIMIT {
                    return Ok(-14);
                }

                if ensure_user_pages_committed(
                    pml4,
                    ool_addr,
                    ool_end - PAGE_SIZE,
                    false,
                    mapping,
                    Some(allocator),
                )
                .is_err()
                {
                    return Ok(-14);
                }

                let mut frames = Vec::with_capacity(page_count);
                for i in 0..page_count {
                    let va = ool_addr + (i as u64) * PAGE_SIZE;
                    let Ok(virt) = VirtAddr::try_new(va) else {
                        for fr in frames {
                            let _ = allocator.deallocate_frame(fr);
                        }
                        return Ok(-14);
                    };
                    let Some(trans) = mapping.translate_virtual_address_in(pml4, virt) else {
                        for fr in frames {
                            let _ = allocator.deallocate_frame(fr);
                        }
                        return Ok(-14);
                    };
                    let phys = trans.physical_address & !(PAGE_SIZE - 1);

                    if deallocate {
                        if mapping.unmap_page_in(pml4, virt).is_err() {
                            for fr in frames {
                                let _ = allocator.deallocate_frame(fr);
                            }
                            return Ok(-14);
                        }
                        frames.push(phys);
                    } else {
                        if !crate::memory::inc_frame_ref(allocator, phys) {
                            for fr in frames {
                                let _ = allocator.deallocate_frame(fr);
                            }
                            return Ok(-12);
                        }
                        if trans.flags.contains(PageTableFlags::WRITABLE) {
                            let _ = mapping.mark_page_cow_in(pml4, virt);
                        }
                        frames.push(phys);
                    }
                }

                if deallocate {
                    without_interrupts(|| {
                        let mut sched = USER_SCHED.lock();
                        if let Some(ref mut proc) = sched.slots[slot_idx] {
                            let mut removed_count = 0usize;
                            if let Some(ref mut img) = proc.image {
                                let before = img.mapped_pages.len();
                                img.mapped_pages
                                    .retain(|&(va, _)| va < ool_addr || va >= ool_end);
                                removed_count = before.saturating_sub(img.mapped_pages.len());
                                proc.pages_mapped = img.mapped_pages.len();
                            }
                            proc.remove_mmap_range(ool_addr, ool_end);
                            proc.dynamic_bytes = proc
                                .dynamic_bytes
                                .saturating_sub(removed_count * (PAGE_SIZE as usize));
                        }
                    });
                }

                ool_payload = Some(MachOolPayload {
                    frames,
                    size_bytes: ool_len,
                });
            }
        }

        // Retain non-moved reply right
        let (reply_port_id, reply_right, move_local) = match reply_info {
            Some((rpid, rk, mv_s, mv_so)) => {
                if !mv_s && !mv_so && !ipc::mach_retain_right(rpid, rk, false) {
                    if let Some(ool) = ool_payload {
                        for fr in ool.frames {
                            let _ = allocator.deallocate_frame(fr);
                        }
                    }
                    return Ok(-32);
                }
                (rpid, Some(rk), mv_s || mv_so)
            }
            None => (0, None, false),
        };

        let kmsg = MachKernelMsg {
            bits: hdr.bits,
            id: hdr.id,
            reply_port_id,
            reply_right,
            inline_data: wire[inline_offset..declared].to_vec(),
            ool: ool_payload,
        };

        let consume_so_in_port = remote_kind == PortRightKind::SendOnce;
        if let Err((err, returned_msg)) = ipc::mach_send_msg(target_port_id, consume_so_in_port, kmsg)
        {
            // Clean up OOL frames and reply right (unless caller still retains moved local right)
            if let Some(ool) = returned_msg.ool {
                for fr in ool.frames {
                    let _ = allocator.deallocate_frame(fr);
                }
            }
            if returned_msg.reply_port_id != 0 && !move_local {
                if let Some(rr) = returned_msg.reply_right {
                    ipc::mach_release_port_id_right(returned_msg.reply_port_id, rr, false, allocator);
                }
            }
            if consume_so_in_port {
                without_interrupts(|| {
                    let mut sched = USER_SCHED.lock();
                    if let Some(ref mut p) = sched.slots[slot_idx] {
                        let _ = p.take_port_right(hdr.remote_port);
                    }
                });
            }
            return Ok(err);
        }

        // Commit local/remote MOVE consumptions after successful enqueue
        if remove_remote_after || consume_so_in_port || move_local {
            without_interrupts(|| {
                let mut sched = USER_SCHED.lock();
                if let Some(ref mut p) = sched.slots[slot_idx] {
                    if remove_remote_after || consume_so_in_port {
                        let _ = p.take_port_right(hdr.remote_port);
                    }
                    if move_local {
                        let _ = p.take_port_right(hdr.local_port);
                    }
                }
            });
        }
    }

    if (option & MACH_RCV_MSG) == 0 {
        return Ok(0);
    }

    let rcv_port_id = without_interrupts(|| {
        let sched = USER_SCHED.lock();
        let p = sched.slots[slot_idx].as_ref()?;
        let entry = p.find_port_right(rcv_name)?;
        if entry.right == PortRightKind::Receive {
            Some(entry.port_id)
        } else {
            None
        }
    });
    let Some(port_id) = rcv_port_id else {
        return Ok(-22);
    };

    if rcv_size < core::mem::size_of::<MachMsgHeader>() {
        return Ok(-22);
    }

    match ipc::mach_try_recv_msg(port_id) {
        Ok(msg) => Ok(deliver_mach_msg_to_slot(
            slot_idx,
            my_pid,
            pml4,
            port_id,
            rcv_name,
            msg,
            msg_ptr,
            rcv_size,
            mapping,
            allocator,
        )),
        Err(-11) => {
            if (option & MACH_RCV_TIMEOUT) != 0 && timeout_ms == 0 {
                return Ok(-11);
            }
            let wait_ms = if (option & MACH_RCV_TIMEOUT) != 0 {
                timeout_ms.min(30_000)
            } else {
                10_000
            };
            let deadline_ms = timer::uptime_ms().saturating_add(wait_ms);
            Err(WaitReason::MachPortRecv {
                port_id,
                rcv_name,
                msg_ptr,
                rcv_cap: rcv_size,
                deadline_ms,
            })
        }
        Err(e) => Ok(e),
    }
}

pub fn sys_kevent_current(
    fd: usize,
    changelist_ptr: u64,
    nchanges: usize,
    eventlist_ptr: u64,
    nevents: usize,
    timeout_ptr: u64,
    mapping: &PhysicalMemoryMapping,
    allocator: &mut PhysicalFrameAllocator<'_>,
) -> Result<i64, WaitReason> {
    let caller = without_interrupts(|| {
        let sched = USER_SCHED.lock();
        let idx = sched.current_slot?;
        let proc = sched.slots[idx].as_ref()?;
        if fd < FD_OFFSET || fd >= FD_OFFSET + MAX_PROC_FDS {
            return None;
        }
        let Some(FdEntry::Kqueue { kq_id }) = proc.fds[fd - FD_OFFSET] else {
            return None;
        };
        Some((idx, proc.pml4, kq_id, Box::new(proc.fds.clone())))
    });

    let Some((_slot_idx, pml4, kq_id, owner_fds)) = caller else {
        return Ok(-9); // EBADF
    };

    let now_ms = timer::uptime_ms();
    let kevent_sz = core::mem::size_of::<KEvent>();

    if nchanges > 0 {
        let capped = nchanges.min(64);
        let byte_len = capped * kevent_sz;
        let mut raw = alloc::vec![0u8; byte_len];
        if syscall::copy_from_user_bytes_in(mapping, Some(allocator), pml4, changelist_ptr, &mut raw)
            .is_err()
        {
            return Ok(-14);
        }
        let mut changes = Vec::with_capacity(capped);
        for i in 0..capped {
            let ev = unsafe {
                core::ptr::read_unaligned(raw[i * kevent_sz..].as_ptr() as *const KEvent)
            };
            changes.push(ev);
        }
        if let Err(e) = kqueue::apply_changes(kq_id, &changes, now_ms) {
            return Ok(e);
        }
    }

    if nevents == 0 || eventlist_ptr == 0 {
        return Ok(0);
    }

    let capped_ev = nevents.min(64);
    let wait_ms = if timeout_ptr == 0 {
        10_000u64
    } else {
        let mut raw_ts = [0u8; core::mem::size_of::<freebsd::TimeSpec>()];
        if syscall::copy_from_user_bytes_in(mapping, Some(allocator), pml4, timeout_ptr, &mut raw_ts)
            .is_err()
        {
            return Ok(-14);
        }
        let ts: freebsd::TimeSpec =
            unsafe { core::ptr::read_unaligned(raw_ts.as_ptr() as *const freebsd::TimeSpec) };
        if ts.tv_sec < 0 || ts.tv_nsec < 0 || ts.tv_nsec >= 1_000_000_000 {
            return Ok(-22);
        }
        ((ts.tv_sec as u64).saturating_mul(1000))
            .saturating_add((ts.tv_nsec as u64) / 1_000_000)
            .min(30_000)
    };

    match kqueue::poll_events(
        kq_id,
        owner_fds.as_slice(),
        capped_ev,
        now_ms,
        &|target| is_process_alive(target),
    ) {
        Ok(events) if !events.is_empty() => {
            let raw_bytes = unsafe {
                core::slice::from_raw_parts(events.as_ptr() as *const u8, events.len() * kevent_sz)
            };
            if syscall::copy_to_user_bytes_in(
                mapping,
                Some(allocator),
                pml4,
                eventlist_ptr,
                raw_bytes,
            )
            .is_err()
            {
                Ok(-14)
            } else {
                Ok(events.len() as i64)
            }
        }
        Ok(_) => {
            if wait_ms == 0 {
                Ok(0)
            } else {
                Err(WaitReason::Kqueue {
                    kq_id,
                    eventlist_ptr,
                    nevents: capped_ev,
                    deadline_ms: now_ms.saturating_add(wait_ms),
                })
            }
        }
        Err(e) => Ok(e),
    }
}

pub fn sys_sigprocmask_current(
    how: i32,
    set_ptr: u64,
    oset_ptr: u64,
    mapping: &PhysicalMemoryMapping,
    allocator: &mut PhysicalFrameAllocator<'_>,
) -> i64 {
    let info = without_interrupts(|| {
        let sched = USER_SCHED.lock();
        let idx = sched.current_slot?;
        let proc = sched.slots[idx].as_ref()?;
        Some((idx, proc.pml4, proc.signals.blocked_mask))
    });
    let Some((slot_idx, pml4, old_mask)) = info else {
        return -22;
    };

    let sigset_sz = core::mem::size_of::<FreeBsdSigSet>();
    if oset_ptr != 0 {
        let old_set = FreeBsdSigSet::from_mask64(old_mask);
        let bytes = unsafe {
            core::slice::from_raw_parts((&old_set as *const FreeBsdSigSet) as *const u8, sigset_sz)
        };
        if syscall::copy_to_user_bytes_in(mapping, Some(allocator), pml4, oset_ptr, bytes).is_err()
        {
            return -14;
        }
    }

    if set_ptr != 0 {
        let mut raw = [0u8; core::mem::size_of::<FreeBsdSigSet>()];
        if syscall::copy_from_user_bytes_in(mapping, Some(allocator), pml4, set_ptr, &mut raw)
            .is_err()
        {
            return -14;
        }
        let in_set: FreeBsdSigSet =
            unsafe { core::ptr::read_unaligned(raw.as_ptr() as *const FreeBsdSigSet) };
        let req = in_set.to_mask64() & !signal::sig_bit(SIGKILL);
        let new_mask = match how {
            SIG_BLOCK => old_mask | req,
            SIG_UNBLOCK => old_mask & !req,
            SIG_SETMASK => req,
            _ => return -22,
        };
        without_interrupts(|| {
            let mut sched = USER_SCHED.lock();
            if let Some(ref mut proc) = sched.slots[slot_idx] {
                proc.signals.blocked_mask = new_mask;
            }
        });
    }
    0
}

pub fn sys_sigaction_current(
    sig: u32,
    act_ptr: u64,
    oact_ptr: u64,
    mapping: &PhysicalMemoryMapping,
    allocator: &mut PhysicalFrameAllocator<'_>,
) -> i64 {
    if sig == 0 || sig > signal::MAX_SIGNAL || sig == SIGKILL {
        return -22;
    }

    let info = without_interrupts(|| {
        let sched = USER_SCHED.lock();
        let idx = sched.current_slot?;
        let proc = sched.slots[idx].as_ref()?;
        Some((idx, proc.pml4, proc.signals.actions[sig as usize]))
    });
    let Some((slot_idx, pml4, prev_cfg)) = info else {
        return -22;
    };

    let act_sz = core::mem::size_of::<FreeBsdSigAction>();
    if oact_ptr != 0 {
        let oact = FreeBsdSigAction {
            sa_handler: prev_cfg.handler,
            sa_flags: prev_cfg.flags,
            _pad: 0,
            sa_mask: FreeBsdSigSet::from_mask64(prev_cfg.mask).bits,
        };
        let bytes = unsafe {
            core::slice::from_raw_parts((&oact as *const FreeBsdSigAction) as *const u8, act_sz)
        };
        if syscall::copy_to_user_bytes_in(mapping, Some(allocator), pml4, oact_ptr, bytes).is_err()
        {
            return -14;
        }
    }

    if act_ptr != 0 {
        let mut raw = [0u8; core::mem::size_of::<FreeBsdSigAction>()];
        if syscall::copy_from_user_bytes_in(mapping, Some(allocator), pml4, act_ptr, &mut raw)
            .is_err()
        {
            return -14;
        }
        let act: FreeBsdSigAction =
            unsafe { core::ptr::read_unaligned(raw.as_ptr() as *const FreeBsdSigAction) };
        let new_cfg = SignalHandlerConfig {
            handler: act.sa_handler,
            flags: act.sa_flags,
            mask: (act.sa_mask[0] as u64) & 0xFFFF_FFFF & !signal::sig_bit(SIGKILL),
        };
        without_interrupts(|| {
            let mut sched = USER_SCHED.lock();
            if let Some(ref mut proc) = sched.slots[slot_idx] {
                proc.signals.actions[sig as usize] = new_cfg;
            }
        });
    }
    0
}

pub fn sys_sigreturn_current(
    frame: &mut SyscallFrame,
    uctx_ptr: u64,
    mapping: &PhysicalMemoryMapping,
    allocator: &mut PhysicalFrameAllocator<'_>,
) -> i64 {
    let info = without_interrupts(|| {
        let sched = USER_SCHED.lock();
        let idx = sched.current_slot?;
        let proc = sched.slots[idx].as_ref()?;
        Some((idx, proc.pml4))
    });
    let Some((slot_idx, pml4)) = info else {
        return -22;
    };

    let mut raw = [0u8; core::mem::size_of::<SavedSignalFrame>()];
    if syscall::copy_from_user_bytes_in(mapping, Some(allocator), pml4, uctx_ptr, &mut raw).is_err()
    {
        return -14;
    }
    let saved: SavedSignalFrame =
        unsafe { core::ptr::read_unaligned(raw.as_ptr() as *const SavedSignalFrame) };
    if saved.rip == 0 || saved.rip >= USER_ADDR_LIMIT {
        return -22;
    }

    frame.r15 = saved.r15;
    frame.r14 = saved.r14;
    frame.r13 = saved.r13;
    frame.r12 = saved.r12;
    frame.rbp = saved.rbp;
    frame.rbx = saved.rbx;
    frame.r10 = saved.r10;
    frame.r9 = saved.r9;
    frame.r8 = saved.r8;
    frame.rax = saved.rax;
    frame.rdx = saved.rdx;
    frame.rsi = saved.rsi;
    frame.rdi = saved.rdi;
    frame.user_rip = saved.rip;
    frame.user_rflags = (saved.rflags & 0x0020_0CD5) | 0x0202 | (saved.rflags & 1);
    frame.user_rsp = saved.rsp;

    without_interrupts(|| {
        let mut sched = USER_SCHED.lock();
        if let Some(ref mut proc) = sched.slots[slot_idx] {
            proc.signals.blocked_mask = saved.old_blocked_mask & !signal::sig_bit(SIGKILL);
        }
    });
    0
}

pub fn deliver_pending_signal_on_syscall_return(
    frame: &mut SyscallFrame,
    mapping: &PhysicalMemoryMapping,
    allocator: &mut PhysicalFrameAllocator<'_>,
) {
    let cur_idx = without_interrupts(|| USER_SCHED.lock().current_slot);
    let Some(slot_idx) = cur_idx else {
        return;
    };

    loop {
        let next = without_interrupts(|| {
            let mut sched = USER_SCHED.lock();
            let proc = sched.slots[slot_idx].as_mut()?;
            let sig = proc.signals.next_deliverable()?;
            proc.signals.clear_pending(sig);
            let cfg = proc.signals.actions[sig as usize];
            Some((proc.pml4, proc.signals.blocked_mask, sig, cfg))
        });

        let Some((pml4, old_blocked, sig, cfg)) = next else {
            return;
        };

        if sig == SIGKILL {
            unsafe {
                finish_current_from_syscall(-9);
            }
        }
        if cfg.handler == SIG_IGN {
            continue;
        }
        if cfg.handler == SIG_DFL {
            if signal::default_signal_action(sig) == SignalDefault::Terminate {
                unsafe {
                    finish_current_from_syscall(-(sig as i64));
                }
            }
            continue;
        }

        if !ensure_sigtramp_mapped_for_slot(slot_idx, mapping, allocator) {
            return;
        }

        let frame_size = core::mem::size_of::<SavedSignalFrame>() as u64;
        let frame_vaddr =
            ((frame.user_rsp.saturating_sub(128)) & !0xF).saturating_sub(frame_size) & !0xF;
        let ret_addr_rsp = frame_vaddr.saturating_sub(8);

        let saved = SavedSignalFrame {
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
            rax: frame.rax,
            rcx: frame.user_rip,
            rdx: frame.rdx,
            rsi: frame.rsi,
            rdi: frame.rdi,
            rip: frame.user_rip,
            rflags: frame.user_rflags,
            rsp: frame.user_rsp,
            old_blocked_mask: old_blocked,
            signo: sig,
            reserved: 0,
        };

        let frame_bytes = unsafe {
            core::slice::from_raw_parts(
                (&saved as *const SavedSignalFrame) as *const u8,
                frame_size as usize,
            )
        };
        if syscall::copy_to_user_bytes_in(
            mapping,
            Some(allocator),
            pml4,
            frame_vaddr,
            frame_bytes,
        )
        .is_err()
            || syscall::copy_to_user_bytes_in(
                mapping,
                Some(allocator),
                pml4,
                ret_addr_rsp,
                &USER_SIGTRAMP_ADDR.to_le_bytes(),
            )
            .is_err()
        {
            unsafe {
                finish_current_from_syscall(-(sig as i64));
            }
        }

        without_interrupts(|| {
            let mut sched = USER_SCHED.lock();
            if let Some(ref mut proc) = sched.slots[slot_idx] {
                proc.signals.blocked_mask |= cfg.mask | signal::sig_bit(sig);
            }
        });

        frame.user_rip = cfg.handler;
        frame.user_rsp = ret_addr_rsp;
        frame.rdi = sig as u64;
        frame.rsi = 0;
        frame.rdx = frame_vaddr;
        frame.user_rflags &= !1;
        return;
    }
}

pub fn sys_kill_current(target_pid: isize, sig: u32) -> i64 {
    if sig > signal::MAX_SIGNAL {
        return -22;
    }
    without_interrupts(|| {
        let mut sched = USER_SCHED.lock();
        let my_tgid = sched
            .current_slot
            .and_then(|idx| sched.slots[idx].as_ref().map(|p| p.tgid))
            .unwrap_or(0);
        let dest_tgid = if target_pid <= 0 {
            my_tgid
        } else {
            target_pid as usize
        };
        let mut delivered = false;
        for proc in sched.slots.iter_mut().flatten() {
            if proc.tgid == dest_tgid {
                if sig != 0 {
                    proc.signals.raise(sig);
                }
                delivered = true;
                break;
            }
        }
        if delivered {
            0
        } else {
            -3 // ESRCH
        }
    })
}

pub fn sys_thr_kill_current(tid: i64, sig: u32) -> i64 {
    if sig > signal::MAX_SIGNAL {
        return -22;
    }
    without_interrupts(|| {
        let mut sched = USER_SCHED.lock();
        let my_tgid = sched
            .current_slot
            .and_then(|idx| sched.slots[idx].as_ref().map(|p| p.tgid))
            .unwrap_or(0);
        let target_pid = if tid <= 0 {
            sched
                .current_slot
                .and_then(|idx| sched.slots[idx].as_ref().map(|p| p.pid))
                .unwrap_or(0)
        } else {
            tid as usize
        };
        for proc in sched.slots.iter_mut().flatten() {
            if proc.pid == target_pid && proc.tgid == my_tgid {
                if sig != 0 {
                    proc.signals.raise(sig);
                }
                return 0;
            }
        }
        -3 // ESRCH
    })
}

struct ThrCallerInfo {
    tgid: usize,
    parent_pid: usize,
    abi: ProcessAbi,
    parent_name: String,
    pml4: u64,
    weight: u32,
    entry_point: u64,
    pages_mapped: usize,
    bytes_loaded: u64,
    brk_base: u64,
    brk_current: u64,
    mmap_next: u64,
    shm_attach_next: u64,
    vmas: Vec<VmRegion>,
    thread_fds: [Option<FdEntry>; MAX_PROC_FDS],
    thread_signals: SignalState,
}

pub fn sys_thr_new_current(
    param_ptr: u64,
    param_size: usize,
    mapping: &PhysicalMemoryMapping,
    allocator: &mut PhysicalFrameAllocator<'_>,
) -> i64 {
    if active_count() >= MAX_USER_PROCS {
        return -11; // EAGAIN
    }
    let expected_sz = core::mem::size_of::<ThrParam>();
    if param_size < expected_sz || param_size > 256 || param_ptr == 0 {
        return -22;
    }

    let caller_info = without_interrupts(|| {
        let sched = USER_SCHED.lock();
        let idx = sched.current_slot?;
        let p = sched.slots[idx].as_ref()?;
        Some(Box::new(ThrCallerInfo {
            tgid: p.tgid,
            parent_pid: p.parent_pid,
            abi: p.abi,
            parent_name: p.name.clone(),
            pml4: p.pml4,
            weight: p.weight,
            entry_point: p.entry_point,
            pages_mapped: p.pages_mapped,
            bytes_loaded: p.bytes_loaded,
            brk_base: p.brk_base,
            brk_current: p.brk_current,
            mmap_next: p.mmap_next,
            shm_attach_next: p.shm_attach_next,
            vmas: p.vmas.clone(),
            thread_fds: p.fds.clone(),
            thread_signals: p.signals.clone_for_fork(),
        }))
    });

    let Some(mut info) = caller_info else {
        return -22;
    };
    let pml4 = info.pml4;

    let mut raw = alloc::vec![0u8; expected_sz];
    if syscall::copy_from_user_bytes_in(mapping, Some(allocator), pml4, param_ptr, &mut raw)
        .is_err()
    {
        return -14;
    }
    let param: ThrParam = unsafe { core::ptr::read_unaligned(raw.as_ptr() as *const ThrParam) };

    if param.start_func == 0 || param.start_func >= USER_ADDR_LIMIT {
        return -22;
    }
    if param.stack_base == 0 || param.stack_size < 64 {
        return -22;
    }
    let Some(stack_top) = param.stack_base.checked_add(param.stack_size) else {
        return -22;
    };
    if stack_top > USER_ADDR_LIMIT {
        return -14;
    }
    let entry_rsp = (stack_top & !0xF).saturating_sub(8);
    if syscall::copy_to_user_bytes_in(mapping, Some(allocator), pml4, entry_rsp, &0u64.to_le_bytes())
        .is_err()
    {
        return -14;
    }

    for slot in &mut info.thread_fds {
        match slot {
            Some(FdEntry::PipeRead { pipe_id }) => ipc::retain_pipe_end(*pipe_id, false),
            Some(FdEntry::PipeWrite { pipe_id }) => ipc::retain_pipe_end(*pipe_id, true),
            Some(FdEntry::Socket(_)) | Some(FdEntry::Kqueue { .. }) => *slot = None,
            _ => {}
        }
    }

    let selectors = crate::arch::gdt::selectors();
    let ctx = UserCpuContext {
        rip: param.start_func,
        rsp: entry_rsp,
        rdi: param.arg,
        cs: selectors.user_code.0 as u64,
        ss: selectors.user_data.0 as u64,
        rflags: 0x0202,
        ..UserCpuContext::default()
    };

    let mut thread_slot = Box::new(UserProcessSlot {
        pid: 0,
        tgid: info.tgid,
        is_thread: true,
        parent_pid: info.parent_pid,
        abi: info.abi,
        name: String::new(),
        pml4,
        ctx,
        fpu_state: FxSaveArea::new_clean(),
        fs_base: param.tls_base,
        gs_base: 0,
        clear_child_tid: param.child_tid,
        signals: info.thread_signals,
        state: UserTaskState::Ready,
        cpu_ms: 0,
        vruntime_ms: 0,
        weight: info.weight,
        preempt_count: 0,
        lazy_faults: 0,
        syscalls_served: 0,
        entry_point: info.entry_point,
        pages_mapped: info.pages_mapped,
        bytes_loaded: info.bytes_loaded,
        brk_base: info.brk_base,
        brk_current: info.brk_current,
        mmap_next: info.mmap_next,
        shm_attach_next: info.shm_attach_next,
        shm_attachments: Vec::new(),
        owned_shm_ids: Vec::new(),
        dynamic_bytes: 0,
        vmas: info.vmas,
        image: None,
        fds: info.thread_fds,
        port_rights: Vec::new(),
        next_port_name: 1,
        stdout_buf: String::new(),
        has_gui_window: false,
    });

    let tid_res: Result<usize, Box<UserProcessSlot>> = without_interrupts(|| {
        let mut sched = USER_SCHED.lock();
        let Some(empty_idx) = sched.slots.iter().position(|s| s.is_none()) else {
            return Err(thread_slot);
        };
        let tid = sched.alloc_tid();
        let start_vr = sched.min_runnable_vruntime();
        thread_slot.pid = tid;
        thread_slot.name = format!("{}[thr:{}]", info.parent_name, tid);
        thread_slot.vruntime_ms = start_vr;
        sched.slots[empty_idx] = Some(thread_slot);
        Ok(tid)
    });

    let tid = match tid_res {
        Ok(tid) => tid,
        Err(mut returned_slot) => {
            close_fd_list(&mut returned_slot.fds);
            return -11;
        }
    };

    let tid_bytes = (tid as i64).to_le_bytes();
    if param.child_tid != 0 {
        let _ = syscall::copy_to_user_bytes_in(
            mapping,
            Some(allocator),
            pml4,
            param.child_tid,
            &tid_bytes,
        );
    }
    if param.parent_tid != 0 {
        let _ = syscall::copy_to_user_bytes_in(
            mapping,
            Some(allocator),
            pml4,
            param.parent_tid,
            &tid_bytes,
        );
    }

    0
}

pub fn clear_current_tid_on_exit(
    state_ptr: u64,
    mapping: &PhysicalMemoryMapping,
    allocator: &mut PhysicalFrameAllocator<'_>,
) -> i64 {
    let info = without_interrupts(|| {
        let sched = USER_SCHED.lock();
        let idx = sched.current_slot?;
        let proc = sched.slots[idx].as_ref()?;
        let target = if state_ptr != 0 {
            state_ptr
        } else {
            proc.clear_child_tid
        };
        Some((proc.pml4, target))
    });
    let Some((pml4, addr)) = info else {
        return 0;
    };
    if addr != 0 {
        let _ = syscall::copy_to_user_bytes_in(
            mapping,
            Some(allocator),
            pml4,
            addr,
            &1i64.to_le_bytes(),
        );
        wake_umtx_waiters(pml4, addr, usize::MAX);
    }
    0
}

fn wake_umtx_waiters(pml4: u64, addr: u64, max_wake: usize) -> i64 {
    let mut woken = 0usize;
    without_interrupts(|| {
        let mut sched = USER_SCHED.lock();
        let min_vr = sched.min_runnable_vruntime();
        for slot in &mut sched.slots {
            if woken >= max_wake {
                break;
            }
            if let Some(ref mut proc) = slot {
                if let UserTaskState::Waiting(WaitReason::UmtxWait {
                    pml4: w_pml4,
                    addr: w_addr,
                    ..
                }) = proc.state
                {
                    if w_pml4 == pml4 && w_addr == addr {
                        proc.vruntime_ms = proc
                            .vruntime_ms
                            .max(min_vr.saturating_sub(DEFAULT_QUANTUM_MS));
                        proc.ctx.rax =
                            freebsd::apply_abi_syscall_result(proc.abi, 0, &mut proc.ctx.rflags);
                        proc.state = UserTaskState::Ready;
                        woken += 1;
                    }
                }
            }
        }
    });
    woken as i64
}

pub fn sys_umtx_op_current(
    obj_ptr: u64,
    op: u32,
    val: u64,
    uaddr1: u64,
    uaddr2: u64,
    mapping: &PhysicalMemoryMapping,
    allocator: &mut PhysicalFrameAllocator<'_>,
) -> Result<i64, WaitReason> {
    if obj_ptr == 0 {
        return Ok(-14);
    }
    let pml4 = without_interrupts(|| {
        let sched = USER_SCHED.lock();
        sched
            .current_slot
            .and_then(|idx| sched.slots[idx].as_ref().map(|p| p.pml4))
            .unwrap_or(0)
    });
    if pml4 == 0 {
        return Ok(-22);
    }

    match op {
        UMTX_OP_WAKE | UMTX_OP_WAKE_PRIVATE => {
            let max_wake = if val == 0 { 1 } else { val as usize };
            Ok(wake_umtx_waiters(pml4, obj_ptr, max_wake))
        }
        UMTX_OP_WAIT | UMTX_OP_WAIT_UINT | UMTX_OP_WAIT_UINT_PRIVATE => {
            let is_u64 = op == UMTX_OP_WAIT;
            let sz = if is_u64 { 8 } else { 4 };
            let mut raw = [0u8; 8];
            if syscall::copy_from_user_bytes_in(mapping, Some(allocator), pml4, obj_ptr, &mut raw[..sz])
                .is_err()
            {
                return Ok(-14);
            }
            let cur = if is_u64 {
                u64::from_le_bytes(raw)
            } else {
                u32::from_le_bytes(raw[0..4].try_into().unwrap()) as u64
            };
            let expected = if is_u64 { val } else { val & 0xFFFF_FFFF };
            if cur != expected {
                return Ok(0);
            }

            let wait_ms = if uaddr1 as usize >= core::mem::size_of::<freebsd::TimeSpec>() && uaddr2 != 0 {
                let mut ts_raw = [0u8; core::mem::size_of::<freebsd::TimeSpec>()];
                if syscall::copy_from_user_bytes_in(mapping, Some(allocator), pml4, uaddr2, &mut ts_raw)
                    .is_err()
                {
                    return Ok(-14);
                }
                let ts: freebsd::TimeSpec = unsafe {
                    core::ptr::read_unaligned(ts_raw.as_ptr() as *const freebsd::TimeSpec)
                };
                ((ts.tv_sec.max(0) as u64).saturating_mul(1000))
                    .saturating_add((ts.tv_nsec.max(0) as u64) / 1_000_000)
                    .clamp(1, 30_000)
            } else {
                10_000
            };

            Err(WaitReason::UmtxWait {
                pml4,
                addr: obj_ptr,
                val: expected,
                is_u64,
                deadline_ms: timer::uptime_ms().saturating_add(wait_ms),
            })
        }
        _ => Ok(-22),
    }
}

