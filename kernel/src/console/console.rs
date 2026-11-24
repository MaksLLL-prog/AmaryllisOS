
extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

use crate::graphics::{Color, Renderer};
use spin::Mutex;

const CONTENT_TOP: usize = 50;
const CHAR_WIDTH: usize = 9;
const CHAR_HEIGHT: usize = 10;
const LINE_HEIGHT: usize = 13;

const HISTORY_LINES: usize = 512;
const MAX_LINE_CHARS: usize = 160;

#[derive(Clone, Copy)]
struct Cell {
    character: u8,
    color: Color,
}

impl Cell {
    const EMPTY: Self = Self {
        character: 0,
        color: (235, 235, 240),
    };
}

#[derive(Clone, Copy)]
struct HistoryLine {
    cells: [Cell; MAX_LINE_CHARS],
    len: usize,
}

impl HistoryLine {
    const fn new() -> Self {
        Self {
            cells: [Cell::EMPTY; MAX_LINE_CHARS],
            len: 0,
        }
    }

    fn clear(&mut self) {
        self.len = 0;
    }

    fn push(&mut self, character: u8, color: Color) {
        if self.len >= MAX_LINE_CHARS {
            return;
        }

        self.cells[self.len] = Cell {
            character,
            color,
        };

        self.len += 1;
    }

    fn pop(&mut self) {
        if self.len > 0 {
            self.len -= 1;
            self.cells[self.len] = Cell::EMPTY;
        }
    }
}

struct History {
    lines: [HistoryLine; HISTORY_LINES],
    head: usize,
    len: usize,
    view_offset: usize,
}

impl History {
    const fn new() -> Self {
        Self {
            lines: [HistoryLine::new(); HISTORY_LINES],
            head: 0,
            len: 1,
            view_offset: 0,
        }
    }

    fn current_index(&self) -> usize {
        (self.head + self.len - 1) % HISTORY_LINES
    }

    fn current_line_mut(&mut self) -> &mut HistoryLine {
        let index = self.current_index();
        &mut self.lines[index]
    }

    fn push_char(&mut self, character: u8, color: Color) {
        self.current_line_mut().push(character, color);
    }

    fn newline(&mut self) {
        let next = (self.current_index() + 1) % HISTORY_LINES;

        if self.len < HISTORY_LINES {
            self.len += 1;
        } else {
            self.head = (self.head + 1) % HISTORY_LINES;

            if self.view_offset > 0 {
                self.view_offset = self.view_offset.saturating_sub(1);
            }
        }

        self.lines[next].clear();
    }

    fn clear(&mut self) {
        self.lines = [HistoryLine::new(); HISTORY_LINES];
        self.head = 0;
        self.len = 1;
        self.view_offset = 0;
    }

    fn scroll(&mut self, amount: isize, visible_lines: usize) {
        let max_offset = self.len.saturating_sub(visible_lines);

        if amount > 0 {
            self.view_offset = self
                .view_offset
                .saturating_add(amount as usize)
                .min(max_offset);
        } else if amount < 0 {
            self.view_offset = self
                .view_offset
                .saturating_sub(amount.unsigned_abs());
        }
    }

    fn line_at(&self, offset: usize) -> &HistoryLine {
        let index = (self.head + offset) % HISTORY_LINES;
        &self.lines[index]
    }

    fn view_start(&self, visible_lines: usize) -> usize {
        self.len
            .saturating_sub(visible_lines)
            .saturating_sub(self.view_offset)
    }
}

static HISTORY: Mutex<History> = Mutex::new(History::new());

#[derive(Clone)]
pub struct CapturedSpan {
    pub text: String,
    pub color: Color,
}

#[derive(Clone, Copy)]
pub struct CapturedLine {
    pub spans_start: usize,
}

struct CaptureState {
    active: bool,
    spans: Vec<CapturedSpan>,
}

impl CaptureState {
    const fn new() -> Self {
        Self {
            active: false,
            spans: Vec::new(),
        }
    }
}

static CAPTURE: Mutex<CaptureState> = Mutex::new(CaptureState::new());

pub fn begin_capture() {
    let mut cap = CAPTURE.lock();
    cap.spans.clear();
    cap.active = true;
}

pub fn end_capture() -> Vec<CapturedSpan> {
    let mut cap = CAPTURE.lock();
    cap.active = false;
    core::mem::take(&mut cap.spans)
}

fn push_capture(text: &str, color: Color) -> bool {
    let mut cap = CAPTURE.lock();
    if !cap.active {
        return false;
    }
    if let Some(last) = cap.spans.last_mut() {
        if last.color == color && last.text.len() + text.len() < 4096 {
            last.text.push_str(text);
            return true;
        }
    }
    cap.spans.push(CapturedSpan {
        text: String::from(text),
        color,
    });
    true
}

