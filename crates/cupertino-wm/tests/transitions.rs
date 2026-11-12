//! Close, minimize, zoom and full screen are distinct operations: each keeps the frame to go back to, a refused one
//! leaves everything as it was, closing asks the client and never ends its application, and key and main are separate
//! states.
use crate::common;
use common::*;
use cupertino_wm::*;

const START: Rect = Rect::new(100.0, 100.0, 600.0, 400.0);

fn fullscreen(h: &mut H, window: WindowId, requested_by: Requester) -> Vec<Effect> {
    h.cmd(Command::Fullscreen {
        window,
        output: None,
        requested_by,
    })
}

fn minimize(h: &mut H, window: WindowId) -> Vec<Effect> {
    h.cmd(Command::Minimize {
        window,
        target: Some(Rect::new(900.0, 1040.0, 40.0, 40.0)),
    })
}

fn at_rest(h: &H, w: WindowId, state: LifecycleState, frame: Rect) {
    assert_eq!(h.state(w), state);
    assert!(
        rect_eq(h.wm.window(w).unwrap().geometry(), frame),
        "{state:?}: {:?}, not {frame:?}",
        h.wm.window(w).unwrap().geometry()
    );
    assert!(rect_eq(h.frame(w), frame), "drawn at {:?}", h.frame(w));
}

#[test]
fn each_transition_goes_back_to_the_frame_it_left() {
    let mut h = H::new();
    let w = h.map("a");
    at_rest(&h, w, LifecycleState::Normal, START);

    minimize(&mut h, w);
    h.settle();
    assert_eq!(h.state(w), LifecycleState::Minimized);
    h.cmd(Command::Restore { window: w });
    h.settle();
    at_rest(&h, w, LifecycleState::Normal, START);

    h.cmd(Command::ToggleZoom { window: w });
    h.settle();
    at_rest(&h, w, LifecycleState::Zoomed, USABLE);
    h.cmd(Command::ToggleZoom { window: w });
    h.settle();
    at_rest(&h, w, LifecycleState::Normal, START);

    fullscreen(&mut h, w, Requester::User);
    h.settle();
    at_rest(&h, w, LifecycleState::FullScreen, SCREEN);
    h.cmd(Command::ExitFullscreen { window: w });
    h.settle();
    at_rest(&h, w, LifecycleState::Normal, START);
}

#[test]
fn nested_transitions_unwind_to_each_frame_in_turn() {
    let mut h = H::new();
    let w = h.map("a");
    h.cmd(Command::ToggleZoom { window: w });
    h.settle();
    // a zoomed window minimized comes back zoomed, and still unzooms to where it started
    minimize(&mut h, w);
    h.settle();
    h.cmd(Command::Restore { window: w });
    h.settle();
    at_rest(&h, w, LifecycleState::Zoomed, USABLE);
    // full screen from zoomed goes back to zoomed
    fullscreen(&mut h, w, Requester::User);
    h.settle();
    h.cmd(Command::ExitFullscreen { window: w });
    h.settle();
    at_rest(&h, w, LifecycleState::Zoomed, USABLE);
    h.cmd(Command::ToggleZoom { window: w });
    h.settle();
    at_rest(&h, w, LifecycleState::Normal, START);
}

#[test]
fn a_refused_full_screen_changes_nothing() {
    let mut h = H::new();
    let w = h.map("a");
    let workspaces = h.ws();
    h.cmd(Command::SetDisabledControls {
        window: w,
        disabled: DisabledControls {
            minimize: false,
            zoom: true,
        },
    });
    let fx = fullscreen(&mut h, w, Requester::User);
    assert_eq!(fx, [], "no configure, no animation, no workspace");
    h.settle();
    at_rest(&h, w, LifecycleState::Normal, START);
    assert_eq!(h.ws(), workspaces);
    assert_eq!(h.wm.key_window(), Some(w));
    let states = h.wm.window(w).unwrap().lifecycle.toplevel_states();
    assert!(!states.fullscreen && !states.maximized);

    // a minimized window cannot go full screen either, and stays minimized
    let v = h.map("b");
    minimize(&mut h, v);
    h.settle();
    let fx = fullscreen(&mut h, v, Requester::Client);
    assert!(
        !has(&fx, |e| matches!(e, Effect::StartAnimation { .. })),
        "{fx:?}"
    );
    h.settle();
    assert_eq!(h.state(v), LifecycleState::Minimized);
    assert_eq!(h.ws(), workspaces);
}

