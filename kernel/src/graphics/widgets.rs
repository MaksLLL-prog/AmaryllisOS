use super::font::{
    lookup_glyph, lookup_glyph_bold, ui_glyph_advance, with_ui_glyph, RasterizedGlyph,
    GLYPH_HEIGHT, GLYPH_WIDTH,
};
use super::font_mono::{
    lookup_mono_glyph, with_mono_glyph, MONO_GLYPH_HEIGHT, MONO_GLYPH_WIDTH,
};
use super::icons::{lucide_bitmap, LucideIcon, LUCIDE_ICON_SIZE};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    #[inline]
    pub const fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Self { x, y, w, h }
    }

    #[inline]
    pub const fn right(self) -> i32 {
        self.x + self.w
    }

    #[inline]
    pub const fn bottom(self) -> i32 {
        self.y + self.h
    }

    #[inline]
    pub const fn is_empty(self) -> bool {
        self.w <= 0 || self.h <= 0
    }

    #[inline]
    pub const fn contains(self, px: i32, py: i32) -> bool {
        px >= self.x && py >= self.y && px < self.x + self.w && py < self.y + self.h
    }

    #[inline]
    pub fn expanded(self, pad: i32) -> Self {
        Self::new(self.x - pad, self.y - pad, self.w + pad * 2, self.h + pad * 2)
    }

    #[inline]
    pub fn intersect(self, other: Self) -> Self {
        let x1 = self.x.max(other.x);
        let y1 = self.y.max(other.y);
        let x2 = self.right().min(other.right());
        let y2 = self.bottom().min(other.bottom());
        if x2 <= x1 || y2 <= y1 {
            Self::new(0, 0, 0, 0)
        } else {
            Self::new(x1, y1, x2 - x1, y2 - y1)
        }
    }

    #[inline]
    pub fn union(self, other: Self) -> Self {
        if self.is_empty() {
            return other;
        }
        if other.is_empty() {
            return self;
        }
        let x1 = self.x.min(other.x);
        let y1 = self.y.min(other.y);
        let x2 = self.right().max(other.right());
        let y2 = self.bottom().max(other.bottom());
        Self::new(x1, y1, x2 - x1, y2 - y1)
    }

    #[inline]
    pub fn clamp_to_screen(self, screen_w: usize, screen_h: usize) -> Self {
        self.intersect(Self::new(0, 0, screen_w as i32, screen_h as i32))
    }
}

/// Fast integer square root for SDF Q8 subpixel distance calculations.
#[inline(always)]
fn isqrt_u32(n: u32) -> u32 {
    if n < 2 {
        return n;
    }
    let mut x0 = 1u32 << ((32 - n.leading_zeros() + 1) / 2);
    loop {
        let x1 = (x0 + n / x0) >> 1;
        if x1 >= x0 {
            return x0;
        }
        x0 = x1;
    }
}

/// Computes SDF coverage (0..=255) for a corner delta `(dx_2, dy_2)` where each pixel step = 2 (pixel center offset = +1).
/// `rad_q8` is `(radius << 8)`. Transition width is ~1.25px (`[-160..+160]` in Q8).
#[inline(always)]
fn sdf_corner_fill_cov(dx_q8: i32, dy_q8: i32, rad_q8: i32) -> u8 {
    let dist2 = (dx_q8 as u64) * (dx_q8 as u64) + (dy_q8 as u64) * (dy_q8 as u64);
    let dist_q8 = isqrt_u32(dist2.min(u32::MAX as u64) as u32) as i32;
    let sd_q8: i32 = rad_q8 - dist_q8;
    if sd_q8 >= 160 {
        255
    } else if sd_q8 <= -160 {
        0
    } else {
        (((sd_q8 + 160) * 255) / 320).clamp(0, 255) as u8
    }
}

pub struct Canvas<'a> {
    pub buf: &'a mut [u32],
    pub width: usize,
    pub height: usize,
    pub clip: Rect,
}

impl<'a> Canvas<'a> {
    #[inline]
    pub fn new(buf: &'a mut [u32], width: usize, height: usize, clip: Rect) -> Self {
        Self {
            buf,
            width,
            height,
            clip: clip.clamp_to_screen(width, height),
        }
    }

    #[inline(always)]
    pub fn blend_px(&mut self, x: i32, y: i32, color: u32, alpha: u8) {
        if !self.clip.contains(x, y) || alpha == 0 {
            return;
        }
        let idx = (y as usize) * self.width + (x as usize);
        if alpha == 255 {
            self.buf[idx] = color | 0xFF00_0000;
        } else {
            self.buf[idx] = blend_u32(self.buf[idx], color, alpha);
        }
    }

    #[inline(always)]
    pub fn put_px(&mut self, x: i32, y: i32, color: u32) {
        if !self.clip.contains(x, y) {
            return;
        }
        let idx = (y as usize) * self.width + (x as usize);
        self.buf[idx] = color | 0xFF00_0000;
    }

    /// Blits a pre-rendered rectangular surface (`u32` XRGB) into `dst_rect` clipped cleanly to `self.clip`.
    pub fn blit_surface(&mut self, dst_rect: Rect, src: &[u32], src_stride: usize) {
        let vis = dst_rect.intersect(self.clip);
        if vis.is_empty() || src_stride == 0 {
            return;
        }
        let off_x = (vis.x - dst_rect.x) as usize;
        let off_y = (vis.y - dst_rect.y) as usize;
        let row_w = vis.w as usize;

        for ry in 0..(vis.h as usize) {
            let sy = off_y + ry;
            let dy = (vis.y as usize) + ry;
            let src_start = sy * src_stride + off_x;
            let dst_start = dy * self.width + (vis.x as usize);
            if src_start + row_w > src.len() || dst_start + row_w > self.buf.len() {
                break;
            }
            self.buf[dst_start..dst_start + row_w]
                .copy_from_slice(&src[src_start..src_start + row_w]);
        }
    }

    /// Alpha-blends a packed `0xAARRGGBB` sprite (Colloid icon) at `(x, y)` clipped to `self.clip`.
    pub fn blit_argb_icon(&mut self, x: i32, y: i32, w: usize, h: usize, argb: &[u32]) {
        let dst = Rect::new(x, y, w as i32, h as i32);
        let vis = dst.intersect(self.clip);
        if vis.is_empty() || w == 0 || h == 0 || argb.len() < w * h {
            return;
        }
        for py in vis.y..vis.bottom() {
            let sy = (py - y) as usize;
            let row_off = (py as usize) * self.width;
            let srow = sy * w;
            for px in vis.x..vis.right() {
                let sx = (px - x) as usize;
                let src = argb[srow + sx];
                let a = ((src >> 24) & 0xFF) as u8;
                if a == 0 {
                    continue;
                }
                let idx = row_off + (px as usize);
                if a == 255 {
                    self.buf[idx] = src | 0xFF00_0000;
                } else {
                    self.buf[idx] = blend_u32(self.buf[idx], src, a);
                }
            }
        }
    }

