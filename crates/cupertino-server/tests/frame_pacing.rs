//! Each output is paced by its own refresh: an animation draws once per grid point of every head, a commit marks only
//! the heads that show its surface, and the cursor follows the head under the pointer. Counts are taken on the fixed
//! model clock of the headless compositor, as differences, so they do not depend on machine load. Only the pacing of a
//! change, which no vblank does offscreen, is counted on the running clock.

use crate::common;

use std::time::{Duration, Instant};

use common::desktop::Desktop;
use cupertino_server::backend::headless::{self, HeadlessDisplay};
use cupertino_server::displays::HeadMode;
use smithay::desktop::WindowSurfaceType;
use smithay::wayland::compositor::CompositorHandler;

const SIZE: (i32, i32) = (640, 400);

/// Two heads side by side, the first at 144 Hz and the second at 60 Hz.
fn two_heads() -> Desktop {
    let display = |name: &str| HeadlessDisplay {
        name: name.into(),
        size: SIZE,
        physical_mm: (0, 0),
    };
    let mut d = Desktop::with_displays(&[display("headless-1"), display("headless-2")], 1.0);
    let head = d
        .state
        .displays
        .head_of_output(d.state.outputs[0].id)
        .expect("the first output's display")
        .id;
    d.state.head_mode_replaced(
        head,
        HeadMode {
            width: SIZE.0,
            height: SIZE.1,
            refresh_mhz: 144_000,
        },
    );
    d.at(1000);
    d
}

fn composes(d: &Desktop) -> [u64; 2] {
    [0, 1].map(|idx| d.state.outputs[idx].scheduler.counts().composes)
}

/// Settle every head, so only what the test does marks one.
fn settle(d: &mut Desktop) {
    for o in &mut d.state.outputs {
        o.scheduler.called_back();
    }
}

fn damaged(d: &Desktop) -> [bool; 2] {
    [0, 1].map(|idx| d.state.outputs[idx].scheduler.damaged())
}

#[test]
fn an_animation_draws_once_per_grid_point_of_each_head() {
    let mut d = two_heads();
    assert_eq!(d.state.refresh_of(0), Duration::from_secs_f64(1.0 / 144.0));
    assert_eq!(d.state.refresh_of(1), Duration::from_secs_f64(1.0 / 60.0));
    d.state.toggle_overview(false);
    d.at(1001);
    let before = composes(&d);
    let span = 200;
    for ms in 1002..1002 + span {
        d.at(ms);
        assert!(
            d.state.animating(),
            "the overview still animates at {ms} ms"
        );
    }
    let after = composes(&d);
    for (idx, hz) in [(0, 144.0), (1, 60.0)] {
        let drawn = after[idx] - before[idx];
        let grid = (f64::from(span) / 1000.0 * hz).round() as u64;
        assert!(
            drawn.abs_diff(grid) <= 1,
            "head {idx} at {hz} Hz drew {drawn} frames in {span} ms, its grid has {grid}"
        );
    }
}

#[test]
fn a_commit_marks_only_the_heads_that_show_its_surface() {
    let mut d = two_heads();
    d.window();
    d.at(2000);
    let entry = &d.state.windows[0];
    let surface = entry.toplevel.wl_surface().clone();
    let output = d.state.wm.window(entry.id).expect("the window").output;
    let shown = d
        .state
        .outputs
        .iter()
        .position(|o| o.id == output)
        .expect("the window's output");
    assert!(
        !d.state.animating(),
        "nothing animates once the window is open"
    );

    settle(&mut d);
    CompositorHandler::commit(&mut d.state, &surface);
    let marked = damaged(&d);
    assert!(marked[shown], "the head that shows the window is marked");
    assert!(!marked[1 - shown], "the other head is not");
    let before = composes(&d);
    headless::render(&mut d.state);
    let after = composes(&d);
    assert_eq!(after[shown] - before[shown], 1, "the window's head draws");
    assert_eq!(
        after[1 - shown],
        before[1 - shown],
        "the other head does not"
    );

    let _layer = common::Client::inhibiting_layer(d.socket());
    let deadline = Instant::now() + common::HANG_GUARD;
    let (layer_head, layer) = loop {
        d.pump();
        let found = d.state.outputs.iter().enumerate().find_map(|(idx, o)| {
            smithay::desktop::layer_map_for_output(&o.output)
                .layers()
                .next()
                .map(|l| (idx, l.wl_surface().clone()))
        });
        if let Some(found) = found {
            break found;
        }
        assert!(Instant::now() < deadline, "the layer surface did not map");
    };
    assert!(
        smithay::desktop::layer_map_for_output(&d.state.outputs[layer_head].output)
            .layer_for_surface(&layer, WindowSurfaceType::TOPLEVEL)
            .is_some()
    );
    settle(&mut d);
    CompositorHandler::commit(&mut d.state, &layer);
    let marked = damaged(&d);
    assert!(marked[layer_head], "the layer surface's head is marked");
    assert!(!marked[1 - layer_head], "the other head is not");
}