#[test]
fn closing_asks_the_client_and_the_last_window_leaves_its_application_in_front() {
    let mut h = H::new();
    let w = h.map("a");
    let app = h.app("a");
    let fx = h.cmd(Command::RequestClose { window: w });
    assert_eq!(fx, [Effect::RequestClose { window: w }]);
    // the client may refuse (an unsaved document): nothing changed
    h.settle();
    at_rest(&h, w, LifecycleState::Normal, START);

    // it closes: the window goes, the application stays registered and in front with no key window; whether it then
    // quits is its own policy, not the window manager's
    h.cmd(Command::Unmap { window: w });
    let fx = h.settle();
    assert!(has(&fx, |e| *e == Effect::WindowDestroyed { window: w }));
    assert!(
        !has(&fx, |e| matches!(e, Effect::Activate { .. })),
        "{fx:?}"
    );
    assert!(h.wm.window(w).is_none());
    assert!(h.wm.app(app).is_some());
    assert_eq!(h.wm.front_app(), Some(app));
    assert_eq!(h.wm.key_window(), None);
    assert_eq!(h.wm.main_window(), None);
}

#[test]
fn closing_one_of_two_windows_keeps_the_application_and_gives_the_keyboard_to_the_other() {
    let mut h = H::new();
    let a = h.map("a");
    let b = h.map("a");
    assert_eq!(h.wm.key_window(), Some(b));
    h.cmd(Command::Unmap { window: b });
    h.settle();
    assert_eq!(h.wm.key_window(), Some(a));
    assert_eq!(h.wm.front_app(), Some(h.app("a")));
}

#[test]
fn a_panel_takes_the_keyboard_without_becoming_the_main_window() {
    let mut h = H::new();
    let doc = h.map("a");
    assert_eq!(h.wm.main_window(), Some(doc));
    let panel = WindowId(40);
    h.cmd(Command::Map(MapParams {
        kind: WindowKind::Utility,
        ..MapParams::new(panel, "a", Rect::new(800.0, 100.0, 240.0, 300.0))
    }));
    h.settle();
    assert_eq!(h.wm.window(panel).unwrap().level, WindowLevel::Floating);
    assert_eq!(h.wm.key_window(), Some(panel), "the panel has the keyboard");
    assert_eq!(h.wm.main_window(), Some(doc), "the document stays main");
    assert!(!h.wm.window(panel).unwrap().can_be_main());

    // a second document becomes key and main; the panel's click makes it key again and leaves that one main
    let other = h.map("a");
    assert_eq!(
        (h.wm.key_window(), h.wm.main_window()),
        (Some(other), Some(other))
    );
    h.cmd(Command::FocusWindow { window: panel });
    assert_eq!(
        (h.wm.key_window(), h.wm.main_window()),
        (Some(panel), Some(other))
    );
}

#[test]
fn a_sheet_is_key_and_its_parent_main() {
    let mut h = H::new();
    let doc = h.map("a");
    let sheet = WindowId(41);
    h.cmd(Command::Map(MapParams {
        parent: Some(doc),
        modal: true,
        kind: WindowKind::AppModal,
        ..MapParams::new(sheet, "a", Rect::new(200.0, 140.0, 300.0, 200.0))
    }));
    h.settle();
    assert_eq!(h.wm.key_window(), Some(sheet));
    assert_eq!(h.wm.main_window(), Some(doc));
}

