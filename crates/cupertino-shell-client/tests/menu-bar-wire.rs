//! The menu-bar role over a real wire: how a group of events becomes an update, what each version leaves out and the
//! requests of the bar.

mod support;

use cupertino_protocols::server::cupertino_menu_bar_v1::{
    AgentTrust, AppWindowFlags, BoundAction as WireBoundAction, ClientKind, CupertinoMenuBarV1,
    KeyClass as WireKeyClass, ModifierKey, Request, WindowControl,
};
use cupertino_shell_client::{
    ActiveApp, ActiveClient, AppWindow, BoundAction, EditAction, GtkMenu, KeyClass, LayerShell,
    LayerSurfaceConfig, MenuBar, MenuBarUpdate, PrimaryModifier, SessionAction, SessionAgent,
    Shell, ShellEvent, WindowCommand, WindowMenu, WindowState, MAX_BACKDROP_BANDS,
};
use cupertino_shell_client::{Anchor, KeyboardInteractivity, Layer, Margin};
use support::{Offer, Seen, Wire};

/// A bar on the first output; the server sends `first` as the bar's first group.
fn bar_with(
    offer: Offer,
    first: impl Fn(&CupertinoMenuBarV1) + Send + 'static,
) -> (Wire, MenuBar, cupertino_wayland_client::SurfaceId) {
    let mut wire = Wire::new(offer);
    wire.server(|s| s.greet_menu_bar = Some(Box::new(first)));
    let layers = LayerShell::bind(&mut wire.conn).expect("layer shell");
    let shell = Shell::bind(&mut wire.conn).expect("shell");
    let output = wire.conn.outputs()[0].id;
    let surface = layers
        .layer_surface(
            &mut wire.conn,
            &LayerSurfaceConfig {
                namespace: "menubar".into(),
                output: Some(output),
                layer: Layer::Top,
                anchor: Anchor::Top | Anchor::Left | Anchor::Right,
                size: (0, 24),
                margin: Margin::default(),
                exclusive_zone: 24,
                keyboard: KeyboardInteractivity::None,
            },
        )
        .expect("layer surface")
        .surface();
    let bar = shell
        .menu_bar(&wire.conn, surface, output)
        .expect("menu bar");
    (wire, bar, surface)
}

fn updates(wire: &mut Wire) -> Vec<MenuBarUpdate> {
    wire.shell_events()
        .into_iter()
        .filter_map(|e| match e {
            ShellEvent::MenuBar { update, .. } => Some(*update),
            _ => None,
        })
        .collect()
}

fn bar_requests(wire: &Wire) -> Vec<Request> {
    wire.take()
        .into_iter()
        .filter_map(|s| match s {
            Seen::MenuBar(r) => Some(r),
            _ => None,
        })
        .collect()
}

#[test]
fn the_first_group_arrives_with_the_surface_of_the_bar() {
    let (mut wire, _bar, surface) = bar_with(Offer::all(), |bar| {
        bar.active_app("org.example.Edit".into(), "Edit".into());
        bar.menu_none();
        bar.done();
    });
    let events = wire.shell_events();
    assert_eq!(events.len(), 1);
    let ShellEvent::MenuBar {
        surface: of,
        update,
    } = &events[0]
    else {
        panic!("not a menu bar event: {:?}", events[0]);
    };
    assert_eq!(*of, surface);
    assert_eq!(
        update.active_app,
        Some(ActiveApp {
            app_id: "org.example.Edit".into(),
            title: "Edit".into()
        })
    );
    assert_eq!(update.menu, Some(WindowMenu::None));
}

#[test]
fn a_group_with_nothing_in_it_is_not_reported() {
    let (mut wire, _bar, _) = bar_with(Offer::all(), |bar| bar.done());
    assert!(updates(&mut wire).is_empty());
}

#[test]
fn a_field_the_group_does_not_mention_is_none() {
    let (mut wire, _bar, _) = bar_with(Offer::all(), |bar| {
        bar.active_app("a".into(), "A".into());
        bar.done();
    });
    updates(&mut wire);
    wire.server(|s| {
        s.menu_bars[0].screen_capture(1);
        s.menu_bars[0].done();
    });
    let update = updates(&mut wire).remove(0);
    assert_eq!(
        update,
        MenuBarUpdate {
            screen_capture: Some(true),
            ..MenuBarUpdate::default()
        }
    );
}

