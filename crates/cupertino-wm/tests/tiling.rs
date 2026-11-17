//! Tile rectangles, hot zones and the dwell stillness test.

use cupertino_wm::tiling::{corner_zones, is_still, preview_frame};
use cupertino_wm::{hot_zone, hot_zone_hit, tile_rect, Point, Rect, SnapConfig, TileKind};

const OUTPUT: Rect = Rect::new(0.0, 0.0, 1920.0, 1080.0);
const USABLE: Rect = Rect::new(0.0, 32.0, 1920.0, 1048.0);

const ALL: [TileKind; 9] = [
    TileKind::Left,
    TileKind::Right,
    TileKind::Top,
    TileKind::Bottom,
    TileKind::TopLeft,
    TileKind::TopRight,
    TileKind::BottomLeft,
    TileKind::BottomRight,
    TileKind::Fill,
];

#[test]
fn tiles_without_a_margin_split_the_usable_area_exactly() {
    for (kind, rect) in [
        (TileKind::Fill, USABLE),
        (TileKind::Left, Rect::new(0.0, 32.0, 960.0, 1048.0)),
        (TileKind::Right, Rect::new(960.0, 32.0, 960.0, 1048.0)),
        (TileKind::Top, Rect::new(0.0, 32.0, 1920.0, 524.0)),
        (TileKind::Bottom, Rect::new(0.0, 556.0, 1920.0, 524.0)),
        (TileKind::TopLeft, Rect::new(0.0, 32.0, 960.0, 524.0)),
        (TileKind::TopRight, Rect::new(960.0, 32.0, 960.0, 524.0)),
        (TileKind::BottomLeft, Rect::new(0.0, 556.0, 960.0, 524.0)),
        (TileKind::BottomRight, Rect::new(960.0, 556.0, 960.0, 524.0)),
    ] {
        assert_eq!(tile_rect(kind, USABLE, 0.0), rect, "{kind:?}");
    }
}

#[test]
fn a_margin_is_the_gap_to_the_boundary_and_between_neighbours_and_never_goes_negative() {
    let m = 8.0;
    let left = tile_rect(TileKind::Left, USABLE, m);
    let right = tile_rect(TileKind::Right, USABLE, m);
    assert_eq!(left.x - USABLE.x, m);
    assert_eq!(USABLE.right() - right.right(), m);
    assert_eq!(right.x - left.right(), m, "between neighbours");
    assert_eq!(left.y - USABLE.y, m);
    assert_eq!(USABLE.bottom() - left.bottom(), m);
    assert_eq!(
        tile_rect(TileKind::Fill, USABLE, m),
        Rect::new(8.0, 40.0, 1904.0, 1032.0)
    );
    let tiny = Rect::new(0.0, 0.0, 10.0, 10.0);
    for k in ALL {
        let r = tile_rect(k, USABLE, m);
        assert!(r.x >= USABLE.x && r.y >= USABLE.y, "{k:?}");
        assert!(
            r.right() <= USABLE.right() && r.bottom() <= USABLE.bottom(),
            "{k:?}"
        );
        let collapsed = tile_rect(k, tiny, 40.0);
        assert!(
            collapsed.w >= 0.0 && collapsed.h >= 0.0,
            "{k:?}: {collapsed:?}"
        );
    }
}

#[test]
fn boundary_edges_say_which_sides_touch_the_screen() {
    for (kind, (left, right, top, bottom)) in [
        (TileKind::TopLeft, (true, false, true, false)),
        (TileKind::Fill, (true, true, true, true)),
        (TileKind::Right, (false, true, true, true)),
    ] {
        let e = kind.boundary_edges();
        assert_eq!(
            (e.left, e.right, e.top, e.bottom),
            (left, right, top, bottom),
            "{kind:?}"
        );
    }
}

#[test]
fn hot_zones_select_a_tile_with_the_delay_of_their_zone() {
    let cfg = SnapConfig::default();
    // (point, option held, kind, immediate, delay); None is no zone.
    for (name, point, option, expected) in [
        (
            "left edge",
            Point::new(0.0, 500.0),
            false,
            Some((TileKind::Left, false, cfg.hot_zone_delay)),
        ),
        (
            "right edge",
            Point::new(1920.0, 500.0),
            false,
            Some((TileKind::Right, false, cfg.hot_zone_delay)),
        ),
        (
            "soft zone",
            Point::new(15.0, 500.0),
            false,
            Some((TileKind::Left, false, cfg.edge_dwell)),
        ),
        ("past the soft zone", Point::new(25.0, 500.0), false, None),
        (
            "bottom left corner",
            Point::new(5.0, 1075.0),
            false,
            Some((TileKind::BottomLeft, false, cfg.edge_dwell)),
        ),
        (
            "top right corner with option",
            Point::new(1915.0, 5.0),
            true,
            Some((TileKind::TopRight, true, 0.0)),
        ),
        (
            "wide zone without option",
            Point::new(80.0, 500.0),
            false,
            None,
        ),
        (
            "option widens the edge zone",
            Point::new(80.0, 500.0),
            true,
            Some((TileKind::Left, true, 0.0)),
        ),
        ("middle", Point::new(960.0, 540.0), false, None),
        ("middle with option", Point::new(960.0, 540.0), true, None),
    ] {
        let hit = hot_zone_hit(&cfg, OUTPUT, USABLE, point, option, None)
            .map(|h| (h.kind, h.immediate, h.delay));
        assert_eq!(hit, expected, "{name}");
    }
}

