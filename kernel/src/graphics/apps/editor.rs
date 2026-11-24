use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::cell::Cell;

use crate::graphics::alerts::{self, AlertSeverity, AlertSubsystem};
use crate::graphics::clipboard;
use crate::graphics::personalization::{self, MonoFontFamily};
use crate::graphics::theme::{self, ACCENT_ERR, ACCENT_MINT, ACCENT_WARN};
use crate::graphics::widgets::{mono_metrics, Canvas, Rect};
use crate::input::{KeyCode, KeyEvent};
use crate::storage::vfs;

use super::DesktopAction;

const TOOLBAR_H: i32 = 38;
const STATUS_H: i32 = 24;
const GUTTER_W: i32 = 44;

pub struct EditorApp {
    file_path: String,
    editing_path: bool,
    path_input: String,
    lines: Vec<String>,
    cur_line: usize,
    cur_col: usize,
    scroll_y: usize,
    modified: bool,
    select_all: bool,
    status_msg: String,
    last_visible_rows: Cell<usize>,
}

impl EditorApp {
    pub fn new() -> Self {
        let mut app = Self {
            file_path: String::from("/notes.txt"),
            editing_path: false,
            path_input: String::from("/notes.txt"),
            lines: vec![String::new()],
            cur_line: 0,
            cur_col: 0,
            scroll_y: 0,
            modified: false,
            select_all: false,
            status_msg: String::from("Ready  |  Ctrl+S Save  Ctrl+C Copy  Ctrl+V Paste"),
            last_visible_rows: Cell::new(16),
        };
        app.open_or_welcome("/notes.txt");
        app
    }

    fn open_or_welcome(&mut self, path: &str) {
        self.file_path = String::from(path);
        self.path_input = self.file_path.clone();
        self.editing_path = false;
        self.select_all = false;
        self.cur_line = 0;
        self.cur_col = 0;
        self.scroll_y = 0;

        match vfs::with_vfs(|state| state.read_file(path)) {
            Ok(bytes) => {
                self.load_from_bytes(&bytes);
                self.modified = false;
                self.status_msg = format!("Loaded {} ({} bytes)", path, bytes.len());
            }
            Err(_) => {
                self.lines = vec![
                    String::from("AmaryllisOS Notes"),
                    String::from("================="),
                    String::new(),
                    String::from("Edit this text and press Ctrl+S (or Save) to write to FAT32."),
                ];
                self.cur_line = 3;
                self.cur_col = self.lines[3].len();
                self.modified = false;
                self.status_msg = format!("New buffer: {} (press Ctrl+S to create on disk)", path);
            }
        }
    }

    pub fn open_file(&mut self, path: &str) {
        self.file_path = String::from(path);
        self.path_input = self.file_path.clone();
        self.editing_path = false;
        self.select_all = false;
        self.cur_line = 0;
        self.cur_col = 0;
        self.scroll_y = 0;

        match vfs::with_vfs(|state| state.read_file(path)) {
            Ok(bytes) => {
                self.load_from_bytes(&bytes);
                self.modified = false;
                self.status_msg = format!("Opened {} ({} bytes)", path, bytes.len());
            }
            Err(vfs::VfsError::NotFound) => {
                self.lines = vec![String::new()];
                self.modified = true;
                self.status_msg = format!("New file '{}' — Ctrl+S to save", path);
            }
            Err(err) => {
                self.lines = vec![String::new()];
                self.modified = false;
                self.status_msg = format!("Read error: {}", err.message());
            }
        }
    }

    pub fn new_untitled(&mut self) {
        self.file_path = String::from("/untitled.txt");
        self.path_input = self.file_path.clone();
        self.editing_path = false;
        self.lines = vec![String::new()];
        self.cur_line = 0;
        self.cur_col = 0;
        self.scroll_y = 0;
        self.modified = true;
        self.select_all = false;
        self.status_msg = String::from("New file /untitled.txt — click path bar to rename");
    }

    fn load_from_bytes(&mut self, bytes: &[u8]) {
        self.lines.clear();
        let text = core::str::from_utf8(bytes).unwrap_or("<binary or non-UTF8 file>");
        for raw_line in text.split('\n') {
            let clean = raw_line.strip_suffix('\r').unwrap_or(raw_line);
            let mut s = String::new();
            for ch in clean.chars() {
                if ch == '\t' {
                    s.push_str("    ");
                } else if ch.is_ascii() && !ch.is_ascii_control() {
                    s.push(ch);
                }
            }
            self.lines.push(s);
        }
        if self.lines.is_empty() {
            self.lines.push(String::new());
        }
    }

