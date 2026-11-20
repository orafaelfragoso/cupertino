//! What the lock screen and the login screen stand on: a wallpaper (a decoded picture, a solid colour, or the drawn
//! fallback) scaled to cover the output and centred, under the screens' dimming, made into one opaque picture of the
//! output's device pixels. The screens make that picture once per output size, scale and wallpaper and lay it under
//! every frame ([`crate::view`]), so a frame that changes the clock, the field or a fade draws only what changed.
//!
//! The dimming is two layers over the picture: black at [`DARKEN`] laid over it, and a radial black gradient of
//! [`VIGNETTE`] at the centre, transparent at the edge of a square [`VIGNETTE_SIDE`] points a side centred on the
//! screen. A third layer, black at [`LIGHTEN`] blended to keep the lighter of it and what is under it, changes no
//! pixel (black is never the lighter), so it is not drawn. The layers come in over [`DIM_FADE`] seconds when a screen
//! first shows: [`render`] takes their opacity.
//!
//! [`frosted`] is the material the password field and its buttons stand on: the dimmed picture under them blurred,
//! saturated and tinted dark, made from the picture itself, never from what lies under the window (the unlocked
//! session, under a lock surface).
//!
//! Everything here is plain arithmetic on pixels, on the thread that calls it: the screens make their pictures on Kit's
//! CPU pool, a whole screen's picture in bands of rows that the pool's threads share ([`crate::view::ScreenPictures`]).
//! Reading a picture file ([`decode`], [`for_session`], [`Stamp::of`]) runs on Kit's I/O pool
//! (`cupertino_kit::task::io`), never on the UI thread.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Black laid over the picture, alpha.
pub const DARKEN: f32 = 0.25;
/// Black blended over the darkened picture to keep the lighter of the two, alpha: a layer that changes nothing, kept
/// as the value the dimming is described by.
pub const LIGHTEN: f32 = 0.30;
/// Black at the centre of the radial gradient, alpha, falling to none at its edge.
pub const VIGNETTE: f32 = 0.20;
/// Side of the square the radial gradient fills, centred on the screen, points: its edge is half this from the centre.
pub const VIGNETTE_SIDE: f64 = 1200.0;
/// Seconds the dimming takes to come in when a screen first shows, linear.
pub const DIM_FADE: f64 = 0.25;

/// The drawn fallback's corner colours, top left and bottom right (approximate: a deep blue to violet).
const PLAIN_BASE: [[f32; 3]; 2] = [[0.07, 0.09, 0.20], [0.20, 0.09, 0.27]];
/// The direction of the fallback's diagonal gradient, as weights of x and y in the unit square.
const PLAIN_DIAGONAL: (f32, f32) = (0.45, 0.55);
/// Centre in the unit square, falloff `k` of `exp(-k r^2)` with `r` in heights, and colour of each glow of the
/// fallback.
const PLAIN_GLOWS: [([f32; 2], f32, [f32; 3]); 2] = [
    ([0.22, 0.30], 9.0, [0.10, 0.30, 0.58]),
    ([0.80, 0.68], 7.0, [0.50, 0.16, 0.42]),
];
/// Strength of a glow at its centre.
const GLOW_STRENGTH: f32 = 0.85;

/// The control material under the password field and its buttons: the blur's radius, points; how much it saturates
/// what it blurs; the grey laid over it and that grey's alpha; how far the result is drawn toward the mean colour of
/// the picture under the control, so the material takes a trace of its surroundings; and the grey it is lightened to
/// at least.
pub const MATERIAL_BLUR: f64 = 30.0;
pub const MATERIAL_SATURATION: f32 = 1.6;
pub const MATERIAL_FILL: f32 = 0.1569;
pub const MATERIAL_FILL_ALPHA: f32 = 0.4;
pub const MATERIAL_SURROUND: f32 = 0.05;
pub const MATERIAL_TONE: f32 = 0.08;

/// The most pixels a decoded picture may have a side, and in all.
pub const MAX_SIDE: u32 = 8192;

/// The deviation of the Gaussian a blur of a radius comes close to, per point of the radius: the renderers' blur takes
/// a radius as twice the deviation.
const SIGMA_PER_RADIUS: f32 = 0.5;

/// Side of the ordered dither's matrix, pixels: the offsets repeat every this many pixels each way.
const DITHER_SIZE: u32 = 8;

static NEXT_PICTURE: AtomicU64 = AtomicU64::new(1);

/// A decoded picture: opaque RGB, three bytes a pixel, rows top to bottom. Each picture has an identity of its own, by
/// which the screens keep what they made of it.
pub struct Picture {
    width: u32,
    height: u32,
    rgb: Vec<u8>,
    id: u64,
}

impl std::fmt::Debug for Picture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Picture({}x{} #{})", self.width, self.height, self.id)
    }
}

impl Picture {
    /// A picture of `width` x `height` pixels from straight RGBA bytes; a pixel that is not opaque is laid over black.
    /// `None` when the bytes do not fit the size or a side is 0 or past [`MAX_SIDE`].
    pub fn from_rgba(width: u32, height: u32, rgba: &[u8]) -> Option<Picture> {
        let count = usize::try_from(u64::from(width) * u64::from(height)).ok()?;
        if width == 0 || height == 0 || width > MAX_SIDE || height > MAX_SIDE {
            return None;
        }
        if rgba.len() != count.checked_mul(4)? {
            return None;
        }
        let mut rgb = Vec::with_capacity(count * 3);
        for px in rgba.as_chunks::<4>().0 {
            let a = u16::from(px[3]);
            for &c in &px[..3] {
                rgb.push(((u16::from(c) * a + 127) / 255) as u8);
            }
        }
        Some(Picture {
            width,
            height,
            rgb,
            id: NEXT_PICTURE.fetch_add(1, Ordering::Relaxed),
        })
    }

