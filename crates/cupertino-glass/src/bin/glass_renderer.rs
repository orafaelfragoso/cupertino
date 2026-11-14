//! glass-renderer: render material recipes with the `MaterialRenderer` on a `cupertino-gpu` device (headless).
//!
//! Usage:
//!   glass-renderer [--spec-dir DIR] [--appearance light|dark|light-high-contrast|dark-high-contrast]
//!                  [--material MenuBar] [--window active|inactive] [--interaction normal|rollover|pressed]
//!                  [--reduce-transparency] [--wallpaper IMG] [--size 1920x1080] [--display-scale 1]
//!                  [--rect x,y,w,h] [--radius PX] [--continuous] [--opacity 1.0]
//!                  [--out out.png] [--bench N] [--cpu-check]
//!   glass-renderer --demo          writes the demo images into output/
//!
//! Without `--rect` the region is the menu-bar strip (24 pt tall) for the MenuBar material and a
//! centred 40 % × 60 % panel otherwise.

use anyhow::{anyhow, Context, Result};
use cupertino_geometry::CornerStyle;
use cupertino_glass::gpu::{average_color, procedural_wallpaper, psnr, HeadlessGpu};
use cupertino_glass::renderer::{FrameDesc, MaterialRegion, MaterialRenderer, TargetInit};
use cupertino_glass::{cpu, VibrantDraw, VibrantMode, VibrantStyle};
use cupertino_theme::{Appearance, Interaction, MaterialOptions, Recipe, Theme, WindowState};
use std::path::PathBuf;
use std::time::Instant;

fn output_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("output")
}

struct Args(Vec<String>);

impl Args {
    fn get(&self, k: &str) -> Option<String> {
        self.0
            .iter()
            .position(|a| a == k)
            .and_then(|i| self.0.get(i + 1))
            .cloned()
    }
    fn flag(&self, k: &str) -> bool {
        self.0.iter().any(|a| a == k)
    }
}

fn default_rect(material: &str, w: u32, h: u32, scale: f32) -> [f32; 4] {
    if material.starts_with("MenuBar") && !material.contains("Selection") {
        [0.0, 0.0, w as f32, 24.0 * scale]
    } else {
        let (rw, rh) = (w as f32 * 0.4, h as f32 * 0.6);
        [(w as f32 - rw) * 0.5, (h as f32 - rh) * 0.5, rw, rh]
    }
}

#[allow(clippy::too_many_arguments)]
fn render_frame(
    gpu: &HeadlessGpu,
    renderer: &mut MaterialRenderer,
    src: &cupertino_gpu::Texture,
    dst: &cupertino_gpu::Texture,
    size: (u32, u32),
    scale: f32,
    generation: u64,
    tint: [f32; 3],
    regions: &[MaterialRegion],
) -> cupertino_glass::RenderStats {
    let stats = renderer.render(
        &gpu.device,
        &FrameDesc {
            source: src,
            source_size: size,
            target: dst,
            target_size: size,
            display_scale: scale,
            damage_generation: generation,
            tint_color: tint,
            init: TargetInit::CopySource,
        },
        regions,
    );
    stats.expect("a frame drawn")
}

fn bench(
    gpu: &HeadlessGpu,
    wall: &image::RgbaImage,
    regions: &[MaterialRegion],
    scale: f32,
    tint: [f32; 3],
    n: usize,
    label: &str,
) {
    let (w, h) = wall.dimensions();
    let src = gpu.upload(wall).unwrap();
    let dst = gpu.target(w, h).unwrap();
    let mut renderer = MaterialRenderer::new(&gpu.device).unwrap();
    for g in 0..10 {
        render_frame(
            gpu,
            &mut renderer,
            &src,
            &dst,
            (w, h),
            scale,
            g,
            tint,
            regions,
        );
    }
    gpu.device.finish().unwrap();
    let run = |renderer: &mut MaterialRenderer, bump: bool| {
        let t0 = Instant::now();
        let mut gen = 1000u64;
        for _ in 0..n {
            if bump {
                gen += 1;
            }
            render_frame(gpu, renderer, &src, &dst, (w, h), scale, gen, tint, regions);
        }
        gpu.device.finish().unwrap();
        t0.elapsed().as_secs_f64() / n as f64
    };
    let miss = run(&mut renderer, true);
    let hit = run(&mut renderer, false);
    println!(
        "bench {label}: {w}x{h}, {} region(s): backdrop recomputed every frame {:.3} ms ({:.0} fps); cached backdrop {:.3} ms ({:.0} fps); {n} frames each",
        regions.len(),
        miss * 1e3,
        1.0 / miss,
        hit * 1e3,
        1.0 / hit
    );
}

