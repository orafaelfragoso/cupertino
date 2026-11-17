//! Trash following the freedesktop.org Trash specification 1.0.
//!
//! * Home trash: `$XDG_DATA_HOME/Trash/{files,info}`; used when the item is on the same device.
//! * Otherwise the trash of the mount point: `$topdir/.Trash/$uid` when `$topdir/.Trash` exists, is
//!   a real directory with the sticky bit, else `$topdir/.Trash-$uid`.
//! * Every trashed item has `info/<name>.trashinfo` with `Path=` (percent-encoded; relative to the
//!   topdir for a mount-point trash) and `DeletionDate=` in local time.
//! * Name clashes are resolved by appending ` 2`, ` 3`, ... before the extension.
//! * `directorysizes` is maintained for trashed directories (size in bytes, mtime of the info file,
//!   percent-encoded name).
//!
//! * Emptying takes the home trash and the user's trash of every mounted volume that can be written
//!   ([`Trash::trash_dirs`]), and nothing else: a volume's shared `.Trash` counts only as a real directory with the
//!   sticky bit, a trash directory only as a real directory the user owns, and a link found in a trash is deleted
//!   itself, never followed.
//!
//! * A mounted volume may not answer (a network mount whose server went away blocks every call on it for minutes).
//!   Every function here blocks and starts no thread: a program runs the ones that touch the volumes as tasks. The
//!   volumes are looked at one after another on the calling thread, one look at a time per volume in the process: a
//!   look that finds another still out on a volume waits for it [`VOLUME_ANSWERS_WITHIN`] at most, then counts that
//!   volume as silent, holding no trash for that look. The home trash is always taken first and never waits for a
//!   volume.
//!
//! Absolute locations are injectable through [`Trash`] so tests can use temporary directories.

use percent_encoding::{percent_decode_str, utf8_percent_encode, AsciiSet, CONTROLS};
use std::collections::HashMap;
use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

mod look;
pub use look::{Look, LOOK_DEADLINE};

/// How long a look at the trash waits for another look still out on a mounted volume before it goes on without it.
pub const VOLUME_ANSWERS_WITHIN: Duration = Duration::from_secs(2);

/// What the question asked before the trash is emptied says, for every program that asks it: its title, what happens
/// and the labels of its two buttons.
pub const EMPTY_TITLE: &str = "Empty the Trash?";
pub const EMPTY_MESSAGE: &str =
    "Everything in the Trash is deleted for good. This cannot be undone.";
pub const EMPTY_BUTTON: &str = "Empty Trash";
pub const CANCEL_BUTTON: &str = "Cancel";
/// The name of the picture the question shows, and the alert after an emptying that left something: the full trash.
pub const EMPTY_ICON: &str = "trash.full";
/// What the alert after an emptying that could not delete everything says: its title and the label of its one
/// button. Its message is [`Emptied::failure_message`].
pub const FAILED_TITLE: &str = "The Trash could not be emptied completely.";
pub const FAILED_BUTTON: &str = "OK";

/// One item an emptying could not delete: where it lies in its trash, and why.
#[derive(Debug)]
pub struct Failure {
    pub path: PathBuf,
    pub error: io::Error,
}

/// What an emptying did, over every trash directory it took.
#[derive(Debug, Default)]
pub struct Emptied {
    /// How many items went.
    pub deleted: usize,
    /// The items that are still there, back in their trash.
    pub failures: Vec<Failure>,
}

impl Emptied {
    /// What the alert says under [`FAILED_TITLE`], or `None` when everything went: the one item by name with its
    /// reason, or how many items stayed with the first one's reason.
    pub fn failure_message(&self) -> Option<String> {
        failure_message(
            self.failures
                .iter()
                .map(|f| (f.path.as_path(), f.error.to_string())),
        )
    }
}

/// [`Emptied::failure_message`] for any list of items that stayed and why.
pub fn failure_message<'a>(
    failures: impl IntoIterator<Item = (&'a Path, String)>,
) -> Option<String> {
    let mut failures = failures.into_iter();
    let (path, reason) = failures.next()?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());
    Some(match failures.count() {
        0 => format!("“{name}” could not be deleted: {reason}."),
        more => format!(
            "{} items could not be deleted. “{name}”: {reason}.",
            more + 1
        ),
    })
}

/// Characters percent-encoded in `Path=` (everything but unreserved characters and `/`).
const PATH_SET: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'#')
    .add(b'%')
    .add(b'<')
    .add(b'>')
    .add(b'?')
    .add(b'[')
    .add(b']')
    .add(b'\\')
    .add(b'^')
    .add(b'`')
    .add(b'{')
    .add(b'|')
    .add(b'}');

