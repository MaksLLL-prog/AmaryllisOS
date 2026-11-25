use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use super::personalization::{WallpaperScaleMode, WallpaperSource};
use super::renderer::Renderer;
use super::theme::{self, ThemeMode, WallpaperPreset};
use super::widgets::{blend_u32, Canvas, Rect};
use crate::storage::vfs;

const MAX_DIRTY_RECTS: usize = 24;
const MOTES_COUNT: usize = 10;

struct SavedMotePatch {
    rect: Rect,
    pixels: Vec<u32>,
}

impl SavedMotePatch {
    const fn empty() -> Self {
        Self {
            rect: Rect::new(0, 0, 0, 0),
            pixels: Vec::new(),
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
struct RenderedWallpaperKey {
    mode: Option<ThemeMode>,
    source: WallpaperSource,
    scale: WallpaperScaleMode,
}

impl RenderedWallpaperKey {
    fn for_preset(mode: ThemeMode, preset: WallpaperPreset, scale: WallpaperScaleMode) -> Self {
        let mode_dep = matches!(
            preset,
            WallpaperPreset::ObsidianWaves | WallpaperPreset::SilkDunes
        );
        Self {
            mode: if mode_dep { Some(mode) } else { None },
            source: WallpaperSource::Preset(preset),
            scale,
        }
    }

    fn for_source(mode: ThemeMode, source: &WallpaperSource, scale: WallpaperScaleMode) -> Self {
        match source {
            WallpaperSource::Preset(p) => Self::for_preset(mode, *p, scale),
            WallpaperSource::CustomFile(_) => {
                let mode_dep = matches!(scale, WallpaperScaleMode::Fit | WallpaperScaleMode::Center);
                Self {
                    mode: if mode_dep { Some(mode) } else { None },
                    source: source.clone(),
                    scale,
                }
            }
        }
    }
}

pub struct Compositor {
    width: usize,
    height: usize,
    backbuffer: Vec<u32>,
    wallpaper_cache: Vec<u32>,
    saved_motes: [SavedMotePatch; MOTES_COUNT],
    saved_cursor_rect: Rect,
    saved_cursor_pixels: Vec<u32>,
    dirty: Vec<Rect>,
    full_dirty: bool,
    last_cursor: Option<(i32, i32)>,
    last_anim_phase: u64,
    anim_active: bool,
    last_wp_error: Option<String>,
    rendered_key: Option<RenderedWallpaperKey>,
}

impl Compositor {
    pub fn new(width: usize, height: usize) -> Self {
        let len = width.saturating_mul(height);
        let init_mode = theme::current_theme_mode();
        let init_preset = theme::current_wallpaper();

        let mut wallpaper_cache = vec![0u32; len];
        fill_amaryllis_wallpaper(&mut wallpaper_cache, width, height, init_mode, init_preset);

        let mut backbuffer = vec![0u32; len];
        backbuffer.copy_from_slice(&wallpaper_cache);

        Self {
            width,
            height,
            backbuffer,
            wallpaper_cache,
            saved_motes: [
                SavedMotePatch::empty(),
                SavedMotePatch::empty(),
                SavedMotePatch::empty(),
                SavedMotePatch::empty(),
                SavedMotePatch::empty(),
                SavedMotePatch::empty(),
                SavedMotePatch::empty(),
                SavedMotePatch::empty(),
                SavedMotePatch::empty(),
                SavedMotePatch::empty(),
            ],
            saved_cursor_rect: Rect::new(0, 0, 0, 0),
            saved_cursor_pixels: Vec::with_capacity(12 * 17),
            dirty: Vec::with_capacity(MAX_DIRTY_RECTS),
            full_dirty: true,
            last_cursor: None,
            last_anim_phase: 0,
            anim_active: false,
            last_wp_error: None,
            rendered_key: Some(RenderedWallpaperKey::for_preset(
                init_mode,
                init_preset,
                WallpaperScaleMode::Fill,
            )),
        }
    }

    pub fn last_wallpaper_error(&self) -> Option<&str> {
        self.last_wp_error.as_deref()
    }

    pub fn force_rebuild_custom(&mut self) {
        self.rendered_key = None;
    }

    fn clear_saved_motes(&mut self) {
        for slot in &mut self.saved_motes {
            slot.rect = Rect::new(0, 0, 0, 0);
        }
        self.anim_active = false;
    }

    fn restore_saved_motes(&mut self, invalidate: bool) {
        for i in (0..MOTES_COUNT).rev() {
            let r = self.saved_motes[i].rect;
            if r.is_empty() {
                continue;
            }
            let x0 = r.x as usize;
            let x1 = r.right() as usize;
            let rw = x1.saturating_sub(x0);
            let rh = (r.bottom() as usize).saturating_sub(r.y as usize);
            if rw > 0 && rh > 0 && self.saved_motes[i].pixels.len() >= rw * rh {
                for row_idx in 0..rh {
                    let py = (r.y as usize) + row_idx;
                    let dst_off = py * self.width + x0;
                    let src_off = row_idx * rw;
                    self.wallpaper_cache[dst_off..dst_off + rw]
                        .copy_from_slice(&self.saved_motes[i].pixels[src_off..src_off + rw]);
                }
            }
            self.saved_motes[i].rect = Rect::new(0, 0, 0, 0);
            if invalidate {
                self.invalidate_rect(r);
            }
        }
    }

    pub fn rebuild_wallpaper(&mut self, mode: ThemeMode, preset: WallpaperPreset) {
        let key = RenderedWallpaperKey::for_preset(mode, preset, WallpaperScaleMode::Fill);
        if self.rendered_key.as_ref() == Some(&key) {
            self.last_wp_error = None;
            return;
        }
        self.last_wp_error = None;
        fill_amaryllis_wallpaper(
            &mut self.wallpaper_cache,
            self.width,
            self.height,
            mode,
            preset,
        );
        self.clear_saved_motes();
        self.rendered_key = Some(key);
        self.invalidate_all();
    }

    pub fn rebuild_wallpaper_from_source(
        &mut self,
        mode: ThemeMode,
        source: &WallpaperSource,
        scale: WallpaperScaleMode,
        fallback_preset: WallpaperPreset,
    ) {
        match source {
            WallpaperSource::Preset(p) => {
                self.rebuild_wallpaper(mode, *p);
            }
            WallpaperSource::CustomFile(path) => {
                let key = RenderedWallpaperKey::for_source(mode, source, scale);
                if self.rendered_key.as_ref() == Some(&key) {
                    return;
                }
                match decode_image_from_vfs(path) {
                    Ok((img_w, img_h, pixels)) => {
                        self.last_wp_error = None;
                        let backdrop = if matches!(mode, ThemeMode::Light) {
                            0xFFEAECEF
                        } else {
                            0xFF0A0A0D
                        };
                        scale_image_wallpaper_into(
                            &mut self.wallpaper_cache,
                            self.width,
                            self.height,
                            img_w,
                            img_h,
                            &pixels,
                            scale,
                            backdrop,
                        );
                        self.clear_saved_motes();
                        self.rendered_key = Some(key);
                        self.invalidate_all();
                    }
                    Err(err) => {
                        self.last_wp_error = Some(String::from(err));
                        fill_amaryllis_wallpaper(
                            &mut self.wallpaper_cache,
                            self.width,
                            self.height,
                            mode,
                            fallback_preset,
                        );
                        self.clear_saved_motes();
                        self.rendered_key = Some(RenderedWallpaperKey::for_preset(
                            mode,
                            fallback_preset,
                            WallpaperScaleMode::Fill,
                        ));
                        self.invalidate_all();
                    }
                }
            }
        }
    }

    /// Lightweight ~15Hz animated shimmer overlay on desktop wallpaper when enabled.
    /// Saves/restores only tiny mote patches (< 40 KiB total) instead of duplicating the full framebuffer.
    pub fn step_wallpaper_animation(&mut self, uptime_ms: u64, enabled: bool, accent: u32) {
        if !enabled {
            if self.anim_active {
                self.restore_saved_motes(true);
                self.anim_active = false;
            }
            return;
        }

        let phase = uptime_ms / 66;
        if self.anim_active && phase == self.last_anim_phase {
            return;
        }
        self.last_anim_phase = phase;
        self.anim_active = true;

        self.restore_saved_motes(true);

        let w = self.width as i32;
        let h = self.height as i32;
        let p = phase as i32;
        let soft_col = blend_u32(accent, 0xFFFFFFFF, 120);

        for i in 0..MOTES_COUNT {
            let idx = i as i32;
            let rad = 10 + (i as i32 % 3) * 4;
            let cx = ((idx * 173 + p * (1 + (idx % 3))) % (w + 60)) - 30;
            let bob = ((p + idx * 19) % 80) - 40;
            let dy = (bob.abs() * 14) / 40;
            let cy = (80 + ((idx * 119 + p / 2) % (h - 150).max(80)) + dy).clamp(36, h - 36);
            let r = Rect::new(cx - rad, cy - rad, rad * 2 + 1, rad * 2 + 1)
                .clamp_to_screen(self.width, self.height);
            if !r.is_empty() {
                let x0 = r.x as usize;
                let rw = (r.right() as usize).saturating_sub(x0);
                let rh = (r.bottom() as usize).saturating_sub(r.y as usize);
                let need = rw * rh;
                if self.saved_motes[i].pixels.len() < need {
                    self.saved_motes[i].pixels.resize(need, 0);
                }
                for row_idx in 0..rh {
                    let py = (r.y as usize) + row_idx;
                    let src_off = py * self.width + x0;
                    let dst_off = row_idx * rw;
                    self.saved_motes[i].pixels[dst_off..dst_off + rw]
                        .copy_from_slice(&self.wallpaper_cache[src_off..src_off + rw]);
                }
                self.saved_motes[i].rect = r;

                let col = if i % 2 == 0 { soft_col } else { accent };
                let r2 = rad * rad;
                for py in r.y..r.bottom() {
                    let row = (py as usize) * self.width;
                    let dy_px = py - cy;
                    for px in r.x..r.right() {
                        let dx_px = px - cx;
                        let d2 = dx_px * dx_px + dy_px * dy_px;
                        if d2 < r2 {
                            let a = (((r2 - d2) * 34) / r2.max(1)).clamp(2, 36) as u8;
                            let slot = row + (px as usize);
                            self.wallpaper_cache[slot] = blend_u32(self.wallpaper_cache[slot], col, a);
                        }
                    }
                }
                self.invalidate_rect(r);
            } else {
                self.saved_motes[i].rect = Rect::new(0, 0, 0, 0);
            }
        }
    }

    pub fn width(&self) -> usize {
        self.width
    }

    pub fn height(&self) -> usize {
        self.height
    }

    pub fn screen_rect(&self) -> Rect {
        Rect::new(0, 0, self.width as i32, self.height as i32)
    }

    pub fn invalidate_all(&mut self) {
        self.full_dirty = true;
        self.dirty.clear();
    }

    pub fn invalidate_rect(&mut self, rect: Rect) {
        if self.full_dirty {
            return;
        }
        let r = rect.clamp_to_screen(self.width, self.height);
        if r.is_empty() {
            return;
        }
        for existing in &mut self.dirty {
            let expanded = existing.expanded(4);
            if !expanded.intersect(r).is_empty() {
                *existing = existing.union(r);
                return;
            }
        }
        if self.dirty.len() >= MAX_DIRTY_RECTS {
            self.full_dirty = true;
            self.dirty.clear();
        } else {
            self.dirty.push(r);
        }
    }

    pub fn invalidate_cursor_move(&mut self, new_x: i32, new_y: i32) {
        if let Some((lx, ly)) = self.last_cursor {
            if lx == new_x && ly == new_y {
                return;
            }
            self.invalidate_rect(Rect::new(lx - 1, ly - 1, 16, 20));
        }
        if !self.saved_cursor_rect.is_empty() {
            self.invalidate_rect(self.saved_cursor_rect.expanded(1));
        }
        self.invalidate_rect(Rect::new(new_x - 1, new_y - 1, 16, 20));
    }

    /// Returns the damage rectangles to redraw in the current frame (`[screen_rect]` if full_dirty).
    pub fn damage_list(&self) -> Vec<Rect> {
        if self.full_dirty {
            vec![self.screen_rect()]
        } else {
            self.dirty.clone()
        }
    }

    fn restore_saved_cursor_patch(&mut self) {
        let r = self.saved_cursor_rect.clamp_to_screen(self.width, self.height);
        self.saved_cursor_rect = Rect::new(0, 0, 0, 0);
        if r.is_empty() || self.saved_cursor_pixels.is_empty() {
            self.saved_cursor_pixels.clear();
            return;
        }
        let rw = r.w as usize;
        let rh = r.h as usize;
        if self.saved_cursor_pixels.len() < rw * rh {
            self.saved_cursor_pixels.clear();
            return;
        }
        let x0 = r.x as usize;
        for row_i in 0..rh {
            let dst_off = ((r.y as usize) + row_i) * self.width + x0;
            let src_off = row_i * rw;
            self.backbuffer[dst_off..dst_off + rw]
                .copy_from_slice(&self.saved_cursor_pixels[src_off..src_off + rw]);
        }
        self.saved_cursor_pixels.clear();
    }

    fn save_cursor_patch(&mut self, cx: i32, cy: i32) {
        let r = Rect::new(cx, cy, 12, 17).clamp_to_screen(self.width, self.height);
        self.saved_cursor_pixels.clear();
        if r.is_empty() {
            self.saved_cursor_rect = Rect::new(0, 0, 0, 0);
            return;
        }
        let rw = r.w as usize;
        let rh = r.h as usize;
        self.saved_cursor_rect = r;
        let x0 = r.x as usize;
        for row_i in 0..rh {
            let src_off = ((r.y as usize) + row_i) * self.width + x0;
            self.saved_cursor_pixels
                .extend_from_slice(&self.backbuffer[src_off..src_off + rw]);
        }
    }

    /// Restores damaged areas of `backbuffer` directly from `wallpaper_cache` via fast `copy_from_slice`.
    pub fn begin_frame(&mut self) {
        self.begin_frame_with_occluders(&[]);
    }

    /// Restores damaged areas of `backbuffer` from `wallpaper_cache`, skipping regions
    /// completely occluded by opaque window interiors, after cleanly restoring the previous cursor patch.
    pub fn begin_frame_with_occluders(&mut self, occluders: &[Rect]) {
        if self.full_dirty {
            self.saved_cursor_rect = Rect::new(0, 0, 0, 0);
            self.saved_cursor_pixels.clear();
            self.backbuffer.copy_from_slice(&self.wallpaper_cache);
            return;
        }
        // Always restore the pixels under the previous cursor BEFORE repairing dirty regions
        // or repainting occluded/opaque windows, preventing stale cursor ghosts.
        self.restore_saved_cursor_patch();
        for rect in &self.dirty {
            let r = rect.clamp_to_screen(self.width, self.height);
            if r.is_empty() {
                continue;
            }
            if occluders.iter().any(|occ| {
                r.x >= occ.x
                    && r.y >= occ.y
                    && r.right() <= occ.right()
                    && r.bottom() <= occ.bottom()
            }) {
                continue;
            }
            let x0 = r.x as usize;
            let x1 = r.right() as usize;
            for py in (r.y as usize)..(r.bottom() as usize) {
                let row = py * self.width;
                self.backbuffer[row + x0..row + x1]
                    .copy_from_slice(&self.wallpaper_cache[row + x0..row + x1]);
            }
        }
    }

    pub fn canvas(&mut self) -> Canvas<'_> {
        let clip = self.screen_rect();
        Canvas::new(&mut self.backbuffer, self.width, self.height, clip)
    }

    pub fn canvas_clipped(&mut self, clip: Rect) -> Canvas<'_> {
        Canvas::new(&mut self.backbuffer, self.width, self.height, clip)
    }

    pub fn draw_cursor_and_flush(&mut self, renderer: &mut Renderer<'_>, cx: i32, cy: i32) {
        self.save_cursor_patch(cx, cy);
        self.canvas().draw_cursor(cx, cy);
        self.last_cursor = Some((cx, cy));

        if self.full_dirty {
            renderer.blit_xrgb_region(&self.backbuffer, self.width, 0, 0, self.width, self.height);
            self.full_dirty = false;
            self.dirty.clear();
            return;
        }

        for rect in self.dirty.drain(..) {
            let r = rect.clamp_to_screen(self.width, self.height);
            if r.is_empty() {
                continue;
            }
            renderer.blit_xrgb_region(
                &self.backbuffer,
                self.width,
                r.x as usize,
                r.y as usize,
                r.w as usize,
                r.h as usize,
            );
        }
    }

    #[inline]
    pub fn has_dirty(&self) -> bool {
        self.full_dirty || !self.dirty.is_empty()
    }
}

const BAYER_4X4: [[i32; 4]; 4] = [
    [0, 8, 2, 10],
    [12, 4, 14, 6],
    [3, 11, 1, 9],
    [15, 7, 13, 5],
];

/// Integer square root on u64.
#[inline(always)]
fn isqrt_u64(n: u64) -> u64 {
    if n < 2 {
        return n;
    }
    let mut x0 = 1u64 << ((64 - n.leading_zeros() + 1) / 2);
    loop {
        let x1 = (x0 + n / x0) >> 1;
        if x1 >= x0 {
            return x0;
        }
        x0 = x1;
    }
}

/// Smooth Hermite S-curve in Q12 fixed point (0..4096 -> 0..4096).
#[inline(always)]
fn smooth_q12(t: i64) -> i64 {
    let c = t.clamp(0, 4096);
    (c * c * (12_288 - 2 * c)) >> 24
}

/// Smooth C1 parabolic bell peaking at 4096 when `t == 2048` (0 at 0 and 4096).
#[inline(always)]
fn bell_q12(t: i64) -> i64 {
    let c = t.clamp(0, 4096);
    (4 * c * (4096 - c)) >> 12
}

/// Smooth asymmetric S-wave helper: cubic bump peaking smoothly near t=2/3 without kinks.
#[inline(always)]
fn wave_q12(t: i64) -> i64 {
    let c = t.clamp(0, 4096);
    let c2 = (c * c) >> 12;
    (27 * ((c2 * (4096 - c)) >> 12)) >> 4
}

/// Evaluates the 4 silk fold X boundaries in Q12 pixels (`px * 4096`) for normalized `t_q12`.
#[inline(always)]
fn eval_folds_q12(t_q12: i64, w: i64) -> [i64; 4] {
    let s1 = smooth_q12(((t_q12 - 120) * 4096) / 3600);
    let b1 = bell_q12(t_q12);
    let x1 = ((w * 14) << 12) / 100
        + (((w * 28) * b1) / 100)
        - (((w * 14) * s1) / 100);

    let s2 = smooth_q12((t_q12 * 4096) / 3950);
    let w2 = wave_q12(t_q12);
    let x2 = ((w * 72) << 12) / 100
        - (((w * 70) * s2) / 100)
        + (((w * 16) * w2) / 100);

    let s3 = smooth_q12(t_q12);
    let b3 = (bell_q12(t_q12) * 3) / 4;
    let x3 = ((w * 108) << 12) / 100
        - (((w * 86) * s3) / 100)
        - (((w * 11) * b3) / 100);

    let s4 = smooth_q12(((t_q12 - 140) * 4096) / 3956);
    let b4 = bell_q12(t_q12);
    let x4 = ((w * 116) << 12) / 100
        - (((w * 64) * s4) / 100)
        + (((w * 9) * b4) / 100);

    [x1, x2, x3, x4]
}

#[inline(always)]
fn apply_fold_layer(
    color: u32,
    signed_dx_q12: i64,
    hyp_q12: i64,
    w: i64,
    fill_col: u32,
    bot_col: u32,
    rim_col: u32,
    shadow_col: u32,
    shadow_width_px: i64,
    shadow_strength: i64,
    grad_span_div: i64,
    rim_width_px: i64,
    rim_strength: i64,
) -> u32 {
    // Orthogonal signed distance from fold edge in Q8 subpixels (256 = 1.0 perpendicular px)
    let d_perp_q8 = (signed_dx_q12 * 256) / hyp_q12.max(4096);
    let shadow_lim_q8 = -shadow_width_px * 256;
    if d_perp_q8 <= shadow_lim_q8 {
        return color;
    }

    let mut out = color;
    if d_perp_q8 < 320 {
        // Cast soft ambient shadow just outside the crest (smooth quadratic falloff)
        let s_raw = ((d_perp_q8 - shadow_lim_q8).clamp(0, shadow_width_px * 256) * 256)
            / (shadow_width_px * 256).max(1);
        let sa = (((s_raw * s_raw) >> 16) * shadow_strength).clamp(0, 220) as u8;
        if sa > 0 {
            out = blend_u32(out, shadow_col, sa);
        }
    }

    // Coverage across [-320 .. +320] (~2.5 perpendicular pixels smooth anti-aliasing)
    if d_perp_q8 > -320 {
        let grad_span = (w * grad_span_div / 100).max(1);
        let grad_t = ((d_perp_q8.max(0) * 225) / (grad_span * 256)).min(220) as u8;
        let mut layer_col = blend_u32(fill_col, bot_col, grad_t);

        let rim_lim_q8 = rim_width_px * 256;
        if d_perp_q8 < rim_lim_q8 {
            let rem_q8 = (rim_lim_q8 - d_perp_q8.max(0)).clamp(0, rim_lim_q8);
            let norm_q8 = (rem_q8 * 256) / rim_lim_q8.max(1);
            let ha = ((((norm_q8 * norm_q8) >> 16) * rim_strength)).min(180) as u8;
            if ha > 0 {
                layer_col = blend_u32(layer_col, rim_col, ha);
            }
        }

        let aa = if d_perp_q8 >= 320 {
            255
        } else {
            (((d_perp_q8 + 320) * 255) / 640).clamp(0, 255) as u8
        };
        out = blend_u32(out, layer_col, aa);
    }

    out
}

#[inline(always)]
fn apply_bayer_dither(color: u32, px: usize, py: usize) -> u32 {
    let threshold = BAYER_4X4[py & 3][px & 3] - 8; // -8..+7 centered jitter
    let delta = threshold / 3; // subtle +-2 LSB ordered dither breaks all 8-bit gradient banding
    let r = (((color >> 16) & 0xFF) as i32 + delta).clamp(0, 255) as u32;
    let g = (((color >> 8) & 0xFF) as i32 + delta).clamp(0, 255) as u32;
    let b = ((color & 0xFF) as i32 + delta).clamp(0, 255) as u32;
    0xFF00_0000 | (r << 16) | (g << 8) | b
}

/// Pre-renders one of 5 C2-smooth procedural wallpapers directly into `buf` with 4x4 Bayer dithering.
fn fill_amaryllis_wallpaper(
    buf: &mut [u32],
    width: usize,
    height: usize,
    mode: ThemeMode,
    preset: WallpaperPreset,
) {
    if buf.len() < width.saturating_mul(height) {
        return;
    }
    match preset {
        WallpaperPreset::AuroraBorealis => fill_aurora_wallpaper(buf, width, height),
        WallpaperPreset::SunsetHorizon => fill_sunset_wallpaper(buf, width, height),
        WallpaperPreset::MidnightMinimal => fill_midnight_minimal_wallpaper(buf, width, height),
        WallpaperPreset::SilkDunes => fill_silk_wallpaper(buf, width, height, mode, false),
        WallpaperPreset::ObsidianWaves => fill_silk_wallpaper(buf, width, height, mode, true),
    }
}

fn fill_silk_wallpaper(
    buf: &mut [u32],
    width: usize,
    height: usize,
    mode: ThemeMode,
    monochrome: bool,
) {
    let w = width.max(1) as i64;
    let h = height.max(1) as i64;

    let (
        base_tl,
        base_tr,
        base_bl,
        base_br,
        l1_top,
        l1_bot,
        l1_rim,
        l2_top,
        l2_bot,
        l2_rim,
        l3_top,
        l3_bot,
        l3_rim,
        l4_top,
        l4_bot,
        l4_rim,
        shadow_col,
    ) = if monochrome {
        match mode {
            ThemeMode::Light => (
                0xFFE4E6EC, // Alabaster Silk TL
                0xFFF7F8FA, // TR
                0xFFD4D7DF, // BL
                0xFFECEEF3, // BR
                0xFFF1F2F6, // Layer 1 top
                0xFFDCE0E8, // Layer 1 bot
                0xFFFFFFFF, // Layer 1 rim highlight
                0xFFFFFFFF, // Layer 2 top
                0xFFE5E8EF, // Layer 2 bot
                0xFFFFFFFF, // Layer 2 rim highlight
                0xFFEBEEF4, // Layer 3 top
                0xFFD6DAE3, // Layer 3 bot
                0xFFFFFFFF, // Layer 3 rim highlight
                0xFFF5F6F9, // Layer 4 top
                0xFFE1E4EC, // Layer 4 bot
                0xFFFFFFFF, // Layer 4 rim highlight
                0xFF8C92A0,
            ),
            ThemeMode::Dark => (
                0xFF070709, // Obsidian Carbon TL
                0xFF131318, // TR
                0xFF040406, // BL
                0xFF0E0E12, // BR
                0xFF101014, // Layer 1 top
                0xFF08080B, // Layer 1 bot
                0xFF3E3E48, // Layer 1 rim sheen
                0xFF16161C, // Layer 2 top
                0xFF0B0B0E, // Layer 2 bot
                0xFF52525E, // Layer 2 rim sheen
                0xFF1A1A21, // Layer 3 top
                0xFF0C0C10, // Layer 3 bot
                0xFF5E5E6A, // Layer 3 rim sheen
                0xFF131318, // Layer 4 top
                0xFF09090C, // Layer 4 bot
                0xFF484852, // Layer 4 rim sheen
                0xFF020203,
            ),
        }
    } else {
        match mode {
            ThemeMode::Light => (
                0xFF9E876C, // base TL warm bronze-sand
                0xFFD9CDBF, // base TR soft ivory
                0xFF5D534A, // base BL deep warm taupe
                0xFF8C97A8, // base BR cool slate
                0xFFC8B397, // Layer 1 top
                0xFF7B6753, // Layer 1 bot
                0xFFF3ECE2, // Layer 1 rim highlight
                0xFFE2D8CB, // Layer 2 top
                0xFF9C8E82, // Layer 2 bot
                0xFFFAF6F0, // Layer 2 rim highlight
                0xFF4F556B, // Layer 3 top
                0xFF839AB5, // Layer 3 bot
                0xFFEBEFF5, // Layer 3 rim highlight
                0xFFC4CCD8, // Layer 4 top
                0xFF7D93AE, // Layer 4 bot
                0xFFF8FAFD, // Layer 4 rim highlight
                0xFF1E222C,
            ),
            ThemeMode::Dark => (
                0xFF181E2C, // Dusk Silk Dunes TL
                0xFF2C2B3B, // TR
                0xFF0F131D, // BL
                0xFF241E2A, // BR
                0xFF262D40, // Layer 1 top
                0xFF141824, // Layer 1 bot
                0xFF7B8AA8, // Layer 1 rim
                0xFF383242, // Layer 2 top
                0xFF1C1924, // Layer 2 bot
                0xFFC29B80, // Layer 2 rim warm sand sheen
                0xFF2B3245, // Layer 3 top
                0xFF171C29, // Layer 3 bot
                0xFFAE8E7A, // Layer 3 rim
                0xFF202536, // Layer 4 top
                0xFF121520, // Layer 4 bot
                0xFF7C92B4, // Layer 4 rim
                0xFF05060A,
            ),
        }
    };

    for py in 0..height {
        let py64 = py as i64;
        let ty_q8 = ((py64 * 255) / h) as u8;
        let t_mid = (py64 * 4096 + 2048) / h;
        let t_prev = ((py64 * 4096) - 2048) / h;
        let t_next = ((py64 * 4096) + 6144) / h;

        let folds = eval_folds_q12(t_mid, w);
        let f_prev = eval_folds_q12(t_prev, w);
        let f_next = eval_folds_q12(t_next, w);

        let mut hyp = [4096i64; 4];
        for i in 0..4 {
            let dx_dy = (f_next[i] - f_prev[i]) >> 1;
            let len2 = (4096u64 * 4096u64) + (dx_dy.unsigned_abs() * dx_dy.unsigned_abs());
            hyp[i] = isqrt_u64(len2) as i64;
        }

        let base_l = blend_u32(base_tl, base_bl, ty_q8);
        let base_r = blend_u32(base_tr, base_br, ty_q8);

        let c1 = blend_u32(l1_top, l1_bot, ty_q8);
        let c2 = blend_u32(l2_top, l2_bot, ty_q8);
        let c3 = blend_u32(l3_top, l3_bot, ty_q8);
        let c4 = blend_u32(l4_top, l4_bot, ty_q8);

        let row = py * width;
        for px in 0..width {
            let px64 = px as i64;
            let px_q12 = (px64 << 12) + 2048;
            let tx_q8 = ((px64 * 255) / w) as u8;
            let mut color = blend_u32(base_l, base_r, tx_q8);

            let sheen_base = ((1024 - (px64 * 1024) / w) * 18) >> 10;
            if sheen_base > 0 {
                color = blend_u32(color, l1_rim, sheen_base as u8);
            }

            color = apply_fold_layer(
                color,
                folds[0] - px_q12,
                hyp[0],
                w,
                c1,
                l1_bot,
                l1_rim,
                shadow_col,
                40,
                52,
                34,
                28,
                52,
            );
            color = apply_fold_layer(
                color,
                px_q12 - folds[1],
                hyp[1],
                w,
                c2,
                l2_bot,
                l2_rim,
                shadow_col,
                44,
                60,
                50,
                26,
                58,
            );
            color = apply_fold_layer(
                color,
                px_q12 - folds[2],
                hyp[2],
                w,
                c3,
                l3_bot,
                l3_rim,
                shadow_col,
                48,
                66,
                55,
                24,
                54,
            );
            color = apply_fold_layer(
                color,
                px_q12 - folds[3],
                hyp[3],
                w,
                c4,
                l4_bot,
                l4_rim,
                shadow_col,
                42,
                50,
                36,
                20,
                62,
            );

            buf[row + px] = apply_bayer_dither(color, px, py);
        }
    }
}

fn fill_aurora_wallpaper(buf: &mut [u32], width: usize, height: usize) {
    let w = width.max(1) as i64;
    let h = height.max(1) as i64;

    for py in 0..height {
        let py64 = py as i64;
        let ty_q8 = ((py64 * 255) / h) as u8;
        let t_y = (py64 * 4096) / h;
        let y_env1 = bell_q12(((t_y - 320) * 4096) / 2900);
        let y_env2 = bell_q12(((t_y - 680) * 4096) / 2600);

        let sky_l = blend_u32(0xFF040A16, 0xFF0A1D2D, ty_q8);
        let sky_r = blend_u32(0xFF081426, 0xFF0D2B3A, ty_q8);
        let row = py * width;

        for px in 0..width {
            let px64 = px as i64;
            let tx_q8 = ((px64 * 255) / w) as u8;
            let t_x = (px64 * 4096) / w;
            let mut color = blend_u32(sky_l, sky_r, tx_q8);

            // Curtain 1: Emerald-Teal smooth C2 aurora ribbon
            let ridge1 = 1520 + (smooth_q12(t_x) * 1400 - 700) / 4096 + (wave_q12(t_x) * 650) / 4096;
            let d1 = (t_y - ridge1).abs();
            if d1 < 950 {
                let w1 = smooth_q12(((950 - d1) * 4096) / 950);
                let shimmer1 = 2800 + (bell_q12((t_x * 2) & 4095) * 1296) / 4096;
                let str1 = (((w1 * y_env1) >> 12) * shimmer1) >> 12;
                let a1 = ((str1 * 185) >> 12).clamp(0, 210) as u8;
                if a1 > 0 {
                    let grad1 = blend_u32(0xFF1EE8A8, 0xFF1696C8, ((d1 * 255) / 950).clamp(0, 255) as u8);
                    color = blend_u32(color, grad1, a1);
                }
            }

            // Curtain 2: Upper Cyan-Teal aurora veil
            let ridge2 = 1120 - (wave_q12(4096 - t_x) * 720) / 4096 + (bell_q12(t_x) * 480) / 4096;
            let d2 = (t_y - ridge2).abs();
            if d2 < 820 {
                let w2 = smooth_q12(((820 - d2) * 4096) / 820);
                let str2 = (w2 * y_env2) >> 12;
                let a2 = ((str2 * 135) >> 12).clamp(0, 165) as u8;
                if a2 > 0 {
                    let grad2 = blend_u32(0xFF38E2D4, 0xFF2B88C8, tx_q8);
                    color = blend_u32(color, grad2, a2);
                }
            }

            buf[row + px] = apply_bayer_dither(color, px, py);
        }
    }
}

fn fill_sunset_wallpaper(buf: &mut [u32], width: usize, height: usize) {
    let w = width.max(1) as i64;
    let h = height.max(1) as i64;
    let sun_cx = (w * 58) / 100;
    let sun_cy = (h * 56) / 100;
    let sun_r2 = ((w * 36) / 100).max(1) * ((w * 36) / 100).max(1);

    for py in 0..height {
        let py64 = py as i64;
        let t_y = (py64 * 4096) / h;
        let sky_col = if t_y < 1500 {
            let t = ((t_y * 255) / 1500).clamp(0, 255) as u8;
            blend_u32(0xFF140E18, 0xFF5A2538, t)
        } else if t_y < 2900 {
            let t = (((t_y - 1500) * 255) / 1400).clamp(0, 255) as u8;
            blend_u32(0xFF5A2538, 0xFFD05B52, t)
        } else {
            let t = (((t_y - 2900) * 255) / 1196).clamp(0, 255) as u8;
            blend_u32(0xFFD05B52, 0xFFF3A762, t)
        };
        let dy = py64 - sun_cy;
        let dy2 = dy * dy;
        let row = py * width;

        for px in 0..width {
            let px64 = px as i64;
            let t_x = (px64 * 4096) / w;
            let mut color = sky_col;

            // Warm radial sunset glow
            let dx = px64 - sun_cx;
            let dist2 = dx * dx + dy2;
            if dist2 < sun_r2 {
                let norm = 256 - ((dist2 * 256) / sun_r2).clamp(0, 256);
                let ga = (((norm * norm) >> 8) * 110 >> 8).min(125) as u8;
                if ga > 0 {
                    color = blend_u32(color, 0xFFFFCC80, ga);
                }
            }

            // Smooth twilight horizon dune silhouettes
            let h1 = 2780 + (smooth_q12(t_x) * 420) / 4096 - (bell_q12(t_x) * 260) / 4096;
            if t_y > h1 {
                let cov = (((t_y - h1) * 255) / 18).clamp(0, 255) as u8;
                let depth = (((t_y - h1) * 220) / 1300).clamp(0, 220) as u8;
                let dune1 = blend_u32(0xFF3A1C28, 0xFF1A1016, depth);
                color = blend_u32(color, dune1, cov);
            }
            let h2 = 3240 - (wave_q12(t_x) * 340) / 4096 + (smooth_q12(4096 - t_x) * 220) / 4096;
            if t_y > h2 {
                let cov = (((t_y - h2) * 255) / 18).clamp(0, 255) as u8;
                let depth = (((t_y - h2) * 220) / 900).clamp(0, 220) as u8;
                let dune2 = blend_u32(0xFF24121A, 0xFF10090D, depth);
                color = blend_u32(color, dune2, cov);
            }

            buf[row + px] = apply_bayer_dither(color, px, py);
        }
    }
}

fn fill_midnight_minimal_wallpaper(buf: &mut [u32], width: usize, height: usize) {
    let w = width.max(1) as i64;
    let h = height.max(1) as i64;

    for py in 0..height {
        let py64 = py as i64;
        let ty_q8 = ((py64 * 255) / h) as u8;
        let t_mid = (py64 * 4096 + 2048) / h;
        let t_prev = ((py64 * 4096) - 2048) / h;
        let t_next = ((py64 * 4096) + 6144) / h;

        let folds = eval_folds_q12(t_mid, w);
        let f_prev = eval_folds_q12(t_prev, w);
        let f_next = eval_folds_q12(t_next, w);

        let mut hyp = [4096i64; 4];
        for i in 0..4 {
            let dx_dy = (f_next[i] - f_prev[i]) >> 1;
            let len2 = (4096u64 * 4096u64) + (dx_dy.unsigned_abs() * dx_dy.unsigned_abs());
            hyp[i] = isqrt_u64(len2) as i64;
        }

        let base_l = blend_u32(0xFF0C0F16, 0xFF080A0F, ty_q8);
        let base_r = blend_u32(0xFF171C28, 0xFF10141D, ty_q8);
        let c1 = blend_u32(0xFF1C2230, 0xFF0F131B, ty_q8);
        let c3 = blend_u32(0xFF212838, 0xFF121620, ty_q8);

        let row = py * width;
        for px in 0..width {
            let px64 = px as i64;
            let px_q12 = (px64 << 12) + 2048;
            let tx_q8 = ((px64 * 255) / w) as u8;
            let mut color = blend_u32(base_l, base_r, tx_q8);

            color = apply_fold_layer(
                color,
                folds[0] - px_q12,
                hyp[0],
                w,
                c1,
                0xFF0E121A,
                0xFF424E66,
                0xFF040508,
                34,
                32,
                38,
                22,
                52,
            );
            color = apply_fold_layer(
                color,
                px_q12 - folds[2],
                hyp[2],
                w,
                c3,
                0xFF11151E,
                0xFF4F5D78,
                0xFF040508,
                38,
                36,
                46,
                24,
                56,
            );

            buf[row + px] = apply_bayer_dither(color, px, py);
        }
    }
}

fn check_heap_headroom(bytes_needed: usize) -> Result<(), &'static str> {
    let free = crate::memory::heap::GLOBAL_ALLOCATOR.free_bytes();
    let largest = crate::memory::heap::GLOBAL_ALLOCATOR.largest_free_block();
    if largest < bytes_needed.saturating_add(1024 * 1024)
        || free < bytes_needed.saturating_add(2 * 1024 * 1024)
    {
        Err("Insufficient kernel heap for image")
    } else {
        Ok(())
    }
}

