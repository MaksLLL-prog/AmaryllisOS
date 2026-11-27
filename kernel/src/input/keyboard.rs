
use core::arch::asm;
use core::sync::atomic::{AtomicU8, AtomicUsize, Ordering};

const QUEUE_SIZE: usize = 256;

static QUEUE: [AtomicU8; QUEUE_SIZE] =
    [const { AtomicU8::new(0) }; QUEUE_SIZE];

static HEAD: AtomicUsize = AtomicUsize::new(0);
static TAIL: AtomicUsize = AtomicUsize::new(0);

const DATA_PORT: u16 = 0x60;
const STATUS_PORT: u16 = 0x64;
const STATUS_OUTPUT_FULL: u8 = 0x01;
const STATUS_AUX_DATA: u8 = 0x20;

#[inline]
unsafe fn inb(port: u16) -> u8 {
    let value: u8;
    unsafe {
        asm!(
            "in al, dx",
            in("dx") port,
            out("al") value,
            options(nomem, nostack, preserves_flags)
        );
    }
    value
}

#[inline]
pub(super) fn push_scancode_from_hw(scancode: u8) {
    let head = HEAD.load(Ordering::Relaxed);
    let next = (head + 1) % QUEUE_SIZE;
    let tail = TAIL.load(Ordering::Acquire);

    // Drop the scancode if the queue is full.
    if next != tail {
        QUEUE[head].store(scancode, Ordering::Relaxed);
        HEAD.store(next, Ordering::Release);
    }
}

/// Public entry point for USB HID keyboard drivers to inject PS/2 Set 1 scancodes.
pub fn push_scancode(scancode: u8) {
    push_scancode_from_hw(scancode);
}

static USB_PREV_MODIFIERS: AtomicU8 = AtomicU8::new(0);
static USB_PREV_KEYS: [AtomicU8; 6] = [const { AtomicU8::new(0) }; 6];

fn hid_usage_to_set1(usage: u8) -> Option<(bool, u8)> {
    let pair = match usage {
        0x04 => (false, 0x1E), // A
        0x05 => (false, 0x30), // B
        0x06 => (false, 0x2E), // C
        0x07 => (false, 0x20), // D
        0x08 => (false, 0x12), // E
        0x09 => (false, 0x21), // F
        0x0A => (false, 0x22), // G
        0x0B => (false, 0x23), // H
        0x0C => (false, 0x17), // I
        0x0D => (false, 0x24), // J
        0x0E => (false, 0x25), // K
        0x0F => (false, 0x26), // L
        0x10 => (false, 0x32), // M
        0x11 => (false, 0x31), // N
        0x12 => (false, 0x18), // O
        0x13 => (false, 0x19), // P
        0x14 => (false, 0x10), // Q
        0x15 => (false, 0x13), // R
        0x16 => (false, 0x1F), // S
        0x17 => (false, 0x14), // T
        0x18 => (false, 0x16), // U
        0x19 => (false, 0x2F), // V
        0x1A => (false, 0x11), // W
        0x1B => (false, 0x2D), // X
        0x1C => (false, 0x15), // Y
        0x1D => (false, 0x2C), // Z
        0x1E => (false, 0x02), // 1
        0x1F => (false, 0x03), // 2
        0x20 => (false, 0x04), // 3
        0x21 => (false, 0x05), // 4
        0x22 => (false, 0x06), // 5
        0x23 => (false, 0x07), // 6
        0x24 => (false, 0x08), // 7
        0x25 => (false, 0x09), // 8
        0x26 => (false, 0x0A), // 9
        0x27 => (false, 0x0B), // 0
        0x28 => (false, 0x1C), // Enter
        0x29 => (false, 0x01), // Escape
        0x2A => (false, 0x0E), // Backspace
        0x2B => (false, 0x0F), // Tab
        0x2C => (false, 0x39), // Space
        0x2D => (false, 0x0C), // -
        0x2E => (false, 0x0D), // =
        0x2F => (false, 0x1A), // [
        0x30 => (false, 0x1B), // ]
        0x31 => (false, 0x2B), // backslash
        0x33 => (false, 0x27), // ;
        0x34 => (false, 0x28), // '
        0x35 => (false, 0x29), // `
        0x36 => (false, 0x33), // ,
        0x37 => (false, 0x34), // .
        0x38 => (false, 0x35), // /
        0x3A => (false, 0x3B), // F1
        0x3B => (false, 0x3C), // F2
        0x3C => (false, 0x3D), // F3
        0x3D => (false, 0x3E), // F4
        0x3E => (false, 0x3F), // F5
        0x4A => (true, 0x47),  // Home
        0x4B => (true, 0x49),  // PageUp
        0x4C => (true, 0x53),  // Delete
        0x4D => (true, 0x4F),  // End
        0x4E => (true, 0x51),  // PageDown
        0x4F => (true, 0x4D),  // Right
        0x50 => (true, 0x4B),  // Left
        0x51 => (true, 0x50),  // Down
        0x52 => (true, 0x48),  // Up
        0x58 => (true, 0x1C),  // KP Enter
        _ => return None,
    };
    Some(pair)
}

