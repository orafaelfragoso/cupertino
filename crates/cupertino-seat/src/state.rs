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

    /// Records a device logind just handed out as descriptor number `fd`. `inactive` is the flag
    /// `TakeDevice` returned. A gone entry for the same device number is replaced, because logind
    /// allows taking a device again once it was reported gone; a descriptor handed out for the
    /// replaced entry is then no longer recognised by [`Machine::release`].
    pub fn take(&mut self, dev: DevT, fd: RawFd, inactive: bool) -> Result<(), Error> {
        if self
            .devices
            .get(&dev)
            .is_some_and(|tracked| tracked.state != DeviceState::Gone)
        {
            return Err(Error::AlreadyTaken(dev));
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn drm() -> DevT {
        DevT::new(226, 0)
    }

    fn event() -> DevT {
        DevT::new(13, 64)
    }

    fn pause(dev: DevT, kind: PauseKind) -> Input {
        Input::PauseDevice { dev, kind }
    }

    fn resume(dev: DevT) -> Input {
        Input::ResumeDevice { dev }
    }

    fn active(value: bool) -> Input {
        Input::ActiveChanged(value)
    }

    fn recheck() -> Input {
        Input::Recheck
    }

    /// A running session holding the drm() node as descriptor 7 and an event node as 9.
    fn running() -> Machine {
        let mut machine = Machine::new(true);
        machine.take(drm(), 7, false).unwrap();
        machine.take(event(), 9, false).unwrap();
        machine
    }

    #[test]
    fn pause_type_pause_reports_the_session_then_asks_for_the_ack() {
        let mut machine = running();
        assert_eq!(
            machine.step(pause(drm(), PauseKind::Pause)),
            vec![
                Output::Transition(Transition::Pause),
                Output::CompletePause(drm())
            ]
        );
        assert!(!machine.is_running());
        assert!(machine.ack_pause(drm()));
        assert!(!machine.ack_pause(drm()), "an ack is due once");
    }

    #[test]
    fn second_pause_does_not_report_the_session_again() {
        let mut machine = running();
        machine.step(pause(drm(), PauseKind::Pause));
        assert_eq!(
            machine.step(pause(event(), PauseKind::Pause)),
            vec![Output::CompletePause(event())]
        );
    }

    #[test]
    fn released_device_is_not_acknowledged() {
        let mut machine = running();
        machine.step(pause(drm(), PauseKind::Pause));
        machine.step(pause(event(), PauseKind::Pause));
        assert_eq!(
            machine.release(9).unwrap(),
            Released {
                dev: event(),
                notify_logind: true
            }
        );
        assert!(!machine.ack_pause(event()));
        assert!(machine.ack_pause(drm()));
    }

    #[test]
    fn forced_pause_reports_the_session_without_an_ack() {
        let mut machine = running();
        assert_eq!(
            machine.step(pause(drm(), PauseKind::Force)),
            vec![Output::Transition(Transition::Pause)]
        );
        assert!(!machine.ack_pause(drm()));
    }

    #[test]
    fn gone_device_keeps_the_session_running_and_needs_no_release() {
        let mut machine = running();
        assert_eq!(machine.step(pause(event(), PauseKind::Gone)), vec![]);
        assert!(machine.is_running());
        assert_eq!(
            machine.release(9).unwrap(),
            Released {
                dev: event(),
                notify_logind: false
            }
        );
    }

    #[test]
    fn messages_after_gone_or_release_are_ignored() {
        let mut machine = running();
        machine.step(pause(event(), PauseKind::Gone));
        assert_eq!(machine.step(pause(event(), PauseKind::Pause)), vec![]);
        assert_eq!(machine.step(resume(event())), vec![]);
        machine.release(7).unwrap();
        assert_eq!(machine.step(resume(drm())), vec![]);
    }

    #[test]
    fn vt_switch_away_and_back_activates_on_the_drm_resume() {
        let mut machine = running();
        // Switching away: PauseDevice first, then Active turns false.
        machine.step(pause(drm(), PauseKind::Pause));
        machine.ack_pause(drm());
        assert_eq!(machine.step(active(false)), vec![]);
        // Switching back: Active turns true first, then ResumeDevice.
        assert_eq!(machine.step(active(true)), vec![], "drm() is still paused");
        assert_eq!(
            machine.step(resume(drm())),
            vec![Output::Transition(Transition::Activate)]
        );
        assert!(machine.is_running());
    }

    #[test]
    fn activation_waits_for_every_paused_device() {
        let mut machine = running();
        machine.step(pause(drm(), PauseKind::Force));
        machine.step(pause(event(), PauseKind::Force));
        machine.step(active(false));
        machine.step(active(true));
        assert_eq!(machine.step(resume(event())), vec![]);
        assert_eq!(
            machine.step(resume(drm())),
            vec![Output::Transition(Transition::Activate)]
        );
    }

    #[test]
    fn closing_the_last_paused_device_lets_a_waiting_session_activate() {
        let mut machine = running();
        machine.step(pause(event(), PauseKind::Force));
        machine.step(active(false));
        machine.step(active(true));
        assert_eq!(machine.step(recheck()), vec![]);
        machine.release(9).unwrap();
        assert!(machine.is_waiting_to_activate());
        assert_eq!(
            machine.step(recheck()),
            vec![Output::Transition(Transition::Activate)]
        );
    }

    #[test]
    fn empty_device_list_does_not_activate_before_active_turned_false() {
        let mut machine = running();
        machine.step(pause(event(), PauseKind::Pause));
        machine.release(9).unwrap();
        assert_eq!(machine.step(recheck()), vec![]);
        assert!(!machine.is_running());
    }

    #[test]
    fn active_false_without_a_device_pause_reports_the_session() {
        let mut machine = Machine::new(true);
        assert_eq!(
            machine.step(active(false)),
            vec![Output::Transition(Transition::Pause)]
        );
        assert_eq!(
            machine.step(active(true)),
            vec![Output::Transition(Transition::Activate)]
        );
    }

    #[test]
    fn session_that_starts_inactive_activates_on_active_and_resume() {
        let mut machine = Machine::new(false);
        assert!(!machine.is_running());
        machine.take(drm(), 7, true).unwrap();
        assert_eq!(machine.step(active(true)), vec![]);
        assert_eq!(
            machine.step(resume(drm())),
            vec![Output::Transition(Transition::Activate)]
        );
    }

    #[test]
    fn taking_a_held_device_twice_fails_but_a_gone_one_can_be_retaken() {
        let mut machine = running();
        assert!(
            matches!(machine.take(drm(), 11, false), Err(Error::AlreadyTaken(dev)) if dev == drm())
        );
        machine.step(pause(drm(), PauseKind::Gone));
        machine.take(drm(), 11, false).unwrap();
        assert_eq!(
            machine.release(11).unwrap(),
            Released {
                dev: drm(),
                notify_logind: true
            }
        );
    }

    #[test]
    fn device_stays_unreleased_until_the_release_is_confirmed() {
        let mut machine = running();
        machine.release(9).unwrap();
        assert!(
            machine.is_unreleased(event()),
            "ReleaseDevice has not succeeded yet"
        );
        assert!(
            machine.step(pause(event(), PauseKind::Pause)).is_empty(),
            "no longer held"
        );
        machine.confirm_release(event());
        assert!(!machine.is_unreleased(event()));
        machine.step(pause(drm(), PauseKind::Gone));
        machine.release(7).unwrap();
        assert!(
            !machine.is_unreleased(drm()),
            "a gone device needs no release"
        );
    }

    #[test]
    fn releasing_an_unknown_descriptor_fails() {
        let mut machine = running();
        assert!(matches!(machine.release(99), Err(Error::UnknownFd(99))));
    }

    #[test]
    fn pause_kind_parses_the_wire_strings() {
        assert_eq!(PauseKind::parse("pause").unwrap(), PauseKind::Pause);
        assert_eq!(PauseKind::parse("force").unwrap(), PauseKind::Force);
        assert_eq!(PauseKind::parse("gone").unwrap(), PauseKind::Gone);
        assert!(
            matches!(PauseKind::parse("suspend"), Err(Error::UnknownPauseType(kind)) if kind == "suspend")
        );
    }
}