    /// Bilinear-filtered scaling of packed `0xAARRGGBB` icon sprites (for dynamic Dock sizes & magnification).
    pub fn blit_argb_icon_scaled(
        &mut self,
        x: i32,
        y: i32,
        dst_w: usize,
        dst_h: usize,
        src_w: usize,
        src_h: usize,
        argb: &[u32],
    ) {
        if dst_w == src_w && dst_h == src_h {
            self.blit_argb_icon(x, y, src_w, src_h, argb);
            return;
        }
        let dst = Rect::new(x, y, dst_w as i32, dst_h as i32);
        let vis = dst.intersect(self.clip);
        if vis.is_empty() || dst_w == 0 || dst_h == 0 || src_w == 0 || src_h == 0 || argb.len() < src_w * src_h {
            return;
        }
        let step_x_q16 = ((src_w as i64) << 16) / (dst_w as i64);
        let step_y_q16 = ((src_h as i64) << 16) / (dst_h as i64);
        let max_sx = (src_w - 1) as i64;
        let max_sy = (src_h - 1) as i64;

        for py in vis.y..vis.bottom() {
            let dy = (py - y) as i64;
            let sy_q16 = ((dy * step_y_q16) - 32768 + (step_y_q16 >> 1)).clamp(0, max_sy << 16);
            let sy0 = (sy_q16 >> 16) as usize;
            let sy1 = (sy0 + 1).min(src_h - 1);
            let fy = ((sy_q16 & 0xFFFF) >> 8) as u32;
            let row_off = (py as usize) * self.width;

            for px in vis.x..vis.right() {
                let dx = (px - x) as i64;
                let sx_q16 = ((dx * step_x_q16) - 32768 + (step_x_q16 >> 1)).clamp(0, max_sx << 16);
                let sx0 = (sx_q16 >> 16) as usize;
                let sx1 = (sx0 + 1).min(src_w - 1);
                let fx = ((sx_q16 & 0xFFFF) >> 8) as u32;

                let p00 = argb[sy0 * src_w + sx0];
                let p10 = argb[sy0 * src_w + sx1];
                let p01 = argb[sy1 * src_w + sx0];
                let p11 = argb[sy1 * src_w + sx1];

                let w00 = (256 - fx) * (256 - fy);
                let w10 = fx * (256 - fy);
                let w01 = (256 - fx) * fy;
                let w11 = fx * fy;

                let a = ((((p00 >> 24) & 0xFF) * w00
                    + ((p10 >> 24) & 0xFF) * w10
                    + ((p01 >> 24) & 0xFF) * w01
                    + ((p11 >> 24) & 0xFF) * w11)
                    >> 16)
                    .min(255) as u8;
                if a == 0 {
                    continue;
                }
                let r = ((((p00 >> 16) & 0xFF) * w00
                    + ((p10 >> 16) & 0xFF) * w10
                    + ((p01 >> 16) & 0xFF) * w01
                    + ((p11 >> 16) & 0xFF) * w11)
                    >> 16)
                    .min(255);
                let g = ((((p00 >> 8) & 0xFF) * w00
                    + ((p10 >> 8) & 0xFF) * w10
                    + ((p01 >> 8) & 0xFF) * w01
                    + ((p11 >> 8) & 0xFF) * w11)
                    >> 16)
                    .min(255);
                let b = (((p00 & 0xFF) * w00
                    + (p10 & 0xFF) * w10
                    + (p01 & 0xFF) * w01
                    + (p11 & 0xFF) * w11)
                    >> 16)
                    .min(255);
                let src = 0xFF00_0000 | (r << 16) | (g << 8) | b;
                let idx = row_off + (px as usize);
                if a == 255 {
                    self.buf[idx] = src;
                } else {
                    self.buf[idx] = blend_u32(self.buf[idx], src, a);
                }
            }
        }
    }

    /// Renders a 16x16 Lucide alpha mask icon tinted with `color`.
    pub fn draw_lucide_icon(&mut self, x: i32, y: i32, icon: LucideIcon, color: u32) {
        let bmp = lucide_bitmap(icon);
        let dst = Rect::new(x, y, LUCIDE_ICON_SIZE as i32, LUCIDE_ICON_SIZE as i32);
        let vis = dst.intersect(self.clip);
        if vis.is_empty() {
            return;
        }
        let solid = color | 0xFF00_0000;
        let c_rb = color & 0x00FF_00FF;
        let c_g = color & 0x0000_FF00;

        for py in vis.y..vis.bottom() {
            let sy = (py - y) as usize;
            let row_off = (py as usize) * self.width;
            let srow = sy * LUCIDE_ICON_SIZE;
            for px in vis.x..vis.right() {
                let sx = (px - x) as usize;
                let a = bmp[srow + sx];
                if a == 0 {
                    continue;
                }
                let idx = row_off + (px as usize);
                if a == 255 {
                    self.buf[idx] = solid;
                } else {
                    let a32 = a as u32;
                    let inv = 256 - a32;
                    let dst_px = self.buf[idx];
                    let rb = ((c_rb * a32 + (dst_px & 0x00FF_00FF) * inv) >> 8) & 0x00FF_00FF;
                    let g = ((c_g * a32 + (dst_px & 0x0000_FF00) * inv) >> 8) & 0x0000_FF00;
                    self.buf[idx] = 0xFF00_0000 | rb | g;
                }
            }
        }
    }

    /// Blits directly from an SHM region's HHDM physical frames into `dst_rect` with zero extra allocations.
    pub fn blit_shm_surface(
        &mut self,
        dst_rect: Rect,
        shm_id: u64,
        mapping: &crate::memory::vmm::PhysicalMemoryMapping,
        src_stride: usize,
    ) {
        let vis = dst_rect.intersect(self.clip);
        if vis.is_empty() || src_stride == 0 {
            return;
        }
        let off_x = (vis.x - dst_rect.x) as usize;
        let off_y = (vis.y - dst_rect.y) as usize;
        let row_w = vis.w as usize;

        crate::userland::ipc::blit_shm_rect(
            shm_id,
            mapping,
            src_stride,
            off_x,
            off_y,
            row_w,
            vis.h as usize,
            &mut self.buf,
            self.width,
            vis.x as usize,
            vis.y as usize,
        );
    }

    pub fn fill_rect(&mut self, rect: Rect, color: u32) {
        let r = rect.intersect(self.clip);
        if r.is_empty() {
            return;
        }
        let c = color | 0xFF00_0000;
        let x0 = r.x as usize;
        let x1 = r.right() as usize;
        for py in (r.y as usize)..(r.bottom() as usize) {
            let row = py * self.width;
            self.buf[row + x0..row + x1].fill(c);
        }
    }

    pub fn fill_rect_alpha(&mut self, rect: Rect, color: u32, alpha: u8) {
        if alpha == 0 {
            return;
        }
        if alpha == 255 {
            self.fill_rect(rect, color);
            return;
        }
        let r = rect.intersect(self.clip);
        if r.is_empty() {
            return;
        }
        let x0 = r.x as usize;
        let x1 = r.right() as usize;
        let a = alpha as u32;
        let inv = 256 - a;
        let src_rb = (color & 0x00FF_00FF) * a;
        let src_g = (color & 0x0000_FF00) * a;
        for py in (r.y as usize)..(r.bottom() as usize) {
            let row = py * self.width;
            for px in &mut self.buf[row + x0..row + x1] {
                let dst = *px;
                let rb = ((src_rb + (dst & 0x00FF_00FF) * inv) >> 8) & 0x00FF_00FF;
                let g = ((src_g + (dst & 0x0000_FF00) * inv) >> 8) & 0x0000_FF00;
                *px = 0xFF00_0000 | rb | g;
            }
        }
    }

    pub fn fill_gradient_v(&mut self, rect: Rect, top_color: u32, bottom_color: u32) {
        let r = rect.intersect(self.clip);
        if r.is_empty() {
            return;
        }
        let total_h = rect.h.max(1);
        let x0 = r.x as usize;
        let x1 = r.right() as usize;
        for py in r.y..r.bottom() {
            let t = (((py - rect.y) * 255) / total_h).clamp(0, 255) as u8;
            let c = blend_u32(top_color, bottom_color, t);
            let row = (py as usize) * self.width;
            self.buf[row + x0..row + x1].fill(c);
        }
    }

