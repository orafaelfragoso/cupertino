//! [`LogindSession`] and [`LogindNotifier`]: the Smithay [`Session`] and its calloop event source.
//!
//! logind signals arrive on reader threads (the blocking `zbus` API has no pollable
//! descriptor) and cross to the loop through a calloop channel, so every state change and every
//! callback runs on the loop thread. One thread reads the session's signals, the other waits for
//! logind's bus name to leave the process the session was found on. The method calls (`TakeDevice`, `ReleaseDevice`,
//! `PauseDeviceComplete`, `SwitchTo`) are blocking calls made on the thread that uses the session.
//!
//! # Differences from Smithay's libseat session
//!
//! - **Pause handshake.** libseat acknowledges a disable before the loop callback runs. Here
//!   `PauseDeviceComplete` is sent after the callback returned, so the holder has stopped using
//!   the devices when logind is told they are free. A device the holder closed in the callback is
//!   not acknowledged (logind sends no more signals for it).
//! - **Resumed descriptors are dropped.** For a DRM node logind's `ResumeDevice` names the open
//!   file the holder already has, which is DRM master again. For an input node it is a new open
//!   file and the holder's stays revoked, so the holder closes input devices on
//!   [`Event::PauseSession`] and opens them again on [`Event::ActivateSession`], which is reported
//!   once every paused device was resumed.
//! - **Strict close.** [`Session::close`] on a descriptor not handed out here is an error.
//! - **`flags` of `open` are ignored**, as in Smithay's libseat session: logind opens the device
//!   itself.
//! - **Inactive start.** A session that starts inactive reports no event until it activates:
//!   check [`Session::is_active`] before opening devices.

use std::cell::RefCell;
use std::os::fd::{AsRawFd, OwnedFd};
use std::path::Path;
use std::rc::{Rc, Weak};

use cupertino_devices::DevT;
use rustix::fs::{stat, FileType, OFlags};
use smithay::backend::session::{Event, Session};
use smithay::reexports::calloop::channel::{channel, Channel, Event as ChannelEvent, Sender};
use smithay::reexports::calloop::{
    self, EventSource, Poll, PostAction, Readiness, Token, TokenFactory,
};
use zbus::blocking::MessageIterator;

use crate::error::Error;
use crate::logind::{self, Logind, SessionLookup};
use crate::state::{Input, Machine, Output, Transition};

/// What the reader threads and [`Session::close`] send to the loop.
type Delivery = Result<Input, Error>;

struct Shared {
    logind: Logind,
    machine: Machine,
    sender: Sender<Delivery>,
}

/// A session under the control of this process. Clones share one session; every call fails with
/// [`Error::SessionLost`] once the [`LogindNotifier`] is dropped.
#[derive(Clone)]
pub struct LogindSession {
    shared: Weak<RefCell<Shared>>,
    seat: String,
}

/// The calloop event source that reports [`Event::PauseSession`] and [`Event::ActivateSession`],
/// and owns the session: dropping it calls `ReleaseControl`.
pub struct LogindNotifier {
    shared: Rc<RefCell<Shared>>,
    channel: Channel<Delivery>,
}

impl LogindSession {
    /// Finds this process's session (`XDG_SESSION_ID`, else the process id; see
    /// [`SessionLookup`]), takes control of it and starts listening for its signals.
    ///
    /// Fails when the session already has a controller or the caller is not allowed to control it.
    pub fn new() -> Result<(Self, LogindNotifier), Error> {
        let lookup = SessionLookup::from_environment()?;
        let conn = logind::connect()?;
        let session = logind::locate(&conn, &lookup)?;
        let signals = logind::subscribe(&conn, &session)?;
        let owner_changes = logind::watch_owner(&conn, &session)?;
        let (logind, active, seat) = Logind::take_control(&conn, &session)?;

        let (sender, channel) = channel();
        let reader_sender = sender.clone();
        std::thread::Builder::new()
            .name("cupertino-seat-signals".to_owned())
            .spawn(move || read_signals(signals, reader_sender))
            .map_err(Error::Thread)?;
        let owner_sender = sender.clone();
        std::thread::Builder::new()
            .name("cupertino-seat-owner".to_owned())
            .spawn(move || watch_owner(owner_changes, owner_sender))
            .map_err(Error::Thread)?;

        let shared = Rc::new(RefCell::new(Shared {
            logind,
            machine: Machine::new(active),
            sender,
        }));
        let session = LogindSession {
            shared: Rc::downgrade(&shared),
            seat,
        };
        Ok((session, LogindNotifier { shared, channel }))
    }