    pub fn save(&mut self) -> bool {
        let target = if self.file_path.trim().is_empty() {
            String::from("/notes.txt")
        } else if !self.file_path.starts_with('/') {
            format!("/{}", self.file_path.trim())
        } else {
            self.file_path.clone()
        };
        self.file_path = target.clone();
        self.path_input = target.clone();

        let content = self.lines.join("\n");
        match vfs::with_vfs(|state| state.write_file(&target, content.as_bytes())) {
            Ok(()) => {
                self.modified = false;
                self.status_msg = format!("Saved {} ({} bytes)", target, content.len());
                true
            }
            Err(err) => {
                self.status_msg = format!("Save failed: {}", err.message());
                alerts::report_system_error(
                    AlertSeverity::Warning,
                    AlertSubsystem::StorageVfs,
                    "EDITOR SAVE",
                    &format!("Could not save '{}'", target),
                    err.message(),
                    "Ensure a writable FAT32 disk is mounted (Primary Slave or 'mount').",
                    false,
                );
                false
            }
        }
    }

    pub fn copy_to_clipboard(&mut self) {
        if self.select_all {
            let full = self.lines.join("\n");
            let len = full.len();
            clipboard::set_text(&full);
            self.status_msg = format!("Copied entire buffer ({} chars)", len);
        } else if let Some(line) = self.lines.get(self.cur_line) {
            clipboard::set_text(line);
            self.status_msg = format!("Copied line {} ({} chars)", self.cur_line + 1, line.len());
        }
    }

    pub fn paste_from_clipboard(&mut self) {
        let clip = clipboard::get_text();
        if clip.is_empty() {
            self.status_msg = String::from("Clipboard is empty");
            return;
        }
        if self.select_all {
            self.lines = vec![String::new()];
            self.cur_line = 0;
            self.cur_col = 0;
            self.select_all = false;
        }
        for ch in clip.chars() {
            if ch == '\n' {
                self.insert_newline();
            } else if ch == '\t' {
                for _ in 0..4 {
                    self.insert_char(' ');
                }
            } else if ch.is_ascii() && !ch.is_ascii_control() {
                self.insert_char(ch);
            }
        }
        self.modified = true;
        self.ensure_cursor_visible();
        self.status_msg = format!("Pasted {} chars", clip.len());
    }

    pub fn select_all_text(&mut self) {
        self.select_all = true;
        self.status_msg = String::from("All text selected  |  Ctrl+C Copy  Backspace Clear");
    }

    fn insert_char(&mut self, ch: char) {
        if self.select_all {
            self.lines = vec![String::new()];
            self.cur_line = 0;
            self.cur_col = 0;
            self.select_all = false;
        }
        if let Some(line) = self.lines.get_mut(self.cur_line) {
            let pos = self.cur_col.min(line.len());
            line.insert(pos, ch);
            self.cur_col = pos + 1;
            self.modified = true;
        }
    }

    fn insert_newline(&mut self) {
        if self.select_all {
            self.lines = vec![String::new()];
            self.cur_line = 0;
            self.cur_col = 0;
            self.select_all = false;
        }
        if self.cur_line >= self.lines.len() {
            return;
        }
        let tail = {
            let line = &mut self.lines[self.cur_line];
            let pos = self.cur_col.min(line.len());
            line.split_off(pos)
        };
        self.cur_line += 1;
        self.cur_col = 0;
        self.lines.insert(self.cur_line, tail);
        self.modified = true;
    }

    fn ensure_cursor_visible(&mut self) {
        let vis = self.last_visible_rows.get().max(1);
        if self.cur_line < self.scroll_y {
            self.scroll_y = self.cur_line;
        } else if self.cur_line >= self.scroll_y + vis {
            self.scroll_y = self.cur_line + 1 - vis;
        }
    }

    pub fn on_wheel(&mut self, delta: i8) {
        let vis = self.last_visible_rows.get().max(1);
        let max_scroll = self.lines.len().saturating_sub(vis);
        if delta > 0 {
            self.scroll_y = (self.scroll_y + 3).min(max_scroll);
        } else if delta < 0 {
            self.scroll_y = self.scroll_y.saturating_sub(3);
        }
    }

