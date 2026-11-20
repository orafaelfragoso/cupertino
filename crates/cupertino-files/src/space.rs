//! What the file system says about a folder, for the status bar and the drop targets, and the nearest folder above one
//! that is gone. Each of these touches the disk (and may wait on a mount that does not answer): the application runs
//! them as I/O tasks, never on its UI thread.

use std::io;
use std::path::{Path, PathBuf};

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

/// The nearest folder above `dir` that exists.
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
