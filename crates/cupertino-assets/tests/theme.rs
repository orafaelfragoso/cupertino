//! Finding the theme data and loading the theme from it.

mod common;

use common::Env;
use cupertino_assets::theme::{
    from_dir, load, spec_dir, ANIMATION_FILE, MATERIALS_FILE, SPEC_DIR_ENV, TOKENS_FILE,
};
use cupertino_theme::{Appearance, Error};
use std::path::{Path, PathBuf};

/// The repository's theme data, read in place and only read.
fn repository_spec() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/theme")
}

/// A temp dir holding copies of the repository's three theme files.
fn copy_of_spec() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for file in [TOKENS_FILE, MATERIALS_FILE, ANIMATION_FILE] {
        std::fs::copy(repository_spec().join(file), dir.path().join(file)).unwrap();
    }
    dir
}

#[test]
fn the_theme_file_names_are_the_documented_ones() {
    assert_eq!(SPEC_DIR_ENV, "CUPERTINO_SPEC_DIR");
    assert_eq!(TOKENS_FILE, "tokens.json");
    assert_eq!(MATERIALS_FILE, "materials.json");
    assert_eq!(ANIMATION_FILE, "animations.json");
}

#[test]
fn the_spec_dir_variable_overrides_the_built_in_directory() {
    let mut env = Env::new();
    env.set(SPEC_DIR_ENV, "/somewhere/else");
    assert_eq!(spec_dir(), PathBuf::from("/somewhere/else"));
}

#[test]
fn an_empty_spec_dir_variable_counts_as_unset() {
    let mut env = Env::new();
    env.unset(SPEC_DIR_ENV);
    let built_in = spec_dir();
    env.set(SPEC_DIR_ENV, "");
    assert_eq!(spec_dir(), built_in);
}

#[test]
fn without_the_variable_the_spec_dir_is_the_directory_built_in() {
    let mut env = Env::new();
    env.unset(SPEC_DIR_ENV);
    match option_env!("CUPERTINO_THEME_DIR") {
        Some(dir) => assert_eq!(spec_dir(), PathBuf::from(dir)),
        None => {
            // the checkout's data/theme, where the three files are
            let dir = spec_dir();
            assert!(dir.ends_with("data/theme"), "{}", dir.display());
            assert_eq!(
                dir.canonicalize().unwrap(),
                repository_spec().canonicalize().unwrap()
            );
        }
    }
}

#[test]
fn from_dir_loads_the_repositorys_theme_files() {
    let theme = from_dir(repository_spec()).expect("the repository's theme data loads");
    // the tokens came from the file: they name colours in both appearances
    assert!(!theme.tokens.table(Appearance::Light).is_empty());
    assert!(!theme.tokens.table(Appearance::Dark).is_empty());
}

#[test]
fn from_dir_loads_a_copy_of_the_theme_files_the_same_as_the_original() {
    let copy = copy_of_spec();
    let original = from_dir(repository_spec()).unwrap();
    let copied = from_dir(copy.path()).unwrap();
    for appearance in [Appearance::Light, Appearance::Dark] {
        let (o, c) = (
            original.tokens.table(appearance),
            copied.tokens.table(appearance),
        );
        assert_eq!(o.len(), c.len());
        for name in o.names() {
            assert_eq!(o.get(name), c.get(name), "{name}");
        }
    }
}

#[test]
fn load_reads_the_directory_the_spec_dir_variable_names() {
    let copy = copy_of_spec();
    let mut env = Env::new();
    env.set(SPEC_DIR_ENV, copy.path());
    assert!(load().is_ok());
}

#[test]
fn load_from_a_directory_without_the_files_reports_the_first_missing_path() {
    let empty = tempfile::tempdir().unwrap();
    let mut env = Env::new();
    env.set(SPEC_DIR_ENV, empty.path());
    match load() {
        Err(Error::Io { path, .. }) => assert_eq!(path, empty.path().join(TOKENS_FILE)),
        other => panic!("expected an io error, got {other:?}"),
    }
}

#[test]
fn each_missing_theme_file_is_an_io_error_with_its_own_path() {
    for file in [TOKENS_FILE, MATERIALS_FILE, ANIMATION_FILE] {
        let copy = copy_of_spec();
        std::fs::remove_file(copy.path().join(file)).unwrap();
        match from_dir(copy.path()) {
            Err(Error::Io { path, .. }) => assert_eq!(path, copy.path().join(file)),
            other => panic!("{file}: expected an io error, got {other:?}"),
        }
    }
}

#[test]
fn from_dir_on_a_broken_tokens_file_reports_the_error() {
    let copy = copy_of_spec();
    std::fs::write(copy.path().join(TOKENS_FILE), "{ this is not json").unwrap();
    assert!(matches!(from_dir(copy.path()), Err(Error::Json(_))));
}

#[test]
fn from_dir_on_a_broken_materials_file_reports_the_error() {
    let copy = copy_of_spec();
    std::fs::write(copy.path().join(MATERIALS_FILE), "[").unwrap();
    assert!(from_dir(copy.path()).is_err());
}

#[test]
fn from_dir_on_a_broken_animations_file_reports_the_error() {
    let copy = copy_of_spec();
    std::fs::write(copy.path().join(ANIMATION_FILE), "").unwrap();
    assert!(from_dir(copy.path()).is_err());
}

#[test]
fn an_unreadable_theme_file_is_an_error_that_displays_its_path() {
    let copy = copy_of_spec();
    std::fs::remove_file(copy.path().join(MATERIALS_FILE)).unwrap();
    let text = from_dir(copy.path()).err().unwrap().to_string();
    assert!(text.contains(MATERIALS_FILE), "{text}");
}
