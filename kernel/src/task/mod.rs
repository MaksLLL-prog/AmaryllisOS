
pub mod context;
pub mod kthread;
pub mod preempt;
pub mod scheduler;
pub mod task;

pub use scheduler::Scheduler;

pub use task::{
    CpuContext,
    Task,
    TaskStack,
    TaskState,
    TaskPriority,
    STACK_SIZE,
};

pub static SCHEDULER: spin::Mutex<Scheduler> =
    spin::Mutex::new(Scheduler::new());

/// Voluntarily give control back to the kernel.
pub fn yield_now() {
    context::yield_to_kernel();
}

/// Put the currently running task to sleep for a number of PIT ticks.
pub fn sleep_ticks(ticks: u64) {
    if ticks == 0 {
        yield_now();
        return;
    }

    let current_ticks = crate::interrupts::timer::ticks();

    let should_yield = {
        let mut scheduler = SCHEDULER.lock();

        match scheduler.current_task() {
            Some(id) => {
                scheduler.sleep_task(id, ticks, current_ticks)
            }
            None => false,
        }
    };

    if should_yield {
        context::yield_to_kernel();
    }
}

/// Return the ID of the currently selected task.
pub fn current_task_id() -> Option<usize> {
    SCHEDULER.lock().current_task()
}
