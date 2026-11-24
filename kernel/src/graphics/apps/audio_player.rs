use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::cell::Cell;

use crate::audio::mixer::{self, BuiltinTrack};
use crate::drivers::ac97;
use crate::graphics::theme::{self, ACCENT_ERR, ACCENT_MINT, ACCENT_WARN};
use crate::graphics::widgets::{text_width, Canvas, Rect};
use crate::input::{KeyCode, KeyEvent};
use crate::storage::vfs::{self, VfsNodeKind};

use super::DesktopAction;

const ROW_H: i32 = 28;

#[derive(Clone)]
enum TrackSource {
    Builtin(BuiltinTrack),
    Wav(String),
}

#[derive(Clone)]
struct PlaylistEntry {
    title: String,
    subtitle: String,
    source: TrackSource,
}

pub struct AudioPlayerApp {
    playlist: Vec<PlaylistEntry>,
    selected_idx: usize,
    scroll: usize,
    last_max_rows: Cell<usize>,
    last_ui_tick: u64,
    was_active: bool,
    status_msg: String,
}

impl AudioPlayerApp {
    pub fn new() -> Self {
        let mut app = Self {
            playlist: Vec::new(),
            selected_idx: 0,
            scroll: 0,
            last_max_rows: Cell::new(6),
            last_ui_tick: 0,
            was_active: false,
            status_msg: String::from("Select a track and press Play (non-blocking AC'97 mixer)"),
        };
        app.refresh_playlist();
        app
    }

    pub fn refresh_playlist(&mut self) {
        self.playlist.clear();

        let builtins = [
            (BuiltinTrack::StartupChime, "Synth  |  2.4s  |  44.1 kHz Stereo"),
            (BuiltinTrack::AmbientArp, "Synth  |  6.4s  |  44.1 kHz Stereo"),
            (BuiltinTrack::Reference440, "Tone   |  3.0s  |  44.1 kHz Sine"),
        ];
        for (kind, sub) in builtins {
            self.playlist.push(PlaylistEntry {
                title: String::from(kind.title()),
                subtitle: String::from(sub),
                source: TrackSource::Builtin(kind),
            });
        }

        let scan_dirs = ["/", "/home", "/disk", "/disk/audio"];
        let mut seen_paths: Vec<String> = Vec::new();
        for dir in scan_dirs {
            if let Ok(entries) = vfs::with_vfs(|s| s.list_dir(dir)) {
                for e in entries {
                    if e.kind == VfsNodeKind::File && e.name.to_ascii_lowercase().ends_with(".wav") {
                        let full = if dir == "/" {
                            format!("/{}", e.name)
                        } else {
                            format!("{}/{}", dir, e.name)
                        };
                        if !seen_paths.iter().any(|p| p == &full) {
                            seen_paths.push(full.clone());
                            self.playlist.push(PlaylistEntry {
                                title: e.name.clone(),
                                subtitle: format!("WAV    |  {}  |  {} KB", full, (e.size / 1024).max(1)),
                                source: TrackSource::Wav(full),
                            });
                        }
                    }
                }
            }
        }

        if self.selected_idx >= self.playlist.len() {
            self.selected_idx = 0;
        }
    }

    pub fn play_selected(&mut self) {
        let Some(entry) = self.playlist.get(self.selected_idx).cloned() else {
            return;
        };
        match entry.source {
            TrackSource::Builtin(kind) => {
                mixer::play_builtin_async(kind);
                self.status_msg = format!("Playing: {}", kind.title());
                self.was_active = true;
            }
            TrackSource::Wav(path) => match mixer::play_wav_async(&path) {
                Ok(info) => {
                    self.status_msg = format!(
                        "Playing {} ({} Hz, {}ch, {}b)",
                        path, info.sample_rate, info.channels, info.bits_per_sample
                    );
                    self.was_active = true;
                }
                Err(err) => {
                    self.status_msg = format!("Error: {}", err);
                }
            },
        }
    }

    pub fn play_wav_path(&mut self, path: &str) {
        self.refresh_playlist();
        if let Some(pos) = self.playlist.iter().position(|e| match &e.source {
            TrackSource::Wav(p) => p == path,
            TrackSource::Builtin(_) => false,
        }) {
            self.selected_idx = pos;
        }
        match mixer::play_wav_async(path) {
            Ok(info) => {
                self.status_msg = format!(
                    "Playing {} ({} Hz, {}ch)",
                    path, info.sample_rate, info.channels
                );
                self.was_active = true;
            }
            Err(err) => {
                self.status_msg = format!("Error: {}", err);
            }
        }
    }

