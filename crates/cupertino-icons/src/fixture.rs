//! A small asset directory for the tests: solid-colour 16 px rasters and one-rectangle template symbols.

use cupertino_assets::Assets;
use std::path::Path;

/// The red component of the solid colour the fixture gives the raster `name` (distinct per name).
pub fn red(name: &str) -> u8 {
    name.bytes().fold(7u8, |h, b| h.wrapping_mul(31).wrapping_add(b)) | 1
}

/// An opened asset directory in `root` with the raster icons `rasters` and the template symbols `symbols`.
pub fn assets(root: &Path, rasters: &[&str], symbols: &[&str]) -> Assets {
    let entry = |name: &&str| {
        let raster = if rasters.contains(name) { "\"raster\": [16]" } else { "" };
        let symbol = if symbols.contains(name) { "\"symbol\": true" } else { "" };
        format!("\"{name}\": {{{}}}", [raster, symbol].iter().filter(|s| !s.is_empty()).copied().collect::<Vec<_>>().join(", "))
    };
    let names: Vec<&str> = rasters.iter().chain(symbols).copied().collect();
    let icons = names.iter().map(entry).collect::<Vec<_>>().join(", ");
    std::fs::write(root.join("manifest.json"), format!("{{\"version\": 1, \"fonts\": {{\"ui\": \"Fixture\"}}, \"icons\": {{{icons}}}, \"cursors\": \"fixture\", \"wallpaper\": \"wallpapers/default.png\"}}")).unwrap();
    for rel in ["fonts/a.ttf", "cursors/fixture/index.theme", "wallpapers/default.png"] {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, b"x").unwrap();
    }
    for name in rasters {
        let dir = root.join("icons").join(name);
        std::fs::create_dir_all(&dir).unwrap();
        image::RgbaImage::from_pixel(16, 16, image::Rgba([red(name), 0, 0, 255])).save(dir.join("16.png")).unwrap();
    }
    for name in symbols {
        let dir = root.join("icons").join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("symbol.svg"), "<svg xmlns='http://www.w3.org/2000/svg' width='16' height='16'><rect x='4' width='8' height='16' fill='#000'/></svg>").unwrap();
    }
    Assets::open(root).unwrap()
}
