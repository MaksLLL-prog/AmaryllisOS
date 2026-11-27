use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use lazy_static::lazy_static;
use spin::Mutex;

use crate::arch::rtc;
use crate::graphics::theme::{self, ThemeMode, WallpaperPreset};
use crate::interrupts::timer;
use crate::storage::vfs::{self, VfsError};

const RAM_CONF_DIR: &str = "/home/amaryllis/.config/amaryllis";
const RAM_PROFILES_DIR: &str = "/home/amaryllis/.config/amaryllis/profiles";
const DISK_CONF_DIR: &str = "/disk/home/amaryllis/.config/amaryllis";
const DISK_PROFILES_DIR: &str = "/disk/home/amaryllis/.config/amaryllis/profiles";
const DISK_SYNC_DEBOUNCE_MS: u64 = 400;

pub const DEFAULT_PROFILE_SLUG: &str = "default";
pub const DEFAULT_PROFILE_NAME: &str = "Black (Default)";

static THEME_GEN: AtomicU64 = AtomicU64::new(1);
static WALLPAPER_GEN: AtomicU64 = AtomicU64::new(1);
static LAYOUT_GEN: AtomicU64 = AtomicU64::new(1);
static DISK_SYNC_PENDING: AtomicBool = AtomicBool::new(false);
static DISK_SYNC_DEADLINE_MS: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ThemePreference {
    Dark,
    Light,
}

