//! The user store: segments of JSON lines in `$XDG_STATE_HOME/cupertino/events/`, bounded by size and age, shared by
//! every process of the user under one lock.
//!
//! - The directory is mode 0700 and its files 0600. A directory that is a symbolic link, is not owned by the user, or
//!   that the group or others may write is refused: the store is then unavailable.
//! - Segments are `events-<boot>-<seq>.jsonl`, `<boot>` the first 8 digits of the boot id and `<seq>` a number that grows
//!   across boots; `fatal-<boot>-<pid>.jsonl` holds what a fatal signal wrote with no lock.
//! - A line is written with one `write(2)` to a file opened with `O_APPEND`, under `flock(LOCK_EX)` on `lock`, so lines
//!   of several processes never mix. A short write is cut back off, so the next line starts on a line of its own.
//! - Under the lock, a segment that a line would take past [`SEGMENT_BYTES`] is closed and the next one opened, and the
//!   oldest segments are deleted while the store holds more than [`STORE_BYTES`] or they are older than
//!   [`RETENTION_DAYS`].

use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use crate::event::{Corrupt, Record};
use crate::redact::Salt;

/// The size at which a segment is closed and the next one opened.
pub const SEGMENT_BYTES: u64 = 1 << 20;
/// The most the store holds; the oldest segments go first.
pub const STORE_BYTES: u64 = 8 << 20;
/// How long a segment is kept after its last write.
pub const RETENTION_DAYS: u64 = 14;
/// The longest line of one event, newline included; a longer event loses field text, then fields.
pub const EVENT_BYTES: usize = 4096;

/// The store's limits; [`Limits::default`] is the shipped one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    pub segment_bytes: u64,
    pub store_bytes: u64,
    pub retention: Duration,
    pub event_bytes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            segment_bytes: SEGMENT_BYTES,
            store_bytes: STORE_BYTES,
            retention: Duration::from_secs(RETENTION_DAYS * 86_400),
            event_bytes: EVENT_BYTES,
        }
    }
}

/// Why the store cannot be written.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Unavailable {
    /// Neither `XDG_STATE_HOME` nor `HOME` is set.
    NoHome,
    /// The store directory is a symbolic link.
    Symlink,
    /// The store directory is not a directory.
    NotADirectory,
    /// The store directory belongs to another user.
    NotOwned,
    /// The group or others may write the store directory.
    Writable,
    /// The file system is read-only.
    ReadOnly,
    /// The file system or the user's quota is full.
    NoSpace,
    /// Permission denied.
    Denied,
    /// Another error.
    Io,
}

impl Unavailable {
    /// The stable name of the reason, as `diag.store.unavailable` gives it.
    pub fn code(self) -> &'static str {
        match self {
            Unavailable::NoHome => "no_home",
            Unavailable::Symlink => "symlink",
            Unavailable::NotADirectory => "not_a_directory",
            Unavailable::NotOwned => "not_owned",
            Unavailable::Writable => "writable_by_others",
            Unavailable::ReadOnly => "read_only",
            Unavailable::NoSpace => "no_space",
            Unavailable::Denied => "denied",
            Unavailable::Io => "io_error",
        }
    }

    pub fn describe(self) -> &'static str {
        match self {
            Unavailable::NoHome => "neither XDG_STATE_HOME nor HOME is set",
            Unavailable::Symlink => "the store directory is a symbolic link",
            Unavailable::NotADirectory => "the store path is not a directory",
            Unavailable::NotOwned => "the store directory belongs to another user",
            Unavailable::Writable => "others may write the store directory",
            Unavailable::ReadOnly => "the file system is read-only",
            Unavailable::NoSpace => "the disk is full",
            Unavailable::Denied => "permission denied",
            Unavailable::Io => "the store cannot be written",
        }
    }

    pub fn of(e: &io::Error) -> Self {
        match e.kind() {
            io::ErrorKind::ReadOnlyFilesystem => Unavailable::ReadOnly,
            io::ErrorKind::StorageFull | io::ErrorKind::QuotaExceeded => Unavailable::NoSpace,
            io::ErrorKind::PermissionDenied => Unavailable::Denied,
            io::ErrorKind::NotADirectory => Unavailable::NotADirectory,
            _ => Unavailable::Io,
        }
    }
}

