use crate::console::Console;
use crate::memory::vmm::PhysicalMemoryMapping;
use crate::memory::PhysicalFrameAllocator;
use crate::process::{ProcessState, PROCESS_MANAGER};

use super::elf::{self, ElfError};
use super::sched;
use super::syscall::{self, UserTrapOutcome};

pub struct ExecutionReport {
    pub pid: usize,
    pub entry_point: u64,
    pub pages_mapped: usize,
    pub bytes_loaded: u64,
    pub outcome: UserTrapOutcome,
}

#[derive(Clone, Copy)]
pub enum RunElfError {
    NoPhysicalMapping,
    Elf(ElfError),
    ProcessCapacityReached,
    ProcessMemoryFailed,
}

impl RunElfError {
    pub fn message(self) -> &'static str {
        match self {
            Self::NoPhysicalMapping => {
                "Physical memory mapping is unavailable"
            }
            Self::Elf(err) => err.message(),
            Self::ProcessCapacityReached => {
                "Process table capacity reached"
            }
            Self::ProcessMemoryFailed => {
                "Failed to allocate isolated address space for process"
            }
        }
    }
}

impl From<ElfError> for RunElfError {
    fn from(value: ElfError) -> Self {
        Self::Elf(value)
    }
}

/// Spawns an ELF64 executable asynchronously into the preemptive Ring 3 scheduler.
/// Returns its assigned PID immediately without blocking the kernel or Desktop GUI.
pub fn spawn_elf_async(
    path: &str,
    mapping: Option<&PhysicalMemoryMapping>,
    allocator: &mut PhysicalFrameAllocator<'_>,
) -> Result<usize, RunElfError> {
    spawn_elf_async_with_args(path, &[path], mapping, allocator)
}

/// Spawns an ELF64 executable asynchronously with explicit SysV `argv` arguments.
pub fn spawn_elf_async_with_args(
    path: &str,
    args: &[&str],
    mapping: Option<&PhysicalMemoryMapping>,
    allocator: &mut PhysicalFrameAllocator<'_>,
) -> Result<usize, RunElfError> {
    let mapping = mapping.ok_or(RunElfError::NoPhysicalMapping)?;

    if sched::active_count() >= sched::MAX_USER_PROCS {
        return Err(RunElfError::ProcessCapacityReached);
    }

    let pid = {
        let mut manager = PROCESS_MANAGER.lock();
        match manager.create_process(mapping, allocator) {
            Ok(pid) => pid,
            Err(crate::process::ProcessManagerError::CapacityReached) => {
                return Err(RunElfError::ProcessCapacityReached);
            }
            Err(_) => {
                return Err(RunElfError::ProcessMemoryFailed);
            }
        }
    };

    let pml4 = {
        let mut manager = PROCESS_MANAGER.lock();
        let pml4 = manager
            .get_process(pid)
            .map(|p| p.address_space().pml4_physical_address())
            .ok_or(RunElfError::ProcessMemoryFailed)?;
        manager.set_process_state(pid, ProcessState::Ready);
        pml4
    };

    let nxe_enabled = syscall::nxe_enabled();
    let image = match elf::load_elf_into_address_space_with_args(
        path,
        args,
        pml4,
        mapping,
        allocator,
        nxe_enabled,
    ) {
        Ok(img) => img,
        Err(err) => {
            let mut manager = PROCESS_MANAGER.lock();
            unsafe {
                let _ = manager.terminate_process(pid, mapping, allocator);
            }
            let _ = manager.reap_process(pid);
            return Err(RunElfError::Elf(err));
        }
    };

    if let Err(mut returned_image) = sched::register_process(pid, path, pml4, image) {
        elf::cleanup_loaded_image(&mut returned_image, pml4, mapping, allocator);
        let mut manager = PROCESS_MANAGER.lock();
        unsafe {
            let _ = manager.terminate_process(pid, mapping, allocator);
        }
        let _ = manager.reap_process(pid);
        return Err(RunElfError::ProcessCapacityReached);
    }

    Ok(pid)
}

/// Spawns an ELF64 executable and synchronously waits for completion using the preemptive scheduler
/// (used in text-console CLI `exec <path.elf>`).
pub fn load_and_run_elf(
    console: &mut Console<'_>,
    path: &str,
    mapping: Option<&PhysicalMemoryMapping>,
    allocator: &mut PhysicalFrameAllocator<'_>,
) -> Result<ExecutionReport, RunElfError> {
    load_and_run_elf_with_args(console, path, &[path], mapping, allocator)
}

/// Spawns an ELF64 executable with explicit `argv` arguments and waits for completion.
pub fn load_and_run_elf_with_args(
    console: &mut Console<'_>,
    path: &str,
    args: &[&str],
    mapping: Option<&PhysicalMemoryMapping>,
    allocator: &mut PhysicalFrameAllocator<'_>,
) -> Result<ExecutionReport, RunElfError> {
    let mapping_ref = mapping.ok_or(RunElfError::NoPhysicalMapping)?;
    let pid = spawn_elf_async_with_args(path, args, Some(mapping_ref), allocator)?;
    let finished = sched::wait_for_pid_sync(console, pid, mapping_ref, allocator);

    Ok(ExecutionReport {
        pid: finished.pid,
        entry_point: finished.entry_point,
        pages_mapped: finished.pages_mapped,
        bytes_loaded: finished.bytes_loaded,
        outcome: finished.outcome,
    })
}
