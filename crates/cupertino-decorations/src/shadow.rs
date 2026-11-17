//! Window shadows: parameters from the verified table, a CPU rasteriser and GPU parameters.
//!
//! the parameter table (density, sigma, vertical offset, clip-top, rim terms) is as designed
//! and lives in `cupertino_theme::window_shadow`; the sigma-to-pixel-radius rule
//! `ceil(σ · 2.8)` is as designed. How the design window server uses `clip_top` is unknown: it is
//! carried through [`ShadowParams`] and [`ShadowGpuParams`] but not applied by the CPU rasteriser.
//! The shadow is a Gaussian blur of the window shape mask shifted down by the vertical offset and
//! scaled by the density (structure); the blur is a three-pass box blur
//! (approximate, within about 3 % of a true Gaussian).
//!
//! [`ShadowRasterizer`] renders one small image per parameter set and stretches it with a nine-slice
//! mapping to any frame size; the result equals a direct render (tested).

use cupertino_geometry::shadow::box_blur_sizes;
use cupertino_geometry::{CornerStyle, Radii, Rect, RoundedRect, Size};
use cupertino_theme::window_shadow::{
    sigma_to_radius_px, window_shadow, ShadowOption, WindowShadowEntry,
};
use std::collections::HashMap;
use std::sync::Arc;
use tiny_skia::{FillRule, Paint, Pixmap, Transform};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShadowParams {
    pub entry: WindowShadowEntry,
}

/// Pixel margins around the frame that the shadow occupies.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Margins {
    pub left: u32,
    pub top: u32,
    pub right: u32,
    pub bottom: u32,
}

impl ShadowParams {
    pub fn new(entry: WindowShadowEntry) -> Self {
        Self { entry }
    }

    /// Row of the verified table.
    pub fn lookup(
        option: ShadowOption,
        active: bool,
        dark: bool,
        increased_contrast: bool,
    ) -> Self {
        Self::new(*window_shadow(option, active, dark, increased_contrast))
    }

    pub fn density(&self) -> f64 {
        self.entry.density
    }

    /// Gaussian sigma in points.
    pub fn sigma(&self) -> f64 {
        self.entry.sigma
    }

    pub fn vertical_offset(&self) -> f64 {
        self.entry.vertical_offset
    }

    /// Sigma in device pixels.
    pub fn sigma_px(&self, scale: f64) -> f64 {
        self.entry.sigma * scale
    }

    /// Blur radius in device pixels: `ceil(σ_px · 2.8).
    pub fn radius_px(&self, scale: f64) -> u32 {
        sigma_to_radius_px(self.sigma_px(scale))
    }

    pub fn offset_px(&self, scale: f64) -> u32 {
        (self.entry.vertical_offset * scale).round().max(0.0) as u32
    }

    pub fn margins_px(&self, scale: f64) -> Margins {
        let (r, o) = (self.radius_px(scale), self.offset_px(scale));
        Margins {
            left: r,
            top: r.saturating_sub(o),
            right: r,
            bottom: r + o,
        }
    }

    /// Parameters for a shader-based path (everything in device pixels).
    pub fn gpu(&self, frame_size_pt: Size, corner_radius_pt: f64, scale: f64) -> ShadowGpuParams {
        let m = self.margins_px(scale);
        ShadowGpuParams {
            sigma_px: self.sigma_px(scale) as f32,
            radius_px: self.radius_px(scale) as f32,
            density: self.entry.density as f32,
            clip_top: self.entry.clip_top as f32,
            offset_px: [0.0, self.offset_px(scale) as f32],
            corner_radius_px: (corner_radius_pt * scale) as f32,
            rect_size_px: [
                (frame_size_pt.width * scale).round() as f32,
                (frame_size_pt.height * scale).round() as f32,
            ],
            margins_px: [m.left as f32, m.top as f32, m.right as f32, m.bottom as f32],
        }
    }
}

/// Plain-old-data parameters of the shadow pass, in device pixels. `to_array` gives a flat layout
/// for a uniform buffer: sigma, radius, density, clip_top, offset x/y, corner radius, padding,
/// rect size x/y, padding ×2, margins l/t/r/b.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShadowGpuParams {
    pub sigma_px: f32,
    pub radius_px: f32,
    pub density: f32,
    pub clip_top: f32,
    pub offset_px: [f32; 2],
    pub corner_radius_px: f32,
    pub rect_size_px: [f32; 2],
    pub margins_px: [f32; 4],
}

