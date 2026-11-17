//! The dock role over a real wire: how a group of events becomes a list of changes, the pictures the server sends as
//! files, and the requests of the dock.

mod support;

use cupertino_geometry::Rect;
use cupertino_protocols::server::cupertino_dock_v1::{
    self as wire_dock, CupertinoDockV1, Request, WindowState,
};
use cupertino_shell_client::{
    Anchor, AppCommand, Corner, Dock, DockChange, DockWindow, Error, Gesture,
    KeyboardInteractivity, Layer, LayerShell, LayerSurfaceConfig, Margin, OverviewMode,
    OverviewWindow, OverviewWorkspace, Shell, ShellEvent, ThumbnailImage, WindowIconImage,
    MAX_WINDOW_THUMBNAIL,
};
use std::os::fd::AsFd;
use support::{pixel_file, Offer, Seen, Wire};

/// The largest icon side a dock accepts.
const MAX_ICON_SIDE: u32 = 512;

fn surface_config(output: Option<cupertino_wayland_client::OutputId>) -> LayerSurfaceConfig {
    LayerSurfaceConfig {
        namespace: "dock".into(),
        output,
        layer: Layer::Top,
        anchor: Anchor::Bottom,
        size: (400, 80),
        margin: Margin::default(),
        exclusive_zone: 0,
        keyboard: KeyboardInteractivity::None,
    }
}

/// A dock on the first output; the server sends `first` as the dock's first group.
fn dock_with(offer: Offer, first: impl Fn(&CupertinoDockV1) + Send + 'static) -> (Wire, Dock) {
    let mut wire = Wire::new(offer);
    wire.server(|s| s.greet_dock = Some(Box::new(first)));
    let layers = LayerShell::bind(&mut wire.conn).expect("layer shell");
    let shell = Shell::bind(&mut wire.conn).expect("shell");
    let output = wire.conn.outputs()[0].id;
    let surface = layers
        .layer_surface(&mut wire.conn, &surface_config(Some(output)))
        .expect("layer surface")
        .surface();
    let dock = shell.dock(&wire.conn, surface, output).expect("dock");
    (wire, dock)
}

/// A dock whose first group is empty, with everything sent so far taken.
fn ready(offer: Offer) -> (Wire, Dock) {
    let (mut wire, dock) = dock_with(offer, |dock| dock.done());
    wire.events();
    wire.take();
    (wire, dock)
}

fn changes(wire: &mut Wire) -> Vec<Vec<DockChange>> {
    wire.shell_events()
        .into_iter()
        .filter_map(|e| match e {
            ShellEvent::Dock { update, .. } => Some(update),
            _ => None,
        })
        .collect()
}

/// The one change of the first group.
fn only_change(wire: &mut Wire) -> DockChange {
    changes(wire).remove(0).remove(0)
}

fn dock_requests(wire: &Wire) -> Vec<Request> {
    wire.take()
        .into_iter()
        .filter_map(|s| match s {
            Seen::Dock(r) => Some(r),
            _ => None,
        })
        .collect()
}

fn fails_the_connection(wire: &mut Wire) -> bool {
    let result = wire
        .conn
        .roundtrip()
        .and_then(|()| wire.conn.wait(Some(std::time::Duration::ZERO)).map(|_| ()));
    matches!(result, Err(cupertino_wayland_client::Error::Protocol(_)))
}

#[test]
fn the_first_group_lists_the_windows_and_their_states() {
    let (mut wire, _dock) = dock_with(Offer::all(), |dock| {
        dock.window(1, "org.example.A".into(), "One".into(), WindowState::Active);
        dock.window(
            2,
            "org.example.B".into(),
            "Two".into(),
            WindowState::Minimized,
        );
        dock.window(
            3,
            "org.example.B".into(),
            "Three".into(),
            WindowState::empty(),
        );
        // a state bit a newer server defines is ignored
        dock.window(
            4,
            "org.example.C".into(),
            "Four".into(),
            WindowState::from_bits_retain(0x100 | WindowState::Active.bits()),
        );
        dock.done();
    });
    let window = |id, app: &str, title: &str, minimized, active| {
        DockChange::Window(DockWindow {
            id,
            app_id: app.into(),
            title: title.into(),
            minimized,
            active,
        })
    };
    assert_eq!(
        changes(&mut wire),
        [vec![
            window(1, "org.example.A", "One", false, true),
            window(2, "org.example.B", "Two", true, false),
            window(3, "org.example.B", "Three", false, false),
            window(4, "org.example.C", "Four", false, true),
        ]]
    );
}

