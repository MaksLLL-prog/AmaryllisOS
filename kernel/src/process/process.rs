use crate::memory::address_space::AddressSpace;

/// Maximum length of a process identifier is handled by PID.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessState {
    Created,
    Ready,
    Running,
    Blocked,
    Terminated,
}

/// Process Control Block.
#[derive(Debug)]
pub struct Process {
    pid: usize,
    state: ProcessState,
    address_space: AddressSpace,
    task_id: Option<usize>,
}

impl Process {
    pub fn new(
        pid: usize,
        address_space: AddressSpace,
    ) -> Self {
        Self {
            pid,
            state: ProcessState::Created,
            address_space,
            task_id: None,
        }
    }

    pub fn pid(&self) -> usize {
        self.pid
    }

    pub fn state(&self) -> ProcessState {
        self.state
    }

    pub fn task_id(&self) -> Option<usize> {
        self.task_id
    }

    pub fn address_space(&self) -> &AddressSpace {
        &self.address_space
    }

    pub fn set_task_id(&mut self, task_id: usize) {
        self.task_id = Some(task_id);
    }

    pub(crate) fn set_state(
        &mut self,
        state: ProcessState,
    ) {
        self.state = state;
    }

    pub(crate) fn address_space_mut(
        &mut self,
    ) -> &mut AddressSpace {
        &mut self.address_space
    }
}