    /// Drives live UI animations (VU meters, 32-bar waveform, playback timer) while audio plays.
    pub fn tick(&mut self, now_ticks: u64) -> bool {
        let active = mixer::is_active();
        if !active && !self.was_active {
            return false;
        }
        if now_ticks.saturating_sub(self.last_ui_tick) < 8 {
            return false;
        }
        self.last_ui_tick = now_ticks;
        if !active && self.was_active {
            self.was_active = false;
            return true;
        }
        self.was_active = true;
        true
    }

    pub fn on_wheel(&mut self, delta: i8) {
        let max_rows = self.last_max_rows.get().max(1);
        let max_scroll = self.playlist.len().saturating_sub(max_rows);
        if delta > 0 {
            self.scroll = (self.scroll + 1).min(max_scroll);
        } else if delta < 0 {
            self.scroll = self.scroll.saturating_sub(1);
        }
    }

    pub fn on_key(&mut self, evt: KeyEvent) -> DesktopAction {
        match evt.code {
            KeyCode::Up => {
                self.selected_idx = self.selected_idx.saturating_sub(1);
                if self.selected_idx < self.scroll {
                    self.scroll = self.selected_idx;
                }
            }
            KeyCode::Down => {
                if self.selected_idx + 1 < self.playlist.len() {
                    self.selected_idx += 1;
                    let max_rows = self.last_max_rows.get().max(1);
                    if self.selected_idx >= self.scroll + max_rows {
                        self.scroll = self.selected_idx + 1 - max_rows;
                    }
                }
            }
            KeyCode::Enter => self.play_selected(),
            KeyCode::Char(' ') => {
                let snap = mixer::snapshot();
                if snap.playing || snap.paused {
                    mixer::pause_toggle();
                } else {
                    self.play_selected();
                }
            }
            KeyCode::Char('+') | KeyCode::Char('=') => {
                let v = mixer::snapshot().volume_percent.saturating_add(5).min(100);
                mixer::set_volume(v);
            }
            KeyCode::Char('-') => {
                let v = mixer::snapshot().volume_percent.saturating_sub(5);
                mixer::set_volume(v);
            }
            _ => {}
        }
        DesktopAction::None
    }

    pub fn on_click(&mut self, area: Rect, mx: i32, my: i32) -> DesktopAction {
        let ctrl_y = area.y + 150;
        let btn_play = Rect::new(area.x + 14, ctrl_y, 98, 28);
        let btn_stop = Rect::new(area.x + 118, ctrl_y, 64, 28);
        let btn_vm = Rect::new(area.x + 190, ctrl_y, 50, 28);
        let btn_vp = Rect::new(area.x + 246, ctrl_y, 50, 28);
        let btn_scan = Rect::new(area.right() - 102, ctrl_y, 88, 28);

        let snap = mixer::snapshot();
        if btn_play.contains(mx, my) {
            if snap.playing || snap.paused {
                mixer::pause_toggle();
                self.was_active = mixer::is_active();
            } else {
                self.play_selected();
            }
            return DesktopAction::None;
        }
        if btn_stop.contains(mx, my) {
            mixer::stop();
            self.was_active = false;
            self.status_msg = String::from("Playback stopped");
            return DesktopAction::None;
        }
        if btn_vm.contains(mx, my) {
            mixer::set_volume(snap.volume_percent.saturating_sub(10));
            return DesktopAction::None;
        }
        if btn_vp.contains(mx, my) {
            mixer::set_volume(snap.volume_percent.saturating_add(10).min(100));
            return DesktopAction::None;
        }
        if btn_scan.contains(mx, my) {
            self.refresh_playlist();
            self.status_msg = format!("Playlist refreshed ({} tracks)", self.playlist.len());
            return DesktopAction::None;
        }

        let list_r = Rect::new(area.x + 14, area.y + 192, area.w - 28, (area.h - 224).max(60));
        if list_r.contains(mx, my) {
            let row = ((my - (list_r.y + 4)) / ROW_H).max(0) as usize;
            let idx = self.scroll + row;
            if idx < self.playlist.len() {
                if self.selected_idx == idx {
                    self.play_selected();
                } else {
                    self.selected_idx = idx;
                }
            }
        }
        DesktopAction::None
    }

