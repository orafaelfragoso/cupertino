use crate::common;
use common::*;
use cupertino_wm::*;

#[test]
fn map_activates_app_and_window() {
    let mut h = H::new();
    let a = WindowId(1);
    let fx = h.cmd(Command::Map(MapParams::new(
        a,
        "org.example.edit",
        Rect::new(10.0, 40.0, 500.0, 300.0),
    )));
    assert!(has(
        &fx,
        |e| matches!(e, Effect::AppRegistered { identity, .. } if identity == "org.example.edit")
    ));
    assert!(has(
        &fx,
        |e| matches!(e, Effect::Configure { window, size, .. } if *window == a && *size == Size::new(500.0, 300.0))
    ));
    assert!(has(&fx, |e| matches!(
        e,
        Effect::StartAnimation {
            kind: AnimationKind::Opening,
            ..
        }
    )));
    assert!(has(
        &fx,
        |e| matches!(e, Effect::Focus { window: Some(w) } if *w == a)
    ));
    assert!(has(&fx, |e| matches!(e, Effect::Activate { app: Some(_) })));
    assert_eq!(h.wm.key_window(), Some(a));
    assert_eq!(h.wm.front_app(), h.wm.app_for_identity("org.example.edit"));
    assert_eq!(h.state(a), LifecycleState::Opening);
    h.settle();
    assert_eq!(h.state(a), LifecycleState::Normal);
}

#[test]
fn click_in_other_app_switches_front_app_same_app_does_not() {
    let mut h = H::new();
    let a1 = h.map("a");
    let a2 = h.map("a");
    let b1 = h.map("b");
    let (app_a, app_b) = (h.app("a"), h.app("b"));
    assert_eq!(h.wm.app_mru(), &[app_b, app_a]);

    let fx = h.cmd(Command::FocusWindow { window: a1 });
    assert!(has(
        &fx,
        |e| matches!(e, Effect::Activate { app: Some(a) } if *a == app_a)
    ));
    assert_eq!(h.wm.key_window(), Some(a1));
    assert_eq!(h.wm.app_mru(), &[app_a, app_b]);
    assert_eq!(*h.wm.stacking_order().last().unwrap(), a1);

    let fx = h.cmd(Command::FocusWindow { window: a2 });
    assert!(
        !has(&fx, |e| matches!(e, Effect::Activate { .. })),
        "same app: menu bar does not change"
    );
    assert!(has(
        &fx,
        |e| matches!(e, Effect::Raise { window } if *window == a2)
    ));
    assert!(
        !has(
            &fx,
            |e| matches!(e, Effect::Raise { window } if *window == a1)
        ),
        "only that window is raised"
    );
    assert_eq!(h.wm.app(app_a).unwrap().windows_mru, vec![a2, a1]);
    let _ = b1;
}

#[test]
fn configure_toggles_activated_state() {
    let mut h = H::new();
    let a = h.map("a");
    let b = h.map("b");
    let fx = h.cmd(Command::FocusWindow { window: a });
    let act = |fx: &[Effect], w| {
        fx.iter().find_map(|e| match e {
            Effect::Configure { window, states, .. } if *window == w => Some(states.activated),
            _ => None,
        })
    };
    assert_eq!(act(&fx, a), Some(true));
    assert_eq!(act(&fx, b), Some(false));
}

#[test]
fn closing_key_window_focuses_next_window_of_same_app() {
    let mut h = H::new();
    let a1 = h.map("a");
    let a2 = h.map("a");
    let _b = h.map("b");
    h.cmd(Command::FocusWindow { window: a2 });
    let app_a = h.app("a");
    let fx = h.cmd(Command::Unmap { window: a2 });
    assert!(has(&fx, |e| matches!(
        e,
        Effect::StartAnimation {
            kind: AnimationKind::Close,
            ..
        }
    )));
    assert_eq!(h.wm.key_window(), Some(a1));
    assert_eq!(h.wm.front_app(), Some(app_a));
    let fx = h.settle();
    assert!(has(
        &fx,
        |e| matches!(e, Effect::WindowDestroyed { window } if *window == a2)
    ));
    assert!(h.wm.window(a2).is_none());
    assert_eq!(h.wm.app(app_a).unwrap().windows_mru, vec![a1]);
}