#[inline]
fn emit_set1(ext: bool, code: u8, released: bool) {
    if ext {
        push_scancode_from_hw(0xE0);
    }
    push_scancode_from_hw(if released { code | 0x80 } else { code });
}

/// Decodes a standard 8-byte USB HID Boot Keyboard report into PS/2 Set 1 make/break events.
pub fn push_hid_key_report(modifiers: u8, keys: &[u8; 6]) {
    let prev_mod = USB_PREV_MODIFIERS.swap(modifiers, Ordering::AcqRel);
    let mod_map: [(u8, bool, u8); 8] = [
        (0, false, 0x1D), // LCtrl
        (1, false, 0x2A), // LShift
        (2, false, 0x38), // LAlt
        (3, true, 0x5B),  // LGui/Super
        (4, true, 0x1D),  // RCtrl
        (5, false, 0x36), // RShift
        (6, true, 0x38),  // RAlt
        (7, true, 0x5C),  // RGui/Super
    ];
    for &(bit, ext, sc) in &mod_map {
        let was = (prev_mod & (1 << bit)) != 0;
        let now = (modifiers & (1 << bit)) != 0;
        if was != now {
            emit_set1(ext, sc, !now);
        }
    }

    let mut prev = [0u8; 6];
    for i in 0..6 {
        prev[i] = USB_PREV_KEYS[i].load(Ordering::Acquire);
    }

    // Break scancodes for released keys
    for &old_k in &prev {
        if old_k > 3 && !keys.contains(&old_k) {
            if let Some((ext, sc)) = hid_usage_to_set1(old_k) {
                emit_set1(ext, sc, true);
            }
        }
    }
    // Make scancodes for newly pressed keys
    for &new_k in keys {
        if new_k > 3 && !prev.contains(&new_k) {
            if let Some((ext, sc)) = hid_usage_to_set1(new_k) {
                emit_set1(ext, sc, false);
            }
        }
    }

    for i in 0..6 {
        USB_PREV_KEYS[i].store(keys[i], Ordering::Release);
    }
}

// Called from the keyboard interrupt handler.
pub fn handle_interrupt() {
    let status = unsafe { inb(STATUS_PORT) };
    if (status & STATUS_OUTPUT_FULL) == 0 {
        return;
    }

    let byte = unsafe { inb(DATA_PORT) };
    if (status & STATUS_AUX_DATA) != 0 {
        super::mouse::push_aux_byte_from_irq(byte);
        return;
    }

    push_scancode_from_hw(byte);
}

/// Non-blocking recovery helper: drains pending i8042 output bytes so an unread
/// keyboard byte cannot stall edge-triggered IRQ1/IRQ12 delivery.
pub fn poll_if_pending() -> usize {
    let first_status = unsafe { inb(STATUS_PORT) };
    if (first_status & STATUS_OUTPUT_FULL) == 0 {
        return 0;
    }

    let mut count = 0usize;
    x86_64::instructions::interrupts::without_interrupts(|| {
        for _ in 0..16 {
            let status = unsafe { inb(STATUS_PORT) };
            if (status & STATUS_OUTPUT_FULL) == 0 {
                break;
            }
            let byte = unsafe { inb(DATA_PORT) };
            if (status & STATUS_AUX_DATA) != 0 {
                super::mouse::push_aux_byte_from_irq(byte);
            } else {
                push_scancode_from_hw(byte);
                count += 1;
            }
        }
    });
    count
}

// Read the next scancode from the queue.
pub fn read_scancode() -> Option<u8> {
    let tail = TAIL.load(Ordering::Relaxed);
    let head = HEAD.load(Ordering::Acquire);

    if tail == head {
        return None;
    }

    let scancode = QUEUE[tail].load(Ordering::Relaxed);
    TAIL.store((tail + 1) % QUEUE_SIZE, Ordering::Release);

    Some(scancode)
}

// Convert PS/2 Set 1 scancodes to UK keyboard characters.

