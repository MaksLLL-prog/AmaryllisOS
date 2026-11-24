use alloc::string::String;
use alloc::vec::Vec;
use core::cell::{Cell, RefCell};

use crate::console::CapturedSpan;
use crate::graphics::personalization::{self, MonoFontFamily};
use crate::graphics::theme::{self, *};
use crate::graphics::widgets::{mono_metrics, Canvas, Rect};
use crate::input::{KeyCode, KeyEvent};

use super::DesktopAction;

const MAX_LINES: usize = 300;

#[derive(Clone)]
struct TermSpan {
    text: String,
    color: u32,
}

#[derive(Clone)]
struct TermLine {
    spans: Vec<TermSpan>,
}

impl TermLine {
    fn new() -> Self {
        Self { spans: Vec::new() }
    }

    fn push(&mut self, text: &str, color: u32) {
        if text.is_empty() {
            return;
        }
        if let Some(last) = self.spans.last_mut() {
            if last.color == color {
                last.text.push_str(text);
                return;
            }
        }
        self.spans.push(TermSpan {
            text: String::from(text),
            color,
        });
    }
}

/// Splits a logical line of colored spans into wrapped visual rows of at most `max_cols` characters.
fn wrap_term_line(line: &TermLine, max_cols: usize) -> Vec<Vec<TermSpan>> {
    let max_cols = max_cols.max(1);
    let mut rows: Vec<Vec<TermSpan>> = Vec::new();
    let mut cur_row: Vec<TermSpan> = Vec::new();
    let mut col = 0usize;

    for span in &line.spans {
        for ch in span.text.chars() {
            if col >= max_cols {
                rows.push(cur_row);
                cur_row = Vec::new();
                col = 0;
            }
            if let Some(last) = cur_row.last_mut() {
                if last.color == span.color {
                    last.text.push(ch);
                } else {
                    let mut s = String::new();
                    s.push(ch);
                    cur_row.push(TermSpan {
                        text: s,
                        color: span.color,
                    });
                }
            } else {
                let mut s = String::new();
                s.push(ch);
                cur_row.push(TermSpan {
                    text: s,
                    color: span.color,
                });
            }
            col += 1;
        }
    }

    if !cur_row.is_empty() || rows.is_empty() {
        rows.push(cur_row);
    }
    rows
}

pub struct TerminalApp {
    lines: Vec<TermLine>,
    input: String,
    cursor: usize,
    history: Vec<String>,
    hist_idx: usize,
    scroll_up: usize,
    last_max_scroll: Cell<usize>,
    cached_cols: Cell<usize>,
    cached_vrows: RefCell<Vec<Vec<TermSpan>>>,
}

impl TerminalApp {
    pub fn new() -> Self {
        let mut app = Self {
            lines: Vec::new(),
            input: String::new(),
            cursor: 0,
            history: Vec::new(),
            hist_idx: 0,
            scroll_up: 0,
            last_max_scroll: Cell::new(0),
            cached_cols: Cell::new(0),
            cached_vrows: RefCell::new(Vec::new()),
        };
        app.push_line("AmaryllisOS Interactive Shell (GUI Session)", ACCENT_MINT);
        app.push_line(
            "Type 'help', 'ls', 'diskinfo', 'netinfo', 'mkelf', 'exec /hello.elf', or 'exit'.",
            TEXT_SECONDARY,
        );
        app
    }

    pub fn push_line(&mut self, text: &str, color: u32) {
        let mut line = TermLine::new();
        line.push(text, color);
        self.append_term_line(line);
    }

    fn append_term_line(&mut self, line: TermLine) {
        if self.lines.len() >= MAX_LINES {
            self.lines.remove(0);
        }
        self.lines.push(line);
        self.scroll_up = 0;
        self.cached_cols.set(0);
    }

    pub fn append_captured(&mut self, cmd: &str, spans: Vec<CapturedSpan>) {
        let cwd = crate::storage::vfs::current_working_directory();
        let mut prompt_line = TermLine::new();
        prompt_line.push("amaryllis:", ACCENT_MINT);
        prompt_line.push(&cwd, ACCENT_WARN);
        prompt_line.push("$ ", ACCENT_MINT);
        prompt_line.push(cmd, TEXT_PRIMARY);
        self.append_term_line(prompt_line);

        let mut cur_line = TermLine::new();
        for span in spans {
            if span.text == "\x0c" {
                self.lines.clear();
                self.cached_cols.set(0);
                cur_line = TermLine::new();
                continue;
            }
            if span.text == "\x08" {
                continue;
            }
            let c = rgb_tuple_to_u32(span.color);
            let mut first = true;
            for part in span.text.split('\n') {
                if !first {
                    self.append_term_line(cur_line);
                    cur_line = TermLine::new();
                }
                first = false;
                if !part.is_empty() {
                    cur_line.push(part, c);
                }
            }
        }
        if !cur_line.spans.is_empty() {
            self.append_term_line(cur_line);
        }
    }

