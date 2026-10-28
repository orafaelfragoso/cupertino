//! The text-editing bindings of [`cupertino_key_bindings`], plus the user's override file in the config directory.

pub use cupertino_key_bindings::binding::*;
use std::path::PathBuf;

/// `$XDG_CONFIG_HOME/cupertino/keybindings.conf`, falling back to `~/.config/cupertino/keybindings.conf`.
pub fn default_user_path() -> Option<PathBuf> {
    cupertino_defaults::config_home().map(|c| c.join("cupertino").join("keybindings.conf"))
}

/// Standard table plus the user file when present.
pub fn load_default() -> (BindingTable, Vec<OverrideError>) {
    let mut t = BindingTable::standard();
    let errs = default_user_path()
        .and_then(|p| t.load_user_file(&p).ok())
        .unwrap_or_default();
    (t, errs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_path_shape() {
        if let Some(p) = default_user_path() {
            assert!(p.ends_with("cupertino/keybindings.conf"));
        }
    }
}
