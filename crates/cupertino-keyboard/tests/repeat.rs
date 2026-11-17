//! Key repeat timing, the wl_keyboard advertisement, the repeater and the caps-lock rules.

use cupertino_keyboard::repeat::{
    caps_hold_duration, caps_override_duration, clamp_hid_ns, evdev_repeats, hid_usage_repeats,
    SliderTable, CAPS_HOLD_DEFAULT_MS, CAPS_HOLD_MAX_MS, CAPS_OVERRIDE_DEFAULT_MS,
    FALLBACK_INITIAL_MS, FALLBACK_INTERVAL_MS, MIN_ENABLED_MS, SLOW_KEYS_REPEAT_MS, UNIT_MS,
};
use cupertino_keyboard::{CapsLockDelay, KeyRepeater, RepeatConfig, RepeatInfo};

const KEY_A: u32 = 30;
const KEY_B: u32 = 48;
const KEY_LEFTSHIFT: u32 = 42;

#[test]
fn nanoseconds_below_one_millisecond_disable_repeat() {
    assert_eq!(clamp_hid_ns(0), 0);
    assert_eq!(clamp_hid_ns(999_999), 0);
}

#[test]
fn nanoseconds_up_to_sixteen_milliseconds_are_raised_to_the_minimum() {
    assert_eq!(clamp_hid_ns(1_000_000), MIN_ENABLED_MS);
    assert_eq!(clamp_hid_ns(15_999_999), MIN_ENABLED_MS);
}

#[test]
fn nanoseconds_from_sixteen_milliseconds_are_divided_down() {
    assert_eq!(clamp_hid_ns(16_000_000), 16);
    assert_eq!(clamp_hid_ns(500_999_999), 500);
}

#[test]
fn default_config_is_the_fallback_pair() {
    let cfg = RepeatConfig::default();
    assert_eq!(cfg.initial_delay_ms, FALLBACK_INITIAL_MS);
    assert_eq!(cfg.interval_ms, FALLBACK_INTERVAL_MS);
    assert_eq!((FALLBACK_INITIAL_MS, FALLBACK_INTERVAL_MS), (500, 83));
}

#[test]
fn preference_units_are_sixtieths_of_a_second() {
    let cfg = RepeatConfig::from_pref_units(30.0, 6.0);
    assert_eq!(cfg.initial_delay_ms, 500);
    assert_eq!(cfg.interval_ms, 100);
    assert!((UNIT_MS - 16.6667).abs() < 1e-3);
}

#[test]
fn negative_preference_units_disable_repeat() {
    let cfg = RepeatConfig::from_pref_units(-5.0, 6.0);
    assert!(!cfg.is_enabled());
}

#[test]
fn nanosecond_config_applies_the_clamp_to_both_values() {
    let cfg = RepeatConfig::from_hid_ns(2_000_000, 300_000_000);
    assert_eq!(cfg.initial_delay_ms, MIN_ENABLED_MS);
    assert_eq!(cfg.interval_ms, 300);
}

#[test]
fn slow_keys_repeat_ignores_configuration() {
    let cfg = RepeatConfig::slow_keys();
    assert_eq!(cfg.initial_delay_ms, SLOW_KEYS_REPEAT_MS);
    assert_eq!(cfg.interval_ms, SLOW_KEYS_REPEAT_MS);
    assert_eq!(SLOW_KEYS_REPEAT_MS, 420);
}

#[test]
fn repeat_info_rounds_the_interval_to_a_rate() {
    let info = RepeatConfig::default().repeat_info();
    assert_eq!(
        info,
        RepeatInfo {
            rate: 12,
            delay: 500
        }
    );
}

#[test]
fn repeat_info_is_zero_when_disabled() {
    let cfg = RepeatConfig {
        initial_delay_ms: 0,
        interval_ms: 83,
    };
    assert_eq!(cfg.repeat_info(), RepeatInfo { rate: 0, delay: 0 });
}

#[test]
fn zero_interval_with_a_delay_advertises_the_fastest_rate() {
    let cfg = RepeatConfig {
        initial_delay_ms: 200,
        interval_ms: 0,
    };
    assert_eq!(
        cfg.repeat_info(),
        RepeatInfo {
            rate: 1000,
            delay: 200
        }
    );
}