#[test]
fn closing_last_window_keeps_app_front_with_no_key_window() {
    let mut h = H::new();
    let _b = h.map("b");
    let a = h.map("a");
    let app_a = h.app("a");
    let fx = h.cmd(Command::Unmap { window: a });
    assert!(has(&fx, |e| matches!(e, Effect::Focus { window: None })));
    assert_eq!(h.wm.front_app(), Some(app_a));
    assert_eq!(h.wm.key_window(), None);
    h.settle();
    assert_eq!(h.wm.front_app(), Some(app_a));
}

#[test]
fn destroy_is_immediate_even_while_opening() {
    let mut h = H::new();
    let id = WindowId(77);
    h.cmd(Command::Map(MapParams::new(
        id,
        "a",
        Rect::new(0.0, 30.0, 100.0, 100.0),
    )));
    let fx = h.cmd(Command::Destroy { window: id });
    assert!(has(
        &fx,
        |e| matches!(e, Effect::WindowDestroyed { window } if *window == id)
    ));
    assert!(h.wm.window(id).is_none());
    assert_eq!(h.wm.key_window(), None);
}

#[test]
fn hide_app_moves_focus_to_next_mru_app_and_unhide_restores() {
    let mut h = H::new();
    let a = h.map("a");
    let b = h.map("b");
    let c = h.map("c");
    let (app_a, app_b, app_c) = (h.app("a"), h.app("b"), h.app("c"));
    let fx = h.cmd(Command::HideApp { app: app_c });
    assert!(has(
        &fx,
        |e| matches!(e, Effect::SetVisible { window, visible: false } if *window == c)
    ));
    assert!(h.wm.app(app_c).unwrap().hidden);
    assert!(!h.wm.window_on_screen(c));
    assert_eq!(h.wm.front_app(), Some(app_b));
    assert_eq!(h.wm.key_window(), Some(b));
    assert!(!h.wm.focusable(c));

    let fx = h.cmd(Command::UnhideApp { app: app_c });
    assert!(has(
        &fx,
        |e| matches!(e, Effect::SetVisible { window, visible: true } if *window == c)
    ));
    assert_eq!(h.wm.front_app(), Some(app_c));
    assert_eq!(h.wm.key_window(), Some(c));
    let _ = (a, app_a);
}

#[test]
fn hide_others_and_show_all() {
    let mut h = H::new();
    let a = h.map("a");
    let b = h.map("b");
    let c = h.map("c");
    let app_b = h.app("b");
    h.cmd(Command::HideOthers { app: app_b });
    assert!(!h.wm.window_on_screen(a) && !h.wm.window_on_screen(c));
    assert!(h.wm.window_on_screen(b));
    assert_eq!(h.wm.front_app(), Some(app_b));
    assert_eq!(h.wm.key_window(), Some(b));
    h.cmd(Command::ShowAll);
    assert!(h.wm.window_on_screen(a) && h.wm.window_on_screen(c));
    assert_eq!(h.wm.key_window(), Some(b), "show all does not steal focus");
}

#[test]
fn hide_last_visible_app_clears_front() {
    let mut h = H::new();
    let _a = h.map("a");
    let app_a = h.app("a");
    let fx = h.cmd(Command::HideApp { app: app_a });
    assert!(has(&fx, |e| matches!(e, Effect::Activate { app: None })));
    assert_eq!(h.wm.front_app(), None);
    assert_eq!(h.wm.key_window(), None);
}

fn step(step: SwitcherStep, repeat: bool) -> Command {
    Command::SwitcherStep { step, repeat }
}

fn selected(h: &H) -> usize {
    h.wm.switcher().unwrap().selected
}

#[test]
fn app_switcher_open_step_commit() {
    let mut h = H::new();
    let a = h.map("a");
    let _b = h.map("b");
    let c = h.map("c");
    let (app_a, app_b, app_c) = (h.app("a"), h.app("b"), h.app("c"));
    assert_eq!(h.wm.app_mru(), &[app_c, app_b, app_a]);

    let fx = h.cmd(Command::SwitcherOpen { backward: false });
    assert!(has(
        &fx,
        |e| matches!(e, Effect::SwitcherChanged { selected: 1, apps } if apps == &vec![app_c, app_b, app_a])
    ));
    h.advance(SWITCHER_SHOW_DELAY);
    h.cmd(step(SwitcherStep::Next, false));
    assert_eq!(selected(&h), 2);
    h.cmd(step(SwitcherStep::Next, false));
    assert_eq!(selected(&h), 0, "a fresh press wraps once the panel is up");
    h.cmd(step(SwitcherStep::Previous, false));
    assert_eq!(selected(&h), 2);
    assert_eq!(
        h.wm.front_app(),
        Some(app_c),
        "nothing changes until commit"
    );
    let fx = h.cmd(Command::SwitcherCommit);
    assert!(has(&fx, |e| matches!(
        e,
        Effect::SwitcherClosed { committed: true }
    )));
    assert_eq!(h.wm.front_app(), Some(app_a));
    assert_eq!(h.wm.key_window(), Some(a));
    assert!(h.wm.switcher().is_none());
    assert_eq!(h.wm.app_mru(), &[app_a, app_c, app_b]);
    assert!(h.wm.window(c).is_some());
}

