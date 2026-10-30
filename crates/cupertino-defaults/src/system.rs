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
    if !meta.is_file() || meta.uid() != 0 || meta.mode() & 0o022 != 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "a system configuration must be a regular file owned by root that no one else can write",
        ));
    }
    let mut text = String::new();
    file.read_to_string(&mut text)?;
    Ok(Some(text))
}
