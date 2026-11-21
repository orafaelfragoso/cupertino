//! What the crate reads from the environment: locale, desktop, search path and the XDG directories.
//!
//! The tests of this file change environment variables, so each holds the lock and restores them.

use crate::common;

use common::{desktop, write};
use cupertino_apps::catalogue::Catalogue;
use cupertino_apps::chromium::config_home;
use cupertino_apps::entry::{xdg_application_dirs, Locale, Visibility};
use cupertino_apps::frecency::{default_path, STORE_NAME};
use cupertino_apps::mimeapps::XdgDirs;
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};

static LOCK: Mutex<()> = Mutex::new(());

const VARS: [&str; 11] = [
    "LC_ALL",
    "LC_MESSAGES",
    "LANG",
    "XDG_CURRENT_DESKTOP",
    "PATH",
    "HOME",
    "XDG_DATA_HOME",
    "XDG_DATA_DIRS",
    "XDG_CONFIG_HOME",
    "XDG_CONFIG_DIRS",
    "XDG_UNUSED",
];

/// The environment of a test: every variable the crate reads is unset, and restored when it is dropped.
struct Env {
    _lock: MutexGuard<'static, ()>,
    saved: Vec<(&'static str, Option<OsString>)>,
}

impl Env {
    fn new() -> Env {
        let lock = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let saved = VARS.iter().map(|v| (*v, std::env::var_os(v))).collect();
        for var in VARS {
            std::env::remove_var(var);
        }
        Env { _lock: lock, saved }
    }

    /// An environment with only `vars` set.
    fn with(vars: &[(&str, &str)]) -> Env {
        let env = Env::new();
        for (name, value) in vars {
            env.set(name, value);
        }
        env
    }