/// `$XDG_STATE_HOME/cupertino/events`, else `$HOME/.local/state/cupertino/events`, from `var`.
pub fn user_dir(var: impl Fn(&str) -> Option<std::ffi::OsString>) -> Option<PathBuf> {
    let set = |k: &str| var(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    let state = set("XDG_STATE_HOME")
        .filter(|p| p.is_absolute())
        .or_else(|| set("HOME").map(|h| h.join(".local/state")))?;
    Some(state.join("cupertino/events"))
}

/// How a line is written: one `write(2)`; a test replaces it to fail like a full disk.
pub type WriteFn = fn(&File, &[u8]) -> io::Result<usize>;

fn write_once(file: &File, line: &[u8]) -> io::Result<usize> {
    (&*file).write(line)
}

/// The clock age expiry compares segments with.
pub type Clock = fn() -> SystemTime;

/// A segment file of the store.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Segment {
    pub path: PathBuf,
    /// The number of an `events-` segment; `None` for a `fatal-` file.
    pub seq: Option<u64>,
    pub len: u64,
    pub modified: SystemTime,
}

/// What [`Store::append`] did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Appended {
    pub segment: PathBuf,
    /// How many old segments were deleted to stay within the limits.
    pub deleted: usize,
}

/// A store directory, opened.
pub struct Store {
    dir: PathBuf,
    limits: Limits,
    boot: String,
    clock: Clock,
    write: WriteFn,
    lock: File,
}

impl Store {
    /// Opens the store in `dir`, making it (mode 0700) when it does not exist, and checks it is the user's own.
    pub fn open(dir: &Path, limits: Limits) -> Result<Store, Unavailable> {
        prepare_dir(dir)?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(dir.join("lock"))
            .map_err(|e| Unavailable::of(&e))?;
        Ok(Store {
            dir: dir.to_owned(),
            limits,
            boot: boot_prefix(crate::emitter::boot_id().as_deref()),
            clock: SystemTime::now,
            write: write_once,
            lock,
        })
    }

    pub fn with_clock(mut self, clock: Clock) -> Self {
        self.clock = clock;
        self
    }

    pub fn with_writer(mut self, write: WriteFn) -> Self {
        self.write = write;
        self
    }

