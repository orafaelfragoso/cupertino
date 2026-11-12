//! Settings and XDG base directories shared by the Cupertino processes. Every settings file lives in
//! `$XDG_CONFIG_HOME/cupertino/` (default `~/.config/cupertino/`) and can be redirected by its own
//! environment variable, so tests and fixtures never touch the user's files.

pub mod accessibility;
pub mod appearance;
pub mod atomic;
pub mod compositor;
pub mod conf;
pub mod data_control;
pub mod desktop_control;
pub mod displays;
pub mod dock;
pub mod keyboard;
pub mod lock;

use std::ffi::OsString;
use std::path::PathBuf;

pub mod login;
pub mod menubar;
pub mod notifications;
pub mod power;
pub mod screen_sharing;
pub mod system;
pub mod terminal;
pub mod watch;

/// The user's configuration directory: `$XDG_CONFIG_HOME`, else `$HOME/.config`. `None` when
/// neither is set.
pub fn config_home() -> Option<PathBuf> {
    resolve_home(
        std::env::var_os("XDG_CONFIG_HOME"),
        std::env::var_os("HOME"),
    )
}

/// XDG data directories, most important first: `$XDG_DATA_HOME` (default `~/.local/share`), then
/// `$XDG_DATA_DIRS` (default `/usr/local/share:/usr/share`).
pub fn data_dirs() -> Vec<PathBuf> {
    let home = std::env::var_os("XDG_DATA_HOME")
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")));
    let system = std::env::var("XDG_DATA_DIRS")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    home.into_iter()
        .chain(
            system
                .split(':')
                .filter(|s| !s.is_empty())
                .map(PathBuf::from),
        )
        .collect()
}

/// Path of the settings file `name`: the value of `env` when it is set and not empty, else
/// `<config home>/cupertino/<name>`. The file need not exist.
pub fn config_file(env: &str, name: &str) -> Option<PathBuf> {
    resolve_file(std::env::var_os(env), config_home(), name)
}

fn resolve_home(xdg: Option<OsString>, home: Option<OsString>) -> Option<PathBuf> {
    xdg.filter(|p| !p.is_empty())
        .map(PathBuf::from)
        .or_else(|| home.map(|h| PathBuf::from(h).join(".config")))
}

fn resolve_file(
    env: Option<OsString>,
    config_home: Option<PathBuf>,
    name: &str,
) -> Option<PathBuf> {
    match env.filter(|p| !p.is_empty()) {
        Some(p) => Some(PathBuf::from(p)),
        None => config_home.map(|c| c.join("cupertino").join(name)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_home_prefers_xdg_and_falls_back_to_home() {
        assert_eq!(
            resolve_home(Some("/x".into()), Some("/h".into())),
            Some(PathBuf::from("/x"))
        );
        assert_eq!(
            resolve_home(Some("".into()), Some("/h".into())),
            Some(PathBuf::from("/h/.config"))
        );
        assert_eq!(resolve_home(None, None), None);
    }

    #[test]
    fn the_environment_override_wins_unless_empty() {
        let home = Some(PathBuf::from("/h/.config"));
        assert_eq!(
            resolve_file(Some("/tmp/dock.conf".into()), home.clone(), "dock.conf"),
            Some(PathBuf::from("/tmp/dock.conf"))
        );
        assert_eq!(
            resolve_file(Some("".into()), home.clone(), "dock.conf"),
            Some(PathBuf::from("/h/.config/cupertino/dock.conf"))
        );
        assert_eq!(resolve_file(None, None, "dock.conf"), None);
    }
}
