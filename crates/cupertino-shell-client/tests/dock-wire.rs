//! The dock role over a real wire: how a group of events becomes a list of changes, the pictures the server sends as
//! files, and the requests of the dock.

mod support;

use cupertino_geometry::Rect;
use cupertino_protocols::server::cupertino_dock_v1::{
    self as wire_dock, CupertinoDockV1, Request, WindowState,
};
use cupertino_shell_client::{
    Anchor, AppCommand, Corner, Dock, DockChange, DockWindow, DockWindowIcon, DockWindowThumbnail,
    Error, Gesture, KeyboardInteractivity, Layer, LayerShell, LayerSurfaceConfig, Margin,
    OverviewMode, OverviewWindow, OverviewWorkspace, Shell, ShellEvent, ThumbnailImage,
    WindowIconImage, MAX_WINDOW_THUMBNAIL,
};
use std::os::fd::AsFd;
use support::{pixel_file, Offer, Seen, Wire};

/// A dock on the first output; the server sends `first` as the dock's first group.
fn dock_with(offer: Offer, first: impl Fn(&CupertinoDockV1) + Send + 'static) -> (Wire, Dock) {
    let mut wire = Wire::new(offer);
    wire.server(|s| s.greet_dock = Some(Box::new(first)));
    let layers = LayerShell::bind(&mut wire.conn).expect("layer shell");
    let shell = Shell::bind(&mut wire.conn).expect("shell");
    let output = wire.conn.outputs()[0].id;
    let surface = layers
        .layer_surface(
            &mut wire.conn,
            &LayerSurfaceConfig {
                namespace: "dock".into(),
                output: Some(output),
                layer: Layer::Top,
                anchor: Anchor::Bottom,
                size: (400, 80),
                margin: Margin::default(),
                exclusive_zone: 0,
                keyboard: KeyboardInteractivity::None,
            },
        )
        .expect("layer surface")
        .surface();
    let dock = shell.dock(&wire.conn, surface, output).expect("dock");
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

fn dock_requests(wire: &Wire) -> Vec<Request> {
    wire.take()
        .into_iter()
        .filter_map(|s| match s {
            Seen::Dock(r) => Some(r),
            _ => None,
        })
        .collect()
}

fn ready(wire: &mut Wire) {
    wire.events();
    wire.take();
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
        dock.done();
    });
    let groups = changes(&mut wire);
    assert_eq!(groups.len(), 1);
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
        groups[0],
        [
            window(1, "org.example.A", "One", false, true),
            window(2, "org.example.B", "Two", true, false),
            window(3, "org.example.B", "Three", false, false),
        ]
    );
}

#[test]
fn state_bits_a_newer_server_defines_are_ignored() {
    let (mut wire, _dock) = dock_with(Offer::all(), |dock| {
        dock.window(
            1,
            "a".into(),
            "A".into(),
            WindowState::from_bits_retain(0x100 | WindowState::Active.bits()),
        );
        dock.done();
    });
    let groups = changes(&mut wire);
    assert!(matches!(
        &groups[0][0],
        DockChange::Window(DockWindow {
            active: true,
            minimized: false,
            ..
        })
    ));
}

#[test]
fn a_group_with_nothing_in_it_is_not_reported_and_events_stay_in_their_group() {
    let (mut wire, _dock) = dock_with(Offer::all(), |dock| dock.done());
    assert!(changes(&mut wire).is_empty());

    wire.server(|s| {
        s.docks[0].launching("org.example.A".into());
        s.docks[0].done();
        s.docks[0].window_closed(4);
        s.docks[0].attention("org.example.A".into());
        s.docks[0].done();
    });
    assert_eq!(
        changes(&mut wire),
        [
            vec![DockChange::Launching("org.example.A".into())],
            vec![
                DockChange::WindowClosed(4),
                DockChange::Attention("org.example.A".into())
            ],
        ]
    );
}

