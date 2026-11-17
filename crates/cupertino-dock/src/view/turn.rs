//! Pictures turned about a point. A window draws its glyphs and its images upright, so a part that stands at an angle
//! (a row of a folder's fan) is first drawn level into a picture of its own, finer than the device's pixels, and that
//! picture is then drawn turned, sampled once for each device pixel it covers.

use cupertino_kit::geometry::{Point, Rect};

/// The most pixels a turned picture has: a larger one is not drawn.
const MAX_PIXELS: u64 = 4 << 20;
/// A device pixel of a turned picture is one sample of a source about as fine as the device, and the mean of two
/// samples a side of a source finer than this many of its pixels to a device pixel.
const FINE: f64 = 1.5;

/// A picture in premultiplied RGBA, one byte a component, row after row from the top.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bitmap {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

impl Bitmap {
    /// A clear picture.
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            pixels: vec![0; width as usize * height as usize * 4],
        }
    }

    /// The pixel at (`x`, `y`), clear outside the picture.
    fn pixel(&self, x: i64, y: i64) -> [f64; 4] {
        if x < 0 || y < 0 || x >= i64::from(self.width) || y >= i64::from(self.height) {
            return [0.0; 4];
        }
        let at = (y as usize * self.width as usize + x as usize) * 4;
        [0, 1, 2, 3].map(|c| f64::from(self.pixels[at + c]))
    }

    /// The picture at the point (`x`, `y`) in pixels, the middle of the first pixel being (0.5, 0.5): between the four
    /// pixels around it by their distances.
    fn sample(&self, x: f64, y: f64) -> [f64; 4] {
        let (fx, fy) = ((x - 0.5).floor(), (y - 0.5).floor());
        let (tx, ty) = (x - 0.5 - fx, y - 0.5 - fy);
        let (ix, iy) = (fx as i64, fy as i64);
        let (width, height) = (i64::from(self.width), i64::from(self.height));
        if ix < -1 || iy < -1 || ix >= width || iy >= height {
            return [0.0; 4];
        }
        let mut out = [0.0; 4];
        if ix >= 0 && iy >= 0 && ix + 1 < width && iy + 1 < height {
            // the four pixels are all in the picture
            let stride = self.width as usize * 4;
            let at = iy as usize * stride + ix as usize * 4;
            let top = &self.pixels[at..at + 8];
            let bottom = &self.pixels[at + stride..at + stride + 8];
            for (c, value) in out.iter_mut().enumerate() {
                let (a, b) = (f64::from(top[c]), f64::from(top[c + 4]));
                let (below, beside) = (f64::from(bottom[c]), f64::from(bottom[c + 4]));
                let upper = a + (b - a) * tx;
                let lower = below + (beside - below) * tx;
                *value = upper + (lower - upper) * ty;
            }
            return out;
        }
        let (a, b) = (self.pixel(ix, iy), self.pixel(ix + 1, iy));
        let (c, d) = (self.pixel(ix, iy + 1), self.pixel(ix + 1, iy + 1));
        for (i, value) in out.iter_mut().enumerate() {
            let upper = a[i] + (b[i] - a[i]) * tx;
            let lower = c[i] + (d[i] - c[i]) * tx;
            *value = upper + (lower - upper) * ty;
        }
        out
    }

    /// The picture at half its size, each pixel the mean of the four it takes the place of.
    fn halved(&self) -> Self {
        let (width, height) = (self.width.div_ceil(2), self.height.div_ceil(2));
        let mut half = Self::new(width, height);
        for y in 0..height {
            for x in 0..width {
                let (sx, sy) = (i64::from(x) * 2, i64::from(y) * 2);
                let four = [
                    self.pixel(sx, sy),
                    self.pixel(sx + 1, sy),
                    self.pixel(sx, sy + 1),
                    self.pixel(sx + 1, sy + 1),
                ];
                let at = (y as usize * width as usize + x as usize) * 4;
                for c in 0..4 {
                    let mean = four.iter().map(|p| p[c]).sum::<f64>() / 4.0;
                    half.pixels[at + c] = mean.round() as u8;
                }
            }
        }
        half
    }

    /// Lays `coverage` (one byte a pixel, `stride` bytes a row, `width` by `height`) in the colour `rgb` over the
    /// picture, its top-left corner at (`x`, `y`).
    #[allow(clippy::too_many_arguments)]
    pub fn cover(
        &mut self,
        coverage: &[u8],
        stride: usize,
        width: u32,
        height: u32,
        x: i64,
        y: i64,
        rgb: [f32; 3],
    ) {
        for row in 0..height as usize {
            for column in 0..width as usize {
                let Some(&cover) = coverage.get(row * stride + column) else {
                    return;
                };
                let alpha = f64::from(cover) / 255.0;
                let source = [
                    f64::from(rgb[0]) * 255.0 * alpha,
                    f64::from(rgb[1]) * 255.0 * alpha,
                    f64::from(rgb[2]) * 255.0 * alpha,
                    255.0 * alpha,
                ];
                self.over(x + column as i64, y + row as i64, source, alpha);
            }
        }
    }

    /// Lays a premultiplied RGBA picture (`stride` bytes a row, `width` by `height`) over the picture, its top-left
    /// corner at (`x`, `y`).
    pub fn draw(&mut self, rgba: &[u8], stride: usize, width: u32, height: u32, x: i64, y: i64) {
        for row in 0..height as usize {
            for column in 0..width as usize {
                let Some(px) = rgba.get(row * stride + column * 4..row * stride + column * 4 + 4)
                else {
                    return;
                };
                let source = [0, 1, 2, 3].map(|c| f64::from(px[c]));
                self.over(
                    x + column as i64,
                    y + row as i64,
                    source,
                    f64::from(px[3]) / 255.0,
                );
            }
        }
    }

    /// The premultiplied `source` of opacity `alpha` over the pixel at (`x`, `y`).
    fn over(&mut self, x: i64, y: i64, source: [f64; 4], alpha: f64) {
        if x < 0 || y < 0 || x >= i64::from(self.width) || y >= i64::from(self.height) {
            return;
        }
        let at = (y as usize * self.width as usize + x as usize) * 4;
        for (c, value) in source.iter().enumerate() {
            let under = f64::from(self.pixels[at + c]);
            self.pixels[at + c] = (value + under * (1.0 - alpha)).round().min(255.0) as u8;
        }
    }
}

