//! The selection rectangle: spanning it with a drag, moving it, resizing it by one of its eight handles, and the
//! dimension readout next to the pointer. Every rectangle is in global logical points and snapped to whole points, so
//! the captured image of a selection is exactly its size times the output's scale.

use cupertino_geometry::{Point, Rect};

use crate::values::{HANDLE_SIZE, LABEL_EDGE_INSET, LABEL_MIN_HEIGHT, LABEL_OFFSET};

/// One of the eight resize handles, by where it sits on the selection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Handle {
    TopLeft,
    Top,
    TopRight,
    Right,
    BottomRight,
    Bottom,
    BottomLeft,
    Left,
}

impl Handle {
    pub const ALL: [Handle; 8] = [
        Handle::TopLeft,
        Handle::Top,
        Handle::TopRight,
        Handle::Right,
        Handle::BottomRight,
        Handle::Bottom,
        Handle::BottomLeft,
        Handle::Left,
    ];

    /// The point of `rect` the handle is centred on.
    pub fn anchor(self, rect: &Rect) -> Point {
        let (l, t, r, b) = (rect.min_x(), rect.min_y(), rect.max_x(), rect.max_y());
        let (cx, cy) = (rect.center().x, rect.center().y);
        match self {
            Handle::TopLeft => Point::new(l, t),
            Handle::Top => Point::new(cx, t),
            Handle::TopRight => Point::new(r, t),
            Handle::Right => Point::new(r, cy),
            Handle::BottomRight => Point::new(r, b),
            Handle::Bottom => Point::new(cx, b),
            Handle::BottomLeft => Point::new(l, b),
            Handle::Left => Point::new(l, cy),
        }
    }

    /// The handle's square: [`HANDLE_SIZE`] at (-8, -8) from its anchor.
    pub fn rect(self, selection: &Rect) -> Rect {
        let a = self.anchor(selection);
        Rect::new(
            a.x - HANDLE_SIZE / 2.0,
            a.y - HANDLE_SIZE / 2.0,
            HANDLE_SIZE,
            HANDLE_SIZE,
        )
    }

    /// Which edges of the selection the handle moves: (left, top, right, bottom).
    fn edges(self) -> (bool, bool, bool, bool) {
        match self {
            Handle::TopLeft => (true, true, false, false),
            Handle::Top => (false, true, false, false),
            Handle::TopRight => (false, true, true, false),
            Handle::Right => (false, false, true, false),
            Handle::BottomRight => (false, false, true, true),
            Handle::Bottom => (false, false, false, true),
            Handle::BottomLeft => (true, false, false, true),
            Handle::Left => (true, false, false, false),
        }
    }
}

/// `p` limited to `bounds` and rounded to whole points.
fn snap(p: Point, bounds: &Rect) -> Point {
    Point::new(
        p.x.clamp(bounds.min_x(), bounds.max_x()).round(),
        p.y.clamp(bounds.min_y(), bounds.max_y()).round(),
    )
}

/// The rectangle a drag from `anchor` to `p` spans, inside `bounds` (the output the drag started on).
pub fn span(anchor: Point, p: Point, bounds: &Rect) -> Rect {
    let (a, b) = (snap(anchor, bounds), snap(p, bounds));
    Rect::from_ltrb(a.x.min(b.x), a.y.min(b.y), a.x.max(b.x), a.y.max(b.y))
}

/// `selection` moved by `(dx, dy)` and kept whole inside `bounds`.
pub fn moved(selection: &Rect, dx: f64, dy: f64, bounds: &Rect) -> Rect {
    let x = (selection.x() + dx)
        .round()
        .clamp(bounds.min_x(), bounds.max_x() - selection.width());
    let y = (selection.y() + dy)
        .round()
        .clamp(bounds.min_y(), bounds.max_y() - selection.height());
    Rect::new(x, y, selection.width(), selection.height())
}

/// `selection` with the edges of `handle` dragged to `p`; an edge dragged past the opposite one flips the rectangle,
/// as a fresh drag from the fixed corner would.
pub fn resized(selection: &Rect, handle: Handle, p: Point, bounds: &Rect) -> Rect {
    let p = snap(p, bounds);
    let (left, top, right, bottom) = handle.edges();
    let x0 = if left { p.x } else { selection.min_x() };
    let x1 = if right { p.x } else { selection.max_x() };
    let y0 = if top { p.y } else { selection.min_y() };
    let y1 = if bottom { p.y } else { selection.max_y() };
    Rect::from_ltrb(x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1))
}

/// The handle under `p`, the one drawn on top (last) winning where squares overlap on a small selection.
pub fn handle_at(selection: &Rect, p: Point) -> Option<Handle> {
    Handle::ALL
        .into_iter()
        .rev()
        .find(|h| h.rect(selection).contains(p))
}

/// Whether a selection can be captured: at least one whole point on both axes.
pub fn capturable(selection: &Rect) -> bool {
    selection.width() >= 1.0 && selection.height() >= 1.0
}

/// The readout of the selection's size in points, `"W × H" (approximate: the design's format string is unknown).
pub fn label_text(selection: &Rect) -> String {
    format!(
        "{} \u{d7} {}",
        selection.width().round() as i64,
        selection.height().round() as i64
    )
}

/// Where the readout of `text_width` points sits: [`LABEL_OFFSET`] right of and below the pointer, flipped to the
/// other side of the pointer where it would leave `bounds` (approximate placement). The backdrop is the text plus
/// [`LABEL_EDGE_INSET`] on both sides and [`LABEL_MIN_HEIGHT`] high.
pub fn label_rect(text_width: f64, pointer: Point, bounds: &Rect) -> Rect {
    let (w, h) = (text_width + 2.0 * LABEL_EDGE_INSET, LABEL_MIN_HEIGHT);
    let x = if pointer.x + LABEL_OFFSET + w <= bounds.max_x() {
        pointer.x + LABEL_OFFSET
    } else {
        pointer.x - LABEL_OFFSET - w
    };
    let y = if pointer.y + LABEL_OFFSET + h <= bounds.max_y() {
        pointer.y + LABEL_OFFSET
    } else {
        pointer.y - LABEL_OFFSET - h
    };
    Rect::new(x.round(), y.round(), w, h)
}
