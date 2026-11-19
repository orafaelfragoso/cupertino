//! [`LogindSession`] and [`LogindNotifier`]: the Smithay [`Session`] and its calloop event source
//! over `cupertino-seat`.

mod calls;

use std::cell::RefCell;
use std::fmt;
use std::os::fd::{AsRawFd, OwnedFd};
use std::path::Path;
use std::rc::{Rc, Weak};

use cupertino_devices::DevT;
use cupertino_seat::error::Error as SeatError;
use cupertino_seat::logind::{self, Logind, SessionLookup};
use cupertino_seat::state::{Input, Machine, Output, Transition};
use rustix::fs::{stat, FileType, OFlags};
use smithay::backend::session::{AsErrno, Event, Session};
use smithay::reexports::calloop::channel::{channel, Channel, Event as ChannelEvent, Sender};
use smithay::reexports::calloop::{
    self, EventSource, Poll, PostAction, Readiness, Token, TokenFactory,
};
use zbus::blocking::{Connection, MessageIterator};

use self::calls::Calls;

/// Wrapper around [`cupertino_seat::Error`] implementing Smithay's [`AsErrno`].
#[derive(Debug)]
pub struct SessionError(pub SeatError);

impl fmt::Display for SessionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl std::error::Error for SessionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.0.source()
    }
}

impl From<SeatError> for SessionError {
    fn from(error: SeatError) -> Self {
        Self(error)
    }
}

impl AsErrno for SessionError {
    fn as_errno(&self) -> Option<i32> {
        match &self.0 {
            SeatError::Stat { errno, .. } => Some(errno.raw_os_error()),
            _ => None,
        }
    }
}

/// What the reader threads and [`Session::close`] send to the loop.
pub(crate) type Delivery = Result<Input, SeatError>;

struct Shared {
    logind: Calls,
    machine: Machine,
    sender: Sender<Delivery>,
}

/// A session under the control of this process. Clones share one session; every call fails with
/// [`SeatError::SessionLost`] once the [`LogindNotifier`] is dropped.
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
    /// Finds this process's session (`XDG_SESSION_ID`, else the process id), takes control of it
    /// and starts listening for its signals.
    pub fn new() -> Result<(Self, LogindNotifier), SessionError> {
        let lookup = SessionLookup::from_environment()?;
        let (sender, channel) = channel();
        let (logind, active, seat) = Calls::start(lookup, sender.clone())?;

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

    fn upgrade(&self) -> Result<Rc<RefCell<Shared>>, SessionError> {
        self.shared
            .upgrade()
            .ok_or_else(|| SessionError(SeatError::SessionLost))
    }
}

pub(crate) fn listen(
    conn: &Connection,
    lookup: &SessionLookup,
    sender: Sender<Delivery>,
) -> Result<(Logind, bool, String), SeatError> {
    let session = logind::locate(conn, lookup)?;
    let signals = logind::subscribe(conn, &session)?;
    let owner_changes = logind::watch_owner(conn, &session)?;
    let control = Logind::take_control(conn, &session)?;
    let reader_sender = sender.clone();
    std::thread::Builder::new()
        .name("cupertino-seat-signals".to_owned())
        .spawn(move || read_signals(signals, reader_sender))
        .map_err(SeatError::Thread)?;
    std::thread::Builder::new()
        .name("cupertino-seat-owner".to_owned())
        .spawn(move || watch_owner(owner_changes, sender))
        .map_err(SeatError::Thread)?;
    Ok(control)
}

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
            Err(source) => Err(SeatError::bus("reading a session signal")(source)),
        };
        let failed = delivery.is_err();
        if sender.send(delivery).is_err() || failed {
            return;
        }
    }
    if sender.send(Err(SeatError::SignalsEnded)).is_err() {
        tracing::debug!("the session was dropped before the signal stream ended");
    }
}

fn watch_owner(
    mut owner_changes: impl Iterator<Item = zbus::Result<zbus::Message>>,
    sender: Sender<Delivery>,
) {
    let delivery = match owner_changes.next() {
        Some(Err(source)) => Err(SeatError::bus("watching logind's bus name")(source)),
        Some(Ok(_)) | None => Err(SeatError::SignalsEnded),
    };
    if sender.send(delivery).is_err() {
        tracing::debug!("the session was dropped before logind's bus name changed");
    }
}

fn device_number(path: &Path) -> Result<DevT, SeatError> {
    let status = stat(path).map_err(|errno| SeatError::Stat {
        path: path.to_owned(),
        errno,
    })?;
    if FileType::from_raw_mode(status.st_mode) != FileType::CharacterDevice {
        return Err(SeatError::NotCharDevice(path.to_owned()));
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
    type Error = SessionError;

    fn open(&mut self, path: &Path, _flags: OFlags) -> Result<OwnedFd, SessionError> {
        let dev = device_number(path)?;
        let shared = self.upgrade()?;
        let mut shared = shared.borrow_mut();
        shared.machine.check_available(dev)?;
        if shared.machine.is_unreleased(dev) {
            shared.logind.release_device(dev, true)?;
            shared.machine.confirm_release(dev);
        }
        let (fd, inactive) = shared.logind.take_device(dev)?;
        shared.machine.take(dev, fd.as_raw_fd(), inactive)?;
        Ok(fd)
    }

    fn close(&mut self, fd: OwnedFd) -> Result<(), SessionError> {
        let shared = self.upgrade()?;
        let mut shared = shared.borrow_mut();
        let released = shared.machine.release(fd.as_raw_fd())?;
        if released.notify_logind {
            shared.logind.release_device(released.dev, false)?;
            shared.machine.confirm_release(released.dev);
        }
        if shared.machine.is_waiting_to_activate() {
            shared
                .sender
                .send(Ok(Input::Recheck))
                .map_err(|_| SessionError(SeatError::SessionLost))?;
        }
        Ok(())
    }

    fn change_vt(&mut self, vt: i32) -> Result<(), SessionError> {
        let vt = u32::try_from(vt).map_err(|_| SessionError(SeatError::InvalidVt(vt)))?;
        self.upgrade()?.borrow().logind.switch_to(vt)?;
        Ok(())
    }

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
    fn apply<F: FnMut(Event, &mut ())>(
        &self,
        input: Input,
        callback: &mut F,
    ) -> Result<(), SessionError> {
        let outputs = self.shared.borrow_mut().machine.step(input);
        for output in outputs {
            match output {
                Output::Transition(transition) => callback(event_of(transition), &mut ()),
                Output::CompletePause(dev) => {
                    let mut shared = self.shared.borrow_mut();
                    if shared.machine.ack_pause(dev) {
                        match shared.logind.pause_device_complete(dev) {
                            Err(error) if error.unanswered() => {
                                tracing::warn!(%dev, %error, "pause acknowledgement unanswered; staying paused");
                            }
                            other => other?,
                        }
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
    type Error = SessionError;

    fn process_events<F>(
        &mut self,
        readiness: Readiness,
        token: Token,
        mut callback: F,
    ) -> Result<PostAction, SessionError>
    where
        F: FnMut(Event, &mut ()),
    {
        let mut deliveries = Vec::new();
        self.channel
            .process_events(readiness, token, |event, _| {
                if let ChannelEvent::Msg(delivery) = event {
                    deliveries.push(delivery);
                }
            })
            .map_err(|_| SessionError(SeatError::SessionLost))?;
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
