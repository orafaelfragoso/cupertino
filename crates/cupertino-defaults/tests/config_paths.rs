//! Where the settings files and the XDG directories are, from the environment.

use std::path::PathBuf;

use cupertino_defaults::{config_file, config_home, data_dirs};

fn set(name: &str, value: &str) {
    std::env::set_var(name, value);
}

fn clear(names: &[&str]) {
    for n in names {
        std::env::remove_var(n);
    }
}

#[test]
fn the_config_home_is_xdg_config_home() {
    clear(&["XDG_CONFIG_HOME", "HOME"]);
    set("XDG_CONFIG_HOME", "/xdg");
    set("HOME", "/home/u");
    assert_eq!(config_home(), Some(PathBuf::from("/xdg")));
}

#[test]
fn the_config_home_falls_back_to_home_dot_config() {
    clear(&["XDG_CONFIG_HOME"]);
    set("HOME", "/home/u");
    assert_eq!(config_home(), Some(PathBuf::from("/home/u/.config")));
}

#[test]
fn an_empty_xdg_config_home_counts_as_unset() {
    set("XDG_CONFIG_HOME", "");
    set("HOME", "/home/u");
    assert_eq!(config_home(), Some(PathBuf::from("/home/u/.config")));
}

#[test]
fn the_config_home_is_none_without_either_variable() {
    clear(&["XDG_CONFIG_HOME", "HOME"]);
    assert_eq!(config_home(), None);
}

#[test]
fn a_config_file_is_under_the_cupertino_directory_of_the_config_home() {
    clear(&["CUPERTINO_TEST_CONF"]);
    set("XDG_CONFIG_HOME", "/xdg");
    assert_eq!(
        config_file("CUPERTINO_TEST_CONF", "x.conf"),
        Some(PathBuf::from("/xdg/cupertino/x.conf"))
    );
}

#[test]
fn the_environment_override_wins_over_the_config_home() {
    set("XDG_CONFIG_HOME", "/xdg");
    set("CUPERTINO_TEST_CONF", "/elsewhere/y.conf");
    assert_eq!(
        config_file("CUPERTINO_TEST_CONF", "x.conf"),
        Some(PathBuf::from("/elsewhere/y.conf"))
    );
}

#[test]
fn an_empty_override_is_ignored() {
    set("XDG_CONFIG_HOME", "/xdg");
    set("CUPERTINO_TEST_CONF", "");
    assert_eq!(
        config_file("CUPERTINO_TEST_CONF", "x.conf"),
        Some(PathBuf::from("/xdg/cupertino/x.conf"))
    );
}

#[test]
fn an_override_gives_a_path_even_without_a_config_home() {
    clear(&["XDG_CONFIG_HOME", "HOME"]);
    set("CUPERTINO_TEST_CONF", "/only.conf");
    assert_eq!(
        config_file("CUPERTINO_TEST_CONF", "x.conf"),
        Some(PathBuf::from("/only.conf"))
    );
}

#[test]
fn a_config_file_is_none_without_override_and_config_home() {
    clear(&["XDG_CONFIG_HOME", "HOME", "CUPERTINO_TEST_CONF"]);
    assert_eq!(config_file("CUPERTINO_TEST_CONF", "x.conf"), None);
}

#[test]
fn the_data_directories_are_the_data_home_then_the_data_dirs() {
    set("XDG_DATA_HOME", "/data");
    set("XDG_DATA_DIRS", "/a:/b");
    assert_eq!(
        data_dirs(),
        [
            PathBuf::from("/data"),
            PathBuf::from("/a"),
            PathBuf::from("/b")
        ]
    );
}

#[test]
fn the_data_directories_default_to_the_local_share_and_the_system_ones() {
    clear(&["XDG_DATA_HOME", "XDG_DATA_DIRS"]);
    set("HOME", "/home/u");
    assert_eq!(
        data_dirs(),
        [
            PathBuf::from("/home/u/.local/share"),
            PathBuf::from("/usr/local/share"),
            PathBuf::from("/usr/share")
        ]
    );
}

#[test]
fn empty_entries_of_the_data_dirs_are_skipped() {
    set("XDG_DATA_HOME", "/data");
    set("XDG_DATA_DIRS", "/a::/b:");
    assert_eq!(
        data_dirs(),
        [
            PathBuf::from("/data"),
            PathBuf::from("/a"),
            PathBuf::from("/b")
        ]
    );
}
