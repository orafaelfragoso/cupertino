//! The built `libcupertinokit.so` exports exactly `abi/symbols-1.txt`, with no symbol versions, as
//! `libcupertinokit.so.1`, and is never unloaded. `CUPERTINO_UPDATE_GOLDENS=1` rewrites the list.

use std::path::{Path, PathBuf};
use std::process::Command;

use object::elf;
use object::read::elf::ElfFile64;
use object::{Object, ObjectSection, ObjectSymbol};

/// Builds `target/<profile>/libcupertinokit.so`, next to the `deps` directory this test runs from: a test build links
/// the rlib and leaves the cdylib as it was.
fn library() -> PathBuf {
    let exe = std::env::current_exe().unwrap();
    let profile_dir = exe.parent().and_then(Path::parent).unwrap();
    let mut cargo = Command::new(std::env::var_os("CARGO").expect("CARGO is set by `cargo test`"));
    cargo
        .args(["build", "-p", "cupertino-ui-abi", "--lib", "--target-dir"])
        .arg(profile_dir.parent().unwrap());
    if profile_dir.file_name().unwrap() == "release" {
        cargo.arg("--release");
    }
    let status = cargo.status().unwrap();
    assert!(status.success(), "building the library: {status}");
    profile_dir.join("libcupertinokit.so")
}

#[test]
fn the_library_exports_only_the_pinned_symbols_unversioned() {
    let data = std::fs::read(library()).unwrap();
    let file = ElfFile64::<object::Endianness>::parse(&*data).unwrap();

    let mut exported: Vec<&str> = file
        .dynamic_symbols()
        .filter(|s| s.is_definition() && s.is_global())
        .map(|s| s.name().unwrap())
        .collect();
    exported.sort_unstable();
    let foreign: Vec<&&str> = exported
        .iter()
        .filter(|name| !name.starts_with("ck_"))
        .collect();
    assert!(foreign.is_empty(), "only ck_* is exported, not {foreign:?}");

    let golden = Path::new(env!("CARGO_MANIFEST_DIR")).join("abi/symbols-1.txt");
    let listed = exported
        .iter()
        .map(|name| format!("{name}\n"))
        .collect::<String>();
    if std::env::var("CUPERTINO_UPDATE_GOLDENS").as_deref() == Ok("1") {
        std::fs::write(&golden, &listed).unwrap();
    }
    assert_eq!(
        listed,
        std::fs::read_to_string(&golden).unwrap(),
        "the exports differ from {}",
        golden.display()
    );

    assert!(
        file.section_by_name(".gnu.version_d").is_none(),
        "v1 defines no symbol version nodes"
    );

    let dynamic = file.elf_dynamic_table().unwrap();
    let soname = dynamic
        .iter()
        .find(|d| d.tag == elf::DT_SONAME)
        .map(|d| dynamic.string(d).unwrap());
    assert_eq!(soname, Some(&b"libcupertinokit.so.1"[..]));
    let flags = dynamic
        .iter()
        .find(|d| d.tag == elf::DT_FLAGS_1)
        .map_or(0, |d| d.val);
    assert_ne!(flags & elf::DF_1_NODELETE.0, 0, "linked with -z nodelete");
    assert!(file
        .section_by_name(".dynamic")
        .is_some_and(|s| s.size() > 0));
}