#[derive(Debug)]
pub enum TrashError {
    Io(io::Error),
    /// No trash directory is usable for the device of the item.
    NoTrashForDevice,
    /// The item is itself inside a trash directory.
    AlreadyTrashed,
    BadInfo(String),
}

impl std::fmt::Display for TrashError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TrashError::Io(e) => write!(f, "{e}"),
            TrashError::NoTrashForDevice => write!(f, "no trash directory for this device"),
            TrashError::AlreadyTrashed => write!(f, "item is already in the trash"),
            TrashError::BadInfo(s) => write!(f, "bad trash info: {s}"),
        }
    }
}
impl std::error::Error for TrashError {}
impl From<io::Error> for TrashError {
    fn from(e: io::Error) -> Self {
        TrashError::Io(e)
    }
}

/// Held by every change to a `directorysizes` file in this process: a [`PendingSize`] appends on a thread of its own
/// while [`forget`] rewrites the file, and a line appended between that read and that write would be lost or cut.
static SIZES: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn sizes_lock() -> std::sync::MutexGuard<'static, ()> {
    SIZES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The `directorysizes` line a folder just moved to the trash still needs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingSize {
    trash_dir: PathBuf,
    name: String,
}

impl PendingSize {
    /// Sum the trashed folder's files and add its line. Nothing is written for a folder that has left the trash
    /// meanwhile (put back or emptied), whose line would only be stale.
    pub fn record(&self) -> io::Result<()> {
        let size = dir_size(&self.trash_dir.join("files").join(&self.name));
        let info = self
            .trash_dir
            .join("info")
            .join(format!("{}.trashinfo", self.name));
        let _held = sizes_lock();
        let mtime = match fs::metadata(&info) {
            Ok(meta) => meta.mtime(),
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e),
        };
        let mut f = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.trash_dir.join("directorysizes"))?;
        // the line in one write, so another writer appending to the file cannot split it
        let line = format!("{} {} {}\n", size, mtime, encode_path(&self.name));
        f.write_all(line.as_bytes())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrashedItem {
    /// Name inside `files/`.
    pub name: String,
    pub trash_dir: PathBuf,
    pub original_path: PathBuf,
    pub deletion_date: String,
}

impl TrashedItem {
    pub fn file_path(&self) -> PathBuf {
        self.trash_dir.join("files").join(&self.name)
    }
    pub fn info_path(&self) -> PathBuf {
        self.trash_dir
            .join("info")
            .join(format!("{}.trashinfo", self.name))
    }
}

/// Called with a mount point right before its folders are looked at ([`Trash::with_volume_look`]).
#[derive(Clone)]
struct VolumeLook(Arc<dyn Fn(&Path) + Send + Sync>);

impl std::fmt::Debug for VolumeLook {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("VolumeLook")
    }
}

#[derive(Clone, Debug)]
pub struct Trash {
    home_trash: PathBuf,
    uid: u32,
    /// Mount points to consider for other devices (longest match wins). `None` means read `/proc/self/mountinfo`.
    mounts: Option<Vec<PathBuf>>,
    /// Whether, with no mount points given, the volumes whose trash is emptied are the system's mounts. Only the
    /// trash of the default location says so: one made at a location of its own never reaches for the real volumes.
    system_volumes: bool,
    /// How long a look waits for one volume ([`VOLUME_ANSWERS_WITHIN`] unless set).
    volume_wait: Duration,
    volume_look: Option<VolumeLook>,
}

/// The mount points a look is out on, each with when it began: one look at a time per volume in this process, so the
/// looks at a volume that does not answer cannot pile up the threads that run them. A look that finds one taken waits
/// for it only until the one out has had its time.
static LOOKS_OUT: Mutex<Option<HashMap<PathBuf, Instant>>> = Mutex::new(None);
static LOOK_BACK: Condvar = Condvar::new();

fn looks_out() -> std::sync::MutexGuard<'static, Option<HashMap<PathBuf, Instant>>> {
    LOOKS_OUT
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Take the mount point `top` for a look, waiting while another look is out on it, but no longer than until that one
/// has been out for `wait`. `false` when it is still out then: the volume does not answer.
fn take_volume(top: &Path, wait: Duration) -> bool {
    let mut out = looks_out();
    loop {
        let map = out.get_or_insert_with(HashMap::new);
        let Some(since) = map.get(top) else {
            map.insert(top.to_path_buf(), Instant::now());
            return true;
        };
        let Some(left) = wait.checked_sub(since.elapsed()).filter(|d| !d.is_zero()) else {
            return false;
        };
        out = LOOK_BACK
            .wait_timeout(out, left)
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .0;
    }
}

/// A mount point taken by [`take_volume`]; given back when dropped, wherever the look ended.
struct Taken(PathBuf);

