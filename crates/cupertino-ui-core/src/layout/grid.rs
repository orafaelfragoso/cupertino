//! Grid containers: fixed, auto and fraction tracks, children flowing row by row, each covering one or more cells.

use crate::layout::engine::{align_offset, has_own_size, Arranged, ArrangedChildren, Engine};
use crate::layout::{Align, AxisConstraint, Constraints, GridSpan, LayoutError, LayoutTree, Track};
use bumpalo::collections::Vec as BumpVec;
use bumpalo::Bump;
use cupertino_geometry::{Rect, Size};

const PROBE_FACTOR: usize = 64;

/// Where a child landed: the first track it covers on each axis and how many it covers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Cell {
    pub column: usize,
    pub row: usize,
    pub columns: usize,
    pub rows: usize,
}

/// Auto-flow placement: each child takes the first free spot, scanning row by row from the previous child's spot. A span
/// wider than the grid is cut to its width; a span taller than `max_rows` is cut to it. The occupancy grid may not hold
/// more than `cell_limit` cells, and the cells probed while searching may not exceed [`PROBE_FACTOR`] times that.
pub(crate) fn place<'b>(
    spans: &[GridSpan],
    column_count: usize,
    max_rows: usize,
    cell_limit: usize,
    bump: &'b Bump,
) -> Result<BumpVec<'b, Cell>, LayoutError> {
    // Flat row-major occupancy grid in the scratch arena instead of `Vec<Vec<bool>>`.
    let mut taken = BumpVec::new_in(bump);
    let mut taken_rows = 0usize;
    let mut probed = 0usize;
    let probe_limit = cell_limit.saturating_mul(PROBE_FACTOR);
    let (mut row, mut column) = (0, 0);
    let mut cells = BumpVec::with_capacity_in(spans.len(), bump);
    for span in spans {
        let columns = span.columns.clamp(1, column_count);
        let rows = span.rows.clamp(1, max_rows.max(1));
        loop {
            if column + columns > column_count {
                row += 1;
                column = 0;
            }
            if taken_rows < row + rows {
                let needed_cells = (row + rows).saturating_mul(column_count);
                if needed_cells > cell_limit {
                    return Err(LayoutError::NodeLimit);
                }
                taken.resize(needed_cells, false);
                taken_rows = row + rows;
            }
            probed = probed.saturating_add(rows * columns);
            if probed > probe_limit {
                return Err(LayoutError::NodeLimit);
            }
            let free = (row..row + rows).all(|r| {
                let base = r * column_count;
                taken[base + column..base + column + columns]
                    .iter()
                    .all(|t| !t)
            });
            if free {
                break;
            }
            column += 1;
        }
        for r in row..row + rows {
            let base = r * column_count;
            taken[base + column..base + column + columns].fill(true);
        }
        cells.push(Cell {
            column,
            row,
            columns,
            rows,
        });
        column += columns;
    }
    Ok(cells)
}

