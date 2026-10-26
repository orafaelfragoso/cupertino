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
        Edges { left, right, top, bottom }
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
    hot_zone_hit(cfg, output, usable, pointer, option_held, dock_zone).map(|h| (h.kind, h.immediate))
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
        return Some(HotZoneHit { kind, immediate, delay: if immediate { 0.0 } else { cfg.edge_dwell } });
    }
    if immediate {
        return edge_kind(cfg, output, pointer, cfg.option_edge_extent).map(|kind| HotZoneHit { kind, immediate, delay: 0.0 });
    }
    if let Some(kind) = edge_kind(cfg, output, pointer, cfg.edge_threshold) {
        return Some(HotZoneHit { kind, immediate, delay: cfg.hot_zone_delay });
    }
    let soft = cfg.soft_hot_zone_size.max(cfg.edge_threshold);
    edge_kind(cfg, output, pointer, soft).map(|kind| HotZoneHit { kind, immediate, delay: cfg.edge_dwell })
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
    let (dl, dr, dt) = (pointer.x - output.x, output.right() - pointer.x, pointer.y - output.y);
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
        TileKind::TopLeft | TileKind::TopRight | TileKind::BottomLeft | TileKind::BottomRight => dx.hypot(dy),
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

#[cfg(test)]
mod tests {
    use super::*;
    use cupertino_animation::{AnimatedValue, Curve};

    const OUT: Rect = Rect::new(0.0, 0.0, 1920.0, 1080.0);
    const USABLE: Rect = Rect::new(0.0, 24.0, 1920.0, 1056.0);

    fn zone(p: Point, opt: bool) -> Option<TileKind> {
        hot_zone(&SnapConfig::default(), OUT, USABLE, p, opt, None).map(|z| z.0)
    }

    #[test]
    fn edges_corners_and_top() {
        assert_eq!(zone(Point::new(0.0, 500.0), false), Some(TileKind::Left));
        assert_eq!(zone(Point::new(1919.5, 500.0), false), Some(TileKind::Right));
        assert_eq!(zone(Point::new(960.0, 0.0), false), Some(TileKind::Fill));
        assert_eq!(zone(Point::new(0.0, 10.0), false), Some(TileKind::TopLeft));
        assert_eq!(zone(Point::new(10.0, 0.0), false), Some(TileKind::TopLeft));
        assert_eq!(zone(Point::new(1919.0, 1075.0), false), Some(TileKind::BottomRight));
        assert_eq!(zone(Point::new(960.0, 1079.0), false), None);
        assert_eq!(zone(Point::new(500.0, 500.0), false), None);
        assert_eq!(zone(Point::new(30.0, 500.0), false), None);
    }

    #[test]
    fn soft_zone_has_dwell_and_edge_has_none() {
        let cfg = SnapConfig::default();
        let edge = hot_zone_hit(&cfg, OUT, USABLE, Point::new(0.0, 500.0), false, None).unwrap();
        assert_eq!((edge.kind, edge.delay), (TileKind::Left, 0.0));
        let soft = hot_zone_hit(&cfg, OUT, USABLE, Point::new(15.0, 500.0), false, None).unwrap();
        assert_eq!((soft.kind, soft.delay), (TileKind::Left, 0.2));
        assert_eq!(zone(Point::new(20.0, 500.0), false), Some(TileKind::Left));
        assert_eq!(zone(Point::new(21.0, 500.0), false), None);
        let opt = hot_zone_hit(&cfg, OUT, USABLE, Point::new(60.0, 500.0), true, None).unwrap();
        assert!(opt.immediate && opt.delay == 0.0);
    }

    #[test]
    fn verified_defaults() {
        let c = SnapConfig::default();
        assert_eq!((c.tiled_margin, c.soft_hot_zone_size, c.edge_dwell, c.hot_zone_delay), (8.0, 20.0, 0.2, 0.0));
        assert!(!c.avoid_dock_hot_zones && !c.show_tiled_border && c.order_preview_above_target);
    }

    #[test]
    fn option_accelerator_widens_zone_and_is_immediate() {
        assert_eq!(zone(Point::new(60.0, 500.0), true), Some(TileKind::Left));
        let z = hot_zone(&SnapConfig::default(), OUT, USABLE, Point::new(60.0, 500.0), true, None).unwrap();
        assert!(z.1);
        let cfg = SnapConfig { option_accelerator: false, ..Default::default() };
        assert!(hot_zone(&cfg, OUT, USABLE, Point::new(60.0, 500.0), true, None).is_none());
    }

    #[test]
    fn toggles_and_dock_zone() {
        let cfg = SnapConfig { top_edge_tiling: false, ..Default::default() };
        assert_eq!(hot_zone(&cfg, OUT, USABLE, Point::new(960.0, 0.0), false, None), None);
        let dz = Rect::new(0.0, 900.0, 100.0, 180.0);
        assert!(hot_zone(&SnapConfig::default(), OUT, USABLE, Point::new(0.0, 1000.0), false, Some(dz)).is_some());
        let cfg = SnapConfig { avoid_dock_hot_zones: true, ..Default::default() };
        assert_eq!(hot_zone(&cfg, OUT, USABLE, Point::new(0.0, 1000.0), false, Some(dz)), None);
    }