impl Drop for Taken {
    fn drop(&mut self) {
        if let Some(map) = looks_out().as_mut() {
            map.remove(&self.0);
        }
        LOOK_BACK.notify_all();
    }
}

/// What `look` says of each of the mount points `tops`, in their order, looked at one after another on the calling
/// thread; `None` for a volume another look was still out on after `wait`. The look at a volume that does not answer
/// waits for it, and gives the mount point back when the volume answers after all.
fn ask_volumes<T>(
    tops: Vec<PathBuf>,
    wait: Duration,
    look: impl Fn(&Path) -> T,
) -> Vec<(PathBuf, Option<T>)> {
    tops.into_iter()
        .map(|top| {
            let said = take_volume(&top, wait).then(|| {
                let _taken = Taken(top.clone());
                look(&top)
            });
            (top, said)
        })
        .collect()
}

pub fn encode_path(p: &str) -> String {
    utf8_percent_encode(p, PATH_SET).to_string()
}

pub fn decode_path(s: &str) -> String {
    percent_decode_str(s).decode_utf8_lossy().into_owned()
}

/// Local time `YYYY-MM-DDThh:mm:ss`.
pub fn format_local_time(unix: i64) -> String {
    let tt = unix as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe {
        libc::localtime_r(&tt, &mut tm);
    }
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
        tm.tm_year + 1900,
        tm.tm_mon + 1,
        tm.tm_mday,
        tm.tm_hour,
        tm.tm_min,
        tm.tm_sec
    )
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn now_unix_nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

fn mount_points() -> Vec<PathBuf> {
    let Ok(s) = fs::read_to_string("/proc/self/mountinfo") else {
        return vec![PathBuf::from("/")];
    };
    s.lines()
        .filter_map(|l| l.split(' ').nth(4))
        .map(|m| PathBuf::from(m.replace("\\040", " ")))
        .collect()
}

/// File systems that hold no files of a person's: the kernel's own, and the automounter's, whose mount points mount
/// what they stand for when looked at.
const NOT_VOLUMES: [&str; 20] = [
    "autofs",
    "binfmt_misc",
    "bpf",
    "cgroup",
    "cgroup2",
    "configfs",
    "debugfs",
    "devpts",
    "devtmpfs",
    "efivarfs",
    "fusectl",
    "hugetlbfs",
    "mqueue",
    "nsfs",
    "proc",
    "pstore",
    "rpc_pipefs",
    "securityfs",
    "sysfs",
    "tracefs",
];

/// The mount points of `mountinfo` (the text of `/proc/self/mountinfo`) that can hold a trash: every one but those of
/// [`NOT_VOLUMES`]. A line's fifth field is the mount point; its file system type follows the lone `-`.
fn volumes_of(mountinfo: &str) -> Vec<PathBuf> {
    mountinfo
        .lines()
        .filter_map(|line| {
            let mut fields = line.split(' ');
            let point = fields.nth(4)?;
            let kind = fields.skip_while(|f| *f != "-").nth(1)?;
            (!NOT_VOLUMES.contains(&kind)).then(|| {
                PathBuf::from(
                    point
                        .replace("\\040", " ")
                        .replace("\\011", "\t")
                        .replace("\\012", "\n")
                        .replace("\\134", "\\"),
                )
            })
        })
        .collect()
}

/// Whether `path` is a real directory (no link to one) that `uid` owns.
fn own_dir(path: &Path, uid: u32) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_dir() && m.uid() == uid)
}

/// Whether the user may add to and take from the directory `path`; not so on a file system mounted read-only.
fn writable(path: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Ok(c) = std::ffi::CString::new(path.as_os_str().as_bytes()) else {
        return false;
    };
    // SAFETY: `c` is a NUL-terminated path that lives through the call
    unsafe { libc::access(c.as_ptr(), libc::W_OK | libc::X_OK) == 0 }
}

