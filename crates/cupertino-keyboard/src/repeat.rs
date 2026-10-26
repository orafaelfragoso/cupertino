//! Key repeat model.
//!
//! the repeat generator is a per-keyboard filter that holds exactly one "repeating" key;
//! preference values are expressed in 1/60 s units; the initial delay
//! and interval reach the filter in nanoseconds and are clamped as described on
//! [`clamp_hid_ns`].
//!
//! Wayland: a compositor does not generate repeats for clients. It advertises
//! [`RepeatInfo`] through `wl_keyboard.repeat_info` and each client repeats on its own; use
//! [`KeyRepeater`] for repeats the compositor consumes itself (shortcuts, the accent picker trigger,
//! nested sessions).

/// One preference unit: 1/60 s in milliseconds.
pub const UNIT_MS: f64 = 1000.0 / 60.0;
/// Initial delay used until a value is configured.
pub const FALLBACK_INITIAL_MS: u32 = 500;
/// Interval used until a value is configured.
pub const FALLBACK_INTERVAL_MS: u32 = 83;
/// Initial delay *and* interval of the key repeat while slow keys are on (the filter passes the
/// constant 0x1a4 instead of the configured values). This is not the slow-keys acceptance delay, which is
/// [`crate::slow_keys::DEFAULT_DELAY_MS`].
pub const SLOW_KEYS_REPEAT_MS: u32 = 420;
/// Smallest enabled delay: configured values from 1 ms up to 16 ms are raised to 16 ms.
pub const MIN_ENABLED_MS: u32 = 16;

/// Slider stops of the settings UI in preference units. The binary reads the stops from preferences
/// (`<key>_Level_0..6`) and ships no table: these values are community observations
/// (approximate) and only a default for [`SliderTable`].
pub const COMMUNITY_REPEAT_STOPS: [f64; 7] = [120.0, 90.0, 60.0, 30.0, 12.0, 6.0, 2.0];
/// See [`COMMUNITY_REPEAT_STOPS`] (approximate, six stops).
pub const COMMUNITY_DELAY_STOPS: [f64; 6] = [120.0, 94.0, 68.0, 35.0, 25.0, 15.0];

/// Converts a nanosecond value as received by the repeat filter into milliseconds.
///
/// * below 1 ms: 0 (repeat disabled; the filter skips repeat generation when the initial delay is 0);
/// * 1 ms ..= 15.999 ms: [`MIN_ENABLED_MS`];
/// * otherwise `ns / 1_000_000` (integer division).
pub fn clamp_hid_ns(ns: u64) -> u32 {
    if ns < 1_000_000 {
        0
    } else if ns < 16_000_000 {
        MIN_ENABLED_MS
    } else {
        (ns / 1_000_000).min(u32::MAX as u64) as u32
    }
}

/// `wl_keyboard.repeat_info` payload: `rate` in characters per second (0 disables), `delay` in ms.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RepeatInfo {
    pub rate: i32,
    pub delay: i32,
}

/// Initial delay and repeat interval in milliseconds (0 = disabled).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RepeatConfig {
    pub initial_delay_ms: u32,
    pub interval_ms: u32,
}

impl Default for RepeatConfig {
    fn default() -> Self {
        RepeatConfig {
            initial_delay_ms: FALLBACK_INITIAL_MS,
            interval_ms: FALLBACK_INTERVAL_MS,
        }
    }
}

impl RepeatConfig {
    pub fn from_hid_ns(initial_ns: u64, interval_ns: u64) -> RepeatConfig {
        RepeatConfig {
            initial_delay_ms: clamp_hid_ns(initial_ns),
            interval_ms: clamp_hid_ns(interval_ns),
        }
    }

    /// Builds a config from preference units (1/60 s each).
    pub fn from_pref_units(initial_units: f64, interval_units: f64) -> RepeatConfig {
        let ns = |u: f64| (u.max(0.0) * UNIT_MS * 1e6).round() as u64;
        Self::from_hid_ns(ns(initial_units), ns(interval_units))
    }

