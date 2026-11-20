//! Horizontal and vertical stacks: flexible main-axis sharing, cross-axis alignment and baseline alignment.

use crate::layout::engine::{
    align_offset, has_own_size, is_fixed, Arranged, ArrangedChildren, Engine, Solved,
};
use crate::layout::{
    Align, Axis, AxisConstraint, Constraints, Justify, LayoutError, LayoutTree, SizeSpec,
};
use smallvec::SmallVec;

const EPSILON: f64 = 1e-9;

struct Item<Id> {
    id: Id,
    basis: f64,
    lower: f64,
    upper: f64,
    grow: f64,
    shrink: f64,
    align: Align,
    cross_fixed: bool,
    natural: Option<Solved>,
}

pub(crate) fn arrange<T: LayoutTree>(
    engine: &mut Engine<'_, '_, T>,
    kids: &[T::Id],
    axis: Axis,
    spacing: f64,
    align: Align,
    justify: Justify,
    inner: Constraints,
) -> Result<Arranged<T::Id>, LayoutError> {
    let (main_c, cross_c) = axis.split(inner);
    let natural_c = axis.join(
        AxisConstraint::loose(f64::INFINITY)?,
        AxisConstraint::loose(cross_c.max())?,
    );

    let mut items: SmallVec<[Item<T::Id>; 16]> = SmallVec::with_capacity(kids.len());
    for &id in kids {
        let spec = engine.tree.spec(id);
        let main_spec = axis.size_spec(spec);
        let mut natural = None;
        let basis = match (main_spec, spec.flex.basis) {
            (SizeSpec::Fixed(v), _) => v,
            (_, Some(b)) => b,
            (SizeSpec::Fraction(f), None) if main_c.max().is_finite() => f * main_c.max(),
            _ => {
                let solved = engine.solve(id, natural_c)?;
                let size = axis.main(solved.size);
                natural = Some(solved);
                size
            }
        };
        let pinned = is_fixed(main_spec);
        items.push(Item {
            id,
            basis,
            lower: if pinned { basis } else { 0.0 },
            upper: if pinned { basis } else { f64::INFINITY },
            grow: spec.flex.grow,
            shrink: spec.flex.shrink,
            align: spec.align_self.unwrap_or(align),
            cross_fixed: has_own_size(axis.cross_size_spec(spec)),
            natural,
        });
    }

    let gaps = spacing * kids.len().saturating_sub(1) as f64;
    let total = items.iter().map(|i| i.basis).sum::<f64>() + gaps;
    let main = main_c.clamp(total);
    if total > main {
        for item in items
            .iter_mut()
            .filter(|i| i.shrink > 0.0 && i.lower < i.upper)
        {
            let floor = engine.min_content(item.id, axis, cross_c.max())?;
            item.lower = floor.min(item.basis);
        }
    }
    let (sizes, passes) = distribute(&items, main - gaps);
    engine.stats.flex_passes += passes;

    let mut solved: SmallVec<[Solved; 16]> = SmallVec::with_capacity(items.len());
    for (item, &main_size) in items.iter().zip(sizes.iter()) {
        let reusable = item.natural.filter(|n| axis.main(n.size) == main_size);
        solved.push(match reusable {
            Some(n) => n,
            None => {
                let c = axis.join(
                    AxisConstraint::tight(main_size)?,
                    AxisConstraint::loose(cross_c.max())?,
                );
                engine.solve(item.id, c)?
            }
        });
    }

    let baseline_aligned = axis == Axis::Horizontal;
    let shared_baseline = items
        .iter()
        .zip(solved.iter())
        .filter(|(i, _)| baseline_aligned && i.align == Align::Baseline)
        .filter_map(|(_, s)| s.baseline)
        .fold(None, |acc: Option<f64>, b| {
            Some(acc.map_or(b, |a| a.max(b)))
        });
    let mut offsets: SmallVec<[f64; 16]> = SmallVec::with_capacity(items.len());
    let mut natural_cross = 0.0f64;
    for (i, s) in items.iter().zip(solved.iter()) {
        let off = match (shared_baseline, s.baseline) {
            (Some(shared), Some(b)) if baseline_aligned && i.align == Align::Baseline => shared - b,
            _ => 0.0,
        };
        offsets.push(off);
        natural_cross = natural_cross.max(axis.cross(s.size) + off);
    }
    let cross = cross_c.clamp(natural_cross);

    let mut cross_sizes: SmallVec<[f64; 16]> = SmallVec::with_capacity(items.len());
    for ((item, s), &main_size) in items.iter().zip(solved.iter_mut()).zip(sizes.iter()) {
        if item.align == Align::Stretch && !item.cross_fixed && axis.cross(s.size) != cross {
            let c = axis.join(
                AxisConstraint::tight(main_size)?,
                AxisConstraint::tight(cross)?,
            );
            *s = engine.solve(item.id, c)?;
        }
        cross_sizes.push(axis.cross(s.size));
    }

    let used = sizes.iter().sum::<f64>() + gaps;
    let leftover = (main - used).max(0.0);
    let (mut position, extra) = match justify {
        Justify::Start => (0.0, 0.0),
        Justify::Center => (leftover / 2.0, 0.0),
        Justify::End => (leftover, 0.0),
        Justify::SpaceBetween if kids.len() > 1 => (0.0, leftover / (kids.len() - 1) as f64),
        Justify::SpaceBetween => (0.0, 0.0),
    };
    let mut children = ArrangedChildren::with_capacity(items.len());
    let mut baseline = None;
    for (index, item) in items.iter().enumerate() {
        let s = &solved[index];
        let cross_pos = match item.align {
            Align::Baseline => offsets[index],
            other => align_offset(other, cross, cross_sizes[index]),
        };
        if baseline.is_none() {
            baseline = s.baseline.map(|b| {
                b + if axis == Axis::Horizontal {
                    cross_pos
                } else {
                    position
                }
            });
        }
        children.push((item.id, axis.rect(position, cross_pos, s.size)));
        position += axis.main(s.size) + spacing + extra;
    }
    Ok(Arranged {
        size: axis.pack(main, cross),
        baseline,
        children,
    })
}

