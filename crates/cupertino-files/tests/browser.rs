//! A browser window's folder requests: a folder asked for is shown once its model opens, one that does not open
//! leaves the window where it was, and a folder asked for again while its work runs takes the model it had.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use cupertino_files::browser::{Asked, Browser, Listings, Polled};
use cupertino_kit::files::model::{ModelWork, Notify};
use cupertino_kit::files::sort::SortSpec;

fn browser(start: &Path, listings: &Listings) -> Browser {
    woken_browser(start, listings, Arc::new(|| {}))
}

/// A browser that calls `notify` whenever there is something to poll.
fn woken_browser(start: &Path, listings: &Listings, notify: Notify) -> Browser {
    Browser::open(
        start.to_path_buf(),
        SortSpec::default(),
        start.join("recently-used.xbel"),
        None,
        notify,
        listings.clone(),
    )
    .unwrap()
}

/// The works of `browser`'s models, as the program takes them; the watches are not followed.
fn take(browser: &mut Browser) -> Vec<ModelWork> {
    browser.take_work().0
}

/// Run the browser's works until none is left, polling after each; what the polls brought together.
fn settle(browser: &mut Browser) -> Polled {
    let mut all = Polled::default();
    loop {
        let works = take(browser);
        if works.is_empty() {
            return all;
        }
        for work in works {
            work.run();
        }
        let polled = browser.poll();
        all.changed |= polled.changed;
        all.errors.extend(polled.errors);
        all.entered = all.entered.or(polled.entered);
        all.unopened = all.unopened.or(polled.unopened);
    }
}

fn names(browser: &Browser) -> Vec<String> {
    browser.entries().iter().map(|e| e.name.clone()).collect()
}

#[test]
fn a_folder_that_does_not_open_leaves_the_listing_and_the_history_and_says_so() {
    type Step = fn(&mut Browser, &Path);
    type Request = fn(&mut Browser, &Path) -> PathBuf;
    let rows: [(&str, Step, Request, Asked); 4] = [
        (
            "go",
            |_, _| {},
            |browser, root| {
                assert!(!browser.go(root.join("missing"), None));
                root.join("missing")
            },
            Asked::Go,
        ),
        (
            "back",
            |browser, root| {
                browser.go(root.join("b"), None);
            },
            |browser, root| {
                std::fs::remove_dir_all(root.join("a")).unwrap();
                assert!(browser.back());
                root.join("a")
            },
            Asked::Back,
        ),
        (
            "forward",
            |browser, root| {
                browser.go(root.join("b"), None);
                settle(browser);
                browser.back();
            },
            |browser, root| {
                std::fs::remove_dir_all(root.join("b")).unwrap();
                assert!(browser.forward());
                root.join("b")
            },
            Asked::Forward,
        ),
        (
            "parent",
            |browser, root| {
                browser.go(root.join("a/inner"), None);
            },
            |browser, root| {
                // the folder shown stays listed: its watch is not followed
                std::fs::remove_dir_all(root.join("a")).unwrap();
                assert!(browser.parent());
                root.join("a")
            },
            Asked::Parent,
        ),
    ];
    for (row, before, request, asked) in rows {
        let root = tempfile::tempdir().unwrap();
        for dir in ["a/inner", "b"] {
            std::fs::create_dir_all(root.path().join(dir)).unwrap();
        }
        std::fs::write(root.path().join("a/item"), b"").unwrap();
        std::fs::write(root.path().join("a/inner/deeper"), b"").unwrap();
        let mut browser = browser(&root.path().join("a"), &Listings::default());
        settle(&mut browser);
        before(&mut browser, root.path());
        settle(&mut browser);
        let (dir, history, listed) = (
            browser.dir().to_path_buf(),
            browser.history().clone(),
            names(&browser),
        );

        let target = request(&mut browser, root.path());
        let polled = settle(&mut browser);
        let unopened = polled.unopened.expect(row);
        assert_eq!((unopened.asked, unopened.dir), (asked, target), "{row}");
        assert!(!unopened.error.is_empty(), "{row}");
        assert_eq!(polled.entered, None, "{row}");
        assert_eq!(browser.dir(), dir, "{row}");
        assert_eq!(browser.history(), &history, "{row}");
        assert_eq!(names(&browser), listed, "{row}");
        assert!(!browser.is_waiting(), "{row}");
    }
}

