//! Applications take the keyboard in most-recently-used order, hide and show, and are switched with the app switcher.

mod common;

use common::*;
use cupertino_wm::{
    hidden_index, token_accepted, Activation, Command, Effect, Grant, LifecycleState, MapFocus,
    MapParams, SwitcherStep, GRANT_LIFETIME, LATE_ACTIVATION, TYPING_QUIET, USER_EVENT_SLACK,
};

fn three_apps() -> cupertino_wm::Wm {
    let mut wm = wm();
    open(&mut wm, 1, "a", 1.0);
    open(&mut wm, 2, "b", 2.0);
    open(&mut wm, 3, "c", 3.0);
    wm
}

#[test]
fn applications_are_listed_most_recently_used_first() {
    let mut wm = three_apps();
    let (a, b, c) = (app_of(&wm, 1), app_of(&wm, 2), app_of(&wm, 3));
    assert_eq!(wm.app_mru(), [c, b, a]);
    wm.handle(Command::ActivateApp { app: a }, 10.0);
    assert_eq!(wm.app_mru(), [a, c, b]);
    assert_eq!(wm.front_app(), Some(a));
    assert_eq!(wm.key_window(), Some(win(1)));
}

#[test]
fn activating_an_app_raises_its_most_recent_window() {
    let mut wm = wm();
    open(&mut wm, 1, "a", 1.0);
    open(&mut wm, 2, "a", 2.0);
    open(&mut wm, 3, "b", 3.0);
    let a = app_of(&wm, 1);
    wm.handle(Command::ActivateApp { app: a }, 10.0);
    assert_eq!(wm.key_window(), Some(win(2)));
    assert_eq!(wm.stacking_order().last(), Some(&win(2)));
}

#[test]
fn hiding_an_app_removes_its_windows_from_the_screen_and_moves_the_keyboard() {
    let mut wm = three_apps();
    let c = app_of(&wm, 3);
    wm.handle(Command::HideApp { app: c }, 10.0);
    assert!(wm.app(c).unwrap().hidden);
    assert!(!wm.window_on_screen(win(3)));
    assert!(wm.window_on_screen(win(2)));
    assert_eq!(wm.key_window(), Some(win(2)));
    assert_eq!(wm.front_app(), Some(app_of(&wm, 2)));
}

#[test]
fn unhiding_brings_the_app_back_in_front() {
    let mut wm = three_apps();
    let c = app_of(&wm, 3);
    wm.handle(Command::HideApp { app: c }, 10.0);
    wm.handle(Command::UnhideApp { app: c }, 11.0);
    assert!(!wm.app(c).unwrap().hidden);
    assert!(wm.window_on_screen(win(3)));
    assert_eq!(wm.front_app(), Some(c));
    assert_eq!(wm.key_window(), Some(win(3)));
}

#[test]
fn activating_a_hidden_app_shows_it() {
    let mut wm = three_apps();
    let a = app_of(&wm, 1);
    wm.handle(Command::HideApp { app: a }, 10.0);
    wm.handle(Command::ActivateApp { app: a }, 11.0);
    assert!(!wm.app(a).unwrap().hidden);
    assert!(wm.window_on_screen(win(1)));
}

#[test]
fn hide_others_hides_every_other_app_and_show_all_brings_them_back() {
    let mut wm = three_apps();
    let a = app_of(&wm, 1);
    wm.handle(Command::ActivateApp { app: a }, 10.0);
    wm.handle(Command::HideOthers { app: a }, 11.0);
    assert!(wm.window_on_screen(win(1)));
    assert!(!wm.window_on_screen(win(2)) && !wm.window_on_screen(win(3)));
    wm.handle(Command::ShowAll, 12.0);
    assert!(wm.window_on_screen(win(2)) && wm.window_on_screen(win(3)));
    assert_eq!(
        wm.key_window(),
        Some(win(1)),
        "showing does not steal the keyboard"
    );
}