    #[test]
    fn corner_zones_are_20_pt_and_the_top_ones_reach_below_the_usable_top() {
        assert_eq!(
            corner_zones(OUT, USABLE, SnapConfig::default().soft_hot_zone_size),
            [
                (TileKind::TopLeft, Rect::new(0.0, 0.0, 20.0, 44.0)),
                (TileKind::TopRight, Rect::new(1900.0, 0.0, 20.0, 44.0)),
                (TileKind::BottomLeft, Rect::new(0.0, 1060.0, 20.0, 20.0)),
                (TileKind::BottomRight, Rect::new(1900.0, 1060.0, 20.0, 20.0)),
            ]
        );
    }

    #[test]
    fn a_corner_waits_the_soft_delay_and_is_immediate_with_alt() {
        let cfg = SnapConfig::default();
        let hit = |p: Point, alt: bool| hot_zone_hit(&cfg, OUT, USABLE, p, alt, None).map(|h| (h.kind, h.delay));
        assert_eq!(hit(Point::new(0.0, 0.0), false), Some((TileKind::TopLeft, 0.2)), "even on the corner pixel");
        assert_eq!(hit(Point::new(19.9, 43.9), false), Some((TileKind::TopLeft, 0.2)));
        assert_eq!(hit(Point::new(20.0, 43.9), false), Some((TileKind::Left, 0.2)), "past the zone: the soft side zone");
        assert_eq!(hit(Point::new(5.0, 44.0), false), Some((TileKind::Left, 0.2)));
        assert_eq!(hit(Point::new(1910.0, 1070.0), false), Some((TileKind::BottomRight, 0.2)));
        assert_eq!(hit(Point::new(10.0, 10.0), true), Some((TileKind::TopLeft, 0.0)));
        assert_eq!(hit(Point::new(30.0, 30.0), true), Some((TileKind::Left, 0.0)), "the Alt corner is 20 pt too");
    }

    #[test]
    fn stillness_is_5_pt_inclusive_with_the_zone_s_metric() {
        let corner = Point::new(10.0, 10.0);
        assert!(is_still(TileKind::TopLeft, corner, Point::new(13.0, 14.0), 5.0), "Euclidean 5.0");
        assert!(!is_still(TileKind::TopLeft, corner, Point::new(10.0, 15.01), 5.0));
        let side = Point::new(0.0, 500.0);
        assert!(is_still(TileKind::Left, side, Point::new(15.0, 505.0), 5.0), "|dy| 5.0, dx ignored");
        assert!(!is_still(TileKind::Right, side, Point::new(0.0, 494.99), 5.0));
        let top = Point::new(960.0, 0.0);
        assert!(is_still(TileKind::Fill, top, Point::new(965.0, 15.0), 5.0), "|dx| 5.0, dy ignored");
        assert!(!is_still(TileKind::Fill, top, Point::new(965.01, 0.0), 5.0));
        assert!(!is_still(TileKind::Left, side, Point::new(0.0, f64::NAN), 5.0), "a NaN distance is not still");
    }

    #[test]
    fn the_preview_frame_is_the_tile_outset_by_15_pt() {
        let tile = tile_rect(TileKind::Left, USABLE, 8.0);
        assert_eq!(tile, Rect::new(8.0, 32.0, 948.0, 1040.0));
        assert_eq!(preview_frame(tile, SnapConfig::default().preview_outset), Rect::new(-7.0, 17.0, 978.0, 1070.0));
    }

    #[test]
    fn the_preview_fades_on_its_own_critically_damped_spring() {
        let Curve::Spring(p) = SnapConfig::default().preview_opacity_curve else { panic!("the fade is a spring") };
        let round = |v: f64, places: i32| (v * 10f64.powi(places)).round() / 10f64.powi(places);
        assert_eq!((p.mass, round(p.stiffness, 3), round(p.damping, 3)), (1.0, 767.492, 55.407), "(2 pi / 0.2268)^2 and 2 sqrt(k)");
        let mut opacity = AnimatedValue::new(0.0);
        opacity.animate_to(0.5, Curve::Spring(p), 0.0);
        let at = |t: f64| round(opacity.value(t), 5);
        assert_eq!((at(0.05), at(0.1), at(0.2)), (0.20152, 0.38191, 0.48717));
    }

    #[test]
    fn rects_without_margin_partition_the_area() {
        let l = tile_rect(TileKind::Left, USABLE, 0.0);
        let r = tile_rect(TileKind::Right, USABLE, 0.0);
        assert_eq!(l.right(), r.x);
        assert_eq!(l.union(&r), USABLE);
        let q: Vec<Rect> = [TileKind::TopLeft, TileKind::TopRight, TileKind::BottomLeft, TileKind::BottomRight]
            .iter()
            .map(|k| tile_rect(*k, USABLE, 0.0))
            .collect();
        let area: f64 = q.iter().map(|r| r.w * r.h).sum();
        assert!((area - USABLE.w * USABLE.h).abs() < 1e-9);
        assert_eq!(tile_rect(TileKind::Fill, USABLE, 0.0), USABLE);
    }

    #[test]
    fn margins_give_uniform_gaps() {
        let m = 10.0;
        let l = tile_rect(TileKind::Left, USABLE, m);
        let r = tile_rect(TileKind::Right, USABLE, m);
        assert_eq!(l.x - USABLE.x, m);
        assert_eq!(USABLE.right() - r.right(), m);
        assert!((r.x - l.right() - m).abs() < 1e-9);
        assert_eq!(tile_rect(TileKind::Fill, USABLE, m), USABLE.inset(m));
        let tl = tile_rect(TileKind::TopLeft, USABLE, m);
        let bl = tile_rect(TileKind::BottomLeft, USABLE, m);
        assert!((bl.y - tl.bottom() - m).abs() < 1e-9);
    }
}
