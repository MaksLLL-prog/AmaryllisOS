use core::sync::atomic::{AtomicU8, Ordering};
use spin::Mutex;

// Amaryllis Desktop Unified Color & Layout Tokens (0xFFRRGGBB)

pub const TOP_BAR_HEIGHT: usize = 28;
pub const DOCK_HEIGHT: usize = 60;
pub const DOCK_BOTTOM_MARGIN: usize = 12;
pub const DOCK_RADIUS: usize = 18;
pub const DOCK_ICON_SIZE: usize = 42;
pub const DOCK_ICON_GAP: usize = 10;

pub const WINDOW_TITLEBAR_HEIGHT: usize = 32;
pub const WINDOW_CORNER_RADIUS: usize = 11;
pub const WINDOW_MIN_WIDTH: usize = 380;
pub const WINDOW_MIN_HEIGHT: usize = 250;

// Traffic-light window controls
pub const TRAFFIC_CLOSE: u32 = 0xFFFF5F57;
pub const TRAFFIC_MINIMIZE: u32 = 0xFFFEBC2E;
pub const TRAFFIC_MAXIMIZE: u32 = 0xFF28C840;
pub const TRAFFIC_INACTIVE_LIGHT: u32 = 0xFFD0D6E0;
pub const TRAFFIC_INACTIVE_DARK: u32 = 0xFF464F61;

// Balanced semantic accents (shared across themes)
pub const ACCENT_MINT: u32 = 0xFF24A87E;
pub const ACCENT_CYAN: u32 = 0xFF2884E6;
pub const ACCENT_MONO: u32 = 0xFFEAEAF0;
pub const ACCENT_WARN: u32 = 0xFFD99018;
pub const ACCENT_ERR: u32 = 0xFFDE4B5A;
pub const ACCENT_ERR_BG: u32 = 0xFF341822;

// Snap preview overlay
pub const SNAP_OVERLAY_FILL: u32 = 0xFF3890F0;
pub const SNAP_OVERLAY_BORDER: u32 = 0xFF60AAFA;

// Kept as convenient defaults for terminal dark interior
pub const TEXT_PRIMARY: u32 = 0xFFEAEFF7;
pub const TEXT_SECONDARY: u32 = 0xFF9AA7BC;
pub const TEXT_MUTED: u32 = 0xFF64728A;
pub const GLASS_BORDER_SUBTLE: u32 = 0xFF2B364A;
pub const GLASS_BORDER_LIGHT: u32 = 0xFF6B81A3;
pub const GLASS_MODAL: u32 = 0xFF151E2E;
pub const WIN_BG_ACTIVE: u32 = 0xFF121825;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ThemeMode {
    Light = 0,
    Dark = 1,
}

static CURRENT_THEME: AtomicU8 = AtomicU8::new(ThemeMode::Dark as u8);

#[inline]
pub fn current_theme_mode() -> ThemeMode {
    if CURRENT_THEME.load(Ordering::Relaxed) == ThemeMode::Dark as u8 {
        ThemeMode::Dark
    } else {
        ThemeMode::Light
    }
}

#[inline]
pub fn set_theme_mode(mode: ThemeMode) {
    CURRENT_THEME.store(mode as u8, Ordering::Relaxed);
}