#[test]
fn before_the_panel_is_up_tab_steps_clamp_on_the_hidden_counter() {
    let mut h = H::new();
    for app in ["a", "b", "c"] {
        h.map(app);
    }
    h.cmd(Command::SwitcherOpen { backward: false });
    h.cmd(step(SwitcherStep::Next, false));
    assert_eq!(selected(&h), 2);
    assert!(
        h.cmd(step(SwitcherStep::Next, false)).is_empty(),
        "the count passes the end: the selection stays on the last entry"
    );
    h.cmd(step(SwitcherStep::Previous, false));
    assert_eq!(
        selected(&h),
        2,
        "the counter went from 3 to 2, still the last entry"
    );
    h.advance(SWITCHER_SHOW_DELAY);
    h.cmd(step(SwitcherStep::Next, false));
    assert_eq!(selected(&h), 0, "the panel is up: the press wraps");
}

#[test]
fn the_hidden_counter_counts_back_from_the_end_and_zero_follows_the_last_direction() {
    assert_eq!(hidden_index(-1, 4, false), 3);
    assert_eq!(hidden_index(-4, 4, false), 0);
    assert_eq!(hidden_index(-5, 4, false), 0);
    assert_eq!(
        hidden_index(0, 4, false),
        3,
        "Tab then Shift-Tab lands on the last entry"
    );
    assert_eq!(hidden_index(0, 4, true), 1);
    assert_eq!(hidden_index(1, 1, true), 0);
    let mut h = H::new();
    for app in ["a", "b", "c"] {
        h.map(app);
    }
    h.cmd(Command::SwitcherOpen { backward: false });
    h.cmd(step(SwitcherStep::Previous, false));
    assert_eq!(selected(&h), 2);
}

#[test]
fn key_repeats_stop_at_the_ends_and_home_and_end_jump() {
    let mut h = H::new();
    for app in ["a", "b", "c"] {
        h.map(app);
    }
    h.cmd(Command::SwitcherOpen { backward: false });
    h.advance(SWITCHER_SHOW_DELAY);
    h.cmd(step(SwitcherStep::Next, true));
    assert_eq!(selected(&h), 2);
    assert!(
        h.cmd(step(SwitcherStep::Next, true)).is_empty(),
        "a repeat does not wrap"
    );
    h.cmd(step(SwitcherStep::First, false));
    assert_eq!(selected(&h), 0);
    assert!(h.cmd(step(SwitcherStep::Previous, true)).is_empty());
    h.cmd(step(SwitcherStep::Last, false));
    assert_eq!(selected(&h), 2);
}

#[test]
fn applications_without_windows_are_not_listed() {
    let mut h = H::new();
    let a = h.map("a");
    let _b = h.map("b");
    h.cmd(Command::Destroy { window: a });
    h.cmd(Command::SwitcherOpen { backward: false });
    assert_eq!(h.wm.switcher().unwrap().items, vec![h.app("b")]);
    assert_eq!(selected(&h), 0);
}

