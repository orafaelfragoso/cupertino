//! The lock screen's buses: the session bus (`org.cupertino.Login1`, the notifications component's list) and the
//! system bus (logind, the session manager, the status items), each a connection Kit's loop ticks, their followers
//! futures of the loop that hand news to the program over its channel.

use super::{Heard, Message, Model};
use crate::screen::Result;
use crate::{control, lock_notices, login_manager, logind, status};
use cupertino_kit::bus::{self as kit_bus, Bus};
use cupertino_kit::core::Command;
use std::sync::Arc;

/// How `org.cupertino.Login1` came to be served.
#[derive(Clone, Debug)]
pub enum Served {
    /// Under its name.
    Named,
    /// Another process owns the name: served unnamed.
    Taken,
    Failed(String),
}

/// The streams the lock screen follows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stream {
    /// logind's `Lock` and `PrepareForSleep`.
    Logind,
    /// The session manager's `Unlock` and its name.
    Manager,
    /// What makes the status items out of date.
    Status,
    /// The notifications component's list.
    Notices,
}

/// A value read now and then, one reading at a time: a reading asked for while one runs is made once it is in.
#[derive(Debug, Default)]
pub struct Reading<T> {
    pub value: T,
    reading: bool,
    again: bool,
}

impl<T> Reading<T> {
    /// Whether a reading may start now; when one runs, it is asked for again once that one is in.
    fn start(&mut self) -> bool {
        if self.reading {
            self.again = true;
            false
        } else {
            self.reading = true;
            true
        }
    }

    /// A reading came in; whether another is due.
    pub fn read(&mut self) -> bool {
        self.reading = false;
        std::mem::take(&mut self.again)
    }
}

/// The program's first steps on the buses: a connection to each, and a second one to the session bus that asks who
/// calls `org.cupertino.Login1`.
pub fn start(_heard: &async_channel::Sender<Heard>) -> Vec<Command<Message>> {
    let mut commands = kit_bus::connect(
        Bus::Session,
        |c| Message::Session(c.ok()),
        Message::BusClosed("the session bus"),
    );
    commands.extend(kit_bus::connect(
        Bus::System,
        |c| Message::System(c.ok()),
        Message::BusClosed("the system bus"),
    ));
    commands.extend(kit_bus::connect(
        Bus::Session,
        |c| Message::Lookup(c.ok()),
        Message::BusClosed("the session bus connection that asks who calls"),
    ));
    commands
}

/// Follow a stream on the loop for as long as the program runs: `run` gets what hands a piece of news to the
/// program and returns why its stream ended, which the program then hears.
fn follow<F, Fut>(heard: &async_channel::Sender<Heard>, stream: Stream, run: F) -> Command<Message>
where
    F: FnOnce(Box<dyn Fn(Heard) + Send + Sync>) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = String> + Send + 'static,
{
    let heard = heard.clone();
    Command::Perform(Box::pin(async move {
        let send = heard.clone();
        let why = run(Box::new(move |news| drop(send.try_send(news)))).await;
        Message::Heard(Heard::Ended(stream, why))
    }))
}

/// What the end of `stream` means: logind's and the session manager's are what lock and unlock, so the process ends
/// and the server starts it again; the status items and the notifications are only shown, and stay as they were.
pub fn ended(stream: Stream, why: &str) -> Result<()> {
    match stream {
        Stream::Logind | Stream::Manager => Err(why.to_owned().into()),
        Stream::Status => {
            eprintln!("cupertino-login: the status items are no longer followed: {why}");
            Ok(())
        }
        Stream::Notices => {
            eprintln!(
                "cupertino-login: the lock screen's notifications are no longer followed: {why}"
            );
            Ok(())
        }
    }
}

impl Model {
    /// A message of the buses.
    pub(super) fn bus(
        &mut self,
        message: Message,
        commands: &mut Vec<Command<Message>>,
    ) -> Result<()> {
        match message {
            Message::Session(Some(session)) => {
                self.session = Some(session.clone());
                self.serve_control(commands);
                commands.push(follow(&self.heard.0, Stream::Notices, move |send| async move {
                    lock_notices::follow(&session, move || send(Heard::NoticesDue)).await
                }));
            }
            Message::Session(None) => return Err("no session bus connection".into()),
            Message::Lookup(Some(lookup)) => {
                self.lookup = Some(lookup);
                self.serve_control(commands);
            }
            Message::Lookup(None) => {
                return Err("no session bus connection to ask who calls".into())
            }
            Message::Served(Served::Named) => {}
            // Quitting would leave the session with no lock screen at all; instead this process runs on the server's
            // own connection, which carries the lock request, and on logind's signals.
            Message::Served(Served::Taken) => eprintln!(
                "cupertino-login: error: another process owns {}; a lock asked for over the session bus does not \
                 reach this process, the lock request of the server's connection still does",
                control::BUS_NAME
            ),
            Message::Served(Served::Failed(e)) => {
                return Err(format!("serving {}: {e}", control::BUS_NAME).into())
            }
            Message::System(Some(system)) => {
                self.system = Some(system.clone());
                commands.push(Command::Perform(Box::pin(async move {
                    Message::LoginSession(logind::session(&system).await.map_err(|e| e.to_string()))
                })));
            }
            // without a system bus there is no logind: with no session named either, there is nothing to lock
            Message::System(None) if std::env::var_os("XDG_SESSION_ID").is_none() => {
                return no_session(commands)
            }
            Message::System(None) => return Err("no system bus connection".into()),
            Message::LoginSession(Ok(None)) => return no_session(commands),
            Message::LoginSession(Ok(Some(path))) => self.session_found(path, commands),
            Message::LoginSession(Err(e)) => return Err(e.into()),
            Message::Inhibited(Ok(inhibitor)) => {
                // the lock came up while the inhibitor was being taken: sleep may go on
                if self.lock.is_none() {
                    self.inhibitor = Some(inhibitor);
                }
            }
            Message::Inhibited(Err(e)) => return Err(e.into()),
            Message::Announced(Ok(())) => {}
            // a session the manager did not start (one on a getty, for the tests) is not known to it
            Message::Announced(Err(e)) => eprintln!("cupertino-login: {e}"),
            Message::BusClosed(bus) => return Err(format!("{bus} closed").into()),
            _ => unreachable!("the update handles the other messages"),
        }
        Ok(())
    }

