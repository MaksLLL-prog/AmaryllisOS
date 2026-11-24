use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;

use crate::interrupts::timer;
use crate::userland::syscall::UserFaultInfo;

use super::theme::*;
use super::widgets::{text_width, Canvas, Rect};

const MAX_ALERT_HISTORY: usize = 32;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum AlertSeverity {
    Info,
    Warning,
    Error,
    CriticalFault,
}

impl AlertSeverity {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Info => "INFO",
            Self::Warning => "WARNING",
            Self::Error => "SYSTEM ERROR",
            Self::CriticalFault => "RING 3 FAULT",
        }
    }

    pub const fn color(self) -> u32 {
        match self {
            Self::Info => ACCENT_CYAN,
            Self::Warning => ACCENT_WARN,
            Self::Error => ACCENT_ERR,
            Self::CriticalFault => 0xFFFF4D6D,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum AlertSubsystem {
    CpuRing3,
    Memory,
    StorageVfs,
    Network,
    Audio,
    ElfLoader,
    Desktop,
}

impl AlertSubsystem {
    pub const fn label(self) -> &'static str {
        match self {
            Self::CpuRing3 => "Userland / CPU",
            Self::Memory => "PMM / Virtual Mem",
            Self::StorageVfs => "FAT32 / VFS",
            Self::Network => "Intel e1000 Net",
            Self::Audio => "Intel AC'97 Audio",
            Self::ElfLoader => "ELF64 Loader",
            Self::Desktop => "Amaryllis WM",
        }
    }
}

#[derive(Clone)]
pub struct FaultTelemetry {
    pub pid: usize,
    pub vector: u8,
    pub error_code: u64,
    pub rip: u64,
    pub rsp: u64,
    pub cr2: u64,
    pub syscalls: u64,
    pub pmm_ok: bool,
}

#[derive(Clone)]
pub struct SystemAlert {
    pub id: u64,
    pub ticks: u64,
    pub severity: AlertSeverity,
    pub subsystem: AlertSubsystem,
    pub title: String,
    pub summary: String,
    pub hint: String,
    pub code_tag: String,
    pub telemetry: Option<FaultTelemetry>,
}

struct AlertManager {
    next_id: u64,
    history: Vec<SystemAlert>,
    active_modal: Option<SystemAlert>,
}

impl AlertManager {
    const fn new() -> Self {
        Self {
            next_id: 1,
            history: Vec::new(),
            active_modal: None,
        }
    }

    fn push(&mut self, mut alert: SystemAlert, show_modal: bool) {
        alert.id = self.next_id;
        self.next_id += 1;
        if self.history.len() >= MAX_ALERT_HISTORY {
            self.history.remove(0);
        }
        self.history.push(alert.clone());
        if show_modal {
            self.active_modal = Some(alert);
        }
    }
}

static ALERT_MANAGER: Mutex<AlertManager> = Mutex::new(AlertManager::new());

pub fn report_system_error(
    severity: AlertSeverity,
    subsystem: AlertSubsystem,
    code_tag: &str,
    title: &str,
    summary: &str,
    hint: &str,
    show_modal: bool,
) {
    let alert = SystemAlert {
        id: 0,
        ticks: timer::ticks(),
        severity,
        subsystem,
        title: String::from(title),
        summary: String::from(summary),
        hint: String::from(hint),
        code_tag: String::from(code_tag),
        telemetry: None,
    };
    ALERT_MANAGER.lock().push(alert, show_modal);
}

pub fn report_user_fault(
    pid: usize,
    program_path: &str,
    fault: UserFaultInfo,
    syscalls: u64,
    pmm_ok: bool,
    show_modal: bool,
) {
    let (code_tag, title, summary, hint) = match fault.vector {
        14 => {
            let present = (fault.error_code & 1) != 0;
            let write = (fault.error_code & 2) != 0;
            let nx = (fault.error_code & 16) != 0;

            let in_stack_guard = fault.cr2 >= crate::userland::elf::USER_STACK_GUARD_ADDR
                && fault.cr2 < crate::userland::elf::USER_STACK_MAX_BOTTOM;
            let cause = if in_stack_guard {
                "User Stack Guard Page hit (Stack Overflow cleanly prevented)"
            } else if nx {
                "Attempted instruction fetch from non-executable (NX) page"
            } else if fault.cr2 < 0x1000 {
                "NULL pointer dereference in Ring 3 address space"
            } else if present && write {
                "Write to read-only protected memory page"
            } else if !present {
                "Access to unmapped virtual page outside ELF segments"
            } else {
                "Page protection violation in Ring 3"
            };

            (
                "#PF (Vector 14)",
                format!("Process '{}' terminated by Page Fault", program_path),
                format!(
                    "{} at CR2=0x{:016x}. Kernel address space remained isolated.",
                    cause, fault.cr2
                ),
                String::from(
                    "Check pointers, buffer bounds, and ELF PT_LOAD permissions (PF_W / PF_X).",
                ),
            )
        }
        13 => (
            "#GP (Vector 13)",
            format!("Process '{}' violated General Protection", program_path),
            String::from(
                "Illegal privileged instruction, non-canonical pointer, or segment violation in Ring 3.",
            ),
            String::from(
                "Userland programs cannot execute CLI/HLT/IN/OUT; use SYSCALL instead.",
            ),
        ),
        6 => (
            "#UD (Vector 6)",
            format!("Process '{}' hit Invalid Opcode", program_path),
            format!(
                "CPU could not decode machine instruction at RIP=0x{:016x}.",
                fault.rip
            ),
            String::from("Verify ELF x86_64 target ABI and entry point alignment."),
        ),
        0 => (
            "#DE (Vector 0)",
            format!("Process '{}' divided by zero", program_path),
            format!("Arithmetic divide error triggered at RIP=0x{:016x}.", fault.rip),
            String::from("Guard divisor checks before DIV/IDIV instructions."),
        ),
        12 => (
            "#SS (Vector 12)",
            format!("Process '{}' stack fault", program_path),
            String::from("User stack bounds or alignment corrupted."),
            String::from("Check stack allocations inside user program."),
        ),
        11 => (
            "#NP (Vector 11)",
            format!("Process '{}' segment not present", program_path),
            String::from("Referenced invalid descriptor in user context."),
            String::from("Reload binary or verify GDT user selectors."),
        ),
        _ => (
            "CPU TRAP",
            format!("Process '{}' raised CPU vector {}", program_path, fault.vector),
            format!("Exception stopped process at RIP=0x{:016x}.", fault.rip),
            String::from("Inspect process telemetry in Activity Monitor."),
        ),
    };

    let alert = SystemAlert {
        id: 0,
        ticks: timer::ticks(),
        severity: AlertSeverity::CriticalFault,
        subsystem: AlertSubsystem::CpuRing3,
        title,
        summary,
        hint,
        code_tag: String::from(code_tag),
        telemetry: Some(FaultTelemetry {
            pid,
            vector: fault.vector,
            error_code: fault.error_code,
            rip: fault.rip,
            rsp: fault.rsp,
            cr2: fault.cr2,
            syscalls,
            pmm_ok,
        }),
    };
    ALERT_MANAGER.lock().push(alert, show_modal);
}

pub fn has_active_modal() -> bool {
    ALERT_MANAGER.lock().active_modal.is_some()
}

pub fn dismiss_modal() {
    ALERT_MANAGER.lock().active_modal = None;
}

pub fn open_latest_alert() {
    let mut mgr = ALERT_MANAGER.lock();
    if let Some(last) = mgr.history.last().cloned() {
        mgr.active_modal = Some(last);
    }
}

pub fn alert_count() -> usize {
    ALERT_MANAGER.lock().history.len()
}

pub fn unread_error_count() -> usize {
    ALERT_MANAGER
        .lock()
        .history
        .iter()
        .filter(|a| matches!(a.severity, AlertSeverity::Error | AlertSeverity::CriticalFault))
        .count()
}

pub fn snapshot_history() -> Vec<SystemAlert> {
    ALERT_MANAGER.lock().history.clone()
}

pub fn modal_rect(screen_w: usize, screen_h: usize) -> Rect {
    let w = 636.min(screen_w as i32 - 40);
    let h = 388.min(screen_h as i32 - 64);
    let x = ((screen_w as i32) - w) / 2;
    let y = ((screen_h as i32) - h) / 2 - 10;
    Rect::new(x, y, w, h)
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ModalAction {
    None,
    Dismiss,
    OpenMonitor,
}

pub fn draw_modal_alert(canvas: &mut Canvas<'_>) {
    let alert_opt = ALERT_MANAGER.lock().active_modal.clone();
    let Some(alert) = alert_opt else {
        return;
    };
    let pal = super::theme::palette();

    canvas.fill_rect_alpha(
        Rect::new(0, 0, canvas.width as i32, canvas.height as i32),
        if pal.is_light { 0xFF141C28 } else { 0xFF040810 },
        if pal.is_light { 96 } else { 145 },
    );

    let m = modal_rect(canvas.width, canvas.height);
    canvas.draw_soft_shadow(m, 14, 5, 90);
    canvas.draw_glass_panel(m, 14, pal.modal_bg, pal.modal_alpha, alert.severity.color());

    let sev_color = alert.severity.color();
    canvas.fill_rounded_rect_alpha(Rect::new(m.x + 16, m.y + 14, 126, 26), 6, sev_color, 42);
    canvas.stroke_rounded_rect(Rect::new(m.x + 16, m.y + 14, 126, 26), 6, sev_color, 180);
    canvas.draw_text_bold(m.x + 24, m.y + 19, alert.severity.label(), sev_color);

    canvas.draw_text(
        m.x + 154,
        m.y + 19,
        alert.subsystem.label(),
        pal.text_secondary,
        1,
    );

    let tag_w = text_width(&alert.code_tag, 1);
    canvas.draw_text_bold(
        m.right() - 24 - tag_w,
        m.y + 19,
        &alert.code_tag,
        ACCENT_WARN,
    );

    canvas.draw_text_scaled_clipped(
        m.x + 20,
        m.y + 48,
        &alert.title,
        pal.text_primary,
        320,
        true,
        Rect::new(m.x + 20, m.y + 48, m.w - 40, 28),
    );

    draw_wrapped_text(
        canvas,
        m.x + 20,
        m.y + 82,
        m.w - 40,
        3,
        &alert.summary,
        pal.text_secondary,
        1,
        18,
    );

    let hint_y = if let Some(ref tel) = alert.telemetry {
        let tbox = Rect::new(m.x + 20, m.y + 142, m.w - 40, 88);
        canvas.fill_rounded_rect(tbox, 8, if pal.is_light { 0xFF151C28 } else { 0xFF0B101A });
        canvas.stroke_rounded_rect(tbox, 8, pal.surface_border, 200);

        let line1 = format!(
            "PID: {}   ERR: 0x{:04x}   SYSCALLS: {}   PMM: {}",
            tel.pid,
            tel.error_code,
            tel.syscalls,
            if tel.pmm_ok { "ISOLATED / OK" } else { "CHECK!" }
        );
        let line2 = format!("RIP: 0x{:016x}   RSP: 0x{:016x}", tel.rip, tel.rsp);
        let line3 = if tel.vector == 14 {
            format!("CR2 (Fault Addr): 0x{:016x}   Vector: #PF (14)", tel.cr2)
        } else {
            format!("CPU Vector: {}   Ring 0 Kernel State: SAFE", tel.vector)
        };

        canvas.draw_text_mono_clipped(tbox.x + 12, tbox.y + 10, &line1, ACCENT_MINT, tbox);
        canvas.draw_text_mono_clipped(tbox.x + 12, tbox.y + 34, &line2, 0xFFEAEFF7, tbox);
        canvas.draw_text_mono_clipped(tbox.x + 12, tbox.y + 58, &line3, ACCENT_ERR, tbox);
        m.y + 240
    } else {
        m.y + 154
    };

    let hbox = Rect::new(m.x + 20, hint_y, m.w - 40, 72);
    canvas.fill_rounded_rect_alpha(hbox, 8, pal.accent_green, 28);
    canvas.stroke_rounded_rect(hbox, 8, pal.accent_green, 120);
    canvas.draw_text(hbox.x + 12, hbox.y + 8, "Diagnostics Hint:", pal.accent_green, 1);
    draw_wrapped_text(
        canvas,
        hbox.x + 12,
        hbox.y + 28,
        hbox.w - 24,
        2,
        &alert.hint,
        pal.text_primary,
        1,
        18,
    );

    let btn_dismiss = Rect::new(m.right() - 126, m.bottom() - 46, 106, 32);
    let btn_monitor = Rect::new(m.right() - 274, m.bottom() - 46, 136, 32);

    canvas.fill_rounded_rect(btn_monitor, 7, pal.surface_card);
    canvas.stroke_rounded_rect(btn_monitor, 7, pal.surface_border, 200);
    canvas.draw_text(btn_monitor.x + 18, btn_monitor.y + 8, "Open Monitor", pal.text_primary, 1);

    canvas.fill_rounded_rect(btn_dismiss, 7, sev_color);
    canvas.draw_text(btn_dismiss.x + 28, btn_dismiss.y + 8, "Dismiss", 0xFFFFFFFF, 1);
}

pub fn handle_modal_click(screen_w: usize, screen_h: usize, mx: i32, my: i32) -> ModalAction {
    if !has_active_modal() {
        return ModalAction::None;
    }
    let m = modal_rect(screen_w, screen_h);
    let btn_dismiss = Rect::new(m.right() - 126, m.bottom() - 46, 106, 32);
    let btn_monitor = Rect::new(m.right() - 274, m.bottom() - 46, 136, 32);

    if btn_dismiss.contains(mx, my) {
        dismiss_modal();
        return ModalAction::Dismiss;
    }
    if btn_monitor.contains(mx, my) {
        dismiss_modal();
        return ModalAction::OpenMonitor;
    }
    if !m.contains(mx, my) {
        dismiss_modal();
        return ModalAction::Dismiss;
    }
    ModalAction::None
}

pub fn draw_wrapped_text(
    canvas: &mut Canvas<'_>,
    x: i32,
    y: i32,
    max_w: i32,
    max_lines: usize,
    text: &str,
    color: u32,
    scale: i32,
    line_step: i32,
) {
    let mut cur_y = y;
    let mut lines_drawn = 0usize;

    let mut line = String::new();
    for word in text.split_whitespace() {
        let candidate = if line.is_empty() {
            String::from(word)
        } else {
            format!("{} {}", line, word)
        };
        if text_width(&candidate, scale) <= max_w {
            line = candidate;
        } else if line.is_empty() {
            canvas.draw_text_clipped(x, cur_y, word, color, scale, Rect::new(x, cur_y, max_w, 24));
            cur_y += line_step;
            lines_drawn += 1;
            if lines_drawn >= max_lines {
                return;
            }
        } else {
            canvas.draw_text(x, cur_y, &line, color, scale);
            cur_y += line_step;
            lines_drawn += 1;
            if lines_drawn >= max_lines {
                return;
            }
            line = String::from(word);
        }
    }
    if !line.is_empty() && lines_drawn < max_lines {
        canvas.draw_text(x, cur_y, &line, color, scale);
    }
}
