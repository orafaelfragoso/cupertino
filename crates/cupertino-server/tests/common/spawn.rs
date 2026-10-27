//! Starting a Cupertino binary from a test with a cleared environment.
//!
//! A package build compiles the installed theme directory into every binary (`CUPERTINO_THEME_DIR`), and that directory
//! does not exist while the tests run. A binary started with a cleared environment finds the theme files only through
//! `CUPERTINO_SPEC_DIR`, so every such spawn starts from [`cleared`], which always sets it.
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The theme files of the checkout.
pub fn spec_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data/theme")
}

/// `binary` with an empty environment but `PATH`, `HOME` (the folder `home`) and `CUPERTINO_SPEC_DIR`; the caller adds
/// the rest.
pub fn cleared(binary: impl AsRef<OsStr>, home: &Path) -> Command {
    let mut command = Command::new(binary);
    command
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", home)
        .env("CUPERTINO_SPEC_DIR", spec_dir());
    command
}
