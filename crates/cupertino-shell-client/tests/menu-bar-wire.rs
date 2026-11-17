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

/// A bar whose first group is empty, with everything sent so far taken.
fn ready(offer: Offer) -> (Wire, MenuBar, cupertino_wayland_client::SurfaceId) {
    let (mut wire, bar, surface) = bar_with(offer, |bar| bar.done());
    wire.events();
    wire.take();
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

/// The update of the one group `send` sends.
fn next_update(
    wire: &mut Wire,
    send: impl FnOnce(&CupertinoMenuBarV1) + Send + 'static,
) -> MenuBarUpdate {
    wire.server(move |s| {
        send(&s.menu_bars[0]);
        s.menu_bars[0].done();
    });
    updates(wire).remove(0)
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
fn each_group_is_one_update_of_the_fields_it_mentions_and_an_empty_one_is_not_reported() {
    let (mut wire, _bar, surface) = bar_with(Offer::all(), |bar| {
        bar.active_app("org.example.Edit".into(), "Edit".into());
        bar.menu_none();
        bar.done();
        bar.done();
    });
    let events = wire.shell_events();
    assert_eq!(events.len(), 1, "the empty group is not reported");
    let ShellEvent::MenuBar {
        surface: of,
        update,
    } = &events[0]
    else {
        panic!("not a menu bar event: {:?}", events[0]);
    };
    assert_eq!(
        *of, surface,
        "the first group arrives with the bar's surface"
    );
    assert_eq!(
        update.active_app,
        Some(ActiveApp {
            app_id: "org.example.Edit".into(),
            title: "Edit".into()
        })
    );
    assert_eq!(update.menu, Some(WindowMenu::None));

    assert_eq!(
        next_update(&mut wire, |bar| bar.screen_capture(1)),
        MenuBarUpdate {
            screen_capture: Some(true),
            ..MenuBarUpdate::default()
        }
    );
    assert_eq!(
        next_update(&mut wire, |bar| {
            bar.engage();
            bar.engage_help();
            bar.screen_capture(0);
        }),
        MenuBarUpdate {
            engage: true,
            engage_help: true,
            screen_capture: Some(false),
            ..MenuBarUpdate::default()
        }
    );
}

#[test]
fn the_menus_of_the_active_window_keep_their_addresses() {
    let (mut wire, _bar, _) = ready(Offer::all());
    let update = next_update(&mut wire, |bar| {
        bar.menu_dbusmenu(":1.5".into(), "/menu".into());
    });
    assert_eq!(
        update.menu,
        Some(WindowMenu::DbusMenu {
            service: ":1.5".into(),
            path: "/menu".into()
        })
    );
    let update = next_update(&mut wire, |bar| {
        bar.menu_gtk(
            ":1.9".into(),
            "org.example".into(),
            "/org/example".into(),
            String::new(),
            "/menubar".into(),
            String::new(),
        );
    });
    assert_eq!(
        update.menu,
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
fn window_state_takes_the_controls_full_screen_and_window_list_of_its_group() {
    let (mut wire, _bar, _) = ready(Offer::all());
    let update = next_update(&mut wire, |bar| {
        bar.window_state(1, 2, 4);
        bar.window_controls(WindowControl::Minimize);
        bar.window_full_screen(1);
    });
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
    assert_eq!(
        update.windows,
        Some(vec![]),
        "the window list is complete in every group with a window state"
    );

    let state = next_update(&mut wire, |bar| bar.window_state(1, 1, 1))
        .window_state
        .expect("state");
    assert!(
        state.can_minimize && state.can_zoom && !state.full_screen,
        "without controls every control is enabled"
    );

    let state = next_update(&mut wire, |bar| {
        bar.window_state(0, 1, 1);
        bar.window_controls(WindowControl::Minimize | WindowControl::Zoom);
    })
    .window_state
    .expect("state");
    assert!(
        state.can_minimize && !state.can_zoom,
        "a window that cannot zoom stays unable whatever its controls say"
    );

    let update = next_update(&mut wire, |bar| {
        bar.window_state(1, 1, 1);
        bar.app_window(3, "One".into(), AppWindowFlags::Active);
        bar.app_window(4, "Two".into(), AppWindowFlags::Minimized);
        bar.app_window(5, "Three".into(), AppWindowFlags::empty());
    });
    let window = |id, title: &str, active, minimized| AppWindow {
        id,
        title: title.into(),
        active,
        minimized,
    };
    assert_eq!(
        update.windows,
        Some(vec![
            window(3, "One", true, false),
            window(4, "Two", false, true),
            window(5, "Three", false, false),
        ])
    );

    let (mut wire, _bar, _) = ready(Offer::shell(9));
    let update = next_update(&mut wire, |bar| bar.window_state(1, 1, 1));
    assert!(update.window_state.is_some());
    assert_eq!(update.windows, None, "a server without the window list");
}

#[test]
fn the_session_agent_trust_maps_to_who_may_answer() {
    let (mut wire, _bar, _) = ready(Offer::all());
    for (trust, pid, expected) in [
        (AgentTrust::Process, 1234, SessionAgent::Process(1234)),
        (AgentTrust::Any, 77, SessionAgent::Any),
        (AgentTrust::None, 77, SessionAgent::None),
    ] {
        let update = next_update(&mut wire, move |bar| bar.session_agent(trust, pid));
        assert_eq!(update.session_agent, Some(expected));
    }
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
fn the_backdrop_is_a_bounded_lightness_and_at_most_the_band_limit_of_whole_bands() {
    let (mut wire, _bar, _) = ready(Offer::all());
    for (sent, read) in [(640, 640), (5000, 1000)] {
        let update = next_update(&mut wire, move |bar| bar.backdrop_luminance(sent));
        assert_eq!(update.backdrop_lightness, Some(read), "{sent}");
    }

    let mut partial = bands(3);
    partial.extend([9, 9, 9]);
    let update = next_update(&mut wire, move |bar| bar.backdrop_bands(partial));
    assert_eq!(
        update.backdrop_bands,
        Some(vec![[0, 1, 0xff00], [1, 2, 0xff01], [2, 3, 0xff02]]),
        "little-endian triples, a partial band dropped"
    );

    let update = next_update(&mut wire, |bar| {
        bar.backdrop_bands(bands(MAX_BACKDROP_BANDS + 10))
    });
    let read = update.backdrop_bands.expect("bands");
    assert_eq!(read.len(), MAX_BACKDROP_BANDS);
    assert_eq!(
        read[MAX_BACKDROP_BANDS - 1][0],
        (MAX_BACKDROP_BANDS - 1) as u16
    );
}

#[test]
fn shortcuts_are_added_per_action_and_an_empty_chord_removes_one() {
    let actions = [
        (WireBoundAction::CloseWindow, BoundAction::CloseWindow),
        (WireBoundAction::Minimize, BoundAction::Minimize),
        (WireBoundAction::Zoom, BoundAction::Zoom),
        (
            WireBoundAction::ToggleFullscreen,
            BoundAction::ToggleFullscreen,
        ),
        (WireBoundAction::HideApp, BoundAction::HideApp),
        (WireBoundAction::HideOthers, BoundAction::HideOthers),
        (WireBoundAction::QuitApp, BoundAction::QuitApp),
        (WireBoundAction::TileLeft, BoundAction::TileLeft),
        (WireBoundAction::TileRight, BoundAction::TileRight),
        (WireBoundAction::TileFill, BoundAction::TileFill),
        (WireBoundAction::ForceQuit, BoundAction::ForceQuit),
        (WireBoundAction::Lock, BoundAction::Lock),
        (WireBoundAction::LogOut, BoundAction::LogOut),
        (WireBoundAction::HelpSearch, BoundAction::HelpSearch),
    ];
    let (mut wire, _bar, _) = ready(Offer::all());
    let update = next_update(&mut wire, move |bar| {
        for (i, (action, _)) in actions.into_iter().enumerate() {
            bar.shortcut(action, format!("chord{i}"));
        }
        bar.primary_modifier(ModifierKey::Ctrl);
    });
    let shortcuts = update.shortcuts.expect("shortcuts");
    assert_eq!(shortcuts.chords.len(), actions.len());
    for (i, (_, action)) in actions.into_iter().enumerate() {
        assert_eq!(
            shortcuts.chord(action),
            Some(format!("chord{i}").as_str()),
            "{action:?}"
        );
    }
    assert_eq!(shortcuts.primary, PrimaryModifier::Ctrl);

    // a group lists the chords it changes: an empty one unbinds, and a group that does not name the modifier reads Super
    let update = next_update(&mut wire, |bar| {
        bar.shortcut(WireBoundAction::QuitApp, "Super+q".into());
    });
    let shortcuts = update.shortcuts.expect("shortcuts");
    assert_eq!(shortcuts.chord(BoundAction::QuitApp), Some("Super+q"));
    assert_eq!(shortcuts.chord(BoundAction::HideApp), None);
    assert_eq!(shortcuts.primary, PrimaryModifier::Super);
    let update = next_update(&mut wire, |bar| {
        bar.shortcut(WireBoundAction::QuitApp, String::new());
    });
    assert!(update.shortcuts.expect("shortcuts").chords.is_empty());
}

#[test]
fn the_active_client_and_its_key_class_come_together() {
    let (mut wire, _bar, _) = ready(Offer::all());
    let update = next_update(&mut wire, |bar| {
        bar.window_state(1, 1, 1);
        bar.active_client(500, ClientKind::X11);
        bar.active_client_keys(WireKeyClass::Terminal);
    });
    assert_eq!(update.active_client, Some(ActiveClient::X11(500)));
    assert_eq!(update.key_class, Some(KeyClass::Terminal));
    let update = next_update(&mut wire, |bar| {
        bar.active_client(0, ClientKind::Wayland);
        bar.active_client_keys(WireKeyClass::Toolkit);
    });
    assert_eq!(update.active_client, Some(ActiveClient::Wayland(0)));
    assert_eq!(update.key_class, Some(KeyClass::Toolkit));

    let (mut wire, _bar, _) = ready(Offer::shell(24));
    let update = next_update(&mut wire, |bar| bar.active_client(42, ClientKind::Wayland));
    assert_eq!(update.active_client, Some(ActiveClient::Wayland(42)));
    assert_eq!(
        update.key_class,
        Some(KeyClass::Other),
        "a version 24 server gets the default key class"
    );
}

#[test]
fn the_requests_of_the_bar_carry_their_arguments() {
    let (mut wire, bar, _) = ready(Offer::all());

    bar.set_menu_open(true);
    bar.set_menu_open(false);
    bar.activate_window(9);
    bar.move_to_workspace(3);
    bar.force_quit_app("org.example.Stuck");
    let commands = [
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
    ];
    for command in commands {
        bar.window_command(command);
    }
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

    let requests = bar_requests(&wire);
    assert!(matches!(requests[0], Request::SetMenuOpen { open: 1 }));
    assert!(matches!(requests[1], Request::SetMenuOpen { open: 0 }));
    assert!(matches!(requests[2], Request::ActivateWindow { id: 9 }));
    assert!(matches!(
        requests[3],
        Request::MoveToWorkspace { workspace: 3 }
    ));
    assert!(matches!(
        &requests[4],
        Request::ForceQuitApp { app_id } if app_id == "org.example.Stuck"
    ));
    let values: Vec<(&str, u32)> = requests[5..]
        .iter()
        .map(|r| match r {
            Request::WindowCommand { command } => ("window", u32::from(*command)),
            Request::SessionAction { action } => ("session", u32::from(*action)),
            Request::Edit { action } => ("edit", u32::from(*action)),
            other => panic!("unexpected {other:?}"),
        })
        .collect();
    let expected: Vec<(&str, u32)> = (0..16)
        .map(|v| ("window", v))
        .chain((0..7).map(|v| ("session", v)))
        .chain((0..6).map(|v| ("edit", v)))
        .collect();
    assert_eq!(
        values, expected,
        "each command and action has its own value"
    );
}

#[test]
fn a_server_is_sent_only_the_requests_its_version_knows() {
    let (mut wire, bar, _) = ready(Offer::shell(9));
    bar.window_command(WindowCommand::Zoom);
    bar.window_command(WindowCommand::TileLeft);
    bar.window_command(WindowCommand::ForceQuitDialog);
    bar.activate_window(1);
    bar.session_action(SessionAction::Lock);
    wire.events();
    let requests = bar_requests(&wire);
    assert!(matches!(requests[0], Request::WindowCommand { command } if u32::from(command) == 5));
    assert!(matches!(requests[1], Request::SessionAction { .. }));
    assert_eq!(requests.len(), 2, "version 9");

    for (version, edit, force_quit) in [
        (9, false, false),
        (10, false, false),
        (11, true, false),
        (13, true, true),
    ] {
        let (mut wire, bar, _) = ready(Offer::shell(version));
        assert_eq!(bar.can_edit(), edit, "edit at {version}");
        assert_eq!(
            bar.can_force_quit_app(),
            force_quit,
            "force quit at {version}"
        );
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
fn a_bar_needs_a_plugged_output_and_gives_up_the_role_when_dropped() {
    let (mut wire, bar, surface) = ready(Offer::all());
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
    wire.events();
    wire.take();

    drop(bar);
    wire.events();
    assert!(matches!(&bar_requests(&wire)[..], [Request::Destroy]));
}
