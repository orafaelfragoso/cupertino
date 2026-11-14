//! GPU tests: GPU vs CPU reference, rounded mask, backdrop cache, vibrant blending.
//! Every test skips (passes) when no device is available, and fails then with `CUPERTINO_GPU_REQUIRED=1`.

use cupertino_geometry::{CornerStyle, Point};
use cupertino_glass::gpu::{average_color, procedural_wallpaper, psnr, HeadlessGpu};
use cupertino_glass::ops::color_ops;
use cupertino_glass::renderer::{FrameDesc, MaterialRegion, MaterialRenderer, TargetInit};
use cupertino_glass::{cpu, VibrantDraw, VibrantMode, VibrantStyle};
use cupertino_theme::{Appearance, Backdrop, Blend, Fill, MaterialOptions, Recipe, Theme};

fn gpu() -> Option<HeadlessGpu> {
    let g = HeadlessGpu::new();
    if g.is_none() {
        assert!(
            std::env::var("CUPERTINO_GPU_REQUIRED").as_deref() != Ok("1"),
            "no device"
        );
        eprintln!("skipping: no device available");
    }
    g
}

fn recipe(theme: &Theme, a: Appearance, m: &str) -> Recipe {
    theme.recipe(a, m, &MaterialOptions::default()).unwrap()
}

fn solid(rgba: [f32; 4]) -> Recipe {
    Recipe {
        fills: vec![Fill {
            rgba,
            blend: Blend::Normal,
            tint_layer: false,
        }],
        ..Default::default()
    }
}

#[test]
fn gpu_matches_cpu_reference_psnr_at_least_60_db() {
    let Some(g) = gpu() else { return };
    let theme = Theme::load().unwrap();
    let (w, h) = (640u32, 360u32);
    let wall = procedural_wallpaper(w, h);
    let tint = average_color(&wall);
    for (appearance, scale) in [(Appearance::Light, 1.0f32), (Appearance::Dark, 2.0)] {
        let bar = recipe(&theme, appearance, "MenuBar");
        let side = recipe(&theme, appearance, "Sidebar");
        let menu = recipe(&theme, appearance, "Menu");
        let title = recipe(&theme, appearance, "Titlebar");
        let sel = recipe(&theme, appearance, "MenuBarSelection");
        let regions = [
            MaterialRegion::new([0.0, 0.0, w as f32, 24.0], &bar),
            MaterialRegion::new([20.0, 50.0, 150.0, 260.0], &side)
                .with_corner(12.0, CornerStyle::continuous()),
            MaterialRegion::new([200.0, 60.0, 220.0, 30.0], &title)
                .with_corner(8.0, CornerStyle::Circular),
            MaterialRegion::new([220.0, 110.0, 200.0, 180.0], &menu)
                .with_corner(10.0, CornerStyle::continuous()),
            MaterialRegion::from_recipe([440.0, 4.0, 60.0, 16.0], &sel, 1.0),
        ];
        let gpu_img = g.render_once(&wall, &regions, scale, tint).unwrap();
        let cpu_img = cpu::render_regions(&wall, &regions, scale, tint, false, false);
        let db = psnr(&gpu_img, &cpu_img);
        println!("{appearance:?} scale {scale}: GPU vs CPU {db:.2} dB");
        assert!(db >= 60.0, "{appearance:?} PSNR {db:.2} dB");
    }
}

#[test]
fn partial_opacity_stays_within_two_levels_of_cpu() {
    // Hardware blending quantises the source alpha, so opacity < 1 is checked by maximum error
    // (a PSNR floor would measure the blender, not the shaders).
    let Some(g) = gpu() else { return };
    let theme = Theme::load().unwrap();
    let wall = procedural_wallpaper(320, 200);
    let tint = average_color(&wall);
    let side = recipe(&theme, Appearance::Light, "Sidebar");
    let regions = [MaterialRegion::new([20.0, 20.0, 150.0, 160.0], &side)
        .with_corner(12.0, CornerStyle::continuous())
        .with_opacity(0.9)];
    let a = g.render_once(&wall, &regions, 1.0, tint).unwrap();
    let b = cpu::render_regions(&wall, &regions, 1.0, tint, false, false);
    let worst = a
        .as_raw()
        .iter()
        .zip(b.as_raw())
        .map(|(x, y)| (*x as i32 - *y as i32).abs())
        .max()
        .unwrap();
    assert!(worst <= 2, "worst channel error {worst}");
}