    /// A picture of `width` x `height` pixels from RGB bytes, three a pixel; `None` as for [`Picture::from_rgba`].
    pub fn from_rgb(width: u32, height: u32, rgb: Vec<u8>) -> Option<Picture> {
        let count = usize::try_from(u64::from(width) * u64::from(height)).ok()?;
        if width == 0 || height == 0 || width > MAX_SIDE || height > MAX_SIDE {
            return None;
        }
        if rgb.len() != count.checked_mul(3)? {
            return None;
        }
        Some(Picture {
            width,
            height,
            rgb,
            id: NEXT_PICTURE.fetch_add(1, Ordering::Relaxed),
        })
    }

    /// A picture of `width` x `height` pixels from premultiplied RGBA bytes, four a pixel (as `cupertino_kit::image`
    /// decodes): each pixel laid over black, which keeps its colour bytes. `None` as for [`Picture::from_rgba`].
    pub fn from_premultiplied(width: u32, height: u32, rgba: &[u8]) -> Option<Picture> {
        let count = usize::try_from(u64::from(width) * u64::from(height)).ok()?;
        if rgba.len() != count.checked_mul(4)? {
            return None;
        }
        let rgb = rgba
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|px| [px[0], px[1], px[2]])
            .collect();
        Picture::from_rgb(width, height, rgb)
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    /// The picture halved each way, each pixel the mean of the four it covers (an odd last row or column is kept as
    /// the edge's).
    fn halved(&self) -> Picture {
        let (w, h) = (
            self.width.div_ceil(2).max(1),
            self.height.div_ceil(2).max(1),
        );
        let (sw, sh) = (self.width as usize, self.height as usize);
        let mut rgb = Vec::with_capacity(w as usize * h as usize * 3);
        for y in 0..h as usize {
            let (y0, y1) = (2 * y, (2 * y + 1).min(sh - 1));
            for x in 0..w as usize {
                let (x0, x1) = (2 * x, (2 * x + 1).min(sw - 1));
                for c in 0..3 {
                    let at = |x: usize, y: usize| u32::from(self.rgb[(y * sw + x) * 3 + c]);
                    let sum = at(x0, y0) + at(x1, y0) + at(x0, y1) + at(x1, y1);
                    rgb.push(((sum + 2) / 4) as u8);
                }
            }
        }
        Picture {
            width: w,
            height: h,
            rgb,
            id: self.id,
        }
    }
}

/// What a screen stands on.
#[derive(Clone, Debug)]
pub enum Source {
    /// A decoded picture, scaled to cover the screen.
    Picture(Arc<Picture>),
    /// A solid colour, RGB.
    Solid([u8; 3]),
    /// The drawn fallback: a diagonal gradient with two soft glows.
    Plain,
}

impl Source {
    /// What distinguishes this source from another: a picture by its identity, a colour by its value.
    pub fn key(&self) -> u64 {
        match self {
            Source::Picture(picture) => picture.id << 2,
            Source::Solid([r, g, b]) => {
                (u64::from(*r) << 26 | u64::from(*g) << 18 | u64::from(*b) << 10) | 1
            }
            Source::Plain => 2,
        }
    }
}

/// How much of the dimming shows `elapsed` seconds after a screen first showed: 0 to 1 over [`DIM_FADE`], linear.
pub fn dimming_at(elapsed: f64) -> f64 {
    (elapsed / DIM_FADE).clamp(0.0, 1.0)
}

/// What the dimming at opacity `dim` leaves of a pixel whose centre lies `distance` points from the screen's centre:
/// the darkening over all of it, and the radial gradient inside its circle.
pub fn dimmed_share(distance: f64, dim: f32) -> f32 {
    let edge = VIGNETTE_SIDE / 2.0;
    let gradient = (1.0 - distance / edge).max(0.0) as f32;
    (1.0 - DARKEN * dim) * (1.0 - VIGNETTE * gradient * dim)
}

/// The dimming at one opacity over a screen of device pixels, by its columns and rows: the squared distance of each
/// column's and row's centres from the screen's centre, so a pixel costs a square root only inside the radial
/// gradient's circle, and none outside it.
struct Dimming {
    columns: Vec<f32>,
    rows: Vec<f32>,
    /// What the darkening alone leaves, outside the circle.
    outside: f32,
    /// The gradient's black at its centre, at this opacity.
    vignette: f32,
    /// The circle's radius, points, and its square.
    edge: f32,
    edge2: f32,
}

impl Dimming {
    fn new(width: u32, height: u32, scale: f64, dim: f32) -> Dimming {
        let axis = |count: u32| {
            let centre = f64::from(count) / scale / 2.0;
            (0..count)
                .map(|i| {
                    let d = (f64::from(i) + 0.5) / scale - centre;
                    (d * d) as f32
                })
                .collect()
        };
        let edge = (VIGNETTE_SIDE / 2.0) as f32;
        Dimming {
            columns: axis(width),
            rows: axis(height),
            outside: 1.0 - DARKEN * dim,
            vignette: VIGNETTE * dim,
            edge,
            edge2: edge * edge,
        }
    }

    /// What device pixel `x` of a row whose squared distance is `dy2` keeps of its colour: [`dimmed_share`].
    #[inline]
    fn share(&self, x: usize, dy2: f32) -> f32 {
        let r2 = self.columns[x] + dy2;
        if r2 >= self.edge2 {
            self.outside
        } else {
            self.outside * (1.0 - self.vignette * (1.0 - r2.sqrt() / self.edge))
        }
    }

    /// The columns of a row whose squared distance is `dy2` that lie inside the circle, as a range; empty for a row
    /// that misses it.
    fn inside(&self, dy2: f32) -> std::ops::Range<usize> {
        if dy2 >= self.edge2 || self.vignette == 0.0 {
            return 0..0;
        }
        let Some(first) = self.columns.iter().position(|&c| c + dy2 < self.edge2) else {
            return 0..0;
        };
        let last = self.columns.len()
            - self
                .columns
                .iter()
                .rev()
                .take_while(|&&c| c + dy2 >= self.edge2)
                .count();
        first..last
    }
}

