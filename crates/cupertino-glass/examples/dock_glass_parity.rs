//! Compares the Dock bar's glass, as the CPU model of the glass composite shades it at a scale of 2, with a render of
//! the same scene given as a PNG, and prints the error in 8-bit levels (the largest over the three channels) by region;
//! `--against` measures another render of the scene in place of the model.
//!
//! The scene: a 300 × 70 pt bar with 22 pt continuous corners at (50, 85) pt over the test backdrop below, 400 × 240 pt
//! at 2 pixels a point, the backdrop drawn with y up. The regions: the interior (more than 3 pt inside the outline),
//! the edge (the outer 3 pt inside it and the antialiased pixel), and the band 3 pt outside it.
//!
//! ```text
//! cargo run -p cupertino-glass --example dock_glass_parity -- SCENE.png light|dark [--render OUT.png] [--diff OUT.png]
//!     [--against OTHER.png]
//! ```
//!
//! The backdrop is captured at the recipe's capture scale with 2 × 2 box filtering and blurred by the recipe's
//! Gaussian there, as the compositor does, and kept in single precision.

use cupertino_glass::glass::{Container, GlassMode, GlassShape, Rgb, SHARP_LEVEL};
use cupertino_raster_math::blur::gaussian_weights;
use cupertino_theme::glass::{GlassParams, GlassStyle};
use image::{Rgba, RgbaImage};

/// Pixels per point of the scene.
const SCALE: f32 = 2.0;
/// The scene's size in points.
const SCENE: [f32; 2] = [400.0, 240.0];
/// The bar: x, y, width and height in points, and its corner radius.
const BAR: [f32; 4] = [50.0, 85.0, 300.0, 70.0];
const BAR_RADIUS: f32 = 22.0;
/// The test backdrop: the checker's square in points and its two colours, the white bar across it (y from, to), the
/// white bar down it (x from, to) and the grey ramp from black at the left to white at the right (y from, to).
const CHECKER: f32 = 20.0;
const CHECKER_ORANGE: [u8; 3] = [242, 140, 26];
const CHECKER_BLUE: [u8; 3] = [38, 89, 217];
const ACROSS: [f32; 2] = [117.0, 123.0];
const DOWN: [f32; 2] = [198.0, 202.0];
const RAMP: [f32; 2] = [200.0, 220.0];
/// How deep the edge region reaches inside the outline, and the band outside it, in points.
const EDGE_DEPTH: f32 = 3.0;

type Img = Vec<Rgb>;

/// The test backdrop at `SCALE`, top row first, drawn with y up.
fn backdrop() -> (Img, usize, usize) {
    let (w, h) = ((SCENE[0] * SCALE) as usize, (SCENE[1] * SCALE) as usize);
    let mut img = vec![[0.0; 3]; w * h];
    for y in 0..h {
        for x in 0..w {
            let (xp, yp) = ((x as f32 + 0.5) / SCALE, (y as f32 + 0.5) / SCALE);
            let odd = ((xp / CHECKER) as i32 + (yp / CHECKER) as i32) & 1 == 1;
            let mut c = if odd { CHECKER_ORANGE } else { CHECKER_BLUE };
            if (ACROSS[0]..ACROSS[1]).contains(&yp) || (DOWN[0]..DOWN[1]).contains(&xp) {
                c = [255; 3];
            }
            if (RAMP[0]..RAMP[1]).contains(&yp) {
                c = [(255.0 * xp / SCENE[0]) as u8; 3];
            }
            img[(h - 1 - y) * w + x] = c.map(|v| f32::from(v) / 255.0);
        }
    }
    (img, w, h)
}

/// `img` (`w` × `h`) halved `steps` times with 2 × 2 box filtering.
fn capture(img: &Img, w: usize, h: usize, steps: u32) -> (Img, usize, usize) {
    let (mut img, mut w, mut h) = (img.clone(), w, h);
    for _ in 0..steps {
        let (cw, ch) = (w / 2, h / 2);
        let mut out = vec![[0.0; 3]; cw * ch];
        for y in 0..ch {
            for x in 0..cw {
                let at = |dx: usize, dy: usize| img[(2 * y + dy) * w + 2 * x + dx];
                let (a, b, c, d) = (at(0, 0), at(1, 0), at(0, 1), at(1, 1));
                out[y * cw + x] = std::array::from_fn(|k| 0.25 * (a[k] + b[k] + c[k] + d[k]));
            }
        }
        (img, w, h) = (out, cw, ch);
    }
    (img, w, h)
}