#[test]
fn rounded_mask_matches_geometry_coverage() {
    let Some(g) = gpu() else { return };
    let src_px = [30u8, 34, 40];
    let wall = image::RgbaImage::from_pixel(
        128,
        128,
        image::Rgba([src_px[0], src_px[1], src_px[2], 255]),
    );
    let red = solid([1.0, 0.0, 0.0, 1.0]);
    for style in [
        CornerStyle::Circular,
        CornerStyle::continuous(),
        CornerStyle::Continuous { expansion: 1.25 },
    ] {
        let region = MaterialRegion::new([16.0, 16.0, 96.0, 96.0], &red).with_corner(28.0, style);
        let img = g.render_once(&wall, &[region], 1.0, [0.5; 3]).unwrap();
        let shape = region.rounded_rect();
        let mut worst_approx = 0.0f32;
        let mut worst_exact = 0.0f32;
        for y in 0..128u32 {
            for x in 0..128u32 {
                let p = Point::new(x as f64 + 0.5, y as f64 + 0.5);
                let got = img.get_pixel(x, y).0;
                for (shape_cov, worst) in [
                    (shape.coverage_approx(p, 1.0), &mut worst_approx),
                    (shape.coverage(p, 1.0), &mut worst_exact),
                ] {
                    let expect_r =
                        255.0 * shape_cov as f32 + src_px[0] as f32 * (1.0 - shape_cov as f32);
                    *worst = worst.max((got[0] as f32 - expect_r).abs());
                    let expect_g = src_px[1] as f32 * (1.0 - shape_cov as f32);
                    *worst = worst.max((got[1] as f32 - expect_g).abs());
                }
            }
        }
        assert!(
            worst_approx <= 2.0,
            "{style:?}: GPU vs closed-form mask differs by {worst_approx} levels"
        );
        assert!(
            worst_exact <= 14.0,
            "{style:?}: GPU vs reference SDF mask differs by {worst_exact} levels"
        );
        assert_eq!(img.get_pixel(64, 64).0, [255, 0, 0, 255], "interior");
        assert_eq!(
            img.get_pixel(17, 17).0[..3],
            src_px,
            "sharp corner pixel stays untouched"
        );
        assert_eq!(img.get_pixel(5, 5).0[..3], src_px, "outside");
        assert_eq!(img.get_pixel(64, 20).0, [255, 0, 0, 255], "edge interior");
    }
}

#[test]
fn circle_area_and_opacity() {
    let Some(g) = gpu() else { return };
    let wall = image::RgbaImage::from_pixel(128, 128, image::Rgba([0, 0, 0, 255]));
    let white = solid([1.0, 1.0, 1.0, 1.0]);
    let region = MaterialRegion::new([8.0, 8.0, 112.0, 112.0], &white)
        .with_corner(56.0, CornerStyle::Circular);
    let img = g.render_once(&wall, &[region], 1.0, [0.5; 3]).unwrap();
    let area: f64 = img.pixels().map(|p| p[0] as f64 / 255.0).sum();
    let expect = std::f64::consts::PI * 56.0 * 56.0;
    assert!(
        (area - expect).abs() / expect < 0.005,
        "area {area} vs {expect}"
    );
    let half = g
        .render_once(&wall, &[region.with_opacity(0.5)], 1.0, [0.5; 3])
        .unwrap();
    let centre = half.get_pixel(64, 64).0[0] as i32;
    assert!((centre - 128).abs() <= 1, "opacity 0.5 → {centre}");
}

