//! The thumbnails of the items in view: which are made first, how many at once, what stays in memory, and what a
//! failure leaves.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use cupertino_icons::thumbnail::Thumbnailer;
use cupertino_ui_files::thumbnail_service::{Stamp, ThumbnailService};
use tiny_skia::Pixmap;

/// The edge the window asks for, in device pixels.
const EDGE: u32 = 64;
/// The sample pictures' size in pixels: smaller than the edge, so a thumbnail keeps it.
const PICTURE: (u32, u32) = (48, 32);

/// A service with its cache in `dir` keeping `budget` bytes, taking its ids from `ids`.
fn service(dir: &Path, budget: usize, ids: Arc<dyn Fn() -> u64 + Send + Sync>) -> ThumbnailService {
    ThumbnailService::new(
        Thumbnailer::new(dir.join("cache"), "test"),
        budget,
        Arc::new(|| {}),
        ids,
    )
}

fn counted() -> Arc<dyn Fn() -> u64 + Send + Sync> {
    let next = AtomicU64::new(1);
    Arc::new(move || next.fetch_add(1, Ordering::Relaxed))
}

/// `count` PNG pictures in `dir`, each a colour of its own.
fn pictures(dir: &Path, count: usize) -> Vec<PathBuf> {
    (0..count)
        .map(|n| {
            let path = dir.join(format!("picture-{n}.png"));
            let mut pixmap = Pixmap::new(PICTURE.0, PICTURE.1).unwrap();
            pixmap.fill(tiny_skia::Color::from_rgba8(40 * n as u8, 120, 200, 255));
            pixmap.save_png(&path).unwrap();
            path
        })
        .collect()
}

fn stamp(size: u64) -> Stamp {
    Stamp {
        modified: Some(SystemTime::UNIX_EPOCH + Duration::from_secs(size)),
        size,
    }
}

fn wanted(paths: &[PathBuf]) -> Vec<(PathBuf, Stamp)> {
    paths.iter().map(|p| (p.clone(), stamp(1))).collect()
}

#[test]
fn the_items_wanted_first_are_made_first_two_at_once() {
    let dir = tempfile::tempdir().unwrap();
    let p = pictures(dir.path(), 4);
    let thumbnails = service(dir.path(), usize::MAX, counted());
    thumbnails.want(1, EDGE, wanted(&p));
    let first = thumbnails.take_job().expect("a job");
    assert_eq!(thumbnails.queued(), &p[1..]);
    let second = thumbnails.take_job().expect("a second job");
    assert!(thumbnails.take_job().is_none(), "a third job at once");
    // the window scrolls: the last items are wanted first, and the first two are being made
    thumbnails.want(
        1,
        EDGE,
        wanted(&[p[3].clone(), p[2].clone(), p[1].clone(), p[0].clone()]),
    );
    assert_eq!(thumbnails.queued(), [p[3].clone(), p[2].clone()]);
    first.run();
    assert!(thumbnails.get(&p[0], EDGE).is_some());
    let _third = thumbnails.take_job().expect("a job once one ended");
    assert_eq!(thumbnails.queued(), [p[2].clone()]);
    // what no window wants any more is not made
    thumbnails.want(1, EDGE, wanted(&p[..2]));
    assert!(thumbnails.queued().is_empty());
    second.run();
    assert!(thumbnails.get(&p[1], EDGE).is_some());
}

#[test]
fn past_the_budget_only_the_thumbnails_in_view_stay() {
    let dir = tempfile::tempdir().unwrap();
    let p = pictures(dir.path(), 2);
    let one = (PICTURE.0 * PICTURE.1 * 4) as usize;
    let thumbnails = service(dir.path(), one, counted());
    let make = |paths: &[PathBuf]| {
        thumbnails.want(1, EDGE, wanted(paths));
        while let Some(job) = thumbnails.take_job() {
            job.run();
        }
    };
    make(&p[..1]);
    assert_eq!(thumbnails.memory(), one);
    // the window shows the other one alone: the first goes
    make(&p[1..]);
    assert!(thumbnails.get(&p[0], EDGE).is_none());
    assert!(thumbnails.get(&p[1], EDGE).is_some());
    assert_eq!(thumbnails.memory(), one);
    // both in view stay, past the budget
    make(&p);
    assert!(thumbnails.get(&p[0], EDGE).is_some());
    assert!(thumbnails.get(&p[1], EDGE).is_some());
    assert_eq!(thumbnails.memory(), 2 * one);
}

#[test]
fn a_file_that_cannot_be_thumbnailed_is_not_tried_again_until_it_changes() {
    let dir = tempfile::tempdir().unwrap();
    let bad = dir.path().join("not-a-picture.png");
    std::fs::write(&bad, b"plain text").unwrap();
    let thumbnails = service(dir.path(), usize::MAX, counted());
    thumbnails.want(1, EDGE, [(bad.clone(), stamp(1))]);
    thumbnails.take_job().expect("a job").run();
    assert!(thumbnails.failed(&bad, EDGE));
    assert!(thumbnails.get(&bad, EDGE).is_none());
    thumbnails.want(1, EDGE, [(bad.clone(), stamp(1))]);
    assert!(thumbnails.take_job().is_none(), "tried again unchanged");
    thumbnails.want(1, EDGE, [(bad.clone(), stamp(2))]);
    assert!(
        thumbnails.take_job().is_some(),
        "not tried again once it changed"
    );
}

#[test]
fn a_panic_while_a_thumbnail_is_made_is_kept_as_its_files_failure() {
    let dir = tempfile::tempdir().unwrap();
    let p = pictures(dir.path(), 1);
    let thumbnails = service(
        dir.path(),
        usize::MAX,
        Arc::new(|| panic!("no id for this thumbnail")),
    );
    thumbnails.want(1, EDGE, wanted(&p));
    thumbnails.take_job().expect("a job").run();
    assert!(thumbnails.failed(&p[0], EDGE));
    assert!(thumbnails.get(&p[0], EDGE).is_none());
}
