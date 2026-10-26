//! Sticky keys: modifiers latch for the next key or lock until pressed again.
//!
//! The design keyboard filter keeps one state per modifier (left and right keys share one slot; caps lock is not a
//! sticky modifier and counts as an ordinary key):
//!
//! | state | key down | key up |
//! |---|---|---|
//! | idle | → pressed; delivered | delivered (a plain release), stays idle |
//! | pressed | swallowed (cannot happen) | → latched; swallowed, so the modifier stays down |
//! | latched | → locked; delivered | swallowed (cannot happen) |
//! | locked | → idle; swallowed | swallowed, stays locked |
//! | chorded | swallowed (cannot happen) | → idle; delivered |
//!
//! A non-modifier key going down turns every *pressed* modifier into *chorded* (the modifier is used as an ordinary
//! held modifier). A non-modifier key going up does the same and then releases every *latched* modifier (a release
//! event, state back to idle): a latched modifier applies to exactly one key, a locked one stays down.
//!
//! Five releases of a Shift key in a row toggle the feature when the shortcut is on: the count starts a 30 s window on
//! the first release and any other key event resets it. Turning sticky keys on releases the modifiers that are held at
//! that moment; turning it off releases every modifier that is latched, locked or pressed.
//!
//! Key codes are Linux evdev codes; times are milliseconds on any monotonic clock.

use std::collections::BTreeSet;

/// Shift releases that toggle sticky keys.
pub const TOGGLE_PRESSES: u32 = 5;
/// Window that starts with the first counted Shift release, ms (30 s timer).
pub const TOGGLE_WINDOW_MS: u64 = 30_000;

/// One sticky modifier slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Modifier {
    Shift,
    Control,
    /// Alt (the alternate-symbol modifier on the design keyboard).
    Alt,
    /// The primary (command) modifier key.
    Super,
    Fn,
}

impl Modifier {
    const ALL: [Modifier; 5] = [
        Modifier::Shift,
        Modifier::Control,
        Modifier::Alt,
        Modifier::Super,
        Modifier::Fn,
    ];

    /// The slot of a modifier key (evdev code); caps lock is not one.
    pub fn of_key(code: u32) -> Option<Modifier> {
        Some(match code {
            42 | 54 => Modifier::Shift,
            29 | 97 => Modifier::Control,
            56 | 100 => Modifier::Alt,
            125 | 126 => Modifier::Super,
            464 => Modifier::Fn,
            _ => return None,
        })
    }

    fn index(self) -> usize {
        Self::ALL
            .iter()
            .position(|m| *m == self)
            .unwrap_or_else(|| unreachable!("every modifier is in ALL"))
    }
}

/// State of one modifier slot (the values in parentheses: 1, 8, 2, 4, 0x10).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StickyState {
    Idle,
    /// Down for the first time, not used with another key yet.
    Pressed,
    /// Released after a solo press: stays down for the next key.
    Latched,
    /// Pressed again while latched: stays down until pressed once more.
    Locked,
    /// Held while another key was typed: behaves like a normal modifier.
    Chorded,
}

/// What the caller does with one key event.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StickyOutcome {
    /// Deliver the event itself.
    pub deliver: bool,
    /// Modifier keys (evdev codes) to release after the event.
    pub release: Vec<u32>,
    /// The event toggled the feature: `Some(true)` when it is on now.
    pub toggled: Option<bool>,
}

#[derive(Clone, Copy, Debug)]
struct Slot {
    state: StickyState,
    /// The key that put the slot in its state (left or right), released on its behalf.
    code: u32,
}

/// Sticky-keys filter state.
#[derive(Clone, Debug)]
pub struct StickyKeys {
    enabled: bool,
    /// Five Shift releases toggle the feature.
    shortcut: bool,
    slots: [Slot; 5],
    shift_releases: u32,
    window_start_ms: u64,
    /// Modifier keys physically down (needed to release them when the feature turns on).
    held: BTreeSet<u32>,
}

