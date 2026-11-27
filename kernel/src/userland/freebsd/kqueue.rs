use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use spin::Mutex;
use x86_64::instructions::interrupts::without_interrupts;

use crate::net::{tcp, udp};
use crate::userland::ipc;
use crate::userland::sched::{self, FdEntry, OpenSocket};

// FreeBSD kqueue filter types
pub const EVFILT_READ: i16 = -1;
pub const EVFILT_WRITE: i16 = -2;
pub const EVFILT_PROC: i16 = -5;
pub const EVFILT_TIMER: i16 = -7;
pub const EVFILT_USER: i16 = -11;

// FreeBSD kevent flags
pub const EV_ADD: u16 = 0x0001;
pub const EV_DELETE: u16 = 0x0002;
pub const EV_ENABLE: u16 = 0x0004;
pub const EV_DISABLE: u16 = 0x0008;
pub const EV_ONESHOT: u16 = 0x0010;
pub const EV_CLEAR: u16 = 0x0020;
pub const EV_ERROR: u16 = 0x4000;
pub const EV_EOF: u16 = 0x8000;

// Filter-specific fflags
pub const NOTE_EXIT: u32 = 0x8000_0000;
pub const NOTE_TRIGGER: u32 = 0x0100_0000;
pub const NOTE_MSECONDS: u32 = 0x0000_0002;

pub const MAX_KQUEUES: usize = 64;
pub const MAX_FILTERS_PER_KQ: usize = 64;

/// FreeBSD 12+ x86_64 `struct kevent` (32 bytes, `#[repr(C)]`).
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct KEvent {
    pub ident: u64,
    pub filter: i16,
    pub flags: u16,
    pub fflags: u32,
    pub data: i64,
    pub udata: u64,
}

#[derive(Clone, Copy)]
struct KNote {
    ident: u64,
    filter: i16,
    flags: u16,
    fflags: u32,
    data: i64,
    udata: u64,
    enabled: bool,
    timer_period_ms: u64,
    next_fire_ms: u64,
    proc_exited: bool,
    proc_exit_code: i64,
    user_triggered: bool,
}

struct KQueueInstance {
    notes: Vec<KNote>,
}

struct KQueueTable {
    next_id: usize,
    instances: BTreeMap<usize, KQueueInstance>,
}

impl KQueueTable {
    const fn new() -> Self {
        Self {
            next_id: 1,
            instances: BTreeMap::new(),
        }
    }
}

static KQUEUES: Mutex<KQueueTable> = Mutex::new(KQueueTable::new());

pub fn create_kqueue() -> Result<usize, i64> {
    without_interrupts(|| {
        let mut table = KQUEUES.lock();
        if table.instances.len() >= MAX_KQUEUES {
            return Err(-24); // EMFILE
        }
        let id = table.next_id;
        table.next_id = table.next_id.saturating_add(1).max(1);
        table.instances.insert(id, KQueueInstance { notes: Vec::new() });
        Ok(id)
    })
}

pub fn destroy_kqueue(kq_id: usize) {
    without_interrupts(|| {
        KQUEUES.lock().instances.remove(&kq_id);
    });
}

pub fn has_ready_note(kq_id: usize, now_ms: u64) -> bool {
    without_interrupts(|| {
        let table = KQUEUES.lock();
        let Some(kq) = table.instances.get(&kq_id) else {
            return true;
        };
        kq.notes.iter().any(|note| {
            if !note.enabled {
                return false;
            }
            match note.filter {
                EVFILT_TIMER => now_ms >= note.next_fire_ms,
                EVFILT_PROC => note.proc_exited,
                EVFILT_USER => note.user_triggered,
                _ => false,
            }
        })
    })
}