#[test]
fn activating_an_app_with_only_a_minimized_window_restores_it() {
    let mut wm = three_apps();
    wm.handle(
        Command::Minimize {
            window: win(1),
            target: None,
        },
        10.0,
    );
    settle(&mut wm, 10.0);
    assert_eq!(state(&wm, 1), LifecycleState::Minimized);
    wm.handle(
        Command::ActivateApp {
            app: app_of(&wm, 1),
        },
        20.0,
    );
    settle(&mut wm, 20.0);
    assert_eq!(state(&wm, 1), LifecycleState::Normal);
    assert_eq!(wm.key_window(), Some(win(1)));
}

#[test]
fn the_app_switcher_opens_on_the_previous_app_and_commits_it() {
    let mut wm = three_apps();
    let fx = wm.handle(Command::SwitcherOpen { backward: false }, 10.0);
    let selected = fx.iter().find_map(|e| match e {
        Effect::SwitcherChanged { apps, selected } => Some((apps.clone(), *selected)),
        _ => None,
    });
    let (apps, selected) = selected.expect("the switcher shows");
    assert_eq!(apps, [app_of(&wm, 3), app_of(&wm, 2), app_of(&wm, 1)]);
    assert_eq!(selected, 1, "the previous app, not the current one");
    assert!(wm.switcher().is_some());
    let fx = wm.handle(Command::SwitcherCommit, 10.5);
    assert!(has(&fx, |e| matches!(
        e,
        Effect::SwitcherClosed { committed: true }
    )));
    assert!(wm.switcher().is_none());
    assert_eq!(wm.front_app(), Some(app_of(&wm, 2)));
    assert_eq!(wm.key_window(), Some(win(2)));
}

#[test]
fn opening_backward_selects_the_last_app() {
    let mut wm = three_apps();
    wm.handle(Command::SwitcherOpen { backward: true }, 10.0);
    assert_eq!(wm.switcher().unwrap().selected, 2);
}

#[test]
fn stepping_moves_the_selection_and_wraps_only_for_a_new_press() {
    let mut wm = three_apps();
    wm.handle(Command::SwitcherOpen { backward: false }, 10.0);
    wm.handle(
        Command::SwitcherStep {
            step: SwitcherStep::Next,
            repeat: false,
        },
        10.1,
    );
    assert_eq!(wm.switcher().unwrap().selected, 2);
    wm.handle(
        Command::SwitcherStep {
            step: SwitcherStep::Next,
            repeat: true,
        },
        10.2,
    );
    assert_eq!(
        wm.switcher().unwrap().selected,
        2,
        "a held key stops at the end"
    );
    wm.handle(
        Command::SwitcherStep {
            step: SwitcherStep::Next,
            repeat: false,
        },
        10.3,
    );
    assert_eq!(wm.switcher().unwrap().selected, 0, "a new press wraps");
    wm.handle(
        Command::SwitcherStep {
            step: SwitcherStep::Previous,
            repeat: false,
        },
        10.4,
    );
    assert_eq!(wm.switcher().unwrap().selected, 2);
    wm.handle(
        Command::SwitcherStep {
            step: SwitcherStep::First,
            repeat: false,
        },
        10.5,
    );
    assert_eq!(wm.switcher().unwrap().selected, 0);
    wm.handle(
        Command::SwitcherStep {
            step: SwitcherStep::Last,
            repeat: false,
        },
        10.6,
    );
    assert_eq!(wm.switcher().unwrap().selected, 2);
    wm.handle(Command::SwitcherSelect { index: 1 }, 10.7);
    assert_eq!(wm.switcher().unwrap().selected, 1);
}

#[test]
fn cancelling_the_switcher_changes_nothing() {
    let mut wm = three_apps();
    wm.handle(Command::SwitcherOpen { backward: false }, 10.0);
    let fx = wm.handle(Command::SwitcherCancel, 10.2);
    assert!(has(&fx, |e| matches!(
        e,
        Effect::SwitcherClosed { committed: false }
    )));
    assert_eq!(wm.front_app(), Some(app_of(&wm, 3)));
    assert_eq!(wm.key_window(), Some(win(3)));
}

