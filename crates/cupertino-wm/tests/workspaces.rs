//! Workspaces: per-output lists, switching, gestures, window and app assignment, full-screen workspaces.

use crate::common;

use common::*;
use cupertino_wm::{Assignment, Command, Effect, Requester, WorkspaceId, WorkspaceKind};

fn first(wm: &cupertino_wm::Wm) -> WorkspaceId {
    wm.workspaces(OUT)[0].id
}

fn add(wm: &mut cupertino_wm::Wm) -> WorkspaceId {
    let fx = wm.handle(Command::AddWorkspace { output: OUT }, 1.0);
    fx.iter()
        .find_map(|e| match e {
            Effect::WorkspaceCreated { workspace, .. } => Some(*workspace),
            _ => None,
        })
        .expect("a workspace is created")
}

#[test]
fn a_new_output_has_one_current_workspace_and_added_ones_append() {
    let wm = common::wm();
    assert_eq!(wm.workspaces(OUT).len(), 1);
    assert_eq!(wm.current_workspace(OUT), Some(first(&wm)));
    assert_eq!(wm.workspace_position(OUT, 0.0), 0.0);
    assert!(wm.workspaces(cupertino_wm::OutputId(9)).is_empty());
    assert_eq!(wm.current_workspace(cupertino_wm::OutputId(9)), None);
    let mut wm = common::wm();
    let fx = wm.handle(Command::AddWorkspace { output: OUT }, 1.0);
    assert!(has(&fx, |e| matches!(
        e,
        Effect::WorkspaceCreated { index: 1, .. }
    )));
    add(&mut wm);
    assert_eq!(wm.workspaces(OUT).len(), 3);
    assert!(wm
        .workspaces(OUT)
        .iter()
        .all(|w| w.kind == WorkspaceKind::Regular));
    assert_eq!(
        wm.current_workspace(OUT),
        Some(first(&wm)),
        "adding does not switch"
    );
}

#[test]
fn switching_slides_to_another_workspace_and_ignores_the_current_or_an_unknown_one() {
    let mut wm = common::wm();
    let b = add(&mut wm);
    let a = first(&wm);
    let fx = wm.handle(
        Command::SwitchWorkspace {
            output: OUT,
            workspace: b,
        },
        2.0,
    );
    assert!(has(
        &fx,
        |e| matches!(e, Effect::WorkspaceSwitched { from, to, .. } if *from == a && *to == b)
    ));
    assert_eq!(wm.current_workspace(OUT), Some(b));
    let mid = wm.workspace_position(OUT, 2.05);
    assert!(mid > 0.0 && mid < 1.0, "mid-slide position {mid}");
    wm.tick(10.0);
    assert!((wm.workspace_position(OUT, 10.0) - 1.0).abs() < 1e-6);
    let mut wm = common::wm();
    let a = first(&wm);
    let fx = wm.handle(
        Command::SwitchWorkspace {
            output: OUT,
            workspace: a,
        },
        2.0,
    );
    assert!(!has(&fx, |e| matches!(e, Effect::WorkspaceSwitched { .. })));
    let fx = wm.handle(
        Command::SwitchWorkspace {
            output: OUT,
            workspace: WorkspaceId(999),
        },
        2.0,
    );
    assert!(fx.is_empty());
    assert_eq!(wm.current_workspace(OUT), Some(a));
}

#[test]
fn relative_switching_clamps_at_both_ends() {
    let mut wm = common::wm();
    let a = first(&wm);
    let b = add(&mut wm);
    wm.handle(
        Command::SwitchWorkspaceRelative {
            output: OUT,
            delta: -1,
        },
        2.0,
    );
    assert_eq!(wm.current_workspace(OUT), Some(a));
    wm.handle(
        Command::SwitchWorkspaceRelative {
            output: OUT,
            delta: 5,
        },
        3.0,
    );
    assert_eq!(wm.current_workspace(OUT), Some(b));
    wm.handle(
        Command::SwitchWorkspaceRelative {
            output: OUT,
            delta: -1,
        },
        4.0,
    );
    assert_eq!(wm.current_workspace(OUT), Some(a));
}