    fn upgrade(&self) -> Result<Rc<RefCell<Shared>>, Error> {
        self.shared.upgrade().ok_or(Error::SessionLost)
    }
}

/// Forwards decoded signals until the stream ends or the loop side is gone. A signal that does
/// not decode (a `PauseDevice` type or a property change this crate does not know) is logged and
/// skipped: it cannot be acted on, and the session stays usable. The thread blocks in the
/// iterator, so it outlives the notifier until the next signal or the end of the connection.
fn read_signals(signals: MessageIterator, sender: Sender<Delivery>) {
    for message in signals {
        let delivery = match message {
            Ok(message) => match logind::decode(&message) {
                Ok(Some(input)) => Ok(input),
                Ok(None) => continue,
                Err(error) => {
                    tracing::warn!(%error, "ignoring a session signal");
                    continue;
                }
            },
            Err(source) => Err(Error::bus("reading a session signal")(source)),
        };
        let failed = delivery.is_err();
        if sender.send(delivery).is_err() || failed {
            return;
        }
    }
    if sender.send(Err(Error::SignalsEnded)).is_err() {
        tracing::debug!("the session was dropped before the signal stream ended");
    }
}

/// Reports [`Error::SignalsEnded`] when logind leaves the bus name the session is bound to, or
/// when the stream fails. Blocks like [`read_signals`].
fn watch_owner(
    mut owner_changes: impl Iterator<Item = zbus::Result<zbus::Message>>,
    sender: Sender<Delivery>,
) {
    let delivery = match owner_changes.next() {
        Some(Err(source)) => Err(Error::bus("watching logind's bus name")(source)),
        Some(Ok(_)) | None => Err(Error::SignalsEnded),
    };
    if sender.send(delivery).is_err() {
        tracing::debug!("the session was dropped before logind's bus name changed");
    }
}

/// `dev_t` of a character device node.
fn device_number(path: &Path) -> Result<DevT, Error> {
    let status = stat(path).map_err(|errno| Error::Stat {
        path: path.to_owned(),
        errno,
    })?;
    if FileType::from_raw_mode(status.st_mode) != FileType::CharacterDevice {
        return Err(Error::NotCharDevice(path.to_owned()));
    }
    Ok(DevT::from_raw(status.st_rdev))
}

fn event_of(transition: Transition) -> Event {
    match transition {
        Transition::Pause => Event::PauseSession,
        Transition::Activate => Event::ActivateSession,
    }
}

impl Session for LogindSession {
    type Error = Error;

    /// `TakeDevice` for the device number of `path`, after retrying a `ReleaseDevice` of the same
    /// device that failed in [`Session::close`]. A descriptor returned while the session is
    /// inactive is paused until the session activates.
    fn open(&mut self, path: &Path, _flags: OFlags) -> Result<OwnedFd, Error> {
        let dev = device_number(path)?;
        let shared = self.upgrade()?;
        let mut shared = shared.borrow_mut();
        if shared.machine.is_unreleased(dev) {
            shared.logind.release_device(dev)?;
            shared.machine.confirm_release(dev);
        }
        let (fd, inactive) = shared.logind.take_device(dev)?;
        shared.machine.take(dev, fd.as_raw_fd(), inactive)?;
        Ok(fd)
    }

