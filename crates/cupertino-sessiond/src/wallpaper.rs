//! The users' wallpapers on the login screen. A user's session leaves a JPEG copy of its wallpaper in the home directory
//! ([`LOGIN_WALLPAPER_FILE`]); the login screen's account cannot read homes, so the manager reads that file as the user, by
//! the rules of the pictures ([`crate::picture`]: `setfsuid`, `O_NOFOLLOW | O_NONBLOCK`, a regular file of the user, at most
//! [`MAX_WALLPAPER_BYTES`]), checks that it is a JPEG of at most [`MAX_WALLPAPER_SIDE`] pixels a side by its headers, and
//! keeps a copy of its own, `<uid>.jpg` in [`WALLPAPER_DIR`]: root's, readable by the login screen's group alone. The user
//! row names that copy.
//!
//! The manager decodes no image: the headers are only read to refuse what is no JPEG or too large; the login screen, which
//! runs as its own unprivileged account, decodes the copy. A file unchanged since the last copy is not read again.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use cupertino_login_ipc::message::{
    LOGIN_WALLPAPER_FILE, MAX_WALLPAPER_BYTES, MAX_WALLPAPER_SIDE, WALLPAPER_DIR,
};

use crate::picture;

/// The mode of the copies and of their directory: root writes, the login screen's group reads.
const FILE_MODE: u32 = 0o640;
const DIR_MODE: u32 = 0o750;

/// Why a user's wallpaper file was not taken.
#[derive(Debug)]
pub enum Problem {
    Open(io::Error),
    /// Not a regular file of the account.
    NotOwnRegularFile,
    TooLarge,
    Read(io::Error),
    /// Not a JPEG the login screen takes; why.
    NotJpeg(&'static str),
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Open(e) => write!(f, "cannot open the wallpaper: {e}"),
            Self::NotOwnRegularFile => {
                f.write_str("the wallpaper is not a regular file of the account")
            }
            Self::TooLarge => f.write_str("the wallpaper file is too large"),
            Self::Read(e) => write!(f, "cannot read the wallpaper: {e}"),
            Self::NotJpeg(why) => write!(f, "the wallpaper is refused: {why}"),
        }
    }
}

impl std::error::Error for Problem {}

/// The user's file as last copied: a change to any of these is a new file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Source {
    dev: u64,
    ino: u64,
    size: u64,
    mtime: i64,
    mtime_nsec: i64,
}

impl Source {
    fn of(metadata: &std::fs::Metadata) -> Source {
        Source {
            dev: metadata.dev(),
            ino: metadata.ino(),
            size: metadata.size(),
            mtime: metadata.mtime(),
            mtime_nsec: metadata.mtime_nsec(),
        }
    }
}

/// What a read of the user's file found.
#[derive(Debug)]
enum Found {
    /// The home directory is not there (not mounted while its user is logged out): the copy there was stays.
    NoHome,
    /// The home is there and the file is not: the user has no wallpaper of its own.
    Missing,
    /// The file is the one copied last.
    Unchanged,
    New(Vec<u8>, Source),
}

