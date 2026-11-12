use crate::common;
use common::*;
use cupertino_effects::{point_in_x, settle_time, SpaceSlideState};
use cupertino_wm::*;

#[test]
fn output_starts_with_one_workspace_and_add_appends() {
    let mut h = H::new();
    assert_eq!(h.ws().len(), 1);
    let fx = h.cmd(Command::AddWorkspace { output: OUT });
    assert!(has(&fx, |e| matches!(
        e,
        Effect::WorkspaceCreated { index: 1, .. }
    )));
    assert_eq!(h.ws().len(), 2);
    assert_eq!(h.wm.current_workspace(OUT), Some(h.ws()[0]));
}

#[test]
fn switch_slides_position_and_changes_visible_windows() {
    let mut h = H::new();
    h.cmd(Command::AddWorkspace { output: OUT });
    let ids = h.ws();
    let w0 = h.map("a");
    h.cmd(Command::SwitchWorkspace {
        output: OUT,
        workspace: ids[1],
    });
    assert_eq!(h.wm.visible_windows(OUT), Vec::<WindowId>::new());
    assert_eq!(
        h.wm.key_window(),
        None,
        "nothing to focus on the empty workspace"
    );
    let w1 = h.map("b");
    assert_eq!(h.wm.visible_windows(OUT), vec![w1]);
    assert_eq!(h.wm.windows_in_workspace(ids[0]), vec![w0]);

    h.cmd(Command::SwitchWorkspace {
        output: OUT,
        workspace: ids[0],
    });
    let p = h.wm.workspace_position(OUT, h.now);
    assert!((p - 1.0).abs() < 1e-9, "starts from the previous position");
    h.advance(0.1);
    let mid = h.wm.workspace_position(OUT, h.now);
    assert!(mid < 1.0 && mid > 0.0);
    h.settle();
    assert_eq!(h.wm.workspace_position(OUT, h.now), 0.0);
    assert_eq!(
        h.wm.key_window(),
        Some(w0),
        "focus follows to the workspace's window"
    );
    assert_eq!(h.wm.front_app(), h.wm.app_for_identity("a"));
}

#[test]
fn programmatic_switch_follows_the_sixty_hz_solver() {
    let mut h = H::new();
    h.cmd(Command::AddWorkspace { output: OUT });
    let ids = h.ws();
    let start = h.now;
    h.cmd(Command::SwitchWorkspace {
        output: OUT,
        workspace: ids[1],
    });
    // x1 = 2 * 1 / 60; v1 = 2; x2 = x1 + (2 * (1 - x1) + 0.695 * 2) / 60
    h.now = start + 1.0 / 60.0 + 1e-9;
    assert!((h.wm.workspace_position(OUT, h.now) - 2.0 / 60.0).abs() < 1e-6);
    h.now = start + 2.0 / 60.0 + 1e-9;
    assert!((h.wm.workspace_position(OUT, h.now) - 0.088722).abs() < 1e-6);
}

#[test]
fn programmatic_switch_rests_within_one_point_of_the_target() {
    let mut h = H::new();
    h.cmd(Command::AddWorkspace { output: OUT });
    let ids = h.ws();
    let start = h.now;
    h.cmd(Command::SwitchWorkspace {
        output: OUT,
        workspace: ids[1],
    });
    let rest = settle_time(SpaceSlideState::at_rest(0.0), 1.0, point_in_x(2, SCREEN.w));
    h.now = start + rest - 0.002;
    assert!(h.wm.wants_tick(h.now));
    h.now = start + rest + 0.002;
    assert!(!h.wm.wants_tick(h.now));
}

#[test]
fn switch_relative_clamps() {
    let mut h = H::new();
    h.cmd(Command::AddWorkspace { output: OUT });
    let ids = h.ws();
    h.cmd(Command::SwitchWorkspaceRelative {
        output: OUT,
        delta: -1,
    });
    assert_eq!(h.wm.current_workspace(OUT), Some(ids[0]));
    h.cmd(Command::SwitchWorkspaceRelative {
        output: OUT,
        delta: 5,
    });
    assert_eq!(h.wm.current_workspace(OUT), Some(ids[1]));
}

