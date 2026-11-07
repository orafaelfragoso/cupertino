//! An advisory lock around a load, change and store of a settings file that more than one process changes, so that a
//! store made from an older read never writes over another process's change. It is `flock` on a file of its own next
//! to the settings file, `.<name>.lock` with mode `0600`, never on the settings file itself, which each store replaces.
//! Every writer of such a file holds [`FileLock`] for its whole load-modify-store; a process that only reads needs
//! none, since each store replaces the file atomically.
//!
//! The lock file is made once and left in place: removing it while another process waits on it would let the two
//! lock different files.

use std::fs::{File, OpenOptions};
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use crate::data_control::refused;

/// The lock of one settings file, held until it is dropped.
#[derive(Debug)]
pub struct FileLock {
    // closing the file releases the lock
    _file: File,
}

impl FileLock {
    /// Wait until no other holder has the lock of the settings file at `path`, and take it. The lock file and its
    /// directory are made when missing. `Err` when the lock file cannot be made or opened, or is not a regular file of
    /// this user.
    pub fn acquire(path: &Path) -> io::Result<FileLock> {
        let lock = lock_path(path)?;
        if let Some(dir) = lock.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir)?;
        }
        let file = match OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .mode(0o600)
            // without O_NONBLOCK a FIFO put at the path would block the open before the check refuses it
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
            .open(&lock)
        {
            Ok(file) => file,
            Err(e) if e.raw_os_error() == Some(libc::ELOOP) => {
                return Err(refused("the lock file is a link"))
            }
            Err(e) => return Err(e),
        };
        let meta = file.metadata()?;
        // Safety: geteuid has no preconditions.
        if !meta.is_file() || meta.uid() != unsafe { libc::geteuid() } {
            return Err(refused("the lock file is not a regular file of this user"));
        }
        loop {
            // Safety: the descriptor is open for as long as `file` lives.
            if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } == 0 {
                return Ok(FileLock { _file: file });
            }
            let e = io::Error::last_os_error();
            if e.kind() != io::ErrorKind::Interrupted {
                return Err(e);
            }
        }
    }
}

/// The lock file of the settings file at `path`: `.<name>.lock` in the same directory.
pub fn lock_path(path: &Path) -> io::Result<PathBuf> {
    let name = path.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{} names no file", path.display()),
        )
    })?;
    let mut lock = std::ffi::OsString::from(".");
    lock.push(name);
    lock.push(".lock");
    Ok(path.with_file_name(lock))
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;
    use std::sync::mpsc;
    use std::time::Duration;

    use super::*;

    /// How long a test waits to see that a second taker is still blocked.
    const BLOCKED: Duration = Duration::from_millis(200);

    #[test]
    fn the_lock_file_sits_beside_the_settings_file_with_mode_0600() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("screen-sharing.conf");
        assert_eq!(
            lock_path(&path).unwrap(),
            dir.path().join("sub").join(".screen-sharing.conf.lock")
        );
        drop(FileLock::acquire(&path).unwrap());
        let mode = std::fs::metadata(lock_path(&path).unwrap())
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
        assert!(!path.exists(), "the settings file itself is not made");
    }

    #[test]
    fn a_second_taker_waits_until_the_first_lets_go() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("screen-sharing.conf");
        let held = FileLock::acquire(&path).unwrap();
        let (taken, took) = mpsc::channel();
        let other = path.clone();
        let waiter = std::thread::spawn(move || {
            // a lock of its own open file: flock locks conflict between two opens even in one process
            let lock = FileLock::acquire(&other).unwrap();
            taken.send(()).unwrap();
            drop(lock);
        });
        assert!(
            took.recv_timeout(BLOCKED).is_err(),
            "the lock is held, so the second taker waits"
        );
        drop(held);
        took.recv_timeout(Duration::from_secs(10)).unwrap();
        waiter.join().unwrap();
    }

    #[test]
    fn a_link_or_a_fifo_at_the_lock_path_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("screen-sharing.conf");
        let elsewhere = dir.path().join("elsewhere");
        std::os::unix::fs::symlink(&elsewhere, lock_path(&path).unwrap()).unwrap();
        assert!(FileLock::acquire(&path).is_err());
        assert!(!elsewhere.exists(), "the link is not followed");

        let path = dir.path().join("other.conf");
        let fifo = std::ffi::CString::new(
            lock_path(&path)
                .unwrap()
                .into_os_string()
                .into_encoded_bytes(),
        )
        .unwrap();
        // Safety: `fifo` is a valid C string.
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
        assert!(FileLock::acquire(&path).is_err());
    }
}