    /// The repeat used while slow keys are on, whatever is configured.
    pub fn slow_keys() -> RepeatConfig {
        RepeatConfig {
            initial_delay_ms: SLOW_KEYS_REPEAT_MS,
            interval_ms: SLOW_KEYS_REPEAT_MS,
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.initial_delay_ms != 0
    }

    /// Value for `wl_keyboard.repeat_info`. A zero interval with an enabled delay is advertised as
    /// the fastest representable rate (1000/s).
    pub fn repeat_info(&self) -> RepeatInfo {
        if !self.is_enabled() {
            return RepeatInfo { rate: 0, delay: 0 };
        }
        let rate = if self.interval_ms == 0 {
            1000
        } else {
            (1000.0 / self.interval_ms as f64).round() as i32
        };
        RepeatInfo {
            rate: rate.max(1),
            delay: self.initial_delay_ms as i32,
        }
    }

    /// Preference units (1/60 s) for the initial delay and the interval.
    pub fn to_pref_units(&self) -> (f64, f64) {
        (
            self.initial_delay_ms as f64 / UNIT_MS,
            self.interval_ms as f64 / UNIT_MS,
        )
    }
}

/// Slider stops for the repeat settings.
#[derive(Clone, Debug, PartialEq)]
pub struct SliderTable {
    stops: Vec<f64>,
}

impl SliderTable {
    pub fn new(mut stops: Vec<f64>) -> SliderTable {
        stops.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
        SliderTable { stops }
    }

    pub fn interval_default() -> SliderTable {
        SliderTable::new(COMMUNITY_REPEAT_STOPS.to_vec())
    }

    pub fn delay_default() -> SliderTable {
        SliderTable::new(COMMUNITY_DELAY_STOPS.to_vec())
    }

    pub fn len(&self) -> usize {
        self.stops.len()
    }

    pub fn is_empty(&self) -> bool {
        self.stops.is_empty()
    }

    /// Slider position 0 = slowest. The stop whose value is the first not larger than `units`
    /// (the settings UI walks the stops from slow to fast and stops at the first one `<= value`).
    pub fn position_of(&self, units: f64) -> usize {
        for (i, s) in self.stops.iter().enumerate() {
            if *s <= units {
                return i;
            }
        }
        self.stops.len().saturating_sub(1)
    }

    pub fn value_at(&self, position: usize) -> Option<f64> {
        self.stops.get(position).copied()
    }
}

/// Whether a (HID usage page, usage) pair repeats while held (the filter).
///
/// Modifiers, keypad clear/power/mute, telephony keys, power/sleep/menu and transport keys, and a few vendor keys never repeat.
pub fn hid_usage_repeats(page: u32, usage: u32) -> bool {
    match page {
        0x07 => {
            let modifier = (0xE0..=0xE7).contains(&usage);
            let special = usage == 0x53 || usage == 0x66 || usage == 0x7F;
            !(modifier || special)
        }
        0x0B => false,
        0x0C => !(matches!(usage, 0x30 | 0x32 | 0x40) || matches!(usage, 0xB0 | 0xB8 | 0xCD)),
        0xFF => !(matches!(usage, 6 | 7)),
        0xFF09 => false,
        0xFF01 => usage != 0x22,
        _ => true,
    }
}

/// Linux evdev key codes that never repeat (approximate mapping of [`hid_usage_repeats`]).
pub fn evdev_repeats(code: u32) -> bool {
    !matches!(
        code,
        29 | 42
            | 54
            | 56
            | 58
            | 69
            | 97
            | 100
            | 113
            | 116
            | 125
            | 126
            | 139
            | 142
            | 161
            | 164
            | 207
            | 464
    )
}

/// One repeat tick.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RepeatTick {
    pub key: u32,
    pub time_ms: u64,
}

/// Single-key repeat generator: a new key press replaces the repeating key, releasing the repeating key stops it,
/// releasing another key does not.
#[derive(Clone, Debug)]
pub struct KeyRepeater {
    cfg: RepeatConfig,
    held: Option<(u32, u64)>, // key, next deadline
    repeating: bool,
}

impl KeyRepeater {
    pub fn new(cfg: RepeatConfig) -> KeyRepeater {
        KeyRepeater {
            cfg,
            held: None,
            repeating: false,
        }
    }

    pub fn config(&self) -> RepeatConfig {
        self.cfg
    }

