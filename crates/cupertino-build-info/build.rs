//! Finds the commit the sources come from and hands it to the crate as `CUPERTINO_BUILD_INFO_COMMIT` (empty when
//! unknown) and `CUPERTINO_BUILD_INFO_COMMIT_SOURCE`. The order is in `build/commit.rs`.
//!
//! The script reruns only when the variable or the archive file changes, not on a new commit in a checkout: a commit
//! would otherwise rebuild every binary that links this crate.

#[path = "build/commit.rs"]
mod commit;

use std::path::Path;

fn main() {
    // the workspace directory: this crate is `crates/cupertino-build-info`
    let manifest = std::env::var_os("CARGO_MANIFEST_DIR").expect("Cargo sets CARGO_MANIFEST_DIR");
    let root = Path::new(&manifest).join("../..");
    println!("cargo:rerun-if-env-changed={}", commit::VARIABLE);
    println!("cargo:rerun-if-changed=../../{}", commit::ARCHIVE_FILE);
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=build/commit.rs");
    let variable = std::env::var_os(commit::VARIABLE);
    match commit::resolve(variable.as_deref(), &root, commit::git_head) {
        Ok(found) => {
            for warning in &found.warnings {
                println!("cargo:warning={warning}");
            }
            println!(
                "cargo:rustc-env=CUPERTINO_BUILD_INFO_COMMIT={}",
                found.commit.unwrap_or_default()
            );
            println!(
                "cargo:rustc-env=CUPERTINO_BUILD_INFO_COMMIT_SOURCE={}",
                found.source.name()
            );
        }
        Err(message) => {
            eprintln!("error: {message}");
            std::process::exit(1);
        }
    }
}