    pub fn with_boot(mut self, boot_id: &str) -> Self {
        self.boot = boot_prefix(Some(boot_id));
        self
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn limits(&self) -> Limits {
        self.limits
    }

    /// The store's salt, `salt` (0600), made on first use.
    pub fn salt(&self) -> io::Result<Salt> {
        Salt::load_or_create(&self.dir.join("salt"))
    }

    /// Appends one line (it ends with a newline) under the lock, rotating and deleting as the limits say.
    pub fn append(&self, line: &[u8]) -> Result<Appended, Unavailable> {
        check_dir(&self.dir)?;
        let _held = Held::lock(&self.lock)?;
        let segments = segments(&self.dir).map_err(|e| Unavailable::of(&e))?;
        let last = segments
            .iter()
            .filter(|s| s.seq.is_some())
            .max_by_key(|s| s.seq);
        let (path, len) = match last {
            Some(s) if s.len == 0 || s.len + line.len() as u64 <= self.limits.segment_bytes => {
                (s.path.clone(), s.len)
            }
            other => {
                let seq = other.and_then(|s| s.seq).unwrap_or(0) + 1;
                (
                    self.dir
                        .join(format!("events-{}-{seq:08}.jsonl", self.boot)),
                    0,
                )
            }
        };
        let file = OpenOptions::new()
            .read(true)
            .append(true)
            .create(true)
            .mode(0o600)
            .open(&path)
            .map_err(|e| Unavailable::of(&e))?;
        let before = file.metadata().map(|m| m.len()).unwrap_or(len);
        // a line a crash cut off stays a line of its own: the next one starts after a newline
        let whole;
        let line = if ends_open(&file, before) {
            whole = [b"\n", line].concat();
            &whole[..]
        } else {
            line
        };
        match (self.write)(&file, line) {
            Ok(n) if n == line.len() => {}
            Ok(_) => {
                let _ = file.set_len(before);
                return Err(Unavailable::NoSpace);
            }
            Err(e) => {
                let _ = file.set_len(before);
                return Err(Unavailable::of(&e));
            }
        }
        drop(file);
        let deleted = self.enforce(&path);
        Ok(Appended {
            segment: path,
            deleted,
        })
    }

    /// Deletes expired segments, then the oldest while the store is over its size; never `current`.
    fn enforce(&self, current: &Path) -> usize {
        let Ok(mut all) = segments(&self.dir) else {
            return 0;
        };
        let now = (self.clock)();
        let mut deleted = 0;
        all.retain(|s| {
            let expired = s.path != current
                && now
                    .duration_since(s.modified)
                    .is_ok_and(|age| age > self.limits.retention);
            if expired && remove(&s.path) {
                deleted += 1;
                return false;
            }
            true
        });
        let mut total: u64 = all.iter().map(|s| s.len).sum();
        for s in &all {
            if total <= self.limits.store_bytes {
                break;
            }
            if s.path != current && remove(&s.path) {
                total -= s.len;
                deleted += 1;
            }
        }
        deleted
    }

    /// Deletes every segment under the lock; the salt stays. How many were deleted.
    pub fn clear(&self) -> Result<usize, Unavailable> {
        check_dir(&self.dir)?;
        let _held = Held::lock(&self.lock)?;
        let all = segments(&self.dir).map_err(|e| Unavailable::of(&e))?;
        let mut deleted = 0;
        for s in all {
            match fs::remove_file(&s.path) {
                Ok(()) => deleted += 1,
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return Err(Unavailable::of(&e)),
            }
        }
        Ok(deleted)
    }
}

/// Whether the `len` bytes of `file` end without a newline: the last line was cut off.
fn ends_open(file: &File, len: u64) -> bool {
    use std::os::unix::fs::FileExt;
    let mut last = [0u8; 1];
    len > 0 && file.read_exact_at(&mut last, len - 1).is_ok() && last[0] != b'\n'
}

fn remove(path: &Path) -> bool {
    match fs::remove_file(path) {
        Ok(()) => true,
        // another process deleted it under the lock it held before
        Err(e) => e.kind() == io::ErrorKind::NotFound,
    }
}

/// The lock on `lock`, released when dropped.
struct Held<'f>(&'f File);

impl<'f> Held<'f> {
    fn lock(file: &'f File) -> Result<Self, Unavailable> {
        file.lock().map_err(|e| Unavailable::of(&e))?;
        Ok(Held(file))
    }
}

impl Drop for Held<'_> {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

/// The first 8 hexadecimal digits of the boot id, or `unknown0`.
fn boot_prefix(boot_id: Option<&str>) -> String {
    let digits: String = boot_id
        .unwrap_or("")
        .chars()
        .filter(char::is_ascii_hexdigit)
        .take(8)
        .collect::<String>()
        .to_ascii_lowercase();
    if digits.len() == 8 {
        digits
    } else {
        "unknown0".to_owned()
    }
}

/// The effective user id, read from the owner of the process's own `/proc` entry.
pub(crate) fn uid() -> Option<u32> {
    fs::metadata("/proc/self").ok().map(|m| m.uid())
}

/// Makes `dir` when it does not exist (its parents as usual, itself 0700), then [`check_dir`].
fn prepare_dir(dir: &Path) -> Result<(), Unavailable> {
    match fs::symlink_metadata(dir) {
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            if let Some(parent) = dir.parent() {
                fs::create_dir_all(parent).map_err(|e| Unavailable::of(&e))?;
            }
            match DirBuilder::new().mode(0o700).create(dir) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(Unavailable::of(&e)),
            }
        }
        Err(e) => return Err(Unavailable::of(&e)),
    }
    check_dir(dir)
}

/// The store directory is a real directory of the user's own that no one else may write.
pub fn check_dir(dir: &Path) -> Result<(), Unavailable> {
    let meta = fs::symlink_metadata(dir).map_err(|e| Unavailable::of(&e))?;
    if meta.file_type().is_symlink() {
        return Err(Unavailable::Symlink);
    }
    if !meta.is_dir() {
        return Err(Unavailable::NotADirectory);
    }
    if uid().is_some_and(|uid| uid != meta.uid()) {
        return Err(Unavailable::NotOwned);
    }
    if meta.permissions().mode() & 0o022 != 0 {
        return Err(Unavailable::Writable);
    }
    Ok(())
}

