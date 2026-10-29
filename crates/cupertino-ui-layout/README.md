# cupertino-ui-layout

CupertinoKit layout: constraints, two-pass measure and place over a tree borrowed through `LayoutTree`, stacks, grids,
absolute and overlay containers, scroll math and virtualization windows. MPL-2.0. Depends only on `cupertino-geometry`:
no display, font or scale factor. Values are logical points; `proptest` is a dev-dependency only.

## Use

```rust
let result = cupertino_ui_layout::layout(&tree, root, Constraints::loose(window_size)?, &options)?;
let node = result.nodes[&id]; // rect in the parent's coordinates, clip in the node's own, baseline from its top
```

Leaves (text, images, icons) are measured by `LayoutTree::measure_leaf`; the engine clamps the answer into the
constraints. Text wraps because a vertical stack hands a leaf the width it has; in a horizontal stack a leaf starts from
its unconstrained width and shrinks.

## Behaviour

- **Sizing.** `SizeSpec::Fixed` and `Fraction` (of the offered maximum) keep their size and are never stretched; `Fill`
  takes the offered maximum when it is finite and its content size otherwise; `Fit` takes its content size. A node never
  leaves the range its parent offers.
- **Stack.** A child starts at its `flex.basis`, its fixed size, a fraction, or its unconstrained size. Leftover room is
  shared by `grow`, a shortfall by `shrink` times basis, in passes that freeze children at their bounds (each pass freezes at least one, so
  `children + 1` passes settle; `NoConvergence` guards that). A `Fit` stack does not grow past its content. A shrinking child stops at its smallest size: for a leaf what
  `measure_leaf` returns when the axis maximum is zero (text answers with its longest word), for a container the sum or
  largest of its children's, and its fixed size when it has one; children whose floors exceed the room overflow.
  Cross-axis `Align`; `Align::Baseline` lines up first baselines in horizontal stacks and acts as `Start` in vertical
  ones, where the stack's own baseline is its first child's.
- **Grid.** Fixed, auto and fraction tracks; children fill cells row by row, skipping cells a spanning child covers
  (`LayoutSpec::span`, columns and rows; a span wider than the grid is cut to it). A spanning child that needs more than
  its tracks give widens its auto tracks equally (its fraction tracks when it covers none). Fraction tracks share the leftover room when
  the grid has a definite size, otherwise they are as large as their biggest content needs.
- **Overlay / Absolute.** Children share one box. Overlay children follow `align_self` and stretch by default; absolute
  children keep their size at `LayoutSpec::position` and the container grows to hold them.
- **Scroll.** Content is measured without a limit along the scrolled axes and stretches across the others. The content
  rect keeps its full size at the origin and the node's `clip` is the viewport; the offset belongs to the model, so core
  applies it with `ScrollState` (`clamped`, `scrolled_by`, `page_step`) when painting and hit-testing.
- **Right to left.** Children are mirrored inside their parent's box, so start-aligned content follows the reading
  direction. Insets are physical and do not mirror. A horizontal scroll view in right-to-left starts at
  its right edge: the content rect is mirrored (negative `x` when wider than the viewport), so core measures the scroll
  offset from the start edge and negates it for painting and hit-testing.
- **VirtualList.** Row windowing for 50k-row lists: constant time for fixed heights, `O(log n)` and 16 bytes per row for
  estimated ones, with `anchor_adjust` when rows above the top row are measured. `with_spacing` puts a gap between rows
  (none after the last); an offset inside a gap belongs to the row above. `first..last` is half-open.
- **Cache.** `layout_cached` keeps sizing results across calls in a `LayoutCache`. A node's entries are dropped when its
  `LayoutTree::generation` changed or any descendant's did, and when it left the tree or the direction changed; a cache
  past 16 entries per node is cleared. Output equals an uncached call. Placing every node and building the result still
  costs `O(nodes)`; a warm call with no changes does no sizing or leaf measuring (the bench shows about 3.5 times less
  time).
- **Stats.** `LayoutResult::stats` counts the sizing cache misses and leaf measurements of the call; `cargo bench -p
  cupertino-ui-layout` prints them next to the time for 1k and 10k node trees.
- **Errors.** Non-finite or negative spec values, a node reached twice (a cycle), more than `node_limit` nodes or more
  than 256 levels return a `LayoutError`; leaf measurements that are not finite do too. The engine does not panic on them.

## Known limitations

- A grid child cannot be placed in a chosen cell; placement is automatic. Spanning children are left out of a grid's
  smallest size, and a span's extra width goes to auto tracks equally rather than by need.
- The smallest size of a container ignores what wrapping text would need once it is narrower than the width it was asked
  at: vertical queries use the width given, horizontal ones are asked without a height limit.
- A generation must change whenever the node's spec, its list of children or its leaf content change; a leaf whose
  measurement depends on something else (a font, the scale factor) needs its generation bumped when that changes.
- `LayoutCache` bounds itself by clearing, not by evicting the least recently used entry.