#[test]
fn quitting_from_the_switcher_asks_the_selected_apps_windows_to_close() {
    let mut wm = three_apps();
    wm.handle(Command::SwitcherOpen { backward: false }, 10.0);
    let fx = wm.handle(Command::SwitcherQuit, 10.1);
    assert!(has(
        &fx,
        |e| matches!(e, Effect::RequestClose { window } if *window == win(2))
    ));
}

#[test]
fn hiding_from_the_switcher_hides_the_selected_app_and_unhiding_shows_it() {
    let mut wm = three_apps();
    wm.handle(Command::SwitcherOpen { backward: false }, 10.0);
    wm.handle(Command::SwitcherToggleHide, 10.1);
    assert!(wm.app(app_of(&wm, 2)).unwrap().hidden);
    wm.handle(Command::SwitcherToggleHide, 10.2);
    assert!(!wm.app(app_of(&wm, 2)).unwrap().hidden);
}

#[test]
fn cycling_windows_walks_the_apps_windows_and_ends_with_the_chosen_one_in_front() {
    let mut wm = wm();
    open(&mut wm, 1, "a", 1.0);
    open(&mut wm, 2, "a", 2.0);
    open(&mut wm, 3, "a", 3.0);
    wm.handle(Command::CycleWindows { backward: false }, 10.0);
    let first = wm.key_window();
    assert_ne!(first, Some(win(3)));
    wm.handle(Command::CycleWindows { backward: false }, 10.1);
    let second = wm.key_window();
    assert_ne!(second, first);
    wm.handle(Command::EndCycleWindows, 10.2);
    wm.handle(Command::FocusWindow { window: win(3) }, 10.3);
    assert_eq!(wm.key_window(), Some(win(3)));
}

#[test]
fn a_window_that_maps_while_the_person_works_elsewhere_opens_behind() {
    let mut wm = wm();
    open(&mut wm, 1, "a", 1.0);
    let mut p = MapParams::new(win(2), "b", frame(2));
    p.activation = Activation::Policy(MapFocus {
        first_of_client: true,
        input_elsewhere: true,
        grant: None,
        last_user_event: Some(1.5),
    });
    let fx = wm.handle(Command::Map(p), 2.0);
    assert_eq!(wm.key_window(), Some(win(1)));
    assert!(has(&fx, |e| matches!(e, Effect::AppAttention { .. })));
    assert_eq!(
        wm.stacking_order(),
        [win(2), win(1)],
        "behind the key window"
    );
}

#[test]
fn a_first_window_with_no_competing_input_takes_the_keyboard() {
    let mut wm = wm();
    open(&mut wm, 1, "a", 1.0);
    let mut p = MapParams::new(win(2), "b", frame(2));
    p.activation = Activation::Policy(MapFocus {
        first_of_client: true,
        input_elsewhere: false,
        grant: None,
        last_user_event: Some(2.0 - TYPING_QUIET - 1.0),
    });
    wm.handle(Command::Map(p), 2.0);
    assert_eq!(wm.key_window(), Some(win(2)));
}

#[test]
fn typing_in_progress_keeps_the_keyboard_from_a_new_program() {
    let mut wm = wm();
    open(&mut wm, 1, "a", 1.0);
    let mut p = MapParams::new(win(2), "b", frame(2));
    p.activation = Activation::Policy(MapFocus {
        first_of_client: true,
        input_elsewhere: false,
        grant: None,
        last_user_event: Some(1.9),
    });
    wm.handle(Command::Map(p), 2.0);
    assert_eq!(wm.key_window(), Some(win(1)));
}