#[test]
fn the_simple_events_map_one_to_one_in_order() {
    let (mut wire, _dock) = dock_with(Offer::all(), |dock| dock.done());
    wire.events();
    wire.server(|s| {
        let dock = &s.docks[0];
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
        dock.hot_corner(wire_dock::Corner::BottomRight);
        dock.gesture(wire_dock::Gesture::Spread);
        dock.overview_closed();
        dock.done();
    });
    assert_eq!(
        changes(&mut wire),
        [vec![
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
            DockChange::HotCorner(Corner::BottomRight),
            DockChange::Gesture(Gesture::Spread),
            DockChange::OverviewClosed,
        ]]
    );
}

#[test]
fn the_switcher_names_the_output_the_entries_and_the_selection() {
    let (mut wire, _dock) = dock_with(Offer::all(), |dock| dock.done());
    wire.events();
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
    });
    assert_eq!(
        changes(&mut wire),
        [vec![DockChange::SwitcherChanged {
            output,
            apps: vec![
                "org.example.A".into(),
                "org.example.B".into(),
                "org.example.C".into()
            ],
            selected: 1,
        }]]
    );

    // the entries of one change are not the next one's
    wire.server(|s| {
        let panel = s.outputs[0].clone();
        let dock = &s.docks[0];
        dock.switcher_output(&panel);
        dock.switcher_entry("org.example.D".into());
        dock.switcher_changed(0);
        dock.done();
    });
    assert_eq!(
        changes(&mut wire),
        [vec![DockChange::SwitcherChanged {
            output,
            apps: vec!["org.example.D".into()],
            selected: 0,
        }]]
    );
}

#[test]
fn a_switcher_change_without_its_output_fails_the_connection() {
    let (mut wire, _dock) = dock_with(Offer::all(), |dock| dock.done());
    wire.events();
    wire.server(|s| {
        s.docks[0].switcher_entry("org.example.A".into());
        s.docks[0].switcher_changed(0);
        s.docks[0].done();
    });
    let result = wire
        .conn
        .roundtrip()
        .and_then(|()| wire.conn.wait(Some(std::time::Duration::ZERO)).map(|_| ()));
    assert!(matches!(
        result,
        Err(cupertino_wayland_client::Error::Protocol(_))
    ));
}

#[test]
fn the_overview_carries_its_mode_area_windows_and_workspaces() {
    let (mut wire, _dock) = dock_with(Offer::all(), |dock| dock.done());
    wire.events();
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
fn every_corner_and_gesture_is_reported() {
    let (mut wire, _dock) = dock_with(Offer::all(), |dock| dock.done());
    wire.events();
    wire.server(|s| {
        let dock = &s.docks[0];
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
        dock.done();
    });
    assert_eq!(
        changes(&mut wire),
        [vec![
            DockChange::HotCorner(Corner::TopLeft),
            DockChange::HotCorner(Corner::TopRight),
            DockChange::HotCorner(Corner::BottomLeft),
            DockChange::HotCorner(Corner::BottomRight),
            DockChange::Gesture(Gesture::SwipeUp),
            DockChange::Gesture(Gesture::SwipeDown),
            DockChange::Gesture(Gesture::Pinch),
            DockChange::Gesture(Gesture::Spread),
        ]]
    );
}

fn icon_group(
    id: u32,
    name: &'static str,
    size: u32,
    bytes: Vec<u8>,
) -> impl Fn(&CupertinoDockV1) + Send + 'static {
    move |dock| {
        let file = pixel_file(&bytes);
        dock.window_icon(id, name.into(), size, file.as_fd());
        dock.done();
    }
}

fn icon_of(wire: &mut Wire) -> DockWindowIcon {
    match changes(wire).remove(0).remove(0) {
        DockChange::WindowIcon(icon) => icon,
        other => panic!("not an icon: {other:?}"),
    }
}

#[test]
fn a_window_icon_is_the_file_read_whole() {
    let bytes: Vec<u8> = (0..2 * 2 * 4).collect();
    let (mut wire, _dock) = dock_with(Offer::all(), icon_group(5, "text-editor", 2, bytes.clone()));
    assert_eq!(
        icon_of(&mut wire),
        DockWindowIcon {
            id: 5,
            name: Some("text-editor".into()),
            image: Some(WindowIconImage {
                size: 2,
                argb: bytes
            }),
        }
    );
}