/// The output of a full-screen window and of a zoomed one goes away: both come onto the remaining output, the full-screen
/// one leaving full screen there, and every frame they go back to afterwards is on that output.
#[test]
fn windows_of_a_removed_output_keep_their_restore_frames_on_the_remaining_one() {
    let mut h = H::new();
    let out2 = OutputId(2);
    h.cmd(Command::AddOutput {
        output: out2,
        rect: Rect::new(1920.0, 0.0, 1280.0, 800.0),
        usable: Rect::new(1920.0, 24.0, 1280.0, 776.0),
    });
    let f = h.map_at("a", Rect::new(2000.0, 100.0, 600.0, 400.0));
    let z = h.map_at("b", Rect::new(2100.0, 100.0, 600.0, 400.0));
    for w in [f, z] {
        h.cmd(Command::MoveWindowToWorkspace {
            window: w,
            workspace: h.wm.current_workspace(out2).unwrap(),
        });
    }
    h.cmd(Command::ToggleZoom { window: z });
    h.cmd(Command::FocusWindow { window: f });
    fullscreen(&mut h, f, Requester::User);
    h.settle();
    assert_eq!(h.state(f), LifecycleState::FullScreen);
    assert_eq!(h.state(z), LifecycleState::Zoomed);

    let fx = h.cmd(Command::RemoveOutput { output: out2 });
    assert!(
        has(&fx, |e| *e
            == Effect::StartAnimation {
                window: f,
                kind: AnimationKind::ExitFullScreen
            }),
        "{fx:?}"
    );
    h.settle();
    let on_screen = |h: &H, w: WindowId| {
        let g = h.wm.window(w).unwrap().geometry();
        assert!(
            USABLE.contains(g.origin()) && g.x + g.w <= USABLE.x + USABLE.w + 1e-9,
            "{w:?} at {g:?}"
        );
    };
    assert_eq!(h.state(f), LifecycleState::Normal);
    on_screen(&h, f);
    assert_eq!(h.state(z), LifecycleState::Zoomed);
    on_screen(&h, z);
    assert!(h.wm.visible_windows(OUT).contains(&f));
    h.cmd(Command::ToggleZoom { window: z });
    h.settle();
    assert_eq!(h.state(z), LifecycleState::Normal);
    on_screen(&h, z);
    // full screen again goes onto the remaining output and comes back to the frame it left
    let before = h.wm.window(f).unwrap().geometry();
    fullscreen(&mut h, f, Requester::User);
    h.settle();
    assert_eq!(h.wm.window(f).unwrap().geometry(), SCREEN);
    h.cmd(Command::ExitFullscreen { window: f });
    h.settle();
    at_rest(&h, f, LifecycleState::Normal, before);
}

/// The output a window is being dragged on goes away mid-drag: the window comes onto the remaining output inside its
/// usable area, the drag goes on there with the pointer, and the window rests on that output when it ends.
#[test]
fn a_window_dragged_on_an_output_that_goes_away_keeps_following_the_pointer() {
    let mut h = H::new();
    let out2 = OutputId(2);
    h.cmd(Command::AddOutput {
        output: out2,
        rect: Rect::new(1920.0, 0.0, 1280.0, 800.0),
        usable: Rect::new(1920.0, 24.0, 1280.0, 776.0),
    });
    let w = h.map_at("a", Rect::new(2000.0, 100.0, 600.0, 400.0));
    h.cmd(Command::MoveWindowToWorkspace {
        window: w,
        workspace: h.wm.current_workspace(out2).unwrap(),
    });
    h.cmd(Command::BeginMove {
        window: w,
        pointer: Point::new(2100.0, 110.0),
    });
    h.cmd(motion(2300.0, 300.0));
    assert_eq!(h.wm.window(w).unwrap().output, out2);

    h.cmd(Command::RemoveOutput { output: out2 });
    assert!(h.wm.is_interacting(), "the drag goes on");
    let g = h.wm.window(w).unwrap().lifecycle.normal_frame();
    assert!(
        g.x >= USABLE.x && g.right() <= USABLE.right() && g.y >= USABLE.y,
        "{g:?}"
    );
    assert_eq!(h.wm.window(w).unwrap().output, OUT);
    // the server brings the pointer onto the remaining output; the window follows it there
    h.cmd(motion(1800.0, 300.0));
    h.cmd(motion(1500.0, 320.0));
    h.cmd(Command::PointerRelease);
    h.settle();
    let g = h.wm.window(w).unwrap().geometry();
    assert!(USABLE.contains(g.origin()), "{g:?}");
    assert!(h.wm.visible_windows(OUT).contains(&w));
}