impl StickyKeys {
    pub fn new(enabled: bool, shortcut: bool) -> StickyKeys {
        StickyKeys {
            enabled,
            shortcut,
            slots: [Slot {
                state: StickyState::Idle,
                code: 0,
            }; 5],
            shift_releases: 0,
            window_start_ms: 0,
            held: BTreeSet::new(),
        }
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn state(&self, m: Modifier) -> StickyState {
        self.slots[m.index()].state
    }

    /// Turn the feature on or off; returns the modifier keys to release (see the module documentation).
    pub fn set_enabled(&mut self, on: bool) -> Vec<u32> {
        if on == self.enabled {
            return Vec::new();
        }
        self.enabled = on;
        if on {
            self.slots = [Slot {
                state: StickyState::Idle,
                code: 0,
            }; 5];
            return self.held.iter().copied().collect();
        }
        let mut out = Vec::new();
        for slot in &mut self.slots {
            if matches!(
                slot.state,
                StickyState::Latched | StickyState::Locked | StickyState::Pressed
            ) {
                out.push(slot.code);
            }
            slot.state = StickyState::Idle;
        }
        out
    }

    /// One key event at `now_ms`.
    pub fn key(&mut self, code: u32, pressed: bool, now_ms: u64) -> StickyOutcome {
        let modifier = Modifier::of_key(code);
        if modifier.is_some() {
            if pressed {
                self.held.insert(code);
            } else {
                self.held.remove(&code);
            }
        }
        let mut out = StickyOutcome {
            deliver: true,
            ..StickyOutcome::default()
        };
        let was_on = self.enabled;
        if modifier == Some(Modifier::Shift) {
            if !pressed {
                if let Some(release) = self.count_shift_release(now_ms) {
                    out.toggled = Some(self.enabled);
                    out.release = release;
                }
            }
        } else {
            self.shift_releases = 0;
        }
        // the toggling event itself passes as a plain event
        if !was_on || out.toggled.is_some() {
            return out;
        }
        match modifier {
            Some(m) => {
                let slot = &mut self.slots[m.index()];
                let (next, deliver) = match (slot.state, pressed) {
                    (StickyState::Idle, true) => (StickyState::Pressed, true),
                    (StickyState::Latched, true) => (StickyState::Locked, true),
                    (StickyState::Locked, true) => (StickyState::Idle, false),
                    (StickyState::Idle, false) => (StickyState::Idle, true),
                    (StickyState::Pressed, false) => (StickyState::Latched, false),
                    (StickyState::Chorded, false) => (StickyState::Idle, true),
                    (StickyState::Locked, false) => (StickyState::Locked, false),
                    // states the design logs as "bad state": the event is dropped
                    (state @ (StickyState::Pressed | StickyState::Chorded), true)
                    | (state @ StickyState::Latched, false) => (state, false),
                };
                if pressed && slot.state == StickyState::Idle {
                    slot.code = code;
                }
                slot.state = next;
                out.deliver = deliver;
            }
            None => {
                for slot in &mut self.slots {
                    if slot.state == StickyState::Pressed {
                        slot.state = StickyState::Chorded;
                    }
                }
                if !pressed {
                    for slot in &mut self.slots {
                        if slot.state == StickyState::Latched {
                            slot.state = StickyState::Idle;
                            out.release.push(slot.code);
                        }
                    }
                }
            }
        }
        out
    }

    /// Count one Shift release; at the fifth within the window the feature toggles and the keys to release are returned.
    fn count_shift_release(&mut self, now_ms: u64) -> Option<Vec<u32>> {
        if !self.shortcut {
            return None;
        }
        if self.shift_releases == 0
            || now_ms.saturating_sub(self.window_start_ms) >= TOGGLE_WINDOW_MS
        {
            self.shift_releases = 0;
            self.window_start_ms = now_ms;
        }
        self.shift_releases += 1;
        if self.shift_releases < TOGGLE_PRESSES {
            return None;
        }
        self.shift_releases = 0;
        let on = !self.enabled;
        Some(self.set_enabled(on))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHIFT: u32 = 42;
    const RSHIFT: u32 = 54;
    const CTRL: u32 = 29;
    const A: u32 = 30;
    const CAPS: u32 = 58;

    fn on() -> StickyKeys {
        StickyKeys::new(true, false)
    }

    fn deliver() -> StickyOutcome {
        StickyOutcome {
            deliver: true,
            ..Default::default()
        }
    }

    fn swallow() -> StickyOutcome {
        StickyOutcome {
            deliver: false,
            ..Default::default()
        }
    }

    #[test]
    fn a_tapped_modifier_latches_for_exactly_one_key() {
        let mut s = on();
        assert_eq!(s.key(SHIFT, true, 0), deliver());
        assert_eq!(
            s.key(SHIFT, false, 50),
            swallow(),
            "the release is held back: Shift stays down"
        );
        assert_eq!(s.state(Modifier::Shift), StickyState::Latched);
        assert_eq!(s.key(A, true, 100), deliver());
        assert_eq!(
            s.key(A, false, 150),
            StickyOutcome {
                deliver: true,
                release: vec![SHIFT],
                toggled: None
            }
        );
        assert_eq!(s.state(Modifier::Shift), StickyState::Idle);
        // the next key is not shifted any more
        assert_eq!(s.key(A, false, 250), deliver());
    }

    #[test]
    fn a_second_tap_locks_and_a_third_unlocks() {
        let mut s = on();
        s.key(SHIFT, true, 0);
        s.key(SHIFT, false, 10);
        assert_eq!(s.key(SHIFT, true, 20), deliver());
        assert_eq!(s.key(SHIFT, false, 30), swallow());
        assert_eq!(s.state(Modifier::Shift), StickyState::Locked);
        // a locked modifier survives typed keys
        s.key(A, true, 40);
        assert_eq!(s.key(A, false, 50), deliver());
        assert_eq!(s.state(Modifier::Shift), StickyState::Locked);
        // pressing it again unlocks: the press vanishes, the release is the modifier's release
        assert_eq!(s.key(SHIFT, true, 60), swallow());
        assert_eq!(s.key(SHIFT, false, 70), deliver());
        assert_eq!(s.state(Modifier::Shift), StickyState::Idle);
    }

    #[test]
    fn a_modifier_held_while_typing_is_an_ordinary_modifier() {
        let mut s = on();
        s.key(CTRL, true, 0);
        assert_eq!(s.key(A, true, 10), deliver());
        assert_eq!(s.state(Modifier::Control), StickyState::Chorded);
        assert_eq!(s.key(A, false, 20), deliver());
        assert_eq!(
            s.key(CTRL, false, 30),
            deliver(),
            "released like a normal modifier"
        );
        assert_eq!(s.state(Modifier::Control), StickyState::Idle);
    }

    #[test]
    fn several_modifiers_latch_together_and_the_right_key_is_released() {
        let mut s = on();
        s.key(CTRL, true, 0);
        s.key(CTRL, false, 5);
        s.key(RSHIFT, true, 10);
        s.key(RSHIFT, false, 15);
        s.key(A, true, 20);
        assert_eq!(s.key(A, false, 25).release, vec![RSHIFT, CTRL]);
    }

    #[test]
    fn caps_lock_is_an_ordinary_key() {
        let mut s = on();
        s.key(SHIFT, true, 0);
        s.key(SHIFT, false, 5);
        assert_eq!(s.key(CAPS, true, 10), deliver());
        assert_eq!(s.key(CAPS, false, 20).release, vec![SHIFT]);
    }

    #[test]
    fn five_shift_releases_within_30_s_toggle_when_the_shortcut_is_on() {
        let mut s = StickyKeys::new(false, true);
        for t in 0..4 {
            s.key(SHIFT, true, t * 1000);
            assert_eq!(s.key(SHIFT, false, t * 1000 + 100).toggled, None);
        }
        s.key(SHIFT, true, 4000);
        assert_eq!(
            s.key(SHIFT, false, 4100),
            StickyOutcome {
                deliver: true,
                release: vec![],
                toggled: Some(true)
            }
        );
        assert!(s.enabled());
        // the toggle event passed plainly; taps now walk latched, locked, idle, latched (and still count)
        fn tap(s: &mut StickyKeys, t: u64) -> (StickyOutcome, StickyState) {
            s.key(SHIFT, true, t);
            let out = s.key(SHIFT, false, t + 50);
            (out, s.state(Modifier::Shift))
        }
        assert_eq!(tap(&mut s, 5000).1, StickyState::Latched);
        assert_eq!(tap(&mut s, 5100).1, StickyState::Locked);
        assert_eq!(tap(&mut s, 5200).1, StickyState::Idle);
        assert_eq!(tap(&mut s, 5300).1, StickyState::Latched);
        // the fifth turns it off and releases the Shift that the press locked
        let (out, state) = tap(&mut s, 5400);
        assert_eq!(
            out,
            StickyOutcome {
                deliver: true,
                release: vec![SHIFT],
                toggled: Some(false)
            }
        );
        assert_eq!(state, StickyState::Idle);
        assert!(!s.enabled());
    }

    #[test]
    fn the_toggle_count_resets_on_other_keys_and_after_the_window() {
        let mut s = StickyKeys::new(false, true);
        for t in 0..4 {
            s.key(SHIFT, true, t);
            s.key(SHIFT, false, t);
        }
        s.key(A, true, 10);
        s.key(SHIFT, true, 20);
        assert_eq!(
            s.key(SHIFT, false, 20).toggled,
            None,
            "a typed key reset the count"
        );
        // four more, but the window started 30 s before the last one
        for t in [21, 22, 23] {
            s.key(SHIFT, true, t);
            s.key(SHIFT, false, t);
        }
        s.key(SHIFT, true, 30_020);
        assert_eq!(s.key(SHIFT, false, 30_020).toggled, None);
        assert!(!s.enabled());
        // without the shortcut nothing counts
        let mut quiet = StickyKeys::new(false, false);
        for t in 0..10 {
            quiet.key(SHIFT, true, t);
            quiet.key(SHIFT, false, t);
        }
        assert!(!quiet.enabled());
    }

    #[test]
    fn turning_on_releases_held_modifiers_and_turning_off_releases_stuck_ones() {
        let mut s = StickyKeys::new(false, false);
        s.key(CTRL, true, 0);
        assert_eq!(s.set_enabled(true), vec![CTRL]);
        // the physical release of Ctrl is then a plain release
        assert_eq!(s.key(CTRL, false, 10), deliver());
        s.key(SHIFT, true, 20);
        s.key(SHIFT, false, 30);
        assert_eq!(s.set_enabled(false), vec![SHIFT]);
        assert_eq!(s.state(Modifier::Shift), StickyState::Idle);
        assert_eq!(s.key(A, true, 40), deliver());
    }
}
