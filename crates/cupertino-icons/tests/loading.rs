//! Loading icon files into premultiplied pixels: PNG bytes and files, SVG files, ARGB pixmaps and the system mark.

#[allow(dead_code)]
mod support;

use cupertino_icons::{decode_png, from_argb, load_icon, system_mark, IconImage};

#[test]
fn a_png_is_decoded_at_its_own_size() {
    let img = decode_png(&support::png_bytes(4, 3, [10, 20, 30, 255]), 32).unwrap();
    assert_eq!((img.width, img.height), (4, 3));
    assert_eq!(img.rgba.len(), 4 * 3 * 4);
    assert_eq!(&img.rgba[..4], &[10, 20, 30, 255]);
}

#[test]
fn a_png_is_premultiplied_by_its_alpha() {
    let img = decode_png(&support::png_bytes(1, 1, [200, 100, 50, 128]), 8).unwrap();
    assert_eq!(img.rgba, [100, 50, 25, 128]);
    let clear = decode_png(&support::png_bytes(1, 1, [200, 100, 50, 0]), 8).unwrap();
    assert_eq!(clear.rgba, [0, 0, 0, 0]);
}

#[test]
fn a_png_more_than_twice_as_large_as_asked_is_shrunk_to_it() {
    let img = decode_png(&support::png_bytes(100, 100, [1, 2, 3, 255]), 16).unwrap();
    assert_eq!((img.width, img.height), (16, 16));
}

#[test]
fn a_png_up_to_twice_as_large_as_asked_keeps_its_size() {
    let img = decode_png(&support::png_bytes(32, 20, [1, 2, 3, 255]), 16).unwrap();
    assert_eq!((img.width, img.height), (32, 20));
}

#[test]
fn a_size_of_zero_is_taken_as_one_pixel() {
    let img = decode_png(&support::png_bytes(8, 8, [1, 2, 3, 255]), 0).unwrap();
    assert_eq!((img.width, img.height), (1, 1));
}

#[test]
fn bytes_that_are_not_a_picture_do_not_decode() {
    assert_eq!(decode_png(b"not a picture", 16), None);
    assert_eq!(decode_png(&[], 16), None);
}

#[test]
fn a_png_file_loads_like_its_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.png");
    support::write_png(&path, 6, 6, [9, 8, 7, 255]);
    assert_eq!(
        load_icon(&path, 32),
        decode_png(&support::png_bytes(6, 6, [9, 8, 7, 255]), 32)
    );
}

#[test]
fn a_missing_or_broken_file_does_not_load() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(load_icon(&dir.path().join("none.png"), 16), None);
    let broken = dir.path().join("broken.png");
    support::write(&broken, b"junk");
    assert_eq!(load_icon(&broken, 16), None);
    let bad_svg = dir.path().join("bad.svg");
    support::write(&bad_svg, b"<svg");
    assert_eq!(load_icon(&bad_svg, 16), None);
}

#[test]
fn an_svg_file_is_drawn_to_fit_the_size() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.svg");
    support::write(&path, support::RED_SVG.as_bytes());
    let img = load_icon(&path, 32).unwrap();
    assert_eq!((img.width, img.height), (32, 32));
    assert_eq!(&img.rgba[..4], &[255, 0, 0, 255]);
}

#[test]
fn an_svg_keeps_its_aspect_ratio() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("wide.svg");
    support::write(
        &path,
        br##"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="20"><rect width="40" height="20" fill="#00ff00"/></svg>"##,
    );
    let img = load_icon(&path, 20).unwrap();
    assert_eq!((img.width, img.height), (20, 10));
}

#[test]
fn an_argb_pixmap_becomes_premultiplied_rgba() {
    // network byte order: alpha first
    let img = from_argb(1, 1, &[128, 200, 100, 50]).unwrap();
    assert_eq!(
        img,
        IconImage {
            width: 1,
            height: 1,
            rgba: vec![100, 50, 25, 128]
        }
    );
    let img = from_argb(2, 1, &[255, 1, 2, 3, 255, 4, 5, 6]).unwrap();
    assert_eq!(img.rgba, [1, 2, 3, 255, 4, 5, 6, 255]);
}

#[test]
fn an_argb_pixmap_of_the_wrong_length_or_size_is_refused() {
    assert_eq!(from_argb(2, 2, &[0; 12]), None);
    assert_eq!(from_argb(1, 1, &[0; 8]), None);
    assert_eq!(from_argb(0, 1, &[]), None);
    assert_eq!(from_argb(1, -1, &[0; 4]), None);
}

#[test]
fn the_system_mark_is_drawn_in_the_colour_asked_for() {
    let mark = system_mark(32, [1.0, 0.0, 0.0, 1.0]);
    assert_eq!((mark.width(), mark.height()), (32, 32));
    let opaque: Vec<_> = mark.pixels().iter().filter(|p| p.alpha() == 255).collect();
    assert!(!opaque.is_empty(), "the mark covers some pixels");
    assert!(opaque
        .iter()
        .all(|p| p.red() == 255 && p.green() == 0 && p.blue() == 0));
}

#[test]
fn the_system_mark_colour_alpha_scales_its_coverage() {
    let full = system_mark(32, [0.0, 1.0, 0.0, 1.0]);
    let half = system_mark(32, [0.0, 1.0, 0.0, 0.5]);
    let max = |p: &tiny_skia::Pixmap| p.pixels().iter().map(|p| p.alpha()).max().unwrap();
    assert_eq!(max(&full), 255);
    assert!((i32::from(max(&half)) - 128).abs() <= 1);
    assert!(
        half.pixels().iter().all(|p| p.green() == p.alpha()),
        "premultiplied green"
    );
}

#[test]
fn the_system_mark_is_never_empty_even_for_a_size_of_zero() {
    let mark = system_mark(0, [0.0, 0.0, 0.0, 1.0]);
    assert!(mark.width() >= 1 && mark.height() >= 1);
}
