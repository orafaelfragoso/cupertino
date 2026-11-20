//! The launch history (`frecency::Frecency`): a copy with changes is handed over once to be written, and what is written
//! reads back.

#![expect(
    clippy::disallowed_methods,
    reason = "the test makes and removes its own files; it has no UI thread"
)]

use std::path::PathBuf;

use cupertino_launcher::Frecency;

const NOW: f64 = 1_794_650_400.0;

#[test]
fn unsaved_hands_over_a_changed_store_once() {
    let mut store = Frecency::in_memory();
    assert!(store.unsaved().is_none(), "nothing changed");
    store.record("app:org.example.A.desktop", "a", NOW);
    let copy = store.unsaved().expect("a launch is a change");
    assert_eq!(copy.launches("app:org.example.A.desktop"), 1);
    assert!(copy.is_dirty(), "the copy has the changes to write");
    assert!(!store.is_dirty());
    assert!(store.unsaved().is_none(), "handed over once");
}

#[test]
fn a_saved_store_reads_back() {
    let path = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("launcher-frecency-{}", std::process::id()))
        .join("launcher.json");
    let _ = std::fs::remove_file(&path);
    let mut store = Frecency::load(&path);
    assert!(store.is_empty(), "a missing file starts empty");
    store.record("app:org.example.B.desktop", "b", NOW);
    let mut copy = store.unsaved().unwrap();
    copy.save().expect("the store is written");
    let read = Frecency::load(&path);
    assert_eq!(read.launches("app:org.example.B.desktop"), 1);
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}
