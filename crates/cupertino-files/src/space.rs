//! What the file system says about a folder, for the status bar and the drop targets, and the nearest folder above one
//! that is gone. Each of these touches the disk (and may wait on a mount that does not answer): the application runs
//! them as I/O tasks, never on its UI thread.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// How long a folder's read-only state, once read, answers a drag hovering it before it is read again: a file system
/// can be mounted again read-only, and a drag lasts seconds.
pub const MOUNT_STATE_LIFETIME: Duration = Duration::from_secs(30);

/// How long reading the write state of a few folders should take: one `statvfs` each, held up only by a mount that
/// does not answer.
pub const MOUNT_READ_DEADLINE: Duration = Duration::from_secs(5);

/// The room and the write state of a folder's file system.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Space {
    /// Bytes an unprivileged user can still write there (`statvfs`: available blocks times the fragment size); `None`
    /// for a virtual folder.
    pub available: Option<u64>,
    /// The file system is mounted read-only, or the folder lists what is installed (Applications): nothing is made,
    /// renamed or moved there.
    pub read_only: bool,
}

/// What the file system says about `dir`, a folder shown by a window: a virtual folder (Recents, Applications) has no
/// file system; the error says why the file system of any other could not be read.
pub fn read(dir: &Path) -> Result<Space, String> {
    if dir == cupertino_kit::files::applications::location() {
        return Ok(Space {
            available: None,
            read_only: true,
        });
    }
    if dir == cupertino_kit::files::recents::location() {
        return Ok(Space {
            available: None,
            read_only: false,
        });
    }
    let fs = file_system(dir).map_err(|e| e.to_string())?;
    // the fields are narrower on 32-bit targets
    #[allow(clippy::unnecessary_cast)]
    let available = fs.f_bavail as u64 * fs.f_frsize as u64;
    Ok(Space {
        available: Some(available),
        read_only: fs.f_flag & libc::ST_RDONLY != 0,
    })
}

/// Whether the file system of `dir` is mounted read-only, whatever the medium under it says: a writable stick can
/// hold a file system mounted so.
pub fn mounted_read_only(dir: &Path) -> io::Result<bool> {
    Ok(file_system(dir)?.f_flag & libc::ST_RDONLY != 0)
}

/// The write state of the file systems of the folders a drag hovers, kept so that hovering never touches the disk.
/// A folder not known yet counts as writable, so the drag is allowed, and is asked for: [`MountedCache::take_wanted`]
/// names it for a task that reads it ([`read_each`]) and [`MountedCache::record`] files the answer, after which the
/// folder is refused if its file system is mounted read-only. UI thread only.
#[derive(Default)]
pub struct MountedCache {
    known: RefCell<HashMap<PathBuf, (bool, Instant)>>,
    /// Folders to read, not yet handed to a task.
    wanted: RefCell<Vec<PathBuf>>,
    /// Folders wanted or being read, so each is asked for once.
    asked: RefCell<HashSet<PathBuf>>,
}

impl MountedCache {
    /// Whether the file system of `dir` is known to be mounted read-only. Unknown, or known longer ago than
    /// [`MOUNT_STATE_LIFETIME`] at `now`, the folder is asked for, and the last known state (writable when there is
    /// none) answers.
    pub fn read_only(&self, dir: &Path, now: Instant) -> bool {
        let known = self.known.borrow().get(dir).copied();
        let fresh =
            known.is_some_and(|(_, at)| now.saturating_duration_since(at) < MOUNT_STATE_LIFETIME);
        if !fresh && self.asked.borrow_mut().insert(dir.to_path_buf()) {
            self.wanted.borrow_mut().push(dir.to_path_buf());
        }
        known.is_some_and(|(read_only, _)| read_only)
    }

    /// The folders to read now: each is handed out once until its answer is recorded.
    pub fn take_wanted(&self) -> Vec<PathBuf> {
        std::mem::take(&mut *self.wanted.borrow_mut())
    }

    /// File the state read for `dir` at `now`.
    pub fn record(&self, dir: PathBuf, read_only: bool, now: Instant) {
        self.asked.borrow_mut().remove(&dir);
        self.known.borrow_mut().insert(dir, (read_only, now));
    }
}

/// The write state of the file system of each of `dirs`; a folder that cannot be read counts as writable. Blocks on
/// the file system, so a program runs it on an I/O task.
pub fn read_each(dirs: Vec<PathBuf>) -> Vec<(PathBuf, bool)> {
    dirs.into_iter()
        .map(|dir| {
            let read_only = mounted_read_only(&dir).unwrap_or(false);
            (dir, read_only)
        })
        .collect()
}

/// The nearest folder above `dir` that exists. It blocks on the file system: a program runs it on an I/O task.
#[expect(
    clippy::disallowed_methods,
    reason = "a blocking read that programs run on the I/O pool (kit::task::io)"
)]
pub fn nearest_existing_ancestor(dir: &Path) -> Option<PathBuf> {
    dir.ancestors()
        .skip(1)
        .find(|p| p.is_dir())
        .map(Path::to_path_buf)
}

fn file_system(dir: &Path) -> io::Result<libc::statvfs> {
    let dir = std::ffi::CString::new(dir.as_os_str().as_encoded_bytes())
        .map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
    // SAFETY: a zeroed statvfs is a valid out value; the path is NUL-terminated and outlives the call
    let mut fs: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(dir.as_ptr(), &mut fs) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(fs)
}
