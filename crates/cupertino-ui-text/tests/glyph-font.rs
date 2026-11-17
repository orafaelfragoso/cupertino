//! Glyph rasterisation of the toolkit's built-in font: masks around real outlines, symmetry, subpixel invariance and stem darkening.

use cupertino_assets::BUILTIN_FONT as FONT;
use cupertino_raster_math::glyph::{declarative_size, rasterize, Mask, Smoothing};
use swash::scale::ScaleContext;
use swash::zeno::{Point, Verb};
use swash::FontRef;

fn raster(
    outline: &(Vec<Point>, Vec<Verb>),
    offset: (f32, f32),
    dilation: (f32, f32),
) -> Option<Mask> {
    rasterize(&outline.0, &outline.1, offset, dilation)
}

fn ink(mask: &Mask) -> f32 {
    mask.data.iter().map(|&v| f32::from(v) / 255.0).sum()
}

fn row(mask: &Mask, y: usize) -> &[u8] {
    &mask.data[y * mask.width as usize..(y + 1) * mask.width as usize]
}

fn glyph_outline(c: char, size: f32) -> Option<(Vec<Point>, Vec<Verb>)> {
    let font = FontRef::from_index(FONT, 0).expect("the font parses");
    let id = font.charmap().map(c);
    let mut context = ScaleContext::new();
    let mut scaler = context.builder(font).size(size).hint(false).build();
    let outline = scaler.scale_outline(id)?;
    Some((outline.points().to_vec(), outline.verbs().to_vec()))
}

#[test]
fn a_font_glyph_rasterises_to_a_mask_around_its_ink() {
    let outline = glyph_outline('H', 32.0).expect("the font has an H");
    let m = raster(&outline, (0.0, 0.0), (0.0, 0.0)).unwrap();
    assert!(m.width > 8 && m.width < 32, "{}", m.width);
    // an H is a cap height tall: about 0.73 em
    assert!((20..=26).contains(&m.height), "{}", m.height);
    assert!(m.top > 0, "the glyph stands on the baseline");
    assert_eq!(m.data.len(), (m.width * m.height) as usize);
    // full-coverage pixels sit in the stems, and the ink is some fraction of the box
    assert!(m.data.contains(&255));
    let box_area = (m.width * m.height) as f32;
    assert!(ink(&m) > 0.2 * box_area && ink(&m) < 0.6 * box_area);
}

#[test]
fn an_h_is_left_right_symmetric_to_the_subpixel() {
    let outline = glyph_outline('H', 40.0).unwrap();
    let m = raster(&outline, (0.0, 0.0), (0.0, 0.0)).unwrap();
    let w = m.width as usize;
    let mid = m.height as usize / 4;
    let left: f32 = row(&m, mid)[..w / 2].iter().map(|&v| f32::from(v)).sum();
    let right: f32 = row(&m, mid)[w - w / 2..]
        .iter()
        .map(|&v| f32::from(v))
        .sum();
    assert!(
        (left - right).abs() < 0.2 * left.max(right),
        "{left} {right}"
    );
}

#[test]
fn a_font_glyph_has_the_same_ink_wherever_it_sits_between_pixels() {
    let outline = glyph_outline('o', 24.0).unwrap();
    let base = ink(&raster(&outline, (0.0, 0.0), (0.0, 0.0)).unwrap());
    for dx in [0.25, 0.5, 0.75] {
        let moved = ink(&raster(&outline, (dx, 0.0), (0.0, 0.0)).unwrap());
        assert!(
            (moved - base).abs() < 0.01 * base,
            "{dx}: {moved} against {base}"
        );
    }
}

#[test]
fn stem_darkening_thickens_a_font_glyph_by_the_dilation_it_names() {
    let outline = glyph_outline('l', 16.0).unwrap();
    let plain = raster(&outline, (0.0, 0.0), (0.0, 0.0)).unwrap();
    let d = Smoothing::On.dilation(declarative_size(16.0), 1.0);
    let bold = raster(&outline, (0.0, 0.0), (d, d)).unwrap();
    assert!(d > 0.0 && d < 0.3);
    assert!(ink(&bold) > ink(&plain));
    // a stem gains d on each side
    let w = plain.width as f32;
    let gained = (ink(&bold) - ink(&plain)) / plain.height as f32;
    assert!(gained > d, "{gained} per row for {d}");
    assert!(gained < 4.0 * d + w * 0.01, "{gained}");
}

#[test]
fn the_font_has_glyphs_without_outlines() {
    // a space has no ink: the scaler yields no outline or an empty one, and either way there is no mask
    let mask = glyph_outline(' ', 16.0).and_then(|o| raster(&o, (0.0, 0.0), (0.1, 0.1)));
    assert!(mask.is_none());
}
