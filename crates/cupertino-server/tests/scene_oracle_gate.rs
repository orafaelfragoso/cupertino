//! The old scene builder is compiled only for the gate's oracle step (`CUPERTINO_SCENE_ORACLE=1`, see `build.rs`):
//! a default, release or `--all-features` build must not contain it, because the comparison it runs panics when
//! the builders differ.

#[test]
fn the_oracle_is_compiled_only_for_the_oracle_step() {
    assert_eq!(
        cfg!(scene_oracle),
        std::env::var("CUPERTINO_SCENE_ORACLE").as_deref() == Ok("1"),
        "build and run this crate's tests with CUPERTINO_SCENE_ORACLE=1 for the oracle step, or without it"
    );
}