#[test]
fn backdrop_cache_keyed_by_generation() {
    let Some(g) = gpu() else { return };
    let theme = Theme::load().unwrap();
    let (w, h) = (256u32, 160u32);
    let wall = procedural_wallpaper(w, h);
    let side = recipe(&theme, Appearance::Light, "Sidebar");
    let menu = recipe(&theme, Appearance::Light, "Menu");
    let bar = recipe(&theme, Appearance::Light, "MenuBar");
    let src = g.upload(&wall).unwrap();
    let dst = g.target(w, h).unwrap();
    let mut r = MaterialRenderer::new(&g.device).unwrap();
    let frame = |r: &mut MaterialRenderer, generation: u64, regions: &[MaterialRegion]| {
        r.render(
            &g.device,
            &FrameDesc {
                source: &src,
                source_size: (w, h),
                target: &dst,
                target_size: (w, h),
                display_scale: 1.0,
                damage_generation: generation,
                tint_color: [0.5; 3],
                init: TargetInit::CopySource,
            },
            regions,
        )
        .unwrap()
    };
    let a = MaterialRegion::new([10.0, 10.0, 100.0, 120.0], &side)
        .with_corner(8.0, CornerStyle::Circular);
    let b = MaterialRegion::new([130.0, 10.0, 100.0, 120.0], &side)
        .with_corner(8.0, CornerStyle::Circular);
    let c = MaterialRegion::new([0.0, 0.0, 256.0, 24.0], &bar);
    let d = MaterialRegion::new([130.0, 40.0, 100.0, 100.0], &menu);

    let s1 = frame(&mut r, 1, &[a, b]);
    assert_eq!(
        (s1.backdrop_misses, s1.backdrop_hits),
        (1, 1),
        "two regions share one backdrop"
    );
    assert!(s1.passes_encoded > 0);
    let img1 = g.read(&dst).unwrap();

    let s2 = frame(&mut r, 1, &[a, b]);
    assert_eq!(
        (s2.backdrop_misses, s2.backdrop_hits, s2.passes_encoded),
        (0, 2, 0),
        "unchanged generation reuses the backdrop"
    );
    assert_eq!(
        g.read(&dst).unwrap(),
        img1,
        "cached frame is pixel-identical"
    );

    let s3 = frame(&mut r, 2, &[a, b]);
    assert_eq!(
        (s3.backdrop_misses, s3.backdrop_hits),
        (1, 1),
        "new generation recomputes once"
    );
    assert_eq!(g.read(&dst).unwrap(), img1);

    let s4 = frame(&mut r, 2, &[a, c, d]);
    assert_eq!(
        s4.backdrop_misses, 2,
        "different σ / filters need their own backdrops"
    );
    assert_eq!(s4.backdrop_hits, 1);
    assert!(r.cached_backdrops() >= 3);
    assert_eq!(s4.regions_drawn, 3);

    r.invalidate(&g.device);
    assert_eq!(r.cached_backdrops(), 0);
    let s5 = frame(&mut r, 2, &[a]);
    assert_eq!(s5.backdrop_misses, 1);
    r.destroy(&g.device);
}

#[test]
fn opaque_and_empty_frames() {
    let Some(g) = gpu() else { return };
    let theme = Theme::load().unwrap();
    let wall = procedural_wallpaper(64, 64);
    let opaque = theme
        .recipe(
            Appearance::Light,
            "Sidebar",
            &MaterialOptions {
                reduce_transparency: true,
                ..Default::default()
            },
        )
        .unwrap();
    let img = g
        .render_once(
            &wall,
            &[MaterialRegion::new([8.0, 8.0, 40.0, 40.0], &opaque)],
            1.0,
            [0.5; 3],
        )
        .unwrap();
    let centre = img.get_pixel(28, 28).0;
    assert!(
        (centre[0] as i32 - 246).abs() <= 1 && centre[0] == centre[1],
        "opaque fill 0.965 gray, got {centre:?}"
    );
    let none = g.render_once(&wall, &[], 1.0, [0.5; 3]).unwrap();
    assert_eq!(none, wall, "no regions: target is a copy of the source");
}

fn glyph_texture(w: u32, h: u32) -> image::RgbaImage {
    image::RgbaImage::from_fn(w, h, |x, y| {
        let a = (((x * 7 + y * 13) % 17) as f32 / 16.0) * 0.8 + 0.1;
        let t = ((x + y) % 5) as f32 / 4.0;
        let v = |c: f32| ((c * a).clamp(0.0, 1.0) * 255.0).round() as u8;
        image::Rgba([
            v(0.3 + 0.7 * t),
            v(0.9),
            v(1.0 - 0.5 * t),
            (a * 255.0).round() as u8,
        ])
    })
}

