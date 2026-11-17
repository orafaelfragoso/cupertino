//! What the overlay changes: the output volume and the two backlights, behind traits so the controller and the
//! tests run on in-memory stand-ins.

use std::cmp::Ordering;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use cupertino_alsa::{Master, OpenMode};
use cupertino_pipewire::{Target, VolumeWorker, Wpctl};

/// Why an effect failed; shown to the user as a log line, never as an overlay.
pub type BackendError = Box<dyn std::error::Error + Send + Sync>;

/// The output volume after a change.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VolumeState {
    /// Level, 0 to 1.
    pub level: f64,
    /// Whether the output is muted.
    pub muted: bool,
}

/// The output volume. The sender's step rules live in `sender` and `controller`; a backend only reads and writes.
pub trait Volume: Send {
    /// The level and mute state now.
    fn state(&mut self) -> Result<VolumeState, BackendError>;

    /// Set the level (0 to 1); returns the level reached, which the device may round.
    fn set_level(&mut self, level: f64) -> Result<f64, BackendError>;

    /// Mute or unmute without touching the level.
    fn set_muted(&mut self, muted: bool) -> Result<(), BackendError>;
}

/// A backlight.
pub trait Backlight: Send {
    /// The level now, 0 to 1.
    fn level(&mut self) -> Result<f64, BackendError>;

    /// Set the level (0 to 1); returns the level reached, which the device may round or floor.
    fn set_level(&mut self, level: f64) -> Result<f64, BackendError>;
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

/// The volume of an in-memory stand-in (for `--fake-backends` and tests).
#[derive(Debug)]
pub struct MemoryVolume {
    state: VolumeState,
}

impl MemoryVolume {
    /// A volume at `level` (0 to 1).
    pub fn new(level: f64, muted: bool) -> Self {
        Self {
            state: VolumeState { level, muted },
        }
    }
}

impl Volume for MemoryVolume {
    fn state(&mut self) -> Result<VolumeState, BackendError> {
        Ok(self.state)
    }

    fn set_level(&mut self, level: f64) -> Result<f64, BackendError> {
        self.state.level = level;
        Ok(level)
    }

    fn set_muted(&mut self, muted: bool) -> Result<(), BackendError> {
        self.state.muted = muted;
        Ok(())
    }
}

/// The level of an in-memory backlight.
#[derive(Debug)]
pub struct MemoryBacklight {
    level: f64,
}

impl MemoryBacklight {
    /// A backlight at `level` (0 to 1).
    pub fn new(level: f64) -> Self {
        Self { level }
    }
}

impl Backlight for MemoryBacklight {
    fn level(&mut self) -> Result<f64, BackendError> {
        Ok(self.level)
    }

    fn set_level(&mut self, level: f64) -> Result<f64, BackendError> {
        self.level = level;
        Ok(level)
    }
}

/// The playback master volume through the kernel's sound control interface. The device is opened on the first use and
/// again after a failure, so a card that appears later (or went away) is picked up.
pub struct AlsaVolume {
    dir: PathBuf,
    card: Option<u32>,
    master: Option<Master>,
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

impl Volume for AlsaVolume {
    /// Without a mute switch the master is muted exactly when its level is 0.
    fn state(&mut self) -> Result<VolumeState, BackendError> {
        self.with_master(|master| {
            let level = master.level()?;
            Ok(VolumeState {
                level,
                muted: master.muted()?.unwrap_or(level <= 0.0),
            })
        })
    }

    /// A change of level moves at least one raw step, so a coarse raw range cannot swallow a press.
    fn set_level(&mut self, level: f64) -> Result<f64, BackendError> {
        self.with_master(|master| {
            let current = master.level()?;
            match level.partial_cmp(&current) {
                Some(Ordering::Greater) => master.step(1, level - current),
                Some(Ordering::Less) => master.step(-1, current - level),
                _ => Ok(current),
            }
        })
    }