    pub fn fill_rounded_rect(&mut self, rect: Rect, radius: i32, color: u32) {
        self.fill_rounded_rect_alpha(rect, radius, color, 255);
    }

    /// Fills a rectangle with SDF-antialiased top corners only (titlebar geometry).
    pub fn fill_top_rounded_rect(&mut self, rect: Rect, radius: i32, color: u32) {
        let r = rect.intersect(self.clip);
        if r.is_empty() {
            return;
        }
        let rad = radius.max(0).min(rect.w / 2).min(rect.h);
        if rad == 0 {
            self.fill_rect(r, color);
            return;
        }

        let solid = color | 0xFF00_0000;
        let rad_q8 = rad << 8;

        for py in r.y..r.bottom() {
            let ly = py - rect.y;
            let row_off = (py as usize) * self.width;
            if ly < rad {
                // Distance from pixel center (ly + 0.5) to arc center row `rad` in Q8
                let dy_q8 = ((rad * 2) - (ly * 2 + 1)) * 128;
                for px in r.x..r.right() {
                    let lx = px - rect.x;
                    let dx_q8 = if lx < rad {
                        ((rad * 2) - (lx * 2 + 1)) * 128
                    } else if lx >= rect.w - rad {
                        ((lx * 2 + 1) - (rect.w - rad) * 2) * 128
                    } else {
                        0
                    };
                    if dx_q8 == 0 {
                        self.buf[row_off + px as usize] = solid;
                    } else {
                        let cov = sdf_corner_fill_cov(dx_q8, dy_q8, rad_q8);
                        if cov == 255 {
                            self.buf[row_off + px as usize] = solid;
                        } else if cov > 0 {
                            let idx = row_off + px as usize;
                            self.buf[idx] = blend_u32(self.buf[idx], color, cov);
                        }
                    }
                }
            } else {
                let x0 = r.x as usize;
                let x1 = r.right() as usize;
                self.buf[row_off + x0..row_off + x1].fill(solid);
            }
        }
    }

    /// SDF Anti-Aliased rounded rectangle fill with uniform alpha.
    pub fn fill_rounded_rect_alpha(&mut self, rect: Rect, radius: i32, color: u32, alpha: u8) {
        if alpha == 0 {
            return;
        }
        let r = rect.intersect(self.clip);
        if r.is_empty() {
            return;
        }
        let rad = radius.max(0).min(rect.w / 2).min(rect.h / 2);
        if rad == 0 {
            self.fill_rect_alpha(r, color, alpha);
            return;
        }

        let solid = color | 0xFF00_0000;
        let rad_q8 = rad << 8;

        let a_u32 = alpha as u32;
        let inv_u32 = 256 - a_u32;
        let src_rb = (color & 0x00FF_00FF) * a_u32;
        let src_g = (color & 0x0000_FF00) * a_u32;

        for py in r.y..r.bottom() {
            let ly = py - rect.y;
            let dy_q8 = if ly < rad {
                Some(((rad * 2) - (ly * 2 + 1)) * 128)
            } else if ly >= rect.h - rad {
                Some(((ly * 2 + 1) - (rect.h - rad) * 2) * 128)
            } else {
                None
            };

            let row_off = (py as usize) * self.width;

            if let Some(dy_q8_val) = dy_q8 {
                for px in r.x..r.right() {
                    let lx = px - rect.x;
                    let dx_q8 = if lx < rad {
                        ((rad * 2) - (lx * 2 + 1)) * 128
                    } else if lx >= rect.w - rad {
                        ((lx * 2 + 1) - (rect.w - rad) * 2) * 128
                    } else {
                        0
                    };
                    if dx_q8 == 0 {
                        let idx = row_off + px as usize;
                        if alpha == 255 {
                            self.buf[idx] = solid;
                        } else {
                            let dst = self.buf[idx];
                            let rb = ((src_rb + (dst & 0x00FF_00FF) * inv_u32) >> 8) & 0x00FF_00FF;
                            let g = ((src_g + (dst & 0x0000_FF00) * inv_u32) >> 8) & 0x0000_FF00;
                            self.buf[idx] = 0xFF00_0000 | rb | g;
                        }
                    } else {
                        let cov = sdf_corner_fill_cov(dx_q8, dy_q8_val, rad_q8);
                        if cov == 255 {
                            let idx = row_off + px as usize;
                            if alpha == 255 {
                                self.buf[idx] = solid;
                            } else {
                                let dst = self.buf[idx];
                                let rb = ((src_rb + (dst & 0x00FF_00FF) * inv_u32) >> 8) & 0x00FF_00FF;
                                let g = ((src_g + (dst & 0x0000_FF00) * inv_u32) >> 8) & 0x0000_FF00;
                                self.buf[idx] = 0xFF00_0000 | rb | g;
                            }
                        } else if cov > 0 {
                            let eff_a = (((cov as u32) * a_u32) >> 8) as u8;
                            if eff_a > 0 {
                                let idx = row_off + px as usize;
                                self.buf[idx] = blend_u32(self.buf[idx], color, eff_a);
                            }
                        }
                    }
                }
            } else {
                let x0 = r.x as usize;
                let x1 = r.right() as usize;
                if alpha == 255 {
                    self.buf[row_off + x0..row_off + x1].fill(solid);
                } else {
                    for px in &mut self.buf[row_off + x0..row_off + x1] {
                        let dst = *px;
                        let rb = ((src_rb + (dst & 0x00FF_00FF) * inv_u32) >> 8) & 0x00FF_00FF;
                        let g = ((src_g + (dst & 0x0000_FF00) * inv_u32) >> 8) & 0x0000_FF00;
                        *px = 0xFF00_0000 | rb | g;
                    }
                }
            }
        }
    }

    /// SDF Anti-Aliased 1px rounded rectangle stroke.
    pub fn stroke_rounded_rect(&mut self, rect: Rect, radius: i32, color: u32, alpha: u8) {
        if alpha == 0 || rect.w <= 2 || rect.h <= 2 {
            return;
        }
        if rect.intersect(self.clip).is_empty() {
            return;
        }
        let rad = radius.max(0).min(rect.w / 2).min(rect.h / 2);

        let x0 = (rect.x + rad).max(self.clip.x);
        let x1 = (rect.right() - rad).min(self.clip.right());
        if x1 > x0 {
            if rect.y >= self.clip.y && rect.y < self.clip.bottom() {
                for px in x0..x1 {
                    self.blend_px(px, rect.y, color, alpha);
                }
            }
            let by = rect.bottom() - 1;
            if by >= self.clip.y && by < self.clip.bottom() {
                for px in x0..x1 {
                    self.blend_px(px, by, color, alpha);
                }
            }
        }
        let y0 = (rect.y + rad).max(self.clip.y);
        let y1 = (rect.bottom() - rad).min(self.clip.bottom());
        if y1 > y0 {
            if rect.x >= self.clip.x && rect.x < self.clip.right() {
                for py in y0..y1 {
                    self.blend_px(rect.x, py, color, alpha);
                }
            }
            let rx = rect.right() - 1;
            if rx >= self.clip.x && rx < self.clip.right() {
                for py in y0..y1 {
                    self.blend_px(rx, py, color, alpha);
                }
            }
        }

        if rad == 0 {
            return;
        }

        // Corner stroke centered at radius (rad - 0.5) in Q8 with smooth ~1px SDF falloff
        let ring_r_q8 = (rad << 8) - 128;
        let half_band_q8: i32 = 210;

        for ly in 0..rad {
            let dy_q8 = ((rad * 2) - (ly * 2 + 1)) * 128;
            for lx in 0..rad {
                let dx_q8 = ((rad * 2) - (lx * 2 + 1)) * 128;
                let dist2 = (dx_q8 as u64) * (dx_q8 as u64) + (dy_q8 as u64) * (dy_q8 as u64);
                let dist_q8 = isqrt_u32(dist2.min(u32::MAX as u64) as u32) as i32;
                let diff: i32 = dist_q8 - ring_r_q8;
                let abs_d = diff.abs();
                if abs_d < half_band_q8 {
                    let cov = (((half_band_q8 - abs_d) * 255) / half_band_q8).clamp(0, 255) as u32;
                    let eff_a = ((cov * (alpha as u32)) >> 8) as u8;
                    if eff_a > 0 {
                        self.blend_px(rect.x + lx, rect.y + ly, color, eff_a);
                        self.blend_px(rect.right() - 1 - lx, rect.y + ly, color, eff_a);
                        self.blend_px(rect.x + lx, rect.bottom() - 1 - ly, color, eff_a);
                        self.blend_px(rect.right() - 1 - lx, rect.bottom() - 1 - ly, color, eff_a);
                    }
                }
            }
        }
    }

