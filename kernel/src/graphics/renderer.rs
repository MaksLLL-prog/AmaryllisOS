
use bootloader_api::info::PixelFormat;

pub type Color = (u8, u8, u8);

pub struct Renderer<'a> {
    buffer: &'a mut [u8],
    width: usize,
    height: usize,
    stride: usize,
    bpp: usize,
    format: PixelFormat,
}

impl<'a> Renderer<'a> {
    pub fn new(
        buffer: &'a mut [u8],
        width: usize,
        height: usize,
        stride: usize,
        bpp: usize,
        format: PixelFormat,
    ) -> Self {
        Self {
            buffer,
            width,
            height,
            stride,
            bpp,
            format,
        }
    }

    pub fn width(&self) -> usize {
        self.width
    }

    pub fn height(&self) -> usize {
        self.height
    }

    pub fn stride(&self) -> usize {
        self.stride
    }

    pub fn bpp(&self) -> usize {
        self.bpp
    }

    pub fn format(&self) -> PixelFormat {
        self.format
    }

    /// Blits a rectangle from a `u32` (0x00RRGGBB) source buffer matching display dimensions directly to hardware FB.
    pub fn blit_xrgb_region(
        &mut self,
        src: &[u32],
        src_stride: usize,
        rx: usize,
        ry: usize,
        rw: usize,
        rh: usize,
    ) {
        if self.bpp < 3 || rw == 0 || rh == 0 {
            return;
        }
        let start_x = rx.min(self.width);
        let start_y = ry.min(self.height);
        let end_x = rx.saturating_add(rw).min(self.width);
        let end_y = ry.saturating_add(rh).min(self.height);
        if start_x >= end_x || start_y >= end_y {
            return;
        }

        let bpp = self.bpp;
        let fb_stride = self.stride;

        match self.format {
            PixelFormat::Bgr => {
                if bpp == 4 {
                    let row_len = end_x - start_x;
                    // Fast path: full-width aligned contiguous block copy via copy_nonoverlapping
                    if start_x == 0
                        && end_x == self.width
                        && src_stride == fb_stride
                        && end_y.saturating_mul(fb_stride) <= src.len()
                        && (end_y.saturating_mul(fb_stride) * 4) <= self.buffer.len()
                    {
                        let byte_start = start_y * fb_stride * 4;
                        let total_px = (end_y - start_y) * fb_stride;
                        unsafe {
                            let src_ptr = src.as_ptr().add(start_y * src_stride) as *const u8;
                            let dst_ptr = self.buffer.as_mut_ptr().add(byte_start);
                            core::ptr::copy_nonoverlapping(src_ptr, dst_ptr, total_px * 4);
                        }
                    } else {
                        for py in start_y..end_y {
                            let src_row = py * src_stride + start_x;
                            let fb_row = py * fb_stride + start_x;
                            if src_row + row_len > src.len()
                                || (fb_row + row_len) * 4 > self.buffer.len()
                            {
                                break;
                            }
                            let dst_ptr = unsafe { self.buffer.as_mut_ptr().add(fb_row * 4) };
                            let src_ptr = unsafe { src.as_ptr().add(src_row) };
                            // Use u64 aligned block copy when both pointers are 8-byte aligned
                            if ((dst_ptr as usize) & 7) == 0 && ((src_ptr as usize) & 7) == 0 {
                                let pairs = row_len / 2;
                                let dst64 = dst_ptr as *mut u64;
                                let src64 = src_ptr as *const u64;
                                unsafe {
                                    for k in 0..pairs {
                                        *dst64.add(k) = *src64.add(k);
                                    }
                                    if (row_len & 1) != 0 {
                                        let tail = row_len - 1;
                                        *(dst_ptr as *mut u32).add(tail) = *src_ptr.add(tail);
                                    }
                                }
                            } else {
                                unsafe {
                                    core::ptr::copy_nonoverlapping(
                                        src_ptr as *const u8,
                                        dst_ptr,
                                        row_len * 4,
                                    );
                                }
                            }
                        }
                    }
                } else {
                    for py in start_y..end_y {
                        let src_row = py * src_stride;
                        let fb_row = py * fb_stride;
                        if src_row + end_x > src.len() {
                            break;
                        }
                        let mut off = (fb_row + start_x) * bpp;
                        for &px in &src[src_row + start_x..src_row + end_x] {
                            if off + bpp > self.buffer.len() {
                                break;
                            }
                            self.buffer[off] = (px & 0xFF) as u8;
                            self.buffer[off + 1] = ((px >> 8) & 0xFF) as u8;
                            self.buffer[off + 2] = ((px >> 16) & 0xFF) as u8;
                            off += bpp;
                        }
                    }
                }
            }
            PixelFormat::Rgb => {
                for py in start_y..end_y {
                    let src_row = py * src_stride;
                    let fb_row = py * fb_stride;
                    if src_row + end_x > src.len() {
                        break;
                    }
                    let mut off = (fb_row + start_x) * bpp;
                    for &px in &src[src_row + start_x..src_row + end_x] {
                        if off + bpp > self.buffer.len() {
                            break;
                        }
                        self.buffer[off] = ((px >> 16) & 0xFF) as u8;
                        self.buffer[off + 1] = ((px >> 8) & 0xFF) as u8;
                        self.buffer[off + 2] = (px & 0xFF) as u8;
                        if bpp >= 4 {
                            self.buffer[off + 3] = 255;
                        }
                        off += bpp;
                    }
                }
            }
            _ => {}
        }
    }