    /// `ReleaseDevice` for the device behind `fd`, then closes `fd`. A device that was reported
    /// gone is only forgotten. When `ReleaseDevice` fails the descriptor is still closed and the
    /// release is retried by the next [`Session::open`] of the device.
    fn close(&mut self, fd: OwnedFd) -> Result<(), Error> {
        let shared = self.upgrade()?;
        let mut shared = shared.borrow_mut();
        let released = shared.machine.release(fd.as_raw_fd())?;
        if released.notify_logind {
            shared.logind.release_device(released.dev)?;
            shared.machine.confirm_release(released.dev);
        }
        // The released device may have been the last one the paused session waited for.
        if shared.machine.is_waiting_to_activate() {
            shared
                .sender
                .send(Ok(Input::Recheck))
                .map_err(|_| Error::SessionLost)?;
        }
        Ok(())
    }

    /// `Seat.SwitchTo(vt)`.
    fn change_vt(&mut self, vt: i32) -> Result<(), Error> {
        let vt = u32::try_from(vt).map_err(|_| Error::InvalidVt(vt))?;
        self.upgrade()?.borrow().logind.switch_to(vt)
    }

    /// Whether the session is running. False once the notifier is gone, because the trait has
    /// no way to report the lost session here.
    fn is_active(&self) -> bool {
        self.shared
            .upgrade()
            .is_some_and(|shared| shared.borrow().machine.is_running())
    }

    fn seat(&self) -> String {
        self.seat.clone()
    }
}

impl LogindNotifier {
    /// Applies one input and carries out its outputs in order. No borrow of the shared state is
    /// held while `callback` runs: the holder calls back into the session from there.
    fn apply<F: FnMut(Event, &mut ())>(&self, input: Input, callback: &mut F) -> Result<(), Error> {
        let outputs = self.shared.borrow_mut().machine.step(input);
        for output in outputs {
            match output {
                Output::Transition(transition) => callback(event_of(transition), &mut ()),
                Output::CompletePause(dev) => {
                    let mut shared = self.shared.borrow_mut();
                    // False when the callback closed the device: logind needs no answer then.
                    if shared.machine.ack_pause(dev) {
                        shared.logind.pause_device_complete(dev)?;
                    }
                }
            }
        }
        Ok(())
    }
}

impl EventSource for LogindNotifier {
    type Event = Event;
    type Metadata = ();
    type Ret = ();
    type Error = Error;

    fn process_events<F>(
        &mut self,
        readiness: Readiness,
        token: Token,
        mut callback: F,
    ) -> Result<PostAction, Error>
    where
        F: FnMut(Event, &mut ()),
    {
        // `Shared` holds a sender, so the channel cannot close while this notifier exists: a
        // reader thread that ends reports it with an explicit error delivery.
        let mut deliveries = Vec::new();
        self.channel
            .process_events(readiness, token, |event, _| {
                if let ChannelEvent::Msg(delivery) = event {
                    deliveries.push(delivery);
                }
            })
            .map_err(|_| Error::SessionLost)?;
        for delivery in deliveries {
            self.apply(delivery?, &mut callback)?;
        }
        Ok(PostAction::Continue)
    }

    fn register(&mut self, poll: &mut Poll, factory: &mut TokenFactory) -> calloop::Result<()> {
        self.channel.register(poll, factory)
    }

    fn reregister(&mut self, poll: &mut Poll, factory: &mut TokenFactory) -> calloop::Result<()> {
        self.channel.reregister(poll, factory)
    }

    fn unregister(&mut self, poll: &mut Poll) -> calloop::Result<()> {
        self.channel.unregister(poll)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn character_device_has_its_device_number() {
        // /dev/null is 1:3 on Linux.
        assert_eq!(
            device_number(Path::new("/dev/null")).unwrap(),
            DevT::new(1, 3)
        );
    }

    #[test]
    fn regular_file_is_not_a_device() {
        assert!(matches!(
            device_number(Path::new("/etc/hostname")),
            Err(Error::NotCharDevice(_))
        ));
    }
}
