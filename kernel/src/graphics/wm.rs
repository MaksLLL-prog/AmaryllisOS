use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::arch::asm;
use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;

use crate::arch::rtc::{self, RtcDateTime};
use crate::console::{self, Console};
use crate::interrupts::timer;
use crate::memory::heap::GLOBAL_ALLOCATOR;
use crate::memory::vmm::PhysicalMemoryMapping;
use crate::memory::PhysicalFrameAllocator;
use crate::storage::vfs;
use crate::userland::{runner, sched};

use super::alerts::{self, AlertSeverity, AlertSubsystem, ModalAction};
use super::apps::audio_player::AudioPlayerApp;
use super::apps::editor::EditorApp;
use super::apps::files::FilesApp;
use super::apps::monitor::MonitorApp;
use super::apps::net_viewer::NetViewerApp;
use super::apps::settings::SettingsApp;
use super::apps::terminal::TerminalApp;
use super::apps::{BuiltinAppKind, DesktopAction};
use super::compositor::Compositor;
use super::icons::{dock_icon, LucideIcon, DOCK_SPRITE_SIZE};
use super::personalization::{
    self, cc_idx, ControlCenterModule, DesktopConfig, SystemAppearance, ThemePreference,
    UiFontFamily, WallpaperSource,
};
use super::renderer::Renderer;
use super::theme::{self, *};
use super::widgets::{ui_text_width, Canvas, Rect};

const MAX_WINDOWS: usize = 12;
const TARGET_FRAME_MS: u64 = 16; // ~60 FPS cap
const CAPSULE_PINNED_COUNT: usize = 7;

