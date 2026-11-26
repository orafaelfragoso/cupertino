//! The stages of the glass composite: the recipes of today shade as they did, bit for bit on the CPU model and byte for
//! byte on the device, and the glass program agrees with the CPU model.
//!
//! The device is a surfaceless EGL context; a machine without one has nothing to draw with and the device tests say so.

use cupertino_glass::glass::{Container, GlassEdge, GlassMode, GlassShape, GlassUniform, Rgb};
use cupertino_glass::gpu::HeadlessGpu;
use cupertino_glass::shaders;
use cupertino_theme::glass::GlassStyle;
use image::{Rgba, RgbaImage};

/// The size of the target every scene is drawn in, in pixels (one per point).
const W: u32 = 160;
const H: u32 = 100;

fn gpu() -> Option<HeadlessGpu> {
    let gpu = HeadlessGpu::new();
    if gpu.is_none() {
        eprintln!("no headless graphics device: the glass program is not exercised");
    }
    gpu
}

fn mode(style: GlassStyle, dark: bool) -> GlassMode {
    GlassMode::of_style(dark, style)
}

/// The containers every check runs over: each style's paths through the stages (one shape, joined shapes, a capsule,
/// a soft band with its diffuse band, drop and ring shadows, an edge line kept to the sides, the outline hairline and a
/// container edge).
pub fn scenes() -> Vec<(&'static str, Container)> {
    let panel = GlassShape::new([20.0, 16.0, 120.0, 64.0], 16.0);
    let pair = vec![
        GlassShape::new([14.0, 20.0, 60.0, 56.0], 14.0),
        GlassShape::new([80.0, 24.0, 64.0, 48.0], 12.0),
    ];
    let capsule = GlassShape::capsule([24.0, 30.0, 112.0, 40.0]);
    let mut popover = Container::new(vec![panel], 0.0, mode(GlassStyle::Dense, false));
    popover.params.shadow.outline_opacity = 0.3;
    popover.params.shadow.outline_width = 1.0;
    let mut reduced = mode(GlassStyle::Regular, true);
    reduced.reduce_transparency = true;
    vec![
        (
            "regular dark",
            Container::new(vec![panel], 0.0, mode(GlassStyle::Regular, true)),
        ),
        (
            "regular light pair",
            Container::new(pair.clone(), 12.0, mode(GlassStyle::Regular, false)),
        ),
        (
            "clear dark capsule",
            Container::new(vec![capsule], 0.0, mode(GlassStyle::Clear, true)),
        ),
        (
            "large panel dark",
            Container::new(vec![panel], 0.0, mode(GlassStyle::LargePanel, true)),
        ),
        (
            "card light",
            Container::new(vec![panel], 0.0, mode(GlassStyle::Card, false)),
        ),
        (
            "module pair",
            Container::new(pair.clone(), 0.0, mode(GlassStyle::Module, true)),
        ),
        (
            "bar light capsule",
            Container::new(vec![capsule], 0.0, mode(GlassStyle::Bar, false)),
        ),
        ("popover outline", popover),
        (
            "toolbar edge",
            Container::new(pair, 8.0, mode(GlassStyle::Toolbar, false)).with_edge(GlassEdge {
                width: 1.0,
                color: [0.0, 0.0, 0.0, 0.25],
                soft: 0.0,
            }),
        ),
        (
            "reduced transparency",
            Container::new(vec![capsule], 0.0, reduced),
        ),
    ]
}

/// The texture the program reads at binding `i + 1`: a smooth field of colour of its own, so a sample between texels
/// reads the same on the device as through [`bilinear`].
pub fn level_image(i: usize) -> RgbaImage {
    let k = i as f32 + 1.0;
    RgbaImage::from_fn(W, H, |x, y| {
        let (u, v) = (x as f32 / W as f32, y as f32 / H as f32);
        let r = 0.5 + 0.4 * (3.0 * u * k + 0.7 * k).sin();
        let g = 0.45 + 0.4 * (2.5 * v + 1.3 * u * k).cos();
        let b = 0.2 + 0.6 * u * v + 0.1 * k / 7.0;
        Rgba([r, g, b, 1.0].map(|c: f32| (c.clamp(0.0, 1.0) * 255.0).round() as u8))
    })
}

