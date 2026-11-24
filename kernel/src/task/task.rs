
use core::mem::MaybeUninit;
use core::ptr;

pub const STACK_SIZE: usize = 16 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskState {
    Ready,
    Running,
    Blocked,
    Sleeping,
    Finished,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum TaskPriority {
    Low,
    Normal,
    High,
}

#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct CpuContext {
    pub rsp: u64,
    pub rbp: u64,
    pub rbx: u64,
    pub r12: u64,
    pub r13: u64,
    pub r14: u64,
    pub r15: u64,
    pub rip: u64,
}

impl CpuContext {
    pub const fn empty() -> Self {
        Self {
            rsp: 0,
            rbp: 0,
            rbx: 0,
            r12: 0,
            r13: 0,
            r14: 0,
            r15: 0,
            rip: 0,
        }
    }
}

#[repr(C, align(16))]
pub struct TaskStack {
    data: [MaybeUninit<u8>; STACK_SIZE],
}

impl TaskStack {
    pub const fn new() -> Self {
        Self {
            data: [MaybeUninit::uninit(); STACK_SIZE],
        }
    }

    pub fn top(&self) -> u64 {
        self.data.as_ptr() as u64 + STACK_SIZE as u64
    }

    pub fn initialize(
        &mut self,
        entry: extern "C" fn() -> !,
    ) -> u64 {
        let top = self.top() as usize;
        let initial_rsp = top - 64;

        unsafe {
            let stack = initial_rsp as *mut u64;

            for index in 0..6 {
                ptr::write(stack.add(index), 0);
            }

            ptr::write(stack.add(6), entry as usize as u64);
        }

        initial_rsp as u64
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Task {
    pub id: usize,
    pub state: TaskState,
    pub priority: TaskPriority,
    pub context: CpuContext,
    pub wake_at: u64,

    // Task statistics
    pub run_count: u64,
    pub total_runtime: u64,
    pub last_scheduled: u64,
    pub waiting_time: u64,
}

impl Task {
    pub const fn new(id: usize) -> Self {
        Self::with_priority(id, TaskPriority::Normal)
    }

    pub const fn with_priority(
        id: usize,
        priority: TaskPriority,
    ) -> Self {
        Self {
            id,
            state: TaskState::Ready,
            priority,
            context: CpuContext::empty(),
            wake_at: 0,

            run_count: 0,
            total_runtime: 0,
            last_scheduled: 0,
            waiting_time: 0,
        }
    }
}