#[test]
fn sticky_windows_show_on_every_workspace() {
    let mut h = H::new();
    h.cmd(Command::AddWorkspace { output: OUT });
    let ids = h.ws();
    let w = h.map("a");
    h.cmd(Command::SetSticky {
        window: w,
        sticky: true,
    });
    h.cmd(Command::SwitchWorkspace {
        output: OUT,
        workspace: ids[1],
    });
    assert_eq!(h.wm.visible_windows(OUT), vec![w]);
    assert_eq!(
        h.wm.key_window(),
        Some(w),
        "sticky key window stays focused"
    );
    h.cmd(Command::SetSticky {
        window: w,
        sticky: false,
    });
    assert!(h.wm.visible_windows(OUT).is_empty());
}

#[test]
fn move_window_to_workspace_drops_focus_when_it_leaves_the_screen() {
    let mut h = H::new();
    h.cmd(Command::AddWorkspace { output: OUT });
    let ids = h.ws();
    let a1 = h.map("a");
    let a2 = h.map("a");
    h.cmd(Command::MoveWindowToWorkspace {
        window: a2,
        workspace: ids[1],
    });
    assert_eq!(h.wm.key_window(), Some(a1));
    assert!(!h.wm.window_on_screen(a2));
    assert_eq!(h.wm.windows_in_workspace(ids[1]), vec![a2]);
}

#[test]
fn fullscreen_gets_workspace_after_current_and_is_removed_on_exit() {
    let mut h = H::new();
    h.cmd(Command::AddWorkspace { output: OUT });
    let before = h.ws();
    let w = h.map("a");
    let other = h.map("b");
    let fx = h.cmd(Command::Fullscreen {
        requested_by: Requester::Client,
        window: w,
        output: None,
    });
    assert!(has(
        &fx,
        |e| matches!(e, Effect::Configure { states, size, .. } if states.fullscreen && *size == Size::new(1920.0, 1080.0))
    ));
    assert!(has(&fx, |e| matches!(
        e,
        Effect::WorkspaceCreated { index: 1, .. }
    )));
    let now_ws = h.ws();
    assert_eq!(now_ws.len(), 3);
    assert_eq!(now_ws[0], before[0]);
    assert_eq!(
        now_ws[2], before[1],
        "inserted right after the origin workspace"
    );
    let fs = now_ws[1];
    assert!(
        matches!(h.wm.workspaces(OUT)[1].kind, WorkspaceKind::FullScreen { window, origin } if window == w && origin == before[0])
    );
    assert_eq!(h.wm.current_workspace(OUT), Some(fs));
    assert_eq!(h.wm.window(w).unwrap().workspace, fs);
    assert_eq!(h.wm.visible_windows(OUT), vec![w]);
    h.settle();
    assert_eq!(h.state(w), LifecycleState::FullScreen);
    assert_eq!(h.wm.workspace_position(OUT, h.now), 1.0);

    let fx = h.cmd(Command::ExitFullscreen { window: w });
    assert!(has(
        &fx,
        |e| matches!(e, Effect::WorkspaceSwitched { to, .. } if *to == before[0])
    ));
    assert_eq!(h.ws().len(), 3, "workspace stays until the slide settles");
    assert_eq!(h.wm.current_workspace(OUT), Some(before[0]));
    let fx = h.settle();
    assert!(has(
        &fx,
        |e| matches!(e, Effect::WorkspaceRemoved { workspace, .. } if *workspace == fs)
    ));
    assert_eq!(h.ws(), before);
    assert_eq!(h.wm.window(w).unwrap().workspace, before[0]);
    assert_eq!(h.state(w), LifecycleState::Normal);
    assert_eq!(h.wm.workspace_position(OUT, h.now), 0.0);
    let visible = h.wm.visible_windows(OUT);
    assert_eq!(visible.last(), Some(&w));
    assert!(visible.contains(&other));
    assert_eq!(h.wm.key_window(), Some(w));
}

#[test]
fn a_full_screen_window_covers_its_output_only_while_it_is_in_front() {
    let mut h = H::new();
    let w = h.map("a");
    let other = h.map("b");
    h.cmd(Command::Fullscreen {
        requested_by: Requester::Client,
        window: w,
        output: None,
    });
    assert_eq!(h.wm.fullscreen_front(OUT), None, "still entering");
    h.settle();
    assert_eq!(h.wm.fullscreen_front(OUT), Some(w));
    h.cmd(Command::FocusWindow { window: other });
    h.settle();
    assert_eq!(h.wm.fullscreen_front(OUT), None, "another window in front");
    h.cmd(Command::FocusWindow { window: w });
    h.settle();
    assert_eq!(h.wm.fullscreen_front(OUT), Some(w));
    h.cmd(Command::ExitFullscreen { window: w });
    h.settle();
    assert_eq!(h.wm.fullscreen_front(OUT), None);
}