/// Delete `path` itself for good: a folder with everything in it, anything else (a link too, never what it points
/// at) alone. Nothing there is no error.
fn remove(path: &Path) -> io::Result<()> {
    let gone = match fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_dir() => fs::remove_dir_all(path),
        Ok(_) => fs::remove_file(path),
        Err(e) => Err(e),
    };
    match gone {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

/// A name for `name` that is free in the folder `files`: itself, or with a counter.
fn free_name(files: &Path, name: &str) -> String {
    let mut n = 1;
    loop {
        let candidate = if n == 1 {
            name.to_owned()
        } else {
            with_counter(name, n)
        };
        if fs::symlink_metadata(files.join(&candidate)).is_err() {
            return candidate;
        }
        n += 1;
    }
}

impl Trash {
    /// Trash with the default home location and the current user id.
    pub fn new() -> Self {
        let base = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
            .unwrap_or_else(|| PathBuf::from("."));
        Trash {
            home_trash: base.join("Trash"),
            uid: unsafe { libc::getuid() },
            mounts: None,
            system_volumes: true,
            volume_wait: VOLUME_ANSWERS_WITHIN,
            volume_look: None,
        }
    }

    /// A trash at a location of its own, for the current user id; it knows no volumes until it is given mount points.
    pub fn with_home(home_trash: impl Into<PathBuf>) -> Self {
        Trash {
            home_trash: home_trash.into(),
            uid: unsafe { libc::getuid() },
            mounts: None,
            system_volumes: false,
            volume_wait: VOLUME_ANSWERS_WITHIN,
            volume_look: None,
        }
    }

    pub fn with_mounts(mut self, mounts: Vec<PathBuf>) -> Self {
        self.mounts = Some(mounts);
        self
    }

    /// Wait `wait` for each volume to answer instead of [`VOLUME_ANSWERS_WITHIN`].
    pub fn with_volume_wait(mut self, wait: Duration) -> Self {
        self.volume_wait = wait;
        self
    }

    /// Call `look` with each mount point right before its folders are looked at, on the thread that looks: a test
    /// stands in for a volume that does not answer by not returning from it.
    pub fn with_volume_look(mut self, look: impl Fn(&Path) + Send + Sync + 'static) -> Self {
        self.volume_look = Some(VolumeLook(Arc::new(look)));
        self
    }

    pub fn home_trash(&self) -> &Path {
        &self.home_trash
    }

    fn device_of(p: &Path) -> io::Result<u64> {
        Ok(fs::metadata(p)?.dev())
    }

    fn topdir_of(&self, path: &Path) -> Option<PathBuf> {
        let mounts = self.mounts.clone().unwrap_or_else(mount_points);
        mounts
            .into_iter()
            .filter(|m| path.starts_with(m))
            .max_by_key(|m| m.as_os_str().len())
    }

    /// Trash directory to use for an item at `abs` (and whether `Path=` is relative to a topdir).
    fn trash_dir_for(&self, abs: &Path) -> Result<(PathBuf, Option<PathBuf>), TrashError> {
        fs::create_dir_all(&self.home_trash)?;
        let home_dev = Self::device_of(&self.home_trash)?;
        let item_dev = fs::symlink_metadata(abs)?.dev();
        if home_dev == item_dev {
            return Ok((self.home_trash.clone(), None));
        }
        let top = self.topdir_of(abs).ok_or(TrashError::NoTrashForDevice)?;
        let shared = top.join(".Trash");
        if let Ok(m) = fs::symlink_metadata(&shared) {
            if m.file_type().is_dir()
                && !m.file_type().is_symlink()
                && m.permissions().mode() & 0o1000 != 0
            {
                let d = shared.join(self.uid.to_string());
                fs::create_dir_all(&d)?;
                return Ok((d, Some(top)));
            }
        }
        let d = top.join(format!(".Trash-{}", self.uid));
        fs::create_dir_all(&d)?;
        let mut perm = fs::metadata(&d)?.permissions();
        perm.set_mode(0o700);
        fs::set_permissions(&d, perm)?;
        Ok((d, Some(top)))
    }

    /// The mount points whose trash folders count, each once: those given ([`Trash::with_mounts`]), else the
    /// system's for the trash of the default location, else none. Read from the kernel's list, which touches no
    /// volume.
    fn volumes(&self) -> Vec<PathBuf> {
        let mut tops = match &self.mounts {
            Some(mounts) => mounts.clone(),
            None if self.system_volumes => fs::read_to_string("/proc/self/mountinfo")
                .map(|s| volumes_of(&s))
                .unwrap_or_default(),
            None => Vec::new(),
        };
        let mut seen = std::collections::HashSet::new();
        tops.retain(|top| seen.insert(top.clone()));
        tops
    }

    /// The user's trash directories on the volume mounted at `top`, where they exist and can be written:
    /// `.Trash/$uid` when `.Trash` is a real directory with the sticky bit (never a link, which could lead
    /// anywhere), and `.Trash-$uid`. Each counts only as a real directory the user owns. This touches the volume.
    fn trash_dirs_on(&self, top: &Path) -> Vec<PathBuf> {
        if let Some(look) = &self.volume_look {
            (look.0)(top);
        }
        let shared = top.join(".Trash");
        let shared_ok = fs::symlink_metadata(&shared)
            .is_ok_and(|m| m.file_type().is_dir() && m.permissions().mode() & 0o1000 != 0);
        let candidates = [
            shared_ok.then(|| shared.join(self.uid.to_string())),
            Some(top.join(format!(".Trash-{}", self.uid))),
        ];
        candidates
            .into_iter()
            .flatten()
            .filter(|dir| *dir != self.home_trash && own_dir(dir, self.uid) && writable(dir))
            .collect()
    }

    /// What `read` makes of the trash directories of each mounted volume, by mount point; `None` for a volume another
    /// look was still out on after the time a volume is given. `read` runs on the calling thread with the volume
    /// taken, so it may read the directories it is handed.
    fn ask_volumes<T>(
        &self,
        read: impl Fn(&Trash, Vec<PathBuf>) -> T,
    ) -> Vec<(PathBuf, Option<T>)> {
        ask_volumes(self.volumes(), self.volume_wait, |top: &Path| {
            read(self, self.trash_dirs_on(top))
        })
    }

    /// The user's trash directories on the mounted volumes that answer ([`Trash::silent_volumes`] are left out),
    /// where they exist and can be written: none for a trash at a location of its own that was given no mount points
    /// ([`Trash::with_mounts`]); else, for each mount point, `.Trash/$uid` when `.Trash` is a real directory with the
    /// sticky bit, and `.Trash-$uid`, each a real directory the user owns.
    pub fn volume_trash_dirs(&self) -> Vec<PathBuf> {
        self.ask_volumes(|_, dirs| dirs)
            .into_iter()
            .filter_map(|(_, dirs)| dirs)
            .flatten()
            .collect()
    }

    /// The mount points another look is still out on after the time a volume is given: volumes that do not answer.
    pub fn silent_volumes(&self) -> Vec<PathBuf> {
        self.ask_volumes(|_, _| ())
            .into_iter()
            .filter_map(|(top, said)| said.is_none().then_some(top))
            .collect()
    }

    /// Every trash directory an emptying takes: the home trash, then [`Trash::volume_trash_dirs`].
    pub fn trash_dirs(&self) -> Vec<PathBuf> {
        let mut dirs = vec![self.home_trash.clone()];
        dirs.extend(self.volume_trash_dirs());
        dirs
    }

    /// Move `path` to the trash and return the record, a trashed folder's `directorysizes` line written.
    pub fn trash(&self, path: &Path) -> Result<TrashedItem, TrashError> {
        let (item, size) = self.trash_unsized(path)?;
        if let Some(size) = size {
            size.record()?;
        }
        Ok(item)
    }

    /// Move `path` to the trash as [`Trash::trash`] does, but leave a trashed folder's `directorysizes` line to the
    /// [`PendingSize`] returned with the record, which may be written later and on another thread: summing a large
    /// folder reads every file in it, while the move itself is one rename.
    pub fn trash_unsized(
        &self,
        path: &Path,
    ) -> Result<(TrashedItem, Option<PendingSize>), TrashError> {
        let abs = if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir()?.join(path)
        };
        // do not resolve the final component: a symlink is trashed itself
        let meta = fs::symlink_metadata(&abs)?;
        if abs.starts_with(&self.home_trash) {
            return Err(TrashError::AlreadyTrashed);
        }
        let (dir, top) = self.trash_dir_for(&abs)?;
        let files = dir.join("files");
        let info = dir.join("info");
        fs::create_dir_all(&files)?;
        fs::create_dir_all(&info)?;
        let base = abs
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
        let stored_path = match &top {
            Some(t) => abs
                .strip_prefix(t)
                .unwrap_or(&abs)
                .to_string_lossy()
                .into_owned(),
            None => abs.to_string_lossy().into_owned(),
        };
        let date = format_local_time(now_unix());
        // reserve the name by creating the info file exclusively (as the specification suggests)
        let mut n = 1;
        let (name, mut info_file, info_path) = loop {
            let cand = if n == 1 {
                base.clone()
            } else {
                with_counter(&base, n)
            };
            let ip = info.join(format!("{cand}.trashinfo"));
            match fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&ip)
            {
                Ok(f) if !files.join(&cand).exists() => break (cand, f, ip),
                Ok(_) => {
                    fs::remove_file(&ip)?;
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(e.into()),
            }
            n += 1;
        };
        write!(
            info_file,
            "[Trash Info]\nPath={}\nDeletionDate={}\n",
            encode_path(&stored_path),
            date
        )?;
        drop(info_file);
        if let Err(e) = fs::rename(&abs, files.join(&name)) {
            let _ = fs::remove_file(&info_path);
            return Err(e.into());
        }
        let size = meta.is_dir().then(|| PendingSize {
            trash_dir: dir.clone(),
            name: name.clone(),
        });
        let item = TrashedItem {
            name,
            trash_dir: dir,
            original_path: abs,
            deletion_date: date,
        };
        Ok((item, size))
    }

    /// List the items of one trash directory.
    pub fn list_dir(&self, dir: &Path) -> Result<Vec<TrashedItem>, TrashError> {
        let mut out = Vec::new();
        let Ok(rd) = fs::read_dir(dir.join("info")) else {
            return Ok(out);
        };
        let top = if dir == self.home_trash {
            None
        } else {
            dir.parent().map(Path::to_path_buf).map(|p| {
                if p.file_name().is_some_and(|n| n == ".Trash") {
                    p.parent().map(Path::to_path_buf).unwrap_or(p)
                } else {
                    p
                }
            })
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().is_none_or(|x| x != "trashinfo") {
                continue;
            }
            let name = p
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            let (path, date) = parse_info(&fs::read_to_string(&p)?)?;
            let original_path = match &top {
                Some(t) if !Path::new(&path).is_absolute() => t.join(&path),
                _ => PathBuf::from(path),
            };
            out.push(TrashedItem {
                name,
                trash_dir: dir.to_path_buf(),
                original_path,
                deletion_date: date,
            });
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    pub fn list(&self) -> Result<Vec<TrashedItem>, TrashError> {
        self.list_dir(&self.home_trash)
    }

    /// The items of every trash directory an emptying takes ([`Trash::trash_dirs`]), each directory's by name: the
    /// home trash's first, then those of the volumes that answer.
    pub fn list_all(&self) -> Result<Vec<TrashedItem>, TrashError> {
        let mut items = self.list()?;
        items.extend(self.list_volumes()?);
        Ok(items)
    }

    /// The items of the user's trash directories on the mounted volumes that answer, each directory's by name.
    pub fn list_volumes(&self) -> Result<Vec<TrashedItem>, TrashError> {
        let listed = self.ask_volumes(|trash, dirs| {
            let mut items = Vec::new();
            for dir in dirs {
                items.extend(trash.list_dir(&dir)?);
            }
            Ok::<_, TrashError>(items)
        });
        let mut items = Vec::new();
        for (_, volume) in listed {
            items.extend(volume.transpose()?.unwrap_or_default());
        }
        Ok(items)
    }

    /// The entries of the home trash's `files` folder, without reading their `.trashinfo`. No volume is touched.
    pub fn home_entries(&self) -> Vec<PathBuf> {
        entries(std::slice::from_ref(&self.home_trash))
    }

    /// The entries of the `files` folders of every trash directory an emptying takes, without reading their
    /// `.trashinfo`: a quick look at what it would delete (an entry without its `.trashinfo` too).
    pub fn entries(&self) -> Vec<PathBuf> {
        let mut all = self.home_entries();
        all.extend(self.volume_entries());
        all
    }

    /// The entries of the `files` folders of the user's trash directories on the mounted volumes that answer.
    pub fn volume_entries(&self) -> Vec<PathBuf> {
        self.ask_volumes(|_, dirs| entries(&dirs))
            .into_iter()
            .filter_map(|(_, entries)| entries)
            .flatten()
            .collect()
    }

    /// Whether the home trash holds anything, from its `files` folder alone. No volume is touched.
    pub fn home_holds_items(&self) -> bool {
        holds_items(std::slice::from_ref(&self.home_trash))
    }

    /// Whether the home trash or a volume's holds anything, from the `files` folders alone. The volumes are asked
    /// only when the home trash is empty.
    pub fn holds_items(&self) -> bool {
        self.home_holds_items() || self.volumes_hold_items()
    }

    /// Whether the trash of a mounted volume that answers holds anything, whatever the home trash holds.
    pub fn volumes_hold_items(&self) -> bool {
        self.ask_volumes(|_, dirs| holds_items(&dirs))
            .into_iter()
            .any(|(_, holds)| holds == Some(true))
    }

    /// Put a trashed item back at its original location (fails if the location exists).
    pub fn restore(&self, item: &TrashedItem) -> Result<PathBuf, TrashError> {
        let dest = &item.original_path;
        if fs::symlink_metadata(dest).is_ok() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "original location is occupied",
            )
            .into());
        }
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::rename(item.file_path(), dest)?;
        forget(item);
        Ok(dest.clone())
    }

    /// Permanently delete one trashed item.
    pub fn delete(&self, item: &TrashedItem) -> Result<(), TrashError> {
        remove(&item.file_path())?;
        forget(item);
        Ok(())
    }

    /// Empty the home trash and the user's trash on every mounted volume that can be written
    /// ([`Trash::trash_dirs`]), and touch nothing else. What could not be deleted is in the result's failures and
    /// stays in its trash. The home trash goes first, whatever the volumes do; a volume another look is still out on
    /// after the time a volume is given is left as it is and is one of the failures, by its mount point.
    pub fn empty(&self) -> Emptied {
        let mut emptied = Emptied::default();
        empty_dir(&self.home_trash, &mut emptied);
        for (top, dirs) in self.ask_volumes(|_, dirs| dirs) {
            match dirs {
                Some(dirs) => {
                    for dir in dirs {
                        empty_dir(&dir, &mut emptied);
                    }
                }
                None => emptied.failures.push(Failure {
                    path: top,
                    error: io::Error::new(io::ErrorKind::TimedOut, "the volume does not answer"),
                }),
            }
        }
        emptied
    }

    /// Delete items trashed more than `days` days ago (the "remove after 30 days" option, which the
    /// reference ships disabled by default).
    pub fn purge_older_than(&self, days: i64, now_unix_secs: i64) -> Result<usize, TrashError> {
        let mut n = 0;
        for it in self.list()? {
            if let Some(t) = parse_local_time(&it.deletion_date) {
                if now_unix_secs - t > days * 86_400 {
                    self.delete(&it)?;
                    n += 1;
                }
            }
        }
        Ok(n)
    }
}