#[test]
fn a_window_stays_on_its_workspace_and_leaves_the_screen_on_a_switch_or_a_move() {
    let mut wm = common::wm();
    open(&mut wm, 1, "a", 1.0);
    let a = first(&wm);
    let b = add(&mut wm);
    assert_eq!(wm.window(win(1)).unwrap().workspace, a);
    wm.handle(
        Command::SwitchWorkspace {
            output: OUT,
            workspace: b,
        },
        5.0,
    );
    assert!(!wm.window_on_screen(win(1)));
    assert_eq!(wm.windows_in_workspace(a), [win(1)]);
    assert!(wm.windows_in_workspace(b).is_empty());
    assert_eq!(wm.key_window(), None, "nothing focusable is left");
}

#[test]
fn switching_refocuses_a_window_of_the_new_workspace() {
    let mut wm = common::wm();
    open(&mut wm, 1, "a", 1.0);
    let b = add(&mut wm);
    open(&mut wm, 2, "b", 2.0);
    wm.handle(
        Command::MoveWindowToWorkspace {
            window: win(2),
            workspace: b,
        },
        3.0,
    );
    assert_eq!(
        wm.key_window(),
        None,
        "the keyboard only passes to another window of the same app"
    );
    wm.handle(
        Command::SwitchWorkspace {
            output: OUT,
            workspace: b,
        },
        4.0,
    );
    assert_eq!(wm.key_window(), Some(win(2)));
    let a = first(&wm);
    wm.handle(
        Command::SwitchWorkspace {
            output: OUT,
            workspace: a,
        },
        8.0,
    );
    assert_eq!(wm.key_window(), Some(win(1)));
    let mut wm = common::wm();
    open(&mut wm, 1, "a", 1.0);
    let b = add(&mut wm);
    wm.handle(
        Command::MoveWindowToWorkspace {
            window: win(1),
            workspace: b,
        },
        3.0,
    );
    assert_eq!(wm.window(win(1)).unwrap().workspace, b);
    assert!(!wm.window_on_screen(win(1)));
    assert_eq!(wm.key_window(), None);
}

#[test]
fn a_sticky_window_is_on_every_workspace() {
    let mut wm = common::wm();
    open(&mut wm, 1, "a", 1.0);
    let a = first(&wm);
    let b = add(&mut wm);
    wm.handle(
        Command::SetSticky {
            window: win(1),
            sticky: true,
        },
        2.0,
    );
    wm.handle(
        Command::SwitchWorkspace {
            output: OUT,
            workspace: b,
        },
        3.0,
    );
    assert!(wm.window_on_screen(win(1)));
    assert_eq!(wm.key_window(), Some(win(1)));
    wm.handle(
        Command::SetSticky {
            window: win(1),
            sticky: false,
        },
        4.0,
    );
    assert!(
        !wm.window_on_screen(win(1)),
        "now it belongs to its own workspace only"
    );
    wm.handle(
        Command::SwitchWorkspace {
            output: OUT,
            workspace: a,
        },
        5.0,
    );
    assert!(wm.window_on_screen(win(1)));
}

#[test]
fn assigning_an_app_to_all_workspaces_makes_its_windows_sticky_and_unassigning_undoes_only_that() {
    let mut wm = common::wm();
    open(&mut wm, 1, "a", 1.0);
    open(&mut wm, 2, "b", 2.0);
    wm.handle(
        Command::SetSticky {
            window: win(2),
            sticky: true,
        },
        2.5,
    );
    let app = app_of(&wm, 1);
    wm.handle(
        Command::AssignApp {
            app,
            assignment: Assignment::AllWorkspaces,
        },
        3.0,
    );
    assert!(wm.window(win(1)).unwrap().sticky);
    wm.handle(
        Command::AssignApp {
            app,
            assignment: Assignment::None,
        },
        4.0,
    );
    assert!(!wm.window(win(1)).unwrap().sticky);
    assert!(
        wm.window(win(2)).unwrap().sticky,
        "another app's explicit sticky window is untouched"
    );
}

