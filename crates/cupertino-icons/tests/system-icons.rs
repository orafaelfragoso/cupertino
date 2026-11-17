//! `SystemIcons`: the system's own icons by logical name from the asset packs, with the placeholders.

#[allow(dead_code)]
mod support;

use cupertino_assets::Assets;
use cupertino_icons::SystemIcons;

/// The grey of the placeholder square.
const PLACEHOLDER: (u8, u8, u8) = (140, 140, 140);

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
fn a_raster_icon_comes_from_the_nearest_file_or_is_the_placeholder() {
    let dir = tempfile::tempdir().unwrap();
    let mut icons = SystemIcons::without_host(&pack(dir.path()));
    // (row, name, size, whether a pack file serves it, pixmap side, centre colour)
    for (row, name, size, has, side, centre) in [
        ("the file of the size", "folder", 32, true, 32, (200, 0, 0)),
        ("the next file above", "folder", 48, true, 64, (0, 200, 0)),
        (
            "a file at most twice the size is kept",
            "folder",
            16,
            true,
            32,
            (200, 0, 0),
        ),
        (
            "a file more than twice the size is shrunk",
            "folder",
            8,
            true,
            8,
            (200, 0, 0),
        ),
        (
            "a full-colour svg drawn at the size",
            "document.pdf",
            20,
            true,
            20,
            (255, 0, 0),
        ),
        (
            "no pack provides it",
            "document.audio",
            32,
            false,
            32,
            PLACEHOLDER,
        ),
        (
            "its file does not decode",
            "document.text",
            32,
            false,
            32,
            PLACEHOLDER,
        ),
        (
            "a symbol source is no raster",
            "symbol.house",
            16,
            false,
            16,
            PLACEHOLDER,
        ),
    ] {
        assert_eq!(icons.has_raster(name, size), has, "{row}");
        let pixmap = icons.raster(name, size);
        assert_eq!((pixmap.width(), pixmap.height()), (side, side), "{row}");
        let c = pixmap.pixel(side / 2, side / 2).unwrap();
        assert_eq!((c.red(), c.green(), c.blue()), centre, "{row}");
        if centre == PLACEHOLDER {
            assert_eq!(
                pixmap.pixel(0, 0).unwrap().alpha(),
                0,
                "{row}: a rounded square"
            );
        }
    }
    for (name, own) in [
        ("folder", true),
        ("document.pdf", true),
        ("symbol.house", false),
        ("document.audio", false),
    ] {
        assert_eq!(icons.pack_has_raster(name), own, "pack_has_raster {name}");
    }
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
    let mut icons = SystemIcons::without_host(&Assets::from_dirs(&[first, second]));
    assert_eq!(icons.raster("folder", 32).pixels()[0].blue(), 200);
}

#[test]
fn a_symbol_is_tinted_with_the_colour_or_is_the_outline_placeholder() {
    let dir = tempfile::tempdir().unwrap();
    let mut icons = SystemIcons::without_host(&pack(dir.path()));
    let house = icons.symbol("symbol.house", 16, [0.0, 0.0, 1.0, 1.0]);
    assert_eq!((house.width(), house.height()), (16, 16));
    let p = house.pixels()[0];
    assert_eq!((p.red(), p.green(), p.blue(), p.alpha()), (0, 0, 255, 255));
    let faint = icons
        .symbol("symbol.house", 16, [0.0, 0.0, 1.0, 0.5])
        .pixels()[0];
    assert!((i32::from(faint.alpha()) - 128).abs() <= 1);
    assert_eq!(faint.blue(), faint.alpha(), "premultiplied");

    // a symbol no pack provides, and a raster asked as a symbol, are an outline in the colour
    for name in ["symbol.star", "folder"] {
        let pixmap = icons.symbol(name, 48, [1.0, 0.0, 0.0, 1.0]);
        assert_eq!((pixmap.width(), pixmap.height()), (48, 48), "{name}");
        assert_eq!(
            pixmap.pixel(24, 24).unwrap().alpha(),
            0,
            "{name}: an outline, not a fill"
        );
        let drawn: Vec<_> = pixmap.pixels().iter().filter(|p| p.alpha() > 0).collect();
        assert!(!drawn.is_empty(), "{name}");
        assert!(
            drawn
                .iter()
                .all(|p| p.green() == 0 && p.blue() == 0 && p.red() > 0),
            "{name}"
        );
    }
}

#[test]
fn without_the_host_only_the_packs_serve_and_a_size_of_zero_is_still_square() {
    // without_host never reads the machine's themes: a name only the default map provides is a placeholder
    let mut icons = SystemIcons::without_host(&Assets::builtin());
    assert!(!icons.has_raster("folder", 32));
    assert!(!icons.pack_has_raster("folder"));
    let pixmap = icons.raster("document.audio", 0);
    assert!(pixmap.width() >= 1);
    assert_eq!(pixmap.width(), pixmap.height());
}

#[test]
#[should_panic]
fn a_name_that_is_not_a_logical_name_is_a_programming_error() {
    let mut icons = SystemIcons::without_host(&Assets::builtin());
    icons.raster("Not A Name!", 16);
}
