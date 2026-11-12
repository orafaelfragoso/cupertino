//! The two-pass engine: `solve` sizes a node under constraints and arranges its children, `layout` then places the tree.

use crate::{
    grid, layers, stack, Align, Axis, AxisConstraint, Constraints, ContainerKind, LayoutDirection,
    LayoutError, LayoutOptions, LayoutResult, LayoutSpec, LayoutStats, LayoutTree, NodeLayout,
    Overflow, SizeSpec, Track,
};
use cupertino_geometry::{Point, Rect, Size};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

const MAX_DEPTH: usize = 256;

/// Sizing results kept from earlier calls so an unchanged subtree is not measured again. A node's entries are dropped when
/// its [`Generation`](crate::Generation) changed or any descendant's did; entries of nodes that left the tree go too.
/// Pass the same cache to every [`layout_cached`] call for one tree.
pub struct LayoutCache<Id> {
    entries: HashMap<Key<Id>, Rc<Solved<Id>>>,
    minimums: HashMap<MinKey<Id>, f64>,
    generations: HashMap<Id, crate::Generation>,
    direction: Option<LayoutDirection>,
}

impl<Id> Default for LayoutCache<Id> {
    fn default() -> Self {
        Self {
            entries: HashMap::new(),
            minimums: HashMap::new(),
            generations: HashMap::new(),
            direction: None,
        }
    }
}

impl<Id> LayoutCache<Id> {
    pub fn new() -> Self {
        Self::default()
    }

    /// Entries held, sizing results and minimum sizes together.
    pub fn len(&self) -> usize {
        self.entries.len() + self.minimums.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.minimums.clear();
        self.generations.clear();
        self.direction = None;
    }
}

/// A cache holding more entries than this many per node is dropped whole, so changing constraints cannot grow it forever.
const ENTRIES_PER_NODE: usize = 16;

type Key<Id> = (Id, [u64; 6]);
type MinKey<Id> = (Id, bool, u64);

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
    pub node_limit: usize,
    cache: &'a mut LayoutCache<T::Id>,
    pub stats: LayoutStats,
}

/// Lays out the tree under `root` within `available`. The root sits at the origin; every other node is in its parent's
/// coordinates. Fails without partial results when the tree or a spec is invalid.
pub fn layout<T: LayoutTree>(
    tree: &T,
    root: T::Id,
    available: Constraints,
    options: &LayoutOptions,
) -> Result<LayoutResult<T::Id>, LayoutError> {
    layout_cached(tree, root, available, options, &mut LayoutCache::new())
}