fn recipe(
    theme: &Theme,
    appearance: Appearance,
    material: &str,
    opts: &MaterialOptions,
) -> Result<Recipe> {
    theme
        .recipe(appearance, material, opts)
        .map_err(|e| anyhow!("{e}"))
}

fn reg(rect: [f32; 4], rc: &Recipe, radius: f32, st: CornerStyle) -> MaterialRegion<'_> {
    MaterialRegion::new(rect, rc).with_corner(radius, st)
}

fn glyph_template(w: u32, h: u32) -> image::RgbaImage {
    image::RgbaImage::from_fn(w, h, |x, y| {
        let (fx, fy) = (x as f32, y as f32);
        let mut a = 0.0f32;
        for i in 0..5 {
            let cx = 20.0 + i as f32 * 44.0;
            let d = ((fx - cx).powi(2) + (fy - h as f32 * 0.5).powi(2)).sqrt();
            a = a.max((1.0 - (d - 14.0).abs() / 1.5).clamp(0.0, 1.0));
            if (fx - cx).abs() < 3.0 && (fy - h as f32 * 0.5).abs() < 10.0 {
                a = 1.0;
            }
        }
        if (fy - h as f32 * 0.85).abs() < 2.0 && fx > 8.0 && fx < w as f32 - 8.0 {
            a = a.max(0.8);
        }
        let v = (a * 255.0).round() as u8;
        image::Rgba([v, v, v, v])
    })
}