#[test]
fn a_window_icon_without_a_name_has_none() {
    let (mut wire, _dock) = dock_with(Offer::all(), icon_group(5, "", 1, vec![0; 4]));
    let icon = icon_of(&mut wire);
    assert_eq!(icon.name, None);
    assert!(icon.image.is_some());
}

#[test]
fn a_window_icon_that_is_empty_too_big_or_the_wrong_length_has_no_image() {
    // size 0
    let (mut wire, _dock) = dock_with(Offer::all(), icon_group(5, "x", 0, vec![]));
    assert_eq!(icon_of(&mut wire).image, None);
    // a file shorter than the size says
    let (mut wire, _dock) = dock_with(Offer::all(), icon_group(5, "x", 2, vec![0; 15]));
    assert_eq!(icon_of(&mut wire).image, None);
    // a file longer than the size says
    let (mut wire, _dock) = dock_with(Offer::all(), icon_group(5, "x", 2, vec![0; 17]));
    assert_eq!(icon_of(&mut wire).image, None);
    // over the largest side a dock accepts, with a file of exactly that size
    let side = 513u32;
    let (mut wire, _dock) = dock_with(
        Offer::all(),
        icon_group(5, "x", side, vec![0; (side * side * 4) as usize]),
    );
    assert_eq!(icon_of(&mut wire).image, None);
}

#[test]
fn a_window_icon_of_the_largest_side_is_accepted() {
    let side = 512u32;
    let (mut wire, _dock) = dock_with(
        Offer::all(),
        icon_group(5, "x", side, vec![7; (side * side * 4) as usize]),
    );
    let image = icon_of(&mut wire).image.expect("image");
    assert_eq!(image.size, 512);
    assert_eq!(image.argb.len(), 512 * 512 * 4);
}

fn thumbnail_group(
    width: u32,
    height: u32,
    bytes: Vec<u8>,
) -> impl Fn(&CupertinoDockV1) + Send + 'static {
    move |dock| {
        let file = pixel_file(&bytes);
        dock.window_thumbnail(6, width, height, file.as_fd());
        dock.done();
    }
}

fn thumbnail_of(wire: &mut Wire) -> DockWindowThumbnail {
    match changes(wire).remove(0).remove(0) {
        DockChange::WindowThumbnail(thumbnail) => thumbnail,
        other => panic!("not a thumbnail: {other:?}"),
    }
}

#[test]
fn a_window_thumbnail_is_the_file_read_whole() {
    let bytes: Vec<u8> = (0..4 * 3 * 4).collect();
    let (mut wire, _dock) = dock_with(Offer::all(), thumbnail_group(4, 3, bytes.clone()));
    assert_eq!(
        thumbnail_of(&mut wire),
        DockWindowThumbnail {
            id: 6,
            image: Some(ThumbnailImage {
                width: 4,
                height: 3,
                argb: bytes
            }),
        }
    );
}

#[test]
fn a_window_thumbnail_that_is_empty_too_big_or_the_wrong_length_has_no_image() {
    assert_eq!(MAX_WINDOW_THUMBNAIL, 256);
    let (mut wire, _dock) = dock_with(Offer::all(), thumbnail_group(0, 3, vec![]));
    assert_eq!(thumbnail_of(&mut wire).image, None);
    let (mut wire, _dock) = dock_with(Offer::all(), thumbnail_group(3, 0, vec![]));
    assert_eq!(thumbnail_of(&mut wire).image, None);
    let (mut wire, _dock) = dock_with(Offer::all(), thumbnail_group(2, 2, vec![0; 15]));
    assert_eq!(thumbnail_of(&mut wire).image, None);
    let (mut wire, _dock) = dock_with(Offer::all(), thumbnail_group(257, 1, vec![0; 257 * 4]));
    assert_eq!(thumbnail_of(&mut wire).image, None);
    let (mut wire, _dock) = dock_with(Offer::all(), thumbnail_group(1, 257, vec![0; 257 * 4]));
    assert_eq!(thumbnail_of(&mut wire).image, None);
}