    pub fn on_click(&mut self, area: Rect, mx: i32, my: i32) -> DesktopAction {
        let btn_save = Rect::new(area.x + 10, area.y + 6, 62, 26);
        let btn_new = Rect::new(area.x + 78, area.y + 6, 52, 26);
        let btn_copy = Rect::new(area.x + 136, area.y + 6, 56, 26);
        let btn_paste = Rect::new(area.x + 198, area.y + 6, 60, 26);
        let path_box = Rect::new(area.x + 266, area.y + 6, (area.w - 276).max(80), 26);

        if btn_save.contains(mx, my) {
            self.editing_path = false;
            self.save();
            return DesktopAction::None;
        }
        if btn_new.contains(mx, my) {
            self.new_untitled();
            return DesktopAction::None;
        }
        if btn_copy.contains(mx, my) {
            self.copy_to_clipboard();
            return DesktopAction::None;
        }
        if btn_paste.contains(mx, my) {
            self.paste_from_clipboard();
            return DesktopAction::None;
        }
        if path_box.contains(mx, my) {
            self.editing_path = true;
            self.path_input = self.file_path.clone();
            self.status_msg = String::from("Editing path: type path and press Enter (or Esc)");
            return DesktopAction::None;
        }

        if self.editing_path {
            self.commit_path_edit();
        }

        let body = Rect::new(
            area.x,
            area.y + TOOLBAR_H,
            area.w,
            area.h - TOOLBAR_H - STATUS_H,
        );
        if body.contains(mx, my) {
            self.select_all = false;
            let typo = personalization::snapshot().typography;
            let (char_w, line_h) = mono_metrics(typo.mono_scale_q8);
            let row = ((my - (body.y + 6)) / line_h).max(0) as usize;
            let target_line = (self.scroll_y + row).min(self.lines.len().saturating_sub(1));
            self.cur_line = target_line;
            let text_x = body.x + GUTTER_W + 8;
            let col = ((mx - text_x + char_w / 2) / char_w).max(0) as usize;
            self.cur_col = col.min(self.lines[target_line].len());
        }

        DesktopAction::None
    }

    fn commit_path_edit(&mut self) {
        self.editing_path = false;
        let trimmed = self.path_input.trim();
        if trimmed.is_empty() {
            return;
        }
        let normalized = if trimmed.starts_with('/') {
            String::from(trimmed)
        } else {
            format!("/{}", trimmed)
        };
        self.file_path = normalized.clone();
        self.path_input = normalized;
        self.modified = true;
        self.status_msg = format!("Target path set to '{}' — Ctrl+S to save", self.file_path);
    }