/// The ordered dither the renderers use (`cupertino-raster-math`'s `dither`), the same offsets at the same pixels: the
/// entry, 0 to 63, of pixel `(x, y)` in the [`DITHER_SIZE`] x [`DITHER_SIZE`] ordered matrix, made of the 2 x 2 matrix
/// `[[0, 2], [3, 1]]` at each of the three low bits of `x` and `y`.
fn dither_matrix(x: u32, y: u32) -> u32 {
    let bit = |v: u32, k: u32| (v >> k) & 1;
    let pair = |x: u32, y: u32| 2 * (x ^ y) + y;
    16 * pair(bit(x, 0), bit(y, 0)) + 4 * pair(bit(x, 1), bit(y, 1)) + pair(bit(x, 2), bit(y, 2))
}

/// The dither's offset of pixel `(x, y)`, in 8-bit steps, inside (-0.5, 0.5): `(m + 0.5) / 64 - 0.5` for its matrix
/// entry `m`, so a value 8 bits hold exactly is stored unchanged.
fn dither_offset(x: u32, y: u32) -> f32 {
    let levels = (DITHER_SIZE * DITHER_SIZE) as f32;
    (dither_matrix(x, y) as f32 + 0.5) / levels - 0.5
}

/// `v` (0 to 1) stored in 8 bits: held to 0 to 1 and rounded to nearest, a half away from zero.
fn quantize(v: f32) -> u8 {
    let x = v.clamp(0.0, 1.0) * 255.0;
    // the fraction `x - whole` is exact
    let whole = x as u32;
    (whole + u32::from(x - whole as f32 >= 0.5)) as u8
}

/// `v` (0 to 1) with the dither's offset of pixel `(x, y)` added, stored in 8 bits.
fn dither_store(v: f32, x: u32, y: u32) -> u8 {
    quantize(v + dither_offset(x, y) / 255.0)
}

/// The dither's offsets of a row of the matrix, in 8-bit steps, by the column's place in it.
fn dither_row(y: u32) -> [f32; DITHER_SIZE as usize] {
    std::array::from_fn(|x| dither_offset(x as u32, y))
}

/// `v` (0 to 255) with the dither's offset `offset` added, held to 0 to 255 and rounded to nearest, as
/// [`dither_store`] stores `v / 255`.
#[inline]
fn store(v: f32, offset: f32) -> u8 {
    ((v + offset).clamp(0.0, 255.0) + 0.5) as u8
}

/// How a picture of `src` pixels lies over a screen of `dst` device pixels when scaled to cover it, centred: the
/// device pixels a picture pixel spans, and the screen pixel the picture's top left corner falls on (negative or
/// zero: the overflow of the longer side is cut evenly).
pub fn cover(src: (u32, u32), dst: (u32, u32)) -> (f64, f64, f64) {
    let (sw, sh) = (f64::from(src.0.max(1)), f64::from(src.1.max(1)));
    let (dw, dh) = (f64::from(dst.0), f64::from(dst.1));
    let scale = (dw / sw).max(dh / sh);
    (scale, (dw - sw * scale) / 2.0, (dh - sh * scale) / 2.0)
}

/// The drawn fallback over a screen `width` x `height` points large, by its columns and rows: what each column and
/// row adds to the gradient's position, and each glow's factor along each, so a pixel costs no exponential.
struct Plain {
    columns: Vec<(f32, [f32; 2])>,
    rows: Vec<(f32, [f32; 2])>,
}

impl Plain {
    /// For the device columns and rows `0..columns` and `0..rows` at `scale`.
    fn new(columns: u32, rows: u32, scale: f64, (width, height): (f32, f32)) -> Plain {
        let (a, b) = PLAIN_DIAGONAL;
        // the gradient runs from the top left corner along (a w, b h) / (a² + b²)
        let norm = a * a + b * b;
        let (gx, gy) = (a / norm * width, b / norm * height);
        let length = gx * gx + gy * gy;
        // exp(-k r²) with r in heights is exp(-k dx²) exp(-k dy²)
        let axis = |count: u32, along: f32, side: f32, of: fn([f32; 2]) -> f32| {
            (0..count)
                .map(|i| {
                    let p = ((f64::from(i) + 0.5) / scale) as f32;
                    let glows = PLAIN_GLOWS.map(|(center, k, _)| {
                        let d = (p - of(center) * side) / height;
                        (-k * d * d).exp()
                    });
                    (p * along / length, glows)
                })
                .collect()
        };
        Plain {
            columns: axis(columns, gx, width, |c| c[0]),
            rows: axis(rows, gy, height, |c| c[1]),
        }
    }

    /// The colour at device pixel `(x, y)`, 0 to 1.
    fn at(&self, x: u32, y: u32) -> [f32; 3] {
        let ((tx, gx), (ty, gy)) = (self.columns[x as usize], self.rows[y as usize]);
        let t = (tx + ty).clamp(0.0, 1.0);
        let mut c = [0.0f32; 3];
        for (i, v) in c.iter_mut().enumerate() {
            *v = PLAIN_BASE[0][i] + (PLAIN_BASE[1][i] - PLAIN_BASE[0][i]) * t;
        }
        for (g, (_, _, color)) in PLAIN_GLOWS.iter().enumerate() {
            let alpha = GLOW_STRENGTH * gx[g] * gy[g];
            for (i, v) in c.iter_mut().enumerate() {
                *v += (color[i] - *v) * alpha;
            }
        }
        c
    }
}

