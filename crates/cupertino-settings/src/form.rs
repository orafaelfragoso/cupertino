//! The grouped form a pane is drawn as (§6.1): the form root inside the content column,
//! sections of rows in rounded boxes, headers and footers. The root's insets, its content cap and the trailing row's gap
//! are (roles); the box, row and section values are placeholders of one table
//! until closes (approximate, O4).

use crate::controls::Control;
use crate::geom::Rect;

/// Form metrics.
pub mod metrics {
    /// The form root's insets (top, leading, bottom, trailing).
    pub const ROOT_INSETS: (f64, f64, f64, f64) = (20.0, 20.0, 20.0, 20.0);
    /// The content's largest width, centred in the column.
    pub const CONTENT_MAX_WIDTH: f64 = 600.0;
    /// Between the form content and a trailing button row (no pane of the minimum set has one).
    pub const TRAILING_ROW_GAP: f64 = 20.0;
    /// The smallest row.
    pub const ROW_MIN_HEIGHT: f64 = 24.0;
    /// The placeholders (approximate, O4): between sections (11, measured on a screenshot), the box's corner radius, the row line's thickness, a row's vertical and horizontal padding, the row line's
    /// leading inset and its trailing gap (12, measured there), the header's and footer's size and gap, the label sizes.
    pub const SECTION_SPACING: f64 = 11.0;
    pub const BOX_RADIUS: f64 = 10.0;
    pub const BOX_HAIRLINE: f64 = 1.0;
    pub const ROW_PADDING_V: f64 = 8.0;
    pub const ROW_PADDING_H: f64 = 10.0;
    pub const SEPARATOR_INSET: f64 = 10.0;
    pub const SEPARATOR_TRAILING: f64 = 12.0;
    pub const HEADER_SIZE: f64 = 11.0;
    pub const HEADER_HEIGHT: f64 = 14.0;
    pub const HEADER_GAP: f64 = 6.0;
    pub const FOOTER_SIZE: f64 = 11.0;
    pub const FOOTER_HEIGHT: f64 = 14.0;
    pub const FOOTER_GAP: f64 = 6.0;
    pub const LABEL_SIZE: f64 = 13.0;
    pub const LABEL_HEIGHT: f64 = 16.0;
    /// A row's error text under its label (11 pt).
    pub const ERROR_SIZE: f64 = 11.0;
    pub const ERROR_HEIGHT: f64 = 14.0;
}

use metrics as m;

/// The rectangle the form's content takes in a column `column` wide at x = `x`: inside the root insets, at most
/// [`metrics::CONTENT_MAX_WIDTH`] wide and centred.
pub fn content_frame(x: f64, column: f64) -> (f64, f64) {
    let (_, leading, _, trailing) = m::ROOT_INSETS;
    let available = (column - leading - trailing).max(0.0);
    let w = available.min(m::CONTENT_MAX_WIDTH);
    (x + leading + (available - w) / 2.0, w)
}

/// One row of a section as the layout needs it.
#[derive(Clone, Debug, PartialEq)]
pub struct RowSpec<'a> {
    pub control: &'a Control,
    /// The row shows an error line under its label.
    pub error: bool,
}

/// Where one row's parts lie.
#[derive(Clone, Debug, PartialEq)]
pub struct RowLayout {
    pub row: Rect,
    pub label: Rect,
    pub control: Rect,
    /// The error line under the label, when there is one.
    pub error: Option<Rect>,
}

/// Where one section's parts lie.
#[derive(Clone, Debug, PartialEq)]
pub struct SectionLayout {
    pub header: Option<Rect>,
    pub frame: Rect,
    pub rows: Vec<RowLayout>,
    pub footer: Option<Rect>,
}

/// One section as the layout needs it.
#[derive(Clone, Debug, PartialEq)]
pub struct SectionSpec<'a> {
    pub header: bool,
    pub rows: Vec<RowSpec<'a>>,
    pub footer: bool,
}