/// `p` turned clockwise by the `(sin, cos)` of an angle about `pivot`.
pub fn turn_point(p: Point, pivot: Point, (sin, cos): (f64, f64)) -> Point {
    let (x, y) = (p.x - pivot.x, p.y - pivot.y);
    Point::new(pivot.x + x * cos - y * sin, pivot.y + x * sin + y * cos)
}

/// A picture as it is drawn turned: its pixels are device pixels, and `rect` is where they go, in points.
#[derive(Clone, Debug, PartialEq)]
pub struct Turned {
    pub bitmap: Bitmap,
    pub rect: Rect,
}

/// `source`, which standing level fills `placed` (points), turned clockwise by `angle` radians about `pivot` and
/// shown at `alpha`, for a device of `scale` pixels a point: the device pixels the turned picture covers, each the mean
/// of what the picture shows across it when `fine`, and what it shows at the pixel's middle otherwise (a picture on
/// its way, drawn anew at every frame). `None` for a picture that covers nothing or too much.
pub fn turned(
    source: &Bitmap,
    placed: Rect,
    pivot: Point,
    angle: f64,
    scale: f64,
    alpha: f64,
    fine: bool,
) -> Option<Turned> {
    if source.width == 0
        || source.height == 0
        || placed.width() <= 0.0
        || placed.height() <= 0.0
        || !(scale.is_finite() && scale > 0.0 && angle.is_finite())
    {
        return None;
    }
    // a source much finer than the device is halved first, so the samples of a device pixel miss none of it
    let mut halved;
    let mut source = source;
    while f64::from(source.width) / (placed.width() * scale) > 2.0 && source.width > 1 {
        halved = source.halved();
        source = &halved;
    }
    let sin_cos = angle.sin_cos();
    let corners = [
        Point::new(placed.min_x(), placed.min_y()),
        Point::new(placed.max_x(), placed.min_y()),
        Point::new(placed.max_x(), placed.max_y()),
        Point::new(placed.min_x(), placed.max_y()),
    ]
    .map(|c| turn_point(c, pivot, sin_cos));
    let bound = |pick: fn(&Point) -> f64, least: bool| {
        let all = corners.iter().map(pick);
        if least {
            (all.fold(f64::INFINITY, f64::min) * scale).floor()
        } else {
            (all.fold(f64::NEG_INFINITY, f64::max) * scale).ceil()
        }
    };
    let (x0, x1) = (bound(|p| p.x, true), bound(|p| p.x, false));
    let (y0, y1) = (bound(|p| p.y, true), bound(|p| p.y, false));
    let (width, height) = (x1 - x0, y1 - y0);
    if !(width >= 1.0 && height >= 1.0) || width * height > MAX_PIXELS as f64 {
        return None;
    }
    let mut bitmap = Bitmap::new(width as u32, height as u32);
    // back from the turned picture to the level one
    let back = (-sin_cos.0, sin_cos.1);
    let per_x = f64::from(source.width) / placed.width();
    let per_y = f64::from(source.height) / placed.height();
    let samples: u32 = if fine && per_x / scale > FINE { 2 } else { 1 };
    let share = alpha.clamp(0.0, 1.0) / f64::from(samples * samples);
    for row in 0..bitmap.height {
        for column in 0..bitmap.width {
            let mut sum = [0.0; 4];
            for sy in 0..samples {
                for sx in 0..samples {
                    let part = |i: u32| (f64::from(i) + 0.5) / f64::from(samples);
                    let device = Point::new(
                        (x0 + f64::from(column) + part(sx)) / scale,
                        (y0 + f64::from(row) + part(sy)) / scale,
                    );
                    let level = turn_point(device, pivot, back);
                    let got = source.sample(
                        (level.x - placed.min_x()) * per_x,
                        (level.y - placed.min_y()) * per_y,
                    );
                    for (total, part) in sum.iter_mut().zip(got) {
                        *total += part;
                    }
                }
            }
            let at = (row as usize * bitmap.width as usize + column as usize) * 4;
            for (c, total) in sum.iter().enumerate() {
                bitmap.pixels[at + c] = (total * share).round().min(255.0) as u8;
            }
        }
    }
    Some(Turned {
        bitmap,
        rect: Rect::new(x0 / scale, y0 / scale, width / scale, height / scale),
    })
}