#[test]
fn removing_a_workspace_moves_its_windows_and_assignments_away_but_the_last_one_stays() {
    let mut wm = common::wm();
    open(&mut wm, 1, "a", 1.0);
    let b = add(&mut wm);
    let c = add(&mut wm);
    wm.handle(
        Command::MoveWindowToWorkspace {
            window: win(1),
            workspace: c,
        },
        2.0,
    );
    let fx = wm.handle(Command::RemoveWorkspace { workspace: c }, 3.0);
    assert!(has(
        &fx,
        |e| matches!(e, Effect::WorkspaceRemoved { workspace, .. } if *workspace == c)
    ));
    assert_eq!(wm.window(win(1)).unwrap().workspace, b);
    assert_eq!(wm.workspaces(OUT).len(), 2);
    let mut wm = common::wm();
    open(&mut wm, 1, "a", 1.0);
    let a = first(&wm);
    let b = add(&mut wm);
    wm.handle(Command::RemoveWorkspace { workspace: a }, 3.0);
    assert_eq!(wm.window(win(1)).unwrap().workspace, b);
    assert_eq!(wm.current_workspace(OUT), Some(b));
    assert_eq!(wm.workspaces(OUT).len(), 1);
    let mut wm = common::wm();
    let a = first(&wm);
    let fx = wm.handle(Command::RemoveWorkspace { workspace: a }, 1.0);
    assert!(fx.is_empty());
    assert_eq!(wm.workspaces(OUT).len(), 1);
    let mut wm = common::wm();
    open(&mut wm, 1, "a", 1.0);
    let b = add(&mut wm);
    let app = app_of(&wm, 1);
    wm.handle(
        Command::AssignApp {
            app,
            assignment: Assignment::Workspace(b),
        },
        2.0,
    );
    wm.handle(Command::RemoveWorkspace { workspace: b }, 3.0);
    assert_eq!(wm.app(app).unwrap().assignment, Assignment::None);
}

#[test]
fn moving_a_workspace_reorders_the_list_and_clamps_the_index() {
    let mut wm = common::wm();
    let a = first(&wm);
    let b = add(&mut wm);
    let c = add(&mut wm);
    let fx = wm.handle(
        Command::MoveWorkspace {
            workspace: a,
            index: 99,
        },
        2.0,
    );
    assert!(has(&fx, |e| matches!(
        e,
        Effect::WorkspacesReordered { .. }
    )));
    let ids: Vec<_> = wm.workspaces(OUT).iter().map(|w| w.id).collect();
    assert_eq!(ids, [b, c, a]);
    assert_eq!(
        wm.current_workspace(OUT),
        Some(a),
        "the current workspace follows its move"
    );
    assert_eq!(
        wm.workspace_position(OUT, 2.0),
        2.0,
        "nothing moves on screen"
    );
    let fx = wm.handle(
        Command::MoveWorkspace {
            workspace: a,
            index: 2,
        },
        3.0,
    );
    assert!(
        !has(&fx, |e| matches!(e, Effect::WorkspacesReordered { .. })),
        "same place"
    );
}

#[test]
fn a_full_screen_window_gets_its_own_workspace_after_the_current_one_and_gives_it_up_on_exit() {
    let mut wm = common::wm();
    open(&mut wm, 1, "a", 1.0);
    let a = first(&wm);
    let b = add(&mut wm);
    wm.handle(
        Command::Fullscreen {
            window: win(1),
            output: None,
            requested_by: Requester::User,
        },
        3.0,
    );
    settle(&mut wm, 3.0);
    let list: Vec<_> = wm.workspaces(OUT).to_vec();
    assert_eq!(list.len(), 3);
    assert_eq!(list[0].id, a);
    assert!(
        matches!(list[1].kind, WorkspaceKind::FullScreen { window, origin } if window == win(1) && origin == a)
    );
    assert_eq!(list[2].id, b);
    assert_eq!(wm.current_workspace(OUT), Some(list[1].id));

    wm.handle(Command::ExitFullscreen { window: win(1) }, 20.0);
    settle(&mut wm, 20.0);
    assert_eq!(wm.current_workspace(OUT), Some(a));
    assert_eq!(wm.window(win(1)).unwrap().workspace, a);
    assert_eq!(
        wm.workspaces(OUT).len(),
        2,
        "the full-screen workspace is removed after the slide"
    );
}