    /// High-speed translucent frosted glass surface with thin 1px specular border-highlight.
    pub fn draw_glass_panel(
        &mut self,
        rect: Rect,
        radius: i32,
        tint: u32,
        tint_alpha: u8,
        border_color: u32,
    ) {
        if rect.intersect(self.clip).is_empty() {
            return;
        }
        self.fill_rounded_rect_alpha(rect, radius, tint, tint_alpha);

        let rad = radius.max(0).min(rect.w / 2).min(rect.h / 2);
        let sheen_y = rect.y + 1;
        if sheen_y >= self.clip.y && sheen_y < self.clip.bottom() && rect.w > rad * 2 + 4 {
            let sx0 = (rect.x + rad.max(1)).max(self.clip.x) as usize;
            let sx1 = (rect.right() - rad.max(1)).min(self.clip.right()) as usize;
            if sx1 > sx0 {
                let row = (sheen_y as usize) * self.width;
                for px in &mut self.buf[row + sx0..row + sx1] {
                    *px = blend_u32(*px, 0xFFFF_FFFF, 34);
                }
            }
        }

        if rect.w > 4 && rect.h > 4 {
            self.stroke_rounded_rect(
                Rect::new(rect.x + 1, rect.y + 1, rect.w - 2, rect.h - 2),
                (radius - 1).max(0),
                0xFFFF_FFFF,
                22,
            );
        }
        self.stroke_rounded_rect(rect, radius, border_color, 170);
    }

    /// Multi-layered soft diffused shadow (blends ONLY outside `rect`'s opaque inner box).
    pub fn draw_soft_shadow(&mut self, rect: Rect, radius: i32, layers: i32, base_alpha: u8) {
        let layers = layers.clamp(1, 8);
        let safe_inset = radius.clamp(2, 18);
        let hole = Rect::new(
            rect.x + safe_inset,
            rect.y + safe_inset,
            (rect.w - safe_inset * 2).max(0),
            (rect.h - safe_inset * 2).max(0),
        );
        let shadow_col = super::theme::palette().shadow_color;

        for i in (1..=layers).rev() {
            let spread = i * 3;
            let drop_y = 2 + (i * 2) / 3;
            let shadow_rect = Rect::new(
                rect.x - spread,
                rect.y - spread + drop_y,
                rect.w + spread * 2,
                rect.h + spread * 2,
            );
            if shadow_rect.intersect(self.clip).is_empty() {
                continue;
            }
            let norm = (layers + 1 - i) as i32;
            let a = (((base_alpha as i32) * norm * norm) / (layers * layers * 3)).clamp(3, 72) as u8;
            self.fill_rounded_rect_excluding(shadow_rect, radius + spread, shadow_col, a, hole);
        }
    }

    pub fn draw_scrollbar_v(
        &mut self,
        track: Rect,
        total_items: usize,
        visible_items: usize,
        scroll_offset: usize,
        thumb_color: u32,
        track_color: u32,
    ) {
        if total_items <= visible_items || track.h < 20 || track.w < 4 {
            return;
        }
        self.fill_rounded_rect_alpha(track, track.w / 2, track_color, 90);
        let thumb_h = ((track.h as usize * visible_items) / total_items).max(20).min(track.h as usize) as i32;
        let max_scroll = total_items.saturating_sub(visible_items).max(1);
        let travel = (track.h - thumb_h).max(0);
        let thumb_y = track.y + ((travel as usize * scroll_offset.min(max_scroll)) / max_scroll) as i32;
        self.fill_rounded_rect(
            Rect::new(track.x + 1, thumb_y, (track.w - 2).max(2), thumb_h),
            (track.w - 2).max(2) / 2,
            thumb_color,
        );
    }

    fn fill_rounded_rect_excluding(
        &mut self,
        rect: Rect,
        radius: i32,
        color: u32,
        alpha: u8,
        hole: Rect,
    ) {
        let r = rect.intersect(self.clip);
        if r.is_empty() || alpha == 0 {
            return;
        }
        let rad = radius.max(0).min(rect.w / 2).min(rect.h / 2);
        let rad_q8 = rad << 8;
        let a_u32 = alpha as u32;
        let inv_u32 = 256 - a_u32;
        let src_rb = (color & 0x00FF_00FF) * a_u32;
        let src_g = (color & 0x0000_FF00) * a_u32;
        let mid_x0 = rect.x + rad;
        let mid_x1 = rect.right() - rad;

        for py in r.y..r.bottom() {
            let ly = py - rect.y;
            let in_hole_y = !hole.is_empty() && py >= hole.y && py < hole.bottom();
            let dy_q8 = if ly < rad {
                Some(((rad * 2) - (ly * 2 + 1)) * 128)
            } else if ly >= rect.h - rad {
                Some(((ly * 2 + 1) - (rect.h - rad) * 2) * 128)
            } else {
                None
            };
            let row_off = (py as usize) * self.width;

            let spans: [(i32, i32); 2] = if in_hole_y {
                [
                    (r.x, r.right().min(hole.x)),
                    (r.x.max(hole.right()), r.right()),
                ]
            } else {
                [(r.x, r.right()), (0, 0)]
            };

            for &(sx, ex) in &spans {
                if sx >= ex {
                    continue;
                }
                if let Some(dy_q8_val) = dy_q8 {
                    let left_end = ex.min(mid_x0);
                    for px in sx..left_end {
                        let lx = px - rect.x;
                        let dx_q8 = ((rad * 2) - (lx * 2 + 1)) * 128;
                        let cov = sdf_corner_fill_cov(dx_q8, dy_q8_val, rad_q8);
                        if cov == 255 {
                            let idx = row_off + px as usize;
                            let dst = self.buf[idx];
                            let rb = ((src_rb + (dst & 0x00FF_00FF) * inv_u32) >> 8) & 0x00FF_00FF;
                            let g = ((src_g + (dst & 0x0000_FF00) * inv_u32) >> 8) & 0x0000_FF00;
                            self.buf[idx] = 0xFF00_0000 | rb | g;
                        } else if cov > 0 {
                            let eff_a = (((cov as u32) * a_u32) >> 8) as u8;
                            if eff_a > 0 {
                                let idx = row_off + px as usize;
                                self.buf[idx] = blend_u32(self.buf[idx], color, eff_a);
                            }
                        }
                    }

                    let flat_start = sx.max(mid_x0);
                    let flat_end = ex.min(mid_x1);
                    if flat_start < flat_end {
                        for px in (flat_start as usize)..(flat_end as usize) {
                            let dst = self.buf[row_off + px];
                            let rb = ((src_rb + (dst & 0x00FF_00FF) * inv_u32) >> 8) & 0x00FF_00FF;
                            let g = ((src_g + (dst & 0x0000_FF00) * inv_u32) >> 8) & 0x0000_FF00;
                            self.buf[row_off + px] = 0xFF00_0000 | rb | g;
                        }
                    }

                    let right_start = sx.max(mid_x1);
                    for px in right_start..ex {
                        let lx = px - rect.x;
                        let dx_q8 = ((lx * 2 + 1) - (rect.w - rad) * 2) * 128;
                        let cov = sdf_corner_fill_cov(dx_q8, dy_q8_val, rad_q8);
                        if cov == 255 {
                            let idx = row_off + px as usize;
                            let dst = self.buf[idx];
                            let rb = ((src_rb + (dst & 0x00FF_00FF) * inv_u32) >> 8) & 0x00FF_00FF;
                            let g = ((src_g + (dst & 0x0000_FF00) * inv_u32) >> 8) & 0x0000_FF00;
                            self.buf[idx] = 0xFF00_0000 | rb | g;
                        } else if cov > 0 {
                            let eff_a = (((cov as u32) * a_u32) >> 8) as u8;
                            if eff_a > 0 {
                                let idx = row_off + px as usize;
                                self.buf[idx] = blend_u32(self.buf[idx], color, eff_a);
                            }
                        }
                    }
                } else {
                    for px in (sx as usize)..(ex as usize) {
                        let dst = self.buf[row_off + px];
                        let rb = ((src_rb + (dst & 0x00FF_00FF) * inv_u32) >> 8) & 0x00FF_00FF;
                        let g = ((src_g + (dst & 0x0000_FF00) * inv_u32) >> 8) & 0x0000_FF00;
                        self.buf[row_off + px] = 0xFF00_0000 | rb | g;
                    }
                }
            }
        }
    }

