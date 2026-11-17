//! The user's text-editing binding file, `keybindings.conf` in the settings directory, read on top of the standard
//! table. The file's format and its parsing are `cupertino_keyboard::binding`'s.

use std::io;
use std::path::{Path, PathBuf};

use cupertino_keyboard::binding::{BindingTable, OverrideError};

/// `$XDG_CONFIG_HOME/cupertino/keybindings.conf`, falling back to `~/.config/cupertino/keybindings.conf`.
pub fn default_user_path() -> Option<PathBuf> {
    cupertino_defaults::config_home().map(|c| c.join("cupertino").join("keybindings.conf"))
}

/// Loads a user file on top of `table`. A missing file is not an error.
pub fn load_user_file(table: &mut BindingTable, path: &Path) -> io::Result<Vec<OverrideError>> {
    match std::fs::read_to_string(path) {
        Ok(s) => Ok(table.apply_overrides(&s)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e),
    }
}

/// Standard table plus the user file when present.
pub fn load_default() -> (BindingTable, Vec<OverrideError>) {
    let mut t = BindingTable::standard();
    let errs = default_user_path()
        .and_then(|p| load_user_file(&mut t, &p).ok())
        .unwrap_or_default();
    (t, errs)
}