/// The segments in `dir`, oldest first: `events-` segments by number, and each `fatal-` file, by time, before the first
/// segment written after it.
pub fn segments(dir: &Path) -> io::Result<Vec<Segment>> {
    let mut out = Vec::new();
    for entry in fs::read_dir(dir)? {
        let Ok(entry) = entry else { continue };
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Some(stem) = name.strip_suffix(".jsonl") else {
            continue;
        };
        let seq = if let Some(rest) = stem.strip_prefix("events-") {
            let Some(seq) = rest.rsplit('-').next().and_then(|s| s.parse::<u64>().ok()) else {
                continue;
            };
            Some(seq)
        } else if stem.starts_with("fatal-") {
            None
        } else {
            continue;
        };
        // removed meanwhile by another process
        let Ok(meta) = fs::symlink_metadata(entry.path()) else {
            continue;
        };
        if !meta.is_file() {
            continue;
        }
        out.push(Segment {
            path: entry.path(),
            seq,
            len: meta.len(),
            modified: meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
        });
    }
    // one order for both kinds, whatever the clock did between the writes: segment numbers only grow, times may not
    let (mut events, mut fatal): (Vec<Segment>, Vec<Segment>) =
        out.into_iter().partition(|s| s.seq.is_some());
    events.sort_by_key(|s| s.seq);
    fatal.sort_by(|a, b| {
        a.modified
            .cmp(&b.modified)
            .then_with(|| a.path.cmp(&b.path))
    });
    let mut fatal = fatal.into_iter().peekable();
    let mut ordered = Vec::with_capacity(events.len() + fatal.len());
    for segment in events {
        while let Some(f) = fatal.next_if(|f| f.modified < segment.modified) {
            ordered.push(f);
        }
        ordered.push(segment);
    }
    ordered.extend(fatal);
    Ok(ordered)
}

/// One record read back, with its line as written.
#[derive(Clone, Debug, PartialEq)]
pub struct Stored {
    pub record: Record,
    pub line: String,
}

/// What reading a store gave.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Contents {
    pub records: Vec<Stored>,
    /// Lines that are not records of this schema: garbage, invalid UTF-8, another schema, a cut line in the middle.
    pub corrupt: usize,
    /// Last lines of a segment without their newline that are not a record: a write cut short.
    pub partial: usize,
    /// Segments that went away while they were read.
    pub vanished: usize,
}

/// Reads every segment of `dir`, oldest first. A last line without its newline is counted as `partial` unless it is a
/// whole record.
pub fn read(dir: &Path) -> io::Result<Contents> {
    read_segments(&segments(dir)?)
}

/// Reads `segments` in order; one that is gone is counted as `vanished`.
pub fn read_segments(segments: &[Segment]) -> io::Result<Contents> {
    let mut contents = Contents::default();
    for segment in segments {
        let mut bytes = Vec::new();
        match File::open(&segment.path).and_then(|mut f| f.read_to_end(&mut bytes)) {
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                contents.vanished += 1;
                continue;
            }
            Err(e) => return Err(e),
        }
        read_lines(&bytes, &mut contents);
    }
    Ok(contents)
}

/// Adds the records of one segment's bytes to `contents`.
pub fn read_lines(bytes: &[u8], contents: &mut Contents) {
    let complete = bytes.ends_with(b"\n");
    let lines: Vec<&[u8]> = bytes.split(|b| *b == b'\n').collect();
    let count = lines.len();
    for (i, raw) in lines.into_iter().enumerate() {
        let last = i + 1 == count;
        if raw.is_empty() {
            continue;
        }
        let text = String::from_utf8_lossy(raw);
        match Record::parse(&text) {
            Ok(record) => contents.records.push(Stored {
                record,
                line: text.into_owned(),
            }),
            // the tail of a write in progress, or of one cut short
            Err(Corrupt::Malformed) if last && !complete => contents.partial += 1,
            Err(_) => contents.corrupt += 1,
        }
    }
}