    pub fn draw_circle(&mut self, cx: i32, cy: i32, radius: i32, color: u32) {
        if radius <= 0 {
            return;
        }
        let bbox = Rect::new(cx - radius - 1, cy - radius - 1, radius * 2 + 3, radius * 2 + 3);
        if bbox.intersect(self.clip).is_empty() {
            return;
        }
        let rad_q8 = (radius << 8) + 64;
        for dy in -(radius + 1)..=(radius + 1) {
            let dy_q8 = dy << 8;
            for dx in -(radius + 1)..=(radius + 1) {
                let dx_q8 = dx << 8;
                let cov = sdf_corner_fill_cov(dx_q8, dy_q8, rad_q8);
                if cov > 0 {
                    self.blend_px(cx + dx, cy + dy, color, cov);
                }
            }
        }
    }

    pub fn draw_circle_alpha(&mut self, cx: i32, cy: i32, radius: i32, color: u32, alpha: u8) {
        if radius <= 0 || alpha == 0 {
            return;
        }
        let bbox = Rect::new(cx - radius, cy - radius, radius * 2 + 1, radius * 2 + 1);
        if bbox.intersect(self.clip).is_empty() {
            return;
        }
        let r2 = radius * radius;
        for dy in -radius..=radius {
            for dx in -radius..=radius {
                let d2 = dx * dx + dy * dy;
                if d2 <= r2 {
                    let a = ((((r2 - d2) * alpha as i32) / r2.max(1))).clamp(4, alpha as i32) as u8;
                    self.blend_px(cx + dx, cy + dy, color, a);
                }
            }
        }
    }

    /// Anti-aliased integer line segment for crisp vector UI strokes & sparklines.
    pub fn draw_line_aa(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, color: u32, radius_q8: i32) {
        let min_x = x0.min(x1) - 2;
        let min_y = y0.min(y1) - 2;
        let max_x = x0.max(x1) + 2;
        let max_y = y0.max(y1) + 2;
        let bbox = Rect::new(min_x, min_y, max_x - min_x + 1, max_y - min_y + 1).intersect(self.clip);
        if bbox.is_empty() {
            return;
        }

        let vx = (x1 - x0) as i64;
        let vy = (y1 - y0) as i64;
        let len2 = (vx * vx + vy * vy).max(1);
        let outer = (radius_q8 + 160).max(160) as i64;
        let inner = (radius_q8 - 110).max(0) as i64;
        let o2 = outer * outer;
        let i2 = inner * inner;
        let span = (o2 - i2).max(1);

        for py in bbox.y..bbox.bottom() {
            for px in bbox.x..bbox.right() {
                let wx = (px - x0) as i64;
                let wy = (py - y0) as i64;
                let dot = wx * vx + wy * vy;
                let (dx_q8, dy_q8) = if dot <= 0 {
                    (wx << 8, wy << 8)
                } else if dot >= len2 {
                    (((px - x1) as i64) << 8, ((py - y1) as i64) << 8)
                } else {
                    let t_q8 = (dot << 8) / len2;
                    let cx_q8 = (vx * t_q8) - (wx << 8);
                    let cy_q8 = (vy * t_q8) - (wy << 8);
                    (cx_q8, cy_q8)
                };
                let d2_q16 = dx_q8 * dx_q8 + dy_q8 * dy_q8;
                if d2_q16 <= i2 {
                    self.put_px(px, py, color);
                } else if d2_q16 < o2 {
                    let a = (((o2 - d2_q16) * 255) / span).clamp(0, 255) as u8;
                    if a > 0 {
                        self.blend_px(px, py, color, a);
                    }
                }
            }
        }
    }

    /// Crisp 3x3 App Launcher dots grid icon inside a 16x16 bounding box.
    pub fn draw_app_grid_icon(&mut self, x: i32, y: i32, color: u32) {
        for gy in 0..3i32 {
            for gx in 0..3i32 {
                let rx = x + 2 + gx * 5;
                let ry = y + 2 + gy * 5;
                self.fill_rounded_rect(Rect::new(rx, ry, 3, 3), 1, color);
            }
        }
    }

    /// Clean geometric battery indicator inside a 16x16 bounding box.
    pub fn draw_battery_icon(&mut self, x: i32, y: i32, color: u32) {
        let body = Rect::new(x + 1, y + 4, 12, 8);
        self.stroke_rounded_rect(body, 2, color, 235);
        self.fill_rounded_rect(Rect::new(x + 3, y + 6, 8, 4), 1, color);
        self.fill_rect_alpha(Rect::new(x + 13, y + 6, 2, 4), color, 235);
    }

