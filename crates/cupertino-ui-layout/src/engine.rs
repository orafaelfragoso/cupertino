//! The two-pass engine: `solve` sizes a node under constraints and arranges its children, `layout` then places the tree.

use crate::{
    grid, layers, stack, Align, Axis, AxisConstraint, Constraints, ContainerKind, LayoutDirection,
    LayoutError, LayoutOptions, LayoutResult, LayoutSpec, LayoutTree, NodeLayout, Overflow,
    SizeSpec, Track,
};
use cupertino_geometry::{Point, Rect, Size};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

const MAX_DEPTH: usize = 256;

type Key<Id> = (Id, [u64; 6]);

/// What a container's own algorithm produces, in the coordinates of its inner box (inside the insets), left to right.
pub(crate) struct Arranged<Id> {
    pub size: Size,
    pub baseline: Option<f64>,
    pub children: Vec<(Id, Rect)>,
}

/// A node sized under one set of constraints; its children are in the node's own coordinates.
pub(crate) struct Solved<Id> {
    pub size: Size,
    pub baseline: Option<f64>,
    pub clip: Option<Rect>,
    pub children: Vec<(Id, Rect)>,
}

pub(crate) struct Engine<'a, T: LayoutTree> {
    pub tree: &'a T,
    direction: LayoutDirection,
    cache: HashMap<Key<T::Id>, Rc<Solved<T::Id>>>,
}

/// Lays out the tree under `root` within `available`. The root sits at the origin; every other node is in its parent's
/// coordinates. Fails without partial results when the tree or a spec is invalid.
pub fn layout<T: LayoutTree>(
    tree: &T,
    root: T::Id,
    available: Constraints,
    options: &LayoutOptions,
) -> Result<LayoutResult<T::Id>, LayoutError> {
    validate_tree(tree, root, options.node_limit)?;
    let mut engine = Engine {
        tree,
        direction: options.direction,
        cache: HashMap::new(),
    };
    let size = engine.solve(root, available)?.size;
    let mut nodes = HashMap::new();
    let mut pending = vec![(root, Point::ZERO, size)];
    while let Some((id, origin, size)) = pending.pop() {
        let solved = engine.solve(id, Constraints::tight(size)?)?;
        nodes.insert(
            id,
            NodeLayout {
                rect: Rect::new(origin.x, origin.y, size.width, size.height),
                clip: solved.clip,
                baseline: solved.baseline,
            },
        );
        for (child, rect) in &solved.children {
            pending.push((*child, rect.origin, rect.size));
        }
    }
    Ok(LayoutResult { nodes })
}

fn validate_tree<T: LayoutTree>(
    tree: &T,
    root: T::Id,
    node_limit: usize,
) -> Result<(), LayoutError> {
    let mut seen = HashSet::new();
    let mut pending = vec![(root, 0usize)];
    while let Some((id, depth)) = pending.pop() {
        if depth >= MAX_DEPTH || seen.len() >= node_limit {
            return Err(LayoutError::NodeLimit);
        }
        if !seen.insert(id) {
            return Err(LayoutError::CycleOrMissingNode);
        }
        validate_spec(tree.spec(id))?;
        pending.extend(tree.children(id).iter().map(|child| (*child, depth + 1)));
    }
    Ok(())
}

fn validate_spec(spec: &LayoutSpec) -> Result<(), LayoutError> {
    let insets = spec.insets;
    let mut values = vec![
        insets.top,
        insets.left,
        insets.bottom,
        insets.right,
        spec.flex.grow,
        spec.flex.shrink,
    ];
    values.extend(spec.flex.basis);
    for size in [spec.width, spec.height] {
        match size {
            SizeSpec::Fixed(v) | SizeSpec::Fraction(v) => values.push(v),
            SizeSpec::Fit | SizeSpec::Fill => {}
        }
    }
    match &spec.kind {
        ContainerKind::Stack { spacing, .. } => values.push(*spacing),
        ContainerKind::Grid {
            columns,
            rows,
            gaps,
        } => {
            values.extend([gaps.width, gaps.height]);
            for track in columns.iter().chain(rows) {
                if let Track::Fixed(v) | Track::Fraction(v) = track {
                    values.push(*v);
                }
            }
        }
        _ => {}
    }
    if values.iter().any(|v| !v.is_finite()) {
        return Err(LayoutError::NonFiniteValue);
    }
    if values.iter().any(|v| *v < 0.0) {
        return Err(LayoutError::InvalidConstraint);
    }
    Ok(())
}

