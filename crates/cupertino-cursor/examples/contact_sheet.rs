//! Contact sheet of an XCursor theme: first frame of every cursor file (symlinks skipped) at one
//! nominal size, on a light and a dark tile side by side, with the hotspot marked.
//!
//! `cargo run -p cupertino-cursor --release --example contact_sheet -- [THEME_DIR] [OUT.png] [SIZE]`
//!
//! Without arguments a procedural theme is built in a temporary directory and the sheet is written to
//! `output/procedural_cursors.png`.

use cupertino_cursor::theme::{build_theme, BuildOptions};
use cupertino_cursor::xcursor;
use std::path::{Path, PathBuf};
use tiny_skia::{Color, FillRule, Paint, PathBuilder, Pixmap, PixmapPaint, Rect, Transform};

fn tile(pm: &mut Pixmap, x: i32, y: i32, size: u32, color: Color) {
    let mut p = Paint::default();
    p.set_color(color);
    let mut pb = PathBuilder::new();
    pb.push_rect(Rect::from_xywh(x as f32, y as f32, size as f32, size as f32).unwrap());
    pm.fill_path(
        &pb.finish().unwrap(),
        &p,
        FillRule::Winding,
        Transform::identity(),
        None,
    );
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let tmp = std::env::temp_dir().join(format!("cupertino-cursor-sheet-{}", std::process::id()));
    let (theme_dir, out): (PathBuf, PathBuf) = match args.first() {
        Some(t) => (
            PathBuf::from(t),
            PathBuf::from(
                args.get(1)
                    .cloned()
                    .unwrap_or_else(|| "contact_sheet.png".into()),
            ),
        ),
        None => {
            let mut o = BuildOptions::new(&tmp);
            o.sizes = vec![24, 32, 48];
            let r = build_theme(&o).expect("procedural theme");
            (
                r.theme_dir,
                Path::new(env!("CARGO_MANIFEST_DIR")).join("output/procedural_cursors.png"),
            )
        }
    };
    let want: u32 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(48);
    let mut files: Vec<_> = std::fs::read_dir(theme_dir.join("cursors"))
        .expect("cursors directory")
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .collect();
    files.sort_by_key(|e| e.file_name());
    // Each cursor's first frame at the size nearest `want`.
    let images: Vec<xcursor::XCursorImage> = files
        .iter()
        .map(|f| {
            let images = xcursor::parse(&std::fs::read(f.path()).unwrap()).unwrap();
            let size = images
                .iter()
                .map(|i| i.nominal_size)
                .min_by_key(|s| s.abs_diff(want))
                .unwrap();
            images.into_iter().find(|i| i.nominal_size == size).unwrap()
        })
        .collect();
    // A tile holds the largest image whole; a cell is a light and a dark tile side by side.
    let tile_size = images
        .iter()
        .map(|i| i.width.max(i.height))
        .max()
        .unwrap_or(want)
        + 8;
    let cols = 8u32;
    let rows = (images.len() as u32).div_ceil(cols);
    let mut sheet = Pixmap::new(cols * tile_size * 2, rows * tile_size).unwrap();
    sheet.fill(Color::from_rgba8(128, 128, 128, 255));
    for (i, img) in images.iter().enumerate() {
        let mut pm = Pixmap::new(img.width, img.height).unwrap();
        for (dst, p) in pm.pixels_mut().iter_mut().zip(&img.pixels) {
            let (a, r, g, b) = ((p >> 24) as u8, (p >> 16) as u8, (p >> 8) as u8, *p as u8);
            *dst = tiny_skia::PremultipliedColorU8::from_rgba(r.min(a), g.min(a), b.min(a), a)
                .unwrap();
        }
        let (cx, cy) = (
            (i as u32 % cols) * tile_size * 2,
            (i as u32 / cols) * tile_size,
        );
        for (dark, ox) in [(false, 0u32), (true, tile_size)] {
            let (tx, ty) = ((cx + ox) as i32, cy as i32);
            tile(
                &mut sheet,
                tx + 1,
                ty + 1,
                tile_size - 2,
                if dark {
                    Color::from_rgba8(40, 40, 44, 255)
                } else {
                    Color::from_rgba8(240, 240, 240, 255)
                },
            );
            let (px, py) = (
                tx + (tile_size - img.width) as i32 / 2,
                ty + (tile_size - img.height) as i32 / 2,
            );
            sheet.draw_pixmap(
                px,
                py,
                pm.as_ref(),
                &PixmapPaint::default(),
                Transform::identity(),
                None,
            );
            tile(
                &mut sheet,
                px + img.xhot as i32 - 1,
                py + img.yhot as i32 - 1,
                3,
                Color::from_rgba8(255, 0, 80, 255),
            );
        }
    }
    let _ = std::fs::remove_dir_all(&tmp);
    sheet.save_png(&out).unwrap();
    println!(
        "wrote {} ({} cursors, nominal size {want})",
        out.display(),
        files.len()
    );
}