    pub fn on_key(&mut self, evt: KeyEvent) -> DesktopAction {
        if self.editing_path {
            match evt.code {
                KeyCode::Enter => self.commit_path_edit(),
                KeyCode::Escape => {
                    self.editing_path = false;
                    self.path_input = self.file_path.clone();
                    self.status_msg = String::from("Path edit cancelled");
                }
                KeyCode::Backspace => {
                    self.path_input.pop();
                }
                KeyCode::Char(c) if c.is_ascii() && !c.is_ascii_control() => {
                    if self.path_input.len() < 80 {
                        self.path_input.push(c);
                    }
                }
                _ => {}
            }
            return DesktopAction::None;
        }

        if evt.modifiers.ctrl {
            match evt.code {
                KeyCode::Char('s') | KeyCode::Char('S') => {
                    self.save();
                    return DesktopAction::None;
                }
                KeyCode::Char('c') | KeyCode::Char('C') => {
                    self.copy_to_clipboard();
                    return DesktopAction::None;
                }
                KeyCode::Char('v') | KeyCode::Char('V') => {
                    self.paste_from_clipboard();
                    return DesktopAction::None;
                }
                KeyCode::Char('a') | KeyCode::Char('A') => {
                    self.select_all_text();
                    return DesktopAction::None;
                }
                _ => {}
            }
        }

        match evt.code {
            KeyCode::Enter => self.insert_newline(),
            KeyCode::Tab => {
                for _ in 0..4 {
                    self.insert_char(' ');
                }
            }
            KeyCode::Backspace => {
                if self.select_all {
                    self.lines = vec![String::new()];
                    self.cur_line = 0;
                    self.cur_col = 0;
                    self.select_all = false;
                    self.modified = true;
                } else if self.cur_col > 0 {
                    let line = &mut self.lines[self.cur_line];
                    let pos = self.cur_col.min(line.len());
                    if pos > 0 {
                        line.remove(pos - 1);
                        self.cur_col = pos - 1;
                        self.modified = true;
                    }
                } else if self.cur_line > 0 {
                    let removed = self.lines.remove(self.cur_line);
                    self.cur_line -= 1;
                    self.cur_col = self.lines[self.cur_line].len();
                    self.lines[self.cur_line].push_str(&removed);
                    self.modified = true;
                }
            }
            KeyCode::Delete => {
                if self.select_all {
                    self.lines = vec![String::new()];
                    self.cur_line = 0;
                    self.cur_col = 0;
                    self.select_all = false;
                    self.modified = true;
                } else if self.cur_col < self.lines[self.cur_line].len() {
                    self.lines[self.cur_line].remove(self.cur_col);
                    self.modified = true;
                } else if self.cur_line + 1 < self.lines.len() {
                    let next_line = self.lines.remove(self.cur_line + 1);
                    self.lines[self.cur_line].push_str(&next_line);
                    self.modified = true;
                }
            }
            KeyCode::Left => {
                self.select_all = false;
                if self.cur_col > 0 {
                    self.cur_col -= 1;
                } else if self.cur_line > 0 {
                    self.cur_line -= 1;
                    self.cur_col = self.lines[self.cur_line].len();
                }
            }
            KeyCode::Right => {
                self.select_all = false;
                if self.cur_col < self.lines[self.cur_line].len() {
                    self.cur_col += 1;
                } else if self.cur_line + 1 < self.lines.len() {
                    self.cur_line += 1;
                    self.cur_col = 0;
                }
            }
            KeyCode::Up => {
                self.select_all = false;
                if self.cur_line > 0 {
                    self.cur_line -= 1;
                    self.cur_col = self.cur_col.min(self.lines[self.cur_line].len());
                }
            }
            KeyCode::Down => {
                self.select_all = false;
                if self.cur_line + 1 < self.lines.len() {
                    self.cur_line += 1;
                    self.cur_col = self.cur_col.min(self.lines[self.cur_line].len());
                }
            }
            KeyCode::Home => {
                self.select_all = false;
                self.cur_col = 0;
            }
            KeyCode::End => {
                self.select_all = false;
                self.cur_col = self.lines[self.cur_line].len();
            }
            KeyCode::PageUp => {
                self.select_all = false;
                let step = self.last_visible_rows.get().max(1);
                self.cur_line = self.cur_line.saturating_sub(step);
                self.cur_col = self.cur_col.min(self.lines[self.cur_line].len());
            }
            KeyCode::PageDown => {
                self.select_all = false;
                let step = self.last_visible_rows.get().max(1);
                self.cur_line = (self.cur_line + step).min(self.lines.len().saturating_sub(1));
                self.cur_col = self.cur_col.min(self.lines[self.cur_line].len());
            }
            KeyCode::Escape => {
                self.select_all = false;
            }
            KeyCode::Char(c) if c.is_ascii() && !c.is_ascii_control() => {
                self.insert_char(c);
            }
            _ => {}
        }

        self.ensure_cursor_visible();
        DesktopAction::None
    }

