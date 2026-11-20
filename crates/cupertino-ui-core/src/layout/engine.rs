//! The two-pass engine: `solve` sizes a node under constraints and arranges its children, `layout` then places the tree.
//!
//! Sizing results are cached flat in [`LayoutCache`] (indexed ranges into a shared child-rect pool, avoiding per-entry
//! `Rc` and `Vec` heap allocations), tree-level scratch buffers live in a reusable [`bumpalo::Bump`] arena reset once
//! per layout call, and per-container scratch stays stack-resident via `SmallVec` so recursive solves stay L1-hot.

use crate::layout::{
    grid, layers, stack, Align, Axis, AxisConstraint, Constraints, ContainerKind, LayoutDirection,
    LayoutError, LayoutOptions, LayoutResult, LayoutSpec, LayoutStats, LayoutTree, NodeLayout,
    Overflow, SizeSpec, Track,
};
use crate::layout::{IdMap, IdSet};
use bumpalo::collections::Vec as BumpVec;
use bumpalo::Bump;
use cupertino_geometry::{Point, Rect, Size};
use smallvec::SmallVec;

const MAX_DEPTH: usize = 256;

/// Inline capacity for a single container's arranged child rects before spilling to the heap.
pub(crate) type ArrangedChildren<Id> = SmallVec<[(Id, Rect); 16]>;

/// Flat cache storage backing [`LayoutCache`], split from the scratch arena so [`Engine`] can hold `&Bump` alongside
/// `&mut CacheStore` without borrow conflicts.
struct CacheStore<Id> {
    entries: IdMap<Key<Id>, Solved>,
    children_pool: Vec<(Id, Rect)>,
    minimums: IdMap<MinKey<Id>, f64>,
    generations: IdMap<Id, crate::layout::Generation>,
    seen: IdSet<Id>,
    direction: Option<LayoutDirection>,
}

impl<Id> Default for CacheStore<Id> {
    fn default() -> Self {
        Self {
            entries: IdMap::default(),
            children_pool: Vec::new(),
            minimums: IdMap::default(),
            generations: IdMap::default(),
            seen: IdSet::default(),
            direction: None,
        }
    }
}

/// Sizing results kept from earlier calls so an unchanged subtree is not measured again. A node's entries are dropped when
/// its [`Generation`](crate::layout::Generation) changed or any descendant's did; entries of nodes that left the tree go too.
/// Pass the same cache to every [`layout_cached`] call for one tree.
pub struct LayoutCache<Id> {
    store: CacheStore<Id>,
    scratch: Bump,
}

impl<Id> Default for LayoutCache<Id> {
    fn default() -> Self {
        Self {
            store: CacheStore::default(),
            scratch: Bump::new(),
        }
    }
}

impl<Id> LayoutCache<Id> {
    pub fn new() -> Self {
        Self::default()
    }

