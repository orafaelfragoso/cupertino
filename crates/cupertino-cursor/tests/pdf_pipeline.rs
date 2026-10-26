//! End-to-end tests with poppler: tiny PDFs are generated in the test, rasterised, shadowed and
//! written as XCursor files. Skipped when `pdftocairo` / `pdfinfo` are not installed.

use cupertino_cursor::meta::HotspotSource;
use cupertino_cursor::raster::Poppler;
use cupertino_cursor::theme::{build_theme, BuildOptions, ShadowMode, Source};
use cupertino_cursor::xcursor;
use std::path::Path;

fn pdf(width: f64, height: f64, content: &str) -> Vec<u8> {
    let objs = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {width} {height}] /Contents 4 0 R /Resources << >> >>"),
        format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
    ];
    let mut out = b"%PDF-1.4\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objs.len() + 1
        )
        .as_bytes(),
    );
    out
}

fn write_source(dir: &Path) {
    let put = |name: &str, bytes: Vec<u8>| {
        std::fs::create_dir_all(dir.join(name)).unwrap();
        std::fs::write(dir.join(name).join("cursor.pdf"), bytes).unwrap();
    };
    put("pointinghand", pdf(20.0, 20.0, "1 0 0 rg 5 5 10 10 re f"));
    put(
        "busybutclickable",
        pdf(
            10.0,
            30.0,
            "1 0 0 rg 0 20 10 10 re f 0 1 0 rg 0 10 10 10 re f 0 0 1 rg 0 0 10 10 re f",
        ),
    );
    put("cross", pdf(16.0, 16.0, "0 0 0 rg 7 0 2 16 re f"));
    put(
        "countinguphand",
        pdf(
            8.0,
            24.0,
            "1 0 0 rg 0 16 8 8 re f 0 1 0 rg 0 8 8 8 re f 0 0 1 rg 0 0 8 8 re f",
        ),
    );
}

const META: &str = r#"{"cursors": {
    "pointinghand": {"hotx": 10, "hoty": 10, "hotx-scaled": 4, "hoty-scaled": 6, "shadowcolor": [0,0,0,0.65], "shadowoffsetx": 0, "shadowoffsety": -1, "blur": 1.8},
    "busybutclickable": {"hotx": 5, "hoty": 5, "frames": 3, "delay": 0.1}
}}"#;

fn options(out: &Path, src: &Path) -> Option<BuildOptions> {
    let mut o = BuildOptions::new(out);
    o.poppler = Poppler::default();
    if !o.poppler.available() {
        eprintln!("poppler is not installed, skipping");
        return None;
    }
    let meta = src.join("meta.json");
    std::fs::write(&meta, META).unwrap();
    o.source = Source::Pdf(src.join("cursors"));
    o.metadata = Some(meta);
    o.sizes = vec![24, 48];
    Some(o)
}

fn read(theme: &Path, name: &str) -> Vec<xcursor::XCursorImage> {
    xcursor::parse(&std::fs::read(theme.join("cursors").join(name)).unwrap()).unwrap()
}

fn px(img: &xcursor::XCursorImage, x: u32, y: u32) -> u32 {
    img.pixels[(y * img.width + x) as usize]
}

#[test]
fn pdf_cursor_is_rasterised_shadowed_and_hotspot_scaled() {
    let d = tempfile::tempdir().unwrap();
    write_source(&d.path().join("cursors").parent().unwrap().join("cursors"));
    let Some(o) = options(&d.path().join("out"), d.path()) else {
        return;
    };
    let r = build_theme(&o).unwrap();
    let imgs = read(&r.theme_dir, "pointer");
    assert_eq!(imgs.len(), 2);
    let (a, b) = (&imgs[0], &imgs[1]);
    // scale 1: shadow padding = ceil(3 * 1.8) + 1 = 7 on each side of the 20 px page
    assert_eq!((a.nominal_size, a.width, a.height), (24, 20 + 14, 20 + 14));
    assert_eq!((a.xhot, a.yhot), (10 + 7, 10 + 7));
    // scale 2: page 40 px, padding = ceil(10.8) + 2 = 13
    assert_eq!((b.nominal_size, b.width, b.height), (48, 40 + 26, 40 + 26));
    assert_eq!((b.xhot, b.yhot), (20 + 13, 20 + 13));
    // the red square (5..15 pt) is opaque red inside, the corner of the canvas is transparent
    assert_eq!(px(a, 7 + 10, 7 + 10), 0xffff0000, "centre of the square");
    assert_eq!(px(a, 0, 0) >> 24, 0, "transparent background");
    assert!(
        px(a, 7 + 3, 7 + 3) >> 24 <= 8,
        "page corner is clear or only faintly shadowed"
    );
}

