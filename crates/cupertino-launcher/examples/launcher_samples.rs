//! Renders sample panels to `output/*.png` from a synthetic application set and a temporary file
//! tree (no dependence on what is installed).
//!
//! `cargo run -p cupertino-launcher --release --example launcher_samples [output-dir]`

use cupertino_geometry::Size;
use cupertino_launcher::files::{FileIndex, FileSearchConfig};
use cupertino_launcher::panel::{Key, Mods, Now, Panel};
use cupertino_launcher::render::{Backdrop, PanelRenderer, PanelStyle, RenderOptions};
use cupertino_launcher::{DesktopAction, DesktopEntry, Frecency, Launcher, LauncherConfig};
use cupertino_theme::{Appearance, Theme};
use std::path::PathBuf;
use std::sync::Arc;
use tiny_skia::{
    Color, FillRule, GradientStop, LinearGradient, Paint, PathBuilder, Pixmap, PixmapPaint, Point,
    RadialGradient, SpreadMode, Transform,
};

const VIEW: Size = Size {
    width: 760.0,
    height: 560.0,
};
const SCALE: f32 = 2.0;

fn entry(id: &str, name: &str, generic: &str, exec: &str, comment: &str) -> DesktopEntry {
    DesktopEntry {
        id: format!("{id}.desktop"),
        name: name.into(),
        generic_name: generic.into(),
        comment: comment.into(),
        keywords: vec![],
        categories: vec![],
        icon: None,
        exec: exec.into(),
        try_exec: None,
        terminal: false,
        path: None,
        startup_wm_class: None,
        actions: vec![],
        source: None,
    }
}

fn apps() -> Vec<DesktopEntry> {
    let mut browser = entry(
        "browser",
        "Firefox",
        "Web Browser",
        "firefox %u",
        "Browse the web",
    );
    browser.keywords = vec!["internet".into()];
    browser.actions = vec![
        DesktopAction {
            id: "new-window".into(),
            name: "New Window".into(),
            exec: "firefox --new-window".into(),
            icon: None,
        },
        DesktopAction {
            id: "private".into(),
            name: "New Private Window".into(),
            exec: "firefox --private-window".into(),
            icon: None,
        },
    ];
    vec![
        browser,
        entry(
            "code",
            "Visual Studio Code",
            "Text Editor",
            "code %F",
            "Code editing",
        ),
        entry(
            "writer",
            "LibreOffice Writer",
            "Word Processor",
            "libreoffice --writer %U",
            "",
        ),
        entry(
            "calc",
            "LibreOffice Calc",
            "Spreadsheet",
            "libreoffice --calc %U",
            "",
        ),
        entry(
            "terminal",
            "Terminal",
            "Terminal Emulator",
            "gnome-terminal",
            "",
        ),
        entry("files", "Files", "File Manager", "nautilus %U", ""),
        entry("gimp", "GIMP", "Image Editor", "gimp %U", ""),
        entry(
            "firewall",
            "Firewall Configuration",
            "",
            "firewall-config",
            "",
        ),
        entry("mail", "Mail", "Email Client", "mail %U", ""),
        entry("music", "Music", "Audio Player", "music %U", ""),
    ]
}

