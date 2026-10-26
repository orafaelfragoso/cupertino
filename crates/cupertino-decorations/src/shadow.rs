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
use cupertino_theme::window_shadow::{sigma_to_radius_px, window_shadow, ShadowOption, WindowShadowEntry};
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
    pub fn lookup(option: ShadowOption, active: bool, dark: bool, increased_contrast: bool) -> Self {
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
        Margins { left: r, top: r.saturating_sub(o), right: r, bottom: r + o }
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
            rect_size_px: [(frame_size_pt.width * scale).round() as f32, (frame_size_pt.height * scale).round() as f32],
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
        let shape = RoundedRect::new(Rect::new(self.margins.left as f64, self.margins.top as f64, frame_px.0 as f64, frame_px.1 as f64), corner_radius_px, style);
        let x1 = (self.margins.left + frame_px.0).min(self.width);
        let y1 = (self.margins.top + frame_px.1).min(self.height);
        for y in self.margins.top..y1 {
            for x in self.margins.left..x1 {
                let cov = shape.coverage_approx(cupertino_geometry::Point::new(x as f64 + 0.5, y as f64 + 0.5), 1.0);
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
    let idx = |line: usize, i: usize| if horizontal { line * w + i } else { i * w + line };
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
    ((size.width * scale).round().max(1.0) as u32, (size.height * scale).round().max(1.0) as u32)
}

/// Render the shadow of a frame of `frame_size_pt` directly (no caching, no nine-slice).
pub fn render_direct(params: &ShadowParams, frame_size_pt: Size, corner_radius_pt: f64, style: CornerStyle, scale: f64) -> ShadowImage {
    let m = params.margins_px(scale);
    let (fw, fh) = frame_px(frame_size_pt, scale);
    let (w, h) = (fw + m.left + m.right, fh + m.top + m.bottom);
    let mut pm = Pixmap::new(w, h).expect("non-zero size");
    let off = params.offset_px(scale);
    let r = corner_radius_pt * scale;
    let rect = Rect::new(m.left as f64, (m.top + off) as f64, fw as f64, fh as f64);
    if let Some(path) = crate::shape::rounded_path(rect, Radii::uniform(r), style, 1.0) {
        let mut paint = Paint { anti_alias: true, ..Paint::default() };
        paint.set_color(tiny_skia::Color::BLACK);
        pm.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
    }
    let mut mask: Vec<f32> = pm.pixels().iter().map(|p| p.alpha() as f32 / 255.0).collect();
    gaussian_blur(&mut mask, w as usize, h as usize, params.sigma_px(scale));
    let density = params.density() as f32;
    let mut data = vec![0u8; (w * h * 4) as usize];
    for (i, a) in mask.iter().enumerate() {
        data[i * 4 + 3] = (a * density * 255.0).round().clamp(0.0, 255.0) as u8;
    }
    ShadowImage { width: w, height: h, margins: m, data }
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

    fn mini(&mut self, params: &ShadowParams, corner_radius_pt: f64, style: CornerStyle, scale: f64) -> Arc<Mini> {
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
    pub fn render(&mut self, params: &ShadowParams, frame_size_pt: Size, corner_radius_pt: f64, style: CornerStyle, scale: f64) -> ShadowImage {
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
        ShadowImage { width: w, height: h, margins: m, data }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn active_light() -> ShadowParams {
        ShadowParams::lookup(ShadowOption::Standard, true, false, false)
    }

    #[test]
    fn parameters_come_from_the_verified_table() {
        let a = active_light();
        assert_eq!((a.sigma(), a.vertical_offset(), a.density()), (20.0, 18.0, 0.40));
        let i = ShadowParams::lookup(ShadowOption::Standard, false, false, false);
        assert_eq!((i.sigma(), i.vertical_offset(), i.density()), (12.0, 8.0, 0.25));
        let d = ShadowParams::lookup(ShadowOption::Standard, true, true, false);
        assert_eq!(d.density(), 0.70);
    }

    #[test]
    fn radius_and_margins() {
        let a = active_light();
        assert_eq!(a.radius_px(1.0), 56);
        assert_eq!(a.radius_px(2.0), 112);
        assert_eq!(a.margins_px(1.0), Margins { left: 56, top: 38, right: 56, bottom: 74 });
        assert_eq!(a.margins_px(2.0), Margins { left: 112, top: 76, right: 112, bottom: 148 });
        let t = ShadowParams::lookup(ShadowOption::Tooltip, true, false, false);
        assert_eq!(t.margins_px(1.0), Margins { left: 12, top: 10, right: 12, bottom: 14 });
    }

    #[test]
    fn gpu_params_are_in_device_pixels() {
        let g = active_light().gpu(Size::new(800.0, 600.0), 12.0, 2.0);
        assert_eq!(g.sigma_px, 40.0);
        assert_eq!(g.radius_px, 112.0);
        assert_eq!(g.offset_px, [0.0, 36.0]);
        assert_eq!(g.corner_radius_px, 24.0);
        assert_eq!(g.rect_size_px, [1600.0, 1200.0]);
        assert_eq!(g.to_array().len(), 16);
        assert!((g.density - 0.4).abs() < 1e-6);
        assert_eq!(g.to_array()[12..], [112.0, 76.0, 112.0, 148.0]);
    }

    fn direct(p: &ShadowParams, w: f64, h: f64, scale: f64) -> ShadowImage {
        render_direct(p, Size::new(w, h), 12.0, CornerStyle::continuous(), scale)
    }

    #[test]
    fn image_size_is_frame_plus_margins() {
        let img = direct(&active_light(), 300.0, 200.0, 1.0);
        assert_eq!((img.width, img.height), (300 + 112, 200 + 112));
        assert_eq!(img.frame_origin(), (56, 38));
    }

    #[test]
    fn shadow_falls_off_monotonically_away_from_the_window() {
        let p = active_light();
        let img = direct(&p, 400.0, 300.0, 1.0);
        let (ox, oy) = img.frame_origin();
        let cx = ox + 200;
        let bottom = oy + 300;
        let column: Vec<u8> = (oy + 150..img.height).map(|y| img.alpha(cx, y)).collect();
        assert!(column.windows(2).all(|w| w[0] >= w[1]), "downwards from the centre: {column:?}");
        let row: Vec<u8> = (ox + 200..img.width).map(|x| img.alpha(x, oy + 150)).collect();
        assert!(row.windows(2).all(|w| w[0] >= w[1]), "sideways from the centre");
        let top: Vec<u8> = (0..oy + 150).map(|y| img.alpha(cx, y)).collect();
        assert!(top.windows(2).all(|w| w[0] <= w[1]), "upwards from the centre");
        assert!(img.alpha(cx, bottom + 54) < 12);
        assert!(img.alpha(cx, img.height - 1) <= 1);
    }

    #[test]
    fn peak_density_is_reached_under_a_large_window() {
        let p = active_light();
        let img = direct(&p, 600.0, 500.0, 1.0);
        let (ox, oy) = img.frame_origin();
        let a = img.alpha(ox + 300, oy + 250) as f64 / 255.0;
        assert!((a - 0.40).abs() < 0.01, "{a}");
        let dark = ShadowParams::lookup(ShadowOption::Standard, true, true, false);
        let d = direct(&dark, 600.0, 500.0, 1.0);
        let (dx, dy) = d.frame_origin();
        assert!((d.alpha(dx + 300, dy + 250) as f64 / 255.0 - 0.70).abs() < 0.01);
    }

    #[test]
    fn shadow_is_stronger_below_than_above_and_inactive_is_weaker() {
        let p = active_light();
        let img = direct(&p, 400.0, 300.0, 1.0);
        let (ox, oy) = img.frame_origin();
        let above = img.alpha(ox + 200, oy - 20);
        let below = img.alpha(ox + 200, oy + 300 + 20);
        assert!(below > above, "{below} vs {above}");
        let inactive = ShadowParams::lookup(ShadowOption::Standard, false, false, false);
        let weak = direct(&inactive, 400.0, 300.0, 1.0);
        let (wx, wy) = weak.frame_origin();
        assert!(weak.alpha(wx + 200, wy + 300 + 20) < below);
        assert!(weak.width < img.width, "smaller sigma gives smaller margins");
    }

    #[test]
    fn nine_slice_equals_direct_rendering() {
        let p = active_light();
        let mut r = ShadowRasterizer::new();
        for scale in [1.0, 2.0] {
            for (w, h) in [(300.0, 220.0), (640.0, 480.0)] {
                let a = r.render(&p, Size::new(w, h), 12.0, CornerStyle::continuous(), scale);
                let b = render_direct(&p, Size::new(w, h), 12.0, CornerStyle::continuous(), scale);
                assert_eq!((a.width, a.height), (b.width, b.height));
                let worst = a.data.iter().zip(&b.data).map(|(x, y)| (*x as i32 - *y as i32).abs()).max().unwrap();
                assert!(worst <= 1, "scale {scale} size {w}x{h}: worst difference {worst}");
            }
        }
        assert_eq!(r.cached_sources(), 2, "one cached source per scale");
    }

    #[test]
    fn small_frames_fall_back_to_a_direct_render() {
        let p = active_light();
        let mut r = ShadowRasterizer::new();
        let a = r.render(&p, Size::new(40.0, 30.0), 12.0, CornerStyle::Circular, 1.0);
        let b = render_direct(&p, Size::new(40.0, 30.0), 12.0, CornerStyle::Circular, 1.0);
        assert_eq!(a, b);
    }

    #[test]
    fn cache_distinguishes_parameter_sets() {
        let mut r = ShadowRasterizer::new();
        r.render(&active_light(), Size::new(400.0, 300.0), 12.0, CornerStyle::continuous(), 1.0);
        r.render(&active_light(), Size::new(500.0, 350.0), 12.0, CornerStyle::continuous(), 1.0);
        assert_eq!(r.cached_sources(), 1);
        r.render(&ShadowParams::lookup(ShadowOption::Standard, false, false, false), Size::new(400.0, 300.0), 12.0, CornerStyle::continuous(), 1.0);
        assert_eq!(r.cached_sources(), 2);
    }

    #[test]
    fn knock_out_clears_the_window_area() {
        let p = active_light();
        let mut img = direct(&p, 300.0, 200.0, 1.0);
        let (ox, oy) = img.frame_origin();
        assert!(img.alpha(ox + 150, oy + 100) > 50);
        img.knock_out((300, 200), 12.0, CornerStyle::continuous());
        assert_eq!(img.alpha(ox + 150, oy + 100), 0);
        assert!(img.alpha(ox + 150, oy + 200 + 10) > 20, "outside the window the shadow stays");
    }

    #[test]
    fn corners_are_softer_than_edges() {
        let p = active_light();
        let img = direct(&p, 400.0, 300.0, 1.0);
        let (ox, oy) = img.frame_origin();
        let edge = img.alpha(ox + 200, oy + 300 + 10);
        let corner = img.alpha(ox + 400 + 10, oy + 300 + 10);
        assert!(corner < edge);
    }
}
