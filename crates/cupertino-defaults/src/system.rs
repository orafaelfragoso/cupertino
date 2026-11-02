//! The machine's own files under `/etc/cupertino`: root writes them, every process may read them, and a reader trusts one
//! only when it is what root alone could have put there.

use std::io::Read;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;

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
}