    pub fn draw(&self, canvas: &mut Canvas<'_>, area: Rect) {
        let pal = theme::palette();
        canvas.fill_rect(area, pal.win_bg_active);

        let snap = mixer::snapshot();
        let hw_ok = ac97::is_ready();

        // Hero Now-Playing card
        let hero = Rect::new(area.x + 14, area.y + 10, area.w - 28, 130);
        canvas.fill_rounded_rect(hero, 10, pal.surface_card);
        canvas.stroke_rounded_rect(hero, 10, pal.surface_border, 190);

        // Album-art style gradient tile
        let art = Rect::new(hero.x + 12, hero.y + 12, 68, 68);
        canvas.fill_rounded_rect(art, 10, pal.accent_primary);
        canvas.fill_gradient_v(Rect::new(art.x + 2, art.y + 2, 64, 64), 0xFF34343E, 0xFF101014);
        canvas.draw_circle(art.x + 25, art.y + 46, 6, 0xFFFFFFFF);
        canvas.draw_circle(art.x + 44, art.y + 42, 6, 0xFFFFFFFF);
        canvas.fill_rect(Rect::new(art.x + 29, art.y + 21, 3, 25), 0xFFFFFFFF);
        canvas.fill_rect(Rect::new(art.x + 48, art.y + 17, 3, 25), 0xFFFFFFFF);
        canvas.fill_rect(Rect::new(art.x + 29, art.y + 17, 22, 5), 0xFFFFFFFF);

        // Track title (scaled 1.25x bold) + metadata
        let title_r = Rect::new(hero.x + 92, hero.y + 12, (hero.w - 240).max(80), 26);
        canvas.draw_text_scaled_clipped(
            title_r.x,
            title_r.y,
            &snap.title,
            pal.text_primary,
            320,
            true,
            title_r,
        );

        let codec_line = format!(
            "{}  |  {} Hz  {}ch {}b  |  Vol {}%",
            if hw_ok { "AC'97 Ready" } else { "No AC'97" },
            snap.sample_rate,
            snap.channels,
            snap.bits_per_sample,
            snap.volume_percent
        );
        canvas.draw_text_clipped(
            hero.x + 92,
            hero.y + 39,
            &codec_line,
            if hw_ok { pal.text_secondary } else { ACCENT_WARN },
            1,
            hero,
        );

        // 32-bar live waveform visualizer on right side of hero card
        let spec_r = Rect::new(hero.right() - 144, hero.y + 12, 130, 52);
        canvas.fill_rounded_rect(spec_r, 6, pal.surface_panel);
        canvas.stroke_rounded_rect(spec_r, 6, pal.surface_border, 150);
        let mid_y = spec_r.y + spec_r.h / 2;
        for (bi, &sample) in snap.waveform.iter().enumerate() {
            let bx = spec_r.x + 4 + (bi as i32) * 4;
            let amp = ((sample.abs() as i32) * 20 / 100).max(if snap.playing { 2 } else { 1 });
            let bar_col = if snap.playing {
                if amp > 14 { ACCENT_MINT } else { pal.accent_primary }
            } else {
                pal.text_muted
            };
            canvas.fill_rect(Rect::new(bx, mid_y - amp, 2, amp * 2), bar_col);
        }

        // Stereo VU meters + Progress bar
        let vu_w = ((hero.w - 24) / 2 - 40).max(40);
        let vu_l = Rect::new(hero.x + 12, hero.y + 90, vu_w, 8);
        let vu_r = Rect::new(hero.x + 28 + vu_w + 20, hero.y + 90, vu_w, 8);
        canvas.draw_text(hero.x + 12, hero.y + 87, "L", pal.text_muted, 1);
        draw_meter_bar(canvas, Rect::new(vu_l.x + 14, vu_l.y, vu_l.w, 8), snap.vu_left, pal.surface_border, pal.accent_green);
        canvas.draw_text(vu_r.x - 14, hero.y + 87, "R", pal.text_muted, 1);
        draw_meter_bar(canvas, Rect::new(vu_r.x, vu_r.y, vu_r.w, 8), snap.vu_right, pal.surface_border, pal.accent_primary);

        // Progress bar + mm:ss
        let time_str = format!(
            "{} / {}",
            fmt_ms(snap.position_ms),
            fmt_ms(snap.duration_ms)
        );
        let tw = text_width(&time_str, 1);
        canvas.draw_text(hero.right() - tw - 12, hero.y + 104, &time_str, pal.text_secondary, 1);

        let prog_r = Rect::new(hero.x + 12, hero.y + 109, (hero.w - tw - 36).max(60), 8);
        let prog_pct = if snap.duration_ms > 0 {
            ((snap.position_ms as u64 * 100) / snap.duration_ms as u64).min(100) as u8
        } else {
            0
        };
        draw_meter_bar(canvas, prog_r, prog_pct, pal.surface_border, pal.accent_primary);

        // Transport Controls Row
        let ctrl_y = area.y + 150;
        let play_lbl = if snap.playing {
            "Pause"
        } else if snap.paused {
            "Resume"
        } else {
            "Play"
        };
        let btns = [
            (Rect::new(area.x + 14, ctrl_y, 98, 28), play_lbl, pal.accent_primary, 0xFFFFFFFF),
            (Rect::new(area.x + 118, ctrl_y, 64, 28), "Stop", pal.surface_card, ACCENT_ERR),
            (Rect::new(area.x + 190, ctrl_y, 50, 28), "Vol-", pal.surface_card, pal.text_primary),
            (Rect::new(area.x + 246, ctrl_y, 50, 28), "Vol+", pal.surface_card, pal.text_primary),
            (Rect::new(area.right() - 102, ctrl_y, 88, 28), "Scan .wav", pal.surface_card, pal.accent_primary),
        ];
        for (r, txt, bg, fg) in btns {
            canvas.fill_rounded_rect(r, 7, bg);
            canvas.stroke_rounded_rect(r, 7, pal.surface_border, 190);
            let tx = r.x + ((r.w - text_width(txt, 1)) / 2).max(6);
            canvas.draw_text_bold_clipped(tx, r.y + 6, txt, fg, r);
        }

        // Playlist list box
        let list_r = Rect::new(area.x + 14, area.y + 192, area.w - 28, (area.h - 224).max(60));
        canvas.fill_rounded_rect(list_r, 8, pal.surface_panel);
        canvas.stroke_rounded_rect(list_r, 8, pal.surface_border, 180);

        let max_rows = ((list_r.h - 8) / ROW_H).max(1) as usize;
        self.last_max_rows.set(max_rows);

        for (i, entry) in self.playlist.iter().skip(self.scroll).take(max_rows).enumerate() {
            let abs_idx = self.scroll + i;
            let row_r = Rect::new(
                list_r.x + 4,
                list_r.y + 4 + (i as i32) * ROW_H,
                list_r.w - 18,
                ROW_H - 2,
            );
            if abs_idx == self.selected_idx {
                canvas.fill_rounded_rect(row_r, 6, pal.surface_selected);
            }
            let badge = match entry.source {
                TrackSource::Builtin(_) => "SYNTH",
                TrackSource::Wav(_) => "WAV",
            };
            canvas.draw_text_bold_clipped(
                row_r.x + 8,
                row_r.y + 5,
                badge,
                pal.accent_primary,
                row_r,
            );
            canvas.draw_text_clipped(
                row_r.x + 62,
                row_r.y + 5,
                &entry.title,
                pal.text_primary,
                1,
                Rect::new(row_r.x + 62, row_r.y, (row_r.w / 2 - 40).max(40), row_r.h),
            );
            let sub_x = row_r.x + (row_r.w / 2).max(180);
            canvas.draw_text_clipped(
                sub_x,
                row_r.y + 5,
                &entry.subtitle,
                pal.text_secondary,
                1,
                row_r,
            );
        }

        canvas.draw_scrollbar_v(
            Rect::new(list_r.right() - 10, list_r.y + 4, 7, list_r.h - 8),
            self.playlist.len(),
            max_rows,
            self.scroll,
            pal.text_muted,
            pal.surface_border,
        );

        let footer = snap.last_error.as_deref().unwrap_or(&self.status_msg);
        canvas.draw_text_clipped(
            area.x + 14,
            area.bottom() - 22,
            footer,
            if snap.last_error.is_some() { ACCENT_ERR } else { pal.text_muted },
            1,
            area,
        );
    }
}

fn draw_meter_bar(canvas: &mut Canvas<'_>, r: Rect, pct: u8, track_col: u32, fill_col: u32) {
    canvas.fill_rounded_rect_alpha(r, r.h / 2, track_col, 110);
    let fill_w = ((r.w as usize * (pct.min(100) as usize)) / 100) as i32;
    if fill_w > 2 {
        canvas.fill_rounded_rect(Rect::new(r.x, r.y, fill_w, r.h), r.h / 2, fill_col);
    }
}

fn fmt_ms(ms: u32) -> String {
    let secs = ms / 1000;
    let m = secs / 60;
    let s = secs % 60;
    format!("{:02}:{:02}", m, s)
}