/// Whether the `files` folder of one of the trash directories `dirs` has an entry.
fn holds_items(dirs: &[PathBuf]) -> bool {
    dirs.iter()
        .any(|dir| fs::read_dir(dir.join("files")).is_ok_and(|mut e| e.next().is_some()))
}

/// The entries of the `files` folders of the trash directories `dirs`.
fn entries(dirs: &[PathBuf]) -> Vec<PathBuf> {
    dirs.iter()
        .filter_map(|dir| fs::read_dir(dir.join("files")).ok())
        .flat_map(|entries| entries.flatten().map(|e| e.path()))
        .collect()
}

/// Empty the trash directory `dir` into `emptied`. Its `files` and `info` folders are swapped for empty ones first, so
/// the trash is empty at once (an entry without its `.trashinfo` goes too), and the old ones are deleted after, entry
/// by entry; an entry that cannot be deleted goes back into the trash with its `.trashinfo`, as a failure (one that
/// cannot even be moved back stays in the hidden folder the old ones went to, for the next emptying). Where the
/// swap is refused, the entries are deleted in place. A `files` that is a link is taken away itself, never followed.
fn empty_dir(dir: &Path, emptied: &mut Emptied) {
    let files = dir.join("files");
    let info = dir.join("info");
    match fs::symlink_metadata(&files) {
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => return,
        Err(error) => {
            emptied.failures.push(Failure { path: files, error });
            return;
        }
    }
    let expunged = dir.join(format!(
        ".expunged-{}-{}",
        std::process::id(),
        now_unix_nanos()
    ));
    let swapped = {
        let _held = sizes_lock();
        fs::create_dir(&expunged)
            .and_then(|()| fs::rename(&files, expunged.join("files")))
            .and_then(|()| fs::create_dir(&files))
            .and_then(|()| match fs::rename(&info, expunged.join("info")) {
                Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
                other => other,
            })
            .and_then(|()| fs::create_dir(&info))
            .map(|()| {
                let _ = fs::remove_file(dir.join("directorysizes"));
            })
    };
    if swapped.is_err() {
        // put back whatever moved, then delete in place
        if fs::symlink_metadata(&files).is_err() {
            let _ = fs::rename(expunged.join("files"), &files);
        }
        if fs::symlink_metadata(&info).is_err() {
            let _ = fs::rename(expunged.join("info"), &info);
        }
        let _ = fs::remove_dir_all(&expunged);
        for (name, error) in delete_entries(&files, &info, emptied) {
            emptied.failures.push(Failure {
                path: files.join(name),
                error,
            });
        }
        return;
    }
    let (old_files, old_info) = (expunged.join("files"), expunged.join("info"));
    // what stays goes back where the person can see it, under a free name when its own was taken meanwhile
    let old_records = real_dir(&old_info);
    for (name, error) in delete_entries(&old_files, &old_info, emptied) {
        let back = free_name(&files, &name);
        let path = match fs::rename(old_files.join(&name), files.join(&back)) {
            Ok(()) => {
                if old_records {
                    let _ = fs::rename(
                        old_info.join(format!("{name}.trashinfo")),
                        info.join(format!("{back}.trashinfo")),
                    );
                }
                files.join(back)
            }
            Err(_) => old_files.join(&name),
        };
        emptied.failures.push(Failure { path, error });
    }
    // this emptying's folder, and any an earlier one could not finish
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            if entry
                .file_name()
                .to_string_lossy()
                .starts_with(".expunged-")
            {
                let _ = remove(&entry.path());
            }
        }
    }
}

