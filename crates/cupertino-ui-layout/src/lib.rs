//! CupertinoKit layout. A container measures its children under [`Constraints`] and then places them; core hands its
//! node tree in through [`LayoutTree`] and reads back a [`LayoutResult`]. Leaves (text, images, icons) are measured by
//! the tree, so this crate never sees a font. Values are logical points; the scale factor never reaches layout.

use cupertino_geometry::{Insets, Point, Rect, Size};
use std::collections::HashMap;
use std::hash::Hash;

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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Overflow {
    #[default]
    Visible,
    Clip,
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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayoutError {
    /// Flexible space did not settle within 32 redistribution passes.
    NoConvergence,
    InvalidConstraint,
    NonFiniteValue,
    NodeLimit,
    CycleOrMissingNode,
}

/// The node tree layout runs over, borrowed from its owner.
pub trait LayoutTree {
    type Id: Copy + Eq + Hash;

    fn children(&self, id: Self::Id) -> &[Self::Id];
    fn spec(&self, id: Self::Id) -> &LayoutSpec;
    /// Changes whenever the node's spec, children or leaf content change; cached measurements key on it.
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
    /// Set when the node clips its children; it limits painting and hit-testing alike.
    pub clip: Option<Rect>,
    pub baseline: Option<f64>,
}

#[derive(Clone, Debug)]
pub struct LayoutResult<Id> {
    pub nodes: HashMap<Id, NodeLayout>,
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
