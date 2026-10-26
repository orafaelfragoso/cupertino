//! Mouse keys: the keypad moves the pointer and presses its button.
//!
//! * Toggle (keyboard filter): with the shortcut on, five presses of an Alt key within 30 s toggle
//!   the feature on the release of the fifth; the 30 s window starts with the first press and any other key event
//!   resets the count.
//! * Motion (pointer driver): a direction key moves the pointer one point when it goes down; after the initial
//!   delay (default 1 s) a 50 ms timer repeats the move (its first fire is 50 ms after the delay, the timer is
//!   created when the delay elapses). Speed and tick both start at 1; every repeat first ramps the
//!   speed, then moves `speed` points along each axis of the direction. With a maximum speed below 36 the ramp adds 1
//!   per tick; otherwise the speed follows `tick^1.5` (below 110), `tick^2` (below 160), `tick^3` (below 250) and
//!   `tick^5`, clamped to the maximum (default 11). With an initial delay of 0 there is no first move and the repeat
//!   starts at once. Releasing the direction key stops it; a new direction key replaces it.
//! * Keys (the design user guide): keypad 8 / 2 / 4 / 6 move up / down / left / right, 7 / 9 / 1 / 3
//!   diagonally, 5 clicks (button down with the key, up with its release), 0 presses and holds the button, the keypad
//!   decimal point releases it. While mouse keys are on these keys do not type.
//!
//! Key codes are Linux evdev codes; times are milliseconds on any monotonic clock; distances are logical points with
//! y growing downwards.

/// Alt presses that toggle mouse keys.
pub const TOGGLE_PRESSES: u32 = 5;
/// Window that starts with the first counted Alt press, ms.
pub const TOGGLE_WINDOW_MS: u64 = 30_000;
/// Delay before a held direction key repeats, ms (1 s).
pub const DEFAULT_INITIAL_DELAY_MS: u64 = 1000;
/// Repeat interval of a held direction key, ms (0.05 s).
pub const TICK_MS: u64 = 50;
/// Maximum speed in points per tick.
pub const DEFAULT_MAX_SPEED: u32 = 11;

/// What the pointer does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PointerAction {
    /// Move by this many points.
    Move { dx: i32, dy: i32 },
    /// Press the primary button.
    Press,
    /// Release the primary button.
    Release,
}

/// The result of one key event.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MouseKeysOutcome {
    /// The key belongs to mouse keys and must not reach the keyboard.
    pub consumed: bool,
    pub actions: Vec<PointerAction>,
    /// The event toggled the feature: `Some(true)` when it is on now.
    pub toggled: Option<bool>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Role {
    Direction(i32, i32),
    Click,
    Hold,
    Release,
}

/// The role of a keypad key (evdev code).
fn role_of(code: u32) -> Option<Role> {
    Some(match code {
        72 => Role::Direction(0, -1),
        80 => Role::Direction(0, 1),
        75 => Role::Direction(-1, 0),
        77 => Role::Direction(1, 0),
        71 => Role::Direction(-1, -1),
        73 => Role::Direction(1, -1),
        79 => Role::Direction(-1, 1),
        81 => Role::Direction(1, 1),
        76 => Role::Click,
        82 => Role::Hold,
        83 => Role::Release,
        _ => return None,
    })
}

fn is_alt(code: u32) -> bool {
    matches!(code, 56 | 100)
}

/// Speed after one ramp step (the pointer driver's `_repeatedHotKey:`); `tick` advances only on the power branches.
pub fn ramp(speed: u32, tick: &mut u32, max_speed: u32) -> u32 {
    if speed >= max_speed {
        return speed;
    }
    if max_speed < 36 {
        return speed + 1;
    }
    let t = f64::from(*tick);
    *tick += 1;
    let next = if speed < 110 {
        t.powf(1.5)
    } else if speed < 160 {
        t * t
    } else if speed < 250 {
        t.powf(3.0)
    } else {
        t.powf(5.0)
    };
    (next as u32).min(max_speed)
}

#[derive(Clone, Copy, Debug)]
struct Moving {
    code: u32,
    dir: (i32, i32),
    next_ms: u64,
    speed: u32,
    tick: u32,
}

/// Mouse-keys state.
#[derive(Clone, Debug)]
pub struct MouseKeys {
    enabled: bool,
    shortcut: bool,
    initial_delay_ms: u64,
    max_speed: u32,
    alt_presses: u32,
    window_start_ms: u64,
    moving: Option<Moving>,
}