/// Decodes an uncompressed BMP (24/32-bit BI_RGB), TGA (24/32-bit), or PPM (P6) image from VFS.
pub fn decode_image_from_vfs(path: &str) -> Result<(usize, usize, Vec<u32>), &'static str> {
    let stat = vfs::with_vfs(|s| s.stat(path)).map_err(|_| "Cannot read image file")?;
    if stat.size > 6 * 1024 * 1024 {
        return Err("Image file exceeds 6 MiB limit");
    }
    check_heap_headroom(stat.size as usize)?;
    let bytes = vfs::with_vfs(|s| s.read_file(path)).map_err(|_| "Cannot read image file")?;
    if bytes.len() > 6 * 1024 * 1024 {
        return Err("Image file exceeds 6 MiB limit");
    }
    if bytes.len() >= 2 && bytes[0] == b'B' && bytes[1] == b'M' {
        decode_bmp(&bytes)
    } else if bytes.len() >= 3 && bytes[0] == b'P' && bytes[1] == b'6' {
        decode_ppm_p6(&bytes)
    } else if bytes.len() >= 18 && (bytes[2] == 2 || bytes[2] == 10) {
        decode_tga(&bytes)
    } else {
        Err("Unsupported format (supports BMP, TGA, PPM)")
    }
}

