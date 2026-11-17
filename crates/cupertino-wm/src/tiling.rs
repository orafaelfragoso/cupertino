//! Tiling kinds, hot zones and tile rectangles.

use crate::config::SnapConfig;
use crate::geometry::{Edges, Point, Rect};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TileKind {
    Left,
    Right,
    Top,
    Bottom,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
    Fill,
}

impl TileKind {
    /// Sides of the tile that lie on the usable-area boundary (the others touch a neighbour tile).
    pub fn boundary_edges(self) -> Edges {
        let (left, right, top, bottom) = match self {
            TileKind::Left => (true, false, true, true),
            TileKind::Right => (false, true, true, true),
            TileKind::Top => (true, true, true, false),
            TileKind::Bottom => (true, true, false, true),
            TileKind::TopLeft => (true, false, true, false),
            TileKind::TopRight => (false, true, true, false),
            TileKind::BottomLeft => (true, false, false, true),
            TileKind::BottomRight => (false, true, false, true),
            TileKind::Fill => (true, true, true, true),
        };
        Edges {
            left,
            right,
            top,
            bottom,
        }
    }
}

/// Rectangle of a tile inside `usable`. With a non-zero `margin` the gap to the area boundary is
/// `margin` and the gap between neighbouring tiles is also `margin`.
pub fn tile_rect(kind: TileKind, usable: Rect, margin: f64) -> Rect {
    let (hw, hh) = (usable.w / 2.0, usable.h / 2.0);
    let (x, y, w, h) = match kind {
        TileKind::Left => (usable.x, usable.y, hw, usable.h),
        TileKind::Right => (usable.x + hw, usable.y, hw, usable.h),
        TileKind::Top => (usable.x, usable.y, usable.w, hh),
        TileKind::Bottom => (usable.x, usable.y + hh, usable.w, hh),
        TileKind::TopLeft => (usable.x, usable.y, hw, hh),
        TileKind::TopRight => (usable.x + hw, usable.y, hw, hh),
        TileKind::BottomLeft => (usable.x, usable.y + hh, hw, hh),
        TileKind::BottomRight => (usable.x + hw, usable.y + hh, hw, hh),
        TileKind::Fill => (usable.x, usable.y, usable.w, usable.h),
    };
    if margin <= 0.0 {
        return Rect::new(x, y, w, h);
    }
    let e = kind.boundary_edges();
    let side = |on_boundary: bool| if on_boundary { margin } else { margin / 2.0 };
    let (l, r, t, b) = (side(e.left), side(e.right), side(e.top), side(e.bottom));
    Rect::new(x + l, y + t, (w - l - r).max(0.0), (h - t - b).max(0.0))
}

/// Result of [`hot_zone_hit`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HotZoneHit {
    pub kind: TileKind,
    /// The option accelerator was used (no dwell).
    pub immediate: bool,
    /// Seconds the pointer must dwell before the preview appears: 0 with Alt; in a corner zone the
    /// soft delay (0.2 s); on a side or the top the hard-zone delay (0 s,
    /// ) at the edge itself and the soft delay within the soft zone (20 pt).
    pub delay: f64,
}

/// Tile selected by the pointer position, if any. `output` is the full output rectangle (hot zones
/// are physical edges) and `usable` its usable area; `immediate` reports that the option
/// accelerator was used (no dwell).
pub fn hot_zone(
    cfg: &SnapConfig,
    output: Rect,
    usable: Rect,
    pointer: Point,
    option_held: bool,
    dock_zone: Option<Rect>,
) -> Option<(TileKind, bool)> {
    hot_zone_hit(cfg, output, usable, pointer, option_held, dock_zone)
        .map(|h| (h.kind, h.immediate))
}

