//! Renders sample images into `output/`.
//!
//! `cargo run -p cupertino-decorations --release --example render_samples`
//!
//! Needs `CUPERTINO_ASSET_DIR` (the fonts come from the asset directory).

use cupertino_assets::Assets;
use cupertino_decorations::buttons::{draw_button, ButtonState};
use cupertino_decorations::preview::render_preview;
use cupertino_decorations::shadow::{ShadowParams, ShadowRasterizer};
use cupertino_decorations::*;
use cupertino_geometry::{CornerStyle, Size};
use cupertino_theme::window_shadow::ShadowOption;
use cupertino_theme::{Appearance, Theme, WindowCornerSize};
use tiny_skia::{Color, Pixmap, PixmapPaint, Transform};

fn wallpaper(w: u32, h: u32, dark: bool) -> Pixmap {
    let mut pm = Pixmap::new(w, h).unwrap();
    for y in 0..h {
        for x in 0..w {
            let (u, v) = (x as f32 / w as f32, y as f32 / h as f32);
            let c = if dark { [20.0 + 40.0 * u, 30.0 + 40.0 * v, 70.0 + 60.0 * (1.0 - u)] } else { [150.0 + 60.0 * u, 170.0 + 50.0 * v, 220.0 - 40.0 * u] };
            let i = ((y * w + x) * 4) as usize;
            pm.data_mut()[i..i + 4].copy_from_slice(&[c[0] as u8, c[1] as u8, c[2] as u8, 255]);
        }
    }
    pm
}