fn decode_bmp(data: &[u8]) -> Result<(usize, usize, Vec<u32>), &'static str> {
    if data.len() < 54 {
        return Err("Invalid BMP header");
    }
    let px_offset = u32::from_le_bytes([data[10], data[11], data[12], data[13]]) as usize;
    let dib_size = u32::from_le_bytes([data[14], data[15], data[16], data[17]]);
    if dib_size < 40 {
        return Err("Unsupported BMP DIB header");
    }
    let w_i32 = i32::from_le_bytes([data[18], data[19], data[20], data[21]]);
    let h_i32 = i32::from_le_bytes([data[22], data[23], data[24], data[25]]);
    let planes = u16::from_le_bytes([data[26], data[27]]);
    let bpp = u16::from_le_bytes([data[28], data[29]]);
    let comp = u32::from_le_bytes([data[30], data[31], data[32], data[33]]);

    if planes != 1 || comp != 0 || (bpp != 24 && bpp != 32) {
        return Err("Only uncompressed 24/32-bit BMP supported");
    }
    if w_i32 <= 0 || h_i32 == 0 || w_i32 > 2560 || h_i32.abs() > 1600 {
        return Err("BMP dimensions out of range");
    }
    let w = w_i32 as usize;
    let h = h_i32.unsigned_abs() as usize;
    let px_count = w.checked_mul(h).ok_or("BMP overflow")?;
    let top_down = h_i32 < 0;
    let bytes_per_px = (bpp / 8) as usize;
    let row_stride = ((w * bytes_per_px) + 3) & !3;
    if px_offset + row_stride.saturating_mul(h) > data.len() {
        return Err("Truncated BMP pixel data");
    }
    check_heap_headroom(px_count.saturating_mul(4))?;

    let mut out = vec![0xFF00_0000u32; px_count];
    for sy in 0..h {
        let src_row = if top_down { sy } else { h - 1 - sy };
        let base = px_offset + src_row * row_stride;
        let drow = sy * w;
        for sx in 0..w {
            let p = base + sx * bytes_per_px;
            let b = data[p] as u32;
            let g = data[p + 1] as u32;
            let r = data[p + 2] as u32;
            out[drow + sx] = 0xFF00_0000 | (r << 16) | (g << 8) | b;
        }
    }
    Ok((w, h, out))
}

