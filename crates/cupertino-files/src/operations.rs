//! What the window changes on disk besides the trash (`trash`): renaming in place, creating a folder and dropping
//! items into a folder. Nothing here overwrites: a clash renames the arriving item (drops) or fails (renames).

use crate::rename::unique_name;
use std::fs;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

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
    paths
        .iter()
        .map(|path| {
            let parent = path
                .parent()
                .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
            drop_one(path, parent, DropAction::Copy)
        })
        .collect()
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

/// Copy `from` to `to`, which must not exist, without following links. Every item is made exclusively, so nothing
/// that exists is written over; on a failure what was made of `to` is removed again, so a partial copy is never left
/// to look complete. The items copied, a folder before what it holds.
fn copy_new(from: &Path, to: &Path) -> io::Result<Vec<Copied>> {
    let mut copied = Vec::new();
    match copy_tree(from, to, &mut copied) {
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

fn copy_tree(from: &Path, to: &Path, copied: &mut Vec<Copied>) -> io::Result<()> {
    let meta = fs::symlink_metadata(from)?;
    let kind = meta.file_type();
    if kind.is_symlink() {
        std::os::unix::fs::symlink(fs::read_link(from)?, to)?;
        copied.push(Copied {
            path: from.to_path_buf(),
            meta,
        });
    } else if kind.is_dir() {
        let permissions = meta.permissions();
        fs::create_dir(to)?;
        copied.push(Copied {
            path: from.to_path_buf(),
            meta,
        });
        for child in fs::read_dir(from)? {
            let child = child?;
            copy_tree(&child.path(), &to.join(child.file_name()), copied)?;
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
        io::copy(&mut source, &mut target)?;
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
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
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
/// does nothing and keeps its path; a folder cannot go into itself.
pub fn drop_into(sources: &[PathBuf], dest: &Path, action: DropAction) -> Vec<io::Result<PathBuf>> {
    sources
        .iter()
        .map(|source| drop_one(source, dest, action))
        .collect()
}

fn drop_one(source: &Path, dest: &Path, action: DropAction) -> io::Result<PathBuf> {
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
        DropAction::Move => match rename_no_replace(source, &target) {
            // across volumes no rename reaches: copy, then remove the originals once the whole copy is made
            Err(e) if e.raw_os_error() == Some(libc::EXDEV) => {
                let copied = copy_new(source, &target)?;
                remove_copied(&copied)?;
            }
            moved => moved?,
        },
        DropAction::Copy => {
            copy_new(source, &target)?;
        }
    }
    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplicates_sit_next_to_the_original_with_a_free_copy_name() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.txt");
        fs::write(&file, "x").unwrap();
        let first = duplicate(std::slice::from_ref(&file)).remove(0).unwrap();
        let second = duplicate(std::slice::from_ref(&file)).remove(0).unwrap();
        assert_eq!(
            (
                first.file_name().unwrap().to_str().unwrap(),
                second.file_name().unwrap().to_str().unwrap()
            ),
            ("a copy.txt", "a copy 2.txt")
        );
        assert_eq!(fs::read_to_string(first).unwrap(), "x");
    }

    #[test]
    fn an_alias_is_a_link_to_the_absolute_path_with_a_free_name() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.txt");
        fs::write(&file, "x").unwrap();
        let first = make_alias(&file).unwrap();
        let second = make_alias(&file).unwrap();
        assert_eq!(first.file_name().unwrap().to_str().unwrap(), "a.txt alias");
        assert_eq!(
            second.file_name().unwrap().to_str().unwrap(),
            "a.txt alias 2"
        );
        assert_eq!(
            fs::read_link(&first).unwrap(),
            fs::canonicalize(&file).unwrap()
        );
    }

    #[test]
    fn a_folder_with_the_selection_holds_the_moved_items() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a"), dir.path().join("b"));
        fs::write(&a, "1").unwrap();
        fs::write(&b, "2").unwrap();
        let folder = new_folder_with(dir.path(), &[a.clone(), b.clone()]).unwrap();
        assert_eq!(folder, dir.path().join("untitled folder"));
        assert!(!a.exists() && !b.exists());
        assert_eq!(fs::read_to_string(folder.join("b")).unwrap(), "2");
    }

    #[test]
    fn a_rename_never_replaces_an_existing_item() {
        let d = tempfile::tempdir().unwrap();
        fs::write(d.path().join("a.txt"), "a").unwrap();
        fs::write(d.path().join("b.txt"), "b").unwrap();
        let err = rename_item(&d.path().join("a.txt"), "b.txt").unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read_to_string(d.path().join("b.txt")).unwrap(), "b");
        let renamed = rename_item(&d.path().join("a.txt"), "c.txt").unwrap();
        assert_eq!(fs::read_to_string(renamed).unwrap(), "a");
        assert!(!d.path().join("a.txt").exists());
    }

    #[test]
    fn new_folders_take_the_first_free_numbered_name() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(
            new_folder(d.path()).unwrap(),
            d.path().join("untitled folder")
        );
        assert_eq!(
            new_folder(d.path()).unwrap(),
            d.path().join("untitled folder 2")
        );
        fs::remove_dir(d.path().join("untitled folder")).unwrap();
        assert_eq!(
            new_folder(d.path()).unwrap(),
            d.path().join("untitled folder")
        );
    }

    #[test]
    fn modifiers_choose_the_drop_action_and_volumes_the_default() {
        assert_eq!(drop_action(true, false, false), DropAction::Move);
        assert_eq!(drop_action(false, false, false), DropAction::Copy);
        assert_eq!(drop_action(true, true, false), DropAction::Copy);
        assert_eq!(drop_action(false, false, true), DropAction::Move);
    }

    #[test]
    fn dropping_moves_or_copies_into_the_folder_and_renames_on_a_clash() {
        let d = tempfile::tempdir().unwrap();
        let (src, dest) = (d.path().join("src"), d.path().join("dest"));
        fs::create_dir_all(src.join("tree/inner")).unwrap();
        fs::create_dir(&dest).unwrap();
        fs::write(src.join("tree/inner/f.txt"), "f").unwrap();
        fs::write(src.join("note.txt"), "new").unwrap();
        fs::write(dest.join("note.txt"), "old").unwrap();

        let copied = drop_into(
            &[src.join("tree"), src.join("note.txt")],
            &dest,
            DropAction::Copy,
        );
        assert_eq!(
            copied.into_iter().map(Result::unwrap).collect::<Vec<_>>(),
            [dest.join("tree"), dest.join("note copy.txt")]
        );
        assert_eq!(
            fs::read_to_string(dest.join("tree/inner/f.txt")).unwrap(),
            "f"
        );
        assert_eq!(fs::read_to_string(dest.join("note.txt")).unwrap(), "old");
        assert!(src.join("tree").exists(), "a copy keeps the source");

        let moved = drop_into(&[src.join("tree")], &dest, DropAction::Move);
        assert_eq!(moved[0].as_ref().unwrap(), &dest.join("tree copy"));
        assert!(!src.join("tree").exists());
    }

    #[test]
    fn a_copy_that_fails_part_way_leaves_nothing_at_the_destination_and_the_source_as_it_was() {
        let d = tempfile::tempdir().unwrap();
        let (src, dest) = (d.path().join("src"), d.path().join("dest"));
        fs::create_dir_all(src.join("tree/inner")).unwrap();
        fs::create_dir(&dest).unwrap();
        fs::write(src.join("tree/a.txt"), "a").unwrap();
        fs::write(src.join("tree/inner/b.txt"), "b").unwrap();
        // a pipe is none of what a copy takes, wherever the listing puts it
        let fifo =
            std::ffi::CString::new(src.join("tree/inner/pipe").as_os_str().as_bytes()).unwrap();
        // SAFETY: the path is NUL-terminated and outlives the call
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);

        let placed = drop_into(&[src.join("tree")], &dest, DropAction::Copy);
        assert!(placed[0].is_err(), "{placed:?}");
        assert_eq!(
            fs::read_dir(&dest).unwrap().count(),
            0,
            "no partial copy is left"
        );
        assert_eq!(fs::read_to_string(src.join("tree/a.txt")).unwrap(), "a");
        assert_eq!(
            fs::read_to_string(src.join("tree/inner/b.txt")).unwrap(),
            "b"
        );
    }

    #[test]
    fn a_copy_never_writes_over_an_item_that_is_there() {
        let d = tempfile::tempdir().unwrap();
        let (from, to) = (d.path().join("from.txt"), d.path().join("to.txt"));
        fs::write(&from, "new").unwrap();
        fs::write(&to, "old").unwrap();
        let err = copy_new(&from, &to).err().unwrap();
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read_to_string(&to).unwrap(), "old", "kept, not removed");
    }

    #[test]
    fn the_originals_of_a_copy_go_only_when_nothing_changed_while_it_was_made() {
        let d = tempfile::tempdir().unwrap();
        let (src, copy) = (d.path().join("src"), d.path().join("copy"));
        fs::create_dir_all(src.join("inner")).unwrap();
        fs::write(src.join("a.txt"), "a").unwrap();
        fs::write(src.join("inner/b.txt"), "b").unwrap();
        let copied = copy_new(&src, &copy).unwrap();
        // an item arrives in the folder after the copy listed it
        fs::write(src.join("inner/late.txt"), "late").unwrap();
        assert!(remove_copied(&copied).is_err());
        assert_eq!(
            fs::read_to_string(src.join("inner/late.txt")).unwrap(),
            "late"
        );
        assert_eq!(fs::read_to_string(copy.join("inner/b.txt")).unwrap(), "b");

        let (src, copy) = (d.path().join("src2"), d.path().join("copy2"));
        fs::create_dir(&src).unwrap();
        fs::write(src.join("a.txt"), "a").unwrap();
        let copied = copy_new(&src, &copy).unwrap();
        // a file written after it was copied
        fs::write(src.join("a.txt"), "longer").unwrap();
        assert!(remove_copied(&copied).is_err());
        assert_eq!(fs::read_to_string(src.join("a.txt")).unwrap(), "longer");

        let (src, copy) = (d.path().join("src3"), d.path().join("copy3"));
        fs::create_dir_all(src.join("inner")).unwrap();
        fs::write(src.join("inner/b.txt"), "b").unwrap();
        std::os::unix::fs::symlink(d.path().join("src2"), src.join("link")).unwrap();
        let copied = copy_new(&src, &copy).unwrap();
        remove_copied(&copied).unwrap();
        assert!(!src.exists());
        assert!(
            d.path().join("src2/a.txt").exists(),
            "a link is removed, not what it points at"
        );
        assert_eq!(fs::read_to_string(copy.join("inner/b.txt")).unwrap(), "b");
        assert_eq!(
            fs::read_link(copy.join("link")).unwrap(),
            d.path().join("src2")
        );
    }

    #[test]
    fn a_folder_cannot_be_dropped_into_itself_through_a_link() {
        let d = tempfile::tempdir().unwrap();
        fs::create_dir_all(d.path().join("a/b")).unwrap();
        std::os::unix::fs::symlink(d.path().join("a/b"), d.path().join("link")).unwrap();
        for action in [DropAction::Copy, DropAction::Move] {
            let placed = drop_into(&[d.path().join("a")], &d.path().join("link"), action);
            assert_eq!(
                placed[0].as_ref().unwrap_err().kind(),
                io::ErrorKind::InvalidInput
            );
        }
        assert_eq!(fs::read_dir(d.path().join("a/b")).unwrap().count(), 0);
    }

    #[test]
    fn a_folder_cannot_be_dropped_into_itself_and_a_move_into_its_own_folder_changes_nothing() {
        let d = tempfile::tempdir().unwrap();
        fs::create_dir_all(d.path().join("a/b")).unwrap();
        let into_child = drop_into(
            &[d.path().join("a")],
            &d.path().join("a/b"),
            DropAction::Move,
        );
        assert_eq!(
            into_child[0].as_ref().unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
        let in_place = drop_into(
            &[d.path().join("a/b")],
            &d.path().join("a"),
            DropAction::Move,
        );
        assert_eq!(in_place[0].as_ref().unwrap(), &d.path().join("a/b"));
    }
}
