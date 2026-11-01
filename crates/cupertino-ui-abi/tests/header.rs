//! `abi/cupertinokit.h` is what cbindgen makes of `src/lib.rs` with `cbindgen.toml`. `CUPERTINO_UPDATE_GOLDENS=1`
//! rewrites it.

use std::path::Path;

#[test]
fn the_committed_header_is_current() {
    let crate_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let config = cbindgen::Config::from_file(crate_dir.join("cbindgen.toml")).unwrap();
    let mut header = Vec::new();
    cbindgen::Builder::new()
        .with_config(config)
        .with_src(crate_dir.join("src/lib.rs"))
        .generate()
        .unwrap()
        .write(&mut header);
    let header = String::from_utf8(header).unwrap();

    let golden = crate_dir.join("abi/cupertinokit.h");
    if std::env::var("CUPERTINO_UPDATE_GOLDENS").as_deref() == Ok("1") {
        std::fs::write(&golden, &header).unwrap();
    }
    assert_eq!(
        header,
        std::fs::read_to_string(&golden).unwrap(),
        "abi/cupertinokit.h is stale; rerun with CUPERTINO_UPDATE_GOLDENS=1 and review the change"
    );
}
