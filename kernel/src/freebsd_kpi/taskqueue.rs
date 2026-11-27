use alloc::collections::VecDeque;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::interrupts::timer;
use crate::task::kthread;
use crate::task::preempt::PreemptMutex;

pub type TaskFn = fn(usize);

pub struct Task {
    pub func: TaskFn,
    pub context: usize,
    pub pending: AtomicBool,
}

impl Task {
    pub const fn new(func: TaskFn, context: usize) -> Self {
        Self {
            func,
            context,
            pending: AtomicBool::new(false),
        }
    }
}

pub struct TaskQueue {
    name: &'static str,
    queue: PreemptMutex<VecDeque<(TaskFn, usize)>>,
    executed: AtomicU64,
}

impl TaskQueue {
    pub const fn new(name: &'static str) -> Self {
        Self {
            name,
            queue: PreemptMutex::new(VecDeque::new()),
            executed: AtomicU64::new(0),
        }
    }

    pub const fn name(&self) -> &'static str {
        self.name
    }

    pub fn enqueue(&self, task: &Task) {
        if !task.pending.swap(true, Ordering::AcqRel) {
            self.queue.lock().push_back((task.func, task.context));
        }
    }

    pub fn enqueue_fn(&self, func: TaskFn, context: usize) {
        self.queue.lock().push_back((func, context));
    }

    pub fn drain_one(&self) -> bool {
        let item = self.queue.lock().pop_front();
        if let Some((func, ctx)) = item {
            func(ctx);
            self.executed.fetch_add(1, Ordering::Relaxed);
            true
        } else {
            false
        }
    }

    pub fn drain_all(&self) -> usize {
        let mut count = 0usize;
        while self.drain_one() {
            count += 1;
        }
        count
    }

    pub fn executed_count(&self) -> u64 {
        self.executed.load(Ordering::Relaxed)
    }

    pub fn pending_len(&self) -> usize {
        self.queue.lock().len()
    }
}

/// Global Fast / Thread TaskQueue inspired by FreeBSD `taskqueue_thread`.
pub static KPI_TASKQUEUE: TaskQueue = TaskQueue::new("taskqueue_thread");

static TASKQUEUE_WORKER_STARTED: AtomicBool = AtomicBool::new(false);

fn kpi_taskqueue_worker() {
    loop {
        if KPI_TASKQUEUE.drain_all() == 0 {
            kthread::sleep_current_ms(50);
        }
    }
}

pub fn ensure_taskqueue_worker() {
    if !TASKQUEUE_WORKER_STARTED.swap(true, Ordering::AcqRel) {
        let _ = kthread::spawn_kthread("fbsd_taskq", kpi_taskqueue_worker);
    }
}

/// FreeBSD-compatible timer callout (`struct callout`).
pub struct Callout {
    deadline_ms: Option<u64>,
    callback: fn(usize),
    arg: usize,
    fires: u64,
}

impl Callout {
    pub const fn new(callback: fn(usize), arg: usize) -> Self {
        Self {
            deadline_ms: None,
            callback,
            arg,
            fires: 0,
        }
    }

    pub fn callout_reset(&mut self, timeout_ms: u64, callback: fn(usize), arg: usize) {
        self.callback = callback;
        self.arg = arg;
        self.deadline_ms = Some(timer::uptime_ms().saturating_add(timeout_ms));
    }

    pub fn callout_stop(&mut self) -> bool {
        self.deadline_ms.take().is_some()
    }

    pub fn callout_pending(&self) -> bool {
        self.deadline_ms.is_some()
    }

    pub fn callout_poll(&mut self) -> bool {
        if let Some(dl) = self.deadline_ms {
            if timer::uptime_ms() >= dl {
                self.deadline_ms = None;
                self.fires += 1;
                (self.callback)(self.arg);
                return true;
            }
        }
        false
    }

    pub fn fire_count(&self) -> u64 {
        self.fires
    }
}
