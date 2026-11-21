//! Looking at the trash of the mounted volumes, which may not answer, and what a look reports.

use crate::common;

use common::{put_item, Tree};
use cupertino_trash::{Look, Trash};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
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

fn own_trash(vol: &Path) -> PathBuf {
    vol.join(format!(".Trash-{}", common::uid()))
}

fn volume_with_item(t: &Tree, name: &str) -> PathBuf {
    let vol = t.path(name);
    put_item(
        &own_trash(&vol),
        "v.txt",
        "x",
        "docs/v.txt",
        "2026-01-01T00:00:00",
    );
    vol
}

fn wait_until(what: &str, done: impl Fn() -> bool) {
    let deadline = std::time::Instant::now() + LONG;
    while !done() {
        assert!(std::time::Instant::now() < deadline, "{what}");
        std::thread::yield_now();
    }
}

/// A look at the volumes of `trash` on a thread of the test's, which waits on the silent volume until it is released;
/// returned once it does.
fn look_out(trash: &Trash, calls: &AtomicUsize) -> JoinHandle<()> {
    let out = trash.clone();
    let looking = std::thread::spawn(move || {
        out.silent_volumes();
    });
    wait_until("the look never reached the volume", || {
        calls.load(Ordering::SeqCst) == 1
    });
    looking
}

#[test]
fn a_volume_that_does_not_answer_is_named_holds_nothing_and_is_not_asked_again_while_out() {
    let t = Tree::new();
    let silent = volume_with_item(&t, "silent");
    let trash = t.trash().with_mounts(vec![silent.clone()]);
    let (trash, release, calls) = with_silent_volume(trash, silent.clone());
    let out = look_out(&trash, &calls);
    assert_eq!(trash.silent_volumes(), std::slice::from_ref(&silent));
    assert_eq!(trash.silent_volumes(), std::slice::from_ref(&silent));
    assert!(trash.volume_trash_dirs().is_empty());
    assert!(!trash.volumes_hold_items());
    assert!(trash.entries().is_empty());
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    // the volume answers after all; the look that was out ends and gives the mount point back
    drop(release);
    out.join().unwrap();
    assert!(trash.silent_volumes().is_empty());
}

#[test]
fn the_volumes_after_a_silent_one_still_answer_and_list() {
    let t = Tree::new();
    let silent = volume_with_item(&t, "silent");
    let live = volume_with_item(&t, "live");
    let trash = t.trash().with_mounts(vec![silent.clone(), live.clone()]);
    let (trash, release, calls) = with_silent_volume(trash, silent.clone());
    let out = look_out(&trash, &calls);
    assert_eq!(trash.silent_volumes(), [silent]);
    assert_eq!(trash.volume_trash_dirs(), [own_trash(&live)]);
    assert!(trash.volumes_hold_items());
    assert_eq!(trash.volume_entries().len(), 1);
    assert_eq!(trash.list_volumes().unwrap().len(), 1);
    drop(release);
    out.join().unwrap();
}

#[test]
fn emptying_reports_a_silent_volume_by_its_mount_point_and_still_empties_the_home_trash() {
    let t = Tree::new();
    let silent = volume_with_item(&t, "silent");
    let trash = t.trash().with_mounts(vec![silent.clone()]);
    let (trash, release, calls) = with_silent_volume(trash, silent.clone());
    let out = look_out(&trash, &calls);
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
    assert!(own_trash(&silent).join("files/v.txt").exists());
    drop(release);
    out.join().unwrap();
}

#[test]
fn the_volumes_are_asked_whether_the_trash_holds_items_only_when_the_home_trash_is_empty() {
    let t = Tree::new();
    let live = volume_with_item(&t, "live");
    let trash = t.trash().with_mounts(vec![live.clone()]);
    let (counted, release, calls) = with_silent_volume(trash, PathBuf::from("/nowhere"));
    assert!(!counted.home_holds_items());
    assert!(counted.home_entries().is_empty());
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    counted.trash(&t.file("a.txt", "x")).unwrap();
    assert!(counted.home_holds_items());
    assert!(counted.holds_items());
    assert_eq!(counted.home_entries().len(), 1);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    drop(release);

    let only_volume = t.trash().with_mounts(vec![live]);
    std::fs::remove_dir_all(t.home_trash()).unwrap();
    assert!(only_volume.holds_items());
    assert!(only_volume.volumes_hold_items());
    let none = t.trash().with_mounts(vec![t.path("empty-volume")]);
    assert!(!none.holds_items());
}

