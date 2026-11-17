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
    assert_eq!(tile_rect(TileKind::Fill, USABLE, 0.0), USABLE);
    assert_eq!(
        tile_rect(TileKind::Left, USABLE, 0.0),
        Rect::new(0.0, 32.0, 960.0, 1048.0)
    );
    assert_eq!(
        tile_rect(TileKind::Right, USABLE, 0.0),
        Rect::new(960.0, 32.0, 960.0, 1048.0)
    );
    assert_eq!(
        tile_rect(TileKind::Top, USABLE, 0.0),
        Rect::new(0.0, 32.0, 1920.0, 524.0)
    );
    assert_eq!(
        tile_rect(TileKind::Bottom, USABLE, 0.0),
        Rect::new(0.0, 556.0, 1920.0, 524.0)
    );
    assert_eq!(
        tile_rect(TileKind::TopLeft, USABLE, 0.0),
        Rect::new(0.0, 32.0, 960.0, 524.0)
    );
    assert_eq!(
        tile_rect(TileKind::BottomRight, USABLE, 0.0),
        Rect::new(960.0, 556.0, 960.0, 524.0)
    );
}

#[test]
fn quarters_cover_the_usable_area_without_overlap() {
    let q = [
        TileKind::TopLeft,
        TileKind::TopRight,
        TileKind::BottomLeft,
        TileKind::BottomRight,
    ]
    .map(|k| tile_rect(k, USABLE, 0.0));
    let area: f64 = q.iter().map(|r| r.w * r.h).sum();
    assert_eq!(area, USABLE.w * USABLE.h);
    for (i, a) in q.iter().enumerate() {
        for b in &q[i + 1..] {
            let ox = (a.right().min(b.right()) - a.x.max(b.x)).max(0.0);
            let oy = (a.bottom().min(b.bottom()) - a.y.max(b.y)).max(0.0);
            assert_eq!(ox * oy, 0.0, "{a:?} overlaps {b:?}");
        }
    }
}

#[test]
fn a_margin_is_the_gap_to_the_boundary_and_the_gap_between_neighbours() {
    let m = 8.0;
    let left = tile_rect(TileKind::Left, USABLE, m);
    let right = tile_rect(TileKind::Right, USABLE, m);
    assert_eq!(left.x - USABLE.x, m);
    assert_eq!(USABLE.right() - right.right(), m);
    assert_eq!(
        right.x - left.right(),
        m,
        "the gap between neighbours is the margin as well"
    );
    assert_eq!(left.y - USABLE.y, m);
    assert_eq!(USABLE.bottom() - left.bottom(), m);
    let fill = tile_rect(TileKind::Fill, USABLE, m);
    assert_eq!(fill, Rect::new(8.0, 40.0, 1904.0, 1032.0));
}

#[test]
fn a_margin_larger_than_the_tile_collapses_it_instead_of_going_negative() {
    let tiny = Rect::new(0.0, 0.0, 10.0, 10.0);
    for k in ALL {
        let r = tile_rect(k, tiny, 40.0);
        assert!(r.w >= 0.0 && r.h >= 0.0, "{k:?}: {r:?}");
    }
}

#[test]
fn every_tile_stays_inside_the_usable_area() {
    for k in ALL {
        for m in [0.0, 8.0] {
            let r = tile_rect(k, USABLE, m);
            assert!(r.x >= USABLE.x && r.y >= USABLE.y, "{k:?}");
            assert!(
                r.right() <= USABLE.right() && r.bottom() <= USABLE.bottom(),
                "{k:?}"
            );
        }
    }
}

#[test]
fn boundary_edges_say_which_sides_touch_the_screen() {
    let e = TileKind::TopLeft.boundary_edges();
    assert!(e.left && e.top && !e.right && !e.bottom);
    let e = TileKind::Fill.boundary_edges();
    assert!(e.left && e.right && e.top && e.bottom);
    let e = TileKind::Right.boundary_edges();
    assert!(!e.left && e.right && e.top && e.bottom);
}

#[test]
fn the_pointer_at_the_left_or_right_edge_selects_a_half_without_delay() {
    let cfg = SnapConfig::default();
    let mid_y = 500.0;
    let hit = hot_zone_hit(&cfg, OUTPUT, USABLE, Point::new(0.0, mid_y), false, None).unwrap();
    assert_eq!(
        (hit.kind, hit.immediate, hit.delay),
        (TileKind::Left, false, cfg.hot_zone_delay)
    );
    let hit = hot_zone_hit(&cfg, OUTPUT, USABLE, Point::new(1920.0, mid_y), false, None).unwrap();
    assert_eq!(hit.kind, TileKind::Right);
}

#[test]
fn the_soft_zone_selects_the_same_half_after_a_dwell() {
    let cfg = SnapConfig::default();
    let hit = hot_zone_hit(&cfg, OUTPUT, USABLE, Point::new(15.0, 500.0), false, None).unwrap();
    assert_eq!(hit.kind, TileKind::Left);
    assert_eq!(hit.delay, cfg.edge_dwell);
    assert!(hot_zone_hit(&cfg, OUTPUT, USABLE, Point::new(25.0, 500.0), false, None).is_none());
}

#[test]
fn the_top_edge_applies_the_configured_action() {
    let mut cfg = SnapConfig::default();
    let p = Point::new(960.0, 0.0);
    assert_eq!(
        hot_zone(&cfg, OUTPUT, USABLE, p, false, None),
        Some((TileKind::Fill, false))
    );
    cfg.top_edge_action = TileKind::Top;
    assert_eq!(
        hot_zone(&cfg, OUTPUT, USABLE, p, false, None),
        Some((TileKind::Top, false))
    );
}

