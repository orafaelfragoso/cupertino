//! The size and alignment of every struct of the ABI and the offset of every field equal `abi/layout-1.txt`, which C
//! programs `_Static_assert` against. `CUPERTINO_UPDATE_GOLDENS=1` rewrites it.

use std::fmt::Write;
use std::mem::{align_of, offset_of, size_of};
use std::path::Path;

use cupertinokit::{CkEvent, CkRuntime, CkRuntimeOptions, CkStr, CkWaker, CkWindow};

macro_rules! layout {
    ($out:ident, $ty:ident { $($field:ident),* }) => {
        writeln!($out, "{} size {} align {}", stringify!($ty), size_of::<$ty>(), align_of::<$ty>()).unwrap();
        $(writeln!($out, "  {} {}", stringify!($field), offset_of!($ty, $field)).unwrap();)*
    };
}

#[test]
fn struct_layouts_are_pinned() {
    let mut out = String::new();
    layout!(out, CkRuntime { bits });
    layout!(out, CkWaker { bits });
    layout!(out, CkWindow { bits });
    layout!(out, CkStr { ptr, len });
    layout!(
        out,
        CkRuntimeOptions {
            struct_size,
            abi_major,
            abi_minor,
            backend,
            app_id
        }
    );
    layout!(
        out,
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

    let golden = Path::new(env!("CARGO_MANIFEST_DIR")).join("abi/layout-1.txt");
    if std::env::var("CUPERTINO_UPDATE_GOLDENS").as_deref() == Ok("1") {
        std::fs::write(&golden, &out).unwrap();
    }
    assert_eq!(
        out,
        std::fs::read_to_string(&golden).unwrap(),
        "a struct of ABI 1 changed layout"
    );
}
