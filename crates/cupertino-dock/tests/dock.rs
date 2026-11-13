use cupertino_dock::*;
use cupertino_wm::geometry::{Point, Rect};
use cupertino_wm::ids::WindowId;

const SCREEN: Rect = Rect::new(0.0, 0.0, 1920.0, 1080.0);

/// A 48 pt dock with tiles edge to edge, 6 pt of padding, a 12 pt separator and 4 pt from the screen edge: the
/// behaviour tests' arithmetic (the default geometry has tests of its own below).
fn base_cfg() -> DockConfig {
    DockConfig {
        tile_size: 48.0,
        large_size: 128.0,
        spacing: 0.0,
        padding_ratio: 6.0 / 48.0,
        edge_padding_ratio: 6.0 / 48.0,
        separator_ratio: 12.0 / 48.0,
        edge_margin: 4.0,
        magnification: true,
        show_recents: true,
        ..DockConfig::default()
    }
}

fn dock_with(cfg: DockConfig) -> Dock {
    let mut d = Dock::new(cfg);
    d.set_pinned(&["files", "browser", "mail"]);
    d
}

fn kinds(d: &Dock) -> Vec<TileKind> {
    d.tiles()
        .iter()
        .filter(|t| !t.is_leaving())
        .map(|t| t.kind)
        .collect()
}

fn settle(d: &mut Dock, mut now: f64) -> f64 {
    for _ in 0..2000 {
        if !d.wants_tick(now) {
            break;
        }
        now += 1.0 / 120.0;
        d.tick(now);
    }
    d.tick(now + 10.0);
    now + 10.0
}

fn rect_of(d: &Dock, key: &str, now: f64) -> Rect {
    let id = d.app_tile(key).unwrap().id;
    d.layout(SCREEN, now)
        .tiles
        .iter()
        .find(|t| t.id == id)
        .unwrap()
        .rect
}

fn rects_eq(a: Rect, b: Rect) -> bool {
    a.to_array()
        .iter()
        .zip(b.to_array())
        .all(|(x, y)| (x - y).abs() < 1e-6)
}

#[test]
fn section_order_and_separators() {
    let mut d = dock_with(base_cfg());
    use TileKind::*;
    assert_eq!(kinds(&d), vec![App, App, App, Separator, Trash]);
    d.set_recents(&["doc1", "doc2"], 0.0);
    assert_eq!(
        kinds(&d),
        vec![App, App, App, Separator, Recent, Recent, Separator, Trash],
        "recents between two separators"
    );
    d.window_minimized(WindowId(1), "mail", "Inbox", 0.0);
    assert_eq!(
        kinds(&d),
        vec![
            App,
            App,
            App,
            Separator,
            Recent,
            Recent,
            Separator,
            MinimizedWindow,
            Trash
        ]
    );
    let mut d2 = dock_with(base_cfg());
    d2.window_minimized(WindowId(1), "mail", "Inbox", 0.0);
    assert_eq!(
        kinds(&d2),
        vec![App, App, App, Separator, MinimizedWindow, Trash],
        "no second separator without recents"
    );
    let cfg = DockConfig {
        show_recents: false,
        ..base_cfg()
    };
    let mut d3 = dock_with(cfg);
    d3.set_recents(&["x"], 0.0);
    assert_eq!(kinds(&d3), vec![App, App, App, Separator, Trash]);
}