#[test]
fn a_valid_fresh_grant_takes_the_keyboard_even_while_typing() {
    let mut wm = wm();
    open(&mut wm, 1, "a", 1.0);
    let mut p = MapParams::new(win(2), "b", frame(2));
    p.activation = Activation::Policy(MapFocus {
        first_of_client: false,
        input_elsewhere: true,
        grant: Some(Grant {
            created: 1.95,
            valid: true,
        }),
        last_user_event: Some(1.9),
    });
    wm.handle(Command::Map(p), 2.0);
    assert_eq!(wm.key_window(), Some(win(2)));
}

#[test]
fn a_grant_that_is_invalid_or_too_old_is_not_honoured() {
    for grant in [
        Grant {
            created: 1.95,
            valid: false,
        },
        Grant {
            created: 2.0 - GRANT_LIFETIME - 1.0,
            valid: true,
        },
    ] {
        let mut wm = wm();
        open(&mut wm, 1, "a", 1.0);
        let mut p = MapParams::new(win(2), "b", frame(2));
        p.activation = Activation::Policy(MapFocus {
            first_of_client: false,
            input_elsewhere: true,
            grant: Some(grant),
            last_user_event: None,
        });
        wm.handle(Command::Map(p), 2.0);
        assert_eq!(wm.key_window(), Some(win(1)), "{grant:?}");
    }
}

#[test]
fn a_window_of_the_front_app_or_a_child_of_the_key_window_takes_the_keyboard() {
    let mut wm = wm();
    open(&mut wm, 1, "a", 1.0);
    let mut sibling = MapParams::new(win(2), "a", frame(2));
    sibling.activation = Activation::Policy(MapFocus {
        first_of_client: false,
        input_elsewhere: true,
        grant: None,
        last_user_event: Some(1.99),
    });
    wm.handle(Command::Map(sibling), 2.0);
    assert_eq!(wm.key_window(), Some(win(2)));
}

#[test]
fn an_activation_token_must_be_recent_and_not_older_than_the_apps_last_activation() {
    let mut wm = wm();
    open(&mut wm, 1, "a", 1.0);
    let activated = wm
        .app(app_of(&wm, 1))
        .unwrap()
        .activated_at
        .expect("activated");
    assert!(wm.activation_allowed(win(1), activated + 0.5, None, activated + 0.6));
    assert!(
        !wm.activation_allowed(win(1), activated - 0.5, None, activated + 0.6),
        "older than the activation"
    );
    assert!(
        !wm.activation_allowed(
            win(1),
            activated + 0.5,
            None,
            activated + 0.5 + LATE_ACTIVATION + 0.1
        ),
        "too late"
    );
    assert!(
        !wm.activation_allowed(win(9), 1.0, None, 1.1),
        "no such window"
    );
}

#[test]
fn a_token_is_denied_by_a_later_user_event_but_not_by_one_within_the_slack() {
    assert!(token_accepted(5.0, None, None));
    assert!(token_accepted(5.0, Some(4.0), Some(5.0 + USER_EVENT_SLACK)));
    assert!(!token_accepted(
        5.0,
        Some(4.0),
        Some(5.0 + USER_EVENT_SLACK + 0.01)
    ));
    assert!(!token_accepted(5.0, Some(5.5), None));
    assert!(token_accepted(5.0, Some(5.0), None));
}

#[test]
fn the_switcher_start_index_follows_the_hidden_count_rule() {
    // No count: the previous app going forward, the last one going backward.
    assert_eq!(hidden_index(0, 5, true), 1);
    assert_eq!(hidden_index(0, 5, false), 4);
    assert_eq!(hidden_index(0, 1, true), 0);
    // A positive count is an index from the front, a negative one from the back; both are bounded.
    assert_eq!(hidden_index(3, 5, true), 3);
    assert_eq!(hidden_index(9, 5, true), 4);
    assert_eq!(hidden_index(-1, 5, true), 4);
    assert_eq!(hidden_index(-5, 5, true), 0);
    assert_eq!(hidden_index(-9, 5, true), 0);
}
