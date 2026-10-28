//! The old scene builder is a rustc cfg for the gate's oracle step alone: a default, release or `--all-features`
//! build must not compile it, because the comparison it runs panics when the builders differ.

#[test]
fn the_oracle_is_compiled_only_for_the_oracle_step() {
    assert_eq!(
        cfg!(scene_oracle),
        std::env::var_os("CUPERTINO_SCENE_ORACLE_STEP").is_some(),
        "run this crate's tests with RUSTFLAGS=\"--cfg scene_oracle\" and CUPERTINO_SCENE_ORACLE_STEP=1 together, \
         or with neither"
    );
}