    pub fn append_user_output(&mut self, spans: Vec<CapturedSpan>) {
        let mut cur_line = TermLine::new();
        for span in spans {
            let c = rgb_tuple_to_u32(span.color);
            let mut first = true;
            for part in span.text.split('\n') {
                if !first {
                    self.append_term_line(cur_line);
                    cur_line = TermLine::new();
                }
                first = false;
                if !part.is_empty() {
                    cur_line.push(part, c);
                }
            }
        }
        if !cur_line.spans.is_empty() {
            self.append_term_line(cur_line);
        }
    }

    pub fn on_wheel(&mut self, delta: i8) {
        let max_s = self.last_max_scroll.get();
        if delta > 0 {
            self.scroll_up = (self.scroll_up + 3).min(max_s);
        } else if delta < 0 {
            self.scroll_up = self.scroll_up.saturating_sub(3);
        }
    }

    pub fn clear_lines(&mut self) {
        self.lines.clear();
        self.scroll_up = 0;
        self.cached_cols.set(0);
    }

    pub fn copy_to_clipboard(&self) {
        if !self.input.is_empty() {
            crate::graphics::clipboard::set_text(&self.input);
            return;
        }
        if let Some(last) = self.lines.last() {
            let mut out = String::new();
            for span in &last.spans {
                out.push_str(&span.text);
            }
            crate::graphics::clipboard::set_text(&out);
        }
    }

    pub fn paste_from_clipboard(&mut self) {
        let clip = crate::graphics::clipboard::get_text();
        for ch in clip.chars() {
            if ch == '\n' || ch == '\r' {
                break;
            }
            if ch.is_ascii() && !ch.is_ascii_control() && self.input.len() < 160 {
                self.input.insert(self.cursor, ch);
                self.cursor += 1;
            }
        }
    }

    pub fn on_key(&mut self, evt: KeyEvent) -> DesktopAction {
        if evt.modifiers.ctrl {
            match evt.code {
                KeyCode::Char('c') | KeyCode::Char('C') => {
                    self.copy_to_clipboard();
                    return DesktopAction::None;
                }
                KeyCode::Char('v') | KeyCode::Char('V') => {
                    self.paste_from_clipboard();
                    return DesktopAction::None;
                }
                KeyCode::Char('l') | KeyCode::Char('L') => {
                    self.clear_lines();
                    return DesktopAction::None;
                }
                _ => {}
            }
        }

        match evt.code {
            KeyCode::Enter => {
                let cmd = String::from(self.input.trim());
                self.input.clear();
                self.cursor = 0;
                if cmd == "exit" || cmd == "logout" {
                    return DesktopAction::CloseDesktop;
                }
                if !cmd.is_empty() {
                    if self.history.last().map(|s| s.as_str()) != Some(cmd.as_str()) {
                        self.history.push(cmd.clone());
                    }
                    self.hist_idx = self.history.len();
                    if cmd == "clear" {
                        self.clear_lines();
                        return DesktopAction::None;
                    }
                    return DesktopAction::RunShellCommand(cmd);
                }
                DesktopAction::None
            }
            KeyCode::Backspace => {
                if self.cursor > 0 && self.cursor <= self.input.len() {
                    self.input.remove(self.cursor - 1);
                    self.cursor -= 1;
                }
                DesktopAction::None
            }
            KeyCode::Delete => {
                if self.cursor < self.input.len() {
                    self.input.remove(self.cursor);
                }
                DesktopAction::None
            }
            KeyCode::Left => {
                self.cursor = self.cursor.saturating_sub(1);
                DesktopAction::None
            }
            KeyCode::Right => {
                if self.cursor < self.input.len() {
                    self.cursor += 1;
                }
                DesktopAction::None
            }
            KeyCode::Home => {
                self.cursor = 0;
                DesktopAction::None
            }
            KeyCode::End => {
                self.cursor = self.input.len();
                DesktopAction::None
            }
            KeyCode::Up => {
                if !self.history.is_empty() && self.hist_idx > 0 {
                    self.hist_idx -= 1;
                    self.input = self.history[self.hist_idx].clone();
                    self.cursor = self.input.len();
                }
                DesktopAction::None
            }
            KeyCode::Down => {
                if self.hist_idx + 1 < self.history.len() {
                    self.hist_idx += 1;
                    self.input = self.history[self.hist_idx].clone();
                    self.cursor = self.input.len();
                } else {
                    self.hist_idx = self.history.len();
                    self.input.clear();
                    self.cursor = 0;
                }
                DesktopAction::None
            }
            KeyCode::PageUp => {
                let max_s = self.last_max_scroll.get();
                self.scroll_up = (self.scroll_up + 8).min(max_s);
                DesktopAction::None
            }
            KeyCode::PageDown => {
                self.scroll_up = self.scroll_up.saturating_sub(8);
                DesktopAction::None
            }
            KeyCode::Char(ch) if ch.is_ascii() && !ch.is_ascii_control() => {
                if self.input.len() < 160 {
                    self.input.insert(self.cursor, ch);
                    self.cursor += 1;
                }
                DesktopAction::None
            }
            _ => DesktopAction::None,
        }
    }