/// Whether `path` is a real directory, no link to one: records are only read from, moved out of and deleted in an
/// `info` that is one, so an `info` that is a link never leads a deletion into the folder it points at.
fn real_dir(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_dir())
}

/// Delete every entry of the folder `files` (itself taken away when it is no folder but a link or a file) and, with
/// each entry that went, its `.trashinfo` in `info` when `info` is a real folder; the count goes into `emptied`.
/// Returns the names of the entries that stayed, each with why.
fn delete_entries(files: &Path, info: &Path, emptied: &mut Emptied) -> Vec<(String, io::Error)> {
    let mut stayed = Vec::new();
    let records = real_dir(info);
    let entries = match fs::symlink_metadata(files) {
        Ok(m) if m.file_type().is_dir() => fs::read_dir(files),
        // no folder: whatever it leads to is not the trash's to delete
        Ok(_) => {
            let _ = fs::remove_file(files);
            return stayed;
        }
        Err(e) => Err(e),
    };
    let Ok(entries) = entries else {
        return stayed;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        match remove(&entry.path()) {
            Ok(()) => {
                emptied.deleted += 1;
                if records {
                    let _ = fs::remove_file(info.join(format!("{name}.trashinfo")));
                }
            }
            Err(error) => stayed.push((name, error)),
        }
    }
    stayed
}

