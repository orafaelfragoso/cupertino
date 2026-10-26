use cupertino_wm::*;

const W: Rect = Rect::new(180.0, 100.0, 200.0, 100.0);
/// Its facing left edge sits at 399 for a window on its left (1 pt gap).
const O: Guide = Guide::Window(Rect::new(400.0, 100.0, 200.0, 100.0));

fn cfg() -> MagnetConfig {
    MagnetConfig::default()
}

fn at(x: f64) -> Point {
    Point::new(x, 150.0)
}

/// Window W moved so that the pointer, grabbed at (200, 150), is at `x`.
fn raw(x: f64) -> Rect {
    W.translate(x - 200.0, 0.0)
}

fn step(m: &mut Magnet, x: f64, guides: &[Guide], now: f64) -> MagnetStep {
    m.step(&cfg(), at(x), raw(x), Edges::ALL, guides, true, true, now)
}

/// Snapped: right edge 405 → 399 at 100 pt/s, pointer at 225.
fn snapped() -> Magnet {
    let mut m = Magnet::new(at(200.0), W, 0.0);
    assert_eq!(
        step(&mut m, 225.0, &[O], 0.25),
        MagnetStep {
            frame: Rect::new(199.0, 100.0, 200.0, 100.0),
            jumped: true
        }
    );
    m
}

#[test]
fn slow_edge_snaps_when_it_crosses_the_facing_edge() {
    let m = snapped();
    assert_eq!(
        m.snapped(),
        Edges {
            right: true,
            ..Edges::NONE
        }
    );
}

#[test]
fn edge_faster_than_400_points_per_second_passes_through() {
    let mut m = Magnet::new(at(200.0), W, 0.0);
    assert_eq!(
        step(&mut m, 225.0, &[O], 0.05),
        MagnetStep {
            frame: raw(225.0),
            jumped: false
        }
    );
}

#[test]
fn edge_that_was_30_points_away_or_more_passes_through() {
    let mut m = Magnet::new(at(189.0), raw(189.0), 0.0);
    assert_eq!(
        step(&mut m, 225.0, &[O], 1.0).frame,
        raw(225.0),
        "previous distance 30"
    );
    let mut m = Magnet::new(at(189.1), raw(189.1), 0.0);
    assert_eq!(
        step(&mut m, 225.0, &[O], 1.0).frame,
        Rect::new(199.0, 100.0, 200.0, 100.0),
        "previous distance 29.9"
    );
}

#[test]
fn guides_must_overlap_along_the_edge_within_one_point() {
    let below = |y| [Guide::Window(Rect::new(400.0, y, 200.0, 100.0))];
    let mut m = Magnet::new(at(200.0), W, 0.0);
    assert_eq!(step(&mut m, 225.0, &below(201.5), 0.25).frame, raw(225.0));
    let mut m = Magnet::new(at(200.0), W, 0.0);
    assert_eq!(
        step(&mut m, 225.0, &below(201.0), 0.25).frame,
        Rect::new(199.0, 100.0, 200.0, 100.0)
    );
}

#[test]
fn aligned_edges_and_the_screen_rectangle_attract_without_a_gap() {
    // a window just above: the right edge (380 → 405) lines up with its right edge at 400
    let above = [Guide::Window(Rect::new(0.0, 0.0, 400.0, 99.0))];
    let mut m = Magnet::new(at(200.0), W, 0.0);
    assert_eq!(
        step(&mut m, 225.0, &above, 0.25).frame,
        Rect::new(200.0, 100.0, 200.0, 100.0)
    );
    let screen = [Guide::Screen(Rect::new(0.0, 24.0, 1920.0, 1056.0))];
    let mut m = Magnet::new(
        Point::new(30.0, 60.0),
        Rect::new(10.0, 40.0, 200.0, 100.0),
        0.0,
    );
    let s = m.step(
        &cfg(),
        Point::new(15.0, 45.0),
        Rect::new(-5.0, 25.0, 200.0, 100.0),
        Edges::ALL,
        &screen,
        true,
        true,
        0.25,
    );
    assert_eq!(
        s.frame,
        Rect::new(0.0, 25.0, 200.0, 100.0),
        "left edge on the screen edge; top edge has not reached 24"
    );
    let s = m.step(
        &cfg(),
        Point::new(15.0, 43.0),
        Rect::new(-5.0, 23.0, 200.0, 100.0),
        Edges::ALL,
        &screen,
        true,
        true,
        0.5,
    );
    assert_eq!(s.frame, Rect::new(0.0, 24.0, 200.0, 100.0));
    assert_eq!(
        m.snapped(),
        Edges {
            left: true,
            top: true,
            ..Edges::NONE
        }
    );
}

#[test]
fn pushing_into_the_guide_releases_after_35_points() {
    let mut m = snapped();
    assert_eq!(
        step(&mut m, 259.0, &[O], 0.5).frame,
        Rect::new(199.0, 100.0, 200.0, 100.0),
        "pull 34"
    );
    assert_eq!(
        step(&mut m, 261.0, &[O], 0.75),
        MagnetStep {
            frame: raw(261.0),
            jumped: true
        },
        "pull 36"
    );
}

