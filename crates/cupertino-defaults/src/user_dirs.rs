//! The user directories of `user-dirs.dirs` (xdg-user-dirs): `XDG_<NAME>_DIR="$HOME/<path>"` or an absolute path, one
//! assignment a line.

use std::io;
use std::path::{Path, PathBuf};

/// The name of the file that assigns the user directories, in the configuration directory.
const FILE: &str = "user-dirs.dirs";

/// The `XDG_<NAME>_DIR` assignments of a `user-dirs.dirs` text as `(NAME, path)`, `$HOME` expanded against `home`.
/// Lines that are comments, malformed or relative without `$HOME/` are skipped, as xdg-user-dirs does.
pub fn parse(text: &str, home: &Path) -> Vec<(String, PathBuf)> {
    text.lines()
        .filter_map(|line| {
            let (key, value) = line.trim().split_once('=')?;
            let name = key.trim().strip_prefix("XDG_")?.strip_suffix("_DIR")?;
            let value = value.trim().strip_prefix('"')?.strip_suffix('"')?;
            let path = if value == "$HOME" {
                home.to_path_buf()
            } else if let Some(rest) = value.strip_prefix("$HOME/") {
                home.join(rest)
            } else if value.starts_with('/') {
                PathBuf::from(value)
            } else {
                return None;
            };
            Some((name.to_string(), path))
        })
        .collect()
}

/// The assignments of the session's `user-dirs.dirs`: in `$XDG_CONFIG_HOME` when that is an absolute path, else in
/// `.config` of `home`. A relative `XDG_CONFIG_HOME` is not valid and is passed over, as the base directory
/// specification says. None when the file is missing; an error when it is there and cannot be read. It reads a file.
pub fn read(home: &Path) -> io::Result<Vec<(String, PathBuf)>> {
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| home.join(".config"));
    match std::fs::read_to_string(config.join(FILE)) {
        Ok(text) => Ok(parse(&text, home)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e),
    }
}
