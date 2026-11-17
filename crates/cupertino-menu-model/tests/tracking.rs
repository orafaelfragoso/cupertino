//! Pointer tracking: what a release does, and aiming into a submenu.

use cupertino_menu_model::tracking::{
    point_in_triangle, Edge, Release, SubmenuAim, Tracking, Under, SUBMENU_AIM_PAUSE,
    TRACKING_INTERVAL,
};

/// A named release: how tracking started, when and over what it ends, and what it does.
type ReleaseCase = (&'static str, fn() -> Tracking, f64, Under, Release);

fn from_title() -> Tracking {
    Tracking::from_title(0.0)
}

fn reached() -> Tracking {
    let mut t = Tracking::from_title(0.0);
    t.reach();
    t
}

fn in_menu() -> Tracking {
    Tracking::in_menu(0.0)
}

#[test]
fn a_release_stays_closes_or_chooses_by_where_and_when_it_happens() {
    use Release::{Choose, Close, Stay};
    let cases: [ReleaseCase; 15] = [
        (
            "quick click on the title",
            from_title,
            0.2,
            Under::Title,
            Stay,
        ),
        (
            "inside the interval",
            from_title,
            TRACKING_INTERVAL - 1e-9,
            Under::Title,
            Stay,
        ),
        (
            "held to the interval",
            from_title,
            TRACKING_INTERVAL,
            Under::Title,
            Close,
        ),
        (
            "held long on the title",
            from_title,
            2.0,
            Under::Title,
            Close,
        ),
        (
            "quick on a row never reached",
            from_title,
            0.1,
            Under::Item,
            Stay,
        ),
        (
            "long on a row never reached",
            from_title,
            0.6,
            Under::Item,
            Choose,
        ),
        (
            "dragged onto a reached row",
            reached,
            0.1,
            Under::Item,
            Choose,
        ),
        ("quick on the menu", reached, 0.1, Under::Menu, Stay),
        ("long on the menu", reached, 5.0, Under::Menu, Stay),
        ("quick outside", from_title, 0.1, Under::Outside, Close),
        ("in a menu, on a row", in_menu, 0.01, Under::Item, Choose),
        ("in a menu, on the menu", in_menu, 0.01, Under::Menu, Stay),
        (
            "in a menu, quick on the title",
            in_menu,
            0.01,
            Under::Title,
            Stay,
        ),
        (
            "in a menu, long on the title",
            in_menu,
            0.8,
            Under::Title,
            Close,
        ),
        ("in a menu, outside", in_menu, 0.01, Under::Outside, Close),
    ];
    for (name, start, at, under, expected) in cases {
        assert_eq!(start().release(at, under), expected, "{name}");
    }
    assert!(!from_title().reached());
    assert!(reached().reached());
    assert!(in_menu().reached());
}

const EDGE: Edge = Edge {
    x: 100.0,
    top: 0.0,
    bottom: 100.0,
};

const LEFT: Edge = Edge {
    x: 0.0,
    top: 0.0,
    bottom: 100.0,
};

type Step = ((f64, f64), f64, Option<Edge>, bool);

#[test]
fn a_move_aims_at_the_submenu_while_it_heads_into_the_triangle_in_time() {
    let pause = SUBMENU_AIM_PAUSE;
    let cases: [(&str, &[Step]); 9] = [
        ("first move", &[((10.0, 50.0), 0.0, Some(EDGE), false)]),
        (
            "towards the submenu",
            &[
                ((10.0, 50.0), 0.0, Some(EDGE), false),
                ((20.0, 52.0), 0.1, Some(EDGE), true),
                ((40.0, 55.0), 0.2, Some(EDGE), true),
            ],
        ),
        (
            "out of the triangle",
            &[
                ((10.0, 50.0), 0.0, Some(EDGE), false),
                ((20.0, 52.0), 0.1, Some(EDGE), true),
                ((30.0, 5.0), 0.2, Some(EDGE), false),
            ],
        ),
        (
            "away from the submenu",
            &[
                ((50.0, 50.0), 0.0, Some(EDGE), false),
                ((10.0, 50.0), 0.1, Some(EDGE), false),
            ],
        ),
        (
            "no open submenu",
            &[
                ((10.0, 50.0), 0.0, None, false),
                ((20.0, 50.0), 0.1, None, false),
            ],
        ),
        (
            "resting within then past the pause",
            &[
                ((10.0, 50.0), 0.0, Some(EDGE), false),
                ((20.0, 50.0), 0.1, Some(EDGE), true),
                ((20.0, 50.0), 0.4, Some(EDGE), true),
                ((20.0, 50.0), 0.1 + pause + 0.01, Some(EDGE), false),
            ],
        ),
        (
            "a move after the pause",
            &[
                ((10.0, 50.0), 0.0, Some(EDGE), false),
                ((20.0, 50.0), 0.1, Some(EDGE), true),
                ((30.0, 50.0), 1.0, Some(EDGE), false),
            ],
        ),
        (
            "a rest without an earlier aim",
            &[
                ((10.0, 50.0), 0.0, Some(EDGE), false),
                ((10.0, 50.0), 0.1, Some(EDGE), false),
            ],
        ),
        (
            "towards a submenu on the left",
            &[
                ((90.0, 50.0), 0.0, Some(LEFT), false),
                ((70.0, 50.0), 0.1, Some(LEFT), true),
            ],
        ),
    ];
    for (name, steps) in cases {
        let mut aim = SubmenuAim::default();
        let mut last_aim = None;
        let mut from = None;
        for (i, &(to, at, edge, expected)) in steps.iter().enumerate() {
            assert_eq!(aim.moved(to, at, edge), expected, "{name}, step {i}");
            if expected && from != Some(to) {
                last_aim = Some(at);
            }
            match last_aim {
                Some(since) if expected => {
                    let deadline = aim.deadline().expect("an aim has a deadline");
                    assert!(
                        (deadline - (since + pause)).abs() < 1e-12,
                        "{name}, step {i}"
                    );
                }
                None => assert_eq!(aim.deadline(), None, "{name}, step {i}"),
                Some(_) => {}
            }
            from = Some(to);
        }
    }
}

#[test]
fn the_aim_expires_at_its_deadline_and_a_reset_forgets_it() {
    let mut aim = SubmenuAim::default();
    aim.moved((10.0, 50.0), 0.0, Some(EDGE));
    aim.moved((20.0, 50.0), 0.1, Some(EDGE));
    let deadline = aim.deadline().unwrap();
    assert!(!aim.expired(deadline - 0.01));
    assert!(aim.expired(deadline));
    assert_eq!(aim.deadline(), None);
    assert!(!aim.expired(deadline + 1.0));

    aim.moved((10.0, 50.0), 2.0, Some(EDGE));
    aim.moved((20.0, 50.0), 2.1, Some(EDGE));
    aim.reset();
    assert_eq!(aim, SubmenuAim::default());
    assert!(!aim.moved((30.0, 50.0), 2.2, Some(EDGE)));
}

#[test]
fn a_point_is_in_a_triangle_inside_or_on_its_edges_in_either_winding() {
    let (a, b, c) = ((0.0, 0.0), (10.0, 0.0), (0.0, 10.0));
    for (p, inside) in [
        ((2.0, 2.0), true),
        ((5.0, 0.0), true),
        ((5.0, 5.0), true),
        (a, true),
        (c, true),
        ((8.0, 8.0), false),
        ((-1.0, 1.0), false),
    ] {
        assert_eq!(point_in_triangle(p, a, b, c), inside, "{p:?}");
        assert_eq!(point_in_triangle(p, a, c, b), inside, "{p:?} reversed");
    }
}