#[test]
fn recents_are_truncated_and_reconciled() {
    let cfg = DockConfig {
        recents_max: 2,
        ..base_cfg()
    };
    let mut d = dock_with(cfg);
    d.set_recents(&["a", "b", "c"], 0.0);
    let keys = |d: &Dock| {
        d.tiles()
            .iter()
            .filter(|t| t.kind == TileKind::Recent && !t.is_leaving())
            .map(|t| t.key.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(keys(&d), vec!["a", "b"]);
    let id_b = d.tiles().iter().find(|t| t.key == "b").unwrap().id;
    d.set_recents(&["c", "b"], 1.0);
    assert_eq!(keys(&d), vec!["c", "b"]);
    assert_eq!(
        d.tiles().iter().find(|t| t.key == "b").unwrap().id,
        id_b,
        "existing tile keeps its identity"
    );
    settle(&mut d, 1.0);
    assert_eq!(
        d.tiles()
            .iter()
            .filter(|t| t.kind == TileKind::Recent)
            .count(),
        2
    );
}

#[test]
fn running_unpinned_tile_appears_and_quits_with_animation() {
    let mut d = dock_with(base_cfg());
    d.app_started("editor", 0.0);
    let t = d.app_tile("editor").unwrap();
    assert!(t.running && !t.pinned);
    assert_eq!(t.presence(0.0), 0.0);
    assert!(t.presence(0.1) > 0.0 && t.presence(0.1) < 1.0);
    assert_eq!(t.presence(1.0), 1.0);
    let len_full = d.unmagnified_length(1.0);
    assert!(d.unmagnified_length(0.0) < len_full);

    d.app_quit("editor", 2.0);
    assert!(d.app_tile("editor").is_none());
    assert!(d
        .tiles()
        .iter()
        .any(|t| t.key == "editor" && t.is_leaving()));
    d.tick(2.1);
    assert!(
        d.tiles().iter().any(|t| t.key == "editor"),
        "still shrinking"
    );
    d.tick(3.0);
    assert!(!d.tiles().iter().any(|t| t.key == "editor"));
}

#[test]
fn pinned_app_stays_when_quit_and_indicator_follows_running() {
    let mut d = dock_with(base_cfg());
    let layout = d.layout(SCREEN, 0.0);
    assert!(layout.tiles.iter().all(|t| !t.indicator));
    d.app_started("files", 0.0);
    let id = d.app_tile("files").unwrap().id;
    let p = |d: &Dock| {
        d.layout(SCREEN, 0.0)
            .tiles
            .iter()
            .find(|t| t.id == id)
            .unwrap()
            .clone()
    };
    assert!(p(&d).indicator);
    assert_eq!(p(&d).lifecycle, TileLifecycle::Running);
    d.app_quit("files", 1.0);
    assert!(d.app_tile("files").is_some());
    assert!(!p(&d).indicator);
    assert_eq!(p(&d).lifecycle, TileLifecycle::Idle);

    let cfg = DockConfig {
        show_running_indicators: false,
        ..base_cfg()
    };
    let mut d = dock_with(cfg);
    d.app_started("files", 0.0);
    assert!(d.layout(SCREEN, 0.0).tiles.iter().all(|t| !t.indicator));
}

#[test]
fn launch_bounce_then_running() {
    let mut d = dock_with(base_cfg());
    d.app_launching("mail", 0.0);
    let id = d.app_tile("mail").unwrap().id;
    let get = |d: &Dock, now| {
        d.layout(SCREEN, now)
            .tiles
            .iter()
            .find(|t| t.id == id)
            .unwrap()
            .clone()
    };
    let peak = d.bounce.period / 2.0;
    assert_eq!(get(&d, peak).lifecycle, TileLifecycle::Launching);
    assert!(
        (get(&d, peak).lift - (0.2 * (48.0 - 8.0) + 6.7)).abs() < 1e-9,
        "verified launch lift 0.2*(len-8)+6.7"
    );
    assert_eq!(get(&d, 0.0).lift, 0.0);
    d.app_started("mail", peak + 0.1);
    assert_eq!(get(&d, peak + 0.2).lifecycle, TileLifecycle::Running);
    assert!(get(&d, peak + 0.2).lift > 0.0, "the running arc finishes");
    assert_eq!(get(&d, d.bounce.period + 0.01).lift, 0.0);
    assert!(!d.wants_tick(5.0));
}

#[test]
fn a_launch_without_a_running_app_removes_its_unpinned_tile_at_the_timeout() {
    let mut d = dock_with(base_cfg());
    d.app_launching("ghost", 0.0);
    let timeout = d.bounce.launch_timeout;
    d.tick(timeout - 0.1);
    assert!(d.app_tile("ghost").is_some());
    d.tick(timeout);
    assert!(d.app_tile("ghost").is_none());
    let t = settle(&mut d, timeout);
    assert!(
        d.tiles().iter().all(|x| x.key != "ghost"),
        "the leaving tile is gone after its fade"
    );
    assert!(!d.wants_tick(t));
}

#[test]
fn attention_critical_until_cleared_and_informational_ends() {
    let mut d = dock_with(base_cfg());
    d.app_started("mail", 0.0);
    d.attention("mail", true, 0.0);
    let id = d.app_tile("mail").unwrap().id;
    let life = |d: &Dock, now| {
        d.layout(SCREEN, now)
            .tiles
            .iter()
            .find(|t| t.id == id)
            .unwrap()
            .lifecycle
    };
    assert_eq!(life(&d, 100.0 * 0.71 / 2.0), TileLifecycle::Attention);
    assert_eq!(life(&d, 500.0), TileLifecycle::Attention);
    d.clear_attention("mail", 10.0);
    assert_eq!(life(&d, 20.0), TileLifecycle::Running);
    d.attention("mail", false, 30.0);
    assert_eq!(life(&d, 30.1), TileLifecycle::Attention);
    assert_eq!(life(&d, 40.0), TileLifecycle::Running);
    d.attention("unknown", true, 0.0);
}

#[test]
fn bottom_layout_is_centred_flush_and_bottom_aligned() {
    let d = dock_with(base_cfg());
    let l = d.layout(SCREEN, 0.0);
    let cfg = &d.cfg;
    assert!((l.bar.cx() - 960.0).abs() < 1e-9);
    assert_eq!(l.bar.bottom(), 1080.0 - cfg.edge_margin);
    assert_eq!(l.bar.h, cfg.tile_size + 2.0 * cfg.bar_padding());
    for t in &l.tiles {
        assert_eq!(t.rect.bottom(), l.bar.bottom() - cfg.bar_padding());
        assert!(t.rect.x >= l.bar.x && t.rect.right() <= l.bar.right());
    }
    let xs: Vec<f64> = l.tiles.iter().map(|t| t.rect.x).collect();
    assert!(xs.windows(2).all(|w| w[0] < w[1]));
    let sep = l
        .tiles
        .iter()
        .find(|t| t.kind == TileKind::Separator)
        .unwrap();
    assert_eq!(sep.rect.w, cfg.separator_extent());
}

#[test]
fn left_and_right_orientation() {
    for (o, left) in [(Orientation::Left, true), (Orientation::Right, false)] {
        let d = dock_with(DockConfig {
            orientation: o,
            ..base_cfg()
        });
        let l = d.layout(SCREEN, 0.0);
        assert!((l.bar.cy() - 540.0).abs() < 1e-9);
        if left {
            assert_eq!(l.bar.x, d.cfg.edge_margin);
            assert!(l
                .tiles
                .iter()
                .all(|t| t.rect.x == l.bar.x + d.cfg.bar_padding()));
        } else {
            assert_eq!(l.bar.right(), 1920.0 - d.cfg.edge_margin);
            assert!(l
                .tiles
                .iter()
                .all(|t| t.rect.right() == l.bar.right() - d.cfg.bar_padding()));
        }
        let ys: Vec<f64> = l.tiles.iter().map(|t| t.rect.y).collect();
        assert!(
            ys.windows(2).all(|w| w[0] < w[1]),
            "tiles flow top to bottom"
        );
        let first = &l.tiles[0];
        let hit = d.hit_test(first.rect.center(), SCREEN, 0.0).unwrap();
        assert_eq!(hit.id, first.id);
    }
}

#[test]
fn layout_uses_given_screen_origin() {
    let d = dock_with(base_cfg());
    let second = Rect::new(1920.0, 0.0, 1280.0, 800.0);
    let l = d.layout(second, 0.0);
    assert!((l.bar.cx() - 2560.0).abs() < 1e-9);
    assert_eq!(l.bar.bottom(), 800.0 - d.cfg.edge_margin);
}

#[test]
fn hit_testing_at_rest() {
    let d = dock_with(base_cfg());
    let l = d.layout(SCREEN, 0.0);
    for (i, t) in l.tiles.iter().enumerate() {
        let h = d.hit_test(t.rect.center(), SCREEN, 0.0).unwrap();
        assert_eq!((h.id, h.kind, h.index), (t.id, t.kind, i));
    }
    assert!(d.hit_test(Point::new(10.0, 10.0), SCREEN, 0.0).is_none());
    assert!(d
        .hit_test(Point::new(960.0, l.bar.y - 20.0), SCREEN, 0.0)
        .is_none());
    let t = &l.tiles[0];
    assert!(
        d.hit_test(Point::new(t.rect.cx(), l.bar.y + 0.5), SCREEN, 0.0)
            .is_some(),
        "bar padding area counts"
    );
}

#[test]
fn pointer_drives_magnification_and_hover() {
    let mut d = dock_with(base_cfg());
    let rest = d.layout(SCREEN, 0.0);
    let target = rest.tiles[1].clone();
    d.pointer_moved(Some(target.rect.center()), SCREEN, 0.0);
    assert!(d.magnification_amount(0.0) < 1e-9, "ramp starts from zero");
    assert!(d.magnification_amount(0.05) > 0.0);
    let t = settle(&mut d, 0.0);
    assert!((d.magnification_amount(t) - 1.0).abs() < 1e-6);
    let l = d.layout(SCREEN, t);
    let big = l.tiles.iter().find(|p| p.id == target.id).unwrap();
    assert!((big.rect.w - d.cfg.large_size).abs() < 3.0);
    assert!(
        big.rect.contains(target.rect.center()),
        "tile stays under the cursor"
    );
    assert_eq!(d.hovered(), Some(target.id));
    let hit = d.hit_test(target.rect.center(), SCREEN, t).unwrap();
    assert_eq!(hit.id, target.id);

    d.pointer_moved(Some(Point::new(5.0, 5.0)), SCREEN, t);
    assert_eq!(d.hovered(), None);
    let mid = d.layout(SCREEN, t + 0.05);
    assert!(
        mid.tiles[1].rect.w > d.cfg.tile_size,
        "ramps out smoothly instead of snapping"
    );
    let t2 = settle(&mut d, t);
    assert!(d
        .layout(SCREEN, t2)
        .tiles
        .iter()
        .all(|p| p.kind == TileKind::Separator || (p.rect.w - 48.0).abs() < 1e-6));
}

/// Under Reduce Motion the tiles magnify at once and a bounce dims its tile in place: no lift, a pulse that peaks
/// where the hop would.
#[test]
fn under_reduce_motion_magnification_is_immediate_and_a_bounce_dims_in_place() {
    let mut d = dock_with(base_cfg());
    d.set_reduce_motion(true);
    let rest = d.layout(SCREEN, 0.0);
    let target = rest.tiles[1].clone();
    d.pointer_moved(Some(target.rect.center()), SCREEN, 0.0);
    assert!((d.magnification_amount(0.0) - 1.0).abs() < 1e-9, "no ramp");
    let big = d.layout(SCREEN, 0.0);
    let big = big.tiles.iter().find(|p| p.id == target.id).unwrap();
    assert!((big.rect.w - d.cfg.large_size).abs() < 3.0);
    d.pointer_moved(Some(Point::new(5.0, 5.0)), SCREEN, 0.0);
    assert!(d.magnification_amount(0.0) < 1e-9, "no ramp out");

    d.app_launching("mail", 1.0);
    let id = d.app_tile("mail").unwrap().id;
    let get = |d: &Dock, now| {
        d.layout(SCREEN, now)
            .tiles
            .iter()
            .find(|t| t.id == id)
            .unwrap()
            .clone()
    };
    let peak = 1.0 + d.bounce.period / 2.0;
    assert_eq!(get(&d, peak).lift, 0.0);
    assert!(get(&d, peak).pulse > 0.99, "{}", get(&d, peak).pulse);
    assert!(get(&d, 1.0).pulse < 1e-6);
    assert_eq!(get(&d, peak).lifecycle, TileLifecycle::Launching);

    d.set_reduce_motion(false);
    assert!(get(&d, peak).lift > 0.0, "the bounce lifts again");
    assert_eq!(get(&d, peak).pulse, 0.0);
}

/// An auto-hidden dock held out (beside the open application grid) shows with the pointer away and magnifies under it.
#[test]
fn an_auto_hidden_dock_held_out_shows_and_magnifies() {
    let cfg = DockConfig {
        autohide: true,
        ..base_cfg()
    };
    let mut d = dock_with(cfg);
    d.keep_shown = true;
    d.pointer_moved(Some(Point::new(960.0, 100.0)), SCREEN, 0.0);
    d.pointer_moved(Some(Point::new(960.0, 100.0)), SCREEN, 0.6);
    let t = 5.0;
    d.pointer_moved(Some(Point::new(960.0, 100.0)), SCREEN, t);
    let shown = d.layout(SCREEN, t);
    assert!(shown.bar.bottom() <= SCREEN.bottom(), "{:?}", shown.bar);
    let target = shown.tiles[1].clone();
    d.pointer_moved(Some(target.rect.center()), SCREEN, t);
    let t = settle(&mut d, t);
    let l = d.layout(SCREEN, t);
    let big = l.tiles.iter().find(|p| p.id == target.id).unwrap();
    assert!(
        (big.rect.w - d.cfg.large_size).abs() < 3.0,
        "{:?}",
        big.rect
    );
}

#[test]
fn hover_tracks_magnified_tile_under_pointer_along_a_sweep() {
    let mut d = dock_with(base_cfg());
    let rest = d.layout(SCREEN, 0.0);
    let y = rest.bar.cy();
    d.pointer_moved(Some(Point::new(rest.bar.x + 10.0, y)), SCREEN, 0.0);
    let mut now = settle(&mut d, 0.0);
    let (x0, x1) = (rest.bar.x + 10.0, rest.bar.right() - 10.0);
    let mut x = x0;
    while x < x1 {
        d.pointer_moved(Some(Point::new(x, y)), SCREEN, now);
        let l = d.layout(SCREEN, now);
        let p = Point::new(x, y);
        let under = l
            .tiles
            .iter()
            .find(|t| t.rect.x <= p.x && p.x < t.rect.right());
        let hit = d.hit_test(p, SCREEN, now);
        assert_eq!(hit.map(|h| h.id), under.map(|t| t.id), "x={x}");
        now += 0.001;
        x += 3.0;
    }
}

#[test]
fn magnification_disabled_keeps_tiles_uniform() {
    let mut d = dock_with(DockConfig {
        magnification: false,
        ..base_cfg()
    });
    let rest = d.layout(SCREEN, 0.0);
    d.pointer_moved(Some(rest.tiles[0].rect.center()), SCREEN, 0.0);
    let t = settle(&mut d, 0.0);
    assert_eq!(
        d.layout(SCREEN, t)
            .tiles
            .iter()
            .map(|p| p.rect)
            .collect::<Vec<_>>(),
        rest.tiles.iter().map(|p| p.rect).collect::<Vec<_>>()
    );
}

struct Flat;
impl MagnificationProfile for Flat {
    fn weight(&self, x: f64) -> f64 {
        if x.abs() < 1.0 {
            1.0
        } else {
            0.0
        }
    }
}

#[test]
fn profile_can_be_swapped() {
    let mut d = dock_with(base_cfg());
    d.set_profile(Box::new(Flat));
    let rest = d.layout(SCREEN, 0.0);
    d.pointer_moved(Some(rest.tiles[1].rect.center()), SCREEN, 0.0);
    let t = settle(&mut d, 0.0);
    let l = d.layout(SCREEN, t);
    assert!(
        l.tiles[..3].iter().all(|p| (p.rect.w - 128.0).abs() < 1e-6),
        "flat profile magnifies neighbours fully"
    );
}

#[test]
fn autohide_moves_bar_off_screen_and_reveals() {
    let cfg = DockConfig {
        autohide: true,
        ..base_cfg()
    };
    let mut d = dock_with(cfg);
    let hidden = d.layout(SCREEN, 0.0);
    assert!(
        hidden.bar.y >= SCREEN.bottom(),
        "fully hidden: {:?}",
        hidden.bar
    );
    d.pointer_moved(Some(Point::new(960.0, 1079.5)), SCREEN, 0.0);
    d.pointer_moved(Some(Point::new(960.0, 1079.5)), SCREEN, 0.6);
    let shown = d.layout(SCREEN, 5.0);
    assert!(shown.bar.bottom() <= SCREEN.bottom());
    assert_eq!(d.autohide.state, HideState::Showing);
    d.pointer_moved(Some(Point::new(960.0, shown.bar.cy())), SCREEN, 5.0);
    assert_eq!(d.autohide.state, HideState::Shown);
    d.pointer_moved(Some(Point::new(960.0, 100.0)), SCREEN, 6.0);
    d.pointer_moved(Some(Point::new(960.0, 100.0)), SCREEN, 6.5);
    d.pointer_moved(Some(Point::new(960.0, 100.0)), SCREEN, 20.0);
    assert!(d.layout(SCREEN, 20.0).bar.y > SCREEN.bottom() - 10.0);
}

#[test]
fn external_autohide_ignores_the_pointer_and_follows_set_revealed() {
    let cfg = DockConfig {
        autohide: true,
        ..base_cfg()
    };
    let mut d = dock_with(cfg);
    d.set_revealed(false, 0.0);
    assert!(d.layout(SCREEN, 0.0).bar.y >= SCREEN.bottom());
    // the pointer at the edge does not reveal an externally driven dock
    d.pointer_moved(Some(Point::new(960.0, 1079.5)), SCREEN, 1.0);
    d.pointer_moved(Some(Point::new(960.0, 1079.5)), SCREEN, 2.0);
    assert!(d.layout(SCREEN, 5.0).bar.y >= SCREEN.bottom());
    d.set_revealed(true, 5.0);
    assert!(d.layout(SCREEN, 10.0).bar.bottom() <= SCREEN.bottom());
    d.set_revealed(false, 10.0);
    assert!(d.layout(SCREEN, 20.0).bar.y >= SCREEN.bottom() - 1e-6);
}

#[test]
fn the_autohide_toggle_slides_the_dock_out_and_back_and_moves_the_reserved_zone() {
    let mut d = dock_with(base_cfg());
    let reserved = d.exclusive_zone();
    assert!(reserved > 0.0);
    d.toggle_autohide(0.0);
    assert_eq!(d.exclusive_zone(), 0.0);
    // the pointer away from the dock: the hide delay passes and the bar slides out
    for t in [0.1, 0.2, 0.3, 1.0] {
        d.pointer_moved(Some(Point::new(960.0, 100.0)), SCREEN, t);
    }
    assert!(d.layout(SCREEN, 1.0).bar.y >= SCREEN.bottom() - 1e-6);
    d.toggle_autohide(2.0);
    assert_eq!(d.exclusive_zone(), reserved);
    assert!(d.layout(SCREEN, 2.0).bar.bottom() <= SCREEN.bottom());
}

#[test]
fn minimize_target_matches_final_tile_rect() {
    for o in [Orientation::Bottom, Orientation::Left, Orientation::Right] {
        let mut d = dock_with(DockConfig {
            orientation: o,
            ..base_cfg()
        });
        d.set_recents(&["doc"], 0.0);
        settle(&mut d, 0.0);
        let w = WindowId(7);
        let target = d.minimize_target(w, "mail", SCREEN).unwrap();
        d.window_minimized(w, "mail", "Inbox", 100.0);
        let t = settle(&mut d, 100.0);
        let id = d.tiles().iter().find(|t| t.window == Some(w)).unwrap().id;
        let after = d
            .layout(SCREEN, t)
            .tiles
            .iter()
            .find(|p| p.id == id)
            .unwrap()
            .rect;
        assert!(rects_eq(target, after), "{o:?}: {target:?} vs {after:?}");
        let again = d.minimize_target(w, "mail", SCREEN).unwrap();
        assert!(rects_eq(again, after), "existing tile rect is returned");
    }
}

#[test]
fn second_minimized_window_targets_next_slot() {
    let mut d = dock_with(base_cfg());
    d.window_minimized(WindowId(1), "mail", "a", 0.0);
    settle(&mut d, 0.0);
    let t2 = d.minimize_target(WindowId(2), "mail", SCREEN).unwrap();
    let first = d.minimize_target(WindowId(1), "mail", SCREEN).unwrap();
    assert!(t2.x > first.x);
    d.window_minimized(WindowId(2), "mail", "b", 10.0);
    let t = settle(&mut d, 10.0);
    let id = d
        .tiles()
        .iter()
        .find(|t| t.window == Some(WindowId(2)))
        .unwrap()
        .id;
    let after = d
        .layout(SCREEN, t)
        .tiles
        .iter()
        .find(|p| p.id == id)
        .unwrap()
        .rect;
    assert!(rects_eq(t2, after));
}

#[test]
fn minimized_tile_removed_on_restore() {
    let mut d = dock_with(base_cfg());
    d.window_minimized(WindowId(1), "mail", "a", 0.0);
    settle(&mut d, 0.0);
    assert_eq!(d.minimized_count("mail"), 1);
    d.window_restored(WindowId(1), 5.0);
    assert_eq!(d.minimized_count("mail"), 0);
    settle(&mut d, 5.0);
    assert!(!kinds(&d).contains(&TileKind::MinimizedWindow));
    d.window_restored(WindowId(99), 6.0);
}

#[test]
fn minimize_to_app_targets_app_tile_and_counts_windows() {
    let mut d = dock_with(DockConfig {
        minimize_to_app: true,
        ..base_cfg()
    });
    let w = WindowId(3);
    let t = d.minimize_target(w, "mail", SCREEN).unwrap();
    assert!(rects_eq(t, rect_of(&d, "mail", 0.0)));
    d.window_minimized(w, "mail", "x", 0.0);
    assert!(!kinds(&d).contains(&TileKind::MinimizedWindow));
    assert_eq!(d.minimized_count("mail"), 1);
    assert!(d.minimize_target(w, "not-in-dock", SCREEN).is_none());
    d.app_quit("mail", 1.0);
    assert_eq!(
        d.minimized_count("mail"),
        0,
        "quitting the app drops its minimized windows"
    );
}

#[test]
fn minimize_target_ignores_hover_magnification_and_autohide() {
    let cfg = DockConfig {
        autohide: true,
        ..base_cfg()
    };
    let d = dock_with(cfg);
    let before = d.minimize_target(WindowId(1), "mail", SCREEN).unwrap();
    assert!(
        before.bottom() <= SCREEN.bottom(),
        "target is where the shown dock will be"
    );
    let mut d2 = dock_with(base_cfg());
    let rest = d2.layout(SCREEN, 0.0);
    d2.pointer_moved(Some(rest.tiles[0].rect.center()), SCREEN, 0.0);
    let t = settle(&mut d2, 0.0);
    let a = d2.minimize_target(WindowId(1), "mail", SCREEN).unwrap();
    let mut d3 = dock_with(base_cfg());
    let b = d3.minimize_target(WindowId(1), "mail", SCREEN).unwrap();
    assert!(rects_eq(a, b));
    let _ = (t, &mut d3);
}

/// Press the tile of `key` at its centre and begin the drag there.
fn begin(d: &mut Dock, key: &str) -> Point {
    let c = rect_of(d, key, 0.0).center();
    let id = d.app_tile(key).unwrap().id;
    assert!(d.begin_drag(id, c, c, SCREEN, 0.0));
    c
}

fn app_order(d: &Dock) -> Vec<String> {
    d.tiles()
        .iter()
        .filter(|t| t.kind == TileKind::App && !t.is_leaving())
        .map(|t| t.key.clone())
        .collect()
}

/// The tile row of the 48 pt dock at the bottom of `SCREEN`: the bar's top edge is at 1016, so the
/// removal edge, one bar thickness (60) further, is at 956.
const ABOVE_THE_REMOVAL_EDGE: f64 = 900.0;

#[test]
fn drag_reorder_moves_tile_and_opens_gap() {
    let mut d = dock_with(base_cfg());
    let start = begin(&mut d, "files");
    assert!(d.is_dragging());
    assert_eq!(d.drag_insertion(), Some(0));
    let l = d.layout(SCREEN, 0.0);
    assert!(l.dragging.is_some() && l.placeholder.is_some());
    assert_eq!(
        l.tiles.iter().filter(|t| t.kind == TileKind::App).count(),
        2
    );

    let mail = rect_of(&d, "mail", 0.0);
    d.drag_move(Point::new(mail.cx() + 5.0, start.y), SCREEN, 0.1);
    assert_eq!(d.drag_insertion(), Some(2));
    let l = d.layout(SCREEN, 0.1);
    let ph = l.placeholder.unwrap();
    let after_mail = l
        .tiles
        .iter()
        .filter(|t| t.kind == TileKind::App)
        .map(|t| t.rect.x)
        .fold(f64::MIN, f64::max);
    assert!(
        ph.x > after_mail - 1e-9,
        "gap is after the last other app tile"
    );
    assert!(!d.drag_will_remove(0.1));
    let out = d.end_drag(SCREEN, 0.2);
    assert_eq!(
        out,
        DragOutcome::Reordered {
            key: "files".into(),
            from: 0,
            to: 2
        }
    );
    assert_eq!(app_order(&d), vec!["browser", "mail", "files"]);
    assert!(!d.is_dragging());
}

/// over a tile the gap goes before it left of its middle and after it right of it, on the
/// tiles as they are drawn, so the middle of a neighbour that has moved is where it is now.
#[test]
fn the_gap_follows_the_middle_of_the_tile_under_the_pointer() {
    let mut d = dock_with(base_cfg());
    let start = begin(&mut d, "files");
    let browser = rect_of(&d, "browser", 0.0);
    d.drag_move(Point::new(browser.cx() - 1.0, start.y), SCREEN, 5.0);
    assert_eq!(
        d.drag_insertion(),
        Some(0),
        "left of the middle of the tile"
    );
    d.drag_move(Point::new(browser.cx() + 1.0, start.y), SCREEN, 5.0);
    assert_eq!(
        d.drag_insertion(),
        Some(1),
        "right of the middle of the tile"
    );
    // the neighbour has slid to the left by now: the middle that counts is where it is drawn
    let t = settle(&mut d, 5.0);
    let slid = rect_of(&d, "browser", t);
    assert!(slid.cx() < browser.cx());
    d.drag_move(Point::new(slid.cx() + 1.0, start.y), SCREEN, t);
    assert_eq!(d.drag_insertion(), Some(1));
    d.drag_move(Point::new(slid.cx() - 1.0, start.y), SCREEN, t);
    assert_eq!(d.drag_insertion(), Some(0));
}

/// a neighbour of the gap moves to its new place in 270 ms on the cosine ease.
#[test]
fn a_neighbour_slides_to_its_new_place_in_270_ms_on_the_cosine_ease() {
    let mut d = dock_with(base_cfg());
    let start = begin(&mut d, "files");
    let before = rect_of(&d, "browser", 0.0);
    let mail = rect_of(&d, "mail", 0.0);
    d.drag_move(Point::new(mail.cx() + 5.0, start.y), SCREEN, 1.0);
    let at = |t: f64| rect_of(&d, "browser", 1.0 + t).x;
    assert!((at(0.0) - before.x).abs() < 1e-6, "it starts where it was");
    let slot = 48.0;
    assert!(
        (at(0.135) - (before.x - slot * 0.5)).abs() < 1e-6,
        "halfway at the middle of 270 ms, got {}",
        at(0.135)
    );
    assert!(
        (at(0.270) - (before.x - slot)).abs() < 1e-6,
        "one slot to the left when it arrives"
    );
    assert!((at(5.0) - at(0.270)).abs() < 1e-9);
    assert!(
        d.wants_tick(1.1) && !d.wants_tick(1.3),
        "frames only while it slides"
    );
}

/// the gap is open only within a tile size of the screen edge, over the flow of the
/// tile's own section; otherwise the neighbours close up and a drop puts the tile back.
#[test]
fn the_gap_closes_when_the_pointer_leaves_the_strip_at_the_screen_edge() {
    let mut d = dock_with(base_cfg());
    let c = begin(&mut d, "browser");
    assert_eq!(d.layout(SCREEN, 0.0).placeholder.map(|r| r.w), Some(48.0));
    d.drag_move(Point::new(c.x, SCREEN.bottom() - 48.0 - 1.0), SCREEN, 0.1);
    assert_eq!(
        d.layout(SCREEN, 0.1).placeholder.map(|r| r.w),
        Some(0.0),
        "a tile above the strip"
    );
    d.drag_move(Point::new(c.x, SCREEN.bottom() - 48.0), SCREEN, 0.2);
    assert_eq!(
        d.layout(SCREEN, 0.2).placeholder.map(|r| r.w),
        Some(48.0),
        "the strip is a tile size from the edge"
    );
    d.drag_move(Point::new(c.x + 300.0, c.y), SCREEN, 0.3);
    assert_eq!(
        d.layout(SCREEN, 0.3).placeholder.map(|r| r.w),
        Some(0.0),
        "past the end of the app section"
    );
    assert_eq!(
        d.end_drag(SCREEN, 0.4),
        DragOutcome::Cancelled,
        "released with the gap closed it goes back"
    );
    assert_eq!(app_order(&d), vec!["files", "browser", "mail"]);
}

#[test]
fn drag_back_to_origin_is_cancelled_and_cancel_restores() {
    let mut d = dock_with(base_cfg());
    let c = begin(&mut d, "browser");
    d.drag_move(Point::new(c.x + 3.0, c.y), SCREEN, 0.1);
    assert_eq!(d.end_drag(SCREEN, 0.2), DragOutcome::Cancelled);
    assert_eq!(app_order(&d), vec!["files", "browser", "mail"]);
    let c0 = rect_of(&d, "files", 0.0).center();
    begin(&mut d, "browser");
    d.drag_move(c0, SCREEN, 0.3);
    d.cancel_drag();
    assert_eq!(app_order(&d), vec!["files", "browser", "mail"]);
    assert_eq!(d.end_drag(SCREEN, 0.4), DragOutcome::Cancelled);
}

/// a dropped tile settles into its slot from where it was released, 350 ms, cosine ease.
#[test]
fn a_dropped_tile_settles_into_its_slot_in_350_ms() {
    let mut d = dock_with(base_cfg());
    let start = begin(&mut d, "files");
    let mail = rect_of(&d, "mail", 0.0);
    d.drag_move(Point::new(mail.cx() + 5.0, start.y), SCREEN, 1.0);
    let released = d.layout(SCREEN, 1.0).dragging.unwrap().tile.rect;
    d.end_drag(SCREEN, 1.0);
    let slot = rect_of(&d, "files", 5.0);
    let held = d
        .layout(SCREEN, 1.0)
        .dragging
        .expect("the tile is still drawn on its own");
    assert!(rects_eq(held.tile.rect, released));
    let mid = d.layout(SCREEN, 1.175).dragging.unwrap().tile.rect;
    assert!(
        (mid.x - (released.x + slot.x) / 2.0).abs() < 1e-6,
        "halfway at the middle of 350 ms"
    );
    assert!(d.layout(SCREEN, 1.35).dragging.is_none());
    assert!(
        d.layout(SCREEN, 1.35)
            .tiles
            .iter()
            .any(|t| t.id == held.tile.id),
        "and it is in the bar again"
    );
    assert!(d.wants_tick(1.2) && !d.wants_tick(1.4));
}

/// the icon stays where it was grabbed, relative to the pointer, at the size it had.
#[test]
fn a_dragged_tile_keeps_the_place_it_was_grabbed_at() {
    let mut d = dock_with(base_cfg());
    let tile = rect_of(&d, "browser", 0.0);
    let id = d.app_tile("browser").unwrap().id;
    let press = Point::new(tile.x + 10.0, tile.y + 12.0);
    assert!(d.begin_drag(
        id,
        press,
        Point::new(press.x + 30.0, press.y - 8.0),
        SCREEN,
        0.0
    ));
    let held = d.layout(SCREEN, 0.0).dragging.unwrap().tile.rect;
    assert!(rects_eq(
        held,
        Rect::new(tile.x + 30.0, tile.y - 8.0, tile.w, tile.h)
    ));
    d.drag_move(Point::new(press.x + 100.0, press.y - 40.0), SCREEN, 0.1);
    let held = d.layout(SCREEN, 0.1).dragging.unwrap().tile.rect;
    assert!(
        rects_eq(
            held,
            Rect::new(tile.x + 100.0, tile.y - 40.0, tile.w, tile.h)
        ),
        "no lag, no scale"
    );
}

/// the removal edge is one bar thickness beyond the bar (across the bar only), and a
/// drop removes nothing in the first half second of a drag.
#[test]
fn a_drop_removes_a_tile_only_past_the_edge_across_the_bar_and_after_the_delay() {
    let mut d = dock_with(base_cfg());
    let c = begin(&mut d, "mail");
    // far to the side along the bar: not past the edge
    d.drag_move(Point::new(c.x - 800.0, c.y), SCREEN, 0.1);
    assert!(!d.drag_will_remove(5.0));
    // past the edge at 956, one pt either side of it
    d.drag_move(Point::new(c.x, 957.0), SCREEN, 0.1);
    assert!(!d.drag_will_remove(5.0));
    d.drag_move(Point::new(c.x, 955.0), SCREEN, 0.1);
    assert!(!d.drag_will_remove(0.4), "not before the delay");
    assert!(d.drag_will_remove(0.5));
    assert_eq!(
        d.end_drag(SCREEN, 0.4),
        DragOutcome::Cancelled,
        "released early it goes back"
    );
    assert!(d.app_tile("mail").is_some_and(|t| t.pinned));
}

/// past the edge, once the delay is over, the icon dims to 35 % and the label shows, on
/// the 0.25 s ease-in-ease-out.
#[test]
fn the_icon_dims_and_the_label_shows_once_a_drop_would_remove_the_tile() {
    let mut d = dock_with(base_cfg());
    let c = begin(&mut d, "mail");
    d.drag_move(Point::new(c.x, ABOVE_THE_REMOVAL_EDGE), SCREEN, 0.1);
    assert_eq!(d.layout(SCREEN, 0.3).dragging.unwrap().opacity, 1.0);
    d.tick(0.5);
    let held = |t: f64| d.layout(SCREEN, t).dragging.unwrap();
    assert_eq!(held(0.5).label, 0.0);
    assert!(
        (held(0.5 + 0.125).label - 0.5).abs() < 1e-9,
        "half way through the fade, by symmetry"
    );
    assert!((held(0.5 + 0.25).opacity - 0.35).abs() < 1e-9);
    assert_eq!(held(0.5 + 0.25).label, 1.0);
    assert!(d.wants_tick(0.6) && !d.wants_tick(0.8));
}

/// the removed tile's icon fades from 1 to 0 on the curve in `0.25 / 0.75` s.
#[test]
fn drag_out_removes_pinned_app_and_trash_drop_too() {
    let mut d = dock_with(base_cfg());
    let c = begin(&mut d, "mail");
    d.drag_move(Point::new(c.x, ABOVE_THE_REMOVAL_EDGE), SCREEN, 0.1);
    assert_eq!(
        d.layout(SCREEN, 0.1).placeholder.map(|r| r.w),
        Some(0.0),
        "the gap closes when the pointer leaves the strip"
    );
    let out = d.end_drag(SCREEN, 1.0);
    assert_eq!(
        out,
        DragOutcome::Removed {
            key: "mail".into(),
            kind: TileKind::App
        }
    );
    assert!(d.app_tile("mail").is_none());
    let fade = 0.25 / 0.75;
    assert_eq!(d.layout(SCREEN, 1.0).dragging.unwrap().opacity, 1.0);
    let half = d.layout(SCREEN, 1.0 + fade / 2.0).dragging.unwrap();
    assert!(
        (half.opacity - (1.0 - 0.6019)).abs() < 1e-3,
        "on the cubic-bezier(0.22363, 0.002, 0.5534, 0.9529), got {}",
        half.opacity
    );
    assert_eq!(half.label, half.opacity, "the label fades with the icon");
    assert!(d.layout(SCREEN, 1.0 + fade).dragging.is_none());
    d.tick(5.0);
    assert_eq!(app_order(&d), vec!["files", "browser"]);

    let mut d = dock_with(base_cfg());
    begin(&mut d, "files");
    let trash = d
        .layout(SCREEN, 0.0)
        .tiles
        .iter()
        .find(|t| t.kind == TileKind::Trash)
        .unwrap()
        .rect
        .center();
    d.drag_move(trash, SCREEN, 0.1);
    assert!(d.drag_will_remove(0.1), "the trash takes it at once");
    assert!(matches!(
        d.end_drag(SCREEN, 0.2),
        DragOutcome::Removed { .. }
    ));
}

#[test]
fn the_pinned_keys_follow_a_reorder_and_a_removal_but_not_running_unpinned_apps() {
    let mut d = dock_with(base_cfg());
    d.app_started("calc", 0.0);
    assert_eq!(
        d.pinned_keys(),
        ["files", "browser", "mail"],
        "a running app that is not pinned is not a pin"
    );
    let start = begin(&mut d, "files");
    d.drag_move(
        Point::new(rect_of(&d, "mail", 0.0).cx() + 5.0, start.y),
        SCREEN,
        0.1,
    );
    d.end_drag(SCREEN, 0.2);
    assert_eq!(d.pinned_keys(), ["browser", "mail", "files"]);
    let c = begin(&mut d, "mail");
    d.drag_move(Point::new(c.x, ABOVE_THE_REMOVAL_EDGE), SCREEN, 0.3);
    d.end_drag(SCREEN, 0.9);
    assert_eq!(d.pinned_keys(), ["browser", "files"]);
}

#[test]
fn removing_a_running_app_keeps_its_tile_unpinned() {
    let mut d = dock_with(base_cfg());
    d.app_started("mail", 0.0);
    let c = begin(&mut d, "mail");
    d.drag_move(Point::new(c.x, ABOVE_THE_REMOVAL_EDGE), SCREEN, 0.1);
    d.end_drag(SCREEN, 0.7);
    let t = d.app_tile("mail").unwrap();
    assert!(!t.pinned && t.running);
}

#[test]
fn drag_is_limited_to_its_section_and_only_apps_and_recents() {
    let mut d = dock_with(base_cfg());
    d.set_recents(&["r1", "r2"], 0.0);
    d.window_minimized(WindowId(1), "mail", "m", 0.0);
    settle(&mut d, 0.0);
    let minimized = d
        .tiles()
        .iter()
        .find(|t| t.kind == TileKind::MinimizedWindow)
        .unwrap()
        .id;
    let origin = Point::new(0.0, 0.0);
    assert!(!d.begin_drag(minimized, origin, origin, SCREEN, 0.0));
    let sep = d
        .tiles()
        .iter()
        .find(|t| t.kind == TileKind::Separator)
        .unwrap()
        .id;
    assert!(!d.begin_drag(sep, origin, origin, SCREEN, 0.0));

    let c = begin(&mut d, "files");
    let far_right = Point::new(1500.0, c.y);
    d.drag_move(far_right, SCREEN, 0.1);
    assert_eq!(d.drag_insertion(), Some(2), "cannot pass the separator");
    assert_eq!(
        d.layout(SCREEN, 0.1).placeholder.map(|r| r.w),
        Some(0.0),
        "and the gap closes there"
    );
    d.cancel_drag();

    let r1 = d.tiles().iter().find(|t| t.key == "r1").unwrap().id;
    let r2c = d
        .layout(SCREEN, 5.0)
        .tiles
        .iter()
        .find(|t| t.key_is(&d, "r2"))
        .map(|t| t.rect.center())
        .unwrap();
    let r1c = d
        .layout(SCREEN, 5.0)
        .tiles
        .iter()
        .find(|t| t.id == r1)
        .unwrap()
        .rect
        .center();
    assert!(d.begin_drag(r1, r1c, r1c, SCREEN, 5.0));
    d.drag_move(Point::new(r2c.x + 10.0, r2c.y), SCREEN, 5.1);
    assert_eq!(
        d.end_drag(SCREEN, 5.2),
        DragOutcome::Reordered {
            key: "r1".into(),
            from: 0,
            to: 1
        }
    );
}

trait KeyIs {
    fn key_is(&self, d: &Dock, key: &str) -> bool;
}
impl KeyIs for PlacedTile {
    fn key_is(&self, d: &Dock, key: &str) -> bool {
        d.tile(self.id).is_some_and(|t| t.key == key)
    }
}

#[test]
fn drag_disables_magnification_and_pointer_events() {
    let mut d = dock_with(base_cfg());
    let rest = d.layout(SCREEN, 0.0);
    d.pointer_moved(Some(rest.tiles[0].rect.center()), SCREEN, 0.0);
    let t = settle(&mut d, 0.0);
    assert!(d.magnification_amount(t) > 0.99);
    let id = d.tiles()[0].id;
    let c = rest.tiles[0].rect.center();
    d.begin_drag(id, c, c, SCREEN, t);
    let l = d.layout(SCREEN, t);
    assert!(l
        .tiles
        .iter()
        .all(|p| p.kind == TileKind::Separator || p.rect.w >= d.cfg.tile_size - 1e-9));
    let t2 = settle(&mut d, t);
    let l = d.layout(SCREEN, t2);
    assert!(l
        .tiles
        .iter()
        .all(|p| p.kind == TileKind::Separator || (p.rect.w - d.cfg.tile_size).abs() < 1e-6));
    d.pointer_moved(Some(Point::new(100.0, 100.0)), SCREEN, t2);
    assert!(d.is_dragging());
}

#[test]
fn pin_and_unpin() {
    let mut d = dock_with(base_cfg());
    d.pin("editor", Some(1), 0.0);
    assert_eq!(app_order(&d), vec!["files", "editor", "browser", "mail"]);
    d.pin("mail", Some(0), 0.0);
    assert_eq!(app_order(&d), vec!["mail", "files", "editor", "browser"]);
    d.unpin("editor", 1.0);
    d.tick(5.0);
    assert_eq!(app_order(&d), vec!["mail", "files", "browser"]);
    d.app_started("files", 6.0);
    d.unpin("files", 6.0);
    assert!(
        d.app_tile("files").is_some(),
        "running apps stay until they quit"
    );
}

#[test]
fn trash_state_and_tile_lookup() {
    let mut d = dock_with(base_cfg());
    assert!(!d.tiles().last().unwrap().full);
    d.set_trash_full(true);
    let last = d.tiles().last().unwrap().id;
    assert!(d.tile(last).unwrap().full);
    assert_eq!(d.tile(last).unwrap().kind, TileKind::Trash);
    assert!(d.tile(TileId(12345)).is_none());
}

#[test]
fn wants_tick_false_when_idle() {
    let mut d = dock_with(base_cfg());
    assert!(!d.wants_tick(0.0));
    d.app_started("x", 0.0);
    assert!(d.wants_tick(0.01));
    let t = settle(&mut d, 0.0);
    assert!(!d.wants_tick(t));
}

#[test]
fn parametric_bounce_model_is_still_available() {
    let mut d = dock_with(base_cfg());
    d.bounce = BounceConfig::parametric();
    d.app_launching("mail", 0.0);
    let id = d.app_tile("mail").unwrap().id;
    let lift = d
        .layout(SCREEN, d.bounce.period / 2.0)
        .tiles
        .iter()
        .find(|t| t.id == id)
        .unwrap()
        .lift;
    assert!((lift - d.bounce.height * d.cfg.tile_size).abs() < 1e-9);
}

#[test]
fn attention_bounce_lifts_by_the_full_tile_height() {
    let mut d = dock_with(base_cfg());
    d.app_started("mail", 0.0);
    d.attention("mail", true, 0.0);
    let id = d.app_tile("mail").unwrap().id;
    let lift = |now| {
        d.layout(SCREEN, now)
            .tiles
            .iter()
            .find(|t| t.id == id)
            .unwrap()
            .lift
    };
    assert!(
        (lift(0.25) - 49.0).abs() < 1e-9,
        "len + 1 at the first peak"
    );
    assert!(
        (lift(0.663) - 24.5).abs() < 1e-9,
        "second hop has half the amplitude"
    );
    assert_eq!(lift(1.5), 0.0);
}

#[test]
fn launch_animation_can_be_disabled() {
    let mut d = dock_with(DockConfig {
        launch_animation: false,
        ..base_cfg()
    });
    d.app_launching("mail", 0.0);
    let id = d.app_tile("mail").unwrap().id;
    let l = d.layout(SCREEN, 0.35);
    let t = l.tiles.iter().find(|t| t.id == id).unwrap();
    assert_eq!(t.lift, 0.0);
    assert_eq!(t.lifecycle, TileLifecycle::Idle);
}

#[test]
fn verified_defaults() {
    let c = DockConfig::default();
    assert_eq!(
        (c.tile_size, c.large_size, c.magnification),
        (64.0, 128.0, false)
    );
    assert_eq!(c.autohide_delay, 0.22);
    assert!(
        !c.show_recents,
        "the recents section is off until asked for"
    );
    assert_eq!(c.autohide_time_modifier, 1.0);
    assert!(!c.autohide && c.launch_animation);
    assert_eq!(c.minimize_effect, MinimizeEffect::WarpMinimize);
    assert_eq!(DockConfig::for_display_height(800.0).tile_size, 52.0);
    assert_eq!(DockConfig::for_display_height(801.0).tile_size, 64.0);
    assert_eq!(default_tile_size(1080.0), 64.0);
    assert_eq!(sanitize_tile_size(Some(300.0), 1080.0), 64.0);
    assert_eq!(sanitize_tile_size(Some(0.0), 700.0), 52.0);
    assert_eq!(sanitize_tile_size(Some(40.2), 1080.0), 41.0);
    assert_eq!(sanitize_large_size(Some(513.0)), 128.0);
    assert_eq!(sanitize_large_size(Some(200.5)), 201.0);
    let p = DockConfig::from_preferences(None, Some(256.0), Some(true), 900.0);
    assert_eq!(
        (p.tile_size, p.large_size, p.magnification),
        (64.0, 256.0, true)
    );
}

#[test]
fn minimize_durations_are_verified() {
    assert_eq!(MinimizeEffect::WarpMinimize.duration_ms(), 450.0);
    assert_eq!(MinimizeEffect::Scale.duration_ms(), 200.0);
    assert_eq!(MinimizeEffect::FunnelMinimize.duration_ms(), 400.0);
    assert_eq!(MinimizeEffect::WarpMinimize.duration_s(true), 0.9);
    assert_eq!(DockConfig::default().minimize_duration_s(false), 0.45);
}

#[test]
fn default_dock_hovers_exactly_the_large_size_with_the_verified_fisheye() {
    let mut d = dock_with(DockConfig {
        magnification: true,
        show_recents: true,
        ..DockConfig::default()
    });
    let rest = d.layout(SCREEN, 0.0);
    let target = rest.tiles[1].clone();
    d.pointer_moved(Some(target.rect.center()), SCREEN, 0.0);
    let t = settle(&mut d, 0.0);
    let l = d.layout(SCREEN, t);
    let big = l.tiles.iter().find(|p| p.id == target.id).unwrap();
    assert!((big.rect.w - 128.0).abs() < 1e-6, "{}", big.rect.w);
    assert!((big.rect.h - 128.0).abs() < 1e-6);
    // the dock grows by twice the real scale: (128 − 64) / 2 / sin(π/12) on either side
    let growth = l.bar.w - rest.bar.w;
    assert!(growth > 0.0 && growth <= 2.0 * 123.65 + 1e-6, "{growth}");
}

#[test]
fn cosine_profile_remains_selectable() {
    let mut d = dock_with(base_cfg());
    d.set_profile(Box::new(CosineProfile));
    let rest = d.layout(SCREEN, 0.0);
    d.pointer_moved(Some(rest.tiles[1].rect.center()), SCREEN, 0.0);
    let t = settle(&mut d, 0.0);
    let big = d.layout(SCREEN, t).tiles[1].rect.w;
    assert!((big - 128.0).abs() < 3.0);
}

#[test]
fn next_minimized_slot_is_where_the_next_minimized_tile_lands() {
    let mut d = dock_with(base_cfg());
    let slot = d.next_minimized_slot(SCREEN);
    d.window_minimized(WindowId(1), "mail", "a", 0.0);
    let t = settle(&mut d, 0.0);
    let id = d
        .tiles()
        .iter()
        .find(|t| t.window == Some(WindowId(1)))
        .unwrap()
        .id;
    let tile = d
        .layout(SCREEN, t)
        .tiles
        .iter()
        .find(|p| p.id == id)
        .unwrap()
        .rect;
    assert!(rects_eq(slot, tile), "{slot:?} vs {tile:?}");
    assert!(
        d.next_minimized_slot(SCREEN).x > slot.x,
        "the slot after it is further along"
    );
}

#[test]
fn the_exclusive_zone_is_the_bar_and_its_margin_and_none_while_hiding() {
    let d = Dock::new(DockConfig {
        tile_size: 48.0,
        padding_ratio: 6.0 / 48.0,
        edge_padding_ratio: 6.0 / 48.0,
        edge_margin: 4.0,
        ..DockConfig::default()
    });
    assert_eq!(d.exclusive_zone(), 64.0);
    assert_eq!(
        Dock::new(DockConfig {
            autohide: true,
            ..DockConfig::default()
        })
        .exclusive_zone(),
        0.0
    );
}

#[test]
fn the_surface_extent_holds_a_magnified_bouncing_tile() {
    for orientation in [Orientation::Bottom, Orientation::Left, Orientation::Right] {
        let mut d = dock_with(DockConfig {
            orientation,
            ..base_cfg()
        });
        let surface = match orientation {
            Orientation::Bottom => {
                Rect::new(0.0, 1080.0 - d.surface_extent(), 1920.0, d.surface_extent())
            }
            Orientation::Left => Rect::new(0.0, 0.0, d.surface_extent(), 1080.0),
            Orientation::Right => {
                Rect::new(1920.0 - d.surface_extent(), 0.0, d.surface_extent(), 1080.0)
            }
        };
        d.app_launching("files", 0.0);
        d.attention("browser", true, 0.0);
        let hover = rect_of(&d, "files", 0.0).center();
        let mut worst = 0.0f64;
        for step in 0..400 {
            let now = f64::from(step) * 0.01;
            d.pointer_moved(Some(hover), SCREEN, now);
            for t in d
                .layout(SCREEN, now)
                .tiles
                .iter()
                .filter(|t| t.kind == TileKind::App)
            {
                let r = t.visible_rect(orientation);
                let inside = surface
                    .intersection(&r)
                    .is_some_and(|i| (i.w - r.w).abs() < 1e-6 && (i.h - r.h).abs() < 1e-6);
                assert!(
                    inside || t.presence == 0.0,
                    "{orientation:?} step {step}: {r:?} outside {surface:?}"
                );
                worst = worst.max(t.lift);
            }
        }
        assert!(worst > 40.0, "the attention bounce was exercised");
    }
}

#[test]
fn input_goes_to_the_bar_and_its_tiles_or_only_the_reveal_strip_while_hidden() {
    let mut d = dock_with(DockConfig {
        autohide: true,
        ..base_cfg()
    });
    let hidden = d.input_rects(SCREEN, 0.0);
    assert_eq!(
        hidden,
        vec![Rect::new(
            0.0,
            SCREEN.bottom() - d.cfg.reveal_thickness,
            SCREEN.w,
            d.cfg.reveal_thickness
        )]
    );
    d.set_revealed(true, 1.0);
    let shown = d.input_rects(SCREEN, 5.0);
    let layout = d.layout(SCREEN, 5.0);
    assert_eq!(shown[0], layout.bar);
    assert_eq!(
        shown.len(),
        1 + 1 + 3 + 1,
        "the bar, the strip between it and the screen edge, three apps and the trash; separators take no input"
    );
    assert!(shown
        .iter()
        .all(|r| r.y < SCREEN.bottom() - d.cfg.reveal_thickness));
}

#[test]
fn an_auto_hiding_dock_wants_frames_until_it_has_settled() {
    let mut d = dock_with(DockConfig {
        autohide: true,
        ..base_cfg()
    });
    assert!(!d.wants_tick(0.0), "hidden and idle");
    let edge = Some(Point::new(960.0, 1079.5));
    d.pointer_moved(edge, SCREEN, 0.0);
    assert!(d.wants_tick(0.01), "waiting out the reveal delay");
    d.pointer_moved(edge, SCREEN, 0.2);
    assert!(d.wants_tick(0.21), "sliding in");
    let mut now = 0.2;
    while d.wants_tick(now) {
        now += 0.01;
        d.pointer_moved(edge, SCREEN, now);
        assert!(now < 5.0, "never settles");
    }
    assert!(
        d.layout(SCREEN, now).bar.bottom() <= SCREEN.bottom(),
        "settled shown"
    );
}

/// The pins file Rafael's dock wrote before the file manager was built in.
const RAFAEL_PINS: &str = "# written by cupertino-dock; replaces the pins of dock.conf\nfoot\ngoogle-chrome\norg.gnome.Nautilus\nmpv\n";

fn with_file_manager(file_manager: &str, pins: &str) -> Dock {
    let mut d = Dock::new(base_cfg());
    d.set_built_in(file_manager);
    let pins = cupertino_dock::conf::parse_pins(pins);
    d.set_pinned(&pins.iter().map(String::as_str).collect::<Vec<_>>());
    d
}

#[test]
fn the_session_file_manager_is_the_first_tile_of_an_older_pins_file() {
    let d = with_file_manager("cupertino-files", RAFAEL_PINS);
    assert_eq!(
        app_order(&d),
        [
            "cupertino-files",
            "foot",
            "google-chrome",
            "org.gnome.Nautilus",
            "mpv"
        ]
    );
    assert_eq!(
        d.pinned_keys(),
        ["foot", "google-chrome", "org.gnome.Nautilus", "mpv"],
        "the built-in tile is not a pin"
    );
    // without the session's own, the first installed file manager is built in and its pin is dropped
    let d = with_file_manager("org.gnome.Nautilus", RAFAEL_PINS);
    assert_eq!(
        app_order(&d),
        ["org.gnome.Nautilus", "foot", "google-chrome", "mpv"]
    );
    assert_eq!(d.pinned_keys(), ["foot", "google-chrome", "mpv"]);
}

#[test]
fn the_file_manager_tile_stays_first_and_cannot_be_removed_or_moved() {
    let mut d = with_file_manager("cupertino-files", RAFAEL_PINS);
    let id = d.app_tile("cupertino-files").unwrap().id;
    let c = rect_of(&d, "cupertino-files", 0.0).center();
    assert!(!d.begin_drag(id, c, c, SCREEN, 0.0), "it does not drag");
    d.unpin("cupertino-files", 0.0);
    d.app_started("cupertino-files", 0.0);
    d.app_quit("cupertino-files", 1.0);
    let t = settle(&mut d, 1.0);
    assert_eq!(
        app_order(&d)[0],
        "cupertino-files",
        "kept after Remove from Dock and after it quits"
    );
    assert!(d.app_tile("cupertino-files").unwrap().pinned);
    // another tile dropped ahead of it lands right after it
    let start = begin(&mut d, "mpv");
    let first = rect_of(&d, "cupertino-files", t);
    d.drag_move(Point::new(first.x + 1.0, start.y), SCREEN, t + 0.1);
    assert_eq!(d.drag_insertion(), Some(1));
    assert_eq!(
        d.end_drag(SCREEN, t + 0.2),
        DragOutcome::Reordered {
            key: "mpv".into(),
            from: 4,
            to: 1
        }
    );
    assert_eq!(
        app_order(&d),
        [
            "cupertino-files",
            "mpv",
            "foot",
            "google-chrome",
            "org.gnome.Nautilus"
        ]
    );
    d.pin("org.example.editor", Some(0), t + 1.0);
    assert_eq!(
        app_order(&d)[..2],
        ["cupertino-files", "org.example.editor"]
    );
    d.pin("cupertino-files", Some(3), t + 1.0);
    assert_eq!(
        app_order(&d)[0],
        "cupertino-files",
        "keeping it in the dock does not move it"
    );
}

#[test]
fn the_file_manager_tile_survives_a_live_dock_conf_change() {
    // what `App::apply_conf_change` does when dock.conf changes: new sizes, position and recents, and the auto-hide toggled
    let mut d = with_file_manager("cupertino-files", RAFAEL_PINS);
    d.cfg.tile_size = 40.0;
    d.cfg.large_size = 96.0;
    d.cfg.orientation = Orientation::Left;
    d.cfg.show_recents = false;
    d.toggle_autohide(0.0);
    let t = settle(&mut d, 0.0);
    assert_eq!(
        app_order(&d),
        [
            "cupertino-files",
            "foot",
            "google-chrome",
            "org.gnome.Nautilus",
            "mpv"
        ]
    );
    assert_eq!(
        d.pinned_keys(),
        ["foot", "google-chrome", "org.gnome.Nautilus", "mpv"]
    );
    let fm = d.app_tile("cupertino-files").unwrap().id;
    let first = d
        .layout(SCREEN, t)
        .tiles
        .into_iter()
        .find(|p| p.kind == TileKind::App)
        .unwrap();
    assert_eq!(first.id, fm, "still the first tile of the left dock");
}

// ---- the bar's geometry at the default size ---------------------------------------------------------------------

/// Seven applications, the separator, a minimized window and the trash at the default tile size.
fn default_dock() -> Dock {
    let mut d = Dock::new(DockConfig::default());
    d.set_pinned(&["a", "b", "c", "d", "e", "f", "g"]);
    d.window_minimized(WindowId(1), "a", "w", 0.0);
    settle(&mut d, 0.0);
    d
}

#[test]
fn the_default_bar_is_84_pt_deep_5_pt_above_the_screen_edge_with_its_tiles_inside_it() {
    let d = default_dock();
    let l = d.layout(SCREEN, 100.0);
    assert_eq!(l.bar.h, 84.0);
    assert_eq!(l.bar.bottom(), 1080.0 - 5.0);
    for t in l.tiles.iter().filter(|t| t.kind != TileKind::Separator) {
        assert_eq!((t.rect.w, t.rect.h), (64.0, 64.0));
        assert_eq!(t.rect.y - l.bar.y, 10.0, "padding above");
        assert_eq!(
            l.bar.bottom() - t.rect.bottom(),
            10.0,
            "padding below, on the screen edge's side"
        );
    }
    let first = &l.tiles[0];
    let last = l.tiles.last().unwrap();
    assert_eq!(first.rect.x - l.bar.x, 10.0, "padding at the leading end");
    assert_eq!(
        l.bar.right() - last.rect.right(),
        10.0,
        "padding at the trailing end"
    );
    assert_eq!(d.exclusive_zone(), 89.0, "the bar and its margin");
}

#[test]
fn tiles_stand_68_pt_apart_and_34_pt_across_the_separator() {
    let d = default_dock();
    let l = d.layout(SCREEN, 100.0);
    let centres: Vec<(TileKind, f64)> = l.tiles.iter().map(|t| (t.kind, t.rect.cx())).collect();
    let sep = centres
        .iter()
        .position(|c| c.0 == TileKind::Separator)
        .unwrap();
    for w in centres[..sep].windows(2) {
        assert!((w[1].1 - w[0].1 - 68.0).abs() < 1e-9, "{w:?}");
    }
    let before = &l.tiles[sep - 1].rect;
    let after = &l.tiles[sep + 1].rect;
    assert!((after.x - before.right() - 34.0).abs() < 1e-9);
    assert!(
        (l.tiles[sep].rect.cx() - (before.right() + after.x) / 2.0).abs() < 1e-9,
        "the separator's slot is in the middle of the space"
    );
    assert!((l.tiles[sep + 2].rect.cx() - l.tiles[sep + 1].rect.cx() - 68.0).abs() < 1e-9);
    // 9 tiles, 10 items with their gaps, the separator's slot and the padding
    assert!((l.bar.w - (9.0 * 64.0 + 9.0 * 4.0 + 26.0 + 20.0)).abs() < 1e-9);
}

#[test]
fn the_metrics_scale_with_the_tile_size() {
    let cfg = DockConfig {
        tile_size: 32.0,
        ..DockConfig::default()
    };
    assert_eq!(cfg.gap(), 2.0);
    assert_eq!(cfg.bar_padding(), 5.0);
    assert_eq!(cfg.edge_padding(), 5.0);
    assert_eq!(cfg.separator_extent(), 13.0);
    assert_eq!(cfg.bar_thickness(), 42.0);
    assert_eq!(cfg.edge_margin, 5.0, "the gap to the screen edge stays");
}

#[test]
fn the_pointer_in_the_gap_between_two_tiles_is_on_one_of_them() {
    let d = default_dock();
    let l = d.layout(SCREEN, 100.0);
    let (a, b) = (&l.tiles[0], &l.tiles[1]);
    for x in [a.rect.right() + 0.5, b.rect.x - 0.5] {
        let hit = d
            .hit_test(Point::new(x, a.rect.cy()), SCREEN, 100.0)
            .unwrap();
        assert!(hit.id == a.id || hit.id == b.id, "{x}");
    }
}

// ---- magnification at an 88 pt magnified size, and the launch hop, at the default tile size -----------------------

/// The magnified size the bulge tests use, over the default 64 pt tiles.
const MAGNIFIED_88: f64 = 88.0;

/// Centres of seven tiles in a row with the pointer at `c` along the unmagnified row, fully magnified to 88 pt.
fn magnified_centres(c: f64) -> Vec<f64> {
    let cfg = DockConfig {
        magnification: true,
        large_size: MAGNIFIED_88,
        ..DockConfig::default()
    };
    layout(&cfg, 7, Some(c), 1.0)
        .iter()
        .map(|f| f.pos + f.size / 2.0)
        .collect()
}

#[test]
fn the_tile_under_the_pointer_takes_the_whole_magnified_size() {
    let cfg = DockConfig {
        magnification: true,
        large_size: MAGNIFIED_88,
        ..DockConfig::default()
    };
    let f = layout(&cfg, 7, Some(3.0 * 68.0 + 32.0), 1.0);
    assert!((f[3].size - 88.0).abs() < 1e-9);
}

/// With the pointer on the fifth of seven tiles, a little past its middle, the distances between the tile centres
/// grow as a local bulge: about 141 pt over the first two, then 81, 90, 91 and 86 pt, where they are 68 at rest.
#[test]
fn magnification_is_a_local_bulge_whose_neighbours_grow_too() {
    let c = magnified_centres(4.0 * 68.0 + 47.0);
    let d: Vec<f64> = c.windows(2).map(|w| w[1] - w[0]).collect();
    assert!((d[0] + d[1] - 141.0).abs() < 2.0, "{d:?}");
    for (got, want) in d[2..].iter().zip([81.0, 90.0, 91.0, 86.0]) {
        assert!((got - want).abs() < 2.0, "{d:?}");
    }
    // far along a long row the tiles keep their places: the bulge is local
    let cfg = DockConfig {
        magnification: true,
        large_size: MAGNIFIED_88,
        ..DockConfig::default()
    };
    let row = layout(&cfg, 20, Some(2.0 * 68.0 + 32.0), 1.0);
    let far: Vec<f64> = row[8..].windows(2).map(|w| w[1].pos - w[0].pos).collect();
    assert!(
        far.iter().all(|d| (d - 68.0).abs() < 1e-9),
        "far from the pointer they keep their places: {far:?}"
    );
}

#[test]
fn the_launch_hop_lifts_a_64_pt_tile_17_9_pt_at_355_ms_and_lands_at_710_ms() {
    let mut d = Dock::new(DockConfig::default());
    d.set_pinned(&["a"]);
    d.app_launching("a", 0.0);
    d.app_started("a", 0.1);
    let lift = |d: &Dock, t: f64| {
        d.layout(SCREEN, t)
            .tiles
            .iter()
            .find(|p| p.kind == TileKind::App)
            .unwrap()
            .lift
    };
    assert!((lift(&d, 0.355) - 17.9).abs() < 1e-9);
    // a parabola: three quarters of the height half way up and half way down
    assert!((lift(&d, 0.1775) - 0.75 * 17.9).abs() < 1e-9);
    assert!((lift(&d, 0.5325) - 0.75 * 17.9).abs() < 1e-9);
    assert_eq!(lift(&d, 0.0), 0.0);
    assert_eq!(lift(&d, 0.71), 0.0);
    assert_eq!(
        lift(&d, 0.8),
        0.0,
        "one hop: the window came during the first"
    );
}