/// Where each device column (or row) of a screen reads a picture scaled by `scale` and placed at `offset`: the two
/// picture pixels it lies between and the weight of the second, as bilinear filtering takes them.
fn taps(count: u32, len: u32, scale: f64, offset: f64) -> Vec<(u32, u32, f32)> {
    let last = len.saturating_sub(1);
    (0..count)
        .map(|i| {
            let u = ((f64::from(i) + 0.5 - offset) / scale - 0.5).clamp(0.0, f64::from(last));
            let lo = u.floor() as u32;
            (lo, (lo + 1).min(last), (u - f64::from(lo)) as f32)
        })
        .collect()
}

/// Runs `row(y, bytes)` for the rows of an image `width` pixels wide whose rows from `y0` are `out`.
fn by_rows(out: &mut [u8], width: usize, y0: u32, row: impl Fn(u32, &mut [u8])) {
    for (i, line) in out.chunks_mut(width * 4).enumerate() {
        row(y0 + i as u32, line);
    }
}

/// One row of a picture scaled across: each device column's two pixels of `row` (RGB bytes) blended by the weight of the
/// second, of 256, into `out`, three values a column, each 256 times a byte.
fn across(row: &[u8], columns: &[(usize, usize, u32)], out: &mut [u32]) {
    for ((a, b, wx), px) in columns.iter().zip(out.as_chunks_mut::<3>().0) {
        let (left, right) = (&row[*a..][..3], &row[*b..][..3]);
        for k in 0..3 {
            px[k] = u32::from(left[k]) * (256 - wx) + u32::from(right[k]) * wx;
        }
    }
}

/// The rows `region.1..region.3` and columns `region.0..region.2` of the picture of `source` over a screen of `width` x
/// `height` device pixels at `scale` with the dimming at opacity `dim`: opaque premultiplied RGBA, dithered.
fn render_region(
    source: &Source,
    (width, height): (u32, u32),
    scale: f64,
    dim: f32,
    region: [u32; 4],
) -> Vec<u8> {
    let [x0, y0, x1, y1] = region;
    let (w, h) = (
        x1.saturating_sub(x0) as usize,
        y1.saturating_sub(y0) as usize,
    );
    let mut out = vec![0u8; w * h * 4];
    render_region_into(source, (width, height), scale, dim, region, &mut out);
    out
}

/// [`render_region`] into `out`, which holds the region's pixels.
fn render_region_into(
    source: &Source,
    (width, height): (u32, u32),
    scale: f64,
    dim: f32,
    region: [u32; 4],
    out: &mut [u8],
) {
    let [x0, y0, x1, y1] = region;
    let (w, h) = (
        x1.saturating_sub(x0) as usize,
        y1.saturating_sub(y0) as usize,
    );
    if w == 0 || h == 0 || out.len() != w * h * 4 {
        return;
    }
    let scale = if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    };
    // the picture, halved until it is less than twice as large as the screen, so bilinear taps miss none of it
    let mut halved = None;
    if let Source::Picture(picture) = source {
        let mut p: Option<Picture> = None;
        loop {
            let current = p.as_ref().unwrap_or(picture);
            let (s, _, _) = cover((current.width, current.height), (width, height));
            if s >= 0.5 || current.width < 2 || current.height < 2 {
                break;
            }
            p = Some(current.halved());
        }
        halved = p;
    }
    let picture = match source {
        Source::Picture(picture) => Some(halved.as_ref().unwrap_or(picture)),
        _ => None,
    };
    let columns = picture.map(|p| {
        let (s, ox, oy) = cover((p.width, p.height), (width, height));
        let xs: Vec<_> = taps(width, p.width, s, ox)[x0 as usize..x1 as usize].to_vec();
        (xs, taps(height, p.height, s, oy))
    });
    let solid = match source {
        Source::Solid(c) => Some(c.map(f32::from)),
        _ => None,
    };
    let plain = matches!(source, Source::Plain).then(|| {
        let points = (
            (f64::from(width) / scale) as f32,
            (f64::from(height) / scale) as f32,
        );
        Plain::new(width, height, scale, points)
    });
    let dimming = Dimming::new(width, height, scale, dim);
    // a picture's columns in fixed point: the bytes of the two pixels each reads and the second's weight, of 256
    let fixed: Option<Vec<(usize, usize, u32)>> = columns.as_ref().map(|(xs, _)| {
        xs.iter()
            .map(|&(a, b, f)| (a as usize * 3, b as usize * 3, (f * 256.0).round() as u32))
            .collect()
    });
    if let (Some(fixed), Some((_, ys)), Some(p)) = (&fixed, &columns, picture) {
        // bilinear in whole numbers, across then down: each picture row a band reads is scaled across once, into one of
        // two rows kept from one device row to the next. The weights of 8 bits and the share kept of 16 put a byte at
        // 2^24 a step, where the dither's offset with a half to round to nearest is added
        let stride = p.width as usize * 3;
        let outside = (dimming.outside * 65536.0).round() as u32;
        // one band, the region's rows, in order: each keeps what one row leaves to the next
        let band = |first: u32, bytes: &mut [u8]| {
            let mut held: [(Option<u32>, Vec<u32>); 2] =
                [(None, vec![0; w * 3]), (None, vec![0; w * 3])];
            for (j, line) in bytes.chunks_mut(w * 4).enumerate() {
                let y = first + j as u32;
                let (ya, yb, fy) = ys[y as usize];
                if held[0].0 != Some(ya) {
                    if held[1].0 == Some(ya) {
                        held.swap(0, 1);
                    } else {
                        across(
                            &p.rgb[ya as usize * stride..][..stride],
                            fixed,
                            &mut held[0].1,
                        );
                        held[0].0 = Some(ya);
                    }
                }
                if held[1].0 != Some(yb) {
                    across(
                        &p.rgb[yb as usize * stride..][..stride],
                        fixed,
                        &mut held[1].1,
                    );
                    held[1].0 = Some(yb);
                }
                let wy = (fy * 256.0).round() as u32;
                let dy2 = dimming.rows[y as usize];
                let rounding = dither_row(y).map(|o| u64::from(((o + 0.5) * 16_777_216.0) as u32));
                let inside = dimming.inside(dy2);
                let (top, bottom) = (held[0].1.as_chunks::<3>().0, held[1].1.as_chunks::<3>().0);
                let pixels = line.as_chunks_mut::<4>().0;
                for (i, ((px, t), d)) in pixels.iter_mut().zip(top).zip(bottom).enumerate() {
                    let x = x0 as usize + i;
                    let share = if inside.contains(&x) {
                        (dimming.share(x, dy2) * 65536.0).round() as u32
                    } else {
                        outside
                    };
                    let r = rounding[x & 7];
                    for k in 0..3 {
                        let v = (t[k] * (256 - wy) + d[k] * wy) >> 8;
                        px[k] = ((u64::from(v * share) + r) >> 24).min(255) as u8;
                    }
                    px[3] = 255;
                }
            }
        };
        band(y0, out);
        return;
    }
    let row = |y: u32, line: &mut [u8]| {
        let dy2 = dimming.rows[y as usize];
        let offsets = dither_row(y);
        let pixels = line.as_chunks_mut::<4>().0;
        for (i, px) in pixels.iter_mut().enumerate() {
            let x = x0 + i as u32;
            let share = dimming.share(x as usize, dy2) * 255.0;
            let c = match solid {
                Some(c) => [c[0] / 255.0, c[1] / 255.0, c[2] / 255.0],
                None => plain.as_ref().map_or([0.0; 3], |p| p.at(x, y)),
            };
            let offset = offsets[(x & 7) as usize];
            for k in 0..3 {
                px[k] = store(c[k] * share, offset);
            }
            px[3] = 255;
        }
    };
    by_rows(out, w, y0, row);
}

