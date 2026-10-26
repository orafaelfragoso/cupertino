//! What the panel reads and changes, behind traits so the controller and the tests run on in-memory stand-ins and the
//! process can run without touching the machine (`--fake-backends`).
//!
//! The real implementations over the system bus are in `system` (feature `dbus`); the volume is here, over the kernel's
//! sound control interface through `cupertino-alsa`.

use std::error::Error;
use std::fmt;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use cupertino_alsa::{Master, OpenMode};

use crate::model::{Battery, VolumeState, WifiStatus};

/// Why an effect failed; it disables the tile and goes to the log.
pub type BackendError = Box<dyn Error + Send + Sync>;

/// The Wi-Fi radio.
pub trait Wifi: Send {
    fn read(&mut self) -> Result<WifiStatus, BackendError>;
    /// Switch the radio on or off. The radio may take a moment to follow.
    fn set_enabled(&mut self, enabled: bool) -> Result<(), BackendError>;
}

/// The Bluetooth adapter.
pub trait Bluetooth: Send {
    /// Whether the adapter is powered.
    fn read(&mut self) -> Result<bool, BackendError>;
    fn set_powered(&mut self, powered: bool) -> Result<(), BackendError>;
}

/// Focus mode (do not disturb).
pub trait FocusMode: Send {
    fn read(&mut self) -> Result<bool, BackendError>;
    fn set(&mut self, on: bool) -> Result<(), BackendError>;
}

/// Display brightness.
pub trait Brightness: Send {
    /// The level, 0 to 1.
    fn read(&mut self) -> Result<f64, BackendError>;
    fn set(&mut self, level: f64) -> Result<(), BackendError>;
}

/// The output volume.
pub trait Volume: Send {
    fn read(&mut self) -> Result<VolumeState, BackendError>;
    /// Set the level and the mute state together; returns what the output has after the change.
    fn set(&mut self, level: f64, muted: bool) -> Result<VolumeState, BackendError>;
}

/// The battery.
pub trait BatteryReader: Send {
    /// `None` on a machine without a battery.
    fn read(&mut self) -> Result<Option<Battery>, BackendError>;
}

/// The overlay the panel tells about a level it changed itself.
pub trait OverlayNotifier: Send {
    fn show(&mut self, kind: OverlayKind, level: f64) -> Result<(), BackendError>;
}

/// What the overlay shows (the `Show` kinds of its bus interface).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OverlayKind {
    Volume,
    Mute,
    Brightness,
}

impl OverlayKind {
    /// The name on the bus.
    pub fn name(self) -> &'static str {
        match self {
            OverlayKind::Volume => "volume",
            OverlayKind::Mute => "mute",
            OverlayKind::Brightness => "brightness",
        }
    }
}

/// A backend that always fails with one reason: a service that does not exist, or a stand-in for a broken one.
#[derive(Clone, Debug)]
pub struct Unavailable {
    reason: String,
}

impl Unavailable {
    pub fn new(reason: &str) -> Self {
        Self {
            reason: reason.to_string(),
        }
    }

    fn fail<T>(&self) -> Result<T, BackendError> {
        Err(self.reason.clone().into())
    }
}

impl Wifi for Unavailable {
    fn read(&mut self) -> Result<WifiStatus, BackendError> {
        self.fail()
    }
    fn set_enabled(&mut self, _: bool) -> Result<(), BackendError> {
        self.fail()
    }
}

impl Bluetooth for Unavailable {
    fn read(&mut self) -> Result<bool, BackendError> {
        self.fail()
    }
    fn set_powered(&mut self, _: bool) -> Result<(), BackendError> {
        self.fail()
    }
}

impl FocusMode for Unavailable {
    fn read(&mut self) -> Result<bool, BackendError> {
        self.fail()
    }
    fn set(&mut self, _: bool) -> Result<(), BackendError> {
        self.fail()
    }
}

impl Brightness for Unavailable {
    fn read(&mut self) -> Result<f64, BackendError> {
        self.fail()
    }
    fn set(&mut self, _: f64) -> Result<(), BackendError> {
        self.fail()
    }
}

impl Volume for Unavailable {
    fn read(&mut self) -> Result<VolumeState, BackendError> {
        self.fail()
    }
    fn set(&mut self, _: f64, _: bool) -> Result<VolumeState, BackendError> {
        self.fail()
    }
}

impl BatteryReader for Unavailable {
    fn read(&mut self) -> Result<Option<Battery>, BackendError> {
        self.fail()
    }
}

impl OverlayNotifier for Unavailable {
    fn show(&mut self, _: OverlayKind, _: f64) -> Result<(), BackendError> {
        self.fail()
    }
}

/// An in-memory Wi-Fi radio.
#[derive(Debug)]
pub struct MemoryWifi(pub WifiStatus);

impl Wifi for MemoryWifi {
    fn read(&mut self) -> Result<WifiStatus, BackendError> {
        Ok(self.0.clone())
    }
    fn set_enabled(&mut self, enabled: bool) -> Result<(), BackendError> {
        self.0 = WifiStatus {
            enabled,
            network: self.0.network.clone().filter(|_| enabled),
        };
        Ok(())
    }
}

/// An in-memory Bluetooth adapter.
#[derive(Debug)]
pub struct MemoryBluetooth(pub bool);

impl Bluetooth for MemoryBluetooth {
    fn read(&mut self) -> Result<bool, BackendError> {
        Ok(self.0)
    }
    fn set_powered(&mut self, powered: bool) -> Result<(), BackendError> {
        self.0 = powered;
        Ok(())
    }
}

/// An in-memory focus mode switch.
#[derive(Debug)]
pub struct MemoryFocusMode(pub bool);