#[test]
fn the_menus_of_the_active_window_keep_their_addresses() {
    let (mut wire, _bar, _) = bar_with(Offer::all(), |bar| {
        bar.menu_dbusmenu(":1.5".into(), "/menu".into());
        bar.done();
    });
    assert_eq!(
        updates(&mut wire)[0].menu,
        Some(WindowMenu::DbusMenu {
            service: ":1.5".into(),
            path: "/menu".into()
        })
    );
    wire.server(|s| {
        s.menu_bars[0].menu_gtk(
            ":1.9".into(),
            "org.example".into(),
            "/org/example".into(),
            String::new(),
            "/menubar".into(),
            String::new(),
        );
        s.menu_bars[0].done();
    });
    assert_eq!(
        updates(&mut wire)[0].menu,
        Some(WindowMenu::Gtk(GtkMenu {
            bus_name: ":1.9".into(),
            application_id: Some("org.example".into()),
            application_object_path: Some("/org/example".into()),
            window_object_path: None,
            menubar_object_path: Some("/menubar".into()),
            app_menu_object_path: None,
        }))
    );
}

#[test]
fn window_state_takes_the_enabled_controls_and_full_screen_of_its_group() {
    let (mut wire, _bar, _) = bar_with(Offer::all(), |bar| {
        bar.window_state(1, 2, 4);
        bar.window_controls(WindowControl::Minimize);
        bar.window_full_screen(1);
        bar.done();
    });
    let update = updates(&mut wire).remove(0);
    assert_eq!(
        update.window_state,
        Some(WindowState {
            can_minimize: true,
            can_zoom: false,
            full_screen: true,
            workspace: 2,
            workspaces: 4,
        })
    );
    // the window list is complete in every group with a window state, empty when no window is listed
    assert_eq!(update.windows, Some(vec![]));

    wire.server(|s| {
        s.menu_bars[0].window_state(1, 1, 1);
        s.menu_bars[0].done();
    });
    let state = updates(&mut wire).remove(0).window_state.expect("state");
    assert!(state.can_minimize && state.can_zoom && !state.full_screen);
}

#[test]
fn a_window_that_cannot_zoom_stays_unable_whatever_its_controls_say() {
    let (mut wire, _bar, _) = bar_with(Offer::all(), |bar| {
        bar.window_state(0, 1, 1);
        bar.window_controls(WindowControl::Minimize | WindowControl::Zoom);
        bar.done();
    });
    let state = updates(&mut wire).remove(0).window_state.expect("state");
    assert!(state.can_minimize);
    assert!(!state.can_zoom);
}

#[test]
fn the_window_list_carries_the_active_and_minimized_flags() {
    let (mut wire, _bar, _) = bar_with(Offer::all(), |bar| {
        bar.window_state(1, 1, 1);
        bar.app_window(3, "One".into(), AppWindowFlags::Active);
        bar.app_window(4, "Two".into(), AppWindowFlags::Minimized);
        bar.app_window(5, "Three".into(), AppWindowFlags::empty());
        bar.done();
    });
    assert_eq!(
        updates(&mut wire)[0].windows,
        Some(vec![
            AppWindow {
                id: 3,
                title: "One".into(),
                active: true,
                minimized: false
            },
            AppWindow {
                id: 4,
                title: "Two".into(),
                active: false,
                minimized: true
            },
            AppWindow {
                id: 5,
                title: "Three".into(),
                active: false,
                minimized: false
            },
        ])
    );
}

#[test]
fn a_server_without_the_window_list_sends_no_list() {
    let (mut wire, _bar, _) = bar_with(Offer::shell(9), |bar| {
        bar.window_state(1, 1, 1);
        bar.done();
    });
    let update = updates(&mut wire).remove(0);
    assert!(update.window_state.is_some());
    assert_eq!(update.windows, None);
}

#[test]
fn the_flags_of_a_group_are_reported() {
    let (mut wire, _bar, _) = bar_with(Offer::all(), |bar| {
        bar.engage();
        bar.engage_help();
        bar.screen_capture(0);
        bar.done();
    });
    let update = updates(&mut wire).remove(0);
    assert!(update.engage && update.engage_help);
    assert_eq!(update.screen_capture, Some(false));
}

