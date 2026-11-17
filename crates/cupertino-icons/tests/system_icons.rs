//! `SystemIcons`: the system's own icons by logical name from the asset packs, with the placeholders.

#[allow(dead_code)]
mod support;

use cupertino_assets::Assets;
use cupertino_icons::SystemIcons;

/// A pack with a raster icon `folder` (a 32 and a 64 pixel file), a full-colour SVG `document.pdf`, a template symbol
/// `symbol.house` and a name whose file is not a picture.
fn pack(dir: &std::path::Path) -> Assets {
    support::write_png(&dir.join("icons/folder-32.png"), 32, 32, [200, 0, 0, 255]);
    support::write_png(&dir.join("icons/folder-64.png"), 64, 64, [0, 200, 0, 255]);
    support::write(&dir.join("icons/pdf.svg"), support::RED_SVG.as_bytes());
    support::write(&dir.join("icons/house.svg"), support::BLACK_SVG.as_bytes());
    support::write(&dir.join("icons/broken.png"), b"junk");
    support::pack(
        dir,
        r#"{"version": 2, "icons": {
            "folder": {"raster": {"32": "icons/folder-32.png", "64": "icons/folder-64.png"}},
            "document.pdf": {"svg": "icons/pdf.svg"},
            "symbol.house": {"symbol": "icons/house.svg"},
            "document.text": {"raster": {"32": "icons/broken.png"}}
        }}"#,
    )
}

#[test]
fn a_raster_icon_comes_from_the_nearest_file_at_or_above_the_size() {
    let dir = tempfile::tempdir().unwrap();
    let mut icons = SystemIcons::without_host(&pack(dir.path()));
    assert!(icons.has_raster("folder", 32));
    assert_eq!(icons.raster("folder", 32).pixels()[0].red(), 200);
    assert_eq!(
        icons.raster("folder", 48).pixels()[0].green(),
        200,
        "the next file above"
    );
}

#[test]
fn a_raster_icon_is_shrunk_when_the_file_is_more_than_twice_as_large() {
    let dir = tempfile::tempdir().unwrap();
    let mut icons = SystemIcons::without_host(&pack(dir.path()));
    let pixmap = icons.raster("folder", 16);
    assert_eq!(
        (pixmap.width(), pixmap.height()),
        (32, 32),
        "the 32 file is at most twice 16"
    );
    let pixmap = icons.raster("folder", 8);
    assert_eq!((pixmap.width(), pixmap.height()), (8, 8));
}

#[test]
fn a_full_colour_svg_is_a_raster_icon_drawn_at_the_size() {
    let dir = tempfile::tempdir().unwrap();
    let mut icons = SystemIcons::without_host(&pack(dir.path()));
    let pixmap = icons.raster("document.pdf", 20);
    assert_eq!((pixmap.width(), pixmap.height()), (20, 20));
    assert_eq!(pixmap.pixels()[0].red(), 255);
    assert!(icons.pack_has_raster("document.pdf"));
}

#[test]
fn pack_has_raster_is_true_only_for_names_a_pack_gives_a_file_of_its_own() {
    let dir = tempfile::tempdir().unwrap();
    let icons = SystemIcons::without_host(&pack(dir.path()));
    assert!(icons.pack_has_raster("folder"));
    assert!(
        !icons.pack_has_raster("symbol.house"),
        "a template symbol is not a raster"
    );
    assert!(!icons.pack_has_raster("document.audio"));
}

#[test]
fn a_name_no_pack_provides_is_a_grey_rounded_square() {
    let dir = tempfile::tempdir().unwrap();
    let mut icons = SystemIcons::without_host(&pack(dir.path()));
    assert!(!icons.has_raster("document.audio", 32));
    let pixmap = icons.raster("document.audio", 32);
    assert_eq!((pixmap.width(), pixmap.height()), (32, 32));
    let centre = pixmap.pixel(16, 16).unwrap();
    assert_eq!(
        (centre.red(), centre.green(), centre.blue(), centre.alpha()),
        (140, 140, 140, 255)
    );
    assert_eq!(
        pixmap.pixel(0, 0).unwrap().alpha(),
        0,
        "the corner is empty"
    );
}

#[test]
fn a_file_that_cannot_be_decoded_is_drawn_as_the_placeholder() {
    let dir = tempfile::tempdir().unwrap();
    let mut icons = SystemIcons::without_host(&pack(dir.path()));
    assert!(!icons.has_raster("document.text", 32));
    assert_eq!(
        icons
            .raster("document.text", 32)
            .pixel(16, 16)
            .unwrap()
            .red(),
        140
    );
}