#[test]
fn a_swipe_follows_the_fingers_and_commits_only_past_the_half_way_point() {
    let mut wm = common::wm();
    let b = add(&mut wm);
    wm.handle(Command::WorkspaceGestureBegin { output: OUT }, 1.0);
    wm.handle(
        Command::WorkspaceGestureUpdate {
            output: OUT,
            fraction: -0.7,
        },
        1.01,
    );
    assert!((wm.workspace_position(OUT, 1.01) - 0.7).abs() < 0.05);
    wm.handle(
        Command::WorkspaceGestureUpdate {
            output: OUT,
            fraction: -0.7,
        },
        1.5,
    );
    let fx = wm.handle(Command::WorkspaceGestureEnd { output: OUT }, 1.6);
    assert!(has(
        &fx,
        |e| matches!(e, Effect::WorkspaceSwitched { to, .. } if *to == b)
    ));
    wm.tick(10.0);
    assert!((wm.workspace_position(OUT, 10.0) - 1.0).abs() < 1e-6);
    let mut wm = common::wm();
    let a = first(&wm);
    add(&mut wm);
    wm.handle(Command::WorkspaceGestureBegin { output: OUT }, 1.0);
    wm.handle(
        Command::WorkspaceGestureUpdate {
            output: OUT,
            fraction: -0.2,
        },
        1.1,
    );
    wm.handle(
        Command::WorkspaceGestureUpdate {
            output: OUT,
            fraction: -0.2,
        },
        2.0,
    );
    let fx = wm.handle(Command::WorkspaceGestureEnd { output: OUT }, 2.1);
    assert!(!has(&fx, |e| matches!(e, Effect::WorkspaceSwitched { .. })));
    assert_eq!(wm.current_workspace(OUT), Some(a));
    wm.tick(10.0);
    assert!(wm.workspace_position(OUT, 10.0).abs() < 1e-6);
}

#[test]
fn a_swipe_past_the_ends_rubber_bands_and_stays_bounded() {
    let mut wm = common::wm();
    add(&mut wm);
    wm.handle(Command::WorkspaceGestureBegin { output: OUT }, 1.0);
    wm.handle(
        Command::WorkspaceGestureUpdate {
            output: OUT,
            fraction: 5.0,
        },
        1.1,
    );
    let p = wm.workspace_position(OUT, 1.1);
    assert!(
        p < 0.0 && p > -1.0,
        "pulled before the first workspace but resisting: {p}"
    );
    wm.handle(
        Command::WorkspaceGestureUpdate {
            output: OUT,
            fraction: 50.0,
        },
        1.2,
    );
    assert!(wm.workspace_position(OUT, 1.2) > -1.0);
}

#[test]
fn a_swipe_moves_at_most_one_workspace_and_needs_a_begin() {
    let mut wm = common::wm();
    add(&mut wm);
    let c = add(&mut wm);
    wm.handle(Command::WorkspaceGestureBegin { output: OUT }, 1.0);
    wm.handle(
        Command::WorkspaceGestureUpdate {
            output: OUT,
            fraction: -1.9,
        },
        1.01,
    );
    wm.handle(
        Command::WorkspaceGestureUpdate {
            output: OUT,
            fraction: -1.9,
        },
        1.5,
    );
    wm.handle(Command::WorkspaceGestureEnd { output: OUT }, 1.6);
    assert_ne!(wm.current_workspace(OUT), Some(c));
    let at = wm
        .workspaces(OUT)
        .iter()
        .position(|w| Some(w.id) == wm.current_workspace(OUT));
    assert_eq!(at, Some(1));
    let mut wm = common::wm();
    add(&mut wm);
    wm.handle(
        Command::WorkspaceGestureUpdate {
            output: OUT,
            fraction: -0.9,
        },
        1.0,
    );
    let fx = wm.handle(Command::WorkspaceGestureEnd { output: OUT }, 1.1);
    assert!(fx.is_empty());
    assert_eq!(wm.workspace_position(OUT, 1.1), 0.0);
}
