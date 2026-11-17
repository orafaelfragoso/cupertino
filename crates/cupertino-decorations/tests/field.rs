//! The inside of a text field: where the caret positions fall and what is drawn, clipped to the field.

use cupertino_assets::{Assets, FontRole};
use cupertino_decorations::field::{
    caret_offsets, centred_baseline, draw_field, FieldContent, FieldStyle,
};
use cupertino_decorations::text::TextRenderer;
use cupertino_geometry::Rect;
use tiny_skia::Pixmap;

const TEXT: [f32; 4] = [0.0, 0.0, 0.0, 1.0];
const SELECTION: [f32; 4] = [0.0, 0.0, 1.0, 1.0];
const CARET: [f32; 4] = [1.0, 0.0, 0.0, 1.0];

fn style() -> FieldStyle {
    FieldStyle {
        size: 13.0,
        weight: 400,
        letter_spacing: 0.0,
        text: TEXT,
        selection: SELECTION,
        caret: CARET,
        caret_width: 1.0,
    }
}

fn renderer() -> TextRenderer {
    TextRenderer::new(&Assets::discover(), FontRole::Ui)
        .expect("the fonts of the asset directory load")
}

/// The field `area` drawn with `content` on a transparent pixmap of the area's size at scale 1.
fn drawn(content: &FieldContent<'_>, area: &Rect) -> Pixmap {
    let mut pm = Pixmap::new(area.width() as u32, area.height() as u32).unwrap();
    let baseline = centred_baseline(area, style().size);
    draw_field(
        &mut renderer(),
        &mut pm,
        area,
        baseline,
        1.0,
        content,
        &style(),
    );
    pm
}

fn count(pm: &Pixmap, wanted: [u8; 3]) -> usize {
    pm.pixels()
        .iter()
        .filter(|p| p.alpha() == 255 && [p.red(), p.green(), p.blue()] == wanted)
        .count()
}

#[test]
fn caret_positions_grow_with_the_text_and_scale_with_the_letter_spacing() {
    let mut r = renderer();
    let text = "Hello world";
    let boundaries: Vec<usize> = (0..=text.len()).collect();
    let plain = caret_offsets(&mut r, text, &boundaries, &style(), 2.0);
    assert_eq!(plain[0], 0.0);
    assert!(plain.windows(2).all(|w| w[1] >= w[0]));
    assert!(
        plain[text.len()] > 40.0,
        "eleven letters of 13 pt are wider than 40 pt"
    );

    let mut spaced = style();
    spaced.letter_spacing = 1.0;
    let wide = caret_offsets(&mut r, text, &boundaries, &spaced, 2.0);
    assert!(
        (wide[text.len()] - plain[text.len()] - 10.0).abs() < 0.5,
        "a point between each pair of letters"
    );

    let at_one = caret_offsets(&mut r, text, &boundaries, &style(), 1.0);
    assert!(
        (at_one[text.len()] - plain[text.len()]).abs() < 1.5,
        "offsets are in points, whatever the scale"
    );
}

#[test]
fn the_field_draws_its_text_selection_and_caret_and_nothing_outside_its_area() {
    let area = Rect::new(0.0, 0.0, 120.0, 24.0);
    let text = "Hello";
    let boundaries: Vec<usize> = (0..=text.len()).collect();
    let offsets = caret_offsets(&mut renderer(), text, &boundaries, &style(), 1.0);
    let content =
        |selection: std::ops::Range<usize>, caret: Option<usize>, scroll: f64| FieldContent {
            text,
            boundaries: &boundaries,
            offsets: &offsets,
            selection,
            composition: None,
            caret,
            scroll,
        };
    let blue = [0, 0, 255];
    let red = [255, 0, 0];

    let plain = drawn(&content(0..0, None, 0.0), &area);
    assert!(
        plain.pixels().iter().any(|p| p.alpha() > 0),
        "the text is drawn"
    );
    assert_eq!(count(&plain, blue) + count(&plain, red), 0);

    let selected = drawn(&content(1..4, None, 0.0), &area);
    let band = count(&selected, blue);
    assert!(band > 0, "the selection band is drawn");
    let (first, last) = (offsets[1], offsets[4]);
    let band_columns: Vec<u32> = (0..selected.width())
        .filter(|x| {
            (0..selected.height()).any(|y| {
                let p = selected.pixels()[(y * selected.width() + x) as usize];
                p.alpha() == 255 && [p.red(), p.green(), p.blue()] == blue
            })
        })
        .collect();
    assert!(*band_columns.first().unwrap() as f64 >= first.floor() - 1.0);
    assert!(*band_columns.last().unwrap() as f64 <= last.ceil() + 1.0);

    let careted = drawn(&content(0..0, Some(2), 0.0), &area);
    assert!(count(&careted, red) > 0, "the caret is drawn");

    let scrolled = drawn(&content(0..0, Some(5), offsets[5]), &area);
    assert!(
        count(&scrolled, red) > 0,
        "scrolling the text keeps the caret in view"
    );
    assert_ne!(scrolled.data(), careted.data());
}

#[test]
fn drawing_stays_inside_the_area() {
    let area = Rect::new(10.0, 4.0, 60.0, 20.0);
    let text = "A line far too long to fit in sixty points";
    let boundaries = [0, text.len()];
    let offsets = [0.0, 400.0];
    let content = FieldContent {
        text,
        boundaries: &boundaries,
        offsets: &offsets,
        selection: 0..text.len(),
        composition: None,
        caret: None,
        scroll: 0.0,
    };
    let mut pm = Pixmap::new(100, 40).unwrap();
    draw_field(
        &mut renderer(),
        &mut pm,
        &area,
        20.0,
        1.0,
        &content,
        &style(),
    );
    for (i, p) in pm.pixels().iter().enumerate() {
        let (x, y) = ((i % 100) as f64, (i / 100) as f64);
        if !area.contains(cupertino_geometry::Point::new(x, y)) {
            assert_eq!(p.alpha(), 0, "({x}, {y}) is outside the field");
        }
    }
}