#[test]
fn fullscreen_window_closing_removes_its_workspace_and_returns() {
    let mut h = H::new();
    let a = h.map("a");
    let w = h.map("b");
    h.cmd(Command::Fullscreen {
        requested_by: Requester::Client,
        window: w,
        output: None,
    });
    h.settle();
    assert_eq!(h.ws().len(), 2);
    h.cmd(Command::Unmap { window: w });
    assert_eq!(h.wm.current_workspace(OUT), Some(h.ws()[0]));
    h.settle();
    assert_eq!(h.ws().len(), 1);
    assert_eq!(
        h.wm.key_window(),
        None,
        "closing the key window leaves the app front without a key window"
    );
    assert_eq!(h.wm.front_app(), h.wm.app_for_identity("b"));
    h.cmd(Command::ActivateApp { app: h.app("a") });
    assert_eq!(h.wm.key_window(), Some(a));
}

#[test]
fn exit_fullscreen_while_on_another_workspace_removes_it_immediately() {
    let mut h = H::new();
    let w = h.map("a");
    h.cmd(Command::Fullscreen {
        requested_by: Requester::Client,
        window: w,
        output: None,
    });
    h.settle();
    let origin = h.ws()[0];
    h.cmd(Command::SwitchWorkspace {
        output: OUT,
        workspace: origin,
    });
    h.settle();
    h.cmd(Command::ExitFullscreen { window: w });
    h.settle();
    assert_eq!(h.ws().len(), 1);
    assert_eq!(h.wm.window(w).unwrap().workspace, origin);
}

#[test]
fn fullscreen_reentry_during_exit_reuses_the_workspace() {
    let mut h = H::new();
    let w = h.map("a");
    h.cmd(Command::Fullscreen {
        requested_by: Requester::Client,
        window: w,
        output: None,
    });
    h.settle();
    h.cmd(Command::ExitFullscreen { window: w });
    h.advance(0.1);
    h.cmd(Command::Fullscreen {
        requested_by: Requester::Client,
        window: w,
        output: None,
    });
    h.settle();
    assert_eq!(h.ws().len(), 2);
    assert_eq!(h.state(w), LifecycleState::FullScreen);
    assert_eq!(h.wm.current_workspace(OUT), Some(h.ws()[1]));
}

#[test]
fn two_fullscreen_apps_keep_order() {
    let mut h = H::new();
    let a = h.map("a");
    let b = h.map("b");
    h.cmd(Command::Fullscreen {
        requested_by: Requester::Client,
        window: a,
        output: None,
    });
    h.settle();
    let origin = h.ws()[0];
    h.cmd(Command::SwitchWorkspace {
        output: OUT,
        workspace: origin,
    });
    h.settle();
    h.cmd(Command::Fullscreen {
        requested_by: Requester::Client,
        window: b,
        output: None,
    });
    h.settle();
    let ws = h.wm.workspaces(OUT);
    assert_eq!(ws.len(), 3);
    assert!(matches!(ws[1].kind, WorkspaceKind::FullScreen { window, .. } if window == b));
    assert!(matches!(ws[2].kind, WorkspaceKind::FullScreen { window, .. } if window == a));
}

#[test]
fn app_assignment_all_workspaces_makes_new_and_existing_windows_sticky() {
    let mut h = H::new();
    h.cmd(Command::AddWorkspace { output: OUT });
    let ids = h.ws();
    let w1 = h.map("chat");
    let app = h.app("chat");
    h.cmd(Command::AssignApp {
        app,
        assignment: Assignment::AllWorkspaces,
    });
    assert!(h.wm.window(w1).unwrap().sticky);
    h.cmd(Command::SwitchWorkspace {
        output: OUT,
        workspace: ids[1],
    });
    let w2 = h.map("chat");
    assert!(h.wm.window(w2).unwrap().sticky);
    assert_eq!(h.wm.visible_windows(OUT).len(), 2);
    h.cmd(Command::AssignApp {
        app,
        assignment: Assignment::None,
    });
    assert!(!h.wm.window(w1).unwrap().sticky && !h.wm.window(w2).unwrap().sticky);
}

