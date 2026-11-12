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

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::{Arc, Condvar, Mutex};

    use super::*;
    use crate::operations::DropAction;

    fn copy(from: PathBuf, dest: PathBuf) -> Work {
        Work::Transfer {
            sources: vec![from],
            dest,
            action: DropAction::Copy,
        }
    }

    fn wait(jobs: &mut Jobs, id: u64) -> (Job, Outcome) {
        loop {
            let woken = jobs.running[0].transfer.woken.clone();
            woken.recv_blocking().unwrap();
            if let Polled::Ended(job, outcome, _) = jobs.poll(id) {
                return (job, outcome);
            }
        }
    }

    #[test]
    fn an_operation_ends_once_and_is_never_done_again() {
        let d = tempfile::tempdir().unwrap();
        let (dest, file) = (d.path().join("Documents"), d.path().join("a.txt"));
        std::fs::create_dir(&dest).unwrap();
        std::fs::write(&file, "a").unwrap();
        let mut jobs = Jobs::new(Duration::from_secs(60), None);
        let (job, commands) = jobs.start(copy(file, dest.clone()), 3, None);
        let id = job.transfer.id;
        assert_eq!((job.origin, commands.len()), (3, 2));
        let (_, outcome) = wait(&mut jobs, id);
        assert!(outcome.results[0].is_ok());
        assert!(
            matches!(jobs.poll(id), Polled::Gone),
            "it is handed over once"
        );
        assert_eq!(std::fs::read_dir(&dest).unwrap().count(), 1, "one copy");
        // a later report, stop or delay of it changes nothing
        jobs.stop(id);
        jobs.due(id);
        assert!(!jobs.visible());
        let (next, _) = jobs.start(copy(d.path().join("a.txt"), dest), 3, None);
        assert_ne!(next.transfer.id, id, "ids are never reused");
    }

    #[test]
    fn the_progress_window_shows_after_the_delay_hides_without_stopping_and_closes_when_done() {
        let d = tempfile::tempdir().unwrap();
        let (dest, file) = (d.path().join("Documents"), d.path().join("a.txt"));
        std::fs::create_dir(&dest).unwrap();
        std::fs::write(&file, vec![1u8; 3 << 20]).unwrap();
        // the copy waits after its first chunk until the test lets it go
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let held = Arc::clone(&gate);
        let observer: Observer = Arc::new(move |p| {
            if p.bytes > 0 && p.bytes < p.total_bytes {
                let (open, opened) = &*held;
                let open = open.lock().unwrap();
                let _open = opened
                    .wait_timeout_while(open, Duration::from_secs(10), |open| !*open)
                    .unwrap();
            }
        });
        let mut jobs = Jobs::new(Duration::from_secs(60), Some(observer));
        let id = jobs.start(copy(file, dest.clone()), 0, None).0.transfer.id;
        assert!(!jobs.visible(), "not before the delay");
        assert!(!jobs.running[0].lasted);
        jobs.show();
        assert!(jobs.visible(), "Show Progress shows it at once");
        jobs.hide();
        assert!(!jobs.visible());
        jobs.due(id);
        assert!(jobs.visible(), "the delay shows it while it runs");
        assert!(jobs.running[0].lasted, "it ran for the delay");
        jobs.hide();
        assert!(
            !jobs.running[0].transfer.cancel.is_cancelled(),
            "hiding stops nothing"
        );
        let (open, opened) = &*gate;
        *open.lock().unwrap() = true;
        opened.notify_all();
        let (_, outcome) = wait(&mut jobs, id);
        assert!(!outcome.stopped());
        assert_eq!(std::fs::read(dest.join("a.txt")).unwrap().len(), 3 << 20);

        let failure = Failure {
            id: 9,
            title: "Could not copy “a.txt” to “Documents”".into(),
            reason: "Permission denied.".into(),
        };
        jobs.failed(failure.clone());
        jobs.failed(failure);
        assert!(jobs.visible(), "a failure opens it");
        assert_eq!(jobs.failures.len(), 1, "listed once");
        jobs.hide();
        assert!(jobs.is_empty(), "closing it forgets the failure");
    }
}
