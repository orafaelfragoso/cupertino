//! What the window changes on disk besides the trash (`trash`): renaming in place, creating a folder and dropping
//! items into a folder. Nothing here overwrites: a clash renames the arriving item (drops) or fails (renames). A copy
//! or a move runs in slices ([`Transfer`]), which the blocking functions ([`transfer`], [`drop_into`],
//! [`duplicate_with`]) run one after the other.

use crate::rename::unique_name;
use cupertino_trash as trash;
use std::fs;
use std::io::{self, Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Name of a new folder, then with ` 2`, ` 3` … after it. (the design's default name; the numbering is
/// approximate).
const NEW_FOLDER_NAME: &str = "untitled folder";

/// Move `from` to `to` unless `to` exists (`renameat2` with `RENAME_NOREPLACE`, atomic).
fn rename_no_replace(from: &Path, to: &Path) -> io::Result<()> {
    let c = |p: &Path| {
        std::ffi::CString::new(p.as_os_str().as_bytes())
            .map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))
    };
    let (from, to) = (c(from)?, c(to)?);
    // SAFETY: both pointers are NUL-terminated strings that outlive the call.
    let rc = unsafe {
        libc::renameat2(
            libc::AT_FDCWD,
            from.as_ptr(),
            libc::AT_FDCWD,
            to.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if rc == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// Rename the item at `path` to `new_name` in its folder and return the new path; fails with `AlreadyExists` when
/// that name is taken. The name must already be valid (`rename::validate_name`).
pub fn rename_item(path: &Path, new_name: &str) -> io::Result<PathBuf> {
    let target = path.with_file_name(new_name);
    rename_no_replace(path, &target)?;
    Ok(target)
}

/// Create an empty folder in `dir` with the first free default name and return its path.
pub fn new_folder(dir: &Path) -> io::Result<PathBuf> {
    for n in 1.. {
        let path = dir.join(if n == 1 {
            NEW_FOLDER_NAME.to_string()
        } else {
            format!("{NEW_FOLDER_NAME} {n}")
        });
        match fs::create_dir(&path) {
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            made => return made.map(|()| path),
        }
    }
    unreachable!("the counter does not run out")
}

/// Copy each of `paths` next to itself, with the first free `name copy` name (the design's Duplicate).
pub fn duplicate(paths: &[PathBuf]) -> Vec<io::Result<PathBuf>> {
    duplicate_with(paths, &Cancel::default(), &mut |_| {})
}

/// [`duplicate`] that reports its [`Progress`] over all of `paths` to `progress` and stops at `cancel`, as
/// [`transfer`] does.
pub fn duplicate_with(
    paths: &[PathBuf],
    cancel: &Cancel,
    progress: &mut dyn FnMut(&Progress),
) -> Vec<io::Result<PathBuf>> {
    let items = paths
        .iter()
        .map(|path| (path.clone(), path.parent().map(Path::to_path_buf)))
        .collect();
    Transfer::new(items, DropAction::Copy, cancel.clone()).run_all(progress)
}

/// Make a symbolic link to `path` named `name alias` (then `name alias 2` …) in its folder and return it. The link
/// holds the absolute path, so it survives the link being moved.
pub fn make_alias(path: &Path) -> io::Result<PathBuf> {
    let dir = path
        .parent()
        .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?
        .to_string_lossy()
        .into_owned();
    let target = fs::canonicalize(path)?;
    for n in 1.. {
        let link = dir.join(if n == 1 {
            format!("{name} alias")
        } else {
            format!("{name} alias {n}")
        });
        match std::os::unix::fs::symlink(&target, &link) {
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            made => return made.map(|()| link),
        }
    }
    unreachable!("the counter does not run out")
}

/// Create a folder in `dir`, move `items` into it and return it. The first failure stops the moves and is returned; the
/// folder and what moved stay.
pub fn new_folder_with(dir: &Path, items: &[PathBuf]) -> io::Result<PathBuf> {
    let folder = new_folder(dir)?;
    for item in items {
        for moved in drop_into(std::slice::from_ref(item), &folder, DropAction::Move) {
            moved?;
        }
    }
    Ok(folder)
}

/// What dropping items on a folder does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DropAction {
    Move,
    Copy,
}

/// The action of a drop: Alt copies, Super moves, and with neither the items move within a volume and copy across
/// volumes.
pub fn drop_action(same_volume: bool, alt: bool, super_key: bool) -> DropAction {
    match (alt, super_key) {
        (true, _) => DropAction::Copy,
        (false, true) => DropAction::Move,
        (false, false) if same_volume => DropAction::Move,
        (false, false) => DropAction::Copy,
    }
}

/// Whether both paths are on one volume.
pub fn same_volume(a: &Path, b: &Path) -> io::Result<bool> {
    Ok(fs::metadata(a)?.dev() == fs::metadata(b)?.dev())
}

/// One item a copy took: its path in the source and what it was when it was read.
struct Copied {
    path: PathBuf,
    meta: fs::Metadata,
}

/// How much of a file a copy reads and writes before it reports its progress and looks for a cancel.
const COPY_CHUNK: usize = 1 << 20;

/// What stops a transfer: set from any thread, it is seen before the next item and between two chunks of a file.
#[derive(Clone, Debug, Default)]
pub struct Cancel(Arc<AtomicBool>);

impl Cancel {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }

    fn check(&self) -> io::Result<()> {
        match self.is_cancelled() {
            true => Err(cancelled()),
            false => Ok(()),
        }
    }
}

/// The error a stopped transfer gives: its own type, so an interrupted call of the file system is not taken for one.
#[derive(Debug)]
struct Stopped;

impl std::fmt::Display for Stopped {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the transfer was stopped")
    }
}