/// [`layout`] that reuses `cache` from earlier calls. The result is the same as a call without a cache; only the work
/// differs, and [`LayoutResult::stats`] shows it. Placing every node and building the result still costs `O(nodes)`.
pub fn layout_cached<T: LayoutTree>(
    tree: &T,
    root: T::Id,
    available: Constraints,
    options: &LayoutOptions,
    cache: &mut LayoutCache<T::Id>,
) -> Result<LayoutResult<T::Id>, LayoutError> {
    let visits = validate_tree(tree, root, options.node_limit)?;
    refresh(tree, &visits, options.direction, cache);
    let mut engine = Engine {
        tree,
        direction: options.direction,
        node_limit: options.node_limit,
        cache,
        stats: LayoutStats::default(),
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
    Ok(LayoutResult {
        nodes,
        stats: engine.stats,
    })
}

/// Drops what the tree's changes make stale: entries of changed nodes and of all their ancestors, of nodes that are gone,
/// and everything when the direction changed or the cache outgrew its bound.
fn refresh<T: LayoutTree>(
    tree: &T,
    visits: &[Visit<T::Id>],
    direction: LayoutDirection,
    cache: &mut LayoutCache<T::Id>,
) {
    if cache.direction != Some(direction) || cache.len() > ENTRIES_PER_NODE * visits.len() {
        cache.clear();
        cache.direction = Some(direction);
    }
    let mut dirty: Vec<bool> = visits
        .iter()
        .map(|v| cache.generations.get(&v.id) != Some(&tree.generation(v.id)))
        .collect();
    for index in (0..visits.len()).rev() {
        if let (true, Some(parent)) = (dirty[index], visits[index].parent) {
            dirty[parent] = true;
        }
    }
    let stale: HashSet<T::Id> = visits
        .iter()
        .zip(&dirty)
        .filter(|(_, d)| **d)
        .map(|(v, _)| v.id)
        .collect();
    let present: HashSet<T::Id> = visits.iter().map(|v| v.id).collect();
    cache
        .entries
        .retain(|(id, _), _| present.contains(id) && !stale.contains(id));
    cache
        .minimums
        .retain(|(id, _, _), _| present.contains(id) && !stale.contains(id));
    cache.generations = visits
        .iter()
        .map(|v| (v.id, tree.generation(v.id)))
        .collect();
}

struct Visit<Id> {
    id: Id,
    parent: Option<usize>,
}

/// Every node under `root`, parents before children, after checking the tree and its specs.
fn validate_tree<T: LayoutTree>(
    tree: &T,
    root: T::Id,
    node_limit: usize,
) -> Result<Vec<Visit<T::Id>>, LayoutError> {
    let mut seen = HashSet::new();
    let mut visits = Vec::new();
    let mut pending = vec![(root, None, 0usize)];
    while let Some((id, parent, depth)) = pending.pop() {
        if depth >= MAX_DEPTH || seen.len() >= node_limit {
            return Err(LayoutError::NodeLimit);
        }
        if !seen.insert(id) {
            return Err(LayoutError::CycleOrMissingNode);
        }
        validate_spec(tree.spec(id))?;
        let index = visits.len();
        visits.push(Visit { id, parent });
        pending.extend(
            tree.children(id)
                .iter()
                .map(|child| (*child, Some(index), depth + 1)),
        );
    }
    Ok(visits)
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
    if !spec.position.x.is_finite() || !spec.position.y.is_finite() {
        return Err(LayoutError::NonFiniteValue);
    }
    for size in [spec.width, spec.height] {
        match size {
            SizeSpec::Fixed(v) | SizeSpec::Fraction(v) => values.push(v),
            SizeSpec::Fit | SizeSpec::Fill => {}
        }
    }
    match &spec.kind {
        ContainerKind::Stack { spacing, .. } => values.push(*spacing),
        ContainerKind::Wrap { gaps } => values.extend([gaps.width, gaps.height]),
        ContainerKind::Split(split) => values.extend([
            split.leading,
            split.gap,
            split.min_leading,
            split.min_trailing,
        ]),
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
        if let Some(solved) = self.cache.entries.get(&key) {
            return Ok(Rc::clone(solved));
        }
        self.stats.solves += 1;
        let solved = Rc::new(self.compute(id, constraints)?);
        self.cache.entries.insert(key, Rc::clone(&solved));
        Ok(solved)
    }

    /// The smallest size along `axis` the node can take without its content overflowing, when its other axis has at most
    /// `cross_max` (a width; the smallest width is asked for without a height limit). A leaf answers with what `measure_leaf` returns when the axis maximum is zero; containers add up
    /// or take the largest of their children's. Grid cells spanning several tracks are left out.
    pub fn min_content(
        &mut self,
        id: T::Id,
        axis: Axis,
        cross_max: f64,
    ) -> Result<f64, LayoutError> {
        let cross_max = match axis {
            Axis::Horizontal => f64::INFINITY,
            Axis::Vertical => cross_max,
        };
        let key = (id, axis == Axis::Vertical, cross_max.to_bits());
        if let Some(min) = self.cache.minimums.get(&key) {
            return Ok(*min);
        }
        let tree = self.tree;
        let spec = tree.spec(id);
        let (inset, cross_inset) = match axis {
            Axis::Horizontal => (spec.insets.horizontal(), spec.insets.vertical()),
            Axis::Vertical => (spec.insets.vertical(), spec.insets.horizontal()),
        };
        let inner_cross = (cross_max - cross_inset).max(0.0);
        let kids = tree.children(id);
        let content = match &spec.kind {
            ContainerKind::Leaf => {
                let c = axis.join(
                    AxisConstraint::loose(0.0)?,
                    AxisConstraint::loose(inner_cross)?,
                );
                self.stats.leaf_measures += 1;
                let size = axis.main(tree.measure_leaf(id, c).size);
                if !size.is_finite() || size < 0.0 {
                    return Err(LayoutError::NonFiniteValue);
                }
                size
            }
            ContainerKind::Stack {
                axis: along,
                spacing,
                ..
            } => {
                let mut mins = Vec::with_capacity(kids.len());
                for &kid in kids {
                    mins.push(self.min_content(kid, axis, inner_cross)?);
                }
                if *along == axis {
                    mins.iter().sum::<f64>() + spacing * kids.len().saturating_sub(1) as f64
                } else {
                    mins.into_iter().fold(0.0, f64::max)
                }
            }
            ContainerKind::Grid {
                columns,
                rows,
                gaps,
            } => {
                let column_count = columns.len().max(1);
                let spans: Vec<_> = kids.iter().map(|&k| tree.spec(k).span).collect();
                let cells = grid::place(
                    &spans,
                    column_count,
                    rows.len().max(kids.len()),
                    self.node_limit,
                )?;
                let (tracks, gap) = match axis {
                    Axis::Horizontal => (columns, gaps.width),
                    Axis::Vertical => (rows, gaps.height),
                };
                let placed = cells
                    .iter()
                    .map(|c| match axis {
                        Axis::Horizontal => c.column + c.columns,
                        Axis::Vertical => c.row + c.rows,
                    })
                    .max()
                    .unwrap_or(0);
                let count = placed.max(tracks.len()).max(match axis {
                    Axis::Horizontal => column_count,
                    Axis::Vertical => 0,
                });
                let mut sizes = vec![0.0f64; count];
                for (&kid, cell) in kids.iter().zip(&cells) {
                    let (start, length) = match axis {
                        Axis::Horizontal => (cell.column, cell.columns),
                        Axis::Vertical => (cell.row, cell.rows),
                    };
                    if length == 1 {
                        let min = self.min_content(kid, axis, inner_cross)?;
                        sizes[start] = sizes[start].max(min);
                    }
                }
                for (size, track) in sizes.iter_mut().zip(tracks) {
                    if let Track::Fixed(v) = track {
                        *size = *v;
                    }
                }
                sizes.iter().sum::<f64>() + gap * sizes.len().saturating_sub(1) as f64
            }
            ContainerKind::Wrap { gaps } => {
                let mut sizes = Vec::with_capacity(kids.len());
                for &kid in kids {
                    let width = self.min_content(kid, Axis::Horizontal, f64::INFINITY)?;
                    let height = self.min_content(kid, Axis::Vertical, width)?;
                    sizes.push(Size::new(width, height));
                }
                match axis {
                    // the widest child alone in its row
                    Axis::Horizontal => sizes.iter().map(|s| s.width).fold(0.0, f64::max),
                    Axis::Vertical => crate::wrap::place(&sizes, inner_cross, *gaps).1.height,
                }
            }
            ContainerKind::Split(split) if split.axis == axis => match kids.get(1) {
                Some(&trailing) => self.min_content(trailing, axis, inner_cross)?,
                None => 0.0,
            },
            ContainerKind::Split(_) => {
                let mut largest = 0.0f64;
                for &kid in kids {
                    largest = largest.max(self.min_content(kid, axis, inner_cross)?);
                }
                largest
            }
            ContainerKind::Absolute | ContainerKind::Overlay | ContainerKind::Scroll { .. } => {
                let scrolled = match spec.kind {
                    ContainerKind::Scroll {
                        horizontal,
                        vertical,
                    } => match axis {
                        Axis::Horizontal => horizontal,
                        Axis::Vertical => vertical,
                    },
                    _ => false,
                };
                let mut largest = 0.0f64;
                if !scrolled {
                    for &kid in kids {
                        let min = self.min_content(kid, axis, inner_cross)?;
                        let position = tree.spec(kid).position;
                        let at = match (&spec.kind, axis) {
                            (ContainerKind::Absolute, Axis::Horizontal) => position.x,
                            (ContainerKind::Absolute, Axis::Vertical) => position.y,
                            _ => 0.0,
                        };
                        largest = largest.max(at + min);
                    }
                }
                largest
            }
        };
        let min = match axis.size_spec(spec) {
            SizeSpec::Fixed(v) => v,
            _ => content + inset,
        };
        self.cache.minimums.insert(key, min);
        Ok(min)
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
                self.stats.leaf_measures += 1;
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
            ContainerKind::Split(split) => crate::split::arrange(self, kids, split, inner)?,
            ContainerKind::Wrap { gaps } => crate::wrap::arrange(self, kids, *gaps, inner)?,
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
        if self.direction == LayoutDirection::Rtl {
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
