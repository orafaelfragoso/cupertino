//! The accessibility keyboard filters: slow keys, sticky keys and mouse keys.

use cupertino_keyboard::mouse_keys::{
    ramp, DEFAULT_INITIAL_DELAY_MS, DEFAULT_MAX_SPEED, TICK_MS, TOGGLE_WINDOW_MS,
};
use cupertino_keyboard::sticky_keys::{Modifier, StickyState};
use cupertino_keyboard::{MouseKeys, PointerAction, SlowKeys, SlowStep, StickyKeys};

const KEY_A: u32 = 30;
const KEY_B: u32 = 48;
const KEY_MENU: u32 = 139;
const KEY_CAPSLOCK: u32 = 58;
const KEY_LSHIFT: u32 = 42;
const KEY_RSHIFT: u32 = 54;
const KEY_LCTRL: u32 = 29;
const KEY_LALT: u32 = 56;
const KEY_LMETA: u32 = 125;
const KP8: u32 = 72;
const KP5: u32 = 76;
const KP0: u32 = 82;
const KP_DOT: u32 = 83;
const KP7: u32 = 71;

#[test]
fn slow_keys_accept_a_press_only_once_it_is_held_for_the_delay() {
    let mut s = SlowKeys::new(250);
    assert_eq!(s.delay_ms(), 250);
    assert_eq!(s.press(KEY_A, 1000), SlowStep::Pending);
    assert_eq!(s.deadline(), Some(1250));
    assert_eq!(s.due(1249), None);
    assert_eq!(s.due(1250), Some(KEY_A));
    assert_eq!(s.deadline(), None);
    assert_eq!(s.release(KEY_A), SlowStep::Deliver, "accepted press");
    assert_eq!(s.release(KEY_A), SlowStep::Drop, "nothing left to release");

    s.press(KEY_A, 0);
    assert_eq!(s.press(KEY_A, 100), SlowStep::Pending, "same key restarts");
    assert_eq!(s.deadline(), Some(350));
    assert_eq!(s.release(KEY_A), SlowStep::Drop, "early release drops both");
    assert_eq!(s.due(1000), None);

    s.press(KEY_A, 0);
    assert_eq!(s.press(KEY_B, 100), SlowStep::Rejected, "another key");
    assert_eq!(s.deadline(), None);
    assert_eq!(s.release(KEY_B), SlowStep::Drop);

    assert_eq!(s.press(KEY_MENU, 0), SlowStep::Deliver, "menu key");
    assert_eq!(s.release(KEY_MENU), SlowStep::Deliver, "menu key");
    assert_eq!(s.deadline(), None);
}

#[test]
fn slow_keys_reset_forgets_pending_and_delivered_keys() {
    let mut s = SlowKeys::new(250);
    s.press(KEY_A, 0);
    s.reset();
    assert_eq!(s.deadline(), None);
    s.press(KEY_B, 0);
    s.due(250);
    s.reset();
    assert_eq!(s.release(KEY_B), SlowStep::Drop);
}

#[test]
fn sticky_modifiers_cover_both_sides_and_exclude_caps_lock() {
    for (code, modifier) in [
        (KEY_LSHIFT, Some(Modifier::Shift)),
        (KEY_RSHIFT, Some(Modifier::Shift)),
        (KEY_LCTRL, Some(Modifier::Control)),
        (KEY_LMETA, Some(Modifier::Super)),
        (KEY_CAPSLOCK, None),
        (KEY_A, None),
    ] {
        assert_eq!(Modifier::of_key(code), modifier, "evdev {code}");
    }
}

fn sticky() -> StickyKeys {
    StickyKeys::new(true, true)
}

