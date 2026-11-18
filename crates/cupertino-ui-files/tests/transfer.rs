//! Copies and moves in slices: a tree copied and moved slice by slice, a move across two file systems, a stop, a
//! source that changes while it is copied, and the results of the blocking functions that run the slices.

use std::fs;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};

use cupertino_ui_files::operations::{
    drop_into, duplicate, is_cancelled, same_volume, transfer, Cancel, DropAction, Progress,
    Transfer,
};

/// More than one chunk of a file's copy, a megabyte.
const BIG: usize = 3 << 20;

/// A tree under `root`: a big file, a small one in a folder below and a link to the big one. Its path.
fn tree(root: &Path) -> PathBuf {
    let tree = root.join("tree");
    fs::create_dir_all(tree.join("sub")).unwrap();
    fs::write(tree.join("big.bin"), vec![7u8; BIG]).unwrap();
    fs::write(tree.join("sub/small.txt"), b"small").unwrap();
    std::os::unix::fs::symlink("big.bin", tree.join("link")).unwrap();
    tree
}

/// Whether the tree at `path` is the one [`tree`] makes.
fn is_tree(path: &Path) -> bool {
    fs::read(path.join("big.bin")).is_ok_and(|b| b == vec![7u8; BIG])
        && fs::read(path.join("sub/small.txt")).is_ok_and(|b| b == b"small")
        && fs::read_link(path.join("link")).is_ok_and(|l| l == Path::new("big.bin"))
}

/// Run `transfer` slice by slice, checking that a slice out holds the next back and that the progress only grows;
/// its results.
fn slices(mut transfer: Transfer) -> Vec<std::io::Result<PathBuf>> {
    let mut last = transfer.progress();
    while let Some(slice) = transfer.take_slice() {
        assert!(transfer.take_slice().is_none(), "two slices out at once");
        assert!(transfer.results().is_none(), "results before the end");
        slice.run();
        let now = transfer.progress();
        assert!(
            now.bytes >= last.bytes && now.items >= last.items,
            "{now:?} after {last:?}"
        );
        last = now;
    }
    let done = transfer.progress();
    assert!(done.total_bytes > 0);
    assert_eq!(
        (done.bytes, done.items),
        (done.total_bytes, done.total_items)
    );
    transfer.results().expect("the results once it ended")
}

#[test]
fn a_tree_is_copied_moved_and_duplicated_slice_by_slice_with_its_progress() {
    type Make = fn(&Path, &Path) -> Transfer;
    let rows: [(&str, Make, &str, bool); 3] = [
        (
            "copy",
            |tree, dest| Transfer::into_folder(vec![tree.into()], dest.into(), DropAction::Copy),
            "dest/tree",
            true,
        ),
        (
            "move",
            |tree, dest| Transfer::into_folder(vec![tree.into()], dest.into(), DropAction::Move),
            "dest/tree",
            false,
        ),
        (
            "duplicate",
            |tree, _| Transfer::duplicate(vec![tree.into()]),
            "tree copy",
            true,
        ),
    ];
    for (row, make, arrives, stays) in rows {
        let root = tempfile::tempdir().unwrap();
        let tree = tree(root.path());
        let dest = root.path().join("dest");
        fs::create_dir(&dest).unwrap();
        let results = slices(make(&tree, &dest));
        let arrived = root.path().join(arrives);
        assert_eq!(results.len(), 1, "{row}");
        assert_eq!(results[0].as_ref().unwrap(), &arrived, "{row}");
        assert!(is_tree(&arrived), "{row}");
        assert_eq!(is_tree(&tree), stays, "{row}");
    }
}

/// A folder on another file system than the target directory's: memory, where the system has it.
fn elsewhere() -> tempfile::TempDir {
    tempfile::tempdir_in("/dev/shm").unwrap()
}

fn here() -> tempfile::TempDir {
    tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap()
}

#[test]
fn a_move_across_two_file_systems_copies_then_removes_the_originals() {
    let (from, to) = (elsewhere(), here());
    assert!(!same_volume(from.path(), to.path()).unwrap());
    let tree = tree(from.path());
    let results = slices(Transfer::into_folder(
        vec![tree.clone()],
        to.path().into(),
        DropAction::Move,
    ));
    assert_eq!(results[0].as_ref().unwrap(), &to.path().join("tree"));
    assert!(is_tree(&to.path().join("tree")));
    assert!(!tree.exists());
}