pub struct Console<'a> {
    renderer: Renderer<'a>,
    cursor_x: usize,
    cursor_y: usize,
    margin: usize,
    scale: usize,
    background: Color,
}

impl<'a> Console<'a> {
    pub fn new(renderer: Renderer<'a>) -> Self {
        Self {
            renderer,
            cursor_x: 24,
            cursor_y: CONTENT_TOP,
            margin: 24,
            scale: 1,
            background: (8, 8, 12),
        }
    }

    pub fn width(&self) -> usize {
        self.renderer.width()
    }

    pub fn height(&self) -> usize {
        self.renderer.height()
    }

    pub fn renderer_mut(&mut self) -> &mut Renderer<'a> {
        &mut self.renderer
    }

    pub fn repaint_full(&mut self) {
        self.renderer.clear(self.background);
        self.renderer.draw_rect(0, 0, self.renderer.width(), 5, (40, 180, 120));
        self.render_history();

        let visible_lines = self.visible_lines();
        let history = HISTORY.lock();
        let start = history.view_start(visible_lines);
        let used_lines = history.len.saturating_sub(start).min(visible_lines);
        let last_screen_line = used_lines.saturating_sub(1);
        let line_len = if history.len > 0 {
            history.line_at(history.len - 1).len
        } else {
            0
        };
        drop(history);

        self.cursor_x = self.margin + line_len * CHAR_WIDTH * self.scale;
        self.cursor_y = CONTENT_TOP + last_screen_line * LINE_HEIGHT * self.scale;
    }

    pub fn draw_rect(
        &mut self,
        x: usize,
        y: usize,
        width: usize,
        height: usize,
        color: Color,
    ) {
        self.renderer.draw_rect(x, y, width, height, color);
    }

    pub fn cursor_position(&self) -> (usize, usize) {
        (self.cursor_x, self.cursor_y)
    }

    fn visible_lines(&self) -> usize {
        self.renderer
            .height()
            .saturating_sub(CONTENT_TOP)
            / (LINE_HEIGHT * self.scale).max(1)
    }

    fn draw_history_line(
        &mut self,
        history: &History,
        history_index: usize,
        screen_line: usize,
    ) {
        let line_height = LINE_HEIGHT * self.scale;
        let char_width = CHAR_WIDTH * self.scale;
        let y = CONTENT_TOP + screen_line * line_height;

        self.renderer.draw_rect(
            0,
            y,
            self.renderer.width(),
            line_height,
            self.background,
        );

        if history_index >= history.len {
            return;
        }

        let line = history.line_at(history_index);

        for column in 0..line.len {
            let cell = line.cells[column];

            self.renderer.draw_char(
                self.margin + column * char_width,
                y,
                cell.character as char,
                cell.color,
                self.scale,
            );
        }
    }

    fn render_history(&mut self) {
        let visible_lines = self.visible_lines();

        let history = HISTORY.lock();
        let start = history.view_start(visible_lines);

        self.renderer.draw_rect(
            0,
            CONTENT_TOP,
            self.renderer.width(),
            self.renderer.height().saturating_sub(CONTENT_TOP),
            self.background,
        );

        for screen_line in 0..visible_lines {
            self.draw_history_line(
                &history,
                start + screen_line,
                screen_line,
            );
        }
    }

    pub fn scroll_history(&mut self, amount: isize) {
        let visible_lines = self.visible_lines();

        if visible_lines == 0 || amount == 0 {
            return;
        }

        // Инвертируем направление колёсика.
        let amount = amount.saturating_neg();

        let (old_start, new_start) = {
            let mut history = HISTORY.lock();

            let old_start = history.view_start(visible_lines);

            history.scroll(amount, visible_lines);

            let new_start = history.view_start(visible_lines);

            (old_start, new_start)
        };

        if old_start == new_start {
            return;
        }

        let line_difference = old_start.abs_diff(new_start);

        // При большом скачке дешевле перерисовать всю область.
        if line_difference >= visible_lines {
            self.render_history();
            return;
        }

        let line_height = LINE_HEIGHT * self.scale;
        let pixels = line_difference * line_height;

        let history = HISTORY.lock();

        if new_start < old_start {
            // Переходим к более старым строкам.
            // Существующее изображение сдвигается вниз.
            self.renderer.scroll_down_from(
                CONTENT_TOP,
                pixels,
                self.background,
            );

            for screen_line in 0..line_difference {
                self.draw_history_line(
                    &history,
                    new_start + screen_line,
                    screen_line,
                );
            }
        } else {
            // Переходим к более новым строкам.
            // Существующее изображение сдвигается вверх.
            self.renderer.scroll_up_from(
                CONTENT_TOP,
                pixels,
                self.background,
            );

            for screen_line in (visible_lines - line_difference)..visible_lines {
                self.draw_history_line(
                    &history,
                    new_start + screen_line,
                    screen_line,
                );
            }
        }
    }

    pub fn is_history_scrolled(&self) -> bool {
        HISTORY.lock().view_offset > 0
    }

    pub fn redraw_input(
        &mut self,
        start_x: usize,
        start_y: usize,
        text: &str,
        cursor_position: usize,
        color: Color,
    ) {
        let available_width = self.renderer.width().saturating_sub(start_x);

        self.renderer.draw_rect(
            start_x,
            start_y,
            available_width,
            CHAR_HEIGHT * self.scale + 2,
            self.background,
        );

        for (index, character) in text.chars().enumerate() {
            self.renderer.draw_char(
                start_x + index * CHAR_WIDTH * self.scale,
                start_y,
                character,
                color,
                self.scale,
            );
        }

        self.cursor_x = start_x.saturating_add(
            cursor_position * CHAR_WIDTH * self.scale,
        );

        self.cursor_y = start_y;
    }

    pub fn show_cursor(&mut self) {
        self.renderer.draw_rect(
            self.cursor_x,
            self.cursor_y + 9 * self.scale,
            7 * self.scale,
            2 * self.scale,
            (235, 235, 240),
        );
    }

    pub fn hide_cursor(&mut self) {
        self.renderer.draw_rect(
            self.cursor_x,
            self.cursor_y + 9 * self.scale,
            7 * self.scale,
            2 * self.scale,
            self.background,
        );
    }

    pub fn newline(&mut self) {
        if push_capture("\n", (235, 235, 240)) {
            HISTORY.lock().newline();
            return;
        }

        let follow_output = {
            let mut history = HISTORY.lock();
            history.newline();
            history.view_offset == 0
        };

        self.cursor_x = self.margin;

        let line_height = LINE_HEIGHT * self.scale;
        let char_height = CHAR_HEIGHT * self.scale;

        let next_y = self.cursor_y.saturating_add(line_height);
        let screen_bottom = self.renderer.height();

        if next_y.saturating_add(char_height) > screen_bottom {
            if follow_output {
                self.renderer.scroll_up_from(
                    CONTENT_TOP,
                    line_height,
                    self.background,
                );
            }

            self.cursor_y = screen_bottom
                .saturating_sub(char_height)
                .saturating_sub(2);
        } else {
            self.cursor_y = next_y;
        }
    }

    pub fn write(&mut self, text: &str, color: Color) {
        if push_capture(text, color) {
            let mut history = HISTORY.lock();
            for ch in text.chars() {
                if ch == '\n' {
                    history.newline();
                } else if ch != '\r' {
                    history.push_char(ch as u8, color);
                }
            }
            return;
        }

        for character in text.chars() {
            match character {
                '\n' => {
                    self.newline();
                    continue;
                }
                '\r' => {
                    self.cursor_x = self.margin;
                    continue;
                }
                _ => {}
            }

            let char_width = CHAR_WIDTH * self.scale;

            if self.cursor_x.saturating_add(char_width) > self.renderer.width() {
                self.newline();
            }

            let should_draw = {
                let mut history = HISTORY.lock();

                history.push_char(character as u8, color);

                history.view_offset == 0
            };

            if should_draw {
                self.renderer.draw_char(
                    self.cursor_x,
                    self.cursor_y,
                    character,
                    color,
                    self.scale,
                );
            }

            self.cursor_x = self.cursor_x.saturating_add(char_width);
        }
    }

    pub fn backspace(&mut self) {
        if push_capture("\x08", (235, 235, 240)) {
            HISTORY.lock().current_line_mut().pop();
            return;
        }

        if self.cursor_x > self.margin {
            self.cursor_x = self.cursor_x.saturating_sub(
                CHAR_WIDTH * self.scale,
            );

            let should_draw = {
                let mut history = HISTORY.lock();

                history.current_line_mut().pop();

                history.view_offset == 0
            };

            if should_draw {
                self.renderer.draw_rect(
                    self.cursor_x,
                    self.cursor_y,
                    CHAR_WIDTH * self.scale,
                    CHAR_HEIGHT * self.scale,
                    self.background,
                );
            }
        }
    }

    pub fn clear(&mut self) {
        HISTORY.lock().clear();

        if CAPTURE.lock().active {
            push_capture("\x0c", (235, 235, 240));
            return;
        }

        self.renderer.clear(self.background);

        self.cursor_x = self.margin;
        self.cursor_y = CONTENT_TOP;
    }

    pub fn set_scale(&mut self, scale: usize) {
        self.scale = scale.max(1);
        self.render_history();
    }
}