fn demo(gpu: &HeadlessGpu, theme: &Theme) -> Result<()> {
    let out = output_dir();
    std::fs::create_dir_all(&out)?;
    let (w, h) = (1280u32, 720u32);
    let wall = procedural_wallpaper(w, h);
    let tint = average_color(&wall);
    let scale = 1.0f32;
    let opts = MaterialOptions::default();
    for (appearance, name) in [(Appearance::Light, "light"), (Appearance::Dark, "dark")] {
        let bar = recipe(theme, appearance, "MenuBar", &opts)?;
        let menu = recipe(theme, appearance, "Menu", &opts)?;
        let side = recipe(theme, appearance, "Sidebar", &opts)?;
        let title = recipe(theme, appearance, "Titlebar", &opts)?;
        let sel = recipe(theme, appearance, "MenuBarSelection", &opts)?;
        let opaque = recipe(
            theme,
            appearance,
            "Menu",
            &MaterialOptions {
                reduce_transparency: true,
                ..Default::default()
            },
        )?;
        let regions = [
            reg([0.0, 0.0, w as f32, 24.0], &bar, 0.0, CornerStyle::Circular),
            reg(
                [60.0, 80.0, 220.0, 420.0],
                &side,
                12.0,
                CornerStyle::continuous(),
            ),
            reg(
                [320.0, 80.0, 360.0, 40.0],
                &title,
                10.0,
                CornerStyle::continuous(),
            ),
            reg(
                [320.0, 140.0, 300.0, 260.0],
                &menu,
                10.0,
                CornerStyle::continuous(),
            ),
            reg(
                [700.0, 80.0, 300.0, 260.0],
                &opaque,
                10.0,
                CornerStyle::continuous(),
            ),
            MaterialRegion::from_recipe([14.0, 2.0, 70.0, 20.0], &sel, scale),
        ];
        let img = gpu.render_once(&wall, &regions, scale, tint).unwrap();
        let path = out.join(format!("{name}_materials.png"));
        img.save(&path)?;
        println!("wrote {}", path.display());
        for (rect, rc, tag) in [
            ([0.0, 0.0, w as f32, 24.0], &bar, "menubar"),
            ([320.0, 140.0, 300.0, 260.0], &menu, "menu"),
            ([60.0, 80.0, 220.0, 420.0], &side, "sidebar"),
        ] {
            let radius = if tag == "menubar" { 0.0 } else { 12.0 };
            let img = gpu
                .render_once(
                    &wall,
                    &[reg(rect, rc, radius, CornerStyle::continuous())],
                    scale,
                    tint,
                )
                .unwrap();
            let path = out.join(format!("{name}_{tag}.png"));
            img.save(&path)?;
            println!("wrote {}", path.display());
        }
    }

    // corner styles side by side on a flat colour
    let flat = image::RgbaImage::from_pixel(640, 240, image::Rgba([40, 40, 48, 255]));
    let solid = Recipe {
        fills: vec![cupertino_theme::Fill {
            rgba: [0.95, 0.95, 0.97, 1.0],
            blend: cupertino_theme::Blend::Normal,
            tint_layer: false,
        }],
        ..Default::default()
    };
    let regions = [
        MaterialRegion::new([20.0, 20.0, 180.0, 180.0], &solid)
            .with_corner(40.0, CornerStyle::Circular),
        MaterialRegion::new([230.0, 20.0, 180.0, 180.0], &solid)
            .with_corner(40.0, CornerStyle::continuous()),
        MaterialRegion::new([440.0, 20.0, 180.0, 180.0], &solid)
            .with_corner(90.0, CornerStyle::continuous())
            .with_opacity(0.8),
    ];
    let img = gpu.render_once(&flat, &regions, 1.0, [0.5; 3]).unwrap();
    let path = out.join("corner_styles.png");
    img.save(&path)?;
    println!("wrote {}", path.display());

    // vibrant glyphs on a material
    for (appearance, mode, name) in [
        (Appearance::Dark, VibrantMode::Dark, "vibrant_dark"),
        (Appearance::Light, VibrantMode::Light, "vibrant_light"),
    ] {
        let side = recipe(theme, appearance, "Sidebar", &opts)?;
        let src = gpu.upload(&wall).unwrap();
        let a = gpu.target(w, h).unwrap();
        let b = gpu.target(w, h).unwrap();
        let glyphs = gpu.upload(&glyph_template(256, 64)).unwrap();
        let mut renderer = MaterialRenderer::new(&gpu.device)?;
        renderer.render(
            &gpu.device,
            &FrameDesc {
                source: &src,
                source_size: (w, h),
                target: &a,
                target_size: (w, h),
                display_scale: 1.0,
                damage_generation: 1,
                tint_color: tint,
                init: TargetInit::CopySource,
            },
            &[MaterialRegion::new([100.0, 100.0, 520.0, 300.0], &side)
                .with_corner(14.0, CornerStyle::continuous())],
        )?;
        renderer.copy(&gpu.device, &a, &b)?;
        let style = VibrantStyle::new(mode);
        let draws: Vec<VibrantDraw> = (0..3)
            .map(|i| {
                VibrantDraw::new(
                    &glyphs,
                    [130.0, 130.0 + i as f32 * 80.0, 256.0, 64.0],
                    style,
                )
            })
            .collect();
        renderer.draw_vibrant(&gpu.device, &b, &a, &draws)?;
        renderer.destroy(&gpu.device);
        let img = gpu.read(&a).unwrap();
        let path = out.join(format!("{name}.png"));
        img.save(&path)?;
        println!("wrote {}", path.display());
    }
    Ok(())
}