#[test]
fn a_stop_removes_the_item_being_copied_and_leaves_the_rest_where_they_are() {
    let root = tempfile::tempdir().unwrap();
    let big = root.path().join("big.bin");
    fs::write(&big, vec![1u8; BIG]).unwrap();
    let small = root.path().join("small.txt");
    fs::write(&small, b"small").unwrap();
    let dest = root.path().join("dest");
    fs::create_dir(&dest).unwrap();
    let sources = [big.clone(), small.clone()];

    // a stop once the first megabyte of the big file is copied
    let cancel = Cancel::default();
    let stop = cancel.clone();
    let mut stopping = |p: &Progress| {
        if p.bytes > 0 && p.current.as_deref() == Some("big.bin") {
            stop.cancel();
        }
    };
    let results = transfer(&sources, &dest, DropAction::Copy, &cancel, &mut stopping);
    assert!(results.iter().all(|r| r.as_ref().is_err_and(is_cancelled)));
    assert_eq!(fs::read_dir(&dest).unwrap().count(), 0);
    assert!(big.exists() && small.exists());

    // a transfer stopped before its first slice moves nothing
    let mut stopped = Transfer::into_folder(sources.to_vec(), dest.clone(), DropAction::Move);
    stopped.cancel();
    while let Some(slice) = stopped.take_slice() {
        slice.run();
    }
    let results = stopped.results().unwrap();
    assert!(results.iter().all(|r| r.as_ref().is_err_and(is_cancelled)));
    assert_eq!(fs::read_dir(&dest).unwrap().count(), 0);
    assert!(big.exists() && small.exists());
}

#[test]
fn a_source_that_changed_while_it_was_moved_across_file_systems_is_left_where_it_was() {
    let (from, to) = (elsewhere(), here());
    let source = from.path().join("growing.bin");
    fs::write(&source, vec![2u8; BIG]).unwrap();
    let mut grown = false;
    let mut grow = |p: &Progress| {
        if p.bytes > 0 && !grown {
            grown = true;
            let mut file = fs::OpenOptions::new().append(true).open(&source).unwrap();
            file.write_all(b"more").unwrap();
        }
    };
    let results = transfer(
        std::slice::from_ref(&source),
        to.path(),
        DropAction::Move,
        &Cancel::default(),
        &mut grow,
    );
    let error = results[0].as_ref().unwrap_err();
    assert!(
        error.to_string().contains("changed while it was copied"),
        "{error}"
    );
    assert_eq!(fs::metadata(&source).unwrap().len(), BIG as u64 + 4);
}

#[test]
fn the_blocking_functions_give_one_result_a_source_in_order() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path();
    let a = dir.join("a.txt");
    fs::write(&a, b"a").unwrap();
    let dest = dir.join("dest");
    fs::create_dir(&dest).unwrap();
    fs::write(dest.join("a.txt"), b"taken").unwrap();
    let missing = dir.join("missing.txt");

    let copied = drop_into(
        &[a.clone(), missing.clone(), dest.clone()],
        &dest,
        DropAction::Copy,
    );
    assert_eq!(copied[0].as_ref().unwrap(), &dest.join("a copy.txt"));
    assert_eq!(copied[1].as_ref().unwrap_err().kind(), ErrorKind::NotFound);
    assert_eq!(
        copied[2].as_ref().unwrap_err().kind(),
        ErrorKind::InvalidInput,
        "a folder into itself"
    );
    assert_eq!(fs::read(dest.join("a.txt")).unwrap(), b"taken");

    let moved = drop_into(std::slice::from_ref(&a), dir, DropAction::Move);
    assert_eq!(moved[0].as_ref().unwrap(), &a, "a move into its own folder");

    let duplicated = duplicate(&[a.clone(), a.clone()]);
    let names: Vec<PathBuf> = duplicated.into_iter().map(Result::unwrap).collect();
    assert_eq!(names, [dir.join("a copy.txt"), dir.join("a copy 2.txt")]);
}