/// A browser on `a` that asked for `b` and holds the first work of its model, which does not come back yet.
fn asking(a: &Path, b: &Path, listings: &Listings) -> (Browser, Vec<ModelWork>) {
    let mut browser = browser(a, listings);
    settle(&mut browser);
    assert!(!browser.go(b.to_path_buf(), None));
    let hung = take(&mut browser);
    assert_eq!(hung.len(), 1);
    (browser, hung)
}

#[test]
fn a_folder_asked_for_again_while_its_work_runs_takes_the_model_it_had() {
    /// What a row leaves: a browser that asked for b again, the work of the model it had and works held out.
    type Row = fn(&Path, &Path, &Listings) -> (Browser, Vec<ModelWork>, Vec<ModelWork>);
    let rows: [(&str, Row); 2] = [
        ("the same window", |a, b, listings| {
            let (mut browser, hung) = asking(a, b, listings);
            // the window stays on its own folder, then asks for b again
            assert!(browser.go(a.to_path_buf(), None));
            assert!(!browser.go(b.to_path_buf(), None));
            (browser, hung, Vec::new())
        }),
        ("a second model let go after it", |a, b, listings| {
            let (mut first, hung) = asking(a, b, listings);
            let (mut second, held) = asking(a, b, listings);
            first.stop_waiting();
            second.stop_waiting();
            let mut third = browser(a, listings);
            settle(&mut third);
            assert!(!third.go(b.to_path_buf(), None));
            (third, hung, held)
        }),
    ];
    for (row, asked_again) in rows {
        let root = tempfile::tempdir().unwrap();
        let (a, b) = (root.path().join("a"), root.path().join("b"));
        for dir in [&a, &b] {
            std::fs::create_dir(dir).unwrap();
        }
        std::fs::write(b.join("item"), b"").unwrap();
        let (mut browser, hung, _held) = asked_again(&a, &b, &Listings::default());
        assert!(take(&mut browser).is_empty(), "{row}: b opened again");

        for work in hung {
            work.run();
        }
        let polled = browser.poll();
        assert_eq!(
            (polled.entered, polled.unopened),
            (Some(Asked::Go), None),
            "{row}"
        );
        assert_eq!(browser.dir(), b, "{row}");
        settle(&mut browser);
        assert_eq!(names(&browser), ["item"], "{row}");
    }
}

#[test]
fn a_folder_kept_after_its_work_ended_wakes_the_window_that_takes_it() {
    for row in ["open", "go"] {
        let root = tempfile::tempdir().unwrap();
        let (a, b) = (root.path().join("a"), root.path().join("b"));
        for dir in [&a, &b] {
            std::fs::create_dir(dir).unwrap();
        }
        std::fs::write(b.join("item"), b"").unwrap();
        let listings = Listings::default();

        // a window lets b go while its first work runs, and the work ends before the session sweeps its listings
        let (mut first, hung) = asking(&a, &b, &listings);
        first.stop_waiting();
        for work in hung {
            work.run();
        }

        let woken = Arc::new(AtomicUsize::new(0));
        let count = Arc::clone(&woken);
        let notify: Notify = Arc::new(move || {
            count.fetch_add(1, Ordering::SeqCst);
        });
        let mut second = match row {
            "open" => woken_browser(&b, &listings, notify),
            _ => {
                let mut second = woken_browser(&a, &listings, notify);
                settle(&mut second);
                woken.store(0, Ordering::SeqCst);
                assert!(!second.go(b.clone(), None));
                second
            }
        };
        assert!(woken.load(Ordering::SeqCst) > 0, "{row}: not woken");
        assert!(take(&mut second).is_empty(), "{row}: b opened again");

        let polled = second.poll();
        let entered = (row == "go").then_some(Asked::Go);
        assert_eq!((polled.entered, polled.unopened), (entered, None), "{row}");
        assert_eq!(second.dir(), b, "{row}");
        assert_eq!(names(&second), ["item"], "{row}");
    }
}