/// The width and height a JPEG's frame header gives, read from its headers alone; refused unless it starts and ends as a
/// JPEG does and each side is 1 to [`MAX_WALLPAPER_SIDE`] pixels.
pub fn check(bytes: &[u8]) -> Result<(u32, u32), &'static str> {
    if !bytes.starts_with(&[0xFF, 0xD8]) {
        return Err("not a JPEG");
    }
    if !bytes.ends_with(&[0xFF, 0xD9]) {
        return Err("the JPEG is cut short");
    }
    let be16 = |at: usize| -> Option<usize> {
        Some(usize::from(*bytes.get(at)?) << 8 | usize::from(*bytes.get(at + 1)?))
    };
    let mut at = 2;
    loop {
        if bytes.get(at) != Some(&0xFF) {
            return Err("a JPEG segment does not start with a marker");
        }
        // a marker may follow fill bytes
        while bytes.get(at) == Some(&0xFF) {
            at += 1;
        }
        let marker = *bytes.get(at).ok_or("the JPEG ends in a marker")?;
        at += 1;
        match marker {
            // markers without a length
            0x01 | 0xD0..=0xD8 => continue,
            0xD9 | 0xDA => return Err("the JPEG has no frame header"),
            _ => {}
        }
        let length = be16(at).ok_or("the JPEG ends in a segment")?;
        if length < 2 {
            return Err("a JPEG segment has a wrong length");
        }
        // the frame headers: every SOFn but DHT (C4), JPG (C8) and DAC (CC)
        if matches!(marker, 0xC0..=0xCF) && !matches!(marker, 0xC4 | 0xC8 | 0xCC) {
            if length < 8 {
                return Err("the JPEG's frame header is too short");
            }
            let height = be16(at + 3).ok_or("the JPEG ends in its frame header")?;
            let width = be16(at + 5).ok_or("the JPEG ends in its frame header")?;
            let side = 1..=MAX_WALLPAPER_SIDE as usize;
            if !side.contains(&width) || !side.contains(&height) {
                return Err("the picture is empty or larger than the login screen takes");
            }
            return Ok((width as u32, height as u32));
        }
        at += length;
    }
}

/// The manager's copies of the users' wallpapers.
pub struct Store {
    dir: PathBuf,
    /// The group of the login screen's account, which reads the copies.
    gid: u32,
    /// Larger user files are refused.
    max_bytes: usize,
    /// The user file each copy was made from.
    known: HashMap<u32, Source>,
    /// Users whose file was refused, so the journal says it once.
    reported: HashSet<u32>,
}

impl Store {
    /// The copies in [`WALLPAPER_DIR`], readable by the group `gid`.
    pub fn new(gid: u32) -> Store {
        Store::in_dir(PathBuf::from(WALLPAPER_DIR), gid, MAX_WALLPAPER_BYTES)
    }

    /// The copies in `dir`, of files of at most `max_bytes`.
    pub fn in_dir(dir: PathBuf, gid: u32, max_bytes: usize) -> Store {
        Store {
            dir,
            gid,
            max_bytes,
            known: HashMap::new(),
            reported: HashSet::new(),
        }
    }

    fn copy_of(&self, uid: u32) -> PathBuf {
        self.dir.join(format!("{uid}.jpg"))
    }

    /// The path of the copy of the wallpaper of the account `uid`, whose home directory is `home`, made again when the
    /// user's file changed; `None` when the user has no usable wallpaper file. A refused file is logged once a user.
    pub fn wallpaper(&mut self, uid: u32, home: &Path) -> Option<String> {
        let copy = self.copy_of(uid);
        let have_copy = std::fs::symlink_metadata(&copy).is_ok_and(|m| m.is_file());
        let known = self.known.get(&uid).copied().filter(|_| have_copy);
        let max_bytes = self.max_bytes;
        let found = picture::as_user(uid, || read_as_current_fsuid(uid, home, known, max_bytes));
        let taken = match found {
            Ok(Found::NoHome | Found::Unchanged) => return have_copy.then(|| path_text(&copy)),
            Ok(Found::Missing) => Err(None),
            Ok(Found::New(bytes, source)) => match check(&bytes) {
                Ok(_) => Ok((bytes, source)),
                Err(why) => Err(Some(Problem::NotJpeg(why))),
            },
            Err(problem) => Err(Some(problem)),
        };
        match taken {
            Ok((bytes, source)) => match self.put(&copy, &bytes) {
                Ok(()) => {
                    self.known.insert(uid, source);
                    self.reported.remove(&uid);
                    Some(path_text(&copy))
                }
                Err(e) => {
                    eprintln!(
                        "cupertino-sessiond: keeping the wallpaper of uid {uid} in {}: {e}",
                        self.dir.display()
                    );
                    self.forget(uid, &copy);
                    None
                }
            },
            Err(problem) => {
                if let Some(problem) = problem {
                    if self.reported.insert(uid) {
                        eprintln!("cupertino-sessiond: no login screen wallpaper for uid {uid}: {problem}");
                    }
                }
                self.forget(uid, &copy);
                None
            }
        }
    }

