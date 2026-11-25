use core::arch::asm;
use core::fmt::{self, Write};
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use font8x8::UnicodeFonts;

const COM1: u16 = 0x3F8;

static SERIAL_READY: AtomicBool = AtomicBool::new(false);
static SERIAL_LOCK: AtomicBool = AtomicBool::new(false);

static FB_PTR: AtomicUsize = AtomicUsize::new(0);
static FB_BYTE_LEN: AtomicUsize = AtomicUsize::new(0);
static FB_WIDTH: AtomicUsize = AtomicUsize::new(0);
static FB_HEIGHT: AtomicUsize = AtomicUsize::new(0);
static FB_STRIDE: AtomicUsize = AtomicUsize::new(0);
static FB_BPP: AtomicUsize = AtomicUsize::new(0);
static FB_IS_BGR: AtomicBool = AtomicBool::new(true);

#[inline(always)]
unsafe fn outb(port: u16, val: u8) {
    unsafe {
        asm!(
            "out dx, al",
            in("dx") port,
            in("al") val,
            options(nomem, nostack, preserves_flags)
        );
    }
}

#[inline(always)]
unsafe fn inb(port: u16) -> u8 {
    let val: u8;
    unsafe {
        asm!(
            "in al, dx",
            in("dx") port,
            out("al") val,
            options(nomem, nostack, preserves_flags)
        );
    }
    val
}

pub fn init() {
    unsafe {
        outb(COM1 + 1, 0x00); // Disable all interrupts
        outb(COM1 + 3, 0x80); // Enable DLAB
        outb(COM1 + 0, 0x01); // 115200 baud (divisor = 1)
        outb(COM1 + 1, 0x00);
        outb(COM1 + 3, 0x03); // 8N1, DLAB off
        outb(COM1 + 2, 0xC7); // Enable FIFO, clear, 14-byte threshold
        outb(COM1 + 4, 0x0B); // IRQs enabled, RTS/DSR set
    }
    SERIAL_READY.store(true, Ordering::Release);
}

pub fn register_emergency_fb(
    ptr: *mut u8,
    byte_len: usize,
    width: usize,
    height: usize,
    stride: usize,
    bpp: usize,
    is_bgr: bool,
) {
    FB_BYTE_LEN.store(byte_len, Ordering::Release);
    FB_WIDTH.store(width, Ordering::Release);
    FB_HEIGHT.store(height, Ordering::Release);
    FB_STRIDE.store(stride, Ordering::Release);
    FB_BPP.store(bpp, Ordering::Release);
    FB_IS_BGR.store(is_bgr, Ordering::Release);
    FB_PTR.store(ptr as usize, Ordering::Release);
}

#[inline]
fn write_byte_raw(b: u8) {
    if !SERIAL_READY.load(Ordering::Relaxed) {
        init();
    }
    // Bounded wait on LSR THRE bit (bit 5) so missing UART hardware never hangs.
    for _ in 0..4096 {
        let lsr = unsafe { inb(COM1 + 5) };
        if lsr == 0xFF {
            return;
        }
        if (lsr & 0x20) != 0 {
            break;
        }
        core::hint::spin_loop();
    }
    unsafe {
        outb(COM1, b);
    }
}

fn acquire_serial_lock_bounded() -> bool {
    for _ in 0..16_384 {
        if SERIAL_LOCK
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
        {
            return true;
        }
        core::hint::spin_loop();
    }
    false
}

pub struct SerialWriter;

impl Write for SerialWriter {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for b in s.bytes() {
            if b == b'\n' {
                write_byte_raw(b'\r');
            }
            write_byte_raw(b);
        }
        Ok(())
    }
}

pub fn write_str(s: &str) {
    let locked = acquire_serial_lock_bounded();
    let _ = SerialWriter.write_str(s);
    if locked {
        SERIAL_LOCK.store(false, Ordering::Release);
    }
}

pub fn write_fmt(args: fmt::Arguments<'_>) {
    let locked = acquire_serial_lock_bounded();
    let _ = SerialWriter.write_fmt(args);
    if locked {
        SERIAL_LOCK.store(false, Ordering::Release);
    }
}