impl ThemePreference {
    pub const fn title(self) -> &'static str {
        match self {
            Self::Dark => "Black",
            Self::Light => "White",
        }
    }

    pub const fn slug(self) -> &'static str {
        match self {
            Self::Dark => "dark",
            Self::Light => "light",
        }
    }

    pub fn from_slug(s: &str) -> Option<Self> {
        match s.trim() {
            "dark" | "black" | "auto" => Some(Self::Dark),
            "light" | "white" => Some(Self::Light),
            _ => None,
        }
    }

    pub fn resolve_mode(self, _hour: u8) -> ThemeMode {
        match self {
            Self::Dark => ThemeMode::Dark,
            Self::Light => ThemeMode::Light,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SurfaceStyle {
    Standard = 0,
    Midnight = 1,
    Snow = 2,
}

impl SurfaceStyle {
    pub const fn title(self) -> &'static str {
        match self {
            Self::Standard => "Default",
            Self::Midnight => "OLED Black",
            Self::Snow => "Pure White",
        }
    }

    pub const fn slug(self) -> &'static str {
        match self {
            Self::Standard => "standard",
            Self::Midnight => "midnight",
            Self::Snow => "snow",
        }
    }

    pub fn from_slug(s: &str) -> Option<Self> {
        match s.trim() {
            "standard" | "default" => Some(Self::Standard),
            "midnight" => Some(Self::Midnight),
            "snow" => Some(Self::Snow),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum AccentPreset {
    Monochrome,
    CobaltBlue,
    Cyan,
    Emerald,
    Teal,
    AmberOrange,
    CrimsonRed,
    SlateGray,
    Custom(u32),
}

impl AccentPreset {
    pub const BUILTIN: [Self; 8] = [
        Self::Monochrome,
        Self::CobaltBlue,
        Self::Cyan,
        Self::Emerald,
        Self::Teal,
        Self::AmberOrange,
        Self::CrimsonRed,
        Self::SlateGray,
    ];

    pub const fn color(self) -> u32 {
        match self {
            Self::Monochrome => 0xFFEAEAF0,
            Self::CobaltBlue => 0xFF2F80ED,
            Self::Cyan => 0xFF06B6D4,
            Self::Emerald => 0xFF10B981,
            Self::Teal => 0xFF14B8A6,
            Self::AmberOrange => 0xFFF97316,
            Self::CrimsonRed => 0xFFEF4444,
            Self::SlateGray => 0xFF71717A,
            Self::Custom(c) => 0xFF00_0000 | (c & 0x00FF_FFFF),
        }
    }

    pub const fn color_for_mode(self, mode: ThemeMode) -> u32 {
        match (self, mode) {
            (Self::Monochrome, ThemeMode::Light) => 0xFF18181D,
            (Self::Monochrome, ThemeMode::Dark) => 0xFFEAEAF0,
            _ => self.color(),
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Monochrome => "Mono",
            Self::CobaltBlue => "Blue",
            Self::Cyan => "Cyan",
            Self::Emerald => "Green",
            Self::Teal => "Teal",
            Self::AmberOrange => "Orange",
            Self::CrimsonRed => "Red",
            Self::SlateGray => "Slate",
            Self::Custom(_) => "Custom",
        }
    }

    pub fn from_u32_or_preset(col: u32) -> Self {
        let norm = 0xFF00_0000 | (col & 0x00FF_FFFF);
        // Migrate legacy purple / monochrome defaults transparently to Monochrome
        if norm == 0xFF8B5CF6
            || norm == 0xFF7B66E0
            || norm == 0xFF5E6AD2
            || norm == 0xFF18181D
            || norm == 0xFFEAEAF0
        {
            return Self::Monochrome;
        }
        for p in Self::BUILTIN {
            if p.color() == norm {
                return p;
            }
        }
        Self::Custom(norm)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SelectionColorMode {
    FollowAccent,
    Custom(u32),
}

impl SelectionColorMode {
    pub const fn resolve(self, accent: u32) -> u32 {
        match self {
            Self::FollowAccent => accent,
            Self::Custom(c) => 0xFF00_0000 | (c & 0x00FF_FFFF),
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub enum WallpaperSource {
    Preset(WallpaperPreset),
    CustomFile(String),
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum WallpaperScaleMode {
    Fill,
    Fit,
    Stretch,
    Center,
}

impl WallpaperScaleMode {
    pub const ALL: [Self; 4] = [Self::Fill, Self::Fit, Self::Stretch, Self::Center];

    pub const fn title(self) -> &'static str {
        match self {
            Self::Fill => "Fill",
            Self::Fit => "Fit",
            Self::Stretch => "Stretch",
            Self::Center => "Center",
        }
    }

    pub const fn slug(self) -> &'static str {
        match self {
            Self::Fill => "fill",
            Self::Fit => "fit",
            Self::Stretch => "stretch",
            Self::Center => "center",
        }
    }

    pub fn from_slug(s: &str) -> Option<Self> {
        match s.trim() {
            "fill" => Some(Self::Fill),
            "fit" => Some(Self::Fit),
            "stretch" => Some(Self::Stretch),
            "center" => Some(Self::Center),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum DockPosition {
    Bottom,
    Left,
    Right,
}

impl DockPosition {
    pub const ALL: [Self; 3] = [Self::Bottom, Self::Left, Self::Right];

    pub const fn title(self) -> &'static str {
        match self {
            Self::Bottom => "Bottom",
            Self::Left => "Left",
            Self::Right => "Right",
        }
    }

    pub const fn slug(self) -> &'static str {
        match self {
            Self::Bottom => "bottom",
            Self::Left => "left",
            Self::Right => "right",
        }
    }

    pub fn from_slug(s: &str) -> Option<Self> {
        match s.trim() {
            "bottom" => Some(Self::Bottom),
            "left" => Some(Self::Left),
            "right" => Some(Self::Right),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ControlCenterModule {
    Network = 0,
    Audio = 1,
    Theme = 2,
    Volume = 3,
    MediaPlayer = 4,
    Power = 5,
}

impl ControlCenterModule {
    pub const ALL: [Self; 6] = [
        Self::Network,
        Self::Audio,
        Self::Theme,
        Self::Volume,
        Self::MediaPlayer,
        Self::Power,
    ];

    pub const fn title(self) -> &'static str {
        match self {
            Self::Network => "Network",
            Self::Audio => "Audio Mute",
            Self::Theme => "Theme Switch",
            Self::Volume => "Volume Slider",
            Self::MediaPlayer => "Media Player",
            Self::Power => "Console / Exit",
        }
    }

    pub const fn slug(self) -> &'static str {
        match self {
            Self::Network => "network",
            Self::Audio => "audio",
            Self::Theme => "theme",
            Self::Volume => "volume",
            Self::MediaPlayer => "media",
            Self::Power => "power",
        }
    }

    pub fn from_slug(s: &str) -> Option<Self> {
        match s.trim() {
            "network" => Some(Self::Network),
            "audio" => Some(Self::Audio),
            "theme" => Some(Self::Theme),
            "volume" => Some(Self::Volume),
            "media" => Some(Self::MediaPlayer),
            "power" => Some(Self::Power),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum UiFontFamily {
    JakartaSans,
    JetBrainsMono,
}

impl UiFontFamily {
    pub const fn title(self) -> &'static str {
        match self {
            Self::JakartaSans => "Plus Jakarta",
            Self::JetBrainsMono => "JetBrains Mono",
        }
    }

    pub const fn slug(self) -> &'static str {
        match self {
            Self::JakartaSans => "jakarta",
            Self::JetBrainsMono => "jetbrains",
        }
    }

    pub fn from_slug(s: &str) -> Option<Self> {
        match s.trim() {
            "jakarta" | "sans" => Some(Self::JakartaSans),
            "jetbrains" | "mono" => Some(Self::JetBrainsMono),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum MonoFontFamily {
    JetBrainsMono,
    Classic8x8,
}

impl MonoFontFamily {
    pub const fn title(self) -> &'static str {
        match self {
            Self::JetBrainsMono => "JetBrains Mono",
            Self::Classic8x8 => "Classic 8x8",
        }
    }

    pub const fn slug(self) -> &'static str {
        match self {
            Self::JetBrainsMono => "jetbrains",
            Self::Classic8x8 => "classic8x8",
        }
    }

    pub fn from_slug(s: &str) -> Option<Self> {
        match s.trim() {
            "jetbrains" => Some(Self::JetBrainsMono),
            "classic8x8" | "8x8" => Some(Self::Classic8x8),
            _ => None,
        }
    }
}

#[derive(Clone)]
pub struct DesktopConfig {
    pub dock_position: DockPosition,
    pub dock_icon_size: u8,
    pub dock_icon_gap: u8,
    pub dock_magnification: bool,
    pub dock_auto_hide: bool,
    pub dock_show_indicators: bool,
    pub dock_show_divider: bool,
    pub topbar_show_clock: bool,
    pub topbar_clock_24h: bool,
    pub topbar_show_date: bool,
    pub topbar_show_notifications: bool,
    pub topbar_show_network: bool,
    pub topbar_show_audio: bool,
    pub cc_order: [ControlCenterModule; 6],
    pub cc_enabled: [bool; 6],
}

impl Default for DesktopConfig {
    fn default() -> Self {
        Self {
            dock_position: DockPosition::Bottom,
            dock_icon_size: 42,
            dock_icon_gap: 10,
            dock_magnification: true,
            dock_auto_hide: false,
            dock_show_indicators: true,
            dock_show_divider: true,
            topbar_show_clock: true,
            topbar_clock_24h: true,
            topbar_show_date: true,
            topbar_show_notifications: true,
            topbar_show_network: true,
            topbar_show_audio: true,
            cc_order: ControlCenterModule::ALL,
            cc_enabled: [true; 6],
        }
    }
}

#[derive(Clone)]
pub struct WindowConfig {
    pub corner_radius: u8,
    pub shadows_enabled: bool,
    pub shadow_intensity: u8,
    pub active_highlight: bool,
    pub open_animation: bool,
    pub close_animation: bool,
    pub move_animation: bool,
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            corner_radius: 11,
            shadows_enabled: true,
            shadow_intensity: 78,
            active_highlight: true,
            open_animation: true,
            close_animation: true,
            move_animation: true,
        }
    }
}

#[derive(Clone)]
pub struct TypographyConfig {
    pub ui_font: UiFontFamily,
    pub mono_font: MonoFontFamily,
    pub ui_scale_q8: i32,
    pub header_scale_q8: i32,
    pub mono_scale_q8: i32,
}

impl Default for TypographyConfig {
    fn default() -> Self {
        Self {
            ui_font: UiFontFamily::JakartaSans,
            mono_font: MonoFontFamily::JetBrainsMono,
            ui_scale_q8: 256,
            header_scale_q8: 320,
            mono_scale_q8: 256,
        }
    }
}

#[derive(Clone)]
pub struct AnimationConfig {
    pub system_animations: bool,
    pub window_animations: bool,
    pub dock_animations: bool,
    pub hover_animations: bool,
    pub wallpaper_animations: bool,
    pub reduced_motion: bool,
}

impl Default for AnimationConfig {
    fn default() -> Self {
        Self {
            system_animations: true,
            window_animations: true,
            dock_animations: true,
            hover_animations: true,
            wallpaper_animations: false,
            reduced_motion: false,
        }
    }
}

#[derive(Clone)]
pub struct SystemAppearance {
    pub active_profile_slug: String,
    pub active_profile_name: String,
    pub surface_style: SurfaceStyle,
    pub theme: ThemePreference,
    pub accent: AccentPreset,
    pub selection: SelectionColorMode,
    pub wallpaper_source: WallpaperSource,
    pub wallpaper_scale: WallpaperScaleMode,
    pub wallpaper_animated: bool,
    pub desktop: DesktopConfig,
    pub windows: WindowConfig,
    pub typography: TypographyConfig,
    pub animations: AnimationConfig,
}

impl Default for SystemAppearance {
    fn default() -> Self {
        Self {
            active_profile_slug: String::from(DEFAULT_PROFILE_SLUG),
            active_profile_name: String::from(DEFAULT_PROFILE_NAME),
            surface_style: SurfaceStyle::Standard,
            theme: ThemePreference::Dark,
            accent: AccentPreset::Monochrome,
            selection: SelectionColorMode::FollowAccent,
            wallpaper_source: WallpaperSource::Preset(WallpaperPreset::ObsidianWaves),
            wallpaper_scale: WallpaperScaleMode::Fill,
            wallpaper_animated: false,
            desktop: DesktopConfig::default(),
            windows: WindowConfig::default(),
            typography: TypographyConfig::default(),
            animations: AnimationConfig::default(),
        }
    }
}

impl SystemAppearance {
    pub fn clamp(&mut self) {
        if self.active_profile_slug.trim().is_empty() {
            self.active_profile_slug = String::from(DEFAULT_PROFILE_SLUG);
        }
        if self.active_profile_name.trim().is_empty() {
            self.active_profile_name = String::from(DEFAULT_PROFILE_NAME);
        }
        self.accent = AccentPreset::from_u32_or_preset(self.accent.color());
        if let SelectionColorMode::Custom(c) = self.selection {
            self.selection = SelectionColorMode::Custom(0xFF00_0000 | (c & 0x00FF_FFFF));
        }
        self.desktop.dock_icon_size = self.desktop.dock_icon_size.clamp(32, 52);
        self.desktop.dock_icon_gap = self.desktop.dock_icon_gap.clamp(4, 18);
        self.windows.corner_radius = self.windows.corner_radius.clamp(0, 18);
        self.windows.shadow_intensity = self.windows.shadow_intensity.clamp(0, 100);
        self.typography.ui_scale_q8 = self.typography.ui_scale_q8.clamp(240, 320);
        self.typography.header_scale_q8 = self.typography.header_scale_q8.clamp(280, 360);
        self.typography.mono_scale_q8 = self.typography.mono_scale_q8.clamp(256, 320);

        // Deduplicate & preserve full 6 modules in cc_order
        let mut seen = [false; 6];
        let mut cleaned = ControlCenterModule::ALL;
        let mut idx = 0usize;
        for m in self.desktop.cc_order {
            let k = m as usize;
            if k < 6 && !seen[k] {
                seen[k] = true;
                cleaned[idx] = m;
                idx += 1;
            }
        }
        for m in ControlCenterModule::ALL {
            let k = m as usize;
            if !seen[k] && idx < 6 {
                seen[k] = true;
                cleaned[idx] = m;
                idx += 1;
            }
        }
        self.desktop.cc_order = cleaned;
    }

    pub fn effective_theme_mode(&self, hour: u8) -> ThemeMode {
        self.theme.resolve_mode(hour)
    }

    pub fn effective_window_animations(&self) -> bool {
        !self.animations.reduced_motion
            && self.animations.system_animations
            && self.animations.window_animations
    }

    pub fn effective_dock_animations(&self) -> bool {
        !self.animations.reduced_motion
            && self.animations.system_animations
            && self.animations.dock_animations
    }

    pub fn effective_hover_animations(&self) -> bool {
        !self.animations.reduced_motion
            && self.animations.system_animations
            && self.animations.hover_animations
    }

    pub fn effective_wallpaper_animations(&self) -> bool {
        !self.animations.reduced_motion
            && self.animations.system_animations
            && self.animations.wallpaper_animations
            && self.wallpaper_animated
    }
}

#[derive(Clone)]
pub struct ProfileEntry {
    pub slug: String,
    pub name: String,
    pub builtin: bool,
    pub protected: bool,
    pub accent_color: u32,
    pub is_light: bool,
}

struct ServiceState {
    cfg: SystemAppearance,
    last_error: Option<String>,
    last_resolved_hour: u8,
    initialized: bool,
    ram_dirs_ready: bool,
    disk_dirs_ready: bool,
    ram_hashes: [u64; 4],
    disk_hashes: [u64; 4],
}

impl ServiceState {
    fn new() -> Self {
        Self {
            cfg: SystemAppearance::default(),
            last_error: None,
            last_resolved_hour: 12,
            initialized: false,
            ram_dirs_ready: false,
            disk_dirs_ready: false,
            ram_hashes: [0; 4],
            disk_hashes: [0; 4],
        }
    }
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

lazy_static! {
    static ref SERVICE: Mutex<ServiceState> = Mutex::new(ServiceState::new());
}

fn bump_all_generations() {
    THEME_GEN.fetch_add(1, Ordering::Release);
    WALLPAPER_GEN.fetch_add(1, Ordering::Release);
    LAYOUT_GEN.fetch_add(1, Ordering::Release);
}

#[inline]
pub fn generations() -> (u64, u64, u64) {
    (
        THEME_GEN.load(Ordering::Acquire),
        WALLPAPER_GEN.load(Ordering::Acquire),
        LAYOUT_GEN.load(Ordering::Acquire),
    )
}

pub fn snapshot() -> SystemAppearance {
    SERVICE.lock().cfg.clone()
}

pub fn last_error() -> Option<String> {
    SERVICE.lock().last_error.clone()
}

pub fn set_last_error(msg: Option<String>) {
    SERVICE.lock().last_error = msg;
}

fn sync_runtime_theme(cfg: &SystemAppearance, hour: u8) {
    let mode = cfg.effective_theme_mode(hour);
    let accent = cfg.accent.color_for_mode(mode);
    let sel_opt = match cfg.selection {
        SelectionColorMode::FollowAccent => None,
        SelectionColorMode::Custom(c) => Some(c),
    };
    let pal = theme::derive_palette(mode, cfg.surface_style as u8, accent, sel_opt);
    theme::apply_runtime_palette(mode, pal);
    if let WallpaperSource::Preset(preset) = cfg.wallpaper_source {
        theme::set_wallpaper(preset);
    }
}

fn builtin_midnight_profile() -> SystemAppearance {
    let mut c = SystemAppearance::default();
    c.active_profile_slug = String::from("midnight");
    c.active_profile_name = String::from("OLED Black");
    c.surface_style = SurfaceStyle::Midnight;
    c.theme = ThemePreference::Dark;
    c.accent = AccentPreset::Monochrome;
    c.selection = SelectionColorMode::FollowAccent;
    c.wallpaper_source = WallpaperSource::Preset(WallpaperPreset::ObsidianWaves);
    c.windows.shadow_intensity = 85;
    c
}

fn builtin_snow_profile() -> SystemAppearance {
    let mut c = SystemAppearance::default();
    c.active_profile_slug = String::from("snow");
    c.active_profile_name = String::from("Pure White");
    c.surface_style = SurfaceStyle::Snow;
    c.theme = ThemePreference::Light;
    c.accent = AccentPreset::Monochrome;
    c.selection = SelectionColorMode::FollowAccent;
    c.wallpaper_source = WallpaperSource::Preset(WallpaperPreset::ObsidianWaves);
    c.windows.shadow_intensity = 65;
    c
}

pub fn sanitize_profile_slug(name: &str) -> String {
    let mut out = String::new();
    for ch in name.chars() {
        let low = ch.to_ascii_lowercase();
        if low.is_ascii_alphanumeric() {
            out.push(low);
        } else if (low == ' ' || low == '-' || low == '_') && !out.ends_with('_') && !out.is_empty() {
            out.push('_');
        }
        if out.len() >= 28 {
            break;
        }
    }
    while out.ends_with('_') {
        out.pop();
    }
    out
}

fn format_hex_rgb(c: u32) -> String {
    format!("#{:06X}", c & 0x00FF_FFFF)
}

fn parse_hex_color(s: &str) -> Option<u32> {
    let t = s.trim().trim_start_matches('#').trim_start_matches("0x").trim_start_matches("0X");
    if t.len() != 6 {
        return None;
    }
    u32::from_str_radix(t, 16).ok().map(|v| 0xFF00_0000 | v)
}

fn parse_bool(s: &str) -> Option<bool> {
    match s.trim() {
        "true" | "1" | "yes" | "on" => Some(true),
        "false" | "0" | "no" | "off" => Some(false),
        _ => None,
    }
}

fn unquote(s: &str) -> String {
    let t = s.trim();
    if t.len() >= 2 && t.starts_with('"') && t.ends_with('"') {
        let inner = &t[1..t.len() - 1];
        let mut out = String::with_capacity(inner.len());
        let mut esc = false;
        for ch in inner.chars() {
            if esc {
                out.push(ch);
                esc = false;
            } else if ch == '\\' {
                esc = true;
            } else {
                out.push(ch);
            }
        }
        out
    } else {
        String::from(t)
    }
}

fn quote_toml(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for ch in s.chars() {
        if ch == '"' || ch == '\\' {
            out.push('\\');
        }
        out.push(ch);
    }
    out.push('"');
    out
}

pub fn serialize_system_toml(cfg: &SystemAppearance) -> String {
    format!(
        "[profile]\n\
         slug = {}\n\
         name = {}\n\n\
         [theme]\n\
         mode = \"{}\"\n\n\
         [typography]\n\
         ui_font = \"{}\"\n\
         mono_font = \"{}\"\n\
         ui_scale_q8 = {}\n\
         header_scale_q8 = {}\n\
         mono_scale_q8 = {}\n\n\
         [animations]\n\
         system = {}\n\
         window = {}\n\
         dock = {}\n\
         hover = {}\n\
         wallpaper = {}\n\
         reduced_motion = {}\n",
        quote_toml(&cfg.active_profile_slug),
        quote_toml(&cfg.active_profile_name),
        cfg.theme.slug(),
        cfg.typography.ui_font.slug(),
        cfg.typography.mono_font.slug(),
        cfg.typography.ui_scale_q8,
        cfg.typography.header_scale_q8,
        cfg.typography.mono_scale_q8,
        cfg.animations.system_animations,
        cfg.animations.window_animations,
        cfg.animations.dock_animations,
        cfg.animations.hover_animations,
        cfg.animations.wallpaper_animations,
        cfg.animations.reduced_motion,
    )
}

pub fn serialize_appearance_toml(cfg: &SystemAppearance) -> String {
    let (wp_kind, wp_val) = match &cfg.wallpaper_source {
        WallpaperSource::Preset(p) => ("preset", String::from(p.slug())),
        WallpaperSource::CustomFile(path) => ("file", path.clone()),
    };
    let (sel_mode, sel_col) = match cfg.selection {
        SelectionColorMode::FollowAccent => ("accent", cfg.accent.color()),
        SelectionColorMode::Custom(c) => ("custom", c),
    };
    format!(
        "[colors]\n\
         surface_style = \"{}\"\n\
         accent = \"{}\"\n\
         selection_mode = \"{}\"\n\
         selection_color = \"{}\"\n\n\
         [wallpaper]\n\
         source_kind = \"{}\"\n\
         source = {}\n\
         scale_mode = \"{}\"\n\
         animated = {}\n\n\
         [windows]\n\
         corner_radius = {}\n\
         shadows_enabled = {}\n\
         shadow_intensity = {}\n\
         active_highlight = {}\n\
         open_animation = {}\n\
         close_animation = {}\n\
         move_animation = {}\n",
        cfg.surface_style.slug(),
        format_hex_rgb(cfg.accent.color()),
        sel_mode,
        format_hex_rgb(sel_col),
        wp_kind,
        quote_toml(&wp_val),
        cfg.wallpaper_scale.slug(),
        cfg.wallpaper_animated,
        cfg.windows.corner_radius,
        cfg.windows.shadows_enabled,
        cfg.windows.shadow_intensity,
        cfg.windows.active_highlight,
        cfg.windows.open_animation,
        cfg.windows.close_animation,
        cfg.windows.move_animation,
    )
}

pub fn serialize_dock_toml(cfg: &SystemAppearance) -> String {
    let mut order_s = String::new();
    let mut enabled_s = String::new();
    for i in 0..6 {
        if i > 0 {
            order_s.push(',');
            enabled_s.push(',');
        }
        order_s.push_str(cfg.desktop.cc_order[i].slug());
        enabled_s.push_str(if cfg.desktop.cc_enabled[cc_idx(cfg.desktop.cc_order[i])] {
            "1"
        } else {
            "0"
        });
    }
    format!(
        "[dock]\n\
         position = \"{}\"\n\
         icon_size = {}\n\
         icon_gap = {}\n\
         magnification = {}\n\
         auto_hide = {}\n\
         show_indicators = {}\n\
         show_divider = {}\n\n\
         [topbar]\n\
         show_clock = {}\n\
         clock_24h = {}\n\
         show_date = {}\n\
         show_notifications = {}\n\
         show_network = {}\n\
         show_audio = {}\n\n\
         [control_center]\n\
         order = \"{}\"\n\
         enabled = \"{}\"\n",
        cfg.desktop.dock_position.slug(),
        cfg.desktop.dock_icon_size,
        cfg.desktop.dock_icon_gap,
        cfg.desktop.dock_magnification,
        cfg.desktop.dock_auto_hide,
        cfg.desktop.dock_show_indicators,
        cfg.desktop.dock_show_divider,
        cfg.desktop.topbar_show_clock,
        cfg.desktop.topbar_clock_24h,
        cfg.desktop.topbar_show_date,
        cfg.desktop.topbar_show_notifications,
        cfg.desktop.topbar_show_network,
        cfg.desktop.topbar_show_audio,
        order_s,
        enabled_s,
    )
}

pub fn serialize_full_profile_toml(cfg: &SystemAppearance) -> String {
    let mut out = serialize_system_toml(cfg);
    out.push('\n');
    out.push_str(&serialize_appearance_toml(cfg));
    out.push('\n');
    out.push_str(&serialize_dock_toml(cfg));
    out
}

/// Applies key=value lines from a TOML snippet into `cfg`, tolerating unknown keys & invalid values safely.
pub fn parse_toml_into(cfg: &mut SystemAppearance, text: &str) {
    let mut section = String::new();
    let mut wp_kind = String::new();
    let mut wp_src = String::new();
    let mut sel_mode = String::new();
    let mut sel_col = None;

    for raw in text.lines() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') && line.len() >= 2 {
            section = String::from(line[1..line.len() - 1].trim());
            continue;
        }
        let Some((k_raw, v_raw)) = line.split_once('=') else {
            continue;
        };
        let key = k_raw.trim();
        let val = unquote(v_raw);

        match (section.as_str(), key) {
            ("profile", "slug") => {
                let s = sanitize_profile_slug(&val);
                if !s.is_empty() {
                    cfg.active_profile_slug = s;
                }
            }
            ("profile", "name") => {
                let n = val.trim();
                if !n.is_empty() {
                    cfg.active_profile_name = String::from(n);
                }
            }
            ("theme", "mode") => {
                if let Some(m) = ThemePreference::from_slug(&val) {
                    cfg.theme = m;
                }
            }
            ("typography", "ui_font") => {
                if let Some(f) = UiFontFamily::from_slug(&val) {
                    cfg.typography.ui_font = f;
                }
            }
            ("typography", "mono_font") => {
                if let Some(f) = MonoFontFamily::from_slug(&val) {
                    cfg.typography.mono_font = f;
                }
            }
            ("typography", "ui_scale_q8") => {
                if let Ok(v) = val.trim().parse::<i32>() {
                    cfg.typography.ui_scale_q8 = v;
                }
            }
            ("typography", "header_scale_q8") => {
                if let Ok(v) = val.trim().parse::<i32>() {
                    cfg.typography.header_scale_q8 = v;
                }
            }
            ("typography", "mono_scale_q8") => {
                if let Ok(v) = val.trim().parse::<i32>() {
                    cfg.typography.mono_scale_q8 = v;
                }
            }
            ("animations", "system") => {
                if let Some(b) = parse_bool(&val) {
                    cfg.animations.system_animations = b;
                }
            }
            ("animations", "window") => {
                if let Some(b) = parse_bool(&val) {
                    cfg.animations.window_animations = b;
                }
            }
            ("animations", "dock") => {
                if let Some(b) = parse_bool(&val) {
                    cfg.animations.dock_animations = b;
                }
            }
            ("animations", "hover") => {
                if let Some(b) = parse_bool(&val) {
                    cfg.animations.hover_animations = b;
                }
            }
            ("animations", "wallpaper") => {
                if let Some(b) = parse_bool(&val) {
                    cfg.animations.wallpaper_animations = b;
                }
            }
            ("animations", "reduced_motion") => {
                if let Some(b) = parse_bool(&val) {
                    cfg.animations.reduced_motion = b;
                }
            }
            ("colors", "surface_style") => {
                if let Some(s) = SurfaceStyle::from_slug(&val) {
                    cfg.surface_style = s;
                }
            }
            ("colors", "accent") => {
                if let Some(c) = parse_hex_color(&val) {
                    cfg.accent = AccentPreset::from_u32_or_preset(c);
                }
            }
            ("colors", "selection_mode") => {
                sel_mode = val;
            }
            ("colors", "selection_color") => {
                sel_col = parse_hex_color(&val);
            }
            ("wallpaper", "source_kind") => {
                wp_kind = val;
            }
            ("wallpaper", "source") => {
                wp_src = val;
            }
            ("wallpaper", "scale_mode") => {
                if let Some(sm) = WallpaperScaleMode::from_slug(&val) {
                    cfg.wallpaper_scale = sm;
                }
            }
            ("wallpaper", "animated") => {
                if let Some(b) = parse_bool(&val) {
                    cfg.wallpaper_animated = b;
                }
            }
            ("windows", "corner_radius") => {
                if let Ok(v) = val.trim().parse::<u8>() {
                    cfg.windows.corner_radius = v;
                }
            }
            ("windows", "shadows_enabled") => {
                if let Some(b) = parse_bool(&val) {
                    cfg.windows.shadows_enabled = b;
                }
            }
            ("windows", "shadow_intensity") => {
                if let Ok(v) = val.trim().parse::<u8>() {
                    cfg.windows.shadow_intensity = v;
                }
            }
            ("windows", "active_highlight") => {
                if let Some(b) = parse_bool(&val) {
                    cfg.windows.active_highlight = b;
                }
            }
            ("windows", "open_animation") => {
                if let Some(b) = parse_bool(&val) {
                    cfg.windows.open_animation = b;
                }
            }
            ("windows", "close_animation") => {
                if let Some(b) = parse_bool(&val) {
                    cfg.windows.close_animation = b;
                }
            }
            ("windows", "move_animation") => {
                if let Some(b) = parse_bool(&val) {
                    cfg.windows.move_animation = b;
                }
            }
            ("dock", "position") => {
                if let Some(p) = DockPosition::from_slug(&val) {
                    cfg.desktop.dock_position = p;
                }
            }
            ("dock", "icon_size") => {
                if let Ok(v) = val.trim().parse::<u8>() {
                    cfg.desktop.dock_icon_size = v;
                }
            }
            ("dock", "icon_gap") => {
                if let Ok(v) = val.trim().parse::<u8>() {
                    cfg.desktop.dock_icon_gap = v;
                }
            }
            ("dock", "magnification") => {
                if let Some(b) = parse_bool(&val) {
                    cfg.desktop.dock_magnification = b;
                }
            }
            ("dock", "auto_hide") => {
                if let Some(b) = parse_bool(&val) {
                    cfg.desktop.dock_auto_hide = b;
                }
            }
            ("dock", "show_indicators") => {
                if let Some(b) = parse_bool(&val) {
                    cfg.desktop.dock_show_indicators = b;
                }
            }
            ("dock", "show_divider") => {
                if let Some(b) = parse_bool(&val) {
                    cfg.desktop.dock_show_divider = b;
                }
            }
            ("topbar", "show_clock") => {
                if let Some(b) = parse_bool(&val) {
                    cfg.desktop.topbar_show_clock = b;
                }
            }
            ("topbar", "clock_24h") => {
                if let Some(b) = parse_bool(&val) {
                    cfg.desktop.topbar_clock_24h = b;
                }
            }
            ("topbar", "show_date") => {
                if let Some(b) = parse_bool(&val) {
                    cfg.desktop.topbar_show_date = b;
                }
            }
            ("topbar", "show_notifications") => {
                if let Some(b) = parse_bool(&val) {
                    cfg.desktop.topbar_show_notifications = b;
                }
            }
            ("topbar", "show_network") => {
                if let Some(b) = parse_bool(&val) {
                    cfg.desktop.topbar_show_network = b;
                }
            }
            ("topbar", "show_audio") => {
                if let Some(b) = parse_bool(&val) {
                    cfg.desktop.topbar_show_audio = b;
                }
            }
            ("control_center", "order") => {
                let mut i = 0usize;
                for tok in val.split(',') {
                    if i >= 6 {
                        break;
                    }
                    if let Some(m) = ControlCenterModule::from_slug(tok) {
                        cfg.desktop.cc_order[i] = m;
                        i += 1;
                    }
                }
            }
            ("control_center", "enabled") => {
                for (i, tok) in val.split(',').take(6).enumerate() {
                    if let Some(m) = cfg.desktop.cc_order.get(i).copied() {
                        cfg.desktop.cc_enabled[cc_idx(m)] = tok.trim() != "0" && tok.trim() != "false";
                    }
                }
            }
            _ => {}
        }
    }

    if sel_mode.eq_ignore_ascii_case("custom") {
        if let Some(c) = sel_col {
            cfg.selection = SelectionColorMode::Custom(c);
        }
    } else if sel_mode.eq_ignore_ascii_case("accent") {
        cfg.selection = SelectionColorMode::FollowAccent;
    }

    if !wp_src.is_empty() {
        if wp_kind.eq_ignore_ascii_case("file") {
            cfg.wallpaper_source = WallpaperSource::CustomFile(wp_src);
        } else if let Some(preset) = WallpaperPreset::from_slug(&wp_src) {
            cfg.wallpaper_source = WallpaperSource::Preset(preset);
        }
    }

    cfg.clamp();
}

#[inline]
pub const fn cc_idx(m: ControlCenterModule) -> usize {
    m as usize
}

fn ensure_dir_chain(path: &str) -> Result<(), VfsError> {
    let mut cur = String::new();
    for seg in path.split('/').filter(|s| !s.is_empty()) {
        cur.push('/');
        cur.push_str(seg);
        let exists = vfs::with_vfs(|s| s.stat(&cur).is_ok());
        if !exists {
            match vfs::with_vfs(|s| s.create_dir(&cur)) {
                Ok(()) | Err(VfsError::AlreadyExists) => {}
                Err(e) => return Err(e),
            }
        }
    }
    Ok(())
}

fn direct_write_vfs(path: &str, data: &[u8]) -> Result<(), VfsError> {
    if let Some((parent, _)) = path.rsplit_once('/') {
        if !parent.is_empty() {
            ensure_dir_chain(parent)?;
        }
    }
    vfs::with_vfs(|s| s.write_file(path, data))
}

fn is_disk_writable() -> bool {
    vfs::with_vfs(|s| {
        s.is_disk_mounted()
            && s.volume_info_for("/disk").map(|v| !v.read_only).unwrap_or(false)
    })
}

fn seed_builtin_profiles_at(profiles_dir: &str) -> Result<(), VfsError> {
    ensure_dir_chain(profiles_dir)?;
    let builtins = [
        ("default", SystemAppearance::default()),
        ("midnight", builtin_midnight_profile()),
        ("snow", builtin_snow_profile()),
    ];
    for (slug, cfg) in builtins {
        let p = format!("{}/{}.toml", profiles_dir, slug);
        if vfs::with_vfs(|s| s.stat(&p).is_err()) {
            let txt = serialize_full_profile_toml(&cfg);
            direct_write_vfs(&p, txt.as_bytes())?;
        }
    }
    Ok(())
}

fn write_config_bundle_diff(
    dir: &str,
    profiles_dir: &str,
    cfg: &SystemAppearance,
    dirs_ready: &mut bool,
    hashes: &mut [u64; 4],
) -> Result<(), VfsError> {
    if !*dirs_ready {
        ensure_dir_chain(dir)?;
        ensure_dir_chain(profiles_dir)?;
        *dirs_ready = true;
    }

    let sys_txt = serialize_system_toml(cfg);
    let sys_h = fnv1a64(sys_txt.as_bytes());
    if hashes[0] != sys_h {
        vfs::with_vfs(|s| s.write_file(&format!("{}/system.toml", dir), sys_txt.as_bytes()))?;
        hashes[0] = sys_h;
    }

    let app_txt = serialize_appearance_toml(cfg);
    let app_h = fnv1a64(app_txt.as_bytes());
    if hashes[1] != app_h {
        vfs::with_vfs(|s| s.write_file(&format!("{}/appearance.toml", dir), app_txt.as_bytes()))?;
        hashes[1] = app_h;
    }

    let dock_txt = serialize_dock_toml(cfg);
    let dock_h = fnv1a64(dock_txt.as_bytes());
    if hashes[2] != dock_h {
        vfs::with_vfs(|s| s.write_file(&format!("{}/dock.toml", dir), dock_txt.as_bytes()))?;
        hashes[2] = dock_h;
    }

    if cfg.active_profile_slug != "default"
        && cfg.active_profile_slug != "midnight"
        && cfg.active_profile_slug != "snow"
    {
        let prof_txt = serialize_full_profile_toml(cfg);
        let prof_h = fnv1a64(prof_txt.as_bytes());
        if hashes[3] != prof_h {
            let pp = format!("{}/{}.toml", profiles_dir, cfg.active_profile_slug);
            vfs::with_vfs(|s| s.write_file(&pp, prof_txt.as_bytes()))?;
            hashes[3] = prof_h;
        }
    }
    Ok(())
}

fn sync_ram_now(cfg: &SystemAppearance) -> Result<(), String> {
    let (mut ram_ready, mut ram_hashes) = {
        let st = SERVICE.lock();
        (st.ram_dirs_ready, st.ram_hashes)
    };
    if let Err(e) = write_config_bundle_diff(
        RAM_CONF_DIR,
        RAM_PROFILES_DIR,
        cfg,
        &mut ram_ready,
        &mut ram_hashes,
    ) {
        let msg = format!("Save RAMFS config failed: {}", e.message());
        set_last_error(Some(msg.clone()));
        return Err(msg);
    }
    let mut st = SERVICE.lock();
    st.ram_dirs_ready = ram_ready;
    st.ram_hashes = ram_hashes;
    Ok(())
}

/// Immediately flushes any pending `/disk` personalization writes.
pub fn flush_disk_sync() -> Result<(), String> {
    let was_pending = DISK_SYNC_PENDING.swap(false, Ordering::AcqRel);
    if !was_pending {
        return Ok(());
    }
    let (cfg, mut disk_ready, mut disk_hashes) = {
        let st = SERVICE.lock();
        (st.cfg.clone(), st.disk_dirs_ready, st.disk_hashes)
    };
    if !is_disk_writable() {
        return Ok(());
    }
    if let Err(e) = write_config_bundle_diff(
        DISK_CONF_DIR,
        DISK_PROFILES_DIR,
        &cfg,
        &mut disk_ready,
        &mut disk_hashes,
    ) {
        let msg = format!("Saved to RAMFS; /disk sync failed: {}", e.message());
        set_last_error(Some(msg.clone()));
        return Err(msg);
    }
    let mut st = SERVICE.lock();
    st.disk_dirs_ready = disk_ready;
    st.disk_hashes = disk_hashes;
    st.last_error = None;
    drop(st);
    let _ = vfs::sync();
    Ok(())
}

/// Called periodically from the desktop event loop to flush debounced `/disk` saves outside UI clicks.
pub fn tick_disk_sync(now_ms: u64) {
    if !DISK_SYNC_PENDING.load(Ordering::Acquire) {
        return;
    }
    let deadline = DISK_SYNC_DEADLINE_MS.load(Ordering::Acquire);
    if now_ms >= deadline {
        let _ = flush_disk_sync();
    }
}

pub fn save_to_vfs() -> Result<(), String> {
    let cfg = SERVICE.lock().cfg.clone();
    sync_ram_now(&cfg)?;
    DISK_SYNC_PENDING.store(true, Ordering::Release);
    flush_disk_sync()?;
    set_last_error(None);
    Ok(())
}

fn try_load_bundle(dir: &str, cfg: &mut SystemAppearance) -> bool {
    let mut loaded_any = false;
    for fname in ["system.toml", "appearance.toml", "dock.toml"] {
        let path = format!("{}/{}", dir, fname);
        if let Ok(bytes) = vfs::with_vfs(|s| s.read_file(&path)) {
            if let Ok(txt) = core::str::from_utf8(&bytes) {
                parse_toml_into(cfg, txt);
                loaded_any = true;
            }
        }
    }
    loaded_any
}

pub fn init_from_vfs() {
    let hour = rtc::read_datetime().hour;
    let _ = seed_builtin_profiles_at(RAM_PROFILES_DIR);
    let disk_ok = is_disk_writable();
    if disk_ok {
        let _ = seed_builtin_profiles_at(DISK_PROFILES_DIR);
    }

    let mut cfg = SystemAppearance::default();
    let loaded = try_load_bundle(DISK_CONF_DIR, &mut cfg) || try_load_bundle(RAM_CONF_DIR, &mut cfg);
    cfg.clamp();

    let mut ram_ready = false;
    let mut disk_ready = false;
    let mut ram_hashes = [0u64; 4];
    let mut disk_hashes = [0u64; 4];

    if !loaded {
        let _ = write_config_bundle_diff(
            RAM_CONF_DIR,
            RAM_PROFILES_DIR,
            &cfg,
            &mut ram_ready,
            &mut ram_hashes,
        );
        if disk_ok {
            let _ = write_config_bundle_diff(
                DISK_CONF_DIR,
                DISK_PROFILES_DIR,
                &cfg,
                &mut disk_ready,
                &mut disk_hashes,
            );
        }
    } else {
        ram_hashes = [
            fnv1a64(serialize_system_toml(&cfg).as_bytes()),
            fnv1a64(serialize_appearance_toml(&cfg).as_bytes()),
            fnv1a64(serialize_dock_toml(&cfg).as_bytes()),
            0,
        ];
        if disk_ok {
            disk_hashes = ram_hashes;
        }
    }

    {
        let mut st = SERVICE.lock();
        st.cfg = cfg.clone();
        st.last_resolved_hour = hour;
        st.initialized = true;
        st.ram_dirs_ready = ram_ready;
        st.disk_dirs_ready = disk_ready;
        st.ram_hashes = ram_hashes;
        st.disk_hashes = disk_hashes;
        st.last_error = None;
    }
    DISK_SYNC_PENDING.store(false, Ordering::Release);
    sync_runtime_theme(&cfg, hour);
    bump_all_generations();
}

pub fn update_and_save<F: FnOnce(&mut SystemAppearance)>(
    theme_changed: bool,
    wallpaper_changed: bool,
    layout_changed: bool,
    mutator: F,
) {
    let hour = rtc::read_datetime().hour;
    let updated = {
        let mut st = SERVICE.lock();
        mutator(&mut st.cfg);
        st.cfg.clamp();
        st.last_resolved_hour = hour;
        st.cfg.clone()
    };

    if theme_changed {
        sync_runtime_theme(&updated, hour);
        THEME_GEN.fetch_add(1, Ordering::Release);
    }
    if wallpaper_changed {
        WALLPAPER_GEN.fetch_add(1, Ordering::Release);
    }
    if layout_changed {
        LAYOUT_GEN.fetch_add(1, Ordering::Release);
    }
    // Persist immediately to fast in-memory RAMFS and debounce slow FAT32 /disk writes
    let _ = sync_ram_now(&updated);
    DISK_SYNC_DEADLINE_MS.store(
        timer::uptime_ms().saturating_add(DISK_SYNC_DEBOUNCE_MS),
        Ordering::Release,
    );
    DISK_SYNC_PENDING.store(true, Ordering::Release);
}

/// Checks whether theme state needs periodic sync; Black/White manual modes do not auto-switch by clock.
pub fn sync_auto_theme_if_needed(hour: u8) -> bool {
    let mut st = SERVICE.lock();
    st.last_resolved_hour = hour;
    false
}

pub fn reset_to_defaults() -> Result<(), String> {
    update_and_save(true, true, true, |cfg| {
        *cfg = SystemAppearance::default();
    });
    flush_disk_sync()?;
    match last_error() {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

fn active_profiles_dir() -> &'static str {
    if is_disk_writable() && vfs::with_vfs(|s| s.stat(DISK_PROFILES_DIR).is_ok()) {
        DISK_PROFILES_DIR
    } else {
        RAM_PROFILES_DIR
    }
}

pub fn list_profiles() -> Vec<ProfileEntry> {
    if vfs::with_vfs(|s| s.stat(RAM_PROFILES_DIR).is_err()) {
        let _ = seed_builtin_profiles_at(RAM_PROFILES_DIR);
    }
    if is_disk_writable() && vfs::with_vfs(|s| s.stat(DISK_PROFILES_DIR).is_err()) {
        let _ = seed_builtin_profiles_at(DISK_PROFILES_DIR);
    }

    let mut out = Vec::new();
    let mut seen_slugs: Vec<String> = Vec::new();

    for dir in [DISK_PROFILES_DIR, RAM_PROFILES_DIR] {
        if let Ok(entries) = vfs::with_vfs(|s| s.list_dir(dir)) {
            for e in entries {
                if !e.name.ends_with(".toml") {
                    continue;
                }
                let slug = String::from(&e.name[..e.name.len() - 5]);
                if seen_slugs.iter().any(|x| x == &slug) {
                    continue;
                }
                let full = format!("{}/{}", dir, e.name);
                if let Ok(bytes) = vfs::with_vfs(|s| s.read_file(&full)) {
                    if let Ok(txt) = core::str::from_utf8(&bytes) {
                        let mut tmp = SystemAppearance::default();
                        parse_toml_into(&mut tmp, txt);
                        let is_builtin = slug == "default" || slug == "midnight" || slug == "snow";
                        let protected = slug == DEFAULT_PROFILE_SLUG;
                        let hour = rtc::read_datetime().hour;
                        out.push(ProfileEntry {
                            slug: slug.clone(),
                            name: tmp.active_profile_name.clone(),
                            builtin: is_builtin,
                            protected,
                            accent_color: tmp.accent.color(),
                            is_light: tmp.effective_theme_mode(hour) == ThemeMode::Light,
                        });
                        seen_slugs.push(slug);
                    }
                }
            }
        }
    }

    // Stable user-friendly order: Default, Midnight, Snow, then alphabetical custom
    out.sort_by(|a, b| {
        let rank = |s: &str| match s {
            "default" => 0,
            "midnight" => 1,
            "snow" => 2,
            _ => 3,
        };
        rank(&a.slug)
            .cmp(&rank(&b.slug))
            .then_with(|| a.name.cmp(&b.name))
    });
    out
}

pub fn apply_profile(slug: &str) -> Result<String, String> {
    let clean = sanitize_profile_slug(slug);
    let mut loaded_cfg = match clean.as_str() {
        "default" => Some(SystemAppearance::default()),
        "midnight" => Some(builtin_midnight_profile()),
        "snow" => Some(builtin_snow_profile()),
        _ => None,
    };

    for dir in [DISK_PROFILES_DIR, RAM_PROFILES_DIR] {
        let path = format!("{}/{}.toml", dir, clean);
        if let Ok(bytes) = vfs::with_vfs(|s| s.read_file(&path)) {
            if let Ok(txt) = core::str::from_utf8(&bytes) {
                let mut base = loaded_cfg.unwrap_or_default();
                parse_toml_into(&mut base, txt);
                loaded_cfg = Some(base);
                break;
            }
        }
    }

    let Some(mut target_cfg) = loaded_cfg else {
        return Err(format!("Profile '{}' not found", slug));
    };
    target_cfg.active_profile_slug = clean;
    if target_cfg.active_profile_slug == DEFAULT_PROFILE_SLUG {
        target_cfg.active_profile_name = String::from(DEFAULT_PROFILE_NAME);
    }

    let name = target_cfg.active_profile_name.clone();
    update_and_save(true, true, true, |cfg| {
        *cfg = target_cfg;
    });
    let _ = flush_disk_sync();
    Ok(name)
}

pub fn save_current_as_profile(name: &str) -> Result<String, String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(String::from("Enter a profile name"));
    }
    let slug = sanitize_profile_slug(trimmed);
    if slug.is_empty() {
        return Err(String::from("Profile name must use letters or digits"));
    }
    if slug == DEFAULT_PROFILE_SLUG || slug == "midnight" || slug == "snow" {
        return Err(format!("'{}' is a built-in profile name", trimmed));
    }

    let mut cfg = snapshot();
    cfg.active_profile_slug = slug.clone();
    cfg.active_profile_name = String::from(trimmed);
    let data = serialize_full_profile_toml(&cfg);

    let ram_path = format!("{}/{}.toml", RAM_PROFILES_DIR, slug);
    direct_write_vfs(&ram_path, data.as_bytes()).map_err(|e| format!("Save failed: {}", e.message()))?;
    if is_disk_writable() {
        let disk_path = format!("{}/{}.toml", DISK_PROFILES_DIR, slug);
        let _ = direct_write_vfs(&disk_path, data.as_bytes());
    }

    update_and_save(false, false, false, |c| {
        c.active_profile_slug = slug;
        c.active_profile_name = String::from(trimmed);
    });
    let _ = flush_disk_sync();
    Ok(String::from(trimmed))
}

pub fn rename_profile(slug: &str, new_name: &str) -> Result<String, String> {
    if slug == DEFAULT_PROFILE_SLUG || slug == "midnight" || slug == "snow" {
        return Err(String::from("Built-in profiles cannot be renamed"));
    }
    let trimmed = new_name.trim();
    if trimmed.is_empty() {
        return Err(String::from("New profile name is empty"));
    }
    let new_slug = sanitize_profile_slug(trimmed);
    if new_slug.is_empty() || new_slug == DEFAULT_PROFILE_SLUG || new_slug == "midnight" || new_slug == "snow" {
        return Err(String::from("Invalid or reserved target profile name"));
    }

    let mut found_cfg: Option<SystemAppearance> = None;
    for dir in [DISK_PROFILES_DIR, RAM_PROFILES_DIR] {
        let path = format!("{}/{}.toml", dir, slug);
        if let Ok(bytes) = vfs::with_vfs(|s| s.read_file(&path)) {
            if let Ok(txt) = core::str::from_utf8(&bytes) {
                let mut c = SystemAppearance::default();
                parse_toml_into(&mut c, txt);
                found_cfg = Some(c);
                break;
            }
        }
    }
    let Some(mut cfg) = found_cfg else {
        return Err(String::from("Profile file not found"));
    };

    cfg.active_profile_slug = new_slug.clone();
    cfg.active_profile_name = String::from(trimmed);
    let data = serialize_full_profile_toml(&cfg);

    for dir in [RAM_PROFILES_DIR, DISK_PROFILES_DIR] {
        if dir == DISK_PROFILES_DIR && !is_disk_writable() {
            continue;
        }
        let old_p = format!("{}/{}.toml", dir, slug);
        let new_p = format!("{}/{}.toml", dir, new_slug);
        let _ = direct_write_vfs(&new_p, data.as_bytes());
        if old_p != new_p {
            let _ = vfs::with_vfs(|s| s.remove_recursive(&old_p));
        }
    }

    if snapshot().active_profile_slug == slug {
        update_and_save(false, false, false, |c| {
            c.active_profile_slug = new_slug;
            c.active_profile_name = String::from(trimmed);
        });
        let _ = flush_disk_sync();
    }
    Ok(String::from(trimmed))
}

pub fn delete_profile(slug: &str) -> Result<(), String> {
    if slug == DEFAULT_PROFILE_SLUG {
        return Err(String::from("Amaryllis Default cannot be deleted"));
    }
    if slug == "midnight" || slug == "snow" {
        return Err(String::from("Built-in system profiles cannot be deleted"));
    }
    let _ = active_profiles_dir();

    let mut removed = false;
    for dir in [RAM_PROFILES_DIR, DISK_PROFILES_DIR] {
        let p = format!("{}/{}.toml", dir, slug);
        if vfs::with_vfs(|s| s.remove_recursive(&p)).is_ok() {
            removed = true;
        }
    }
    if !removed {
        return Err(String::from("Profile not found"));
    }
    if snapshot().active_profile_slug == slug {
        let _ = apply_profile(DEFAULT_PROFILE_SLUG);
    }
    Ok(())
}
