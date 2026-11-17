//! Applications take the keyboard in most-recently-used order, hide and show, and are switched with the app switcher.

mod common;

use common::*;
use cupertino_wm::{
    hidden_index, token_accepted, Activation, Command, Effect, Grant, LifecycleState, MapFocus,
    MapParams, SwitcherStep, GRANT_LIFETIME, LATE_ACTIVATION, TYPING_QUIET, USER_EVENT_SLACK,
};

fn three_apps() -> cupertino_wm::Wm {
    let mut wm = common::wm();
    open(&mut wm, 1, "a", 1.0);
    open(&mut wm, 2, "b", 2.0);
    open(&mut wm, 3, "c", 3.0);
    wm
}

#[test]
fn activating_an_app_raises_its_most_recent_window_and_puts_it_first() {
    let mut wm = three_apps();
    open(&mut wm, 4, "a", 4.0);
    let (a, b, c) = (app_of(&wm, 1), app_of(&wm, 2), app_of(&wm, 3));
    assert_eq!(wm.app_mru(), [a, c, b]);
    wm.handle(Command::ActivateApp { app: c }, 10.0);
    assert_eq!(wm.app_mru(), [c, a, b]);
    wm.handle(Command::ActivateApp { app: a }, 11.0);
    assert_eq!(wm.app_mru(), [a, c, b]);
    assert_eq!(wm.front_app(), Some(a));
    assert_eq!(wm.key_window(), Some(win(4)));
    assert_eq!(wm.stacking_order().last(), Some(&win(4)));
}

#[test]
fn hiding_an_app_moves_the_keyboard_and_unhiding_or_activating_it_shows_it_in_front() {
    for by_activation in [false, true] {
        let mut wm = three_apps();
        let c = app_of(&wm, 3);
        wm.handle(Command::HideApp { app: c }, 10.0);
        assert!(wm.app(c).unwrap().hidden);
        assert!(!wm.window_on_screen(win(3)));
        assert!(wm.window_on_screen(win(2)));
        assert_eq!(wm.key_window(), Some(win(2)));
        assert_eq!(wm.front_app(), Some(app_of(&wm, 2)));
        let unhide = if by_activation {
            Command::ActivateApp { app: c }
        } else {
            Command::UnhideApp { app: c }
        };
        wm.handle(unhide, 11.0);
        assert!(!wm.app(c).unwrap().hidden);
        assert!(wm.window_on_screen(win(3)));
        assert_eq!(wm.front_app(), Some(c));
        assert_eq!(wm.key_window(), Some(win(3)));
    }
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
    let (apps, selected) = fx
        .iter()
        .find_map(|e| match e {
            Effect::SwitcherChanged { apps, selected } => Some((apps.clone(), *selected)),
            _ => None,
        })
        .expect("the switcher shows");
    assert_eq!(apps, [app_of(&wm, 3), app_of(&wm, 2), app_of(&wm, 1)]);
    assert_eq!(selected, 1, "the previous app, not the current one");
    let fx = wm.handle(Command::SwitcherCommit, 10.5);
    assert!(has(&fx, |e| matches!(
        e,
        Effect::SwitcherClosed { committed: true }
    )));
    assert!(wm.switcher().is_none());
    assert_eq!(wm.front_app(), Some(app_of(&wm, 2)));
    assert_eq!(wm.key_window(), Some(win(2)));

    let mut wm = three_apps();
    wm.handle(Command::SwitcherOpen { backward: true }, 10.0);
    assert_eq!(wm.switcher().unwrap().selected, 2, "backward: the last app");
}

#[test]
fn stepping_moves_the_selection_and_wraps_only_for_a_new_press() {
    let mut wm = three_apps();
    wm.handle(Command::SwitcherOpen { backward: false }, 10.0);
    for (name, command, selected, now) in [
        (
            "next",
            Command::SwitcherStep {
                step: SwitcherStep::Next,
                repeat: false,
            },
            2,
        ),
        (
            "a held key stops at the end",
            Command::SwitcherStep {
                step: SwitcherStep::Next,
                repeat: true,
            },
            2,
        ),
        (
            "a new press wraps",
            Command::SwitcherStep {
                step: SwitcherStep::Next,
                repeat: false,
            },
            0,
        ),
        (
            "previous wraps",
            Command::SwitcherStep {
                step: SwitcherStep::Previous,
                repeat: false,
            },
            2,
        ),
        (
            "first",
            Command::SwitcherStep {
                step: SwitcherStep::First,
                repeat: false,
            },
            0,
        ),
        (
            "last",
            Command::SwitcherStep {
                step: SwitcherStep::Last,
                repeat: false,
            },
            2,
        ),
        ("select", Command::SwitcherSelect { index: 1 }, 1),
    ]
    .into_iter()
    .enumerate()
    .map(|(i, (name, command, selected))| (name, command, selected, 10.1 + 0.1 * i as f64))
    {
        wm.handle(command, now);
        assert_eq!(wm.switcher().unwrap().selected, selected, "{name}");
    }
}

