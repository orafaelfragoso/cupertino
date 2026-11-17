//! An advisory lock around a load, change and store of a settings file that more than one process changes, so that a
//! store made from an older read never writes over another process's change. It is `flock` on a file of its own next
//! to the settings file, `.<name>.lock` with mode `0600`, never on the settings file itself, which each store replaces.
//! Every writer of such a file holds [`FileLock`] for its whole load-modify-store; a process that only reads needs
//! none, since each store replaces the file atomically.
//!
//! The lock file is made once and left in place: removing it while another process waits on it would let the two
//! lock different files.
//!
//! A taker waits at most [`LOCK_WAIT`]: the portal takes the lock while it answers on the bus and Settings while its
//! window runs, so a holder that never lets go makes the change fail instead of stopping either.

use std::fs::{File, OpenOptions};
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::data_control::refused;

/// How long a taker waits for another holder to let go before the change fails. A holder reads and writes one small
/// file, so only one that is stuck keeps the lock longer.
pub const LOCK_WAIT: Duration = Duration::from_secs(2);

/// How often a waiting taker tries the lock again.
const LOCK_RETRY: Duration = Duration::from_millis(10);

/// The lock of one settings file, held until it is dropped.
#[derive(Debug)]
pub struct FileLock {
    // closing the file releases the lock
    _file: File,
}

impl FileLock {
    /// Wait until no other holder has the lock of the settings file at `path`, at most [`LOCK_WAIT`], and take it.
    /// The lock file and its directory are made when missing. `Err` when the lock file cannot be made or opened, or is
    /// not a regular file of this user, and [`io::ErrorKind::TimedOut`] when another holder kept it all that time.
    pub fn acquire(path: &Path) -> io::Result<FileLock> {
        Self::acquire_within(path, LOCK_WAIT)
    }

    /// [`Self::acquire`], waiting at most `wait`.
    pub fn acquire_within(path: &Path, wait: Duration) -> io::Result<FileLock> {
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
        let start = Instant::now();
        loop {
            // Safety: the descriptor is open for as long as `file` lives.
            if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
                return Ok(FileLock { _file: file });
            }
            let e = io::Error::last_os_error();
            match e.kind() {
                io::ErrorKind::Interrupted => {}
                io::ErrorKind::WouldBlock if start.elapsed() < wait => {
                    std::thread::sleep(LOCK_RETRY)
                }
                io::ErrorKind::WouldBlock => {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        format!("{} is held by another writer", lock.display()),
                    ))
                }
                _ => return Err(e),
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
