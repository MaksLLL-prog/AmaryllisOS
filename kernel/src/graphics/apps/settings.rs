use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::cell::Cell;

use crate::audio::mixer::{self, BuiltinTrack};
use crate::drivers;
use crate::graphics::icons::LucideIcon;
use crate::graphics::personalization::{
    self, cc_idx, AccentPreset, DockPosition, MonoFontFamily, ProfileEntry,
    SelectionColorMode, SurfaceStyle, SystemAppearance, ThemePreference, UiFontFamily,
    WallpaperScaleMode, WallpaperSource,
};
use crate::graphics::theme::{self, ThemeMode, WallpaperPreset, ACCENT_ERR, ACCENT_MINT, ACCENT_WARN};
use crate::graphics::widgets::{text_width, text_width_bold, ui_text_width, Canvas, Rect};
use crate::input::{KeyCode, KeyEvent};
use crate::net::{self, dhcp, icmp};
use crate::storage::vfs;

use super::{BuiltinAppKind, DesktopAction};

const SIDEBAR_W: i32 = 184;

#[derive(Clone, Copy, PartialEq, Eq)]
enum SettingsSection {
    Appearance,
    Wallpaper,
    Desktop,
    WindowsMotion,
    Typography,
    Sound,
    Network,
}

impl SettingsSection {
    const ALL: [Self; 7] = [
        Self::Appearance,
        Self::Wallpaper,
        Self::Desktop,
        Self::WindowsMotion,
        Self::Typography,
        Self::Sound,
        Self::Network,
    ];