    /// The user has no wallpaper (any more): its copy goes.
    fn forget(&mut self, uid: u32, copy: &Path) {
        self.known.remove(&uid);
        match std::fs::remove_file(copy) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => eprintln!("cupertino-sessiond: removing {}: {e}", copy.display()),
        }
    }

    /// Write `bytes` to `copy` whole: a new file renamed over the old one, so the login screen reads the old or the new.
    fn put(&self, copy: &Path, bytes: &[u8]) -> io::Result<()> {
        self.ensure_dir()?;
        let name = copy.file_name().ok_or(io::ErrorKind::InvalidInput)?;
        let new = self.dir.join(format!(".{}.new", name.to_string_lossy()));
        match std::fs::remove_file(&new) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        let mut file = File::options()
            .write(true)
            .create_new(true)
            .mode(FILE_MODE)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&new)?;
        let written = (|| {
            std::os::unix::fs::fchown(&file, None, Some(self.gid))?;
            // the mode given at creation is masked by the umask
            file.set_permissions(std::fs::Permissions::from_mode(FILE_MODE))?;
            file.write_all(bytes)?;
            file.sync_all()
        })();
        if let Err(e) = written.and_then(|()| std::fs::rename(&new, copy)) {
            let _ = std::fs::remove_file(&new);
            return Err(e);
        }
        Ok(())
    }

    /// The directory of the copies, made when missing: a real directory (not a link) of this process's user, mode
    /// [`DIR_MODE`], of the login screen's group.
    fn ensure_dir(&self) -> io::Result<()> {
        if let Some(parent) = self.dir.parent() {
            std::fs::create_dir_all(parent)?;
        }
        match std::fs::DirBuilder::new().mode(DIR_MODE).create(&self.dir) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
        let metadata = std::fs::symlink_metadata(&self.dir)?;
        // Safety: `geteuid` has no preconditions.
        let owner = unsafe { libc::geteuid() };
        if !metadata.is_dir() || metadata.uid() != owner {
            return Err(io::Error::other("not a directory of the manager's"));
        }
        if metadata.gid() != self.gid {
            std::os::unix::fs::lchown(&self.dir, None, Some(self.gid))?;
        }
        if metadata.mode() & 0o7777 != DIR_MODE {
            std::fs::set_permissions(&self.dir, std::fs::Permissions::from_mode(DIR_MODE))?;
        }
        Ok(())
    }
}

fn path_text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// Read the user's file under `home` as the current file system user; `known` is the file copied last.
fn read_as_current_fsuid(
    uid: u32,
    home: &Path,
    known: Option<Source>,
    max_bytes: usize,
) -> Result<Found, Problem> {
    let path = home.join(LOGIN_WALLPAPER_FILE);
    let file = match File::options()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(&path)
    {
        Ok(file) => file,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Ok(match std::fs::symlink_metadata(home) {
                Err(e) if e.kind() == io::ErrorKind::NotFound => Found::NoHome,
                _ => Found::Missing,
            })
        }
        Err(e) => return Err(Problem::Open(e)),
    };
    let metadata = file.metadata().map_err(Problem::Read)?;
    if !metadata.is_file() || metadata.uid() != uid {
        return Err(Problem::NotOwnRegularFile);
    }
    if metadata.len() > max_bytes as u64 {
        return Err(Problem::TooLarge);
    }
    let source = Source::of(&metadata);
    if known == Some(source) {
        return Ok(Found::Unchanged);
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    // one byte more than the cap shows a file that grew since `fstat`
    file.take(max_bytes as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(Problem::Read)?;
    if bytes.len() > max_bytes {
        return Err(Problem::TooLarge);
    }
    Ok(Found::New(bytes, source))
}