fn decode_tga(data: &[u8]) -> Result<(usize, usize, Vec<u32>), &'static str> {
    if data.len() < 18 {
        return Err("Invalid TGA header");
    }
    let id_len = data[0] as usize;
    let color_map_type = data[1];
    let img_type = data[2];
    if color_map_type != 0 || img_type != 2 {
        return Err("Only uncompressed true-color TGA supported");
    }
    let w = u16::from_le_bytes([data[12], data[13]]) as usize;
    let h = u16::from_le_bytes([data[14], data[15]]) as usize;
    let bpp = data[16];
    let desc = data[17];
    let top_origin = (desc & 0x20) != 0;
    if (bpp != 24 && bpp != 32) || w == 0 || h == 0 || w > 2560 || h > 1600 {
        return Err("Unsupported TGA geometry or depth");
    }
    let px_count = w.checked_mul(h).ok_or("TGA overflow")?;
    let step = (bpp / 8) as usize;
    let px_start = 18 + id_len;
    if px_start + px_count * step > data.len() {
        return Err("Truncated TGA image");
    }
    check_heap_headroom(px_count.saturating_mul(4))?;

    let mut out = vec![0xFF00_0000u32; px_count];
    for sy in 0..h {
        let src_row = if top_origin { sy } else { h - 1 - sy };
        let srow = px_start + src_row * w * step;
        let drow = sy * w;
        for sx in 0..w {
            let p = srow + sx * step;
            let b = data[p] as u32;
            let g = data[p + 1] as u32;
            let r = data[p + 2] as u32;
            out[drow + sx] = 0xFF00_0000 | (r << 16) | (g << 8) | b;
        }
    }
    Ok((w, h, out))
}

