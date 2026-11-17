//! Carries out a command (change a level, then show it) on the backends and the model.

use std::fmt;

use crate::backend::{BackendError, Backlight, Volume, VolumeState};
use crate::model::{Hud, Kind, Overlay, Phase, View};
use crate::sender::{hold_seconds, step_level};

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
    fn name(&self) -> &'static str {
        match self {
            Command::VolumeStep { .. } => "volume step",
            Command::ToggleMute => "mute toggle",
            Command::ToggleMicrophoneMute => "microphone mute toggle",
            Command::BrightnessStep { .. } => "brightness step",
            Command::KeyboardBrightnessStep { .. } => "keyboard brightness step",
            Command::Show(_) => "show",
        }
    }
}

/// A command whose effect failed; nothing was shown.
#[derive(Debug)]
pub struct EffectError {
    command: &'static str,
    source: BackendError,
}

impl fmt::Display for EffectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} failed, nothing shown: {}", self.command, self.source)
    }
}

impl std::error::Error for EffectError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.source.as_ref())
    }
}

/// The overlay model together with the things it changes.
pub struct Controller {
    hud: Hud,
    volume: Box<dyn Volume>,
    microphone: Box<dyn Volume>,
    display: Box<dyn Backlight>,
    keyboard: Box<dyn Backlight>,
}

/// What a command did: the overlay to show and whether it changed a value, which sets the hold.
struct Effect {
    overlay: Overlay,
    changed: bool,
}

/// The overlay of a volume after a change: muted shows the mute glyph with an empty meter.
fn volume_overlay(state: VolumeState) -> Result<Overlay, BackendError> {
    Ok(if state.muted {
        Overlay::new(Kind::Mute, 0.0)?
    } else {
        Overlay::new(Kind::Volume, state.level)?
    })
}

/// One volume key press: a new level of 0 or less mutes, and a new level above 0 unmutes.
fn step_volume(
    volume: &mut dyn Volume,
    direction: i32,
    fine: bool,
) -> Result<Effect, BackendError> {
    let before = volume.state()?;
    let target = step_level(before.level, direction, fine);
    let level = if target == before.level {
        target
    } else {
        volume.set_level(target)?
    };
    let muted = target <= 0.0;
    if muted != before.muted {
        volume.set_muted(muted)?;
    }
    Ok(Effect {
        overlay: volume_overlay(VolumeState { level, muted })?,
        changed: level != before.level || muted != before.muted,
    })
}

/// The mute key: toggles, but at volume 0 the result is always muted.
fn toggle_mute(volume: &mut dyn Volume) -> Result<Effect, BackendError> {
    let before = volume.state()?;
    let muted = before.level <= 0.0 || !before.muted;
    if muted != before.muted {
        volume.set_muted(muted)?;
    }
    Ok(Effect {
        overlay: volume_overlay(VolumeState {
            level: before.level,
            muted,
        })?,
        changed: muted != before.muted,
    })
}

/// The microphone mute key: a plain toggle; muted shows the slashed microphone with an empty meter.
fn toggle_microphone(microphone: &mut dyn Volume) -> Result<Effect, BackendError> {
    let before = microphone.state()?;
    let muted = !before.muted;
    microphone.set_muted(muted)?;
    Ok(Effect {
        overlay: if muted {
            Overlay::new(Kind::MicrophoneMute, 0.0)?
        } else {
            Overlay::new(Kind::Microphone, before.level)?
        },
        changed: true,
    })
}

/// One backlight key press.
fn step_backlight(
    backlight: &mut dyn Backlight,
    kind: Kind,
    direction: i32,
    fine: bool,
) -> Result<Effect, BackendError> {
    let before = backlight.level()?;
    let target = step_level(before, direction, fine);
    let level = if target == before {
        target
    } else {
        backlight.set_level(target)?
    };
    Ok(Effect {
        overlay: Overlay::new(kind, level)?,
        changed: level != before,
    })
}

impl Controller {
    /// A controller over the four backends.
    pub fn new(
        volume: Box<dyn Volume>,
        microphone: Box<dyn Volume>,
        display: Box<dyn Backlight>,
        keyboard: Box<dyn Backlight>,
    ) -> Self {
        Self {
            hud: Hud::new(),
            volume,
            microphone,
            display,
            keyboard,
        }
    }

    /// Carry out `command` at `now` (seconds on the caller's monotonic clock). On success the overlay shows the new
    /// level and its hold restarts, 4 s after a press that changed a value and 1 s after one that changed nothing; on
    /// failure the overlay is unchanged and the reason is returned.
    pub fn apply(&mut self, now: f64, command: Command) -> Result<(), EffectError> {
        let effect = match command {
            Command::VolumeStep { direction, fine } => {
                step_volume(self.volume.as_mut(), direction, fine)
            }
            Command::ToggleMute => toggle_mute(self.volume.as_mut()),
            Command::ToggleMicrophoneMute => toggle_microphone(self.microphone.as_mut()),
            Command::BrightnessStep { direction, fine } => {
                step_backlight(self.display.as_mut(), Kind::Brightness, direction, fine)
            }
            Command::KeyboardBrightnessStep { direction, fine } => step_backlight(
                self.keyboard.as_mut(),
                Kind::KeyboardBrightness,
                direction,
                fine,
            ),
            // the control panel changed the level itself, so the changed-value hold applies
            Command::Show(overlay) => Ok(Effect {
                overlay,
                changed: true,
            }),
        }
        .map_err(|source| EffectError {
            command: command.name(),
            source,
        })?;
        self.hud
            .show(now, effect.overlay, hold_seconds(effect.changed));
        Ok(())
    }

    /// What to draw at `now`.
    pub fn view(&self, now: f64) -> Option<View> {
        self.hud.view(now)
    }

    /// The lifetime phase at `now`.
    pub fn phase(&self, now: f64) -> Phase {
        self.hud.phase(now)
    }

    /// Seconds until the fade starts; `None` unless holding.
    pub fn until_fade(&self, now: f64) -> Option<f64> {
        self.hud.until_fade(now)
    }
}
