//! The accessibility keyboard filters: slow keys, sticky keys and mouse keys.

use cupertino_keyboard::mouse_keys::{
    ramp, DEFAULT_INITIAL_DELAY_MS, DEFAULT_MAX_SPEED, TICK_MS, TOGGLE_WINDOW_MS,
};
use cupertino_keyboard::slow_keys::DEFAULT_DELAY_MS;
use cupertino_keyboard::sticky_keys::{Modifier, StickyState};
use cupertino_keyboard::{MouseKeys, PointerAction, SlowKeys, SlowStep, StickyKeys};

const KEY_A: u32 = 30;
const KEY_B: u32 = 48;
const KEY_MENU: u32 = 139;
const KEY_LSHIFT: u32 = 42;
const KEY_RSHIFT: u32 = 54;
const KEY_LCTRL: u32 = 29;
const KEY_LALT: u32 = 56;
const KP8: u32 = 72;
const KP5: u32 = 76;
const KP0: u32 = 82;
const KP_DOT: u32 = 83;
const KP7: u32 = 71;

// slow keys

#[test]
fn slow_keys_default_delay_is_a_quarter_second() {
    assert_eq!(DEFAULT_DELAY_MS, 250);
    assert_eq!(SlowKeys::new(DEFAULT_DELAY_MS).delay_ms(), 250);
}

#[test]
fn slow_keys_hold_a_press_until_the_delay_has_elapsed() {
    let mut s = SlowKeys::new(250);
    assert_eq!(s.press(KEY_A, 1000), SlowStep::Pending);
    assert_eq!(s.deadline(), Some(1250));
    assert_eq!(s.due(1249), None);
    assert_eq!(s.due(1250), Some(KEY_A));
    assert_eq!(s.deadline(), None);
}

#[test]
fn slow_keys_deliver_the_release_of_an_accepted_press() {
    let mut s = SlowKeys::new(250);
    s.press(KEY_A, 0);
    s.due(250);
    assert_eq!(s.release(KEY_A), SlowStep::Deliver);
    assert_eq!(s.release(KEY_A), SlowStep::Drop);
}

#[test]
fn slow_keys_early_release_drops_both_events() {
    let mut s = SlowKeys::new(250);
    s.press(KEY_A, 0);
    assert_eq!(s.release(KEY_A), SlowStep::Drop);
    assert_eq!(s.deadline(), None);
    assert_eq!(s.due(1000), None);
}

#[test]
fn slow_keys_reject_another_key_while_one_is_pending() {
    let mut s = SlowKeys::new(250);
    s.press(KEY_A, 0);
    assert_eq!(s.press(KEY_B, 100), SlowStep::Rejected);
    assert_eq!(s.deadline(), None);
    assert_eq!(s.release(KEY_B), SlowStep::Drop);
}

#[test]
fn slow_keys_restart_the_wait_for_the_same_key() {
    let mut s = SlowKeys::new(250);
    s.press(KEY_A, 0);
    assert_eq!(s.press(KEY_A, 100), SlowStep::Pending);
    assert_eq!(s.deadline(), Some(350));
}

