//! What the commands change, through the services that own the levels: the output and input volumes through
//! `org.cupertino.Audio1` of `cupertino-audiod` on the session bus, the display backlight through
//! `org.cupertino.Power1` and the keyboard backlight through `org.freedesktop.UPower.KbdBacklight` on the system bus.
//! Every call is made with zbus's async API, in a future the component's UI loop polls: nothing waits for a service
//! on the UI thread.

use zbus::Connection;

use crate::controller::{
    backlight_step, microphone_overlay, microphone_toggle, mute_toggle, volume_overlay,
    volume_step, Command, EffectError, Shown, VolumeState,
};
use crate::model::{Kind, Overlay};
use crate::sender::DISPLAY_FLOOR_RAW;

/// The sound service: its name, object and interface (`docs/architecture/contracts.yaml`).
const AUDIO: Service = Service {
    name: "org.cupertino.Audio1",
    path: "/org/cupertino/Audio1",
    interface: "org.cupertino.Audio1",
};
/// The direction `Audio1` calls the default sink, and the default source.
const OUTPUT: &str = "output";
const INPUT: &str = "input";

/// The power daemon's display backlight.
const DISPLAY: Service = Service {
    name: "org.cupertino.Power1",
    path: "/org/cupertino/Power1",
    interface: "org.cupertino.Power1",
};
/// UPower's keyboard backlight.
const KEYBOARD: Service = Service {
    name: "org.freedesktop.UPower",
    path: "/org/freedesktop/UPower/KbdBacklight",
    interface: "org.freedesktop.UPower.KbdBacklight",
};

/// Why a call failed.
type Failure = Box<dyn std::error::Error + Send + Sync>;

/// A bus object and its interface.
#[derive(Clone, Copy)]
struct Service {
    name: &'static str,
    path: &'static str,
    interface: &'static str,
}

impl Service {
    /// Calls `method` with `body` on `connection` and reads the answer as `T`.
    async fn call<B, T>(self, connection: &Connection, method: &str, body: &B) -> zbus::Result<T>
    where
        B: serde::Serialize + zbus::zvariant::DynamicType,
        T: for<'d> serde::Deserialize<'d> + zbus::zvariant::Type,
    {
        connection
            .call_method(
                Some(self.name),
                self.path,
                Some(self.interface),
                method,
                body,
            )
            .await?
            .body()
            .deserialize()
    }
}

/// The connections the effects go through: the component's session bus connection and its system bus connection,
/// each `None` while there is none.
#[derive(Clone, Debug, Default)]
pub struct Buses {
    pub session: Option<Connection>,
    pub system: Option<Connection>,
}

/// Carries out `command` and says what to show; on failure nothing changed that the overlay would show.
pub async fn apply(command: Command, buses: Buses) -> Result<Shown, EffectError> {
    let fail = |reason: Failure| EffectError {
        command: command.name(),
        reason: reason.to_string(),
    };
    let session = || buses.session.as_ref().ok_or("no session bus connection");
    let system = || buses.system.as_ref().ok_or("no system bus connection");
    match command {
        Command::VolumeStep { direction, fine } => async {
            let audio = session()?;
            let before = volume(audio, OUTPUT).await?;
            let target = volume_step(before, direction, fine);
            let reached = set_volume(audio, OUTPUT, before, target).await?;
            Ok(Shown {
                overlay: volume_overlay(reached)?,
                changed: reached != before,
            })
        }
        .await
        .map_err(fail),
        Command::ToggleMute => async {
            let audio = session()?;
            let before = volume(audio, OUTPUT).await?;
            let reached = set_volume(audio, OUTPUT, before, mute_toggle(before)).await?;
            Ok(Shown {
                overlay: volume_overlay(reached)?,
                changed: reached.muted != before.muted,
            })
        }
        .await
        .map_err(fail),
        Command::ToggleMicrophoneMute => async {
            let audio = session()?;
            let before = volume(audio, INPUT).await?;
            let reached = set_volume(audio, INPUT, before, microphone_toggle(before)).await?;
            Ok(Shown {
                overlay: microphone_overlay(reached)?,
                changed: true,
            })
        }
        .await
        .map_err(fail),
        Command::BrightnessStep { direction, fine } => {
            display_step(system().map_err(|e| fail(e.into()))?, direction, fine)
                .await
                .map_err(fail)
        }
        Command::KeyboardBrightnessStep { direction, fine } => {
            keyboard_step(system().map_err(|e| fail(e.into()))?, direction, fine)
                .await
                .map_err(fail)
        }
        // the control panel changed the level itself, so the changed-value hold applies
        Command::Show(overlay) => Ok(Shown {
            overlay,
            changed: true,
        }),
    }
}