#[test]
fn a_window_thumbnail_of_the_largest_side_is_accepted() {
    let (mut wire, _dock) = dock_with(Offer::all(), thumbnail_group(256, 1, vec![1; 256 * 4]));
    let image = thumbnail_of(&mut wire).image.expect("image");
    assert_eq!((image.width, image.height), (256, 1));
}

#[test]
fn the_requests_of_the_dock_carry_their_arguments() {
    let (mut wire, dock) = dock_with(Offer::all(), |dock| dock.done());
    ready(&mut wire);

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
    dock.app_command("org.example.A", AppCommand::Hide);
    dock.switcher_select(2).unwrap();
    dock.switcher_activate(1).unwrap();
    dock.move_workspace(5, 0).unwrap();
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
    assert!(matches!(
        &requests[10],
        Request::AppCommand { app_id, command }
            if app_id == "org.example.A" && u32::from(*command) == 1
    ));
    assert!(matches!(requests[11], Request::SwitcherSelect { index: 2 }));
    assert!(matches!(
        requests[12],
        Request::SwitcherActivate { index: 1 }
    ));
    assert!(matches!(
        requests[13],
        Request::MoveWorkspace { id: 5, index: 0 }
    ));
    assert_eq!(requests.len(), 14);
}

#[test]
fn every_app_command_has_its_own_wire_value() {
    let (mut wire, dock) = dock_with(Offer::all(), |dock| dock.done());
    ready(&mut wire);
    for command in [AppCommand::ShowWindows, AppCommand::Hide, AppCommand::Quit] {
        dock.app_command("a", command);
    }
    wire.events();
    let values: Vec<u32> = dock_requests(&wire)
        .into_iter()
        .map(|r| match r {
            Request::AppCommand { command, .. } => u32::from(command),
            other => panic!("unexpected {other:?}"),
        })
        .collect();
    assert_eq!(values, [0, 1, 2]);
}

#[test]
fn rectangles_are_rounded_outwards_to_whole_points() {
    let (mut wire, dock) = dock_with(Offer::all(), |dock| dock.done());
    ready(&mut wire);
    let rect = Rect::new(0.5, 0.25, 10.0, 10.0);

    dock.restore_window(3, &rect).unwrap();
    dock.set_minimize_target(&rect).unwrap();
    dock.overview_frame(4, &rect).unwrap();
    dock.overview_drag(5, &rect).unwrap();
    dock.overview_workspace_frame(6, &rect).unwrap();
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
}

#[test]
fn a_rectangle_that_is_not_finite_is_refused_and_nothing_is_sent() {
    let (mut wire, dock) = dock_with(Offer::all(), |dock| dock.done());
    ready(&mut wire);
    for bad in [
        Rect::new(f64::NAN, 0.0, 1.0, 1.0),
        Rect::new(0.0, f64::INFINITY, 1.0, 1.0),
        Rect::new(0.0, 0.0, f64::INFINITY, 1.0),
        Rect::new(0.0, 0.0, 1.0, f64::NAN),
    ] {
        assert!(matches!(
            dock.restore_window(1, &bad),
            Err(Error::InvalidShape)
        ));
        assert!(matches!(
            dock.set_minimize_target(&bad),
            Err(Error::InvalidShape)
        ));
        assert!(matches!(
            dock.overview_frame(1, &bad),
            Err(Error::InvalidShape)
        ));
        assert!(matches!(
            dock.overview_drag(1, &bad),
            Err(Error::InvalidShape)
        ));
        assert!(matches!(
            dock.overview_workspace_frame(1, &bad),
            Err(Error::InvalidShape)
        ));
    }
    wire.events();
    assert!(dock_requests(&wire).is_empty());
}

