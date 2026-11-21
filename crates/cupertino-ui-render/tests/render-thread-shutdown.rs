//! A process ending waits for its render thread to finish what was handed to it, so no driver call is left running
//! while the process's exit tears the driver's libraries down. Its own test binary: the render thread is the
//! process's, and the first part needs one that never started.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use cupertino_ui_render::render_thread::{self, Resident, Status, SHUTDOWN_WAIT};

/// A value of the render thread that takes a while to drop, as freeing a window's buffers does, and counts its drop.
struct SlowToDrop(Arc<AtomicUsize>);

impl Drop for SlowToDrop {
    fn drop(&mut self) {
        std::thread::sleep(Duration::from_millis(20));
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

#[test]
fn shutdown_waits_for_the_work_handed_over_before_it_and_gives_up_on_a_wedged_thread() {
    // a process that never drew on the render thread does not start it to end
    assert!(render_thread::shutdown());
    assert_eq!(render_thread::heartbeat().done, 0);

    // values of the render thread dropped on this thread, and work handed over, all done once shutdown returns
    let dropped = Arc::new(AtomicUsize::new(0));
    let values: Vec<Resident<SlowToDrop>> = (0..5)
        .map(|_| {
            let counter = Arc::clone(&dropped);
            render_thread::run(move |on| Resident::new(on, SlowToDrop(counter)))
        })
        .collect();
    let worked = Arc::new(AtomicUsize::new(0));
    let theirs = Arc::clone(&worked);
    render_thread::hand_over(move || {
        std::thread::sleep(Duration::from_millis(20));
        theirs.fetch_add(1, Ordering::SeqCst);
    });
    drop(values);
    assert!(render_thread::shutdown());
    assert_eq!(dropped.load(Ordering::SeqCst), 5);
    assert_eq!(worked.load(Ordering::SeqCst), 1);
    assert!(matches!(render_thread::status(), Status::Idle));

    // a render thread stuck in its work does not hold the process past the bound
    let (release, wedged) = mpsc::channel::<()>();
    render_thread::hand_over(move || {
        let _ = wedged.recv();
    });
    let started = Instant::now();
    assert!(!render_thread::shutdown());
    let waited = started.elapsed();
    assert!(waited >= SHUTDOWN_WAIT, "{waited:?}");
    assert!(waited < SHUTDOWN_WAIT * 3, "{waited:?}");
    release.send(()).unwrap();
    assert!(render_thread::shutdown());
}