impl std::error::Error for Stopped {}

fn cancelled() -> io::Error {
    io::Error::new(io::ErrorKind::Interrupted, Stopped)
}

/// The error of an item a stopped transfer did not reach, which [`is_cancelled`] tells.
pub fn stopped() -> io::Error {
    cancelled()
}

/// Whether `error` is a transfer's stop rather than a failure: it is not reported.
pub fn is_cancelled(error: &io::Error) -> bool {
    error.get_ref().is_some_and(|e| e.is::<Stopped>())
}

/// How far a transfer has come.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Progress {
    /// Bytes of the items' files, links and folders done, and in all.
    pub bytes: u64,
    pub total_bytes: u64,
    /// Items (sources) done, and in all.
    pub items: usize,
    pub total_items: usize,
    /// The name of the item being transferred.
    pub current: Option<String>,
}

impl Progress {
    /// How much is done, from 0 to 1: by bytes, or by items when the items hold none.
    pub fn fraction(&self) -> f64 {
        let (done, total) = match self.total_bytes {
            0 => (self.items as f64, self.total_items as f64),
            total => (self.bytes as f64, total as f64),
        };
        if total > 0.0 {
            (done / total).clamp(0.0, 1.0)
        } else {
            0.0
        }
    }
}

/// The bytes a transfer of `path` counts: a file's length, and one for a folder or a link, so a tree of empty items
/// still moves the progress. Read without following links; a stop ends the count, so a large tree does not hold it.
fn weight(path: &Path, cancel: &Cancel) -> u64 {
    if cancel.is_cancelled() {
        return 0;
    }
    let Ok(meta) = fs::symlink_metadata(path) else {
        return 0;
    };
    if meta.is_dir() {
        let children = fs::read_dir(path).into_iter().flatten().flatten();
        1 + children.map(|c| weight(&c.path(), cancel)).sum::<u64>()
    } else if meta.is_file() {
        meta.len().max(1)
    } else {
        1
    }
}

/// What is left of copying one item, a step at a time: the tree is walked depth first, each folder made before its
/// items and given its source's permissions after them, each file copied a chunk at a time.
struct ItemCopy {
    /// Where the item arrives: the first item made, so the copy's to remove when it fails.
    target: PathBuf,
    /// For a move across volumes, the item's trash record, if it is in a trash: the originals are removed once the
    /// copy is made.
    moving: Option<Option<trash::TrashedItem>>,
    /// The steps left, the next last.
    steps: Vec<Step>,
    /// The file being copied.
    file: Option<OpenFile>,
    copied: Vec<Copied>,
}

enum Step {
    /// Copy the item at `.0` to `.1`.
    Visit(PathBuf, PathBuf),
    /// The items of the folder made at `.0` are copied: give it its source's permissions.
    Seal(PathBuf, fs::Permissions),
}

/// A file being copied: its source and copy open, what the source was, and the buffer of its chunks.
struct OpenFile {
    source: fs::File,
    target: fs::File,
    meta: fs::Metadata,
    buffer: Vec<u8>,
}