    pub fn set_config(&mut self, cfg: RepeatConfig) {
        self.cfg = cfg;
    }

    /// `no_repeat` mirrors the event flag that suppresses repeat for a single press.
    pub fn press(&mut self, key: u32, time_ms: u64, no_repeat: bool) {
        self.repeating = false;
        if no_repeat || !self.cfg.is_enabled() || !evdev_repeats(key) {
            self.held = None;
            return;
        }
        self.held = Some((key, time_ms + self.cfg.initial_delay_ms as u64));
    }

    pub fn release(&mut self, key: u32) {
        if matches!(self.held, Some((k, _)) if k == key) {
            self.held = None;
            self.repeating = false;
        }
    }

    /// Stop whatever key is repeating.
    pub fn stop(&mut self) {
        self.held = None;
        self.repeating = false;
    }

    /// The key that is repeating or waiting for its first repeat.
    pub fn key(&self) -> Option<u32> {
        self.held.map(|(k, _)| k)
    }

    pub fn next_deadline(&self) -> Option<u64> {
        self.held.map(|(_, t)| t)
    }

    pub fn is_repeating(&self) -> bool {
        self.repeating
    }

    /// Returns every tick due at or before `now_ms`.
    pub fn poll(&mut self, now_ms: u64) -> Vec<RepeatTick> {
        let mut out = Vec::new();
        let step = self.cfg.interval_ms.max(1) as u64;
        while let Some((key, due)) = self.held {
            if due > now_ms {
                break;
            }
            out.push(RepeatTick { key, time_ms: due });
            self.repeating = true;
            self.held = Some((key, due + step));
        }
        out
    }
}

/// Caps-lock activation delay: the key press is held back and delivered only if the key is still down
/// after the delay (the filter; the default delay of 75 ms comes from the
/// built-in keyboard personalities).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CapsLockDelay {
    pub delay_ms: u32,
    /// Override used while an input-source switch by caps lock is armed (0 / negative = none).
    pub override_ms: Option<u32>,
}

impl Default for CapsLockDelay {
    fn default() -> Self {
        CapsLockDelay {
            delay_ms: 75,
            override_ms: None,
        }
    }
}

impl CapsLockDelay {
    fn effective(&self) -> u32 {
        match self.override_ms {
            Some(v) if v > 0 => v,
            _ => self.delay_ms,
        }
    }

    /// Decides the outcome of a press that lasted `held_ms`: `true` when the press must be delivered.
    pub fn press_counts(&self, held_ms: u32) -> bool {
        let d = self.effective();
        d == 0 || held_ms >= d
    }
}

/// Duration limits used for caps-lock hold-to-switch input source switching.
pub const CAPS_HOLD_DEFAULT_MS: u32 = 500;
/// The upper bound the settings file accepts (`keyboard.conf`, defined with the file in `cupertino_defaults::keyboard`).
pub const CAPS_HOLD_MAX_MS: u32 = cupertino_defaults::keyboard::CAPS_LOCK_HOLD_MAX_MS;
pub const CAPS_OVERRIDE_DEFAULT_MS: u32 = 250;
pub const CAPS_OVERRIDE_MAX_MS: u32 = 3000;

/// Validates the configured caps-lock hold duration: accepted range 1..=1500 ms, otherwise the default.
pub fn caps_hold_duration(configured: Option<i64>) -> u32 {
    match configured {
        Some(v) if (1..=CAPS_HOLD_MAX_MS as i64).contains(&v) => v as u32,
        _ => CAPS_HOLD_DEFAULT_MS,
    }
}