#[test]
fn each_event_is_one_change_in_its_group_and_an_empty_group_is_not_reported() {
    let (mut wire, _dock) = dock_with(Offer::all(), |dock| dock.done());
    assert!(changes(&mut wire).is_empty());

    wire.server(|s| {
        let dock = &s.docks[0];
        dock.launching("org.example.A".into());
        dock.done();
        dock.window_closed(4);
        dock.attention("org.example.A".into());
        dock.toggle_auto_hide();
        dock.toggle_app_grid();
        dock.unresponsive("org.example.A".into(), 1);
        dock.unresponsive("org.example.A".into(), 0);
        dock.force_quit_dialog();
        dock.engage();
        dock.app_resident("org.example.A".into(), 1);
        dock.app_resident("org.example.B".into(), 0);
        dock.switcher_closed(1);
        dock.switcher_closed(0);
        for corner in [
            wire_dock::Corner::TopLeft,
            wire_dock::Corner::TopRight,
            wire_dock::Corner::BottomLeft,
            wire_dock::Corner::BottomRight,
        ] {
            dock.hot_corner(corner);
        }
        for gesture in [
            wire_dock::Gesture::SwipeUp,
            wire_dock::Gesture::SwipeDown,
            wire_dock::Gesture::Pinch,
            wire_dock::Gesture::Spread,
        ] {
            dock.gesture(gesture);
        }
        dock.overview_closed();
        dock.done();
        dock.done();
    });
    assert_eq!(
        changes(&mut wire),
        [
            vec![DockChange::Launching("org.example.A".into())],
            vec![
                DockChange::WindowClosed(4),
                DockChange::Attention("org.example.A".into()),
                DockChange::ToggleAutoHide,
                DockChange::ToggleAppGrid,
                DockChange::Unresponsive {
                    app_id: "org.example.A".into(),
                    unresponsive: true
                },
                DockChange::Unresponsive {
                    app_id: "org.example.A".into(),
                    unresponsive: false
                },
                DockChange::ForceQuitDialog,
                DockChange::Engage,
                DockChange::AppResident {
                    app_id: "org.example.A".into(),
                    resident: true
                },
                DockChange::AppResident {
                    app_id: "org.example.B".into(),
                    resident: false
                },
                DockChange::SwitcherClosed { committed: true },
                DockChange::SwitcherClosed { committed: false },
                DockChange::HotCorner(Corner::TopLeft),
                DockChange::HotCorner(Corner::TopRight),
                DockChange::HotCorner(Corner::BottomLeft),
                DockChange::HotCorner(Corner::BottomRight),
                DockChange::Gesture(Gesture::SwipeUp),
                DockChange::Gesture(Gesture::SwipeDown),
                DockChange::Gesture(Gesture::Pinch),
                DockChange::Gesture(Gesture::Spread),
                DockChange::OverviewClosed,
            ],
        ]
    );
}

#[test]
fn the_switcher_names_the_output_the_entries_and_the_selection() {
    let (mut wire, _dock) = ready(Offer::all());
    let output = wire.conn.outputs()[0].id;
    wire.server(|s| {
        let panel = s.outputs[0].clone();
        let dock = &s.docks[0];
        dock.switcher_output(&panel);
        dock.switcher_entry("org.example.A".into());
        dock.switcher_entry("org.example.B".into());
        dock.switcher_entry("org.example.C".into());
        dock.switcher_changed(1);
        dock.done();
        // the entries of one change are not the next one's
        dock.switcher_output(&panel);
        dock.switcher_entry("org.example.D".into());
        dock.switcher_changed(0);
        dock.done();
    });
    assert_eq!(
        changes(&mut wire),
        [
            vec![DockChange::SwitcherChanged {
                output,
                apps: vec![
                    "org.example.A".into(),
                    "org.example.B".into(),
                    "org.example.C".into()
                ],
                selected: 1,
            }],
            vec![DockChange::SwitcherChanged {
                output,
                apps: vec!["org.example.D".into()],
                selected: 0,
            }]
        ]
    );

    wire.server(|s| {
        s.docks[0].switcher_entry("org.example.A".into());
        s.docks[0].switcher_changed(0);
        s.docks[0].done();
    });
    assert!(
        fails_the_connection(&mut wire),
        "a switcher change without its output"
    );
}