/// Like [`hot_zone`] but also reports the dwell delay of the zone that was hit. The corner zones
/// ([`corner_zones`]) come first, with Alt or without; then the sides and the top.
pub fn hot_zone_hit(
    cfg: &SnapConfig,
    output: Rect,
    usable: Rect,
    pointer: Point,
    option_held: bool,
    dock_zone: Option<Rect>,
) -> Option<HotZoneHit> {
    let immediate = option_held && cfg.option_accelerator;
    if !cfg.edge_drag_tiling && !cfg.top_edge_tiling {
        return None;
    }
    if cfg.avoid_dock_hot_zones && dock_zone.is_some_and(|z| z.contains(pointer)) {
        return None;
    }
    let corner = corner_zones(output, usable, cfg.soft_hot_zone_size)
        .into_iter()
        .find(|(kind, zone)| corner_enabled(cfg, *kind) && zone.contains(pointer))
        .map(|(kind, _)| kind);
    if let Some(kind) = corner {
        return Some(HotZoneHit {
            kind,
            immediate,
            delay: if immediate { 0.0 } else { cfg.edge_dwell },
        });
    }
    if immediate {
        return edge_kind(cfg, output, pointer, cfg.option_edge_extent).map(|kind| HotZoneHit {
            kind,
            immediate,
            delay: 0.0,
        });
    }
    if let Some(kind) = edge_kind(cfg, output, pointer, cfg.edge_threshold) {
        return Some(HotZoneHit {
            kind,
            immediate,
            delay: cfg.hot_zone_delay,
        });
    }
    let soft = cfg.soft_hot_zone_size.max(cfg.edge_threshold);
    edge_kind(cfg, output, pointer, soft).map(|kind| HotZoneHit {
        kind,
        immediate,
        delay: cfg.edge_dwell,
    })
}

/// The four corner zones of `output`: `size` wide at each corner and
/// `size + max(0, usable.y − y)` high, `y` being the zone's top, so the top two reach `size` below
/// the top of the usable area. That the builder's second display rectangle is the usable area is
/// (leaves it unnamed). The extension belongs to the top corners, the builder's extended rows
/// write the top-left and top-right corner positions.
pub fn corner_zones(output: Rect, usable: Rect, size: f64) -> [(TileKind, Rect); 4] {
    let zone = |x: f64, y: f64| Rect::new(x, y, size, size + (usable.y - y).max(0.0));
    let (left, right) = (output.x, output.right() - size);
    let (top, bottom) = (output.y, output.bottom() - size);
    [
        (TileKind::TopLeft, zone(left, top)),
        (TileKind::TopRight, zone(right, top)),
        (TileKind::BottomLeft, zone(left, bottom)),
        (TileKind::BottomRight, zone(right, bottom)),
    ]
}

/// Bottom corners come with side tiling, top corners with side or top tiling.
fn corner_enabled(cfg: &SnapConfig, kind: TileKind) -> bool {
    match kind {
        TileKind::TopLeft | TileKind::TopRight => cfg.edge_drag_tiling || cfg.top_edge_tiling,
        _ => cfg.edge_drag_tiling,
    }
}

/// Half or top tile within `ext` of the left, right or top edge.
fn edge_kind(cfg: &SnapConfig, output: Rect, pointer: Point, ext: f64) -> Option<TileKind> {
    let (dl, dr, dt) = (
        pointer.x - output.x,
        output.right() - pointer.x,
        pointer.y - output.y,
    );
    let side = cfg.edge_drag_tiling;
    if side && dl <= ext {
        Some(TileKind::Left)
    } else if side && dr <= ext {
        Some(TileKind::Right)
    } else if cfg.top_edge_tiling && dt <= ext {
        Some(cfg.top_edge_action)
    } else {
        None
    }
}

/// Whether the pointer at `pointer` still counts as resting at `anchor` in the zone of `kind`
/// within `tolerance`, inclusive (a NaN distance is not), measured as the
/// Euclidean distance in a corner zone, as |dy| on the left and right sides and as |dx| on the top.
/// The design's position codes 0–3, 4–5 and 6–8; that 6–8 are the top-edge tiles is.
pub fn is_still(kind: TileKind, anchor: Point, pointer: Point, tolerance: f64) -> bool {
    let (dx, dy) = (pointer.x - anchor.x, pointer.y - anchor.y);
    let d = match kind {
        TileKind::TopLeft | TileKind::TopRight | TileKind::BottomLeft | TileKind::BottomRight => {
            dx.hypot(dy)
        }
        TileKind::Left | TileKind::Right => dy.abs(),
        TileKind::Top | TileKind::Bottom | TileKind::Fill => dx.abs(),
    };
    tolerance >= d
}

/// Frame of the tiling preview for a tile at `tile`: outset by `outset` on every side
/// (15 pt, the frame inset by −15 on both axes).
pub fn preview_frame(tile: Rect, outset: f64) -> Rect {
    tile.inset(-outset)
}