#[test]
fn settings_shape_the_hot_zones() {
    let top = Point::new(960.0, 0.0);
    let left = Point::new(0.0, 500.0);
    let wide = Point::new(80.0, 500.0);
    let bottom_corner = Point::new(5.0, 1075.0);
    let top_corner = Point::new(5.0, 5.0);
    let top_action = SnapConfig {
        top_edge_action: TileKind::Top,
        ..SnapConfig::default()
    };
    let no_accelerator = SnapConfig {
        option_accelerator: false,
        ..SnapConfig::default()
    };
    let no_sides = SnapConfig {
        edge_drag_tiling: false,
        ..SnapConfig::default()
    };
    let nothing = SnapConfig {
        edge_drag_tiling: false,
        top_edge_tiling: false,
        ..SnapConfig::default()
    };
    for (name, cfg, point, option, expected) in [
        (
            "the top edge fills",
            SnapConfig::default(),
            top,
            false,
            Some(TileKind::Fill),
        ),
        (
            "the top edge's action is a setting",
            top_action,
            top,
            false,
            Some(TileKind::Top),
        ),
        (
            "the accelerator is a setting",
            no_accelerator,
            wide,
            true,
            None,
        ),
        (
            "no side tiling: no side",
            no_sides.clone(),
            left,
            false,
            None,
        ),
        (
            "no side tiling: no bottom corner",
            no_sides.clone(),
            bottom_corner,
            false,
            None,
        ),
        (
            "no side tiling: top corners stay",
            no_sides.clone(),
            top_corner,
            false,
            Some(TileKind::TopLeft),
        ),
        (
            "no side tiling: the top edge stays",
            no_sides,
            top,
            false,
            Some(TileKind::Fill),
        ),
        ("no tiling: no side", nothing.clone(), left, true, None),
        (
            "no tiling: no corner",
            nothing.clone(),
            top_corner,
            true,
            None,
        ),
        ("no tiling: no top", nothing, top, true, None),
    ] {
        assert_eq!(
            hot_zone(&cfg, OUTPUT, USABLE, point, option, None).map(|h| h.0),
            expected,
            "{name}"
        );
    }
}

#[test]
fn the_dock_zone_can_suppress_hot_zones() {
    let mut cfg = SnapConfig::default();
    let dock = Rect::new(0.0, 400.0, 80.0, 300.0);
    let p = Point::new(0.0, 500.0);
    assert!(hot_zone(&cfg, OUTPUT, USABLE, p, false, Some(dock)).is_some());
    cfg.avoid_dock_hot_zones = true;
    assert!(hot_zone(&cfg, OUTPUT, USABLE, p, false, Some(dock)).is_none());
    assert!(hot_zone(&cfg, OUTPUT, USABLE, p, false, None).is_some());
}

#[test]
fn the_top_corner_zones_reach_down_to_the_usable_area() {
    let zones = corner_zones(OUTPUT, USABLE, 20.0);
    for (kind, rect) in [
        // 20 below the menu bar: 32 + 20.
        (TileKind::TopLeft, Rect::new(0.0, 0.0, 20.0, 52.0)),
        (TileKind::BottomLeft, Rect::new(0.0, 1060.0, 20.0, 20.0)),
        (TileKind::BottomRight, Rect::new(1900.0, 1060.0, 20.0, 20.0)),
    ] {
        let zone = zones.iter().find(|(k, _)| *k == kind).unwrap().1;
        assert_eq!(zone, rect, "{kind:?}");
    }
}

#[test]
fn stillness_is_measured_per_zone_and_is_inclusive() {
    let a = Point::new(100.0, 100.0);
    for (name, kind, to, still) in [
        (
            "a side ignores movement across the edge",
            TileKind::Left,
            Point::new(160.0, 105.0),
            true,
        ),
        (
            "a side counts movement along the edge",
            TileKind::Left,
            Point::new(100.0, 106.0),
            false,
        ),
        (
            "the top ignores vertical movement",
            TileKind::Fill,
            Point::new(105.0, 400.0),
            true,
        ),
        (
            "the top counts horizontal movement",
            TileKind::Top,
            Point::new(106.0, 100.0),
            false,
        ),
        (
            "a corner inside the distance",
            TileKind::TopLeft,
            Point::new(103.0, 104.0),
            true,
        ),
        (
            "a corner past the distance",
            TileKind::TopLeft,
            Point::new(104.0, 104.0),
            false,
        ),
        (
            "not a number is never still",
            TileKind::TopLeft,
            Point::new(f64::NAN, 100.0),
            false,
        ),
    ] {
        assert_eq!(is_still(kind, a, to, 5.0), still, "{name}");
    }
}

#[test]
fn the_preview_frame_is_the_tile_grown_on_every_side() {
    let t = Rect::new(100.0, 100.0, 400.0, 300.0);
    assert_eq!(preview_frame(t, 15.0), Rect::new(85.0, 85.0, 430.0, 330.0));
    assert_eq!(preview_frame(t, 0.0), t);
}