#[inline]
pub fn toggle_theme_mode() -> ThemeMode {
    let next = match current_theme_mode() {
        ThemeMode::Light => ThemeMode::Dark,
        ThemeMode::Dark => ThemeMode::Light,
    };
    set_theme_mode(next);
    next
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum WallpaperPreset {
    ObsidianWaves = 0,
    SilkDunes = 1,
    AuroraBorealis = 2,
    SunsetHorizon = 3,
    MidnightMinimal = 4,
}

impl WallpaperPreset {
    pub const ALL: [Self; 5] = [
        Self::ObsidianWaves,
        Self::SilkDunes,
        Self::AuroraBorealis,
        Self::SunsetHorizon,
        Self::MidnightMinimal,
    ];

    pub const fn title(self) -> &'static str {
        match self {
            Self::ObsidianWaves => "Monochrome Silk",
            Self::SilkDunes => "Silk Dunes",
            Self::AuroraBorealis => "Aurora Borealis",
            Self::SunsetHorizon => "Sunset Horizon",
            Self::MidnightMinimal => "Midnight Minimal",
        }
    }

    pub const fn short_title(self) -> &'static str {
        match self {
            Self::ObsidianWaves => "Monochrome",
            Self::SilkDunes => "Silk Dunes",
            Self::AuroraBorealis => "Aurora",
            Self::SunsetHorizon => "Sunset",
            Self::MidnightMinimal => "Carbon",
        }
    }

    pub const fn slug(self) -> &'static str {
        match self {
            Self::ObsidianWaves => "monochrome",
            Self::SilkDunes => "silk_dunes",
            Self::AuroraBorealis => "aurora",
            Self::SunsetHorizon => "sunset",
            Self::MidnightMinimal => "midnight",
        }
    }

    pub fn from_slug(s: &str) -> Option<Self> {
        match s.trim() {
            "monochrome" | "obsidian" | "blossom" | "amaryllis_blossom" | "0" => {
                Some(Self::ObsidianWaves)
            }
            "silk_dunes" | "silkdunes" | "1" => Some(Self::SilkDunes),
            "aurora" | "aurora_borealis" | "2" => Some(Self::AuroraBorealis),
            "sunset" | "sunset_horizon" | "3" => Some(Self::SunsetHorizon),
            "midnight" | "midnight_minimal" | "4" => Some(Self::MidnightMinimal),
            _ => None,
        }
    }

    pub const fn next(self) -> Self {
        match self {
            Self::ObsidianWaves => Self::SilkDunes,
            Self::SilkDunes => Self::AuroraBorealis,
            Self::AuroraBorealis => Self::SunsetHorizon,
            Self::SunsetHorizon => Self::MidnightMinimal,
            Self::MidnightMinimal => Self::ObsidianWaves,
        }
    }

    pub const fn preview_colors(self) -> (u32, u32, u32) {
        match self {
            Self::ObsidianWaves => (0xFF0A0A0D, 0xFF1A1A20, 0xFFE4E4EA),
            Self::SilkDunes => (0xFF334D70, 0xFF9C7662, 0xFFF3C6A2),
            Self::AuroraBorealis => (0xFF061622, 0xFF0F4C50, 0xFF34E8B6),
            Self::SunsetHorizon => (0xFF1C1418, 0xFF8C3648, 0xFFF49D60),
            Self::MidnightMinimal => (0xFF0B0C10, 0xFF171920, 0xFF4A505E),
        }
    }
}

static CURRENT_WALLPAPER: AtomicU8 = AtomicU8::new(WallpaperPreset::ObsidianWaves as u8);

#[inline]
pub fn current_wallpaper() -> WallpaperPreset {
    match CURRENT_WALLPAPER.load(Ordering::Relaxed) {
        1 => WallpaperPreset::SilkDunes,
        2 => WallpaperPreset::AuroraBorealis,
        3 => WallpaperPreset::SunsetHorizon,
        4 => WallpaperPreset::MidnightMinimal,
        _ => WallpaperPreset::ObsidianWaves,
    }
}

#[inline]
pub fn set_wallpaper(preset: WallpaperPreset) {
    CURRENT_WALLPAPER.store(preset as u8, Ordering::Relaxed);
}

#[inline]
pub fn next_wallpaper() -> WallpaperPreset {
    let nxt = current_wallpaper().next();
    set_wallpaper(nxt);
    nxt
}

#[derive(Clone, Copy)]
pub struct ThemePalette {
    pub is_light: bool,
    pub topbar_bg: u32,
    pub topbar_alpha: u8,
    pub topbar_border: u32,
    pub topbar_text: u32,
    pub topbar_muted: u32,

    pub dock_bg: u32,
    pub dock_alpha: u8,
    pub dock_border: u32,
    pub dock_divider: u32,
    pub dock_dot: u32,

    pub modal_bg: u32,
    pub modal_alpha: u8,
    pub modal_border: u32,
    pub modal_input_bg: u32,
    pub modal_hover: u32,

    pub win_bg_active: u32,
    pub win_bg_inactive: u32,
    pub win_title_active: u32,
    pub win_title_inactive: u32,
    pub win_border_active: u32,
    pub win_border_inactive: u32,
    pub win_divider: u32,

    pub surface_panel: u32,
    pub surface_sidebar: u32,
    pub surface_card: u32,
    pub surface_toolbar: u32,
    pub surface_border: u32,
    pub surface_selected: u32,
    pub surface_hover: u32,

    pub text_primary: u32,
    pub text_secondary: u32,
    pub text_muted: u32,
    pub traffic_inactive: u32,
    pub accent_primary: u32,
    pub on_accent: u32,
    pub selection_color: u32,
    pub accent_green: u32,
    pub shadow_color: u32,
}

#[inline]
pub const fn contrast_on(bg: u32) -> u32 {
    let r = (bg >> 16) & 0xFF;
    let g = (bg >> 8) & 0xFF;
    let b = bg & 0xFF;
    let luma = (r * 77 + g * 150 + b * 29) >> 8;
    if luma > 152 {
        0xFF0B0B0E
    } else {
        0xFFFFFFFF
    }
}

