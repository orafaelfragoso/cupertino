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
