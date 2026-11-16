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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::sync::atomic::AtomicUsize;
    use std::sync::{mpsc, Condvar, Mutex};
    use std::time::{Duration, Instant};

    /// A volume that answers no look until it is opened, counting the looks that reached it.
    #[derive(Default)]
    struct Shut {
        open: Mutex<bool>,
        opened: Condvar,
        looks: AtomicUsize,
    }

    impl Shut {
        fn on(self: &Arc<Self>, trash: Trash) -> Trash {
            let shut = self.clone();
            trash.with_volume_look(move |_| {
                shut.looks.fetch_add(1, Ordering::Relaxed);
                let mut open = shut.open.lock().unwrap();
                while !*open {
                    open = shut.opened.wait(open).unwrap();
                }
            })
        }

        fn open(&self) {
            *self.open.lock().unwrap() = true;
            self.opened.notify_all();
        }
    }

    /// A volume at `dir` whose trash of the user holds one item, and the home trash beside it.
    fn volume(dir: &Path) -> Trash {
        let uid = unsafe { libc::getuid() };
        let usb = dir.join("usb");
        std::fs::create_dir_all(usb.join(format!(".Trash-{uid}/files"))).unwrap();
        std::fs::write(usb.join(format!(".Trash-{uid}/files/item")), b"1").unwrap();
        Trash::with_home(dir.join("share/Trash")).with_mounts(vec![usb])
    }

    /// Ask `look` as a program does at each check until it says `full`, for two seconds at most.
    fn until(look: &Arc<Look>, full: bool) -> bool {
        let began = Instant::now();
        while look.full() != full {
            if began.elapsed() > Duration::from_secs(2) {
                return false;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        true
    }

    #[test]
    fn the_first_look_does_not_wait_for_a_volume_and_shows_it_once_it_answers() {
        let d = tempfile::tempdir().unwrap();
        let shut = Arc::new(Shut::default());
        // the volume is given far longer than the test may take: only a look that waits for it would see it
        let trash = shut.on(volume(d.path()).with_volume_wait(Duration::from_secs(600)));
        let (tell, told) = mpsc::channel();
        let look = Look::new(trash.clone(), move || {
            let _ = tell.send(());
        });

        let began = Instant::now();
        assert!(!look.full(), "the home trash is empty, the volume silent");
        assert!(
            began.elapsed() < Duration::from_millis(500),
            "the first look came back at once: {:?}",
            began.elapsed()
        );
        // the home trash shows while the volume is silent, and the checks start no second look
        std::fs::create_dir_all(trash.home_trash().join("files")).unwrap();
        std::fs::write(trash.home_trash().join("files/a"), b"1").unwrap();
        for _ in 0..50 {
            assert!(look.full());
        }
        std::fs::remove_file(trash.home_trash().join("files/a")).unwrap();
        assert!(!look.full());
        assert!(look.looking());
        assert!(told.try_recv().is_err(), "nothing changed on the volumes");

        shut.open();
        told.recv_timeout(Duration::from_secs(2))
            .expect("the look that was out tells of the volume's trash");
        assert!(look.full(), "the volume's trash shows");
        assert_eq!(shut.looks.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn looks_go_on_without_a_volume_that_stays_silent_and_pick_it_up_when_it_answers() {
        let d = tempfile::tempdir().unwrap();
        let shut = Arc::new(Shut::default());
        let trash = shut.on(volume(d.path()).with_volume_wait(Duration::from_millis(30)));
        let (tell, told) = mpsc::channel();
        let look = Look::new(trash, move || {
            let _ = tell.send(());
        });

        assert!(!look.full());
        // the look gives the volume up after its time, and the checks after it start looks that end at once
        let began = Instant::now();
        while look.looking() {
            assert!(began.elapsed() < Duration::from_secs(2), "the look ended");
            std::thread::sleep(Duration::from_millis(2));
        }
        for _ in 0..20 {
            assert!(!look.full());
        }
        assert_eq!(
            shut.looks.load(Ordering::Relaxed),
            1,
            "one look is out on the silent volume, however many checks there were"
        );
        assert!(told.try_recv().is_err());

        shut.open();
        assert!(until(&look, true), "a later look sees the volume's trash");
        told.recv_timeout(Duration::from_secs(2))
            .expect("and tells of the change");
    }
}
