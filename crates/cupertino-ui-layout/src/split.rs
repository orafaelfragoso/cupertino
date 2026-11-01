//! Two panes along an axis: the leading one at its asked extent within its limits, the trailing one taking the rest.

use crate::engine::{Arranged, Engine};
use crate::{AxisConstraint, Constraints, LayoutError, LayoutTree, SplitSpec};

pub(crate) fn arrange<T: LayoutTree>(
    engine: &mut Engine<'_, T>,
    kids: &[T::Id],
    split: &SplitSpec,
    inner: Constraints,
) -> Result<Arranged<T::Id>, LayoutError> {
    let axis = split.axis;
    let (main, cross) = axis.split(inner);
    let room = |c: AxisConstraint| {
        if c.max().is_finite() {
            c.max()
        } else {
            c.min()
        }
    };
    let (extent, across) = (room(main), room(cross));
    let (leading, gap) = split
        .shown(extent)
        .map_or((0.0, 0.0), |leading| (leading, split.gap));
    let trailing = (extent - leading - gap).max(0.0);
    let mut children = Vec::with_capacity(kids.len());
    let mut position = 0.0;
    for (&id, length) in kids.iter().zip([leading, trailing]) {
        let c = axis.join(
            AxisConstraint::tight(length)?,
            AxisConstraint::tight(across)?,
        );
        let solved = engine.solve(id, c)?;
        children.push((id, axis.rect(position, 0.0, solved.size)));
        position += length + gap;
    }
    Ok(Arranged {
        size: axis.pack(extent, across),
        baseline: None,
        children,
    })
}
