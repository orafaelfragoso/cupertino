//! CupertinoKit layout. A container measures its children under [`Constraints`] and then places them; core hands its
//! node tree in through [`LayoutTree`] and reads back a [`LayoutResult`]. Leaves (text, images, icons) are measured by
//! the tree, so this crate never sees a font. Values are logical points; the scale factor never reaches layout.

mod engine;
mod grid;
mod layers;
mod scroll;
mod split;
mod stack;
mod virtual_list;
mod wrap;

use cupertino_geometry::{Insets, Point, Rect, Size};
use std::collections::{HashMap, HashSet};
use std::hash::{BuildHasherDefault, Hash, Hasher};

pub use engine::{layout, layout_cached, LayoutCache};

/// A hasher for ids a program hands out itself (node ids, paint node ids): a rotate, an exclusive or and a multiply per
/// word. The standard maps' keyed hash guards against keys chosen to collide, which such ids never are, and costs more
/// than the rest of a lookup; a frame looks up every node of a window several times.
#[derive(Clone, Copy, Debug, Default)]
pub struct IdHasher(u64);

/// The odd constant [`IdHasher`] multiplies by: its bits spread each word over the whole hash.
const ID_SPREAD: u64 = 0x517c_c1b7_2722_0a95;

impl IdHasher {
    fn add(&mut self, word: u64) {
        self.0 = (self.0.rotate_left(5) ^ word).wrapping_mul(ID_SPREAD);
    }
}

impl Hasher for IdHasher {
    fn write(&mut self, bytes: &[u8]) {
        for chunk in bytes.chunks(8) {
            let mut word = [0u8; 8];
            word[..chunk.len()].copy_from_slice(chunk);
            self.add(u64::from_le_bytes(word));
        }
    }

    fn write_u8(&mut self, v: u8) {
        self.add(u64::from(v));
    }

    fn write_u32(&mut self, v: u32) {
        self.add(u64::from(v));
    }

    fn write_u64(&mut self, v: u64) {
        self.add(v);
    }

    fn write_usize(&mut self, v: usize) {
        self.add(v as u64);
    }

    fn finish(&self) -> u64 {
        self.0
    }
}

/// A map keyed by ids, hashed by [`IdHasher`].
pub type IdMap<K, V> = HashMap<K, V, BuildHasherDefault<IdHasher>>;

/// A set of ids, hashed by [`IdHasher`].
pub type IdSet<K> = HashSet<K, BuildHasherDefault<IdHasher>>;
pub use virtual_list::VirtualList;

/// The range one axis may take: `0 <= min <= ideal <= max`, where only `max` may be infinite.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AxisConstraint {
    min: f64,
    ideal: f64,
    max: f64,
}

impl AxisConstraint {
    pub fn new(min: f64, ideal: f64, max: f64) -> Result<Self, LayoutError> {
        if !min.is_finite() || !ideal.is_finite() || max.is_nan() {
            return Err(LayoutError::NonFiniteValue);
        }
        if !(0.0 <= min && min <= ideal && ideal <= max) {
            return Err(LayoutError::InvalidConstraint);
        }
        Ok(Self { min, ideal, max })
    }

    /// Exactly `value`.
    pub fn tight(value: f64) -> Result<Self, LayoutError> {
        Self::new(value, value, value)
    }

    /// Anything from zero up to `max`.
    pub fn loose(max: f64) -> Result<Self, LayoutError> {
        Self::new(0.0, 0.0, max)
    }

    /// `value` moved into `min..=max`.
    pub fn clamp(self, value: f64) -> f64 {
        value.max(self.min).min(self.max)
    }

    pub fn min(self) -> f64 {
        self.min
    }

    pub fn ideal(self) -> f64 {
        self.ideal
    }

