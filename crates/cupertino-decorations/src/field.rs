//! The inside of a single-line text field: the selection band, the text, the composition underline and the caret,
//! clipped to the field and scrolled horizontally. The field's model (`cupertino-text-field`) gives the ranges, the
//! caret positions and the scroll; [`caret_offsets`] measures where its caret positions are.

use std::ops::Range;

use cupertino_geometry::Rect;
use tiny_skia::{Paint, Pixmap, PixmapPaint, Rect as SkRect, Transform};

use crate::palette::Rgba;
use crate::text::TextRenderer;

/// Points the selection band and the caret reach above and below the text size.
pub const BAND_OVERHANG: f64 = 2.0;
/// The caret's width in points (at least one device pixel).
pub const CARET_WIDTH: f64 = 1.0;
/// The baseline's distance below the field's vertical centre, as a share of the text size.
const BASELINE_SHARE: f64 = 0.36;

/// Text size and colours of a field.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FieldStyle {
    /// Points.
    pub size: f64,
    pub weight: u16,
    pub text: Rgba,
    /// The selected text's background (the `selected-text-background` token).
    pub selection: Rgba,
    pub caret: Rgba,
}

/// What the field shows, in byte offsets of `text`.
#[derive(Clone, Debug, PartialEq)]
pub struct FieldContent<'a> {
    pub text: &'a str,
    /// The caret positions and their x in points from the text's start ([`caret_offsets`]), in the same order.
    pub boundaries: &'a [usize],
    pub offsets: &'a [f64],
    pub selection: Range<usize>,
    pub composition: Option<Range<usize>>,
    /// The caret, when it shows (the field has the keyboard, nothing is selected, the blink is on).
    pub caret: Option<usize>,
    /// Points the text is scrolled left.
    pub scroll: f64,
}

/// The x in points of each of `boundaries` (byte offsets of `text`) from the text's start.
pub fn caret_offsets(
    renderer: &mut TextRenderer,
    text: &str,
    boundaries: &[usize],
    style: &FieldStyle,
    scale: f32,
) -> Vec<f64> {
    let px = style.size as f32 * scale;
    boundaries
        .iter()
        .map(|b| f64::from(renderer.measure(&text[..*b], px, style.weight, 0.0) / scale))
        .collect()
}

/// Draw `content` into `area` (points) of `pm` (device pixels, `scale` per point), clipped to `area`.
pub fn draw_field(
    renderer: &mut TextRenderer,
    pm: &mut Pixmap,
    area: &Rect,
    scale: f32,
    content: &FieldContent<'_>,
    style: &FieldStyle,
) {
    let s = f64::from(scale);
    let (Some(mut clip), x0, y0) = (
        Pixmap::new(
            (area.width() * s).ceil().max(1.0) as u32,
            (area.height() * s).ceil().max(1.0) as u32,
        ),
        (area.x() * s).round(),
        (area.y() * s).round(),
    ) else {
        return;
    };
    let x_of = |byte: usize| {
        content
            .boundaries
            .iter()
            .position(|b| *b == byte)
            .map_or(0.0, |i| content.offsets[i])
            - content.scroll
    };
    let centre = area.height() / 2.0;
    let band_top = centre - style.size / 2.0 - BAND_OVERHANG;
    let band_height = style.size + 2.0 * BAND_OVERHANG;
    if !content.selection.is_empty() {
        let (a, b) = (x_of(content.selection.start), x_of(content.selection.end));
        fill(
            &mut clip,
            a,
            band_top,
            b - a,
            band_height,
            s,
            style.selection,
        );
    }
    let baseline = centre + style.size * BASELINE_SHARE;
    renderer.draw(
        &mut clip,
        content.text,
        (-content.scroll * s) as f32,
        (baseline * s) as f32,
        style.size as f32 * scale,
        style.weight,
        0.0,
        style.text,
    );
    if let Some(marked) = &content.composition {
        let (a, b) = (x_of(marked.start), x_of(marked.end));
        fill(&mut clip, a, baseline + 1.5, b - a, 1.0 / s, s, style.text);
    }
    if let Some(caret) = content.caret {
        let x = (x_of(caret) * s).round() / s;
        let width = (CARET_WIDTH * s).round().max(1.0) / s;
        fill(&mut clip, x, band_top, width, band_height, s, style.caret);
    }
    pm.draw_pixmap(
        x0 as i32,
        y0 as i32,
        clip.as_ref(),
        &PixmapPaint::default(),
        Transform::identity(),
        None,
    );
}

