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

#[cfg(test)]
mod tests {
    use super::*;
    use rustix::process::{pidfd_open, Pid, PidfdFlags};

    #[test]
    fn a_replaced_file_with_no_user_session_hands_over() {
        assert_eq!(decide(Installed::Replaced, true, false), Decision::HandOver);
    }

    #[test]
    fn a_replaced_file_waits_for_the_last_user_session() {
        assert_eq!(decide(Installed::Replaced, true, true), Decision::Keep);
    }

    #[test]
    fn an_unchanged_or_missing_file_or_one_not_settled_is_kept() {
        for user_session in [false, true] {
            assert_eq!(decide(Installed::Same, true, user_session), Decision::Keep);
            assert_eq!(
                decide(Installed::Missing, true, user_session),
                Decision::Keep
            );
            assert_eq!(
                decide(Installed::Replaced, false, user_session),
                Decision::Keep
            );
        }
    }

    #[test]
    fn a_file_renamed_over_the_program_is_a_replacement_that_settles() {
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join("program");
        std::fs::write(&program, b"old").unwrap();
        let mut watch = Watch::of_file(&program).unwrap();
        let start = Instant::now();
        assert_eq!(watch.look(start), (Installed::Same, false));
        let new = dir.path().join("program.new");
        std::fs::write(&new, b"new").unwrap();
        std::fs::rename(&new, &program).unwrap();
        assert_eq!(watch.look(start), (Installed::Replaced, false));
        assert_eq!(watch.look(start + SETTLE / 2), (Installed::Replaced, false));
        assert_eq!(watch.look(start + SETTLE), (Installed::Replaced, true));
    }

    #[test]
    fn another_new_file_starts_the_wait_again_and_a_removed_one_is_missing() {
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join("program");
        std::fs::write(&program, b"old").unwrap();
        let mut watch = Watch::of_file(&program).unwrap();
        let start = Instant::now();
        // keeps the first replacement's inode in use, so the second cannot get it
        let first = dir.path().join("first");
        std::fs::write(&first, b"first").unwrap();
        std::fs::hard_link(&first, dir.path().join("first.keep")).unwrap();
        std::fs::rename(&first, &program).unwrap();
        assert_eq!(watch.look(start), (Installed::Replaced, false));
        let second = dir.path().join("second");
        std::fs::write(&second, b"second").unwrap();
        std::fs::rename(&second, &program).unwrap();
        assert_eq!(watch.look(start + SETTLE), (Installed::Replaced, false));
        assert_eq!(watch.look(start + SETTLE * 2), (Installed::Replaced, true));
        std::fs::remove_file(&program).unwrap();
        assert_eq!(watch.look(start + SETTLE * 3), (Installed::Missing, false));
    }

    #[test]
    fn this_process_is_its_own_file() {
        let mut watch = Watch::this_process().unwrap();
        assert_eq!(watch.path(), std::env::current_exe().unwrap());
        assert_eq!(watch.look(Instant::now()), (Installed::Same, false));
    }

    fn pidfd(child: &std::process::Child) -> std::os::fd::OwnedFd {
        pidfd_open(
            Pid::from_raw(child.id() as i32).unwrap(),
            PidfdFlags::empty(),
        )
        .unwrap()
    }

    #[test]
    fn a_process_asked_to_stop_ends_and_one_that_ignores_it_is_reported() {
        let mut ends = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .unwrap();
        assert!(stop(pidfd(&ends), Duration::from_secs(10)).unwrap());
        ends.wait().unwrap();
        let mut stays = std::process::Command::new("sh")
            .args(["-c", "trap '' TERM; echo ready; exec sleep 30"])
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        // the trap is set once the shell says so
        let mut line = String::new();
        std::io::BufRead::read_line(
            &mut std::io::BufReader::new(stays.stdout.take().unwrap()),
            &mut line,
        )
        .unwrap();
        assert!(!stop(pidfd(&stays), Duration::from_millis(100)).unwrap());
        stays.kill().unwrap();
        stays.wait().unwrap();
    }
}