fn cell(dec: &mut Decorations, shadows: &mut ShadowRasterizer, state: &DecorationState, dark: bool) -> Pixmap {
    let p = render_preview(dec, shadows, Size::new(340.0, 170.0), 1.0, state);
    let mut bg = wallpaper(p.image.width(), p.image.height(), dark);
    bg.draw_pixmap(0, 0, p.image.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
    bg
}

fn sheet(frames: &[Pixmap], cols: usize, pad: u32, bg: Color) -> Pixmap {
    let (cw, ch) = (frames[0].width(), frames[0].height());
    let rows = frames.len().div_ceil(cols);
    let mut s = Pixmap::new(cols as u32 * (cw + pad) + pad, rows as u32 * (ch + pad) + pad).unwrap();
    s.fill(bg);
    for (i, f) in frames.iter().enumerate() {
        let (c, r) = ((i % cols) as u32, (i / cols) as u32);
        s.draw_pixmap((pad + c * (cw + pad)) as i32, (pad + r * (ch + pad)) as i32, f.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
    }
    s
}

fn config(theme: &Theme, appearance: Appearance, style: TitlebarStyle) -> DecorationConfig {
    DecorationConfig::from_theme(theme, appearance, style)
}

fn make_decorations(cfg: DecorationConfig, assets: &Assets) -> Decorations {
    Decorations::new(cfg, assets).unwrap_or_else(|e| panic!("{e}"))
}

fn main() {
    let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("output");
    std::fs::create_dir_all(&out).unwrap();
    let assets = Assets::discover();
    let theme = Theme::load().expect("specification files");
    let mut shadows = ShadowRasterizer::new();
    let title = "Document — Draft".to_string();
    let states: Vec<(&str, DecorationState)> = vec![
        ("active", DecorationState { title: title.clone(), ..Default::default() }),
        ("inactive", DecorationState { title: title.clone(), active: false, ..Default::default() }),
        ("hover-group", DecorationState { title: title.clone(), group_hovered: true, ..Default::default() }),
        ("pressed-close", DecorationState { title: title.clone(), group_hovered: true, pressed: Some(ButtonKind::Close), ..Default::default() }),
        ("zoom-disabled", DecorationState { title: title.clone(), group_hovered: true, enabled: [true, true, false], ..Default::default() }),
    ];
    for (appearance, name) in [(Appearance::Light, "light"), (Appearance::Dark, "dark")] {
        let mut dec = make_decorations(config(&theme, appearance, TitlebarStyle::Standard), &assets);
        let frames: Vec<Pixmap> = states.iter().map(|(_, s)| cell(&mut dec, &mut shadows, s, appearance.is_dark())).collect();
        sheet(&frames, 3, 8, Color::from_rgba8(20, 20, 24, 255)).save_png(out.join(format!("window_states_{name}.png"))).unwrap();
        println!("font family: {:?}", dec.text_renderer().family());
    }

    let mut frames = Vec::new();
    for (appearance, style, corner) in [
        (Appearance::Light, TitlebarStyle::Unified, WindowCornerSize::Regular),
        (Appearance::Dark, TitlebarStyle::Unified, WindowCornerSize::Regular),
        (Appearance::Light, TitlebarStyle::Standard, WindowCornerSize::Small),
    ] {
        let mut cfg = config(&theme, appearance, style);
        cfg.corner = corner;
        let mut dec = make_decorations(cfg, &assets);
        frames.push(cell(&mut dec, &mut shadows, &DecorationState { title: "Files".into(), ..Default::default() }, appearance.is_dark()));
    }
    sheet(&frames, 3, 8, Color::from_rgba8(20, 20, 24, 255)).save_png(out.join("styles_unified_and_small.png")).unwrap();

    let mut closeups = Vec::new();
    for appearance in [Appearance::Light, Appearance::Dark] {
        let cfg = config(&theme, appearance, TitlebarStyle::Standard);
        for (state, _) in [
            (ButtonState::default(), ()),
            (ButtonState { group_hovered: true, ..Default::default() }, ()),
            (ButtonState { group_hovered: true, pressed: true, ..Default::default() }, ()),
            (ButtonState { window_active: false, ..Default::default() }, ()),
            (ButtonState { enabled: false, ..Default::default() }, ()),
        ] {
            let mut pm = Pixmap::new(3 * 20 * 8 + 16, 24 * 8).unwrap();
            pm.fill(Color::from_rgba(cfg.palette.titlebar_active[0], cfg.palette.titlebar_active[1], cfg.palette.titlebar_active[2], 1.0).unwrap());
            for (i, kind) in ButtonKind::ALL.iter().enumerate() {
                draw_button(&mut pm, *kind, cupertino_geometry::Point::new(10.0 + i as f64 * 20.0, 12.0), 8.0, &cfg.metrics, &cfg.palette, state);
            }
            closeups.push(pm);
        }
    }
    sheet(&closeups, 5, 8, Color::from_rgba8(20, 20, 24, 255)).save_png(out.join("buttons_closeup.png")).unwrap();

    let mut shadow_cells = Vec::new();
    for dark in [false, true] {
        for (option, active) in [(ShadowOption::Standard, true), (ShadowOption::Standard, false), (ShadowOption::Panel, true), (ShadowOption::Tooltip, true)] {
            let params = ShadowParams::lookup(option, active, dark, false);
            let size = match option {
                ShadowOption::Tooltip => Size::new(120.0, 40.0),
                ShadowOption::Panel => Size::new(200.0, 120.0),
                _ => Size::new(260.0, 160.0),
            };
            let img = shadows.render(&params, size, 12.0, CornerStyle::continuous(), 1.0);
            let mut canvas = wallpaper(320, 260, dark);
            let mut shadow = Pixmap::new(img.width, img.height).unwrap();
            shadow.data_mut().copy_from_slice(&img.data);
            let (ox, oy) = (160 - (size.width as i32) / 2 - img.margins.left as i32, 130 - (size.height as i32) / 2 - img.margins.top as i32);
            canvas.draw_pixmap(ox, oy, shadow.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
            let fill = if dark { Color::from_rgba8(50, 50, 50, 255) } else { Color::from_rgba8(236, 236, 236, 255) };
            let mut pb = tiny_skia::PathBuilder::new();
            pb.push_rect(tiny_skia::Rect::from_xywh(160.0 - size.width as f32 / 2.0, 130.0 - size.height as f32 / 2.0, size.width as f32, size.height as f32).unwrap());
            let mut paint = tiny_skia::Paint::default();
            paint.set_color(fill);
            canvas.fill_path(&pb.finish().unwrap(), &paint, tiny_skia::FillRule::Winding, Transform::identity(), None);
            shadow_cells.push(canvas);
        }
    }
    sheet(&shadow_cells, 4, 8, Color::from_rgba8(20, 20, 24, 255)).save_png(out.join("shadow_profiles.png")).unwrap();
    println!("wrote output/*.png");
}