#[test]
fn the_cursor_is_paced_by_the_head_under_the_pointer_or_the_first_powered_one() {
    let mut d = two_heads();
    let second = d.state.outputs[1].rect;
    d.state
        .pointer_moved_to((second.x + 10.0, second.y + 10.0).into(), 0, None);
    assert_eq!(d.state.pointer_output_idx(), Some(1));
    let id = d.state.outputs[1].id;
    d.state.set_output_power(id, false).expect("powering off");
    assert_eq!(
        d.state.pointer_output_idx(),
        Some(0),
        "an animated cursor keeps animating on a powered head"
    );
}

#[test]
fn a_window_shown_nowhere_marks_only_the_head_under_the_pointer() {
    let mut d = two_heads();
    d.window();
    d.at(2000);
    let entry = &d.state.windows[0];
    let (id, surface) = (entry.id, entry.toplevel.wl_surface().clone());
    let output = d.state.wm.window(id).expect("the window").output;
    let shown = d
        .state
        .outputs
        .iter()
        .position(|o| o.id == output)
        .expect("the window's output");
    let other = 1 - shown;
    d.state.command(cupertino_wm::Command::Minimize {
        window: id,
        target: None,
    });
    d.at(5000);
    let rect = d.state.outputs[other].rect;
    d.state
        .pointer_moved_to((rect.x + 10.0, rect.y + 10.0).into(), 0, None);

    settle(&mut d);
    CompositorHandler::commit(&mut d.state, &surface);
    let marked = damaged(&d);
    assert!(marked[other], "the head under the pointer is marked");
    assert!(
        !marked[shown],
        "the window's own head, where it is hidden, is not"
    );
}

#[test]
fn on_the_running_clock_a_window_that_commits_after_every_frame_draws_once_per_refresh() {
    let mut d = Desktop::new(1.0);
    d.window();
    d.at(2000);
    let surface = d.state.windows[0].toplevel.wl_surface().clone();
    d.state.fixed_time = None;
    let before = d.state.outputs[0].scheduler.counts().composes;
    let span = Duration::from_secs(1);
    let end = Instant::now() + span;
    while Instant::now() < end {
        CompositorHandler::commit(&mut d.state, &surface);
        headless::step(&mut d.state, &mut d.event_loop, Duration::from_millis(1))
            .expect("a loop pass");
    }
    let drawn = d.state.outputs[0].scheduler.counts().composes - before;
    let grid = (span.as_secs_f64() / d.state.refresh_of(0).as_secs_f64()).round() as u64;
    assert!(
        drawn <= grid + 2,
        "{drawn} frames in {span:?}, the 60 Hz grid has {grid}"
    );
}

/// Two heads plugged in at 120 Hz and 60 Hz: each is paced by its own mode, and an animation stands at the same point at
/// the same time on a desktop whose heads both refresh at 60 Hz, however many frames each head drew of it.
#[test]
fn an_animation_follows_the_clock_not_the_frames_of_heads_at_120_and_60_hz() {
    let display = |name: &str| HeadlessDisplay {
        name: name.into(),
        size: SIZE,
        physical_mm: (0, 0),
    };
    let desktop = |first_mhz: i32| {
        let mut d = Desktop::with_displays(&[display("headless-1")], 1.0);
        let head = d.state.displays.heads[0].id;
        d.state.head_mode_replaced(
            head,
            HeadMode {
                width: SIZE.0,
                height: SIZE.1,
                refresh_mhz: first_mhz,
            },
        );
        headless::add_display_at(&mut d.state, &display("headless-2"), 60_000, &[], Some(1.0))
            .unwrap();
        d.at(1000);
        d
    };
    let mut fast = desktop(120_000);
    let mut slow = desktop(60_000);
    assert_eq!(
        fast.state.refresh_of(0),
        Duration::from_secs_f64(1.0 / 120.0)
    );
    assert_eq!(
        fast.state.refresh_of(1),
        Duration::from_secs_f64(1.0 / 60.0)
    );
    for d in [&mut fast, &mut slow] {
        d.state.toggle_overview(false);
        d.at(1001);
    }
    let before = [composes(&fast), composes(&slow)];
    for ms in 1002..1202 {
        fast.at(ms);
        // the other desktop's loop runs far less often
        if ms % 25 == 0 {
            slow.at(ms);
        }
    }
    slow.at(1201);
    let progress = |d: &Desktop| {
        let now = d.state.now();
        d.state
            .shell
            .overview
            .as_ref()
            .unwrap()
            .transition
            .progress_at(now)
    };
    assert_eq!(fast.state.now(), slow.state.now());
    assert_eq!(progress(&fast), progress(&slow));
    let drawn = |d: &Desktop, i: usize, idx: usize| composes(d)[idx] - before[i][idx];
    assert!(
        drawn(&fast, 0, 0).abs_diff(24) <= 1,
        "120 Hz head: {}",
        drawn(&fast, 0, 0)
    );
    assert!(
        drawn(&fast, 0, 1).abs_diff(12) <= 1,
        "60 Hz head beside it: {}",
        drawn(&fast, 0, 1)
    );
}
