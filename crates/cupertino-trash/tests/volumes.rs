//! Looking at the trash of the mounted volumes, which may not answer, and what a look reports.

mod common;

use common::{put_item, Tree};
use cupertino_trash::{Look, Trash};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const SHORT: Duration = Duration::from_millis(150);
const LONG: Duration = Duration::from_secs(20);

/// A trash whose volume `silent` does not answer until the returned sender is dropped; `calls` counts how often a
/// volume of any name was looked at.
fn with_silent_volume(trash: Trash, silent: PathBuf) -> (Trash, Sender<()>, Arc<AtomicUsize>) {
    let (release, blocked) = mpsc::channel::<()>();
    let blocked: Mutex<Receiver<()>> = Mutex::new(blocked);
    let calls = Arc::new(AtomicUsize::new(0));
    let counted = calls.clone();
    let trash = trash
        .with_volume_wait(SHORT)
        .with_volume_look(move |top: &Path| {
            counted.fetch_add(1, Ordering::SeqCst);
            if top == silent {
                let _ = blocked.lock().unwrap().recv();
            }
        });
    (trash, release, calls)
}

fn volume_with_item(t: &Tree, name: &str) -> PathBuf {
    let vol = t.path(name);
    let own = vol.join(format!(".Trash-{}", common::uid()));
    put_item(&own, "v.txt", "x", "docs/v.txt", "2026-01-01T00:00:00");
    vol
}

#[test]
fn a_volume_that_does_not_answer_is_named_and_counts_as_holding_nothing() {
    let t = Tree::new();
    let silent = volume_with_item(&t, "silent");
    let trash = t.trash().with_mounts(vec![silent.clone()]);
    let (trash, release, _) = with_silent_volume(trash, silent.clone());
    assert_eq!(trash.silent_volumes(), [silent]);
    assert!(trash.volume_trash_dirs().is_empty());
    assert!(!trash.volumes_hold_items());
    drop(release);
}

#[test]
fn a_volume_that_does_not_answer_is_not_looked_at_again_while_the_first_look_is_out() {
    let t = Tree::new();
    let silent = volume_with_item(&t, "silent");
    let trash = t.trash().with_mounts(vec![silent.clone()]);
    let (trash, release, calls) = with_silent_volume(trash, silent.clone());
    assert_eq!(trash.silent_volumes(), std::slice::from_ref(&silent));
    assert_eq!(trash.silent_volumes(), [silent]);
    assert!(trash.entries().is_empty());
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    drop(release);
}

#[test]
fn the_volumes_after_a_silent_one_still_answer() {
    let t = Tree::new();
    let silent = volume_with_item(&t, "silent");
    let live = volume_with_item(&t, "live");
    let trash = t.trash().with_mounts(vec![silent.clone(), live.clone()]);
    let (trash, release, _) = with_silent_volume(trash, silent.clone());
    assert_eq!(trash.silent_volumes(), [silent]);
    assert_eq!(
        trash.volume_trash_dirs(),
        [live.join(format!(".Trash-{}", common::uid()))]
    );
    assert!(trash.volumes_hold_items());
    assert_eq!(trash.volume_entries().len(), 1);
    drop(release);
}

#[test]
fn a_volume_that_answers_late_is_looked_at_again_afterwards() {
    let t = Tree::new();
    let slow = volume_with_item(&t, "slow");
    let trash = t.trash().with_mounts(vec![slow.clone()]);
    let (trash, release, _) = with_silent_volume(trash, slow.clone());
    assert_eq!(trash.silent_volumes(), [slow]);
    // the volume answers after all; the look that was out ends and gives the mount point back
    drop(release);
    let deadline = std::time::Instant::now() + LONG;
    while !trash.silent_volumes().is_empty() {
        assert!(
            std::time::Instant::now() < deadline,
            "the volume never came back"
        );
        std::thread::yield_now();
    }
}

#[test]
fn emptying_reports_a_silent_volume_by_its_mount_point_and_still_empties_the_home_trash() {
    let t = Tree::new();
    let silent = volume_with_item(&t, "silent");
    let trash = t.trash().with_mounts(vec![silent.clone()]);
    let (trash, release, _) = with_silent_volume(trash, silent.clone());
    let home = trash.trash(&t.file("a.txt", "x")).unwrap();
    let emptied = trash.empty();
    assert_eq!(emptied.deleted, 1);
    assert_eq!(emptied.failures.len(), 1);
    assert_eq!(emptied.failures[0].path, silent);
    assert_eq!(
        emptied.failures[0].error.kind(),
        std::io::ErrorKind::TimedOut
    );
    assert!(!home.file_path().exists());
    assert!(silent
        .join(format!(".Trash-{}", common::uid()))
        .join("files/v.txt")
        .exists());
    drop(release);
}

#[test]
fn the_home_trash_is_checked_without_asking_a_volume() {
    let t = Tree::new();
    let live = volume_with_item(&t, "live");
    let trash = t.trash().with_mounts(vec![live.clone()]);
    let (trash, release, calls) = with_silent_volume(trash, PathBuf::from("/nowhere"));
    assert!(!trash.home_holds_items());
    assert!(trash.home_entries().is_empty());
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    trash.trash(&t.file("a.txt", "x")).unwrap();
    assert!(trash.home_holds_items());
    assert!(trash.holds_items());
    assert_eq!(trash.home_entries().len(), 1);
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "the volumes are asked only when the home trash is empty"
    );
    drop(release);
}

