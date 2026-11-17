//! Where the settings files and the XDG directories are, from the environment.

use std::path::PathBuf;

use cupertino_defaults::{config_file, config_home, data_dirs};

/// Sets each variable to its value, or removes it for `None`.
fn environment(vars: &[(&str, Option<&str>)]) {
    for (name, value) in vars {
        match value {
            Some(v) => std::env::set_var(name, v),
            None => std::env::remove_var(name),
        }
    }
}

#[test]
fn the_config_home_is_xdg_config_home_or_home_dot_config() {
    for (row, vars, expected) in [
        (
            "XDG_CONFIG_HOME set",
            [("XDG_CONFIG_HOME", Some("/xdg")), ("HOME", Some("/home/u"))],
            Some("/xdg"),
        ),
        (
            "only HOME",
            [("XDG_CONFIG_HOME", None), ("HOME", Some("/home/u"))],
            Some("/home/u/.config"),
        ),
        (
            "empty XDG_CONFIG_HOME counts as unset",
            [("XDG_CONFIG_HOME", Some("")), ("HOME", Some("/home/u"))],
            Some("/home/u/.config"),
        ),
        ("neither", [("XDG_CONFIG_HOME", None), ("HOME", None)], None),
    ] {
        environment(&vars);
        assert_eq!(config_home(), expected.map(PathBuf::from), "{row}");
    }
}

#[test]
fn a_config_file_is_its_override_or_under_the_cupertino_directory_of_the_config_home() {
    const VAR: &str = "CUPERTINO_TEST_CONF";
    for (row, vars, expected) in [
        (
            "under the config home",
            [
                (VAR, None),
                ("XDG_CONFIG_HOME", Some("/xdg")),
                ("HOME", None),
            ],
            Some("/xdg/cupertino/x.conf"),
        ),
        (
            "the override wins",
            [
                (VAR, Some("/elsewhere/y.conf")),
                ("XDG_CONFIG_HOME", Some("/xdg")),
                ("HOME", None),
            ],
            Some("/elsewhere/y.conf"),
        ),
        (
            "an empty override is ignored",
            [
                (VAR, Some("")),
                ("XDG_CONFIG_HOME", Some("/xdg")),
                ("HOME", None),
            ],
            Some("/xdg/cupertino/x.conf"),
        ),
        (
            "an override without a config home",
            [
                (VAR, Some("/only.conf")),
                ("XDG_CONFIG_HOME", None),
                ("HOME", None),
            ],
            Some("/only.conf"),
        ),
        (
            "no override and no config home",
            [(VAR, None), ("XDG_CONFIG_HOME", None), ("HOME", None)],
            None,
        ),
    ] {
        environment(&vars);
        assert_eq!(
            config_file(VAR, "x.conf"),
            expected.map(PathBuf::from),
            "{row}"
        );
    }
}

#[test]
fn the_data_directories_are_the_data_home_then_the_data_dirs() {
    for (row, vars, expected) in [
        (
            "both set",
            [
                ("XDG_DATA_HOME", Some("/data")),
                ("XDG_DATA_DIRS", Some("/a:/b")),
            ],
            &["/data", "/a", "/b"][..],
        ),
        (
            "empty entries are skipped",
            [
                ("XDG_DATA_HOME", Some("/data")),
                ("XDG_DATA_DIRS", Some("/a::/b:")),
            ],
            &["/data", "/a", "/b"][..],
        ),
        (
            "defaults",
            [("XDG_DATA_HOME", None), ("XDG_DATA_DIRS", None)],
            &["/home/u/.local/share", "/usr/local/share", "/usr/share"][..],
        ),
    ] {
        environment(&vars);
        std::env::set_var("HOME", "/home/u");
        let expected: Vec<PathBuf> = expected.iter().map(PathBuf::from).collect();
        assert_eq!(data_dirs(), expected, "{row}");
    }
}