/// `img` blurred by a separable Gaussian of `sigma` texels, its edge texels repeated past it.
fn blur(img: &Img, w: usize, h: usize, sigma: f32) -> Img {
    let wts = gaussian_weights(sigma);
    let pass = |src: &Img, dx: i64, dy: i64| -> Img {
        let at = |x: i64, y: i64| {
            src[y.clamp(0, h as i64 - 1) as usize * w + x.clamp(0, w as i64 - 1) as usize]
        };
        let mut out = vec![[0.0; 3]; w * h];
        for y in 0..h as i64 {
            for x in 0..w as i64 {
                let mut c = at(x, y).map(|v| v * wts[0]);
                for (i, wt) in wts.iter().enumerate().skip(1) {
                    let (a, b) = (
                        at(x + dx * i as i64, y + dy * i as i64),
                        at(x - dx * i as i64, y - dy * i as i64),
                    );
                    for k in 0..3 {
                        c[k] += (a[k] + b[k]) * wt;
                    }
                }
                out[y as usize * w + x as usize] = c;
            }
        }
        out
    };
    pass(&pass(img, 1, 0), 0, 1)
}

/// `img` read at `q` (texels, y down) through a bilinear filter clamped to its edge.
fn bilinear(img: &Img, w: usize, h: usize, q: [f32; 2]) -> Rgb {
    let (x, y) = (q[0] - 0.5, q[1] - 0.5);
    let (x0, y0) = (x.floor(), y.floor());
    let (fx, fy) = (x - x0, y - y0);
    let at = |xi: i64, yi: i64| {
        img[yi.clamp(0, h as i64 - 1) as usize * w + xi.clamp(0, w as i64 - 1) as usize]
    };
    let (x0, y0) = (x0 as i64, y0 as i64);
    let (a, b, c, d) = (
        at(x0, y0),
        at(x0 + 1, y0),
        at(x0, y0 + 1),
        at(x0 + 1, y0 + 1),
    );
    std::array::from_fn(|i| {
        let top = a[i] + (b[i] - a[i]) * fx;
        let bottom = c[i] + (d[i] - c[i]) * fx;
        top + (bottom - top) * fy
    })
}

/// `p` with every length in units of `k` of its own: a recipe in points as the composite reads it in pixels.
fn in_pixels(mut p: GlassParams, k: f32) -> GlassParams {
    let r = &mut p.refraction;
    (r.amount, r.height, r.outer_amount, r.outer_height) = (
        r.amount * k,
        r.height * k,
        r.outer_amount * k,
        r.outer_height * k,
    );
    let b = &mut p.bleed;
    (b.amount, b.height, b.blur) = (b.amount * k, b.height * k, b.blur * k);
    let h = &mut p.highlight;
    (h.height, h.offset, h.glow_height, h.glow_offset) = (
        h.height * k,
        h.offset * k,
        h.glow_height * k,
        h.glow_offset * k,
    );
    p.specular.height *= k;
    let s = &mut p.shadow;
    s.offset = s.offset.map(|v| v * k);
    (
        s.height,
        s.ring_blur,
        s.ring_stroke,
        s.ring_offset,
        s.outline_width,
    ) = (
        s.height * k,
        s.ring_blur * k,
        s.ring_stroke * k,
        s.ring_offset * k,
        s.outline_width * k,
    );
    p.border.width *= k;
    p
}

fn load(path: &str) -> Result<(Img, usize, usize), String> {
    let img = image::open(path)
        .map_err(|e| format!("{path}: {e}"))?
        .to_rgba8();
    let (w, h) = (img.width() as usize, img.height() as usize);
    let px = img
        .pixels()
        .map(|p| [p[0], p[1], p[2]].map(|v| f32::from(v) / 255.0))
        .collect();
    Ok((px, w, h))
}

fn save(img: &Img, w: usize, h: usize, path: &str) -> Result<(), String> {
    RgbaImage::from_fn(w as u32, h as u32, |x, y| {
        let c = img[y as usize * w + x as usize].map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8);
        Rgba([c[0], c[1], c[2], 255])
    })
    .save(path)
    .map_err(|e| format!("{path}: {e}"))
}

