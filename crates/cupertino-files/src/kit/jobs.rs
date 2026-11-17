//! The file operations of the process, apart from its windows: each runs on a worker ([`Transfer`]) until it ends or
//! its Stop button stops it. Closing the window that started one leaves it running, and Files does not exit while one
//! runs. The progress window ([`super::progress`]) lists them; it opens by itself once one has run for
//! [`PROGRESS_WINDOW_DELAY`], so a quick operation never shows it, and its Close button only hides it. A failure is
//! listed there until it is closed, and opens it. An operation that ran for the delay has [`Job::lasted`]: its end is
//! worth a notification.

use std::time::Duration;

use cupertino_kit::layout::ScrollState;
use cupertino_kit::platform::DropToken;
use cupertino_kit::prelude::*;

use super::transfer::{Observer, Outcome, Transfer, Work};
use super::Message;

/// How long a file operation runs before the progress window opens by itself (approximate).
pub const PROGRESS_WINDOW_DELAY: Duration = Duration::from_secs(1);

/// What a file operation, or the progress window, asks of the application.
#[derive(Clone, Debug, PartialEq)]
pub enum JobEvent {
    /// The operation with this id has progress or its outcome waiting.
    Reported(u64),
    /// Stop the operation with this id: its Stop button, in a browser window or the progress window.
    Stop(u64),
    /// The operation with this id has run for the delay: show the progress window if it still runs.
    Due(u64),
    /// Show the progress window: the menu bar's Show Progress, or a click on a window's progress row.
    Show,
    /// Hide the progress window: its Close button. Nothing stops.
    Hide,
    /// The progress window's list scrolled.
    Scroll(ScrollState),
}

/// One running operation.
pub(super) struct Job {
    pub transfer: Transfer,
    /// The browser window that started it, which shows its row while it is open.
    pub origin: u64,
    /// The drop it does, whose source hears what was done only once it ends.
    pub drop: Option<DropToken>,
    /// It ran for the delay: long enough that a person may have turned to something else.
    pub lasted: bool,
}

/// An operation that ended with a failure, listed in the progress window until it is closed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Failure {
    pub id: u64,
    /// What it did, as "Could not copy “a.txt” to “Documents”".
    pub title: String,
    /// The first failure, as a sentence.
    pub reason: String,
}

/// What [`Jobs::poll`] found.
pub(super) enum Polled {
    /// No operation has this id: it ended already.
    Gone,
    /// Still running: wait for its next report with this.
    Running(Command<Message>),
    /// It ended: it is no longer listed, and its outcome is handed over this once. Whether the progress window was
    /// shown as it ended.
    Ended(Job, Outcome, bool),
}

/// The running operations and the progress window.
pub(super) struct Jobs {
    /// Oldest first.
    pub running: Vec<Job>,
    pub failures: Vec<Failure>,
    /// The progress window is shown, while it lists something.
    shown: bool,
    pub scroll: ScrollState,
    next: u64,
    delay: Duration,
    observer: Option<Observer>,
}

impl Jobs {
    /// Every operation's worker calls `observer`; the progress window opens after `delay`.
    pub fn new(delay: Duration, observer: Option<Observer>) -> Jobs {
        Jobs {
            running: Vec::new(),
            failures: Vec::new(),
            shown: false,
            scroll: ScrollState {
                offset: cupertino_kit::geometry::Point::ZERO,
                content: cupertino_kit::geometry::Size::ZERO,
                viewport: cupertino_kit::geometry::Size::ZERO,
            },
            next: 0,
            delay,
            observer,
        }
    }

    /// Start `work` for the window `origin`, for `drop` if it is one. What waits for its reports and for its delay.
    pub fn start(
        &mut self,
        work: Work,
        origin: u64,
        drop: Option<DropToken>,
    ) -> (&Job, Vec<Command<Message>>) {
        let id = self.next;
        self.next += 1;
        let transfer = Transfer::start(id, work, self.observer.clone());
        let commands = vec![
            reported(&transfer),
            Command::After {
                delay: self.delay,
                message: Message::Job(JobEvent::Due(id)),
            },
        ];
        self.running.push(Job {
            transfer,
            origin,
            drop,
            lasted: false,
        });
        let job = self.running.last().expect("just pushed");
        (job, commands)
    }

    /// Take what the operation `id` reported.
    pub fn poll(&mut self, id: u64) -> Polled {
        let Some(at) = self.running.iter().position(|j| j.transfer.id == id) else {
            return Polled::Gone;
        };
        match self.running[at].transfer.poll() {
            None => Polled::Running(reported(&self.running[at].transfer)),
            Some(outcome) => {
                let seen = self.visible();
                let job = self.running.remove(at);
                self.close_when_empty();
                Polled::Ended(job, outcome, seen)
            }
        }
    }

    /// Stop the operation `id`; one that ended already is left alone.
    pub fn stop(&self, id: u64) {
        if let Some(job) = self.running.iter().find(|j| j.transfer.id == id) {
            job.transfer.stop();
        }
    }

    /// The operation `id` ran for the delay: the progress window opens while it still runs.
    pub fn due(&mut self, id: u64) {
        if let Some(job) = self.running.iter_mut().find(|j| j.transfer.id == id) {
            job.lasted = true;
            self.shown = true;
        }
    }

    /// List a failure, which opens the progress window; one listed already is listed once.
    pub fn failed(&mut self, failure: Failure) {
        if !self.failures.iter().any(|f| f.id == failure.id) {
            self.failures.push(failure);
        }
        self.shown = true;
    }

    /// Show the progress window, when it has something to list.
    pub fn show(&mut self) {
        self.shown = !self.is_empty();
    }

    /// Hide the progress window, forgetting the failures it listed. The operations go on.
    pub fn hide(&mut self) {
        self.shown = false;
        self.failures.clear();
    }

    /// Whether the progress window is shown.
    pub fn visible(&self) -> bool {
        self.shown && !self.is_empty()
    }

    /// Whether nothing runs and no failure is listed: the progress window has nothing to show.
    pub fn is_empty(&self) -> bool {
        self.running.is_empty() && self.failures.is_empty()
    }

    /// The progress window closes by itself once it lists nothing.
    fn close_when_empty(&mut self) {
        if self.is_empty() {
            self.shown = false;
        }
    }
}

/// What wakes when `transfer` reports.
fn reported(transfer: &Transfer) -> Command<Message> {
    super::woken(
        &transfer.woken,
        Message::Job(JobEvent::Reported(transfer.id)),
    )
}
