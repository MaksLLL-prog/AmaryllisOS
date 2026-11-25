pub mod alerts;
pub mod apps;
pub mod clipboard;
pub mod compositor;
pub mod font;
pub mod font_cyrillic;
pub mod font_mono;
pub mod icons;
pub mod personalization;
mod renderer;
pub mod theme;
pub mod widgets;
pub mod wm;

pub use renderer::{Color, Renderer};