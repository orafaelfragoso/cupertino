//! What the crate reads from the environment: locale, desktop, search path and the XDG directories.
//!
//! The tests of this file change environment variables, so each holds the lock and restores them.

mod common;

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
fn the_locale_comes_from_lc_all_then_lc_messages_then_lang() {
    let env = Env::new();
    env.set("LANG", "fr_FR.UTF-8");
    assert_eq!(Locale::from_env().candidates, ["fr_FR", "fr"]);
    env.set("LC_MESSAGES", "de_DE.UTF-8");
    assert_eq!(Locale::from_env().candidates, ["de_DE", "de"]);
    env.set("LC_ALL", "es_ES.UTF-8");
    assert_eq!(Locale::from_env().candidates, ["es_ES", "es"]);
}

#[test]
fn an_empty_locale_variable_is_skipped() {
    let env = Env::new();
    env.set("LC_ALL", "").set("LANG", "it_IT");
    assert_eq!(Locale::from_env().candidates, ["it_IT", "it"]);
}

#[test]
fn without_a_locale_variable_there_are_no_keys() {
    let _env = Env::new();
    assert!(Locale::from_env().candidates.is_empty());
}

#[test]
fn visibility_reads_the_desktops_and_the_search_path() {
    let env = Env::new();
    env.set("XDG_CURRENT_DESKTOP", "Cupertino::GNOME:")
        .set("PATH", "/one:/two");
    let vis = Visibility::from_env();
    assert_eq!(vis.current_desktops, ["Cupertino", "GNOME"]);
    assert_eq!(vis.path_dirs, [PathBuf::from("/one"), "/two".into()]);
    assert!(!vis.skip_try_exec);
}

#[test]
fn visibility_without_variables_is_empty() {
    let _env = Env::new();
    let vis = Visibility::from_env();
    assert!(vis.current_desktops.is_empty() && vis.path_dirs.is_empty());
}

#[test]
fn application_folders_run_from_the_data_home_through_the_data_dirs() {
    let env = Env::new();
    env.set("XDG_DATA_HOME", "/h")
        .set("XDG_DATA_DIRS", "/a::/b");
    assert_eq!(
        xdg_application_dirs(),
        [
            PathBuf::from("/h/applications"),
            "/a/applications".into(),
            "/b/applications".into()
        ]
    );
}

#[test]
fn application_folders_default_to_the_home_and_the_system_folders() {
    let env = Env::new();
    env.set("HOME", "/home/u");
    assert_eq!(
        xdg_application_dirs(),
        [
            PathBuf::from("/home/u/.local/share/applications"),
            "/usr/local/share/applications".into(),
            "/usr/share/applications".into()
        ]
    );
    env.set("XDG_DATA_HOME", "").set("XDG_DATA_DIRS", "");
    assert_eq!(xdg_application_dirs().len(), 3);
}

#[test]
fn application_folders_without_a_home_are_only_the_data_dirs() {
    let _env = Env::new();
    assert_eq!(
        xdg_application_dirs(),
        [
            PathBuf::from("/usr/local/share/applications"),
            "/usr/share/applications".into()
        ]
    );
}

#[test]
fn the_xdg_directories_come_from_the_variables() {
    let env = Env::new();
    env.set("XDG_CONFIG_HOME", "/c")
        .set("XDG_CONFIG_DIRS", "/c1:/c2")
        .set("XDG_DATA_HOME", "/d")
        .set("XDG_DATA_DIRS", "/d1:/d2")
        .set("XDG_CURRENT_DESKTOP", "A:B");
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

#[test]
fn the_xdg_directories_default_as_the_base_directory_specification_says() {
    let env = Env::new();
    env.set("HOME", "/home/u");
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
fn the_usage_store_is_in_the_data_home_of_the_environment() {
    let env = Env::new();
    assert_eq!(default_path(), None);
    env.set("HOME", "/home/u");
    assert_eq!(
        default_path(),
        Some(PathBuf::from("/home/u/.local/share").join(STORE_NAME))
    );
    env.set("XDG_DATA_HOME", "/d");
    assert_eq!(default_path(), Some(PathBuf::from("/d").join(STORE_NAME)));
}

#[test]
fn the_config_home_is_xdg_config_home_else_dot_config_in_the_home() {
    let env = Env::new();
    assert_eq!(config_home(), None);
    env.set("HOME", "/home/u");
    assert_eq!(config_home(), Some(PathBuf::from("/home/u/.config")));
    env.set("XDG_CONFIG_HOME", "/c");
    assert_eq!(config_home(), Some(PathBuf::from("/c")));
    env.set("XDG_CONFIG_HOME", "");
    assert_eq!(config_home(), Some(PathBuf::from("/home/u/.config")));
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
