//! Contact sheet of an XCursor theme: first frame of every cursor file (symlinks skipped) at one
//! nominal size, on a light and a dark tile, with the hotspot marked.
//!
//! `cargo run -p cupertino-cursor --release --example contact_sheet -- [THEME_DIR] [OUT.png] [SIZE]`
//!
//! Without arguments a procedural theme is built in a temporary directory and the sheet is written to
//! `output/procedural_cursors.png`. Sheets of themes derived from a reference cursor set must stay in
//! git-ignored locations.

use cupertino_cursor::theme::{build_theme, BuildOptions};
use cupertino_cursor::xcursor;
use std::path::{Path, PathBuf};
use tiny_skia::{Color, FillRule, Paint, PathBuilder, Pixmap, PixmapPaint, Rect, Transform};

fn tile(pm: &mut Pixmap, x: i32, y: i32, size: u32, color: Color) {
    let mut p = Paint::default();
    p.set_color(color);
    let mut pb = PathBuilder::new();
    pb.push_rect(Rect::from_xywh(x as f32, y as f32, size as f32, size as f32).unwrap());
    pm.fill_path(&pb.finish().unwrap(), &p, FillRule::Winding, Transform::identity(), None);
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let tmp = std::env::temp_dir().join(format!("cupertino-cursor-sheet-{}", std::process::id()));
    let (theme_dir, out): (PathBuf, PathBuf) = match args.first() {
        Some(t) => (PathBuf::from(t), PathBuf::from(args.get(1).cloned().unwrap_or_else(|| "contact_sheet.png".into()))),
        None => {
            let mut o = BuildOptions::new(&tmp);
            o.sizes = vec![24, 32, 48];
            let r = build_theme(&o).expect("procedural theme");
            (r.theme_dir, Path::new(env!("CARGO_MANIFEST_DIR")).join("output/procedural_cursors.png"))
        }
    };
    let want: u32 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(48);
    let mut files: Vec<_> = std::fs::read_dir(theme_dir.join("cursors")).expect("cursors directory").flatten().filter(|e| e.file_type().is_ok_and(|t| t.is_file())).collect();
    files.sort_by_key(|e| e.file_name());
    let cell = want * 2 + 24;
    let cols = 8u32;
    let rows = (files.len() as u32).div_ceil(cols);
    let mut sheet = Pixmap::new(cols * cell, rows * cell).unwrap();
    sheet.fill(Color::from_rgba8(128, 128, 128, 255));
    for (i, f) in files.iter().enumerate() {
        let images = xcursor::parse(&std::fs::read(f.path()).unwrap()).unwrap();
        let sizes: Vec<u32> = images.iter().map(|i| i.nominal_size).collect();
        let size = sizes.iter().copied().min_by_key(|s| s.abs_diff(want)).unwrap();
        let img = images.iter().find(|i| i.nominal_size == size).unwrap();
        let mut pm = Pixmap::new(img.width, img.height).unwrap();
        for (dst, p) in pm.pixels_mut().iter_mut().zip(&img.pixels) {
            let (a, r, g, b) = ((p >> 24) as u8, (p >> 16) as u8, (p >> 8) as u8, *p as u8);
            *dst = tiny_skia::PremultipliedColorU8::from_rgba(r.min(a), g.min(a), b.min(a), a).unwrap();
        }
        let (cx, cy) = ((i as u32 % cols) * cell, (i as u32 / cols) * cell);
        for (dark, oy) in [(false, 0u32), (true, cell / 2)] {
            let (tx, ty) = (cx as i32, (cy + oy) as i32);
            let _ = oy;
            tile(&mut sheet, tx + 2, ty + 2, cell / 2 - 4, if dark { Color::from_rgba8(40, 40, 44, 255) } else { Color::from_rgba8(240, 240, 240, 255) });
            let (px, py) = (tx + (cell / 2) as i32 / 2 - img.xhot as i32, ty + (cell / 2) as i32 / 2 - img.yhot as i32);
            sheet.draw_pixmap(px, py, pm.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
            tile(&mut sheet, tx + (cell / 2) as i32 / 2 - 1, ty + (cell / 2) as i32 / 2 - 1, 3, Color::from_rgba8(255, 0, 80, 255));
        }
    }
    let _ = std::fs::remove_dir_all(&tmp);
    sheet.save_png(&out).unwrap();
    println!("wrote {} ({} cursors, nominal size {want})", out.display(), files.len());
}