impl ItemCopy {
    fn new(from: &Path, to: PathBuf, moving: Option<Option<trash::TrashedItem>>) -> ItemCopy {
        ItemCopy {
            steps: vec![Step::Visit(from.to_path_buf(), to.clone())],
            target: to,
            moving,
            file: None,
            copied: Vec::new(),
        }
    }

    /// One step, `advance` taking the bytes it did; the item's result once it is copied (and, for a move, its originals
    /// removed) or has failed, which removes what the copy made.
    fn step(
        &mut self,
        cancel: &Cancel,
        advance: &mut dyn FnMut(u64),
    ) -> Option<io::Result<PathBuf>> {
        match self.copy_step(cancel, advance) {
            Ok(false) => None,
            Ok(true) => Some(self.made()),
            Err(e) => {
                self.file = None;
                // the first item copied is the target itself: made by this copy, so it is this copy's to remove
                if !self.copied.is_empty() {
                    let _ = remove_made(&self.target);
                }
                Some(Err(e))
            }
        }
    }

    /// The copy is made: a move removes its originals.
    fn made(&mut self) -> io::Result<PathBuf> {
        if let Some(trashed) = self.moving.take() {
            remove_copied(&self.copied)?;
            if let Some(item) = trashed {
                trash::forget(&item);
            }
        }
        Ok(self.target.clone())
    }

    /// Copy one chunk of the file being copied, or take the next step; whether the copy is made.
    fn copy_step(&mut self, cancel: &Cancel, advance: &mut dyn FnMut(u64)) -> io::Result<bool> {
        cancel.check()?;
        if let Some(file) = self.file.as_mut() {
            let read = loop {
                match file.source.read(&mut file.buffer) {
                    Ok(read) => break read,
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    Err(e) => return Err(e),
                }
            };
            if read > 0 {
                file.target.write_all(&file.buffer[..read])?;
                advance(read as u64);
                cancel.check()?;
                return Ok(false);
            }
            if file.meta.len() == 0 {
                advance(1);
            }
            file.target.set_permissions(file.meta.permissions())?;
            self.file = None;
            return Ok(self.steps.is_empty());
        }
        match self.steps.pop() {
            None => return Ok(true),
            Some(Step::Seal(to, permissions)) => fs::set_permissions(&to, permissions)?,
            Some(Step::Visit(from, to)) => self.visit(&from, &to, advance)?,
        }
        Ok(self.steps.is_empty() && self.file.is_none())
    }

    /// Make `to` as a copy of the item at `from`: a link and a folder at once (the folder's items as steps), a file
    /// opened to be copied chunk by chunk.
    fn visit(&mut self, from: &Path, to: &Path, advance: &mut dyn FnMut(u64)) -> io::Result<()> {
        let meta = fs::symlink_metadata(from)?;
        let kind = meta.file_type();
        if kind.is_symlink() {
            std::os::unix::fs::symlink(fs::read_link(from)?, to)?;
            self.copied.push(Copied {
                path: from.to_path_buf(),
                meta,
            });
            advance(1);
        } else if kind.is_dir() {
            let permissions = meta.permissions();
            fs::create_dir(to)?;
            self.copied.push(Copied {
                path: from.to_path_buf(),
                meta,
            });
            advance(1);
            let mut children = Vec::new();
            for child in fs::read_dir(from)? {
                let child = child?;
                children.push(Step::Visit(child.path(), to.join(child.file_name())));
            }
            self.steps.push(Step::Seal(to.to_path_buf(), permissions));
            self.steps.extend(children.into_iter().rev());
        } else if kind.is_file() {
            let source = fs::File::open(from)?;
            let target = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(to)?;
            self.copied.push(Copied {
                path: from.to_path_buf(),
                meta: meta.clone(),
            });
            let buffer = vec![0; COPY_CHUNK.min(meta.len() as usize).max(1)];
            self.file = Some(OpenFile {
                source,
                target,
                meta,
                buffer,
            });
        } else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "neither a file, a folder nor a link",
            ));
        }
        Ok(())
    }
}

/// Remove what a failed copy made at `path`, without following links.
fn remove_made(path: &Path) -> io::Result<()> {
    if fs::symlink_metadata(path)?.is_dir() {
        make_removable(path);
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    }
}