    fn set(&self, name: &str, value: impl AsRef<std::ffi::OsStr>) -> &Env {
        std::env::set_var(name, value);
        self
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        for (name, value) in &self.saved {
            match value {
                Some(v) => std::env::set_var(name, v),
                None => std::env::remove_var(name),
            }
        }
    }
}

#[test]
fn the_locale_comes_from_the_first_set_of_lc_all_lc_messages_and_lang() {
    for (vars, expected) in [
        (&[][..], &[][..]),
        (&[("LANG", "fr_FR.UTF-8")], &["fr_FR", "fr"]),
        (
            &[("LANG", "fr_FR.UTF-8"), ("LC_MESSAGES", "de_DE.UTF-8")],
            &["de_DE", "de"],
        ),
        (
            &[
                ("LANG", "fr_FR.UTF-8"),
                ("LC_MESSAGES", "de_DE.UTF-8"),
                ("LC_ALL", "es_ES.UTF-8"),
            ],
            &["es_ES", "es"],
        ),
        (&[("LC_ALL", ""), ("LANG", "it_IT")], &["it_IT", "it"]),
    ] {
        let _env = Env::with(vars);
        assert_eq!(Locale::from_env().candidates, expected, "{vars:?}");
    }
}

#[test]
fn visibility_reads_the_desktops_and_the_search_path() {
    let env = Env::new();
    let vis = Visibility::from_env();
    assert!(vis.current_desktops.is_empty() && vis.path_dirs.is_empty());
    env.set("XDG_CURRENT_DESKTOP", "Cupertino::GNOME:")
        .set("PATH", "/one:/two");
    let vis = Visibility::from_env();
    assert_eq!(vis.current_desktops, ["Cupertino", "GNOME"]);
    assert_eq!(vis.path_dirs, [PathBuf::from("/one"), "/two".into()]);
    assert!(!vis.skip_try_exec);
}

#[test]
fn application_folders_run_from_the_data_home_through_the_data_dirs() {
    let system = ["/usr/local/share/applications", "/usr/share/applications"];
    let home = "/home/u/.local/share/applications";
    for (vars, expected) in [
        (
            &[("XDG_DATA_HOME", "/h"), ("XDG_DATA_DIRS", "/a::/b")][..],
            vec!["/h/applications", "/a/applications", "/b/applications"],
        ),
        (&[("HOME", "/home/u")], vec![home, system[0], system[1]]),
        (
            &[
                ("HOME", "/home/u"),
                ("XDG_DATA_HOME", ""),
                ("XDG_DATA_DIRS", ""),
            ],
            vec![home, system[0], system[1]],
        ),
        (&[], system.to_vec()),
    ] {
        let _env = Env::with(vars);
        let expected: Vec<PathBuf> = expected.into_iter().map(PathBuf::from).collect();
        assert_eq!(xdg_application_dirs(), expected, "{vars:?}");
    }
}

#[test]
fn the_xdg_directories_come_from_the_variables_or_the_specifications_defaults() {
    {
        let _env = Env::with(&[
            ("XDG_CONFIG_HOME", "/c"),
            ("XDG_CONFIG_DIRS", "/c1:/c2"),
            ("XDG_DATA_HOME", "/d"),
            ("XDG_DATA_DIRS", "/d1:/d2"),
            ("XDG_CURRENT_DESKTOP", "A:B"),
        ]);
        let dirs = XdgDirs::from_env().unwrap();
        assert_eq!(dirs.config_home, PathBuf::from("/c"));
        assert_eq!(dirs.config_dirs, [PathBuf::from("/c1"), "/c2".into()]);
        assert_eq!(dirs.data_home, PathBuf::from("/d"));
        assert_eq!(dirs.data_dirs, [PathBuf::from("/d1"), "/d2".into()]);
        assert_eq!(dirs.current_desktops, ["A", "B"]);
        assert_eq!(
            dirs.all_data_dirs(),
            [PathBuf::from("/d"), "/d1".into(), "/d2".into()]
        );
    }
    let _env = Env::with(&[("HOME", "/home/u")]);
    let dirs = XdgDirs::from_env().unwrap();
    assert_eq!(dirs.config_home, PathBuf::from("/home/u/.config"));
    assert_eq!(dirs.config_dirs, [PathBuf::from("/etc/xdg")]);
    assert_eq!(dirs.data_home, PathBuf::from("/home/u/.local/share"));
    assert_eq!(
        dirs.data_dirs,
        [PathBuf::from("/usr/local/share"), "/usr/share".into()]
    );
    assert!(dirs.current_desktops.is_empty());
}

#[test]
fn the_xdg_directories_need_a_home_when_a_default_is_used() {
    let env = Env::new();
    let err = XdgDirs::from_env().unwrap_err();
    assert!(
        err.contains("XDG_CONFIG_HOME") && err.contains("HOME"),
        "{err}"
    );
    env.set("XDG_CONFIG_HOME", "/c");
    let err = XdgDirs::from_env().unwrap_err();
    assert!(err.contains("XDG_DATA_HOME"), "{err}");
    env.set("XDG_DATA_HOME", "/d");
    assert!(XdgDirs::from_env().is_ok());
}

#[test]
fn the_usage_store_and_config_home_follow_their_xdg_variable_else_the_home() {
    let home_store = PathBuf::from("/home/u/.local/share").join(STORE_NAME);
    let home_config = PathBuf::from("/home/u/.config");
    for (vars, store, config) in [
        (&[][..], None, None),
        (
            &[("HOME", "/home/u")],
            Some(home_store.clone()),
            Some(home_config.clone()),
        ),
        (
            &[
                ("HOME", "/home/u"),
                ("XDG_DATA_HOME", "/d"),
                ("XDG_CONFIG_HOME", "/c"),
            ],
            Some(PathBuf::from("/d").join(STORE_NAME)),
            Some(PathBuf::from("/c")),
        ),
        (
            &[("HOME", "/home/u"), ("XDG_CONFIG_HOME", "")],
            Some(home_store.clone()),
            Some(home_config.clone()),
        ),
    ] {
        let _env = Env::with(vars);
        assert_eq!(default_path(), store, "{vars:?}");
        assert_eq!(config_home(), config, "{vars:?}");
    }
}

#[test]
fn a_catalogue_from_the_environment_reads_its_application_folders() {
    let env = Env::new();
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "applications/a.desktop",
        &desktop("A", "sh", ""),
    );
    write(
        dir.path(),
        "applications/gnome-only.desktop",
        &desktop("G", "sh", "OnlyShowIn=GNOME;"),
    );
    env.set("XDG_DATA_HOME", dir.path())
        .set("XDG_DATA_DIRS", dir.path().join("none"))
        .set("XDG_CURRENT_DESKTOP", "Cupertino")
        .set("PATH", "/bin:/usr/bin");
    let catalogue = Catalogue::from_env();
    assert_eq!(catalogue.dirs()[0], dir.path().join("applications"));
    let ids: Vec<&str> = catalogue.entries().iter().map(|e| e.id.as_str()).collect();
    assert_eq!(ids, ["a.desktop"]);
}
