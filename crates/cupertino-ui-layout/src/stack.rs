//! Horizontal and vertical stacks: flexible main-axis sharing, cross-axis alignment and baseline alignment.

use crate::engine::{align_offset, has_own_size, is_fixed, Arranged, Engine, Solved};
use crate::{Align, Axis, AxisConstraint, Constraints, Justify, LayoutError, LayoutTree, SizeSpec};
use std::rc::Rc;

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
    natural: Option<Rc<Solved<Id>>>,
}

pub(crate) fn arrange<T: LayoutTree>(
    engine: &mut Engine<'_, T>,
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

    let mut items = Vec::with_capacity(kids.len());
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

    let mut solved = Vec::with_capacity(items.len());
    for (item, main_size) in items.iter().zip(&sizes) {
        let reusable = item
            .natural
            .as_ref()
            .filter(|n| axis.main(n.size) == *main_size);
        solved.push(match reusable {
            Some(n) => Rc::clone(n),
            None => {
                let c = axis.join(
                    AxisConstraint::tight(*main_size)?,
                    AxisConstraint::loose(cross_c.max())?,
                );
                engine.solve(item.id, c)?
            }
        });
    }

    let baseline_aligned = axis == Axis::Horizontal;
    let shared_baseline = items
        .iter()
        .zip(&solved)
        .filter(|(i, _)| baseline_aligned && i.align == Align::Baseline)
        .filter_map(|(_, s)| s.baseline)
        .fold(None, |acc: Option<f64>, b| {
            Some(acc.map_or(b, |a| a.max(b)))
        });
    let offsets: Vec<f64> = items
        .iter()
        .zip(&solved)
        .map(|(i, s)| match (shared_baseline, s.baseline) {
            (Some(shared), Some(b)) if baseline_aligned && i.align == Align::Baseline => shared - b,
            _ => 0.0,
        })
        .collect();
    let natural_cross = solved
        .iter()
        .zip(&offsets)
        .map(|(s, o)| axis.cross(s.size) + o)
        .fold(0.0, f64::max);
    let cross = cross_c.clamp(natural_cross);

    let mut cross_sizes = Vec::with_capacity(items.len());
    for ((item, s), main_size) in items.iter().zip(&mut solved).zip(&sizes) {
        if item.align == Align::Stretch && !item.cross_fixed && axis.cross(s.size) != cross {
            let c = axis.join(
                AxisConstraint::tight(*main_size)?,
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
    let mut children = Vec::with_capacity(items.len());
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
fn distribute<Id>(items: &[Item<Id>], room: f64) -> (Vec<f64>, usize) {
    let mut sizes: Vec<f64> = items.iter().map(|i| i.basis).collect();
    let free = room - sizes.iter().sum::<f64>();
    let growing = free > 0.0;
    let weight = |i: &Item<Id>| if growing { i.grow } else { i.shrink * i.basis };
    let slack = |i: &Item<Id>| {
        if growing {
            i.upper - i.basis
        } else {
            i.basis - i.lower
        }
    };
    let mut flexible: Vec<(f64, usize)> = items
        .iter()
        .enumerate()
        .filter(|(_, i)| weight(i) > 0.0 && i.lower != i.upper)
        .map(|(index, i)| (slack(i) / weight(i), index))
        .collect();
    if flexible.is_empty() || free.abs() < EPSILON {
        return (sizes, 0);
    }
    flexible.sort_by(|a, b| a.0.total_cmp(&b.0));
    // the weight of the items from each position on, summed from the end so a small weight is not lost in a large one
    let mut weights = vec![0.0; flexible.len() + 1];
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

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn item(basis: f64, lower: f64, upper: f64, grow: f64, shrink: f64) -> Item<()> {
        Item {
            id: (),
            basis,
            lower,
            upper,
            grow,
            shrink,
            align: Align::Start,
            cross_fixed: false,
            natural: None,
        }
    }

    /// Freeze-and-share: share the room, freeze every item that leaves its bounds at the bound, share again. Returns the
    /// sizes and the passes it took.
    fn iterated(items: &[Item<()>], room: f64) -> (Vec<f64>, usize) {
        let mut sizes: Vec<f64> = items.iter().map(|i| i.basis).collect();
        let growing = room - sizes.iter().sum::<f64>() > 0.0;
        let weight = |i: &Item<()>| if growing { i.grow } else { i.shrink * i.basis };
        let mut frozen: Vec<bool> = items
            .iter()
            .map(|i| weight(i) <= 0.0 || i.lower == i.upper)
            .collect();
        for pass in 0.. {
            let free = room - sizes.iter().sum::<f64>();
            let weights: f64 = items
                .iter()
                .zip(&frozen)
                .filter(|(_, f)| !**f)
                .map(|(i, _)| weight(i))
                .sum();
            if weights <= 0.0 || free.abs() < EPSILON || (growing != (free > 0.0)) {
                return (sizes, pass);
            }
            let mut violated = false;
            for (index, item) in items.iter().enumerate() {
                if frozen[index] {
                    continue;
                }
                let wanted = sizes[index] + free * weight(item) / weights;
                let bounded = wanted.max(item.lower).min(item.upper);
                if bounded != wanted {
                    violated = true;
                    frozen[index] = true;
                }
                sizes[index] = bounded;
            }
            if !violated {
                return (sizes, pass + 1);
            }
        }
        unreachable!()
    }

    fn close(a: &[f64], b: &[f64]) -> bool {
        a.iter()
            .zip(b)
            .all(|(x, y)| (x - y).abs() <= 1e-6 * x.abs().max(y.abs()).max(1.0))
    }

    /// Each child's weight is 0.6 of what is left and its slack 0.7 of the share per weight, so freezing and sharing again
    /// frees one child per pass.
    fn one_bound_per_pass(count: usize, shortfall: f64) -> Vec<Item<()>> {
        let mut weights = Vec::with_capacity(count);
        let mut left = 1.0;
        for k in 0..count {
            let w = if k + 1 < count { 0.6 * left } else { left };
            weights.push(w);
            left -= w;
        }
        let (mut room, mut total) = (shortfall, 1.0);
        let mut slacks = Vec::with_capacity(count);
        for w in &weights {
            let slack = 0.7 * room / total * w;
            slacks.push(slack);
            room -= slack;
            total -= w;
        }
        let basis = 2.0 * slacks.iter().cloned().fold(0.0, f64::max);
        weights
            .iter()
            .zip(&slacks)
            .map(|(w, s)| item(basis, basis - s, basis, 0.0, *w))
            .collect()
    }

    #[test]
    fn a_shortfall_that_bounds_one_child_per_pass_is_shared_in_one() {
        let items = one_bound_per_pass(60, 1e5);
        let room = items.iter().map(|i| i.basis).sum::<f64>() - 1e5;
        let (reference, passes) = iterated(&items, room);
        assert!(passes > 32, "freeze-and-share takes {passes} passes");
        let (sizes, solved) = distribute(&items, room);
        assert_eq!(solved, 1);
        assert!(close(&sizes, &reference));
        assert!(items.iter().zip(&sizes).all(|(i, s)| *s == i.lower));
    }

    /// Basis, lower, upper, grow and shrink of each item, with `lower <= basis <= upper` as the stack makes them.
    fn specs() -> impl Strategy<Value = Vec<(f64, f64, f64, f64, f64)>> {
        prop::collection::vec(
            (
                0.0..500.0f64,
                0.0..1.0f64,
                prop::option::of(0.0..300.0f64),
                0.0..3.0f64,
                0.0..3.0f64,
                prop::bool::weighted(0.1),
            ),
            1..12,
        )
        .prop_map(|v| {
            v.into_iter()
                .map(|(basis, floor, extra, grow, shrink, pinned)| {
                    if pinned {
                        (basis, basis, basis, grow, shrink)
                    } else {
                        let upper = extra.map_or(f64::INFINITY, |e| basis + e);
                        (basis, basis * floor, upper, grow, shrink)
                    }
                })
                .collect()
        })
    }

    proptest! {
        #[test]
        fn the_one_pass_share_is_the_freeze_and_share_fixpoint(specs in specs(), room in 0.0..4000.0f64) {
            let items: Vec<Item<()>> = specs.iter().map(|&(b, l, u, g, s)| item(b, l, u, g, s)).collect();
            let (sizes, passes) = distribute(&items, room);
            let (reference, _) = iterated(&items, room);
            prop_assert!(passes <= 1);
            prop_assert!(close(&sizes, &reference), "{sizes:?} != {reference:?}");
            for (i, s) in items.iter().zip(&sizes) {
                prop_assert!(*s >= i.lower && *s <= i.upper);
            }
        }
    }
}