    pub fn draw(&self, canvas: &mut Canvas<'_>, area: Rect, blink_on: bool, focused: bool) {
        let pal = theme::palette();
        let typo = personalization::snapshot().typography;
        let (char_w, line_h) = mono_metrics(typo.mono_scale_q8);
        let use_classic = typo.mono_font == MonoFontFamily::Classic8x8;
        let term_bg = if pal.is_light { 0xFF0B0B0E } else { pal.surface_panel };

        canvas.fill_rect(area, term_bg);
        let pad_x = area.x + 10;
        let pad_y = area.y + 8;
        let max_cols = ((area.w - 20) / char_w).max(8) as usize;
        let visible_rows = ((area.h - 14) / line_h).max(2) as usize;

        if self.cached_cols.get() != max_cols {
            let mut vrows = self.cached_vrows.borrow_mut();
            vrows.clear();
            for line in &self.lines {
                vrows.extend(wrap_term_line(line, max_cols));
            }
            self.cached_cols.set(max_cols);
        }

        let cwd = crate::storage::vfs::current_working_directory();
        let mut active_prompt = TermLine::new();
        active_prompt.push("amaryllis:", ACCENT_MINT);
        active_prompt.push(&cwd, ACCENT_WARN);
        active_prompt.push("$ ", ACCENT_MINT);
        active_prompt.push(&self.input, TEXT_PRIMARY);

        let prompt_vlines = wrap_term_line(&active_prompt, max_cols);
        let prefix_chars = "amaryllis:".len() + cwd.chars().count() + 2;
        let cursor_total_col = prefix_chars + self.cursor.min(self.input.len());
        let cursor_vrow = cursor_total_col / max_cols;
        let cursor_vcol = cursor_total_col % max_cols;

        let max_prompt_rows = visible_rows.saturating_sub(1).max(1);
        let prompt_slice_start = if prompt_vlines.len() > max_prompt_rows {
            prompt_vlines.len() - max_prompt_rows
        } else {
            0
        };
        let shown_prompt = &prompt_vlines[prompt_slice_start..];
        let log_budget = visible_rows.saturating_sub(shown_prompt.len());

        let visual_log = self.cached_vrows.borrow();
        let max_scroll = visual_log.len().saturating_sub(log_budget);
        self.last_max_scroll.set(max_scroll);
        let eff_scroll = self.scroll_up.min(max_scroll);

        let end_idx = visual_log.len().saturating_sub(eff_scroll);
        let start_idx = end_idx.saturating_sub(log_budget);

        let mut y = pad_y;
        for vrow in &visual_log[start_idx..end_idx] {
            let mut x = pad_x;
            for span in vrow {
                canvas.draw_text_mono_ex_clipped(
                    x,
                    y,
                    &span.text,
                    span.color,
                    typo.mono_scale_q8,
                    use_classic,
                    area,
                );
                x += (span.text.chars().count() as i32) * char_w;
            }
            y += line_h;
        }

        for (idx, vrow) in shown_prompt.iter().enumerate() {
            if y + line_h > area.bottom() {
                break;
            }
            let mut x = pad_x;
            for span in vrow {
                canvas.draw_text_mono_ex_clipped(
                    x,
                    y,
                    &span.text,
                    span.color,
                    typo.mono_scale_q8,
                    use_classic,
                    area,
                );
                x += (span.text.chars().count() as i32) * char_w;
            }
            let abs_row = prompt_slice_start + idx;
            if focused && blink_on && abs_row == cursor_vrow {
                let cur_px = pad_x + (cursor_vcol as i32) * char_w;
                if cur_px + char_w - 1 < area.right() {
                    canvas.fill_rect(Rect::new(cur_px, y + line_h - 4, (char_w - 1).max(4), 2), pal.accent_primary);
                }
            }
            y += line_h;
        }
    }
}