/// The layout of `sections` in the column at x = `x`, `column` wide, starting at `top` (the toolbar's bottom); also the
/// height of the whole form including the root's bottom inset.
pub fn layout(
    sections: &[SectionSpec<'_>],
    x: f64,
    column: f64,
    top: f64,
) -> (Vec<SectionLayout>, f64) {
    let (cx, w) = content_frame(x, column);
    let mut y = top + m::ROOT_INSETS.0;
    let mut out = Vec::new();
    for (i, s) in sections.iter().enumerate() {
        if i > 0 {
            y += m::SECTION_SPACING;
        }
        let header = s.header.then(|| {
            let r = Rect::new(cx, y, w, m::HEADER_HEIGHT);
            y += m::HEADER_HEIGHT + m::HEADER_GAP;
            r
        });
        let box_top = y;
        let mut rows = Vec::new();
        for spec in &s.rows {
            let (cw, ch) = spec.control.size();
            let text = m::LABEL_HEIGHT + if spec.error { m::ERROR_HEIGHT } else { 0.0 };
            let h = (ch.max(text) + 2.0 * m::ROW_PADDING_V).max(m::ROW_MIN_HEIGHT);
            let row = Rect::new(cx, y, w, h);
            let control = Rect::new(
                row.right() - m::ROW_PADDING_H - cw,
                y + (h - ch) / 2.0,
                cw,
                ch,
            );
            let label_w = (control.x - cx - 2.0 * m::ROW_PADDING_H).max(0.0);
            let label_y = y + (h - text) / 2.0;
            let label = Rect::new(cx + m::ROW_PADDING_H, label_y, label_w, m::LABEL_HEIGHT);
            let error = spec
                .error
                .then(|| Rect::new(label.x, label.bottom(), label_w, m::ERROR_HEIGHT));
            rows.push(RowLayout {
                row,
                label,
                control,
                error,
            });
            y += h;
        }
        let frame = Rect::new(cx, box_top, w, y - box_top);
        let footer = s.footer.then(|| {
            let r = Rect::new(cx, y + m::FOOTER_GAP, w, m::FOOTER_HEIGHT);
            y += m::FOOTER_GAP + m::FOOTER_HEIGHT;
            r
        });
        out.push(SectionLayout {
            header,
            frame,
            rows,
            footer,
        });
    }
    (out, y + m::ROOT_INSETS.2 - top)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_form_sits_inside_the_root_insets_and_is_capped_at_600_centred() {
        assert_eq!(
            content_frame(216.0, 499.0),
            (236.0, 459.0),
            "W = 499: inset 20 on both sides, not capped"
        );
        assert_eq!(
            content_frame(0.0, 800.0),
            (100.0, 600.0),
            "W = 800: 600 wide and centred"
        );
    }

    #[test]
    fn rows_take_their_control_with_padding_and_sections_are_spaced() {
        let switch = Control::Switch { on: true };
        let slider = Control::Slider {
            value: 0.0,
            stops: Some(10),
            marks: &[],
            labels: ("Slow", "Fast"),
        };
        let sections = [
            SectionSpec {
                header: true,
                rows: vec![
                    RowSpec {
                        control: &switch,
                        error: false,
                    },
                    RowSpec {
                        control: &slider,
                        error: false,
                    },
                ],
                footer: false,
            },
            SectionSpec {
                header: false,
                rows: vec![RowSpec {
                    control: &switch,
                    error: true,
                }],
                footer: true,
            },
        ];
        let (l, height) = layout(&sections, 216.0, 499.0, 52.0);
        assert_eq!(l[0].header, Some(Rect::new(236.0, 72.0, 459.0, 14.0)));
        assert_eq!(
            l[0].frame.y, 92.0,
            "header 14 + gap 6 under the 20 pt top inset"
        );
        assert_eq!(
            l[0].rows[0].row,
            Rect::new(236.0, 92.0, 459.0, 38.0),
            "a switch row: 22 + 2 × 8"
        );
        assert_eq!(
            l[0].rows[0].control,
            Rect::new(236.0 + 459.0 - 10.0 - 38.0, 100.0, 38.0, 22.0),
            "right-aligned inside the padding"
        );
        assert_eq!(
            l[0].rows[1].row.h, 49.0,
            "a slider row: knob 20 + labels 13 + 2 × 8"
        );
        assert_eq!(l[1].frame.y, l[0].frame.bottom() + 11.0);
        assert_eq!(l[1].rows[0].row.h, 46.0, "label and error line 30 + 2 × 8");
        assert!(l[1].rows[0].error.is_some());
        let footer = l[1].footer.unwrap();
        assert_eq!(footer.y, l[1].frame.bottom() + 6.0);
        assert_eq!(
            height,
            footer.bottom() + 20.0 - 52.0,
            "the root's bottom inset ends the form"
        );
    }
}