#[test]
fn vibrant_blend_matches_cpu_reference() {
    let Some(g) = gpu() else { return };
    let (w, h) = (96u32, 64u32);
    let dest = image::RgbaImage::from_fn(w, h, |x, y| {
        image::Rgba([(x * 255 / w) as u8, (y * 255 / h) as u8, 120, 255])
    });
    let glyphs = glyph_texture(w, h);
    let src = g.upload(&dest).unwrap();
    let a = g.target(w, h).unwrap();
    let b = g.target(w, h).unwrap();
    let gt = g.upload(&glyphs).unwrap();
    for (mode, reversed) in [
        (VibrantMode::Dark, false),
        (VibrantMode::Dark, true),
        (VibrantMode::Light, false),
        (VibrantMode::Light, true),
    ] {
        let mut r = MaterialRenderer::new(&g.device).unwrap();
        r.render(
            &g.device,
            &FrameDesc {
                source: &src,
                source_size: (w, h),
                target: &a,
                target_size: (w, h),
                display_scale: 1.0,
                damage_generation: 1,
                tint_color: [0.5; 3],
                init: TargetInit::CopySource,
            },
            &[],
        )
        .unwrap();
        r.copy(&g.device, &a, &b).unwrap();
        let style = VibrantStyle {
            mode,
            reversed,
            c0: [0.9, 0.8, 1.0, 1.0],
            c1: [1.0, 0.9, 0.7, 1.0],
        };
        r.draw_vibrant(
            &g.device,
            &b,
            &a,
            &[VibrantDraw::new(&gt, [0.0, 0.0, w as f32, h as f32], style)],
        )
        .unwrap();
        r.destroy(&g.device);
        let out = g.read(&a).unwrap();
        let mut worst = 0i32;
        for y in 0..h {
            for x in 0..w {
                let s = glyphs.get_pixel(x, y).0.map(|v| v as f32 / 255.0);
                let dpx = dest.get_pixel(x, y).0;
                let d = [
                    dpx[0] as f32 / 255.0,
                    dpx[1] as f32 / 255.0,
                    dpx[2] as f32 / 255.0,
                    1.0,
                ];
                let e = match mode {
                    VibrantMode::Dark => cpu::vibrant_dark(s, d, reversed, style.c0, style.c1),
                    VibrantMode::Light => cpu::vibrant_light(s, d, reversed, style.c0, style.c1),
                };
                let got = out.get_pixel(x, y).0;
                for c in 0..3 {
                    worst = worst
                        .max((got[c] as i32 - (e[c].clamp(0.0, 1.0) * 255.0).round() as i32).abs());
                }
            }
        }
        assert!(
            worst <= 2,
            "{mode:?} reversed={reversed}: worst channel error {worst}"
        );
        assert_ne!(out, dest, "glyphs changed the image");
    }
}

/// Fully saturated colour at `t` of the way around the hue circle.
fn hue(t: f32) -> [u8; 3] {
    let h = t * 6.0;
    let f = h.fract();
    let ch = |v: f32| (v * 255.0).round() as u8;
    match h as u32 {
        0 => [255, ch(f), 0],
        1 => [ch(1.0 - f), 255, 0],
        2 => [0, 255, ch(f)],
        3 => [0, ch(1.0 - f), 255],
        4 => [ch(f), 0, 255],
        _ => [255, 0, ch(1.0 - f)],
    }
}

#[test]
fn the_cpu_colour_chain_matches_the_gpu_colour_pass_within_one_level() {
    let Some(g) = gpu() else { return };
    let theme = Theme::load().unwrap();
    let photo = procedural_wallpaper(256, 64);
    // a grey ramp, a saturated hue ramp, then a crop of the photo-like wallpaper
    let wall = image::RgbaImage::from_fn(256, 66, |x, y| match y {
        0 => image::Rgba([x as u8, x as u8, x as u8, 255]),
        1 => {
            let [r, g, b] = hue(x as f32 / 256.0);
            image::Rgba([r, g, b, 255])
        }
        _ => *photo.get_pixel(x, y - 2),
    });
    for appearance in [Appearance::Light, Appearance::Dark] {
        let bar = recipe(&theme, appearance, "MenuBar");
        let ops = bar.backdrop.unwrap().ops;
        assert!(!ops.is_empty(), "{appearance:?} MenuBar has colour ops");
        // the colour pass alone: no capture scale, no blur, no fills
        let unblurred = Recipe {
            backdrop: Some(Backdrop {
                scale: 1.0,
                sigma_pt: 0.0,
                ops: ops.clone(),
            }),
            ..Default::default()
        };
        let regions = [MaterialRegion::new([0.0, 0.0, 256.0, 66.0], &unblurred)];
        let out = g
            .render_once(&wall, &regions, 1.0, average_color(&wall))
            .unwrap();
        let packed = color_ops(&ops);
        let mut worst = 0;
        for (src, got) in wall.pixels().zip(out.pixels()) {
            let c = [0, 1, 2].map(|i| f32::from(src[i]) / 255.0);
            let want = cpu::apply_color_ops(c, &packed);
            for i in 0..3 {
                let want = (want[i].clamp(0.0, 1.0) * 255.0).round() as i32;
                worst = worst.max((i32::from(got[i]) - want).abs());
            }
        }
        assert!(worst <= 1, "{appearance:?}: worst channel error {worst}");
    }
}