#[test]
fn q_closes_the_selected_application_and_its_entry_leaves_with_its_last_window() {
    let mut h = H::new();
    let a1 = h.map("a");
    let a2 = h.map("a");
    let _b = h.map("b");
    let c = h.map("c");
    let (app_b, app_c) = (h.app("b"), h.app("c"));
    h.cmd(Command::SwitcherOpen { backward: false });
    h.advance(SWITCHER_SHOW_DELAY);
    h.cmd(step(SwitcherStep::Next, false));
    assert_eq!(selected(&h), 2, "a is the least recent");
    let fx = h.cmd(Command::SwitcherQuit);
    let closes: Vec<WindowId> = fx
        .iter()
        .filter_map(|e| {
            if let Effect::RequestClose { window } = e {
                Some(*window)
            } else {
                None
            }
        })
        .collect();
    assert_eq!(closes, vec![a1, a2]);
    assert!(
        h.cmd(Command::Destroy { window: a1 })
            .iter()
            .all(|e| !matches!(e, Effect::SwitcherChanged { .. })),
        "a window is left"
    );
    let fx = h.cmd(Command::Destroy { window: a2 });
    assert!(
        has(
            &fx,
            |e| matches!(e, Effect::SwitcherChanged { apps, selected: 1 } if apps == &vec![app_c, app_b])
        ),
        "{fx:?}"
    );
    h.cmd(Command::SwitcherCommit);
    assert_eq!(h.wm.front_app(), Some(app_b));
    let _ = c;
}

#[test]
fn q_with_a_single_entry_does_nothing() {
    let mut h = H::new();
    h.map("a");
    h.cmd(Command::SwitcherOpen { backward: false });
    assert!(h.cmd(Command::SwitcherQuit).is_empty());
}

#[test]
fn h_toggles_the_hidden_state_and_keeps_the_switcher() {
    let mut h = H::new();
    let a = h.map("a");
    let _b = h.map("b");
    let app_a = h.app("a");
    h.cmd(Command::SwitcherOpen { backward: false });
    let fx = h.cmd(Command::SwitcherToggleHide);
    assert!(has(
        &fx,
        |e| matches!(e, Effect::SetVisible { window, visible: false } if *window == a)
    ));
    assert!(h.wm.app(app_a).unwrap().hidden);
    assert!(h.wm.switcher().is_some(), "the panel stays");
    let fx = h.cmd(Command::SwitcherToggleHide);
    assert!(has(
        &fx,
        |e| matches!(e, Effect::SetVisible { window, visible: true } if *window == a)
    ));
    assert!(!h.wm.app(app_a).unwrap().hidden);
}

#[test]
fn an_activation_token_is_judged_against_the_last_activation_and_user_event() {
    let mut h = H::new();
    let a = h.map("a");
    let activated = h.now;
    let _b = h.map("b");
    let now = activated + 1.0;
    assert!(
        !h.wm.activation_allowed(a, activated - 1.0, None, activated),
        "older than a's last activation"
    );
    assert!(h.wm.activation_allowed(a, activated + 0.5, None, now));
    assert!(
        !h.wm
            .activation_allowed(a, activated + 0.5, Some(activated + 0.8), now),
        "newer, but the person typed 0.3 s after the token"
    );
    assert!(
        h.wm.activation_allowed(a, activated + 0.5, Some(activated + 0.55), now),
        "a press within the slack of the token"
    );
    assert!(
        !h.wm
            .activation_allowed(a, activated + 0.5, None, activated + 2.6),
        "used 2.1 s after it was made"
    );
    assert!(!h
        .wm
        .activation_allowed(WindowId(99), activated + 0.5, None, now));
}

#[test]
fn attention_is_asked_for_an_inactive_application_only() {
    let mut h = H::new();
    let a = h.map("a");
    let b = h.map("b");
    let fx = h.cmd(Command::DemandAttention { window: a });
    assert_eq!(fx, vec![Effect::AppAttention { app: h.app("a") }]);
    assert!(
        h.cmd(Command::DemandAttention { window: b }).is_empty(),
        "b is active"
    );
}

#[test]
fn app_switcher_select_moves_the_selection_only_to_an_entry() {
    let mut h = H::new();
    let a = h.map("a");
    let _b = h.map("b");
    let _c = h.map("c");
    h.cmd(Command::SwitcherOpen { backward: false });
    assert_eq!(selected(&h), 1);
    let fx = h.cmd(Command::SwitcherSelect { index: 2 });
    assert!(has(&fx, |e| matches!(
        e,
        Effect::SwitcherChanged { selected: 2, .. }
    )));
    assert!(
        h.cmd(Command::SwitcherSelect { index: 2 }).is_empty(),
        "the selected entry again changes nothing"
    );
    assert!(
        h.cmd(Command::SwitcherSelect { index: 3 }).is_empty(),
        "past the last entry changes nothing"
    );
    assert_eq!(h.wm.switcher().unwrap().selected, 2);
    h.cmd(Command::SwitcherCommit);
    assert_eq!(h.wm.key_window(), Some(a));
    assert!(
        h.cmd(Command::SwitcherSelect { index: 0 }).is_empty(),
        "no switcher, nothing to select"
    );
}