#[test]
fn an_empty_home_trash_asks_the_volumes() {
    let t = Tree::new();
    let live = volume_with_item(&t, "live");
    let trash = t.trash().with_mounts(vec![live]);
    assert!(trash.holds_items());
    assert!(trash.volumes_hold_items());
    let none = t.trash().with_mounts(vec![t.path("empty-volume")]);
    assert!(!none.holds_items());
}

#[test]
fn the_entries_are_the_home_trashs_then_the_volumes_and_include_those_without_records() {
    let t = Tree::new();
    let live = volume_with_item(&t, "live");
    let trash = t.trash().with_mounts(vec![live.clone()]);
    std::fs::create_dir_all(t.home_trash().join("files")).unwrap();
    std::fs::write(t.home_trash().join("files/orphan"), "").unwrap();
    let own = live.join(format!(".Trash-{}", common::uid()));
    assert_eq!(
        trash.entries(),
        [t.home_trash().join("files/orphan"), own.join("files/v.txt")]
    );
    assert_eq!(trash.volume_entries(), [own.join("files/v.txt")]);
}

#[test]
fn the_list_of_everything_has_the_home_trashs_items_then_the_volumes() {
    let t = Tree::new();
    let live = volume_with_item(&t, "live");
    let trash = t.trash().with_mounts(vec![live.clone()]);
    put_item(
        &t.home_trash(),
        "h.txt",
        "x",
        "/home/u/h.txt",
        "2026-01-02T00:00:00",
    );
    let items = trash.list_all().unwrap();
    let listed: Vec<(&str, PathBuf)> = items
        .iter()
        .map(|i| (i.name.as_str(), i.original_path.clone()))
        .collect();
    assert_eq!(
        listed,
        [
            ("h.txt", PathBuf::from("/home/u/h.txt")),
            ("v.txt", live.join("docs/v.txt"))
        ]
    );
    assert_eq!(trash.list_volumes().unwrap().len(), 1);
}

#[test]
fn a_bad_record_on_a_volume_is_an_error_of_the_list() {
    let t = Tree::new();
    let live = volume_with_item(&t, "live");
    let own = live.join(format!(".Trash-{}", common::uid()));
    std::fs::write(own.join("info/bad.trashinfo"), "[Trash Info]\n").unwrap();
    let trash = t.trash().with_mounts(vec![live]);
    assert!(trash.list_volumes().is_err());
    assert!(trash.list_all().is_err());
}

#[test]
fn a_silent_volume_leaves_the_list_of_the_others() {
    let t = Tree::new();
    let silent = volume_with_item(&t, "silent");
    let live = volume_with_item(&t, "live");
    let trash = t.trash().with_mounts(vec![silent.clone(), live]);
    let (trash, release, _) = with_silent_volume(trash, silent);
    assert_eq!(trash.list_volumes().unwrap().len(), 1);
    drop(release);
}

fn watched(trash: Trash) -> (Arc<Look>, Receiver<()>) {
    let (changed, rx) = mpsc::channel();
    let changed = Mutex::new(changed);
    let look = Look::new(trash, move || {
        let _ = changed.lock().unwrap().send(());
    });
    (look, rx)
}

#[test]
fn a_look_has_the_trash_it_looks_at() {
    let t = Tree::new();
    let (look, _) = watched(t.trash());
    assert_eq!(look.trash().home_trash(), t.home_trash());
    assert!(!look.looking());
    assert!(!look.volumes_held());
}

#[test]
fn a_look_answers_from_the_home_trash_at_once() {
    let t = Tree::new();
    let (look, _) = watched(t.trash());
    assert!(!look.full());
    t.trash().trash(&t.file("a.txt", "x")).unwrap();
    assert!(look.full());
}

#[test]
fn a_look_reports_a_change_of_the_volumes_answer_and_then_answers_from_it() {
    let t = Tree::new();
    let live = volume_with_item(&t, "live");
    let (look, changed) = watched(t.trash().with_mounts(vec![live.clone()]));
    // nothing is known until the first look has come back
    assert!(!look.full());
    changed.recv_timeout(LONG).unwrap();
    assert!(look.volumes_held());
    // the volume is emptied; the next look says so
    std::fs::remove_file(
        live.join(format!(".Trash-{}", common::uid()))
            .join("files/v.txt"),
    )
    .unwrap();
    look.look();
    changed.recv_timeout(LONG).unwrap();
    assert!(!look.volumes_held());
    assert!(!look.looking());
}

#[test]
fn what_an_emptying_knows_can_be_set_without_a_look() {
    let t = Tree::new();
    let (look, _) = watched(t.trash());
    look.set_volumes_held(true);
    assert!(look.volumes_held());
    assert!(!look.looking());
    look.set_volumes_held(false);
    assert!(!look.volumes_held());
}

#[test]
fn a_look_at_a_silent_volume_is_not_started_twice() {
    let t = Tree::new();
    let silent = volume_with_item(&t, "silent");
    let trash = t.trash().with_mounts(vec![silent.clone()]);
    let (trash, release, calls) = with_silent_volume(trash, silent);
    let (look, _) = watched(trash);
    look.look();
    look.look();
    assert!(look.looking());
    drop(release);
    // the look ends once the volume answers; a second look can then be started
    let deadline = std::time::Instant::now() + LONG;
    while look.looking() {
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}
