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
    std::thread::scope(|scope| {
        scope
            .spawn(|| {
                // Safety: `setfsuid` has no memory effects. The thread ends after this read, so the id is never restored.
                unsafe { libc::setfsuid(uid) };
                read_as_current_fsuid(uid, &path)
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The user id of the test process: the files the tests create are its own.
    fn own_uid() -> u32 {
        // Safety: `getuid` has no preconditions.
        unsafe { libc::getuid() }
    }

    #[test]
    fn a_missing_picture_is_none() {
        let home = tempfile::tempdir().unwrap();
        assert!(read(own_uid(), home.path()).unwrap().is_none());
    }

    #[test]
    fn a_regular_file_of_the_account_is_read_whole() {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(home.path().join(".face"), b"\x89PNG bytes").unwrap();
        assert_eq!(
            read(own_uid(), home.path()).unwrap().unwrap(),
            b"\x89PNG bytes"
        );
    }

    #[test]
    fn a_symlink_is_not_followed() {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(home.path().join("secret"), b"root only").unwrap();
        std::os::unix::fs::symlink("secret", home.path().join(".face")).unwrap();
        assert!(matches!(
            read(own_uid(), home.path()),
            Err(PictureProblem::Open(_))
        ));
    }

    #[test]
    fn a_device_or_a_directory_is_not_a_picture() {
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir(home.path().join(".face")).unwrap();
        assert!(matches!(
            read(own_uid(), home.path()),
            Err(PictureProblem::NotOwnRegularFile)
        ));
    }

    #[test]
    fn a_file_of_another_account_is_refused() {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(home.path().join(".face"), b"x").unwrap();
        assert!(matches!(
            read(own_uid() + 1, home.path()),
            Err(PictureProblem::NotOwnRegularFile | PictureProblem::Open(_))
        ));
    }

    #[test]
    fn a_file_over_the_cap_is_refused() {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(home.path().join(".face"), vec![0u8; MAX_PICTURE_BYTES + 1]).unwrap();
        assert!(matches!(
            read(own_uid(), home.path()),
            Err(PictureProblem::TooLarge)
        ));
        std::fs::write(home.path().join(".face"), vec![0u8; MAX_PICTURE_BYTES]).unwrap();
        assert_eq!(
            read(own_uid(), home.path()).unwrap().unwrap().len(),
            MAX_PICTURE_BYTES
        );
    }
}
