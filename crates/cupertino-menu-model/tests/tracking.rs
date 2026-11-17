//! Pointer tracking: what a release does, and aiming into a submenu.

use cupertino_menu_model::tracking::{
    point_in_triangle, Edge, Release, SubmenuAim, Tracking, Under, SUBMENU_AIM_PAUSE,
    SUBMENU_OPEN_DELAY, TRACKING_INTERVAL,
};

#[test]
fn the_timing_constants_are_documented() {
    assert_eq!(TRACKING_INTERVAL, 0.5);
    assert_eq!(SUBMENU_OPEN_DELAY, 0.2);
    assert_eq!(SUBMENU_AIM_PAUSE, 0.375);
}

// presses on a title

#[test]
fn a_quick_click_on_the_title_leaves_the_menu_open() {
    let t = Tracking::from_title(10.0);
    assert_eq!(t.release(10.2, Under::Title), Release::Stay);
}

#[test]
fn a_press_held_on_the_title_closes_the_menu_when_let_go_there() {
    let t = Tracking::from_title(10.0);
    assert_eq!(t.release(10.5, Under::Title), Release::Close);
    assert_eq!(t.release(12.0, Under::Title), Release::Close);
}

#[test]
fn the_interval_is_inclusive() {
    let t = Tracking::from_title(0.0);
    assert_eq!(
        t.release(TRACKING_INTERVAL - 1e-9, Under::Title),
        Release::Stay
    );
    assert_eq!(t.release(TRACKING_INTERVAL, Under::Title), Release::Close);
}

#[test]
fn a_drag_from_the_title_onto_a_row_chooses_it_on_release() {
    let mut t = Tracking::from_title(0.0);
    assert!(!t.reached());
    t.reach();
    assert!(t.reached());
    assert_eq!(t.release(0.1, Under::Item), Release::Choose);
}

#[test]
fn a_quick_release_on_a_row_never_reached_stays_open() {
    let t = Tracking::from_title(0.0);
    assert_eq!(t.release(0.1, Under::Item), Release::Stay);
}

#[test]
fn a_long_press_released_on_a_row_chooses_it_even_if_never_reached() {
    let t = Tracking::from_title(0.0);
    assert_eq!(t.release(0.6, Under::Item), Release::Choose);
}

#[test]
fn releasing_on_other_parts_of_the_menu_stays_open() {
    let mut t = Tracking::from_title(0.0);
    t.reach();
    assert_eq!(t.release(0.1, Under::Menu), Release::Stay);
    assert_eq!(t.release(5.0, Under::Menu), Release::Stay);
}

#[test]
fn releasing_away_from_the_menus_closes_them() {
    let t = Tracking::from_title(0.0);
    assert_eq!(t.release(0.1, Under::Outside), Release::Close);
    assert_eq!(t.release(5.0, Under::Outside), Release::Close);
}

// presses in an open menu

#[test]
fn a_press_in_an_open_menu_chooses_where_it_is_let_go() {
    let t = Tracking::in_menu(0.0);
    assert!(t.reached());
    assert_eq!(t.release(0.01, Under::Item), Release::Choose);
}

#[test]
fn a_press_in_an_open_menu_let_go_on_the_menu_or_a_quick_title_stays() {
    let t = Tracking::in_menu(0.0);
    assert_eq!(t.release(0.01, Under::Menu), Release::Stay);
    assert_eq!(t.release(0.01, Under::Title), Release::Stay);
    assert_eq!(t.release(0.8, Under::Title), Release::Close);
    assert_eq!(t.release(0.01, Under::Outside), Release::Close);
}

// aiming into a submenu

const EDGE: Edge = Edge {
    x: 100.0,
    top: 0.0,
    bottom: 100.0,
};

#[test]
fn the_first_move_does_not_head_anywhere() {
    let mut aim = SubmenuAim::default();
    assert!(!aim.moved((10.0, 50.0), 0.0, Some(EDGE)));
    assert_eq!(aim.deadline(), None);
}

#[test]
fn a_move_into_the_triangle_towards_the_submenu_keeps_aiming() {
    let mut aim = SubmenuAim::default();
    aim.moved((10.0, 50.0), 0.0, Some(EDGE));
    assert!(aim.moved((20.0, 52.0), 0.1, Some(EDGE)));
    assert!((aim.deadline().unwrap() - (0.1 + SUBMENU_AIM_PAUSE)).abs() < 1e-12);
    assert!(aim.moved((40.0, 55.0), 0.2, Some(EDGE)));
}

#[test]
fn a_move_outside_the_triangle_gives_the_submenu_up() {
    let mut aim = SubmenuAim::default();
    aim.moved((10.0, 50.0), 0.0, Some(EDGE));
    assert!(aim.moved((20.0, 52.0), 0.1, Some(EDGE)));
    assert!(!aim.moved((30.0, 5.0), 0.2, Some(EDGE)));
    assert_eq!(aim.deadline(), None);
}