/// A window put back on an output that came back takes the frame it is given there, fitted to the output's usable area;
/// a zoomed one is zoomed to the new output, and a full-screen one stays where it is.
#[test]
fn a_window_moved_to_an_output_takes_its_frame_there_fitted_and_keeps_its_layout() {
    let mut h = H::new();
    let a = h.map_at("a", Rect::new(100.0, 100.0, 600.0, 400.0));
    let z = h.map_at("b", Rect::new(200.0, 100.0, 600.0, 400.0));
    let f = h.map_at("c", Rect::new(300.0, 100.0, 600.0, 400.0));
    h.cmd(Command::ToggleZoom { window: z });
    h.cmd(Command::FocusWindow { window: f });
    fullscreen(&mut h, f, Requester::User);
    h.settle();
    let out2 = OutputId(2);
    let usable2 = Rect::new(1920.0, 24.0, 1280.0, 776.0);
    h.cmd(Command::AddOutput {
        output: out2,
        rect: Rect::new(1920.0, 0.0, 1280.0, 800.0),
        usable: usable2,
    });
    for (w, x) in [(a, 2000.0), (z, 2100.0), (f, 2200.0)] {
        h.cmd(Command::MoveWindowToOutput {
            window: w,
            output: out2,
            frame: Rect::new(x, 100.0, 600.0, 400.0),
        });
    }
    h.settle();
    assert_eq!(h.wm.window(a).unwrap().output, out2);
    assert_eq!(h.frame(a), Rect::new(2000.0, 100.0, 600.0, 400.0));
    assert_eq!(h.state(z), LifecycleState::Zoomed);
    assert_eq!(h.wm.window(z).unwrap().output, out2);
    let g = h.frame(z);
    assert!(
        g.x >= usable2.x && g.right() <= usable2.right() + 1e-9,
        "{g:?}"
    );
    assert_eq!(h.wm.window(f).unwrap().output, OUT, "full screen stays");
    assert_eq!(h.state(f), LifecycleState::FullScreen);
    // a frame larger than the output is fitted into it
    h.cmd(Command::MoveWindowToOutput {
        window: a,
        output: OUT,
        frame: Rect::new(5000.0, -300.0, 600.0, 400.0),
    });
    h.settle();
    let g = h.frame(a);
    assert!(
        USABLE.contains(g.origin()) && g.right() <= USABLE.right() + 1e-9,
        "{g:?}"
    );
}