#[test]
fn app_switcher_quick_tap_toggles_between_two_apps() {
    let mut h = H::new();
    let a = h.map("a");
    let b = h.map("b");
    h.cmd(Command::SwitcherOpen { backward: false });
    h.cmd(Command::SwitcherCommit);
    assert_eq!(h.wm.key_window(), Some(a));
    h.cmd(Command::SwitcherOpen { backward: false });
    h.cmd(Command::SwitcherCommit);
    assert_eq!(h.wm.key_window(), Some(b));
}

#[test]
fn app_switcher_cancel_and_backward() {
    let mut h = H::new();
    let _a = h.map("a");
    let b = h.map("b");
    let _c = h.map("c");
    let (app_a, app_c) = (h.app("a"), h.app("c"));
    h.cmd(Command::SwitcherOpen { backward: true });
    assert_eq!(h.wm.switcher().unwrap().selected, 2);
    assert_eq!(h.wm.switcher().unwrap().items[2], app_a);
    let fx = h.cmd(Command::SwitcherCancel);
    assert!(has(&fx, |e| matches!(
        e,
        Effect::SwitcherClosed { committed: false }
    )));
    assert_eq!(h.wm.front_app(), Some(app_c));
    assert!(h.wm.switcher().is_none());
    assert!(h.cmd(Command::SwitcherCommit).is_empty());
    let _ = b;
}

#[test]
fn switcher_selects_hidden_app_and_unhides_it() {
    let mut h = H::new();
    let a = h.map("a");
    let _b = h.map("b");
    let app_a = h.app("a");
    h.cmd(Command::HideApp { app: app_a });
    h.cmd(Command::SwitcherOpen { backward: false });
    assert_eq!(h.wm.switcher().unwrap().items.len(), 2);
    h.cmd(Command::SwitcherCommit);
    assert!(!h.wm.app(app_a).unwrap().hidden);
    assert_eq!(h.wm.key_window(), Some(a));
}

#[test]
fn window_cycling_within_app_uses_snapshot_and_updates_mru_at_end() {
    let mut h = H::new();
    let w1 = h.map("a");
    let w2 = h.map("a");
    let w3 = h.map("a");
    let other = h.map("b");
    h.cmd(Command::FocusWindow { window: w3 });
    let app_a = h.app("a");
    assert_eq!(h.wm.app(app_a).unwrap().windows_mru, vec![w3, w2, w1]);

    h.cmd(Command::CycleWindows { backward: false });
    assert_eq!(h.wm.key_window(), Some(w2));
    h.cmd(Command::CycleWindows { backward: false });
    assert_eq!(h.wm.key_window(), Some(w1));
    assert_eq!(
        h.wm.app(app_a).unwrap().windows_mru,
        vec![w3, w2, w1],
        "MRU deferred during the session"
    );
    h.cmd(Command::CycleWindows { backward: false });
    assert_eq!(h.wm.key_window(), Some(w3), "wraps");
    h.cmd(Command::CycleWindows { backward: true });
    assert_eq!(h.wm.key_window(), Some(w1));
    h.cmd(Command::EndCycleWindows);
    assert_eq!(h.wm.app(app_a).unwrap().windows_mru[0], w1);
    h.cmd(Command::CycleWindows { backward: false });
    assert_eq!(h.wm.key_window(), Some(w3), "next in MRU after w1");
    assert_ne!(h.wm.key_window(), Some(other));
    assert_eq!(h.wm.front_app(), Some(app_a));
}

#[test]
fn window_cycling_with_single_window_is_noop() {
    let mut h = H::new();
    let w = h.map("a");
    let fx = h.cmd(Command::CycleWindows { backward: false });
    assert!(fx.is_empty());
    assert_eq!(h.wm.key_window(), Some(w));
}

fn map_modal(h: &mut H, id: u64, parent: WindowId) -> WindowId {
    let mut p = MapParams::new(WindowId(id), "a", Rect::new(0.0, 0.0, 300.0, 200.0));
    p.parent = Some(parent);
    p.modal = true;
    h.cmd(Command::Map(p));
    h.settle();
    WindowId(id)
}