#[test]
fn the_session_agent_trust_maps_to_who_may_answer() {
    let (mut wire, _bar, _) = bar_with(Offer::all(), |bar| {
        bar.session_agent(AgentTrust::Process, 1234);
        bar.done();
    });
    assert_eq!(
        updates(&mut wire)[0].session_agent,
        Some(SessionAgent::Process(1234))
    );
    for (trust, expected) in [
        (AgentTrust::Any, SessionAgent::Any),
        (AgentTrust::None, SessionAgent::None),
    ] {
        wire.server(move |s| {
            s.menu_bars[0].session_agent(trust, 77);
            s.menu_bars[0].done();
        });
        assert_eq!(updates(&mut wire)[0].session_agent, Some(expected));
    }
}

#[test]
fn the_backdrop_lightness_is_at_most_one_thousand() {
    let (mut wire, _bar, _) = bar_with(Offer::all(), |bar| {
        bar.backdrop_luminance(640);
        bar.done();
    });
    assert_eq!(updates(&mut wire)[0].backdrop_lightness, Some(640));
    wire.server(|s| {
        s.menu_bars[0].backdrop_luminance(5000);
        s.menu_bars[0].done();
    });
    assert_eq!(updates(&mut wire)[0].backdrop_lightness, Some(1000));
}

fn bands(count: usize) -> Vec<u8> {
    (0..count)
        .flat_map(|i| {
            let i = i as u16;
            [i, i + 1, 0xff00 | i]
                .into_iter()
                .flat_map(u16::to_le_bytes)
                .collect::<Vec<u8>>()
        })
        .collect()
}

#[test]
fn backdrop_bands_are_little_endian_triples_and_a_partial_band_is_dropped() {
    let mut bytes = bands(3);
    bytes.extend([9, 9, 9]);
    let (mut wire, _bar, _) = bar_with(Offer::all(), move |bar| {
        bar.backdrop_bands(bytes.clone());
        bar.done();
    });
    assert_eq!(
        updates(&mut wire)[0].backdrop_bands,
        Some(vec![[0, 1, 0xff00], [1, 2, 0xff01], [2, 3, 0xff02]])
    );
}

#[test]
fn backdrop_bands_beyond_the_limit_are_dropped() {
    let (mut wire, _bar, _) = bar_with(Offer::all(), |bar| {
        bar.backdrop_bands(bands(MAX_BACKDROP_BANDS + 10));
        bar.done();
    });
    let read = updates(&mut wire)[0].backdrop_bands.clone().expect("bands");
    assert_eq!(read.len(), MAX_BACKDROP_BANDS);
    assert_eq!(
        read[MAX_BACKDROP_BANDS - 1][0],
        (MAX_BACKDROP_BANDS - 1) as u16
    );
}

#[test]
fn shortcuts_are_added_and_an_empty_chord_removes_one() {
    let (mut wire, _bar, _) = bar_with(Offer::all(), |bar| {
        bar.shortcut(WireBoundAction::QuitApp, "Super+q".into());
        bar.shortcut(WireBoundAction::Lock, "Ctrl+Super+q".into());
        bar.primary_modifier(ModifierKey::Ctrl);
        bar.done();
    });
    let shortcuts = updates(&mut wire)[0].shortcuts.clone().expect("shortcuts");
    assert_eq!(shortcuts.chord(BoundAction::QuitApp), Some("Super+q"));
    assert_eq!(shortcuts.chord(BoundAction::Lock), Some("Ctrl+Super+q"));
    assert_eq!(shortcuts.chord(BoundAction::HideApp), None);
    assert_eq!(shortcuts.primary, PrimaryModifier::Ctrl);

    // a group lists the chords it changes: an empty one unbinds, and a group that does not name the modifier reads Super
    wire.server(|s| {
        s.menu_bars[0].shortcut(WireBoundAction::Lock, String::new());
        s.menu_bars[0].done();
    });
    let shortcuts = updates(&mut wire)[0].shortcuts.clone().expect("shortcuts");
    assert!(shortcuts.chords.is_empty());
    assert_eq!(shortcuts.primary, PrimaryModifier::Super);
}

