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
//! Absolute locations are injectable through [`Trash`] so tests can use temporary directories.

use percent_encoding::{percent_decode_str, utf8_percent_encode, AsciiSet, CONTROLS};
use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

/// What the question asked before the trash is emptied says, for every program that asks it: its title, what happens
/// and the labels of its two buttons.
pub const EMPTY_TITLE: &str = "Empty the Trash?";
pub const EMPTY_MESSAGE: &str =
    "Everything in the Trash is deleted for good. This cannot be undone.";
pub const EMPTY_BUTTON: &str = "Empty Trash";
pub const CANCEL_BUTTON: &str = "Cancel";

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

#[derive(Clone, Debug)]
pub struct Trash {
    home_trash: PathBuf,
    uid: u32,
    /// Mount points to consider for other devices (longest match wins). Empty means read `/proc/self/mountinfo`.
    mounts: Option<Vec<PathBuf>>,
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

fn mount_points() -> Vec<PathBuf> {
    let Ok(s) = fs::read_to_string("/proc/self/mountinfo") else {
        return vec![PathBuf::from("/")];
    };
    s.lines()
        .filter_map(|l| l.split(' ').nth(4))
        .map(|m| PathBuf::from(m.replace("\\040", " ")))
        .collect()
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
        }
    }

    pub fn with_home(home_trash: impl Into<PathBuf>) -> Self {
        Trash {
            home_trash: home_trash.into(),
            uid: unsafe { libc::getuid() },
            mounts: None,
        }
    }

    pub fn with_mounts(mut self, mounts: Vec<PathBuf>) -> Self {
        self.mounts = Some(mounts);
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
        let f = item.file_path();
        match fs::symlink_metadata(&f) {
            Ok(m) if m.is_dir() => fs::remove_dir_all(&f)?,
            Ok(_) => fs::remove_file(&f)?,
            Err(_) => {}
        }
        forget(item);
        Ok(())
    }

    /// Empty the home trash.
    pub fn empty(&self) -> Result<usize, TrashError> {
        let items = self.list()?;
        for i in &items {
            self.delete(i)?;
        }
        Ok(items.len())
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

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> (tempfile::TempDir, Trash) {
        let d = tempfile::tempdir().unwrap();
        let t = Trash::with_home(d.path().join("share/Trash"));
        (d, t)
    }

    #[test]
    fn trash_and_restore_file() {
        let (d, t) = setup();
        let f = d.path().join("a b#c.txt");
        fs::write(&f, b"hi").unwrap();
        let item = t.trash(&f).unwrap();
        assert!(!f.exists());
        assert!(item.file_path().exists());
        let info = fs::read_to_string(item.info_path()).unwrap();
        assert!(info.starts_with("[Trash Info]\nPath="));
        assert!(info.contains("a%20b%23c.txt"), "{info}");
        assert!(info.contains("DeletionDate="));
        let listed = t.list().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].original_path, f);
        assert_eq!(t.restore(&listed[0]).unwrap(), f);
        assert_eq!(fs::read(&f).unwrap(), b"hi");
        assert!(t.list().unwrap().is_empty());
    }

    #[test]
    fn a_folders_size_is_written_when_asked_and_not_once_it_has_left_the_trash() {
        let (d, t) = setup();
        let folder = d.path().join("dir");
        fs::create_dir(&folder).unwrap();
        fs::write(folder.join("f"), vec![0u8; 10]).unwrap();
        let (item, size) = t.trash_unsized(&folder).unwrap();
        let sizes = item.trash_dir.join("directorysizes");
        assert!(item.file_path().is_dir(), "the folder is moved at once");
        assert!(!sizes.exists(), "its size is not summed yet");
        let size = size.expect("a folder has a size to write");
        size.record().unwrap();
        assert!(fs::read_to_string(&sizes).unwrap().starts_with("10 "));

        let other = d.path().join("other");
        fs::create_dir(&other).unwrap();
        let (item, size) = t.trash_unsized(&other).unwrap();
        t.restore(&item).unwrap();
        size.unwrap().record().unwrap();
        assert_eq!(fs::read_to_string(&sizes).unwrap().lines().count(), 1);

        let file = d.path().join("file");
        fs::write(&file, "f").unwrap();
        assert_eq!(t.trash_unsized(&file).unwrap().1, None);
    }

    #[test]
    fn sizes_written_on_threads_while_lines_are_forgotten_all_stay_whole() {
        let (d, t) = setup();
        let trash = |name: &str| {
            let folder = d.path().join(name);
            fs::create_dir(&folder).unwrap();
            fs::write(folder.join("f"), vec![0u8; 7]).unwrap();
            t.trash_unsized(&folder).unwrap()
        };
        let kept: Vec<_> = (0..24).map(|i| trash(&format!("kept {i}"))).collect();
        let gone: Vec<_> = (0..24).map(|i| trash(&format!("gone {i}"))).collect();
        for (_, size) in &gone {
            size.as_ref().unwrap().record().unwrap();
        }
        let threads: Vec<_> = kept
            .iter()
            .map(|(_, size)| {
                let size = size.clone().unwrap();
                std::thread::spawn(move || size.record().unwrap())
            })
            .collect();
        // each restore rewrites the file without its line while the threads append theirs
        for (item, _) in &gone {
            t.restore(item).unwrap();
        }
        for thread in threads {
            thread.join().unwrap();
        }
        let sizes = fs::read_to_string(kept[0].0.trash_dir.join("directorysizes")).unwrap();
        let mut names: Vec<&str> = sizes
            .lines()
            .map(|line| {
                let fields: Vec<&str> = line.splitn(3, ' ').collect();
                assert_eq!(fields.len(), 3, "a whole line: {line:?}");
                assert_eq!(fields[0], "7", "{line:?}");
                fields[2]
            })
            .collect();
        names.sort_unstable();
        let mut expected: Vec<String> = kept
            .iter()
            .map(|(item, _)| encode_path(&item.name))
            .collect();
        expected.sort_unstable();
        assert_eq!(names, expected);
    }

    #[test]
    fn name_clashes_get_counters() {
        let (d, t) = setup();
        let mut names = Vec::new();
        for _ in 0..3 {
            let f = d.path().join("x.txt");
            fs::write(&f, b"1").unwrap();
            names.push(t.trash(&f).unwrap().name);
        }
        assert_eq!(names, ["x.txt", "x 2.txt", "x 3.txt"]);
    }

    #[test]
    fn directories_update_directorysizes() {
        let (d, t) = setup();
        let dir = d.path().join("dir");
        fs::create_dir(&dir).unwrap();
        fs::write(dir.join("f"), vec![0u8; 100]).unwrap();
        let item = t.trash(&dir).unwrap();
        let sizes = fs::read_to_string(item.trash_dir.join("directorysizes")).unwrap();
        assert!(sizes.starts_with("100 "), "{sizes}");
        assert!(sizes.trim_end().ends_with(" dir"));
        t.delete(&item).unwrap();
        assert_eq!(
            fs::read_to_string(item.trash_dir.join("directorysizes"))
                .unwrap()
                .trim(),
            ""
        );
    }

    #[test]
    fn restore_refuses_to_overwrite() {
        let (d, t) = setup();
        let f = d.path().join("a");
        fs::write(&f, b"1").unwrap();
        let item = t.trash(&f).unwrap();
        fs::write(&f, b"2").unwrap();
        assert!(t.restore(&item).is_err());
        assert_eq!(fs::read(&f).unwrap(), b"2");
    }

    #[test]
    fn empty_and_purge() {
        let (d, t) = setup();
        for n in ["a", "b"] {
            let f = d.path().join(n);
            fs::write(&f, b"1").unwrap();
            t.trash(&f).unwrap();
        }
        assert_eq!(t.purge_older_than(30, now_unix()).unwrap(), 0);
        assert_eq!(t.purge_older_than(30, now_unix() + 31 * 86_400).unwrap(), 2);
        let f = d.path().join("c");
        fs::write(&f, b"1").unwrap();
        t.trash(&f).unwrap();
        assert_eq!(t.empty().unwrap(), 1);
    }

    #[test]
    fn items_in_the_trash_are_not_trashed_again() {
        let (d, t) = setup();
        let f = d.path().join("a");
        fs::write(&f, b"1").unwrap();
        let item = t.trash(&f).unwrap();
        assert!(matches!(
            t.trash(&item.file_path()),
            Err(TrashError::AlreadyTrashed)
        ));
    }

    #[test]
    fn info_parsing_and_encoding() {
        assert_eq!(
            decode_path(&encode_path("/home/u/é ü/#1")),
            "/home/u/é ü/#1"
        );
        let (p, d) =
            parse_info("[Trash Info]\nPath=/a%20b\nDeletionDate=2026-01-02T03:04:05\n").unwrap();
        assert_eq!((p.as_str(), d.as_str()), ("/a b", "2026-01-02T03:04:05"));
        assert!(parse_info("[Trash Info]\n").is_err());
        assert!(parse_local_time("2026-01-02T03:04:05").is_some());
    }

    #[test]
    fn mount_point_selection_prefers_longest_match() {
        let t = Trash::with_home("/tmp/x").with_mounts(vec![
            "/".into(),
            "/mnt".into(),
            "/mnt/usb".into(),
        ]);
        assert_eq!(
            t.topdir_of(Path::new("/mnt/usb/a/b")),
            Some(PathBuf::from("/mnt/usb"))
        );
        assert_eq!(t.topdir_of(Path::new("/etc/x")), Some(PathBuf::from("/")));
    }
}