#[test]
fn a_sticky_tap_latches_until_the_next_key_and_a_second_tap_locks() {
    let mut s = sticky();
    assert!(s.key(KEY_LCTRL, true, 0).deliver);
    assert_eq!(s.state(Modifier::Control), StickyState::Pressed);
    assert!(
        !s.key(KEY_LCTRL, false, 10).deliver,
        "the release is swallowed"
    );
    assert_eq!(s.state(Modifier::Control), StickyState::Latched);
    let down = s.key(KEY_A, true, 20);
    assert!(down.deliver && down.release.is_empty());
    let up = s.key(KEY_A, false, 30);
    assert!(up.deliver);
    assert_eq!(up.release, [KEY_LCTRL], "released after the next key");
    assert_eq!(s.state(Modifier::Control), StickyState::Idle);

    let mut s = sticky();
    s.key(KEY_LCTRL, true, 0);
    s.key(KEY_LCTRL, false, 1);
    assert!(s.key(KEY_LCTRL, true, 2).deliver);
    assert_eq!(s.state(Modifier::Control), StickyState::Locked);
    assert!(!s.key(KEY_LCTRL, false, 3).deliver);
    s.key(KEY_A, true, 4);
    assert!(
        s.key(KEY_A, false, 5).release.is_empty(),
        "locked survives typing"
    );
    assert_eq!(s.state(Modifier::Control), StickyState::Locked);
    assert!(!s.key(KEY_LCTRL, true, 6).deliver, "third tap unlocks");
    assert_eq!(s.state(Modifier::Control), StickyState::Idle);
}

#[test]
fn a_sticky_modifier_held_with_a_key_is_chorded_and_released_normally() {
    let mut s = sticky();
    s.key(KEY_LCTRL, true, 0);
    s.key(KEY_A, true, 1);
    assert_eq!(s.state(Modifier::Control), StickyState::Chorded);
    s.key(KEY_A, false, 2);
    assert!(s.key(KEY_LCTRL, false, 3).deliver);
    assert_eq!(s.state(Modifier::Control), StickyState::Idle);

    let mut off = StickyKeys::new(false, false);
    assert!(!off.enabled());
    assert!(off.key(KEY_LCTRL, true, 0).deliver);
    assert_eq!(
        off.state(Modifier::Control),
        StickyState::Idle,
        "disabled filter passes everything"
    );
}

/// Taps Shift at `times` (press, release 5 ms later) and returns the last release's toggle.
fn tap_shift(s: &mut StickyKeys, times: &[u64]) -> Option<bool> {
    let mut last = None;
    for &t in times {
        s.key(KEY_LSHIFT, true, t);
        last = s.key(KEY_LSHIFT, false, t + 5).toggled;
    }
    last
}

#[test]
fn five_shift_releases_within_thirty_seconds_toggle_sticky_keys() {
    let mut s = StickyKeys::new(false, true);
    assert_eq!(tap_shift(&mut s, &[0, 100, 200, 300]), None);
    assert_eq!(tap_shift(&mut s, &[400]), Some(true), "the fifth release");
    assert!(s.enabled());

    let mut slow = StickyKeys::new(false, true);
    tap_shift(&mut slow, &[0, 100, 200, 300]);
    assert_eq!(tap_shift(&mut slow, &[40_000]), None, "outside the window");

    let mut broken = StickyKeys::new(false, true);
    tap_shift(&mut broken, &[0, 10, 20, 30]);
    broken.key(KEY_A, true, 100);
    broken.key(KEY_A, false, 101);
    assert_eq!(tap_shift(&mut broken, &[200]), None, "another key resets");

    let mut no_shortcut = StickyKeys::new(false, false);
    assert_eq!(tap_shift(&mut no_shortcut, &[0, 1, 2, 3, 4, 5]), None);
}

#[test]
fn switching_sticky_keys_releases_the_modifiers_it_holds() {
    let mut s = StickyKeys::new(false, false);
    s.key(KEY_LCTRL, true, 0);
    assert_eq!(s.set_enabled(true), [KEY_LCTRL], "held when turned on");
    assert_eq!(s.set_enabled(true), Vec::<u32>::new());

    let mut s = sticky();
    s.key(KEY_LALT, true, 0);
    s.key(KEY_LALT, false, 1);
    assert_eq!(s.set_enabled(false), [KEY_LALT], "latched when turned off");
    assert_eq!(s.state(Modifier::Alt), StickyState::Idle);
}

fn mouse() -> MouseKeys {
    MouseKeys::new(true, true, DEFAULT_INITIAL_DELAY_MS, DEFAULT_MAX_SPEED)
}