/// The trashed item `path` is, when it is an item of a trash directory's `files` folder that has its `.trashinfo`:
/// whatever trash holds it, the home trash or a mount point's.
pub fn trashed_item(path: &Path) -> Option<TrashedItem> {
    let name = path.file_name()?.to_str()?.to_owned();
    let files = path.parent()?;
    if files.file_name()? != "files" {
        return None;
    }
    let trash_dir = files.parent()?.to_path_buf();
    let info = trash_dir.join("info").join(format!("{name}.trashinfo"));
    let (original, deletion_date) = parse_info(&fs::read_to_string(info).ok()?).ok()?;
    Some(TrashedItem {
        name,
        trash_dir,
        original_path: PathBuf::from(original),
        deletion_date,
    })
}

/// Forget `item`, which has left its trash's `files` folder: an item moved out of the trash is restored, so its
/// `.trashinfo` and its `directorysizes` line go (Trash specification 1.0), and nothing is left for an item that is not
/// there.
pub fn forget(item: &TrashedItem) {
    let _ = fs::remove_file(item.info_path());
    let path = item.trash_dir.join("directorysizes");
    let _held = sizes_lock();
    let Ok(s) = fs::read_to_string(&path) else {
        return;
    };
    let enc = encode_path(&item.name);
    let kept: Vec<&str> = s
        .lines()
        .filter(|l| l.splitn(3, ' ').nth(2) != Some(enc.as_str()))
        .collect();
    if kept.len() != s.lines().count() {
        let _ = fs::write(
            &path,
            kept.join("\n") + if kept.is_empty() { "" } else { "\n" },
        );
    }
}