#[test]
fn the_overview_carries_its_mode_area_windows_and_workspaces() {
    let (mut wire, _dock) = ready(Offer::all());
    wire.server(|s| {
        let dock = &s.docks[0];
        dock.overview_window(7, 10, 20, 300, 200);
        dock.overview_window(8, -5, 0, 100, 50);
        dock.overview_workspace(1, 1);
        dock.overview_workspace(2, 0);
        dock.overview_changed(
            wire_dock::OverviewMode::All,
            String::new(),
            0,
            30,
            1920,
            1050,
        );
        dock.overview_window(9, 0, 0, 10, 10);
        dock.overview_changed(
            wire_dock::OverviewMode::App,
            "org.example.A".into(),
            0,
            0,
            800,
            600,
        );
        dock.done();
    });
    assert_eq!(
        changes(&mut wire),
        [vec![
            DockChange::OverviewChanged {
                mode: OverviewMode::All,
                area: Rect::new(0.0, 30.0, 1920.0, 1050.0),
                windows: vec![
                    OverviewWindow {
                        id: 7,
                        frame: Rect::new(10.0, 20.0, 300.0, 200.0)
                    },
                    OverviewWindow {
                        id: 8,
                        frame: Rect::new(-5.0, 0.0, 100.0, 50.0)
                    },
                ],
                workspaces: vec![
                    OverviewWorkspace {
                        id: 1,
                        current: true
                    },
                    OverviewWorkspace {
                        id: 2,
                        current: false
                    },
                ],
            },
            DockChange::OverviewChanged {
                mode: OverviewMode::App("org.example.A".into()),
                area: Rect::new(0.0, 0.0, 800.0, 600.0),
                windows: vec![OverviewWindow {
                    id: 9,
                    frame: Rect::new(0.0, 0.0, 10.0, 10.0)
                }],
                workspaces: vec![],
            },
        ]]
    );
}

#[test]
fn a_window_icon_is_its_file_read_whole_or_no_image() {
    let side = MAX_ICON_SIDE;
    let over = MAX_ICON_SIDE + 1;
    let ramp: Vec<u8> = (0..2 * 2 * 4).collect();
    let rows: Vec<(&str, &'static str, u32, Vec<u8>, bool)> = vec![
        ("a named icon", "text-editor", 2, ramp, true),
        ("an icon without a name", "", 1, vec![0; 4], true),
        (
            "the largest side",
            "x",
            side,
            vec![7; (side * side * 4) as usize],
            true,
        ),
        ("size 0", "x", 0, vec![], false),
        (
            "a file shorter than the size says",
            "x",
            2,
            vec![0; 15],
            false,
        ),
        (
            "a file longer than the size says",
            "x",
            2,
            vec![0; 17],
            false,
        ),
        (
            "over the largest side",
            "x",
            over,
            vec![0; (over * over * 4) as usize],
            false,
        ),
    ];
    for (row, name, size, bytes, has_image) in rows {
        let sent = bytes.clone();
        let (mut wire, _dock) = dock_with(Offer::all(), move |dock| {
            let file = pixel_file(&sent);
            dock.window_icon(5, name.into(), size, file.as_fd());
            dock.done();
        });
        let DockChange::WindowIcon(icon) = only_change(&mut wire) else {
            panic!("{row}: not an icon");
        };
        assert_eq!(icon.id, 5, "{row}");
        assert_eq!(
            icon.name,
            (!name.is_empty()).then(|| name.to_string()),
            "{row}"
        );
        let expected = has_image.then_some(WindowIconImage { size, argb: bytes });
        assert!(icon.image == expected, "{row}");
    }
}

#[test]
fn a_window_thumbnail_is_its_file_read_whole_or_no_image() {
    let max = MAX_WINDOW_THUMBNAIL;
    let ramp: Vec<u8> = (0..4 * 3 * 4).collect();
    let rows: Vec<(&str, u32, u32, Vec<u8>, bool)> = vec![
        ("a thumbnail", 4, 3, ramp, true),
        ("the largest side", max, 1, vec![1; max as usize * 4], true),
        ("no width", 0, 3, vec![], false),
        ("no height", 3, 0, vec![], false),
        ("a file of the wrong length", 2, 2, vec![0; 15], false),
        (
            "too wide",
            max + 1,
            1,
            vec![0; (max as usize + 1) * 4],
            false,
        ),
        (
            "too tall",
            1,
            max + 1,
            vec![0; (max as usize + 1) * 4],
            false,
        ),
    ];
    for (row, width, height, bytes, has_image) in rows {
        let sent = bytes.clone();
        let (mut wire, _dock) = dock_with(Offer::all(), move |dock| {
            let file = pixel_file(&sent);
            dock.window_thumbnail(6, width, height, file.as_fd());
            dock.done();
        });
        let DockChange::WindowThumbnail(thumbnail) = only_change(&mut wire) else {
            panic!("{row}: not a thumbnail");
        };
        assert_eq!(thumbnail.id, 6, "{row}");
        let expected = has_image.then_some(ThumbnailImage {
            width,
            height,
            argb: bytes,
        });
        assert!(thumbnail.image == expected, "{row}");
    }
}

