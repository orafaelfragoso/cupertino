//! What the window changes on disk besides the trash (`trash`): renaming in place, creating a folder and dropping
//! items into a folder. Nothing here overwrites: a clash renames the arriving item (drops) or fails (renames).

use crate::rename::unique_name;
use cupertino_trash as trash;
use std::fs;
use std::io::{self, Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

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
    let items: Vec<(&Path, Option<&Path>)> = paths
        .iter()
        .map(|path| (path.as_path(), path.parent()))
        .collect();
    transfer_each(&items, DropAction::Copy, cancel, progress)
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
    for moved in drop_into(items, &folder, DropAction::Move) {
        moved?;
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

/// A copy's progress and its stop.
struct Copying<'a> {
    cancel: &'a Cancel,
    /// Takes the bytes done since the last call.
    advance: &'a mut dyn FnMut(u64),
}

fn copy_new_with(from: &Path, to: &Path, copying: &mut Copying<'_>) -> io::Result<Vec<Copied>> {
    let mut copied = Vec::new();
    match copy_tree(from, to, &mut copied, copying) {
        Ok(()) => Ok(copied),
        Err(e) => {
            // the first item copied is `to` itself: made by this copy, so it is this copy's to remove
            if !copied.is_empty() {
                let _ = remove_made(to);
            }
            Err(e)
        }
    }
}

fn copy_tree(
    from: &Path,
    to: &Path,
    copied: &mut Vec<Copied>,
    copying: &mut Copying<'_>,
) -> io::Result<()> {
    copying.cancel.check()?;
    let meta = fs::symlink_metadata(from)?;
    let kind = meta.file_type();
    if kind.is_symlink() {
        std::os::unix::fs::symlink(fs::read_link(from)?, to)?;
        copied.push(Copied {
            path: from.to_path_buf(),
            meta,
        });
        (copying.advance)(1);
    } else if kind.is_dir() {
        let permissions = meta.permissions();
        fs::create_dir(to)?;
        copied.push(Copied {
            path: from.to_path_buf(),
            meta,
        });
        (copying.advance)(1);
        for child in fs::read_dir(from)? {
            let child = child?;
            copy_tree(&child.path(), &to.join(child.file_name()), copied, copying)?;
        }
        fs::set_permissions(to, permissions)?;
    } else if kind.is_file() {
        let mut source = fs::File::open(from)?;
        let mut target = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(to)?;
        copied.push(Copied {
            path: from.to_path_buf(),
            meta: meta.clone(),
        });
        let mut buffer = vec![0; COPY_CHUNK.min(meta.len() as usize).max(1)];
        loop {
            let read = match source.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => read,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            };
            target.write_all(&buffer[..read])?;
            (copying.advance)(read as u64);
            copying.cancel.check()?;
        }
        if meta.len() == 0 {
            (copying.advance)(1);
        }
        target.set_permissions(meta.permissions())?;
    } else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "neither a file, a folder nor a link",
        ));
    }
    Ok(())
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
/// is removed again and the items after it are left where they are, each with an error [`is_cancelled`] tells.
pub fn transfer(
    sources: &[PathBuf],
    dest: &Path,
    action: DropAction,
    cancel: &Cancel,
    progress: &mut dyn FnMut(&Progress),
) -> Vec<io::Result<PathBuf>> {
    let items: Vec<(&Path, Option<&Path>)> =
        sources.iter().map(|s| (s.as_path(), Some(dest))).collect();
    transfer_each(&items, action, cancel, progress)
}

/// Move or copy each item into its folder (`None`: an item with no folder, which fails), as [`transfer`] does, with
/// one [`Progress`] over them all.
fn transfer_each(
    items: &[(&Path, Option<&Path>)],
    action: DropAction,
    cancel: &Cancel,
    progress: &mut dyn FnMut(&Progress),
) -> Vec<io::Result<PathBuf>> {
    let weights: Vec<u64> = items.iter().map(|(s, _)| weight(s, cancel)).collect();
    let mut done = Progress {
        total_bytes: weights.iter().sum(),
        total_items: items.len(),
        ..Progress::default()
    };
    let mut results = Vec::with_capacity(items.len());
    for (&(source, dest), weight) in items.iter().zip(weights) {
        if cancel.is_cancelled() {
            results.push(Err(cancelled()));
            continue;
        }
        let before = done.bytes;
        done.current = source.file_name().map(|n| n.to_string_lossy().into_owned());
        progress(&done);
        let result = {
            let mut advance = |bytes: u64| {
                // a file that grew while it was copied counts no further than it weighed
                done.bytes = (done.bytes + bytes).min(before + weight);
                progress(&done);
            };
            let mut copying = Copying {
                cancel,
                advance: &mut advance,
            };
            match dest {
                Some(dest) => drop_one(source, dest, action, &mut copying),
                None => Err(io::Error::from(io::ErrorKind::InvalidInput)),
            }
        };
        done.bytes = before + weight;
        done.items += 1;
        done.current = None;
        progress(&done);
        results.push(result);
    }
    results
}

fn drop_one(
    source: &Path,
    dest: &Path,
    action: DropAction,
    copying: &mut Copying<'_>,
) -> io::Result<PathBuf> {
    if inside(dest, source) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "a folder cannot be dropped into itself",
        ));
    }
    if action == DropAction::Move && source.parent() == Some(dest) {
        return Ok(source.to_path_buf());
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
                Err(e) if e.raw_os_error() == Some(libc::EXDEV) => {
                    let copied = copy_new_with(source, &target, copying)?;
                    remove_copied(&copied)?;
                }
                moved => moved?,
            }
            if let Some(item) = trashed {
                trash::forget(&item);
            }
        }
        DropAction::Copy => {
            copy_new_with(source, &target, copying)?;
        }
    }
    Ok(target)
}