/// The picture of `source` over a screen of `width` x `height` device pixels at `scale` (device pixels a point), with
/// the dimming at opacity `dim` (0 to 1): opaque premultiplied RGBA, rows top to bottom, dithered.
pub fn render(source: &Source, width: u32, height: u32, scale: f64, dim: f32) -> Vec<u8> {
    render_region(source, (width, height), scale, dim, [0, 0, width, height])
}

/// [`render`] into `out`, which holds `width` x `height` pixels: the screens make their picture in the memory they hand
/// on, with no copy of it.
pub fn render_into(source: &Source, width: u32, height: u32, scale: f64, dim: f32, out: &mut [u8]) {
    render_rows_into(source, width, height, scale, dim, 0..height, out);
}

/// The rows `rows` of [`render`]'s picture into `out`, which holds those rows: one band of a screen's picture, which the
/// screens make apart from the others on the CPU pool. The bands of a picture together hold the very pixels
/// [`render`] makes.
pub fn render_rows_into(
    source: &Source,
    width: u32,
    height: u32,
    scale: f64,
    dim: f32,
    rows: std::ops::Range<u32>,
    out: &mut [u8],
) {
    let rows = rows.start.min(height)..rows.end.min(height);
    render_region_into(
        source,
        (width, height),
        scale,
        dim,
        [0, rows.start, width, rows.end],
        out,
    );
}

/// The picture `undimmed` (made by [`render`] with the dimming at 0) with the dimming at opacity `dim`, into `out`: what a
/// frame of the dimming coming in shows, made from the kept picture with no new look at the source.
pub fn dim_into(undimmed: &[u8], width: u32, height: u32, scale: f64, dim: f32, out: &mut [u8]) {
    dim_rows_into(undimmed, width, height, scale, dim, 0, out);
}

/// [`dim_into`] for the rows of the screen from `first_row` that `undimmed` and `out` hold: one band of a frame of the
/// dimming coming in, made from the same band of the undimmed picture.
pub fn dim_rows_into(
    undimmed: &[u8],
    width: u32,
    height: u32,
    scale: f64,
    dim: f32,
    first_row: u32,
    out: &mut [u8],
) {
    let w = width as usize;
    let rows = out.len() / (w * 4).max(1);
    if w == 0
        || undimmed.len() != out.len()
        || out.len() != w * rows * 4
        || u64::from(first_row) + rows as u64 > u64::from(height)
    {
        return;
    }
    let scale = if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    };
    let dimming = Dimming::new(width, height, scale, dim);
    // outside the circle every pixel keeps the same share: a table of what each byte becomes at each column of the
    // dither's matrix, by its row
    let tables: Vec<[[u8; 256]; 8]> = (0..8)
        .map(|y| {
            let offsets = dither_row(y);
            std::array::from_fn(|x| {
                std::array::from_fn(|v| store(v as f32 * dimming.outside, offsets[x]))
            })
        })
        .collect();
    let row = |y: u32, line: &mut [u8]| {
        let from = &undimmed[(y - first_row) as usize * w * 4..][..w * 4];
        let dy2 = dimming.rows[y as usize];
        let offsets = dither_row(y);
        let table = &tables[(y & 7) as usize];
        let inside = dimming.inside(dy2);
        let (pixels, sources) = (line.as_chunks_mut::<4>().0, from.as_chunks::<4>().0);
        for (x, (px, src)) in pixels.iter_mut().zip(sources).enumerate() {
            if inside.contains(&x) {
                let share = dimming.share(x, dy2);
                let offset = offsets[x & 7];
                for k in 0..3 {
                    px[k] = store(f32::from(src[k]) * share, offset);
                }
            } else {
                let table = &table[x & 7];
                for k in 0..3 {
                    px[k] = table[src[k] as usize];
                }
            }
            px[3] = 255;
        }
    };
    by_rows(out, w, first_row, row);
}

