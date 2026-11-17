//! Loading icon files into premultiplied pixels: PNG bytes and files, SVG files, ARGB pixmaps and the system mark.

#[allow(dead_code)]
mod support;

use cupertino_icons::{decode_png, from_argb, load_icon, system_mark};

#[test]
fn a_png_decodes_premultiplied_and_is_shrunk_only_past_twice_the_size() {
    // (row, picture size, colour, asked size, decoded size, first pixel)
    for (row, (w, h), rgba, size, expected, first) in [
        (
            "its own size",
            (4, 3),
            [10, 20, 30, 255],
            32,
            (4, 3),
            [10, 20, 30, 255],
        ),
        (
            "premultiplied",
            (1, 1),
            [200, 100, 50, 128],
            8,
            (1, 1),
            [100, 50, 25, 128],
        ),
        ("clear", (1, 1), [200, 100, 50, 0], 8, (1, 1), [0, 0, 0, 0]),
        (
            "up to twice keeps its size",
            (32, 20),
            [1, 2, 3, 255],
            16,
            (32, 20),
            [1, 2, 3, 255],
        ),
        (
            "more than twice is shrunk",
            (100, 100),
            [1, 2, 3, 255],
            16,
            (16, 16),
            [1, 2, 3, 255],
        ),
        (
            "a size of zero is one pixel",
            (8, 8),
            [1, 2, 3, 255],
            0,
            (1, 1),
            [1, 2, 3, 255],
        ),
    ] {
        let img = decode_png(&support::png_bytes(w, h, rgba), size).expect(row);
        assert_eq!((img.width, img.height), expected, "{row}");
        assert_eq!(
            img.rgba.len(),
            (expected.0 * expected.1 * 4) as usize,
            "{row}"
        );
        assert_eq!(img.rgba[..4], first, "{row}");
    }
    for bytes in [&b"not a picture"[..], &[]] {
        assert_eq!(decode_png(bytes, 16), None, "{bytes:?}");
    }
}

#[test]
fn an_icon_file_loads_by_its_content_drawn_to_fit_the_size() {
    let dir = tempfile::tempdir().unwrap();
    let file = |name: &str, bytes: &[u8]| {
        let path = dir.path().join(name);
        support::write(&path, bytes);
        path
    };
    let png = support::png_bytes(6, 6, [9, 8, 7, 255]);
    let wide = br##"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="20"><rect width="40" height="20" fill="#00ff00"/></svg>"##;
    for (row, path, size, expected) in [
        (
            "a png file is its bytes",
            file("a.png", &png),
            32,
            decode_png(&png, 32),
        ),
        ("missing", dir.path().join("none.png"), 16, None),
        ("a broken png", file("broken.png", b"junk"), 16, None),
        ("a broken svg", file("bad.svg", b"<svg"), 16, None),
    ] {
        assert_eq!(load_icon(&path, size), expected, "{row}");
    }
    for (row, path, size, dims, first) in [
        (
            "an svg fills the size",
            file("red.svg", support::RED_SVG.as_bytes()),
            32,
            (32, 32),
            [255, 0, 0, 255],
        ),
        (
            "an svg keeps its aspect",
            file("wide.svg", wide),
            20,
            (20, 10),
            [0, 255, 0, 255],
        ),
    ] {
        let img = load_icon(&path, size).expect(row);
        assert_eq!((img.width, img.height), dims, "{row}");
        assert_eq!(img.rgba[..4], first, "{row}");
    }
}

#[test]
fn an_argb_pixmap_becomes_premultiplied_rgba_and_a_wrong_size_is_refused() {
    // network byte order: alpha first
    for (row, w, h, argb, expected) in [
        (
            "premultiplied",
            1,
            1,
            &[128, 200, 100, 50][..],
            Some(vec![100, 50, 25, 128]),
        ),
        (
            "two pixels",
            2,
            1,
            &[255, 1, 2, 3, 255, 4, 5, 6],
            Some(vec![1, 2, 3, 255, 4, 5, 6, 255]),
        ),
        ("too short", 2, 2, &[0; 12], None),
        ("too long", 1, 1, &[0; 8], None),
        ("no width", 0, 1, &[], None),
        ("a negative height", 1, -1, &[0; 4], None),
    ] {
        let img = from_argb(w, h, argb);
        assert_eq!(img.as_ref().map(|i| i.rgba.clone()), expected, "{row}");
        if let Some(img) = img {
            assert_eq!((img.width, img.height), (w as u32, h as u32), "{row}");
        }
    }
}

#[test]
fn the_system_mark_is_drawn_in_the_colour_asked_for_at_any_size() {
    let mark = system_mark(32, [1.0, 0.0, 0.0, 1.0]);
    assert_eq!((mark.width(), mark.height()), (32, 32));
    let opaque: Vec<_> = mark.pixels().iter().filter(|p| p.alpha() == 255).collect();
    assert!(!opaque.is_empty(), "the mark covers some pixels");
    assert!(opaque
        .iter()
        .all(|p| p.red() == 255 && p.green() == 0 && p.blue() == 0));

    let half = system_mark(32, [0.0, 1.0, 0.0, 0.5]);
    let max = half.pixels().iter().map(|p| p.alpha()).max().unwrap();
    assert!((i32::from(max) - 128).abs() <= 1, "alpha scales coverage");
    assert!(
        half.pixels().iter().all(|p| p.green() == p.alpha()),
        "premultiplied green"
    );

    let tiny = system_mark(0, [0.0, 0.0, 0.0, 1.0]);
    assert!(tiny.width() >= 1 && tiny.height() >= 1, "never empty");
}