#[test]
fn app_assignment_to_workspace_opens_there_and_switches() {
    let mut h = H::new();
    h.cmd(Command::AddWorkspace { output: OUT });
    let ids = h.ws();
    let _w = h.map("a");
    h.cmd(Command::RegisterApp {
        identity: "mail".into(),
    });
    let mail = h.app("mail");
    h.cmd(Command::AssignApp {
        app: mail,
        assignment: Assignment::Workspace(ids[1]),
    });
    let m = h.map("mail");
    assert_eq!(h.wm.window(m).unwrap().workspace, ids[1]);
    assert_eq!(
        h.wm.current_workspace(OUT),
        Some(ids[1]),
        "activation switches to the assigned workspace"
    );
}

#[test]
fn assignment_to_workspace_without_auto_switch_pulls_window_to_current() {
    let mut cfg = WmConfig::default();
    cfg.workspaces.switch_on_activate = false;
    let mut h = H::with_config(cfg);
    h.cmd(Command::AddWorkspace { output: OUT });
    let ids = h.ws();
    h.cmd(Command::RegisterApp {
        identity: "mail".into(),
    });
    let mail = h.app("mail");
    h.cmd(Command::AssignApp {
        app: mail,
        assignment: Assignment::Workspace(ids[1]),
    });
    let m = h.map("mail");
    assert_eq!(h.wm.current_workspace(OUT), Some(ids[0]));
    assert_eq!(h.wm.window(m).unwrap().workspace, ids[0]);
}

#[test]
fn activating_app_switches_to_workspace_with_its_windows() {
    let mut h = H::new();
    h.cmd(Command::AddWorkspace { output: OUT });
    let ids = h.ws();
    let a = h.map("a");
    h.cmd(Command::SwitchWorkspace {
        output: OUT,
        workspace: ids[1],
    });
    let _b = h.map("b");
    let app_a = h.app("a");
    h.cmd(Command::ActivateApp { app: app_a });
    assert_eq!(h.wm.current_workspace(OUT), Some(ids[0]));
    assert_eq!(h.wm.key_window(), Some(a));
}

#[test]
fn activating_app_stays_put_when_auto_switch_disabled() {
    let mut cfg = WmConfig::default();
    cfg.workspaces.switch_on_activate = false;
    let mut h = H::with_config(cfg);
    h.cmd(Command::AddWorkspace { output: OUT });
    let ids = h.ws();
    let _a = h.map("a");
    h.cmd(Command::SwitchWorkspace {
        output: OUT,
        workspace: ids[1],
    });
    let app_a = h.app("a");
    h.cmd(Command::ActivateApp { app: app_a });
    assert_eq!(h.wm.current_workspace(OUT), Some(ids[1]));
    assert_eq!(h.wm.key_window(), None);
}

#[test]
fn removing_a_workspace_moves_windows_to_previous_and_keeps_last() {
    let mut h = H::new();
    h.cmd(Command::AddWorkspace { output: OUT });
    h.cmd(Command::AddWorkspace { output: OUT });
    let ids = h.ws();
    h.cmd(Command::SwitchWorkspace {
        output: OUT,
        workspace: ids[1],
    });
    let w = h.map("a");
    h.cmd(Command::RemoveWorkspace { workspace: ids[1] });
    assert_eq!(h.ws(), vec![ids[0], ids[2]]);
    assert_eq!(h.wm.window(w).unwrap().workspace, ids[0]);
    assert_eq!(h.wm.current_workspace(OUT), Some(ids[0]));
    h.settle();
    assert_eq!(h.wm.workspace_position(OUT, h.now), 0.0);
    h.cmd(Command::RemoveWorkspace { workspace: ids[2] });
    h.cmd(Command::RemoveWorkspace { workspace: ids[0] });
    assert_eq!(h.ws(), vec![ids[0]], "the last regular workspace stays");
}

#[test]
fn moving_a_workspace_reorders_the_list_and_keeps_the_screen_still() {
    let mut h = H::new();
    h.cmd(Command::AddWorkspace { output: OUT });
    h.cmd(Command::AddWorkspace { output: OUT });
    let ids = h.ws();
    h.cmd(Command::SwitchWorkspace {
        output: OUT,
        workspace: ids[1],
    });
    h.settle();
    let fx = h.cmd(Command::MoveWorkspace {
        workspace: ids[0],
        index: 9,
    });
    assert_eq!(
        h.ws(),
        vec![ids[1], ids[2], ids[0]],
        "an index past the end moves it last"
    );
    assert!(fx
        .iter()
        .any(|e| matches!(e, Effect::WorkspacesReordered { output } if *output == OUT)));
    assert_eq!(h.wm.current_workspace(OUT), Some(ids[1]));
    assert_eq!(
        h.wm.workspace_position(OUT, h.now),
        0.0,
        "the slide position follows the current workspace's new index"
    );
    let fx = h.cmd(Command::MoveWorkspace {
        workspace: ids[2],
        index: 1,
    });
    assert_eq!(h.ws(), vec![ids[1], ids[2], ids[0]]);
    assert!(fx.is_empty(), "a move to its own place changes nothing");
}

