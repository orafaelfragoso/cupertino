//! Children in rows across the width: each row takes as many as fit, at their natural sizes, and the next row starts
//! below the tallest child of the one before.

use crate::engine::{Arranged, Engine};
use crate::{AxisConstraint, Constraints, LayoutError, LayoutTree};
use cupertino_geometry::{Rect, Size};

/// Where each of `sizes` goes in rows no wider than `width`, `gaps` apart, and the size the rows take. A full row spreads
/// what is left of `width` evenly between its children, and the last row keeps the spacing of the row above it, so the
/// children of equal widths line up in columns. A child wider than `width` has a row of its own.
pub(crate) fn place(sizes: &[Size], width: f64, gaps: Size) -> (Vec<Rect>, Size) {
    // the children of each row
    let mut rows: Vec<std::ops::Range<usize>> = Vec::new();
    let (mut start, mut x) = (0, 0.0f64);
    for (i, size) in sizes.iter().enumerate() {
        if i > start && x + size.width > width + 1e-9 {
            rows.push(start..i);
            start = i;
            x = 0.0;
        }
        x += size.width + gaps.width;
    }
    if start < sizes.len() {
        rows.push(start..sizes.len());
    }
    let mut rects = Vec::with_capacity(sizes.len());
    let (mut y, mut widest) = (0.0f64, 0.0f64);
    let mut spacing = gaps.width;
    for (n, row) in rows.iter().enumerate() {
        let items = &sizes[row.clone()];
        let used: f64 = items.iter().map(|s| s.width).sum();
        let last = n + 1 == rows.len();
        if (!last || n == 0) && items.len() > 1 && width.is_finite() {
            let spread = (width - used) / (items.len() - 1) as f64;
            // a row alone keeps its gaps unless it fills the width
            spacing = if last {
                gaps.width
            } else {
                spread.max(gaps.width)
            };
        }
        let mut x = 0.0;
        let mut tallest = 0.0f64;
        for size in items {
            rects.push(Rect::new(x, y, size.width, size.height));
            widest = widest.max(x + size.width);
            x += size.width + spacing;
            tallest = tallest.max(size.height);
        }
        y += tallest + gaps.height;
    }
    let height = if rows.is_empty() {
        0.0
    } else {
        y - gaps.height
    };
    (rects, Size::new(widest, height))
}

pub(crate) fn arrange<T: LayoutTree>(
    engine: &mut Engine<'_, T>,
    kids: &[T::Id],
    gaps: Size,
    inner: Constraints,
) -> Result<Arranged<T::Id>, LayoutError> {
    let width = inner.width.max();
    let each = Constraints::new(
        AxisConstraint::loose(width)?,
        AxisConstraint::loose(inner.height.max())?,
    );
    let mut sizes = Vec::with_capacity(kids.len());
    for &id in kids {
        sizes.push(engine.solve(id, each)?.size);
    }
    let (rects, taken) = place(&sizes, width, gaps);
    Ok(Arranged {
        size: Size::new(
            inner.width.clamp(taken.width),
            inner.height.clamp(taken.height),
        ),
        baseline: None,
        children: kids.iter().copied().zip(rects).collect(),
    })
}
