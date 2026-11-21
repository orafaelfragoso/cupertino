//! Files on the session bus: the one connection a Files makes there (`cupertino_kit::bus`, ticked by the UI loop)
//! posts its notices and serves its names ([`crate::instance`]). A launch first claims [`instance::BUS_NAME`]; when
//! another Files owns it, the launch hands its request over and exits. A hand-over that is not answered within
//! [`instance::FORWARD_TIMEOUT`] fails the launch unless the owner left meanwhile; while the owner quits and has not
//! let go of the name, the launch tries again after [`instance::RETRY_PAUSE`], a scheduled wake, for up to
//! [`instance::HANDOVER_WINDOW`] more. The first window opens once the launch serves the name, or finds no bus.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

use cupertino_kit::files::notice::Notices;
use cupertino_kit::prelude::*;
use zbus::Connection;

use super::{remote, window_key, woken, Message, Model};
use crate::instance::{self, Handover, Request, Service};

/// What Files does on the buses ([`super::Start::with_buses`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Buses {
    /// A launch: claim [`instance::BUS_NAME`], or hand the request to the Files that owns it and exit.
    Launch(Request),
    /// Started by the session bus for a call: serve the names, and exit at once when another process owns them.
    Service,
}

/// How Files ended ([`super::run`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ended {
    /// Its windows closed and its work ended, or it handed its launch over.
    Closed,
    /// The launch could not be served or handed over, for this reason.
    Refused(String),
}

/// A value a message hands over once: a message may be cloned, and the first that is handled takes what it carries.
pub struct Handed<T>(Arc<Mutex<Option<T>>>);

impl<T> Handed<T> {
    pub fn new(value: T) -> Handed<T> {
        Handed(Arc::new(Mutex::new(Some(value))))
    }

    /// The value, unless it was taken.
    pub fn take(&self) -> Option<T> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).take()
    }
}

impl<T> Clone for Handed<T> {
    fn clone(&self) -> Self {
        Handed(Arc::clone(&self.0))
    }
}

impl<T> std::fmt::Debug for Handed<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Handed")
    }
}

/// Two messages are the same when they hand over the same value.
impl<T> PartialEq for Handed<T> {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

/// What a launch hears, by the number of the try it belongs to: a late answer to an earlier try is dropped.
#[derive(Clone, Debug, PartialEq)]
pub enum LaunchEvent {
    /// The try to claim the name ended.
    Claimed(u64, Handed<zbus::Result<Service>>),
    /// The hand-over ended.
    Forwarded(u64, Handed<Result<(), Handover>>),
    /// The hand-over was not answered within [`instance::FORWARD_TIMEOUT`].
    Unanswered(u64),
    /// The pause before the next try passed.
    Again(u64),
}

/// A launch under way on the session connection: what it hands over (nothing for a Files the bus started), and until
/// when it tries.
pub(super) struct Launching {
    connection: Connection,
    request: Option<Request>,
    token: Option<String>,
    until: Instant,
    attempt: u64,
}

thread_local! {
    /// How the Files of this thread ended, for [`super::run`].
    static ENDED: std::cell::RefCell<Ended> = const { std::cell::RefCell::new(Ended::Closed) };
}

/// How the Files of this thread ended, read once its loop returned.
pub(super) fn ended() -> Ended {
    ENDED.with(|e| e.replace(Ended::Closed))
}

impl Model {
    /// The session bus connected, or could not be: the notices are posted on it, and the launch claims the name. With
    /// no session bus the notices are logged and the first window opens; a Files the bus started fails.
    pub(super) fn session_bus(
        &mut self,
        connection: zbus::Result<Connection>,
    ) -> Vec<Command<Message>> {
        let buses = self.buses.take();
        let connection = match connection {
            Ok(connection) => connection,
            Err(e) => {
                return match buses {
                    Some(Buses::Service) => {
                        self.refuse(format!("cannot claim {}: {e}", instance::BUS_NAME))
                    }
                    _ => {
                        eprintln!(
                            "cupertino-files: a later launch cannot reach this one: cannot claim {}: {e}",
                            instance::BUS_NAME
                        );
                        self.launched()
                    }
                };
            }
        };
        let (notices, posting) = Notices::on(connection.clone());
        self.session.notices = notices;
        let mut commands = vec![Command::Perform(Box::pin(async move {
            posting.await;
            Message::Worked
        }))];
        commands.extend(self.session.notice_clicks());
        let request = match buses {
            Some(Buses::Launch(request)) => Some(request),
            Some(Buses::Service) => None,
            None => return commands,
        };
        self.launching = Some(Launching {
            connection,
            request,
            token: self.activation.clone(),
            until: Instant::now() + instance::FORWARD_TIMEOUT + instance::HANDOVER_WINDOW,
            attempt: 0,
        });
        commands.extend(self.claim());
        commands
    }