/// One pass of a box blur `radius` pixels each side along rows (`across` false) or columns of an RGB float image.
fn box_pass(pixels: &mut [[f32; 3]], (w, h): (usize, usize), radius: usize, across: bool) {
    let (lines, len) = if across { (w, h) } else { (h, w) };
    let index = |line: usize, i: usize| if across { i * w + line } else { line * w + i };
    let mut held = vec![[0.0f32; 3]; len];
    let span = (2 * radius + 1) as f32;
    for line in 0..lines {
        for (i, v) in held.iter_mut().enumerate() {
            *v = pixels[index(line, i)];
        }
        let at = |i: isize| held[i.clamp(0, len as isize - 1) as usize];
        let mut sum = [0.0f32; 3];
        for i in -(radius as isize)..=radius as isize {
            let v = at(i);
            for k in 0..3 {
                sum[k] += v[k];
            }
        }
        for i in 0..len {
            let out = &mut pixels[index(line, i)];
            for k in 0..3 {
                out[k] = sum[k] / span;
            }
            let (enter, leave) = (
                at(i as isize + radius as isize + 1),
                at(i as isize - radius as isize),
            );
            for k in 0..3 {
                sum[k] += enter[k] - leave[k];
            }
        }
    }
}

/// The radii of the three box blurs whose sequence comes close to a Gaussian of deviation `sigma` pixels.
pub fn box_radii(sigma: f64) -> [usize; 3] {
    // three boxes of width w have the variance 3 (w² - 1) / 12 = sigma²
    let width = (4.0 * sigma * sigma + 1.0).sqrt();
    let radius = ((width - 1.0) / 2.0).round().max(0.0) as usize;
    [radius; 3]
}

/// The control material over `region` (device pixels `[x0, y0, x1, y1]`) of the fully dimmed picture of `source` on a
/// screen of `width` x `height` device pixels at `scale`: the picture blurred by [`MATERIAL_BLUR`] points, saturated by
/// [`MATERIAL_SATURATION`], [`MATERIAL_FILL`] grey laid over at [`MATERIAL_FILL_ALPHA`], drawn toward the region's
/// mean blurred colour by [`MATERIAL_SURROUND`] and lightened to at least [`MATERIAL_TONE`]. Opaque premultiplied RGBA
/// of the region, dithered; empty for an empty region.
pub fn frosted(source: &Source, width: u32, height: u32, scale: f64, region: [u32; 4]) -> Vec<u8> {
    let Some((blurred, [x0, y0, x1, _])) =
        blurred(source, width, height, scale, region, MATERIAL_BLUR)
    else {
        return Vec::new();
    };
    let rw = (x1 - x0) as usize;
    let surround = mean(&blurred);
    let mut out = vec![0u8; blurred.len() * 4];
    for (j, line) in out.chunks_mut(rw * 4).enumerate() {
        for (i, px) in line.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let c = material(blurred[j * rw + i], surround);
            let (x, y) = (x0 + i as u32, y0 + j as u32);
            for k in 0..3 {
                px[k] = dither_store(c[k], x, y);
            }
            px[3] = 255;
        }
    }
    out
}

/// The fully dimmed picture of `source` on a screen of `width` x `height` device pixels at `scale`, blurred by `blur`
/// points, over `region` (device pixels `[x0, y0, x1, y1]`, cut to the screen): its colours (0 to 1) row by row and the
/// region they cover; `None` for an empty region.
fn blurred(
    source: &Source,
    width: u32,
    height: u32,
    scale: f64,
    region: [u32; 4],
    blur: f64,
) -> Option<(Vec<[f32; 3]>, [u32; 4])> {
    let [x0, y0, x1, y1] = [
        region[0].min(width),
        region[1].min(height),
        region[2].min(width),
        region[3].min(height),
    ];
    if x0 >= x1 || y0 >= y1 {
        return None;
    }
    let scale = if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    };
    let sigma = f64::from(blur as f32 * SIGMA_PER_RADIUS) * scale;
    let radii = box_radii(sigma);
    let reach = radii.iter().sum::<usize>() as u32;
    // the blur reads the picture around the region
    let outer = [
        x0.saturating_sub(reach),
        y0.saturating_sub(reach),
        (x1 + reach).min(width),
        (y1 + reach).min(height),
    ];
    let bytes = render_region(source, (width, height), scale, 1.0, outer);
    let (w, h) = (
        (outer[2] - outer[0]) as usize,
        (outer[3] - outer[1]) as usize,
    );
    let mut pixels: Vec<[f32; 3]> = bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|p| {
            [
                f32::from(p[0]) / 255.0,
                f32::from(p[1]) / 255.0,
                f32::from(p[2]) / 255.0,
            ]
        })
        .collect();
    for radius in radii {
        box_pass(&mut pixels, (w, h), radius, false);
        box_pass(&mut pixels, (w, h), radius, true);
    }
    let (rw, rh) = ((x1 - x0) as usize, (y1 - y0) as usize);
    let mut out = Vec::with_capacity(rw * rh);
    for j in 0..rh {
        let sy = (y0 - outer[1]) as usize + j;
        let at = sy * w + (x0 - outer[0]) as usize;
        out.extend_from_slice(&pixels[at..at + rw]);
    }
    Some((out, [x0, y0, x1, y1]))
}

/// The glass the large time is drawn in: the picture under its letters blurred by this radius, points, and white laid
/// over it at this alpha, so the letters read as white glass the picture shows through (approximate).
pub const GLASS_TEXT_BLUR: f64 = 20.0;
pub const GLASS_TEXT_WHITE: f32 = 0.78;
/// The glass letters' rims, along the inside of their edges: this wide, points; white at this alpha along the top and
/// left edges and black at this alpha along the bottom and right ones (approximate).
pub const GLASS_TEXT_RIM: f64 = 1.0;
pub const GLASS_TEXT_RIM_LIGHT: f32 = 0.5;
pub const GLASS_TEXT_RIM_DARK: f32 = 0.12;