/// Validates the delay override duration: values above 3000 ms fall back to 250 ms.
pub fn caps_override_duration(configured: Option<i64>) -> u32 {
    match configured {
        Some(v) if (0..=CAPS_OVERRIDE_MAX_MS as i64).contains(&v) => v as u32,
        _ => CAPS_OVERRIDE_DEFAULT_MS,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_matches_filter() {
        assert_eq!(clamp_hid_ns(0), 0);
        assert_eq!(clamp_hid_ns(999_999), 0);
        assert_eq!(clamp_hid_ns(1_000_000), 16);
        assert_eq!(clamp_hid_ns(15_999_999), 16);
        assert_eq!(clamp_hid_ns(16_000_000), 16);
        assert_eq!(clamp_hid_ns(250_000_000), 250);
        assert_eq!(clamp_hid_ns(33_333_333), 33);
    }

    #[test]
    fn pref_units_are_sixtieths_of_a_second() {
        let c = RepeatConfig::from_pref_units(15.0, 2.0);
        assert_eq!(c.initial_delay_ms, 250);
        assert_eq!(c.interval_ms, 33);
        let (i, r) = RepeatConfig::from_pref_units(25.0, 6.0).to_pref_units();
        assert!((i - 25.0).abs() < 0.1 && (r - 6.0).abs() < 0.2);
    }

    #[test]
    fn repeat_info_conversion() {
        assert_eq!(
            RepeatConfig::default().repeat_info(),
            RepeatInfo {
                rate: 12,
                delay: 500
            }
        );
        assert_eq!(
            RepeatConfig {
                initial_delay_ms: 0,
                interval_ms: 83
            }
            .repeat_info(),
            RepeatInfo { rate: 0, delay: 0 }
        );
        assert_eq!(RepeatConfig::slow_keys().repeat_info().delay, 420);
    }

    #[test]
    fn slider_walks_slow_to_fast() {
        let t = SliderTable::interval_default();
        assert_eq!(t.position_of(120.0), 0);
        assert_eq!(t.position_of(6.0), 5);
        assert_eq!(t.position_of(1.0), 6);
        assert_eq!(t.value_at(4), Some(12.0));
        let d = SliderTable::delay_default();
        assert_eq!(d.position_of(25.0), 4);
        assert_eq!(d.len(), 6);
    }

    #[test]
    fn repeater_single_key_semantics() {
        let mut r = KeyRepeater::new(RepeatConfig {
            initial_delay_ms: 400,
            interval_ms: 100,
        });
        r.press(30, 0, false);
        assert_eq!(r.poll(399), vec![]);
        assert_eq!(
            r.poll(650),
            vec![
                RepeatTick {
                    key: 30,
                    time_ms: 400
                },
                RepeatTick {
                    key: 30,
                    time_ms: 500
                },
                RepeatTick {
                    key: 30,
                    time_ms: 600
                }
            ]
        );
        assert!(r.is_repeating());
        r.press(31, 700, false);
        assert!(!r.is_repeating());
        r.release(30);
        assert_eq!(r.next_deadline(), Some(1100));
        r.release(31);
        assert_eq!(r.next_deadline(), None);
    }

    #[test]
    fn modifiers_and_flagged_presses_do_not_repeat() {
        let mut r = KeyRepeater::new(RepeatConfig::default());
        r.press(42, 0, false);
        assert_eq!(r.next_deadline(), None);
        r.press(30, 0, true);
        assert_eq!(r.next_deadline(), None);
        r.set_config(RepeatConfig {
            initial_delay_ms: 0,
            interval_ms: 0,
        });
        r.press(30, 0, false);
        assert_eq!(r.next_deadline(), None);
    }

    #[test]
    fn hid_exclusions() {
        assert!(!hid_usage_repeats(0x07, 0xE1));
        assert!(!hid_usage_repeats(0x07, 0x53));
        assert!(hid_usage_repeats(0x07, 0x04));
        assert!(!hid_usage_repeats(0x0C, 0xB8));
        assert!(hid_usage_repeats(0x0C, 0xE9));
        assert!(!hid_usage_repeats(0xFF01, 0x22));
    }

    #[test]
    fn caps_lock_delay() {
        let d = CapsLockDelay::default();
        assert!(!d.press_counts(74));
        assert!(d.press_counts(75));
        let o = CapsLockDelay {
            delay_ms: 75,
            override_ms: Some(250),
        };
        assert!(!o.press_counts(200));
        assert!(o.press_counts(250));
        assert_eq!(caps_hold_duration(Some(0)), 500);
        assert_eq!(caps_hold_duration(Some(1500)), 1500);
        assert_eq!(caps_hold_duration(Some(1501)), 500);
        assert_eq!(caps_override_duration(Some(3001)), 250);
    }
}