/// Called by scheduler whenever a Ring 3 child exits or is killed so active `EVFILT_PROC` notes fire immediately.
pub fn notify_process_exit(pid: usize, exit_code: i64) {
    without_interrupts(|| {
        let mut table = KQUEUES.lock();
        for kq in table.instances.values_mut() {
            for note in &mut kq.notes {
                if note.filter == EVFILT_PROC && note.ident == pid as u64 {
                    note.proc_exited = true;
                    note.proc_exit_code = exit_code;
                }
            }
        }
    });
}

pub fn apply_changes(kq_id: usize, changes: &[KEvent], now_ms: u64) -> Result<(), i64> {
    without_interrupts(|| {
        let mut table = KQUEUES.lock();
        let Some(kq) = table.instances.get_mut(&kq_id) else {
            return Err(-9); // EBADF
        };

        for ch in changes {
            let pos = kq
                .notes
                .iter()
                .position(|n| n.ident == ch.ident && n.filter == ch.filter);

            if (ch.flags & EV_DELETE) != 0 {
                if let Some(idx) = pos {
                    kq.notes.swap_remove(idx);
                }
                continue;
            }

            if (ch.flags & EV_ADD) != 0 {
                let period_ms = if ch.filter == EVFILT_TIMER {
                    let raw = ch.data.max(1) as u64;
                    if (ch.fflags & NOTE_MSECONDS) != 0 || ch.fflags == 0 {
                        raw
                    } else {
                        raw
                    }
                } else {
                    0
                };
                let note = KNote {
                    ident: ch.ident,
                    filter: ch.filter,
                    flags: ch.flags & !(EV_ADD | EV_ENABLE | EV_DISABLE),
                    fflags: ch.fflags,
                    data: ch.data,
                    udata: ch.udata,
                    enabled: (ch.flags & EV_DISABLE) == 0,
                    timer_period_ms: period_ms,
                    next_fire_ms: now_ms.saturating_add(period_ms),
                    proc_exited: false,
                    proc_exit_code: 0,
                    user_triggered: (ch.fflags & NOTE_TRIGGER) != 0,
                };
                if let Some(idx) = pos {
                    kq.notes[idx] = note;
                } else if kq.notes.len() < MAX_FILTERS_PER_KQ {
                    kq.notes.push(note);
                } else {
                    return Err(-12); // ENOMEM
                }
            } else if let Some(idx) = pos {
                let note = &mut kq.notes[idx];
                if (ch.flags & EV_ENABLE) != 0 {
                    note.enabled = true;
                }
                if (ch.flags & EV_DISABLE) != 0 {
                    note.enabled = false;
                }
                if ch.filter == EVFILT_USER && (ch.fflags & NOTE_TRIGGER) != 0 {
                    note.user_triggered = true;
                }
            }
        }
        Ok(())
    })
}

fn eval_fd_readiness(fds: &[Option<FdEntry>], fd: usize, is_write: bool) -> Option<(i64, u16)> {
    if fd == 1 || fd == 2 {
        return if is_write { Some((4096, 0)) } else { None };
    }
    if fd == 0 {
        return None;
    }
    if fd < sched::FD_OFFSET || fd >= sched::FD_OFFSET + sched::MAX_PROC_FDS {
        return Some((0, EV_ERROR));
    }
    let entry = fds.get(fd - sched::FD_OFFSET)?.as_ref()?;
    match entry {
        FdEntry::PipeRead { pipe_id } => {
            if is_write {
                return None;
            }
            let (readable, _writable, _readers, writers) = ipc::pipe_status(*pipe_id)?;
            if readable > 0 {
                Some((readable as i64, if writers == 0 { EV_EOF } else { 0 }))
            } else if writers == 0 {
                Some((0, EV_EOF))
            } else {
                None
            }
        }
        FdEntry::PipeWrite { pipe_id } => {
            if !is_write {
                return None;
            }
            let (_readable, writable, readers, _writers) = ipc::pipe_status(*pipe_id)?;
            if readers == 0 {
                Some((0, EV_EOF))
            } else if writable > 0 {
                Some((writable as i64, 0))
            } else {
                None
            }
        }
        FdEntry::File { .. } => Some((4096, 0)),
        FdEntry::Socket(OpenSocket::Tcp { .. }) => {
            // Tcp sockets allow polling write readiness immediately; reads happen non-blockingly via recv.
            if is_write {
                Some((2048, 0))
            } else {
                None
            }
        }
        FdEntry::Socket(OpenSocket::Udp { handle }) => {
            let _ = handle;
            if is_write {
                Some((1472, 0))
            } else {
                None
            }
        }
        _ => None,
    }
}