/// The glass letters over `region` (device pixels `[x0, y0, x1, y1]`) of the fully dimmed picture of `source` on a
/// screen of `width` x `height` device pixels at `scale`, in the shape of `coverage` (one byte a pixel of the region,
/// row by row): the picture blurred by [`GLASS_TEXT_BLUR`], [`GLASS_TEXT_WHITE`] white laid over it, a light rim along
/// the letters' top and left edges and a dark one along their bottom and right, each [`GLASS_TEXT_RIM`] wide.
/// Premultiplied RGBA of the region, dithered, clear outside the letters; empty when the region is empty or the
/// coverage is not of its size.
#[allow(clippy::too_many_arguments)]
pub fn glass_letters(
    source: &Source,
    width: u32,
    height: u32,
    scale: f64,
    region: [u32; 4],
    coverage: &[u8],
) -> Vec<u8> {
    let Some((blurred, [x0, y0, x1, y1])) =
        blurred(source, width, height, scale, region, GLASS_TEXT_BLUR)
    else {
        return Vec::new();
    };
    let (rw, rh) = ((x1 - x0) as usize, (y1 - y0) as usize);
    if coverage.len() != rw * rh || region != [x0, y0, x1, y1] {
        return Vec::new();
    }
    let rim = (GLASS_TEXT_RIM * scale).round().max(1.0) as isize;
    let cover = |i: isize, j: isize| -> f32 {
        if i < 0 || j < 0 || i >= rw as isize || j >= rh as isize {
            0.0
        } else {
            f32::from(coverage[j as usize * rw + i as usize]) / 255.0
        }
    };
    let mut out = vec![0u8; rw * rh * 4];
    for (j, line) in out.chunks_mut(rw * 4).enumerate() {
        for (i, px) in line.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let a = f32::from(coverage[j * rw + i]) / 255.0;
            if a <= 0.0 {
                continue;
            }
            let (ii, jj) = (i as isize, j as isize);
            // an edge facing up and left: the letter here, less of it `rim` up and to the left
            let light = (a - cover(ii - rim, jj - rim)).clamp(0.0, 1.0) * GLASS_TEXT_RIM_LIGHT;
            let dark = (a - cover(ii + rim, jj + rim)).clamp(0.0, 1.0) * GLASS_TEXT_RIM_DARK;
            let c = glass_text(blurred[j * rw + i]).map(|v| {
                let lit = v + (1.0 - v) * light;
                lit * (1.0 - dark)
            });
            let (x, y) = (x0 + i as u32, y0 + j as u32);
            for k in 0..3 {
                px[k] = dither_store(c[k] * a, x, y);
            }
            px[3] = dither_store(a, x, y).max(px[0].max(px[1]).max(px[2]));
        }
    }
    out
}

/// The glass letters' colour over the blurred colour `c` (0 to 1): [`GLASS_TEXT_WHITE`] white laid over it.
pub fn glass_text(c: [f32; 3]) -> [f32; 3] {
    c.map(|v| v + (1.0 - v) * GLASS_TEXT_WHITE)
}

/// The mean of `colours`; black for none.
fn mean(colours: &[[f32; 3]]) -> [f32; 3] {
    if colours.is_empty() {
        return [0.0; 3];
    }
    let mut sum = [0.0f64; 3];
    for c in colours {
        for k in 0..3 {
            sum[k] += f64::from(c[k]);
        }
    }
    sum.map(|v| (v / colours.len() as f64) as f32)
}

/// The control material's colour over the blurred colour `c` (0 to 1) of a picture whose mean colour under the
/// control is `surround`: saturated, the grey laid over, drawn a little toward `surround`, lightened to the tone:
/// darker than a light picture and never black.
pub fn material(c: [f32; 3], surround: [f32; 3]) -> [f32; 3] {
    // Rec. 709 luma keeps its brightness as the colour moves away from grey
    let luma = 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
    let mut out = [0.0; 3];
    for k in 0..3 {
        let saturated = (luma + (c[k] - luma) * MATERIAL_SATURATION).clamp(0.0, 1.0);
        let filled = saturated + (MATERIAL_FILL - saturated) * MATERIAL_FILL_ALPHA;
        let surrounded = filled + (surround[k] - filled) * MATERIAL_SURROUND;
        out[k] = surrounded.max(MATERIAL_TONE);
    }
    out
}

/// The largest picture file [`decode`] reads.
pub const MAX_FILE_BYTES: u64 = 256 << 20;

/// Decodes the PNG or JPEG at `path` into a picture: refused when it is not a regular file, larger than `max_bytes`
/// (at most [`MAX_FILE_BYTES`]), of another format, or past [`MAX_SIDE`] a side. A FIFO or a device named as the
/// wallpaper never blocks it. It reads the file and decodes it: run it on Kit's I/O pool (`cupertino_kit::task::io`),
/// never on the UI thread.
#[expect(
    clippy::disallowed_methods,
    reason = "runs on the I/O pool (kit::task::io)"
)]
pub fn decode(path: &std::path::Path, max_bytes: u64) -> Result<Picture, String> {
    use std::io::Read;
    use std::os::unix::fs::OpenOptionsExt;
    // a FIFO or a device named as the wallpaper never blocks the screen
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_NOCTTY | libc::O_CLOEXEC)
        .open(path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    let meta = file.metadata().map_err(|e| e.to_string())?;
    let limit = max_bytes.min(MAX_FILE_BYTES);
    if !meta.is_file() || meta.len() > limit {
        return Err(format!("{}: not a picture file", path.display()));
    }
    // no more than the limit, should the file grow while it is read
    let mut data = Vec::with_capacity(meta.len() as usize);
    file.take(limit + 1)
        .read_to_end(&mut data)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    if data.len() as u64 > limit {
        return Err(format!("{}: not a picture file", path.display()));
    }
    let image = cupertino_kit::image::decode(&data, MAX_SIDE).map_err(|e| match e {
        cupertino_kit::image::ImageError::Unsupported => {
            format!("{}: only PNG and JPEG are read", path.display())
        }
        e => format!("{}: {e}", path.display()),
    })?;
    // a picture Kit decodes is premultiplied rows with no gap between them
    Picture::from_premultiplied(image.width, image.height, &image.pixels)
        .ok_or_else(|| format!("{}: no pixels", path.display()))
}

