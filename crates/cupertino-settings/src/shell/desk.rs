//! Work off the window's thread: the blocking reads and calls of [`Services`], and the other work that waits (a file,
//! a folder, a program), run as tasks of Kit's I/O pool. The services are one value that a task holds while it runs:
//! the desk keeps the tasks that wait for them in order, so the calls and reads reach the services one at a time and
//! in the order the window asked, as they would if the window made them itself, and nothing the window does waits for
//! them. What a task found comes back to the window loop as a [`Message::Done`] that applies it to the [`State`].

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use cupertino_kit::core::Command;
use cupertino_kit::task;

use super::state::State;
use super::Message;
use crate::services::Services;

/// How long a task of the services should take: the readings of a pane, or a call, each a few bus round trips; the
/// bus gives up on a call after 25 s, so a task still running past this has met a service that does not answer.
pub(crate) const SERVICES_DEADLINE: Duration = Duration::from_secs(5);
/// How long a change that waits for a device should take: joining a network or connecting, disconnecting or forgetting
/// a device waits for the radio, and a display's mode for the panel.
pub(crate) const DEVICE_DEADLINE: Duration = Duration::from_secs(30);
/// How long a pairing should take: it waits for the person to answer the pairing sheet.
pub(crate) const PAIRING_DEADLINE: Duration = Duration::from_secs(120);

/// What a finished task does to the window's state, on the window's thread.
pub(crate) type Apply = Box<dyn FnOnce(&mut State) + Send>;

/// A finished task's [`Apply`], as a message: it is taken once, by the window loop that hears it.
#[derive(Clone)]
pub struct Done(Arc<Mutex<Option<Apply>>>);

impl Done {
    pub(crate) fn new(apply: impl FnOnce(&mut State) + Send + 'static) -> Done {
        Done(Arc::new(Mutex::new(Some(Box::new(apply)))))
    }

    /// The task's work for the window, once.
    pub(crate) fn take(&self) -> Option<Apply> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).take()
    }
}

impl PartialEq for Done {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl std::fmt::Debug for Done {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Done")
    }
}

/// A task of the services: its blocking work with the services in hand, and what it found for the window.
type Job = Box<dyn FnOnce(&mut Services) -> Apply + Send>;

/// A task to start: the window loop makes its command when it takes it, on its own thread, so a task asked for before
/// the loop runs (the window's first readings) starts with it.
type Start = Box<dyn FnOnce() -> Command<Message> + Send>;

/// The services and the tasks waiting for them, and the tasks started since the window loop last took them.
pub(crate) struct Desk {
    /// The services while no task holds them.
    services: Option<Box<Services>>,
    waiting: VecDeque<Job>,
    started: Vec<Start>,
}

impl Desk {
    pub(crate) fn new(services: Services) -> Desk {
        Desk {
            services: Some(Box::new(services)),
            waiting: VecDeque::new(),
            started: Vec::new(),
        }
    }

    /// Run `work` with the services on the I/O pool once the tasks asked for before it are done; `apply` gets what it
    /// found on the window's thread.
    pub(crate) fn serve<T: Send + 'static>(
        &mut self,
        work: impl FnOnce(&mut Services) -> T + Send + 'static,
        apply: impl FnOnce(&mut State, T) + Send + 'static,
    ) {
        self.waiting.push_back(Box::new(move |services| {
            let found = work(services);
            Box::new(move |state: &mut State| apply(state, found))
        }));
        self.next();
    }

    /// Hand the services to the first waiting task, if they are free.
    fn next(&mut self) {
        if self.services.is_none() {
            return;
        }
        let Some(job) = self.waiting.pop_front() else {
            return;
        };
        let mut services = self.services.take().expect("checked above");
        self.started.push(Box::new(move || {
            task::io(
                SERVICES_DEADLINE,
                move || {
                    let apply = job(&mut services);
                    (services, apply)
                },
                |(services, apply)| {
                    Message::Done(Done::new(move |state: &mut State| {
                        state.desk.services = Some(services);
                        state.desk.next();
                        apply(state);
                    }))
                },
            )
        }));
    }

    /// Run `work`, which needs no services (a file, a folder, a program, a call on a connection of its own), on the I/O
    /// pool at once; `deadline` is how long it should take. `apply` gets what it found on the window's thread.
    pub(crate) fn off<T: Send + 'static>(
        &mut self,
        deadline: Duration,
        work: impl FnOnce() -> T + Send + 'static,
        apply: impl FnOnce(&mut State, T) + Send + 'static,
    ) {
        self.started.push(Box::new(move || {
            task::io(deadline, work, move |found| {
                Message::Done(Done::new(move |state: &mut State| apply(state, found)))
            })
        }));
    }

    /// Run `task`, a future of the window's loop (a watch, a reader), with the tasks.
    pub(crate) fn run(
        &mut self,
        task: impl std::future::Future<Output = Message> + Send + 'static,
    ) {
        self.started
            .push(Box::new(move || Command::Perform(Box::pin(task))));
    }

    /// The tasks asked for since the last call, for the window loop to run.
    pub(crate) fn commands(&mut self) -> Vec<Command<Message>> {
        self.started.drain(..).map(|start| start()).collect()
    }
}
