//! Sets `--cfg scene_oracle` for the oracle step of the gate (`CUPERTINO_SCENE_ORACLE=1`), which keeps the
//! pre-scene-frame builder to compare the composition against; removed with wp04-seam-gates.
//!
//! This is a build script and not a `RUSTFLAGS` cfg like `cupertino_capture_fixtures` for one reason: `RUSTFLAGS`
//! changes the fingerprint of every dependency, so the oracle step would need a second full set of dependency
//! artifacts, while a build script recompiles this crate alone. It is never a cargo feature, so `--all-features`
//! builds cannot contain it, and a release build refuses to set it. The guard looks at `PROFILE`, which is `release`
//! for every profile that inherits from release, so custom release profiles are covered too. Packaging must not export
//! `CUPERTINO_SCENE_ORACLE`: a debug build with it set compiles the oracle in.

fn main() {
    println!("cargo:rustc-check-cfg=cfg(scene_oracle)");
    println!("cargo:rerun-if-env-changed=CUPERTINO_SCENE_ORACLE");
    if std::env::var("CUPERTINO_SCENE_ORACLE").as_deref() != Ok("1") {
        return;
    }
    let profile = std::env::var("PROFILE").unwrap_or_default();
    assert!(
        profile != "release",
        "CUPERTINO_SCENE_ORACLE=1 is for test builds of the gate; a release build must not contain the oracle"
    );
    println!("cargo:rustc-cfg=scene_oracle");
}
