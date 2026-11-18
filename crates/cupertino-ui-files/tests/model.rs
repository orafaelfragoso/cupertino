//! A folder's model: its work run as a program runs it, off the model's thread, and its watch followed on a thread
//! of the test's.

use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;
use std::time::{Duration, Instant};

use cupertino_ui_files::model::{DirectoryModel, ModelDelta, ModelOptions, ModelWatch};

/// How long a change may take to come through the watch.
const HEARD: Duration = Duration::from_secs(5);

/// A model of `dir` whose notify is heard on the receiver.
fn model(dir: &Path, opts: ModelOptions) -> (DirectoryModel, Receiver<()>) {
    let (tx, rx) = channel();
    let notify = Arc::new(move || {
        let _ = tx.send(());
    });
    (DirectoryModel::open(dir, opts, notify), rx)
}

fn options(watch: bool, batch_size: usize) -> ModelOptions {
    ModelOptions {
        watch,
        batch_size,
        ..ModelOptions::default()
    }
}

/// Follow `watch` on a thread of its own; the receiver hears when the future ends.
fn follow(watch: ModelWatch) -> Receiver<()> {
    let (ended, ends) = channel();
    std::thread::spawn(move || {
        futures_lite::future::block_on(watch.follow());
        let _ = ended.send(());
    });
    ends
}

/// Run the model's works, one at a time, until none waits; every delta they brought.
fn run_all(model: &mut DirectoryModel) -> Vec<ModelDelta> {
    let mut deltas = Vec::new();
    while let Some(work) = model.take_work() {
        work.run();
        deltas.extend(model.poll());
    }
    deltas
}

/// Hear the watch and run what it asks for until `done` holds of the model.
fn until(model: &mut DirectoryModel, woken: &Receiver<()>, done: impl Fn(&DirectoryModel) -> bool) {
    let start = Instant::now();
    while !done(model) {
        let left = HEARD.saturating_sub(start.elapsed());
        assert!(woken.recv_timeout(left).is_ok(), "the watch told nothing");
        model.poll();
        run_all(model);
    }
}

fn names(model: &DirectoryModel) -> Vec<&str> {
    model.entries().iter().map(|e| e.name.as_str()).collect()
}

#[test]
fn a_folder_opens_then_comes_in_batches_and_completes() {
    let dir = tempfile::tempdir().unwrap();
    for name in ["e", "d", "c", "b", "a"] {
        std::fs::write(dir.path().join(name), b"").unwrap();
    }
    std::fs::write(dir.path().join(".hidden"), b"").unwrap();
    let (mut model, _woken) = model(dir.path(), options(false, 2));
    let deltas = run_all(&mut model);
    assert_eq!(
        deltas,
        [
            ModelDelta::Opened,
            ModelDelta::Reloaded { count: 2 },
            ModelDelta::Reloaded { count: 4 },
            ModelDelta::Reloaded { count: 5 },
            ModelDelta::LoadComplete { count: 5 },
        ]
    );
    assert_eq!(names(&model), ["a", "b", "c", "d", "e"]);
    assert_eq!((model.opened(), model.is_loading()), (Some(true), false));
}

