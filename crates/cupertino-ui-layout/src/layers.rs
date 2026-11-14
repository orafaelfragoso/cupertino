//! Containers whose children share one box: absolute, overlay and scroll content.

use crate::engine::{align_offset, has_own_size, Arranged, Engine};
use crate::{Align, AxisConstraint, Constraints, LayoutError, LayoutTree, SizeSpec};
use cupertino_geometry::{Point, Rect, Size};

#[derive(Clone, Copy)]
pub(crate) enum Mode {
    /// Children keep their natural size at their `position`; in a container that `crops` them ([`Overflow::Crop`]),
    /// a size of their own even past its edge, where it cuts them off.
    ///
    /// [`Overflow::Crop`]: crate::Overflow::Crop
    Absolute { crops: bool },
    /// Children are layered; each follows its `align_self`, stretching by default.
    Overlay,
    /// Content is as large as it wants along the scrolled axes and stretches across the others.
    Scroll { horizontal: bool, vertical: bool },
}

pub(crate) fn arrange<T: LayoutTree>(
    engine: &mut Engine<'_, T>,
    kids: &[T::Id],
    mode: Mode,
    inner: Constraints,
) -> Result<Arranged<T::Id>, LayoutError> {
    let (scroll_x, scroll_y) = match mode {
        Mode::Scroll {
            horizontal,
            vertical,
        } => (horizontal, vertical),
        _ => (false, false),
    };
    // content may run past the viewport on a scrolled axis; the viewport is still the size it would ideally fill
    let reach = |max: f64, scrolled: bool| {
        if scrolled {
            AxisConstraint::new(0.0, if max.is_finite() { max } else { 0.0 }, f64::INFINITY)
        } else {
            AxisConstraint::loose(max)
        }
    };
    let natural_c = Constraints::new(
        reach(inner.width.max(), scroll_x)?,
        reach(inner.height.max(), scroll_y)?,
    );
    let mut natural = Vec::with_capacity(kids.len());
    for &id in kids {
        // an absolute child of a size of its own keeps it in a container that crops it, past the container's edge if
        // need be, as a picture cut off by a frame rather than laid out again smaller
        let c = match (mode, engine.tree.spec(id)) {
            (Mode::Absolute { crops: true }, spec) => {
                let own = |size: SizeSpec, axis: AxisConstraint| match size {
                    SizeSpec::Fixed(v) if v > axis.max() => AxisConstraint::loose(v),
                    _ => Ok(axis),
                };
                Constraints::new(
                    own(spec.width, natural_c.width)?,
                    own(spec.height, natural_c.height)?,
                )
            }
            _ => natural_c,
        };
        natural.push(engine.solve(id, c)?);
    }
    let origin = |id: T::Id| match mode {
        Mode::Absolute { .. } => engine.tree.spec(id).position,
        _ => Point::ZERO,
    };
    let content = kids.iter().zip(&natural).fold(Size::ZERO, |acc, (&id, s)| {
        let at = origin(id);
        Size::new(
            acc.width.max(at.x + s.size.width),
            acc.height.max(at.y + s.size.height),
        )
    });
    let size = Size::new(
        inner.width.clamp(content.width),
        inner.height.clamp(content.height),
    );

    let mut children = Vec::with_capacity(kids.len());
    let mut baseline = None;
    for (&id, natural) in kids.iter().zip(natural) {
        let spec = engine.tree.spec(id);
        let align_for = |scrolled: bool| match mode {
            Mode::Absolute { .. } => Align::Start,
            Mode::Scroll { .. } if scrolled => Align::Start,
            _ => spec.align_self.unwrap_or(Align::Stretch),
        };
        let (align_x, align_y) = (align_for(scroll_x), align_for(scroll_y));
        let stretch_x = align_x == Align::Stretch
            && !has_own_size(spec.width)
            && natural.size.width != size.width;
        let stretch_y = align_y == Align::Stretch
            && !has_own_size(spec.height)
            && natural.size.height != size.height;
        let solved = if stretch_x || stretch_y {
            let tight_or = |stretch: bool, extent: f64, fallback: AxisConstraint| {
                if stretch {
                    AxisConstraint::tight(extent)
                } else {
                    Ok(fallback)
                }
            };
            let c = Constraints::new(
                tight_or(stretch_x, size.width, natural_c.width)?,
                tight_or(stretch_y, size.height, natural_c.height)?,
            );
            engine.solve(id, c)?
        } else {
            natural
        };
        let at = origin(id);
        let x = at.x + align_offset(align_x, size.width, solved.size.width);
        let y = at.y + align_offset(align_y, size.height, solved.size.height);
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
