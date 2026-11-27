use core::ops::{Deref, DerefMut};
use spin::{Mutex, MutexGuard, RwLock, RwLockReadGuard, RwLockWriteGuard};
use x86_64::instructions::interrupts;

use crate::task::preempt::{PreemptMutex, PreemptMutexGuard};

/// FreeBSD-compatible sleepable mutex (`struct mtx` / `MTX_DEF`) backed by `PreemptMutex`.
pub struct Mtx<T> {
    name: &'static str,
    inner: PreemptMutex<T>,
}

impl<T> Mtx<T> {
    pub const fn new(name: &'static str, val: T) -> Self {
        Self {
            name,
            inner: PreemptMutex::new(val),
        }
    }

    pub const fn name(&self) -> &'static str {
        self.name
    }

    #[inline]
    pub fn lock(&self) -> PreemptMutexGuard<'_, T> {
        self.inner.lock()
    }

    #[inline]
    pub fn try_lock(&self) -> Option<PreemptMutexGuard<'_, T>> {
        self.inner.try_lock()
    }
}

/// FreeBSD-compatible spin mutex (`struct mtx` / `MTX_SPIN`) that disables local interrupts
/// while held, safe for top-half IRQ handlers and scheduler-critical data.
pub struct MtxSpin<T> {
    name: &'static str,
    inner: Mutex<T>,
}

pub struct MtxSpinGuard<'a, T> {
    guard: Option<MutexGuard<'a, T>>,
    irq_was_enabled: bool,
}

impl<T> Deref for MtxSpinGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        self.guard.as_ref().expect("MtxSpinGuard missing inner guard")
    }
}

impl<T> DerefMut for MtxSpinGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.guard.as_mut().expect("MtxSpinGuard missing inner guard")
    }
}

impl<T> Drop for MtxSpinGuard<'_, T> {
    fn drop(&mut self) {
        let _ = self.guard.take();
        if self.irq_was_enabled {
            interrupts::enable();
        }
    }
}

impl<T> MtxSpin<T> {
    pub const fn new(name: &'static str, val: T) -> Self {
        Self {
            name,
            inner: Mutex::new(val),
        }
    }

    pub const fn name(&self) -> &'static str {
        self.name
    }

    #[inline]
    pub fn lock_spin(&self) -> MtxSpinGuard<'_, T> {
        let irq_was_enabled = interrupts::are_enabled();
        if irq_was_enabled {
            interrupts::disable();
        }
        MtxSpinGuard {
            guard: Some(self.inner.lock()),
            irq_was_enabled,
        }
    }
}

/// FreeBSD-compatible shared/exclusive lock (`struct sx`).
pub struct SxLock<T> {
    name: &'static str,
    inner: RwLock<T>,
}

impl<T> SxLock<T> {
    pub const fn new(name: &'static str, val: T) -> Self {
        Self {
            name,
            inner: RwLock::new(val),
        }
    }

    pub const fn name(&self) -> &'static str {
        self.name
    }

    #[inline]
    pub fn sx_slock(&self) -> RwLockReadGuard<'_, T> {
        self.inner.read()
    }

    #[inline]
    pub fn sx_xlock(&self) -> RwLockWriteGuard<'_, T> {
        self.inner.write()
    }
}