/// Shares `room` among the items: leftover space by `grow`, a shortfall by `shrink` times basis, each item held within
/// its bounds (`lower <= basis <= upper`, so growing meets only upper bounds and shrinking only lower ones). The share
/// per unit of weight is solved in one pass: with the items ordered by the share at which each reaches its bound, an
/// item whose bound comes before the share of those still free sits at its bound, and the rest take that share.
/// Returns the sizes and the number of passes, 0 when there is nothing to share.
fn distribute<Id>(items: &[Item<Id>], room: f64) -> (SmallVec<[f64; 16]>, usize) {
    let mut sizes: SmallVec<[f64; 16]> = SmallVec::with_capacity(items.len());
    let mut sum = 0.0;
    for item in items {
        sizes.push(item.basis);
        sum += item.basis;
    }
    let free = room - sum;
    let growing = free > 0.0;
    let weight = |i: &Item<Id>| if growing { i.grow } else { i.shrink * i.basis };
    let slack = |i: &Item<Id>| {
        if growing {
            i.upper - i.basis
        } else {
            i.basis - i.lower
        }
    };
    let mut flexible: SmallVec<[(f64, usize); 16]> = SmallVec::new();
    for (index, i) in items.iter().enumerate() {
        let w = weight(i);
        if w > 0.0 && i.lower != i.upper {
            flexible.push((slack(i) / w, index));
        }
    }
    if flexible.is_empty() || free.abs() < EPSILON {
        return (sizes, 0);
    }
    flexible.sort_by(|a, b| a.0.total_cmp(&b.0));
    // the weight of the items from each position on, summed from the end so a small weight is not lost in a large one
    let mut weights: SmallVec<[f64; 17]> = smallvec::smallvec![0.0; flexible.len() + 1];
    for (k, &(_, index)) in flexible.iter().enumerate().rev() {
        weights[k] = weights[k + 1] + weight(&items[index]);
    }
    let mut left = free.abs();
    let mut bounded = 0;
    for &(reach, index) in &flexible {
        if reach >= left / weights[bounded] {
            break;
        }
        let item = &items[index];
        left -= slack(item);
        sizes[index] = if growing { item.upper } else { item.lower };
        bounded += 1;
    }
    if bounded < flexible.len() {
        let share = free.signum() * left / weights[bounded];
        for &(_, index) in &flexible[bounded..] {
            let item = &items[index];
            sizes[index] = (item.basis + share * weight(item)).clamp(item.lower, item.upper);
        }
    }
    (sizes, 1)
}
