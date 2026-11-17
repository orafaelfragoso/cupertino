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
fn rereading_says_whether_the_applications_changed_and_keeps_folder_precedence() {
    let dir = tempfile::tempdir().unwrap();
    let (first, second) = (dir.path().join("one"), dir.path().join("two"));
    write(&second, "a.desktop", &desktop("Second", "a", ""));
    let mut catalogue = Catalogue::new(
        vec![first.clone(), second.clone()],
        Locale::default(),
        vis(),
    );
    assert!(!catalogue.reread());

    write(&second, "b.desktop", &desktop("B", "b", ""));
    assert!(catalogue.reread(), "added");
    assert_eq!(ids(&catalogue), ["a.desktop", "b.desktop"]);
    assert!(!catalogue.reread(), "unchanged");

    write(&second, "b.desktop", &desktop("B edited", "b", ""));
    assert!(catalogue.reread(), "edited");
    assert_eq!(catalogue.entries()[1].name, "B edited");

    write(&first, "a.desktop", &desktop("First", "a", ""));
    assert!(catalogue.reread(), "shadowed");
    assert_eq!(catalogue.entries()[0].name, "First");

    std::fs::remove_file(second.join("b.desktop")).unwrap();
    assert!(catalogue.reread(), "removed");
    assert_eq!(ids(&catalogue), ["a.desktop"]);
}

#[test]
fn a_burst_ends_once_quiet_or_at_the_longest_time() {
    let start = Instant::now();
    let ms = Duration::from_millis;
    let timing = Timing::default();
    // (touches after start, asked at, time left)
    for (touches, at, left) in [
        (&[][..], ms(0), None),
        (&[0][..], ms(0), Some(timing.quiet)),
        (&[0], ms(100), Some(timing.quiet - ms(100))),
        (&[0], timing.quiet + ms(50), Some(Duration::ZERO)),
        (&[0, 200], ms(200), Some(timing.quiet)),
        (&[0, 1900], ms(1900), Some(timing.longest - ms(1900))),
        (&[0, 1900], timing.longest + ms(1000), Some(Duration::ZERO)),
    ] {
        let mut burst = Burst::new(timing);
        for t in touches {
            burst.touch(start + ms(*t));
        }
        assert_eq!(
            burst.left(start + at),
            left,
            "touches {touches:?} at {at:?}"
        );
    }
}

#[test]
fn ending_a_burst_clears_it_for_the_next_one() {
    let start = Instant::now();
    let mut burst = Burst::new(Timing::default());
    burst.touch(start);
    burst.end();
    assert_eq!(burst.left(start), None);
    let later = start + Duration::from_secs(10);
    burst.touch(later);
    assert_eq!(burst.left(later), Some(Timing::default().quiet));
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
    fn changes_are_reported_with_every_application_including_a_folder_made_later() {
        let dir = tempfile::tempdir().unwrap();
        let existing = dir.path().join("existing");
        let late = dir.path().join("share/applications");
        write(&existing, "a.desktop", &desktop("A", "a", ""));
        let catalogue = Catalogue::new(
            vec![existing.clone(), late.clone()],
            Locale::default(),
            vis(),
        );
        let (send, changes) = mpsc::channel();
        let mut watch = ApplicationWatch::new(catalogue, fast()).unwrap();
        // the program's loop: the folders are read, then the next burst is waited out
        assert!(watch.reread().is_empty(), "nothing changed yet");
        std::thread::spawn(move || loop {
            async_io::block_on(watch.quiet()).unwrap();
            for change in watch.reread() {
                if send.send(change).is_err() {
                    return;
                }
            }
        });
        write(&existing, "b.desktop", &desktop("B", "b", ""));
        assert_eq!(
            until(&changes, "b.desktop", true),
            ["a.desktop", "b.desktop"]
        );
        std::fs::remove_file(existing.join("a.desktop")).unwrap();
        assert_eq!(until(&changes, "a.desktop", false), ["b.desktop"]);
        write(&late, "late.desktop", &desktop("Late", "late", ""));
        assert_eq!(
            until(&changes, "late.desktop", true),
            ["b.desktop", "late.desktop"]
        );
    }
}