/// Mean, median, 99th percentile and largest of `v`, sorted in place.
fn stats(name: &str, v: &mut [f32]) {
    v.sort_by(f32::total_cmp);
    if v.is_empty() {
        return;
    }
    let mean = v.iter().map(|&e| f64::from(e)).sum::<f64>() / v.len() as f64;
    let at = |q: f64| v[((v.len() as f64 * q) as usize).min(v.len() - 1)];
    println!(
        "{name:<8} mean {mean:5.2}  p50 {:5.1}  p99 {:5.1}  max {:5.1}  ({} px)",
        at(0.5),
        at(0.99),
        v[v.len() - 1],
        v.len()
    );
}

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let usage = "dock_glass_parity SCENE.png light|dark [--render OUT.png] [--diff OUT.png] [--against OTHER.png]";
    let (Some(scene), Some(appearance)) = (args.first(), args.get(1)) else {
        return Err(usage.into());
    };
    let dark = match appearance.as_str() {
        "light" => false,
        "dark" => true,
        _ => return Err(usage.into()),
    };
    let flag = |name: &str| {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
    };
    let (expected, ew, eh) = load(scene)?;
    let (bg, w, h) = backdrop();
    if (ew, eh) != (w, h) {
        return Err(format!("{scene} is {ew} × {eh}, not {w} × {h}"));
    }

    let mode = GlassMode::of_style(dark, GlassStyle::Bar);
    let points = GlassParams::new(mode, BAR[2].min(BAR[3]));
    // the capture: halved until it reaches the recipe's scale, blurred there by its Gaussian
    let steps = (1.0 / points.capture_scale).log2().round().max(0.0) as u32;
    let (cap, cw, ch) = capture(&bg, w, h, steps);
    let texels_per_point = cw as f32 / SCENE[0];
    let blurred = blur(&cap, cw, ch, points.blur_sigma() * texels_per_point);
    let px = in_pixels(points, SCALE);
    let shape = GlassShape::new(BAR.map(|v| v * SCALE), BAR_RADIUS * SCALE);
    let mut container = Container::new(vec![shape], 0.0, mode).with_shape_recipe(0, px);
    container.params = px;
    let to_texels = cw as f32 / w as f32;
    let sample = |level: usize, q: [f32; 2]| {
        let t = [q[0] * to_texels, q[1] * to_texels];
        if level == SHARP_LEVEL {
            bilinear(&cap, cw, ch, t)
        } else {
            bilinear(&blurred, cw, ch, t)
        }
    };

    // another render of the scene in place of the model, to measure it the same way
    let against = match flag("--against") {
        Some(path) => Some(load(path)?.0),
        None => None,
    };
    let mut out = bg.clone();
    let (mut inner, mut edge, mut outside) = (Vec::new(), Vec::new(), Vec::new());
    let mut diff = vec![[0.0; 3]; w * h];
    for y in 0..h {
        for x in 0..w {
            let p = [x as f32 + 0.5, y as f32 + 0.5];
            let i = y * w + x;
            let (c, a) = container.shade(p, &sample);
            let o: Rgb = match &against {
                Some(img) => img[i],
                None => std::array::from_fn(|k| {
                    let v = c[k].clamp(0.0, 1.0) * a + bg[i][k] * (1.0 - a);
                    (v.clamp(0.0, 1.0) * 255.0).round() / 255.0
                }),
            };
            out[i] = o;
            let e = (0..3)
                .map(|k| (o[k] - expected[i][k]).abs() * 255.0)
                .fold(0.0, f32::max);
            diff[i] = [(e / 32.0).min(1.0); 3];
            let d = shape.sdf(p);
            if d < -EDGE_DEPTH * SCALE {
                inner.push(e);
            } else if d < 0.5 {
                edge.push(e);
            } else if d < EDGE_DEPTH * SCALE {
                outside.push(e);
            }
        }
    }
    stats("inner", &mut inner);
    stats("edge", &mut edge);
    stats("outside", &mut outside);
    if let Some(path) = flag("--render") {
        save(&out, w, h, path)?;
    }
    if let Some(path) = flag("--diff") {
        save(&diff, w, h, path)?;
    }
    Ok(())
}

fn main() {
    if let Err(e) = run() {
        eprintln!("{e}");
        std::process::exit(1);
    }
}
