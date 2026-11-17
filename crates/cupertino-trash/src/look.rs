//! Whether the trash holds anything, for a program's own thread: the home trash is read where the question is asked,
//! since it is one local folder; the trash folders of the mounted volumes, which can take long on a slow volume and
//! for ever on one that went away, are only ever read on a thread of their own, and what they held at the last look
//! is used meanwhile.

use crate::Trash;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// The trash as last looked at. Nothing here waits for a volume: [`Look::full`] and [`Look::volumes_hold`] start a
/// look at the volumes unless one is still out, so the looks cannot pile up behind a volume that does not answer, and
/// `changed` is called from the look's thread when the volumes' answer differs from the one before.
pub struct Look {
    trash: Trash,
    /// Whether a mounted volume's trash held anything at the last look that came back; `false` until the first.
    volumes: AtomicBool,
    /// Whether a look at the volumes is out.
    looking: AtomicBool,
    changed: Box<dyn Fn() + Send + Sync>,
}

impl Look {
    /// A look at `trash` that calls `changed` when the volumes' part of the answer changed.
    pub fn new(trash: Trash, changed: impl Fn() + Send + Sync + 'static) -> Arc<Self> {
        Arc::new(Look {
            trash,
            volumes: AtomicBool::new(false),
            looking: AtomicBool::new(false),
            changed: Box::new(changed),
        })
    }

    /// The trash this looks at.
    pub fn trash(&self) -> &Trash {
        &self.trash
    }

    /// Look at the volumes on a thread of its own, unless a look is still out.
    pub fn look(self: &Arc<Self>) {
        if self.looking.swap(true, Ordering::AcqRel) {
            return;
        }
        let look = self.clone();
        let started = std::thread::Builder::new()
            .name("trash-look".into())
            .spawn(move || {
                let full = look.trash.volumes_hold_items();
                let was = look.volumes.swap(full, Ordering::AcqRel);
                look.looking.store(false, Ordering::Release);
                if was != full {
                    (look.changed)();
                }
            });
        if started.is_err() {
            self.looking.store(false, Ordering::Release);
        }
    }

    /// Whether the trash holds anything: the home trash as it is now, or a mounted volume's as last looked. Starts
    /// the next look at the volumes.
    pub fn full(self: &Arc<Self>) -> bool {
        self.look();
        self.trash.home_holds_items() || self.volumes_held()
    }

    /// Whether a mounted volume's trash holds anything, as last looked. Starts the next look at the volumes.
    pub fn volumes_hold(self: &Arc<Self>) -> bool {
        self.look();
        self.volumes_held()
    }

    /// Whether a mounted volume's trash held anything at the last look, without starting one.
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