#[test]
fn a_folder_that_does_not_open_says_why_and_asks_for_no_more_work() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("file");
    std::fs::write(&file, b"").unwrap();
    let locked = dir.path().join("locked");
    std::fs::create_dir(&locked).unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
    // a user that reads any folder (root) has no folder it cannot read
    let lockable = std::fs::read_dir(&locked).is_err();
    for watch in [false, true] {
        for (row, folder, kind) in [
            (
                "missing",
                dir.path().join("missing"),
                io::ErrorKind::NotFound,
            ),
            ("a file", file.clone(), io::ErrorKind::NotADirectory),
            (
                "no read permission",
                locked.clone(),
                io::ErrorKind::PermissionDenied,
            ),
        ] {
            if kind == io::ErrorKind::PermissionDenied && !lockable {
                continue;
            }
            let (mut model, _woken) = model(&folder, options(watch, 256));
            let _watch = model.take_watch();
            let deltas = run_all(&mut model);
            assert!(
                matches!(deltas.as_slice(), [ModelDelta::OpenFailed { kind: k, .. }] if *k == kind),
                "{row}, watch {watch}: {deltas:?}"
            );
            assert_eq!(model.opened(), Some(false), "{row}, watch {watch}");
            assert!(model.take_work().is_none(), "{row}, watch {watch}");
        }
    }
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn the_watch_the_first_work_registers_brings_created_renamed_and_removed_items() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("kept"), b"").unwrap();
    let (mut model, woken) = model(dir.path(), options(true, 256));
    let watch = model.take_watch().expect("a watch");
    let _ended = follow(watch);
    run_all(&mut model);
    assert_eq!(names(&model), ["kept"]);

    std::fs::write(dir.path().join("made"), b"").unwrap();
    until(&mut model, &woken, |m| names(m) == ["kept", "made"]);
    std::fs::rename(dir.path().join("made"), dir.path().join("renamed")).unwrap();
    until(&mut model, &woken, |m| names(m) == ["kept", "renamed"]);
    std::fs::remove_file(dir.path().join("renamed")).unwrap();
    until(&mut model, &woken, |m| names(m) == ["kept"]);
}

#[test]
fn works_run_one_at_a_time_in_the_order_asked() {
    let dir = tempfile::tempdir().unwrap();
    let (mut model, woken) = model(dir.path(), options(true, 256));
    let _ended = follow(model.take_watch().expect("a watch"));
    let first = model.take_work().expect("the first work");
    assert!(
        model.take_work().is_none(),
        "a second work while the first is out"
    );
    first.run();
    model.poll();

    // a change's work, then another change asked for while it is out
    std::fs::write(dir.path().join("a"), b"").unwrap();
    let start = Instant::now();
    let read_a = loop {
        assert!(woken
            .recv_timeout(HEARD.saturating_sub(start.elapsed()))
            .is_ok());
        model.poll();
        if let Some(work) = model.take_work() {
            break work;
        }
    };
    while woken.try_recv().is_ok() {}
    std::fs::write(dir.path().join("b"), b"").unwrap();
    assert!(woken.recv_timeout(HEARD).is_ok(), "the watch told nothing");
    model.poll();
    assert!(
        model.take_work().is_none(),
        "the change waits while a work is out"
    );
    read_a.run();
    assert_eq!(model.poll(), [ModelDelta::Inserted { index: 0 }]);
    assert_eq!(names(&model), ["a"]);
    until(&mut model, &woken, |m| names(m) == ["a", "b"]);
}

#[test]
fn a_listing_whose_work_panics_leaves_the_model_failed() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("item"), b"").unwrap();
    // the work panics the first time it tells the model's consumer
    let armed = Arc::new(AtomicBool::new(true));
    let notify = Arc::new(move || {
        if armed.swap(false, Ordering::SeqCst) {
            panic!("the consumer is not there");
        }
    });
    let mut model = DirectoryModel::open(dir.path(), options(false, 256), notify);
    let deltas = run_all(&mut model);
    assert!(
        matches!(
            deltas.last(),
            Some(ModelDelta::OpenFailed {
                kind: io::ErrorKind::Other,
                ..
            })
        ),
        "{deltas:?}"
    );
    assert_eq!(model.opened(), Some(false));
    assert!(!model.is_loading());
    assert!(!model.is_working());
    assert!(model.take_work().is_none());
}

#[test]
fn the_watch_ends_once_its_model_is_dropped_with_no_event() {
    let dir = tempfile::tempdir().unwrap();
    let (mut model, _woken) = model(dir.path(), options(true, 256));
    let ended = follow(model.take_watch().expect("a watch"));
    run_all(&mut model);
    assert!(ended.recv_timeout(Duration::from_millis(100)).is_err());
    drop(model);
    assert!(ended.recv_timeout(HEARD).is_ok(), "the watch went on");
}
