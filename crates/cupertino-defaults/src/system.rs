//! The machine's own files under `/etc/cupertino`: root writes them, every process may read them, and a reader trusts one
//! only when it is what root alone could have put there.

use std::collections::VecDeque;
use std::ffi::OsString;
use std::io::Read;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Component, Path, PathBuf};

/// The text of a system file, read from the one descriptor that was checked: opened without following a link, owned by
/// root and not writable by anyone else. `None` when there is no such file.
pub fn read_root_file(path: &Path) -> std::io::Result<Option<String>> {
    let mut file = match std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
    {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let meta = file.metadata()?;
    if !is_root_only(meta.uid(), meta.mode()) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "a system configuration must be a regular file owned by root that no one else can write",
        ));
    }
    let mut text = String::new();
    file.read_to_string(&mut text)?;
    Ok(Some(text))
}

/// Whether a file of owner `owner` and `st_mode` `mode` (type and permission bits) is a regular file only root can change:
/// owned by root and writable by no group or other user.
pub fn is_root_only(owner: u32, mode: u32) -> bool {
    mode & libc::S_IFMT == libc::S_IFREG && owner == 0 && mode & 0o022 == 0
}

/// Whether only root can change or replace the file at `path`, an absolute path without links: the file is
/// [`is_root_only`] and every directory above it, up to `/`, is a directory owned by root that no group or other user
/// can write. A path that cannot be looked at is not.
pub fn only_root_can_change(path: &Path) -> bool {
    let root_dir = |dir: &Path| {
        std::fs::symlink_metadata(dir)
            .is_ok_and(|m| m.is_dir() && m.uid() == 0 && m.mode() & 0o022 == 0)
    };
    path.is_absolute()
        && std::fs::symlink_metadata(path).is_ok_and(|m| is_root_only(m.uid(), m.mode()))
        && path.ancestors().skip(1).all(root_dir)
}

/// The file the absolute `path` names when only root can change that file and every step that leads to it: each
/// directory on the way is root's and writable by no group or other user, and each link on the way is root's, in such
/// a directory, and is followed the same way (a distribution's `/usr/bin/orca` may be a link). The file is
/// [`is_root_only`]. `None` for anything else, a loop of links included.
pub fn root_only_target(path: &Path) -> Option<PathBuf> {
    const MAX_LINKS: usize = 40;
    let root_dir = |m: &std::fs::Metadata| m.is_dir() && m.uid() == 0 && m.mode() & 0o022 == 0;
    if !path.is_absolute() || !root_dir(&std::fs::symlink_metadata("/").ok()?) {
        return None;
    }
    let mut at = PathBuf::from("/");
    let mut rest: VecDeque<OsString> = VecDeque::new();
    let mut links = 0;
    push_front(&mut rest, path);
    while let Some(name) = rest.pop_front() {
        if name == ".." {
            at.pop();
            continue;
        }
        let next = at.join(&name);
        let meta = std::fs::symlink_metadata(&next).ok()?;
        if meta.file_type().is_symlink() {
            links += 1;
            if links > MAX_LINKS || meta.uid() != 0 {
                return None;
            }
            let target = std::fs::read_link(&next).ok()?;
            if target.is_absolute() {
                at = PathBuf::from("/");
            }
            push_front(&mut rest, &target);
        } else if rest.is_empty() {
            return is_root_only(meta.uid(), meta.mode()).then_some(next);
        } else if root_dir(&meta) {
            at = next;
        } else {
            return None;
        }
    }
    None
}

/// Put the names of `path` before `rest`, `..` kept for the walk to apply in order.
fn push_front(rest: &mut VecDeque<OsString>, path: &Path) {
    for component in path.components().rev() {
        match component {
            Component::Normal(name) => rest.push_front(name.to_owned()),
            Component::ParentDir => rest.push_front("..".into()),
            Component::CurDir | Component::RootDir | Component::Prefix(_) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_program_of_the_system_only_root_can_change_and_one_of_the_user_others_can() {
        assert!(only_root_can_change(
            &std::fs::canonicalize("/usr/bin/env").unwrap()
        ));
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join("program");
        std::fs::write(&program, "").unwrap();
        assert!(!only_root_can_change(&program));
        assert!(
            !only_root_can_change(Path::new("usr/bin/env")),
            "a relative path"
        );
        assert!(!only_root_can_change(Path::new("/usr/bin/no-such-program")));
    }

    #[test]
    fn a_program_behind_links_of_the_system_is_the_file_they_lead_to_and_a_link_of_the_user_leads_nowhere(
    ) {
        let env = std::fs::canonicalize("/usr/bin/env").unwrap();
        assert!(
            std::fs::symlink_metadata("/bin").unwrap().is_symlink(),
            "the test needs a merged /usr, where /bin is root's link to usr/bin"
        );
        assert!(!only_root_can_change(Path::new("/bin/env")));
        assert_eq!(root_only_target(Path::new("/bin/env")), Some(env.clone()));
        assert_eq!(
            root_only_target(Path::new("/usr/bin/../bin/./env")),
            Some(env.clone())
        );
        let dir = tempfile::tempdir().unwrap();
        let link = dir.path().join("env");
        std::os::unix::fs::symlink(&env, &link).unwrap();
        assert_eq!(
            root_only_target(&link),
            None,
            "a link in the user's directory"
        );
        let program = dir.path().join("program");
        std::fs::write(&program, "").unwrap();
        assert_eq!(root_only_target(&program), None);
        assert_eq!(
            root_only_target(Path::new("bin/env")),
            None,
            "a relative path"
        );
        assert_eq!(root_only_target(Path::new("/usr/bin")), None, "a directory");
        assert_eq!(
            root_only_target(Path::new("/usr/bin/no-such-program")),
            None
        );
    }
}