#[test]
fn corner_zones_select_quarters_with_the_soft_delay_unless_option_is_held() {
    let cfg = SnapConfig::default();
    let hit = hot_zone_hit(&cfg, OUTPUT, USABLE, Point::new(5.0, 1075.0), false, None).unwrap();
    assert_eq!(
        (hit.kind, hit.delay),
        (TileKind::BottomLeft, cfg.edge_dwell)
    );
    let hit = hot_zone_hit(&cfg, OUTPUT, USABLE, Point::new(1915.0, 5.0), true, None).unwrap();
    assert_eq!(
        (hit.kind, hit.immediate, hit.delay),
        (TileKind::TopRight, true, 0.0)
    );
}

#[test]
fn option_widens_the_edge_zones_and_removes_the_dwell() {
    let cfg = SnapConfig::default();
    let p = Point::new(80.0, 500.0);
    assert!(hot_zone_hit(&cfg, OUTPUT, USABLE, p, false, None).is_none());
    let hit = hot_zone_hit(&cfg, OUTPUT, USABLE, p, true, None).unwrap();
    assert_eq!(
        (hit.kind, hit.immediate, hit.delay),
        (TileKind::Left, true, 0.0)
    );
    let off = SnapConfig {
        option_accelerator: false,
        ..SnapConfig::default()
    };
    assert!(
        hot_zone_hit(&off, OUTPUT, USABLE, p, true, None).is_none(),
        "the accelerator is a setting"
    );
}

#[test]
fn the_middle_of_the_screen_selects_nothing() {
    let cfg = SnapConfig::default();
    assert!(hot_zone(&cfg, OUTPUT, USABLE, Point::new(960.0, 540.0), false, None).is_none());
    assert!(hot_zone(&cfg, OUTPUT, USABLE, Point::new(960.0, 540.0), true, None).is_none());
}

#[test]
fn disabling_edge_and_top_tiling_disables_every_zone() {
    let cfg = SnapConfig {
        edge_drag_tiling: false,
        top_edge_tiling: false,
        ..SnapConfig::default()
    };
    for p in [
        Point::new(0.0, 500.0),
        Point::new(5.0, 5.0),
        Point::new(960.0, 0.0),
    ] {
        assert!(hot_zone_hit(&cfg, OUTPUT, USABLE, p, true, None).is_none());
    }
}

#[test]
fn without_side_tiling_only_the_top_corners_and_top_edge_remain() {
    let cfg = SnapConfig {
        edge_drag_tiling: false,
        ..SnapConfig::default()
    };
    assert!(hot_zone(&cfg, OUTPUT, USABLE, Point::new(0.0, 500.0), false, None).is_none());
    assert!(
        hot_zone(&cfg, OUTPUT, USABLE, Point::new(5.0, 1075.0), false, None).is_none(),
        "no bottom corners"
    );
    assert_eq!(
        hot_zone(&cfg, OUTPUT, USABLE, Point::new(5.0, 5.0), false, None).map(|h| h.0),
        Some(TileKind::TopLeft)
    );
    assert_eq!(
        hot_zone(&cfg, OUTPUT, USABLE, Point::new(960.0, 0.0), false, None).map(|h| h.0),
        Some(TileKind::Fill)
    );
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
    let tl = zones
        .iter()
        .find(|(k, _)| *k == TileKind::TopLeft)
        .unwrap()
        .1;
    assert_eq!(
        tl,
        Rect::new(0.0, 0.0, 20.0, 52.0),
        "20 below the menu bar, 32 + 20"
    );
    let bl = zones
        .iter()
        .find(|(k, _)| *k == TileKind::BottomLeft)
        .unwrap()
        .1;
    assert_eq!(bl, Rect::new(0.0, 1060.0, 20.0, 20.0));
    let br = zones
        .iter()
        .find(|(k, _)| *k == TileKind::BottomRight)
        .unwrap()
        .1;
    assert_eq!(br, Rect::new(1900.0, 1060.0, 20.0, 20.0));
}

#[test]
fn stillness_is_measured_per_zone_and_is_inclusive() {
    let a = Point::new(100.0, 100.0);
    // A side zone only cares about movement along the edge.
    assert!(is_still(TileKind::Left, a, Point::new(160.0, 105.0), 5.0));
    assert!(!is_still(TileKind::Left, a, Point::new(100.0, 106.0), 5.0));
    // The top cares about horizontal movement only.
    assert!(is_still(TileKind::Fill, a, Point::new(105.0, 400.0), 5.0));
    assert!(!is_still(TileKind::Top, a, Point::new(106.0, 100.0), 5.0));
    // A corner measures the straight distance.
    assert!(is_still(
        TileKind::TopLeft,
        a,
        Point::new(103.0, 104.0),
        5.0
    ));
    assert!(!is_still(
        TileKind::TopLeft,
        a,
        Point::new(104.0, 104.0),
        5.0
    ));
    assert!(
        !is_still(TileKind::TopLeft, a, Point::new(f64::NAN, 100.0), 5.0),
        "NaN is never still"
    );
}

#[test]
fn the_preview_frame_is_the_tile_grown_on_every_side() {
    let t = Rect::new(100.0, 100.0, 400.0, 300.0);
    assert_eq!(preview_frame(t, 15.0), Rect::new(85.0, 85.0, 430.0, 330.0));
    assert_eq!(preview_frame(t, 0.0), t);
}
