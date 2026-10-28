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
    let sizes = distribute(&items, main - gaps)?;

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

/// Shares `room` among the items: leftover space by `grow`, a shortfall by `shrink` times basis. An item that would
/// leave its bounds is frozen at the bound and the rest is shared again, until nothing violates. Every pass but the
/// last freezes at least one item, so `items.len() + 1` passes always settle; the error guards that invariant.
fn distribute<Id>(items: &[Item<Id>], room: f64) -> Result<Vec<f64>, LayoutError> {
    let mut sizes: Vec<f64> = items.iter().map(|i| i.basis).collect();
    let growing = room - sizes.iter().sum::<f64>() > 0.0;
    let weight = |i: &Item<Id>| if growing { i.grow } else { i.shrink * i.basis };
    let mut frozen: Vec<bool> = items
        .iter()
        .map(|i| weight(i) <= 0.0 || i.lower == i.upper)
        .collect();
    for _ in 0..=items.len() {
        let used: f64 = sizes.iter().sum();
        let free = room - used;
        let weights: f64 = items
            .iter()
            .zip(&frozen)
            .filter(|(_, f)| !**f)
            .map(|(i, _)| weight(i))
            .sum();
        if weights <= 0.0
            || free.abs() < EPSILON
            || (growing && free < 0.0)
            || (!growing && free > 0.0)
        {
            return Ok(sizes);
        }
        let mut violated = false;
        let mut next = sizes.clone();
        for (index, item) in items.iter().enumerate() {
            if frozen[index] {
                continue;
            }
            let wanted = sizes[index] + free * weight(item) / weights;
            let bounded = wanted.max(item.lower).min(item.upper);
            violated |= bounded != wanted;
            next[index] = bounded;
            if bounded != wanted {
                frozen[index] = true;
            }
        }
        sizes = next;
        if !violated {
            return Ok(sizes);
        }
    }
    Err(LayoutError::NoConvergence)
}
