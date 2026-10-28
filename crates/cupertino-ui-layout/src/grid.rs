//! Grid containers: fixed, auto and fraction tracks, children flowing row by row, one per cell.

use crate::engine::{align_offset, has_own_size, Arranged, Engine};
use crate::{Align, AxisConstraint, Constraints, LayoutError, LayoutTree, Track};
use cupertino_geometry::{Rect, Size};

pub(crate) fn arrange<T: LayoutTree>(
    engine: &mut Engine<'_, T>,
    kids: &[T::Id],
    columns: &[Track],
    rows: &[Track],
    gaps: Size,
    inner: Constraints,
) -> Result<Arranged<T::Id>, LayoutError> {
    let column_count = columns.len().max(1);
    let row_count = rows.len().max(kids.len().div_ceil(column_count));
    let column_track = |i: usize| columns.get(i).copied().unwrap_or(Track::Auto);
    let row_track = |i: usize| rows.get(i).copied().unwrap_or(Track::Auto);

    let mut natural_widths = vec![0.0f64; column_count];
    let loose = Constraints::new(
        AxisConstraint::loose(inner.width.max())?,
        AxisConstraint::loose(inner.height.max())?,
    );
    for (index, &id) in kids.iter().enumerate() {
        let column = index % column_count;
        if !matches!(column_track(column), Track::Fixed(_)) {
            let width = engine.solve(id, loose)?.size.width;
            natural_widths[column] = natural_widths[column].max(width);
        }
    }
    let widths = resolve(
        (0..column_count).map(column_track),
        &natural_widths,
        gaps.width,
        inner.width,
    );

    let mut natural_heights = vec![0.0f64; row_count];
    for (index, &id) in kids.iter().enumerate() {
        let row = index / column_count;
        if !matches!(row_track(row), Track::Fixed(_)) {
            let c = Constraints::new(
                AxisConstraint::tight(widths[index % column_count])?,
                AxisConstraint::loose(inner.height.max())?,
            );
            let height = engine.solve(id, c)?.size.height;
            natural_heights[row] = natural_heights[row].max(height);
        }
    }
    let heights = resolve(
        (0..row_count).map(row_track),
        &natural_heights,
        gaps.height,
        inner.height,
    );

    let lefts = offsets(&widths, gaps.width);
    let tops = offsets(&heights, gaps.height);
    let size = Size::new(
        inner.width.clamp(extent(&widths, gaps.width)),
        inner.height.clamp(extent(&heights, gaps.height)),
    );

    let mut children = Vec::with_capacity(kids.len());
    let mut baseline = None;
    for (index, &id) in kids.iter().enumerate() {
        let (column, row) = (index % column_count, index / column_count);
        let (cell_w, cell_h) = (widths[column], heights[row]);
        let spec = engine.tree.spec(id);
        let align = spec.align_self.unwrap_or(Align::Stretch);
        let stretch = align == Align::Stretch;
        let c = Constraints::new(
            if stretch && !has_own_size(spec.width) {
                AxisConstraint::tight(cell_w)?
            } else {
                AxisConstraint::loose(cell_w)?
            },
            if stretch && !has_own_size(spec.height) {
                AxisConstraint::tight(cell_h)?
            } else {
                AxisConstraint::loose(cell_h)?
            },
        );
        let solved = engine.solve(id, c)?;
        let x = lefts[column] + align_offset(align, cell_w, solved.size.width);
        let y = tops[row] + align_offset(align, cell_h, solved.size.height);
        if baseline.is_none() {
            baseline = solved.baseline.map(|b| b + y);
        }
        children.push((id, Rect::new(x, y, solved.size.width, solved.size.height)));
    }
    Ok(Arranged {
        size,
        baseline,
        children,
    })
}

/// Track sizes along one axis. Fraction tracks share what fixed and auto tracks leave when the axis has a definite size;
/// otherwise they are as large as their biggest content needs, in proportion.
fn resolve(
    tracks: impl Iterator<Item = Track>,
    natural: &[f64],
    gap: f64,
    available: AxisConstraint,
) -> Vec<f64> {
    let tracks: Vec<Track> = tracks.collect();
    let gaps = gap * tracks.len().saturating_sub(1) as f64;
    let mut fixed_and_auto = 0.0;
    let mut fractions = 0.0;
    let mut content_unit = 0.0f64;
    for (track, natural) in tracks.iter().zip(natural) {
        match track {
            Track::Fixed(v) => fixed_and_auto += v,
            Track::Auto => fixed_and_auto += natural,
            Track::Fraction(f) if *f > 0.0 => {
                fractions += f;
                content_unit = content_unit.max(natural / f);
            }
            Track::Fraction(_) => {}
        }
    }
    let unit = if fractions > 0.0 && available.min() == available.max() {
        ((available.max() - fixed_and_auto - gaps) / fractions).max(0.0)
    } else {
        content_unit
    };
    tracks
        .iter()
        .zip(natural)
        .map(|(track, natural)| match track {
            Track::Fixed(v) => *v,
            Track::Auto => *natural,
            Track::Fraction(f) => unit * f,
        })
        .collect()
}

fn extent(sizes: &[f64], gap: f64) -> f64 {
    sizes.iter().sum::<f64>() + gap * sizes.len().saturating_sub(1) as f64
}

fn offsets(sizes: &[f64], gap: f64) -> Vec<f64> {
    let mut at = 0.0;
    sizes
        .iter()
        .map(|size| {
            let start = at;
            at += size + gap;
            start
        })
        .collect()
}