    /// Entries held, sizing results and minimum sizes together.
    pub fn len(&self) -> usize {
        self.store.entries.len() + self.store.minimums.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn clear(&mut self) {
        self.store.entries.clear();
        self.store.children_pool.clear();
        self.store.minimums.clear();
        self.store.generations.clear();
        self.store.seen.clear();
        self.store.direction = None;
        self.scratch.reset();
    }
}

/// A cache holding more entries than this many per node is dropped whole, so changing constraints cannot grow it forever.
const ENTRIES_PER_NODE: usize = 16;

/// Child-pool compaction multiplier: once the append-only child-rect pool grows past this multiple of live cache entries,
/// the cache resets so stale child slices cannot accumulate indefinitely.
const POOL_COMPACT_FACTOR: usize = 4;

type Key<Id> = (Id, [u64; 6]);
type MinKey<Id> = (Id, bool, u64);

/// What a container's own algorithm produces, in the coordinates of its inner box (inside the insets), left to right.
pub(crate) struct Arranged<Id> {
    pub size: Size,
    pub baseline: Option<f64>,
    pub children: ArrangedChildren<Id>,
}

/// A node sized under one set of constraints; its children slice lives in `CacheStore::children_pool`.
#[derive(Clone, Copy)]
pub(crate) struct Solved {
    pub size: Size,
    pub baseline: Option<f64>,
    pub clip: Option<Rect>,
    children_start: u32,
    children_len: u32,
}

pub(crate) struct Engine<'a, 'b, T: LayoutTree> {
    pub tree: &'a T,
    pub bump: &'b Bump,
    direction: LayoutDirection,
    pub node_limit: usize,
    store: &'a mut CacheStore<T::Id>,
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
    cache.scratch.reset();
    let bump = &cache.scratch;
    let store = &mut cache.store;
    let (visits, all_clean) = validate_tree(
        tree,
        root,
        options.node_limit,
        store,
        options.direction,
        bump,
    )?;
    let node_count = visits.len();
    if !all_clean {
        refresh(tree, &visits, options.direction, store, bump);
    }
    if store.entries.capacity() < node_count * 2 {
        store
            .entries
            .reserve((node_count * 2).saturating_sub(store.entries.len()));
    }
    if store.children_pool.capacity() < node_count * 2 {
        store
            .children_pool
            .reserve((node_count * 2).saturating_sub(store.children_pool.len()));
    }
    let mut engine = Engine {
        tree,
        bump,
        direction: options.direction,
        node_limit: options.node_limit,
        store,
        stats: LayoutStats::default(),
    };
    let size = engine.solve(root, available)?.size;
    let mut nodes = IdMap::with_capacity_and_hasher(node_count, Default::default());
    let mut pending = BumpVec::with_capacity_in(node_count.min(256), bump);
    pending.push((root, Point::ZERO, size));
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
        let start = solved.children_start as usize;
        let end = start + solved.children_len as usize;
        for &(child, rect) in &engine.store.children_pool[start..end] {
            pending.push((child, rect.origin, rect.size));
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
    store: &mut CacheStore<T::Id>,
    bump: &Bump,
) {
    let cache_len = store.entries.len() + store.minimums.len();
    let max_entries = ENTRIES_PER_NODE * visits.len();
    let reset_all = store.direction != Some(direction)
        || cache_len > max_entries
        || store.children_pool.len() > max_entries.saturating_mul(POOL_COMPACT_FACTOR).max(4096);
    if reset_all {
        store.entries.clear();
        store.children_pool.clear();
        store.minimums.clear();
        store.generations.clear();
        store.direction = Some(direction);
        store.generations.reserve(visits.len());
        for v in visits {
            store.generations.insert(v.id, tree.generation(v.id));
        }
        return;
    }
    let mut dirty = BumpVec::with_capacity_in(visits.len(), bump);
    for v in visits {
        dirty.push(store.generations.get(&v.id) != Some(&tree.generation(v.id)));
    }
    for index in (0..visits.len()).rev() {
        if let (true, Some(parent)) = (dirty[index], visits[index].parent) {
            dirty[parent] = true;
        }
    }
    let mut stale = IdSet::default();
    for (v, &is_dirty) in visits.iter().zip(dirty.iter()) {
        if is_dirty {
            stale.insert(v.id);
        }
    }
    // `store.seen` already holds the exact set of present ids from `validate_tree`.
    if !stale.is_empty() || store.seen.len() != store.generations.len() {
        let present = &store.seen;
        store
            .entries
            .retain(|(id, _), _| present.contains(id) && !stale.contains(id));
        store
            .minimums
            .retain(|(id, _, _), _| present.contains(id) && !stale.contains(id));
    }
    store.generations.clear();
    store.generations.reserve(visits.len());
    for v in visits {
        store.generations.insert(v.id, tree.generation(v.id));
    }
}

struct Visit<Id> {
    id: Id,
    parent: Option<usize>,
}

type ValidatedTree<'b, Id> = (BumpVec<'b, Visit<Id>>, bool);

/// Every node under `root`, parents before children, after checking the tree and its specs.
/// Also reports whether the cache is already completely clean for this tree and direction.
fn validate_tree<'b, T: LayoutTree>(
    tree: &T,
    root: T::Id,
    node_limit: usize,
    store: &mut CacheStore<T::Id>,
    direction: LayoutDirection,
    bump: &'b Bump,
) -> Result<ValidatedTree<'b, T::Id>, LayoutError> {
    let expected = store.generations.len();
    store.seen.clear();
    if store.seen.capacity() < expected {
        store.seen.reserve(expected);
    }
    let mut visits = BumpVec::with_capacity_in(expected.max(64), bump);
    let mut pending = BumpVec::with_capacity_in(64, bump);
    pending.push((root, None, 0usize));
    let mut generations_match = store.direction == Some(direction) && expected > 0;
    while let Some((id, parent, depth)) = pending.pop() {
        if depth >= MAX_DEPTH || store.seen.len() >= node_limit {
            return Err(LayoutError::NodeLimit);
        }
        if !store.seen.insert(id) {
            return Err(LayoutError::CycleOrMissingNode);
        }
        validate_spec(tree.spec(id))?;
        if generations_match && store.generations.get(&id) != Some(&tree.generation(id)) {
            generations_match = false;
        }
        let index = visits.len();
        visits.push(Visit { id, parent });
        for &child in tree.children(id) {
            pending.push((child, Some(index), depth + 1));
        }
    }
    let cache_len = store.entries.len() + store.minimums.len();
    let max_entries = ENTRIES_PER_NODE * visits.len();
    let all_clean = generations_match
        && visits.len() == expected
        && cache_len <= max_entries
        && store.children_pool.len() <= max_entries.saturating_mul(POOL_COMPACT_FACTOR).max(4096);
    Ok((visits, all_clean))
}

#[inline]
fn check_non_negative(value: f64) -> Result<(), LayoutError> {
    if !value.is_finite() {
        return Err(LayoutError::NonFiniteValue);
    }
    if value < 0.0 {
        return Err(LayoutError::InvalidConstraint);
    }
    Ok(())
}