pub const LIGHT_PALETTE: ThemePalette = ThemePalette {
    is_light: true,
    topbar_bg: 0xFFF5F6F8,
    topbar_alpha: 214,
    topbar_border: 0xFFD4D7DF,
    topbar_text: 0xFF121216,
    topbar_muted: 0xFF5A5D66,

    dock_bg: 0xFFF2F3F6,
    dock_alpha: 204,
    dock_border: 0xFFFFFFFF,
    dock_divider: 0xFFB8BDC7,
    dock_dot: 0xFF141418,

    modal_bg: 0xFFF7F8FA,
    modal_alpha: 240,
    modal_border: 0xFFC2C7D0,
    modal_input_bg: 0xFFFFFFFF,
    modal_hover: 0xFFE6E8ED,

    win_bg_active: 0xFFFFFFFF,
    win_bg_inactive: 0xFFF4F5F8,
    win_title_active: 0xFFF1F2F5,
    win_title_inactive: 0xFFE8EAEE,
    win_border_active: 0xFFC4C8D2,
    win_border_inactive: 0xFFD8DCE4,
    win_divider: 0xFFE0E3EA,

    surface_panel: 0xFFFFFFFF,
    surface_sidebar: 0xFFF1F2F5,
    surface_card: 0xFFF5F6F9,
    surface_toolbar: 0xFFF6F7FA,
    surface_border: 0xFFD8DCE4,
    surface_selected: 0xFFE2E5EC,
    surface_hover: 0xFFECEEF3,

    text_primary: 0xFF111216,
    text_secondary: 0xFF4E525C,
    text_muted: 0xFF7D828E,
    traffic_inactive: TRAFFIC_INACTIVE_LIGHT,
    accent_primary: 0xFF18181D,
    on_accent: 0xFFFFFFFF,
    selection_color: 0xFFDCE0E8,
    accent_green: 0xFF1D9A72,
    shadow_color: 0xFF0A0A0E,
};

pub const DARK_PALETTE: ThemePalette = ThemePalette {
    is_light: false,
    topbar_bg: 0xFF0A0A0D,
    topbar_alpha: 214,
    topbar_border: 0xFF23232A,
    topbar_text: 0xFFF4F4F7,
    topbar_muted: 0xFF9898A4,

    dock_bg: 0xFF0E0E12,
    dock_alpha: 206,
    dock_border: 0xFF2D2D36,
    dock_divider: 0xFF2D2D36,
    dock_dot: 0xFFFFFFFF,

    modal_bg: 0xFF111115,
    modal_alpha: 240,
    modal_border: 0xFF2E2E38,
    modal_input_bg: 0xFF09090C,
    modal_hover: 0xFF22222A,

    win_bg_active: 0xFF0C0C10,
    win_bg_inactive: 0xFF101014,
    win_title_active: 0xFF141419,
    win_title_inactive: 0xFF101014,
    win_border_active: 0xFF34343F,
    win_border_inactive: 0xFF22222A,
    win_divider: 0xFF212128,

    surface_panel: 0xFF0B0B0F,
    surface_sidebar: 0xFF0E0E13,
    surface_card: 0xFF14141A,
    surface_toolbar: 0xFF121218,
    surface_border: 0xFF26262F,
    surface_selected: 0xFF282832,
    surface_hover: 0xFF1D1D25,

    text_primary: 0xFFF4F4F7,
    text_secondary: 0xFFAEAEB8,
    text_muted: 0xFF6E6E7A,
    traffic_inactive: TRAFFIC_INACTIVE_DARK,
    accent_primary: 0xFFEAEAF0,
    on_accent: 0xFF0A0A0D,
    selection_color: 0xFF2A2A34,
    accent_green: 0xFF3CC296,
    shadow_color: 0xFF020204,
};

#[inline]
const fn mix_rgb(a: u32, b: u32, t: u32) -> u32 {
    let inv = 256 - t;
    let r = ((((a >> 16) & 0xFF) * inv + ((b >> 16) & 0xFF) * t) >> 8) & 0xFF;
    let g = ((((a >> 8) & 0xFF) * inv + ((b >> 8) & 0xFF) * t) >> 8) & 0xFF;
    let bl = (((a & 0xFF) * inv + (b & 0xFF) * t) >> 8) & 0xFF;
    0xFF00_0000 | (r << 16) | (g << 8) | bl
}

