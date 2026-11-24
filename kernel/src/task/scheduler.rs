use super::task::{
    Task,
    TaskStack,
    TaskState,
    TaskPriority,
};

const MAX_TASKS: usize = 16;
const AGING_INTERVAL: u64 = 100;

pub struct Scheduler {
    tasks: [Option<Task>; MAX_TASKS],
    stacks: [TaskStack; MAX_TASKS],
    waiting_cycles: [u64; MAX_TASKS],
    waiting_since: [u64; MAX_TASKS],
    waiting_active: [bool; MAX_TASKS],
    task_count: usize,
    current_task: Option<usize>,
}

impl Scheduler {
    pub const fn new() -> Self {
        Self {
            tasks: [const { None }; MAX_TASKS],
            stacks: [const { TaskStack::new() }; MAX_TASKS],
            waiting_cycles: [0; MAX_TASKS],
            waiting_since: [0; MAX_TASKS],
            waiting_active: [false; MAX_TASKS],
            task_count: 0,
            current_task: None,
        }
    }

    pub fn add_task(
        &mut self,
        mut task: Task,
        entry: extern "C" fn() -> !,
    ) -> bool {
        if self.task_count >= MAX_TASKS {
            return false;
        }

        for existing in self.tasks.iter().flatten() {
            if existing.id == task.id {
                return false;
            }
        }

        let index = self.task_count;

        task.context.rsp = self.stacks[index].initialize(entry);

        self.tasks[index] = Some(task);
        self.waiting_cycles[index] = 0;
        self.waiting_since[index] = 0;
        self.waiting_active[index] = false;
        self.task_count += 1;

        true
    }

    pub fn task_count(&self) -> usize {
        self.task_count
    }

    pub fn current_task(&self) -> Option<usize> {
        self.current_task
    }

    pub fn task_rsp(&self, id: usize) -> Option<u64> {
        for task in self.tasks.iter().flatten() {
            if task.id == id {
                return Some(task.context.rsp);
            }
        }

        None
    }

    pub fn task_rsp_ptr(&mut self, id: usize) -> Option<*mut u64> {
        for task in self.tasks.iter_mut().flatten() {
            if task.id == id {
                return Some(core::ptr::addr_of_mut!(task.context.rsp));
            }
        }

        None
    }

    pub fn stack_top(&self, id: usize) -> Option<u64> {
        for index in 0..self.task_count {
            if let Some(task) = self.tasks[index] {
                if task.id == id {
                    return Some(self.stacks[index].top());
                }
            }
        }

        None
    }

    pub fn get_task_state(&self, id: usize) -> Option<TaskState> {
        for task in self.tasks.iter().flatten() {
            if task.id == id {
                return Some(task.state);
            }
        }

        None
    }

    pub fn get_task_priority(&self, id: usize) -> Option<TaskPriority> {
        for task in self.tasks.iter().flatten() {
            if task.id == id {
                return Some(task.priority);
            }
        }

        None
    }

    pub fn get_task_statistics(
        &self,
        id: usize,
    ) -> Option<(u64, u64, u64, u64)> {
        for task in self.tasks.iter().flatten() {
            if task.id == id {
                return Some((
                    task.run_count,
                    task.total_runtime,
                    task.last_scheduled,
                    task.waiting_time,
                ));
            }
        }

        None
    }

    pub fn record_task_start(
        &mut self,
        id: usize,
        current_ticks: u64,
    ) {
        for task in self.tasks.iter_mut().flatten() {
            if task.id == id {
                task.run_count = task.run_count.saturating_add(1);
                task.last_scheduled = current_ticks;
                return;
            }
        }
    }

    pub fn record_task_runtime(
        &mut self,
        id: usize,
        runtime: u64,
    ) {
        for task in self.tasks.iter_mut().flatten() {
            if task.id == id {
                task.total_runtime =
                    task.total_runtime.saturating_add(runtime);
                return;
            }
        }
    }

    pub fn get_effective_task_priority(
        &self,
        id: usize,
    ) -> Option<TaskPriority> {
        for (index, task) in self.tasks.iter().enumerate() {
            if let Some(task) = task {
                if task.id == id {
                    let priority = Self::effective_priority(
                        task.priority,
                        self.waiting_cycles[index],
                    );

                    return Some(match priority {
                        0 => TaskPriority::Low,
                        1 => TaskPriority::Normal,
                        _ => TaskPriority::High,
                    });
                }
            }
        }

        None
    }

    pub fn set_task_priority(
        &mut self,
        id: usize,
        priority: TaskPriority,
    ) -> bool {
        for task in self.tasks.iter_mut().flatten() {
            if task.id == id {
                if task.state == TaskState::Finished {
                    return false;
                }

                task.priority = priority;
                return true;
            }
        }

        false
    }

    pub fn set_task_state(
        &mut self,
        id: usize,
        state: TaskState,
    ) -> bool {
        for index in 0..self.task_count {
            if let Some(task) = self.tasks[index].as_mut() {
                if task.id == id {
                    task.state = state;

                    if state != TaskState::Sleeping {
                        task.wake_at = 0;
                    }

                    if state != TaskState::Ready {
                        self.waiting_active[index] = false;
                        self.waiting_since[index] = 0;
                    }

                    return true;
                }
            }
        }

        false
    }