static ACTIVE_HHDM_OFFSET: AtomicU64 = AtomicU64::new(0);

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct UserShmWindowInfo {
    pub win_id: u64,
    pub shm_id: u64,
    pub vaddr: u64,
    pub size_bytes: u64,
    pub width: u32,
    pub height: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct UserGuiEvent {
    pub kind: u32, // 0=None, 1=CloseRequested, 2=MouseDown, 3=KeyChar
    pub p1: i32,
    pub p2: i32,
    pub p3: u32,
}

pub enum WindowContent {
    Terminal(TerminalApp),
    Files(FilesApp),
    Editor(EditorApp),
    AudioPlayer(AudioPlayerApp),
    NetViewer(NetViewerApp),
    Settings(SettingsApp),
    Monitor(MonitorApp),
    ExternalUser {
        owner_pid: usize,
        width: usize,
        height: usize,
        pixels: Vec<u32>,
        shm_id: Option<u64>,
        events: Vec<UserGuiEvent>,
    },
}

pub struct Window {
    pub id: u64,
    pub title: String,
    pub rect: Rect,
    pub restore_rect: Option<Rect>,
    pub minimized: bool,
    pub maximized: bool,
    pub external_user: bool,
    pub content: WindowContent,
    surface_cache: Vec<u32>,
    surface_w: usize,
    surface_h: usize,
    surface_dirty: bool,
    anim_from: Rect,
    anim_to: Rect,
    anim_start_ms: u64,
    anim_dur_ms: u64,
    open_anim_start_ms: u64,
    open_anim_dur_ms: u64,
}

struct ClosingGhost {
    rect: Rect,
    rad: i32,
    tbg: u32,
    bg: u32,
    start_ms: u64,
    dur_ms: u64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DragMode {
    None,
    Move { win_id: u64, off_x: i32, off_y: i32 },
    Resize { win_id: u64, start_mx: i32, start_my: i32, orig_w: i32, orig_h: i32 },
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SnapZone {
    None,
    LeftHalf,
    RightHalf,
    Maximize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TopMenu {
    None,
    Amaryllis,
    File,
    Edit,
    View,
    Window,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum WindowBtn {
    Minimize,
    Maximize,
    Close,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CapsuleHit {
    None,
    Launcher,
    Task(usize),
    Search,
    Status,
    Clock,
    Power,
}

struct CapsuleLayout {
    capsule: Rect,
    launcher_btn: Rect,
    div1_x: i32,
    task_rects: Vec<Rect>,
    div2_x: i32,
    search_btn: Rect,
    status_btn: Rect,
    clock_btn: Rect,
    power_btn: Rect,
}

struct LauncherEntry {
    label: String,
    subtitle: String,
    action: DesktopAction,
}

struct SpotlightState {
    open: bool,
    query: String,
    selected: usize,
    items: Vec<LauncherEntry>,
}

impl SpotlightState {
    fn new() -> Self {
        Self {
            open: false,
            query: String::new(),
            selected: 0,
            items: Vec::new(),
        }
    }

    fn rebuild_items(&mut self) {
        self.items.clear();
        let q = self.query.to_ascii_lowercase();

        let builtins: [(&str, &str, DesktopAction); 12] = [
            ("Files Explorer", "Browse RAMFS / & /disk FAT32, create folders & files", DesktopAction::OpenApp(BuiltinAppKind::Files)),
            ("Amaryllis Terminal", "Interactive kernel & userland shell", DesktopAction::OpenApp(BuiltinAppKind::Terminal)),
            ("Text Editor", "Create & edit files on / or /disk (Ctrl+S)", DesktopAction::OpenApp(BuiltinAppKind::Editor)),
            ("Audio Player", "Non-blocking AC'97 mixer, synth tracks & .wav player", DesktopAction::OpenApp(BuiltinAppKind::AudioPlayer)),
            ("Net / HTTP Viewer", "Live HTTP client, DHCP & ICMP ping over Intel e1000", DesktopAction::OpenApp(BuiltinAppKind::NetViewer)),
            ("System Settings", "Black/White theme, AC'97 sound & e1000 network", DesktopAction::OpenApp(BuiltinAppKind::Settings)),
            ("Activity Monitor", "Live RAM, FPS, drivers & fault alerts", DesktopAction::OpenApp(BuiltinAppKind::Monitor)),
            ("Switch Black / White Theme", "Toggle monochrome Black and White mode", DesktopAction::ToggleTheme),
            ("Play Audio Chime", "Test non-blocking Intel AC'97 audio mixer", DesktopAction::Beep(587, 90)),
            ("Create Demo ELFs (mkelf)", "Write /hello, /counter, /gui_demo, /net_demo, /fault ELFs", DesktopAction::RunShellCommand(String::from("mkelf all"))),
            ("Acquire IP (dhcp)", "Run DHCP discovery on Intel e1000", DesktopAction::RunShellCommand(String::from("dhcp"))),
            ("Exit to Text Console", "Return cleanly to VGA/FB shell", DesktopAction::CloseDesktop),
        ];

        for (label, sub, act) in builtins {
            if q.is_empty()
                || label.to_ascii_lowercase().contains(&q)
                || sub.to_ascii_lowercase().contains(&q)
            {
                self.items.push(LauncherEntry {
                    label: String::from(label),
                    subtitle: String::from(sub),
                    action: act,
                });
            }
        }

        for dir in ["/", "/disk"] {
            if let Ok(entries) = vfs::with_vfs(|s| s.list_dir(dir)) {
                for e in entries {
                    if e.name.ends_with(".elf") {
                        let path = if dir == "/" {
                            format!("/{}", e.name)
                        } else {
                            format!("{}/{}", dir, e.name)
                        };
                        if q.is_empty() || path.to_ascii_lowercase().contains(&q) {
                            self.items.push(LauncherEntry {
                                label: format!("Run {}", path),
                                subtitle: format!("Ring 3 ELF64 binary ({} B)", e.size),
                                action: DesktopAction::ExecElf(path),
                            });
                        }
                    }
                }
            }
        }

        if self.selected >= self.items.len() {
            self.selected = 0;
        }
    }
}

pub struct DesktopState {
    compositor: Compositor,
    windows: Vec<Window>,
    closing_ghosts: Vec<ClosingGhost>,
    next_win_id: u64,
    cursor_x: i32,
    cursor_y: i32,
    prev_left_btn: bool,
    prev_right_btn: bool,
    drag_mode: DragMode,
    snap_zone: SnapZone,
    open_menu: TopMenu,
    desktop_menu: Option<(i32, i32)>,
    control_center_open: bool,
    calendar_open: bool,
    power_menu_open: bool,
    dock_reveal_q8: i32,
    seen_gens: (u64, u64, u64),
    saved_volume: u8,
    music_track_idx: usize,
    last_audio_anim_ms: u64,
    spotlight: SpotlightState,
    ext_scancode: bool,
    modifiers: crate::input::KeyModifiers,
    pending_actions: Vec<DesktopAction>,
    should_exit: bool,
    toast: Option<(String, u64)>,
    hovered_capsule: CapsuleHit,
    hovered_top_menu: TopMenu,
    hovered_win_btn: Option<(u64, WindowBtn)>,
    last_clock_sec: u64,
    rtc_now: RtcDateTime,
    last_blink_on: bool,
    last_focused_id: Option<u64>,
    last_frame_ms: u64,
    fps_window_start_ms: u64,
    fps_frames_rendered: u32,
    current_fps: u32,
}

static ACTIVE_DESKTOP: Mutex<Option<Box<DesktopState>>> = Mutex::new(None);

#[inline]
fn ease_out_q8(elapsed: u64, dur: u64) -> i32 {
    if dur == 0 || elapsed >= dur {
        return 256;
    }
    let t = ((elapsed * 256) / dur) as i32;
    let inv = 256 - t;
    let inv3 = (inv * inv * inv) >> 16;
    (256 - inv3).clamp(0, 256)
}

#[inline]
fn lerp_rect(a: Rect, b: Rect, q8: i32) -> Rect {
    Rect::new(
        a.x + (((b.x - a.x) * q8) >> 8),
        a.y + (((b.y - a.y) * q8) >> 8),
        a.w + (((b.w - a.w) * q8) >> 8),
        a.h + (((b.h - a.h) * q8) >> 8),
    )
}

#[inline]
const fn rect_contains_rect(outer: Rect, inner: Rect) -> bool {
    !outer.is_empty()
        && !inner.is_empty()
        && outer.x <= inner.x
        && outer.y <= inner.y
        && outer.right() >= inner.right()
        && outer.bottom() >= inner.bottom()
}

fn days_in_month(year: u16, month: u8) -> u8 {
    let leap = (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0);
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => if leap { 29 } else { 28 },
        _ => 30,
    }
}

/// Returns 0 = Monday .. 6 = Sunday for the 1st of `(year, month)`.
fn first_weekday_mon0(year: u16, month: u8) -> u8 {
    const T: [i32; 12] = [0, 3, 2, 5, 0, 3, 5, 1, 4, 6, 2, 4];
    let m = (month as i32).clamp(1, 12);
    let y = (year as i32) - if m < 3 { 1 } else { 0 };
    let sun0 = (y + y / 4 - y / 100 + y / 400 + T[(m - 1) as usize] + 1).rem_euclid(7);
    ((sun0 + 6) % 7) as u8
}

fn control_center_height(cfg: &DesktopConfig) -> i32 {
    let mut h = 46;
    let mut any = false;
    for &m in &cfg.cc_order {
        if !cfg.cc_enabled[cc_idx(m)] {
            continue;
        }
        any = true;
        h += match m {
            ControlCenterModule::Network
            | ControlCenterModule::Audio
            | ControlCenterModule::Theme => 38,
            ControlCenterModule::Volume => 42,
            ControlCenterModule::MediaPlayer => 102,
            ControlCenterModule::Power => 34,
        } + 6;
    }
    if any { h + 6 } else { 54 }
}

fn control_center_slots(cfg: &DesktopConfig, cr: Rect) -> Vec<(ControlCenterModule, Rect)> {
    let mut slots = Vec::new();
    let mut y = cr.y + 46;
    for &m in &cfg.cc_order {
        if !cfg.cc_enabled[cc_idx(m)] {
            continue;
        }
        let h = match m {
            ControlCenterModule::Network
            | ControlCenterModule::Audio
            | ControlCenterModule::Theme => 38,
            ControlCenterModule::Volume => 42,
            ControlCenterModule::MediaPlayer => 102,
            ControlCenterModule::Power => 34,
        };
        slots.push((m, Rect::new(cr.x + 12, y, cr.w - 24, h)));
        y += h + 6;
    }
    slots
}

impl DesktopState {
    #[inline(never)]
    fn new(width: usize, height: usize) -> Self {
        let now_ms = timer::uptime_ms();
        let now_ticks = timer::ticks();
        let rtc_now = rtc::read_datetime();
        let cfg = personalization::snapshot();
        let _ = personalization::sync_auto_theme_if_needed(rtc_now.hour);
        let cfg = personalization::snapshot();

        let mut st = Self {
            compositor: Compositor::new(width, height),
            windows: Vec::new(),
            closing_ghosts: Vec::new(),
            next_win_id: 1,
            cursor_x: (width as i32) / 2,
            cursor_y: (height as i32) / 2,
            prev_left_btn: false,
            prev_right_btn: false,
            drag_mode: DragMode::None,
            snap_zone: SnapZone::None,
            open_menu: TopMenu::None,
            desktop_menu: None,
            control_center_open: false,
            calendar_open: false,
            power_menu_open: false,
            dock_reveal_q8: if cfg.desktop.dock_auto_hide { 0 } else { 256 },
            seen_gens: personalization::generations(),
            saved_volume: 85,
            music_track_idx: 0,
            last_audio_anim_ms: now_ms,
            spotlight: SpotlightState::new(),
            ext_scancode: false,
            modifiers: crate::input::KeyModifiers::default(),
            pending_actions: Vec::new(),
            should_exit: false,
            toast: Some((
                String::from("AmaryllisOS 0.10  |  Alt+Space: Spotlight  |  Right-click: Menu"),
                now_ticks + 320,
            )),
            hovered_capsule: CapsuleHit::None,
            hovered_top_menu: TopMenu::None,
            hovered_win_btn: None,
            last_clock_sec: now_ms / 1000,
            rtc_now,
            last_blink_on: false,
            last_focused_id: None,
            last_frame_ms: now_ms.saturating_sub(TARGET_FRAME_MS),
            fps_window_start_ms: now_ms,
            fps_frames_rendered: 0,
            current_fps: 0,
        };

        st.sync_wallpaper_from_config(&cfg);
        st.seen_gens = personalization::generations();
        st.compositor.invalidate_all();
        st
    }

    fn sync_wallpaper_from_config(&mut self, cfg: &SystemAppearance) {
        let mode = cfg.effective_theme_mode(self.rtc_now.hour);
        let fallback = theme::current_wallpaper();
        self.compositor.rebuild_wallpaper_from_source(
            mode,
            &cfg.wallpaper_source,
            cfg.wallpaper_scale,
            fallback,
        );
        if let Some(err) = self.compositor.last_wallpaper_error() {
            let msg = format!("Wallpaper fallback: {}", err);
            personalization::set_last_error(Some(msg.clone()));
            self.toast = Some((msg, timer::ticks() + 260));
        } else {
            personalization::set_last_error(None);
        }
    }

    fn has_external_windows(&self) -> bool {
        self.windows.iter().any(|w| w.external_user)
    }

    #[inline]
    fn window_control_rects(win_rect: Rect) -> (Rect, Rect, Rect) {
        let cy = win_rect.y + (WINDOW_TITLEBAR_HEIGHT as i32) / 2;
        let btn_close = Rect::new(win_rect.right() - 32, cy - 11, 24, 22);
        let btn_max = Rect::new(win_rect.right() - 58, cy - 11, 24, 22);
        let btn_min = Rect::new(win_rect.right() - 84, cy - 11, 24, 22);
        (btn_min, btn_max, btn_close)
    }

    #[inline]
    fn window_mini_icon(content: &WindowContent) -> LucideIcon {
        match content {
            WindowContent::Files(_) => LucideIcon::Folder,
            WindowContent::Terminal(_) => LucideIcon::Terminal,
            WindowContent::Editor(_) => LucideIcon::FileText,
            WindowContent::AudioPlayer(_) => LucideIcon::Music,
            WindowContent::NetViewer(_) => LucideIcon::Wifi,
            WindowContent::Settings(_) => LucideIcon::Settings,
            WindowContent::Monitor(_) => LucideIcon::Activity,
            WindowContent::ExternalUser { .. } => LucideIcon::Layers,
        }
    }

    fn workspace_rect(&self) -> Rect {
        let cfg = personalization::snapshot();
        let w = self.compositor.width() as i32;
        let h = self.compositor.height() as i32;
        let top = TOP_BAR_HEIGHT as i32 + 4;
        if cfg.desktop.dock_auto_hide {
            return Rect::new(6, top, (w - 12).max(200), (h - top - 6).max(160));
        }
        let icon_sz = (cfg.desktop.dock_icon_size as i32).clamp(32, 44);
        let cap_h = (icon_sz + 16).max(54);
        let bottom = h - (cap_h + DOCK_BOTTOM_MARGIN as i32 + 8);
        Rect::new(6, top, (w - 12).max(200), (bottom - top).max(160))
    }

    fn capsule_layout(
        &self,
        cfg: &SystemAppearance,
        has_ext: bool,
        err_count: usize,
    ) -> CapsuleLayout {
        let sw = self.compositor.width() as i32;
        let sh = self.compositor.height() as i32;
        let icon_sz = (cfg.desktop.dock_icon_size as i32).clamp(32, 44);
        let icon_gap = (cfg.desktop.dock_icon_gap as i32).clamp(6, 16);
        let cap_h = (icon_sz + 16).max(54);
        let hide_shift = if cfg.desktop.dock_auto_hide {
            ((cap_h + 18) * (256 - self.dock_reveal_q8)) >> 8
        } else {
            0
        };

        let launcher_w = 40i32;
        let task_count = CAPSULE_PINNED_COUNT + usize::from(has_ext);
        let tasks_w =
            (task_count as i32) * icon_sz + ((task_count as i32 - 1).max(0)) * icon_gap;

        let search_w = 30i32;
        let mut status_items = 1i32; // Battery always shown
        if cfg.desktop.topbar_show_network {
            status_items += 1;
        }
        if cfg.desktop.topbar_show_audio {
            status_items += 1;
        }
        if cfg.desktop.topbar_show_notifications || err_count > 0 {
            status_items += 1;
        }
        let status_w = 10 + status_items * 22;

        let ui_scale = cfg.typography.ui_scale_q8.clamp(240, 276);
        let mono_ui = cfg.typography.ui_font == UiFontFamily::JetBrainsMono;
        let clock_txt = self.format_clock_text(&cfg.desktop);
        let clock_w = if clock_txt.is_empty() {
            0
        } else {
            ui_text_width(&clock_txt, ui_scale, false, mono_ui) + 14
        };
        let power_w = 30i32;

        let total_w = 12
            + launcher_w
            + 12
            + 1
            + 12
            + tasks_w
            + 12
            + 1
            + 10
            + search_w
            + 4
            + status_w
            + if clock_w > 0 { 4 + clock_w } else { 0 }
            + 4
            + power_w
            + 12;

        let cap_x = (sw - total_w) / 2;
        let cap_y = sh - cap_h - (DOCK_BOTTOM_MARGIN as i32) + hide_shift;
        let capsule = Rect::new(cap_x, cap_y, total_w, cap_h);

        let mut cx = cap_x + 12;
        let launcher_btn = Rect::new(cx, cap_y + (cap_h - 40) / 2, launcher_w, 40);
        cx += launcher_w + 12;
        let div1_x = cx;
        cx += 1 + 12;

        let mut task_rects = Vec::with_capacity(task_count);
        let task_y = cap_y + (cap_h - icon_sz) / 2 - 2;
        for i in 0..task_count {
            task_rects.push(Rect::new(cx, task_y, icon_sz, icon_sz));
            cx += icon_sz + if i + 1 < task_count { icon_gap } else { 0 };
        }

        cx += 12;
        let div2_x = cx;
        cx += 1 + 10;

        let ctrl_h = 32i32;
        let ctrl_y = cap_y + (cap_h - ctrl_h) / 2;
        let search_btn = Rect::new(cx, ctrl_y, search_w, ctrl_h);
        cx += search_w + 4;
        let status_btn = Rect::new(cx, ctrl_y, status_w, ctrl_h);
        cx += status_w + 4;
        let clock_btn = if clock_w > 0 {
            let r = Rect::new(cx, ctrl_y, clock_w, ctrl_h);
            cx += clock_w + 4;
            r
        } else {
            Rect::new(cx, ctrl_y, 0, ctrl_h)
        };
        let power_btn = Rect::new(cx, ctrl_y, power_w, ctrl_h);

        CapsuleLayout {
            capsule,
            launcher_btn,
            div1_x,
            task_rects,
            div2_x,
            search_btn,
            status_btn,
            clock_btn,
            power_btn,
        }
    }

    fn dock_rect_with_cfg(&self, cfg: &SystemAppearance) -> Rect {
        self.capsule_layout(cfg, self.has_external_windows(), alerts::unread_error_count())
            .capsule
    }

    fn dock_rect(&self) -> Rect {
        let cfg = personalization::snapshot();
        self.dock_rect_with_cfg(&cfg)
    }

    fn dock_invalidation_rect_with_cfg(&self, cfg: &SystemAppearance) -> Rect {
        let d = self.dock_rect_with_cfg(cfg);
        Rect::new(d.x - 48, d.y - 46, d.w + 96, d.h + 72)
    }

    fn dock_invalidation_rect(&self) -> Rect {
        let cfg = personalization::snapshot();
        self.dock_invalidation_rect_with_cfg(&cfg)
    }

    fn dock_hot_zone(&self, cfg: &SystemAppearance) -> Rect {
        let sh = self.compositor.height() as i32;
        let d = self.dock_rect_with_cfg(cfg);
        if self.dock_reveal_q8 > 40 {
            return d.expanded(12);
        }
        Rect::new(d.x.max(0), sh - 18, d.w, 18)
    }

    fn active_app_label(&self) -> &'static str {
        match self.windows.last().filter(|w| !w.minimized) {
            Some(w) => match w.content {
                WindowContent::Files(_) => "Files",
                WindowContent::Terminal(_) => "Terminal",
                WindowContent::Editor(_) => "Text Editor",
                WindowContent::AudioPlayer(_) => "Audio Player",
                WindowContent::NetViewer(_) => "Net Viewer",
                WindowContent::Settings(_) => "Settings",
                WindowContent::Monitor(_) => "Activity Monitor",
                WindowContent::ExternalUser { .. } => "App",
            },
            None => "Desktop",
        }
    }

    fn top_menu_bar_rects(&self) -> [(TopMenu, Rect, &'static str); 5] {
        let cfg = personalization::snapshot();
        let ui_scale = cfg.typography.ui_scale_q8.clamp(240, 276);
        let mono_ui = cfg.typography.ui_font == UiFontFamily::JetBrainsMono;
        let app_name = self.active_app_label();
        let app_w = (ui_text_width(app_name, ui_scale, true, mono_ui) + 42).max(108);
        let h = TOP_BAR_HEIGHT as i32 - 4;
        let mut x = 8;
        let r_ama = Rect::new(x, 2, app_w, h);
        x += app_w + 4;
        let w_file = (ui_text_width("File", ui_scale, false, mono_ui) + 18).max(44);
        let r_file = Rect::new(x, 2, w_file, h);
        x += w_file + 2;
        let w_edit = (ui_text_width("Edit", ui_scale, false, mono_ui) + 18).max(44);
        let r_edit = Rect::new(x, 2, w_edit, h);
        x += w_edit + 2;
        let w_view = (ui_text_width("View", ui_scale, false, mono_ui) + 18).max(46);
        let r_view = Rect::new(x, 2, w_view, h);
        x += w_view + 2;
        let w_win = (ui_text_width("Window", ui_scale, false, mono_ui) + 18).max(64);
        let r_win = Rect::new(x, 2, w_win, h);
        [
            (TopMenu::Amaryllis, r_ama, app_name),
            (TopMenu::File, r_file, "File"),
            (TopMenu::Edit, r_edit, "Edit"),
            (TopMenu::View, r_view, "View"),
            (TopMenu::Window, r_win, "Window"),
        ]
    }

    fn format_clock_text(&self, cfg: &DesktopConfig) -> String {
        if !cfg.topbar_show_clock && !cfg.topbar_show_date {
            return String::new();
        }
        let date_part = if cfg.topbar_show_date {
            format!(
                "{} {} {:02}",
                self.rtc_now.weekday_short(),
                self.rtc_now.month_short(),
                self.rtc_now.day
            )
        } else {
            String::new()
        };
        let time_part = if cfg.topbar_show_clock {
            if cfg.topbar_clock_24h {
                format!("{:02}:{:02}", self.rtc_now.hour, self.rtc_now.minute)
            } else {
                let (h12, ampm) = if self.rtc_now.hour == 0 {
                    (12, "AM")
                } else if self.rtc_now.hour < 12 {
                    (self.rtc_now.hour, "AM")
                } else if self.rtc_now.hour == 12 {
                    (12, "PM")
                } else {
                    (self.rtc_now.hour - 12, "PM")
                };
                format!("{}:{:02} {}", h12, self.rtc_now.minute, ampm)
            }
        } else {
            String::new()
        };
        if !date_part.is_empty() && !time_part.is_empty() {
            format!("{}  {}", date_part, time_part)
        } else if !date_part.is_empty() {
            date_part
        } else {
            time_part
        }
    }

    fn top_menu_invalidation_rect(&self) -> Rect {
        let rects = self.top_menu_bar_rects();
        Rect::new(0, 0, rects[4].1.right() + 20, TOP_BAR_HEIGHT as i32 + 4)
    }

    fn top_menu_bar_strip_contains(&self, mx: i32, my: i32) -> bool {
        if my >= TOP_BAR_HEIGHT as i32 {
            return false;
        }
        self.top_menu_bar_rects()
            .iter()
            .any(|(_, r, _)| r.contains(mx, my))
    }

    fn dropdown_menu_rect(&self, menu: TopMenu) -> Rect {
        let rects = self.top_menu_bar_rects();
        let anchor_x = rects
            .iter()
            .find(|(m, _, _)| *m == menu)
            .map(|(_, r, _)| r.x)
            .unwrap_or(8);
        let (w, item_count) = match menu {
            TopMenu::Amaryllis => (262, 7),
            TopMenu::File => (220, 5),
            TopMenu::Edit => (208, 4),
            TopMenu::View => (228, 5),
            TopMenu::Window => (246, self.windows.len().clamp(1, 7)),
            TopMenu::None => (0, 0),
        };
        Rect::new(anchor_x, TOP_BAR_HEIGHT as i32 + 4, w, 12 + (item_count as i32) * 26)
    }

    fn desktop_context_rect(&self) -> Option<Rect> {
        self.desktop_menu.map(|(x, y)| {
            let sw = self.compositor.width() as i32;
            let sh = self.compositor.height() as i32;
            let w = 232;
            let h = 170;
            Rect::new(
                x.min(sw - w - 8).max(8),
                y.min(sh - h - 80).max(TOP_BAR_HEIGHT as i32 + 6),
                w,
                h,
            )
        })
    }

    fn control_center_rect(&self) -> Rect {
        let cfg = personalization::snapshot();
        let sw = self.compositor.width() as i32;
        let lay = self.capsule_layout(&cfg, self.has_external_windows(), alerts::unread_error_count());
        let w = 264;
        let h = control_center_height(&cfg.desktop);
        let x = (lay.status_btn.x + lay.status_btn.w / 2 - w / 2).clamp(12, (sw - w - 12).max(12));
        let y = (lay.capsule.y - h - 10).max(TOP_BAR_HEIGHT as i32 + 6);
        Rect::new(x, y, w, h)
    }

    fn calendar_rect(&self) -> Rect {
        let cfg = personalization::snapshot();
        let sw = self.compositor.width() as i32;
        let lay = self.capsule_layout(&cfg, self.has_external_windows(), alerts::unread_error_count());
        let w = 248;
        let h = 244;
        let anchor_x = if lay.clock_btn.is_empty() {
            lay.capsule.right()
        } else {
            lay.clock_btn.right()
        };
        let x = (anchor_x - w + 18).clamp(12, (sw - w - 12).max(12));
        let y = (lay.capsule.y - h - 10).max(TOP_BAR_HEIGHT as i32 + 6);
        Rect::new(x, y, w, h)
    }

    fn power_menu_rect(&self) -> Rect {
        let cfg = personalization::snapshot();
        let sw = self.compositor.width() as i32;
        let lay = self.capsule_layout(&cfg, self.has_external_windows(), alerts::unread_error_count());
        let w = 216;
        let h = 92;
        let x = (lay.power_btn.right() - w + 8).clamp(12, (sw - w - 12).max(12));
        let y = (lay.capsule.y - h - 10).max(TOP_BAR_HEIGHT as i32 + 6);
        Rect::new(x, y, w, h)
    }

    fn spotlight_rect(&self) -> Rect {
        let w = 540.min(self.compositor.width() as i32 - 40);
        let h = 360.min(self.compositor.height() as i32 - 80);
        Rect::new(
            ((self.compositor.width() as i32) - w) / 2,
            ((self.compositor.height() as i32) - h) / 2 - 28,
            w,
            h,
        )
    }

    fn toast_rect(&self, msg: &str) -> Rect {
        let cfg = personalization::snapshot();
        let mono_ui = cfg.typography.ui_font == UiFontFamily::JetBrainsMono;
        let bar_scale = cfg.typography.ui_scale_q8.clamp(240, 276);
        let sw = self.compositor.width() as i32;
        let tw = ui_text_width(msg, bar_scale, false, mono_ui) + 28;
        Rect::new((sw - tw) / 2, TOP_BAR_HEIGHT as i32 + 10, tw, 28)
    }

    fn snap_target_rect(&self, zone: SnapZone) -> Rect {
        let ws = self.workspace_rect();
        match zone {
            SnapZone::None | SnapZone::Maximize => ws,
            SnapZone::LeftHalf => Rect::new(ws.x, ws.y, ws.w / 2 - 2, ws.h),
            SnapZone::RightHalf => Rect::new(ws.x + ws.w / 2 + 2, ws.y, ws.w - ws.w / 2 - 2, ws.h),
        }
    }

    fn invalidate_dock_and_topbar(&mut self) {
        self.compositor.invalidate_rect(self.dock_invalidation_rect());
        self.compositor.invalidate_rect(self.top_menu_invalidation_rect());
    }

    fn close_control_center(&mut self) {
        if self.control_center_open {
            self.control_center_open = false;
            self.compositor.invalidate_rect(self.control_center_rect().expanded(28));
            self.invalidate_dock_and_topbar();
        }
    }

    fn close_calendar(&mut self) {
        if self.calendar_open {
            self.calendar_open = false;
            self.compositor.invalidate_rect(self.calendar_rect().expanded(28));
            self.invalidate_dock_and_topbar();
        }
    }

    fn close_power_menu(&mut self) {
        if self.power_menu_open {
            self.power_menu_open = false;
            self.compositor.invalidate_rect(self.power_menu_rect().expanded(28));
            self.invalidate_dock_and_topbar();
        }
    }

    fn dismiss_all_context_menus(&mut self, except_win_id: Option<u64>) {
        if let Some(dmr) = self.desktop_context_rect() {
            self.desktop_menu = None;
            self.compositor.invalidate_rect(dmr.expanded(28));
        }
        if self.open_menu != TopMenu::None {
            let mr = self.dropdown_menu_rect(self.open_menu);
            self.open_menu = TopMenu::None;
            self.compositor.invalidate_rect(mr.expanded(28));
            self.invalidate_dock_and_topbar();
        }
        self.close_control_center();
        self.close_calendar();
        self.close_power_menu();
        let mut dismissed_rects: Vec<Rect> = Vec::new();
        for w in &mut self.windows {
            if Some(w.id) == except_win_id {
                continue;
            }
            if let WindowContent::Files(ref mut f) = w.content {
                if f.dismiss_context_menu() {
                    w.surface_dirty = true;
                    dismissed_rects.push(w.rect.expanded(28));
                }
            }
        }
        for r in dismissed_rects {
            self.compositor.invalidate_rect(r);
        }
    }

    fn open_builtin(&mut self, kind: BuiltinAppKind) {
        if let Some(pos) = self.windows.iter().position(|w| match (&w.content, kind) {
            (WindowContent::Terminal(_), BuiltinAppKind::Terminal)
            | (WindowContent::Files(_), BuiltinAppKind::Files)
            | (WindowContent::Editor(_), BuiltinAppKind::Editor)
            | (WindowContent::AudioPlayer(_), BuiltinAppKind::AudioPlayer)
            | (WindowContent::NetViewer(_), BuiltinAppKind::NetViewer)
            | (WindowContent::Settings(_), BuiltinAppKind::Settings)
            | (WindowContent::Monitor(_), BuiltinAppKind::Monitor) => true,
            _ => false,
        }) {
            let mut win = self.windows.remove(pos);
            let old_rect = win.rect;
            win.minimized = false;
            match win.content {
                WindowContent::Files(ref mut f) => {
                    f.refresh();
                    win.surface_dirty = true;
                }
                WindowContent::AudioPlayer(ref mut ap) => {
                    ap.refresh_playlist();
                    win.surface_dirty = true;
                }
                WindowContent::Settings(ref mut s) => {
                    s.reload_lists();
                    win.surface_dirty = true;
                }
                WindowContent::Terminal(_)
                | WindowContent::Editor(_)
                | WindowContent::NetViewer(_) => {
                    win.surface_dirty = true;
                }
                _ => {}
            }
            self.windows.push(win);
            self.compositor.invalidate_rect(old_rect.expanded(28));
            self.invalidate_dock_and_topbar();
            return;
        }

        if self.windows.len() >= MAX_WINDOWS {
            return;
        }

        let ws = self.workspace_rect();
        let id = self.next_win_id;
        self.next_win_id += 1;

        let (rect, content) = match kind {
            BuiltinAppKind::Terminal => {
                let w = 680.min(ws.w - 48);
                let h = 420.min(ws.h - 44);
                (
                    Rect::new(
                        ws.x + ((ws.w - w) / 2).max(24) + 28,
                        ws.y + ((ws.h - h) / 2).max(20) + 24,
                        w,
                        h,
                    ),
                    WindowContent::Terminal(TerminalApp::new()),
                )
            }
            BuiltinAppKind::Files => {
                let w = 720.min(ws.w - 48);
                let h = 460.min(ws.h - 44);
                (
                    Rect::new(
                        ws.x + ((ws.w - w) / 2 - 44).max(20),
                        ws.y + ((ws.h - h) / 2 - 22).max(16),
                        w,
                        h,
                    ),
                    WindowContent::Files(FilesApp::new()),
                )
            }
            BuiltinAppKind::Editor => {
                let w = 720.min(ws.w - 48);
                let h = 480.min(ws.h - 40);
                (
                    Rect::new(
                        ws.x + ((ws.w - w) / 2).max(24),
                        ws.y + ((ws.h - h) / 2).max(18),
                        w,
                        h,
                    ),
                    WindowContent::Editor(EditorApp::new()),
                )
            }
            BuiltinAppKind::AudioPlayer => {
                let w = 620.min(ws.w - 48);
                let h = 420.min(ws.h - 44);
                (
                    Rect::new(
                        ws.x + ((ws.w - w) / 2 + 20).max(24),
                        ws.y + ((ws.h - h) / 2).max(20),
                        w,
                        h,
                    ),
                    WindowContent::AudioPlayer(AudioPlayerApp::new()),
                )
            }
            BuiltinAppKind::NetViewer => {
                let w = 740.min(ws.w - 48);
                let h = 480.min(ws.h - 40);
                (
                    Rect::new(
                        ws.x + ((ws.w - w) / 2 - 18).max(20),
                        ws.y + ((ws.h - h) / 2).max(18),
                        w,
                        h,
                    ),
                    WindowContent::NetViewer(NetViewerApp::new()),
                )
            }
            BuiltinAppKind::Settings => {
                let w = 820.min(ws.w - 32);
                let h = 510.min(ws.h - 32);
                (
                    Rect::new(
                        ws.x + ((ws.w - w) / 2).max(16),
                        ws.y + ((ws.h - h) / 2 - 10).max(14),
                        w,
                        h,
                    ),
                    WindowContent::Settings(SettingsApp::new()),
                )
            }
            BuiltinAppKind::Monitor => {
                let w = 680.min(ws.w - 48);
                let h = 450.min(ws.h - 42);
                (
                    Rect::new(
                        ws.x + ((ws.w - w) / 2 + 16).max(24),
                        ws.y + ((ws.h - h) / 2 + 12).max(20),
                        w,
                        h,
                    ),
                    WindowContent::Monitor(MonitorApp::new()),
                )
            }
        };

        let cfg = personalization::snapshot();
        let now_ms = timer::uptime_ms();
        let open_dur = if cfg.effective_window_animations() && cfg.windows.open_animation {
            150
        } else {
            0
        };

        self.windows.push(Window {
            id,
            title: String::from(kind.title()),
            rect,
            restore_rect: None,
            minimized: false,
            maximized: false,
            external_user: false,
            content,
            surface_cache: Vec::new(),
            surface_w: 0,
            surface_h: 0,
            surface_dirty: true,
            anim_from: rect,
            anim_to: rect,
            anim_start_ms: 0,
            anim_dur_ms: 0,
            open_anim_start_ms: now_ms,
            open_anim_dur_ms: open_dur,
        });
        self.compositor.invalidate_rect(rect.expanded(36));
        self.invalidate_dock_and_topbar();
    }

    fn open_editor_with_file(&mut self, path: &str) {
        self.open_builtin(BuiltinAppKind::Editor);
        if let Some(win) = self.windows.last_mut() {
            if let WindowContent::Editor(ref mut ed) = win.content {
                ed.open_file(path);
                win.title = format!("Text Editor — {}", path);
                win.surface_dirty = true;
                self.compositor.invalidate_rect(win.rect.expanded(12));
                self.invalidate_dock_and_topbar();
            }
        }
    }

    fn open_audio_player_with_wav(&mut self, path: &str) {
        self.open_builtin(BuiltinAppKind::AudioPlayer);
        if let Some(win) = self.windows.last_mut() {
            if let WindowContent::AudioPlayer(ref mut ap) = win.content {
                ap.play_wav_path(path);
                win.surface_dirty = true;
                self.compositor.invalidate_rect(win.rect.expanded(12));
                self.invalidate_dock_and_topbar();
            }
        }
    }

    fn create_external_window(
        &mut self,
        owner_pid: usize,
        title: &str,
        w: usize,
        h: usize,
        shm_id: Option<u64>,
    ) -> Result<u64, i64> {
        if self.windows.len() >= MAX_WINDOWS {
            return Err(-12);
        }
        let cw = w.clamp(160, 1280);
        let ch = h.clamp(120, 800);
        let ws = self.workspace_rect();
        let win_w = (cw as i32) + 2;
        let win_h = (ch as i32) + WINDOW_TITLEBAR_HEIGHT as i32 + 2;
        let cascade = (self.windows.iter().filter(|win| win.external_user).count() as i32) * 28;
        let x = (ws.x + ((ws.w - win_w) / 2).max(16) + cascade).min(ws.right() - win_w - 8);
        let y = (ws.y + ((ws.h - win_h) / 2).max(16) + cascade).min(ws.bottom() - win_h - 8);

        let id = self.next_win_id;
        self.next_win_id += 1;

        let rect = Rect::new(x, y, win_w, win_h);
        let initial_pixels = if shm_id.is_some() {
            Vec::new()
        } else {
            vec![0xFF0B0B0E; cw * ch]
        };
        let cfg = personalization::snapshot();
        let now_ms = timer::uptime_ms();
        let open_dur = if cfg.effective_window_animations() && cfg.windows.open_animation {
            150
        } else {
            0
        };

        self.windows.push(Window {
            id,
            title: String::from(title),
            rect,
            restore_rect: None,
            minimized: false,
            maximized: false,
            external_user: true,
            content: WindowContent::ExternalUser {
                owner_pid,
                width: cw,
                height: ch,
                pixels: initial_pixels,
                shm_id,
                events: Vec::new(),
            },
            surface_cache: Vec::new(),
            surface_w: 0,
            surface_h: 0,
            surface_dirty: false,
            anim_from: rect,
            anim_to: rect,
            anim_start_ms: 0,
            anim_dur_ms: 0,
            open_anim_start_ms: now_ms,
            open_anim_dur_ms: open_dur,
        });
        self.compositor.invalidate_rect(rect.expanded(28));
        self.invalidate_dock_and_topbar();
        Ok(id)
    }

    fn close_window(&mut self, win_id: u64) {
        if let Some(pos) = self.windows.iter().position(|w| w.id == win_id) {
            if self.windows[pos].external_user {
                if let WindowContent::ExternalUser { ref mut events, .. } = self.windows[pos].content {
                    events.push(UserGuiEvent { kind: 1, p1: 0, p2: 0, p3: 0 });
                    return;
                }
            }
            let r = self.windows[pos].rect;
            let cfg = personalization::snapshot();
            if cfg.effective_window_animations() && cfg.windows.close_animation {
                let pal = theme::palette();
                self.closing_ghosts.push(ClosingGhost {
                    rect: r,
                    rad: cfg.windows.corner_radius as i32,
                    tbg: pal.win_title_active,
                    bg: pal.win_bg_active,
                    start_ms: timer::uptime_ms(),
                    dur_ms: 140,
                });
            }
            self.windows.remove(pos);
            self.compositor.invalidate_rect(r.expanded(32));
            self.invalidate_dock_and_topbar();
        }
    }

    fn focus_window(&mut self, idx: usize) {
        if idx < self.windows.len() {
            let mut win = self.windows.remove(idx);
            let r = win.rect;
            win.minimized = false;
            win.surface_dirty = win.surface_dirty
                || matches!(win.content, WindowContent::Terminal(_) | WindowContent::Editor(_));
            self.windows.push(win);
            self.compositor.invalidate_rect(r.expanded(28));
            self.invalidate_dock_and_topbar();
        }
    }

    fn toggle_maximize(&mut self, idx: usize) {
        let ws = self.workspace_rect();
        let cfg = personalization::snapshot();
        let animate_move = cfg.effective_window_animations() && cfg.windows.move_animation;
        let now_ms = timer::uptime_ms();
        let mut union_dirty = None;
        if let Some(win) = self.windows.get_mut(idx) {
            let old_rect = win.rect;
            if win.maximized {
                if let Some(old) = win.restore_rect.take() {
                    win.rect = old;
                }
                win.maximized = false;
            } else {
                win.restore_rect = Some(win.rect);
                win.rect = ws;
                win.maximized = true;
            }
            if animate_move {
                win.anim_from = old_rect;
                win.anim_to = win.rect;
                win.anim_start_ms = now_ms;
                win.anim_dur_ms = 160;
            } else {
                win.anim_dur_ms = 0;
            }
            win.surface_dirty = true;
            union_dirty = Some(old_rect.union(win.rect).expanded(32));
        }
        if let Some(r) = union_dirty {
            self.compositor.invalidate_rect(r);
        }
    }

    fn apply_theme(&mut self, mode: ThemeMode) {
        let pref = match mode {
            ThemeMode::Light => ThemePreference::Light,
            ThemeMode::Dark => ThemePreference::Dark,
        };
        personalization::update_and_save(true, true, false, |c| {
            c.theme = pref;
        });
        let label = match mode {
            ThemeMode::Light => "Switched to White Theme",
            ThemeMode::Dark => "Switched to Black Theme",
        };
        let tr = self.toast_rect(label).expanded(24);
        self.toast = Some((String::from(label), timer::ticks() + 220));
        self.compositor.invalidate_rect(tr);
    }

    fn toggle_theme(&mut self) {
        let cfg = personalization::snapshot();
        let cur = cfg.effective_theme_mode(self.rtc_now.hour);
        let next = match cur {
            ThemeMode::Light => ThemeMode::Dark,
            ThemeMode::Dark => ThemeMode::Light,
        };
        self.apply_theme(next);
    }

    fn apply_wallpaper(&mut self, preset: WallpaperPreset) {
        personalization::update_and_save(false, true, false, |c| {
            c.wallpaper_source = WallpaperSource::Preset(preset);
        });
        let msg = format!("Wallpaper: {}", preset.title());
        let tr = self.toast_rect(&msg).expanded(24);
        self.toast = Some((msg, timer::ticks() + 200));
        self.compositor.invalidate_rect(tr);
    }

    fn apply_custom_wallpaper(&mut self, path: &str) {
        let p = String::from(path.trim());
        personalization::update_and_save(false, true, false, |c| {
            c.wallpaper_source = WallpaperSource::CustomFile(p.clone());
        });
        let cfg = personalization::snapshot();
        self.seen_gens = personalization::generations();
        self.compositor.force_rebuild_custom();
        self.sync_wallpaper_from_config(&cfg);
        if self.compositor.last_wallpaper_error().is_none() {
            self.toast = Some((format!("Wallpaper: {}", p), timer::ticks() + 220));
        }
        for w in &mut self.windows {
            if matches!(w.content, WindowContent::Settings(_)) {
                w.surface_dirty = true;
            }
        }
        self.compositor.invalidate_all();
    }

    fn next_wallpaper(&mut self) {
        let cfg = personalization::snapshot();
        let cur = match cfg.wallpaper_source {
            WallpaperSource::Preset(p) => p,
            WallpaperSource::CustomFile(_) => theme::current_wallpaper(),
        };
        self.apply_wallpaper(cur.next());
    }

    fn process_inputs(&mut self, allow_shell_spawn: bool) {
        let mut left_pressed_edge = false;
        let mut left_released_edge = false;
        let mut right_pressed_edge = false;
        let mut wheel_total: i8 = 0;
        let mut cursor_moved = false;

        let mut got_irq_packets = false;
        loop {
            while let Some(pkt) = crate::input::read_mouse_packet() {
                got_irq_packets = true;
                let new_x = (self.cursor_x + pkt.dx as i32).clamp(0, self.compositor.width() as i32 - 2);
                let new_y = (self.cursor_y + pkt.dy as i32).clamp(0, self.compositor.height() as i32 - 2);
                if new_x != self.cursor_x || new_y != self.cursor_y {
                    self.compositor.invalidate_cursor_move(new_x, new_y);
                    self.cursor_x = new_x;
                    self.cursor_y = new_y;
                    cursor_moved = true;
                }
                let left = pkt.left();
                if left && !self.prev_left_btn {
                    left_pressed_edge = true;
                } else if !left && self.prev_left_btn {
                    left_released_edge = true;
                }
                self.prev_left_btn = left;

                let right = pkt.right();
                if right && !self.prev_right_btn {
                    right_pressed_edge = true;
                }
                self.prev_right_btn = right;

                wheel_total = wheel_total.saturating_add(pkt.wheel);
            }
            if got_irq_packets || crate::input::poll_mouse_hw() == 0 {
                break;
            }
            got_irq_packets = true;
        }

        if cursor_moved {
            let cfg_hov = personalization::snapshot();
            let lay = self.capsule_layout(
                &cfg_hov,
                self.has_external_windows(),
                alerts::unread_error_count(),
            );
            let mut new_cap = CapsuleHit::None;
            if lay.capsule.contains(self.cursor_x, self.cursor_y) {
                if lay.launcher_btn.contains(self.cursor_x, self.cursor_y) {
                    new_cap = CapsuleHit::Launcher;
                } else if lay.search_btn.contains(self.cursor_x, self.cursor_y) {
                    new_cap = CapsuleHit::Search;
                } else if lay.status_btn.contains(self.cursor_x, self.cursor_y) {
                    new_cap = CapsuleHit::Status;
                } else if !lay.clock_btn.is_empty()
                    && lay.clock_btn.contains(self.cursor_x, self.cursor_y)
                {
                    new_cap = CapsuleHit::Clock;
                } else if lay.power_btn.contains(self.cursor_x, self.cursor_y) {
                    new_cap = CapsuleHit::Power;
                } else {
                    for (i, r) in lay.task_rects.iter().enumerate() {
                        if r.contains(self.cursor_x, self.cursor_y) {
                            new_cap = CapsuleHit::Task(i);
                            break;
                        }
                    }
                }
            }
            if new_cap != self.hovered_capsule {
                self.hovered_capsule = new_cap;
                self.compositor
                    .invalidate_rect(self.dock_invalidation_rect_with_cfg(&cfg_hov));
            }

            let mut new_tm = TopMenu::None;
            if self.cursor_y < TOP_BAR_HEIGHT as i32 {
                for (mk, r, _) in self.top_menu_bar_rects() {
                    if r.contains(self.cursor_x, self.cursor_y) {
                        new_tm = mk;
                        break;
                    }
                }
            }
            if new_tm != self.hovered_top_menu {
                self.hovered_top_menu = new_tm;
                self.compositor.invalidate_rect(self.top_menu_invalidation_rect());
            }

            let mut new_win_btn: Option<(u64, WindowBtn)> = None;
            for w in self.windows.iter().rev() {
                if !w.minimized && w.rect.contains(self.cursor_x, self.cursor_y) {
                    let (b_min, b_max, b_close) = Self::window_control_rects(w.rect);
                    if b_close.contains(self.cursor_x, self.cursor_y) {
                        new_win_btn = Some((w.id, WindowBtn::Close));
                    } else if b_max.contains(self.cursor_x, self.cursor_y) {
                        new_win_btn = Some((w.id, WindowBtn::Maximize));
                    } else if b_min.contains(self.cursor_x, self.cursor_y) {
                        new_win_btn = Some((w.id, WindowBtn::Minimize));
                    }
                    break;
                }
            }
            if new_win_btn != self.hovered_win_btn {
                let prev_id = self.hovered_win_btn.map(|(id, _)| id);
                let next_id = new_win_btn.map(|(id, _)| id);
                self.hovered_win_btn = new_win_btn;
                let mut inv_rects: Vec<Rect> = Vec::new();
                for w in &self.windows {
                    if Some(w.id) == prev_id || Some(w.id) == next_id {
                        inv_rects.push(Rect::new(
                            w.rect.right() - 92,
                            w.rect.y,
                            92,
                            WINDOW_TITLEBAR_HEIGHT as i32 + 2,
                        ));
                    }
                }
                for r in inv_rects {
                    self.compositor.invalidate_rect(r);
                }
            }

            if self.open_menu != TopMenu::None {
                let mr = self.dropdown_menu_rect(self.open_menu);
                if mr.contains(self.cursor_x, self.cursor_y) {
                    self.compositor.invalidate_rect(mr.expanded(12));
                }
            }
            if self.power_menu_open {
                let pr = self.power_menu_rect();
                if pr.contains(self.cursor_x, self.cursor_y) {
                    self.compositor.invalidate_rect(pr.expanded(12));
                }
            }
            if let Some(dmr) = self.desktop_context_rect() {
                if dmr.contains(self.cursor_x, self.cursor_y) {
                    self.compositor.invalidate_rect(dmr.expanded(12));
                }
            }
        }

        match self.drag_mode {
            DragMode::Move { win_id, off_x, off_y } => {
                if left_released_edge || !self.prev_left_btn {
                    let old_snap = self.snap_zone;
                    let mut dirty = None;
                    if old_snap != SnapZone::None {
                        let target = self.snap_target_rect(old_snap);
                        let cfg = personalization::snapshot();
                        let animate_move = cfg.effective_window_animations() && cfg.windows.move_animation;
                        let now_ms = timer::uptime_ms();
                        if let Some(w) = self.windows.iter_mut().find(|w| w.id == win_id) {
                            let old_r = w.rect;
                            w.restore_rect.get_or_insert(w.rect);
                            w.rect = target;
                            w.maximized = old_snap == SnapZone::Maximize;
                            if animate_move {
                                w.anim_from = old_r;
                                w.anim_to = target;
                                w.anim_start_ms = now_ms;
                                w.anim_dur_ms = 160;
                            }
                            w.surface_dirty = true;
                            dirty = Some(old_r.union(target).expanded(32));
                        }
                    }
                    self.drag_mode = DragMode::None;
                    self.snap_zone = SnapZone::None;
                    if let Some(d) = dirty {
                        self.compositor.invalidate_rect(d);
                    }
                    if old_snap != SnapZone::None {
                        self.compositor.invalidate_rect(self.snap_target_rect(old_snap).expanded(8));
                    }
                } else {
                    let nx = self.cursor_x - off_x;
                    let ny = (self.cursor_y - off_y).max(TOP_BAR_HEIGHT as i32);
                    let mut move_dirty = None;
                    if let Some(w) = self.windows.iter_mut().find(|w| w.id == win_id) {
                        if w.rect.x != nx || w.rect.y != ny {
                            let old_r = w.rect;
                            w.rect.x = nx;
                            w.rect.y = ny;
                            w.maximized = false;
                            move_dirty = Some(old_r.union(w.rect).expanded(28));
                        }
                    }
                    if let Some(d) = move_dirty {
                        self.compositor.invalidate_rect(d);
                    }
                    let sw = self.compositor.width() as i32;
                    let new_snap = if self.cursor_x <= 14 {
                        SnapZone::LeftHalf
                    } else if self.cursor_x >= sw - 16 {
                        SnapZone::RightHalf
                    } else if self.cursor_y <= TOP_BAR_HEIGHT as i32 + 4 {
                        SnapZone::Maximize
                    } else {
                        SnapZone::None
                    };
                    if new_snap != self.snap_zone {
                        if self.snap_zone != SnapZone::None {
                            self.compositor.invalidate_rect(self.snap_target_rect(self.snap_zone).expanded(8));
                        }
                        self.snap_zone = new_snap;
                        if new_snap != SnapZone::None {
                            self.compositor.invalidate_rect(self.snap_target_rect(new_snap).expanded(8));
                        }
                    }
                }
            }
            DragMode::Resize { win_id, start_mx, start_my, orig_w, orig_h } => {
                if left_released_edge || !self.prev_left_btn {
                    self.drag_mode = DragMode::None;
                } else {
                    let nw = (orig_w + (self.cursor_x - start_mx)).max(WINDOW_MIN_WIDTH as i32);
                    let nh = (orig_h + (self.cursor_y - start_my)).max(WINDOW_MIN_HEIGHT as i32);
                    let mut resize_dirty = None;
                    if let Some(w) = self.windows.iter_mut().find(|w| w.id == win_id) {
                        if w.rect.w != nw || w.rect.h != nh {
                            let old_r = w.rect;
                            w.rect.w = nw;
                            w.rect.h = nh;
                            w.surface_dirty = true;
                            resize_dirty = Some(old_r.union(w.rect).expanded(28));
                        }
                    }
                    if let Some(d) = resize_dirty {
                        self.compositor.invalidate_rect(d);
                    }
                }
            }
            DragMode::None => {}
        }

        if wheel_total != 0 && !alerts::has_active_modal() && !self.spotlight.open {
            if let Some(top) = self.windows.last_mut() {
                if !top.minimized && top.rect.contains(self.cursor_x, self.cursor_y) {
                    let mut scrolled = false;
                    match &mut top.content {
                        WindowContent::Terminal(t) => {
                            t.on_wheel(wheel_total);
                            scrolled = true;
                        }
                        WindowContent::Files(f) => {
                            f.on_wheel(wheel_total);
                            scrolled = true;
                        }
                        WindowContent::Editor(e) => {
                            e.on_wheel(wheel_total);
                            scrolled = true;
                        }
                        WindowContent::AudioPlayer(ap) => {
                            ap.on_wheel(wheel_total);
                            scrolled = true;
                        }
                        WindowContent::NetViewer(nv) => {
                            nv.on_wheel(wheel_total);
                            scrolled = true;
                        }
                        WindowContent::Settings(s) => {
                            s.on_wheel(wheel_total);
                            scrolled = true;
                        }
                        _ => {}
                    }
                    if scrolled {
                        top.surface_dirty = true;
                        self.compositor.invalidate_rect(top.rect);
                    }
                }
            }
        }

        if right_pressed_edge {
            self.handle_right_click(self.cursor_x, self.cursor_y);
        }

        if left_pressed_edge {
            self.handle_left_click(self.cursor_x, self.cursor_y, allow_shell_spawn);
        }

        let _ = crate::input::poll_keyboard_hw();
        while let Some(sc) = crate::input::read_scancode() {
            let Some(evt) = crate::input::decode_scancode(sc, &mut self.ext_scancode, &mut self.modifiers) else {
                continue;
            };
            self.handle_key_event(evt, allow_shell_spawn);
        }

        let now_ms = timer::uptime_ms();
        let now_ticks = timer::ticks();

        if now_ms.saturating_sub(self.fps_window_start_ms) >= 1000 {
            self.current_fps = self.fps_frames_rendered;
            self.fps_frames_rendered = 0;
            self.fps_window_start_ms = now_ms;
        }

        let now_sec = now_ms / 1000;
        if now_sec != self.last_clock_sec {
            self.last_clock_sec = now_sec;
            let new_rtc = rtc::read_datetime();
            let _ = personalization::sync_auto_theme_if_needed(new_rtc.hour);
            if new_rtc.minute != self.rtc_now.minute
                || new_rtc.hour != self.rtc_now.hour
                || new_rtc.day != self.rtc_now.day
            {
                self.rtc_now = new_rtc;
                self.compositor.invalidate_rect(self.dock_invalidation_rect());
            } else {
                self.rtc_now.second = new_rtc.second;
            }
            if self.calendar_open {
                self.compositor.invalidate_rect(self.calendar_rect().expanded(8));
            }
        }

        // Flush debounced /disk personalization config writes cleanly outside clicks/drags
        if self.drag_mode == DragMode::None {
            personalization::tick_disk_sync(now_ms);
        }

        // Reactive synchronization with Amaryllis Personalization Service
        let gens = personalization::generations();
        if gens != self.seen_gens {
            let theme_changed = gens.0 != self.seen_gens.0;
            let wp_changed = gens.1 != self.seen_gens.1;
            let layout_changed = gens.2 != self.seen_gens.2;
            self.seen_gens = gens;
            let cfg = personalization::snapshot();
            if wp_changed || theme_changed {
                self.sync_wallpaper_from_config(&cfg);
            }
            if !cfg.desktop.dock_auto_hide {
                self.dock_reveal_q8 = 256;
            }
            if theme_changed || layout_changed || wp_changed {
                for w in &mut self.windows {
                    w.surface_dirty = true;
                }
                self.compositor.invalidate_all();
            }
        }

        let cfg = personalization::snapshot();
        let pal = theme::palette();
        self.compositor.step_wallpaper_animation(
            now_ms,
            cfg.effective_wallpaper_animations(),
            pal.accent_primary,
        );

        // Smooth Amaryllis Capsule Auto-Hide slide interpolation
        if cfg.desktop.dock_auto_hide {
            let hot = self.dock_hot_zone(&cfg);
            let want_show = hot.contains(self.cursor_x, self.cursor_y)
                || self.hovered_capsule != CapsuleHit::None;
            let target = if want_show { 256 } else { 0 };
            if self.dock_reveal_q8 != target {
                if !cfg.effective_dock_animations() {
                    self.dock_reveal_q8 = target;
                } else if self.dock_reveal_q8 < target {
                    self.dock_reveal_q8 = (self.dock_reveal_q8 + 40).min(target);
                } else {
                    self.dock_reveal_q8 = (self.dock_reveal_q8 - 32).max(target);
                }
                self.compositor
                    .invalidate_rect(self.dock_invalidation_rect_with_cfg(&cfg));
            }
        } else if self.dock_reveal_q8 != 256 {
            self.dock_reveal_q8 = 256;
            self.compositor
                .invalidate_rect(self.dock_invalidation_rect_with_cfg(&cfg));
        }

        // Advance active window & closing ghost animations
        let mut anim_dirty_rects: Vec<Rect> = Vec::new();
        for w in &mut self.windows {
            if w.anim_dur_ms > 0 {
                anim_dirty_rects.push(w.anim_from.union(w.anim_to).expanded(36));
                if now_ms.saturating_sub(w.anim_start_ms) >= w.anim_dur_ms {
                    w.anim_dur_ms = 0;
                    w.surface_dirty = true;
                }
            }
            if w.open_anim_dur_ms > 0 {
                anim_dirty_rects.push(w.rect.expanded(36));
                if now_ms.saturating_sub(w.open_anim_start_ms) >= w.open_anim_dur_ms {
                    w.open_anim_dur_ms = 0;
                }
            }
        }
        let mut ghost_i = 0usize;
        while ghost_i < self.closing_ghosts.len() {
            let g = &self.closing_ghosts[ghost_i];
            anim_dirty_rects.push(g.rect.expanded(32));
            if now_ms.saturating_sub(g.start_ms) >= g.dur_ms {
                self.closing_ghosts.remove(ghost_i);
            } else {
                ghost_i += 1;
            }
        }
        for r in anim_dirty_rects {
            self.compositor.invalidate_rect(r);
        }

        if self.control_center_open
            && crate::audio::mixer::is_active()
            && now_ms.saturating_sub(self.last_audio_anim_ms) >= 140
        {
            self.last_audio_anim_ms = now_ms;
            self.compositor.invalidate_rect(self.control_center_rect().expanded(12));
        }

        let focused_id = self
            .windows
            .last()
            .filter(|w| !w.minimized && !self.spotlight.open)
            .map(|w| w.id);
        if focused_id != self.last_focused_id {
            self.last_focused_id = focused_id;
            for w in &mut self.windows {
                if matches!(w.content, WindowContent::Terminal(_) | WindowContent::Editor(_)) {
                    w.surface_dirty = true;
                }
            }
        }

        let blink_on = (now_ms / 450) % 2 == 0;
        if blink_on != self.last_blink_on {
            self.last_blink_on = blink_on;
            let spotlight_open = self.spotlight.open;
            if let Some(top) = self.windows.last_mut() {
                if !top.minimized
                    && !spotlight_open
                    && matches!(top.content, WindowContent::Terminal(_) | WindowContent::Editor(_))
                {
                    top.surface_dirty = true;
                    self.compositor.invalidate_rect(top.rect);
                }
            }
        }

        let mut expired_toast_rect: Option<Rect> = None;
        if let Some((ref msg, exp)) = self.toast {
            if now_ticks > exp {
                expired_toast_rect = Some(self.toast_rect(msg).expanded(24));
            }
        }
        if let Some(tr) = expired_toast_rect {
            self.toast = None;
            self.compositor.invalidate_rect(tr);
        }

        // Non-blocking background AC'97 mixer tick
        let _audio_active = crate::audio::mixer::tick();

        let mut app_dirty_rects: Vec<Rect> = Vec::new();
        for w in &mut self.windows {
            if !w.minimized {
                match &mut w.content {
                    WindowContent::Monitor(m) => {
                        if m.tick(now_ticks) {
                            w.surface_dirty = true;
                            app_dirty_rects.push(w.rect);
                        }
                    }
                    WindowContent::AudioPlayer(ap) => {
                        if ap.tick(now_ticks) {
                            w.surface_dirty = true;
                            app_dirty_rects.push(w.rect);
                        }
                    }
                    WindowContent::NetViewer(nv) => {
                        if nv.tick() {
                            w.surface_dirty = true;
                            app_dirty_rects.push(w.rect);
                        }
                    }
                    _ => {}
                }
            }
        }
        for r in app_dirty_rects {
            self.compositor.invalidate_rect(r);
        }
    }

    fn handle_right_click(&mut self, mx: i32, my: i32) {
        if alerts::has_active_modal() || self.spotlight.open {
            return;
        }
        if self.top_menu_bar_strip_contains(mx, my) || self.dock_rect().contains(mx, my) {
            self.dismiss_all_context_menus(None);
            return;
        }

        let mut hit_idx: Option<usize> = None;
        for i in (0..self.windows.len()).rev() {
            let w = &self.windows[i];
            if !w.minimized && w.rect.contains(mx, my) {
                hit_idx = Some(i);
                break;
            }
        }

        if let Some(idx) = hit_idx {
            let hit_id = self.windows[idx].id;
            self.dismiss_all_context_menus(Some(hit_id));
            self.focus_window(idx);
            let top_rect = self.windows.last().unwrap().rect;
            let content_rect = Rect::new(
                top_rect.x + 1,
                top_rect.y + WINDOW_TITLEBAR_HEIGHT as i32,
                top_rect.w - 2,
                top_rect.h - WINDOW_TITLEBAR_HEIGHT as i32 - 1,
            );
            let mut prev_cm = None;
            if content_rect.contains(mx, my) {
                let top = self.windows.last_mut().unwrap();
                if let WindowContent::Files(ref mut f) = top.content {
                    prev_cm = f.on_right_click(content_rect, mx, my);
                    top.surface_dirty = true;
                }
            }
            if let Some(prev_r) = prev_cm {
                self.compositor.invalidate_rect(prev_r.expanded(24));
            }
            self.compositor.invalidate_rect(top_rect.expanded(28));
            return;
        }

        self.dismiss_all_context_menus(None);
        self.desktop_menu = Some((mx, my));
        if let Some(dmr) = self.desktop_context_rect() {
            self.compositor.invalidate_rect(dmr.expanded(24));
        }
    }

    fn handle_top_menu_item(&mut self, menu: TopMenu, row: usize) {
        match menu {
            TopMenu::Amaryllis => match row {
                0 => {
                    self.spotlight.open = true;
                    self.spotlight.query.clear();
                    self.spotlight.rebuild_items();
                    self.compositor.invalidate_rect(self.spotlight_rect().expanded(28));
                    self.invalidate_dock_and_topbar();
                }
                1 => self.open_builtin(BuiltinAppKind::Settings),
                2 => self.toggle_theme(),
                3 => self.open_builtin(BuiltinAppKind::Monitor),
                4 => {
                    alerts::report_system_error(
                        AlertSeverity::Info,
                        AlertSubsystem::Desktop,
                        "ABOUT OS",
                        "AmaryllisOS 0.10 Desktop",
                        "64-bit Rust monolithic OS with RAMFS + FAT32 /disk, Intel e1000 HTTP, async AC'97 mixer, and Ring 3 ELF64.",
                        "Use Spotlight (Alt+Space) to launch apps or run .elf binaries.",
                        true,
                    );
                    self.compositor.invalidate_all();
                }
                5 => {
                    self.pending_actions.push(DesktopAction::ExecElf(String::from("/gui_demo.elf")));
                }
                6 => self.should_exit = true,
                _ => {}
            },
            TopMenu::File => match row {
                0 => {
                    self.open_builtin(BuiltinAppKind::Editor);
                    if let Some(win) = self.windows.last_mut() {
                        if let WindowContent::Editor(ref mut ed) = win.content {
                            ed.new_untitled();
                            win.title = String::from("Text Editor — /untitled.txt");
                            win.surface_dirty = true;
                            self.compositor.invalidate_rect(win.rect.expanded(28));
                        }
                    }
                }
                1 => {
                    self.open_builtin(BuiltinAppKind::Files);
                    if let Some(win) = self.windows.last_mut() {
                        if let WindowContent::Files(ref mut f) = win.content {
                            f.begin_new_dir();
                            win.surface_dirty = true;
                            self.compositor.invalidate_rect(win.rect.expanded(28));
                        }
                    }
                }
                2 => {
                    if let Some(win) = self.windows.last_mut() {
                        if let WindowContent::Editor(ref mut ed) = win.content {
                            ed.save();
                            win.surface_dirty = true;
                            self.compositor.invalidate_rect(win.rect.expanded(28));
                        }
                    }
                }
                3 => {
                    for win in &mut self.windows {
                        if let WindowContent::Files(ref mut f) = win.content {
                            f.refresh();
                            win.surface_dirty = true;
                            self.compositor.invalidate_rect(win.rect.expanded(28));
                        }
                    }
                }
                4 => {
                    if let Some(id) = self.windows.last().map(|w| w.id) {
                        self.close_window(id);
                    }
                }
                _ => {}
            },
            TopMenu::Edit => {
                if let Some(win) = self.windows.last_mut() {
                    match &mut win.content {
                        WindowContent::Editor(ed) => {
                            match row {
                                0 => ed.copy_to_clipboard(),
                                1 => ed.paste_from_clipboard(),
                                2 => ed.select_all_text(),
                                _ => {}
                            }
                            win.surface_dirty = true;
                            self.compositor.invalidate_rect(win.rect.expanded(28));
                        }
                        WindowContent::Files(f) => {
                            match row {
                                0 => f.copy_selected(),
                                1 => f.paste_clipboard(),
                                2 => f.cut_selected(),
                                3 => f.begin_rename(),
                                _ => {}
                            }
                            win.surface_dirty = true;
                            self.compositor.invalidate_rect(win.rect.expanded(28));
                        }
                        WindowContent::Terminal(t) => {
                            match row {
                                0 => t.copy_to_clipboard(),
                                1 => t.paste_from_clipboard(),
                                3 => t.clear_lines(),
                                _ => {}
                            }
                            win.surface_dirty = true;
                            self.compositor.invalidate_rect(win.rect.expanded(28));
                        }
                        _ => {}
                    }
                }
            }
            TopMenu::View => {
                let top_idx = self.windows.len().checked_sub(1);
                match row {
                    0 | 1 | 2 => {
                        if let Some(idx) = top_idx {
                            let zone = match row {
                                0 => SnapZone::LeftHalf,
                                1 => SnapZone::RightHalf,
                                _ => SnapZone::Maximize,
                            };
                            if zone == SnapZone::Maximize {
                                self.toggle_maximize(idx);
                            } else {
                                let target = self.snap_target_rect(zone);
                                let cfg = personalization::snapshot();
                                let animate_move = cfg.effective_window_animations() && cfg.windows.move_animation;
                                let now_ms = timer::uptime_ms();
                                let old_r = self.windows[idx].rect;
                                self.windows[idx].restore_rect.get_or_insert(old_r);
                                self.windows[idx].rect = target;
                                self.windows[idx].maximized = false;
                                if animate_move {
                                    self.windows[idx].anim_from = old_r;
                                    self.windows[idx].anim_to = target;
                                    self.windows[idx].anim_start_ms = now_ms;
                                    self.windows[idx].anim_dur_ms = 160;
                                }
                                self.windows[idx].surface_dirty = true;
                                self.compositor.invalidate_rect(old_r.union(target).expanded(32));
                            }
                        }
                    }
                    3 => {
                        if let Some(idx) = top_idx {
                            let r = self.windows[idx].rect;
                            self.windows[idx].minimized = true;
                            self.compositor.invalidate_rect(r.expanded(28));
                            self.invalidate_dock_and_topbar();
                        }
                    }
                    4 => self.toggle_theme(),
                    _ => {}
                }
            }
            TopMenu::Window => {
                if row < self.windows.len() {
                    self.focus_window(row);
                }
            }
            TopMenu::None => {}
        }
    }

    fn handle_left_click(&mut self, mx: i32, my: i32, allow_shell_spawn: bool) {
        if alerts::has_active_modal() {
            match alerts::handle_modal_click(self.compositor.width(), self.compositor.height(), mx, my) {
                ModalAction::OpenMonitor => {
                    self.open_builtin(BuiltinAppKind::Monitor);
                    if let Some(w) = self.windows.last_mut() {
                        if let WindowContent::Monitor(ref mut m) = w.content {
                            m.select_alerts_tab();
                            w.surface_dirty = true;
                        }
                    }
                    self.compositor.invalidate_all();
                }
                ModalAction::Dismiss => {
                    self.compositor.invalidate_all();
                }
                ModalAction::None => {}
            }
            return;
        }

        if self.spotlight.open {
            let sp = self.spotlight_rect();
            if !sp.contains(mx, my) {
                self.spotlight.open = false;
                self.compositor.invalidate_rect(sp.expanded(28));
                self.invalidate_dock_and_topbar();
                return;
            }
            let list_y = sp.y + 62;
            if my >= list_y {
                let row = ((my - list_y) / 44) as usize;
                if let Some(item) = self.spotlight.items.get(row) {
                    let act = item.action.clone();
                    self.spotlight.open = false;
                    self.compositor.invalidate_rect(sp.expanded(28));
                    self.invalidate_dock_and_topbar();
                    self.dispatch_action(act, allow_shell_spawn);
                }
            }
            return;
        }

        if let Some(dmr) = self.desktop_context_rect() {
            self.desktop_menu = None;
            self.compositor.invalidate_rect(dmr.expanded(28));
            if dmr.contains(mx, my) {
                let row = ((my - dmr.y - 7) / 26).max(0) as usize;
                match row {
                    0 => self.open_builtin(BuiltinAppKind::Editor),
                    1 => self.open_builtin(BuiltinAppKind::Terminal),
                    2 => self.open_builtin(BuiltinAppKind::Files),
                    3 => self.next_wallpaper(),
                    4 => self.toggle_theme(),
                    5 => {
                        self.spotlight.open = true;
                        self.spotlight.query.clear();
                        self.spotlight.rebuild_items();
                        self.compositor.invalidate_rect(self.spotlight_rect().expanded(28));
                        self.invalidate_dock_and_topbar();
                    }
                    _ => {}
                }
                return;
            }
        }

        if self.open_menu != TopMenu::None {
            let active_menu = self.open_menu;
            let menu_r = self.dropdown_menu_rect(active_menu);
            self.open_menu = TopMenu::None;
            self.compositor.invalidate_rect(menu_r.expanded(28));
            self.invalidate_dock_and_topbar();
            if menu_r.contains(mx, my) {
                let row = ((my - menu_r.y - 6) / 26).max(0) as usize;
                self.handle_top_menu_item(active_menu, row);
                return;
            }
        }

        let cfg_click = personalization::snapshot();
        let lay_click = self.capsule_layout(
            &cfg_click,
            self.has_external_windows(),
            alerts::unread_error_count(),
        );

        if self.control_center_open {
            let cr = self.control_center_rect();
            if cr.contains(mx, my) {
                self.handle_control_center_click(cr, mx, my);
                return;
            }
            self.close_control_center();
            if lay_click.status_btn.contains(mx, my) {
                return;
            }
        }

        if self.calendar_open {
            let cal_r = self.calendar_rect();
            if cal_r.contains(mx, my) {
                let set_btn = Rect::new(cal_r.x + 12, cal_r.bottom() - 32, cal_r.w - 24, 24);
                if set_btn.contains(mx, my) {
                    self.close_calendar();
                    self.open_builtin(BuiltinAppKind::Settings);
                }
                return;
            }
            self.close_calendar();
            if !lay_click.clock_btn.is_empty() && lay_click.clock_btn.contains(mx, my) {
                return;
            }
        }

        if self.power_menu_open {
            let pr = self.power_menu_rect();
            self.close_power_menu();
            if pr.contains(mx, my) {
                let row = ((my - pr.y - 6) / 26).max(0) as usize;
                match row {
                    0 => self.should_exit = true,
                    1 => {
                        let _ = personalization::flush_disk_sync();
                        let _ = crate::storage::vfs::sync();
                        crate::arch::acpi::system_reboot();
                    }
                    2 => {
                        let _ = personalization::flush_disk_sync();
                        let _ = crate::storage::vfs::sync();
                        crate::arch::acpi::system_shutdown();
                    }
                    _ => {}
                }
                return;
            }
            if lay_click.power_btn.contains(mx, my) {
                return;
            }
        }

        if my < TOP_BAR_HEIGHT as i32 {
            for (menu_kind, r, _) in self.top_menu_bar_rects() {
                if r.contains(mx, my) {
                    self.dismiss_all_context_menus(None);
                    self.open_menu = menu_kind;
                    let mr = self.dropdown_menu_rect(menu_kind);
                    self.compositor.invalidate_rect(mr.expanded(28));
                    self.invalidate_dock_and_topbar();
                    return;
                }
            }
        }

        if lay_click.capsule.contains(mx, my) {
            self.dismiss_all_context_menus(None);
            if lay_click.launcher_btn.contains(mx, my) || lay_click.search_btn.contains(mx, my) {
                self.spotlight.open = true;
                self.spotlight.query.clear();
                self.spotlight.rebuild_items();
                self.compositor.invalidate_rect(self.spotlight_rect().expanded(28));
                self.invalidate_dock_and_topbar();
                return;
            }
            if lay_click.status_btn.contains(mx, my) {
                self.control_center_open = true;
                self.compositor.invalidate_rect(self.control_center_rect().expanded(28));
                self.invalidate_dock_and_topbar();
                return;
            }
            if !lay_click.clock_btn.is_empty() && lay_click.clock_btn.contains(mx, my) {
                self.calendar_open = true;
                self.compositor.invalidate_rect(self.calendar_rect().expanded(28));
                self.invalidate_dock_and_topbar();
                return;
            }
            if lay_click.power_btn.contains(mx, my) {
                self.power_menu_open = true;
                self.compositor.invalidate_rect(self.power_menu_rect().expanded(28));
                self.invalidate_dock_and_topbar();
                return;
            }
            for (i, r) in lay_click.task_rects.iter().enumerate() {
                if r.contains(mx, my) {
                    match i {
                        0 => self.open_builtin(BuiltinAppKind::Files),
                        1 => self.open_builtin(BuiltinAppKind::Terminal),
                        2 => self.open_builtin(BuiltinAppKind::Editor),
                        3 => self.open_builtin(BuiltinAppKind::AudioPlayer),
                        4 => self.open_builtin(BuiltinAppKind::NetViewer),
                        5 => self.open_builtin(BuiltinAppKind::Settings),
                        6 => self.open_builtin(BuiltinAppKind::Monitor),
                        _ => {
                            if let Some(pos) = self.windows.iter().position(|w| w.external_user) {
                                self.focus_window(pos);
                            }
                        }
                    }
                    return;
                }
            }
            return;
        }

        let mut hit_idx: Option<usize> = None;
        for i in (0..self.windows.len()).rev() {
            let w = &self.windows[i];
            if !w.minimized && w.rect.contains(mx, my) {
                hit_idx = Some(i);
                break;
            }
        }

        let Some(idx) = hit_idx else {
            self.dismiss_all_context_menus(None);
            return;
        };

        let hit_id = self.windows[idx].id;
        self.dismiss_all_context_menus(Some(hit_id));
        self.focus_window(idx);
        let top_idx = self.windows.len() - 1;
        let win_rect = self.windows[top_idx].rect;
        let win_id = self.windows[top_idx].id;

        let (btn_min, btn_max, btn_close) = Self::window_control_rects(win_rect);

        if btn_close.contains(mx, my) {
            self.close_window(win_id);
            return;
        }
        if btn_min.contains(mx, my) {
            if let Some(w) = self.windows.get_mut(top_idx) {
                w.minimized = true;
                self.compositor.invalidate_rect(w.rect.expanded(28));
            }
            self.invalidate_dock_and_topbar();
            return;
        }
        if btn_max.contains(mx, my) {
            self.toggle_maximize(top_idx);
            return;
        }

        let title_rect = Rect::new(win_rect.x, win_rect.y, win_rect.w, WINDOW_TITLEBAR_HEIGHT as i32);
        if title_rect.contains(mx, my) {
            if let WindowContent::Files(ref mut f) = self.windows[top_idx].content {
                if f.dismiss_context_menu() {
                    self.windows[top_idx].surface_dirty = true;
                }
            }
            self.drag_mode = DragMode::Move {
                win_id,
                off_x: mx - win_rect.x,
                off_y: my - win_rect.y,
            };
            return;
        }

        let grip = Rect::new(win_rect.right() - 16, win_rect.bottom() - 16, 16, 16);
        if grip.contains(mx, my) && !self.windows[top_idx].external_user {
            self.drag_mode = DragMode::Resize {
                win_id,
                start_mx: mx,
                start_my: my,
                orig_w: win_rect.w,
                orig_h: win_rect.h,
            };
            return;
        }

        let content_rect = Rect::new(
            win_rect.x + 1,
            win_rect.y + WINDOW_TITLEBAR_HEIGHT as i32,
            win_rect.w - 2,
            win_rect.h - WINDOW_TITLEBAR_HEIGHT as i32 - 1,
        );
        if content_rect.contains(mx, my) {
            let act = match &mut self.windows[top_idx].content {
                WindowContent::Files(f) => f.on_click(content_rect, mx, my),
                WindowContent::Editor(e) => e.on_click(content_rect, mx, my),
                WindowContent::AudioPlayer(ap) => ap.on_click(content_rect, mx, my),
                WindowContent::NetViewer(nv) => nv.on_click(content_rect, mx, my),
                WindowContent::Settings(st) => st.on_click(content_rect, mx, my),
                WindowContent::Monitor(m) => m.on_click(content_rect, mx, my),
                WindowContent::ExternalUser { events, .. } => {
                    events.push(UserGuiEvent {
                        kind: 2,
                        p1: mx - content_rect.x,
                        p2: my - content_rect.y,
                        p3: 1,
                    });
                    DesktopAction::None
                }
                WindowContent::Terminal(_) => DesktopAction::None,
            };
            let is_files = matches!(self.windows[top_idx].content, WindowContent::Files(_));
            self.windows[top_idx].surface_dirty = true;
            if is_files {
                self.compositor.invalidate_rect(win_rect.expanded(24));
            } else {
                self.compositor.invalidate_rect(win_rect);
            }
            self.dispatch_action(act, allow_shell_spawn);
        }
    }

    fn handle_key_event(&mut self, evt: crate::input::KeyEvent, allow_shell_spawn: bool) {
        use crate::input::KeyCode;

        if alerts::has_active_modal() {
            if matches!(evt.code, KeyCode::Escape | KeyCode::Enter) {
                alerts::dismiss_modal();
                self.compositor.invalidate_all();
            }
            return;
        }

        if evt.code == KeyCode::Escape
            && (self.control_center_open || self.calendar_open || self.power_menu_open)
        {
            self.close_control_center();
            self.close_calendar();
            self.close_power_menu();
            return;
        }

        if evt.code == KeyCode::Super || (evt.modifiers.alt && evt.code == KeyCode::Char(' ')) {
            self.open_menu = TopMenu::None;
            self.desktop_menu = None;
            self.control_center_open = false;
            self.calendar_open = false;
            self.power_menu_open = false;
            self.spotlight.open = !self.spotlight.open;
            if self.spotlight.open {
                self.spotlight.query.clear();
                self.spotlight.rebuild_items();
            }
            self.compositor.invalidate_rect(self.spotlight_rect().expanded(28));
            self.invalidate_dock_and_topbar();
            return;
        }

        if self.spotlight.open {
            let sp = self.spotlight_rect();
            match evt.code {
                KeyCode::Escape => {
                    self.spotlight.open = false;
                }
                KeyCode::Up => {
                    self.spotlight.selected = self.spotlight.selected.saturating_sub(1);
                }
                KeyCode::Down => {
                    if self.spotlight.selected + 1 < self.spotlight.items.len() {
                        self.spotlight.selected += 1;
                    }
                }
                KeyCode::Enter => {
                    if let Some(item) = self.spotlight.items.get(self.spotlight.selected) {
                        let act = item.action.clone();
                        self.spotlight.open = false;
                        self.compositor.invalidate_rect(sp.expanded(28));
                        self.invalidate_dock_and_topbar();
                        self.dispatch_action(act, allow_shell_spawn);
                        return;
                    }
                }
                KeyCode::Backspace => {
                    self.spotlight.query.pop();
                    self.spotlight.rebuild_items();
                }
                KeyCode::Char(c) if c.is_ascii() && !c.is_ascii_control() => {
                    if self.spotlight.query.len() < 48 {
                        self.spotlight.query.push(c);
                        self.spotlight.rebuild_items();
                    }
                }
                _ => {}
            }
            self.compositor.invalidate_rect(sp.expanded(28));
            if !self.spotlight.open {
                self.invalidate_dock_and_topbar();
            }
            return;
        }

        if let Some(top) = self.windows.last_mut() {
            if !top.minimized {
                let win_rect = top.rect;
                match &mut top.content {
                    WindowContent::Terminal(t) => {
                        let act = t.on_key(evt);
                        top.surface_dirty = true;
                        self.compositor.invalidate_rect(win_rect);
                        self.dispatch_action(act, allow_shell_spawn);
                    }
                    WindowContent::Files(f) => {
                        let act = f.on_key(evt);
                        top.surface_dirty = true;
                        self.compositor.invalidate_rect(win_rect.expanded(24));
                        self.dispatch_action(act, allow_shell_spawn);
                    }
                    WindowContent::Editor(e) => {
                        let act = e.on_key(evt);
                        top.surface_dirty = true;
                        self.compositor.invalidate_rect(win_rect);
                        self.dispatch_action(act, allow_shell_spawn);
                    }
                    WindowContent::AudioPlayer(ap) => {
                        let act = ap.on_key(evt);
                        top.surface_dirty = true;
                        self.compositor.invalidate_rect(win_rect);
                        self.dispatch_action(act, allow_shell_spawn);
                    }
                    WindowContent::NetViewer(nv) => {
                        let act = nv.on_key(evt);
                        top.surface_dirty = true;
                        self.compositor.invalidate_rect(win_rect);
                        self.dispatch_action(act, allow_shell_spawn);
                    }
                    WindowContent::Settings(s) => {
                        let act = s.on_key(evt);
                        top.surface_dirty = true;
                        self.compositor.invalidate_rect(win_rect);
                        self.dispatch_action(act, allow_shell_spawn);
                    }
                    WindowContent::ExternalUser { events, .. } => {
                        if evt.code == KeyCode::Escape || evt.code == KeyCode::Char('q') {
                            events.push(UserGuiEvent { kind: 1, p1: 0, p2: 0, p3: 0 });
                        } else if let KeyCode::Char(c) = evt.code {
                            events.push(UserGuiEvent { kind: 3, p1: c as i32, p2: 0, p3: 0 });
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    fn handle_control_center_click(&mut self, cr: Rect, mx: i32, my: i32) {
        use crate::audio::mixer::{self, BuiltinTrack};

        let cfg = personalization::snapshot();
        let slots = control_center_slots(&cfg.desktop, cr);
        const TRACKS: [BuiltinTrack; 3] = [
            BuiltinTrack::StartupChime,
            BuiltinTrack::AmbientArp,
            BuiltinTrack::Reference440,
        ];

        for (module, r) in slots {
            if !r.contains(mx, my) {
                continue;
            }
            match module {
                ControlCenterModule::Network => {
                    if crate::drivers::e1000::is_ready() {
                        let dhcp_ok = crate::net::NET_STATE.lock().dhcp_configured;
                        if !dhcp_ok {
                            if let Ok(lease) = crate::net::dhcp::discover_and_configure() {
                                let msg = format!(
                                    "DHCP: {}.{}.{}.{}",
                                    lease.ip[0], lease.ip[1], lease.ip[2], lease.ip[3]
                                );
                                let tr = self.toast_rect(&msg).expanded(24);
                                self.toast = Some((msg, timer::ticks() + 220));
                                self.compositor.invalidate_rect(tr);
                            } else {
                                self.close_control_center();
                                self.open_builtin(BuiltinAppKind::NetViewer);
                            }
                        } else {
                            self.close_control_center();
                            self.open_builtin(BuiltinAppKind::NetViewer);
                        }
                    } else {
                        self.close_control_center();
                        self.open_builtin(BuiltinAppKind::NetViewer);
                    }
                    self.compositor.invalidate_rect(cr.expanded(24));
                    return;
                }
                ControlCenterModule::Audio => {
                    let cur_vol = mixer::snapshot().volume_percent;
                    if cur_vol > 0 {
                        self.saved_volume = cur_vol;
                        mixer::set_volume(0);
                    } else {
                        mixer::set_volume(self.saved_volume.max(50));
                    }
                    self.compositor.invalidate_rect(cr.expanded(24));
                    return;
                }
                ControlCenterModule::Theme => {
                    self.toggle_theme();
                    return;
                }
                ControlCenterModule::Volume => {
                    let bar_x = r.x + 10;
                    let bar_w = (r.w - 20).max(1);
                    let pct = (((mx - bar_x) * 100) / bar_w).clamp(0, 100) as u8;
                    if pct > 0 {
                        self.saved_volume = pct;
                    }
                    mixer::set_volume(pct);
                    self.compositor.invalidate_rect(cr.expanded(24));
                    return;
                }
                ControlCenterModule::MediaPlayer => {
                    let btn_prev = Rect::new(r.x + 10, r.y + 60, 26, 26);
                    let btn_play = Rect::new(r.x + 42, r.y + 58, 30, 30);
                    let btn_next = Rect::new(r.x + 78, r.y + 60, 26, 26);
                    if btn_prev.contains(mx, my) {
                        self.music_track_idx = (self.music_track_idx + TRACKS.len() - 1) % TRACKS.len();
                        mixer::play_builtin_async(TRACKS[self.music_track_idx]);
                    } else if btn_play.contains(mx, my) {
                        if mixer::is_active() {
                            mixer::pause_toggle();
                        } else {
                            mixer::play_builtin_async(TRACKS[self.music_track_idx]);
                        }
                    } else if btn_next.contains(mx, my) {
                        self.music_track_idx = (self.music_track_idx + 1) % TRACKS.len();
                        mixer::play_builtin_async(TRACKS[self.music_track_idx]);
                    } else {
                        self.close_control_center();
                        self.open_builtin(BuiltinAppKind::AudioPlayer);
                        return;
                    }
                    self.compositor.invalidate_rect(cr.expanded(24));
                    return;
                }
                ControlCenterModule::Power => {
                    self.should_exit = true;
                    return;
                }
            }
        }
    }

    fn dispatch_action(&mut self, act: DesktopAction, allow_shell_spawn: bool) {
        match act {
            DesktopAction::None => {}
            DesktopAction::OpenApp(kind) => self.open_builtin(kind),
            DesktopAction::OpenEditor(path) => self.open_editor_with_file(&path),
            DesktopAction::PlayWav(path) => self.open_audio_player_with_wav(&path),
            DesktopAction::Beep(freq, dur) => {
                crate::audio::mixer::play_tone_async(freq, dur);
            }
            DesktopAction::ToggleTheme => self.toggle_theme(),
            DesktopAction::SetTheme(mode) => self.apply_theme(mode),
            DesktopAction::SetWallpaper(preset) => self.apply_wallpaper(preset),
            DesktopAction::SetCustomWallpaper(path) => self.apply_custom_wallpaper(&path),
            DesktopAction::NextWallpaper => self.next_wallpaper(),
            DesktopAction::CloseDesktop => self.should_exit = true,
            DesktopAction::OpenLatestAlert => {
                alerts::open_latest_alert();
                self.compositor.invalidate_all();
            }
            other => {
                if allow_shell_spawn {
                    self.pending_actions.push(other);
                } else {
                    let msg = "Wait for active Ring 3 program to finish.";
                    let tr = self.toast_rect(msg).expanded(24);
                    self.toast = Some((String::from(msg), timer::ticks() + 250));
                    self.compositor.invalidate_rect(tr);
                }
            }
        }
    }

    fn try_render_frame(
        &mut self,
        renderer: &mut Renderer<'_>,
        allocator: Option<&PhysicalFrameAllocator<'_>>,
    ) {
        if !self.compositor.has_dirty() {
            return;
        }
        let now_ms = timer::uptime_ms();
        if now_ms.saturating_sub(self.last_frame_ms) < TARGET_FRAME_MS {
            return;
        }
        self.last_frame_ms = now_ms;
        self.fps_frames_rendered = self.fps_frames_rendered.saturating_add(1);
        self.render_frame(renderer, allocator);
    }

    fn refresh_dirty_surfaces(&mut self, allocator: Option<&PhysicalFrameAllocator<'_>>) {
        let blink_on = self.last_blink_on;
        let win_count = self.windows.len();
        let spotlight_open = self.spotlight.open;
        let fps_val = self.current_fps.min(60);
        let pal = theme::palette();

        for (i, win) in self.windows.iter_mut().enumerate() {
            if win.minimized || win.external_user {
                continue;
            }
            let cw = (win.rect.w - 2).max(0) as usize;
            let ch = (win.rect.h - WINDOW_TITLEBAR_HEIGHT as i32 - 1).max(0) as usize;
            if cw == 0 || ch == 0 {
                continue;
            }
            if !win.surface_dirty && win.surface_w == cw && win.surface_h == ch {
                continue;
            }

            let need_len = cw * ch;
            if win.surface_cache.len() != need_len {
                let bytes_needed = need_len.saturating_mul(4);
                if GLOBAL_ALLOCATOR.free_bytes() < bytes_needed + 2 * 1024 * 1024 {
                    win.surface_cache.clear();
                    win.surface_w = 0;
                    win.surface_h = 0;
                    continue;
                }
                win.surface_cache.resize(need_len, pal.win_bg_active);
            }
            win.surface_w = cw;
            win.surface_h = ch;

            let local_rect = Rect::new(0, 0, cw as i32, ch as i32);
            let mut wcanvas = Canvas::new(&mut win.surface_cache, cw, ch, local_rect);
            let focused = i + 1 == win_count && !spotlight_open;
            match &win.content {
                WindowContent::Terminal(t) => t.draw(&mut wcanvas, local_rect, blink_on, focused),
                WindowContent::Files(f) => f.draw(&mut wcanvas, local_rect),
                WindowContent::Editor(e) => e.draw(&mut wcanvas, local_rect, blink_on, focused),
                WindowContent::AudioPlayer(ap) => ap.draw(&mut wcanvas, local_rect),
                WindowContent::NetViewer(nv) => nv.draw(&mut wcanvas, local_rect),
                WindowContent::Settings(st) => st.draw(&mut wcanvas, local_rect),
                WindowContent::Monitor(m) => m.draw(&mut wcanvas, local_rect, allocator, fps_val),
                WindowContent::ExternalUser { .. } => {}
            }
            win.surface_dirty = false;
        }
    }

    fn render_frame(&mut self, renderer: &mut Renderer<'_>, allocator: Option<&PhysicalFrameAllocator<'_>>) {
        self.refresh_dirty_surfaces(allocator);

        let cfg = personalization::snapshot();
        let pal = theme::palette();

        let mut win_occluders: Vec<(usize, Rect)> = Vec::with_capacity(self.windows.len());
        let mut occ_rects: Vec<Rect> = Vec::with_capacity(self.windows.len());
        let rad_margin = (cfg.windows.corner_radius as i32).max(2) + 2;
        for (i, win) in self.windows.iter().enumerate() {
            if !win.minimized && win.anim_dur_ms == 0 && win.open_anim_dur_ms == 0 {
                let occ = Rect::new(
                    win.rect.x + 2,
                    win.rect.y + WINDOW_TITLEBAR_HEIGHT as i32,
                    (win.rect.w - 4).max(0),
                    (win.rect.h - WINDOW_TITLEBAR_HEIGHT as i32 - rad_margin).max(0),
                );
                if !occ.is_empty() {
                    win_occluders.push((i, occ));
                    occ_rects.push(occ);
                }
            }
        }
        self.compositor.begin_frame_with_occluders(&occ_rects);

        let sw = self.compositor.width() as i32;
        let sh = self.compositor.height() as i32;
        let now_ms = timer::uptime_ms();
        let mono_ui = cfg.typography.ui_font == UiFontFamily::JetBrainsMono;
        let ui_scale = cfg.typography.ui_scale_q8;
        let bar_scale = ui_scale.clamp(240, 276);
        let hdr_scale = cfg.typography.header_scale_q8.clamp(256, 304);

        let err_count = alerts::unread_error_count();
        let has_ext = self.has_external_windows();
        let cap_lay = self.capsule_layout(&cfg, has_ext, err_count);
        let dock_bbox = self.dock_invalidation_rect_with_cfg(&cfg);
        let files_open = self.windows.iter().any(|w| matches!(w.content, WindowContent::Files(_)));
        let term_open = self.windows.iter().any(|w| matches!(w.content, WindowContent::Terminal(_)));
        let ed_open = self.windows.iter().any(|w| matches!(w.content, WindowContent::Editor(_)));
        let audio_open = self.windows.iter().any(|w| matches!(w.content, WindowContent::AudioPlayer(_)));
        let net_open = self.windows.iter().any(|w| matches!(w.content, WindowContent::NetViewer(_)));
        let set_open = self.windows.iter().any(|w| matches!(w.content, WindowContent::Settings(_)));
        let mon_open = self.windows.iter().any(|w| matches!(w.content, WindowContent::Monitor(_)));
        let focused_task_idx = self
            .windows
            .last()
            .filter(|w| !w.minimized && !self.spotlight.open)
            .map(|w| match w.content {
                WindowContent::Files(_) => 0,
                WindowContent::Terminal(_) => 1,
                WindowContent::Editor(_) => 2,
                WindowContent::AudioPlayer(_) => 3,
                WindowContent::NetViewer(_) => 4,
                WindowContent::Settings(_) => 5,
                WindowContent::Monitor(_) => 6,
                WindowContent::ExternalUser { .. } => 7,
            });

        let spotlight_rect = self.spotlight_rect();
        let menu_rects = self.top_menu_bar_rects();
        let top_menu_bbox = self.top_menu_invalidation_rect();
        let open_menu_rect = if self.open_menu != TopMenu::None {
            Some((self.open_menu, self.dropdown_menu_rect(self.open_menu)))
        } else {
            None
        };
        let desktop_ctx_rect = self.desktop_context_rect();
        let snap_rect = if self.snap_zone != SnapZone::None {
            Some(self.snap_target_rect(self.snap_zone))
        } else {
            None
        };

        let net_up = crate::drivers::e1000::is_ready();
        let snd_up = crate::drivers::ac97::is_ready();
        let mixer_snap = crate::audio::mixer::snapshot();
        let rtc_dt = self.rtc_now;
        let clock_txt = self.format_clock_text(&cfg.desktop);
        let fps_val = self.current_fps.min(60);
        let blink_on = self.last_blink_on;

        let cc_rect = self.control_center_rect();
        let cc_open = self.control_center_open;
        let cal_rect = self.calendar_rect();
        let cal_open = self.calendar_open;
        let pwr_rect = self.power_menu_rect();
        let pwr_open = self.power_menu_open;

        let damage_rects = self.compositor.damage_list();

        for clip_r in damage_rects {
            let mut canvas = self.compositor.canvas_clipped(clip_r);

            // 1. Visible windows back-to-front (skip occluded lower windows & chrome when interior-only)
            let min_visible_idx = win_occluders
                .iter()
                .rev()
                .find(|(_, occ)| rect_contains_rect(*occ, clip_r))
                .map(|(idx, _)| *idx)
                .unwrap_or(0);
            let win_count = self.windows.len();
            for (i, win) in self.windows.iter().enumerate() {
                if win.minimized || i < min_visible_idx {
                    continue;
                }
                let mut draw_rect = if win.anim_dur_ms > 0 {
                    let q = ease_out_q8(now_ms.saturating_sub(win.anim_start_ms), win.anim_dur_ms);
                    lerp_rect(win.anim_from, win.anim_to, q)
                } else {
                    win.rect
                };
                if win.open_anim_dur_ms > 0 {
                    let q = ease_out_q8(
                        now_ms.saturating_sub(win.open_anim_start_ms),
                        win.open_anim_dur_ms,
                    );
                    draw_rect.y += ((256 - q) * 14) >> 8;
                }
                if draw_rect.expanded(32).intersect(clip_r).is_empty() {
                    continue;
                }

                let focused = i + 1 == win_count && !self.spotlight.open;
                let rad = (cfg.windows.corner_radius as i32)
                    .min(draw_rect.w / 2)
                    .min(draw_rect.h / 2);
                let occluded_interior = win_occluders
                    .iter()
                    .any(|(oi, occ)| *oi == i && rect_contains_rect(*occ, clip_r));

                if !occluded_interior {
                    if cfg.windows.shadows_enabled && cfg.windows.shadow_intensity > 0 {
                        let base_a = if focused {
                            if pal.is_light { 74 } else { 98 }
                        } else if pal.is_light {
                            36
                        } else {
                            48
                        };
                        let eff_a = ((base_a * (cfg.windows.shadow_intensity as i32)) / 78)
                            .clamp(4, 150) as u8;
                        let layers = if focused { 5 } else { 3 };
                        canvas.draw_soft_shadow(draw_rect, rad, layers, eff_a);
                    }

                    let bg = if focused { pal.win_bg_active } else { pal.win_bg_inactive };
                    let tbg = if focused { pal.win_title_active } else { pal.win_title_inactive };

                    canvas.fill_rounded_rect(draw_rect, rad, bg);
                    canvas.fill_top_rounded_rect(
                        Rect::new(draw_rect.x, draw_rect.y, draw_rect.w, WINDOW_TITLEBAR_HEIGHT as i32),
                        rad,
                        tbg,
                    );
                    canvas.fill_rect(
                        Rect::new(
                            draw_rect.x + 1,
                            draw_rect.y + WINDOW_TITLEBAR_HEIGHT as i32 - 1,
                            draw_rect.w - 2,
                            1,
                        ),
                        pal.win_divider,
                    );

                    // Custom AmaryllisOS Window Chrome: Mini App Icon + Left Title + Right Geometric Controls
                    let cy = draw_rect.y + (WINDOW_TITLEBAR_HEIGHT as i32) / 2;
                    let mini_ico = Self::window_mini_icon(&win.content);
                    canvas.draw_lucide_icon(
                        draw_rect.x + 12,
                        cy - 8,
                        mini_ico,
                        if focused { pal.accent_primary } else { pal.text_secondary },
                    );

                    let title_clip = Rect::new(
                        draw_rect.x + 32,
                        draw_rect.y,
                        (draw_rect.w - 122).max(16),
                        WINDOW_TITLEBAR_HEIGHT as i32,
                    );
                    canvas.draw_ui_text_clipped(
                        draw_rect.x + 34,
                        draw_rect.y + 7,
                        &win.title,
                        if focused { pal.text_primary } else { pal.text_secondary },
                        hdr_scale,
                        true,
                        mono_ui,
                        title_clip,
                    );

                    let (btn_min, btn_max, btn_close) = Self::window_control_rects(draw_rect);
                    let hov_min = self.hovered_win_btn == Some((win.id, WindowBtn::Minimize));
                    let hov_max = self.hovered_win_btn == Some((win.id, WindowBtn::Maximize));
                    let hov_cls = self.hovered_win_btn == Some((win.id, WindowBtn::Close));

                    let base_ico_col = if focused { pal.text_primary } else { pal.text_secondary };
                    if hov_min {
                        canvas.fill_rounded_rect_alpha(btn_min, 6, pal.surface_hover, 210);
                    }
                    canvas.fill_rect(
                        Rect::new(btn_min.x + 7, btn_min.y + 11, 10, 2),
                        base_ico_col,
                    );

                    if hov_max {
                        canvas.fill_rounded_rect_alpha(btn_max, 6, pal.surface_hover, 210);
                    }
                    canvas.stroke_rounded_rect(
                        Rect::new(btn_max.x + 7, btn_max.y + 6, 10, 10),
                        1,
                        base_ico_col,
                        245,
                    );
                    if win.maximized {
                        canvas.fill_rect(
                            Rect::new(btn_max.x + 9, btn_max.y + 8, 6, 6),
                            base_ico_col,
                        );
                    }

                    if hov_cls {
                        canvas.fill_rounded_rect_alpha(btn_close, 6, ACCENT_ERR, 220);
                    }
                    let x_col = if hov_cls { 0xFFFF_FFFF } else { base_ico_col };
                    canvas.draw_line_aa(
                        btn_close.x + 8,
                        btn_close.y + 7,
                        btn_close.x + 16,
                        btn_close.y + 15,
                        x_col,
                        150,
                    );
                    canvas.draw_line_aa(
                        btn_close.x + 16,
                        btn_close.y + 7,
                        btn_close.x + 8,
                        btn_close.y + 15,
                        x_col,
                        150,
                    );
                }

                let content = Rect::new(
                    draw_rect.x + 1,
                    draw_rect.y + WINDOW_TITLEBAR_HEIGHT as i32,
                    (draw_rect.w - 2).max(0),
                    (draw_rect.h - WINDOW_TITLEBAR_HEIGHT as i32 - 1).max(0),
                );

                if !content.intersect(clip_r).is_empty() {
                    if let WindowContent::ExternalUser { width, height, pixels, shm_id, .. } = &win.content {
                        if !occluded_interior {
                            canvas.fill_rect(content, 0xFF0B0B0E);
                        }
                        let copy_r = Rect::new(
                            content.x,
                            content.y,
                            (*width as i32).min(content.w),
                            (*height as i32).min(content.h),
                        );
                        if !occluded_interior && (*width as i32) < content.w {
                            canvas.fill_rect(
                                Rect::new(content.x + (*width as i32), content.y, content.w - (*width as i32), content.h),
                                0xFF0B0B0E,
                            );
                        }
                        if !occluded_interior && (*height as i32) < content.h {
                            canvas.fill_rect(
                                Rect::new(content.x, content.y + (*height as i32), content.w, content.h - (*height as i32)),
                                0xFF0B0B0E,
                            );
                        }
                        if let Some(sid) = *shm_id {
                            let hhdm_off = ACTIVE_HHDM_OFFSET.load(Ordering::Acquire);
                            if let Some(hhdm_map) = PhysicalMemoryMapping::new(hhdm_off) {
                                canvas.blit_shm_surface(copy_r, sid, &hhdm_map, *width);
                            }
                        } else {
                            canvas.blit_surface(copy_r, pixels, *width);
                        }
                    } else if !win.surface_cache.is_empty() && win.surface_w > 0 && win.surface_h > 0 {
                        let copy_r = Rect::new(
                            content.x,
                            content.y,
                            content.w.min(win.surface_w as i32),
                            content.h.min(win.surface_h as i32),
                        );
                        canvas.blit_surface(copy_r, &win.surface_cache, win.surface_w);
                    } else {
                        match &win.content {
                            WindowContent::Terminal(t) => t.draw(&mut canvas, content, blink_on, focused),
                            WindowContent::Files(f) => f.draw(&mut canvas, content),
                            WindowContent::Editor(e) => e.draw(&mut canvas, content, blink_on, focused),
                            WindowContent::AudioPlayer(ap) => ap.draw(&mut canvas, content),
                            WindowContent::NetViewer(nv) => nv.draw(&mut canvas, content),
                            WindowContent::Settings(st) => st.draw(&mut canvas, content),
                            WindowContent::Monitor(m) => m.draw(&mut canvas, content, allocator, fps_val),
                            WindowContent::ExternalUser { .. } => {}
                        }
                    }
                }

                if !occluded_interior {
                    let border = if focused { pal.win_border_active } else { pal.win_border_inactive };
                    canvas.stroke_rounded_rect(
                        Rect::new(draw_rect.x + 1, draw_rect.y + 1, draw_rect.w - 2, draw_rect.h - 2),
                        (rad - 1).max(0),
                        0xFFFF_FFFF,
                        if pal.is_light { 120 } else if focused { 28 } else { 14 },
                    );
                    if focused && cfg.windows.active_highlight {
                        canvas.stroke_rounded_rect(
                            draw_rect.expanded(1),
                            (rad + 1).max(0),
                            pal.accent_primary,
                            92,
                        );
                        canvas.stroke_rounded_rect(draw_rect, rad, pal.accent_primary, 220);
                    } else {
                        canvas.stroke_rounded_rect(draw_rect, rad, border, 210);
                    }
                }
            }

            // 1b. Closing window fade/shrink animation ghosts
            for g in &self.closing_ghosts {
                if g.rect.expanded(24).intersect(clip_r).is_empty() {
                    continue;
                }
                let q = ease_out_q8(now_ms.saturating_sub(g.start_ms), g.dur_ms);
                let shrink = (q * 18) >> 8;
                let gr = Rect::new(
                    g.rect.x + shrink / 2,
                    g.rect.y + ((q * 14) >> 8),
                    (g.rect.w - shrink).max(24),
                    (g.rect.h - shrink).max(24),
                );
                let alpha = (180 - ((q * 180) >> 8)).clamp(0, 200) as u8;
                let r_eff = g.rad.min(gr.w / 2).min(gr.h / 2);
                canvas.fill_rounded_rect_alpha(gr, r_eff, g.bg, alpha);
                canvas.fill_rounded_rect_alpha(
                    Rect::new(gr.x, gr.y, gr.w, WINDOW_TITLEBAR_HEIGHT.min(gr.h as usize) as i32),
                    r_eff,
                    g.tbg,
                    alpha,
                );
                canvas.stroke_rounded_rect(gr, r_eff, pal.accent_primary, alpha);
            }

            // 2. Window Snap translucent preview overlay
            if let Some(sr) = snap_rect {
                if !sr.intersect(clip_r).is_empty() {
                    canvas.fill_rounded_rect_alpha(sr, 12, SNAP_OVERLAY_FILL, 44);
                    canvas.stroke_rounded_rect(sr, 12, SNAP_OVERLAY_BORDER, 210);
                }
            }

            // 3. Borderless Top-Left App Menu (directly over wallpaper; translucent pill ONLY on hover/open)
            if !top_menu_bbox.intersect(clip_r).is_empty() {
                for (idx, (menu_kind, r, label)) in menu_rects.iter().enumerate() {
                    let is_open = self.open_menu == *menu_kind;
                    let hovered = self.hovered_top_menu == *menu_kind || r.contains(self.cursor_x, self.cursor_y);
                    if is_open || hovered {
                        canvas.fill_rounded_rect_alpha(
                            *r,
                            7,
                            if is_open { pal.accent_primary } else { pal.surface_hover },
                            if is_open { 215 } else { 155 },
                        );
                    }
                    let txt_col = if is_open { pal.on_accent } else { pal.topbar_text };
                    if idx == 0 {
                        let app_ico = match self.windows.last().filter(|w| !w.minimized) {
                            Some(w) => Self::window_mini_icon(&w.content),
                            None => LucideIcon::Settings,
                        };
                        canvas.draw_lucide_icon(
                            r.x + 6,
                            r.y + 4,
                            app_ico,
                            if is_open { pal.on_accent } else { pal.topbar_text },
                        );
                        canvas.draw_ui_text_clipped(
                            r.x + 26,
                            6,
                            label,
                            txt_col,
                            bar_scale,
                            true,
                            mono_ui,
                            *r,
                        );
                    } else {
                        canvas.draw_ui_text_clipped(
                            r.x + 8,
                            6,
                            label,
                            txt_col,
                            bar_scale,
                            false,
                            mono_ui,
                            *r,
                        );
                    }
                }
            }

            // 4. Unified Bottom Floating «Amaryllis Capsule» (3 zones: Launcher | Taskbar | System Island)
            let dock_rect = cap_lay.capsule;
            let dock_visible_on_screen = dock_rect.right() > 0
                && dock_rect.x < sw
                && dock_rect.bottom() > 0
                && dock_rect.y < sh;
            if dock_visible_on_screen && !dock_bbox.intersect(clip_r).is_empty() {
                canvas.draw_soft_shadow(dock_rect, DOCK_RADIUS as i32, 5, if pal.is_light { 58 } else { 96 });
                canvas.draw_glass_panel(
                    dock_rect,
                    DOCK_RADIUS as i32,
                    pal.dock_bg,
                    pal.dock_alpha,
                    pal.dock_border,
                );

                // Zone 1: App Launcher (3x3 dots button)
                let launch_hov = self.hovered_capsule == CapsuleHit::Launcher || self.spotlight.open;
                canvas.fill_rounded_rect_alpha(
                    cap_lay.launcher_btn,
                    10,
                    if launch_hov { pal.surface_hover } else { pal.surface_card },
                    if launch_hov { 220 } else { 145 },
                );
                canvas.stroke_rounded_rect(cap_lay.launcher_btn, 10, pal.dock_border, 150);
                canvas.draw_app_grid_icon(
                    cap_lay.launcher_btn.x + (cap_lay.launcher_btn.w - 16) / 2,
                    cap_lay.launcher_btn.y + (cap_lay.launcher_btn.h - 16) / 2,
                    pal.topbar_text,
                );
                if launch_hov {
                    let tip = "Applications (Alt+Space)";
                    let tip_w = ui_text_width(tip, bar_scale, true, mono_ui) + 18;
                    let tx = (cap_lay.launcher_btn.x + cap_lay.launcher_btn.w / 2 - tip_w / 2)
                        .clamp(6, sw - tip_w - 6);
                    let tip_r = Rect::new(tx, dock_rect.y - 32, tip_w, 24);
                    canvas.draw_soft_shadow(tip_r, 7, 4, 62);
                    canvas.fill_rounded_rect_alpha(tip_r, 7, pal.modal_bg, pal.modal_alpha);
                    canvas.stroke_rounded_rect(tip_r, 7, pal.modal_border, 190);
                    canvas.draw_ui_text_clipped(
                        tip_r.x + 9,
                        tip_r.y + 4,
                        tip,
                        pal.text_primary,
                        bar_scale,
                        true,
                        mono_ui,
                        tip_r,
                    );
                }

                // Divider 1 | 2
                canvas.fill_rect_alpha(
                    Rect::new(cap_lay.div1_x, dock_rect.y + 12, 1, (dock_rect.h - 24).max(8)),
                    pal.dock_divider,
                    195,
                );

                // Zone 2: Taskbar (7 builtin apps + dynamic external Ring 3 app tile)
                let open_flags = [
                    files_open,
                    term_open,
                    ed_open,
                    audio_open,
                    net_open,
                    set_open,
                    mon_open,
                    has_ext,
                ];
                let mag_active = cfg.desktop.dock_magnification && cfg.effective_dock_animations();
                let hov_task = match self.hovered_capsule {
                    CapsuleHit::Task(idx) => Some(idx),
                    _ => None,
                };
                for (i, r) in cap_lay.task_rects.iter().enumerate() {
                    let hovered = hov_task == Some(i);
                    if hovered {
                        canvas.fill_rounded_rect_alpha(r.expanded(3), 10, pal.surface_hover, 150);
                    }
                    let boost = if mag_active {
                        if let Some(hi) = hov_task {
                            let dist = if i >= hi { i - hi } else { hi - i };
                            match dist {
                                0 => 6,
                                1 => 2,
                                _ => 0,
                            }
                        } else {
                            0
                        }
                    } else {
                        0
                    };
                    let draw_r = Rect::new(
                        r.x - boost / 2,
                        r.y - boost / 2,
                        r.w + boost,
                        r.h + boost,
                    );
                    draw_dock_tile_scaled(&mut canvas, draw_r, i, hovered);

                    if cfg.desktop.dock_show_indicators && open_flags.get(i).copied().unwrap_or(false) {
                        let is_focused = focused_task_idx == Some(i);
                        let bar_w = if is_focused { 22 } else { 12 };
                        let bar_r = Rect::new(
                            r.x + (r.w - bar_w) / 2,
                            dock_rect.bottom() - 5,
                            bar_w,
                            3,
                        );
                        canvas.fill_rounded_rect_alpha(
                            bar_r,
                            1,
                            if is_focused { pal.accent_primary } else { pal.topbar_muted },
                            if is_focused { 245 } else { 195 },
                        );
                    }

                    if hovered {
                        let tip = match i {
                            0 => "Files",
                            1 => "Terminal",
                            2 => "Text Editor",
                            3 => "Audio Player",
                            4 => "Net Viewer",
                            5 => "Settings",
                            6 => "Activity Monitor",
                            _ => "Ring 3 App",
                        };
                        let tip_w = ui_text_width(tip, bar_scale, true, mono_ui) + 18;
                        let tx = (r.x + r.w / 2 - tip_w / 2).clamp(6, sw - tip_w - 6);
                        let tip_r = Rect::new(tx, dock_rect.y - 32, tip_w, 24);
                        canvas.draw_soft_shadow(tip_r, 7, 4, 62);
                        canvas.fill_rounded_rect_alpha(tip_r, 7, pal.modal_bg, pal.modal_alpha);
                        canvas.stroke_rounded_rect(tip_r, 7, pal.modal_border, 190);
                        canvas.draw_ui_text_clipped(
                            tip_r.x + 9,
                            tip_r.y + 4,
                            tip,
                            pal.text_primary,
                            bar_scale,
                            true,
                            mono_ui,
                            tip_r,
                        );
                    }
                }

                // Divider 2 | 3
                canvas.fill_rect_alpha(
                    Rect::new(cap_lay.div2_x, dock_rect.y + 12, 1, (dock_rect.h - 24).max(8)),
                    pal.dock_divider,
                    195,
                );

                // Zone 3: System Island (Search | Status Cluster -> Control Center | Clock -> Calendar | Power)
                let search_hov = self.hovered_capsule == CapsuleHit::Search || self.spotlight.open;
                if search_hov {
                    canvas.fill_rounded_rect_alpha(
                        cap_lay.search_btn,
                        8,
                        if self.spotlight.open { pal.accent_primary } else { pal.surface_hover },
                        if self.spotlight.open { 205 } else { 160 },
                    );
                }
                canvas.draw_lucide_icon(
                    cap_lay.search_btn.x + 7,
                    cap_lay.search_btn.y + 8,
                    LucideIcon::Search,
                    if self.spotlight.open { pal.on_accent } else { pal.topbar_text },
                );

                let status_hov = self.hovered_capsule == CapsuleHit::Status || cc_open;
                if status_hov {
                    canvas.fill_rounded_rect_alpha(
                        cap_lay.status_btn,
                        8,
                        if cc_open { pal.accent_primary } else { pal.surface_hover },
                        if cc_open { 205 } else { 160 },
                    );
                }
                let mut sx = cap_lay.status_btn.x + 6;
                let sy = cap_lay.status_btn.y + 8;
                if cfg.desktop.topbar_show_network {
                    let ncol = if cc_open {
                        pal.on_accent
                    } else if net_up {
                        pal.topbar_text
                    } else {
                        pal.topbar_muted
                    };
                    canvas.draw_lucide_icon(sx, sy, LucideIcon::Wifi, ncol);
                    sx += 22;
                }
                if cfg.desktop.topbar_show_audio {
                    let scol = if cc_open {
                        pal.on_accent
                    } else if snd_up && mixer_snap.volume_percent > 0 {
                        pal.topbar_text
                    } else {
                        pal.topbar_muted
                    };
                    canvas.draw_lucide_icon(sx, sy, LucideIcon::Volume2, scol);
                    sx += 22;
                }
                if cfg.desktop.topbar_show_notifications || err_count > 0 {
                    let bcol = if cc_open { pal.on_accent } else { pal.topbar_text };
                    canvas.draw_lucide_icon(sx, sy, LucideIcon::Bell, bcol);
                    if err_count > 0 {
                        canvas.draw_circle(sx + 13, sy + 2, 3, ACCENT_ERR);
                    }
                    sx += 22;
                }
                canvas.draw_battery_icon(
                    sx,
                    sy,
                    if cc_open { pal.on_accent } else { pal.topbar_text },
                );

                if !cap_lay.clock_btn.is_empty() && !clock_txt.is_empty() {
                    let clk_hov = self.hovered_capsule == CapsuleHit::Clock || cal_open;
                    if clk_hov {
                        canvas.fill_rounded_rect_alpha(
                            cap_lay.clock_btn,
                            8,
                            if cal_open { pal.accent_primary } else { pal.surface_hover },
                            if cal_open { 205 } else { 160 },
                        );
                    }
                    canvas.draw_ui_text_clipped(
                        cap_lay.clock_btn.x + 7,
                        cap_lay.clock_btn.y + 8,
                        &clock_txt,
                        if cal_open { pal.on_accent } else { pal.topbar_text },
                        bar_scale,
                        false,
                        mono_ui,
                        cap_lay.clock_btn,
                    );
                }

                let pwr_hov = self.hovered_capsule == CapsuleHit::Power || pwr_open;
                if pwr_hov {
                    canvas.fill_rounded_rect_alpha(
                        cap_lay.power_btn,
                        8,
                        if pwr_open { pal.accent_primary } else { pal.surface_hover },
                        if pwr_open { 205 } else { 160 },
                    );
                }
                canvas.draw_lucide_icon(
                    cap_lay.power_btn.x + 7,
                    cap_lay.power_btn.y + 8,
                    LucideIcon::Power,
                    if pwr_open { pal.on_accent } else { pal.topbar_text },
                );
            }

            // 5. Dropdown Menu Bar Popover
            if let Some((menu_kind, mrect)) = open_menu_rect {
                if !mrect.expanded(24).intersect(clip_r).is_empty() {
                    canvas.draw_soft_shadow(mrect, 10, 5, 76);
                    canvas.draw_glass_panel(mrect, 10, pal.modal_bg, pal.modal_alpha, pal.modal_border);

                    let mut labels: Vec<String> = Vec::new();
                    match menu_kind {
                        TopMenu::Amaryllis => {
                            labels.push(String::from("Spotlight Search (Alt+Space)"));
                            labels.push(String::from("System Settings..."));
                            labels.push(String::from(if pal.is_light {
                                "Switch to Black Theme"
                            } else {
                                "Switch to White Theme"
                            }));
                            labels.push(String::from("Activity Monitor"));
                            labels.push(String::from("About AmaryllisOS"));
                            labels.push(String::from("Run /gui_demo.elf"));
                            labels.push(String::from("Exit to Text Console"));
                        }
                        TopMenu::File => {
                            labels.push(String::from("New Text File"));
                            labels.push(String::from("New Folder..."));
                            labels.push(String::from("Save Document (Ctrl+S)"));
                            labels.push(String::from("Refresh Files"));
                            labels.push(String::from("Close Active Window"));
                        }
                        TopMenu::Edit => {
                            labels.push(String::from("Copy (Ctrl+C)"));
                            labels.push(String::from("Paste (Ctrl+V)"));
                            labels.push(String::from("Cut / Select All"));
                            labels.push(String::from("Rename (F2) / Clear"));
                        }
                        TopMenu::View => {
                            labels.push(String::from("Tile Left Half"));
                            labels.push(String::from("Tile Right Half"));
                            labels.push(String::from("Maximize / Restore"));
                            labels.push(String::from("Minimize Window"));
                            labels.push(String::from("Toggle Black / White Theme"));
                        }
                        TopMenu::Window => {
                            if self.windows.is_empty() {
                                labels.push(String::from("(No open windows)"));
                            } else {
                                for (wi, w) in self.windows.iter().take(7).enumerate() {
                                    let mark = if w.minimized {
                                        "[min] "
                                    } else if wi + 1 == self.windows.len() {
                                        "* "
                                    } else {
                                        "  "
                                    };
                                    labels.push(format!("{}{}", mark, w.title));
                                }
                            }
                        }
                        TopMenu::None => {}
                    }

                    for (mi, label) in labels.iter().enumerate() {
                        let ry = mrect.y + 6 + (mi as i32) * 26;
                        let row_r = Rect::new(mrect.x + 6, ry, mrect.w - 12, 24);
                        if row_r.contains(self.cursor_x, self.cursor_y) {
                            canvas.fill_rounded_rect(row_r, 5, pal.modal_hover);
                        }
                        canvas.draw_ui_text_clipped(
                            row_r.x + 10,
                            row_r.y + 4,
                            label,
                            pal.text_primary,
                            bar_scale,
                            false,
                            mono_ui,
                            row_r,
                        );
                    }
                }
            }

            // 5b. Slide-out Control Center Popover (anchored upward above Zone 3)
            if cc_open && !cc_rect.expanded(24).intersect(clip_r).is_empty() {
                draw_control_center_popover(
                    &mut canvas,
                    &pal,
                    &cfg,
                    cc_rect,
                    net_up,
                    snd_up,
                    &mixer_snap,
                );
            }

            // 5c. Calendar Popover (anchored upward above Capsule Clock)
            if cal_open && !cal_rect.expanded(24).intersect(clip_r).is_empty() {
                draw_calendar_popover(&mut canvas, &pal, &cfg, cal_rect, rtc_dt);
            }

            // 5d. Power / Session Popover (anchored upward above Capsule Power button)
            if pwr_open && !pwr_rect.expanded(24).intersect(clip_r).is_empty() {
                draw_power_menu_popover(
                    &mut canvas,
                    &pal,
                    &cfg,
                    pwr_rect,
                    self.cursor_x,
                    self.cursor_y,
                );
            }

            // 6. Desktop Right-Click Context Menu
            if let Some(dmr) = desktop_ctx_rect {
                if !dmr.expanded(24).intersect(clip_r).is_empty() {
                    canvas.draw_soft_shadow(dmr, 10, 5, 74);
                    canvas.draw_glass_panel(dmr, 10, pal.modal_bg, pal.modal_alpha, pal.modal_border);
                    let items: [(&str, LucideIcon); 6] = [
                        ("New Text File", LucideIcon::FilePlus),
                        ("New Terminal", LucideIcon::Terminal),
                        ("Open Files", LucideIcon::Folder),
                        ("Next Wallpaper", LucideIcon::LayoutGrid),
                        (
                            if pal.is_light { "Switch to Black Theme" } else { "Switch to White Theme" },
                            if pal.is_light { LucideIcon::Moon } else { LucideIcon::Sun },
                        ),
                        ("Spotlight Search", LucideIcon::Search),
                    ];
                    for (idx, (txt, ico)) in items.iter().enumerate() {
                        let row_r = Rect::new(dmr.x + 6, dmr.y + 7 + (idx as i32) * 26, dmr.w - 12, 24);
                        let hov = row_r.contains(self.cursor_x, self.cursor_y);
                        if hov {
                            canvas.fill_rounded_rect(row_r, 5, pal.modal_hover);
                        }
                        canvas.draw_lucide_icon(
                            row_r.x + 8,
                            row_r.y + 4,
                            *ico,
                            if hov { pal.accent_primary } else { pal.text_secondary },
                        );
                        canvas.draw_ui_text_clipped(
                            row_r.x + 30,
                            row_r.y + 4,
                            txt,
                            pal.text_primary,
                            bar_scale,
                            false,
                            mono_ui,
                            row_r,
                        );
                    }
                }
            }

            // 7. Spotlight Quick Launcher Modal
            if self.spotlight.open && !spotlight_rect.expanded(24).intersect(clip_r).is_empty() {
                canvas.draw_soft_shadow(spotlight_rect, 14, 6, 86);
                canvas.draw_glass_panel(spotlight_rect, 14, pal.modal_bg, pal.modal_alpha, pal.accent_primary);

                let input_box = Rect::new(
                    spotlight_rect.x + 14,
                    spotlight_rect.y + 14,
                    spotlight_rect.w - 28,
                    36,
                );
                canvas.fill_rounded_rect(input_box, 8, pal.modal_input_bg);
                canvas.stroke_rounded_rect(input_box, 8, pal.accent_primary, 190);
                canvas.draw_lucide_icon(input_box.x + 10, input_box.y + 10, LucideIcon::Search, pal.text_muted);
                let qtxt = if self.spotlight.query.is_empty() {
                    "Search apps, .elf binaries, or commands..."
                } else {
                    &self.spotlight.query
                };
                canvas.draw_ui_text_clipped(
                    input_box.x + 32,
                    input_box.y + 10,
                    qtxt,
                    if self.spotlight.query.is_empty() { pal.text_muted } else { pal.text_primary },
                    bar_scale,
                    false,
                    mono_ui,
                    input_box,
                );

                for (ri, item) in self.spotlight.items.iter().take(6).enumerate() {
                    let row_r = Rect::new(
                        spotlight_rect.x + 14,
                        spotlight_rect.y + 62 + (ri as i32) * 44,
                        spotlight_rect.w - 28,
                        40,
                    );
                    if ri == self.spotlight.selected {
                        canvas.fill_rounded_rect(row_r, 7, pal.surface_selected);
                        canvas.stroke_rounded_rect(row_r, 7, pal.accent_primary, 170);
                    }
                    canvas.draw_ui_text_clipped(
                        row_r.x + 10,
                        row_r.y + 3,
                        &item.label,
                        pal.text_primary,
                        bar_scale,
                        true,
                        mono_ui,
                        row_r,
                    );
                    canvas.draw_ui_text_clipped(
                        row_r.x + 10,
                        row_r.y + 21,
                        &item.subtitle,
                        pal.text_secondary,
                        ui_scale.clamp(240, 260),
                        false,
                        mono_ui,
                        row_r,
                    );
                }
            }

            // 8. Toast banner
            if let Some((ref msg, _)) = self.toast {
                let tw = ui_text_width(msg, bar_scale, false, mono_ui) + 28;
                let tr = Rect::new((sw - tw) / 2, TOP_BAR_HEIGHT as i32 + 10, tw, 28);
                if !tr.intersect(clip_r).is_empty() {
                    canvas.draw_soft_shadow(tr, 9, 4, 54);
                    canvas.fill_rounded_rect_alpha(tr, 9, pal.modal_bg, pal.modal_alpha);
                    canvas.stroke_rounded_rect(tr, 9, pal.modal_border, 190);
                    canvas.draw_ui_text_clipped(
                        tr.x + 14,
                        tr.y + 6,
                        msg,
                        pal.text_primary,
                        bar_scale,
                        false,
                        mono_ui,
                        tr,
                    );
                }
            }

            // 9. Enhanced System Error Modal
            alerts::draw_modal_alert(&mut canvas);
        }

        self.compositor
            .draw_cursor_and_flush(renderer, self.cursor_x, self.cursor_y);
    }
}

/// Renders the Control Center popover panel dynamically respecting module visibility and order.
fn draw_control_center_popover(
    canvas: &mut Canvas<'_>,
    pal: &ThemePalette,
    cfg: &SystemAppearance,
    cr: Rect,
    net_up: bool,
    snd_up: bool,
    mixer_snap: &crate::audio::mixer::MixerSnapshot,
) {
    let mono_ui = cfg.typography.ui_font == UiFontFamily::JetBrainsMono;
    let ui_scale = cfg.typography.ui_scale_q8.clamp(240, 268);

    canvas.draw_soft_shadow(cr, 14, 6, if pal.is_light { 58 } else { 96 });
    canvas.draw_glass_panel(
        cr,
        14,
        pal.modal_bg,
        if pal.is_light { 232 } else { 226 },
        pal.modal_border,
    );

    // Header: active profile & identity
    let av_cx = cr.x + 26;
    let av_cy = cr.y + 23;
    canvas.draw_circle(av_cx, av_cy, 12, pal.accent_primary);
    canvas.draw_lucide_icon(av_cx - 8, av_cy - 8, LucideIcon::User, pal.on_accent);
    canvas.draw_ui_text_clipped(cr.x + 46, cr.y + 8, "amaryllis", pal.text_primary, ui_scale, true, mono_ui, cr);
    canvas.draw_circle(cr.x + 49, cr.y + 30, 3, pal.accent_green);
    canvas.draw_ui_text_clipped(
        cr.x + 56,
        cr.y + 23,
        &cfg.active_profile_name,
        pal.text_secondary,
        ui_scale.min(256),
        false,
        mono_ui,
        cr,
    );

    let audio_on = snd_up && mixer_snap.volume_percent > 0;
    let slots = control_center_slots(&cfg.desktop, cr);
    for (module, r) in slots {
        match module {
            ControlCenterModule::Network => {
                draw_cc_row_toggle(
                    canvas,
                    pal,
                    r,
                    LucideIcon::Wifi,
                    "Network",
                    if net_up { "e1000 Ready" } else { "Offline" },
                    net_up,
                    ui_scale,
                    mono_ui,
                );
            }
            ControlCenterModule::Audio => {
                draw_cc_row_toggle(
                    canvas,
                    pal,
                    r,
                    LucideIcon::Volume2,
                    "Audio Output",
                    if audio_on { "AC'97 Active" } else { "Muted" },
                    audio_on,
                    ui_scale,
                    mono_ui,
                );
            }
            ControlCenterModule::Theme => {
                let theme_lbl = match cfg.theme {
                    ThemePreference::Light => "White Theme",
                    ThemePreference::Dark => "Black Theme",
                };
                draw_cc_row_toggle(
                    canvas,
                    pal,
                    r,
                    if pal.is_light { LucideIcon::Sun } else { LucideIcon::Moon },
                    "Appearance",
                    theme_lbl,
                    true,
                    ui_scale,
                    mono_ui,
                );
            }
            ControlCenterModule::Volume => {
                canvas.fill_rounded_rect_alpha(r, 10, pal.surface_card, 225);
                canvas.stroke_rounded_rect(r, 10, pal.surface_border, 155);
                canvas.draw_ui_text_clipped(
                    r.x + 10,
                    r.y + 5,
                    &format!("Volume  {}%", mixer_snap.volume_percent),
                    pal.text_secondary,
                    ui_scale,
                    false,
                    mono_ui,
                    r,
                );
                let bar_r = Rect::new(r.x + 10, r.y + 25, r.w - 20, 7);
                canvas.fill_rounded_rect(bar_r, 3, pal.surface_panel);
                let fill_w = (((bar_r.w as usize) * (mixer_snap.volume_percent.min(100) as usize)) / 100) as i32;
                if fill_w > 4 {
                    canvas.fill_rounded_rect(Rect::new(bar_r.x, bar_r.y, fill_w, bar_r.h), 3, pal.accent_primary);
                }
                let knob_x = (bar_r.x + fill_w).clamp(bar_r.x + 5, bar_r.right() - 5);
                canvas.draw_circle(knob_x, bar_r.y + 3, 6, pal.surface_border);
                canvas.draw_circle(knob_x, bar_r.y + 3, 5, if pal.is_light { 0xFFFFFFFF } else { 0xFFEAEAF0 });
            }
            ControlCenterModule::MediaPlayer => {
                canvas.fill_rounded_rect_alpha(r, 11, pal.surface_card, 225);
                canvas.stroke_rounded_rect(r, 11, pal.surface_border, 155);

                let art_r = Rect::new(r.x + 10, r.y + 10, 34, 34);
                canvas.fill_rounded_rect(art_r, 8, pal.accent_primary);
                canvas.draw_lucide_icon(art_r.x + 9, art_r.y + 9, LucideIcon::Music, pal.on_accent);

                let track_title = if mixer_snap.playing {
                    mixer_snap.title.as_str()
                } else {
                    "Amaryllis Synth"
                };
                let track_sub = if mixer_snap.playing && !mixer_snap.paused {
                    "Playing (AC'97)"
                } else if mixer_snap.paused {
                    "Paused"
                } else {
                    "Ready to play"
                };
                canvas.draw_ui_text_clipped(r.x + 52, r.y + 10, track_title, pal.text_primary, ui_scale, true, mono_ui, r);
                canvas.draw_ui_text_clipped(r.x + 52, r.y + 26, track_sub, pal.text_secondary, ui_scale.min(256), false, mono_ui, r);

                let btn_y = r.y + 62;
                canvas.draw_lucide_icon(r.x + 15, btn_y, LucideIcon::SkipBack, pal.text_secondary);
                canvas.draw_circle(r.x + 57, btn_y + 8, 13, pal.accent_primary);
                canvas.draw_lucide_icon(
                    r.x + 49,
                    btn_y,
                    if mixer_snap.playing && !mixer_snap.paused {
                        LucideIcon::Pause
                    } else {
                        LucideIcon::Play
                    },
                    pal.on_accent,
                );
                canvas.draw_lucide_icon(r.x + 83, btn_y, LucideIcon::SkipForward, pal.text_secondary);

                for b in 0..8i32 {
                    let wv: i32 = mixer_snap.waveform[(b as usize) * 4 % 32] as i32;
                    let amp = if mixer_snap.playing && !mixer_snap.paused {
                        (wv.abs() / 6).clamp(4, 20)
                    } else {
                        4 + ((b * 3 + 2) % 7)
                    };
                    let bx = r.right() - 88 + b * 9;
                    let by = r.bottom() - 14 - amp;
                    canvas.fill_rounded_rect(
                        Rect::new(bx, by, 4, amp),
                        2,
                        if mixer_snap.playing && !mixer_snap.paused && b % 2 == 0 {
                            pal.accent_primary
                        } else {
                            pal.text_secondary
                        },
                    );
                }
            }
            ControlCenterModule::Power => {
                canvas.fill_rounded_rect_alpha(r, 9, pal.surface_card, 225);
                canvas.stroke_rounded_rect(r, 9, pal.surface_border, 155);
                canvas.draw_lucide_icon(r.x + 10, r.y + 9, LucideIcon::Power, ACCENT_ERR);
                canvas.draw_ui_text_clipped(
                    r.x + 34,
                    r.y + 9,
                    "Exit to Text Console",
                    pal.text_primary,
                    ui_scale,
                    true,
                    mono_ui,
                    r,
                );
            }
        }
    }
}

fn draw_cc_row_toggle(
    canvas: &mut Canvas<'_>,
    pal: &ThemePalette,
    r: Rect,
    icon: LucideIcon,
    title: &str,
    sub: &str,
    active: bool,
    ui_scale: i32,
    mono_ui: bool,
) {
    canvas.fill_rounded_rect_alpha(
        r,
        10,
        if active { pal.surface_selected } else { pal.surface_card },
        225,
    );
    canvas.stroke_rounded_rect(
        r,
        10,
        if active { pal.accent_primary } else { pal.surface_border },
        165,
    );
    canvas.draw_circle(
        r.x + 19,
        r.y + r.h / 2,
        11,
        if active { pal.accent_primary } else { pal.surface_hover },
    );
    canvas.draw_lucide_icon(
        r.x + 11,
        r.y + r.h / 2 - 8,
        icon,
        if active { pal.on_accent } else { pal.text_primary },
    );
    canvas.draw_ui_text_clipped(r.x + 38, r.y + 5, title, pal.text_primary, ui_scale, true, mono_ui, r);
    canvas.draw_ui_text_clipped(r.x + 38, r.y + 20, sub, pal.text_secondary, ui_scale.min(256), false, mono_ui, r);
}

/// Renders the Calendar Popover anchored below the Top Bar Clock.
fn draw_calendar_popover(
    canvas: &mut Canvas<'_>,
    pal: &ThemePalette,
    cfg: &SystemAppearance,
    cr: Rect,
    dt: RtcDateTime,
) {
    let mono_ui = cfg.typography.ui_font == UiFontFamily::JetBrainsMono;
    let ui_scale = cfg.typography.ui_scale_q8.clamp(240, 268);

    canvas.draw_soft_shadow(cr, 14, 6, if pal.is_light { 58 } else { 96 });
    canvas.draw_glass_panel(
        cr,
        14,
        pal.modal_bg,
        if pal.is_light { 234 } else { 228 },
        pal.modal_border,
    );

    let time_s = if cfg.desktop.topbar_clock_24h {
        format!("{:02}:{:02}:{:02}", dt.hour, dt.minute, dt.second)
    } else {
        let (h12, ampm) = if dt.hour == 0 {
            (12, "AM")
        } else if dt.hour < 12 {
            (dt.hour, "AM")
        } else if dt.hour == 12 {
            (12, "PM")
        } else {
            (dt.hour - 12, "PM")
        };
        format!("{}:{:02}:{:02} {}", h12, dt.minute, dt.second, ampm)
    };
    canvas.draw_ui_text_clipped(
        cr.x + 14,
        cr.y + 12,
        &format!("{} {}", dt.month_short(), dt.year),
        pal.text_primary,
        ui_scale,
        true,
        mono_ui,
        cr,
    );
    let tw = ui_text_width(&time_s, ui_scale, false, mono_ui);
    canvas.draw_ui_text_clipped(
        cr.right() - tw - 14,
        cr.y + 12,
        &time_s,
        pal.accent_primary,
        ui_scale,
        true,
        mono_ui,
        cr,
    );

    canvas.fill_rect(Rect::new(cr.x + 12, cr.y + 36, cr.w - 24, 1), pal.surface_border);

    let cell_w = (cr.w - 24) / 7;
    let headers = ["Mo", "Tu", "We", "Th", "Fr", "Sa", "Su"];
    for (ci, lbl) in headers.iter().enumerate() {
        let cx = cr.x + 12 + (ci as i32) * cell_w + 6;
        canvas.draw_ui_text_clipped(cx, cr.y + 44, lbl, pal.text_muted, 256, true, mono_ui, cr);
    }

    let first_wd = first_weekday_mon0(dt.year, dt.month) as i32;
    let total_days = days_in_month(dt.year, dt.month) as i32;
    for d in 1..=total_days {
        let idx = first_wd + (d - 1);
        let row = idx / 7;
        let col = idx % 7;
        if row >= 6 {
            break;
        }
        let cx = cr.x + 12 + col * cell_w;
        let cy = cr.y + 64 + row * 22;
        let is_today = d == dt.day as i32;
        if is_today {
            canvas.fill_rounded_rect(Rect::new(cx + 2, cy - 1, cell_w - 4, 20), 9, pal.accent_primary);
        }
        let ds = format!("{:2}", d);
        canvas.draw_ui_text_clipped(
            cx + 7,
            cy + 2,
            &ds,
            if is_today { pal.on_accent } else { pal.text_primary },
            256,
            is_today,
            mono_ui,
            cr,
        );
    }

    let set_btn = Rect::new(cr.x + 12, cr.bottom() - 32, cr.w - 24, 24);
    canvas.fill_rounded_rect(set_btn, 7, pal.surface_card);
    canvas.stroke_rounded_rect(set_btn, 7, pal.surface_border, 175);
    canvas.draw_ui_text_clipped(
        set_btn.x + 48,
        set_btn.y + 5,
        "Time & Desktop Settings",
        pal.text_secondary,
        256,
        false,
        mono_ui,
        set_btn,
    );
}

/// Renders the Power / Session Popover anchored upward above the Capsule Power button.
fn draw_power_menu_popover(
    canvas: &mut Canvas<'_>,
    pal: &ThemePalette,
    cfg: &SystemAppearance,
    pr: Rect,
    cursor_x: i32,
    cursor_y: i32,
) {
    let mono_ui = cfg.typography.ui_font == UiFontFamily::JetBrainsMono;
    let ui_scale = cfg.typography.ui_scale_q8.clamp(240, 268);

    canvas.draw_soft_shadow(pr, 12, 5, if pal.is_light { 58 } else { 96 });
    canvas.draw_glass_panel(
        pr,
        12,
        pal.modal_bg,
        if pal.is_light { 236 } else { 230 },
        pal.modal_border,
    );

    let items: [(&str, LucideIcon, bool); 3] = [
        ("Exit to Text Console", LucideIcon::Terminal, false),
        ("Reboot", LucideIcon::RefreshCw, false),
        ("Shutdown", LucideIcon::Power, true),
    ];
    for (idx, (label, ico, danger)) in items.iter().enumerate() {
        let row_r = Rect::new(pr.x + 6, pr.y + 6 + (idx as i32) * 26, pr.w - 12, 24);
        let hov = row_r.contains(cursor_x, cursor_y);
        if hov {
            canvas.fill_rounded_rect(row_r, 6, pal.modal_hover);
        }
        let ico_col = if *danger {
            ACCENT_ERR
        } else if hov {
            pal.accent_primary
        } else {
            pal.text_secondary
        };
        canvas.draw_lucide_icon(row_r.x + 8, row_r.y + 4, *ico, ico_col);
        canvas.draw_ui_text_clipped(
            row_r.x + 30,
            row_r.y + 4,
            label,
            if *danger && hov { ACCENT_ERR } else { pal.text_primary },
            ui_scale,
            false,
            mono_ui,
            row_r,
        );
    }
}

/// Renders Colloid Icon Theme App Sprites directly on the Amaryllis Capsule with bilinear scaling.
fn draw_dock_tile_scaled(canvas: &mut Canvas<'_>, r: Rect, index: usize, _hovered: bool) {
    let dst_w = r.w.max(20) as usize;
    let dst_h = r.h.max(20) as usize;
    canvas.blit_argb_icon_scaled(
        r.x,
        r.y,
        dst_w,
        dst_h,
        DOCK_SPRITE_SIZE,
        DOCK_SPRITE_SIZE,
        dock_icon(index),
    );
}

pub fn is_desktop_active() -> bool {
    ACTIVE_DESKTOP.lock().is_some()
}

pub fn create_external_window_from_syscall(
    owner_pid: usize,
    title: &str,
    w: usize,
    h: usize,
) -> Result<u64, i64> {
    let mut guard = ACTIVE_DESKTOP.lock();
    let Some(ref mut desk) = *guard else {
        return Err(-22);
    };
    desk.create_external_window(owner_pid, title, w, h, None)
}

pub fn create_external_shm_window_from_syscall(
    owner_pid: usize,
    title: &str,
    w: usize,
    h: usize,
    shm_id: u64,
) -> Result<(u64, usize, usize), i64> {
    let cw = w.clamp(160, 1280);
    let ch = h.clamp(120, 800);
    let mut guard = ACTIVE_DESKTOP.lock();
    let Some(ref mut desk) = *guard else {
        return Err(-22);
    };
    let win_id = desk.create_external_window(owner_pid, title, cw, ch, Some(shm_id))?;
    Ok((win_id, cw, ch))
}

pub fn commit_external_shm_window_from_syscall(win_id: u64) -> Result<(), i64> {
    let mut guard = ACTIVE_DESKTOP.lock();
    let Some(ref mut desk) = *guard else {
        return Err(-22);
    };
    let Some(win) = desk.windows.iter().find(|win| win.id == win_id && win.external_user) else {
        return Err(-9);
    };
    let win_rect = win.rect;
    desk.compositor.invalidate_rect(win_rect);
    Ok(())
}

pub fn blit_external_window_from_syscall(
    win_id: u64,
    src_u32: &[u32],
    w: usize,
    h: usize,
) -> Result<(), i64> {
    let mut guard = ACTIVE_DESKTOP.lock();
    let Some(ref mut desk) = *guard else {
        return Err(-22);
    };
    let Some(win) = desk.windows.iter_mut().find(|win| win.id == win_id && win.external_user) else {
        return Err(-9);
    };
    let win_rect = win.rect;
    if let WindowContent::ExternalUser { width, height, pixels, shm_id, .. } = &mut win.content {
        if shm_id.is_some() {
            desk.compositor.invalidate_rect(win_rect);
            return Ok(());
        }
        let copy_w = w.min(*width);
        let copy_h = h.min(*height);
        for py in 0..copy_h {
            let srow = py * w;
            let drow = py * (*width);
            if srow + copy_w <= src_u32.len() && drow + copy_w <= pixels.len() {
                for px in 0..copy_w {
                    pixels[drow + px] = src_u32[srow + px] | 0xFF00_0000;
                }
            }
        }
        desk.compositor.invalidate_rect(win_rect);
        Ok(())
    } else {
        Err(-22)
    }
}

pub fn pop_external_event_from_syscall(win_id: u64) -> UserGuiEvent {
    let mut guard = ACTIVE_DESKTOP.lock();
    let Some(ref mut desk) = *guard else {
        return UserGuiEvent { kind: 1, p1: 0, p2: 0, p3: 0 };
    };

    if let Some(win) = desk.windows.iter_mut().find(|win| win.id == win_id && win.external_user) {
        if let WindowContent::ExternalUser { ref mut events, .. } = win.content {
            if !events.is_empty() {
                return events.remove(0);
            }
        }
        UserGuiEvent::default()
    } else {
        UserGuiEvent { kind: 1, p1: 0, p2: 0, p3: 0 }
    }
}

pub fn close_external_windows_for_pid(target_pid: usize) -> Vec<u64> {
    let mut released_shms = Vec::new();
    let mut guard = ACTIVE_DESKTOP.lock();
    if let Some(ref mut desk) = *guard {
        let before = desk.windows.len();
        desk.windows.retain(|w| {
            if let WindowContent::ExternalUser { owner_pid, shm_id, .. } = w.content {
                if owner_pid == target_pid {
                    if let Some(sid) = shm_id {
                        released_shms.push(sid);
                    }
                    return false;
                }
            }
            true
        });
        if desk.windows.len() != before {
            desk.compositor.invalidate_all();
        }
    }
    released_shms
}

pub fn cleanup_external_windows() {
    let mut guard = ACTIVE_DESKTOP.lock();
    if let Some(ref mut desk) = *guard {
        let before = desk.windows.len();
        desk.windows.retain(|w| !w.external_user);
        if desk.windows.len() != before {
            desk.compositor.invalidate_all();
        }
    }
}

pub fn append_user_stdout(text: &str) {
    let mut guard = ACTIVE_DESKTOP.lock();
    let Some(ref mut desk) = *guard else { return };
    for win in &mut desk.windows {
        if let WindowContent::Terminal(ref mut t) = win.content {
            let mut spans = Vec::new();
            spans.push(console::CapturedSpan {
                text: String::from(text),
                color: (156, 224, 198),
            });
            t.append_user_output(spans);
            win.surface_dirty = true;
            desk.compositor.invalidate_rect(win.rect);
            return;
        }
    }
}

#[inline(never)]
pub fn run_desktop(
    console: &mut Console<'_>,
    mapping: Option<&PhysicalMemoryMapping>,
    allocator: &mut PhysicalFrameAllocator<'_>,
) {
    crate::serial::log("wm", "run_desktop entered");
    let w = console.width();
    let h = console.height();
    ACTIVE_HHDM_OFFSET.store(mapping.map(|m| m.offset()).unwrap_or(0), Ordering::Release);
    if !crate::input::mouse::is_initialized() {
        let _ = crate::input::initialize_mouse();
    }
    if crate::input::mouse::is_initialized() {
        crate::interrupts::enable_mouse_irq();
    }
    crate::input::drain_mouse();

    {
        let mut guard = ACTIVE_DESKTOP.lock();
        *guard = Some(Box::new(DesktopState::new(w, h)));
    }
    crate::serial::log("wm", "DesktopState allocated");

    let mut first_frame_logged = false;
    loop {
        let mut pending: Vec<DesktopAction> = Vec::new();
        let mut exit_now = false;
        let mut last_frame_ms = 0u64;

        {
            let mut guard = ACTIVE_DESKTOP.lock();
            if let Some(ref mut desk) = *guard {
                desk.process_inputs(true);
                core::mem::swap(&mut pending, &mut desk.pending_actions);
                exit_now = desk.should_exit;
                desk.try_render_frame(console.renderer_mut(), Some(allocator));
                last_frame_ms = desk.last_frame_ms;
            }
        }

        if !first_frame_logged {
            first_frame_logged = true;
            crate::serial::log("wm", "first desktop frame rendered");
        }

        if exit_now {
            break;
        }

        if !pending.is_empty() {
            for act in pending {
                execute_pending_action(console, act, mapping, allocator);
            }
            let mut guard = ACTIVE_DESKTOP.lock();
            if let Some(ref mut desk) = *guard {
                let mut dirty_wins: Vec<Rect> = Vec::new();
                for w in &mut desk.windows {
                    match w.content {
                        WindowContent::Files(ref mut f) => {
                            f.refresh();
                            w.surface_dirty = true;
                            dirty_wins.push(w.rect);
                        }
                        WindowContent::Monitor(_) => {
                            w.surface_dirty = true;
                            dirty_wins.push(w.rect);
                        }
                        _ => {}
                    }
                }
                for r in dirty_wins {
                    desk.compositor.invalidate_rect(r);
                }
                desk.invalidate_dock_and_topbar();
            }
        }

        // Step cooperative kernel worker threads (knet_worker async HTTP/DHCP/Ping/DNS)
        let _ = crate::task::kthread::step_workers(2, 2);

        // Preemptive multi-process Ring 3 execution between 16ms 60 FPS compositor frames
        let mut quanta_used = 0usize;
        while sched::has_ready_now() && quanta_used < 2 {
            if timer::uptime_ms().saturating_sub(last_frame_ms) >= TARGET_FRAME_MS {
                break;
            }
            if !sched::step_user_tasks(console, mapping, allocator) {
                break;
            }
            quanta_used += 1;
        }
        // Always flush exited/faulted tasks cleanly and wake Waiting queues even if none ran a fresh quantum
        if quanta_used == 0 && sched::has_runnable_tasks() {
            let _ = sched::step_user_tasks(console, mapping, allocator);
        }

        let need_sleep = !sched::has_ready_now()
            && !crate::task::kthread::has_pending_net_jobs()
            && timer::uptime_ms().saturating_sub(last_frame_ms) < TARGET_FRAME_MS;
        if need_sleep {
            unsafe {
                asm!("sti", "hlt", options(nomem, nostack));
            }
        }
    }

    // Reap any active Ring 3 processes cleanly before leaving Desktop mode
    let active_pids: Vec<usize> = sched::snapshot_processes().into_iter().map(|p| p.pid).collect();
    for pid in active_pids {
        let _ = sched::kill_user_process(pid, mapping, allocator);
    }

    let _ = personalization::flush_disk_sync();
    let _ = crate::storage::vfs::sync();
    *ACTIVE_DESKTOP.lock() = None;
    ACTIVE_HHDM_OFFSET.store(0, Ordering::Release);
    crate::input::drain_mouse();
    console.repaint_full();
}

fn execute_pending_action(
    console: &mut Console<'_>,
    act: DesktopAction,
    mapping: Option<&PhysicalMemoryMapping>,
    allocator: &mut PhysicalFrameAllocator<'_>,
) {
    match act {
        DesktopAction::Beep(freq, dur) => {
            crate::audio::mixer::play_tone_async(freq, dur);
        }
        DesktopAction::PlayWav(path) => {
            let _ = crate::audio::mixer::play_wav_async(&path);
        }
        DesktopAction::RunShellCommand(cmd) => {
            console::begin_capture();
            crate::shell::execute(console, &cmd, mapping, allocator);
            let spans = console::end_capture();
            push_spans_to_terminal(&cmd, spans);
        }
        DesktopAction::ExecElf(path) => {
            console::begin_capture();
            let res = runner::spawn_elf_async(&path, mapping, allocator);
            let spans = console::end_capture();
            if !spans.is_empty() {
                push_spans_to_terminal(&format!("execbg {}", path), spans);
            }
            match res {
                Ok(pid) => {
                    append_user_stdout(&format!(
                        "[Launched Ring 3 PID {} '{}' (preemptive multitasking)]\n",
                        pid, path
                    ));
                }
                Err(err) => {
                    alerts::report_system_error(
                        AlertSeverity::Error,
                        AlertSubsystem::ElfLoader,
                        "ELF RUN ERR",
                        &format!("Failed to execute '{}'", path),
                        err.message(),
                        "Run 'mkelf all' to write demo Ring 3 binaries, or inspect with 'elfinfo'.",
                        true,
                    );
                }
            }
        }
        DesktopAction::KillProcess(pid) => {
            if let Err(err) = sched::kill_user_process(pid, mapping, allocator) {
                alerts::report_system_error(
                    AlertSeverity::Warning,
                    AlertSubsystem::CpuRing3,
                    "PROC KILL",
                    &format!("Cannot kill PID {}", pid),
                    err,
                    "Refresh Activity Monitor to verify active Ring 3 processes.",
                    true,
                );
            }
        }
        DesktopAction::OpenApp(_)
        | DesktopAction::OpenEditor(_)
        | DesktopAction::ToggleTheme
        | DesktopAction::SetTheme(_)
        | DesktopAction::SetWallpaper(_)
        | DesktopAction::SetCustomWallpaper(_)
        | DesktopAction::NextWallpaper
        | DesktopAction::OpenLatestAlert
        | DesktopAction::CloseDesktop
        | DesktopAction::None => {}
    }
}

fn push_spans_to_terminal(cmd: &str, spans: Vec<console::CapturedSpan>) {
    let mut guard = ACTIVE_DESKTOP.lock();
    let Some(ref mut desk) = *guard else { return };
    for win in &mut desk.windows {
        if let WindowContent::Terminal(ref mut t) = win.content {
            t.append_captured(cmd, spans);
            win.surface_dirty = true;
            desk.compositor.invalidate_rect(win.rect);
            return;
        }
    }
}