#[test]
fn sliding_along_the_guide_holds_for_140_points() {
    let mut m = snapped();
    let mut slide = |y: f64, now| {
        m.step(
            &cfg(),
            Point::new(225.0, 150.0 + y),
            raw(225.0).translate(0.0, y),
            Edges::ALL,
            &[O],
            true,
            true,
            now,
        )
    };
    assert_eq!(
        slide(139.0, 1.25).frame,
        Rect::new(199.0, 239.0, 200.0, 100.0)
    );
    assert_eq!(
        slide(141.0, 1.5).frame,
        Rect::new(205.0, 241.0, 200.0, 100.0)
    );
}

#[test]
fn unsnap_distance_grows_with_the_square_of_the_angle() {
    let c = cfg();
    assert_eq!(unsnap_distance(&c, 0.0), 35.0);
    assert_eq!(unsnap_distance(&c, 30.0), 61.25);
    assert_eq!(unsnap_distance(&c, 60.0), 140.0);
    assert_eq!(unsnap_distance(&c, 90.0), 140.0);
}

#[test]
fn pulling_away_releases_at_once_and_fast_motion_releases_too() {
    let mut m = snapped();
    assert_eq!(
        step(&mut m, 224.0, &[O], 0.5),
        MagnetStep {
            frame: raw(224.0),
            jumped: true
        }
    );
    let mut m = snapped();
    assert_eq!(
        step(&mut m, 240.0, &[O], 0.26).frame,
        raw(240.0),
        "1500 pt/s"
    );
}

#[test]
fn an_unsnapped_axis_snaps_again_only_after_55_points_or_a_reversal() {
    // a second window whose facing edge is at 445
    let guides = [O, Guide::Window(Rect::new(446.0, 120.0, 100.0, 50.0))];
    let mut m = Magnet::new(at(200.0), W, 0.0);
    assert_eq!(
        step(&mut m, 225.0, &guides, 0.25).frame,
        Rect::new(199.0, 100.0, 200.0, 100.0)
    );
    assert_eq!(
        step(&mut m, 262.0, &guides, 0.5).frame,
        raw(262.0),
        "pull 37 unsnaps"
    );
    assert_eq!(
        step(&mut m, 267.0, &guides, 0.75).frame,
        raw(267.0),
        "crosses 445 only 48 pt from 399"
    );
    assert_eq!(
        step(&mut m, 264.0, &guides, 1.0).frame,
        Rect::new(245.0, 100.0, 200.0, 100.0),
        "reversed"
    );
}

#[test]
fn resizing_moves_only_the_dragged_edge_onto_the_guide() {
    let right = Edges {
        right: true,
        ..Edges::NONE
    };
    let mut m = Magnet::new(
        Point::new(395.0, 150.0),
        Rect::new(100.0, 100.0, 295.0, 100.0),
        0.0,
    );
    let s = m.step(
        &cfg(),
        Point::new(402.0, 150.0),
        Rect::new(100.0, 100.0, 302.0, 100.0),
        right,
        &[O],
        true,
        true,
        0.1,
    );
    assert_eq!(s.frame, Rect::new(100.0, 100.0, 299.0, 100.0));
}

#[test]
fn disabled_releases_and_catch_up_blocks_new_snaps() {
    let mut m = snapped();
    let s = m.step(
        &cfg(),
        at(226.0),
        raw(226.0),
        Edges::ALL,
        &[O],
        false,
        true,
        0.5,
    );
    assert_eq!(
        s,
        MagnetStep {
            frame: raw(226.0),
            jumped: true
        }
    );
    let mut m = Magnet::new(at(200.0), W, 0.0);
    let s = m.step(
        &cfg(),
        at(225.0),
        raw(225.0),
        Edges::ALL,
        &[O],
        true,
        false,
        0.25,
    );
    assert_eq!(s.frame, raw(225.0));
}

#[test]
fn moving_along_the_guide_after_a_leftward_unsnap_keeps_the_re_snap_guard() {
    // window right of O, grabbed at (700, 150); its left edge snaps onto O's right edge (601)
    let w = Rect::new(615.0, 100.0, 200.0, 100.0);
    let raw = |x: f64, y: f64| w.translate(x - 700.0, y);
    // a second window whose facing right edge is at 554 (snap line 555)
    let guides = [O, Guide::Window(Rect::new(454.0, 100.0, 100.0, 100.0))];
    let mut m = Magnet::new(at(700.0), w, 0.0);
    let mut go = |x: f64, y: f64, now| {
        m.step(
            &cfg(),
            Point::new(x, 150.0 + y),
            raw(x, y),
            Edges::ALL,
            &guides,
            true,
            true,
            now,
        )
        .frame
        .x
    };
    assert_eq!(go(680.0, 0.0, 0.25), 601.0, "snapped");
    assert_eq!(go(644.0, 0.0, 0.5), 559.0, "pull 42 unsnaps, moving left");
    assert_eq!(
        go(644.0, 10.0, 0.75),
        559.0,
        "a purely vertical move is not a reversal"
    );
    assert_eq!(
        go(639.0, 10.0, 1.0),
        554.0,
        "crosses 555 only 47 pt from 601: the guard holds"
    );
}
