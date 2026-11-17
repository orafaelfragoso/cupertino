//! Compiles in the repository's theme directory as an absolute path. Cargo fingerprints a path package by paths
//! relative to its checkout, so a build whose checkout moved (a gate's merge worktree that shares the branch's target)
//! kept the old directory; naming this script by its absolute path makes cargo run it again, and rebuild the crate,
//! whenever the checkout is somewhere else.

fn main() {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR");
    println!("cargo:rerun-if-changed={manifest}/build.rs");
    println!("cargo:rustc-env=CUPERTINO_REPOSITORY_THEME_DIR={manifest}/../../data/theme");
}