fn decode_ppm_p6(data: &[u8]) -> Result<(usize, usize, Vec<u32>), &'static str> {
    let mut pos = 2usize;
    let mut tokens = [0usize; 3];
    let mut tok_idx = 0usize;

    while tok_idx < 3 && pos < data.len() {
        while pos < data.len() && data[pos].is_ascii_whitespace() {
            pos += 1;
        }
        if pos < data.len() && data[pos] == b'#' {
            while pos < data.len() && data[pos] != b'\n' {
                pos += 1;
            }
            continue;
        }
        let start = pos;
        while pos < data.len() && data[pos].is_ascii_digit() {
            pos += 1;
        }
        if start == pos {
            return Err("Invalid PPM header");
        }
        let s = core::str::from_utf8(&data[start..pos]).map_err(|_| "Bad PPM header")?;
        tokens[tok_idx] = s.parse::<usize>().map_err(|_| "Bad PPM number")?;
        tok_idx += 1;
    }

    if tok_idx < 3 || pos >= data.len() {
        return Err("Incomplete PPM header");
    }
    if data[pos].is_ascii_whitespace() {
        pos += 1;
    }
    let (w, h, maxv) = (tokens[0], tokens[1], tokens[2]);
    if w == 0 || h == 0 || w > 2560 || h > 1600 || maxv != 255 {
        return Err("PPM dimensions or maxval != 255 unsupported");
    }
    let px_count = w.checked_mul(h).ok_or("PPM overflow")?;
    if pos + px_count * 3 > data.len() {
        return Err("Truncated PPM pixels");
    }
    check_heap_headroom(px_count.saturating_mul(4))?;

    let mut out = vec![0xFF00_0000u32; px_count];
    for i in 0..px_count {
        let p = pos + i * 3;
        let r = data[p] as u32;
        let g = data[p + 1] as u32;
        let b = data[p + 2] as u32;
        out[i] = 0xFF00_0000 | (r << 16) | (g << 8) | b;
    }
    Ok((w, h, out))
}

