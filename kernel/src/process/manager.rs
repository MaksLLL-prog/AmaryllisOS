
use crate::memory::address_space::{
    AddressSpace,
    AddressSpaceError,
};
use crate::memory::vmm::PhysicalMemoryMapping;
use crate::memory::PhysicalFrameAllocator;

use crate::task::{
    Scheduler,
    Task,
    TaskPriority,
    TaskState,
};

use super::process::{
    Process,
    ProcessState,
};

pub const MAX_PROCESSES: usize = 24;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessManagerError {
    CapacityReached,
    AddressSpaceCreationFailed,
    ProcessNotFound,
    AlreadyTerminated,
    NotTerminated,
    AddressSpaceError(AddressSpaceError),
    TaskAlreadyExists,
    TaskCreationFailed,
    TaskNotAttached,
    TaskNotFound,
    TaskIsRunning,
}

pub struct ProcessManager {
    processes: [Option<Process>; MAX_PROCESSES],
    next_pid: usize,
    process_count: usize,
}

impl ProcessManager {
    pub const fn new() -> Self {
        Self {
            processes: [const { None }; MAX_PROCESSES],
            next_pid: 1,
            process_count: 0,
        }
    }

    fn find_free_slot(&self) -> Option<usize> {
        self.processes
            .iter()
            .position(|process| process.is_none())
    }

    fn allocate_pid(&mut self) -> usize {
        loop {
            let candidate = self.next_pid.max(1);

            self.next_pid = candidate.wrapping_add(1);

            if self.next_pid == 0 {
                self.next_pid = 1;
            }

            let already_used = self.processes
                .iter()
                .flatten()
                .any(|process| process.pid() == candidate);

            if !already_used {
                return candidate;
            }
        }
    }

    // --------------------------------------------------------
    // CREATE PROCESS
    // --------------------------------------------------------

    pub fn create_process(
        &mut self,
        mapping: &PhysicalMemoryMapping,
        allocator: &mut PhysicalFrameAllocator<'_>,
    ) -> Result<usize, ProcessManagerError> {
        let slot = self.find_free_slot()
            .ok_or(ProcessManagerError::CapacityReached)?;

        let address_space = AddressSpace::new(
            mapping,
            allocator,
        )
        .ok_or(
            ProcessManagerError::AddressSpaceCreationFailed
        )?;

        let pid = self.allocate_pid();

        self.processes[slot] = Some(
            Process::new(pid, address_space)
        );

        self.process_count += 1;

        Ok(pid)
    }

    // --------------------------------------------------------
    // CREATE PROCESS WITH TASK
    // --------------------------------------------------------

    pub fn create_process_with_task(
        &mut self,
        mapping: &PhysicalMemoryMapping,
        allocator: &mut PhysicalFrameAllocator<'_>,
        scheduler: &mut Scheduler,
        task_id: usize,
        priority: TaskPriority,
        entry: extern "C" fn() -> !,
    ) -> Result<usize, ProcessManagerError> {
        let slot = self.find_free_slot()
            .ok_or(ProcessManagerError::CapacityReached)?;

        if scheduler.get_task_state(task_id).is_some() {
            return Err(ProcessManagerError::TaskAlreadyExists);
        }

        let mut address_space = AddressSpace::new(
            mapping,
            allocator,
        )
        .ok_or(
            ProcessManagerError::AddressSpaceCreationFailed
        )?;

        let task = Task::with_priority(task_id, priority);

        if !scheduler.add_task(task, entry) {
            unsafe {
                address_space.destroy(mapping, allocator)
            }
            .map_err(ProcessManagerError::AddressSpaceError)?;

            return Err(ProcessManagerError::TaskCreationFailed);
        }

        let pid = self.allocate_pid();

        let mut process = Process::new(
            pid,
            address_space,
        );

        process.set_task_id(task_id);
        process.set_state(ProcessState::Ready);

        self.processes[slot] = Some(process);
        self.process_count += 1;

        Ok(pid)
    }

    // --------------------------------------------------------
    // PROCESS INFORMATION
    // --------------------------------------------------------

    pub fn get_process(
        &self,
        pid: usize,
    ) -> Option<&Process> {
        self.processes
            .iter()
            .flatten()
            .find(|process| process.pid() == pid)
    }

    pub fn get_process_mut(
        &mut self,
        pid: usize,
    ) -> Option<&mut Process> {
        self.processes
            .iter_mut()
            .flatten()
            .find(|process| process.pid() == pid)
    }

    pub fn process_count(&self) -> usize {
        self.process_count
    }

    pub fn is_empty(&self) -> bool {
        self.process_count == 0
    }

    pub fn iter(
        &self,
    ) -> impl Iterator<Item = &Process> {
        self.processes.iter().filter_map(Option::as_ref)
    }

    // --------------------------------------------------------
    // PROCESS STATE
    // --------------------------------------------------------

    pub fn set_process_state(
        &mut self,
        pid: usize,
        state: ProcessState,
    ) -> bool {
        let process = match self.get_process_mut(pid) {
            Some(process) => process,
            None => return false,
        };

        if process.state() == ProcessState::Terminated
            || state == ProcessState::Terminated
        {
            return false;
        }

        process.set_state(state);

        true
    }