    pub fn block_task(&mut self, id: usize) -> bool {
        for index in 0..self.task_count {
            if let Some(task) = self.tasks[index].as_mut() {
                if task.id == id {
                    if task.state == TaskState::Finished {
                        return false;
                    }

                    task.state = TaskState::Blocked;
                    task.wake_at = 0;
                    self.waiting_cycles[index] = 0;
                    self.waiting_active[index] = false;
                    self.waiting_since[index] = 0;

                    return true;
                }
            }
        }

        false
    }

    pub fn unblock_task(&mut self, id: usize) -> bool {
        for index in 0..self.task_count {
            if let Some(task) = self.tasks[index].as_mut() {
                if task.id == id {
                    if task.state != TaskState::Blocked {
                        return false;
                    }

                    task.state = TaskState::Ready;
                    task.wake_at = 0;
                    self.waiting_cycles[index] = 0;
                    self.waiting_active[index] = false;
                    self.waiting_since[index] = 0;

                    return true;
                }
            }
        }

        false
    }

    pub fn sleep_task(
        &mut self,
        id: usize,
        duration: u64,
        current_ticks: u64,
    ) -> bool {
        if duration == 0 {
            return false;
        }

        for index in 0..self.task_count {
            if let Some(task) = self.tasks[index].as_mut() {
                if task.id == id {
                    if task.state != TaskState::Ready
                        && task.state != TaskState::Running
                    {
                        return false;
                    }

                    task.state = TaskState::Sleeping;
                    task.wake_at =
                        current_ticks.saturating_add(duration);

                    self.waiting_cycles[index] = 0;
                    self.waiting_active[index] = false;
                    self.waiting_since[index] = 0;

                    return true;
                }
            }
        }

        false
    }

    pub fn wake_sleeping_tasks(
        &mut self,
        current_ticks: u64,
    ) -> usize {
        let mut awakened = 0;

        for index in 0..self.task_count {
            if let Some(task) = self.tasks[index].as_mut() {
                if task.state == TaskState::Sleeping
                    && current_ticks >= task.wake_at
                {
                    task.state = TaskState::Ready;
                    task.wake_at = 0;

                    self.waiting_cycles[index] = 0;
                    self.waiting_since[index] = current_ticks;
                    self.waiting_active[index] = true;

                    awakened += 1;
                }
            }
        }

        awakened
    }

    pub fn finish_task(&mut self, id: usize) -> bool {
        for index in 0..self.task_count {
            if let Some(task) = self.tasks[index].as_mut() {
                if task.id == id {
                    if task.state == TaskState::Finished {
                        return false;
                    }

                    task.state = TaskState::Finished;
                    task.wake_at = 0;

                    self.waiting_cycles[index] = 0;
                    self.waiting_active[index] = false;
                    self.waiting_since[index] = 0;

                    return true;
                }
            }
        }

        false
    }

    fn effective_priority(
        priority: TaskPriority,
        waiting_cycles: u64,
    ) -> u8 {
        let base = match priority {
            TaskPriority::Low => 0,
            TaskPriority::Normal => 1,
            TaskPriority::High => 2,
        };

        let boost = (waiting_cycles / AGING_INTERVAL).min(2) as u8;

        (base + boost).min(2)
    }

    pub fn next_task(
        &mut self,
        current_ticks: u64,
    ) -> Option<usize> {
        if self.task_count == 0 {
            self.current_task = None;
            return None;
        }

        // Update waiting time and priority aging.
        for index in 0..self.task_count {
            if let Some(task) = self.tasks[index].as_mut() {
                if task.state == TaskState::Ready {
                    if self.waiting_active[index] {
                        let elapsed = current_ticks.saturating_sub(
                            self.waiting_since[index],
                        );

                        task.waiting_time =
                            task.waiting_time.saturating_add(elapsed);
                    } else {
                        self.waiting_active[index] = true;
                    }

                    self.waiting_since[index] = current_ticks;

                    self.waiting_cycles[index] =
                        self.waiting_cycles[index].saturating_add(1);
                } else {
                    self.waiting_active[index] = false;
                    self.waiting_since[index] = 0;
                    self.waiting_cycles[index] = 0;
                }
            }
        }

        // Find the highest effective priority.
        let mut best_priority: Option<u8> = None;

        for index in 0..self.task_count {
            if let Some(task) = self.tasks[index] {
                if task.state != TaskState::Ready {
                    continue;
                }

                let priority = Self::effective_priority(
                    task.priority,
                    self.waiting_cycles[index],
                );

                if best_priority
                    .map(|best| priority > best)
                    .unwrap_or(true)
                {
                    best_priority = Some(priority);
                }
            }
        }

        let priority = match best_priority {
            Some(priority) => priority,
            None => {
                self.current_task = None;
                return None;
            }
        };

        // Start searching after the previously selected task.
        let start = match self.current_task {
            Some(id) => {
                self.tasks
                    .iter()
                    .position(|task| {
                        task.as_ref().map(|t| t.id) == Some(id)
                    })
                    .map(|index| index + 1)
                    .unwrap_or(0)
            }
            None => 0,
        };

        // Round-robin selection among equal priorities.
        for offset in 0..self.task_count {
            let index = (start + offset) % self.task_count;

            if let Some(task) = self.tasks[index] {
                if task.state == TaskState::Ready {
                    let effective = Self::effective_priority(
                        task.priority,
                        self.waiting_cycles[index],
                    );

                    if effective == priority {
                        let id = task.id;

                        self.current_task = Some(id);
                        self.waiting_cycles[index] = 0;

                        self.waiting_active[index] = false;
                        self.waiting_since[index] = 0;

                        return Some(id);
                    }
                }
            }
        }

        self.current_task = None;
        None
    }
}
