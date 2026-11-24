//! AmaryllisOS Desktop compositor window & event API for Ring 3 ELF64 programs.

use crate::syscall::{
    syscall1, syscall2, syscall3, syscall4, SYS_BEEP, SYS_COMMIT_SHM_WINDOW,
    SYS_CREATE_SHM_WINDOW, SYS_CREATE_WINDOW, SYS_POLL_GUI_EVENT, SYS_SHM_UNMAP, SYS_WINDOW_BLIT,
};

/// Raw C ABI struct filled by `SYS_POLL_GUI_EVENT`.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct RawGuiEvent {
    pub kind: u32,
    pub p1: i32,
    pub p2: i32,
    pub p3: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct RawShmWindowInfo {
    pub win_id: u64,
    pub shm_id: u64,
    pub vaddr: u64,
    pub size_bytes: u64,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GuiEvent {
    None,
    CloseRequested,
    MouseDown { x: i32, y: i32, buttons: u32 },
    KeyChar(char),
}

fn decode_gui_event(res: i64, raw: RawGuiEvent) -> GuiEvent {
    if res <= 0 {
        return GuiEvent::None;
    }
    match raw.kind {
        1 => GuiEvent::CloseRequested,
        2 => GuiEvent::MouseDown {
            x: raw.p1,
            y: raw.p2,
            buttons: raw.p3,
        },
        3 => {
            let code = raw.p1 as u32;
            let ch = char::from_u32(code).unwrap_or('\0');
            GuiEvent::KeyChar(ch)
        }
        _ => GuiEvent::None,
    }
}

/// Handle to an external Ring 3 window managed by Amaryllis Desktop WM.
pub struct Window {
    id: u64,
    width: usize,
    height: usize,
}

impl Window {
    /// Creates a new Desktop window with client framebuffer size `(width, height)`.
    /// Maximum supported dimensions are `1280 x 800`.
    pub fn create(title: &str, width: usize, height: usize) -> Result<Self, i64> {
        let w = width.clamp(120, 1280);
        let h = height.clamp(80, 800);
        let packed = ((w as u64) << 16) | (h as u64);
        let res = unsafe {
            syscall3(
                SYS_CREATE_WINDOW,
                title.as_ptr() as u64,
                title.len() as u64,
                packed,
            )
        };
        if res >= 0 {
            Ok(Self {
                id: res as u64,
                width: w,
                height: h,
            })
        } else {
            Err(res)
        }
    }

    #[inline]
    pub const fn id(&self) -> u64 {
        self.id
    }

    #[inline]
    pub const fn width(&self) -> usize {
        self.width
    }

    #[inline]
    pub const fn height(&self) -> usize {
        self.height
    }

    /// Uploads a `0x00RRGGBB` pixel buffer (`pixels.len() >= width * height`) to the compositor.
    pub fn blit(&self, pixels: &[u32]) -> Result<(), i64> {
        let needed = self.width.saturating_mul(self.height);
        if pixels.len() < needed {
            return Err(-22);
        }
        let packed = ((self.width as u64) << 16) | (self.height as u64);
        let res = unsafe {
            syscall3(
                SYS_WINDOW_BLIT,
                self.id,
                pixels.as_ptr() as u64,
                packed,
            )
        };
        if res == 0 {
            Ok(())
        } else {
            Err(res)
        }
    }

    /// Polls for the next window event, sleeping cooperatively up to `wait_ms` if empty.
    pub fn poll_event(&self, wait_ms: u64) -> GuiEvent {
        let mut raw = RawGuiEvent::default();
        let res = unsafe {
            syscall3(
                SYS_POLL_GUI_EVENT,
                self.id,
                (&mut raw as *mut RawGuiEvent) as u64,
                wait_ms,
            )
        };
        decode_gui_event(res, raw)
    }
}

/// Zero-copy Shared-Memory Ring 3 window. Pixels written to `pixels_mut()` are composited
/// directly from shared physical memory frames on `commit()` with no kernel copy syscall.
pub struct ShmWindow {
    info: RawShmWindowInfo,
}

impl ShmWindow {
    pub fn create(title: &str, width: usize, height: usize) -> Result<Self, i64> {
        let w = width.clamp(160, 1280);
        let h = height.clamp(120, 800);
        let packed = ((w as u64) << 16) | (h as u64);
        let mut info = RawShmWindowInfo::default();
        let res = unsafe {
            syscall4(
                SYS_CREATE_SHM_WINDOW,
                title.as_ptr() as u64,
                title.len() as u64,
                packed,
                (&mut info as *mut RawShmWindowInfo) as u64,
            )
        };
        if res < 0 {
            Err(res)
        } else {
            Ok(Self { info })
        }
    }

    #[inline]
    pub const fn id(&self) -> u64 {
        self.info.win_id
    }

    #[inline]
    pub const fn shm_id(&self) -> u64 {
        self.info.shm_id
    }

    #[inline]
    pub const fn width(&self) -> usize {
        self.info.width as usize
    }

    #[inline]
    pub const fn height(&self) -> usize {
        self.info.height as usize
    }

    pub fn pixels_mut(&mut self) -> &mut [u32] {
        let count = (self.info.width as usize) * (self.info.height as usize);
        unsafe { core::slice::from_raw_parts_mut(self.info.vaddr as *mut u32, count) }
    }

    pub fn commit(&self) -> Result<(), i64> {
        let res = unsafe { syscall1(SYS_COMMIT_SHM_WINDOW, self.info.win_id) };
        if res == 0 {
            Ok(())
        } else {
            Err(res)
        }
    }

    pub fn poll_event(&self, wait_ms: u64) -> GuiEvent {
        let mut raw = RawGuiEvent::default();
        let res = unsafe {
            syscall3(
                SYS_POLL_GUI_EVENT,
                self.info.win_id,
                (&mut raw as *mut RawGuiEvent) as u64,
                wait_ms,
            )
        };
        decode_gui_event(res, raw)
    }
}

impl Drop for ShmWindow {
    fn drop(&mut self) {
        if self.info.vaddr != 0 {
            let _ = unsafe { syscall1(SYS_SHM_UNMAP, self.info.vaddr) };
            self.info.vaddr = 0;
        }
    }
}

/// Triggers an asynchronous non-blocking AC'97 synth tone (`60..12000` Hz, `20..3000` ms).
#[inline]
pub fn beep(freq_hz: u32, duration_ms: u32) {
    let _ = unsafe { syscall2(SYS_BEEP, freq_hz as u64, duration_ms as u64) };
}

/// Encodes RGB color into `0x00RRGGBB` XRGB pixel format expected by `Window::blit`.
#[inline]
pub const fn rgb(r: u8, g: u8, b: u8) -> u32 {
    ((r as u32) << 16) | ((g as u32) << 8) | (b as u32)
}

pub fn clear(pixels: &mut [u32], color: u32) {
    pixels.fill(color);
}

pub fn fill_rect(
    pixels: &mut [u32],
    stride: usize,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    color: u32,
) {
    if w <= 0 || h <= 0 || stride == 0 {
        return;
    }
    let buf_h = (pixels.len() / stride) as i32;
    let x0 = x.max(0);
    let y0 = y.max(0);
    let x1 = (x + w).min(stride as i32);
    let y1 = (y + h).min(buf_h);
    if x0 >= x1 || y0 >= y1 {
        return;
    }
    for py in y0..y1 {
        let row = (py as usize) * stride;
        for px in x0..x1 {
            pixels[row + px as usize] = color;
        }
    }
}

pub fn draw_circle_filled(
    pixels: &mut [u32],
    stride: usize,
    cx: i32,
    cy: i32,
    radius: i32,
    color: u32,
) {
    if radius <= 0 || stride == 0 {
        return;
    }
    let buf_h = (pixels.len() / stride) as i32;
    let r2 = radius * radius;
    let y0 = (cy - radius).max(0);
    let y1 = (cy + radius).min(buf_h - 1);
    let x0 = (cx - radius).max(0);
    let x1 = (cx + radius).min(stride as i32 - 1);
    for py in y0..=y1 {
        let dy = py - cy;
        let row = (py as usize) * stride;
        for px in x0..=x1 {
            let dx = px - cx;
            if dx * dx + dy * dy <= r2 {
                pixels[row + px as usize] = color;
            }
        }
    }
}