#[test]
fn the_switcher_cancels_quits_and_hides_the_selected_app() {
    let mut wm = three_apps();
    wm.handle(Command::SwitcherOpen { backward: false }, 10.0);
    let fx = wm.handle(Command::SwitcherQuit, 10.1);
    assert!(has(
        &fx,
        |e| matches!(e, Effect::RequestClose { window } if *window == win(2))
    ));
    wm.handle(Command::SwitcherToggleHide, 10.2);
    assert!(wm.app(app_of(&wm, 2)).unwrap().hidden);
    wm.handle(Command::SwitcherToggleHide, 10.3);
    assert!(!wm.app(app_of(&wm, 2)).unwrap().hidden);
    let fx = wm.handle(Command::SwitcherCancel, 10.4);
    assert!(has(&fx, |e| matches!(
        e,
        Effect::SwitcherClosed { committed: false }
    )));
    assert_eq!(
        wm.front_app(),
        Some(app_of(&wm, 3)),
        "cancel changes nothing"
    );
    assert_eq!(wm.key_window(), Some(win(3)));
}

#[test]
fn cycling_windows_walks_the_apps_windows() {
    let mut wm = common::wm();
    open(&mut wm, 1, "a", 1.0);
    open(&mut wm, 2, "a", 2.0);
    open(&mut wm, 3, "a", 3.0);
    wm.handle(Command::CycleWindows { backward: false }, 10.0);
    let first = wm.key_window();
    assert_ne!(first, Some(win(3)));
    wm.handle(Command::CycleWindows { backward: false }, 10.1);
    assert_ne!(wm.key_window(), first);
    wm.handle(Command::EndCycleWindows, 10.2);
    wm.handle(Command::FocusWindow { window: win(3) }, 10.3);
    assert_eq!(wm.key_window(), Some(win(3)));
}

#[test]
fn a_new_window_takes_the_keyboard_only_when_the_policy_allows_it() {
    let policy = |first_of_client, input_elsewhere, grant, last_user_event| MapFocus {
        first_of_client,
        input_elsewhere,
        grant,
        last_user_event,
    };
    let fresh = |valid| Grant {
        created: 1.95,
        valid,
    };
    let stale = Grant {
        created: 2.0 - GRANT_LIFETIME - 1.0,
        valid: true,
    };
    for (name, app, focus, takes) in [
        (
            "the person works elsewhere",
            "b",
            policy(true, true, None, Some(1.5)),
            false,
        ),
        (
            "a first window with no competing input",
            "b",
            policy(true, false, None, Some(2.0 - TYPING_QUIET - 1.0)),
            true,
        ),
        (
            "typing in progress",
            "b",
            policy(true, false, None, Some(1.9)),
            false,
        ),
        (
            "a valid fresh grant, even while typing",
            "b",
            policy(false, true, Some(fresh(true)), Some(1.9)),
            true,
        ),
        (
            "an invalid grant",
            "b",
            policy(false, true, Some(fresh(false)), None),
            false,
        ),
        (
            "a grant too old",
            "b",
            policy(false, true, Some(stale), None),
            false,
        ),
        (
            "a window of the front app",
            "a",
            policy(false, true, None, Some(1.99)),
            true,
        ),
    ] {
        let mut wm = common::wm();
        open(&mut wm, 1, "a", 1.0);
        let mut p = MapParams::new(win(2), app, frame(2));
        p.activation = Activation::Policy(focus);
        let fx = wm.handle(Command::Map(p), 2.0);
        let key = if takes { win(2) } else { win(1) };
        assert_eq!(wm.key_window(), Some(key), "{name}");
        if name == "the person works elsewhere" {
            assert!(
                has(&fx, |e| matches!(e, Effect::AppAttention { .. })),
                "{name}"
            );
            assert_eq!(
                wm.stacking_order(),
                [win(2), win(1)],
                "behind the key window"
            );
        }
    }
}

#[test]
fn an_activation_token_must_be_recent_and_not_older_than_the_apps_last_activation() {
    let mut wm = common::wm();
    open(&mut wm, 1, "a", 1.0);
    let at = wm
        .app(app_of(&wm, 1))
        .unwrap()
        .activated_at
        .expect("activated");
    for (name, window, created, now, allowed) in [
        ("recent", 1, at + 0.5, at + 0.6, true),
        ("older than the activation", 1, at - 0.5, at + 0.6, false),
        (
            "too late",
            1,
            at + 0.5,
            at + 0.5 + LATE_ACTIVATION + 0.1,
            false,
        ),
        ("no such window", 9, 1.0, 1.1, false),
    ] {
        assert_eq!(
            wm.activation_allowed(win(window), created, None, now),
            allowed,
            "{name}"
        );
    }
}

#[test]
fn a_token_is_denied_by_a_later_user_event_but_not_by_one_within_the_slack() {
    for (created, user_event, used, accepted) in [
        (5.0, None, None, true),
        (5.0, Some(4.0), Some(5.0 + USER_EVENT_SLACK), true),
        (5.0, Some(4.0), Some(5.0 + USER_EVENT_SLACK + 0.01), false),
        (5.0, Some(5.5), None, false),
        (5.0, Some(5.0), None, true),
    ] {
        assert_eq!(
            token_accepted(created, user_event, used),
            accepted,
            "{created} {user_event:?} {used:?}"
        );
    }
}

#[test]
fn the_switcher_start_index_follows_the_hidden_count_rule() {
    // No count: the previous app going forward, the last one going backward. A positive count is an index from the
    // front, a negative one from the back; both are bounded.
    for (count, len, forward, index) in [
        (0, 5, true, 1),
        (0, 5, false, 4),
        (0, 1, true, 0),
        (3, 5, true, 3),
        (9, 5, true, 4),
        (-1, 5, true, 4),
        (-5, 5, true, 0),
        (-9, 5, true, 0),
    ] {
        assert_eq!(
            hidden_index(count, len, forward),
            index,
            "{count} of {len} forward {forward}"
        );
    }
}