/// Give the owner write and search permission on every folder of a tree a failed copy made: a finished folder took its
/// source's mode, and the items of a read-only folder cannot be removed. Each folder is opened without following links
/// and changed through its descriptor; one that cannot be opened is left as it is.
fn make_removable(dir: &Path) {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let Ok(folder) = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(dir)
    else {
        return;
    };
    if let Ok(meta) = folder.metadata() {
        let mode = meta.permissions().mode();
        if mode & 0o700 != 0o700 {
            let _ = folder.set_permissions(fs::Permissions::from_mode(mode | 0o700));
        }
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if entry.file_type().is_ok_and(|t| t.is_dir()) {
            make_removable(&entry.path());
        }
    }
}

/// Remove the originals of a finished copy, a folder's items before the folder. An item that changed since it was
/// copied stops the removal with an error, and a folder that gained an item is not removed, so nothing that was not
/// copied is lost.
fn remove_copied(copied: &[Copied]) -> io::Result<()> {
    let changed = |path: &Path| {
        io::Error::other(format!(
            "“{}” changed while it was copied, so it was left where it was",
            path.display()
        ))
    };
    for item in copied.iter().rev() {
        let now = fs::symlink_metadata(&item.path)?;
        let same = now.ino() == item.meta.ino()
            && now.dev() == item.meta.dev()
            && now.file_type() == item.meta.file_type();
        if !same {
            return Err(changed(&item.path));
        }
        if now.is_dir() {
            fs::remove_dir(&item.path).map_err(|e| match e.kind() {
                io::ErrorKind::DirectoryNotEmpty => changed(&item.path),
                _ => e,
            })?;
        } else {
            if now.len() != item.meta.len()
                || now.mtime() != item.meta.mtime()
                || now.mtime_nsec() != item.meta.mtime_nsec()
            {
                return Err(changed(&item.path));
            }
            fs::remove_file(&item.path)?;
        }
    }
    Ok(())
}

/// Whether the folder `dest` is `source` or inside it, by path or once links are followed.
fn inside(dest: &Path, source: &Path) -> bool {
    if dest.starts_with(source) {
        return true;
    }
    // a folder reached through a link may still be inside: compare where both really are
    fs::symlink_metadata(source).is_ok_and(|m| m.is_dir())
        && matches!(
            (fs::canonicalize(dest), fs::canonicalize(source)),
            (Ok(dest), Ok(source)) if dest.starts_with(&source)
        )
}

/// Move or copy each of `sources` into the folder `dest`, giving an arriving item a free name (`name copy`) when
/// `dest` has one with its name. One result per source, in order: the new path. A move into the item's own folder
/// does nothing and keeps its path; a folder cannot go into itself. An item moved out of a trash is restored: its
/// `.trashinfo` goes with it.
pub fn drop_into(sources: &[PathBuf], dest: &Path, action: DropAction) -> Vec<io::Result<PathBuf>> {
    transfer(sources, dest, action, &Cancel::default(), &mut |_| {})
}

/// [`drop_into`] that reports its [`Progress`] to `progress` as it goes and stops at `cancel`: the item being copied
/// is removed again and the items after it are left where they are, each with an error [`is_cancelled`] tells. It
/// runs a [`Transfer`]'s slices one after the other.
pub fn transfer(
    sources: &[PathBuf],
    dest: &Path,
    action: DropAction,
    cancel: &Cancel,
    progress: &mut dyn FnMut(&Progress),
) -> Vec<io::Result<PathBuf>> {
    let items = sources
        .iter()
        .map(|s| (s.clone(), Some(dest.to_path_buf())))
        .collect();
    Transfer::new(items, action, cancel.clone()).run_all(progress)
}

/// How long a slice of a [`Transfer`] copies before it gives its I/O turn back: it ends at the first megabyte or item
/// that ends past it.
pub const SLICE: Duration = Duration::from_millis(100);

/// How long one slice should take: a megabyte, or a few items, of a disk that answers.
pub const SLICE_DEADLINE: Duration = Duration::from_secs(10);

/// A copy or a move that runs in slices, so it holds an I/O turn for [`SLICE`] at a time: the program runs each slice
/// off its UI thread, within [`SLICE_DEADLINE`]. Its results are the ones of [`drop_into`] and [`duplicate`].
pub struct Transfer {
    shared: Arc<Mutex<Shared>>,
    cancel: Cancel,
}

/// What a transfer and its slice share.
struct Shared {
    /// What is left, while no slice holds it; `None` while one does and once the transfer ended.
    state: Option<State>,
    progress: Progress,
    /// The results, once the transfer ended, until they are taken.
    results: Option<Vec<io::Result<PathBuf>>>,
}