impl FocusMode for MemoryFocusMode {
    fn read(&mut self) -> Result<bool, BackendError> {
        Ok(self.0)
    }
    fn set(&mut self, on: bool) -> Result<(), BackendError> {
        self.0 = on;
        Ok(())
    }
}

/// An in-memory display brightness.
#[derive(Debug)]
pub struct MemoryBrightness(pub f64);

impl Brightness for MemoryBrightness {
    fn read(&mut self) -> Result<f64, BackendError> {
        Ok(self.0)
    }
    fn set(&mut self, level: f64) -> Result<(), BackendError> {
        self.0 = level;
        Ok(())
    }
}

/// An in-memory volume.
#[derive(Debug)]
pub struct MemoryVolume(pub VolumeState);

impl Volume for MemoryVolume {
    fn read(&mut self) -> Result<VolumeState, BackendError> {
        Ok(self.0)
    }
    fn set(&mut self, level: f64, muted: bool) -> Result<VolumeState, BackendError> {
        self.0 = VolumeState { level, muted };
        Ok(self.0)
    }
}

/// An in-memory battery.
#[derive(Debug)]
pub struct MemoryBattery(pub Option<Battery>);

impl BatteryReader for MemoryBattery {
    fn read(&mut self) -> Result<Option<Battery>, BackendError> {
        Ok(self.0)
    }
}

/// An overlay stand-in that records what it was told; the handle keeps reading after the notifier moved into a controller.
#[derive(Clone, Debug, Default)]
pub struct RecordingOverlay {
    shown: Arc<Mutex<Vec<(OverlayKind, f64)>>>,
}

impl RecordingOverlay {
    /// Everything shown so far, oldest first.
    pub fn shown(&self) -> Vec<(OverlayKind, f64)> {
        self.shown
            .lock()
            .expect("the lock is never poisoned")
            .clone()
    }
}

impl OverlayNotifier for RecordingOverlay {
    fn show(&mut self, kind: OverlayKind, level: f64) -> Result<(), BackendError> {
        self.shown
            .lock()
            .expect("the lock is never poisoned")
            .push((kind, level));
        Ok(())
    }
}

/// The playback master volume through the kernel's sound control interface. The device is opened on the first use and
/// again after a failure, so a card that appears later (or went away) is picked up.
pub struct AlsaVolume {
    dir: PathBuf,
    card: Option<u32>,
    master: Option<Master>,
}

impl fmt::Debug for AlsaVolume {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AlsaVolume")
            .field("dir", &self.dir)
            .field("card", &self.card)
            .field("open", &self.master.is_some())
            .finish()
    }
}

impl AlsaVolume {
    /// The master of card `card`, or the best master of all cards (see `cupertino_alsa::master`), below `dir`.
    pub fn new(dir: PathBuf, card: Option<u32>) -> Self {
        Self {
            dir,
            card,
            master: None,
        }
    }

    fn master(&mut self) -> Result<&Master, cupertino_alsa::Error> {
        if self.master.is_none() {
            self.master = Some(match self.card {
                Some(card) => Master::open_card(&self.dir, card, OpenMode::ReadWrite)?,
                None => Master::open_default(&self.dir, OpenMode::ReadWrite)?,
            });
        }
        Ok(self.master.as_ref().expect("opened above"))
    }

    /// Run `action` on the master; a failure drops the open device so the next call opens it again.
    fn with_master<T>(
        &mut self,
        action: impl FnOnce(&Master) -> Result<T, cupertino_alsa::Error>,
    ) -> Result<T, BackendError> {
        let result = self.master().and_then(action);
        if result.is_err() {
            self.master = None;
        }
        Ok(result?)
    }
}

/// The level and mute state of `master`; a card without a mute switch is muted exactly at level 0, as in the overlay.
fn state(master: &Master) -> Result<VolumeState, cupertino_alsa::Error> {
    let level = master.level()?;
    Ok(VolumeState {
        level,
        muted: master.muted()?.unwrap_or(level <= 0.0),
    })
}

impl Volume for AlsaVolume {
    fn read(&mut self) -> Result<VolumeState, BackendError> {
        self.with_master(state)
    }

    fn set(&mut self, level: f64, muted: bool) -> Result<VolumeState, BackendError> {
        self.with_master(|master| {
            master.set_level(level)?;
            // a card without a mute switch is silenced by its level alone
            if master.muted()?.is_some_and(|current| current != muted) {
                master.set_muted(muted)?;
            }
            state(master)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_memory_radio_loses_its_network_when_switched_off() {
        let mut wifi = MemoryWifi(WifiStatus {
            enabled: true,
            network: Some("Home".into()),
        });
        wifi.set_enabled(false).unwrap();
        assert_eq!(
            wifi.read().unwrap(),
            WifiStatus {
                enabled: false,
                network: None
            }
        );
    }

    #[test]
    fn an_unavailable_backend_fails_with_its_reason() {
        let mut backend = Unavailable::new("no adapter");
        assert_eq!(
            Bluetooth::read(&mut backend).unwrap_err().to_string(),
            "no adapter"
        );
        assert_eq!(
            Brightness::set(&mut backend, 0.5).unwrap_err().to_string(),
            "no adapter"
        );
    }

    #[test]
    fn the_recording_overlay_keeps_its_history_through_a_clone() {
        let overlay = RecordingOverlay::default();
        let mut notifier = overlay.clone();
        notifier.show(OverlayKind::Volume, 0.5).unwrap();
        notifier.show(OverlayKind::Mute, 0.0).unwrap();
        assert_eq!(
            overlay.shown(),
            [(OverlayKind::Volume, 0.5), (OverlayKind::Mute, 0.0)]
        );
    }
}