#[test]
fn an_empty_workspace_frame_sends_zeros_to_clear_it() {
    let (mut wire, dock) = dock_with(Offer::all(), |dock| dock.done());
    ready(&mut wire);
    dock.overview_workspace_frame(2, &Rect::new(5.0, 5.0, 0.0, 10.0))
        .unwrap();
    dock.overview_workspace_frame(2, &Rect::new(5.0, 5.0, 10.0, -1.0))
        .unwrap();
    wire.events();
    let requests = dock_requests(&wire);
    for request in &requests {
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
    assert_eq!(requests.len(), 2);
}

#[test]
fn indices_that_do_not_fit_the_wire_are_refused() {
    let (mut wire, dock) = dock_with(Offer::all(), |dock| dock.done());
    ready(&mut wire);
    let too_big = u32::MAX as usize + 1;
    assert!(matches!(
        dock.switcher_select(too_big),
        Err(Error::InvalidShape)
    ));
    assert!(matches!(
        dock.switcher_activate(too_big),
        Err(Error::InvalidShape)
    ));
    assert!(matches!(
        dock.move_workspace(1, too_big),
        Err(Error::InvalidShape)
    ));
    dock.switcher_select(u32::MAX as usize).unwrap();
    wire.events();
    assert!(matches!(
        dock_requests(&wire)[..],
        [Request::SwitcherSelect { index: u32::MAX }]
    ));
}

#[test]
fn a_server_older_than_the_overview_drag_is_sent_nothing_for_it() {
    let (mut wire, dock) = dock_with(Offer::shell(22), |dock| dock.done());
    ready(&mut wire);
    let rect = Rect::new(0.0, 0.0, 10.0, 10.0);
    dock.overview_drag(1, &rect).unwrap();
    dock.overview_move_window(1, 2);
    dock.overview_workspace_frame(2, &rect).unwrap();
    dock.overview_frame(1, &rect).unwrap();
    wire.events();
    let requests = dock_requests(&wire);
    assert!(matches!(requests[..], [Request::OverviewFrame { .. }]));
}

#[test]
fn the_overview_drag_requests_are_sent_from_version_twenty_three() {
    let (mut wire, dock) = dock_with(Offer::shell(23), |dock| dock.done());
    ready(&mut wire);
    dock.overview_move_window(8, 2);
    wire.events();
    assert!(matches!(
        dock_requests(&wire)[..],
        [Request::OverviewMoveWindow {
            id: 8,
            workspace: 2
        }]
    ));
}

#[test]
fn showing_one_window_needs_version_twenty_six() {
    let (mut wire, dock) = dock_with(Offer::shell(25), |dock| dock.done());
    ready(&mut wire);
    assert!(!dock.activate_window(3));
    wire.events();
    assert!(dock_requests(&wire).is_empty());
}

#[test]
fn dropping_the_dock_gives_up_the_role() {
    let (mut wire, dock) = dock_with(Offer::all(), |dock| dock.done());
    ready(&mut wire);
    drop(dock);
    wire.events();
    assert!(matches!(&dock_requests(&wire)[..], [Request::Destroy]));
}

#[test]
fn a_dock_for_an_unplugged_output_is_refused() {
    let (mut wire, _dock) = dock_with(Offer::all(), |dock| dock.done());
    let shell = Shell::bind(&mut wire.conn).expect("shell");
    let layers = LayerShell::bind(&mut wire.conn).expect("layer shell");
    let surface = layers
        .layer_surface(
            &mut wire.conn,
            &LayerSurfaceConfig {
                namespace: "dock".into(),
                output: None,
                layer: Layer::Top,
                anchor: Anchor::Bottom,
                size: (1, 1),
                margin: Margin::default(),
                exclusive_zone: 0,
                keyboard: KeyboardInteractivity::None,
            },
        )
        .expect("surface")
        .surface();
    let result = shell.dock(
        &wire.conn,
        surface,
        cupertino_wayland_client::OutputId::from_registry_name(9999),
    );
    assert!(matches!(result, Err(Error::UnknownOutput)));
}