#[test]
fn modal_blocks_parent_focus_and_pulses_dialog() {
    let mut h = H::new();
    let parent = h.map("a");
    let other = h.map("b");
    let dialog = map_modal(&mut h, 50, parent);
    assert_eq!(h.wm.key_window(), Some(dialog));
    assert!(h.wm.is_blocked(parent));
    assert!(!h.wm.is_blocked(dialog));
    assert_eq!(h.wm.blocking_modal(parent), Some(dialog));

    h.cmd(Command::FocusWindow { window: other });
    let fx = h.cmd(Command::FocusWindow { window: parent });
    assert!(has(
        &fx,
        |e| matches!(e, Effect::Attention { window } if *window == dialog)
    ));
    assert_eq!(h.wm.key_window(), Some(dialog));
    assert_eq!(h.wm.front_app(), Some(h.app("a")));
    let top = h.wm.stacking_order();
    assert!(top.iter().position(|w| *w == dialog) > top.iter().position(|w| *w == parent));

    h.cmd(Command::ActivateApp { app: h.app("b") });
    h.cmd(Command::ActivateApp { app: h.app("a") });
    assert_eq!(
        h.wm.key_window(),
        Some(dialog),
        "activation lands on the dialog"
    );

    h.cmd(Command::Unmap { window: dialog });
    assert_eq!(
        h.wm.key_window(),
        Some(parent),
        "focus returns to the parent"
    );
    assert!(!h.wm.is_blocked(parent));
}

#[test]
fn modal_is_sheet_placed_under_parent_titlebar_and_skipped_by_cycling() {
    let mut h = H::new();
    let p1 = h.map_at("a", Rect::new(200.0, 100.0, 600.0, 400.0));
    let p2 = h.map("a");
    let dialog = map_modal(&mut h, 60, p1);
    let g = h.wm.window(dialog).unwrap().geometry();
    assert_eq!(g.cx(), 500.0);
    assert_eq!(g.y, 100.0 + h.wm.config().snap.titlebar_height);

    h.cmd(Command::FocusWindow { window: p2 });
    h.cmd(Command::CycleWindows { backward: false });
    assert_ne!(h.wm.key_window(), Some(p1), "blocked parent is skipped");
    assert_eq!(h.wm.key_window(), Some(dialog));
}

#[test]
fn parent_cycles_and_reparenting_are_rejected() {
    let mut h = H::new();
    let a = h.map("a");
    let b = h.map("a");
    h.cmd(Command::SetParent {
        window: b,
        parent: Some(a),
    });
    h.cmd(Command::SetParent {
        window: a,
        parent: Some(b),
    });
    assert_eq!(h.wm.window(a).unwrap().parent, None);
    assert_eq!(h.wm.window(b).unwrap().parent, Some(a));
    h.cmd(Command::SetModal {
        window: b,
        modal: true,
    });
    assert_eq!(h.wm.blocking_modal(a), Some(b));
}

#[test]
fn child_is_reparented_when_parent_is_destroyed() {
    let mut h = H::new();
    let a = h.map("a");
    let b = h.map("a");
    h.cmd(Command::SetParent {
        window: b,
        parent: Some(a),
    });
    h.cmd(Command::Destroy { window: a });
    assert_eq!(h.wm.window(b).unwrap().parent, None);
}

#[test]
fn restore_flows_out_of_the_retargeted_tile() {
    let mut h = H::new();
    let w = h.map("a");
    h.cmd(Command::Minimize {
        window: w,
        target: Some(Rect::new(900.0, 1000.0, 48.0, 48.0)),
    });
    h.settle();
    let moved = Rect::new(500.0, 1000.0, 48.0, 48.0);
    h.cmd(Command::SetMinimizeTarget {
        window: w,
        target: moved,
    });
    h.cmd(Command::Restore { window: w });
    assert_eq!(h.wm.presentation(w, h.now).unwrap().minimize_target, moved);
    h.settle();
    h.cmd(Command::SetMinimizeTarget {
        window: w,
        target: Rect::new(1.0, 2.0, 3.0, 4.0),
    });
    assert_eq!(
        h.wm.presentation(w, h.now).unwrap().minimize_target,
        moved,
        "a normal window keeps its target"
    );
}

