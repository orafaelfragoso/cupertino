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

fn system_packs() -> [PathBuf; 2] {
    [
        PathBuf::from(SYSTEM_PACK).join("cupertino"),
        PathBuf::from(SYSTEM_PACK),
    ]
}

#[test]
fn the_variable_names_are_the_documented_ones() {
    assert_eq!(ASSET_DIR_ENV, "CUPERTINO_ASSET_DIR");
    assert_eq!(SEARCH_PATH_ENV, "CUPERTINO_ASSET_SEARCH_PATH");
    assert_eq!(SYSTEM_PACK, "/usr/share/cupertino/assets");
}

#[test]
fn the_default_list_is_asset_dir_user_pack_then_the_two_system_packs() {
    let mut env = clean();
    env.set(ASSET_DIR_ENV, "/first")
        .set("XDG_DATA_HOME", "/data");
    let [a, b] = system_packs();
    assert_eq!(
        search_path(),
        vec![
            PathBuf::from("/first"),
            PathBuf::from("/data/cupertino/assets"),
            a,
            b
        ]
    );
}

#[test]
fn an_empty_asset_dir_is_ignored() {
    let mut env = clean();
    env.set(ASSET_DIR_ENV, "").set("XDG_DATA_HOME", "/data");
    let [a, b] = system_packs();
    assert_eq!(
        search_path(),
        vec![PathBuf::from("/data/cupertino/assets"), a, b]
    );
}

#[test]
fn the_user_pack_is_under_home_when_xdg_data_home_is_unset() {
    let mut env = clean();
    env.set("HOME", "/home/someone");
    assert_eq!(
        search_path()[0],
        PathBuf::from("/home/someone/.local/share/cupertino/assets")
    );
}

#[test]
fn an_empty_xdg_data_home_counts_as_unset() {
    let mut env = clean();
    env.set("XDG_DATA_HOME", "").set("HOME", "/home/someone");
    assert_eq!(
        search_path()[0],
        PathBuf::from("/home/someone/.local/share/cupertino/assets")
    );
}

#[test]
fn without_home_or_xdg_data_home_there_is_no_user_pack() {
    let mut env = clean();
    env.set(ASSET_DIR_ENV, "/first");
    let [a, b] = system_packs();
    assert_eq!(search_path(), vec![PathBuf::from("/first"), a, b]);
}

#[test]
fn the_search_path_variable_replaces_the_whole_list() {
    let mut env = clean();
    env.set(SEARCH_PATH_ENV, "/a:/b")
        .set(ASSET_DIR_ENV, "/ignored")
        .set("XDG_DATA_HOME", "/ignored-too");
    assert_eq!(
        search_path(),
        vec![PathBuf::from("/a"), PathBuf::from("/b")]
    );
}

#[test]
fn empty_entries_of_the_search_path_variable_are_skipped() {
    let mut env = clean();
    env.set(SEARCH_PATH_ENV, "/a::/b:");
    assert_eq!(
        search_path(),
        vec![PathBuf::from("/a"), PathBuf::from("/b")]
    );
}

#[test]
fn an_empty_search_path_variable_selects_the_built_in_fallback_alone() {
    let mut env = clean();
    env.set(SEARCH_PATH_ENV, "").set(ASSET_DIR_ENV, "/ignored");
    assert!(search_path().is_empty());
    let assets = Assets::discover();
    assert_eq!(assets.roots().count(), 0);
    assert_eq!(assets.fonts(), vec![FontSource::Builtin]);
}

#[test]
fn discover_reads_the_packs_of_the_search_path_in_order() {
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    manifest(first.path(), r#"{"version": 2, "name": "one"}"#);
    manifest(second.path(), r#"{"version": 2, "name": "two"}"#);
    let mut env = clean();
    env.set(
        SEARCH_PATH_ENV,
        std::env::join_paths([first.path(), second.path()]).unwrap(),
    );
    let assets = Assets::discover();
    let roots: Vec<_> = assets.roots().collect();
    assert_eq!(roots, vec![first.path(), second.path()]);
    let names: Vec<_> = assets.credits().map(|(_, name, _)| name).collect();
    assert_eq!(names, vec![Some("one"), Some("two")]);
}

#[test]
fn discover_skips_a_missing_directory_and_one_without_a_usable_manifest() {
    let parent = tempfile::tempdir().unwrap();
    let missing = parent.path().join("nowhere");
    let no_manifest = tempfile::tempdir().unwrap();
    let bad_version = tempfile::tempdir().unwrap();
    let good = tempfile::tempdir().unwrap();
    manifest(bad_version.path(), r#"{"version": 9}"#);
    manifest(good.path(), r#"{"version": 2}"#);
    let mut env = clean();
    env.set(
        SEARCH_PATH_ENV,
        std::env::join_paths([
            missing.as_path(),
            no_manifest.path(),
            bad_version.path(),
            good.path(),
        ])
        .unwrap(),
    );
    let assets = Assets::discover();
    assert_eq!(assets.roots().collect::<Vec<_>>(), vec![good.path()]);
}

#[test]
fn the_asset_dir_pack_comes_before_the_user_pack() {
    let first = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    manifest(first.path(), r#"{"version": 2, "name": "env"}"#);
    manifest(
        &data.path().join("cupertino/assets"),
        r#"{"version": 2, "name": "user"}"#,
    );
    let mut env = clean();
    env.set(ASSET_DIR_ENV, first.path())
        .set("XDG_DATA_HOME", data.path());
    let assets = Assets::discover();
    let names: Vec<_> = assets.credits().filter_map(|(_, n, _)| n).take(2).collect();
    assert_eq!(names, vec!["env", "user"]);
}