impl ShadowGpuParams {
    pub fn to_array(&self) -> [f32; 16] {
        [
            self.sigma_px,
            self.radius_px,
            self.density,
            self.clip_top,
            self.offset_px[0],
            self.offset_px[1],
            self.corner_radius_px,
            0.0,
            self.rect_size_px[0],
            self.rect_size_px[1],
            0.0,
            0.0,
            self.margins_px[0],
            self.margins_px[1],
            self.margins_px[2],
            self.margins_px[3],
        ]
    }
}

/// Premultiplied RGBA8 shadow image covering the frame plus margins.
#[derive(Clone, Debug, PartialEq)]
pub struct ShadowImage {
    pub width: u32,
    pub height: u32,
    pub margins: Margins,
    pub data: Vec<u8>,
}

impl ShadowImage {
    pub fn alpha(&self, x: u32, y: u32) -> u8 {
        self.data[((y * self.width + x) * 4 + 3) as usize]
    }

    /// Position of the frame's top-left corner in the image.
    pub fn frame_origin(&self) -> (u32, u32) {
        (self.margins.left, self.margins.top)
    }

    /// Remove the shadow beneath the window shape so translucent windows do not show it.
    pub fn knock_out(&mut self, frame_px: (u32, u32), corner_radius_px: f64, style: CornerStyle) {
        let shape = RoundedRect::new(
            Rect::new(
                self.margins.left as f64,
                self.margins.top as f64,
                frame_px.0 as f64,
                frame_px.1 as f64,
            ),
            corner_radius_px,
            style,
        );
        let x1 = (self.margins.left + frame_px.0).min(self.width);
        let y1 = (self.margins.top + frame_px.1).min(self.height);
        for y in self.margins.top..y1 {
            for x in self.margins.left..x1 {
                let cov = shape.coverage_approx(
                    cupertino_geometry::Point::new(x as f64 + 0.5, y as f64 + 0.5),
                    1.0,
                );
                if cov > 0.0 {
                    let i = ((y * self.width + x) * 4) as usize;
                    for c in &mut self.data[i..i + 4] {
                        *c = (*c as f64 * (1.0 - cov)).round() as u8;
                    }
                }
            }
        }
    }
}

fn box_blur_axis(src: &[f32], dst: &mut [f32], w: usize, h: usize, size: usize, horizontal: bool) {
    let r = (size / 2) as i64;
    let inv = 1.0 / size as f32;
    let (len, lines) = if horizontal { (w, h) } else { (h, w) };
    let idx = |line: usize, i: usize| {
        if horizontal {
            line * w + i
        } else {
            i * w + line
        }
    };
    for line in 0..lines {
        let mut acc = 0.0f32;
        for i in 0..=(r.min(len as i64 - 1) as usize) {
            acc += src[idx(line, i)];
        }
        for i in 0..len {
            dst[idx(line, i)] = acc * inv;
            let add = i as i64 + r + 1;
            let sub = i as i64 - r;
            if add < len as i64 {
                acc += src[idx(line, add as usize)];
            }
            if sub >= 0 {
                acc -= src[idx(line, sub as usize)];
            }
        }
    }
}

fn gaussian_blur(mask: &mut [f32], w: usize, h: usize, sigma: f64) {
    if sigma < 0.3 {
        return;
    }
    let mut tmp = vec![0.0f32; mask.len()];
    for size in box_blur_sizes(sigma, 3) {
        box_blur_axis(mask, &mut tmp, w, h, size, true);
        box_blur_axis(&tmp, mask, w, h, size, false);
    }
}

fn frame_px(size: Size, scale: f64) -> (u32, u32) {
    (
        (size.width * scale).round().max(1.0) as u32,
        (size.height * scale).round().max(1.0) as u32,
    )
}