/// What the lock screen of a session stands on, by the compositor's `settings` and the session's appearance (`dark`): a
/// solid colour when one is set, else the appearance's picture (the dark one in a dark appearance when it is set), else
/// the default picture `default` names for the appearance, else the drawn fallback. A picture that does not decode is
/// passed over, and why is said on standard error. It reads the pictures ([`decode`]): run it on Kit's I/O pool.
pub fn for_session(
    settings: &cupertino_defaults::compositor::CompositorSettings,
    dark: bool,
    default: impl FnOnce(bool) -> Option<std::path::PathBuf>,
) -> Source {
    if let Some([r, g, b, _]) = settings.wallpaper_color {
        return Source::Solid([r, g, b]);
    }
    let own = if dark {
        settings
            .wallpaper_dark
            .as_ref()
            .or(settings.wallpaper.as_ref())
    } else {
        settings.wallpaper.as_ref()
    };
    let own = own.map(std::path::PathBuf::from);
    for path in own.into_iter().chain(default(dark)) {
        match decode(&path, MAX_FILE_BYTES) {
            Ok(picture) => return Source::Picture(Arc::new(picture)),
            Err(e) => eprintln!("cupertino-login: the wallpaper: {e}"),
        }
    }
    Source::Plain
}

/// What a picture file was when it was read: a file replaced whole (a new inode) or changed in place is another one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stamp {
    inode: u64,
    len: u64,
    modified: (i64, i64),
}

impl Stamp {
    /// The file at `path` now; `None` when there is none. It asks the file system: run it on Kit's I/O pool
    /// (`cupertino_kit::task::io`), never on the UI thread.
    #[expect(
        clippy::disallowed_methods,
        reason = "runs on the I/O pool (kit::task::io)"
    )]
    pub fn of(path: &std::path::Path) -> Option<Stamp> {
        use std::os::unix::fs::MetadataExt;
        let meta = std::fs::metadata(path).ok()?;
        Some(Stamp {
            inode: meta.ino(),
            len: meta.len(),
            modified: (meta.mtime(), meta.mtime_nsec()),
        })
    }
}

/// How many of the users' pictures the login screen keeps decoded.
const KEPT_USERS: usize = 4;

/// The solid colour the login screen stands on where a user has no wallpaper or it does not decode: the deep blue the
/// desktop's own fallback starts from (approximate).
pub const LOGIN_FALLBACK: [u8; 3] = [18, 23, 51];

/// The login screen's users' wallpapers, decoded off the screen's thread as each user is selected and kept for a few
/// users: each file once, again when it changes.
#[derive(Debug, Default)]
pub struct Pictures {
    /// Most recently used last; a file that did not decode is kept as the fallback colour, so it is not read again.
    kept: Vec<(std::path::PathBuf, Stamp, Source)>,
    reading: Option<(std::path::PathBuf, Stamp)>,
}

/// What [`Pictures::want`] says of a user's wallpaper.
#[derive(Clone, Debug)]
pub enum Want {
    /// Show this.
    Ready(Source),
    /// Read this file, as it is now, on the I/O pool ([`decode`]) and hand it to [`Pictures::read`]; until then the screen
    /// shows what it showed.
    Read(std::path::PathBuf, Stamp),
    /// It is being read.
    Reading,
}

impl Pictures {
    /// The wallpaper at `path` (`None`: the user has none) as the file is now: `stamp` is what [`Stamp::of`] said of it,
    /// read on the I/O pool (`None`: there is no such file). Without either the screen shows [`LOGIN_FALLBACK`]. It
    /// touches no file.
    pub fn want(&mut self, path: Option<&std::path::Path>, stamp: Option<Stamp>) -> Want {
        let (Some(path), Some(stamp)) = (path, stamp) else {
            return Want::Ready(Source::Solid(LOGIN_FALLBACK));
        };
        if let Some(i) = self
            .kept
            .iter()
            .position(|(p, s, _)| p == path && *s == stamp)
        {
            let entry = self.kept.remove(i);
            let source = entry.2.clone();
            self.kept.push(entry);
            return Want::Ready(source);
        }
        match &self.reading {
            Some((p, s)) if p == path && *s == stamp => Want::Reading,
            // one file at a time: another is read once the one being read is in
            Some(_) => Want::Reading,
            None => {
                self.reading = Some((path.to_owned(), stamp));
                Want::Read(path.to_owned(), stamp)
            }
        }
    }

    /// What reading `path` as of `stamp` gave.
    pub fn read(
        &mut self,
        path: std::path::PathBuf,
        stamp: Stamp,
        picture: Result<Picture, String>,
    ) {
        if self
            .reading
            .as_ref()
            .is_some_and(|(p, s)| *p == path && *s == stamp)
        {
            self.reading = None;
        }
        let source = match picture {
            Ok(picture) => Source::Picture(Arc::new(picture)),
            Err(e) => {
                eprintln!("cupertino-login: a login screen wallpaper: {e}");
                Source::Solid(LOGIN_FALLBACK)
            }
        };
        self.kept.retain(|(p, _, _)| *p != path);
        self.kept.push((path, stamp, source));
        if self.kept.len() > KEPT_USERS {
            self.kept.remove(0);
        }
    }
}