#[test]
fn the_entries_and_the_list_are_the_home_trashs_then_the_volumes() {
    let t = Tree::new();
    let live = volume_with_item(&t, "live");
    let trash = t.trash().with_mounts(vec![live.clone()]);
    // entries include those without records
    std::fs::create_dir_all(t.home_trash().join("files")).unwrap();
    std::fs::write(t.home_trash().join("files/orphan"), "").unwrap();
    let own = own_trash(&live);
    assert_eq!(
        trash.entries(),
        [t.home_trash().join("files/orphan"), own.join("files/v.txt")]
    );
    assert_eq!(trash.volume_entries(), [own.join("files/v.txt")]);
    put_item(
        &t.home_trash(),
        "h.txt",
        "x",
        "/home/u/h.txt",
        "2026-01-02T00:00:00",
    );
    let listed: Vec<(String, PathBuf)> = trash
        .list_all()
        .unwrap()
        .into_iter()
        .map(|i| (i.name, i.original_path))
        .collect();
    assert_eq!(
        listed,
        [
            ("h.txt".into(), PathBuf::from("/home/u/h.txt")),
            ("v.txt".into(), live.join("docs/v.txt"))
        ]
    );
    assert_eq!(trash.list_volumes().unwrap().len(), 1);
    // a bad record on a volume is an error of the lists
    std::fs::write(own.join("info/bad.trashinfo"), "[Trash Info]\n").unwrap();
    assert!(trash.list_volumes().is_err());
    assert!(trash.list_all().is_err());
}

#[test]
fn a_look_answers_from_the_home_trash_at_once() {
    let t = Tree::new();
    let look = Look::new(t.trash());
    assert_eq!(look.trash().home_trash(), t.home_trash());
    assert!(look.next().is_none(), "nothing asked for a look");
    assert!(!look.looking());
    assert!(!look.volumes_held());
    assert!(!look.full());
    t.trash().trash(&t.file("a.txt", "x")).unwrap();
    assert!(look.full());
}

#[test]
fn a_look_says_whether_the_volumes_answer_changed_and_is_then_answered_from() {
    let t = Tree::new();
    let live = volume_with_item(&t, "live");
    let look = Look::new(t.trash().with_mounts(vec![live.clone()]));
    // nothing is known until the first look has come back
    assert!(!look.full());
    let first = look.next().expect("the look full asked for");
    assert!(look.looking());
    assert!(first(), "the volume holds an item");
    assert!(look.volumes_held());
    assert!(look.full());
    assert!(!look.next().unwrap()(), "the same answer");
    // the volume is emptied; the next look says so
    std::fs::remove_file(own_trash(&live).join("files/v.txt")).unwrap();
    assert!(look.volumes_hold());
    assert!(look.next().unwrap()());
    assert!(!look.volumes_held());
    assert!(!look.looking());
}

#[test]
fn what_an_emptying_knows_can_be_set_without_a_look() {
    let t = Tree::new();
    let look = Look::new(t.trash());
    for held in [true, false] {
        look.set_volumes_held(held);
        assert_eq!(look.volumes_held(), held);
        assert!(!look.looking());
        assert!(look.next().is_none());
    }
}

#[test]
fn a_look_at_a_silent_volume_is_not_handed_out_twice() {
    let t = Tree::new();
    let silent = volume_with_item(&t, "silent");
    let trash = t.trash().with_mounts(vec![silent.clone()]);
    let (trash, release, calls) = with_silent_volume(trash, silent);
    let look = Look::new(trash);
    look.look();
    let first = look.next().expect("the look asked for");
    let out = std::thread::spawn(first);
    wait_until("the look never reached the volume", || {
        calls.load(Ordering::SeqCst) == 1
    });
    look.look();
    assert!(look.next().is_none(), "a look is out");
    assert!(look.looking());
    drop(release);
    // the look ends once the volume answers; the one asked for meanwhile is handed out then
    assert!(out.join().unwrap(), "the volume holds an item");
    assert!(!look.looking());
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(look.next().is_some());
}
