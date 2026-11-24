use core::arch::asm;
use core::sync::atomic::{AtomicU64, Ordering};

use super::pic;

static TICKS: AtomicU64 = AtomicU64::new(0);
static UPTIME_MS: AtomicU64 = AtomicU64::new(0);

const PIT_FREQUENCY: u32 = 1_193_182;
/// 1000 Hz (1 ms per IRQ0) for accurate 60 FPS frame pacing (~16 ms) and sleep_ms,
/// while keeping `ticks()` at 100 Hz (10 ms) for full compatibility across subsystems.
const TIMER_HZ: u32 = 1000;
const TICKS_DIVISOR: u64 = 10;

pub fn initialize() {
    let divisor = (PIT_FREQUENCY / TIMER_HZ) as u16;

    unsafe {
        // Channel 0, low/high byte, square wave mode.
        asm!(
            "out dx, al",
            in("dx") 0x43u16,
            in("al") 0x36u8,
            options(nomem, nostack, preserves_flags)
        );

        // Send divisor low byte.
        asm!(
            "out dx, al",
            in("dx") 0x40u16,
            in("al") (divisor & 0xFF) as u8,
            options(nomem, nostack, preserves_flags)
        );

        // Send divisor high byte.
        asm!(
            "out dx, al",
            in("dx") 0x40u16,
            in("al") (divisor >> 8) as u8,
            options(nomem, nostack, preserves_flags)
        );
    }
}

/// 100 Hz monotonic kernel tick counter (10 ms resolution).
#[inline]
pub fn ticks() -> u64 {
    TICKS.load(Ordering::Relaxed)
}

/// 1000 Hz monotonic millisecond uptime counter (1 ms resolution).
#[inline]
pub fn uptime_ms() -> u64 {
    UPTIME_MS.load(Ordering::Relaxed)
}

fn interrupts_enabled() -> bool {
    let rflags: u64;
    unsafe {
        asm!(
            "pushfq",
            "pop {}",
            out(reg) rflags,
            options(nomem, preserves_flags)
        );
    }
    (rflags & (1 << 9)) != 0
}

pub fn sleep_ticks(wait_ticks: u64) {
    if wait_ticks == 0 {
        return;
    }

    let start = ticks();
    let target = start.saturating_add(wait_ticks);

    if interrupts_enabled() {
        while ticks() < target {
            unsafe {
                asm!("hlt", options(nomem, nostack, preserves_flags));
            }
        }
    } else {
        // Fallback busy loop if called with interrupts disabled.
        let iterations = wait_ticks.saturating_mul(150_000);
        for _ in 0..iterations {
            core::hint::spin_loop();
        }
    }
}

pub fn sleep_ms(ms: u64) {
    if ms == 0 {
        return;
    }
    if crate::task::kthread::is_in_kthread() {
        crate::task::kthread::sleep_current_ms(ms);
        return;
    }
    if interrupts_enabled() {
        let target = uptime_ms().saturating_add(ms);
        while uptime_ms() < target {
            unsafe {
                asm!("hlt", options(nomem, nostack, preserves_flags));
            }
        }
    } else {
        let wait_ticks = ms.div_ceil(10).max(1);
        sleep_ticks(wait_ticks);
    }
}

#[inline]
pub fn on_irq0_tick() -> u64 {
    let ms = UPTIME_MS.fetch_add(1, Ordering::Relaxed) + 1;
    if ms % TICKS_DIVISOR == 0 {
        TICKS.fetch_add(1, Ordering::Relaxed);
    }
    pic::send_eoi(0);
    ms
}

pub extern "x86-interrupt" fn timer_handler(
    _stack_frame: x86_64::structures::idt::InterruptStackFrame,
) {
    let _ = on_irq0_tick();
}
