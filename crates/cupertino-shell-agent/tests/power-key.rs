//! The power key's hold timing.

use cupertino_shell_agent::power_button::{
    Effect, PowerButton, DEBOUNCE_SECONDS, SHUT_DOWN_UI_SECONDS,
};

#[test]
fn a_short_press_does_nothing_and_a_medium_one_sleeps_only_when_set() {
    let mut off = PowerButton::new(false);
    off.press(0.0);
    assert_eq!(off.release(DEBOUNCE_SECONDS - 0.01), None);
    off.press(1.0);
    assert_eq!(off.release(1.0 + 1.0), None);

    let mut on = PowerButton::new(true);
    on.press(0.0);
    assert_eq!(on.release(DEBOUNCE_SECONDS - 0.01), None);
    on.press(1.0);
    assert_eq!(on.release(1.0 + 1.0), Some(Effect::Sleep));
}

#[test]
fn a_hold_shows_the_panel_once_and_its_release_does_nothing() {
    let mut key = PowerButton::new(false);
    key.press(5.0);
    assert_eq!(key.next_wakeup(5.5), Some(SHUT_DOWN_UI_SECONDS - 0.5));
    assert_eq!(key.tick(5.0 + SHUT_DOWN_UI_SECONDS - 0.01), None);
    assert_eq!(
        key.tick(5.0 + SHUT_DOWN_UI_SECONDS),
        Some(Effect::ShowPanel)
    );
    assert_eq!(key.tick(10.0), None);
    assert_eq!(key.next_wakeup(10.0), None);
    assert_eq!(key.release(10.0), None);
}

#[test]
fn a_late_release_shows_the_panel_when_the_timer_has_not_fired() {
    let mut key = PowerButton::new(false);
    key.press(0.0);
    assert_eq!(key.release(SHUT_DOWN_UI_SECONDS), Some(Effect::ShowPanel));
}

#[test]
fn a_release_without_a_press_and_a_forgotten_key_do_nothing() {
    let mut key = PowerButton::new(true);
    assert_eq!(key.release(3.0), None);
    key.press(0.0);
    key.reset();
    assert_eq!(key.tick(10.0), None);
    assert_eq!(key.release(10.0), None);
}