    const fn label(self) -> &'static str {
        match self {
            Self::Appearance => "Appearance",
            Self::Wallpaper => "Wallpaper",
            Self::Desktop => "Desktop & Bar",
            Self::WindowsMotion => "Windows & Motion",
            Self::Typography => "Typography",
            Self::Sound => "Sound",
            Self::Network => "Network",
        }
    }

    const fn icon(self) -> LucideIcon {
        match self {
            Self::Appearance => LucideIcon::Sun,
            Self::Wallpaper => LucideIcon::LayoutGrid,
            Self::Desktop => LucideIcon::Monitor,
            Self::WindowsMotion => LucideIcon::Layers,
            Self::Typography => LucideIcon::FileText,
            Self::Sound => LucideIcon::Volume2,
            Self::Network => LucideIcon::Wifi,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum InputTarget {
    None,
    NewProfile,
    RenameProfile,
    WallpaperPath,
}

pub struct SettingsApp {
    section: SettingsSection,
    status_msg: String,
    is_error_status: bool,
    scroll_y: i32,
    last_max_scroll: Cell<i32>,
    profiles: Vec<ProfileEntry>,
    selected_profile_idx: usize,
    image_files: Vec<String>,
    input_target: InputTarget,
    input_buf: String,
    editing_selection_rgb: bool,
}

impl SettingsApp {
    pub fn new() -> Self {
        let mut app = Self {
            section: SettingsSection::Appearance,
            status_msg: String::from("Personalization synced with VFS"),
            is_error_status: false,
            scroll_y: 0,
            last_max_scroll: Cell::new(0),
            profiles: Vec::new(),
            selected_profile_idx: 0,
            image_files: Vec::new(),
            input_target: InputTarget::None,
            input_buf: String::new(),
            editing_selection_rgb: false,
        };
        app.reload_lists();
        app
    }

    pub fn reload_lists(&mut self) {
        self.profiles = personalization::list_profiles();
        let active_slug = personalization::snapshot().active_profile_slug;
        if let Some(pos) = self.profiles.iter().position(|p| p.slug == active_slug) {
            self.selected_profile_idx = pos;
        } else if self.selected_profile_idx >= self.profiles.len() {
            self.selected_profile_idx = 0;
        }

        self.image_files.clear();
        for dir in ["/home/wallpapers", "/home", "/disk", "/"] {
            if let Ok(entries) = vfs::with_vfs(|s| s.list_dir(dir)) {
                for e in entries {
                    let lower = e.name.to_ascii_lowercase();
                    if lower.ends_with(".bmp") || lower.ends_with(".tga") || lower.ends_with(".ppm") {
                        let p = if dir == "/" {
                            format!("/{}", e.name)
                        } else {
                            format!("{}/{}", dir, e.name)
                        };
                        if !self.image_files.iter().any(|x| x == &p) {
                            self.image_files.push(p);
                        }
                    }
                }
            }
        }
    }

    fn set_ok(&mut self, msg: String) {
        self.status_msg = msg;
        self.is_error_status = false;
    }

    fn set_err(&mut self, msg: String) {
        self.status_msg = msg;
        self.is_error_status = true;
    }

    pub fn on_wheel(&mut self, delta: i8) {
        let max_s = self.last_max_scroll.get();
        if delta > 0 {
            self.scroll_y = (self.scroll_y + 28).min(max_s);
        } else if delta < 0 {
            self.scroll_y = (self.scroll_y - 28).max(0);
        }
    }

    pub fn on_key(&mut self, evt: KeyEvent) -> DesktopAction {
        if self.input_target == InputTarget::None {
            return DesktopAction::None;
        }
        match evt.code {
            KeyCode::Escape => {
                self.input_target = InputTarget::None;
                self.input_buf.clear();
                self.set_ok(String::from("Cancelled text input"));
            }
            KeyCode::Enter => {
                let target = self.input_target;
                let text = self.input_buf.trim();
                let val = String::from(text);
                self.input_target = InputTarget::None;
                self.input_buf.clear();
                match target {
                    InputTarget::NewProfile => {
                        match personalization::save_current_as_profile(&val) {
                            Ok(name) => {
                                self.reload_lists();
                                self.set_ok(format!("Saved profile '{}'", name));
                            }
                            Err(e) => self.set_err(e),
                        }
                    }
                    InputTarget::RenameProfile => {
                        if let Some(p) = self.profiles.get(self.selected_profile_idx).cloned() {
                            match personalization::rename_profile(&p.slug, &val) {
                                Ok(new_name) => {
                                    self.reload_lists();
                                    self.set_ok(format!("Renamed profile to '{}'", new_name));
                                }
                                Err(e) => self.set_err(e),
                            }
                        }
                    }
                    InputTarget::WallpaperPath => {
                        if val.is_empty() {
                            self.set_err(String::from("Enter a VFS image path (.bmp/.tga/.ppm)"));
                        } else {
                            return DesktopAction::SetCustomWallpaper(val);
                        }
                    }
                    InputTarget::None => {}
                }
            }
            KeyCode::Backspace => {
                self.input_buf.pop();
            }
            KeyCode::Char(c) if c.is_ascii() && !c.is_ascii_control() => {
                if self.input_buf.len() < 72 {
                    self.input_buf.push(c);
                }
            }
            _ => {}
        }
        DesktopAction::None
    }

    pub fn on_click(&mut self, area: Rect, mx: i32, my: i32) -> DesktopAction {
        // Sidebar navigation
        for (i, sec) in SettingsSection::ALL.iter().enumerate() {
            let sr = Rect::new(area.x + 10, area.y + 38 + (i as i32) * 33, SIDEBAR_W - 20, 28);
            if sr.contains(mx, my) {
                self.section = *sec;
                self.scroll_y = 0;
                self.input_target = InputTarget::None;
                if *sec == SettingsSection::Appearance || *sec == SettingsSection::Wallpaper {
                    self.reload_lists();
                }
                return DesktopAction::None;
            }
        }

        // Sidebar bottom Save & Reset buttons
        let btn_save = Rect::new(area.x + 10, area.bottom() - 68, SIDEBAR_W - 20, 26);
        let btn_reset = Rect::new(area.x + 10, area.bottom() - 36, SIDEBAR_W - 20, 26);
        if btn_save.contains(mx, my) {
            match personalization::save_to_vfs() {
                Ok(()) => self.set_ok(String::from("Saved settings to VFS (.config/amaryllis)")),
                Err(e) => self.set_err(e),
            }
            return DesktopAction::None;
        }
        if btn_reset.contains(mx, my) {
            match personalization::reset_to_defaults() {
                Ok(()) => {
                    self.reload_lists();
                    self.set_ok(String::from("Reset all settings to Black (Default)"));
                }
                Err(e) => self.set_err(e),
            }
            return DesktopAction::None;
        }

        let main = Rect::new(
            area.x + SIDEBAR_W + 14,
            area.y + 12,
            area.w - SIDEBAR_W - 28,
            area.h - 40,
        );
        if !main.contains(mx, my) {
            return DesktopAction::None;
        }
        let vm_y = my + self.scroll_y;

        match self.section {
            SettingsSection::Appearance => self.click_appearance(main, mx, vm_y),
            SettingsSection::Wallpaper => self.click_wallpaper(main, mx, vm_y),
            SettingsSection::Desktop => self.click_desktop(main, mx, vm_y),
            SettingsSection::WindowsMotion => self.click_windows_motion(main, mx, vm_y),
            SettingsSection::Typography => self.click_typography(main, mx, vm_y),
            SettingsSection::Sound => self.click_sound(main, mx, vm_y),
            SettingsSection::Network => self.click_network(main, mx, vm_y),
        }
    }

    fn click_appearance(&mut self, main: Rect, mx: i32, my: i32) -> DesktopAction {
        let cfg = personalization::snapshot();
        // 1. Theme cards (Black / White)
        let card_w = ((main.w - 12) / 2).max(120);
        let modes = [ThemePreference::Dark, ThemePreference::Light];
        for (i, pref) in modes.iter().enumerate() {
            let r = Rect::new(main.x + (i as i32) * (card_w + 12), main.y + 30, card_w, 66);
            if r.contains(mx, my) {
                let p = *pref;
                personalization::update_and_save(true, true, false, |c| c.theme = p);
                self.set_ok(format!("Theme set to {}", p.title()));
                return DesktopAction::None;
            }
        }

        // 2. Surface Style (Standard / Midnight / Snow)
        let styles = [SurfaceStyle::Standard, SurfaceStyle::Midnight, SurfaceStyle::Snow];
        for (i, st) in styles.iter().enumerate() {
            let sr = Rect::new(main.x + 106 + (i as i32) * 92, main.y + 106, 86, 24);
            if sr.contains(mx, my) {
                let s = *st;
                personalization::update_and_save(true, false, false, |c| c.surface_style = s);
                self.set_ok(format!("Surface style: {}", s.title()));
                return DesktopAction::None;
            }
        }

        // 3. Accent Color Swatches
        let cur_mode = theme::current_theme_mode();
        for (i, preset) in AccentPreset::BUILTIN.iter().enumerate() {
            let sr = Rect::new(main.x + (i as i32) * 32, main.y + 158, 24, 24);
            if sr.contains(mx, my) {
                let ap = *preset;
                personalization::update_and_save(true, false, false, |c| {
                    if self.editing_selection_rgb {
                        c.selection = SelectionColorMode::Custom(ap.color_for_mode(cur_mode));
                    } else {
                        c.accent = ap;
                    }
                });
                self.set_ok(format!(
                    "{}: {}",
                    if self.editing_selection_rgb { "Selection" } else { "Accent" },
                    ap.label()
                ));
                return DesktopAction::None;
            }
        }

        // Toggle Target: Accent vs Selection RGB
        let btn_target_acc = Rect::new(main.x + 266, main.y + 158, 72, 24);
        let btn_target_sel = Rect::new(main.x + 344, main.y + 158, 86, 24);
        let btn_sel_follow = Rect::new(main.x + 436, main.y + 158, 86, 24);
        if btn_target_acc.contains(mx, my) {
            self.editing_selection_rgb = false;
            return DesktopAction::None;
        }
        if btn_target_sel.contains(mx, my) {
            self.editing_selection_rgb = true;
            return DesktopAction::None;
        }
        if btn_sel_follow.contains(mx, my) {
            personalization::update_and_save(true, false, false, |c| {
                c.selection = SelectionColorMode::FollowAccent;
            });
            self.set_ok(String::from("Selection color synced with Accent"));
            return DesktopAction::None;
        }

        // 4. Custom RGB Sliders
        let accent_base = cfg.accent.color_for_mode(cur_mode);
        let active_col = if self.editing_selection_rgb {
            cfg.selection.resolve(accent_base)
        } else {
            accent_base
        };
        let mut r_ch = ((active_col >> 16) & 0xFF) as u8;
        let mut g_ch = ((active_col >> 8) & 0xFF) as u8;
        let mut b_ch = (active_col & 0xFF) as u8;

        let slider_w = (main.w - 170).max(120);
        for ch_idx in 0..3i32 {
            let sr = Rect::new(main.x + 30, main.y + 194 + ch_idx * 20, slider_w, 14);
            if sr.contains(mx, my) {
                let val = (((mx - sr.x) * 255) / sr.w.max(1)).clamp(0, 255) as u8;
                match ch_idx {
                    0 => r_ch = val,
                    1 => g_ch = val,
                    _ => b_ch = val,
                }
                let rgb = 0xFF00_0000 | ((r_ch as u32) << 16) | ((g_ch as u32) << 8) | (b_ch as u32);
                personalization::update_and_save(true, false, false, |c| {
                    if self.editing_selection_rgb {
                        c.selection = SelectionColorMode::Custom(rgb);
                    } else {
                        c.accent = AccentPreset::from_u32_or_preset(rgb);
                    }
                });
                self.set_ok(format!("Custom color #{:06X}", rgb & 0x00FF_FFFF));
                return DesktopAction::None;
            }
        }

        // 5. Profiles Management
        let prof_box = Rect::new(main.x, main.y + 278, main.w, 128);
        for (i, _) in self.profiles.iter().take(4).enumerate() {
            let row_r = Rect::new(prof_box.x + 8, prof_box.y + 6 + (i as i32) * 28, prof_box.w - 140, 24);
            if row_r.contains(mx, my) {
                self.selected_profile_idx = i;
                return DesktopAction::None;
            }
        }

        let bx = prof_box.right() - 122;
        let btn_apply = Rect::new(bx, prof_box.y + 8, 114, 24);
        let btn_new = Rect::new(bx, prof_box.y + 36, 114, 24);
        let btn_ren = Rect::new(bx, prof_box.y + 64, 114, 24);
        let btn_del = Rect::new(bx, prof_box.y + 92, 114, 24);

        if btn_apply.contains(mx, my) {
            if let Some(p) = self.profiles.get(self.selected_profile_idx).cloned() {
                match personalization::apply_profile(&p.slug) {
                    Ok(name) => {
                        self.reload_lists();
                        self.set_ok(format!("Applied profile '{}'", name));
                    }
                    Err(e) => self.set_err(e),
                }
            }
            return DesktopAction::None;
        }
        if btn_new.contains(mx, my) {
            self.input_target = InputTarget::NewProfile;
            self.input_buf.clear();
            self.set_ok(String::from("Type new profile name and press Enter"));
            return DesktopAction::None;
        }
        if btn_ren.contains(mx, my) {
            if let Some(p) = self.profiles.get(self.selected_profile_idx).cloned() {
                self.input_target = InputTarget::RenameProfile;
                self.input_buf = p.name;
                self.set_ok(String::from("Type new name and press Enter"));
            }
            return DesktopAction::None;
        }
        if btn_del.contains(mx, my) {
            if let Some(p) = self.profiles.get(self.selected_profile_idx).cloned() {
                match personalization::delete_profile(&p.slug) {
                    Ok(()) => {
                        self.reload_lists();
                        self.set_ok(format!("Deleted profile '{}'", p.name));
                    }
                    Err(e) => self.set_err(e),
                }
            }
            return DesktopAction::None;
        }

        DesktopAction::None
    }

    fn click_wallpaper(&mut self, main: Rect, mx: i32, my: i32) -> DesktopAction {
        let wp_y = main.y + 32;
        let wp_gap = 8;
        let wp_w = ((main.w - wp_gap * 4) / 5).max(58);
        for (i, preset) in WallpaperPreset::ALL.iter().enumerate() {
            let wr = Rect::new(main.x + (i as i32) * (wp_w + wp_gap), wp_y, wp_w, 86);
            if wr.contains(mx, my) {
                self.set_ok(format!("Wallpaper: {}", preset.title()));
                return DesktopAction::SetWallpaper(*preset);
            }
        }

        // Scaling modes
        for (i, sm) in WallpaperScaleMode::ALL.iter().enumerate() {
            let sr = Rect::new(main.x + 92 + (i as i32) * 76, main.y + 132, 70, 26);
            if sr.contains(mx, my) {
                let m = *sm;
                personalization::update_and_save(false, true, false, |c| c.wallpaper_scale = m);
                self.set_ok(format!("Wallpaper scaling: {}", m.title()));
                return DesktopAction::None;
            }
        }

        // Animated backdrop toggle
        let anim_btn = Rect::new(main.x + 410, main.y + 132, 128, 26);
        if anim_btn.contains(mx, my) {
            let next_val = !personalization::snapshot().wallpaper_animated;
            personalization::update_and_save(false, false, false, |c| {
                c.wallpaper_animated = next_val;
                if next_val {
                    c.animations.wallpaper_animations = true;
                }
            });
            self.set_ok(if next_val {
                String::from("Animated wallpaper shimmer enabled")
            } else {
                String::from("Static wallpaper cache enabled")
            });
            return DesktopAction::None;
        }

        // Custom path input bar + Apply
        let path_box = Rect::new(main.x, main.y + 190, (main.w - 94).max(160), 28);
        let apply_btn = Rect::new(path_box.right() + 8, main.y + 190, 84, 28);
        if path_box.contains(mx, my) {
            self.input_target = InputTarget::WallpaperPath;
            if let WallpaperSource::CustomFile(p) = personalization::snapshot().wallpaper_source {
                self.input_buf = p;
            } else {
                self.input_buf = String::from("/home/wallpapers/");
            }
            self.set_ok(String::from("Enter .bmp/.tga/.ppm path and press Enter"));
            return DesktopAction::None;
        }
        if apply_btn.contains(mx, my) {
            let path = if self.input_target == InputTarget::WallpaperPath && !self.input_buf.trim().is_empty() {
                String::from(self.input_buf.trim())
            } else if let Some(first) = self.image_files.first() {
                first.clone()
            } else {
                self.set_err(String::from("No custom image selected"));
                return DesktopAction::None;
            };
            self.input_target = InputTarget::None;
            self.set_ok(format!("Applying image {}", path));
            return DesktopAction::SetCustomWallpaper(path);
        }

        // Discovered image files list
        for (i, path) in self.image_files.iter().take(5).enumerate() {
            let row = Rect::new(main.x + 8, main.y + 230 + (i as i32) * 28, main.w - 16, 24);
            if row.contains(mx, my) {
                let p = path.clone();
                self.set_ok(format!("Loaded custom image {}", p));
                return DesktopAction::SetCustomWallpaper(p);
            }
        }

        DesktopAction::None
    }

    fn click_desktop(&mut self, main: Rect, mx: i32, my: i32) -> DesktopAction {
        let cfg = personalization::snapshot();

        // Dock Position
        for (i, pos) in DockPosition::ALL.iter().enumerate() {
            let br = Rect::new(main.x + 110 + (i as i32) * 86, main.y + 30, 80, 26);
            if br.contains(mx, my) {
                let p = *pos;
                personalization::update_and_save(false, false, true, |c| c.desktop.dock_position = p);
                self.set_ok(format!("Dock moved to {}", p.title()));
                return DesktopAction::None;
            }
        }

        // Dock Icon Size slider + +/-
        let sz_bar = Rect::new(main.x + 110, main.y + 68, 170, 16);
        let sz_dec = Rect::new(main.x + 288, main.y + 64, 24, 24);
        let sz_inc = Rect::new(main.x + 316, main.y + 64, 24, 24);
        if sz_bar.contains(mx, my) {
            let v = (32 + (((mx - sz_bar.x) * 20) / sz_bar.w.max(1))).clamp(32, 52) as u8;
            personalization::update_and_save(false, false, true, |c| c.desktop.dock_icon_size = v);
            self.set_ok(format!("Dock icon size: {}px", v));
            return DesktopAction::None;
        }
        if sz_dec.contains(mx, my) || sz_inc.contains(mx, my) {
            let cur = cfg.desktop.dock_icon_size as i32;
            let next = (if sz_inc.contains(mx, my) { cur + 2 } else { cur - 2 }).clamp(32, 52) as u8;
            personalization::update_and_save(false, false, true, |c| c.desktop.dock_icon_size = next);
            self.set_ok(format!("Dock icon size: {}px", next));
            return DesktopAction::None;
        }

        // Dock Icon Gap slider + +/-
        let gap_bar = Rect::new(main.x + 110, main.y + 98, 170, 16);
        let gap_dec = Rect::new(main.x + 288, main.y + 94, 24, 24);
        let gap_inc = Rect::new(main.x + 316, main.y + 94, 24, 24);
        if gap_bar.contains(mx, my) {
            let v = (4 + (((mx - gap_bar.x) * 14) / gap_bar.w.max(1))).clamp(4, 18) as u8;
            personalization::update_and_save(false, false, true, |c| c.desktop.dock_icon_gap = v);
            self.set_ok(format!("Dock spacing: {}px", v));
            return DesktopAction::None;
        }
        if gap_dec.contains(mx, my) || gap_inc.contains(mx, my) {
            let cur = cfg.desktop.dock_icon_gap as i32;
            let next = (if gap_inc.contains(mx, my) { cur + 2 } else { cur - 2 }).clamp(4, 18) as u8;
            personalization::update_and_save(false, false, true, |c| c.desktop.dock_icon_gap = next);
            self.set_ok(format!("Dock spacing: {}px", next));
            return DesktopAction::None;
        }

        // Dock Feature Toggles (4 pills)
        let dock_toggles = [
            ("Magnification", 0),
            ("Auto-Hide", 1),
            ("Indicators", 2),
            ("Divider", 3),
        ];
        let tw = ((main.w - 18) / 4).max(90);
        for (i, (_, id)) in dock_toggles.iter().enumerate() {
            let tr = Rect::new(main.x + (i as i32) * (tw + 6), main.y + 128, tw, 26);
            if tr.contains(mx, my) {
                personalization::update_and_save(false, false, true, |c| match *id {
                    0 => c.desktop.dock_magnification = !c.desktop.dock_magnification,
                    1 => c.desktop.dock_auto_hide = !c.desktop.dock_auto_hide,
                    2 => c.desktop.dock_show_indicators = !c.desktop.dock_show_indicators,
                    _ => c.desktop.dock_show_divider = !c.desktop.dock_show_divider,
                });
                self.set_ok(String::from("Updated Dock behavior"));
                return DesktopAction::None;
            }
        }

        // Top Bar Toggles (6 toggles in 2x3 grid)
        let tb_w = ((main.w - 12) / 3).max(110);
        for idx in 0..6usize {
            let col = (idx % 3) as i32;
            let row = (idx / 3) as i32;
            let tr = Rect::new(main.x + col * (tb_w + 6), main.y + 186 + row * 30, tb_w, 25);
            if tr.contains(mx, my) {
                personalization::update_and_save(false, false, true, |c| match idx {
                    0 => c.desktop.topbar_show_clock = !c.desktop.topbar_show_clock,
                    1 => c.desktop.topbar_clock_24h = !c.desktop.topbar_clock_24h,
                    2 => c.desktop.topbar_show_date = !c.desktop.topbar_show_date,
                    3 => c.desktop.topbar_show_notifications = !c.desktop.topbar_show_notifications,
                    4 => c.desktop.topbar_show_network = !c.desktop.topbar_show_network,
                    _ => c.desktop.topbar_show_audio = !c.desktop.topbar_show_audio,
                });
                self.set_ok(String::from("Updated Top Bar configuration"));
                return DesktopAction::None;
            }
        }

        // Control Center Modules (enable/disable & Up/Down reorder)
        for i in 0..6usize {
            let ry = main.y + 276 + (i as i32) * 26;
            let tog_r = Rect::new(main.x, ry, main.w - 72, 23);
            let up_r = Rect::new(main.right() - 64, ry, 28, 23);
            let dn_r = Rect::new(main.right() - 32, ry, 28, 23);

            if tog_r.contains(mx, my) {
                let m = cfg.desktop.cc_order[i];
                personalization::update_and_save(false, false, true, |c| {
                    c.desktop.cc_enabled[cc_idx(m)] = !c.desktop.cc_enabled[cc_idx(m)];
                });
                self.set_ok(format!("Toggled Control Center module '{}'", m.title()));
                return DesktopAction::None;
            }
            if up_r.contains(mx, my) && i > 0 {
                personalization::update_and_save(false, false, true, |c| {
                    c.desktop.cc_order.swap(i, i - 1);
                });
                self.set_ok(String::from("Reordered Control Center modules"));
                return DesktopAction::None;
            }
            if dn_r.contains(mx, my) && i + 1 < 6 {
                personalization::update_and_save(false, false, true, |c| {
                    c.desktop.cc_order.swap(i, i + 1);
                });
                self.set_ok(String::from("Reordered Control Center modules"));
                return DesktopAction::None;
            }
        }

        DesktopAction::None
    }

    fn click_windows_motion(&mut self, main: Rect, mx: i32, my: i32) -> DesktopAction {
        let cfg = personalization::snapshot();

        // Corner radius bar + presets
        let rad_bar = Rect::new(main.x + 158, main.y + 34, 154, 16);
        if rad_bar.contains(mx, my) {
            let v = (((mx - rad_bar.x) * 18) / rad_bar.w.max(1)).clamp(0, 18) as u8;
            personalization::update_and_save(false, false, true, |c| c.windows.corner_radius = v);
            self.set_ok(format!("Window corner radius: {}px", v));
            return DesktopAction::None;
        }
        for (i, rval) in [0u8, 6, 11, 16].iter().enumerate() {
            let br = Rect::new(main.x + 324 + (i as i32) * 46, main.y + 30, 40, 24);
            if br.contains(mx, my) {
                let rv = *rval;
                personalization::update_and_save(false, false, true, |c| c.windows.corner_radius = rv);
                self.set_ok(format!("Window corner radius: {}px", rv));
                return DesktopAction::None;
            }
        }

        // Shadow toggle & intensity slider
        let sh_tog = Rect::new(main.x, main.y + 68, 126, 26);
        let sh_bar = Rect::new(main.x + 210, main.y + 73, (main.w - 256).max(120), 16);
        if sh_tog.contains(mx, my) {
            personalization::update_and_save(false, false, true, |c| {
                c.windows.shadows_enabled = !c.windows.shadows_enabled;
            });
            self.set_ok(String::from("Toggled window drop shadows"));
            return DesktopAction::None;
        }
        if sh_bar.contains(mx, my) {
            let v = (((mx - sh_bar.x) * 100) / sh_bar.w.max(1)).clamp(0, 100) as u8;
            personalization::update_and_save(false, false, true, |c| {
                c.windows.shadows_enabled = true;
                c.windows.shadow_intensity = v;
            });
            self.set_ok(format!("Shadow intensity: {}%", v));
            return DesktopAction::None;
        }

        // Window behavior toggles (4)
        let wt_w = ((main.w - 18) / 2).max(140);
        for idx in 0..4usize {
            let col = (idx % 2) as i32;
            let row = (idx / 2) as i32;
            let r = Rect::new(main.x + col * (wt_w + 10), main.y + 108 + row * 32, wt_w, 26);
            if r.contains(mx, my) {
                personalization::update_and_save(false, false, true, |c| match idx {
                    0 => c.windows.active_highlight = !c.windows.active_highlight,
                    1 => c.windows.open_animation = !c.windows.open_animation,
                    2 => c.windows.close_animation = !c.windows.close_animation,
                    _ => c.windows.move_animation = !c.windows.move_animation,
                });
                self.set_ok(String::from("Updated window appearance & transitions"));
                return DesktopAction::None;
            }
        }

        // Motion & Reduced Motion controls
        let rm_btn = Rect::new(main.x, main.y + 206, main.w, 34);
        if rm_btn.contains(mx, my) {
            let next = !cfg.animations.reduced_motion;
            personalization::update_and_save(false, false, true, |c| {
                c.animations.reduced_motion = next;
            });
            self.set_ok(if next {
                String::from("Reduced Motion enabled — all UI transitions instant")
            } else {
                String::from("Reduced Motion disabled — smooth transitions active")
            });
            return DesktopAction::None;
        }

        let at_w = ((main.w - 18) / 2).max(140);
        for idx in 0..5usize {
            let col = (idx % 2) as i32;
            let row = (idx / 2) as i32;
            let r = Rect::new(main.x + col * (at_w + 10), main.y + 250 + row * 32, at_w, 26);
            if r.contains(mx, my) {
                personalization::update_and_save(false, false, true, |c| match idx {
                    0 => c.animations.system_animations = !c.animations.system_animations,
                    1 => c.animations.window_animations = !c.animations.window_animations,
                    2 => c.animations.dock_animations = !c.animations.dock_animations,
                    3 => c.animations.hover_animations = !c.animations.hover_animations,
                    _ => {
                        c.animations.wallpaper_animations = !c.animations.wallpaper_animations;
                        c.wallpaper_animated = c.animations.wallpaper_animations;
                    }
                });
                self.set_ok(String::from("Updated motion preferences"));
                return DesktopAction::None;
            }
        }

        DesktopAction::None
    }

    fn click_typography(&mut self, main: Rect, mx: i32, my: i32) -> DesktopAction {
        // UI Font Family
        for (i, uf) in [UiFontFamily::JakartaSans, UiFontFamily::JetBrainsMono].iter().enumerate() {
            let br = Rect::new(main.x + 124 + (i as i32) * 144, main.y + 32, 136, 26);
            if br.contains(mx, my) {
                let f = *uf;
                personalization::update_and_save(false, false, true, |c| c.typography.ui_font = f);
                self.set_ok(format!("UI font: {}", f.title()));
                return DesktopAction::None;
            }
        }

        // Mono Font Family
        for (i, mf) in [MonoFontFamily::JetBrainsMono, MonoFontFamily::Classic8x8].iter().enumerate() {
            let br = Rect::new(main.x + 124 + (i as i32) * 144, main.y + 66, 136, 26);
            if br.contains(mx, my) {
                let f = *mf;
                personalization::update_and_save(false, false, true, |c| c.typography.mono_font = f);
                self.set_ok(format!("Monospace font: {}", f.title()));
                return DesktopAction::None;
            }
        }

        // UI Scale
        for (i, (q8, lbl)) in [(256i32, "Compact"), (272, "Default"), (304, "Large")].iter().enumerate() {
            let br = Rect::new(main.x + 124 + (i as i32) * 102, main.y + 104, 96, 26);
            if br.contains(mx, my) {
                let val = *q8;
                personalization::update_and_save(false, false, true, |c| c.typography.ui_scale_q8 = val);
                self.set_ok(format!("UI text size: {}", lbl));
                return DesktopAction::None;
            }
        }

        // Header Scale
        for (i, (q8, lbl)) in [(288i32, "Subtle"), (320, "Default"), (352, "Display")].iter().enumerate() {
            let br = Rect::new(main.x + 124 + (i as i32) * 102, main.y + 138, 96, 26);
            if br.contains(mx, my) {
                let val = *q8;
                personalization::update_and_save(false, false, true, |c| c.typography.header_scale_q8 = val);
                self.set_ok(format!("Header scale: {}", lbl));
                return DesktopAction::None;
            }
        }

        // Mono Size
        for (i, (q8, lbl)) in [(256i32, "100% (8x16)"), (288, "112%"), (320, "125%")].iter().enumerate() {
            let br = Rect::new(main.x + 124 + (i as i32) * 102, main.y + 172, 96, 26);
            if br.contains(mx, my) {
                let val = *q8;
                personalization::update_and_save(false, false, true, |c| c.typography.mono_scale_q8 = val);
                self.set_ok(format!("Mono / Terminal size: {}", lbl));
                return DesktopAction::None;
            }
        }

        DesktopAction::None
    }

    fn click_sound(&mut self, main: Rect, mx: i32, my: i32) -> DesktopAction {
        let vol_bar = Rect::new(main.x, main.y + 138, main.w.min(340), 16);
        if vol_bar.contains(mx, my) {
            let pct = (((mx - vol_bar.x) * 100) / vol_bar.w.max(1)).clamp(0, 100) as u8;
            mixer::set_volume(pct);
            self.set_ok(format!("Master Volume set to {}%", pct));
            return DesktopAction::None;
        }
        for (i, pct) in [0u8, 25, 50, 75, 100].iter().enumerate() {
            let br = Rect::new(main.x + (i as i32) * 62, main.y + 164, 56, 26);
            if br.contains(mx, my) {
                mixer::set_volume(*pct);
                self.set_ok(format!("Master Volume set to {}%", pct));
                return DesktopAction::None;
            }
        }
        let btn_chime = Rect::new(main.x, main.y + 208, 136, 30);
        let btn_player = Rect::new(main.x + 146, main.y + 208, 164, 30);
        if btn_chime.contains(mx, my) {
            mixer::play_builtin_async(BuiltinTrack::StartupChime);
            self.set_ok(String::from("Playing Startup Chime in background mixer"));
            return DesktopAction::None;
        }
        if btn_player.contains(mx, my) {
            return DesktopAction::OpenApp(BuiltinAppKind::AudioPlayer);
        }
        DesktopAction::None
    }

    fn click_network(&mut self, main: Rect, mx: i32, my: i32) -> DesktopAction {
        let btn_dhcp = Rect::new(main.x, main.y + 190, 136, 30);
        let btn_ping = Rect::new(main.x + 146, main.y + 190, 132, 30);
        let btn_viewer = Rect::new(main.x + 288, main.y + 190, 146, 30);

        if btn_dhcp.contains(mx, my) {
            match dhcp::discover_and_configure() {
                Ok(lease) => self.set_ok(format!(
                    "DHCP configured IP {}.{}.{}.{}",
                    lease.ip[0], lease.ip[1], lease.ip[2], lease.ip[3]
                )),
                Err(e) => self.set_err(format!("DHCP error: {}", e)),
            }
            return DesktopAction::None;
        }
        if btn_ping.contains(mx, my) {
            let gw = {
                let st = net::NET_STATE.lock();
                if st.gateway != [0, 0, 0, 0] {
                    st.gateway
                } else {
                    [10, 0, 2, 2]
                }
            };
            match icmp::send_echo_and_wait(gw, 0x414D, 1, 1200) {
                Ok(rep) => self.set_ok(format!(
                    "Ping GW {}.{}.{}.{}: {} ms (ttl={})",
                    rep.from_ip[0], rep.from_ip[1], rep.from_ip[2], rep.from_ip[3], rep.rtt_ms, rep.ttl
                )),
                Err(e) => self.set_err(format!("Ping failed: {}", e)),
            }
            return DesktopAction::None;
        }
        if btn_viewer.contains(mx, my) {
            return DesktopAction::OpenApp(BuiltinAppKind::NetViewer);
        }
        DesktopAction::None
    }

    pub fn draw(&self, canvas: &mut Canvas<'_>, area: Rect) {
        let pal = theme::palette();
        let cfg = personalization::snapshot();
        let typo = &cfg.typography;
        let mono_ui = typo.ui_font == UiFontFamily::JetBrainsMono;

        canvas.fill_rect(area, pal.win_bg_active);

        // Sidebar
        let sidebar = Rect::new(area.x, area.y, SIDEBAR_W, area.h);
        canvas.fill_rect(sidebar, pal.surface_sidebar);
        canvas.fill_rect(Rect::new(area.x + SIDEBAR_W - 1, area.y, 1, area.h), pal.surface_border);
        canvas.draw_text_bold(area.x + 12, area.y + 12, "SETTINGS", pal.text_muted);

        for (i, sec) in SettingsSection::ALL.iter().enumerate() {
            let r = Rect::new(area.x + 10, area.y + 38 + (i as i32) * 33, SIDEBAR_W - 20, 28);
            let active = self.section == *sec;
            if active {
                canvas.fill_rounded_rect(r, 7, pal.surface_selected);
            }
            canvas.draw_lucide_icon(
                r.x + 8,
                r.y + 6,
                sec.icon(),
                if active { pal.accent_primary } else { pal.text_secondary },
            );
            canvas.draw_ui_text_clipped(
                r.x + 28,
                r.y + 6,
                sec.label(),
                if active { pal.text_primary } else { pal.text_secondary },
                typo.ui_scale_q8.min(264),
                active,
                mono_ui,
                r,
            );
        }

        // Sidebar footer: Save & Reset to Defaults
        let btn_save = Rect::new(area.x + 10, area.bottom() - 68, SIDEBAR_W - 20, 26);
        canvas.fill_rounded_rect(btn_save, 6, pal.accent_primary);
        let save_tw = text_width_bold("Save Config");
        canvas.draw_text_bold_clipped(
            btn_save.x + ((btn_save.w - save_tw) / 2).max(4),
            btn_save.y + 5,
            "Save Config",
            pal.on_accent,
            btn_save,
        );

        let btn_reset = Rect::new(area.x + 10, area.bottom() - 36, SIDEBAR_W - 20, 26);
        canvas.fill_rounded_rect(btn_reset, 6, pal.surface_card);
        canvas.stroke_rounded_rect(btn_reset, 6, pal.surface_border, 190);
        let reset_tw = text_width("Reset Defaults", 1);
        canvas.draw_text_clipped(
            btn_reset.x + ((btn_reset.w - reset_tw) / 2).max(4),
            btn_reset.y + 5,
            "Reset Defaults",
            pal.text_primary,
            1,
            btn_reset,
        );

        // Main scrollable content viewport
        let view = Rect::new(
            area.x + SIDEBAR_W + 14,
            area.y + 12,
            area.w - SIDEBAR_W - 28,
            area.h - 40,
        );
        let content_h = match self.section {
            SettingsSection::Appearance => 420,
            SettingsSection::Wallpaper => 380,
            SettingsSection::Desktop => 440,
            SettingsSection::WindowsMotion => 360,
            SettingsSection::Typography => 340,
            SettingsSection::Sound | SettingsSection::Network => 260,
        };
        let max_scroll = (content_h - view.h).max(0);
        self.last_max_scroll.set(max_scroll);
        let eff_scroll = self.scroll_y.clamp(0, max_scroll);
        let vmain = Rect::new(view.x, view.y - eff_scroll, view.w, content_h);

        let prev_clip = canvas.clip;
        canvas.clip = prev_clip.intersect(view);
        match self.section {
            SettingsSection::Appearance => self.draw_appearance(canvas, vmain, view, &cfg),
            SettingsSection::Wallpaper => self.draw_wallpaper(canvas, vmain, view, &cfg),
            SettingsSection::Desktop => self.draw_desktop(canvas, vmain, view, &cfg),
            SettingsSection::WindowsMotion => self.draw_windows_motion(canvas, vmain, view, &cfg),
            SettingsSection::Typography => self.draw_typography(canvas, vmain, view, &cfg),
            SettingsSection::Sound => self.draw_sound(canvas, vmain, view),
            SettingsSection::Network => self.draw_network(canvas, vmain, view),
        }
        canvas.clip = prev_clip;

        // Status bar at bottom
        let st_r = Rect::new(area.x + SIDEBAR_W + 14, area.bottom() - 24, area.w - SIDEBAR_W - 28, 20);
        let err_extra = personalization::last_error();
        let (msg, col) = if let Some(ref pe) = err_extra {
            (pe.as_str(), ACCENT_ERR)
        } else {
            (
                self.status_msg.as_str(),
                if self.is_error_status { ACCENT_ERR } else { pal.text_muted },
            )
        };
        canvas.draw_text_clipped(st_r.x, st_r.y, msg, col, 1, st_r);
    }

    fn draw_appearance(&self, canvas: &mut Canvas<'_>, main: Rect, clip: Rect, cfg: &SystemAppearance) {
        let pal = theme::palette();
        let cur_mode = theme::current_theme_mode();
        let typo = &cfg.typography;
        let mono_ui = typo.ui_font == UiFontFamily::JetBrainsMono;

        canvas.draw_ui_text_clipped(
            main.x,
            main.y,
            &format!("Appearance — Profile: {}", cfg.active_profile_name),
            pal.text_primary,
            typo.header_scale_q8,
            true,
            mono_ui,
            clip,
        );

        let card_w = ((main.w - 12) / 2).max(120);
        let modes = [
            (ThemePreference::Dark, "Black", false),
            (ThemePreference::Light, "White", true),
        ];
        for (i, (pref, label, light_prev)) in modes.iter().enumerate() {
            let r = Rect::new(main.x + (i as i32) * (card_w + 12), main.y + 30, card_w, 66);
            draw_theme_preview(canvas, r, label, *light_prev, cfg.theme == *pref, pal.accent_primary);
        }

        // Surface Style
        canvas.draw_text_bold_clipped(main.x, main.y + 110, "Surface Style:", pal.text_secondary, clip);
        for (i, st) in [SurfaceStyle::Standard, SurfaceStyle::Midnight, SurfaceStyle::Snow].iter().enumerate() {
            let sr = Rect::new(main.x + 106 + (i as i32) * 92, main.y + 106, 86, 24);
            let act = cfg.surface_style == *st;
            canvas.fill_rounded_rect(sr, 6, if act { pal.surface_selected } else { pal.surface_card });
            canvas.stroke_rounded_rect(sr, 6, if act { pal.accent_primary } else { pal.surface_border }, 190);
            let stw = text_width(st.title(), 1);
            canvas.draw_text_clipped(
                sr.x + ((sr.w - stw) / 2).max(4),
                sr.y + 4,
                st.title(),
                pal.text_primary,
                1,
                sr,
            );
        }

        // Accent & Selection Color
        canvas.draw_text_bold_clipped(main.x, main.y + 138, "Accent & Selection Colors", pal.text_primary, clip);
        for (i, preset) in AccentPreset::BUILTIN.iter().enumerate() {
            let sr = Rect::new(main.x + (i as i32) * 32, main.y + 158, 24, 24);
            let act = cfg.accent == *preset && !self.editing_selection_rgb;
            let pcol = preset.color_for_mode(cur_mode);
            canvas.draw_circle(sr.x + 12, sr.y + 12, 11, pcol);
            canvas.stroke_rounded_rect(sr, 12, pal.surface_border, 155);
            if act {
                canvas.stroke_rounded_rect(sr.expanded(2), 14, pal.text_primary, 235);
            }
        }

        draw_pill_btn(
            canvas,
            Rect::new(main.x + 266, main.y + 158, 72, 24),
            "Accent",
            !self.editing_selection_rgb,
            pal.accent_primary,
        );
        draw_pill_btn(
            canvas,
            Rect::new(main.x + 344, main.y + 158, 86, 24),
            "Selection",
            self.editing_selection_rgb,
            pal.accent_primary,
        );
        draw_pill_btn(
            canvas,
            Rect::new(main.x + 436, main.y + 158, 86, 24),
            "Sync Accent",
            matches!(cfg.selection, SelectionColorMode::FollowAccent),
            pal.accent_green,
        );

        // RGB sliders
        let accent_base = cfg.accent.color_for_mode(cur_mode);
        let active_col = if self.editing_selection_rgb {
            cfg.selection.resolve(accent_base)
        } else {
            accent_base
        };
        let r_ch = ((active_col >> 16) & 0xFF) as i32;
        let g_ch = ((active_col >> 8) & 0xFF) as i32;
        let b_ch = (active_col & 0xFF) as i32;
        let slider_w = (main.w - 170).max(120);
        let chs = [("R", r_ch, 0xFFE5484D), ("G", g_ch, 0xFF30A46C), ("B", b_ch, 0xFF3B82F6)];
        for (idx, (lbl, val, fill_col)) in chs.iter().enumerate() {
            let sy = main.y + 194 + (idx as i32) * 20;
            canvas.draw_text_bold_clipped(main.x, sy - 1, lbl, pal.text_secondary, clip);
            let sr = Rect::new(main.x + 30, sy, slider_w, 12);
            canvas.fill_rounded_rect(sr, 6, pal.surface_card);
            canvas.stroke_rounded_rect(sr, 6, pal.surface_border, 175);
            let fw = ((sr.w * (*val)) / 255).clamp(4, sr.w);
            canvas.fill_rounded_rect(Rect::new(sr.x, sr.y, fw, sr.h), 6, *fill_col);
            canvas.draw_text_clipped(sr.right() + 8, sy - 2, &format!("{:3}", val), pal.text_muted, 1, clip);
        }

        // Hex color swatch preview
        let sw_r = Rect::new(main.right() - 88, main.y + 194, 84, 54);
        canvas.fill_rounded_rect(sw_r, 8, active_col);
        canvas.stroke_rounded_rect(sw_r, 8, pal.surface_border, 200);
        let hex_lbl = format!("#{:06X}", active_col & 0x00FF_FFFF);
        let hex_tw = text_width(&hex_lbl, 1);
        canvas.draw_text_clipped(
            sw_r.x + ((sw_r.w - hex_tw) / 2).max(4),
            sw_r.y + 19,
            &hex_lbl,
            theme::contrast_on(active_col),
            1,
            sw_r,
        );

        // Profiles Box
        canvas.draw_text_bold_clipped(main.x, main.y + 256, "Saved Theme Profiles (.toml)", pal.text_primary, clip);
        let prof_box = Rect::new(main.x, main.y + 278, main.w, 128);
        canvas.fill_rounded_rect(prof_box, 8, pal.surface_card);
        canvas.stroke_rounded_rect(prof_box, 8, pal.surface_border, 185);

        for (i, p) in self.profiles.iter().take(4).enumerate() {
            let row_r = Rect::new(prof_box.x + 8, prof_box.y + 6 + (i as i32) * 28, prof_box.w - 140, 24);
            let sel = i == self.selected_profile_idx;
            let active = p.slug == cfg.active_profile_slug;
            if sel {
                canvas.fill_rounded_rect(row_r, 6, pal.surface_selected);
            }
            canvas.draw_circle(row_r.x + 12, row_r.y + 12, 6, p.accent_color);
            let tag = if active {
                " [Active]"
            } else if p.protected {
                " [Protected]"
            } else {
                ""
            };
            canvas.draw_text_clipped(
                row_r.x + 24,
                row_r.y + 4,
                &format!("{}{}", p.name, tag),
                if active { pal.accent_primary } else { pal.text_primary },
                1,
                row_r,
            );
        }

        let bx = prof_box.right() - 122;
        draw_pill_btn(canvas, Rect::new(bx, prof_box.y + 8, 114, 24), "Apply", true, pal.accent_primary);
        draw_pill_btn(canvas, Rect::new(bx, prof_box.y + 36, 114, 24), "+ New Profile", false, pal.accent_primary);
        draw_pill_btn(canvas, Rect::new(bx, prof_box.y + 64, 114, 24), "Rename", false, pal.accent_primary);
        draw_pill_btn(canvas, Rect::new(bx, prof_box.y + 92, 114, 24), "Delete", false, ACCENT_ERR);

        if matches!(self.input_target, InputTarget::NewProfile | InputTarget::RenameProfile) {
            let ib = Rect::new(prof_box.x + 8, prof_box.bottom() - 28, prof_box.w - 140, 24);
            canvas.fill_rounded_rect(ib, 6, pal.modal_input_bg);
            canvas.stroke_rounded_rect(ib, 6, pal.accent_primary, 220);
            canvas.draw_text_clipped(ib.x + 8, ib.y + 4, &format!("> {}_", self.input_buf), pal.text_primary, 1, ib);
        }
    }

    fn draw_wallpaper(&self, canvas: &mut Canvas<'_>, main: Rect, clip: Rect, cfg: &SystemAppearance) {
        let pal = theme::palette();
        let active_preset = match &cfg.wallpaper_source {
            WallpaperSource::Preset(p) => Some(*p),
            WallpaperSource::CustomFile(_) => None,
        };

        canvas.draw_ui_text_clipped(
            main.x,
            main.y,
            "Desktop Wallpaper & Backdrop",
            pal.text_primary,
            cfg.typography.header_scale_q8,
            true,
            cfg.typography.ui_font == UiFontFamily::JetBrainsMono,
            clip,
        );

        let wp_y = main.y + 32;
        let wp_gap = 8;
        let wp_w = ((main.w - wp_gap * 4) / 5).max(58);
        for (i, preset) in WallpaperPreset::ALL.iter().enumerate() {
            let wr = Rect::new(main.x + (i as i32) * (wp_w + wp_gap), wp_y, wp_w, 86);
            draw_wallpaper_card(canvas, wr, *preset, active_preset == Some(*preset), pal.accent_primary);
        }

        // Scaling Mode & Animated Shimmer
        canvas.draw_text_bold_clipped(main.x, main.y + 136, "Scale Mode:", pal.text_secondary, clip);
        for (i, sm) in WallpaperScaleMode::ALL.iter().enumerate() {
            let sr = Rect::new(main.x + 92 + (i as i32) * 76, main.y + 132, 70, 26);
            draw_pill_btn(canvas, sr, sm.title(), cfg.wallpaper_scale == *sm, pal.accent_primary);
        }

        let anim_btn = Rect::new(main.x + 410, main.y + 132, 128, 26);
        draw_pill_btn(
            canvas,
            anim_btn,
            if cfg.wallpaper_animated { "Animated: ON" } else { "Animated: OFF" },
            cfg.wallpaper_animated,
            pal.accent_green,
        );

        // Custom Image File Picker
        canvas.draw_text_bold_clipped(
            main.x,
            main.y + 168,
            "Custom Wallpaper Image (BMP / TGA / PPM from VFS)",
            pal.text_primary,
            clip,
        );

        let path_box = Rect::new(main.x, main.y + 190, (main.w - 94).max(160), 28);
        canvas.fill_rounded_rect(path_box, 6, pal.modal_input_bg);
        let editing = self.input_target == InputTarget::WallpaperPath;
        canvas.stroke_rounded_rect(
            path_box,
            6,
            if editing { pal.accent_primary } else { pal.surface_border },
            200,
        );
        let show_txt = if editing {
            format!("{}_", self.input_buf)
        } else {
            match &cfg.wallpaper_source {
                WallpaperSource::Preset(p) => format!("Preset: {} (click to enter custom path)", p.title()),
                WallpaperSource::CustomFile(p) => p.clone(),
            }
        };
        canvas.draw_text_clipped(path_box.x + 8, path_box.y + 6, &show_txt, pal.text_primary, 1, path_box);

        let apply_btn = Rect::new(path_box.right() + 8, main.y + 190, 84, 28);
        draw_pill_btn(canvas, apply_btn, "Apply", true, pal.accent_primary);

        let list_box = Rect::new(main.x, main.y + 226, main.w, 146);
        canvas.fill_rounded_rect(list_box, 8, pal.surface_card);
        canvas.stroke_rounded_rect(list_box, 8, pal.surface_border, 175);
        if self.image_files.is_empty() {
            canvas.draw_text_clipped(
                list_box.x + 12,
                list_box.y + 14,
                "No .bmp/.tga/.ppm images found in /home or /disk",
                pal.text_muted,
                1,
                list_box,
            );
        } else {
            for (i, path) in self.image_files.iter().take(5).enumerate() {
                let rr = Rect::new(main.x + 8, main.y + 230 + (i as i32) * 28, main.w - 16, 24);
                let sel = matches!(&cfg.wallpaper_source, WallpaperSource::CustomFile(cp) if cp == path);
                if sel {
                    canvas.fill_rounded_rect(rr, 6, pal.surface_selected);
                }
                canvas.draw_text_clipped(
                    rr.x + 8,
                    rr.y + 4,
                    path,
                    if sel { pal.accent_primary } else { pal.text_primary },
                    1,
                    rr,
                );
            }
        }
    }

    fn draw_desktop(&self, canvas: &mut Canvas<'_>, main: Rect, clip: Rect, cfg: &SystemAppearance) {
        let pal = theme::palette();
        let d = &cfg.desktop;

        canvas.draw_ui_text_clipped(
            main.x,
            main.y,
            "Dock, Top Bar & Control Center",
            pal.text_primary,
            cfg.typography.header_scale_q8,
            true,
            cfg.typography.ui_font == UiFontFamily::JetBrainsMono,
            clip,
        );

        canvas.draw_text_bold_clipped(main.x, main.y + 34, "Dock Position:", pal.text_secondary, clip);
        for (i, pos) in DockPosition::ALL.iter().enumerate() {
            let br = Rect::new(main.x + 110 + (i as i32) * 86, main.y + 30, 80, 26);
            draw_pill_btn(canvas, br, pos.title(), d.dock_position == *pos, pal.accent_primary);
        }

        // Dock Size
        canvas.draw_text_bold_clipped(main.x, main.y + 68, &format!("Icon Size: {}px", d.dock_icon_size), pal.text_secondary, clip);
        draw_slider(canvas, Rect::new(main.x + 110, main.y + 68, 170, 14), ((d.dock_icon_size as i32 - 32) * 100) / 20, pal.accent_primary);
        draw_pill_btn(canvas, Rect::new(main.x + 288, main.y + 64, 24, 24), "-", false, pal.accent_primary);
        draw_pill_btn(canvas, Rect::new(main.x + 316, main.y + 64, 24, 24), "+", false, pal.accent_primary);

        // Dock Gap
        canvas.draw_text_bold_clipped(main.x, main.y + 98, &format!("Icon Gap: {}px", d.dock_icon_gap), pal.text_secondary, clip);
        draw_slider(canvas, Rect::new(main.x + 110, main.y + 98, 170, 14), ((d.dock_icon_gap as i32 - 4) * 100) / 14, pal.accent_primary);
        draw_pill_btn(canvas, Rect::new(main.x + 288, main.y + 94, 24, 24), "-", false, pal.accent_primary);
        draw_pill_btn(canvas, Rect::new(main.x + 316, main.y + 94, 24, 24), "+", false, pal.accent_primary);

        let dock_toggles = [
            ("Magnify", d.dock_magnification),
            ("Auto-Hide", d.dock_auto_hide),
            ("Indicators", d.dock_show_indicators),
            ("Divider", d.dock_show_divider),
        ];
        let tw = ((main.w - 18) / 4).max(90);
        for (i, (lbl, on)) in dock_toggles.iter().enumerate() {
            let tr = Rect::new(main.x + (i as i32) * (tw + 6), main.y + 128, tw, 26);
            draw_pill_btn(canvas, tr, lbl, *on, pal.accent_primary);
        }

        // Top Bar
        canvas.draw_text_bold_clipped(main.x, main.y + 164, "Top Bar Elements", pal.text_primary, clip);
        let tb_items = [
            ("Clock", d.topbar_show_clock),
            ("24-Hour Time", d.topbar_clock_24h),
            ("Show Date", d.topbar_show_date),
            ("Alerts Bell", d.topbar_show_notifications),
            ("Network Icon", d.topbar_show_network),
            ("Audio Icon", d.topbar_show_audio),
        ];
        let tb_w = ((main.w - 12) / 3).max(110);
        for (idx, (lbl, on)) in tb_items.iter().enumerate() {
            let col = (idx % 3) as i32;
            let row = (idx / 3) as i32;
            let tr = Rect::new(main.x + col * (tb_w + 6), main.y + 186 + row * 30, tb_w, 25);
            draw_pill_btn(canvas, tr, lbl, *on, pal.accent_primary);
        }

        // Control Center Modules Order & Visibility
        canvas.draw_text_bold_clipped(main.x, main.y + 254, "Control Center Modules (Click to Toggle / Reorder)", pal.text_primary, clip);
        for (i, m) in d.cc_order.iter().enumerate() {
            let ry = main.y + 276 + (i as i32) * 26;
            let on = d.cc_enabled[cc_idx(*m)];
            let tog_r = Rect::new(main.x, ry, main.w - 72, 23);
            canvas.fill_rounded_rect(tog_r, 5, if on { pal.surface_selected } else { pal.surface_card });
            canvas.stroke_rounded_rect(tog_r, 5, if on { pal.accent_primary } else { pal.surface_border }, 175);
            canvas.draw_text_clipped(
                tog_r.x + 10,
                tog_r.y + 3,
                &format!("{}. {} — {}", i + 1, m.title(), if on { "Visible" } else { "Hidden" }),
                if on { pal.text_primary } else { pal.text_muted },
                1,
                tog_r,
            );
            draw_pill_btn(canvas, Rect::new(main.right() - 64, ry, 28, 23), "^", false, pal.accent_primary);
            draw_pill_btn(canvas, Rect::new(main.right() - 32, ry, 28, 23), "v", false, pal.accent_primary);
        }
    }

    fn draw_windows_motion(&self, canvas: &mut Canvas<'_>, main: Rect, clip: Rect, cfg: &SystemAppearance) {
        let pal = theme::palette();
        let w = &cfg.windows;
        let a = &cfg.animations;

        canvas.draw_ui_text_clipped(
            main.x,
            main.y,
            "Windows & Motion Preferences",
            pal.text_primary,
            cfg.typography.header_scale_q8,
            true,
            cfg.typography.ui_font == UiFontFamily::JetBrainsMono,
            clip,
        );

        canvas.draw_text_bold_clipped(main.x, main.y + 34, &format!("Corner Radius: {}px", w.corner_radius), pal.text_secondary, clip);
        draw_slider(canvas, Rect::new(main.x + 158, main.y + 34, 154, 14), ((w.corner_radius as i32) * 100) / 18, pal.accent_primary);
        for (i, rv) in [0u8, 6, 11, 16].iter().enumerate() {
            let br = Rect::new(main.x + 324 + (i as i32) * 46, main.y + 30, 40, 24);
            draw_pill_btn(canvas, br, &format!("{}", rv), w.corner_radius == *rv, pal.accent_primary);
        }

        let sh_tog = Rect::new(main.x, main.y + 68, 126, 26);
        draw_pill_btn(
            canvas,
            sh_tog,
            if w.shadows_enabled { "Shadows: ON" } else { "Shadows: OFF" },
            w.shadows_enabled,
            pal.accent_primary,
        );
        canvas.draw_text_clipped(main.x + 136, main.y + 72, &format!("{}%", w.shadow_intensity), pal.text_secondary, 1, clip);
        draw_slider(canvas, Rect::new(main.x + 210, main.y + 73, (main.w - 256).max(120), 14), w.shadow_intensity as i32, pal.accent_primary);

        let wt_w = ((main.w - 18) / 2).max(140);
        let w_items = [
            ("Active Window Highlight", w.active_highlight),
            ("Open Transition", w.open_animation),
            ("Close Transition", w.close_animation),
            ("Snap / Tile Transition", w.move_animation),
        ];
        for (idx, (lbl, on)) in w_items.iter().enumerate() {
            let col = (idx % 2) as i32;
            let row = (idx / 2) as i32;
            let r = Rect::new(main.x + col * (wt_w + 10), main.y + 108 + row * 32, wt_w, 26);
            draw_pill_btn(canvas, r, lbl, *on, pal.accent_primary);
        }

        canvas.draw_text_bold_clipped(main.x, main.y + 182, "Accessibility & System Animations", pal.text_primary, clip);
        let rm_btn = Rect::new(main.x, main.y + 206, main.w, 34);
        draw_pill_btn(
            canvas,
            rm_btn,
            if a.reduced_motion {
                "Reduced Motion: ENABLED (Instant Transitions)"
            } else {
                "Reduced Motion: OFF (Smooth Transitions Active)"
            },
            a.reduced_motion,
            ACCENT_WARN,
        );

        let at_w = ((main.w - 18) / 2).max(140);
        let a_items = [
            ("System Animations", a.system_animations),
            ("Window Animations", a.window_animations),
            ("Dock Magnification Anim", a.dock_animations),
            ("Hover Animations", a.hover_animations),
            ("Wallpaper Shimmer", a.wallpaper_animations),
        ];
        for (idx, (lbl, on)) in a_items.iter().enumerate() {
            let col = (idx % 2) as i32;
            let row = (idx / 2) as i32;
            let r = Rect::new(main.x + col * (at_w + 10), main.y + 250 + row * 32, at_w, 26);
            draw_pill_btn(canvas, r, lbl, *on, pal.accent_primary);
        }
    }

    fn draw_typography(&self, canvas: &mut Canvas<'_>, main: Rect, clip: Rect, cfg: &SystemAppearance) {
        let pal = theme::palette();
        let t = &cfg.typography;
        let mono_ui = t.ui_font == UiFontFamily::JetBrainsMono;

        canvas.draw_ui_text_clipped(
            main.x,
            main.y,
            "System Typography & Readability",
            pal.text_primary,
            t.header_scale_q8,
            true,
            mono_ui,
            clip,
        );

        canvas.draw_text_bold_clipped(main.x, main.y + 36, "UI Font Family:", pal.text_secondary, clip);
        for (i, uf) in [UiFontFamily::JakartaSans, UiFontFamily::JetBrainsMono].iter().enumerate() {
            let br = Rect::new(main.x + 124 + (i as i32) * 144, main.y + 32, 136, 26);
            draw_pill_btn(canvas, br, uf.title(), t.ui_font == *uf, pal.accent_primary);
        }

        canvas.draw_text_bold_clipped(main.x, main.y + 70, "Monospace Font:", pal.text_secondary, clip);
        for (i, mf) in [MonoFontFamily::JetBrainsMono, MonoFontFamily::Classic8x8].iter().enumerate() {
            let br = Rect::new(main.x + 124 + (i as i32) * 144, main.y + 66, 136, 26);
            draw_pill_btn(canvas, br, mf.title(), t.mono_font == *mf, pal.accent_primary);
        }

        canvas.draw_text_bold_clipped(main.x, main.y + 108, "UI Text Size:", pal.text_secondary, clip);
        for (i, (q8, lbl)) in [(256i32, "Compact"), (272, "Default"), (304, "Large")].iter().enumerate() {
            let br = Rect::new(main.x + 124 + (i as i32) * 102, main.y + 104, 96, 26);
            draw_pill_btn(canvas, br, lbl, t.ui_scale_q8 == *q8, pal.accent_primary);
        }

        canvas.draw_text_bold_clipped(main.x, main.y + 142, "Header Scale:", pal.text_secondary, clip);
        for (i, (q8, lbl)) in [(288i32, "Subtle"), (320, "Default"), (352, "Display")].iter().enumerate() {
            let br = Rect::new(main.x + 124 + (i as i32) * 102, main.y + 138, 96, 26);
            draw_pill_btn(canvas, br, lbl, t.header_scale_q8 == *q8, pal.accent_primary);
        }

        canvas.draw_text_bold_clipped(main.x, main.y + 176, "Terminal / Code:", pal.text_secondary, clip);
        for (i, (q8, lbl)) in [(256i32, "100%"), (288, "112%"), (320, "125%")].iter().enumerate() {
            let br = Rect::new(main.x + 124 + (i as i32) * 102, main.y + 172, 96, 26);
            draw_pill_btn(canvas, br, lbl, t.mono_scale_q8 == *q8, pal.accent_primary);
        }

        // Live Typography Preview Box
        let pbox = Rect::new(main.x, main.y + 214, main.w, 114);
        canvas.fill_rounded_rect(pbox, 9, pal.surface_card);
        canvas.stroke_rounded_rect(pbox, 9, pal.surface_border, 185);
        canvas.draw_ui_text_clipped(
            pbox.x + 14,
            pbox.y + 10,
            "Amaryllis OS — Live Typography Preview",
            pal.text_primary,
            t.header_scale_q8,
            true,
            mono_ui,
            pbox,
        );
        let sel_r = Rect::new(pbox.x + 14, pbox.y + 42, ui_text_width("Привет, Amaryllis!", t.ui_scale_q8, false, mono_ui) + 12, 24);
        canvas.fill_rounded_rect(sel_r, 5, pal.selection_color);
        canvas.draw_ui_text_clipped(
            pbox.x + 20,
            pbox.y + 45,
            "Привет, Amaryllis! System controls adapt cleanly.",
            pal.text_primary,
            t.ui_scale_q8,
            false,
            mono_ui,
            pbox,
        );
        canvas.draw_text_mono_ex_clipped(
            pbox.x + 14,
            pbox.y + 76,
            "fn init_os(accent: u32) -> &str { \"JetBrains / 8x8\" }",
            pal.accent_primary,
            t.mono_scale_q8,
            t.mono_font == MonoFontFamily::Classic8x8,
            pbox,
        );
    }

    fn draw_sound(&self, canvas: &mut Canvas<'_>, main: Rect, clip: Rect) {
        let pal = theme::palette();
        let st = drivers::audio_stats();
        let snap = mixer::snapshot();

        canvas.draw_text_scaled_clipped(main.x, main.y, "Sound & Audio Mixer", pal.text_primary, 320, true, clip);

        let card = Rect::new(main.x, main.y + 32, main.w, 84);
        canvas.fill_rounded_rect(card, 9, pal.surface_card);
        canvas.stroke_rounded_rect(card, 9, pal.surface_border, 180);
        canvas.draw_text_bold_clipped(
            card.x + 12,
            card.y + 10,
            if st.initialized { st.driver_name } else { "Hardware Audio Not Detected" },
            if st.initialized { ACCENT_MINT } else { ACCENT_ERR },
            card,
        );
        let io_line = if st.mmio_phys != 0 {
            format!("MMIO: 0x{:x}  IRQ: {}  Buffers: {}", st.mmio_phys, st.irq_line, st.buffers_played)
        } else {
            format!("NAMBAR: 0x{:04x}  NABMBAR: 0x{:04x}  IRQ: {}", st.nambar, st.nabmbar, st.irq_line)
        };
        canvas.draw_text_clipped(
            card.x + 12,
            card.y + 34,
            &io_line,
            pal.text_primary,
            1,
            card,
        );
        canvas.draw_text_clipped(
            card.x + 12,
            card.y + 54,
            &format!("Sample Rate: {} Hz  VRA: {}  Active Stream: {}", st.sample_rate, if st.vra_supported { "Yes" } else { "Fixed 48k" }, snap.title),
            pal.text_secondary,
            1,
            card,
        );

        canvas.draw_text_bold_clipped(main.x, main.y + 118, &format!("Master Output Volume: {}%", snap.volume_percent), pal.text_primary, clip);
        draw_slider(canvas, Rect::new(main.x, main.y + 138, main.w.min(340), 16), snap.volume_percent as i32, pal.accent_primary);

        for (i, (pct, lbl)) in [(0u8, "Mute"), (25, "25%"), (50, "50%"), (75, "75%"), (100, "100%")].iter().enumerate() {
            let br = Rect::new(main.x + (i as i32) * 62, main.y + 164, 56, 26);
            draw_pill_btn(canvas, br, lbl, snap.volume_percent == *pct, pal.accent_primary);
        }

        let btn_chime = Rect::new(main.x, main.y + 208, 136, 30);
        let btn_player = Rect::new(main.x + 146, main.y + 208, 164, 30);
        canvas.fill_rounded_rect(btn_chime, 7, pal.accent_green);
        let chime_tw = text_width_bold("Test Chime");
        canvas.draw_text_bold_clipped(
            btn_chime.x + ((btn_chime.w - chime_tw) / 2).max(4),
            btn_chime.y + 7,
            "Test Chime",
            theme::contrast_on(pal.accent_green),
            btn_chime,
        );
        canvas.fill_rounded_rect(btn_player, 7, pal.accent_primary);
        let player_tw = text_width_bold("Open Audio Player");
        canvas.draw_text_bold_clipped(
            btn_player.x + ((btn_player.w - player_tw) / 2).max(4),
            btn_player.y + 7,
            "Open Audio Player",
            pal.on_accent,
            btn_player,
        );
    }

    fn draw_network(&self, canvas: &mut Canvas<'_>, main: Rect, clip: Rect) {
        let pal = theme::palette();
        let nic = drivers::net_stats();
        let ns = net::NET_STATE.lock();
        let (ip, mask, gw, dns, dhcp_ok) = (ns.ip, ns.netmask, ns.gateway, ns.dns, ns.dhcp_configured);
        drop(ns);

        canvas.draw_text_scaled_clipped(main.x, main.y, "Network Interface", pal.text_primary, 320, true, clip);

        let card = Rect::new(main.x, main.y + 32, main.w, 144);
        canvas.fill_rounded_rect(card, 9, pal.surface_card);
        canvas.stroke_rounded_rect(card, 9, pal.surface_border, 180);

        let title_buf = if nic.initialized && nic.link_up {
            format!("{} — Link UP", nic.driver_name)
        } else if nic.initialized {
            format!("{} — Link DOWN", nic.driver_name)
        } else {
            String::from("Network Controller Not Found")
        };
        canvas.draw_text_bold_clipped(
            card.x + 12,
            card.y + 10,
            &title_buf,
            if nic.link_up { ACCENT_MINT } else { ACCENT_WARN },
            card,
        );
        canvas.draw_text_clipped(
            card.x + 12,
            card.y + 34,
            &format!("MAC: {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}   IRQ: {}", nic.mac[0], nic.mac[1], nic.mac[2], nic.mac[3], nic.mac[4], nic.mac[5], nic.irq_line),
            pal.text_primary,
            1,
            card,
        );
        canvas.draw_text_clipped(
            card.x + 12,
            card.y + 56,
            &format!("IPv4: {}.{}.{}.{}   Mask: {}.{}.{}.{}   ({})", ip[0], ip[1], ip[2], ip[3], mask[0], mask[1], mask[2], mask[3], if dhcp_ok { "DHCP" } else { "Unconfigured" }),
            pal.text_primary,
            1,
            card,
        );
        canvas.draw_text_clipped(
            card.x + 12,
            card.y + 78,
            &format!("Gateway: {}.{}.{}.{}   DNS: {}.{}.{}.{}", gw[0], gw[1], gw[2], gw[3], dns[0], dns[1], dns[2], dns[3]),
            pal.text_secondary,
            1,
            card,
        );
        canvas.draw_text_clipped(
            card.x + 12,
            card.y + 102,
            &format!("TX: {} pkts ({} B)   RX: {} pkts ({} B)   Err: {}", nic.tx_packets, nic.tx_bytes, nic.rx_packets, nic.rx_bytes, nic.tx_errors),
            pal.text_secondary,
            1,
            card,
        );

        let btn_dhcp = Rect::new(main.x, main.y + 190, 136, 30);
        let btn_ping = Rect::new(main.x + 146, main.y + 190, 132, 30);
        let btn_viewer = Rect::new(main.x + 288, main.y + 190, 146, 30);
        canvas.fill_rounded_rect(btn_dhcp, 7, pal.accent_green);
        let dhcp_tw = text_width_bold("Acquire DHCP");
        canvas.draw_text_bold_clipped(
            btn_dhcp.x + ((btn_dhcp.w - dhcp_tw) / 2).max(4),
            btn_dhcp.y + 7,
            "Acquire DHCP",
            theme::contrast_on(pal.accent_green),
            btn_dhcp,
        );
        canvas.fill_rounded_rect(btn_ping, 7, pal.surface_card);
        canvas.stroke_rounded_rect(btn_ping, 7, pal.surface_border, 190);
        let ping_tw = text_width_bold("Ping Gateway");
        canvas.draw_text_bold_clipped(
            btn_ping.x + ((btn_ping.w - ping_tw) / 2).max(4),
            btn_ping.y + 7,
            "Ping Gateway",
            pal.text_primary,
            btn_ping,
        );
        canvas.fill_rounded_rect(btn_viewer, 7, pal.accent_primary);
        let viewer_tw = text_width_bold("Open Net Viewer");
        canvas.draw_text_bold_clipped(
            btn_viewer.x + ((btn_viewer.w - viewer_tw) / 2).max(4),
            btn_viewer.y + 7,
            "Open Net Viewer",
            pal.on_accent,
            btn_viewer,
        );
    }
}

fn draw_pill_btn(canvas: &mut Canvas<'_>, r: Rect, label: &str, active: bool, accent: u32) {
    let pal = theme::palette();
    canvas.fill_rounded_rect(r, 6, if active { pal.surface_selected } else { pal.surface_card });
    canvas.stroke_rounded_rect(r, 6, if active { accent } else { pal.surface_border }, if active { 230 } else { 180 });
    let tw = text_width(label, 1);
    let tx = r.x + ((r.w - tw) / 2).max(4);
    let ty = r.y + ((r.h - 16) / 2).max(2);
    canvas.draw_text_clipped(tx, ty, label, if active { pal.text_primary } else { pal.text_secondary }, 1, r);
}

fn draw_slider(canvas: &mut Canvas<'_>, bar: Rect, pct: i32, accent: u32) {
    let pal = theme::palette();
    let rad = bar.h / 2;
    canvas.fill_rounded_rect(bar, rad, pal.surface_card);
    canvas.stroke_rounded_rect(bar, rad, pal.surface_border, 180);
    let fw = ((bar.w * pct.clamp(0, 100)) / 100).max(4);
    canvas.fill_rounded_rect(Rect::new(bar.x, bar.y, fw, bar.h), rad, accent);
}

fn draw_theme_preview(
    canvas: &mut Canvas<'_>,
    r: Rect,
    title: &str,
    is_light_preview: bool,
    selected: bool,
    accent: u32,
) {
    let pal = theme::palette();
    canvas.fill_rounded_rect(r, 9, pal.surface_card);
    canvas.stroke_rounded_rect(
        r,
        9,
        if selected { accent } else { pal.surface_border },
        if selected { 245 } else { 175 },
    );

    let mock = Rect::new(r.x + 8, r.y + 7, r.w - 16, r.h - 30);
    if is_light_preview {
        canvas.fill_gradient_v(mock, 0xFFE6E8EE, 0xFFF7F8FA);
        canvas.fill_rounded_rect(Rect::new(mock.x + 14, mock.y + 6, mock.w - 28, mock.h - 10), 4, 0xFFFFFFFF);
        canvas.stroke_rounded_rect(Rect::new(mock.x + 14, mock.y + 6, mock.w - 28, mock.h - 10), 4, 0xFFD0D4DC, 180);
    } else {
        canvas.fill_gradient_v(mock, 0xFF070709, 0xFF15151B);
        canvas.fill_rounded_rect(Rect::new(mock.x + 14, mock.y + 6, mock.w - 28, mock.h - 10), 4, 0xFF1A1A22);
        canvas.stroke_rounded_rect(Rect::new(mock.x + 14, mock.y + 6, mock.w - 28, mock.h - 10), 4, 0xFF2E2E38, 180);
    }

    let tw = text_width_bold(title);
    canvas.draw_text_bold_clipped(
        r.x + ((r.w - tw) / 2).max(6),
        r.bottom() - 20,
        title,
        if selected { accent } else { pal.text_primary },
        r,
    );
}

fn draw_wallpaper_card(
    canvas: &mut Canvas<'_>,
    r: Rect,
    preset: WallpaperPreset,
    selected: bool,
    accent: u32,
) {
    let pal = theme::palette();
    canvas.fill_rounded_rect(r, 9, pal.surface_card);
    canvas.stroke_rounded_rect(
        r,
        9,
        if selected { accent } else { pal.surface_border },
        if selected { 245 } else { 175 },
    );

    let (c_top, c_bot, c_glow) = preset.preview_colors();
    let art = Rect::new(r.x + 6, r.y + 6, r.w - 12, r.h - 30);
    if preset == WallpaperPreset::ObsidianWaves && pal.is_light {
        canvas.fill_gradient_v(art, 0xFFF2F3F7, 0xFFDADCE4);
    } else {
        canvas.fill_gradient_v(art, c_top, c_bot);
    }
    match preset {
        WallpaperPreset::ObsidianWaves => {
            let line_col = if pal.is_light { 0xFF18181D } else { c_glow };
            canvas.draw_line_aa(art.x + 4, art.bottom() - 6, art.right() - 4, art.y + 8, line_col, 130);
            canvas.draw_line_aa(art.x + 6, art.bottom() - 16, art.right() - 8, art.y + 16, line_col, 75);
        }
        WallpaperPreset::AuroraBorealis => {
            canvas.fill_rounded_rect_alpha(
                Rect::new(art.x + 4, art.y + art.h / 3, art.w - 8, 9),
                4,
                c_glow,
                165,
            );
        }
        WallpaperPreset::SunsetHorizon => {
            canvas.draw_circle_alpha(art.x + art.w / 2, art.bottom() - 10, 12, c_glow, 195);
            canvas.fill_rect(Rect::new(art.x, art.bottom() - 9, art.w, 9), 0xFF1A1016);
        }
        WallpaperPreset::SilkDunes => {
            canvas.fill_rounded_rect_alpha(
                Rect::new(art.x + 8, art.y + 8, art.w - 12, art.h - 14),
                8,
                c_glow,
                145,
            );
        }
        WallpaperPreset::MidnightMinimal => {
            canvas.draw_line_aa(art.x + 4, art.bottom() - 4, art.right() - 4, art.y + 6, c_glow, 170);
        }
    }
    canvas.stroke_rounded_rect(art, 5, pal.surface_border, 120);

    let lbl = preset.short_title();
    let tw = text_width(lbl, 1);
    let tx = r.x + ((r.w - tw) / 2).max(4);
    canvas.draw_text_clipped(
        tx,
        r.bottom() - 20,
        lbl,
        if selected { accent } else { pal.text_primary },
        1,
        r,
    );
}
