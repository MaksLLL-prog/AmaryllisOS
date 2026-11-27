pub const MAX_SIGNAL: u32 = 31;

pub const SIGHUP: u32 = 1;
pub const SIGINT: u32 = 2;
pub const SIGQUIT: u32 = 3;
pub const SIGILL: u32 = 4;
pub const SIGTRAP: u32 = 5;
pub const SIGABRT: u32 = 6;
pub const SIGKILL: u32 = 9;
pub const SIGSEGV: u32 = 11;
pub const SIGPIPE: u32 = 13;
pub const SIGALRM: u32 = 14;
pub const SIGTERM: u32 = 15;
pub const SIGCHLD: u32 = 20;
pub const SIGUSR1: u32 = 30;
pub const SIGUSR2: u32 = 31;

pub const SIG_DFL: u64 = 0;
pub const SIG_IGN: u64 = 1;

pub const SIG_BLOCK: i32 = 1;
pub const SIG_UNBLOCK: i32 = 2;
pub const SIG_SETMASK: i32 = 3;

/// Read+Exec user trampoline page mapped lazily on caught signals so NX user stacks work cleanly.
pub const USER_SIGTRAMP_ADDR: u64 = 0x0000_007F_0000_0000;

/// Machine code placed at `USER_SIGTRAMP_ADDR`:
///   mov rdi, rsp        ; 48 89 E7 (ucontext frame sits at top of signal handler stack)
///   mov eax, 417        ; B8 A1 01 00 00 (SYS_FBSD_SIGRETURN)
///   syscall             ; 0F 05
pub const SIGTRAMP_CODE: [u8; 10] = [
    0x48, 0x89, 0xE7,
    0xB8, 0xA1, 0x01, 0x00, 0x00,
    0x0F, 0x05,
];

/// FreeBSD x86_64 `struct sigaction` (32 bytes):
/// handler (8B) + flags (4B) + pad (4B) + 128-bit sigset_t (16B).
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct FreeBsdSigAction {
    pub sa_handler: u64,
    pub sa_flags: u32,
    pub _pad: u32,
    pub sa_mask: [u32; 4],
}

/// FreeBSD x86_64 `sigset_t` (16 bytes).
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct FreeBsdSigSet {
    pub bits: [u32; 4],
}

impl FreeBsdSigSet {
    pub fn to_mask64(self) -> u64 {
        (self.bits[0] as u64) & 0xFFFF_FFFF
    }

    pub fn from_mask64(mask: u64) -> Self {
        Self {
            bits: [(mask & 0xFFFF_FFFF) as u32, 0, 0, 0],
        }
    }
}

#[derive(Clone, Copy)]
pub struct SignalHandlerConfig {
    pub handler: u64,
    pub flags: u32,
    pub mask: u64,
}

impl SignalHandlerConfig {
    pub const DEFAULT: Self = Self {
        handler: SIG_DFL,
        flags: 0,
        mask: 0,
    };
}

#[derive(Clone)]
pub struct SignalState {
    pub pending_mask: u64,
    pub blocked_mask: u64,
    pub actions: [SignalHandlerConfig; 32],
    pub sigtramp_mapped: bool,
}

impl SignalState {
    pub const fn new() -> Self {
        Self {
            pending_mask: 0,
            blocked_mask: 0,
            actions: [SignalHandlerConfig::DEFAULT; 32],
            sigtramp_mapped: false,
        }
    }

    pub fn clone_for_fork(&self) -> Self {
        Self {
            pending_mask: 0,
            blocked_mask: self.blocked_mask,
            actions: self.actions,
            sigtramp_mapped: self.sigtramp_mapped,
        }
    }

    #[inline]
    pub fn raise(&mut self, sig: u32) {
        if sig >= 1 && sig <= MAX_SIGNAL {
            self.pending_mask |= 1u64 << sig;
        }
    }

    #[inline]
    pub fn next_deliverable(&self) -> Option<u32> {
        let ready = self.pending_mask & !self.blocked_mask;
        if ready == 0 {
            return None;
        }
        let bit = ready.trailing_zeros();
        if bit <= MAX_SIGNAL && bit >= 1 {
            Some(bit)
        } else {
            None
        }
    }

    #[inline]
    pub fn clear_pending(&mut self, sig: u32) {
        if sig >= 1 && sig <= MAX_SIGNAL {
            self.pending_mask &= !(1u64 << sig);
        }
    }

    #[inline]
    pub fn is_interrupting_signal(&self, sig: u32) -> bool {
        if sig == 0 || sig > MAX_SIGNAL {
            return false;
        }
        if sig == SIGKILL {
            return true;
        }
        if (self.blocked_mask & (1u64 << sig)) != 0 {
            return false;
        }
        let cfg = self.actions[sig as usize];
        if cfg.handler == SIG_IGN {
            return false;
        }
        if cfg.handler == SIG_DFL {
            return default_signal_action(sig) != SignalDefault::Ignore;
        }
        true
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SignalDefault {
    Ignore,
    Terminate,
}

#[inline]
pub fn default_signal_action(sig: u32) -> SignalDefault {
    match sig {
        SIGCHLD => SignalDefault::Ignore,
        SIGHUP | SIGINT | SIGQUIT | SIGILL | SIGTRAP | SIGABRT | SIGKILL | SIGSEGV | SIGPIPE
        | SIGALRM | SIGTERM | SIGUSR1 | SIGUSR2 => SignalDefault::Terminate,
        _ => SignalDefault::Ignore,
    }
}

#[inline]
pub fn sig_bit(sig: u32) -> u64 {
    if sig >= 1 && sig <= MAX_SIGNAL {
        1u64 << sig
    } else {
        0
    }
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct SavedSignalFrame {
    pub r15: u64,
    pub r14: u64,
    pub r13: u64,
    pub r12: u64,
    pub rbp: u64,
    pub rbx: u64,
    pub r11: u64,
    pub r10: u64,
    pub r9: u64,
    pub r8: u64,
    pub rax: u64,
    pub rcx: u64,
    pub rdx: u64,
    pub rsi: u64,
    pub rdi: u64,
    pub rip: u64,
    pub rflags: u64,
    pub rsp: u64,
    pub old_blocked_mask: u64,
    pub signo: u32,
    pub reserved: u32,
}