#[test]
fn minimize_key_window_focuses_next_and_restore_returns_exact_geometry() {
    let mut h = H::new();
    let g = Rect::new(150.0, 120.0, 640.0, 480.0);
    let a1 = h.map_at("a", g);
    let a2 = h.map("a");
    h.cmd(Command::FocusWindow { window: a1 });
    let target = Rect::new(900.0, 1000.0, 48.0, 48.0);
    let fx = h.cmd(Command::Minimize {
        window: a1,
        target: Some(target),
    });
    assert!(has(&fx, |e| matches!(
        e,
        Effect::StartAnimation {
            kind: AnimationKind::Minimize,
            ..
        }
    )));
    assert_eq!(h.wm.key_window(), Some(a2));
    assert!(!h.wm.focusable(a1));
    h.settle();
    assert_eq!(h.state(a1), LifecycleState::Minimized);
    let p = h.wm.presentation(a1, h.now).unwrap();
    assert!(!p.visible && p.minimize_target == target);

    let fx = h.cmd(Command::Restore { window: a1 });
    assert!(has(&fx, |e| matches!(
        e,
        Effect::StartAnimation {
            kind: AnimationKind::Restore,
            ..
        }
    )));
    assert_eq!(h.wm.key_window(), Some(a1));
    h.settle();
    assert_eq!(h.state(a1), LifecycleState::Normal);
    assert!(rect_eq(h.frame(a1), g));
}

#[test]
fn minimize_without_target_uses_bottom_centre_fallback() {
    let mut h = H::new();
    let a = h.map("a");
    h.cmd(Command::Minimize {
        window: a,
        target: None,
    });
    let t = h.wm.presentation(a, h.now).unwrap().minimize_target;
    assert_eq!((t.cx(), t.bottom()), (960.0, 1080.0));
}

#[test]
fn activating_app_with_only_minimized_windows_restores_most_recent() {
    let mut h = H::new();
    let a1 = h.map("a");
    let a2 = h.map("a");
    let _b = h.map("b");
    h.cmd(Command::Minimize {
        window: a2,
        target: None,
    });
    h.cmd(Command::Minimize {
        window: a1,
        target: None,
    });
    h.settle();
    let app_a = h.app("a");
    h.cmd(Command::ActivateApp { app: app_a });
    assert_eq!(
        h.state(a2),
        LifecycleState::Restoring,
        "a2 is first in the app's window MRU"
    );
    assert_eq!(h.state(a1), LifecycleState::Minimized);
    assert_eq!(h.wm.key_window(), Some(a2));
}

#[test]
fn activating_app_without_windows_leaves_empty_menu_bar() {
    let mut h = H::new();
    let _a = h.map("a");
    let fx = h.cmd(Command::RegisterApp {
        identity: "files".into(),
    });
    assert!(has(&fx, |e| matches!(e, Effect::AppRegistered { .. })));
    let app = h.app("files");
    h.cmd(Command::ActivateApp { app });
    assert_eq!(h.wm.front_app(), Some(app));
    assert_eq!(h.wm.key_window(), None);
    assert_eq!(h.wm.app_mru()[0], app);
}

#[test]
fn bring_all_to_front_raises_every_window_of_the_app() {
    let mut cfg = WmConfig::default();
    cfg.focus.bring_all_to_front = true;
    let mut h = H::with_config(cfg);
    let a1 = h.map("a");
    let a2 = h.map("a");
    let b = h.map("b");
    let fx = h.cmd(Command::ActivateApp { app: h.app("a") });
    assert!(has(
        &fx,
        |e| matches!(e, Effect::Raise { window } if *window == a1)
    ));
    assert!(has(
        &fx,
        |e| matches!(e, Effect::Raise { window } if *window == a2)
    ));
    assert!(!has(
        &fx,
        |e| matches!(e, Effect::Raise { window } if *window == b)
    ));
    assert_eq!(h.wm.key_window(), Some(a2));
    assert_eq!(h.wm.stacking_order().last(), Some(&a2));
}

#[test]
fn remove_app_destroys_windows_and_moves_focus() {
    let mut h = H::new();
    let a = h.map("a");
    let b = h.map("b");
    let fx = h.cmd(Command::RemoveApp { app: h.app("b") });
    assert!(has(
        &fx,
        |e| matches!(e, Effect::WindowDestroyed { window } if *window == b)
    ));
    assert_eq!(h.wm.front_app(), h.wm.app_for_identity("a"));
    assert_eq!(h.wm.key_window(), Some(a));
    assert!(h.wm.app_for_identity("b").is_none());
}