#[test]
fn very_slow_interval_still_advertises_one_per_second() {
    let cfg = RepeatConfig {
        initial_delay_ms: 200,
        interval_ms: 5000,
    };
    assert_eq!(cfg.repeat_info().rate, 1);
}

#[test]
fn config_converts_back_to_preference_units() {
    let (delay, interval) = RepeatConfig::from_pref_units(30.0, 6.0).to_pref_units();
    assert!((delay - 30.0).abs() < 0.1);
    assert!((interval - 6.0).abs() < 0.1);
}

#[test]
fn slider_stops_are_ordered_slowest_first() {
    let t = SliderTable::new(vec![2.0, 120.0, 30.0]);
    assert_eq!(t.len(), 3);
    assert!(!t.is_empty());
    assert_eq!(t.value_at(0), Some(120.0));
    assert_eq!(t.value_at(2), Some(2.0));
    assert_eq!(t.value_at(3), None);
}

#[test]
fn default_slider_tables_have_seven_and_six_stops() {
    assert_eq!(SliderTable::interval_default().len(), 7);
    assert_eq!(SliderTable::delay_default().len(), 6);
}

#[test]
fn slider_position_is_the_first_stop_not_larger_than_the_value() {
    let t = SliderTable::interval_default();
    assert_eq!(t.position_of(120.0), 0);
    assert_eq!(t.position_of(100.0), 1);
    assert_eq!(t.position_of(6.0), 5);
    assert_eq!(t.position_of(1.0), 6);
}

#[test]
fn slider_position_of_an_empty_table_is_zero() {
    let t = SliderTable::new(Vec::new());
    assert!(t.is_empty());
    assert_eq!(t.position_of(5.0), 0);
}

#[test]
fn modifier_usages_never_repeat() {
    for usage in 0xE0..=0xE7 {
        assert!(!hid_usage_repeats(0x07, usage), "usage {usage:#x}");
    }
    assert!(hid_usage_repeats(0x07, 0x04));
}

#[test]
fn keypad_clear_and_power_usages_never_repeat() {
    for usage in [0x53, 0x66, 0x7F] {
        assert!(!hid_usage_repeats(0x07, usage));
    }
}

#[test]
fn transport_and_telephony_usages_never_repeat() {
    assert!(!hid_usage_repeats(0x0B, 0x01));
    assert!(!hid_usage_repeats(0x0C, 0xCD));
    assert!(hid_usage_repeats(0x0C, 0xE9));
    assert!(!hid_usage_repeats(0xFF09, 1));
    assert!(!hid_usage_repeats(0xFF, 6));
    assert!(hid_usage_repeats(0xFF, 8));
}

#[test]
fn modifier_and_lock_evdev_codes_do_not_repeat() {
    for code in [29, 42, 54, 56, 58, 97, 100, 125, 126, 464] {
        assert!(!evdev_repeats(code), "code {code}");
    }
    assert!(evdev_repeats(KEY_A));
}

#[test]
fn repeater_waits_for_the_initial_delay_then_ticks_at_the_interval() {
    let mut r = KeyRepeater::new(RepeatConfig {
        initial_delay_ms: 500,
        interval_ms: 100,
    });
    r.press(KEY_A, 1000, false);
    assert_eq!(r.key(), Some(KEY_A));
    assert_eq!(r.next_deadline(), Some(1500));
    assert!(r.poll(1499).is_empty());
    assert!(!r.is_repeating());
    let ticks = r.poll(1720);
    let times: Vec<u64> = ticks.iter().map(|t| t.time_ms).collect();
    assert_eq!(times, vec![1500, 1600, 1700]);
    assert!(ticks.iter().all(|t| t.key == KEY_A));
    assert!(r.is_repeating());
    assert_eq!(r.next_deadline(), Some(1800));
}

#[test]
fn repeater_stops_when_the_repeating_key_is_released() {
    let mut r = KeyRepeater::new(RepeatConfig::default());
    r.press(KEY_A, 0, false);
    r.release(KEY_A);
    assert_eq!(r.key(), None);
    assert!(r.poll(10_000).is_empty());
}

