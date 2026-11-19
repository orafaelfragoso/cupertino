//! Device and session bookkeeping as a pure state machine.
//!
//! [`Machine`] knows which devices the session holds (`dev_t` to descriptor number), whether each
//! is active, waiting for our `PauseDeviceComplete`, paused or gone, and whether the session as a
//! whole runs, and which handed-back devices logind may still count as taken. It never touches
//! D-Bus or a descriptor: the caller feeds it [`Input`]s decoded from logind messages and carries
//! out the [`Output`]s it returns.
//!
//! The ordering rules come from `man org.freedesktop.login1`, section "org.freedesktop.login1.Session":
//! `PauseDevice` signals are sent before the `Active` property changes, and `ResumeDevice` signals
//! after it. The session is therefore reported paused on the first pause of any device, and
//! reported active again only once logind says `Active` and every paused device has been resumed.

use std::collections::{HashMap, HashSet};
use std::os::fd::RawFd;

use cupertino_devices::DevT;

use crate::error::Error;

/// The `type` argument of a `PauseDevice` signal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PauseKind {
    /// logind grants a limited time to stop using the device; the controller must answer with
    /// `PauseDeviceComplete`.
    Pause,
    /// The device is already paused; the signal is only a notification.
    Force,
    /// The device was unplugged; no further signals follow and `ReleaseDevice` is not needed.
    Gone,
}

