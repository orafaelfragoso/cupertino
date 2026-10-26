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
    let c = |p: &Path| std::ffi::CString::new(p.as_os_str().as_bytes()).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput));
    let (from, to) = (c(from)?, c(to)?);
    // SAFETY: both pointers are NUL-terminated strings that outlive the call.
    let rc = unsafe { libc::renameat2(libc::AT_FDCWD, from.as_ptr(), libc::AT_FDCWD, to.as_ptr(), libc::RENAME_NOREPLACE) };
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
        let path = dir.join(if n == 1 { NEW_FOLDER_NAME.to_string() } else { format!("{NEW_FOLDER_NAME} {n}") });
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
            let parent = path.parent().ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
            drop_one(path, parent, DropAction::Copy)
        })
        .collect()
}

/// Make a symbolic link to `path` named `name alias` (then `name alias 2` …) in its folder and return it. The link
/// holds the absolute path, so it survives the link being moved.
pub fn make_alias(path: &Path) -> io::Result<PathBuf> {
    let dir = path.parent().ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
    let name = path.file_name().ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?.to_string_lossy().into_owned();
    let target = fs::canonicalize(path)?;
    for n in 1.. {
        let link = dir.join(if n == 1 { format!("{name} alias") } else { format!("{name} alias {n}") });
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

fn copy_tree(from: &Path, to: &Path) -> io::Result<()> {
    let meta = fs::symlink_metadata(from)?;
    if meta.file_type().is_symlink() {
        std::os::unix::fs::symlink(fs::read_link(from)?, to)
    } else if meta.is_dir() {
        fs::create_dir(to)?;
        for child in fs::read_dir(from)? {
            let child = child?;
            copy_tree(&child.path(), &to.join(child.file_name()))?;
        }
        fs::set_permissions(to, meta.permissions())
    } else if meta.is_file() {
        fs::copy(from, to).map(|_| ())
    } else {
        Err(io::Error::new(io::ErrorKind::InvalidInput, "neither a file, a folder nor a link"))
    }
}

fn remove_tree(path: &Path) -> io::Result<()> {
    if fs::symlink_metadata(path)?.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    }
}

/// Move or copy each of `sources` into the folder `dest`, giving an arriving item a free name (`name copy`) when
/// `dest` has one with its name. One result per source, in order: the new path. A move into the item's own folder
/// does nothing and keeps its path; a folder cannot go into itself.
pub fn drop_into(sources: &[PathBuf], dest: &Path, action: DropAction) -> Vec<io::Result<PathBuf>> {
    sources.iter().map(|source| drop_one(source, dest, action)).collect()
}

fn drop_one(source: &Path, dest: &Path, action: DropAction) -> io::Result<PathBuf> {
    if dest.starts_with(source) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "a folder cannot be dropped into itself"));
    }
    if action == DropAction::Move && source.parent() == Some(dest) {
        return Ok(source.to_path_buf());
    }
    let name = source.file_name().ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?.to_string_lossy().into_owned();
    let is_dir = fs::symlink_metadata(source)?.is_dir();
    // another process may take the name between the check and the move: the no-replace move fails then
    let target = dest.join(unique_name(&name, is_dir, &|candidate| dest.join(candidate).symlink_metadata().is_ok()));
    match action {
        DropAction::Move => match rename_no_replace(source, &target) {
            // Super forces a move across volumes, where no rename reaches: copy, then remove the original
            Err(e) if e.raw_os_error() == Some(libc::EXDEV) => {
                copy_tree(source, &target)?;
                remove_tree(source)?;
            }
            moved => moved?,
        },
        DropAction::Copy => copy_tree(source, &target)?,
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
        assert_eq!((first.file_name().unwrap().to_str().unwrap(), second.file_name().unwrap().to_str().unwrap()), ("a copy.txt", "a copy 2.txt"));
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
        assert_eq!(second.file_name().unwrap().to_str().unwrap(), "a.txt alias 2");
        assert_eq!(fs::read_link(&first).unwrap(), fs::canonicalize(&file).unwrap());
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
        assert_eq!(new_folder(d.path()).unwrap(), d.path().join("untitled folder"));
        assert_eq!(new_folder(d.path()).unwrap(), d.path().join("untitled folder 2"));
        fs::remove_dir(d.path().join("untitled folder")).unwrap();
        assert_eq!(new_folder(d.path()).unwrap(), d.path().join("untitled folder"));
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

        let copied = drop_into(&[src.join("tree"), src.join("note.txt")], &dest, DropAction::Copy);
        assert_eq!(copied.into_iter().map(Result::unwrap).collect::<Vec<_>>(), [dest.join("tree"), dest.join("note copy.txt")]);
        assert_eq!(fs::read_to_string(dest.join("tree/inner/f.txt")).unwrap(), "f");
        assert_eq!(fs::read_to_string(dest.join("note.txt")).unwrap(), "old");
        assert!(src.join("tree").exists(), "a copy keeps the source");

        let moved = drop_into(&[src.join("tree")], &dest, DropAction::Move);
        assert_eq!(moved[0].as_ref().unwrap(), &dest.join("tree copy"));
        assert!(!src.join("tree").exists());
    }

    #[test]
    fn a_folder_cannot_be_dropped_into_itself_and_a_move_into_its_own_folder_changes_nothing() {
        let d = tempfile::tempdir().unwrap();
        fs::create_dir_all(d.path().join("a/b")).unwrap();
        let into_child = drop_into(&[d.path().join("a")], &d.path().join("a/b"), DropAction::Move);
        assert_eq!(into_child[0].as_ref().unwrap_err().kind(), io::ErrorKind::InvalidInput);
        let in_place = drop_into(&[d.path().join("a/b")], &d.path().join("a"), DropAction::Move);
        assert_eq!(in_place[0].as_ref().unwrap(), &d.path().join("a/b"));
    }
}