/// The image read at `q` (pixels, y down) through a bilinear filter clamped to its edge, as the device's sampler reads
/// it.
pub fn bilinear(img: &RgbaImage, q: [f32; 2]) -> Rgb {
    let (w, h) = (img.width() as i64, img.height() as i64);
    let (x, y) = (q[0] - 0.5, q[1] - 0.5);
    let (x0, y0) = (x.floor(), y.floor());
    let (fx, fy) = (x - x0, y - y0);
    let at = |xi: i64, yi: i64| {
        let p = img.get_pixel(xi.clamp(0, w - 1) as u32, yi.clamp(0, h - 1) as u32);
        [p[0], p[1], p[2]].map(|c| f32::from(c) / 255.0)
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

/// The textures of the glass program, in binding order.
pub fn textures() -> Vec<RgbaImage> {
    (0..shaders::GLASS_TEXTURES.len())
        .map(level_image)
        .collect()
}

/// The uniform of a scene: one point per target pixel, the textures one texel per pixel.
pub fn uniform(c: &Container) -> GlassUniform {
    let uv = [[1.0 / W as f32, 0.0, 0.0], [0.0, 1.0 / H as f32, 0.0]];
    GlassUniform::new(
        c,
        1.0,
        uv,
        [0.0, 0.0, W as f32, H as f32],
        [W as f32, H as f32],
        1.0,
    )
}

/// The CPU model of a scene over the target: each pixel's colour times its coverage, as it lands over black.
pub fn model(c: &Container, textures: &[RgbaImage]) -> Vec<(Rgb, f32)> {
    let sample = |level: usize, q: [f32; 2]| bilinear(&textures[level], q);
    let mut out = Vec::with_capacity((W * H) as usize);
    for y in 0..H {
        for x in 0..W {
            out.push(c.shade([x as f32 + 0.5, y as f32 + 0.5], &sample));
        }
    }
    out
}

/// A fold of every bit of `values` (FNV-1a over their bytes).
pub fn fingerprint(values: impl IntoIterator<Item = u32>) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for v in values {
        for b in v.to_le_bytes() {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
    }
    h
}

fn model_fingerprint(c: &Container, textures: &[RgbaImage]) -> u64 {
    fingerprint(
        model(c, textures)
            .into_iter()
            .flat_map(|(rgb, a)| [rgb[0], rgb[1], rgb[2], a].map(f32::to_bits)),
    )
}

#[test]
fn the_cpu_model_of_todays_recipes_is_unchanged() {
    let textures = textures();
    let got: Vec<(&str, u64)> = scenes()
        .iter()
        .map(|(name, c)| (*name, model_fingerprint(c, &textures)))
        .collect();
    assert_eq!(got, MODEL_FINGERPRINTS.to_vec());
}

#[test]
fn the_glass_program_draws_todays_recipes_as_before() {
    let Some(gpu) = gpu() else { return };
    let textures = textures();
    let got: Vec<(&str, u64)> = scenes()
        .iter()
        .map(|(name, c)| {
            let img = gpu.render_glass(&uniform(c), &textures, (W, H)).unwrap();
            (
                *name,
                fingerprint(img.as_raw().iter().map(|b| u32::from(*b))),
            )
        })
        .collect();
    assert_eq!(got, DEVICE_FINGERPRINTS.to_vec());
}

/// How far the device may land from the CPU model on one channel, in 8-bit steps, and on how many pixels in a
/// thousand it may land further (the antialiased edges, where the two take the field's slope apart).
const CHANNEL_TOLERANCE: u8 = 3;
const FAR_PER_MILLE: usize = 15;

/// Whether the device's drawing of `c` agrees with the CPU model; the worst pixel otherwise.
pub fn agrees(gpu: &HeadlessGpu, c: &Container) -> Result<(), String> {
    let textures = textures();
    let img = gpu.render_glass(&uniform(c), &textures, (W, H)).unwrap();
    let expected = model(c, &textures);
    let mut far = 0;
    let mut worst = (0u8, 0u32, 0u32);
    for (i, (rgb, a)) in expected.iter().enumerate() {
        let (x, y) = (i as u32 % W, i as u32 / W);
        let p = img.get_pixel(x, y);
        let diff = (0..3)
            .map(|k| {
                let e = ((rgb[k] * a).clamp(0.0, 1.0) * 255.0).round() as u8;
                e.abs_diff(p[k])
            })
            .max()
            .unwrap_or(0);
        if diff > CHANNEL_TOLERANCE {
            far += 1;
        }
        if diff > worst.0 {
            worst = (diff, x, y);
        }
    }
    if far * 1000 > FAR_PER_MILLE * expected.len() {
        return Err(format!(
            "{far} pixels differ by more than {CHANNEL_TOLERANCE}; the worst by {} at ({}, {})",
            worst.0, worst.1, worst.2
        ));
    }
    Ok(())
}

#[test]
fn the_glass_program_agrees_with_the_cpu_model() {
    let Some(gpu) = gpu() else { return };
    for (name, c) in scenes() {
        if let Err(e) = agrees(&gpu, &c) {
            panic!("{name}: {e}");
        }
    }
}

const MODEL_FINGERPRINTS: [(&str, u64); 10] = [
    ("regular dark", 4984355373766000757),
    ("regular light pair", 8232545495425435992),
    ("clear dark capsule", 17406019792880935583),
    ("large panel dark", 17965285217899160828),
    ("card light", 6920963231937255085),
    ("module pair", 11254255364824064283),
    ("bar light capsule", 5221462194324128147),
    ("popover outline", 4029269704112854822),
    ("toolbar edge", 533714028573828113),
    ("reduced transparency", 7841611908674553762),
];

const DEVICE_FINGERPRINTS: [(&str, u64); 10] = [
    ("regular dark", 15539228235446619217),
    ("regular light pair", 12585710504181001491),
    ("clear dark capsule", 9177257824171055044),
    ("large panel dark", 17064248079944181856),
    ("card light", 8456804677487394069),
    ("module pair", 1010109452199837771),
    ("bar light capsule", 10284589693616193943),
    ("popover outline", 11123273970963773877),
    ("toolbar edge", 14008253558358335584),
    ("reduced transparency", 16664983129397743722),
];