impl MouseKeys {
    pub fn new(enabled: bool, shortcut: bool, initial_delay_ms: u64, max_speed: u32) -> MouseKeys {
        MouseKeys { enabled, shortcut, initial_delay_ms, max_speed, alt_presses: 0, window_start_ms: 0, moving: None }
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn set_enabled(&mut self, on: bool) {
        self.enabled = on;
        if !on {
            self.moving = None;
        }
    }

    /// One key event at `now_ms`.
    pub fn key(&mut self, code: u32, pressed: bool, now_ms: u64) -> MouseKeysOutcome {
        let mut out = MouseKeysOutcome::default();
        if is_alt(code) {
            if self.count_alt(pressed, now_ms) {
                let on = !self.enabled;
                self.set_enabled(on);
                out.toggled = Some(on);
            }
            return out;
        }
        self.alt_presses = 0;
        if !self.enabled {
            return out;
        }
        let Some(role) = role_of(code) else { return out };
        out.consumed = true;
        match (role, pressed) {
            (Role::Direction(dx, dy), true) => {
                let first = self.initial_delay_ms > 0;
                if first {
                    out.actions.push(PointerAction::Move { dx, dy });
                }
                let next_ms = now_ms + if first { self.initial_delay_ms + TICK_MS } else { TICK_MS };
                self.moving = Some(Moving { code, dir: (dx, dy), next_ms, speed: 1, tick: 1 });
            }
            (Role::Direction(..), false) => {
                if self.moving.is_some_and(|m| m.code == code) {
                    self.moving = None;
                }
            }
            (Role::Click, true) | (Role::Hold, true) => out.actions.push(PointerAction::Press),
            (Role::Click, false) | (Role::Release, true) => out.actions.push(PointerAction::Release),
            (Role::Hold, false) | (Role::Release, false) => {}
        }
        out
    }

    /// When the held direction key moves next.
    pub fn deadline(&self) -> Option<u64> {
        self.moving.map(|m| m.next_ms)
    }

    /// Every repeat move due at or before `now_ms`.
    pub fn poll(&mut self, now_ms: u64) -> Vec<PointerAction> {
        let mut out = Vec::new();
        let max = self.max_speed;
        while let Some(m) = self.moving.as_mut().filter(|m| m.next_ms <= now_ms) {
            m.speed = ramp(m.speed, &mut m.tick, max);
            let step = i32::try_from(m.speed).unwrap_or(i32::MAX);
            out.push(PointerAction::Move { dx: m.dir.0 * step, dy: m.dir.1 * step });
            m.next_ms += TICK_MS;
        }
        out
    }

    /// Count one Alt event; true when it toggles the feature.
    fn count_alt(&mut self, pressed: bool, now_ms: u64) -> bool {
        if !self.shortcut {
            return false;
        }
        if pressed {
            if self.alt_presses == 0 || now_ms.saturating_sub(self.window_start_ms) >= TOGGLE_WINDOW_MS {
                self.alt_presses = 0;
                self.window_start_ms = now_ms;
            }
            self.alt_presses += 1;
            return false;
        }
        if self.alt_presses >= TOGGLE_PRESSES {
            self.alt_presses = 0;
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KP8: u32 = 72;
    const KP6: u32 = 77;
    const KP3: u32 = 81;
    const KP5: u32 = 76;
    const KP0: u32 = 82;
    const KPDOT: u32 = 83;
    const ALT: u32 = 56;
    const A: u32 = 30;

    fn on() -> MouseKeys {
        MouseKeys::new(true, false, DEFAULT_INITIAL_DELAY_MS, DEFAULT_MAX_SPEED)
    }

    #[test]
    fn a_direction_key_moves_one_point_then_repeats_after_the_delay_with_a_ramp() {
        let mut m = on();
        let out = m.key(KP6, true, 0);
        assert!(out.consumed);
        assert_eq!(out.actions, vec![PointerAction::Move { dx: 1, dy: 0 }]);
        assert_eq!(m.poll(1049), vec![]);
        // first repeat 50 ms after the 1 s delay; the speed ramps by one per tick up to 11
        let moves: Vec<i32> = m.poll(1000 + 50 * 12).iter().map(|a| if let PointerAction::Move { dx, .. } = a { *dx } else { 0 }).collect();
        assert_eq!(moves, vec![2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 11, 11]);
        m.key(KP6, false, 1700);
        assert_eq!(m.poll(5000), vec![]);
    }

    #[test]
    fn diagonals_move_on_both_axes_and_up_is_negative_y() {
        let mut m = on();
        assert_eq!(m.key(KP3, true, 0).actions, vec![PointerAction::Move { dx: 1, dy: 1 }]);
        assert_eq!(m.key(KP8, true, 10).actions, vec![PointerAction::Move { dx: 0, dy: -1 }], "a new direction replaces the old one");
        assert_eq!(m.poll(1060), vec![PointerAction::Move { dx: 0, dy: -2 }]);
        m.key(KP3, false, 1070);
        assert_eq!(m.deadline(), Some(1110), "releasing the replaced key does not stop the new one");
    }

    #[test]
    fn without_an_initial_delay_the_repeat_starts_at_once() {
        let mut m = MouseKeys::new(true, false, 0, DEFAULT_MAX_SPEED);
        assert_eq!(m.key(KP6, true, 0).actions, vec![]);
        assert_eq!(m.poll(100), vec![PointerAction::Move { dx: 2, dy: 0 }, PointerAction::Move { dx: 3, dy: 0 }]);
    }

    #[test]
    fn click_hold_and_release_keys() {
        let mut m = on();
        assert_eq!(m.key(KP5, true, 0).actions, vec![PointerAction::Press]);
        assert_eq!(m.key(KP5, false, 10).actions, vec![PointerAction::Release]);
        assert_eq!(m.key(KP0, true, 20).actions, vec![PointerAction::Press]);
        assert_eq!(m.key(KP0, false, 30), MouseKeysOutcome { consumed: true, ..Default::default() });
        assert_eq!(m.key(KPDOT, true, 40).actions, vec![PointerAction::Release]);
        assert!(!m.key(A, true, 50).consumed, "other keys type");
    }

    #[test]
    fn five_alt_presses_toggle_on_the_fifth_release() {
        let mut m = MouseKeys::new(false, true, DEFAULT_INITIAL_DELAY_MS, DEFAULT_MAX_SPEED);
        for t in 0..4 {
            m.key(ALT, true, t * 100);
            assert_eq!(m.key(ALT, false, t * 100 + 50).toggled, None);
        }
        assert!(!m.key(KP6, true, 450).consumed, "keypad keys type while it is off");
        // the keypad key reset the count
        for t in 0..5 {
            m.key(ALT, true, 1000 + t * 100);
            let out = m.key(ALT, false, 1050 + t * 100);
            assert_eq!(out.toggled, (t == 4).then_some(true));
        }
        assert!(m.enabled());
        assert!(m.key(KP6, true, 2000).consumed);
    }

    #[test]
    fn the_alt_count_window_is_30_s_and_needs_the_shortcut() {
        let mut m = MouseKeys::new(false, true, DEFAULT_INITIAL_DELAY_MS, DEFAULT_MAX_SPEED);
        for t in 0..4 {
            m.key(ALT, true, t);
            m.key(ALT, false, t);
        }
        m.key(ALT, true, 30_000);
        assert_eq!(m.key(ALT, false, 30_001).toggled, None, "the fifth press came after the window");
        let mut quiet = MouseKeys::new(false, false, DEFAULT_INITIAL_DELAY_MS, DEFAULT_MAX_SPEED);
        for t in 0..6 {
            quiet.key(ALT, true, t);
            quiet.key(ALT, false, t);
        }
        assert!(!quiet.enabled());
    }

    #[test]
    fn fast_maximum_speeds_follow_the_power_ramp() {
        let mut tick = 1;
        let mut speeds = Vec::new();
        let mut s = 1;
        for _ in 0..6 {
            s = ramp(s, &mut tick, 1000);
            speeds.push(s);
        }
        // tick^1.5 for ticks 1..=6, truncated
        assert_eq!(speeds, vec![1, 2, 5, 8, 11, 14]);
        assert_eq!(ramp(200, &mut 7, 1000), 343, "tick^3 between 160 and 250");
        assert_eq!(ramp(300, &mut 3, 1000), 243, "tick^5 above 250");
        assert_eq!(ramp(300, &mut 9, 400), 400, "clamped to the maximum");
        assert_eq!(ramp(400, &mut 9, 400), 400);
    }
}
