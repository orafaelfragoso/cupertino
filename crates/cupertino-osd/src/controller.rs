//! What a command asks of the levels and what the overlay then shows: the sender's rules, as pure functions of the
//! levels read before the press. [`crate::effects`] reads and writes the levels and calls these in between.

use std::fmt;

use crate::model::{InvalidLevel, Kind, Overlay};
use crate::sender::step_level;

/// What a sender asks of the overlay.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Command {
    /// Raise (`direction` > 0) or lower the volume one press.
    VolumeStep {
        /// The sign gives the direction.
        direction: i32,
        /// A fine press (Shift and Alt held).
        fine: bool,
    },
    /// Mute, or unmute when muted.
    ToggleMute,
    /// Mute the microphone, or unmute it when muted.
    ToggleMicrophoneMute,
    /// Raise or lower the display brightness one press.
    BrightnessStep {
        /// The sign gives the direction.
        direction: i32,
        /// A fine press.
        fine: bool,
    },
    /// Raise or lower the keyboard backlight one press.
    KeyboardBrightnessStep {
        /// The sign gives the direction.
        direction: i32,
        /// A fine press.
        fine: bool,
    },
    /// Show a level somebody else changed (the control panel).
    Show(Overlay),
}

impl Command {
    /// What the command is called in a log line.
    pub fn name(&self) -> &'static str {
        match self {
            Command::VolumeStep { .. } => "volume step",
            Command::ToggleMute => "mute toggle",
            Command::ToggleMicrophoneMute => "microphone mute toggle",
            Command::BrightnessStep { .. } => "brightness step",
            Command::KeyboardBrightnessStep { .. } => "keyboard brightness step",
            Command::Show(_) => "show",
        }
    }

    /// Whether the command changes a backlight, through the system bus.
    pub fn backlight(&self) -> bool {
        matches!(
            self,
            Command::BrightnessStep { .. } | Command::KeyboardBrightnessStep { .. }
        )
    }
}

/// A volume, output or input.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VolumeState {
    /// Level, 0 to 1.
    pub level: f64,
    /// Whether it is muted.
    pub muted: bool,
}

/// What a command did: the overlay to show and whether it changed a value, which sets the hold.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shown {
    pub overlay: Overlay,
    pub changed: bool,
}

/// A command whose effect failed; nothing was shown.
#[derive(Clone, Debug, PartialEq)]
pub struct EffectError {
    pub command: &'static str,
    pub reason: String,
}

impl fmt::Display for EffectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} failed, nothing shown: {}", self.command, self.reason)
    }
}

impl std::error::Error for EffectError {}

/// The volume one volume key press asks for: a new level of 0 or less mutes, and a new level above 0 unmutes.
pub fn volume_step(before: VolumeState, direction: i32, fine: bool) -> VolumeState {
    let level = step_level(before.level, direction, fine);
    VolumeState {
        level,
        muted: level <= 0.0,
    }
}

/// The volume the mute key asks for: it toggles, but at volume 0 the result is always muted.
pub fn mute_toggle(before: VolumeState) -> VolumeState {
    VolumeState {
        level: before.level,
        muted: before.level <= 0.0 || !before.muted,
    }
}

/// The input volume the microphone mute key asks for: a plain toggle.
pub fn microphone_toggle(before: VolumeState) -> VolumeState {
    VolumeState {
        level: before.level,
        muted: !before.muted,
    }
}

/// The overlay of an output volume: muted shows the mute glyph with an empty meter.
pub fn volume_overlay(state: VolumeState) -> Result<Overlay, InvalidLevel> {
    if state.muted {
        Overlay::new(Kind::Mute, 0.0)
    } else {
        Overlay::new(Kind::Volume, state.level)
    }
}

/// The overlay of an input volume: muted shows the slashed microphone with an empty meter.
pub fn microphone_overlay(state: VolumeState) -> Result<Overlay, InvalidLevel> {
    if state.muted {
        Overlay::new(Kind::MicrophoneMute, 0.0)
    } else {
        Overlay::new(Kind::Microphone, state.level)
    }
}

/// The raw value for `level` (a fraction of `max`) within `floor..=max`. A change of level always moves at least one
/// raw unit from `current`, in the direction of `level`, unless that end is reached.
pub fn linear_raw(current: u32, max: u32, level: f64, floor: u32) -> u32 {
    let exact = level * f64::from(max);
    let wanted = exact.round().clamp(f64::from(floor), f64::from(max)) as u32;
    if wanted != current {
        return wanted;
    }
    if exact > f64::from(current) {
        (current + 1).min(max)
    } else if exact < f64::from(current) {
        current.saturating_sub(1).max(floor)
    } else {
        current
    }
}

/// The raw value one backlight key press asks for, from `current` of `max` and no lower than `floor`; `None` when
/// the press asks for the level it has.
pub fn backlight_step(
    current: u32,
    max: u32,
    floor: u32,
    direction: i32,
    fine: bool,
) -> Option<u32> {
    let before = f64::from(current) / f64::from(max);
    let target = step_level(before, direction, fine);
    (target != before).then(|| linear_raw(current, max, target, floor))
}