fn main() -> Result<()> {
    let args = Args(std::env::args().collect());
    let theme = match args.get("--spec-dir") {
        Some(d) => Theme::from_dir(d),
        None => Theme::load(),
    }
    .map_err(|e| anyhow!("{e}"))
    .context("loading specification")?;
    let gpu = HeadlessGpu::new().ok_or_else(|| anyhow!("no GPU device"))?;
    println!("adapter: {}", gpu.adapter_name);
    if args.flag("--demo") {
        return demo(&gpu, &theme);
    }

    let appearance: Appearance = args
        .get("--appearance")
        .unwrap_or_else(|| "light".into())
        .parse()
        .map_err(|e: String| anyhow!(e))?;
    let material = args.get("--material").unwrap_or_else(|| "MenuBar".into());
    let opts = MaterialOptions {
        window: if args.get("--window").as_deref() == Some("inactive") {
            WindowState::Inactive
        } else {
            WindowState::Active
        },
        interaction: match args.get("--interaction").as_deref() {
            Some("rollover") => Interaction::Rollover,
            Some("pressed") => Interaction::Pressed,
            _ => Interaction::Normal,
        },
        reduce_transparency: args.flag("--reduce-transparency"),
        rendition_key: None,
    };
    let r = recipe(&theme, appearance, &material, &opts)?;
    let (w, h) = args
        .get("--size")
        .and_then(|s| {
            s.split_once('x')
                .map(|(a, b)| (a.parse().unwrap(), b.parse().unwrap()))
        })
        .unwrap_or((1920u32, 1080u32));
    let wall = match args.get("--wallpaper") {
        Some(p) => image::open(p)?
            .resize_exact(w, h, image::imageops::FilterType::Triangle)
            .to_rgba8(),
        None => procedural_wallpaper(w, h),
    };
    let scale: f32 = args
        .get("--display-scale")
        .map(|s| s.parse().unwrap())
        .unwrap_or(1.0);
    let rect: [f32; 4] = args
        .get("--rect")
        .map(|s| {
            let v: Vec<f32> = s.split(',').map(|x| x.parse().unwrap()).collect();
            [v[0], v[1], v[2], v[3]]
        })
        .unwrap_or_else(|| default_rect(&material, w, h, scale));
    let mut region = MaterialRegion::from_recipe(rect, &r, scale);
    if let Some(rad) = args.get("--radius") {
        region.corner_radius = rad.parse().unwrap();
    }
    if args.flag("--continuous") {
        region.corner_style = CornerStyle::continuous();
    }
    if let Some(o) = args.get("--opacity") {
        region.opacity = o.parse().unwrap();
    }
    let tint = average_color(&wall);
    println!("recipe {}: {:#?}", r.name, r);
    let img = gpu.render_once(&wall, &[region], scale, tint).unwrap();
    let default_name = format!(
        "{}_{}.png",
        appearance.as_str().replace('-', "_"),
        material.to_lowercase()
    );
    let out = args
        .get("--out")
        .map(PathBuf::from)
        .unwrap_or_else(|| output_dir().join(default_name));
    if let Some(p) = out.parent() {
        std::fs::create_dir_all(p)?;
    }
    img.save(&out)?;
    println!("wrote {}", out.display());
    if args.flag("--cpu-check") {
        let t0 = Instant::now();
        let reference = cpu::render_regions(&wall, &[region], scale, tint, false, false);
        println!(
            "cpu reference (same plan) in {:?}: PSNR GPU vs CPU = {:.2} dB",
            t0.elapsed(),
            psnr(&img, &reference)
        );
        let t0 = Instant::now();
        let exact = cpu::render_regions(&wall, &[region], scale, tint, true, true);
        println!(
            "cpu exact (full Gaussian, reference SDF) in {:?}: PSNR GPU vs exact = {:.2} dB",
            t0.elapsed(),
            psnr(&img, &exact)
        );
        exact.save(out.with_extension("cpu_exact.png"))?;
    }
    if let Some(n) = args.get("--bench").map(|s| s.parse::<usize>().unwrap()) {
        bench(&gpu, &wall, &[region], scale, tint, n, &r.name);
        let menu = recipe(&theme, appearance, "Menu", &MaterialOptions::default())?;
        let side = recipe(&theme, appearance, "Sidebar", &MaterialOptions::default())?;
        let bar = recipe(&theme, appearance, "MenuBar", &MaterialOptions::default())?;
        let title = recipe(&theme, appearance, "Titlebar", &MaterialOptions::default())?;
        let (fw, fh) = (w as f32, h as f32);
        let regions = [
            MaterialRegion::new([0.0, 0.0, fw, 24.0 * scale], &bar),
            MaterialRegion::new([fw * 0.05, fh * 0.1, fw * 0.2, fh * 0.6], &side)
                .with_corner(12.0 * scale, CornerStyle::continuous()),
            MaterialRegion::new([fw * 0.3, fh * 0.1, fw * 0.3, 40.0 * scale], &title)
                .with_corner(10.0 * scale, CornerStyle::continuous()),
            MaterialRegion::new([fw * 0.3, fh * 0.2, fw * 0.25, fh * 0.4], &menu)
                .with_corner(10.0 * scale, CornerStyle::continuous()),
            MaterialRegion::new([fw * 0.6, fh * 0.2, fw * 0.25, fh * 0.4], &menu)
                .with_corner(10.0 * scale, CornerStyle::continuous()),
            MaterialRegion::new([fw * 0.6, fh * 0.7, fw * 0.3, fh * 0.2], &side)
                .with_corner(12.0 * scale, CornerStyle::continuous()),
        ];
        bench(
            &gpu,
            &wall,
            &regions,
            scale,
            tint,
            n,
            "6 regions (menu bar, 2 sidebars, titlebar, 2 menus)",
        );
    }
    Ok(())
}
