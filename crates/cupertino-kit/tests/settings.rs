//! `kit::task::follow_settings`: the files of a settings watch are followed while the application runs, a burst of
//! changes is reported once after it is quiet, a removal is a change, and each report is heard on the UI thread.

use crate::support;

use std::sync::{Arc, Mutex};
use std::thread::ThreadId;
use std::time::{Duration, Instant};

use cupertino_defaults::watch::{ConfigWatcher, WatchEvent};
use cupertino_kit::core::{Application, Command, Window};
use cupertino_kit::task::follow_settings;
use cupertino_kit::test::TestDriver;

type Heard = Arc<Mutex<Vec<(WatchEvent, ThreadId)>>>;

thread_local! {
    /// What the application the thread runs is to watch, and where it reports.
    static WATCH: std::cell::RefCell<Option<(ConfigWatcher, Heard)>> = const { std::cell::RefCell::new(None) };
}

struct Following;

impl Application for Following {
    type Model = ();
    type Message = ();
    const RESIDENT: bool = true;

    fn init() -> ((), Vec<Command<()>>) {
        let (watch, heard) = WATCH
            .with(|w| w.borrow_mut().take())
            .expect("a watch to follow");
        let report = move |event| {
            heard
                .lock()
                .unwrap()
                .push((event, std::thread::current().id()));
        };
        (
            (),
            vec![Command::Perform(Box::pin(follow_settings(watch, report)))],
        )
    }

    fn update(_: &mut (), (): ()) -> Vec<Command<()>> {
        Vec::new()
    }

    fn view(_: &()) -> Vec<Window<()>> {
        Vec::new()
    }
}

/// Run `driver` until `heard` holds `count` reports, waiting for the wakes of the file notification in between.
fn until(driver: &mut TestDriver<Following>, heard: &Heard, count: usize) {
    let end = Instant::now() + support::PATIENCE;
    loop {
        driver
            .run_until_idle(std::num::NonZeroU32::new(1000).unwrap())
            .unwrap();
        if heard.lock().unwrap().len() >= count {
            return;
        }
        let left = end.saturating_duration_since(Instant::now());
        assert!(
            driver.window_system_mut().wait_for_wake(left),
            "{count} reports did not come within {:?}",
            support::PATIENCE
        );
    }
}

#[test]
fn a_burst_of_changes_to_a_followed_file_is_reported_once_and_its_removal_is_a_change_too() {
    let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("kit-settings-follow");
    let _ = std::fs::remove_dir_all(&dir);
    let file = dir.join("app.conf");
    let watch = ConfigWatcher::new(vec![file.clone()]).unwrap();
    let heard: Heard = Arc::default();
    WATCH.with(|w| *w.borrow_mut() = Some((watch, Arc::clone(&heard))));
    let mut driver = support::driver::<Following>(support::text_system());
    driver
        .run_until_idle(std::num::NonZeroU32::new(1000).unwrap())
        .unwrap();

    for text in ["a=1", "a=2", "a=3"] {
        std::fs::write(&file, text).unwrap();
    }
    until(&mut driver, &heard, 1);
    // nothing else is on its way: a wake that comes is the file system's, and the list does not grow past one
    driver
        .window_system_mut()
        .wait_for_wake(Duration::from_millis(300));
    driver
        .run_until_idle(std::num::NonZeroU32::new(1000).unwrap())
        .unwrap();
    assert_eq!(
        heard
            .lock()
            .unwrap()
            .iter()
            .map(|(e, _)| e.clone())
            .collect::<Vec<_>>(),
        [WatchEvent::Changed(vec![file.clone()])]
    );

    std::fs::remove_file(&file).unwrap();
    until(&mut driver, &heard, 2);
    let heard = heard.lock().unwrap();
    assert_eq!(heard[1].0, WatchEvent::Changed(vec![file]));
    let ui = std::thread::current().id();
    assert!(
        heard.iter().all(|(_, thread)| *thread == ui),
        "heard on the UI thread"
    );
    drop(heard);
    let _ = std::fs::remove_dir_all(&dir);
}
