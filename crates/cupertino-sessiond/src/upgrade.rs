//! Handing the console over to a new version of the manager. A package upgrade replaces the manager's file while it runs;
//! the running manager goes on with its own image, and the login screen it starts next (another program, already new)
//! would talk to an old manager. So once the new file has been in place for [`SETTLE`] and no user session runs, the
//! manager stops the login screen's session and ends; the service manager starts the new version, which starts a new login
//! screen: the manager, the login screen and its compositor are one version again. A user session is never ended for it:
//! the manager waits until the last one has ended.

use std::io;
use std::os::fd::AsFd;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use rustix::event::{poll, PollFd, PollFlags, Timespec};
use rustix::process::{pidfd_send_signal, Signal};

/// How often the manager looks at its file.
pub const CHECK_INTERVAL: Duration = Duration::from_secs(10);
/// How long a new file must stay in place before the manager hands over to it, so the rest of the upgrade (the login
/// screen's programs, written after this one) is in place too (approximate, ours: a package writes its files in seconds).
pub const SETTLE: Duration = Duration::from_secs(10);

/// What the kernel appends to the path of an image whose file is gone.
const DELETED: &str = " (deleted)";

/// A file by its device and inode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FileId {
    dev: u64,
    ino: u64,
}

impl FileId {
    fn of(metadata: &std::fs::Metadata) -> FileId {
        FileId {
            dev: metadata.dev(),
            ino: metadata.ino(),
        }
    }
}

/// What is at the path the running program was started from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Installed {
    /// The running program's own file.
    Same,
    /// Another file: a new version.
    Replaced,
    /// Nothing (the package was removed), or nothing that can be read: there is no new version to hand over to.
    Missing,
}

/// What the manager does about its file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    Keep,
    /// Stop the login screen and end, so the new version starts.
    HandOver,
}

/// Hand over only to a new file that has `settled` in place, and only while no user session runs (`user_session`: a
/// session, or an attempt or a switch that leads to one), since a new manager would have to adopt it.
pub fn decide(installed: Installed, settled: bool, user_session: bool) -> Decision {
    match installed {
        Installed::Replaced if settled && !user_session => Decision::HandOver,
        _ => Decision::Keep,
    }
}

/// The running program's file and whether a new one replaced it.
#[derive(Debug)]
pub struct Watch {
    path: PathBuf,
    image: FileId,
    /// The new file seen at the path, and when it was first seen there.
    replacement: Option<(FileId, Instant)>,
}

impl Watch {
    /// The image of this process and the path it was started from.
    pub fn this_process() -> io::Result<Watch> {
        let link = std::fs::read_link("/proc/self/exe")?;
        let text = link.to_string_lossy();
        let path = PathBuf::from(text.strip_suffix(DELETED).unwrap_or(&text));
        let image = FileId::of(&std::fs::metadata("/proc/self/exe")?);
        Ok(Watch {
            path,
            image,
            replacement: None,
        })
    }

    /// A program started from `path`, whose file is the one there now.
    pub fn of_file(path: &Path) -> io::Result<Watch> {
        Ok(Watch {
            path: path.to_owned(),
            image: FileId::of(&std::fs::metadata(path)?),
            replacement: None,
        })
    }

    /// The path the program was started from.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// What is at the path now, and whether it is a new file that has been there, unchanged, for [`SETTLE`] at `now`.
    pub fn look(&mut self, now: Instant) -> (Installed, bool) {
        let found = match std::fs::metadata(&self.path) {
            Ok(metadata) => Some(FileId::of(&metadata)),
            Err(_) => None,
        };
        let installed = match found {
            Some(id) if id == self.image => Installed::Same,
            Some(_) => Installed::Replaced,
            None => Installed::Missing,
        };
        let Some(id) = found.filter(|_| installed == Installed::Replaced) else {
            self.replacement = None;
            return (installed, false);
        };
        let since = match self.replacement {
            Some((seen, since)) if seen == id => since,
            // a first sight, or yet another file: the wait starts again
            _ => now,
        };
        self.replacement = Some((id, since));
        (installed, now.saturating_duration_since(since) >= SETTLE)
    }
}

/// Ask the process behind `pidfd` to end (`SIGTERM`) and wait up to `timeout` for it; whether it ended.
pub fn stop(pidfd: impl AsFd, timeout: Duration) -> io::Result<bool> {
    match pidfd_send_signal(&pidfd, Signal::TERM) {
        Ok(()) => {}
        // it ended already
        Err(rustix::io::Errno::SRCH) => return Ok(true),
        Err(e) => return Err(e.into()),
    }
    let deadline = Instant::now() + timeout;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        let wait = Timespec {
            tv_sec: left.as_secs() as i64,
            tv_nsec: i64::from(left.subsec_nanos()),
        };
        let mut fds = [PollFd::new(&pidfd, PollFlags::IN)];
        match poll(&mut fds, Some(&wait)) {
            Ok(0) => return Ok(false),
            Ok(_) => return Ok(true),
            Err(rustix::io::Errno::INTR) => continue,
            Err(e) => return Err(e.into()),
        }
    }
}