fn fill(pm: &mut Pixmap, x: f64, y: f64, w: f64, h: f64, s: f64, color: Rgba) {
    let Some(rect) = SkRect::from_xywh(
        (x * s) as f32,
        (y * s) as f32,
        (w * s) as f32,
        (h * s) as f32,
    ) else {
        return;
    };
    let mut paint = Paint::default();
    paint.set_color_rgba8(
        (color[0] * 255.0).round() as u8,
        (color[1] * 255.0).round() as u8,
        (color[2] * 255.0).round() as u8,
        (color[3] * 255.0).round() as u8,
    );
    pm.fill_rect(rect, &paint, Transform::identity(), None);
}

#[cfg(test)]
mod tests {
    use super::*;
    use cupertino_assets::{Assets, FontRole};

    const BLUE: Rgba = [0.0, 0.0, 1.0, 1.0];
    const RED: Rgba = [1.0, 0.0, 0.0, 1.0];
    const STYLE: FieldStyle = FieldStyle {
        size: 13.0,
        weight: 400,
        text: [0.0, 0.0, 0.0, 1.0],
        selection: BLUE,
        caret: RED,
    };

    fn rgb(pm: &Pixmap, x: u32, y: u32) -> [u8; 4] {
        let p = pm.pixel(x, y).unwrap().demultiply();
        [p.red(), p.green(), p.blue(), p.alpha()]
    }

    #[test]
    fn the_band_covers_the_selection_the_caret_sits_on_its_offset_and_nothing_leaves_the_field() {
        let mut r = TextRenderer::new(&Assets::builtin(), FontRole::Ui).unwrap();
        let text = "ab cd efgh ijkl mnop";
        let boundaries: Vec<usize> = (0..=text.len()).collect();
        let offsets = caret_offsets(&mut r, text, &boundaries, &STYLE, 1.0);
        assert_eq!(offsets[0], 0.0);
        assert!(offsets.windows(2).all(|w| w[1] > w[0]), "{offsets:?}");
        let area = Rect::new(10.0, 10.0, 60.0, 20.0);
        let draw = |r: &mut TextRenderer, selection: Range<usize>, caret: Option<usize>| {
            let mut pm = Pixmap::new(200, 40).unwrap();
            let content = FieldContent {
                text,
                boundaries: &boundaries,
                offsets: &offsets,
                selection,
                composition: None,
                caret,
                scroll: 0.0,
            };
            draw_field(r, &mut pm, &area, 1.0, &content, &STYLE);
            pm
        };
        // the band's top row, above the glyphs: y = 10 + 10 - 6.5 - 2
        let band_y = 12;
        let selected = draw(&mut r, 3..5, None);
        let mid = 10 + ((offsets[3] + offsets[5]) / 2.0) as u32;
        assert_eq!(rgb(&selected, mid, band_y), [0, 0, 255, 255]);
        assert_eq!(
            rgb(&selected, 10 + offsets[1] as u32, band_y)[3],
            0,
            "outside the selection"
        );
        let caret = draw(&mut r, 0..0, Some(3));
        assert_eq!(
            rgb(&caret, 10 + offsets[3].round() as u32, band_y),
            [255, 0, 0, 255]
        );
        let ink = |pm: &Pixmap, x: u32| (0..40).any(|y| rgb(pm, x, y)[3] > 0);
        assert!(ink(&caret, 30), "text inside the field");
        assert!(
            offsets[text.len()] > 80.0,
            "the text is wider than the field"
        );
        assert!(
            !(71..200).any(|x| ink(&caret, x)),
            "nothing right of the field"
        );
    }
}