/// One slice of a [`Transfer`] ([`Transfer::take_slice`]).
pub struct TransferSlice {
    shared: Arc<Mutex<Shared>>,
    cancel: Cancel,
    state: Option<State>,
}

/// Where a transfer is.
struct State {
    /// Each item and the folder it goes into; `None`: an item with no folder, which fails.
    items: Vec<(PathBuf, Option<PathBuf>)>,
    action: DropAction,
    /// The bytes each item weighs, as far as they are weighed.
    weights: Vec<u64>,
    phase: Phase,
    done: Progress,
    results: Vec<io::Result<PathBuf>>,
}

enum Phase {
    /// The items are weighed, one at a time.
    Weighing,
    /// Item `index` is transferred: begun when its copy is under way, which started at `before` bytes.
    Moving {
        index: usize,
        before: u64,
        copy: Option<Box<ItemCopy>>,
    },
    Ended,
}

/// What beginning an item gave: its result, or a copy to make.
enum Begun {
    Ended(io::Result<PathBuf>),
    Copy(Box<ItemCopy>),
}

fn lock(shared: &Mutex<Shared>) -> std::sync::MutexGuard<'_, Shared> {
    shared
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl Transfer {
    /// Move or copy `sources` into the folder `dest`, as [`drop_into`] does.
    pub fn into_folder(sources: Vec<PathBuf>, dest: PathBuf, action: DropAction) -> Transfer {
        let items = sources
            .into_iter()
            .map(|s| (s, Some(dest.clone())))
            .collect();
        Transfer::new(items, action, Cancel::default())
    }

    /// Copy each of `paths` next to itself, as [`duplicate`] does.
    pub fn duplicate(paths: Vec<PathBuf>) -> Transfer {
        let items = paths
            .into_iter()
            .map(|p| {
                let parent = p.parent().map(Path::to_path_buf);
                (p, parent)
            })
            .collect();
        Transfer::new(items, DropAction::Copy, Cancel::default())
    }

    fn new(items: Vec<(PathBuf, Option<PathBuf>)>, action: DropAction, cancel: Cancel) -> Transfer {
        let state = State {
            weights: Vec::with_capacity(items.len()),
            done: Progress {
                total_items: items.len(),
                ..Progress::default()
            },
            results: Vec::with_capacity(items.len()),
            items,
            action,
            phase: Phase::Weighing,
        };
        Transfer {
            shared: Arc::new(Mutex::new(Shared {
                progress: state.done.clone(),
                state: Some(state),
                results: None,
            })),
            cancel,
        }
    }

    /// The next slice; `None` while one is out and once the transfer has ended.
    pub fn take_slice(&mut self) -> Option<TransferSlice> {
        let state = lock(&self.shared).state.take()?;
        Some(TransferSlice {
            shared: Arc::clone(&self.shared),
            cancel: self.cancel.clone(),
            state: Some(state),
        })
    }

    /// How far it has come, as of the last slice.
    pub fn progress(&self) -> Progress {
        lock(&self.shared).progress.clone()
    }

    /// Stops it: the next slice removes the item being copied and ends the transfer, the items after it left where
    /// they are, each with an error [`is_cancelled`] tells.
    pub fn cancel(&mut self) {
        self.cancel.cancel();
    }

    /// One result a source, in order, once it has ended.
    pub fn results(&mut self) -> Option<Vec<io::Result<PathBuf>>> {
        lock(&self.shared).results.take()
    }

    /// Run every slice here, `progress` hearing each step.
    fn run_all(mut self, progress: &mut dyn FnMut(&Progress)) -> Vec<io::Result<PathBuf>> {
        while let Some(slice) = self.take_slice() {
            slice.run_with(progress);
        }
        self.results().unwrap_or_default()
    }
}

impl TransferSlice {
    /// Blocking: copies until the first megabyte or item that ends past [`SLICE`].
    pub fn run(self) {
        self.run_with(&mut |_| {});
    }

    fn run_with(mut self, progress: &mut dyn FnMut(&Progress)) {
        let Some(mut state) = self.state.take() else {
            return;
        };
        let start = Instant::now();
        while state.step(&self.cancel, progress) && start.elapsed() < SLICE {}
        let mut shared = lock(&self.shared);
        shared.progress = state.done.clone();
        match state.phase {
            Phase::Ended => shared.results = Some(std::mem::take(&mut state.results)),
            _ => shared.state = Some(state),
        }
    }
}