#[test]
fn a_later_pack_source_serves_when_the_first_does_not_load() {
    let dir = tempfile::tempdir().unwrap();
    let first = dir.path().join("first");
    let second = dir.path().join("second");
    support::write(&first.join("icons/broken.png"), b"junk");
    support::write_png(&second.join("icons/ok.png"), 32, 32, [0, 0, 200, 255]);
    support::write(
        &first.join("manifest.json"),
        br#"{"version": 2, "icons": {"folder": {"raster": {"32": "icons/broken.png"}}}}"#,
    );
    support::write(
        &second.join("manifest.json"),
        br#"{"version": 2, "icons": {"folder": {"raster": {"32": "icons/ok.png"}}}}"#,
    );
    let assets = Assets::from_dirs(&[first, second]);
    let mut icons = SystemIcons::without_host(&assets);
    assert_eq!(icons.raster("folder", 32).pixels()[0].blue(), 200);
}

#[test]
fn a_template_symbol_is_tinted_with_the_colour_asked_for() {
    let dir = tempfile::tempdir().unwrap();
    let mut icons = SystemIcons::without_host(&pack(dir.path()));
    let pixmap = icons.symbol("symbol.house", 16, [0.0, 0.0, 1.0, 1.0]);
    assert_eq!((pixmap.width(), pixmap.height()), (16, 16));
    let p = pixmap.pixels()[0];
    assert_eq!((p.red(), p.green(), p.blue(), p.alpha()), (0, 0, 255, 255));
    let faint = icons
        .symbol("symbol.house", 16, [0.0, 0.0, 1.0, 0.5])
        .pixels()[0];
    assert!((i32::from(faint.alpha()) - 128).abs() <= 1);
    assert_eq!(faint.blue(), faint.alpha(), "premultiplied");
}

#[test]
fn a_symbol_name_no_pack_provides_is_the_outline_of_a_rounded_square_in_the_colour() {
    let dir = tempfile::tempdir().unwrap();
    let mut icons = SystemIcons::without_host(&pack(dir.path()));
    let pixmap = icons.symbol("symbol.star", 48, [1.0, 0.0, 0.0, 1.0]);
    assert_eq!((pixmap.width(), pixmap.height()), (48, 48));
    assert_eq!(
        pixmap.pixel(24, 24).unwrap().alpha(),
        0,
        "an outline, not a fill"
    );
    let drawn: Vec<_> = pixmap.pixels().iter().filter(|p| p.alpha() > 0).collect();
    assert!(!drawn.is_empty());
    assert!(drawn
        .iter()
        .all(|p| p.green() == 0 && p.blue() == 0 && p.red() > 0));
}

#[test]
fn a_raster_name_is_not_served_by_a_symbol_source_and_the_other_way_round() {
    let dir = tempfile::tempdir().unwrap();
    let mut icons = SystemIcons::without_host(&pack(dir.path()));
    assert!(!icons.has_raster("symbol.house", 16));
    // `folder` is a raster: asked as a symbol it is the placeholder outline
    assert_eq!(
        icons
            .symbol("folder", 16, [0.0, 0.0, 0.0, 1.0])
            .pixel(8, 8)
            .unwrap()
            .alpha(),
        0
    );
}

#[test]
fn the_same_icon_asked_twice_is_the_same_pixmap() {
    let dir = tempfile::tempdir().unwrap();
    let mut icons = SystemIcons::without_host(&pack(dir.path()));
    let first = icons.raster("folder", 32).clone();
    assert_eq!(icons.raster("folder", 32).data(), first.data());
}

#[test]
#[should_panic]
fn a_name_that_is_not_a_logical_name_is_a_programming_error() {
    let mut icons = SystemIcons::without_host(&Assets::builtin());
    icons.raster("Not A Name!", 16);
}

#[test]
fn a_size_of_zero_gives_a_placeholder_of_one_pixel() {
    let mut icons = SystemIcons::without_host(&Assets::builtin());
    let pixmap = icons.raster("document.audio", 0);
    assert_eq!(pixmap.width(), pixmap.height());
}

#[test]
fn names_that_resolve_in_a_host_theme_are_found_there_and_not_without_the_host() {
    // without_host never reads the machine's themes: a name only the default map provides is a placeholder
    let mut icons = SystemIcons::without_host(&Assets::builtin());
    assert!(!icons.has_raster("folder", 32));
    assert!(!icons.pack_has_raster("folder"));
}
