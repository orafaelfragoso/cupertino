//! Key repeat timing, the wl_keyboard advertisement, the repeater and the caps-lock rules.

use cupertino_keyboard::repeat::{
    caps_hold_duration, caps_override_duration, clamp_hid_ns, evdev_repeats, hid_usage_repeats,
    SliderTable, CAPS_HOLD_DEFAULT_MS, CAPS_HOLD_MAX_MS, CAPS_OVERRIDE_DEFAULT_MS,
    FALLBACK_INITIAL_MS, FALLBACK_INTERVAL_MS, MIN_ENABLED_MS, SLOW_KEYS_REPEAT_MS,
};
use cupertino_keyboard::{CapsLockDelay, KeyRepeater, RepeatConfig, RepeatInfo};

const KEY_A: u32 = 30;
const KEY_B: u32 = 48;
const KEY_LEFTSHIFT: u32 = 42;

fn cfg(initial_delay_ms: u32, interval_ms: u32) -> RepeatConfig {
    RepeatConfig {
        initial_delay_ms,
        interval_ms,
    }
}

#[test]
fn hid_nanoseconds_clamp_to_off_the_minimum_or_milliseconds() {
    for (ns, ms) in [
        (0, 0),
        (999_999, 0),
        (1_000_000, MIN_ENABLED_MS),
        (15_999_999, MIN_ENABLED_MS),
        (16_000_000, 16),
        (500_999_999, 500),
    ] {
        assert_eq!(clamp_hid_ns(ns), ms, "{ns} ns");
    }
    assert_eq!(
        RepeatConfig::from_hid_ns(2_000_000, 300_000_000),
        cfg(MIN_ENABLED_MS, 300),
        "the clamp applies to both values"
    );
}

#[test]
fn configs_come_from_preferences_and_convert_back() {
    assert_eq!(
        RepeatConfig::default(),
        cfg(FALLBACK_INITIAL_MS, FALLBACK_INTERVAL_MS)
    );
    assert_eq!(
        RepeatConfig::slow_keys(),
        cfg(SLOW_KEYS_REPEAT_MS, SLOW_KEYS_REPEAT_MS)
    );
    let from_units = RepeatConfig::from_pref_units(30.0, 6.0);
    assert_eq!(from_units, cfg(500, 100), "units are sixtieths of a second");
    let (delay, interval) = from_units.to_pref_units();
    assert!((delay - 30.0).abs() < 0.1 && (interval - 6.0).abs() < 0.1);
    assert!(!RepeatConfig::from_pref_units(-5.0, 6.0).is_enabled());
}

#[test]
fn repeat_info_advertises_a_rate_and_delay() {
    for (config, info) in [
        (
            RepeatConfig::default(),
            RepeatInfo {
                rate: 12,
                delay: 500,
            },
        ),
        (cfg(0, 83), RepeatInfo { rate: 0, delay: 0 }),
        (
            cfg(200, 0),
            RepeatInfo {
                rate: 1000,
                delay: 200,
            },
        ),
        (
            cfg(200, 5000),
            RepeatInfo {
                rate: 1,
                delay: 200,
            },
        ),
    ] {
        assert_eq!(config.repeat_info(), info, "{config:?}");
    }
}

#[test]
fn slider_tables_order_stops_slowest_first_and_find_positions() {
    let t = SliderTable::new(vec![2.0, 120.0, 30.0]);
    assert_eq!(t.len(), 3);
    for (i, v) in [(0, Some(120.0)), (2, Some(2.0)), (3, None)] {
        assert_eq!(t.value_at(i), v, "stop {i}");
    }
    let interval = SliderTable::interval_default();
    for (value, position) in [(120.0, 0), (100.0, 1), (6.0, 5), (1.0, 6)] {
        assert_eq!(interval.position_of(value), position, "value {value}");
    }
    let empty = SliderTable::new(Vec::new());
    assert!(empty.is_empty());
    assert_eq!(empty.position_of(5.0), 0);
}

#[test]
fn modifier_lock_and_special_keys_never_repeat() {
    for (page, usage, repeats) in [
        (0x07, 0x04, true),
        (0x07, 0xE0, false),
        (0x07, 0xE7, false),
        (0x07, 0x53, false),
        (0x07, 0x66, false),
        (0x07, 0x7F, false),
        (0x0B, 0x01, false),
        (0x0C, 0xCD, false),
        (0x0C, 0xE9, true),
        (0xFF09, 1, false),
        (0xFF, 6, false),
        (0xFF, 8, true),
    ] {
        assert_eq!(
            hid_usage_repeats(page, usage),
            repeats,
            "page {page:#x} usage {usage:#x}"
        );
    }
    for code in [29, 42, 54, 56, 58, 97, 100, 125, 126, 464] {
        assert!(!evdev_repeats(code), "evdev {code}");
    }
    assert!(evdev_repeats(KEY_A));
}