    // --------------------------------------------------------
    // SYNCHRONIZE PROCESS STATE WITH TASK
    // --------------------------------------------------------

    pub fn sync_process_state(
        &mut self,
        pid: usize,
        scheduler: &Scheduler,
    ) -> Result<(), ProcessManagerError> {
        let task_id = self.get_process(pid)
            .ok_or(ProcessManagerError::ProcessNotFound)?
            .task_id()
            .ok_or(ProcessManagerError::TaskNotAttached)?;

        let task_state = scheduler
            .get_task_state(task_id)
            .ok_or(ProcessManagerError::TaskNotFound)?;

        let process_state = match task_state {
            TaskState::Ready => ProcessState::Ready,
            TaskState::Running => ProcessState::Running,
            TaskState::Blocked | TaskState::Sleeping => {
                ProcessState::Blocked
            }
            TaskState::Finished => {
                return Err(ProcessManagerError::AlreadyTerminated);
            }
        };

        let process = self.get_process_mut(pid)
            .ok_or(ProcessManagerError::ProcessNotFound)?;

        if process.state() == ProcessState::Terminated {
            return Err(ProcessManagerError::AlreadyTerminated);
        }

        process.set_state(process_state);

        Ok(())
    }

    // --------------------------------------------------------
    // TERMINATE PROCESS WITHOUT TASK
    // --------------------------------------------------------

    /// Destroys an address space belonging to a process without
    /// an attached scheduler task.
    ///
    /// # Safety
    ///
    /// The caller must guarantee that no CPU is using this
    /// address space and that its private page tables are not
    /// being modified concurrently.
    pub unsafe fn terminate_process(
        &mut self,
        pid: usize,
        mapping: &PhysicalMemoryMapping,
        allocator: &mut PhysicalFrameAllocator<'_>,
    ) -> Result<(), ProcessManagerError> {
        let process = self.get_process_mut(pid)
            .ok_or(ProcessManagerError::ProcessNotFound)?;

        if process.state() == ProcessState::Terminated {
            return Err(ProcessManagerError::AlreadyTerminated);
        }

        if process.task_id().is_some() {
            return Err(ProcessManagerError::TaskNotAttached);
        }

        unsafe {
            process.address_space_mut().destroy(
                mapping,
                allocator,
            )
        }
        .map_err(ProcessManagerError::AddressSpaceError)?;

        process.set_state(ProcessState::Terminated);

        Ok(())
    }

    // --------------------------------------------------------
    // TERMINATE PROCESS WITH TASK
    // --------------------------------------------------------

    /// Terminates a process and marks its associated task Finished.
    ///
    /// # Safety
    ///
    /// The caller must guarantee that the process address space
    /// is not active on any CPU and cannot be activated while
    /// this operation is running.
    pub unsafe fn terminate_process_with_task(
        &mut self,
        pid: usize,
        mapping: &PhysicalMemoryMapping,
        allocator: &mut PhysicalFrameAllocator<'_>,
        scheduler: &mut Scheduler,
    ) -> Result<(), ProcessManagerError> {
        let task_id = {
            let process = self.get_process(pid)
                .ok_or(ProcessManagerError::ProcessNotFound)?;

            if process.state() == ProcessState::Terminated {
                return Err(ProcessManagerError::AlreadyTerminated);
            }

            process.task_id()
                .ok_or(ProcessManagerError::TaskNotAttached)?
        };

        let task_state = scheduler
            .get_task_state(task_id)
            .ok_or(ProcessManagerError::TaskNotFound)?;

        if task_state == TaskState::Running {
            return Err(ProcessManagerError::TaskIsRunning);
        }

        {
            let process = self.get_process_mut(pid)
                .ok_or(ProcessManagerError::ProcessNotFound)?;

            unsafe {
                process.address_space_mut().destroy(
                    mapping,
                    allocator,
                )
            }
            .map_err(ProcessManagerError::AddressSpaceError)?;

            process.set_state(ProcessState::Terminated);
        }

        if task_state != TaskState::Finished {
            scheduler.finish_task(task_id);
        }

        Ok(())
    }

    // --------------------------------------------------------
    // REAP TERMINATED PROCESS
    // --------------------------------------------------------

    pub fn reap_process(
        &mut self,
        pid: usize,
    ) -> Result<(), ProcessManagerError> {
        let index = self.processes
            .iter()
            .position(|entry| {
                entry.as_ref().map(|process| process.pid())
                    == Some(pid)
            })
            .ok_or(ProcessManagerError::ProcessNotFound)?;

        let process = self.processes[index]
            .as_ref()
            .ok_or(ProcessManagerError::ProcessNotFound)?;

        if process.state() != ProcessState::Terminated {
            return Err(ProcessManagerError::NotTerminated);
        }

        self.processes[index] = None;
        self.process_count = self.process_count.saturating_sub(1);

        Ok(())
    }
}
