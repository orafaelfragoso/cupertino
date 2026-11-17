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
fn the_spec_dir_is_the_variable_when_set_and_the_built_in_directory_otherwise() {
    let mut env = Env::new();
    env.unset(SPEC_DIR_ENV);
    let built_in = spec_dir();
    match option_env!("CUPERTINO_THEME_DIR") {
        Some(dir) => assert_eq!(built_in, PathBuf::from(dir)),
        None => assert_eq!(
            built_in.canonicalize().unwrap(),
            repository_spec().canonicalize().unwrap(),
            "the checkout's data/theme"
        ),
    }
    env.set(SPEC_DIR_ENV, "");
    assert_eq!(spec_dir(), built_in, "an empty variable counts as unset");
    env.set(SPEC_DIR_ENV, "/somewhere/else");
    assert_eq!(spec_dir(), PathBuf::from("/somewhere/else"));
}

#[test]
fn load_reads_the_theme_files_of_the_spec_dir() {
    let copy = copy_of_spec();
    let original = from_dir(repository_spec()).expect("the repository's theme data loads");
    let mut env = Env::new();
    env.set(SPEC_DIR_ENV, copy.path());
    let copied = load().expect("the copy loads through the variable");
    for appearance in [Appearance::Light, Appearance::Dark] {
        let (o, c) = (
            original.tokens.table(appearance),
            copied.tokens.table(appearance),
        );
        assert!(!o.is_empty(), "{appearance:?}");
        assert_eq!(o.len(), c.len(), "{appearance:?}");
        for name in o.names() {
            assert_eq!(o.get(name), c.get(name), "{appearance:?} {name}");
        }
    }
}

#[test]
fn a_missing_theme_file_is_an_io_error_naming_it() {
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
fn a_broken_theme_file_is_an_error() {
    for (file, text) in [
        (TOKENS_FILE, "{ this is not json"),
        (MATERIALS_FILE, "["),
        (ANIMATION_FILE, ""),
    ] {
        let copy = copy_of_spec();
        std::fs::write(copy.path().join(file), text).unwrap();
        let result = from_dir(copy.path());
        assert!(result.is_err(), "{file}");
        if file == TOKENS_FILE {
            assert!(matches!(result, Err(Error::Json(_))), "{file}");
        }
    }
}