/// Render the shadow of a frame of `frame_size_pt` directly (no caching, no nine-slice).
pub fn render_direct(
    params: &ShadowParams,
    frame_size_pt: Size,
    corner_radius_pt: f64,
    style: CornerStyle,
    scale: f64,
) -> ShadowImage {
    let m = params.margins_px(scale);
    let (fw, fh) = frame_px(frame_size_pt, scale);
    let (w, h) = (fw + m.left + m.right, fh + m.top + m.bottom);
    let mut pm = Pixmap::new(w, h).expect("non-zero size");
    let off = params.offset_px(scale);
    let r = corner_radius_pt * scale;
    let rect = Rect::new(m.left as f64, (m.top + off) as f64, fw as f64, fh as f64);
    if let Some(path) = crate::shape::rounded_path(rect, Radii::uniform(r), style, 1.0) {
        let mut paint = Paint {
            anti_alias: true,
            ..Paint::default()
        };
        paint.set_color(tiny_skia::Color::BLACK);
        pm.fill_path(
            &path,
            &paint,
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
    let mut mask: Vec<f32> = pm
        .pixels()
        .iter()
        .map(|p| p.alpha() as f32 / 255.0)
        .collect();
    gaussian_blur(&mut mask, w as usize, h as usize, params.sigma_px(scale));
    let density = params.density() as f32;
    let mut data = vec![0u8; (w * h * 4) as usize];
    for (i, a) in mask.iter().enumerate() {
        data[i * 4 + 3] = (a * density * 255.0).round().clamp(0.0, 255.0) as u8;
    }
    ShadowImage {
        width: w,
        height: h,
        margins: m,
        data,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct Key {
    sigma: u64,
    offset: u64,
    density: u64,
    scale: u64,
    corner: u64,
    expansion: u64,
}

struct Mini {
    image: ShadowImage,
    /// Distances in pixels from the image edges to the stretchable zone: left, top, right, bottom.
    insets: [u32; 4],
}

/// Shadow renderer with a cache of nine-slice sources keyed by the parameter set.
#[derive(Default)]
pub struct ShadowRasterizer {
    cache: HashMap<Key, Arc<Mini>>,
}

impl ShadowRasterizer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cached_sources(&self) -> usize {
        self.cache.len()
    }

    fn mini(
        &mut self,
        params: &ShadowParams,
        corner_radius_pt: f64,
        style: CornerStyle,
        scale: f64,
    ) -> Arc<Mini> {
        let key = Key {
            sigma: params.sigma().to_bits(),
            offset: params.vertical_offset().to_bits(),
            density: params.density().to_bits(),
            scale: scale.to_bits(),
            corner: corner_radius_pt.to_bits(),
            expansion: style.expansion().to_bits(),
        };
        self.cache
            .entry(key)
            .or_insert_with(|| {
                let m = params.margins_px(scale);
                let off = params.offset_px(scale);
                let ce = (corner_radius_pt * scale * style.expansion()).ceil() as u32;
                let r = params.radius_px(scale);
                let c = ce + r + 2;
                let insets = [m.left + c, m.top + off + c, m.right + c, r + c];
                let (cw, ch) = (insets[0] + insets[2] + 3, insets[1] + insets[3] + 3);
                let fw = cw - m.left - m.right;
                let fh = ch - m.top - m.bottom;
                let size = Size::new(fw as f64 / scale, fh as f64 / scale);
                let image = render_direct(params, size, corner_radius_pt, style, scale);
                debug_assert_eq!((image.width, image.height), (cw, ch));
                Arc::new(Mini { image, insets })
            })
            .clone()
    }

    /// Shadow image for a frame of `frame_size_pt`, built by nine-slice stretching of a cached
    /// source (falls back to a direct render for frames smaller than the source).
    pub fn render(
        &mut self,
        params: &ShadowParams,
        frame_size_pt: Size,
        corner_radius_pt: f64,
        style: CornerStyle,
        scale: f64,
    ) -> ShadowImage {
        let m = params.margins_px(scale);
        let (fw, fh) = frame_px(frame_size_pt, scale);
        let (w, h) = (fw + m.left + m.right, fh + m.top + m.bottom);
        let mini = self.mini(params, corner_radius_pt, style, scale);
        let [il, it, ir, ib] = mini.insets;
        if w < il + ir + 3 || h < it + ib + 3 {
            return render_direct(params, frame_size_pt, corner_radius_pt, style, scale);
        }
        let src = &mini.image;
        let map = |t: u32, total: u32, lo: u32, hi: u32, src_total: u32| -> u32 {
            if t < lo {
                t
            } else if t >= total - hi {
                src_total - (total - t)
            } else {
                lo + (src_total - lo - hi) / 2
            }
        };
        let xs: Vec<u32> = (0..w).map(|x| map(x, w, il, ir, src.width)).collect();
        let mut data = vec![0u8; (w * h * 4) as usize];
        for y in 0..h {
            let sy = map(y, h, it, ib, src.height);
            let row = &src.data[(sy * src.width * 4) as usize..][..(src.width * 4) as usize];
            for (x, &sx) in xs.iter().enumerate() {
                let o = (y as usize * w as usize + x) * 4;
                data[o..o + 4].copy_from_slice(&row[sx as usize * 4..][..4]);
            }
        }
        ShadowImage {
            width: w,
            height: h,
            margins: m,
            data,
        }
    }
}
