use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::{self, Write};
use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;
use x86_64::instructions::interrupts::without_interrupts;

use crate::interrupts::timer;

pub const KLOG_CAPACITY: usize = 128;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum KLogLevel {
    Info,
    Warn,
    Fault,
    Preempt,
    Mm,
}

impl KLogLevel {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Info => "INFO",
            Self::Warn => "WARN",
            Self::Fault => "FAULT",
            Self::Preempt => "PREEMPT",
            Self::Mm => "MM",
        }
    }
}

#[derive(Clone, Copy)]
struct KLogRecord {
    seq: u64,
    uptime_ms: u64,
    level: KLogLevel,
    tag: [u8; 18],
    tag_len: u8,
    msg: [u8; 112],
    msg_len: u8,
}

impl KLogRecord {
    const EMPTY: Self = Self {
        seq: 0,
        uptime_ms: 0,
        level: KLogLevel::Info,
        tag: [0; 18],
        tag_len: 0,
        msg: [0; 112],
        msg_len: 0,
    };
}

#[derive(Clone)]
pub struct KLogEntryView {
    pub seq: u64,
    pub uptime_ms: u64,
    pub level: KLogLevel,
    pub tag: String,
    pub msg: String,
}

#[derive(Clone, Copy, Default)]
pub struct KLogStats {
    pub total_events: u64,
    pub buffered: usize,
    pub dropped_irq_logs: u64,
    pub apic_timer_irqs: u64,
    pub kthread_preempts: u64,
    pub user_preempts: u64,
    pub pf_lazy_commits: u64,
    pub pf_guard_faults: u64,
    pub pf_user_kills: u64,
    pub copy_user_faults: u64,
}

struct KLogRing {
    records: [KLogRecord; KLOG_CAPACITY],
    head: usize,
    len: usize,
    next_seq: u64,
}

impl KLogRing {
    const fn new() -> Self {
        Self {
            records: [KLogRecord::EMPTY; KLOG_CAPACITY],
            head: 0,
            len: 0,
            next_seq: 1,
        }
    }

    fn push(&mut self, uptime_ms: u64, level: KLogLevel, tag: &[u8], msg: &[u8]) {
        let mut rec = KLogRecord::EMPTY;
        rec.seq = self.next_seq;
        self.next_seq = self.next_seq.saturating_add(1);
        rec.uptime_ms = uptime_ms;
        rec.level = level;

        let tlen = tag.len().min(rec.tag.len());
        rec.tag[..tlen].copy_from_slice(&tag[..tlen]);
        rec.tag_len = tlen as u8;

        let mlen = msg.len().min(rec.msg.len());
        rec.msg[..mlen].copy_from_slice(&msg[..mlen]);
        rec.msg_len = mlen as u8;

        let write_idx = (self.head + self.len) % KLOG_CAPACITY;
        self.records[write_idx] = rec;
        if self.len < KLOG_CAPACITY {
            self.len += 1;
        } else {
            self.head = (self.head + 1) % KLOG_CAPACITY;
        }
    }

    fn clear(&mut self) {
        self.head = 0;
        self.len = 0;
    }
}

static KLOG: Mutex<KLogRing> = Mutex::new(KLogRing::new());

static TOTAL_EVENTS: AtomicU64 = AtomicU64::new(0);
static DROPPED_IRQ_LOGS: AtomicU64 = AtomicU64::new(0);
static APIC_TIMER_IRQS: AtomicU64 = AtomicU64::new(0);
static KTHREAD_PREEMPTS: AtomicU64 = AtomicU64::new(0);
static USER_PREEMPTS: AtomicU64 = AtomicU64::new(0);
static PF_LAZY_COMMITS: AtomicU64 = AtomicU64::new(0);
static PF_GUARD_FAULTS: AtomicU64 = AtomicU64::new(0);
static PF_USER_KILLS: AtomicU64 = AtomicU64::new(0);
static COPY_USER_FAULTS: AtomicU64 = AtomicU64::new(0);

struct StackFmtBuf<const N: usize> {
    buf: [u8; N],
    len: usize,
}

impl<const N: usize> StackFmtBuf<N> {
    const fn new() -> Self {
        Self {
            buf: [0; N],
            len: 0,
        }
    }

    fn as_bytes(&self) -> &[u8] {
        &self.buf[..self.len]
    }
}

impl<const N: usize> Write for StackFmtBuf<N> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let bytes = s.as_bytes();
        let avail = N.saturating_sub(self.len);
        let take = bytes.len().min(avail);
        if take > 0 {
            self.buf[self.len..self.len + take].copy_from_slice(&bytes[..take]);
            self.len += take;
        }
        Ok(())
    }
}

