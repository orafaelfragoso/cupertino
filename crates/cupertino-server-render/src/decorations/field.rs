//! The inside of a single-line text field: the selection band, the text, the composition underline and the caret,
//! clipped to the field and scrolled horizontally. The field's model (`cupertino-ui-text`'s `TextBuffer`) gives the ranges, the
//! caret positions and the scroll; [`caret_offsets`] measures where its caret positions are.

use std::ops::Range;

use cupertino_geometry::Rect;
use tiny_skia::{Paint, Pixmap, PixmapPaint, Rect as SkRect, Transform};

use crate::decorations::palette::Rgba;
use crate::decorations::text::TextRenderer;

/// Points the selection band and the caret reach above and below the text size.
pub const BAND_OVERHANG: f64 = 2.0;
/// The baseline's distance below the centre of the text's band, as a share of the text size.
const BASELINE_SHARE: f64 = 0.36;

/// Text size and colours of a field.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FieldStyle {
    /// The drawn text size in points.
    pub size: f64,
    pub weight: u16,
    /// Points between glyphs (tracking).
    pub letter_spacing: f64,
    pub text: Rgba,
    /// The selected text's background (the `selected-text-background` token).
    pub selection: Rgba,
    pub caret: Rgba,
    /// Points (at least one device pixel).
    pub caret_width: f64,
}

/// The baseline (points) that centres text of `size` in `area` vertically.
pub fn centred_baseline(area: &Rect, size: f64) -> f64 {
    area.y() + area.height() / 2.0 + size * BASELINE_SHARE
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
    renderer.set_scale(scale);
    let px = style.size as f32 * scale;
    let spacing = (style.letter_spacing * f64::from(scale)) as f32;
    renderer
        .offsets(text, boundaries, px, style.weight, spacing)
        .into_iter()
        .map(|x| f64::from(x / scale))
        .collect()
}

/// Draw `content` on `baseline` (points) into `area` (points) of `pm` (device pixels, `scale` per point), clipped to
/// `area`.
pub fn draw_field(
    renderer: &mut TextRenderer,
    pm: &mut Pixmap,
    area: &Rect,
    baseline: f64,
    scale: f32,
    content: &FieldContent<'_>,
    style: &FieldStyle,
) {
    renderer.set_scale(scale);
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
    let baseline = baseline - area.y();
    let centre = baseline - style.size * BASELINE_SHARE;
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
    renderer.draw(
        &mut clip,
        content.text,
        (-content.scroll * s) as f32,
        (baseline * s) as f32,
        style.size as f32 * scale,
        style.weight,
        (style.letter_spacing * s) as f32,
        style.text,
    );
    if let Some(marked) = &content.composition {
        let (a, b) = (x_of(marked.start), x_of(marked.end));
        fill(&mut clip, a, baseline + 1.5, b - a, 1.0 / s, s, style.text);
    }
    if let Some(caret) = content.caret {
        let x = (x_of(caret) * s).round() / s;
        let width = (style.caret_width * s).round().max(1.0) / s;
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