impl Default for Trash {
    fn default() -> Self {
        Self::new()
    }
}

fn with_counter(name: &str, n: u32) -> String {
    match name.rfind('.') {
        Some(i) if i > 0 => format!("{} {}{}", &name[..i], n, &name[i..]),
        _ => format!("{name} {n}"),
    }
}

fn parse_info(text: &str) -> Result<(String, String), TrashError> {
    let mut path = None;
    let mut date = String::new();
    let mut in_group = false;
    for l in text.lines() {
        if l.trim() == "[Trash Info]" {
            in_group = true;
        } else if in_group {
            if let Some(v) = l.strip_prefix("Path=") {
                path = Some(decode_path(v));
            } else if let Some(v) = l.strip_prefix("DeletionDate=") {
                date = v.trim().to_string();
            }
        }
    }
    path.map(|p| (p, date))
        .ok_or_else(|| TrashError::BadInfo("missing Path".into()))
}

fn parse_local_time(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() != 19 {
        return None;
    }
    let num = |a: usize, z: usize| s.get(a..z)?.parse::<i32>().ok();
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    tm.tm_year = num(0, 4)? - 1900;
    tm.tm_mon = num(5, 7)? - 1;
    tm.tm_mday = num(8, 10)?;
    tm.tm_hour = num(11, 13)?;
    tm.tm_min = num(14, 16)?;
    tm.tm_sec = num(17, 19)?;
    tm.tm_isdst = -1;
    Some(unsafe { libc::mktime(&mut tm) } as i64)
}

fn dir_size(p: &Path) -> u64 {
    let mut total = 0;
    if let Ok(rd) = fs::read_dir(p) {
        for e in rd.flatten() {
            if let Ok(m) = e.metadata() {
                total += if m.is_dir() {
                    dir_size(&e.path())
                } else {
                    m.len()
                };
            }
        }
    }
    total
}
