//! Reading a user's picture (`~/.face`, the freedesktop convention) as root without trusting the user's files.
//!
//! The read runs on a short-lived thread whose file system user id is the account's (`setfsuid`; glibc does not copy it to
//! the other threads, unlike `setuid`), so the kernel checks every access as that user, the file is opened with
//! `O_NOFOLLOW | O_NONBLOCK` (no symlink to a root-only file, no device or fifo that blocks), and `fstat` insists on a
//! regular file of that user of at most [`MAX_PICTURE_BYTES`].

use std::fmt;
use std::fs::File;
use std::io::Read;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;

use cupertino_login_ipc::message::MAX_PICTURE_BYTES;

/// The picture file in a home directory.
const FACE_FILE: &str = ".face";

/// Why a picture was not read. A missing file is not a problem, it is `Ok(None)`.
#[derive(Debug)]
pub enum PictureProblem {
    Open(std::io::Error),
    /// Not a regular file of the account.
    NotOwnRegularFile,
    TooLarge,
    Read(std::io::Error),
}

impl fmt::Display for PictureProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Open(e) => write!(f, "cannot open the picture: {e}"),
            Self::NotOwnRegularFile => {
                f.write_str("the picture is not a regular file of the account")
            }
            Self::TooLarge => write!(f, "the picture is larger than {MAX_PICTURE_BYTES} bytes"),
            Self::Read(e) => write!(f, "cannot read the picture: {e}"),
        }
    }
}

impl std::error::Error for PictureProblem {}

/// The picture of the account `uid` whose home directory is `home`.
pub fn read(uid: u32, home: &Path) -> Result<Option<Vec<u8>>, PictureProblem> {
    let path = home.join(FACE_FILE);
    as_user(uid, || read_as_current_fsuid(uid, &path))
}

/// `read` run on a short-lived thread whose file system user id is `uid`, so the kernel checks each file access of it as
/// that user.
pub(crate) fn as_user<T: Send>(uid: u32, read: impl FnOnce() -> T + Send) -> T {
    std::thread::scope(|scope| {
        scope
            .spawn(|| {
                // Safety: `setfsuid` has no memory effects. The thread ends after this read, so the id is never restored.
                unsafe { libc::setfsuid(uid) };
                read()
            })
            .join()
            .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
    })
}

fn read_as_current_fsuid(uid: u32, path: &Path) -> Result<Option<Vec<u8>>, PictureProblem> {
    let file = match File::options()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)
    {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(PictureProblem::Open(e)),
    };
    let metadata = file.metadata().map_err(PictureProblem::Read)?;
    if !metadata.is_file() || metadata.uid() != uid {
        return Err(PictureProblem::NotOwnRegularFile);
    }
    if metadata.len() > MAX_PICTURE_BYTES as u64 {
        return Err(PictureProblem::TooLarge);
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    // one byte more than the cap shows a file that grew since `fstat`
    file.take(MAX_PICTURE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(PictureProblem::Read)?;
    if bytes.len() > MAX_PICTURE_BYTES {
        return Err(PictureProblem::TooLarge);
    }
    Ok(Some(bytes))
}
