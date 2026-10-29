//! The text-editing bindings of [`cupertino_key_bindings`], plus the user's override file and modifier profile from
//! the config directory.

use cupertino_defaults::keyboard::KeyboardSettings;
pub use cupertino_key_bindings::binding::*;
use cupertino_modifiers::Profile;
use std::path::PathBuf;

/// `$XDG_CONFIG_HOME/cupertino/keybindings.conf`, falling back to `~/.config/cupertino/keybindings.conf`.
pub fn default_user_path() -> Option<PathBuf> {
    cupertino_defaults::config_home().map(|c| c.join("cupertino").join("keybindings.conf"))
}

/// The modifier profile of `keyboard.conf`; the design's profile when the file or the setting cannot be read.
pub fn user_profile() -> Profile {
    KeyboardSettings::path()
        .and_then(|path| KeyboardSettings::load(&path, None).ok())
        .and_then(|(settings, _)| settings.modifier_profile)
        .and_then(|p| Profile::by_name(p.name()))
        .unwrap_or_else(Profile::cupertino)
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
