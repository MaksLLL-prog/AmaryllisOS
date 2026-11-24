use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use spin::{Mutex, MutexGuard};

use crate::arch::percpu::{self, MAX_SMP_CPUS};

static PREEMPT_COUNT: [AtomicUsize; MAX_SMP_CPUS] =
    [const { AtomicUsize::new(0) }; MAX_SMP_CPUS];
static NEEDS_RESCHED: [AtomicBool; MAX_SMP_CPUS] =
    [const { AtomicBool::new(false) }; MAX_SMP_CPUS];

#[inline]
fn cpu_slot() -> usize {
    percpu::current_cpu_id() % MAX_SMP_CPUS
}

#[inline]
pub fn preempt_count() -> usize {
    PREEMPT_COUNT[cpu_slot()].load(Ordering::Acquire)
}

#[inline]
pub fn is_preemptible() -> bool {
    PREEMPT_COUNT[cpu_slot()].load(Ordering::Acquire) == 0
}

#[inline]
pub fn preempt_disable() {
    PREEMPT_COUNT[cpu_slot()].fetch_add(1, Ordering::Acquire);
}

#[inline]
pub fn preempt_enable_no_resched() {
    let slot = cpu_slot();
    let prev = PREEMPT_COUNT[slot].fetch_sub(1, Ordering::Release);
    debug_assert!(prev > 0, "preempt_enable underflow");
}

#[inline]
pub fn preempt_enable() {
    let slot = cpu_slot();
    let prev = PREEMPT_COUNT[slot].fetch_sub(1, Ordering::Release);
    debug_assert!(prev > 0, "preempt_enable underflow");
    if prev == 1 && take_need_resched() && crate::task::kthread::is_in_kthread() {
        crate::task::kthread::yield_to_scheduler();
    }
}

#[inline]
pub fn set_need_resched() {
    NEEDS_RESCHED[cpu_slot()].store(true, Ordering::Release);
}

#[inline]
pub fn set_need_resched_on_cpu(cpu_id: usize) {
    if cpu_id < MAX_SMP_CPUS {
        NEEDS_RESCHED[cpu_id].store(true, Ordering::Release);
    }
}

#[inline]
pub fn clear_need_resched() {
    NEEDS_RESCHED[cpu_slot()].store(false, Ordering::Release);
}

#[inline]
pub fn take_need_resched() -> bool {
    NEEDS_RESCHED[cpu_slot()].swap(false, Ordering::AcqRel)
}

/// RAII critical-section guard that disables kthread preemption on the current CPU until dropped.
pub struct PreemptGuard {
    allow_resched_on_drop: bool,
}

impl PreemptGuard {
    #[inline]
    pub fn new() -> Self {
        preempt_disable();
        Self {
            allow_resched_on_drop: true,
        }
    }

    #[inline]
    pub fn no_resched() -> Self {
        preempt_disable();
        Self {
            allow_resched_on_drop: false,
        }
    }
}

impl Drop for PreemptGuard {
    #[inline]
    fn drop(&mut self) {
        if self.allow_resched_on_drop {
            preempt_enable();
        } else {
            preempt_enable_no_resched();
        }
    }
}

/// SMP-safe spinlock that disables local-CPU preemption while held so timer-IRQ preemption
/// can never switch away mid-lock and deadlock another thread on the same or remote CPU.
pub struct PreemptMutex<T> {
    inner: Mutex<T>,
}

pub struct PreemptMutexGuard<'a, T> {
    guard: Option<MutexGuard<'a, T>>,
}

impl<T> PreemptMutex<T> {
    pub const fn new(val: T) -> Self {
        Self {
            inner: Mutex::new(val),
        }
    }

    #[inline]
    pub fn lock(&self) -> PreemptMutexGuard<'_, T> {
        preempt_disable();
        let guard = self.inner.lock();
        PreemptMutexGuard { guard: Some(guard) }
    }

    #[inline]
    pub fn try_lock(&self) -> Option<PreemptMutexGuard<'_, T>> {
        preempt_disable();
        if let Some(guard) = self.inner.try_lock() {
            Some(PreemptMutexGuard { guard: Some(guard) })
        } else {
            preempt_enable_no_resched();
            None
        }
    }

    #[inline]
    pub fn try_lock_irq(&self) -> Option<MutexGuard<'_, T>> {
        self.inner.try_lock()
    }
}

impl<T> Deref for PreemptMutexGuard<'_, T> {
    type Target = T;

    #[inline]
    fn deref(&self) -> &Self::Target {
        self.guard.as_ref().expect("PreemptMutexGuard dropped")
    }
}

impl<T> DerefMut for PreemptMutexGuard<'_, T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.guard.as_mut().expect("PreemptMutexGuard dropped")
    }
}

impl<T> Drop for PreemptMutexGuard<'_, T> {
    #[inline]
    fn drop(&mut self) {
        self.guard.take();
        preempt_enable();
    }
}