#[inline(always)]
fn sample_bilinear_u32(src: &[u32], sw: usize, sh: usize, sx_q16: i64, sy_q16: i64) -> u32 {
    let x_max = (sw - 1) as i64;
    let y_max = (sh - 1) as i64;
    let cx = sx_q16.clamp(0, x_max << 16);
    let cy = sy_q16.clamp(0, y_max << 16);
    let x0 = (cx >> 16) as usize;
    let y0 = (cy >> 16) as usize;
    let x1 = (x0 + 1).min(sw - 1);
    let y1 = (y0 + 1).min(sh - 1);
    let fx = ((cx & 0xFFFF) >> 8) as u32;
    let fy = ((cy & 0xFFFF) >> 8) as u32;

    let p00 = src[y0 * sw + x0];
    let p10 = src[y0 * sw + x1];
    let p01 = src[y1 * sw + x0];
    let p11 = src[y1 * sw + x1];

    let w00 = (256 - fx) * (256 - fy);
    let w10 = fx * (256 - fy);
    let w01 = (256 - fx) * fy;
    let w11 = fx * fy;

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
    0xFF00_0000 | (r << 16) | (g << 8) | b
}

fn scale_image_wallpaper_into(
    out: &mut [u32],
    dst_w: usize,
    dst_h: usize,
    src_w: usize,
    src_h: usize,
    src: &[u32],
    mode: WallpaperScaleMode,
    backdrop: u32,
) {
    let needed = dst_w.saturating_mul(dst_h);
    if out.len() < needed {
        return;
    }
    let bg = backdrop | 0xFF00_0000;
    if matches!(mode, WallpaperScaleMode::Fit | WallpaperScaleMode::Center)
        || src_w == 0
        || src_h == 0
        || src.len() < src_w * src_h
    {
        out[..needed].fill(bg);
    }
    if src_w == 0 || src_h == 0 || src.len() < src_w * src_h {
        return;
    }

    let dw = dst_w as i64;
    let dh = dst_h as i64;
    let sw = src_w as i64;
    let sh = src_h as i64;

    match mode {
        WallpaperScaleMode::Stretch => {
            for py in 0..dst_h {
                let sy_q16 = ((py as i64) * (sh << 16)) / dh;
                let row = py * dst_w;
                for px in 0..dst_w {
                    let sx_q16 = ((px as i64) * (sw << 16)) / dw;
                    out[row + px] = sample_bilinear_u32(src, src_w, src_h, sx_q16, sy_q16);
                }
            }
        }
        WallpaperScaleMode::Fill => {
            // Preserve aspect ratio covering full screen
            let (crop_w_q16, crop_h_q16, off_x_q16, off_y_q16) =
                if sw * dh > dw * sh {
                    let cw = (dw * sh << 16) / dh;
                    (cw, sh << 16, ((sw << 16) - cw) / 2, 0)
                } else {
                    let ch = (dh * sw << 16) / dw;
                    (sw << 16, ch, 0, ((sh << 16) - ch) / 2)
                };
            for py in 0..dst_h {
                let sy_q16 = off_y_q16 + ((py as i64) * crop_h_q16) / dh;
                let row = py * dst_w;
                for px in 0..dst_w {
                    let sx_q16 = off_x_q16 + ((px as i64) * crop_w_q16) / dw;
                    out[row + px] = sample_bilinear_u32(src, src_w, src_h, sx_q16, sy_q16);
                }
            }
        }
        WallpaperScaleMode::Fit => {
            let (fit_w, fit_h) = if sw * dh > dw * sh {
                (dst_w, ((dst_w as i64 * sh) / sw).max(1) as usize)
            } else {
                (((dst_h as i64 * sw) / sh).max(1) as usize, dst_h)
            };
            let ox = (dst_w.saturating_sub(fit_w)) / 2;
            let oy = (dst_h.saturating_sub(fit_h)) / 2;
            for ry in 0..fit_h {
                let sy_q16 = ((ry as i64) * (sh << 16)) / (fit_h as i64);
                let row = (oy + ry) * dst_w + ox;
                for rx in 0..fit_w {
                    let sx_q16 = ((rx as i64) * (sw << 16)) / (fit_w as i64);
                    out[row + rx] = sample_bilinear_u32(src, src_w, src_h, sx_q16, sy_q16);
                }
            }
        }
        WallpaperScaleMode::Center => {
            let ox = (dst_w as i32 - src_w as i32) / 2;
            let oy = (dst_h as i32 - src_h as i32) / 2;
            for sy in 0..src_h {
                let dy = oy + sy as i32;
                if dy < 0 || dy >= dst_h as i32 {
                    continue;
                }
                let srow = sy * src_w;
                let drow = (dy as usize) * dst_w;
                for sx in 0..src_w {
                    let dx = ox + sx as i32;
                    if dx >= 0 && dx < dst_w as i32 {
                        out[drow + dx as usize] = src[srow + sx];
                    }
                }
            }
        }
    }
}