    pub fn max(self) -> f64 {
        self.max
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Constraints {
    pub width: AxisConstraint,
    pub height: AxisConstraint,
}

impl Constraints {
    pub fn new(width: AxisConstraint, height: AxisConstraint) -> Self {
        Self { width, height }
    }

    /// Exactly `size`.
    pub fn tight(size: Size) -> Result<Self, LayoutError> {
        Ok(Self::new(
            AxisConstraint::tight(size.width)?,
            AxisConstraint::tight(size.height)?,
        ))
    }

    /// Anything from zero up to `max`.
    pub fn loose(max: Size) -> Result<Self, LayoutError> {
        Ok(Self::new(
            AxisConstraint::loose(max.width)?,
            AxisConstraint::loose(max.height)?,
        ))
    }
}

/// A measured node: its size and the distance from its top to its first baseline, when it has text.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Measured {
    pub size: Size,
    pub baseline: Option<f64>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum LayoutDirection {
    #[default]
    Ltr,
    Rtl,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Axis {
    Horizontal,
    Vertical,
}

/// A logical edge: `Start` and `End` follow the [`LayoutDirection`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Edge {
    Start,
    End,
    Top,
    Bottom,
}

/// Cross-axis alignment.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Align {
    #[default]
    Start,
    Center,
    End,
    Stretch,
    Baseline,
}

/// Main-axis distribution of what flexible children leave over.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Justify {
    #[default]
    Start,
    Center,
    End,
    SpaceBetween,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum SizeSpec {
    #[default]
    Fit,
    Fill,
    Fixed(f64),
    Fraction(f64),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Flex {
    pub grow: f64,
    pub shrink: f64,
    pub basis: Option<f64>,
}

impl Flex {
    /// Keeps its natural size.
    pub const NONE: Flex = Flex {
        grow: 0.0,
        shrink: 0.0,
        basis: None,
    };

    /// Shares the leftover space in proportion to `grow`, starting from nothing.
    pub const fn grow(grow: f64) -> Flex {
        Flex {
            grow,
            shrink: 1.0,
            basis: Some(0.0),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Overflow {
    #[default]
    Visible,
    Clip,
    /// Clips as [`Overflow::Clip`] does, and an absolute container's child of a size of its own keeps it past the
    /// container's edge: a picture cut off by its frame as the frame shrinks, rather than laid out again smaller.
    Crop,
}

/// A grid track.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Track {
    Fixed(f64),
    Auto,
    Fraction(f64),
}

#[derive(Clone, Debug, PartialEq)]
pub enum ContainerKind {
    Leaf,
    Stack {
        axis: Axis,
        spacing: f64,
        align: Align,
        justify: Justify,
    },
    Grid {
        columns: Vec<Track>,
        rows: Vec<Track>,
        gaps: Size,
    },
    Absolute,
    Overlay,
    Scroll {
        horizontal: bool,
        vertical: bool,
    },
    /// Two children along an axis: the leading one at its asked extent within its limits, the trailing one taking
    /// the rest; see [`SplitSpec::shown`].
    Split(SplitSpec),
    /// Children at their natural sizes in rows across the width, as many in each row as fit, `gaps` apart.
    Wrap {
        gaps: Size,
    },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SplitSpec {
    pub axis: Axis,
    /// The leading child's asked extent.
    pub leading: f64,
    /// The room between the children.
    pub gap: f64,
    pub min_leading: f64,
    pub min_trailing: f64,
}

impl SplitSpec {
    /// The leading child's extent in `extent`: the asked one, kept between its minimum and what leaves the trailing
    /// child its minimum. None when both minimums do not fit, so the leading child hides and the trailing one takes
    /// all of `extent`.
    pub fn shown(&self, extent: f64) -> Option<f64> {
        let room = extent - self.gap - self.min_trailing;
        (self.min_leading <= room).then(|| self.leading.clamp(self.min_leading, room))
    }
}

/// How many grid tracks a cell covers. Zero counts as one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct GridSpan {
    pub columns: usize,
    pub rows: usize,
}

impl GridSpan {
    pub const ONE: GridSpan = GridSpan {
        columns: 1,
        rows: 1,
    };
}

impl Default for GridSpan {
    fn default() -> Self {
        Self::ONE
    }
}

/// How one node takes part in layout.
#[derive(Clone, Debug, PartialEq)]
pub struct LayoutSpec {
    pub kind: ContainerKind,
    pub insets: Insets,
    pub width: SizeSpec,
    pub height: SizeSpec,
    pub flex: Flex,
    pub align_self: Option<Align>,
    pub overflow: Overflow,
    /// The tracks this node covers when its parent is a grid.
    pub span: GridSpan,
    /// Where this node's top-leading corner sits in an absolute container, from the container's own corner.
    pub position: Point,
}

impl LayoutSpec {
    /// Fits its content, keeps its natural size, no insets, no clipping.
    pub fn new(kind: ContainerKind) -> Self {
        Self {
            kind,
            insets: Insets::ZERO,
            width: SizeSpec::Fit,
            height: SizeSpec::Fit,
            flex: Flex::NONE,
            align_self: None,
            overflow: Overflow::Visible,
            span: GridSpan::ONE,
            position: Point::ZERO,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayoutError {
    InvalidConstraint,
    NonFiniteValue,
    /// More nodes than [`LayoutOptions::node_limit`], a tree deeper than 256 levels, or a grid whose cells or placement
    /// work exceed that limit.
    NodeLimit,
    /// A node reachable twice, which includes a cycle.
    CycleOrMissingNode,
}

/// The node tree layout runs over, borrowed from its owner.
pub trait LayoutTree {
    type Id: Copy + Eq + Hash;

    fn children(&self, id: Self::Id) -> &[Self::Id];
    fn spec(&self, id: Self::Id) -> &LayoutSpec;
    /// Changes whenever the node's spec, its list of children or its leaf content change. It need not change when a
    /// descendant does: [`layout_cached`] invalidates the ancestors itself.
    fn generation(&self, id: Self::Id) -> Generation;
    /// The size of a leaf (text, image, icon) under `constraints`.
    fn measure_leaf(&self, id: Self::Id, constraints: Constraints) -> Measured;
}

/// Bumped by the tree's owner whenever a node's spec, children or leaf content change.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Generation(pub u64);

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LayoutOptions {
    pub direction: LayoutDirection,
    pub node_limit: usize,
}

/// Where one node ended up, in its parent's coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NodeLayout {
    pub rect: Rect,
    /// Set when the node clips its children, in the node's own coordinates; it limits painting and hit-testing alike.
    pub clip: Option<Rect>,
    pub baseline: Option<f64>,
}

#[derive(Clone, Debug)]
pub struct LayoutResult<Id> {
    pub nodes: IdMap<Id, NodeLayout>,
    pub stats: LayoutStats,
}

/// How much work one `layout` call did: a node sized under one set of constraints counts once however often it is asked.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LayoutStats {
    /// Cache misses of the sizing step.
    pub solves: usize,
    /// Calls to `LayoutTree::measure_leaf`.
    pub leaf_measures: usize,
    /// Passes in which a stack shared its room among its flexible children: one per solved stack with room to share,
    /// however many children reach a bound.
    pub flex_passes: usize,
}

/// A scroll view's position, kept in the application's model.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScrollState {
    pub offset: Point,
    pub content: Size,
    pub viewport: Size,
}

/// Row heights of a virtualized list.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ItemMetrics {
    Fixed(f64),
    /// Rows not measured yet count as this height.
    Estimated(f64),
}

/// The rows of a virtualized list that exist as nodes: the visible ones plus overscan.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VirtualWindow {
    pub first: usize,
    pub last: usize,
    /// How far to move the offset so the visible rows stay put after rows above them were measured.
    pub anchor_adjust: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_handed_out_in_order_hash_apart_and_an_id_map_finds_each() {
        let hash = |v: u64| {
            let mut h = IdHasher::default();
            h.write_u64(v);
            h.finish()
        };
        // ids made of a slot index and a generation, as node ids are: neither the bucket bits nor the top bits that a
        // map's probe compares collide among a few thousand of them
        let ids: Vec<u64> = (0..4096u64).map(|i| 1 << 32 | i).collect();
        let low: HashSet<u64> = ids.iter().map(|&v| hash(v) & 0xfff).collect();
        let top: HashSet<u64> = ids.iter().map(|&v| hash(v) >> 57).collect();
        assert!(low.len() > 2400, "{} of 4096 low parts", low.len());
        assert_eq!(top.len(), 128, "every top part");
        let map: IdMap<u64, u64> = ids.iter().map(|&v| (v, v * 3)).collect();
        assert!(ids.iter().all(|v| map.get(v) == Some(&(v * 3))));
        assert_eq!(map.len(), ids.len());
    }

    #[test]
    fn an_axis_constraint_is_ordered_and_finite_except_its_max() {
        assert!(AxisConstraint::new(0.0, 10.0, f64::INFINITY).is_ok());
        assert_eq!(
            AxisConstraint::new(5.0, 4.0, 10.0),
            Err(LayoutError::InvalidConstraint)
        );
        assert_eq!(
            AxisConstraint::new(-1.0, 0.0, 1.0),
            Err(LayoutError::InvalidConstraint)
        );
        assert_eq!(
            AxisConstraint::new(f64::NAN, 0.0, 1.0),
            Err(LayoutError::NonFiniteValue)
        );
        assert_eq!(
            AxisConstraint::new(0.0, f64::INFINITY, f64::INFINITY),
            Err(LayoutError::NonFiniteValue)
        );
        assert_eq!(
            AxisConstraint::new(0.0, 0.0, f64::NAN),
            Err(LayoutError::NonFiniteValue)
        );
    }
}
