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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::{MemoryBacklight, MemoryVolume};

    struct Absent;

    impl Backlight for Absent {
        fn level(&mut self) -> Result<f64, BackendError> {
            Err("no backlight device".into())
        }

        fn set_level(&mut self, _: f64) -> Result<f64, BackendError> {
            Err("no backlight device".into())
        }
    }

    fn controller_with(volume: MemoryVolume, display: MemoryBacklight) -> Controller {
        Controller::new(
            Box::new(volume),
            Box::new(MemoryVolume::new(0.75, false)),
            Box::new(display),
            Box::new(Absent),
        )
    }

    fn controller() -> Controller {
        controller_with(MemoryVolume::new(0.5, false), MemoryBacklight::new(0.5))
    }

    fn shown(controller: &Controller, now: f64) -> (Kind, f64) {
        let overlay = controller.view(now).expect("an overlay is shown").overlay;
        (overlay.kind(), overlay.level())
    }

    #[test]
    fn a_coarse_press_moves_a_sixteenth_and_shows_it() {
        let mut controller = controller();
        controller
            .apply(
                5.0,
                Command::VolumeStep {
                    direction: 1,
                    fine: false,
                },
            )
            .unwrap();
        assert_eq!(shown(&controller, 5.0), (Kind::Volume, 0.5625));
        controller
            .apply(
                5.1,
                Command::VolumeStep {
                    direction: -1,
                    fine: false,
                },
            )
            .unwrap();
        controller
            .apply(
                5.2,
                Command::VolumeStep {
                    direction: -1,
                    fine: false,
                },
            )
            .unwrap();
        assert_eq!(shown(&controller, 5.2), (Kind::Volume, 0.4375));
    }

    #[test]
    fn a_fine_press_moves_a_sixty_fourth() {
        let mut controller = controller();
        controller
            .apply(
                5.0,
                Command::BrightnessStep {
                    direction: 1,
                    fine: true,
                },
            )
            .unwrap();
        assert_eq!(shown(&controller, 5.0), (Kind::Brightness, 0.515625));
    }

    #[test]
    fn a_press_snaps_an_off_grid_level_first() {
        let mut controller =
            controller_with(MemoryVolume::new(0.30, false), MemoryBacklight::new(0.30));
        controller
            .apply(
                0.0,
                Command::VolumeStep {
                    direction: 1,
                    fine: false,
                },
            )
            .unwrap();
        assert_eq!(shown(&controller, 0.0), (Kind::Volume, 0.3125));
        controller
            .apply(
                0.0,
                Command::BrightnessStep {
                    direction: -1,
                    fine: false,
                },
            )
            .unwrap();
        assert_eq!(shown(&controller, 0.0), (Kind::Brightness, 0.25));
    }

    #[test]
    fn brightness_presses_clamp_at_zero_and_one() {
        let mut controller =
            controller_with(MemoryVolume::new(0.99, false), MemoryBacklight::new(0.01));
        controller
            .apply(
                0.0,
                Command::VolumeStep {
                    direction: 1,
                    fine: false,
                },
            )
            .unwrap();
        assert_eq!(shown(&controller, 0.0), (Kind::Volume, 1.0));
        controller
            .apply(
                0.0,
                Command::BrightnessStep {
                    direction: -1,
                    fine: false,
                },
            )
            .unwrap();
        assert_eq!(shown(&controller, 0.0), (Kind::Brightness, 0.0));
        controller
            .apply(
                0.0,
                Command::BrightnessStep {
                    direction: -1,
                    fine: true,
                },
            )
            .unwrap();
        assert_eq!(shown(&controller, 0.0), (Kind::Brightness, 0.0));
    }

    #[test]
    fn lowering_the_volume_to_zero_mutes_and_shows_the_mute_glyph() {
        let mut controller =
            controller_with(MemoryVolume::new(0.0625, false), MemoryBacklight::new(0.5));
        controller
            .apply(
                1.0,
                Command::VolumeStep {
                    direction: -1,
                    fine: false,
                },
            )
            .unwrap();
        assert_eq!(shown(&controller, 1.0), (Kind::Mute, 0.0));
    }

    #[test]
    fn raising_the_volume_from_muted_at_zero_unmutes() {
        let mut controller =
            controller_with(MemoryVolume::new(0.0, true), MemoryBacklight::new(0.5));
        controller
            .apply(
                1.0,
                Command::VolumeStep {
                    direction: 1,
                    fine: false,
                },
            )
            .unwrap();
        assert_eq!(shown(&controller, 1.0), (Kind::Volume, 0.0625));
    }

    #[test]
    fn the_mute_key_toggles_but_at_volume_zero_stays_muted() {
        let mut controller = controller();
        controller.apply(1.0, Command::ToggleMute).unwrap();
        assert_eq!(shown(&controller, 1.0), (Kind::Mute, 0.0));
        controller.apply(2.0, Command::ToggleMute).unwrap();
        assert_eq!(shown(&controller, 2.0), (Kind::Volume, 0.5));

        let mut silent = controller_with(MemoryVolume::new(0.0, true), MemoryBacklight::new(0.5));
        silent.apply(1.0, Command::ToggleMute).unwrap();
        assert_eq!(shown(&silent, 1.0), (Kind::Mute, 0.0));
        let mut silent_unmuted =
            controller_with(MemoryVolume::new(0.0, false), MemoryBacklight::new(0.5));
        silent_unmuted.apply(1.0, Command::ToggleMute).unwrap();
        assert_eq!(shown(&silent_unmuted, 1.0), (Kind::Mute, 0.0));
    }

    #[test]
    fn the_microphone_key_toggles_its_mute_and_leaves_the_output_alone() {
        let mut controller = controller();
        controller
            .apply(1.0, Command::ToggleMicrophoneMute)
            .unwrap();
        assert_eq!(shown(&controller, 1.0), (Kind::MicrophoneMute, 0.0));
        controller
            .apply(2.0, Command::ToggleMicrophoneMute)
            .unwrap();
        assert_eq!(shown(&controller, 2.0), (Kind::Microphone, 0.75));
        controller.apply(3.0, Command::ToggleMute).unwrap();
        assert_eq!(shown(&controller, 3.0), (Kind::Mute, 0.0));
    }

    #[test]
    fn a_press_that_changed_the_level_holds_four_seconds_then_fades() {
        let mut controller = controller();
        controller
            .apply(
                10.0,
                Command::VolumeStep {
                    direction: 1,
                    fine: false,
                },
            )
            .unwrap();
        assert_eq!(controller.until_fade(10.0), Some(4.0));
        assert_eq!(controller.phase(13.999), Phase::Holding);
        assert_eq!(controller.phase(14.0), Phase::Fading);
        assert_eq!(controller.phase(14.7), Phase::Hidden);
    }

    #[test]
    fn a_press_that_changed_nothing_holds_one_second() {
        let mut controller =
            controller_with(MemoryVolume::new(1.0, false), MemoryBacklight::new(0.5));
        controller
            .apply(
                10.0,
                Command::VolumeStep {
                    direction: 1,
                    fine: false,
                },
            )
            .unwrap();
        assert_eq!(shown(&controller, 10.0), (Kind::Volume, 1.0));
        assert_eq!(controller.until_fade(10.0), Some(1.0));
        assert_eq!(controller.phase(11.0), Phase::Fading);
        assert_eq!(controller.phase(11.7), Phase::Hidden);
    }

    #[test]
    fn a_show_from_the_control_panel_holds_four_seconds() {
        let mut controller = controller();
        controller
            .apply(
                1.0,
                Command::Show(Overlay::new(Kind::Brightness, 0.25).unwrap()),
            )
            .unwrap();
        assert_eq!(shown(&controller, 1.0), (Kind::Brightness, 0.25));
        assert_eq!(controller.until_fade(1.0), Some(4.0));
    }

    #[test]
    fn a_failing_effect_shows_nothing_and_returns_the_reason() {
        let mut controller = controller();
        let error = controller
            .apply(
                1.0,
                Command::KeyboardBrightnessStep {
                    direction: 1,
                    fine: false,
                },
            )
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "keyboard brightness step failed, nothing shown: no backlight device"
        );
        assert_eq!(controller.view(1.0), None);
    }

    #[test]
    fn a_failing_effect_leaves_the_overlay_on_screen_unchanged() {
        let mut controller = controller();
        controller
            .apply(
                1.0,
                Command::VolumeStep {
                    direction: 1,
                    fine: false,
                },
            )
            .unwrap();
        controller
            .apply(
                1.2,
                Command::KeyboardBrightnessStep {
                    direction: 1,
                    fine: false,
                },
            )
            .unwrap_err();
        assert_eq!(shown(&controller, 1.2), (Kind::Volume, 0.5625));
        // the hold still runs from the first press: it started at 1.0 and lasts 4 s
        assert_eq!(controller.phase(4.9), Phase::Holding);
        assert_eq!(controller.phase(5.1), Phase::Fading);
    }
}