/// Derives a system-wide consistent ThemePalette for Black (Dark) or White (Light) mode,
/// Surface style (0=Default, 1=Midnight OLED, 2=Snow Crisp), custom Accent color, and Selection tint.
pub const fn derive_palette(
    mode: ThemeMode,
    surface_style: u8,
    accent_rgb: u32,
    selection_rgb: Option<u32>,
) -> ThemePalette {
    let accent = 0xFF00_0000 | (accent_rgb & 0x00FF_FFFF);
    let sel_base = match selection_rgb {
        Some(c) => 0xFF00_0000 | (c & 0x00FF_FFFF),
        None => accent,
    };

    let mut pal = match mode {
        ThemeMode::Light => LIGHT_PALETTE,
        ThemeMode::Dark => DARK_PALETTE,
    };

    match (mode, surface_style) {
        (ThemeMode::Dark, 1) => {
            // Pitch OLED Black profile
            pal.topbar_bg = 0xFF050507;
            pal.topbar_alpha = 224;
            pal.topbar_border = 0xFF1A1A20;
            pal.topbar_muted = 0xFF8C8C98;
            pal.dock_bg = 0xFF070709;
            pal.dock_alpha = 216;
            pal.dock_border = 0xFF25252D;
            pal.dock_divider = 0xFF23232B;
            pal.modal_bg = 0xFF09090C;
            pal.modal_alpha = 244;
            pal.modal_border = 0xFF25252E;
            pal.modal_input_bg = 0xFF040406;
            pal.win_bg_active = 0xFF060608;
            pal.win_bg_inactive = 0xFF09090C;
            pal.win_title_active = 0xFF0C0C10;
            pal.win_title_inactive = 0xFF09090C;
            pal.win_border_active = 0xFF282832;
            pal.win_border_inactive = 0xFF18181F;
            pal.win_divider = 0xFF17171E;
            pal.surface_panel = 0xFF050507;
            pal.surface_sidebar = 0xFF08080B;
            pal.surface_card = 0xFF0E0E13;
            pal.surface_toolbar = 0xFF0B0B0F;
            pal.surface_border = 0xFF1F1F27;
            pal.text_secondary = 0xFFA2A2AE;
            pal.text_muted = 0xFF646470;
        }
        (ThemeMode::Light, 2) => {
            // Pure White Snow profile
            pal.topbar_bg = 0xFFFFFFFF;
            pal.topbar_alpha = 230;
            pal.topbar_border = 0xFFDCDFE6;
            pal.dock_bg = 0xFFFAFAFC;
            pal.dock_alpha = 216;
            pal.win_bg_active = 0xFFFFFFFF;
            pal.win_bg_inactive = 0xFFF8F9FB;
            pal.win_title_active = 0xFFF6F7F9;
            pal.win_title_inactive = 0xFFEDEFF3;
            pal.surface_sidebar = 0xFFF5F6F9;
            pal.surface_card = 0xFFFAFAFC;
            pal.surface_toolbar = 0xFFFAFAFC;
            pal.surface_border = 0xFFDCDFE6;
        }
        _ => {}
    }

    pal.accent_primary = accent;
    pal.on_accent = contrast_on(accent);
    if matches!(mode, ThemeMode::Light) {
        pal.surface_selected = mix_rgb(pal.surface_card, sel_base, 48);
        pal.selection_color = mix_rgb(0xFFFFFFFF, sel_base, 64);
        pal.modal_hover = mix_rgb(pal.modal_bg, accent, 34);
        pal.surface_hover = mix_rgb(pal.surface_card, accent, 24);
        pal.dock_dot = mix_rgb(0xFF141418, accent, 145);
        pal.win_border_active = mix_rgb(pal.win_border_active, accent, 52);
        pal.dock_border = mix_rgb(0xFFFFFFFF, accent, 22);
    } else {
        pal.surface_selected = mix_rgb(pal.surface_card, sel_base, 64);
        pal.selection_color = mix_rgb(pal.surface_panel, sel_base, 86);
        pal.modal_hover = mix_rgb(pal.modal_bg, accent, 42);
        pal.surface_hover = mix_rgb(pal.surface_card, accent, 32);
        pal.dock_dot = mix_rgb(0xFFFFFFFF, accent, 180);
        pal.win_border_active = mix_rgb(pal.win_border_active, accent, 64);
        pal.dock_border = mix_rgb(pal.surface_border, accent, 54);
    }

    pal
}

static RUNTIME_PALETTE: Mutex<ThemePalette> = Mutex::new(DARK_PALETTE);

#[inline]
pub fn apply_runtime_palette(mode: ThemeMode, pal: ThemePalette) {
    set_theme_mode(mode);
    *RUNTIME_PALETTE.lock() = pal;
}

#[inline]
pub fn palette() -> ThemePalette {
    *RUNTIME_PALETTE.lock()
}

#[inline]
pub const fn rgb_tuple_to_u32(c: (u8, u8, u8)) -> u32 {
    0xFF00_0000 | ((c.0 as u32) << 16) | ((c.1 as u32) << 8) | (c.2 as u32)
}
