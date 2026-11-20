//! Children in rows across the width: each row takes as many as fit, at their natural sizes, and the next row starts
//! below the tallest child of the one before.

use crate::layout::engine::{Arranged, ArrangedChildren, Engine};
use crate::layout::{AxisConstraint, Constraints, LayoutError, LayoutTree};
use bumpalo::collections::Vec as BumpVec;
use bumpalo::Bump;
use cupertino_geometry::{Rect, Size};

/// Where each of `sizes` goes in rows no wider than `width`, `gaps` apart, and the size the rows take. A full row spreads
/// what is left of `width` evenly between its children, and the last row keeps the spacing of the row above it, so the
/// children of equal widths line up in columns. A child wider than `width` has a row of its own.
pub(crate) fn place<'b>(
    sizes: &[Size],
    width: f64,
    gaps: Size,
    bump: &'b Bump,
) -> (BumpVec<'b, Rect>, Size) {
    // the children of each row
    let mut rows: BumpVec<'b, std::ops::Range<usize>> = BumpVec::new_in(bump);
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
    let mut rects = BumpVec::with_capacity_in(sizes.len(), bump);
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
    engine: &mut Engine<'_, '_, T>,
    kids: &[T::Id],
    gaps: Size,
    inner: Constraints,
) -> Result<Arranged<T::Id>, LayoutError> {
    let bump = engine.bump;
    let width = inner.width.max();
    let each = Constraints::new(
        AxisConstraint::loose(width)?,
        AxisConstraint::loose(inner.height.max())?,
    );
    let mut sizes = BumpVec::with_capacity_in(kids.len(), bump);
    for &id in kids {
        sizes.push(engine.solve(id, each)?.size);
    }
    let (rects, taken) = place(&sizes, width, gaps, bump);
    let mut children = ArrangedChildren::with_capacity(kids.len());
    for (&id, &rect) in kids.iter().zip(rects.iter()) {
        children.push((id, rect));
    }
    Ok(Arranged {
        size: Size::new(
            inner.width.clamp(taken.width),
            inner.height.clamp(taken.height),
        ),
        baseline: None,
        children,
    })
}