    #[inline]
    fn draw_rasterized_glyph_clipped(
        &mut self,
        x: i32,
        baseline_y: i32,
        glyph: &RasterizedGlyph,
        color: u32,
        clip: Rect,
    ) {
        if glyph.width == 0 || glyph.height == 0 || glyph.bitmap.is_empty() {
            return;
        }
        let gx_start = x + glyph.xmin;
        let gy_start = baseline_y - (glyph.ymin + glyph.height as i32);
        let eff = clip.intersect(self.clip);
        let gx0 = gx_start.max(eff.x);
        let gy0 = gy_start.max(eff.y);
        let gx1 = (gx_start + glyph.width as i32).min(eff.right());
        let gy1 = (gy_start + glyph.height as i32).min(eff.bottom());
        if gx0 >= gx1 || gy0 >= gy1 {
            return;
        }

        let solid = color | 0xFF00_0000;
        let c_rb = color & 0x00FF_00FF;
        let c_g = color & 0x0000_FF00;
        let gw = glyph.width;

        for py in gy0..gy1 {
            let sy = (py - gy_start) as usize;
            let row_off = (py as usize) * self.width;
            let glyph_row = sy * gw;
            for px in gx0..gx1 {
                let sx = (px - gx_start) as usize;
                let a = glyph.bitmap[glyph_row + sx];
                if a == 0 {
                    continue;
                }
                let idx = row_off + (px as usize);
                if a == 255 {
                    self.buf[idx] = solid;
                } else {
                    let a32 = a as u32;
                    let inv = 256 - a32;
                    let dst = self.buf[idx];
                    let rb = ((c_rb * a32 + (dst & 0x00FF_00FF) * inv) >> 8) & 0x00FF_00FF;
                    let g = ((c_g * a32 + (dst & 0x0000_FF00) * inv) >> 8) & 0x0000_FF00;
                    self.buf[idx] = 0xFF00_0000 | rb | g;
                }
            }
        }
    }

    #[inline]
    fn draw_ui_bitmap_clipped(
        &mut self,
        x: i32,
        y: i32,
        bitmap: &[u8; 160],
        advance: i32,
        color: u32,
        clip: Rect,
    ) -> i32 {
        let eff = clip.intersect(self.clip);
        let gx0 = x.max(eff.x);
        let gy0 = y.max(eff.y);
        let gx1 = (x + GLYPH_WIDTH).min(eff.right());
        let gy1 = (y + GLYPH_HEIGHT).min(eff.bottom());
        if gx0 >= gx1 || gy0 >= gy1 {
            return advance;
        }

        let solid = color | 0xFF00_0000;
        let c_rb = color & 0x00FF_00FF;
        let c_g = color & 0x0000_FF00;
        let gw = GLYPH_WIDTH as usize;

        for py in gy0..gy1 {
            let sy = (py - y) as usize;
            let row_off = (py as usize) * self.width;
            let glyph_row = sy * gw;
            for px in gx0..gx1 {
                let sx = (px - x) as usize;
                let a = bitmap[glyph_row + sx];
                if a == 0 {
                    continue;
                }
                let idx = row_off + (px as usize);
                if a == 255 {
                    self.buf[idx] = solid;
                } else {
                    let a32 = a as u32;
                    let inv = 256 - a32;
                    let dst = self.buf[idx];
                    let rb = ((c_rb * a32 + (dst & 0x00FF_00FF) * inv) >> 8) & 0x00FF_00FF;
                    let g = ((c_g * a32 + (dst & 0x0000_FF00) * inv) >> 8) & 0x0000_FF00;
                    self.buf[idx] = 0xFF00_0000 | rb | g;
                }
            }
        }

        advance
    }

    #[inline]
    pub fn draw_glyph_aa_clipped(&mut self, x: i32, y: i32, ch: char, color: u32, clip: Rect) -> i32 {
        if let Some(adv) = with_ui_glyph(ch, 13, false, |g| {
            if ch != ' ' {
                self.draw_rasterized_glyph_clipped(x, y + 12, g, color, clip);
            }
            g.advance
        }) {
            return adv;
        }
        let (bitmap, advance) = lookup_glyph(ch);
        if ch == ' ' {
            return advance;
        }
        self.draw_ui_bitmap_clipped(x, y, bitmap, advance, color, clip)
    }

    #[inline]
    pub fn draw_glyph_bold_clipped(&mut self, x: i32, y: i32, ch: char, color: u32, clip: Rect) -> i32 {
        if let Some(adv) = with_ui_glyph(ch, 13, true, |g| {
            if ch != ' ' {
                self.draw_rasterized_glyph_clipped(x, y + 12, g, color, clip);
            }
            g.advance
        }) {
            return adv;
        }
        let (bitmap, advance) = lookup_glyph_bold(ch);
        if ch == ' ' {
            return advance;
        }
        self.draw_ui_bitmap_clipped(x, y, bitmap, advance, color, clip)
    }

    /// Smooth fontdue vector / Q8 bilinearly-upscaled anti-aliased glyph (Regular / SemiBold).
    pub fn draw_glyph_scaled_clipped(
        &mut self,
        x: i32,
        y: i32,
        ch: char,
        color: u32,
        scale_q8: i32,
        bold: bool,
        clip: Rect,
    ) -> i32 {
        let s = scale_q8.max(256);
        let px_size = (((13 * s + 128) >> 8).clamp(9, 42)) as u16;
        let baseline_y = y + ((12 * s + 128) >> 8);
        if let Some(adv) = with_ui_glyph(ch, px_size, bold, |g| {
            if ch != ' ' {
                self.draw_rasterized_glyph_clipped(x, baseline_y, g, color, clip);
            }
            g.advance
        }) {
            return adv;
        }

        if s == 256 {
            return if bold {
                self.draw_glyph_bold_clipped(x, y, ch, color, clip)
            } else {
                self.draw_glyph_aa_clipped(x, y, ch, color, clip)
            };
        }

        let (bitmap, advance) = if bold {
            lookup_glyph_bold(ch)
        } else {
            lookup_glyph(ch)
        };
        let out_adv = ((advance * s + 128) >> 8).max(1);
        if ch == ' ' {
            return out_adv;
        }

        let dst_w = (GLYPH_WIDTH * s + 255) >> 8;
        let dst_h = (GLYPH_HEIGHT * s + 255) >> 8;

        let eff = clip.intersect(self.clip);
        let gx0 = x.max(eff.x);
        let gy0 = y.max(eff.y);
        let gx1 = (x + dst_w).min(eff.right());
        let gy1 = (y + dst_h).min(eff.bottom());
        if gx0 >= gx1 || gy0 >= gy1 {
            return out_adv;
        }

        let c_rb = color & 0x00FF_00FF;
        let c_g = color & 0x0000_FF00;

        #[inline(always)]
        fn sample_bilinear(bmp: &[u8; 160], sx_q8: i32, sy_q8: i32) -> u32 {
            if sx_q8 < 0
                || sy_q8 < 0
                || sx_q8 >= (GLYPH_WIDTH << 8)
                || sy_q8 >= (GLYPH_HEIGHT << 8)
            {
                return 0;
            }
            let x0 = (sx_q8 >> 8) as usize;
            let y0 = (sy_q8 >> 8) as usize;
            let fx = (sx_q8 & 0xFF) as u32;
            let fy = (sy_q8 & 0xFF) as u32;
            let x1 = (x0 + 1).min((GLYPH_WIDTH - 1) as usize);
            let y1 = (y0 + 1).min((GLYPH_HEIGHT - 1) as usize);
            let gw = GLYPH_WIDTH as usize;

            let p00 = bmp[y0 * gw + x0] as u32;
            let p10 = bmp[y0 * gw + x1] as u32;
            let p01 = bmp[y1 * gw + x0] as u32;
            let p11 = bmp[y1 * gw + x1] as u32;

            let top = p00 * (256 - fx) + p10 * fx;
            let bot = p01 * (256 - fx) + p11 * fx;
            ((top * (256 - fy) + bot * fy) >> 16).min(255)
        }

        let inv_s_q16 = (1 << 24) / s;
        for py in gy0..gy1 {
            let sy_q8 = ((py - y) * inv_s_q16) >> 8;
            let row_off = (py as usize) * self.width;
            for px in gx0..gx1 {
                let sx_q8 = ((px - x) * inv_s_q16) >> 8;
                let a = sample_bilinear(bitmap, sx_q8, sy_q8);
                if a == 0 {
                    continue;
                }
                let idx = row_off + (px as usize);
                if a == 255 {
                    self.buf[idx] = color | 0xFF00_0000;
                } else {
                    let inv = 256 - a;
                    let dst = self.buf[idx];
                    let rb = ((c_rb * a + (dst & 0x00FF_00FF) * inv) >> 8) & 0x00FF_00FF;
                    let g = ((c_g * a + (dst & 0x0000_FF00) * inv) >> 8) & 0x0000_FF00;
                    self.buf[idx] = 0xFF00_0000 | rb | g;
                }
            }
        }

        out_adv
    }