impl<T: LayoutTree> Engine<'_, T> {
    pub fn solve(
        &mut self,
        id: T::Id,
        constraints: Constraints,
    ) -> Result<Rc<Solved<T::Id>>, LayoutError> {
        let key = (id, key_of(constraints));
        if let Some(solved) = self.cache.get(&key) {
            return Ok(Rc::clone(solved));
        }
        let solved = Rc::new(self.compute(id, constraints)?);
        self.cache.insert(key, Rc::clone(&solved));
        Ok(solved)
    }

    fn compute(
        &mut self,
        id: T::Id,
        constraints: Constraints,
    ) -> Result<Solved<T::Id>, LayoutError> {
        let tree = self.tree;
        let spec = tree.spec(id);
        let inner = Constraints::new(
            inner_axis(spec.width, constraints.width, spec.insets.horizontal())?,
            inner_axis(spec.height, constraints.height, spec.insets.vertical())?,
        );
        let kids = tree.children(id);
        let mut arranged = match &spec.kind {
            ContainerKind::Leaf => {
                let measured = tree.measure_leaf(id, inner);
                let (w, h) = (measured.size.width, measured.size.height);
                if !w.is_finite()
                    || !h.is_finite()
                    || w < 0.0
                    || h < 0.0
                    || measured.baseline.is_some_and(|b| !b.is_finite())
                {
                    return Err(LayoutError::NonFiniteValue);
                }
                Arranged {
                    size: Size::new(inner.width.clamp(w), inner.height.clamp(h)),
                    baseline: measured.baseline,
                    children: Vec::new(),
                }
            }
            ContainerKind::Stack {
                axis,
                spacing,
                align,
                justify,
            } => stack::arrange(self, kids, *axis, *spacing, *align, *justify, inner)?,
            ContainerKind::Grid {
                columns,
                rows,
                gaps,
            } => grid::arrange(self, kids, columns, rows, *gaps, inner)?,
            ContainerKind::Absolute => layers::arrange(self, kids, layers::Mode::Absolute, inner)?,
            ContainerKind::Overlay => layers::arrange(self, kids, layers::Mode::Overlay, inner)?,
            ContainerKind::Scroll {
                horizontal,
                vertical,
            } => layers::arrange(
                self,
                kids,
                layers::Mode::Scroll {
                    horizontal: *horizontal,
                    vertical: *vertical,
                },
                inner,
            )?,
        };
        let scrolls = matches!(spec.kind, ContainerKind::Scroll { .. });
        if self.direction == LayoutDirection::Rtl && !scrolls {
            for (_, rect) in &mut arranged.children {
                rect.origin.x = arranged.size.width - rect.max_x();
            }
        }
        let insets = spec.insets;
        let size = Size::new(
            arranged.size.width + insets.horizontal(),
            arranged.size.height + insets.vertical(),
        );
        for (_, rect) in &mut arranged.children {
            rect.origin.x += insets.left;
            rect.origin.y += insets.top;
        }
        let clip = (scrolls || spec.overflow == Overflow::Clip)
            .then(|| Rect::new(0.0, 0.0, size.width, size.height));
        Ok(Solved {
            size,
            baseline: arranged.baseline.map(|b| b + insets.top),
            clip,
            children: arranged.children,
        })
    }
}

/// The room inside the insets on one axis. A node that sizes itself gets a tight range; otherwise its content may take
/// any size the parent allows.
fn inner_axis(
    spec: SizeSpec,
    outer: AxisConstraint,
    inset: f64,
) -> Result<AxisConstraint, LayoutError> {
    let forced = match spec {
        SizeSpec::Fit => None,
        SizeSpec::Fixed(v) => Some(outer.clamp(v)),
        SizeSpec::Fill => outer.max().is_finite().then_some(outer.max()),
        SizeSpec::Fraction(f) => outer
            .max()
            .is_finite()
            .then(|| outer.clamp(f * outer.max())),
    };
    match forced {
        Some(size) => AxisConstraint::tight((size - inset).max(0.0)),
        None => AxisConstraint::new(
            (outer.min() - inset).max(0.0),
            (outer.ideal() - inset).max(0.0),
            (outer.max() - inset).max(0.0),
        ),
    }
}

fn key_of(c: Constraints) -> [u64; 6] {
    [
        c.width.min().to_bits(),
        c.width.ideal().to_bits(),
        c.width.max().to_bits(),
        c.height.min().to_bits(),
        c.height.ideal().to_bits(),
        c.height.max().to_bits(),
    ]
}

impl Axis {
    pub fn split(self, c: Constraints) -> (AxisConstraint, AxisConstraint) {
        match self {
            Axis::Horizontal => (c.width, c.height),
            Axis::Vertical => (c.height, c.width),
        }
    }

    pub fn join(self, main: AxisConstraint, cross: AxisConstraint) -> Constraints {
        match self {
            Axis::Horizontal => Constraints::new(main, cross),
            Axis::Vertical => Constraints::new(cross, main),
        }
    }

    pub fn main(self, size: Size) -> f64 {
        match self {
            Axis::Horizontal => size.width,
            Axis::Vertical => size.height,
        }
    }

    pub fn cross(self, size: Size) -> f64 {
        match self {
            Axis::Horizontal => size.height,
            Axis::Vertical => size.width,
        }
    }

    pub fn pack(self, main: f64, cross: f64) -> Size {
        match self {
            Axis::Horizontal => Size::new(main, cross),
            Axis::Vertical => Size::new(cross, main),
        }
    }

    pub fn rect(self, main_pos: f64, cross_pos: f64, size: Size) -> Rect {
        match self {
            Axis::Horizontal => Rect::new(main_pos, cross_pos, size.width, size.height),
            Axis::Vertical => Rect::new(cross_pos, main_pos, size.width, size.height),
        }
    }

    pub fn size_spec(self, spec: &LayoutSpec) -> SizeSpec {
        match self {
            Axis::Horizontal => spec.width,
            Axis::Vertical => spec.height,
        }
    }

    pub fn cross_size_spec(self, spec: &LayoutSpec) -> SizeSpec {
        match self {
            Axis::Horizontal => spec.height,
            Axis::Vertical => spec.width,
        }
    }
}

pub(crate) fn is_fixed(spec: SizeSpec) -> bool {
    matches!(spec, SizeSpec::Fixed(_))
}

/// Whether the node sizes itself on this axis, so stretching must leave it alone.
pub(crate) fn has_own_size(spec: SizeSpec) -> bool {
    matches!(spec, SizeSpec::Fixed(_) | SizeSpec::Fraction(_))
}

pub(crate) fn align_offset(align: Align, room: f64, size: f64) -> f64 {
    match align {
        Align::Center => (room - size) / 2.0,
        Align::End => room - size,
        Align::Start | Align::Stretch | Align::Baseline => 0.0,
    }
}