fn validate_spec(spec: &LayoutSpec) -> Result<(), LayoutError> {
    let insets = spec.insets;
    check_non_negative(insets.top)?;
    check_non_negative(insets.left)?;
    check_non_negative(insets.bottom)?;
    check_non_negative(insets.right)?;
    check_non_negative(spec.flex.grow)?;
    check_non_negative(spec.flex.shrink)?;
    if let Some(basis) = spec.flex.basis {
        check_non_negative(basis)?;
    }
    if !spec.position.x.is_finite() || !spec.position.y.is_finite() {
        return Err(LayoutError::NonFiniteValue);
    }
    for size in [spec.width, spec.height] {
        if let SizeSpec::Fixed(v) | SizeSpec::Fraction(v) = size {
            check_non_negative(v)?;
        }
    }
    match &spec.kind {
        ContainerKind::Stack { spacing, .. } => check_non_negative(*spacing)?,
        ContainerKind::Wrap { gaps } => {
            check_non_negative(gaps.width)?;
            check_non_negative(gaps.height)?;
        }
        ContainerKind::Split(split) => {
            check_non_negative(split.leading)?;
            check_non_negative(split.gap)?;
            check_non_negative(split.min_leading)?;
            check_non_negative(split.min_trailing)?;
        }
        ContainerKind::Grid {
            columns,
            rows,
            gaps,
        } => {
            check_non_negative(gaps.width)?;
            check_non_negative(gaps.height)?;
            for track in columns.iter().chain(rows) {
                if let Track::Fixed(v) | Track::Fraction(v) = track {
                    check_non_negative(*v)?;
                }
            }
        }
        _ => {}
    }
    Ok(())
}

impl<T: LayoutTree> Engine<'_, '_, T> {
    pub fn solve(&mut self, id: T::Id, constraints: Constraints) -> Result<Solved, LayoutError> {
        let key = (id, key_of(constraints));
        if let Some(&solved) = self.store.entries.get(&key) {
            return Ok(solved);
        }
        self.stats.solves += 1;
        let solved = self.compute(id, constraints)?;
        self.store.entries.insert(key, solved);
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
        if let Some(&min) = self.store.minimums.get(&key) {
            return Ok(min);
        }
        let tree = self.tree;
        let bump = self.bump;
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
                if *along == axis {
                    let mut sum = 0.0;
                    for &kid in kids {
                        sum += self.min_content(kid, axis, inner_cross)?;
                    }
                    sum + spacing * kids.len().saturating_sub(1) as f64
                } else {
                    let mut max_val = 0.0f64;
                    for &kid in kids {
                        max_val = max_val.max(self.min_content(kid, axis, inner_cross)?);
                    }
                    max_val
                }
            }
            ContainerKind::Grid {
                columns,
                rows,
                gaps,
            } => {
                let column_count = columns.len().max(1);
                let mut spans: SmallVec<[_; 16]> = SmallVec::with_capacity(kids.len());
                for &k in kids {
                    spans.push(tree.spec(k).span);
                }
                let cells = grid::place(
                    &spans,
                    column_count,
                    rows.len().max(kids.len()),
                    self.node_limit,
                    bump,
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
                let mut sizes: SmallVec<[f64; 16]> = smallvec::smallvec![0.0f64; count];
                for (&kid, cell) in kids.iter().zip(cells.iter()) {
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
                let mut sizes: SmallVec<[Size; 16]> = SmallVec::with_capacity(kids.len());
                for &kid in kids {
                    let width = self.min_content(kid, Axis::Horizontal, f64::INFINITY)?;
                    let height = self.min_content(kid, Axis::Vertical, width)?;
                    sizes.push(Size::new(width, height));
                }
                match axis {
                    // the widest child alone in its row
                    Axis::Horizontal => sizes.iter().map(|s| s.width).fold(0.0, f64::max),
                    Axis::Vertical => {
                        crate::layout::wrap::place(&sizes, inner_cross, *gaps, bump)
                            .1
                            .height
                    }
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
        self.store.minimums.insert(key, min);
        Ok(min)
    }

    fn compute(&mut self, id: T::Id, constraints: Constraints) -> Result<Solved, LayoutError> {
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
                    children: ArrangedChildren::new(),
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
            ContainerKind::Absolute => {
                let crops = spec.overflow == Overflow::Crop;
                layers::arrange(self, kids, layers::Mode::Absolute { crops }, inner)?
            }
            ContainerKind::Overlay => layers::arrange(self, kids, layers::Mode::Overlay, inner)?,
            ContainerKind::Split(split) => crate::layout::split::arrange(self, kids, split, inner)?,
            ContainerKind::Wrap { gaps } => crate::layout::wrap::arrange(self, kids, *gaps, inner)?,
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
        let clip = (scrolls || spec.overflow != Overflow::Visible)
            .then(|| Rect::new(0.0, 0.0, size.width, size.height));
        let children_start = self.store.children_pool.len() as u32;
        let children_len = arranged.children.len() as u32;
        if children_len > 0 {
            self.store
                .children_pool
                .extend_from_slice(&arranged.children);
        }
        Ok(Solved {
            size,
            baseline: arranged.baseline.map(|b| b + insets.top),
            clip,
            children_start,
            children_len,
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