#[test]
fn every_bound_action_has_its_own_entry() {
    let (mut wire, _bar, _) = bar_with(Offer::all(), |bar| {
        for (i, action) in [
            WireBoundAction::CloseWindow,
            WireBoundAction::Minimize,
            WireBoundAction::Zoom,
            WireBoundAction::ToggleFullscreen,
            WireBoundAction::HideApp,
            WireBoundAction::HideOthers,
            WireBoundAction::QuitApp,
            WireBoundAction::TileLeft,
            WireBoundAction::TileRight,
            WireBoundAction::TileFill,
            WireBoundAction::ForceQuit,
            WireBoundAction::Lock,
            WireBoundAction::LogOut,
            WireBoundAction::HelpSearch,
        ]
        .into_iter()
        .enumerate()
        {
            bar.shortcut(action, format!("chord{i}"));
        }
        bar.done();
    });
    let shortcuts = updates(&mut wire)[0].shortcuts.clone().expect("shortcuts");
    assert_eq!(shortcuts.chords.len(), 14);
    assert_eq!(shortcuts.chord(BoundAction::CloseWindow), Some("chord0"));
    assert_eq!(shortcuts.chord(BoundAction::ForceQuit), Some("chord10"));
    assert_eq!(shortcuts.chord(BoundAction::HelpSearch), Some("chord13"));
}

#[test]
fn the_active_client_and_its_key_class_come_together() {
    let (mut wire, _bar, _) = bar_with(Offer::all(), |bar| {
        bar.window_state(1, 1, 1);
        bar.active_client(500, ClientKind::X11);
        bar.active_client_keys(WireKeyClass::Terminal);
        bar.done();
    });
    let update = updates(&mut wire).remove(0);
    assert_eq!(update.active_client, Some(ActiveClient::X11(500)));
    assert_eq!(update.key_class, Some(KeyClass::Terminal));

    wire.server(|s| {
        s.menu_bars[0].active_client(0, ClientKind::Wayland);
        s.menu_bars[0].active_client_keys(WireKeyClass::Toolkit);
        s.menu_bars[0].done();
    });
    let update = updates(&mut wire).remove(0);
    assert_eq!(update.active_client, Some(ActiveClient::Wayland(0)));
    assert_eq!(update.key_class, Some(KeyClass::Toolkit));
}

#[test]
fn a_version_24_server_gets_the_default_key_class() {
    let (mut wire, _bar, _) = bar_with(Offer::shell(24), |bar| {
        bar.active_client(42, ClientKind::Wayland);
        bar.done();
    });
    let update = updates(&mut wire).remove(0);
    assert_eq!(update.active_client, Some(ActiveClient::Wayland(42)));
    assert_eq!(update.key_class, Some(KeyClass::Other));
    assert_eq!(KeyClass::default(), KeyClass::Other);
}

#[test]
fn the_requests_of_the_bar_carry_their_arguments() {
    let (mut wire, bar, _) = bar_with(Offer::all(), |bar| bar.done());
    wire.events();
    wire.take();

    bar.set_menu_open(true);
    bar.set_menu_open(false);
    bar.window_command(WindowCommand::Minimize);
    bar.window_command(WindowCommand::CascadeAll);
    bar.activate_window(9);
    bar.session_action(SessionAction::ShutDown);
    bar.move_to_workspace(3);
    bar.edit(EditAction::Paste);
    bar.force_quit_app("org.example.Stuck");
    wire.events();

    let requests = bar_requests(&wire);
    assert!(matches!(requests[0], Request::SetMenuOpen { open: 1 }));
    assert!(matches!(requests[1], Request::SetMenuOpen { open: 0 }));
    assert!(matches!(requests[2], Request::WindowCommand { command } if u32::from(command) == 4));
    assert!(matches!(requests[3], Request::WindowCommand { command } if u32::from(command) == 12));
    assert!(matches!(requests[4], Request::ActivateWindow { id: 9 }));
    assert!(matches!(requests[5], Request::SessionAction { action } if u32::from(action) == 2));
    assert!(matches!(
        requests[6],
        Request::MoveToWorkspace { workspace: 3 }
    ));
    assert!(matches!(requests[7], Request::Edit { action } if u32::from(action) == 4));
    assert!(matches!(
        &requests[8],
        Request::ForceQuitApp { app_id } if app_id == "org.example.Stuck"
    ));
    assert_eq!(requests.len(), 9);
}