#[test]
fn shadow_is_black_offset_downwards_and_translucent() {
    let d = tempfile::tempdir().unwrap();
    write_source(&d.path().join("cursors"));
    let Some(o) = options(&d.path().join("out"), d.path()) else {
        return;
    };
    let r = build_theme(&o).unwrap();
    let a = &read(&r.theme_dir, "pointer")[0];
    let pad = 7u32;
    let above = px(a, pad + 10, pad + 4); // just above the square (square spans y 5..15)
    let below = px(a, pad + 10, pad + 16); // just below
    let (aa, ab) = (above >> 24, below >> 24);
    assert!(
        ab > aa,
        "shadow is heavier below ({ab}) than above ({aa}): offset down"
    );
    assert!(ab > 40 && ab <= 166, "translucent: {ab}");
    assert_eq!(below & 0x00ff_ffff, 0, "the shadow is black");
}

#[test]
fn shadow_can_be_turned_off_and_hotspot_source_chosen() {
    let d = tempfile::tempdir().unwrap();
    write_source(&d.path().join("cursors"));
    let Some(mut o) = options(&d.path().join("out"), d.path()) else {
        return;
    };
    o.shadow = ShadowMode::Never;
    o.hotspot_source = HotspotSource::Scaled;
    let r = build_theme(&o).unwrap();
    let a = &read(&r.theme_dir, "pointer")[0];
    assert_eq!((a.width, a.height), (20, 20));
    assert_eq!((a.xhot, a.yhot), (4, 6));
}

#[test]
fn stacked_frames_become_an_animation_with_aliases() {
    let d = tempfile::tempdir().unwrap();
    write_source(&d.path().join("cursors"));
    let Some(mut o) = options(&d.path().join("out"), d.path()) else {
        return;
    };
    o.shadow = ShadowMode::Never;
    let r = build_theme(&o).unwrap();
    let frames = read(&r.theme_dir, "progress");
    assert_eq!(frames.len(), 3 * 2, "three frames at two sizes");
    let first_size: Vec<_> = frames.iter().filter(|f| f.nominal_size == 24).collect();
    assert_eq!(first_size.len(), 3);
    assert!(first_size
        .iter()
        .all(|f| f.delay_ms == 100 && (f.width, f.height) == (10, 10)));
    assert_eq!(
        px(first_size[0], 5, 5),
        0xffff0000,
        "top frame is the first"
    );
    assert_eq!(px(first_size[1], 5, 5), 0xff00ff00);
    assert_eq!(px(first_size[2], 5, 5), 0xff0000ff);
    let cur = r.theme_dir.join("cursors");
    assert_eq!(
        std::fs::read_link(cur.join("wait")).unwrap(),
        Path::new("progress")
    );
    assert_eq!(
        std::fs::read_link(cur.join("left_ptr_watch")).unwrap(),
        Path::new("progress")
    );
}

#[test]
fn cursors_without_metadata_get_a_centre_hotspot_and_a_warning() {
    let d = tempfile::tempdir().unwrap();
    write_source(&d.path().join("cursors"));
    let Some(mut o) = options(&d.path().join("out"), d.path()) else {
        return;
    };
    o.shadow = ShadowMode::Never;
    let r = build_theme(&o).unwrap();
    assert!(
        r.warnings
            .iter()
            .any(|w| w.starts_with("cross:") && w.contains("hotspot")),
        "{:?}",
        r.warnings
    );
    let imgs = read(&r.theme_dir, "cross");
    assert_eq!((imgs[0].xhot, imgs[0].yhot), (8, 8));
    assert_eq!((imgs[1].xhot, imgs[1].yhot), (16, 16));
}

#[test]
fn stacked_square_frames_without_metadata_are_guessed() {
    let d = tempfile::tempdir().unwrap();
    write_source(&d.path().join("cursors"));
    let Some(mut o) = options(&d.path().join("out"), d.path()) else {
        return;
    };
    o.shadow = ShadowMode::Never;
    o.default_delay_ms = 250;
    let r = build_theme(&o).unwrap();
    assert!(
        r.warnings
            .iter()
            .any(|w| w.starts_with("countinguphand:") && w.contains("guessed 3")),
        "{:?}",
        r.warnings
    );
    let frames = read(&r.theme_dir, "countinguphand");
    assert_eq!(frames.len(), 3 * 2);
    assert!(frames.iter().all(|f| f.delay_ms == 250));
    assert_eq!(px(&frames[0], 4, 4), 0xffff0000);
    assert_eq!(px(&frames[2], 4, 4), 0xff0000ff);
    let cross = read(&r.theme_dir, "cross");
    assert_eq!(cross.len(), 2, "a square page is a single frame");
}

