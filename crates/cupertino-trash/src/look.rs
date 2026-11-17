//! Whether the trash holds anything, for a program's UI thread: the home trash is read where the question is asked,
//! since it is one local folder; the trash folders of the mounted volumes, which can take long on a slow volume and
//! for ever on one that went away, are read only by a look the program runs as a task ([`Look::next`]), and what they
//! held at the last look that came back is used meanwhile.

use crate::Trash;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// How long a look at the volumes ([`Look::next`]) should take: the deadline a program gives the task that runs it.
/// Volumes that answer do well within it, and a few out on another look are each given
/// [`VOLUME_ANSWERS_WITHIN`](crate::VOLUME_ANSWERS_WITHIN); a look past it is held up by a volume that does not answer.
pub const LOOK_DEADLINE: Duration = Duration::from_secs(5);

/// The trash as last looked at. Nothing here waits for a volume: [`Look::full`] and [`Look::volumes_hold`] ask for
/// the next look at the volumes, and [`Look::next`] hands it out unless one is still out, so the looks cannot pile up
/// behind a volume that does not answer.
pub struct Look {
    trash: Trash,
    /// Whether a mounted volume's trash held anything at the last look that came back; `false` until the first.
    volumes: AtomicBool,
    /// Whether a look at the volumes was asked for since the last one was handed out.
    asked: AtomicBool,
    /// Whether a look at the volumes is out.
    looking: AtomicBool,
}

impl Look {
    /// A look at `trash`.
    pub fn new(trash: Trash) -> Arc<Self> {
        Arc::new(Look {
            trash,
            volumes: AtomicBool::new(false),
            asked: AtomicBool::new(false),
            looking: AtomicBool::new(false),
        })
    }

    /// The trash this looks at.
    pub fn trash(&self) -> &Trash {
        &self.trash
    }

    /// Ask for a look at the volumes, which [`Look::next`] hands out.
    pub fn look(&self) {
        self.asked.store(true, Ordering::Release);
    }

    /// The look at the volumes asked for since the last one was handed out, unless one is still out: blocking work
    /// for a task, which stores what the volumes hold and says whether that differs from the last look's answer.
    /// A look asked for while one is out is handed out by the first call after that one came back.
    pub fn next(self: &Arc<Self>) -> Option<impl FnOnce() -> bool + Send + 'static> {
        if !self.asked.load(Ordering::Acquire) || self.looking.swap(true, Ordering::AcqRel) {
            return None;
        }
        self.asked.store(false, Ordering::Release);
        let look = Arc::clone(self);
        Some(move || {
            let full = look.trash.volumes_hold_items();
            let was = look.volumes.swap(full, Ordering::AcqRel);
            look.looking.store(false, Ordering::Release);
            was != full
        })
    }

    /// Whether the trash holds anything: the home trash as it is now, or a mounted volume's as last looked. Asks for
    /// the next look at the volumes.
    pub fn full(&self) -> bool {
        self.look();
        self.trash.home_holds_items() || self.volumes_held()
    }

    /// Whether a mounted volume's trash holds anything, as last looked. Asks for the next look at the volumes.
    pub fn volumes_hold(&self) -> bool {
        self.look();
        self.volumes_held()
    }

    /// Whether a mounted volume's trash held anything at the last look, without asking for one.
    pub fn volumes_held(&self) -> bool {
        self.volumes.load(Ordering::Acquire)
    }

    /// Whether a look at the volumes is out.
    pub fn looking(&self) -> bool {
        self.looking.load(Ordering::Acquire)
    }

    /// Take what an emptying left on the volumes, known to the thread that emptied them, without waiting for the
    /// next look.
    pub fn set_volumes_held(&self, full: bool) {
        self.volumes.store(full, Ordering::Release);
    }
}