#[test]
fn every_session_action_and_edit_action_has_its_own_wire_value() {
    let (mut wire, bar, _) = bar_with(Offer::all(), |bar| bar.done());
    wire.events();
    wire.take();
    for action in [
        SessionAction::Sleep,
        SessionAction::Restart,
        SessionAction::ShutDown,
        SessionAction::Lock,
        SessionAction::LogOut,
        SessionAction::SwitchUser,
        SessionAction::RefreshCapabilities,
    ] {
        bar.session_action(action);
    }
    for action in [
        EditAction::Undo,
        EditAction::Redo,
        EditAction::Cut,
        EditAction::Copy,
        EditAction::Paste,
        EditAction::SelectAll,
    ] {
        bar.edit(action);
    }
    wire.events();
    let wire_values: Vec<u32> = bar_requests(&wire)
        .into_iter()
        .map(|r| match r {
            Request::SessionAction { action } => u32::from(action),
            Request::Edit { action } => u32::from(action),
            other => panic!("unexpected {other:?}"),
        })
        .collect();
    assert_eq!(wire_values, [0, 1, 2, 3, 4, 5, 6, 0, 1, 2, 3, 4, 5]);
}

#[test]
fn every_window_command_has_its_own_wire_value() {
    let (mut wire, bar, _) = bar_with(Offer::all(), |bar| bar.done());
    wire.events();
    wire.take();
    for command in [
        WindowCommand::HideApp,
        WindowCommand::HideOthers,
        WindowCommand::ShowAll,
        WindowCommand::QuitApp,
        WindowCommand::Minimize,
        WindowCommand::Zoom,
        WindowCommand::Close,
        WindowCommand::ToggleFullscreen,
        WindowCommand::TileLeft,
        WindowCommand::TileRight,
        WindowCommand::TileFill,
        WindowCommand::BringAllToFront,
        WindowCommand::CascadeAll,
        WindowCommand::MinimizeAll,
        WindowCommand::ZoomAll,
        WindowCommand::ForceQuitDialog,
    ] {
        bar.window_command(command);
    }
    wire.events();
    let values: Vec<u32> = bar_requests(&wire)
        .into_iter()
        .map(|r| match r {
            Request::WindowCommand { command } => u32::from(command),
            other => panic!("unexpected {other:?}"),
        })
        .collect();
    assert_eq!(values, (0..16).collect::<Vec<u32>>());
}

#[test]
fn a_server_of_version_nine_is_sent_only_what_it_knows() {
    let (mut wire, bar, _) = bar_with(Offer::shell(9), |bar| bar.done());
    wire.events();
    wire.take();

    assert!(!bar.can_edit());
    assert!(!bar.can_force_quit_app());
    bar.window_command(WindowCommand::Zoom);
    bar.window_command(WindowCommand::TileLeft);
    bar.window_command(WindowCommand::ForceQuitDialog);
    bar.activate_window(1);
    bar.edit(EditAction::Copy);
    bar.force_quit_app("org.example.Stuck");
    bar.session_action(SessionAction::Lock);
    wire.events();

    let requests = bar_requests(&wire);
    assert!(matches!(requests[0], Request::WindowCommand { command } if u32::from(command) == 5));
    assert!(matches!(requests[1], Request::SessionAction { .. }));
    assert_eq!(requests.len(), 2);
}

#[test]
fn the_edit_and_force_quit_requests_start_at_their_versions() {
    for (version, edit, force_quit) in [(10, false, false), (11, true, false), (13, true, true)] {
        let (mut wire, bar, _) = bar_with(Offer::shell(version), |bar| bar.done());
        wire.events();
        assert_eq!(bar.can_edit(), edit, "edit at {version}");
        assert_eq!(
            bar.can_force_quit_app(),
            force_quit,
            "force quit at {version}"
        );
        wire.take();
        bar.edit(EditAction::Undo);
        bar.force_quit_app("x");
        wire.events();
        assert_eq!(
            bar_requests(&wire).len(),
            usize::from(edit) + usize::from(force_quit),
            "at {version}"
        );
    }
}

#[test]
fn dropping_the_bar_gives_up_the_role() {
    let (mut wire, bar, _) = bar_with(Offer::all(), |bar| bar.done());
    wire.events();
    wire.take();
    drop(bar);
    wire.events();
    assert!(matches!(&bar_requests(&wire)[..], [Request::Destroy]));
}

#[test]
fn a_bar_for_an_unplugged_output_is_refused() {
    let (mut wire, _bar, surface) = bar_with(Offer::all(), |bar| bar.done());
    let shell = Shell::bind(&mut wire.conn).expect("shell");
    let result = shell.menu_bar(
        &wire.conn,
        surface,
        cupertino_wayland_client::OutputId::from_registry_name(9999),
    );
    assert!(matches!(
        result,
        Err(cupertino_shell_client::Error::UnknownOutput)
    ));
}