    /// Serve `org.cupertino.Login1` once both session bus connections are there, the one that serves it and the one
    /// that asks who calls; each comes once.
    fn serve_control(&self, commands: &mut Vec<Command<Message>>) {
        let (Some(served), Some(lookup)) = (self.session.clone(), self.lookup.clone()) else {
            return;
        };
        let lock = self.heard.0.clone();
        commands.push(Command::Perform(Box::pin(async move {
            let on_request = move |control::Request::Lock| drop(lock.try_send(Heard::Lock));
            Message::Served(match control::serve(&served, lookup, on_request).await {
                Ok(()) => Served::Named,
                Err(zbus::Error::NameTaken) => Served::Taken,
                Err(e) => Served::Failed(e.to_string()),
            })
        })));
    }

    /// logind knows this process's session at `path`: follow its requests and the session manager, hold sleep back
    /// while the session is unlocked, announce the session and read the status items.
    fn session_found(
        &mut self,
        path: zbus::zvariant::OwnedObjectPath,
        commands: &mut Vec<Command<Message>>,
    ) {
        let Some(system) = self.system.clone() else {
            return;
        };
        self.logind = true;
        commands.push(follow(&self.heard.0, Stream::Logind, {
            let system = system.clone();
            move |send| async move {
                let following = || send(Heard::Following(Stream::Logind));
                logind::follow(&system, path, following, |_| send(Heard::Lock)).await
            }
        }));
        commands.push(follow(&self.heard.0, Stream::Manager, {
            let system = system.clone();
            move |send| async move {
                let following = || send(Heard::Following(Stream::Manager));
                login_manager::follow(&system, following, |news| {
                    send(match news {
                        login_manager::News::Unlock => Heard::Unlock,
                        login_manager::News::Restarted => Heard::ManagerRestarted,
                    })
                })
                .await
            }
        }));
        commands.push(follow(&self.heard.0, Stream::Status, move |send| async move {
            status::follow(&system, move || send(Heard::StatusDue)).await
        }));
        if self.lock.is_none() {
            commands.extend(self.inhibit());
        }
        commands.extend(self.read_status());
    }

    /// `stream` is subscribed to. Once logind's requests and the session manager's news both are, the session is
    /// announced to the manager: the lock screen hears what either sends from then on.
    pub(super) fn following(&mut self, stream: Stream) -> Option<Command<Message>> {
        if !self.followed.contains(&stream) {
            self.followed.push(stream);
        }
        [Stream::Logind, Stream::Manager]
            .iter()
            .all(|s| self.followed.contains(s))
            .then(|| self.announce(" to the session manager"))
            .flatten()
    }

    /// Hold sleep back while the session is unlocked, so the system cannot sleep before the lock is up.
    pub(super) fn inhibit(&self) -> Option<Command<Message>> {
        let system = self.system.clone().filter(|_| self.logind)?;
        Some(Command::Perform(Box::pin(async move {
            Message::Inhibited(
                logind::inhibit_sleep(&system)
                    .await
                    .map(Arc::new)
                    .map_err(|e| e.to_string()),
            )
        })))
    }

    /// Tell the session manager this session's lock screen is up; `to` completes what the journal says.
    pub(super) fn announce(&self, to: &'static str) -> Option<Command<Message>> {
        let system = self.system.clone()?;
        Some(Command::Perform(Box::pin(async move {
            let announced = login_manager::session_ready(&system).await;
            if announced.is_ok() {
                eprintln!("cupertino-login: session announced{to}");
            }
            Message::Announced(announced.map_err(|e| format!("announcing the session: {e}")))
        })))
    }

    /// Read the status items on the I/O pool, one reading at a time.
    pub(super) fn read_status(&mut self) -> Option<Command<Message>> {
        let system = self.system.clone()?;
        if !self.status.start() {
            return None;
        }
        let input = self.settings.show_input_menu;
        Some(cupertino_kit::task::io(
            status::READ_DEADLINE,
            move || status::read(Some(system), None, input),
            Message::Status,
        ))
    }

    /// Ask the notifications component for the lock screen's list, while the session is locked, one reading at a
    /// time. A list that cannot be read is reported and shown as empty, so nothing stale stays.
    pub(super) fn read_notices(&mut self) -> Option<Command<Message>> {
        let session = self.session.clone().filter(|_| self.lock.is_some())?;
        if !self.notices.start() {
            return None;
        }
        Some(Command::Perform(Box::pin(async move {
            Message::Notices(lock_notices::fetch(&session).await.unwrap_or_else(|e| {
                eprintln!("cupertino-login: the lock screen's notifications: {e}");
                Vec::new()
            }))
        })))
    }
}

/// The process is in no login session (a container, CI): there is no session to lock, and the server does not start
/// it again.
fn no_session(commands: &mut Vec<Command<Message>>) -> Result<()> {
    eprintln!(
        "cupertino-login: this process is in no login session: there is no session to lock, exiting"
    );
    commands.extend(crate::screen::end(Ok(())));
    Ok(())
}
