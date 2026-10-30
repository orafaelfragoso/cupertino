//! Each output is paced by its own refresh: an animation draws once per grid point of every head, a commit marks only
//! the heads that show its surface, and the cursor follows the head under the pointer. Counts are taken on the fixed
//! model clock of the headless compositor, as differences, so they do not depend on machine load.

mod common;

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
    let deadline = Instant::now() + Duration::from_secs(10);
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