#[inline]
pub fn record_apic_timer_irq() {
    APIC_TIMER_IRQS.fetch_add(1, Ordering::Relaxed);
}

#[inline]
pub fn record_kthread_preempt() {
    KTHREAD_PREEMPTS.fetch_add(1, Ordering::Relaxed);
}

#[inline]
pub fn record_user_preempt() {
    USER_PREEMPTS.fetch_add(1, Ordering::Relaxed);
}

#[inline]
pub fn record_pf_lazy_commit() {
    PF_LAZY_COMMITS.fetch_add(1, Ordering::Relaxed);
}

#[inline]
pub fn record_pf_guard_fault() {
    PF_GUARD_FAULTS.fetch_add(1, Ordering::Relaxed);
}

#[inline]
pub fn record_pf_user_kill() {
    PF_USER_KILLS.fetch_add(1, Ordering::Relaxed);
}

#[inline]
pub fn record_copy_user_fault() {
    COPY_USER_FAULTS.fetch_add(1, Ordering::Relaxed);
}

#[inline]
pub fn apic_timer_irq_count() -> u64 {
    APIC_TIMER_IRQS.load(Ordering::Relaxed)
}

pub fn log_bytes_irq_safe(level: KLogLevel, tag: &str, msg: &[u8]) {
    let now_ms = timer::uptime_ms();
    TOTAL_EVENTS.fetch_add(1, Ordering::Relaxed);
    without_interrupts(|| {
        if let Some(mut ring) = KLOG.try_lock() {
            ring.push(now_ms, level, tag.as_bytes(), msg);
        } else {
            DROPPED_IRQ_LOGS.fetch_add(1, Ordering::Relaxed);
        }
    });
}

pub fn log_str(level: KLogLevel, tag: &str, msg: &str) {
    log_bytes_irq_safe(level, tag, msg.as_bytes());
}

pub fn log_fmt(level: KLogLevel, tag: &str, args: fmt::Arguments<'_>) {
    let mut sb = StackFmtBuf::<112>::new();
    let _ = sb.write_fmt(args);
    log_bytes_irq_safe(level, tag, sb.as_bytes());
}

#[inline]
pub fn log_info(tag: &str, msg: &str) {
    log_str(KLogLevel::Info, tag, msg);
}

#[inline]
pub fn log_warn(tag: &str, msg: &str) {
    log_str(KLogLevel::Warn, tag, msg);
}

#[inline]
pub fn log_fault(tag: &str, msg: &str) {
    log_str(KLogLevel::Fault, tag, msg);
}

#[inline]
pub fn log_mm(tag: &str, msg: &str) {
    log_str(KLogLevel::Mm, tag, msg);
}

pub fn clear() {
    without_interrupts(|| {
        if let Some(mut ring) = KLOG.try_lock() {
            ring.clear();
        }
    });
}

pub fn stats() -> KLogStats {
    let buffered = without_interrupts(|| KLOG.try_lock().map(|r| r.len).unwrap_or(0));
    KLogStats {
        total_events: TOTAL_EVENTS.load(Ordering::Relaxed),
        buffered,
        dropped_irq_logs: DROPPED_IRQ_LOGS.load(Ordering::Relaxed),
        apic_timer_irqs: APIC_TIMER_IRQS.load(Ordering::Relaxed),
        kthread_preempts: KTHREAD_PREEMPTS.load(Ordering::Relaxed),
        user_preempts: USER_PREEMPTS.load(Ordering::Relaxed),
        pf_lazy_commits: PF_LAZY_COMMITS.load(Ordering::Relaxed),
        pf_guard_faults: PF_GUARD_FAULTS.load(Ordering::Relaxed),
        pf_user_kills: PF_USER_KILLS.load(Ordering::Relaxed),
        copy_user_faults: COPY_USER_FAULTS.load(Ordering::Relaxed),
    }
}

pub fn snapshot() -> Vec<KLogEntryView> {
    without_interrupts(|| {
        let ring = KLOG.lock();
        let mut out = Vec::with_capacity(ring.len);
        for i in 0..ring.len {
            let idx = (ring.head + i) % KLOG_CAPACITY;
            let rec = &ring.records[idx];
            let tlen = (rec.tag_len as usize).min(rec.tag.len());
            let mlen = (rec.msg_len as usize).min(rec.msg.len());
            let tag = core::str::from_utf8(&rec.tag[..tlen]).unwrap_or("kernel");
            let msg = core::str::from_utf8(&rec.msg[..mlen]).unwrap_or("");
            out.push(KLogEntryView {
                seq: rec.seq,
                uptime_ms: rec.uptime_ms,
                level: rec.level,
                tag: String::from(tag),
                msg: String::from(msg),
            });
        }
        out
    })
}