pub(crate) fn arrange<T: LayoutTree>(
    engine: &mut Engine<'_, '_, T>,
    kids: &[T::Id],
    columns: &[Track],
    rows: &[Track],
    gaps: Size,
    inner: Constraints,
) -> Result<Arranged<T::Id>, LayoutError> {
    let bump = engine.bump;
    let column_count = columns.len().max(1);
    let mut spans = BumpVec::with_capacity_in(kids.len(), bump);
    for &id in kids {
        spans.push(engine.tree.spec(id).span);
    }
    let cells = place(
        &spans,
        column_count,
        rows.len().max(kids.len()),
        engine.node_limit,
        bump,
    )?;
    let row_count = rows
        .len()
        .max(cells.iter().map(|c| c.row + c.rows).max().unwrap_or(0));
    let mut column_tracks = BumpVec::with_capacity_in(column_count, bump);
    for i in 0..column_count {
        column_tracks.push(columns.get(i).copied().unwrap_or(Track::Auto));
    }
    let mut row_tracks = BumpVec::with_capacity_in(row_count, bump);
    for i in 0..row_count {
        row_tracks.push(rows.get(i).copied().unwrap_or(Track::Auto));
    }

    let loose = Constraints::new(
        AxisConstraint::loose(inner.width.max())?,
        AxisConstraint::loose(inner.height.max())?,
    );
    let mut natural_widths = BumpVec::with_capacity_in(column_count, bump);
    natural_widths.resize(column_count, 0.0f64);
    let mut wide = BumpVec::new_in(bump);
    for (&id, cell) in kids.iter().zip(cells.iter()) {
        let covered = &column_tracks[cell.column..cell.column + cell.columns];
        if covered.iter().all(|t| matches!(t, Track::Fixed(_))) {
            continue;
        }
        let width = engine.solve(id, loose)?.size.width;
        if cell.columns == 1 {
            natural_widths[cell.column] = natural_widths[cell.column].max(width);
        } else {
            wide.push((cell.column, cell.columns, width));
        }
    }
    spread_spans(&column_tracks, &mut natural_widths, &wide, gaps.width, bump);
    let widths = resolve(
        &column_tracks,
        &natural_widths,
        gaps.width,
        inner.width,
        bump,
    );
    let lefts = offsets(&widths, gaps.width, bump);

    let mut natural_heights = BumpVec::with_capacity_in(row_count, bump);
    natural_heights.resize(row_count, 0.0f64);
    let mut tall = BumpVec::new_in(bump);
    for (&id, cell) in kids.iter().zip(cells.iter()) {
        let covered = &row_tracks[cell.row..cell.row + cell.rows];
        if covered.iter().all(|t| matches!(t, Track::Fixed(_))) {
            continue;
        }
        let width = covered_extent(&widths, cell.column, cell.columns, gaps.width);
        let c = Constraints::new(
            AxisConstraint::tight(width)?,
            AxisConstraint::loose(inner.height.max())?,
        );
        let height = engine.solve(id, c)?.size.height;
        if cell.rows == 1 {
            natural_heights[cell.row] = natural_heights[cell.row].max(height);
        } else {
            tall.push((cell.row, cell.rows, height));
        }
    }
    spread_spans(&row_tracks, &mut natural_heights, &tall, gaps.height, bump);
    let heights = resolve(
        &row_tracks,
        &natural_heights,
        gaps.height,
        inner.height,
        bump,
    );
    let tops = offsets(&heights, gaps.height, bump);

    let size = Size::new(
        inner.width.clamp(extent(&widths, gaps.width)),
        inner.height.clamp(extent(&heights, gaps.height)),
    );

    let mut children = ArrangedChildren::with_capacity(kids.len());
    let mut baseline = None;
    for (&id, cell) in kids.iter().zip(cells.iter()) {
        let cell_w = covered_extent(&widths, cell.column, cell.columns, gaps.width);
        let cell_h = covered_extent(&heights, cell.row, cell.rows, gaps.height);
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
        let x = lefts[cell.column] + align_offset(align, cell_w, solved.size.width);
        let y = tops[cell.row] + align_offset(align, cell_h, solved.size.height);
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

/// A child covering several tracks that needs more than they give adds the shortfall to its auto tracks equally, or
/// to its fraction tracks when it covers none. Narrower spans settle first so wider ones only add what is still missing.
fn spread_spans(
    tracks: &[Track],
    natural: &mut [f64],
    spans: &[(usize, usize, f64)],
    gap: f64,
    bump: &Bump,
) {
    if spans.is_empty() {
        return;
    }
    let mut ordered = BumpVec::with_capacity_in(spans.len(), bump);
    ordered.extend_from_slice(spans);
    ordered.sort_by_key(|(_, length, _)| *length);
    let mut targets = BumpVec::new_in(bump);
    for &(start, length, needed) in &ordered {
        let range = start..start + length;
        let have: f64 = range
            .clone()
            .map(|i| match tracks[i] {
                Track::Fixed(v) => v,
                _ => natural[i],
            })
            .sum::<f64>()
            + gap * (length - 1) as f64;
        let missing = needed - have;
        if missing <= 0.0 {
            continue;
        }
        targets.clear();
        for i in range.clone() {
            if matches!(tracks[i], Track::Auto) {
                targets.push(i);
            }
        }
        if targets.is_empty() {
            for i in range {
                if matches!(tracks[i], Track::Fraction(_)) {
                    targets.push(i);
                }
            }
        }
        for &i in &targets {
            natural[i] += missing / targets.len() as f64;
        }
    }
}

/// Track sizes along one axis. Fraction tracks share what fixed and auto tracks leave when the axis has a definite size;
/// otherwise they are as large as their biggest content needs, in proportion.
fn resolve<'b>(
    tracks: &[Track],
    natural: &[f64],
    gap: f64,
    available: AxisConstraint,
    bump: &'b Bump,
) -> BumpVec<'b, f64> {
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
    let mut out = BumpVec::with_capacity_in(tracks.len(), bump);
    for (track, natural) in tracks.iter().zip(natural) {
        out.push(match track {
            Track::Fixed(v) => *v,
            Track::Auto => *natural,
            Track::Fraction(f) => unit * f,
        });
    }
    out
}

fn covered_extent(sizes: &[f64], start: usize, length: usize, gap: f64) -> f64 {
    extent(&sizes[start..start + length], gap)
}

fn extent(sizes: &[f64], gap: f64) -> f64 {
    sizes.iter().sum::<f64>() + gap * sizes.len().saturating_sub(1) as f64
}

fn offsets<'b>(sizes: &[f64], gap: f64, bump: &'b Bump) -> BumpVec<'b, f64> {
    let mut out = BumpVec::with_capacity_in(sizes.len(), bump);
    let mut at = 0.0;
    for &size in sizes {
        out.push(at);
        at += size + gap;
    }
    out
}