impl PauseKind {
    /// Parses the wire string (`"pause"`, `"force"`, `"gone"`).
    pub fn parse(wire: &str) -> Result<Self, Error> {
        match wire {
            "pause" => Ok(Self::Pause),
            "force" => Ok(Self::Force),
            "gone" => Ok(Self::Gone),
            other => Err(Error::UnknownPauseType(other.to_owned())),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeviceState {
    Active,
    /// logind asked for a pause and waits for `PauseDeviceComplete`.
    PausePending,
    Paused,
    /// Unplugged. The entry stays until the holder hands the descriptor back, so that
    /// [`Machine::release`] can recognise it.
    Gone,
}

impl DeviceState {
    /// Whether the session still waits for this device to resume before it can run again.
    fn blocks_activation(self) -> bool {
        matches!(self, Self::PausePending | Self::Paused)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Running,
    /// A device was paused but `Active` has not turned false yet.
    Pausing,
    /// `Active` turned false (or the session started inactive).
    Paused,
}

#[derive(Debug)]
struct Tracked {
    /// The descriptor number handed to the holder, by which [`Machine::release`] finds the device.
    fd: RawFd,
    state: DeviceState,
}

/// A change of the whole session, for the notifier to report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transition {
    Pause,
    Activate,
}

/// A decoded logind message, or a request to look again.
#[derive(Debug, PartialEq, Eq)]
pub enum Input {
    /// `PauseDevice(major, minor, type)`.
    PauseDevice { dev: DevT, kind: PauseKind },
    /// `ResumeDevice(major, minor, fd)`. The descriptor is not kept: for a DRM node it is the open
    /// file the holder already has, for an input node a new one the holder gets by opening the
    /// device again after the activation.
    ResumeDevice { dev: DevT },
    /// The `Active` property changed to this value.
    ActiveChanged(bool),
    /// A device was released while the session waits to activate; re-evaluate.
    Recheck,
}

/// What the caller must do after an [`Input`], in order.
#[derive(Debug, PartialEq, Eq)]
pub enum Output {
    /// Tell the holder that the session paused or activated. Carry this out before the
    /// following outputs: the holder must stop using devices before they are acknowledged.
    Transition(Transition),
    /// Call `PauseDeviceComplete`, after checking [`Machine::ack_pause`].
    CompletePause(DevT),
}

/// A device taken back by [`Machine::release`].
#[derive(Debug, PartialEq, Eq)]
pub struct Released {
    pub dev: DevT,
    /// Whether `ReleaseDevice` must be called, followed by [`Machine::confirm_release`] once it
    /// succeeded. A gone device needs no release.
    pub notify_logind: bool,
}

/// The bookkeeping of one session. See the module documentation.
#[derive(Debug)]
pub struct Machine {
    phase: Phase,
    /// The last value of the `Active` property.
    logind_active: bool,
    devices: HashMap<DevT, Tracked>,
    /// Devices handed back whose `ReleaseDevice` has not succeeded yet: logind still counts them
    /// as taken, so they must be released before they are taken again.
    unreleased: HashSet<DevT>,
}

impl Machine {
    /// A session that starts with `Active` set to `active`.
    pub fn new(active: bool) -> Self {
        Self {
            phase: if active {
                Phase::Running
            } else {
                Phase::Paused
            },
            logind_active: active,
            devices: HashMap::new(),
            unreleased: HashSet::new(),
        }
    }

    /// Whether the session is running, which is what the holder's `is_active` reports.
    pub fn is_running(&self) -> bool {
        self.phase == Phase::Running
    }

    /// Whether a [`Input::Recheck`] can change anything.
    pub fn is_waiting_to_activate(&self) -> bool {
        self.phase == Phase::Paused
    }

    pub fn check_available(&self, dev: DevT) -> Result<(), Error> {
        if self
            .devices
            .get(&dev)
            .is_some_and(|tracked| tracked.state != DeviceState::Gone)
        {
            return Err(Error::AlreadyTaken(dev));
        }
        Ok(())
    }

    /// Records a device logind just handed out as descriptor number `fd`. `inactive` is the flag
    /// `TakeDevice` returned. A gone entry for the same device number is replaced, because logind
    /// allows taking a device again once it was reported gone; a descriptor handed out for the
    /// replaced entry is then no longer recognised by [`Machine::release`].
    pub fn take(&mut self, dev: DevT, fd: RawFd, inactive: bool) -> Result<(), Error> {
        self.check_available(dev)?;
        let state = if inactive {
            DeviceState::Paused
        } else {
            DeviceState::Active
        };
        self.devices.insert(dev, Tracked { fd, state });
        Ok(())
    }

    /// Forgets the device behind descriptor number `fd`. A device that still needs `ReleaseDevice`
    /// stays recorded as unreleased until [`Machine::confirm_release`].
    pub fn release(&mut self, fd: RawFd) -> Result<Released, Error> {
        let dev = self
            .devices
            .iter()
            .find(|(_, tracked)| tracked.fd == fd)
            .map(|(dev, _)| *dev)
            .ok_or(Error::UnknownFd(fd))?;
        let tracked = self
            .devices
            .remove(&dev)
            .expect("the device was just found");
        let notify_logind = tracked.state != DeviceState::Gone;
        if notify_logind {
            self.unreleased.insert(dev);
        }
        Ok(Released { dev, notify_logind })
    }

    /// Records that `ReleaseDevice` for `dev` succeeded.
    pub fn confirm_release(&mut self, dev: DevT) {
        self.unreleased.remove(&dev);
    }

    /// Whether `dev` was handed back but logind may still count it as taken, because its
    /// `ReleaseDevice` failed. Release it before taking it again.
    pub fn is_unreleased(&self, dev: DevT) -> bool {
        self.unreleased.contains(&dev)
    }

    /// Marks a pending pause as answered. False when the device was released or went away since
    /// the pause arrived, in which case logind must not be told.
    pub fn ack_pause(&mut self, dev: DevT) -> bool {
        match self.devices.get_mut(&dev) {
            Some(tracked) if tracked.state == DeviceState::PausePending => {
                tracked.state = DeviceState::Paused;
                true
            }
            _ => false,
        }
    }

    /// Applies one message. Messages about a device that is not held are ignored: logind sends
    /// no signals after `ReleaseDevice`, but one can already be in flight.
    pub fn step(&mut self, input: Input) -> Vec<Output> {
        let mut outputs = Vec::new();
        match input {
            Input::PauseDevice { dev, kind } => {
                let Some(tracked) = self.devices.get_mut(&dev) else {
                    return outputs;
                };
                if tracked.state == DeviceState::Gone {
                    return outputs;
                }
                match kind {
                    PauseKind::Gone => tracked.state = DeviceState::Gone,
                    PauseKind::Pause => {
                        tracked.state = DeviceState::PausePending;
                        self.pause_session(&mut outputs);
                        outputs.push(Output::CompletePause(dev));
                    }
                    PauseKind::Force => {
                        tracked.state = DeviceState::Paused;
                        self.pause_session(&mut outputs);
                    }
                }
            }
            Input::ResumeDevice { dev } => {
                let Some(tracked) = self.devices.get_mut(&dev) else {
                    return outputs;
                };
                if tracked.state == DeviceState::Gone {
                    return outputs;
                }
                tracked.state = DeviceState::Active;
                self.try_activate(true, &mut outputs);
            }
            Input::ActiveChanged(false) => {
                self.logind_active = false;
                self.pause_session(&mut outputs);
                self.phase = Phase::Paused;
            }
            Input::ActiveChanged(true) => {
                self.logind_active = true;
                self.try_activate(false, &mut outputs);
            }
            Input::Recheck => self.try_activate(false, &mut outputs),
        }
        outputs
    }

    /// Reports the pause once, on whichever message announces it first.
    fn pause_session(&mut self, outputs: &mut Vec<Output>) {
        if self.phase == Phase::Running {
            self.phase = Phase::Pausing;
            outputs.push(Output::Transition(Transition::Pause));
        }
    }

    /// Reports the activation once `Active` is true and no device is still paused. A session that
    /// never saw `Active` turn false (`Pausing`) only activates when a resume proves devices came
    /// back, not when a release or a property change merely finds the device list empty.
    fn try_activate(&mut self, from_resume: bool, outputs: &mut Vec<Output>) {
        let idle = !self
            .devices
            .values()
            .any(|tracked| tracked.state.blocks_activation());
        let ready = match self.phase {
            Phase::Running => false,
            Phase::Pausing => from_resume && idle,
            Phase::Paused => self.logind_active && idle,
        };
        if ready {
            self.phase = Phase::Running;
            outputs.push(Output::Transition(Transition::Activate));
        }
    }
}