    pub fn draw_text(&mut self, x: i32, y: i32, text: &str, color: u32, scale: i32) {
        let clip = self.clip;
        self.draw_text_clipped(x, y, text, color, scale, clip);
    }

    pub fn draw_text_clipped(
        &mut self,
        x: i32,
        y: i32,
        text: &str,
        color: u32,
        scale: i32,
        clip: Rect,
    ) {
        if scale >= 2 {
            self.draw_text_scaled_clipped(x, y, text, color, (scale * 256).min(512), false, clip);
            return;
        }
        let right_lim = clip.right().min(self.clip.right());
        let mut cx = x;
        for ch in text.chars() {
            if cx >= right_lim {
                break;
            }
            let adv = self.draw_glyph_aa_clipped(cx, y, ch, color, clip);
            cx += adv;
        }
    }

    pub fn draw_text_bold(&mut self, x: i32, y: i32, text: &str, color: u32) {
        let clip = self.clip;
        self.draw_text_bold_clipped(x, y, text, color, clip);
    }

    pub fn draw_text_bold_clipped(
        &mut self,
        x: i32,
        y: i32,
        text: &str,
        color: u32,
        clip: Rect,
    ) {
        let right_lim = clip.right().min(self.clip.right());
        let mut cx = x;
        for ch in text.chars() {
            if cx >= right_lim {
                break;
            }
            let adv = self.draw_glyph_bold_clipped(cx, y, ch, color, clip);
            cx += adv;
        }
    }

    pub fn draw_text_scaled_clipped(
        &mut self,
        x: i32,
        y: i32,
        text: &str,
        color: u32,
        scale_q8: i32,
        bold: bool,
        clip: Rect,
    ) {
        let right_lim = clip.right().min(self.clip.right());
        let mut cx = x;
        for ch in text.chars() {
            if cx >= right_lim {
                break;
            }
            let adv = self.draw_glyph_scaled_clipped(cx, y, ch, color, scale_q8, bold, clip);
            cx += adv;
        }
    }

    /// Renders UI text honoring Typography preferences (`scale_q8`, `bold`, `mono_ui` = JetBrains Mono vs Plus Jakarta Sans).
    pub fn draw_ui_text_clipped(
        &mut self,
        x: i32,
        y: i32,
        text: &str,
        color: u32,
        scale_q8: i32,
        bold: bool,
        mono_ui: bool,
        clip: Rect,
    ) {
        if mono_ui {
            self.draw_text_mono_ex_clipped(x, y, text, color, scale_q8.clamp(240, 360), false, clip);
        } else {
            self.draw_text_scaled_clipped(x, y, text, color, scale_q8.clamp(240, 384), bold, clip);
        }
    }

    /// Fixed-width monospace renderer supporting scaled JetBrains Mono (8x16) and Classic 8x8 (`font8x8`).
    pub fn draw_text_mono_ex_clipped(
        &mut self,
        x: i32,
        y: i32,
        text: &str,
        color: u32,
        scale_q8: i32,
        use_classic_8x8: bool,
        clip: Rect,
    ) {
        let s = scale_q8.clamp(240, 360);
        if !use_classic_8x8 && s == 256 {
            self.draw_text_mono_clipped(x, y, text, color, clip);
            return;
        }
        let eff = clip.intersect(self.clip);
        let adv = ((MONO_GLYPH_WIDTH * s + 128) >> 8).max(7);
        let dst_h = ((MONO_GLYPH_HEIGHT * s + 128) >> 8).max(12);
        let gy0 = y.max(eff.y);
        let gy1 = (y + dst_h).min(eff.bottom());
        if gy0 >= gy1 {
            return;
        }

        if use_classic_8x8 {
            use font8x8::{UnicodeFonts, BASIC_FONTS};
            let cell_scale_y = (dst_h / 8).max(1);
            let y_pad = ((dst_h - cell_scale_y * 8) / 2).max(0);
            let mut cx = x;
            for ch in text.chars() {
                if cx >= eff.right() {
                    break;
                }
                if cx + adv > eff.x && ch != ' ' {
                    if let Some(glyph) = BASIC_FONTS.get(ch) {
                        for (row, &bits) in glyph.iter().enumerate() {
                            for col in 0..8usize {
                                if (bits & (1u8 << col)) != 0 {
                                    let rx0 = cx + ((col as i32) * adv) / 8;
                                    let rx1 = cx + (((col as i32) + 1) * adv) / 8;
                                    let ry0 = y + y_pad + (row as i32) * cell_scale_y;
                                    for py in ry0.max(eff.y)..(ry0 + cell_scale_y).min(eff.bottom()) {
                                        for px in rx0.max(eff.x)..rx1.min(eff.right()) {
                                            self.put_px(px, py, color);
                                        }
                                    }
                                }
                            }
                        }
                    } else {
                        self.draw_glyph_scaled_clipped(cx, y, ch, color, s, false, clip);
                    }
                }
                cx += adv;
            }
            return;
        }

        // Vector fontdue JetBrains Mono with automatic bilinear 8x16 bitmap fallback
        let px_size = (((13 * s + 128) >> 8).clamp(9, 32)) as u16;
        let baseline_y = y + ((12 * s + 128) >> 8);
        let solid = color | 0xFF00_0000;
        let c_rb = color & 0x00FF_00FF;
        let c_g = color & 0x0000_FF00;
        let inv_s_q16 = (1 << 24) / s;
        let mw = MONO_GLYPH_WIDTH as usize;

        let mut cx = x;
        for ch in text.chars() {
            if cx >= eff.right() {
                break;
            }
            if cx + adv > eff.x && ch != ' ' {
                let drawn_vec = with_mono_glyph(ch, px_size, |g| {
                    self.draw_rasterized_glyph_clipped(cx, baseline_y, g, color, clip);
                })
                .is_some();
                if !drawn_vec {
                    let bmp = lookup_mono_glyph(ch);
                    let gx0 = cx.max(eff.x);
                    let gx1 = (cx + adv).min(eff.right());
                    for py in gy0..gy1 {
                        let sy_q8 = ((py - y) * inv_s_q16) >> 8;
                        let y0 = (sy_q8 >> 8).clamp(0, MONO_GLYPH_HEIGHT - 1) as usize;
                        let y1 = (y0 + 1).min((MONO_GLYPH_HEIGHT - 1) as usize);
                        let fy = (sy_q8 & 0xFF) as u32;
                        let row_off = (py as usize) * self.width;
                        for px in gx0..gx1 {
                            let sx_q8 = ((px - cx) * inv_s_q16) >> 8;
                            let x0 = (sx_q8 >> 8).clamp(0, MONO_GLYPH_WIDTH - 1) as usize;
                            let x1 = (x0 + 1).min((MONO_GLYPH_WIDTH - 1) as usize);
                            let fx = (sx_q8 & 0xFF) as u32;
                            let p00 = bmp[y0 * mw + x0] as u32;
                            let p10 = bmp[y0 * mw + x1] as u32;
                            let p01 = bmp[y1 * mw + x0] as u32;
                            let p11 = bmp[y1 * mw + x1] as u32;
                            let top = p00 * (256 - fx) + p10 * fx;
                            let bot = p01 * (256 - fx) + p11 * fx;
                            let a = ((top * (256 - fy) + bot * fy) >> 16).min(255);
                            if a == 0 {
                                continue;
                            }
                            let idx = row_off + (px as usize);
                            if a == 255 {
                                self.buf[idx] = solid;
                            } else {
                                let inv = 256 - a;
                                let dst = self.buf[idx];
                                let rb = ((c_rb * a + (dst & 0x00FF_00FF) * inv) >> 8) & 0x00FF_00FF;
                                let g = ((c_g * a + (dst & 0x0000_FF00) * inv) >> 8) & 0x0000_FF00;
                                self.buf[idx] = 0xFF00_0000 | rb | g;
                            }
                        }
                    }
                }
            }
            cx += adv;
        }
    }

