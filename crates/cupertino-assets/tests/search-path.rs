//! The pack search list and discovery through the environment variables.

mod common;

use common::{manifest, Env};
use cupertino_assets::{
    search_path, Assets, FontSource, ASSET_DIR_ENV, SEARCH_PATH_ENV, SYSTEM_PACK,
};
use std::path::PathBuf;

/// An environment with none of the variables the list reads.
fn clean() -> Env {
    let mut env = Env::new();
    for name in [SEARCH_PATH_ENV, ASSET_DIR_ENV, "XDG_DATA_HOME", "HOME"] {
        env.unset(name);
    }
    env
}

/// A row: its name, the variables set, the list before the system packs, whether the system packs follow.
type Row = (
    &'static str,
    &'static [(&'static str, &'static str)],
    Vec<&'static str>,
    bool,
);

#[test]
fn the_search_list_follows_the_environment() {
    let system = [
        PathBuf::from(SYSTEM_PACK).join("cupertino"),
        PathBuf::from(SYSTEM_PACK),
    ];
    let user = "/home/someone/.local/share/cupertino/assets";
    let rows: [Row; 8] = [
        (
            "asset dir, user pack, then the system packs",
            &[(ASSET_DIR_ENV, "/first"), ("XDG_DATA_HOME", "/data")],
            vec!["/first", "/data/cupertino/assets"],
            true,
        ),
        (
            "an empty asset dir is ignored",
            &[(ASSET_DIR_ENV, ""), ("XDG_DATA_HOME", "/data")],
            vec!["/data/cupertino/assets"],
            true,
        ),
        (
            "the user pack is under HOME without XDG_DATA_HOME",
            &[("HOME", "/home/someone")],
            vec![user],
            true,
        ),
        (
            "an empty XDG_DATA_HOME counts as unset",
            &[("XDG_DATA_HOME", ""), ("HOME", "/home/someone")],
            vec![user],
            true,
        ),
        (
            "without HOME or XDG_DATA_HOME there is no user pack",
            &[(ASSET_DIR_ENV, "/first")],
            vec!["/first"],
            true,
        ),
        (
            "the search path variable replaces the whole list",
            &[
                (SEARCH_PATH_ENV, "/a:/b"),
                (ASSET_DIR_ENV, "/ignored"),
                ("XDG_DATA_HOME", "/ignored-too"),
            ],
            vec!["/a", "/b"],
            false,
        ),
        (
            "empty entries of the search path are skipped",
            &[(SEARCH_PATH_ENV, "/a::/b:")],
            vec!["/a", "/b"],
            false,
        ),
        (
            "an empty search path is an empty list",
            &[(SEARCH_PATH_ENV, ""), (ASSET_DIR_ENV, "/ignored")],
            vec![],
            false,
        ),
    ];
    for (row, vars, first, with_system) in rows {
        let mut env = clean();
        for (name, value) in vars {
            env.set(name, value);
        }
        let mut expected: Vec<PathBuf> = first.into_iter().map(PathBuf::from).collect();
        if with_system {
            expected.extend(system.iter().cloned());
        }
        assert_eq!(search_path(), expected, "{row}");
    }
}

#[test]
fn discover_reads_the_usable_packs_of_the_search_path_in_order() {
    let parent = tempfile::tempdir().unwrap();
    let missing = parent.path().join("nowhere");
    let no_manifest = tempfile::tempdir().unwrap();
    let bad_version = tempfile::tempdir().unwrap();
    let one = tempfile::tempdir().unwrap();
    let two = tempfile::tempdir().unwrap();
    manifest(bad_version.path(), r#"{"version": 9}"#);
    manifest(one.path(), r#"{"version": 2, "name": "one"}"#);
    manifest(two.path(), r#"{"version": 2, "name": "two"}"#);
    let mut env = clean();
    env.set(
        SEARCH_PATH_ENV,
        std::env::join_paths([
            missing.as_path(),
            one.path(),
            no_manifest.path(),
            bad_version.path(),
            two.path(),
        ])
        .unwrap(),
    );
    let assets = Assets::discover();
    assert_eq!(
        assets.roots().collect::<Vec<_>>(),
        vec![one.path(), two.path()]
    );
    let names: Vec<_> = assets.credits().map(|(_, name, _)| name).collect();
    assert_eq!(names, vec![Some("one"), Some("two")]);

    env.set(SEARCH_PATH_ENV, "");
    let assets = Assets::discover();
    assert_eq!(assets.roots().count(), 0, "an empty search path");
    assert_eq!(assets.fonts(), vec![FontSource::Builtin]);
}