pub fn scancode_to_char(scancode: u8, shift: bool) -> Option<char> {
    let character = match scancode {
        // Number row — US layout
        0x02 => if shift { '!' } else { '1' },
        0x03 => if shift { '@' } else { '2' },
        0x04 => if shift { '#' } else { '3' },
        0x05 => if shift { '$' } else { '4' },
        0x06 => if shift { '%' } else { '5' },
        0x07 => if shift { '^' } else { '6' },
        0x08 => if shift { '&' } else { '7' },
        0x09 => if shift { '*' } else { '8' },
        0x0A => if shift { '(' } else { '9' },
        0x0B => if shift { ')' } else { '0' },

        // Top row
        0x10 => if shift { 'Q' } else { 'q' },
        0x11 => if shift { 'W' } else { 'w' },
        0x12 => if shift { 'E' } else { 'e' },
        0x13 => if shift { 'R' } else { 'r' },
        0x14 => if shift { 'T' } else { 't' },
        0x15 => if shift { 'Y' } else { 'y' },
        0x16 => if shift { 'U' } else { 'u' },
        0x17 => if shift { 'I' } else { 'i' },
        0x18 => if shift { 'O' } else { 'o' },
        0x19 => if shift { 'P' } else { 'p' },

        // Home row
        0x1E => if shift { 'A' } else { 'a' },
        0x1F => if shift { 'S' } else { 's' },
        0x20 => if shift { 'D' } else { 'd' },
        0x21 => if shift { 'F' } else { 'f' },
        0x22 => if shift { 'G' } else { 'g' },
        0x23 => if shift { 'H' } else { 'h' },
        0x24 => if shift { 'J' } else { 'j' },
        0x25 => if shift { 'K' } else { 'k' },
        0x26 => if shift { 'L' } else { 'l' },

        // Bottom row
        0x2C => if shift { 'Z' } else { 'z' },
        0x2D => if shift { 'X' } else { 'x' },
        0x2E => if shift { 'C' } else { 'c' },
        0x2F => if shift { 'V' } else { 'v' },
        0x30 => if shift { 'B' } else { 'b' },
        0x31 => if shift { 'N' } else { 'n' },
        0x32 => if shift { 'M' } else { 'm' },

        // US punctuation
        0x29 => if shift { '~' } else { '`' },
        0x0C => if shift { '_' } else { '-' },
        0x0D => if shift { '+' } else { '=' },
        0x1A => if shift { '{' } else { '[' },
        0x1B => if shift { '}' } else { ']' },
        0x27 => if shift { ':' } else { ';' },
        0x28 => if shift { '"' } else { '\'' },
        0x2B => if shift { '|' } else { '\\' },
        0x33 => if shift { '<' } else { ',' },
        0x34 => if shift { '>' } else { '.' },
        0x35 => if shift { '?' } else { '/' },

        // Space and Enter
        0x39 => ' ',
        0x1C => '\n',

        _ => return None,
    };

    Some(character)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct KeyModifiers {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub super_key: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyCode {
    Char(char),
    Enter,
    Backspace,
    Delete,
    Escape,
    Tab,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    F2,
    F5,
    Super,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyEvent {
    pub code: KeyCode,
    pub modifiers: KeyModifiers,
}

pub fn decode_scancode(
    scancode: u8,
    ext_prefix: &mut bool,
    modifiers: &mut KeyModifiers,
) -> Option<KeyEvent> {
    if scancode == 0xE0 {
        *ext_prefix = true;
        return None;
    }

    let is_ext = *ext_prefix;
    *ext_prefix = false;

    let released = (scancode & 0x80) != 0;
    let code = scancode & 0x7F;

    if is_ext {
        match code {
            0x1D => {
                modifiers.ctrl = !released;
                return None;
            }
            0x38 => {
                modifiers.alt = !released;
                return None;
            }
            0x5B | 0x5C => {
                modifiers.super_key = !released;
                if !released {
                    return Some(KeyEvent {
                        code: KeyCode::Super,
                        modifiers: *modifiers,
                    });
                }
                return None;
            }
            _ => {}
        }

        if released {
            return None;
        }

        let key = match code {
            0x48 => KeyCode::Up,
            0x50 => KeyCode::Down,
            0x4B => KeyCode::Left,
            0x4D => KeyCode::Right,
            0x47 => KeyCode::Home,
            0x4F => KeyCode::End,
            0x49 => KeyCode::PageUp,
            0x51 => KeyCode::PageDown,
            0x53 => KeyCode::Delete,
            0x1C => KeyCode::Enter,
            _ => return None,
        };

        return Some(KeyEvent {
            code: key,
            modifiers: *modifiers,
        });
    }

    match code {
        0x2A | 0x36 => {
            modifiers.shift = !released;
            return None;
        }
        0x1D => {
            modifiers.ctrl = !released;
            return None;
        }
        0x38 => {
            modifiers.alt = !released;
            return None;
        }
        _ => {}
    }

    if released {
        return None;
    }

    let key = match code {
        0x01 => KeyCode::Escape,
        0x0E => KeyCode::Backspace,
        0x0F => KeyCode::Tab,
        0x1C => KeyCode::Enter,
        0x3C => KeyCode::F2,
        0x3F => KeyCode::F5,
        0x48 => KeyCode::Up,
        0x50 => KeyCode::Down,
        0x4B => KeyCode::Left,
        0x4D => KeyCode::Right,
        0x47 => KeyCode::Home,
        0x4F => KeyCode::End,
        0x49 => KeyCode::PageUp,
        0x51 => KeyCode::PageDown,
        0x53 => KeyCode::Delete,
        _ => {
            let ch = scancode_to_char(code, modifiers.shift)?;
            KeyCode::Char(ch)
        }
    };

    Some(KeyEvent {
        code: key,
        modifiers: *modifiers,
    })
}