    /// Fixed-width JetBrains Mono (vector fontdue at 13px + 8x16 bitmap fallback) for Terminal and Text Editor.
    pub fn draw_text_mono_clipped(
        &mut self,
        x: i32,
        y: i32,
        text: &str,
        color: u32,
        clip: Rect,
    ) {
        let eff = clip.intersect(self.clip);
        let solid = color | 0xFF00_0000;
        let c_rb = color & 0x00FF_00FF;
        let c_g = color & 0x0000_FF00;
        let mw = MONO_GLYPH_WIDTH as usize;

        let gy0 = y.max(eff.y);
        let gy1 = (y + MONO_GLYPH_HEIGHT).min(eff.bottom());
        if gy0 >= gy1 {
            return;
        }

        let mut cx = x;
        for ch in text.chars() {
            if cx >= eff.right() {
                break;
            }
            if cx + MONO_GLYPH_WIDTH > eff.x && ch != ' ' {
                let drawn_vec = with_mono_glyph(ch, 13, |g| {
                    self.draw_rasterized_glyph_clipped(cx, y + 12, g, color, clip);
                })
                .is_some();
                if !drawn_vec {
                    let bmp = lookup_mono_glyph(ch);
                    let gx0 = cx.max(eff.x);
                    let gx1 = (cx + MONO_GLYPH_WIDTH).min(eff.right());
                    for py in gy0..gy1 {
                        let sy = (py - y) as usize;
                        let row_off = (py as usize) * self.width;
                        let glyph_row = sy * mw;
                        for px in gx0..gx1 {
                            let sx = (px - cx) as usize;
                            let a = bmp[glyph_row + sx];
                            if a == 0 {
                                continue;
                            }
                            let idx = row_off + (px as usize);
                            if a == 255 {
                                self.buf[idx] = solid;
                            } else {
                                let a32 = a as u32;
                                let inv = 256 - a32;
                                let dst = self.buf[idx];
                                let rb = ((c_rb * a32 + (dst & 0x00FF_00FF) * inv) >> 8) & 0x00FF_00FF;
                                let g = ((c_g * a32 + (dst & 0x0000_FF00) * inv) >> 8) & 0x0000_FF00;
                                self.buf[idx] = 0xFF00_0000 | rb | g;
                            }
                        }
                    }
                }
            }
            cx += MONO_GLYPH_WIDTH;
        }
    }

    pub fn draw_cursor(&mut self, x: i32, y: i32) {
        const CURSOR_ROWS: [&[u8; 12]; 17] = [
            b"#...........",
            b"##..........",
            b"#W#.........",
            b"#WW#........",
            b"#WWW#.......",
            b"#WWWW#......",
            b"#WWWWW#.....",
            b"#WWWWWW#....",
            b"#WWWWWWW#...",
            b"#WWWWWWWW#..",
            b"#WWWWW####..",
            b"#WW#WW#.....",
            b"#W#.#WW#....",
            b"##..#WW#....",
            b"#....#WW#...",
            b".....#WW#...",
            b"......##....",
        ];

        for (dy, row) in CURSOR_ROWS.iter().enumerate() {
            for (dx, &px) in row.iter().enumerate() {
                match px {
                    b'#' => self.put_px(x + dx as i32, y + dy as i32, 0xFFFFFFFF),
                    b'W' => self.put_px(x + dx as i32, y + dy as i32, 0xFF0A0B0E),
                    _ => {}
                }
            }
        }
    }
}

/// Fast branchless integer alpha blend using `>> 8` bit shifts (zero divisions).
#[inline(always)]
pub fn blend_u32(dst: u32, src: u32, alpha: u8) -> u32 {
    let a = alpha as u32;
    let inv = 256 - a;
    let rb = (((src & 0x00FF_00FF) * a + (dst & 0x00FF_00FF) * inv) >> 8) & 0x00FF_00FF;
    let g = (((src & 0x0000_FF00) * a + (dst & 0x0000_FF00) * inv) >> 8) & 0x0000_FF00;
    0xFF00_0000 | rb | g
}

pub fn text_width(text: &str, scale: i32) -> i32 {
    if scale >= 2 {
        return text_width_scaled(text, (scale * 256).min(512), false);
    }
    let mut w = 0i32;
    for ch in text.chars() {
        w += ui_glyph_advance(ch, 13, false).unwrap_or_else(|| lookup_glyph(ch).1);
    }
    w
}

pub fn text_width_bold(text: &str) -> i32 {
    let mut w = 0i32;
    for ch in text.chars() {
        w += ui_glyph_advance(ch, 13, true).unwrap_or_else(|| lookup_glyph_bold(ch).1);
    }
    w
}

pub fn text_width_scaled(text: &str, scale_q8: i32, bold: bool) -> i32 {
    let s = scale_q8.max(256);
    if s == 256 {
        return if bold { text_width_bold(text) } else { text_width(text, 1) };
    }
    let px_size = (((13 * s + 128) >> 8).clamp(9, 42)) as u16;
    let mut w = 0i32;
    for ch in text.chars() {
        if let Some(adv) = ui_glyph_advance(ch, px_size, bold) {
            w += adv;
        } else {
            let adv = if bold {
                lookup_glyph_bold(ch).1
            } else {
                lookup_glyph(ch).1
            };
            w += ((adv * s + 128) >> 8).max(1);
        }
    }
    w
}

#[inline]
pub fn text_width_mono(text: &str) -> i32 {
    (text.chars().count() as i32) * MONO_GLYPH_WIDTH
}

#[inline]
pub fn mono_metrics(scale_q8: i32) -> (i32, i32) {
    let s = scale_q8.clamp(240, 360);
    let adv = ((MONO_GLYPH_WIDTH * s + 128) >> 8).max(7);
    let line_h = (((MONO_GLYPH_HEIGHT + 2) * s + 128) >> 8).max(16);
    (adv, line_h)
}

#[inline]
pub fn text_width_mono_ex(text: &str, scale_q8: i32) -> i32 {
    let (adv, _) = mono_metrics(scale_q8);
    (text.chars().count() as i32) * adv
}

#[inline]
pub fn ui_text_width(text: &str, scale_q8: i32, bold: bool, mono_ui: bool) -> i32 {
    if mono_ui {
        text_width_mono_ex(text, scale_q8)
    } else {
        text_width_scaled(text, scale_q8, bold)
    }
}