    pub fn draw(&self, canvas: &mut Canvas<'_>, area: Rect, blink_on: bool, focused: bool) {
        let pal = theme::palette();
        canvas.fill_rect(area, pal.surface_panel);

        // Toolbar
        let toolbar = Rect::new(area.x, area.y, area.w, TOOLBAR_H);
        canvas.fill_rect(toolbar, pal.surface_toolbar);
        canvas.fill_rect(Rect::new(toolbar.x, toolbar.bottom() - 1, toolbar.w, 1), pal.surface_border);

        let save_bg = if self.modified { pal.accent_green } else { pal.surface_card };
        let save_fg = if self.modified { 0xFFFFFFFF } else { pal.text_primary };
        let btns = [
            (Rect::new(area.x + 10, area.y + 6, 62, 26), "Save", save_bg, save_fg),
            (Rect::new(area.x + 78, area.y + 6, 52, 26), "New", pal.surface_card, pal.text_primary),
            (Rect::new(area.x + 136, area.y + 6, 56, 26), "Copy", pal.surface_card, pal.text_primary),
            (Rect::new(area.x + 198, area.y + 6, 60, 26), "Paste", pal.surface_card, pal.text_primary),
        ];
        for (r, label, bg, fg) in btns {
            canvas.fill_rounded_rect(r, 6, bg);
            canvas.stroke_rounded_rect(r, 6, pal.surface_border, 190);
            canvas.draw_text_clipped(r.x + 10, r.y + 5, label, fg, 1, r);
        }

        let path_box = Rect::new(area.x + 266, area.y + 6, (area.w - 276).max(80), 26);
        canvas.fill_rounded_rect(path_box, 6, pal.modal_input_bg);
        canvas.stroke_rounded_rect(
            path_box,
            6,
            if self.editing_path { pal.accent_primary } else { pal.surface_border },
            210,
        );
        let path_show = if self.editing_path {
            &self.path_input
        } else {
            &self.file_path
        };
        canvas.draw_text_mono_clipped(
            path_box.x + 8,
            path_box.y + 5,
            path_show,
            if self.editing_path { pal.accent_primary } else { pal.text_primary },
            path_box,
        );

        // Editor body
        let body = Rect::new(
            area.x,
            area.y + TOOLBAR_H,
            area.w,
            (area.h - TOOLBAR_H - STATUS_H).max(20),
        );
        let gutter = Rect::new(body.x, body.y, GUTTER_W, body.h);
        canvas.fill_rect(gutter, pal.surface_sidebar);
        canvas.fill_rect(Rect::new(gutter.right() - 1, gutter.y, 1, gutter.h), pal.surface_border);

        let typo = personalization::snapshot().typography;
        let (char_w, line_h) = mono_metrics(typo.mono_scale_q8);
        let use_classic = typo.mono_font == MonoFontFamily::Classic8x8;

        let visible_rows = ((body.h - 10) / line_h).max(1) as usize;
        self.last_visible_rows.set(visible_rows);

        let text_area = Rect::new(body.x + GUTTER_W + 8, body.y + 6, body.w - GUTTER_W - 22, body.h - 10);
        for (vi, line_idx) in (self.scroll_y..self.lines.len()).take(visible_rows).enumerate() {
            let y = text_area.y + (vi as i32) * line_h;
            let is_cur = line_idx == self.cur_line;

            if self.select_all {
                canvas.fill_rect_alpha(
                    Rect::new(body.x + GUTTER_W, y - 1, body.w - GUTTER_W - 10, line_h),
                    pal.selection_color,
                    125,
                );
            } else if is_cur && focused {
                canvas.fill_rect_alpha(
                    Rect::new(body.x + GUTTER_W, y - 1, body.w - GUTTER_W - 10, line_h),
                    pal.surface_hover,
                    115,
                );
            }

            let ln_str = format!("{:>3}", line_idx + 1);
            canvas.draw_text_mono_ex_clipped(
                gutter.x + 6,
                y,
                &ln_str,
                if is_cur { pal.text_primary } else { pal.text_muted },
                typo.mono_scale_q8,
                use_classic,
                gutter,
            );

            let line_txt = &self.lines[line_idx];
            canvas.draw_text_mono_ex_clipped(
                text_area.x,
                y,
                line_txt,
                pal.text_primary,
                typo.mono_scale_q8,
                use_classic,
                text_area,
            );

            if is_cur && focused && blink_on && !self.editing_path {
                let cx = text_area.x + (self.cur_col.min(line_txt.len()) as i32) * char_w;
                if cx + 2 < text_area.right() {
                    canvas.fill_rect(Rect::new(cx, y + 1, 2, (line_h - 3).max(12)), pal.accent_primary);
                }
            }
        }

        // Scrollbar
        let sb_track = Rect::new(body.right() - 10, body.y + 4, 8, body.h - 8);
        canvas.draw_scrollbar_v(
            sb_track,
            self.lines.len(),
            visible_rows,
            self.scroll_y,
            pal.text_muted,
            pal.surface_border,
        );

        // Status bar
        let status_bar = Rect::new(area.x, area.bottom() - STATUS_H, area.w, STATUS_H);
        canvas.fill_rect(status_bar, pal.surface_toolbar);
        canvas.fill_rect(Rect::new(status_bar.x, status_bar.y, status_bar.w, 1), pal.surface_border);

        let badge = if self.modified { "MODIFIED" } else { "SAVED" };
        let badge_col = if self.modified { ACCENT_WARN } else { ACCENT_MINT };
        canvas.draw_text_clipped(status_bar.x + 10, status_bar.y + 4, badge, badge_col, 1, status_bar);

        let pos_txt = format!("Ln {}, Col {}", self.cur_line + 1, self.cur_col + 1);
        canvas.draw_text_clipped(status_bar.x + 88, status_bar.y + 4, &pos_txt, pal.text_secondary, 1, status_bar);

        let msg_col = if self.status_msg.contains("failed") || self.status_msg.contains("error") {
            ACCENT_ERR
        } else {
            pal.text_muted
        };
        canvas.draw_text_clipped(
            status_bar.x + 190,
            status_bar.y + 4,
            &self.status_msg,
            msg_col,
            1,
            status_bar,
        );
    }
}
