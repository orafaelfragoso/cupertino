//! The catalogue of applications, the burst timing and (feature `watch`) following the folders.

mod common;

use common::{desktop, vis, write};
use cupertino_apps::catalogue::{Burst, Catalogue, Timing};
use cupertino_apps::entry::Locale;
use std::time::{Duration, Instant};

fn ids(catalogue: &Catalogue) -> Vec<&str> {
    catalogue.entries().iter().map(|e| e.id.as_str()).collect()
}

#[test]
fn a_catalogue_lists_the_applications_of_its_folders() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "b.desktop", &desktop("B", "b", ""));
    write(dir.path(), "a.desktop", &desktop("A", "a", ""));
    write(
        dir.path(),
        "cli.desktop",
        &desktop("Cli", "cli", "Terminal=true"),
    );
    let catalogue = Catalogue::new(vec![dir.path().to_path_buf()], Locale::default(), vis());
    assert_eq!(ids(&catalogue), ["a.desktop", "b.desktop"]);
    assert_eq!(catalogue.dirs(), [dir.path().to_path_buf()]);
}

#[test]
fn rereading_says_whether_the_applications_changed() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "a.desktop", &desktop("A", "a", ""));
    let mut catalogue = Catalogue::new(vec![dir.path().to_path_buf()], Locale::default(), vis());
    assert!(!catalogue.reread());
    write(dir.path(), "b.desktop", &desktop("B", "b", ""));
    assert!(catalogue.reread());
    assert_eq!(ids(&catalogue), ["a.desktop", "b.desktop"]);
    assert!(!catalogue.reread());
    std::fs::remove_file(dir.path().join("a.desktop")).unwrap();
    assert!(catalogue.reread());
    assert_eq!(ids(&catalogue), ["b.desktop"]);
}

#[test]
fn rereading_sees_an_edited_entry() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "a.desktop", &desktop("Before", "a", ""));
    let mut catalogue = Catalogue::new(vec![dir.path().to_path_buf()], Locale::default(), vis());
    write(dir.path(), "a.desktop", &desktop("After", "a", ""));
    assert!(catalogue.reread());
    assert_eq!(catalogue.entries()[0].name, "After");
}

#[test]
fn rereading_keeps_the_precedence_of_the_folders() {
    let dir = tempfile::tempdir().unwrap();
    let (first, second) = (dir.path().join("one"), dir.path().join("two"));
    write(&second, "a.desktop", &desktop("Second", "a", ""));
    let mut catalogue = Catalogue::new(vec![first.clone(), second], Locale::default(), vis());
    assert_eq!(catalogue.entries()[0].name, "Second");
    write(&first, "a.desktop", &desktop("First", "a", ""));
    assert!(catalogue.reread());
    assert_eq!(catalogue.entries()[0].name, "First");
}

#[test]
fn the_default_timing_reads_once_quiet_for_250_ms_and_at_the_latest_after_2_s() {
    let timing = Timing::default();
    assert_eq!(timing.quiet, Duration::from_millis(250));
    assert_eq!(timing.longest, Duration::from_secs(2));
}

#[test]
fn a_burst_that_has_not_started_has_nothing_left() {
    let burst = Burst::new(Timing::default());
    assert_eq!(burst.left(Instant::now()), None);
}

#[test]
fn a_burst_ends_once_quiet_for_the_quiet_time() {
    let start = Instant::now();
    let mut burst = Burst::new(Timing::default());
    burst.touch(start);
    assert_eq!(burst.left(start), Some(Duration::from_millis(250)));
    assert_eq!(
        burst.left(start + Duration::from_millis(100)),
        Some(Duration::from_millis(150))
    );
    assert_eq!(
        burst.left(start + Duration::from_millis(300)),
        Some(Duration::ZERO)
    );
}

#[test]
fn every_event_of_a_burst_restarts_the_quiet_time() {
    let start = Instant::now();
    let mut burst = Burst::new(Timing::default());
    burst.touch(start);
    burst.touch(start + Duration::from_millis(200));
    assert_eq!(
        burst.left(start + Duration::from_millis(200)),
        Some(Duration::from_millis(250))
    );
}

#[test]
fn a_burst_that_never_goes_quiet_ends_at_the_longest_time() {
    let start = Instant::now();
    let mut burst = Burst::new(Timing::default());
    burst.touch(start);
    burst.touch(start + Duration::from_millis(1900));
    assert_eq!(
        burst.left(start + Duration::from_millis(1900)),
        Some(Duration::from_millis(100))
    );
    assert_eq!(
        burst.left(start + Duration::from_secs(3)),
        Some(Duration::ZERO)
    );
}

#[test]
fn ending_a_burst_clears_it() {
    let start = Instant::now();
    let mut burst = Burst::new(Timing::default());
    burst.touch(start);
    burst.end();
    assert_eq!(burst.left(start), None);
    burst.touch(start + Duration::from_secs(10));
    assert_eq!(
        burst.left(start + Duration::from_secs(10)),
        Some(Duration::from_millis(250))
    );
}

#[cfg(feature = "watch")]
mod watch {
    use super::*;
    use cupertino_apps::catalogue::{ApplicationWatch, Change};
    use std::sync::mpsc;

    const WAIT: Duration = Duration::from_secs(20);

    fn fast() -> Timing {
        Timing {
            quiet: Duration::from_millis(50),
            longest: Duration::from_millis(500),
        }
    }

    /// The next list of applications the watch reports, which must name `id` when `present`.
    fn until(changes: &mpsc::Receiver<Change>, id: &str, present: bool) -> Vec<String> {
        let deadline = Instant::now() + WAIT;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match changes.recv_timeout(left) {
                Ok(Change::Applications(list)) => {
                    let names: Vec<String> = list.into_iter().map(|e| e.id).collect();
                    if names.iter().any(|n| n == id) == present {
                        return names;
                    }
                }
                Ok(Change::Error(e)) => panic!("{e}"),
                Err(_) => panic!("no report with {id} present={present}"),
            }
        }
    }

    #[test]
    fn a_new_application_is_reported_with_all_of_them() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "a.desktop", &desktop("A", "a", ""));
        let catalogue = Catalogue::new(vec![dir.path().to_path_buf()], Locale::default(), vis());
        let (send, changes) = mpsc::channel();
        let _watch = ApplicationWatch::start(catalogue, fast(), move |c| {
            let _ = send.send(c);
        })
        .unwrap();
        write(dir.path(), "b.desktop", &desktop("B", "b", ""));
        assert_eq!(
            until(&changes, "b.desktop", true),
            ["a.desktop", "b.desktop"]
        );
        std::fs::remove_file(dir.path().join("a.desktop")).unwrap();
        assert_eq!(until(&changes, "a.desktop", false), ["b.desktop"]);
    }

    #[test]
    fn a_folder_made_later_is_followed() {
        let dir = tempfile::tempdir().unwrap();
        let apps = dir.path().join("share/applications");
        let catalogue = Catalogue::new(vec![apps.clone()], Locale::default(), vis());
        let (send, changes) = mpsc::channel();
        let _watch = ApplicationWatch::start(catalogue, fast(), move |c| {
            let _ = send.send(c);
        })
        .unwrap();
        write(&apps, "late.desktop", &desktop("Late", "late", ""));
        assert_eq!(until(&changes, "late.desktop", true), ["late.desktop"]);
    }
}
