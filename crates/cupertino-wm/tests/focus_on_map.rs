//! The focus-on-map rule: a window that maps takes the keyboard only when there is nothing to take it from, when it belongs
//! to the key window or the front application, when its client presented a fresh valid grant, or when it is the first
//! window of a client the person has not left; any other opens behind the key window and its tile asks for attention.
mod common;
use common::*;
use cupertino_wm::*;

fn rect() -> Rect {
    Rect::new(200.0, 200.0, 400.0, 300.0)
}

impl H {
    fn map_with(
        &mut self,
        app: &str,
        parent: Option<WindowId>,
        facts: MapFocus,
    ) -> (WindowId, Vec<Effect>) {
        let id = WindowId(self.next);
        self.next += 1;
        let params = MapParams {
            parent,
            activation: Activation::Policy(facts),
            ..MapParams::new(id, app, rect())
        };
        let fx = self.cmd(Command::Map(params));
        (id, fx)
    }
}

fn attention(fx: &[Effect]) -> bool {
    has(fx, |e| matches!(e, Effect::AppAttention { .. }))
}

#[test]
fn nothing_is_taken_from_an_empty_desktop() {
    let mut h = H::new();
    let (a, fx) = h.map_with("a", None, MapFocus::default());
    assert_eq!(h.wm.key_window(), Some(a));
    assert!(!attention(&fx));
}

#[test]
fn a_window_of_the_front_application_and_a_sheet_of_the_key_window_take_the_focus() {
    let mut h = H::new();
    let a1 = h.map("a");
    let (a2, _) = h.map_with("a", None, MapFocus::default());
    assert_eq!(
        h.wm.key_window(),
        Some(a2),
        "the application is already in front"
    );
    let b = h.map("b");
    let (sheet, fx) = h.map_with("c", Some(b), MapFocus::default());
    assert_eq!(
        h.wm.key_window(),
        Some(sheet),
        "it belongs to the key window"
    );
    assert!(!attention(&fx));
    let _ = a1;
}

#[test]
fn a_window_nobody_asked_for_opens_behind_the_key_window_and_its_tile_asks_for_attention() {
    let mut h = H::new();
    let a = h.map("a");
    let b = h.map("b");
    let (c1, _) = h.map_with(
        "c",
        None,
        MapFocus {
            first_of_client: true,
            input_elsewhere: true,
            ..MapFocus::default()
        },
    );
    let (c2, fx) = h.map_with("c", None, MapFocus::default());
    assert_eq!(
        h.wm.key_window(),
        Some(b),
        "the keyboard stays where it was"
    );
    assert!(attention(&fx));
    let order = h.wm.stacking_order();
    let at = |w: WindowId| order.iter().position(|x| *x == w).unwrap();
    assert!(
        at(c1) < at(b) && at(c2) < at(b),
        "behind the key window: {order:?}"
    );
    assert!(at(a) < at(b));
}

#[test]
fn a_fresh_valid_grant_brings_the_window_forward() {
    let mut h = H::new();
    let _a = h.map("a");
    h.now = 100.0;
    let grant = |created: f64, valid: bool| MapFocus {
        grant: Some(Grant { created, valid }),
        ..MapFocus::default()
    };
    let (fresh, fx) = h.map_with("b", None, grant(98.0, true));
    assert_eq!(h.wm.key_window(), Some(fresh));
    assert!(!attention(&fx));

    // too old: made 11 s ago
    let (old, fx) = h.map_with("c", None, grant(89.0, true));
    assert_ne!(h.wm.key_window(), Some(old), "an expired grant");
    assert!(attention(&fx));

    // made by a client that had no input to answer
    let (unearned, fx) = h.map_with("d", None, grant(99.0, false));
    assert_ne!(
        h.wm.key_window(),
        Some(unearned),
        "a grant that was never valid"
    );
    assert!(attention(&fx));
}

#[test]
fn a_grant_older_than_the_applications_last_activation_is_dead_and_a_newer_one_lives() {
    let mut h = H::new();
    h.now = 40.0;
    let _b = h.map("b");
    h.now = 50.0;
    let _a = h.map("a");
    h.now = 52.0;
    let _b2 = h.map("b");
    h.now = 53.0;
    let facts = |created: f64| MapFocus {
        grant: Some(Grant {
            created,
            valid: true,
        }),
        ..MapFocus::default()
    };
    // made at 45, before a was last active at 50, and young enough: dead all the same
    let (dead, fx) = h.map_with("a", None, facts(45.0));
    assert_ne!(h.wm.key_window(), Some(dead));
    assert!(attention(&fx));
    // made at 51, after
    let (alive, _) = h.map_with("a", None, facts(51.0));
    assert_eq!(h.wm.key_window(), Some(alive));
}

#[test]
fn the_first_window_of_a_new_client_comes_to_the_front_unless_the_person_moved_on() {
    let mut h = H::new();
    let a = h.map("a");
    // a command typed in a terminal: no other input since the program connected
    let (typed, fx) = h.map_with(
        "new",
        None,
        MapFocus {
            first_of_client: true,
            ..MapFocus::default()
        },
    );
    assert_eq!(h.wm.key_window(), Some(typed));
    assert!(!attention(&fx));
    // the same launch, but the person clicked into another application before the window mapped
    let (moved_on, fx) = h.map_with(
        "other",
        None,
        MapFocus {
            first_of_client: true,
            input_elsewhere: true,
            ..MapFocus::default()
        },
    );
    assert_eq!(h.wm.key_window(), Some(typed));
    assert!(attention(&fx));
    // a later window of that client follows the strict rule
    let (later, fx) = h.map_with(
        "new2",
        None,
        MapFocus {
            first_of_client: false,
            ..MapFocus::default()
        },
    );
    assert_eq!(h.wm.key_window(), Some(typed));
    assert!(attention(&fx));
    let _ = (a, moved_on, later);
}
