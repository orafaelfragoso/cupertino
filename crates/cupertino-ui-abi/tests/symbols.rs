//! The built `libcupertinokit.so` exports exactly `abi/symbols-1.txt`, with no symbol versions, as
//! `libcupertinokit.so.1`, and is never unloaded. `CUPERTINO_UPDATE_GOLDENS=1` rewrites the list.

use std::path::{Path, PathBuf};

use object::elf;
use object::read::elf::ElfFile64;
use object::{Object, ObjectSection, ObjectSymbol};

#[path = "../../../ci/test-fixtures.rs"]
#[allow(dead_code)]
mod fixtures;

/// Cargo's exact cdylib artifact, prepared before this test starts.
fn library() -> PathBuf {
    fixtures::artifact("cupertino-ui-abi", "cdylib", "cupertinokit")
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
