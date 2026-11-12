//! The edge of a piece of glass the toolkit paints itself: a fine dark outline along the sides, strongest at the ends of
//! a capsule and gone along the top and bottom, and a light rim along the top and bottom that fades towards the sides.
//! The compositor draws the same edge for the glass of a menu.

use cupertino_geometry::{Insets, Point, Rect};
use cupertino_ui_core::PaintCx;
use cupertino_ui_render::{Color, GradientStop, Paint, PaintOp, Shape};

/// How strong the parts of a glass edge are.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlassEdge {
    /// Opacity of the black outline along the sides.
    pub outline: f32,
    /// Width of the outline in points.
    pub outline_width: f64,
    /// Opacity of the white rim along the top.
    pub rim: f32,
    /// The bottom rim's opacity as a share of the top's.
    pub rim_bottom: f32,
    /// Opacity of the rim down the sides.
    pub rim_side: f32,
    /// Width of the rim in points.
    pub rim_width: f64,
}

/// Paint the edge of `edge` just inside the outline of `rect`, whose corners are rounded by `radius`.
pub fn paint_glass_edge(cx: &mut PaintCx<'_>, rect: Rect, radius: f64, edge: GlassEdge) {
    let height = rect.height().max(1.0);
    let white = |offset: f32, a: f32| GradientStop {
        offset,
        color: Color::new(1.0, 1.0, 1.0, a),
    };
    let black = |offset: f32, a: f32| GradientStop {
        offset,
        color: Color::new(0.0, 0.0, 0.0, a),
    };
    if edge.rim > 0.0 && edge.rim_width > 0.0 {
        let band = (radius / height).clamp(0.0, 0.5) as f32;
        cx.push(PaintOp::Stroke {
            shape: Shape::RoundedRect {
                rect: rect.inset(Insets::uniform(edge.rim_width / 2.0)),
                radius: (radius - edge.rim_width / 2.0).max(0.0),
            },
            width: edge.rim_width,
            paint: Paint::Linear {
                start: Point::new(rect.min_x(), rect.min_y()),
                end: Point::new(rect.min_x(), rect.max_y()),
                stops: vec![
                    white(0.0, edge.rim),
                    white(band, edge.rim_side),
                    white(1.0 - band, edge.rim_side),
                    white(1.0, edge.rim * edge.rim_bottom),
                ],
            },
        });
    }
    if edge.outline > 0.0 && edge.outline_width > 0.0 {
        let ends = (radius / rect.width().max(1.0)).clamp(0.0, 0.5) as f32;
        cx.push(PaintOp::Stroke {
            shape: Shape::RoundedRect {
                rect: rect.inset(Insets::uniform(edge.outline_width / 2.0)),
                radius: (radius - edge.outline_width / 2.0).max(0.0),
            },
            width: edge.outline_width,
            paint: Paint::Linear {
                start: Point::new(rect.min_x(), rect.min_y()),
                end: Point::new(rect.max_x(), rect.min_y()),
                stops: vec![
                    black(0.0, edge.outline),
                    black(ends, 0.0),
                    black(1.0 - ends, 0.0),
                    black(1.0, edge.outline),
                ],
            },
        });
    }
}