pub fn log(tag: &str, msg: &str) {
    write_fmt(format_args!("[{}] {}\n", tag, msg));
}

pub fn log_fmt(tag: &str, args: fmt::Arguments<'_>) {
    write_fmt(format_args!("[{}] {}\n", tag, args));
}

#[inline]
unsafe fn fb_put_pixel(x: usize, y: usize, r: u8, g: u8, b: u8) {
    let ptr_addr = FB_PTR.load(Ordering::Acquire);
    if ptr_addr == 0 {
        return;
    }
    let w = FB_WIDTH.load(Ordering::Relaxed);
    let h = FB_HEIGHT.load(Ordering::Relaxed);
    let stride = FB_STRIDE.load(Ordering::Relaxed);
    let bpp = FB_BPP.load(Ordering::Relaxed);
    let byte_len = FB_BYTE_LEN.load(Ordering::Relaxed);
    if x >= w || y >= h || bpp < 3 {
        return;
    }
    let off = (y * stride + x) * bpp;
    if off + bpp > byte_len {
        return;
    }
    let dst = (ptr_addr as *mut u8).add(off);
    if FB_IS_BGR.load(Ordering::Relaxed) {
        dst.write_volatile(b);
        dst.add(1).write_volatile(g);
        dst.add(2).write_volatile(r);
    } else {
        dst.write_volatile(r);
        dst.add(1).write_volatile(g);
        dst.add(2).write_volatile(b);
    }
    if bpp >= 4 {
        dst.add(3).write_volatile(0xFF);
    }
}

fn fb_draw_text_2x(start_x: usize, start_y: usize, text: &str, r: u8, g: u8, b: u8) {
    let mut cx = start_x;
    for ch in text.chars() {
        let glyph = font8x8::BASIC_FONTS
            .get(ch)
            .or_else(|| font8x8::BASIC_FONTS.get('?'))
            .unwrap_or([0; 8]);
        for (row, &bits) in glyph.iter().enumerate() {
            for col in 0..8 {
                if (bits & (1 << col)) != 0 {
                    let px = cx + col * 2;
                    let py = start_y + row * 2;
                    unsafe {
                        fb_put_pixel(px, py, r, g, b);
                        fb_put_pixel(px + 1, py, r, g, b);
                        fb_put_pixel(px, py + 1, r, g, b);
                        fb_put_pixel(px + 1, py + 1, r, g, b);
                    }
                }
            }
        }
        cx += 18;
    }
}

/// Lock-free, zero-allocation crash banner written both to COM1 serial and
/// directly to the top of the linear framebuffer when registered.
pub fn panic_banner(title: &str, line1: &str, line2: &str) {
    write_fmt(format_args!("\n=== {} ===\n{}\n{}\n", title, line1, line2));

    let ptr_addr = FB_PTR.load(Ordering::Acquire);
    if ptr_addr == 0 {
        return;
    }
    let w = FB_WIDTH.load(Ordering::Relaxed);
    let h = FB_HEIGHT.load(Ordering::Relaxed);
    let banner_h = h.min(112);

    for y in 0..banner_h {
        for x in 0..w {
            let (r, g, b) = if y >= banner_h.saturating_sub(3) {
                (235, 90, 90)
            } else {
                (110, 16, 24)
            };
            unsafe {
                fb_put_pixel(x, y, r, g, b);
            }
        }
    }

    fb_draw_text_2x(20, 14, title, 255, 220, 120);
    fb_draw_text_2x(20, 44, line1, 245, 245, 245);
    fb_draw_text_2x(20, 74, line2, 220, 220, 230);
}

pub struct FixedBuf<const N: usize> {
    buf: [u8; N],
    len: usize,
}

impl<const N: usize> FixedBuf<N> {
    pub const fn new() -> Self {
        Self {
            buf: [0; N],
            len: 0,
        }
    }

    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.buf[..self.len]).unwrap_or("")
    }
}

impl<const N: usize> Write for FixedBuf<N> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let bytes = s.as_bytes();
        let avail = N.saturating_sub(self.len);
        let n = bytes.len().min(avail);
        self.buf[self.len..self.len + n].copy_from_slice(&bytes[..n]);
        self.len += n;
        Ok(())
    }
}
