//! The icon grid shapes the names past the cells it builds in the window's idle time, between frames: a scroll whose
//! frames build new rows then shapes no name inside a frame, and every frame has the pixels it has without it. A slice
//! with time for only a few names leaves the rest to the next, which goes on where it stopped, so the work ends.

#![cfg(feature = "app")]

use crate::common;

use common::{budget, place, Run};
use cupertino_files::notice::Notices;
use cupertino_files::places::PlaceKind;
use cupertino_kit::core::Key;
use cupertino_kit::geometry::Point;
use cupertino_kit::platform::{PointerEvent, ScrollSource, Timestamp, WindowEvent};
use cupertino_kit::text::TextWork;

/// A name long enough to wrap and be cut short.
fn long(i: usize) -> String {
    format!(
        "file {i:03} the quarterly report of the eastern regional office with all of its appendices \
         and the minutes of every meeting held.xlsx"
    )
}

/// The folder's items: short names, and every seventh one long enough to wrap and be cut short.
fn names(count: usize) -> Vec<String> {
    (0..count)
        .map(|i| match i % 7 {
            0 => long(i),
            _ => format!("file {i:03}.txt"),
        })
        .collect()
}

/// A window on a folder of `count` items, in the icon view.
fn start(count: usize) -> Run {
    start_with(names(count))
}

/// A window on a folder of the items `names`, in the icon view.
fn start_with(names: Vec<String>) -> Run {
    let home = tempfile::tempdir().unwrap();
    let folder = home.path().join("Documents");
    std::fs::create_dir_all(&folder).unwrap();
    for name in names {
        std::fs::write(folder.join(name), "x").unwrap();
    }
    let places = vec![place("Documents", &folder, PlaceKind::Documents)];
    let mut run = Run::start(home, folder, places, Notices::start(None), None);
    run.wait_until(|run| run.rows().len() >= 10);
    run.menu("view-icons");
    run.wait_until(|run| !run.icons().is_empty());
    run.frames(1000, 16);
    run
}

/// One wheel step down the grid, and the frames it draws.
fn scroll(run: &mut Run) {
    let runtime = run.driver.runtime();
    let id = runtime.find(run.window, &Key::from("icons")).unwrap();
    let snapshot = runtime.snapshot(run.window).unwrap();
    let grid = snapshot.nodes.iter().find(|n| n.id == id).unwrap().rect;
    run.send(WindowEvent::Pointer(
        PointerEvent::Scroll {
            position: grid.center(),
            delta: Point::new(0.0, 120.0),
            value120: None,
            source: ScrollSource::Wheel,
            stop: false,
            inverted: (false, false),
        },
        Timestamp(0),
    ));
    run.frames(160, 16);
}

/// What each of `steps` wheel steps shaped inside its frames, the names the grid built new and the last frame's
/// pixels, with the idle time handed out between steps when `idle`.
fn scrolled(steps: usize, idle: bool) -> Vec<(u64, usize, Vec<u8>)> {
    let mut run = start(280);
    let mut out = Vec::new();
    for _ in 0..steps {
        if idle {
            run.driver.runtime_mut().run_idle().unwrap();
            assert!(!run.driver.runtime().has_idle_work(), "the work was done");
        }
        let shown: Vec<String> = run.icons().into_iter().map(|(name, _)| name).collect();
        let before = TextWork::current().shaped;
        scroll(&mut run);
        let shaped = TextWork::current().shaped - before;
        let new = run
            .icons()
            .into_iter()
            .filter(|(name, _)| !shown.contains(name))
            .count();
        let frames = run.driver.runtime().window_system().frames(run.window);
        let pixels = frames.unwrap().last().unwrap().pixels.clone();
        out.push((shaped, new, pixels));
    }
    out
}

#[test]
fn a_scroll_whose_frames_build_new_rows_shapes_no_name_inside_a_frame_once_the_idle_work_ran() {
    let steps = 6;
    let ahead = scrolled(steps, true);
    let inside = scrolled(steps, false);
    for (step, ((shaped, new, pixels), (shaped_inside, new_inside, pixels_inside))) in
        ahead.iter().zip(&inside).enumerate()
    {
        assert!(*new > 0, "step {step} built new rows");
        assert_eq!(new, new_inside, "step {step}: the same cells");
        assert_eq!(*shaped, 0, "step {step}: no name shaped inside its frames");
        assert!(
            *shaped_inside > 0,
            "step {step}: without the idle work its frames shape the new names"
        );
        assert!(
            pixels == pixels_inside,
            "step {step}: the frame has the pixels it has without the idle work"
        );
    }
}

#[test]
fn idle_work_on_long_names_with_time_for_a_few_in_each_slice_goes_on_where_it_stopped_and_ends() {
    // every name wraps and is cut short, and each look at the clock takes a quarter of a slice: a slice has time for a
    // few names, far fewer than the walk past the band holds; work that never ends reads the clock past its limit
    let mut run = start_with((0..280).map(long).collect());
    assert!(run.driver.runtime().has_idle_work());
    let before = TextWork::current().shaped;
    run.driver.clock().tick_on_read(250_000, 100_000);
    let mut rounds = 0;
    while run.driver.runtime().has_idle_work() && rounds < 64 {
        run.driver.runtime_mut().run_idle().unwrap();
        // a wake that came due as the time went on, and the frames it draws, go first
        run.driver.run_until_idle(budget()).unwrap();
        rounds += 1;
    }
    run.driver.clock().tick_on_read(0, 0);
    assert!(
        !run.driver.runtime().has_idle_work(),
        "the work ended, after {rounds} rounds"
    );
    assert!(
        TextWork::current().shaped > before,
        "the names past the band were shaped"
    );
    // and it is the work the frames want: a scroll then shapes no name inside them
    let before = TextWork::current().shaped;
    scroll(&mut run);
    assert_eq!(
        TextWork::current().shaped - before,
        0,
        "no name shaped inside the frames"
    );
}
