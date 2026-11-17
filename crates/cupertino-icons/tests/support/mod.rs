//! Files for the icon tests: small pictures and asset packs in a temporary directory.

use std::path::{Path, PathBuf};

use cupertino_assets::Assets;

/// Encoded PNG bytes of a `w` x `h` picture of one straight RGBA colour.
pub fn png_bytes(w: u32, h: u32, rgba: [u8; 4]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut enc = png::Encoder::new(&mut out, w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut writer = enc.write_header().unwrap();
    let data: Vec<u8> = (0..w * h).flat_map(|_| rgba).collect();
    writer.write_image_data(&data).unwrap();
    drop(writer);
    out
}

/// Write a file, making its directories.
pub fn write(path: &Path, bytes: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

pub fn write_png(path: &Path, w: u32, h: u32, rgba: [u8; 4]) {
    write(path, &png_bytes(w, h, rgba));
}

pub const RED_SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 16 16"><rect width="16" height="16" fill="#ff0000"/></svg>"##;
pub const BLACK_SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 16 16"><rect width="16" height="16" fill="#000000"/></svg>"##;

/// An asset pack directory with `manifest` written as `manifest.json`, read as the only pack.
pub fn pack(dir: &Path, manifest: &str) -> Assets {
    write(&dir.join("manifest.json"), manifest.as_bytes());
    Assets::from_dirs(&[dir.to_path_buf()])
}

/// A theme `name` under `base` with the directories `(path, size, type)`; the files are written by the caller.
pub fn theme(
    base: &Path,
    name: &str,
    inherits: Option<&str>,
    dirs: &[(&str, u32, &str)],
) -> PathBuf {
    let mut text = String::from("[Icon Theme]\nName=T\n");
    if let Some(i) = inherits {
        text.push_str(&format!("Inherits={i}\n"));
    }
    text.push_str("Directories=");
    text.push_str(&dirs.iter().map(|d| d.0).collect::<Vec<_>>().join(","));
    text.push('\n');
    for (path, size, kind) in dirs {
        text.push_str(&format!("\n[{path}]\nSize={size}\nType={kind}\n"));
    }
    let root = base.join(name);
    write(&root.join("index.theme"), text.as_bytes());
    root
}