#[test]
fn keypad_keys_move_click_and_hold_the_pointer() {
    for (row, code, actions) in [
        (
            "8 moves up",
            KP8,
            vec![PointerAction::Move { dx: 0, dy: -1 }],
        ),
        (
            "7 moves diagonally",
            KP7,
            vec![PointerAction::Move { dx: -1, dy: -1 }],
        ),
        ("5 presses", KP5, vec![PointerAction::Press]),
        ("0 presses", KP0, vec![PointerAction::Press]),
    ] {
        let out = mouse().key(code, true, 0);
        assert!(out.consumed, "{row}");
        assert_eq!(out.actions, actions, "{row}");
    }
    let mut m = mouse();
    m.key(KP5, true, 0);
    assert_eq!(
        m.key(KP5, false, 1).actions,
        [PointerAction::Release],
        "5 clicks"
    );
    m.key(KP0, true, 2);
    assert!(m.key(KP0, false, 3).actions.is_empty(), "0 holds");
    assert_eq!(
        m.key(KP_DOT, true, 4).actions,
        [PointerAction::Release],
        ". releases"
    );

    assert!(!mouse().key(KEY_A, true, 0).consumed, "other keys pass");
    let mut off = MouseKeys::new(false, false, 1000, 11);
    assert!(!off.enabled());
    assert!(
        !off.key(KP8, true, 0).consumed,
        "disabled mouse keys consume nothing"
    );
}

#[test]
fn a_held_direction_repeats_after_the_delay_and_ramps_to_the_maximum() {
    let mut m = mouse();
    m.key(KP8, true, 0);
    let first = DEFAULT_INITIAL_DELAY_MS + TICK_MS;
    assert_eq!(m.deadline(), Some(first));
    assert!(m.poll(first - 1).is_empty());
    assert_eq!(m.poll(first), [PointerAction::Move { dx: 0, dy: -2 }]);
    assert_eq!(m.deadline(), Some(first + TICK_MS));
    let speeds: Vec<i32> = m
        .poll(first + 20 * TICK_MS)
        .iter()
        .map(|a| match a {
            PointerAction::Move { dy, .. } => -dy,
            _ => panic!("only moves repeat"),
        })
        .collect();
    assert_eq!(&speeds[..4], &[3, 4, 5, 6]);
    assert_eq!(*speeds.last().unwrap(), DEFAULT_MAX_SPEED as i32);
    assert!(speeds.windows(2).all(|w| w[0] <= w[1]));

    let mut instant = MouseKeys::new(true, false, 0, DEFAULT_MAX_SPEED);
    assert!(
        instant.key(KP8, true, 0).actions.is_empty(),
        "no first move"
    );
    assert_eq!(instant.deadline(), Some(TICK_MS), "repeats at once");
}

#[test]
fn releasing_the_moving_direction_stops_the_repeat_unless_another_replaced_it() {
    let mut m = mouse();
    m.key(KP8, true, 0);
    m.key(KP8, false, 100);
    assert_eq!(m.deadline(), None);
    assert!(m.poll(5000).is_empty());

    let mut m = mouse();
    m.key(KP8, true, 0);
    m.key(KP7, true, 10);
    m.key(KP8, false, 20);
    assert!(m.deadline().is_some());
}

#[test]
fn five_alt_presses_within_the_window_toggle_mouse_keys() {
    let mut m = MouseKeys::new(false, true, 1000, 11);
    for i in 0..4 {
        m.key(KEY_LALT, true, i * 100);
        assert_eq!(m.key(KEY_LALT, false, i * 100 + 50).toggled, None);
    }
    m.key(KEY_LALT, true, 500);
    assert_eq!(m.key(KEY_LALT, false, 550).toggled, Some(true));
    assert!(m.enabled());

    let mut slow = MouseKeys::new(false, true, 1000, 11);
    slow.key(KEY_LALT, true, 0);
    slow.key(KEY_LALT, false, 10);
    for i in 0..4 {
        let t = TOGGLE_WINDOW_MS + i * 100;
        slow.key(KEY_LALT, true, t);
        assert_eq!(slow.key(KEY_LALT, false, t + 50).toggled, None);
    }
    assert!(!slow.enabled());
}

#[test]
fn ramp_adds_one_per_tick_below_the_fast_limit_and_follows_a_power_curve_above() {
    // (speed, tick, max) -> (new speed, tick after)
    for ((speed, tick, max), (next, tick_after)) in [
        ((1, 1, 11), (2, 1)),
        ((10, 1, 11), (11, 1)),
        ((11, 1, 11), (11, 1)),
        ((1, 4, 100), (8, 5)),
        ((1, 1000, 100), (100, 1001)),
    ] {
        let mut t = tick;
        assert_eq!(
            ramp(speed, &mut t, max),
            next,
            "speed {speed} tick {tick} max {max}"
        );
        assert_eq!(t, tick_after, "speed {speed} tick {tick} max {max}");
    }
}