    /// The next try to claim the name.
    fn claim(&mut self) -> Option<Command<Message>> {
        let launching = self.launching.as_mut()?;
        launching.attempt += 1;
        let (connection, attempt) = (launching.connection.clone(), launching.attempt);
        Some(Command::Perform(Box::pin(async move {
            let served = Service::serve(&connection).await;
            Message::Launch(LaunchEvent::Claimed(attempt, Handed::new(served)))
        })))
    }

    /// Follow the launch: serve the name once claimed, hand the request over while another Files owns it, try again
    /// while that one quits, and exit once it took the request.
    pub(super) fn launch_event(&mut self, event: LaunchEvent) -> Vec<Command<Message>> {
        let Some(launching) = self.launching.as_mut() else {
            return Vec::new();
        };
        match event {
            LaunchEvent::Claimed(attempt, served) if attempt == launching.attempt => {
                match served.take() {
                    Some(Ok(service)) => {
                        self.launching = None;
                        let mut commands = vec![woken(service.woken(), Message::Remote)];
                        self.session.instance = Some(service);
                        commands.extend(self.launched());
                        commands
                    }
                    Some(Err(zbus::Error::NameTaken)) => {
                        let Some(request) = launching.request.clone() else {
                            // the Files that owns the names takes the call the bus started this one for
                            self.launching = None;
                            return vec![Command::Exit];
                        };
                        let (connection, token) =
                            (launching.connection.clone(), launching.token.clone());
                        vec![
                            Command::Perform(Box::pin(async move {
                                let forwarded =
                                    instance::forward(&connection, &request, token.as_deref())
                                        .await;
                                Message::Launch(LaunchEvent::Forwarded(
                                    attempt,
                                    Handed::new(forwarded),
                                ))
                            })),
                            Command::After {
                                delay: instance::FORWARD_TIMEOUT,
                                message: Message::Launch(LaunchEvent::Unanswered(attempt)),
                            },
                        ]
                    }
                    Some(Err(e)) => match launching.request {
                        None => self.refuse(format!("cannot claim {}: {e}", instance::BUS_NAME)),
                        Some(_) => {
                            eprintln!(
                                "cupertino-files: a later launch cannot reach this one: cannot claim {}: {e}",
                                instance::BUS_NAME
                            );
                            self.launching = None;
                            self.launched()
                        }
                    },
                    None => Vec::new(),
                }
            }
            LaunchEvent::Forwarded(attempt, forwarded) if attempt == launching.attempt => {
                match forwarded.take() {
                    Some(Ok(())) => {
                        self.launching = None;
                        vec![Command::Exit]
                    }
                    Some(Err(Handover::Retry(why))) => self.retry(why),
                    Some(Err(Handover::Failed(why))) => self.refuse(why),
                    None => Vec::new(),
                }
            }
            LaunchEvent::Unanswered(attempt) if attempt == launching.attempt => {
                // the late answer of this try is dropped: what the owner does now decides
                launching.attempt += 1;
                let (connection, attempt) = (launching.connection.clone(), launching.attempt);
                vec![Command::Perform(Box::pin(async move {
                    let handover = instance::unanswered(&connection).await;
                    Message::Launch(LaunchEvent::Forwarded(attempt, Handed::new(Err(handover))))
                }))]
            }
            LaunchEvent::Again(attempt) if attempt == launching.attempt => {
                self.claim().into_iter().collect()
            }
            _ => Vec::new(),
        }
    }

    /// The owner left or is quitting: claim the name again after a pause, unless the launch has tried for too long.
    fn retry(&mut self, why: String) -> Vec<Command<Message>> {
        let Some(launching) = self.launching.as_mut() else {
            return Vec::new();
        };
        if Instant::now() >= launching.until {
            return self.refuse(format!("the running Files did not take the launch: {why}"));
        }
        // a try of its own: the hand-over's pending Unanswered no longer matches
        launching.attempt += 1;
        vec![Command::After {
            delay: instance::RETRY_PAUSE,
            message: Message::Launch(LaunchEvent::Again(launching.attempt)),
        }]
    }

    /// The launch failed: Files exits, and [`super::run`] says why.
    fn refuse(&mut self, why: String) -> Vec<Command<Message>> {
        self.launching = None;
        self.first = None;
        ENDED.with(|e| *e.borrow_mut() = Ended::Refused(why));
        vec![Command::Exit]
    }

    /// The launch serves the name, or runs alone: the first window opens, raised with the launch's token.
    pub(super) fn launched(&mut self) -> Vec<Command<Message>> {
        let Some((browser, woken, wake)) = self.first.take() else {
            return Vec::new();
        };
        let first = self.next_window;
        let mut commands = vec![self.add_window(browser, woken, wake)];
        if let Some(token) = self.activation.take() {
            commands.push(remote::raise(window_key(first), Some(token)));
        }
        commands
    }
}