fn wallpaper(w: u32, h: u32, dark: bool) -> Pixmap {
    let mut pm = Pixmap::new(w, h).unwrap();
    let (a, b) = if dark {
        (
            Color::from_rgba8(20, 28, 64, 255),
            Color::from_rgba8(82, 34, 96, 255),
        )
    } else {
        (
            Color::from_rgba8(110, 170, 235, 255),
            Color::from_rgba8(248, 196, 160, 255),
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
    let mut pb = PathBuilder::new();
    pb.push_rect(tiny_skia::Rect::from_xywh(0.0, 0.0, w as f32, h as f32).unwrap());
    let full = pb.finish().unwrap();
    pm.fill_path(
        &full,
        &Paint {
            shader,
            ..Paint::default()
        },
        FillRule::Winding,
        Transform::identity(),
        None,
    );
    for (cx, cy, r, c) in [
        (0.2, 0.3, 0.35, (255, 120, 90)),
        (0.85, 0.6, 0.3, (80, 220, 180)),
        (0.5, 0.85, 0.25, (255, 230, 90)),
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
        pm.fill_path(
            &full,
            &Paint {
                shader,
                ..Paint::default()
            },
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

#[allow(clippy::too_many_arguments)]
fn render(
    name: &str,
    query: &str,
    appearance: Appearance,
    launcher: &mut Launcher,
    theme: &Theme,
    assets: &cupertino_assets::Assets,
    out: &std::path::Path,
    select: usize,
) {
    let mut style = PanelStyle::from_theme(theme, appearance, false);
    style.top_fraction = 0.05;
    let mut r = PanelRenderer::new(style, assets).unwrap_or_else(|e| panic!("{e}"));
    let now = Now {
        mono: 10.0,
        unix: 1.0e6,
    };
    let mut panel = Panel::new();
    panel.max_rows = r.style.max_rows;
    panel.show(launcher, now);
    for c in query.chars() {
        panel.handle_key(Key::Char(c), Mods::NONE, launcher, now);
    }
    for _ in 0..select {
        panel.handle_key(Key::Down, Mods::NONE, launcher, now);
    }
    let layout = r.layout(&panel);
    let rect = r.style.placement(VIEW, layout.size.height);
    let (w, h) = (
        (VIEW.width * SCALE as f64) as u32,
        (VIEW.height * SCALE as f64) as u32,
    );
    let mut scene = wallpaper(w, h, appearance.is_dark());
    let margin = 120u32;
    let (px, py) = (
        (rect.x() * SCALE as f64) as i32,
        (rect.y() * SCALE as f64) as i32,
    );
    let (pw, ph) = (
        (layout.size.width * SCALE as f64) as u32,
        (layout.size.height * SCALE as f64) as u32,
    );
    let backdrop = crop(
        &scene,
        px - margin as i32,
        py - margin as i32,
        pw + 2 * margin,
        ph + 2 * margin,
    );
    let img = r.render(
        &panel,
        &layout,
        &RenderOptions {
            scale: SCALE,
            backdrop: Some(Backdrop {
                image: &backdrop,
                panel_origin: (margin, margin),
            }),
            ..Default::default()
        },
    );
    scene.draw_pixmap(
        px - img.margins[0] as i32,
        py - img.margins[1] as i32,
        img.pixmap.as_ref(),
        &PixmapPaint::default(),
        Transform::identity(),
        None,
    );
    let path = out.join(format!("{name}.png"));
    let keep = ((py + ph as i32 + 140) as u32).min(h);
    crop(&scene, 0, 0, w, keep).save_png(&path).unwrap();
    println!(
        "wrote {} ({} results)",
        path.display(),
        panel.results().len()
    );
}

fn main() {
    let out: PathBuf = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("output"));
    std::fs::create_dir_all(&out).unwrap();
    let assets = cupertino_assets::Assets::discover();
    let theme = Theme::load().expect("specification files");

    let tree =
        std::env::temp_dir().join(format!("cupertino-launcher-sample-{}", std::process::id()));
    for f in [
        "Documents/Quarterly Report 2024.pdf",
        "Documents/report-draft.txt",
        "Projects/reports/summary.md",
        "Downloads/annual-report.pdf",
        "Pictures/holiday.jpg",
    ] {
        let p = tree.join(f);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, "x").unwrap();
    }
    let mut frecency = Frecency::in_memory();
    for (key, n) in [
        ("app:terminal.desktop", 9),
        ("app:browser.desktop", 7),
        ("app:mail.desktop", 4),
        ("app:code.desktop", 3),
    ] {
        for _ in 0..n {
            frecency.record(key, "", 1.0e6 - 3600.0);
        }
    }
    let mut launcher = Launcher::new(LauncherConfig::default(), apps(), frecency);
    launcher.set_file_index(Some(Arc::new(FileIndex::build(FileSearchConfig::new(
        &tree,
    )))));

    render(
        "panel_suggestions_light",
        "",
        Appearance::Light,
        &mut launcher,
        &theme,
        &assets,
        &out,
        0,
    );
    render(
        "panel_apps_light",
        "fire",
        Appearance::Light,
        &mut launcher,
        &theme,
        &assets,
        &out,
        0,
    );
    render(
        "panel_acronym_light",
        "vsc",
        Appearance::Light,
        &mut launcher,
        &theme,
        &assets,
        &out,
        0,
    );
    render(
        "panel_multiword_dark",
        "libre",
        Appearance::Dark,
        &mut launcher,
        &theme,
        &assets,
        &out,
        1,
    );
    render(
        "panel_calculator_light",
        "12*(3+4)^2/7",
        Appearance::Light,
        &mut launcher,
        &theme,
        &assets,
        &out,
        0,
    );
    render(
        "panel_files_dark",
        "report",
        Appearance::Dark,
        &mut launcher,
        &theme,
        &assets,
        &out,
        2,
    );
    let _ = std::fs::remove_dir_all(tree);
}