    /// Without a mute switch only the state the level already implies can be set (the step to or from 0 that
    /// mutes or unmutes); toggling mute at another level fails with the missing switch.
    fn set_muted(&mut self, muted: bool) -> Result<(), BackendError> {
        self.with_master(|master| match master.muted()? {
            Some(_) => master.set_muted(muted),
            None if muted == (master.level()? <= 0.0) => Ok(()),
            None => Err(cupertino_alsa::Error::NoMuteSwitch),
        })
    }
}

/// The default PipeWire source through `wpctl`; without a PipeWire daemon there is no microphone to mute.
pub struct SessionMicrophone {
    wpctl: Wpctl,
}

impl SessionMicrophone {
    pub fn new(wpctl: Wpctl) -> Self {
        Self { wpctl }
    }
}

impl Volume for SessionMicrophone {
    fn state(&mut self) -> Result<VolumeState, BackendError> {
        let source = self.wpctl.volume(Target::DefaultSource)?;
        Ok(VolumeState {
            level: source.level,
            muted: source.muted,
        })
    }

    fn set_level(&mut self, level: f64) -> Result<f64, BackendError> {
        self.wpctl.set_level(Target::DefaultSource, level)?;
        Ok(self.wpctl.volume(Target::DefaultSource)?.level)
    }

    fn set_muted(&mut self, muted: bool) -> Result<(), BackendError> {
        Ok(self.wpctl.set_muted(Target::DefaultSource, muted)?)
    }
}

/// How long the sink's last known volume stands in for a reading: the presses of a held or quickly repeated key step
/// from it without waiting for `wpctl`, while a press after a pause reads the sink again and so follows a change made
/// elsewhere (approximate).
pub const KNOWN_FOR: Duration = Duration::from_millis(500);

/// The default PipeWire sink through `wpctl`, so the overlay shows the level `wpctl get-volume` reports. A read that
/// no PipeWire daemon answers goes to the ALSA master instead, and so do the changes after it until the next read.
/// Changes are written in the background ([`VolumeWorker`]) and the level reached is the one asked; for [`KNOWN_FOR`]
/// after a reading or a change the state is the known one.
pub struct SessionVolume {
    wpctl: Wpctl,
    worker: VolumeWorker,
    alsa: AlsaVolume,
    pipewire: bool,
    known: Option<(VolumeState, Instant)>,
}

impl SessionVolume {
    pub fn new(wpctl: Wpctl, alsa: AlsaVolume) -> Self {
        Self {
            worker: VolumeWorker::new(wpctl.clone(), Target::DefaultSink),
            wpctl,
            alsa,
            pipewire: true,
            known: None,
        }
    }

    fn write(&mut self, state: VolumeState) -> Result<(), BackendError> {
        self.known = Some((state, Instant::now()));
        Ok(self.worker.set(cupertino_pipewire::Volume {
            level: state.level,
            muted: state.muted,
        })?)
    }
}

impl Volume for SessionVolume {
    fn state(&mut self) -> Result<VolumeState, BackendError> {
        if let Some((state, _)) = self.known.filter(|(_, at)| at.elapsed() < KNOWN_FOR) {
            return Ok(state);
        }
        self.worker.wait_idle();
        match self.wpctl.volume(Target::DefaultSink) {
            Ok(sink) => {
                self.pipewire = true;
                let state = VolumeState {
                    level: sink.level,
                    muted: sink.muted,
                };
                self.known = Some((state, Instant::now()));
                Ok(state)
            }
            Err(_) => {
                self.pipewire = false;
                self.known = None;
                self.alsa.state()
            }
        }
    }

    fn set_level(&mut self, level: f64) -> Result<f64, BackendError> {
        if !self.pipewire {
            return self.alsa.set_level(level);
        }
        let level = if level.is_nan() {
            0.0
        } else {
            level.clamp(0.0, 1.0)
        };
        let muted = self.state()?.muted;
        self.write(VolumeState { level, muted })?;
        Ok(level)
    }

    fn set_muted(&mut self, muted: bool) -> Result<(), BackendError> {
        if !self.pipewire {
            return self.alsa.set_muted(muted);
        }
        let level = self.state()?.level;
        self.write(VolumeState { level, muted })
    }
}
