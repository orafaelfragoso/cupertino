//! Renders sample banner stacks to `output/*.png` (light, dark, and an animation strip).
//!
//! `cargo run -p cupertino-notifications --release --example notifications_samples [output-dir]`

use cupertino_geometry::Size;
use cupertino_notifications::render::{Backdrop, BannerRenderer, BannerStyle, RenderOptions};
use cupertino_notifications::{
    CenterConfig, DndMode, Hints, NotificationCenter, NotifyInput, Urgency,
};
use cupertino_theme::{Appearance, Theme};
use std::path::PathBuf;
use tiny_skia::{
    Color, FillRule, GradientStop, LinearGradient, Paint, PathBuilder, Pixmap, PixmapPaint, Point,
    RadialGradient, SpreadMode, Transform,
};

const VIEW: Size = Size {
    width: 640.0,
    height: 360.0,
};
const SCALE: f32 = 2.0;

fn wallpaper(w: u32, h: u32, dark: bool) -> Pixmap {
    let mut pm = Pixmap::new(w, h).unwrap();
    let (a, b) = if dark {
        (
            Color::from_rgba8(18, 24, 58, 255),
            Color::from_rgba8(70, 30, 90, 255),
        )
    } else {
        (
            Color::from_rgba8(120, 180, 240, 255),
            Color::from_rgba8(250, 200, 160, 255),
        )
    };
    let shader = LinearGradient::new(
        Point::from_xy(0.0, 0.0),
        Point::from_xy(w as f32, h as f32),
        vec![GradientStop::new(0.0, a), GradientStop::new(1.0, b)],
        SpreadMode::Pad,
        Transform::identity(),
    )
    .unwrap();
    let p = Paint {
        shader,
        ..Paint::default()
    };
    let mut pb = PathBuilder::new();
    pb.push_rect(tiny_skia::Rect::from_xywh(0.0, 0.0, w as f32, h as f32).unwrap());
    pm.fill_path(
        &pb.finish().unwrap(),
        &p,
        FillRule::Winding,
        Transform::identity(),
        None,
    );
    for (cx, cy, r, c) in [
        (0.75, 0.2, 0.35, (255, 120, 90)),
        (0.9, 0.7, 0.3, (80, 220, 180)),
        (0.6, 0.5, 0.2, (255, 230, 90)),
    ] {
        let (cx, cy, r) = (cx * w as f32, cy * h as f32, r * h as f32);
        let shader = RadialGradient::new(
            Point::from_xy(cx, cy),
            Point::from_xy(cx, cy),
            r,
            vec![
                GradientStop::new(0.0, Color::from_rgba8(c.0, c.1, c.2, 230)),
                GradientStop::new(1.0, Color::from_rgba8(c.0, c.1, c.2, 0)),
            ],
            SpreadMode::Pad,
            Transform::identity(),
        )
        .unwrap();
        let p = Paint {
            shader,
            ..Paint::default()
        };
        let mut pb = PathBuilder::new();
        pb.push_rect(tiny_skia::Rect::from_xywh(0.0, 0.0, w as f32, h as f32).unwrap());
        pm.fill_path(
            &pb.finish().unwrap(),
            &p,
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
    pm
}

fn crop(src: &Pixmap, x: i32, y: i32, w: u32, h: u32) -> Pixmap {
    let mut out = Pixmap::new(w, h).unwrap();
    out.draw_pixmap(
        -x,
        -y,
        src.as_ref(),
        &PixmapPaint::default(),
        Transform::identity(),
        None,
    );
    out
}

/// Sample notifications, one per row of the light and dark images.
fn samples() -> Vec<NotifyInput> {
    let mut mail = NotifyInput::new("Mail", "Alice Morgan", "Lunch on Thursday? The new place on <b>Maple Street</b> has a table free at noon and I booked it just in case.");
    mail.hints = Hints {
        desktop_entry: Some("mail".into()),
        ..Default::default()
    };
    let build = NotifyInput::new("Terminal", "Build finished", "All 1,284 tests passed in 42 s. See <a href=\"https://example.org/build/17\">the build log</a>.");
    vec![
        mail.with_action("default", "Open")
            .with_action("reply", "Reply")
            .with_action("archive", "Archive"),
        build,
        NotifyInput::new(
            "Backup",
            "Disk almost full",
            "Only 1.2 GB left on the backup volume.",
        )
        .with_urgency(Urgency::Critical),
    ]
}

fn compose(
    c: &mut NotificationCenter,
    r: &mut BannerRenderer,
    wall: &Pixmap,
    now: f64,
    hovered: Option<usize>,
) -> Pixmap {
    let mut scene = wall.clone();
    c.tick(now);
    let placements = c.placements(VIEW, r, now);
    for (i, p) in placements.iter().enumerate() {
        let n = c.get(p.id).unwrap().clone();
        let layout = r.layout_cached(&n, p.rect.width()).clone();
        let margin = 100u32;
        let (bx, by) = (
            (p.rect.x() * SCALE as f64) as i32,
            (p.rect.y() * SCALE as f64) as i32,
        );
        let (bw, bh) = (
            (layout.size.width * SCALE as f64) as u32,
            (layout.size.height * SCALE as f64) as u32,
        );
        let backdrop = crop(
            &scene,
            bx - margin as i32,
            by - margin as i32,
            bw + 2 * margin,
            bh + 2 * margin,
        );
        let opts = RenderOptions {
            scale: SCALE,
            hovered: hovered == Some(i),
            backdrop: Some(Backdrop {
                image: &backdrop,
                banner_origin: (margin, margin),
            }),
            ..Default::default()
        };
        let img = r.render(&n, &layout, &opts);
        let paint = PixmapPaint {
            opacity: p.opacity as f32,
            ..Default::default()
        };
        scene.draw_pixmap(
            bx - img.margins[0] as i32,
            by - img.margins[1] as i32,
            img.pixmap.as_ref(),
            &paint,
            Transform::identity(),
            None,
        );
    }
    scene
}

fn main() {
    let out: PathBuf = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("output"));
    std::fs::create_dir_all(&out).unwrap();
    let assets = cupertino_assets::Assets::discover();
    let theme = Theme::load().expect("specification files");
    let (w, h) = (
        (VIEW.width * SCALE as f64) as u32,
        (VIEW.height * SCALE as f64) as u32,
    );
    for (name, appearance) in [
        ("banners_light", Appearance::Light),
        ("banners_dark", Appearance::Dark),
    ] {
        // one banner is on screen at a time: each sample gets its own centre (and renderer, whose
        // layout cache is keyed by id), one row each
        let mut scene = wallpaper(w, h, appearance.is_dark());
        for (i, input) in samples().into_iter().enumerate() {
            let mut r =
                BannerRenderer::new(BannerStyle::from_theme(&theme, appearance, false), &assets)
                    .unwrap_or_else(|e| panic!("{e}"));
            let mut c = NotificationCenter::new(CenterConfig::from_theme(&theme));
            c.config.layout.top_inset = 12.0 + 132.0 * i as f64;
            c.notify(input, 0.0);
            scene = compose(&mut c, &mut r, &scene, 3.0, (i == 0).then_some(0));
        }
        scene.save_png(out.join(format!("{name}.png"))).unwrap();
        println!("wrote {}", out.join(format!("{name}.png")).display());
    }

    let mut r = BannerRenderer::new(
        BannerStyle::from_theme(&theme, Appearance::Light, false),
        &assets,
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let mut c = NotificationCenter::new(CenterConfig::from_theme(&theme));
    c.set_dnd(DndMode::Off, 0.0);
    c.notify(
        NotifyInput::new(
            "Calendar",
            "Design review",
            "Starts in 5 minutes in Room 4.",
        ),
        0.0,
    );
    let wall = wallpaper(w, h, false);
    let frames: Vec<Pixmap> = [0.0, 0.0625, 0.125, 0.1875, 0.25]
        .iter()
        .map(|t| compose(&mut c, &mut r, &wall, *t, None))
        .collect();
    let (fw, fh) = (800u32, h / 3);
    let mut strip = Pixmap::new(fw * frames.len() as u32, fh).unwrap();
    for (i, f) in frames.iter().enumerate() {
        let small = crop(f, (w - fw) as i32, 0, fw, fh);
        strip.draw_pixmap(
            (i as u32 * fw) as i32,
            0,
            small.as_ref(),
            &PixmapPaint::default(),
            Transform::identity(),
            None,
        );
    }
    strip.save_png(out.join("banner_slide_in.png")).unwrap();
    println!("wrote {}", out.join("banner_slide_in.png").display());
}