#[test]
fn slow_keys_never_hold_back_the_menu_key() {
    let mut s = SlowKeys::new(250);
    assert_eq!(s.press(KEY_MENU, 0), SlowStep::Deliver);
    assert_eq!(s.release(KEY_MENU), SlowStep::Deliver);
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

// sticky keys

fn sticky() -> StickyKeys {
    StickyKeys::new(true, true)
}

#[test]
fn sticky_modifier_of_key_covers_both_sides_and_excludes_caps_lock() {
    assert_eq!(Modifier::of_key(KEY_LSHIFT), Some(Modifier::Shift));
    assert_eq!(Modifier::of_key(KEY_RSHIFT), Some(Modifier::Shift));
    assert_eq!(Modifier::of_key(KEY_LCTRL), Some(Modifier::Control));
    assert_eq!(Modifier::of_key(125), Some(Modifier::Super));
    assert_eq!(Modifier::of_key(58), None);
    assert_eq!(Modifier::of_key(KEY_A), None);
}

#[test]
fn sticky_first_press_is_delivered_and_marks_the_modifier_pressed() {
    let mut s = sticky();
    let out = s.key(KEY_LCTRL, true, 0);
    assert!(out.deliver);
    assert_eq!(s.state(Modifier::Control), StickyState::Pressed);
}

#[test]
fn sticky_tap_latches_the_modifier_and_swallows_its_release() {
    let mut s = sticky();
    s.key(KEY_LCTRL, true, 0);
    let up = s.key(KEY_LCTRL, false, 10);
    assert!(!up.deliver);
    assert_eq!(s.state(Modifier::Control), StickyState::Latched);
}

#[test]
fn sticky_latched_modifier_is_released_after_the_next_key() {
    let mut s = sticky();
    s.key(KEY_LCTRL, true, 0);
    s.key(KEY_LCTRL, false, 10);
    let down = s.key(KEY_A, true, 20);
    assert!(down.deliver);
    assert!(down.release.is_empty());
    let up = s.key(KEY_A, false, 30);
    assert!(up.deliver);
    assert_eq!(up.release, vec![KEY_LCTRL]);
    assert_eq!(s.state(Modifier::Control), StickyState::Idle);
}

#[test]
fn sticky_second_tap_locks_and_third_unlocks() {
    let mut s = sticky();
    s.key(KEY_LCTRL, true, 0);
    s.key(KEY_LCTRL, false, 1);
    let second = s.key(KEY_LCTRL, true, 2);
    assert!(second.deliver);
    assert_eq!(s.state(Modifier::Control), StickyState::Locked);
    assert!(!s.key(KEY_LCTRL, false, 3).deliver);
    assert_eq!(s.state(Modifier::Control), StickyState::Locked);
    let third = s.key(KEY_LCTRL, true, 4);
    assert!(!third.deliver);
    assert_eq!(s.state(Modifier::Control), StickyState::Idle);
}

#[test]
fn sticky_locked_modifier_survives_typing() {
    let mut s = sticky();
    s.key(KEY_LCTRL, true, 0);
    s.key(KEY_LCTRL, false, 1);
    s.key(KEY_LCTRL, true, 2);
    s.key(KEY_A, true, 3);
    let up = s.key(KEY_A, false, 4);
    assert!(up.release.is_empty());
    assert_eq!(s.state(Modifier::Control), StickyState::Locked);
}

#[test]
fn sticky_modifier_held_with_a_key_is_chorded_and_released_normally() {
    let mut s = sticky();
    s.key(KEY_LCTRL, true, 0);
    s.key(KEY_A, true, 1);
    assert_eq!(s.state(Modifier::Control), StickyState::Chorded);
    s.key(KEY_A, false, 2);
    let up = s.key(KEY_LCTRL, false, 3);
    assert!(up.deliver);
    assert_eq!(s.state(Modifier::Control), StickyState::Idle);
}

#[test]
fn sticky_disabled_filter_passes_everything() {
    let mut s = StickyKeys::new(false, false);
    assert!(!s.enabled());
    let out = s.key(KEY_LCTRL, true, 0);
    assert!(out.deliver);
    assert_eq!(s.state(Modifier::Control), StickyState::Idle);
}

#[test]
fn five_shift_releases_toggle_the_feature() {
    let mut s = StickyKeys::new(false, true);
    let mut toggled = None;
    for i in 0..5 {
        s.key(KEY_LSHIFT, true, i * 100);
        let out = s.key(KEY_LSHIFT, false, i * 100 + 50);
        if out.toggled.is_some() {
            toggled = out.toggled;
            assert_eq!(i, 4);
            assert!(out.deliver);
        }
    }
    assert_eq!(toggled, Some(true));
    assert!(s.enabled());
}

#[test]
fn shift_releases_outside_the_thirty_second_window_do_not_toggle() {
    let mut s = StickyKeys::new(false, true);
    for i in 0..4 {
        s.key(KEY_LSHIFT, true, i * 100);
        s.key(KEY_LSHIFT, false, i * 100 + 10);
    }
    s.key(KEY_LSHIFT, true, 40_000);
    let out = s.key(KEY_LSHIFT, false, 40_010);
    assert_eq!(out.toggled, None);
    assert!(!s.enabled());
}

#[test]
fn another_key_resets_the_shift_release_count() {
    let mut s = StickyKeys::new(false, true);
    for i in 0..4 {
        s.key(KEY_LSHIFT, true, i * 10);
        s.key(KEY_LSHIFT, false, i * 10 + 5);
    }
    s.key(KEY_A, true, 100);
    s.key(KEY_A, false, 101);
    s.key(KEY_LSHIFT, true, 200);
    let out = s.key(KEY_LSHIFT, false, 201);
    assert_eq!(out.toggled, None);
}

#[test]
fn shift_releases_do_nothing_without_the_shortcut() {
    let mut s = StickyKeys::new(false, false);
    for i in 0..10 {
        s.key(KEY_LSHIFT, true, i);
        assert_eq!(s.key(KEY_LSHIFT, false, i).toggled, None);
    }
}

#[test]
fn turning_sticky_keys_on_releases_the_modifiers_held_at_that_moment() {
    let mut s = StickyKeys::new(false, false);
    s.key(KEY_LCTRL, true, 0);
    assert_eq!(s.set_enabled(true), vec![KEY_LCTRL]);
    assert_eq!(s.set_enabled(true), Vec::<u32>::new());
}

#[test]
fn turning_sticky_keys_off_releases_latched_modifiers() {
    let mut s = sticky();
    s.key(KEY_LALT, true, 0);
    s.key(KEY_LALT, false, 1);
    assert_eq!(s.set_enabled(false), vec![KEY_LALT]);
    assert_eq!(s.state(Modifier::Alt), StickyState::Idle);
}

// mouse keys

fn mouse() -> MouseKeys {
    MouseKeys::new(true, true, DEFAULT_INITIAL_DELAY_MS, DEFAULT_MAX_SPEED)
}

#[test]
fn mouse_key_defaults_are_documented() {
    assert_eq!(DEFAULT_INITIAL_DELAY_MS, 1000);
    assert_eq!(TICK_MS, 50);
    assert_eq!(DEFAULT_MAX_SPEED, 11);
}

#[test]
fn a_direction_key_moves_one_point_on_the_press() {
    let mut m = mouse();
    let out = m.key(KP8, true, 0);
    assert!(out.consumed);
    assert_eq!(out.actions, vec![PointerAction::Move { dx: 0, dy: -1 }]);
}

#[test]
fn diagonal_keys_move_along_both_axes() {
    let mut m = mouse();
    let out = m.key(KP7, true, 0);
    assert_eq!(out.actions, vec![PointerAction::Move { dx: -1, dy: -1 }]);
}

#[test]
fn the_repeat_starts_after_the_delay_and_a_tick() {
    let mut m = mouse();
    m.key(KP8, true, 0);
    assert_eq!(m.deadline(), Some(1050));
    assert!(m.poll(1049).is_empty());
    assert_eq!(m.poll(1050), vec![PointerAction::Move { dx: 0, dy: -2 }]);
    assert_eq!(m.deadline(), Some(1100));
}

#[test]
fn the_repeat_speed_ramps_up_to_the_maximum() {
    let mut m = mouse();
    m.key(KP8, true, 0);
    let moves = m.poll(1050 + 20 * TICK_MS);
    let speeds: Vec<i32> = moves
        .iter()
        .map(|a| match a {
            PointerAction::Move { dy, .. } => -dy,
            _ => panic!("only moves repeat"),
        })
        .collect();
    assert_eq!(&speeds[..4], &[2, 3, 4, 5]);
    assert_eq!(*speeds.last().unwrap(), DEFAULT_MAX_SPEED as i32);
    assert!(speeds.windows(2).all(|w| w[0] <= w[1]));
}

#[test]
fn zero_initial_delay_has_no_first_move_and_repeats_at_once() {
    let mut m = MouseKeys::new(true, false, 0, DEFAULT_MAX_SPEED);
    let out = m.key(KP8, true, 0);
    assert!(out.actions.is_empty());
    assert_eq!(m.deadline(), Some(TICK_MS));
}

#[test]
fn releasing_the_direction_key_stops_the_repeat() {
    let mut m = mouse();
    m.key(KP8, true, 0);
    m.key(KP8, false, 100);
    assert_eq!(m.deadline(), None);
    assert!(m.poll(5000).is_empty());
}

#[test]
fn a_new_direction_key_replaces_the_moving_one() {
    let mut m = mouse();
    m.key(KP8, true, 0);
    m.key(KP7, true, 10);
    m.key(KP8, false, 20);
    assert!(m.deadline().is_some());
}

#[test]
fn keypad_five_clicks_with_press_and_release() {
    let mut m = mouse();
    assert_eq!(m.key(KP5, true, 0).actions, vec![PointerAction::Press]);
    assert_eq!(m.key(KP5, false, 1).actions, vec![PointerAction::Release]);
}

#[test]
fn keypad_zero_holds_and_the_decimal_point_releases() {
    let mut m = mouse();
    assert_eq!(m.key(KP0, true, 0).actions, vec![PointerAction::Press]);
    assert!(m.key(KP0, false, 1).actions.is_empty());
    assert_eq!(m.key(KP_DOT, true, 2).actions, vec![PointerAction::Release]);
}

#[test]
fn other_keys_are_not_consumed_and_disabled_mouse_keys_consume_nothing() {
    let mut m = mouse();
    assert!(!m.key(KEY_A, true, 0).consumed);
    let mut off = MouseKeys::new(false, false, 1000, 11);
    assert!(!off.enabled());
    assert!(!off.key(KP8, true, 0).consumed);
}

#[test]
fn five_alt_presses_toggle_mouse_keys_on_the_fifth_release() {
    let mut m = MouseKeys::new(false, true, 1000, 11);
    for i in 0..4 {
        m.key(KEY_LALT, true, i * 100);
        assert_eq!(m.key(KEY_LALT, false, i * 100 + 50).toggled, None);
    }
    m.key(KEY_LALT, true, 500);
    let out = m.key(KEY_LALT, false, 550);
    assert_eq!(out.toggled, Some(true));
    assert!(m.enabled());
}

#[test]
fn alt_presses_spread_over_more_than_the_window_do_not_toggle() {
    let mut m = MouseKeys::new(false, true, 1000, 11);
    m.key(KEY_LALT, true, 0);
    m.key(KEY_LALT, false, 10);
    for i in 0..4 {
        let t = TOGGLE_WINDOW_MS + i * 100;
        m.key(KEY_LALT, true, t);
        assert_eq!(m.key(KEY_LALT, false, t + 50).toggled, None);
    }
    assert!(!m.enabled());
}

#[test]
fn ramp_adds_one_per_tick_below_the_fast_limit() {
    let mut tick = 1;
    assert_eq!(ramp(1, &mut tick, 11), 2);
    assert_eq!(ramp(10, &mut tick, 11), 11);
    assert_eq!(ramp(11, &mut tick, 11), 11);
    assert_eq!(tick, 1);
}

#[test]
fn ramp_follows_a_power_curve_for_high_maximum_speeds() {
    let mut tick = 4;
    assert_eq!(ramp(1, &mut tick, 100), 8);
    assert_eq!(tick, 5);
    let mut tick = 1000;
    assert_eq!(ramp(1, &mut tick, 100), 100);
}