#[test]
fn the_requests_of_the_dock_carry_their_arguments() {
    let (mut wire, dock) = ready(Offer::all());

    dock.activate_app("org.example.A");
    dock.force_quit_app("org.example.B");
    assert!(dock.activate_window(11));
    dock.switcher_cancel();
    dock.overview_select(12);
    dock.overview_close();
    dock.switch_workspace(3);
    dock.add_workspace();
    dock.remove_workspace(4);
    dock.run_action("toggle-overview");
    dock.switcher_select(2).unwrap();
    dock.switcher_activate(1).unwrap();
    dock.move_workspace(5, 0).unwrap();
    for command in [AppCommand::ShowWindows, AppCommand::Hide, AppCommand::Quit] {
        dock.app_command("org.example.A", command);
    }
    wire.events();

    let requests = dock_requests(&wire);
    assert!(matches!(&requests[0], Request::ActivateApp { app_id } if app_id == "org.example.A"));
    assert!(matches!(&requests[1], Request::ForceQuitApp { app_id } if app_id == "org.example.B"));
    assert!(matches!(requests[2], Request::ActivateWindow { id: 11 }));
    assert!(matches!(requests[3], Request::SwitcherCancel));
    assert!(matches!(requests[4], Request::OverviewSelect { id: 12 }));
    assert!(matches!(requests[5], Request::OverviewClose));
    assert!(matches!(requests[6], Request::SwitchWorkspace { id: 3 }));
    assert!(matches!(requests[7], Request::AddWorkspace));
    assert!(matches!(requests[8], Request::RemoveWorkspace { id: 4 }));
    assert!(matches!(&requests[9], Request::RunAction { name } if name == "toggle-overview"));
    assert!(matches!(requests[10], Request::SwitcherSelect { index: 2 }));
    assert!(matches!(
        requests[11],
        Request::SwitcherActivate { index: 1 }
    ));
    assert!(matches!(
        requests[12],
        Request::MoveWorkspace { id: 5, index: 0 }
    ));
    let commands: Vec<u32> = requests[13..]
        .iter()
        .map(|r| match r {
            Request::AppCommand { app_id, command } if app_id == "org.example.A" => {
                u32::from(*command)
            }
            other => panic!("unexpected {other:?}"),
        })
        .collect();
    assert_eq!(commands, [0, 1, 2], "each app command has its own value");
}

#[test]
fn a_rectangle_is_sent_as_the_whole_points_around_it_and_an_empty_workspace_frame_as_zeros() {
    let (mut wire, dock) = ready(Offer::all());
    let rect = Rect::new(0.5, 0.25, 10.0, 10.0);

    dock.restore_window(3, &rect).unwrap();
    dock.set_minimize_target(&rect).unwrap();
    dock.overview_frame(4, &rect).unwrap();
    dock.overview_drag(5, &rect).unwrap();
    dock.overview_workspace_frame(6, &rect).unwrap();
    dock.overview_workspace_frame(2, &Rect::new(5.0, 5.0, 0.0, 10.0))
        .unwrap();
    dock.overview_workspace_frame(2, &Rect::new(5.0, 5.0, 10.0, -1.0))
        .unwrap();
    wire.events();

    let requests = dock_requests(&wire);
    assert!(matches!(
        requests[0],
        Request::RestoreWindow {
            id: 3,
            x: 0,
            y: 0,
            width: 11,
            height: 11
        }
    ));
    assert!(matches!(
        requests[1],
        Request::SetMinimizeTarget {
            x: 0,
            y: 0,
            width: 11,
            height: 11
        }
    ));
    assert!(matches!(
        requests[2],
        Request::OverviewFrame {
            id: 4,
            x: 0,
            y: 0,
            width: 11,
            height: 11
        }
    ));
    assert!(matches!(
        requests[3],
        Request::OverviewDrag {
            id: 5,
            x: 0,
            y: 0,
            width: 11,
            height: 11
        }
    ));
    assert!(matches!(
        requests[4],
        Request::OverviewWorkspaceFrame {
            workspace: 6,
            x: 0,
            y: 0,
            width: 11,
            height: 11
        }
    ));
    for request in &requests[5..] {
        assert!(matches!(
            request,
            Request::OverviewWorkspaceFrame {
                workspace: 2,
                x: 0,
                y: 0,
                width: 0,
                height: 0
            }
        ));
    }
    assert_eq!(requests.len(), 7);
}