pub fn poll_events(
    kq_id: usize,
    owner_fds: &[Option<FdEntry>],
    max_events: usize,
    now_ms: u64,
    process_alive_fn: &dyn Fn(usize) -> bool,
) -> Result<Vec<KEvent>, i64> {
    if max_events == 0 {
        return Ok(Vec::new());
    }
    let _ = (tcp::poll_connect, udp::get_bound_port);

    without_interrupts(|| {
        let mut table = KQUEUES.lock();
        let Some(kq) = table.instances.get_mut(&kq_id) else {
            return Err(-9);
        };

        let mut ready = Vec::new();
        let mut remove_indices = Vec::new();

        for (idx, note) in kq.notes.iter_mut().enumerate() {
            if ready.len() >= max_events {
                break;
            }
            if !note.enabled {
                continue;
            }

            let fired: Option<KEvent> = match note.filter {
                EVFILT_READ => eval_fd_readiness(owner_fds, note.ident as usize, false).map(
                    |(data, extra_flags)| KEvent {
                        ident: note.ident,
                        filter: EVFILT_READ,
                        flags: note.flags | extra_flags,
                        fflags: 0,
                        data,
                        udata: note.udata,
                    },
                ),
                EVFILT_WRITE => eval_fd_readiness(owner_fds, note.ident as usize, true).map(
                    |(data, extra_flags)| KEvent {
                        ident: note.ident,
                        filter: EVFILT_WRITE,
                        flags: note.flags | extra_flags,
                        fflags: 0,
                        data,
                        udata: note.udata,
                    },
                ),
                EVFILT_TIMER => {
                    if now_ms >= note.next_fire_ms {
                        let expirations = if note.timer_period_ms > 0 {
                            1 + ((now_ms - note.next_fire_ms) / note.timer_period_ms)
                        } else {
                            1
                        };
                        note.next_fire_ms = now_ms.saturating_add(note.timer_period_ms.max(1));
                        Some(KEvent {
                            ident: note.ident,
                            filter: EVFILT_TIMER,
                            flags: note.flags,
                            fflags: note.fflags,
                            data: expirations as i64,
                            udata: note.udata,
                        })
                    } else {
                        None
                    }
                }
                EVFILT_PROC => {
                    let target_pid = note.ident as usize;
                    if note.proc_exited || !process_alive_fn(target_pid) {
                        note.proc_exited = false;
                        Some(KEvent {
                            ident: note.ident,
                            filter: EVFILT_PROC,
                            flags: note.flags | EV_EOF,
                            fflags: NOTE_EXIT,
                            data: note.proc_exit_code,
                            udata: note.udata,
                        })
                    } else {
                        None
                    }
                }
                EVFILT_USER => {
                    if note.user_triggered {
                        if (note.flags & EV_CLEAR) != 0 {
                            note.user_triggered = false;
                        }
                        Some(KEvent {
                            ident: note.ident,
                            filter: EVFILT_USER,
                            flags: note.flags,
                            fflags: note.fflags & !NOTE_TRIGGER,
                            data: note.data,
                            udata: note.udata,
                        })
                    } else {
                        None
                    }
                }
                _ => None,
            };

            if let Some(ev) = fired {
                ready.push(ev);
                if (note.flags & EV_ONESHOT) != 0 || note.filter == EVFILT_PROC {
                    remove_indices.push(idx);
                }
            }
        }

        for idx in remove_indices.into_iter().rev() {
            kq.notes.swap_remove(idx);
        }

        Ok(ready)
    })
}