#[test]
fn title_and_hints_update_and_close_request() {
    let mut h = H::new();
    let a = h.map("a");
    h.cmd(Command::SetTitle {
        window: a,
        title: "Doc".into(),
    });
    h.cmd(Command::SetSizeHints {
        window: a,
        min: Some(Size::new(100.0, 80.0)),
        max: None,
        content: None,
    });
    let w = h.wm.window(a).unwrap();
    assert_eq!(w.title, "Doc");
    assert_eq!(w.hints().min, Some(Size::new(100.0, 80.0)));
    let fx = h.cmd(Command::RequestClose { window: a });
    assert_eq!(fx, vec![Effect::RequestClose { window: a }]);
}

#[test]
fn the_bring_all_to_front_command_keeps_the_order_and_the_focus_and_can_cascade() {
    let mut h = H::new();
    let a1 = h.map_at("a", Rect::new(400.0, 300.0, 600.0, 400.0));
    let a2 = h.map("a");
    let a3 = h.map("a");
    let b = h.map("b");
    h.cmd(Command::FocusWindow { window: a1 });
    assert_eq!(h.wm.stacking_order(), &[a2, a3, b, a1]);

    let app = h.app("a");
    h.cmd(Command::BringAllToFront {
        app,
        cascade: false,
    });
    assert_eq!(h.wm.stacking_order(), &[b, a2, a3, a1]);
    assert_eq!(h.wm.key_window(), Some(a1));

    let origin = h.wm.window(a1).unwrap().geometry();
    h.cmd(Command::BringAllToFront { app, cascade: true });
    h.settle();
    assert_eq!(h.wm.stacking_order(), &[b, a2, a3, a1]);
    assert_eq!(h.wm.key_window(), Some(a1));
    assert!(rect_eq(h.frame(a1), origin), "the key window stays");
    let behind = |n: f64| {
        Rect::new(
            origin.x - CASCADE_STEP * n,
            origin.y - CASCADE_STEP * n,
            600.0,
            400.0,
        )
    };
    assert!(rect_eq(h.frame(a3), behind(1.0)), "{:?}", h.frame(a3));
    assert!(rect_eq(h.frame(a2), behind(2.0)), "{:?}", h.frame(a2));
}

#[test]
fn a_cascade_moves_a_sheet_with_its_parent_instead_of_cascading_it() {
    let mut h = H::new();
    let a1 = h.map_at("a", Rect::new(400.0, 300.0, 600.0, 400.0));
    let a2 = h.map("a");
    let sheet = map_modal(&mut h, 50, a2);
    h.cmd(Command::FocusWindow { window: a1 });
    h.settle();
    let (parent, child) = (h.frame(a2), h.frame(sheet));

    let app = h.app("a");
    h.cmd(Command::BringAllToFront { app, cascade: true });
    h.settle();
    let origin = h.frame(a1);
    let moved = h.frame(a2);
    assert!(
        rect_eq(
            moved,
            Rect::new(
                origin.x - CASCADE_STEP,
                origin.y - CASCADE_STEP,
                600.0,
                400.0
            )
        ),
        "the only other top-level window takes the first step: {moved:?}"
    );
    assert!(
        rect_eq(
            h.frame(sheet),
            child.translate(moved.x - parent.x, moved.y - parent.y)
        ),
        "the sheet keeps its place on the parent: {:?}",
        h.frame(sheet)
    );
}

#[test]
fn a_cascade_from_a_key_sheet_starts_at_its_parent_which_stays() {
    let mut h = H::new();
    let a1 = h.map_at("a", Rect::new(400.0, 300.0, 600.0, 400.0));
    let a2 = h.map("a");
    let sheet = map_modal(&mut h, 50, a1);
    assert_eq!(h.wm.key_window(), Some(sheet));
    let (parent, child) = (h.frame(a1), h.frame(sheet));

    let app = h.app("a");
    h.cmd(Command::BringAllToFront { app, cascade: true });
    h.settle();
    assert!(rect_eq(h.frame(a1), parent), "the key sheet's parent stays");
    assert!(rect_eq(h.frame(sheet), child), "and so does the sheet");
    assert!(
        rect_eq(
            h.frame(a2),
            Rect::new(
                parent.x - CASCADE_STEP,
                parent.y - CASCADE_STEP,
                600.0,
                400.0
            )
        ),
        "{:?}",
        h.frame(a2)
    );
}