#[test]
fn the_repeater_waits_for_the_delay_then_ticks_at_the_interval() {
    let mut r = KeyRepeater::new(cfg(500, 100));
    r.press(KEY_A, 1000, false);
    assert_eq!(r.key(), Some(KEY_A));
    assert_eq!(r.next_deadline(), Some(1500));
    assert!(r.poll(1499).is_empty());
    assert!(!r.is_repeating());
    let ticks = r.poll(1720);
    let got: Vec<(u32, u64)> = ticks.iter().map(|t| (t.key, t.time_ms)).collect();
    assert_eq!(got, [(KEY_A, 1500), (KEY_A, 1600), (KEY_A, 1700)]);
    assert!(r.is_repeating());
    assert_eq!(r.next_deadline(), Some(1800));
}

#[test]
fn the_repeating_key_follows_presses_and_releases() {
    let mut r = KeyRepeater::new(RepeatConfig::default());
    r.press(KEY_A, 0, false);
    r.release(KEY_B);
    assert_eq!(r.key(), Some(KEY_A), "releasing another key keeps it");
    r.press(KEY_B, 100, false);
    assert_eq!(r.key(), Some(KEY_B), "a new press replaces it");
    assert_eq!(r.next_deadline(), Some(600));
    r.release(KEY_B);
    assert_eq!(r.key(), None, "releasing it stops the repeat");
    assert!(r.poll(10_000).is_empty());

    r.press(KEY_A, 0, false);
    r.press(KEY_B, 100, true);
    assert_eq!(r.key(), None, "a no-repeat press stops the previous repeat");

    r.press(KEY_LEFTSHIFT, 0, false);
    assert_eq!(r.key(), None, "modifier keys never arm it");
    let mut off = KeyRepeater::new(cfg(0, 83));
    off.press(KEY_A, 0, false);
    assert_eq!(off.key(), None, "disabled repeat never arms it");

    r.press(KEY_A, 0, false);
    r.poll(600);
    assert!(r.is_repeating());
    r.stop();
    assert!(!r.is_repeating());
    assert_eq!(r.key(), None);
    r.set_config(cfg(100, 50));
    assert_eq!(r.config(), cfg(100, 50));
}

#[test]
fn a_caps_lock_press_counts_after_the_delay_or_its_positive_override() {
    for (delay_ms, override_ms, held, counts) in [
        (75, None, 74, false),
        (75, None, 75, true),
        (0, None, 0, true),
        (75, Some(250), 200, false),
        (75, Some(250), 250, true),
        (75, Some(0), 10, false),
        (75, Some(0), 75, true),
    ] {
        let d = CapsLockDelay {
            delay_ms,
            override_ms,
        };
        assert_eq!(
            d.press_counts(held),
            counts,
            "delay {delay_ms} override {override_ms:?} held {held}"
        );
    }
    assert_eq!(CapsLockDelay::default().delay_ms, 75);
}

#[test]
fn caps_lock_durations_fall_back_outside_their_range() {
    for (value, ms) in [
        (Some(1), 1),
        (Some(CAPS_HOLD_MAX_MS as i64), CAPS_HOLD_MAX_MS),
        (Some(CAPS_HOLD_MAX_MS as i64 + 1), CAPS_HOLD_DEFAULT_MS),
        (Some(0), CAPS_HOLD_DEFAULT_MS),
        (Some(-4), CAPS_HOLD_DEFAULT_MS),
        (None, CAPS_HOLD_DEFAULT_MS),
    ] {
        assert_eq!(caps_hold_duration(value), ms, "hold {value:?}");
    }
    for (value, ms) in [
        (Some(0), 0),
        (Some(3000), 3000),
        (Some(3001), CAPS_OVERRIDE_DEFAULT_MS),
        (Some(-1), CAPS_OVERRIDE_DEFAULT_MS),
        (None, CAPS_OVERRIDE_DEFAULT_MS),
    ] {
        assert_eq!(caps_override_duration(value), ms, "override {value:?}");
    }
}