#[test]
fn removing_the_first_workspace_moves_windows_to_next() {
    let mut h = H::new();
    h.cmd(Command::AddWorkspace { output: OUT });
    let ids = h.ws();
    let w = h.map("a");
    h.cmd(Command::RemoveWorkspace { workspace: ids[0] });
    assert_eq!(h.wm.window(w).unwrap().workspace, ids[1]);
    h.settle();
    assert_eq!(h.wm.workspace_position(OUT, h.now), 0.0);
    assert_eq!(h.wm.key_window(), Some(w));
}

#[test]
fn mru_reorder_moves_used_workspace_to_front_after_settle() {
    let mut cfg = WmConfig::default();
    cfg.workspaces.mru_reorder = true;
    let mut h = H::with_config(cfg);
    h.cmd(Command::AddWorkspace { output: OUT });
    h.cmd(Command::AddWorkspace { output: OUT });
    let ids = h.ws();
    h.cmd(Command::SwitchWorkspace {
        output: OUT,
        workspace: ids[2],
    });
    assert_eq!(h.ws(), ids, "order is unchanged while sliding");
    let fx = h.settle();
    assert!(has(&fx, |e| matches!(
        e,
        Effect::WorkspacesReordered { .. }
    )));
    assert_eq!(h.ws(), vec![ids[2], ids[0], ids[1]]);
    assert_eq!(h.wm.workspace_position(OUT, h.now), 0.0);
}

#[test]
fn reorder_is_off_by_default() {
    let mut h = H::new();
    h.cmd(Command::AddWorkspace { output: OUT });
    let ids = h.ws();
    h.cmd(Command::SwitchWorkspace {
        output: OUT,
        workspace: ids[1],
    });
    h.settle();
    assert_eq!(h.ws(), ids);
}

fn swipe(h: &mut H, fractions: &[f64], dt: f64) {
    h.cmd(Command::WorkspaceGestureBegin { output: OUT });
    for f in fractions {
        h.now += dt;
        h.cmd(Command::WorkspaceGestureUpdate {
            output: OUT,
            fraction: *f,
        });
    }
}

#[test]
fn gesture_tracks_one_to_one() {
    let mut h = H::new();
    h.cmd(Command::AddWorkspace { output: OUT });
    swipe(&mut h, &[-0.25], 0.016);
    assert!((h.wm.workspace_position(OUT, h.now) - 0.25).abs() < 1e-9);
    assert!(h.wm.wants_tick(h.now));
}

#[test]
fn fast_flick_advances_slow_short_drag_returns() {
    let mut h = H::new();
    h.cmd(Command::AddWorkspace { output: OUT });
    let ids = h.ws();
    swipe(&mut h, &[-0.05, -0.1, -0.15, -0.2, -0.25, -0.3], 0.008);
    let fx = h.cmd(Command::WorkspaceGestureEnd { output: OUT });
    assert!(has(
        &fx,
        |e| matches!(e, Effect::WorkspaceSwitched { to, .. } if *to == ids[1])
    ));
    h.settle();
    assert_eq!(h.wm.workspace_position(OUT, h.now), 1.0);

    swipe(
        &mut h,
        &[
            0.02, 0.04, 0.06, 0.08, 0.1, 0.12, 0.14, 0.16, 0.18, 0.2, 0.22, 0.24, 0.26, 0.28,
        ],
        0.1,
    );
    h.now += 0.5;
    h.wm.handle(
        Command::WorkspaceGestureUpdate {
            output: OUT,
            fraction: 0.28,
        },
        h.now,
    );
    let fx = h.cmd(Command::WorkspaceGestureEnd { output: OUT });
    assert!(
        !has(&fx, |e| matches!(e, Effect::WorkspaceSwitched { .. })),
        "slow short drag returns to the same workspace"
    );
    h.settle();
    assert_eq!(h.wm.workspace_position(OUT, h.now), 1.0);
    assert_eq!(h.wm.current_workspace(OUT), Some(ids[1]));
}