    pub fn draw_pixel(&mut self, x: usize, y: usize, color: Color) {
        if x >= self.width || y >= self.height || self.bpp < 3 {
            return;
        }

        let offset = match y
            .checked_mul(self.stride)
            .and_then(|v| v.checked_add(x))
            .and_then(|v| v.checked_mul(self.bpp))
        {
            Some(value) => value,
            None => return,
        };

        self.write_pixel(offset, color);
    }

    #[inline]
    fn write_pixel(&mut self, offset: usize, color: Color) {
        if offset + self.bpp > self.buffer.len() {
            return;
        }

        let (r, g, b) = color;

        match self.format {
            PixelFormat::Rgb => {
                self.buffer[offset] = r;
                self.buffer[offset + 1] = g;
                self.buffer[offset + 2] = b;
            }
            PixelFormat::Bgr => {
                self.buffer[offset] = b;
                self.buffer[offset + 1] = g;
                self.buffer[offset + 2] = r;
            }
            _ => return,
        }

        if self.bpp >= 4 {
            self.buffer[offset + 3] = 255;
        }
    }

    pub fn draw_rect(
        &mut self,
        x: usize,
        y: usize,
        width: usize,
        height: usize,
        color: Color,
    ) {
        if self.bpp < 3 || width == 0 || height == 0 {
            return;
        }

        if !matches!(self.format, PixelFormat::Rgb | PixelFormat::Bgr) {
            return;
        }

        let start_x = x.min(self.width);
        let start_y = y.min(self.height);

        let end_x = x.saturating_add(width).min(self.width);
        let end_y = y.saturating_add(height).min(self.height);

        if start_x >= end_x || start_y >= end_y {
            return;
        }

        let (r, g, b) = color;

        for py in start_y..end_y {
            let row_start = match py
                .checked_mul(self.stride)
                .and_then(|v| v.checked_mul(self.bpp))
            {
                Some(value) => value,
                None => continue,
            };

            let pixel_start = match start_x.checked_mul(self.bpp) {
                Some(value) => value,
                None => continue,
            };

            let mut offset = match row_start.checked_add(pixel_start) {
                Some(value) => value,
                None => continue,
            };

            for _ in start_x..end_x {
                if offset + self.bpp > self.buffer.len() {
                    break;
                }

                match self.format {
                    PixelFormat::Rgb => {
                        self.buffer[offset] = r;
                        self.buffer[offset + 1] = g;
                        self.buffer[offset + 2] = b;
                    }
                    PixelFormat::Bgr => {
                        self.buffer[offset] = b;
                        self.buffer[offset + 1] = g;
                        self.buffer[offset + 2] = r;
                    }
                    _ => {}
                }

                if self.bpp >= 4 {
                    self.buffer[offset + 3] = 255;
                }

                offset += self.bpp;
            }
        }
    }

    pub fn clear(&mut self, color: Color) {
        self.draw_rect(0, 0, self.width, self.height, color);
    }

    pub fn draw_char(
        &mut self,
        x: usize,
        y: usize,
        character: char,
        color: Color,
        scale: usize,
    ) {
        use font8x8::{BASIC_FONTS, UnicodeFonts};

        let scale = scale.max(1);

        let glyph = BASIC_FONTS
            .get(character)
            .or_else(|| BASIC_FONTS.get('?'));

        if let Some(glyph) = glyph {
            for (row, bits) in glyph.iter().enumerate() {
                let mut col = 0;

                while col < 8 {
                    if bits & (1 << col) != 0 {
                        let start = col;

                        while col < 8 && bits & (1 << col) != 0 {
                            col += 1;
                        }

                        self.draw_rect(
                            x + start * scale,
                            y + row * scale,
                            (col - start) * scale,
                            scale,
                            color,
                        );
                    } else {
                        col += 1;
                    }
                }
            }
        }
    }

    pub fn scroll_up(&mut self, pixels: usize, background: Color) {
        self.scroll_up_from(0, pixels, background);
    }

    pub fn scroll_up_from(
        &mut self,
        start_y: usize,
        pixels: usize,
        background: Color,
    ) {
        if self.bpp < 3 || start_y >= self.height || pixels == 0 {
            return;
        }

        let pixels = pixels.min(self.height - start_y);
        let row_bytes = match self.stride.checked_mul(self.bpp) {
            Some(value) if value > 0 => value,
            _ => return,
        };

        let rows_to_move = self.height - start_y - pixels;

        if rows_to_move > 0 {
            let destination = start_y * row_bytes;
            let source = (start_y + pixels) * row_bytes;
            let bytes = rows_to_move * row_bytes;

            if source + bytes <= self.buffer.len()
                && destination + bytes <= self.buffer.len()
            {
                self.buffer.copy_within(
                    source..source + bytes,
                    destination,
                );
            }
        }

        self.draw_rect(
            0,
            self.height - pixels,
            self.width,
            pixels,
            background,
        );
    }

    pub fn scroll_down_from(
        &mut self,
        start_y: usize,
        pixels: usize,
        background: Color,
    ) {
        if self.bpp < 3 || start_y >= self.height || pixels == 0 {
            return;
        }

        let pixels = pixels.min(self.height - start_y);
        let row_bytes = match self.stride.checked_mul(self.bpp) {
            Some(value) if value > 0 => value,
            _ => return,
        };

        let rows_to_move = self.height - start_y - pixels;

        if rows_to_move > 0 {
            let source = start_y * row_bytes;
            let destination = (start_y + pixels) * row_bytes;
            let bytes = rows_to_move * row_bytes;

            if source + bytes <= self.buffer.len()
                && destination + bytes <= self.buffer.len()
            {
                self.buffer.copy_within(
                    source..source + bytes,
                    destination,
                );
            }
        }

        self.draw_rect(
            0,
            start_y,
            self.width,
            pixels,
            background,
        );
    }
}