/// The output a window is being resized on goes away mid-resize: the resize ends with the frame the window was given on
/// the remaining output, so the pointer coming onto that output does not pull its edges back towards the one that went.
#[test]
fn a_resize_on_an_output_that_goes_away_ends_with_the_window_on_the_remaining_one() {
    let mut h = H::new();
    let out2 = OutputId(2);
    h.cmd(Command::AddOutput {
        output: out2,
        rect: Rect::new(1920.0, 0.0, 1280.0, 800.0),
        usable: Rect::new(1920.0, 24.0, 1280.0, 776.0),
    });
    let w = h.map_at("a", Rect::new(2000.0, 100.0, 600.0, 400.0));
    h.cmd(Command::MoveWindowToWorkspace {
        window: w,
        workspace: h.wm.current_workspace(out2).unwrap(),
    });
    let right_edge = Edges {
        right: true,
        ..Edges::NONE
    };
    h.cmd(Command::BeginResize {
        window: w,
        edges: right_edge,
        pointer: Point::new(2600.0, 300.0),
    });
    h.cmd(motion(2700.0, 300.0));
    assert!(h.wm.is_interacting());

    let fx = h.cmd(Command::RemoveOutput { output: out2 });
    assert!(!h.wm.is_interacting(), "the resize ended");
    assert!(
        fx.iter().any(|e| matches!(
            e,
            Effect::Configure { window, states, .. } if *window == w && !states.resizing
        )),
        "the client hears the resize ended: {fx:?}"
    );
    let placed = h.wm.window(w).unwrap().lifecycle.normal_frame();
    assert!(
        placed.x >= USABLE.x && placed.right() <= USABLE.right() && placed.y >= USABLE.y,
        "{placed:?}"
    );
    // the server brings the pointer onto the remaining output, then the button comes up
    h.cmd(motion(1919.0, 300.0));
    h.cmd(Command::PointerRelease);
    h.settle();
    assert_eq!(h.wm.window(w).unwrap().lifecycle.normal_frame(), placed);
    assert_eq!(h.wm.window(w).unwrap().output, OUT);
}

/// A drag cancelled after its output went leaves the window on the remaining output, not where the drag started on the
/// output that went.
#[test]
fn a_drag_cancelled_after_its_output_goes_leaves_the_window_reachable() {
    let mut h = H::new();
    let out2 = OutputId(2);
    h.cmd(Command::AddOutput {
        output: out2,
        rect: Rect::new(1920.0, 0.0, 1280.0, 800.0),
        usable: Rect::new(1920.0, 24.0, 1280.0, 776.0),
    });
    let w = h.map_at("a", Rect::new(2000.0, 100.0, 600.0, 400.0));
    h.cmd(Command::MoveWindowToWorkspace {
        window: w,
        workspace: h.wm.current_workspace(out2).unwrap(),
    });
    h.cmd(Command::BeginMove {
        window: w,
        pointer: Point::new(2100.0, 110.0),
    });
    h.cmd(motion(2300.0, 300.0));
    h.cmd(Command::RemoveOutput { output: out2 });
    h.cmd(Command::CancelInteraction);
    h.settle();
    let g = h.wm.window(w).unwrap().geometry();
    assert!(
        g.x >= USABLE.x && g.right() <= USABLE.right() && g.y >= USABLE.y,
        "{g:?}"
    );
}

/// The last output goes and a different one comes: the windows left without an output come onto it inside its usable
/// area, a full-screen one as a normal window.
#[test]
fn windows_left_without_an_output_come_onto_the_next_one() {
    let mut h = H::new();
    let a = h.map_at("a", Rect::new(1500.0, 700.0, 600.0, 400.0));
    let f = h.map_at("b", Rect::new(100.0, 100.0, 600.0, 400.0));
    h.cmd(Command::FocusWindow { window: f });
    fullscreen(&mut h, f, Requester::User);
    h.settle();
    h.cmd(Command::RemoveOutput { output: OUT });
    h.settle();
    let next = OutputId(7);
    let usable = Rect::new(0.0, 24.0, 1280.0, 776.0);
    h.cmd(Command::AddOutput {
        output: next,
        rect: Rect::new(0.0, 0.0, 1280.0, 800.0),
        usable,
    });
    h.settle();
    for w in [a, f] {
        assert_eq!(h.wm.window(w).unwrap().output, next, "{w:?}");
        let g = h.wm.window(w).unwrap().geometry();
        assert!(
            g.x >= usable.x && g.right() <= usable.right() && g.y >= usable.y,
            "{w:?} at {g:?}"
        );
        assert!(h.wm.visible_windows(next).contains(&w), "{w:?}");
    }
    assert_eq!(h.state(f), LifecycleState::Normal);
}
