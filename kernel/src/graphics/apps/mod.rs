pub mod audio_player;
pub mod editor;
pub mod files;
pub mod monitor;
pub mod net_viewer;
pub mod settings;
pub mod terminal;

use alloc::string::String;

use crate::graphics::theme::{ThemeMode, WallpaperPreset};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum BuiltinAppKind {
    Terminal,
    Files,
    Editor,
    AudioPlayer,
    NetViewer,
    Settings,
    Monitor,
}

impl BuiltinAppKind {
    pub const fn title(self) -> &'static str {
        match self {
            Self::Terminal => "Amaryllis Terminal",
            Self::Files => "Files",
            Self::Editor => "Text Editor",
            Self::AudioPlayer => "Audio Player",
            Self::NetViewer => "Net / HTTP Viewer",
            Self::Settings => "System Settings",
            Self::Monitor => "Activity Monitor",
        }
    }

    pub const fn short_label(self) -> &'static str {
        match self {
            Self::Terminal => "Terminal",
            Self::Files => "Files",
            Self::Editor => "Text Editor",
            Self::AudioPlayer => "Audio Player",
            Self::NetViewer => "Net Viewer",
            Self::Settings => "Settings",
            Self::Monitor => "Activity Monitor",
        }
    }
}

#[derive(Clone)]
pub enum DesktopAction {
    None,
    RunShellCommand(String),
    ExecElf(String),
    KillProcess(usize),
    PlayWav(String),
    Beep(u32, u32),
    OpenApp(BuiltinAppKind),
    OpenEditor(String),
    ToggleTheme,
    SetTheme(ThemeMode),
    SetWallpaper(WallpaperPreset),
    SetCustomWallpaper(String),
    NextWallpaper,
    OpenLatestAlert,
    CloseDesktop,
}