#[test]
fn a_shape_the_wire_cannot_carry_is_refused_and_nothing_is_sent() {
    let (mut wire, dock) = ready(Offer::all());
    for bad in [
        Rect::new(f64::NAN, 0.0, 1.0, 1.0),
        Rect::new(0.0, f64::INFINITY, 1.0, 1.0),
        Rect::new(0.0, 0.0, f64::INFINITY, 1.0),
        Rect::new(0.0, 0.0, 1.0, f64::NAN),
    ] {
        for (request, result) in [
            ("restore_window", dock.restore_window(1, &bad)),
            ("set_minimize_target", dock.set_minimize_target(&bad)),
            ("overview_frame", dock.overview_frame(1, &bad)),
            ("overview_drag", dock.overview_drag(1, &bad)),
            (
                "overview_workspace_frame",
                dock.overview_workspace_frame(1, &bad),
            ),
        ] {
            assert!(
                matches!(result, Err(Error::InvalidShape)),
                "{request} of {bad:?}"
            );
        }
    }
    let too_big = u32::MAX as usize + 1;
    for (request, result) in [
        ("switcher_select", dock.switcher_select(too_big)),
        ("switcher_activate", dock.switcher_activate(too_big)),
        ("move_workspace", dock.move_workspace(1, too_big)),
    ] {
        assert!(matches!(result, Err(Error::InvalidShape)), "{request}");
    }
    dock.switcher_select(u32::MAX as usize).unwrap();
    wire.events();
    assert!(matches!(
        dock_requests(&wire)[..],
        [Request::SwitcherSelect { index: u32::MAX }]
    ));
}

#[test]
fn a_request_newer_than_the_server_is_not_sent() {
    let rect = Rect::new(0.0, 0.0, 10.0, 10.0);
    for version in [22, 23, 25, 26] {
        let (mut wire, dock) = ready(Offer::shell(version));
        dock.overview_drag(1, &rect).unwrap();
        dock.overview_move_window(8, 2);
        dock.overview_workspace_frame(2, &rect).unwrap();
        assert_eq!(dock.activate_window(3), version >= 26, "version {version}");
        wire.events();
        let requests = dock_requests(&wire);
        let sent = |matches: fn(&Request) -> bool| requests.iter().any(matches);
        let drags = version >= 23;
        assert_eq!(
            sent(|r| matches!(r, Request::OverviewDrag { .. })),
            drags,
            "version {version}: overview_drag"
        );
        assert_eq!(
            sent(|r| matches!(
                r,
                Request::OverviewMoveWindow {
                    id: 8,
                    workspace: 2
                }
            )),
            drags,
            "version {version}: overview_move_window"
        );
        assert_eq!(
            sent(|r| matches!(r, Request::OverviewWorkspaceFrame { .. })),
            drags,
            "version {version}: overview_workspace_frame"
        );
        assert_eq!(
            sent(|r| matches!(r, Request::ActivateWindow { id: 3 })),
            version >= 26,
            "version {version}: activate_window"
        );
    }
}

#[test]
fn a_dock_needs_a_plugged_output_and_gives_up_the_role_when_dropped() {
    let (mut wire, dock) = ready(Offer::all());
    let shell = Shell::bind(&mut wire.conn).expect("shell");
    let layers = LayerShell::bind(&mut wire.conn).expect("layer shell");
    let surface = layers
        .layer_surface(&mut wire.conn, &surface_config(None))
        .expect("surface")
        .surface();
    let result = shell.dock(
        &wire.conn,
        surface,
        cupertino_wayland_client::OutputId::from_registry_name(9999),
    );
    assert!(matches!(result, Err(Error::UnknownOutput)));
    wire.events();
    wire.take();

    drop(dock);
    wire.events();
    assert!(matches!(&dock_requests(&wire)[..], [Request::Destroy]));
}