#[test]
fn missing_names_are_completed_with_the_procedural_set() {
    let d = tempfile::tempdir().unwrap();
    write_source(&d.path().join("cursors"));
    let Some(o) = options(&d.path().join("out"), d.path()) else {
        return;
    };
    let r = build_theme(&o).unwrap();
    let cur = r.theme_dir.join("cursors");
    for n in cupertino_cursor::names::REQUIRED_NAMES {
        assert!(cur.join(n).exists(), "{n}");
    }
    let default = read(&r.theme_dir, "default");
    assert_eq!(default.len(), 2);
    let mut no_fill = o.clone();
    no_fill.fill_missing = false;
    no_fill.output = d.path().join("out2");
    let r2 = build_theme(&no_fill).unwrap();
    assert!(!r2.theme_dir.join("cursors/default").exists());
    assert!(r2.theme_dir.join("cursors/pointer").exists());
}

#[test]
fn hyprcursor_source_theme_is_written_with_square_images() {
    let d = tempfile::tempdir().unwrap();
    write_source(&d.path().join("cursors"));
    let Some(mut o) = options(&d.path().join("out"), d.path()) else {
        return;
    };
    o.hyprcursor = true;
    let r = build_theme(&o).unwrap();
    let h = r.hypr_dir.expect("hyprcursor directory");
    let manifest = std::fs::read_to_string(h.join("manifest.hl")).unwrap();
    assert!(
        manifest.contains("name = Cupertino")
            && manifest.contains("cursors_directory = hyprcursors")
    );
    let meta = std::fs::read_to_string(h.join("hyprcursors/pointer/meta.hl")).unwrap();
    assert!(
        meta.contains("resize_algorithm = bilinear")
            && meta.contains("define_override = pointer")
            && meta.contains("define_override = hand2")
    );
    let sizes: Vec<&str> = meta
        .lines()
        .filter(|l| l.starts_with("define_size"))
        .collect();
    assert_eq!(sizes.len(), 2);
    assert!(
        sizes[0].starts_with("define_size = 34, image34.png"),
        "{}",
        sizes[0]
    );
    let img = tiny_skia::Pixmap::load_png(h.join("hyprcursors/pointer/image34.png")).unwrap();
    assert_eq!((img.width(), img.height()), (34, 34));
    let hot: Vec<f64> = meta
        .lines()
        .filter_map(|l| {
            l.strip_prefix("hotspot_x = ")
                .or_else(|| l.strip_prefix("hotspot_y = "))
        })
        .map(|v| v.parse().unwrap())
        .collect();
    assert!(hot.iter().all(|v| (0.0..=1.0).contains(v)));
    assert!((hot[0] - 17.5 / 34.0).abs() < 1e-3);
    let anim = std::fs::read_to_string(h.join("hyprcursors/progress/meta.hl")).unwrap();
    assert!(
        anim.lines()
            .filter(|l| l.contains("anim") && l.ends_with(", 100"))
            .count()
            >= 3,
        "{anim}"
    );
}

#[test]
fn the_reference_set_builds_when_provided() {
    let Some(dir) = std::env::var_os("CUPERTINO_CURSOR_SOURCE") else {
        return;
    };
    let d = tempfile::tempdir().unwrap();
    let mut o = BuildOptions::new(d.path());
    if !o.poppler.available() {
        return;
    }
    o.source = Source::Pdf(dir.into());
    o.metadata = std::env::var_os("CUPERTINO_CURSOR_METADATA").map(Into::into);
    o.sizes = vec![24, 32];
    let r = build_theme(&o).unwrap_or_else(|e| panic!("{e}"));
    let cur = r.theme_dir.join("cursors");
    for n in cupertino_cursor::names::REQUIRED_NAMES {
        assert!(cur.join(n).exists(), "{n}");
    }
    for f in std::fs::read_dir(&cur).unwrap().flatten() {
        let imgs = xcursor::parse(&std::fs::read(f.path()).unwrap())
            .unwrap_or_else(|e| panic!("{}: {e}", f.path().display()));
        assert!(
            imgs.iter().all(|i| i.pixels.iter().any(|p| p >> 24 != 0)),
            "{} is blank",
            f.path().display()
        );
    }
    assert!(std::fs::read_to_string(r.theme_dir.join("README.txt"))
        .unwrap()
        .contains("LOCAL USE ONLY"));
}