impl Drop for TransferSlice {
    /// A slice dropped without running hands the transfer back as it was.
    fn drop(&mut self) {
        if let Some(state) = self.state.take() {
            lock(&self.shared).state = Some(state);
        }
    }
}

impl State {
    /// One step: weigh an item, begin one, copy a chunk or an item of it, or end it; `false` once the transfer ended.
    fn step(&mut self, cancel: &Cancel, progress: &mut dyn FnMut(&Progress)) -> bool {
        match &mut self.phase {
            Phase::Weighing => {
                if let Some((source, _)) = self.items.get(self.weights.len()) {
                    self.weights.push(weight(source, cancel));
                }
                if self.weights.len() == self.items.len() {
                    self.done.total_bytes = self.weights.iter().sum();
                    self.phase = Phase::Moving {
                        index: 0,
                        before: 0,
                        copy: None,
                    };
                }
                true
            }
            Phase::Moving {
                index,
                before,
                copy,
            } => {
                let at = *index;
                let Some((source, dest)) = self.items.get(at) else {
                    self.phase = Phase::Ended;
                    return false;
                };
                let weight = self.weights[at];
                let ended = match copy {
                    Some(item) => {
                        let (done, before) = (&mut self.done, *before);
                        item.step(cancel, &mut |bytes| {
                            // a file that grew while it was copied counts no further than it weighed
                            done.bytes = (done.bytes + bytes).min(before + weight);
                            progress(done);
                        })
                    }
                    None if cancel.is_cancelled() => {
                        self.results.push(Err(cancelled()));
                        *index += 1;
                        return true;
                    }
                    None => {
                        *before = self.done.bytes;
                        self.done.current =
                            source.file_name().map(|n| n.to_string_lossy().into_owned());
                        progress(&self.done);
                        match begin(source, dest.as_deref(), self.action) {
                            Begun::Ended(result) => Some(result),
                            Begun::Copy(item) => {
                                *copy = Some(item);
                                None
                            }
                        }
                    }
                };
                if let Some(result) = ended {
                    self.done.bytes = *before + weight;
                    self.done.items += 1;
                    self.done.current = None;
                    progress(&self.done);
                    self.results.push(result);
                    *index += 1;
                    *copy = None;
                }
                true
            }
            Phase::Ended => false,
        }
    }
}

/// Begin moving or copying `source` into the folder `dest`: a move within a volume is done at once, a copy and a move
/// across volumes are a copy to make.
fn begin(source: &Path, dest: Option<&Path>, action: DropAction) -> Begun {
    let Some(dest) = dest else {
        return Begun::Ended(Err(io::Error::from(io::ErrorKind::InvalidInput)));
    };
    prepare(source, dest, action).unwrap_or_else(|e| Begun::Ended(Err(e)))
}

fn prepare(source: &Path, dest: &Path, action: DropAction) -> io::Result<Begun> {
    if inside(dest, source) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "a folder cannot be dropped into itself",
        ));
    }
    if action == DropAction::Move && source.parent() == Some(dest) {
        return Ok(Begun::Ended(Ok(source.to_path_buf())));
    }
    let name = source
        .file_name()
        .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?
        .to_string_lossy()
        .into_owned();
    let is_dir = fs::symlink_metadata(source)?.is_dir();
    // another process may take the name between the check and the move: the no-replace move fails then
    let target = dest.join(unique_name(&name, is_dir, &|candidate| {
        dest.join(candidate).symlink_metadata().is_ok()
    }));
    match action {
        DropAction::Move => {
            // read before the item leaves the trash, which its record names
            let trashed = trash::trashed_item(source);
            match rename_no_replace(source, &target) {
                // across volumes no rename reaches: copy, then remove the originals once the whole copy is made
                Err(e) if e.raw_os_error() == Some(libc::EXDEV) => Ok(Begun::Copy(Box::new(
                    ItemCopy::new(source, target, Some(trashed)),
                ))),
                moved => {
                    moved?;
                    if let Some(item) = trashed {
                        trash::forget(&item);
                    }
                    Ok(Begun::Ended(Ok(target)))
                }
            }
        }
        DropAction::Copy => Ok(Begun::Copy(Box::new(ItemCopy::new(source, target, None)))),
    }
}