/// The volume of `direction` now.
async fn volume(audio: &Connection, direction: &str) -> Result<VolumeState, Failure> {
    let (level, muted): (f64, bool) = AUDIO.call(audio, "GetVolume", &(direction,)).await?;
    Ok(VolumeState { level, muted })
}

/// Sets the volume of `direction` to `target` unless it is `before`, and gives the volume reached.
async fn set_volume(
    audio: &Connection,
    direction: &str,
    before: VolumeState,
    target: VolumeState,
) -> Result<VolumeState, Failure> {
    if target == before {
        return Ok(before);
    }
    let (level, muted): (f64, bool) = AUDIO
        .call(audio, "SetVolume", &(direction, target.level, target.muted))
        .await?;
    Ok(VolumeState { level, muted })
}

/// One display brightness press, in the daemon's raw units mapped linearly onto 0 to 1 (no fixed curve is known);
/// a level never reaches below raw [`DISPLAY_FLOOR_RAW`].
async fn display_step(system: &Connection, direction: i32, fine: bool) -> Result<Shown, Failure> {
    let current: u32 = DISPLAY.call(system, "GetBrightness", &()).await?;
    let max: u32 = DISPLAY.call(system, "GetMaxBrightness", &()).await?;
    if max == 0 {
        return Err("the display backlight reports a maximum of 0".into());
    }
    let reached = match backlight_step(current, max, DISPLAY_FLOOR_RAW.min(max), direction, fine) {
        Some(target) if target != current => {
            DISPLAY
                .call::<_, ()>(system, "SetBrightness", &(target,))
                .await?;
            target
        }
        _ => current,
    };
    backlight_shown(Kind::Brightness, current, reached, max)
}

/// One keyboard backlight press, in the daemon's raw units mapped linearly onto 0 to 1; it reaches 0.
async fn keyboard_step(system: &Connection, direction: i32, fine: bool) -> Result<Shown, Failure> {
    let current: i32 = KEYBOARD.call(system, "GetBrightness", &()).await?;
    let max: i32 = KEYBOARD.call(system, "GetMaxBrightness", &()).await?;
    let (Ok(current), Ok(max)) = (u32::try_from(current), u32::try_from(max)) else {
        return Err(format!("the keyboard backlight reports {current} of {max}").into());
    };
    if max == 0 {
        return Err("the keyboard backlight reports a maximum of 0".into());
    }
    let reached = match backlight_step(current, max, 0, direction, fine) {
        Some(target) if target != current => {
            let raw = i32::try_from(target)?;
            KEYBOARD
                .call::<_, ()>(system, "SetBrightness", &(raw,))
                .await?;
            target
        }
        _ => current,
    };
    backlight_shown(Kind::KeyboardBrightness, current, reached, max)
}

/// What a backlight press shows: the level reached, and whether it moved.
fn backlight_shown(kind: Kind, before: u32, reached: u32, max: u32) -> Result<Shown, Failure> {
    Ok(Shown {
        overlay: Overlay::new(kind, f64::from(reached) / f64::from(max))?,
        changed: reached != before,
    })
}
