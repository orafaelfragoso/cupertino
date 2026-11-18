//! What compiled programs depend on: the library exports exactly the functions of the ABI's first version, under its
//! soname, and every struct has the size, alignment and field offsets the header gives it.

use cupertinokit::{CkEvent, CkRuntime, CkRuntimeOptions, CkStr, CkWaker, CkWindow};
use object::{Object, ObjectSymbol};
use std::collections::BTreeSet;
use std::fmt::Write;
use std::mem::{align_of, offset_of, size_of};
use std::path::PathBuf;

/// The shared library the build made last: cargo puts it among the package's build outputs, or beside the test
/// binaries.
fn built_library() -> PathBuf {
    let exe = std::env::current_exe().unwrap();
    let profile = exe
        .ancestors()
        .find(|dir| dir.join("build").is_dir())
        .expect("the test binary is in a cargo target directory");
    let mut places = vec![profile.to_owned(), profile.join("deps")];
    if let Ok(builds) = std::fs::read_dir(profile.join("build/cupertino-ui-abi")) {
        places.extend(builds.flatten().map(|b| b.path().join("out")));
    }
    places
        .iter()
        .map(|dir| dir.join("libcupertinokit.so"))
        .filter_map(|path| Some((path.metadata().ok()?.modified().ok()?, path)))
        .max()
        .map(|(_, path)| path)
        .expect("the shared library is not built")
}

#[test]
fn the_library_exports_the_functions_of_version_1_and_nothing_else() {
    let library = built_library();
    let bytes = std::fs::read(&library)
        .unwrap_or_else(|e| panic!("the shared library {} is not built: {e}", library.display()));
    let file = object::File::parse(&*bytes).unwrap();
    let exported: BTreeSet<String> = file
        .dynamic_symbols()
        .filter(|s| s.is_definition() && s.is_global())
        .map(|s| s.name().unwrap().to_owned())
        .collect();
    let baseline: BTreeSet<String> = include_str!("../abi/symbols-1.txt")
        .lines()
        .map(str::to_owned)
        .collect();
    assert_eq!(exported, baseline);
}

/// The layout of `$type` as `abi/layout-1.txt` writes it.
macro_rules! layout {
    ($out:expr, $type:ty { $($field:ident),+ }) => {
        writeln!($out, "{} size {} align {}", stringify!($type), size_of::<$type>(), align_of::<$type>()).unwrap();
        $(writeln!($out, "  {} {}", stringify!($field), offset_of!($type, $field)).unwrap();)+
    };
}

#[test]
fn every_struct_has_the_layout_of_the_baseline() {
    let mut layouts = String::new();
    layout!(layouts, CkRuntime { bits });
    layout!(layouts, CkWaker { bits });
    layout!(layouts, CkWindow { bits });
    layout!(layouts, CkStr { ptr, len });
    layout!(
        layouts,
        CkRuntimeOptions {
            struct_size,
            abi_major,
            abi_minor,
            backend,
            app_id
        }
    );
    layout!(
        layouts,
        CkEvent {
            kind,
            reserved,
            window,
            message,
            value,
            payload_offset,
            payload_len
        }
    );
    assert_eq!(layouts, include_str!("../abi/layout-1.txt"));
}