#[test]
fn a_move_away_from_the_submenu_does_not_aim() {
    let mut aim = SubmenuAim::default();
    aim.moved((50.0, 50.0), 0.0, Some(EDGE));
    assert!(!aim.moved((10.0, 50.0), 0.1, Some(EDGE)));
}

#[test]
fn without_an_open_submenu_nothing_is_aimed_at() {
    let mut aim = SubmenuAim::default();
    aim.moved((10.0, 50.0), 0.0, None);
    assert!(!aim.moved((20.0, 50.0), 0.1, None));
}

#[test]
fn resting_within_the_pause_keeps_aiming_and_resting_past_it_does_not() {
    let mut aim = SubmenuAim::default();
    aim.moved((10.0, 50.0), 0.0, Some(EDGE));
    aim.moved((20.0, 50.0), 0.1, Some(EDGE));
    assert!(aim.moved((20.0, 50.0), 0.4, Some(EDGE)));
    assert!(!aim.moved((20.0, 50.0), 0.1 + SUBMENU_AIM_PAUSE + 0.01, Some(EDGE)));
}

#[test]
fn a_move_after_the_pause_does_not_aim_even_inside_the_triangle() {
    let mut aim = SubmenuAim::default();
    aim.moved((10.0, 50.0), 0.0, Some(EDGE));
    aim.moved((20.0, 50.0), 0.1, Some(EDGE));
    assert!(!aim.moved((30.0, 50.0), 1.0, Some(EDGE)));
}

#[test]
fn a_rest_without_an_earlier_aim_does_not_aim() {
    let mut aim = SubmenuAim::default();
    aim.moved((10.0, 50.0), 0.0, Some(EDGE));
    assert!(!aim.moved((10.0, 50.0), 0.1, Some(EDGE)));
}

#[test]
fn the_aim_expires_once_the_deadline_passes() {
    let mut aim = SubmenuAim::default();
    aim.moved((10.0, 50.0), 0.0, Some(EDGE));
    aim.moved((20.0, 50.0), 0.1, Some(EDGE));
    let deadline = aim.deadline().unwrap();
    assert!(!aim.expired(deadline - 0.01));
    assert!(aim.expired(deadline));
    assert_eq!(aim.deadline(), None);
    assert!(!aim.expired(deadline + 1.0));
}

#[test]
fn resetting_forgets_the_last_point_and_the_aim() {
    let mut aim = SubmenuAim::default();
    aim.moved((10.0, 50.0), 0.0, Some(EDGE));
    aim.moved((20.0, 50.0), 0.1, Some(EDGE));
    aim.reset();
    assert_eq!(aim, SubmenuAim::default());
    assert!(!aim.moved((30.0, 50.0), 0.2, Some(EDGE)));
}

#[test]
fn aiming_works_towards_a_submenu_on_the_left() {
    let left = Edge {
        x: 0.0,
        top: 0.0,
        bottom: 100.0,
    };
    let mut aim = SubmenuAim::default();
    aim.moved((90.0, 50.0), 0.0, Some(left));
    assert!(aim.moved((70.0, 50.0), 0.1, Some(left)));
}

// triangle

#[test]
fn a_point_inside_a_triangle_is_in_it() {
    assert!(point_in_triangle(
        (2.0, 2.0),
        (0.0, 0.0),
        (10.0, 0.0),
        (0.0, 10.0)
    ));
}

#[test]
fn a_point_outside_a_triangle_is_not_in_it() {
    assert!(!point_in_triangle(
        (8.0, 8.0),
        (0.0, 0.0),
        (10.0, 0.0),
        (0.0, 10.0)
    ));
    assert!(!point_in_triangle(
        (-1.0, 1.0),
        (0.0, 0.0),
        (10.0, 0.0),
        (0.0, 10.0)
    ));
}

#[test]
fn edges_and_vertices_count_as_inside() {
    let (a, b, c) = ((0.0, 0.0), (10.0, 0.0), (0.0, 10.0));
    assert!(point_in_triangle((5.0, 0.0), a, b, c));
    assert!(point_in_triangle((5.0, 5.0), a, b, c));
    assert!(point_in_triangle(a, a, b, c));
    assert!(point_in_triangle(c, a, b, c));
}

#[test]
fn the_winding_of_the_triangle_does_not_matter() {
    let (a, b, c) = ((0.0, 0.0), (10.0, 0.0), (0.0, 10.0));
    for p in [(2.0, 2.0), (8.0, 8.0), (5.0, 5.0)] {
        assert_eq!(point_in_triangle(p, a, b, c), point_in_triangle(p, a, c, b));
    }
}