#[test]
fn gesture_rubber_bands_at_the_ends() {
    let mut h = H::new();
    h.cmd(Command::AddWorkspace { output: OUT });
    swipe(&mut h, &[0.5, 1.0, 1.5], 0.016);
    let p = h.wm.workspace_position(OUT, h.now);
    assert!(p < 0.0 && p > -1.0, "overscroll is damped: {p}");
    assert!(p.abs() < 1.5 * 0.55);
    h.now += 0.5;
    let fx = h.cmd(Command::WorkspaceGestureEnd { output: OUT });
    assert!(fx.is_empty() || !has(&fx, |e| matches!(e, Effect::WorkspaceSwitched { .. })));
    h.settle();
    assert_eq!(h.wm.workspace_position(OUT, h.now), 0.0);
    assert_eq!(h.wm.current_workspace(OUT), Some(h.ws()[0]));

    h.cmd(Command::SwitchWorkspaceRelative {
        output: OUT,
        delta: 1,
    });
    h.settle();
    swipe(&mut h, &[-0.5, -1.0], 0.016);
    let p = h.wm.workspace_position(OUT, h.now);
    assert!(p > 1.0 && p < 2.0);
}

#[test]
fn single_swipe_is_limited_to_max_jump() {
    let mut h = H::new();
    for _ in 0..3 {
        h.cmd(Command::AddWorkspace { output: OUT });
    }
    let ids = h.ws();
    swipe(&mut h, &[-0.2, -0.5, -0.9, -1.4, -1.9, -2.4], 0.008);
    h.cmd(Command::WorkspaceGestureEnd { output: OUT });
    assert_eq!(
        h.wm.current_workspace(OUT),
        Some(ids[1]),
        "at most one workspace per swipe by default"
    );
    h.settle();

    let mut cfg = WmConfig::default();
    cfg.workspaces.max_swipe_jump = 3;
    let mut h = H::with_config(cfg);
    for _ in 0..3 {
        h.cmd(Command::AddWorkspace { output: OUT });
    }
    let ids = h.ws();
    swipe(&mut h, &[-0.2, -0.5, -0.9, -1.4, -1.9, -2.4], 0.008);
    h.cmd(Command::WorkspaceGestureEnd { output: OUT });
    assert!(
        h.wm.current_workspace(OUT) != Some(ids[0]) && h.wm.current_workspace(OUT) != Some(ids[1])
    );
}

#[test]
fn gesture_can_interrupt_a_running_switch_without_jump() {
    let mut h = H::new();
    h.cmd(Command::AddWorkspace { output: OUT });
    let ids = h.ws();
    h.cmd(Command::SwitchWorkspace {
        output: OUT,
        workspace: ids[1],
    });
    h.advance(0.1);
    let p = h.wm.workspace_position(OUT, h.now);
    h.cmd(Command::WorkspaceGestureBegin { output: OUT });
    assert!((h.wm.workspace_position(OUT, h.now) - p).abs() < 1e-9);
}

#[test]
fn workspaces_are_per_output() {
    let mut h = H::new();
    let out2 = OutputId(2);
    h.cmd(Command::AddOutput {
        output: out2,
        rect: Rect::new(1920.0, 0.0, 1280.0, 800.0),
        usable: Rect::new(1920.0, 24.0, 1280.0, 776.0),
    });
    h.cmd(Command::AddWorkspace { output: out2 });
    let ids2: Vec<_> = h.wm.workspaces(out2).iter().map(|w| w.id).collect();
    h.cmd(Command::SwitchWorkspace {
        output: out2,
        workspace: ids2[1],
    });
    assert_eq!(h.wm.current_workspace(OUT), Some(h.ws()[0]));
    assert_eq!(h.wm.current_workspace(out2), Some(ids2[1]));
    assert_eq!(h.ws().len(), 1);
    assert_ne!(h.ws()[0], ids2[0]);
}

#[test]
fn removing_output_moves_windows_to_remaining_output() {
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
    assert_eq!(h.wm.window(w).unwrap().output, out2);
    h.cmd(Command::RemoveOutput { output: out2 });
    assert_eq!(h.wm.window(w).unwrap().output, OUT);
    assert!(h.wm.visible_windows(OUT).contains(&w));
    assert!(USABLE.contains(h.wm.window(w).unwrap().geometry().origin()));
}