#[test]
fn releasing_another_key_does_not_stop_the_repeat() {
    let mut r = KeyRepeater::new(RepeatConfig::default());
    r.press(KEY_A, 0, false);
    r.release(KEY_B);
    assert_eq!(r.key(), Some(KEY_A));
}

#[test]
fn a_new_press_replaces_the_repeating_key() {
    let mut r = KeyRepeater::new(RepeatConfig::default());
    r.press(KEY_A, 0, false);
    r.press(KEY_B, 100, false);
    assert_eq!(r.key(), Some(KEY_B));
    assert_eq!(r.next_deadline(), Some(600));
}

#[test]
fn a_no_repeat_press_stops_the_previous_repeat() {
    let mut r = KeyRepeater::new(RepeatConfig::default());
    r.press(KEY_A, 0, false);
    r.press(KEY_B, 100, true);
    assert_eq!(r.key(), None);
}

#[test]
fn modifier_keys_and_disabled_repeat_never_arm_the_repeater() {
    let mut r = KeyRepeater::new(RepeatConfig::default());
    r.press(KEY_LEFTSHIFT, 0, false);
    assert_eq!(r.key(), None);
    let mut off = KeyRepeater::new(RepeatConfig {
        initial_delay_ms: 0,
        interval_ms: 83,
    });
    off.press(KEY_A, 0, false);
    assert_eq!(off.key(), None);
}

#[test]
fn stop_clears_the_repeat_and_set_config_replaces_the_timing() {
    let mut r = KeyRepeater::new(RepeatConfig::default());
    r.press(KEY_A, 0, false);
    r.poll(600);
    assert!(r.is_repeating());
    r.stop();
    assert!(!r.is_repeating());
    assert_eq!(r.key(), None);
    let cfg = RepeatConfig {
        initial_delay_ms: 100,
        interval_ms: 50,
    };
    r.set_config(cfg);
    assert_eq!(r.config(), cfg);
}

#[test]
fn caps_lock_press_counts_after_the_default_delay() {
    let d = CapsLockDelay::default();
    assert_eq!(d.delay_ms, 75);
    assert!(!d.press_counts(74));
    assert!(d.press_counts(75));
}

#[test]
fn caps_lock_zero_delay_always_counts() {
    let d = CapsLockDelay {
        delay_ms: 0,
        override_ms: None,
    };
    assert!(d.press_counts(0));
}

#[test]
fn caps_lock_positive_override_replaces_the_delay() {
    let d = CapsLockDelay {
        delay_ms: 75,
        override_ms: Some(250),
    };
    assert!(!d.press_counts(200));
    assert!(d.press_counts(250));
}

#[test]
fn caps_lock_zero_override_means_none() {
    let d = CapsLockDelay {
        delay_ms: 75,
        override_ms: Some(0),
    };
    assert!(!d.press_counts(10));
    assert!(d.press_counts(75));
}

#[test]
fn caps_hold_accepts_one_to_the_limit_and_otherwise_falls_back() {
    assert_eq!(caps_hold_duration(Some(1)), 1);
    assert_eq!(caps_hold_duration(Some(1500)), CAPS_HOLD_MAX_MS);
    assert_eq!(caps_hold_duration(Some(1501)), CAPS_HOLD_DEFAULT_MS);
    assert_eq!(caps_hold_duration(Some(0)), CAPS_HOLD_DEFAULT_MS);
    assert_eq!(caps_hold_duration(Some(-4)), CAPS_HOLD_DEFAULT_MS);
    assert_eq!(caps_hold_duration(None), 500);
}

#[test]
fn caps_override_accepts_zero_to_the_limit_and_otherwise_falls_back() {
    assert_eq!(caps_override_duration(Some(0)), 0);
    assert_eq!(caps_override_duration(Some(3000)), 3000);
    assert_eq!(caps_override_duration(Some(3001)), CAPS_OVERRIDE_DEFAULT_MS);
    assert_eq!(caps_override_duration(Some(-1)), 250);
    assert_eq!(caps_override_duration(None), 250);
}
