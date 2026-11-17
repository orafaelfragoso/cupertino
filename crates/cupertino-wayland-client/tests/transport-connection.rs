//! The transport's connection against an in-process server: capability discovery, window configures, shm buffer
//! leases, the wake channel and the end of a connection.

mod support;

use cupertino_wayland_client::{
    BufferFormat, BufferLayout, Capability, Connection, Decorations, Disconnect, Error, Event,
    PixelRect, Scale, ToplevelConfig,
};
use std::time::{Duration, Instant};
use support::*;

const STEP: Duration = Duration::from_millis(50);

fn config() -> ToplevelConfig {
    ToplevelConfig {
        app_id: "org.example.test".into(),
        title: "Test".into(),
        min_size: None,
        max_size: None,
        decorations: Decorations::Client,
        foreign_parent: None,
    }
}

/// States as the array of an `xdg_toplevel.configure`: native-endian `u32` values.
fn states(values: &[u32]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_ne_bytes()).collect()
}

/// Wait for the next event that `pick` takes, returning it.
fn wait_for<T>(conn: &mut Connection, pick: impl Fn(&Event) -> Option<T>) -> T {
    for _ in 0..100 {
        for event in conn.wait(Some(STEP)).unwrap() {
            if let Some(found) = pick(&event) {
                return found;
            }
        }
    }
    panic!("the event did not arrive within 5 s");
}

/// Waits until the connection fails, returning the error.
fn wait_for_error(conn: &mut Connection) -> Error {
    loop {
        if let Err(e) = conn.wait(Some(STEP)) {
            break e;
        }
    }
}

#[test]
fn a_server_without_the_window_shell_is_refused() {
    let (_server, result) = Harness::try_start(Offer {
        shell: false,
        ..Offer::NONE
    });
    match result {
        Err(Error::MissingRequiredProtocol {
            interface,
            minimum_version,
        }) => {
            assert_eq!(interface, "xdg_wm_base");
            assert!(minimum_version >= 1);
        }
        Err(e) => panic!("{e}"),
        Ok(_) => panic!("connected without a window shell"),
    }
}

#[test]
fn a_connection_reports_what_the_server_offers_and_what_it_bound() {
    let (_server, conn) = Harness::start(Offer::NONE);
    let offered: Vec<_> = conn.capabilities().iter().collect();
    assert_eq!(offered, [Capability::Seat]);
    assert!(conn.capabilities().extensions().is_empty());
    assert!(conn.offers(BufferFormat::Argb8888));
    assert!(conn.offers(BufferFormat::Xrgb8888));
    assert!(
        !conn.offers(BufferFormat::Abgr8888),
        "only the two required formats without an announcement"
    );
    assert!(conn.outputs().is_empty());
    assert_eq!(conn.presentation_clock(), None);
    assert_eq!(conn.input_serial(), None);
    assert!(conn.held_press().is_none());
    assert_eq!(conn.modifiers(), Default::default());

    let (_server, mut conn) = Harness::start(Offer::ALL);
    assert!(cupertino_wayland_client::app::Layers::bind(&mut conn).is_some());
    assert!(cupertino_wayland_client::app::ScrollCounts::bind(&mut conn).is_some());
    let listed: Vec<_> = conn
        .capabilities()
        .extensions()
        .iter()
        .map(|e| (e.interface, e.version))
        .collect();
    assert_eq!(
        listed,
        [("cupertino_layers_v1", 6), ("cupertino_scroll_v1", 1)]
    );
}

#[test]
fn a_window_takes_the_servers_configures_and_close_request() {
    let (server, mut conn) = Harness::start(Offer::NONE);
    let window = conn.toplevel(&config()).unwrap();
    let (configured, configure) = wait_for(&mut conn, |e| match e {
        Event::Configure { window, configure } => Some((*window, *configure)),
        _ => None,
    });
    assert_eq!(configured, window);
    assert_eq!(configure.size, None);
    assert_eq!(configure.bounds, None);
    assert!(!configure.states.size_is_fixed());
    assert_eq!(configure.decorations, Decorations::Client);
    assert_eq!(configure.capabilities, None);

    server.run(|s| {
        let (toplevel, surface) = &s.toplevels[0];
        toplevel.configure_bounds(1600, 900);
        // maximized, activated, an unknown state
        toplevel.configure(800, 600, states(&[1, 4, 99]));
        // window menu, maximize
        toplevel.wm_capabilities(states(&[1, 2]));
        surface.configure(50);
    });
    let configure = wait_for(&mut conn, |e| match e {
        Event::Configure { configure, .. } if configure.serial == 50 => Some(*configure),
        _ => None,
    });
    assert_eq!(configure.size, Some((800, 600)));
    assert_eq!(configure.bounds, Some((1600, 900)));
    assert!(configure.states.maximized && configure.states.activated);
    assert!(!configure.states.fullscreen && !configure.states.suspended);
    let capabilities = configure.capabilities.expect("the server said");
    assert!(capabilities.window_menu && capabilities.maximize);
    assert!(!capabilities.fullscreen && !capabilities.minimize);

    server.run(|s| {
        let (toplevel, surface) = &s.toplevels[0];
        toplevel.configure(0, 600, Vec::new());
        surface.configure(60);
    });
    let configure = wait_for(&mut conn, |e| match e {
        Event::Configure { configure, .. } if configure.serial == 60 => Some(*configure),
        _ => None,
    });
    assert_eq!(
        configure.size, None,
        "a zero extent leaves the size to the client"
    );

    server.run(|s| s.toplevels[0].0.close());
    let closed = wait_for(&mut conn, |e| match e {
        Event::Close(w) => Some(*w),
        _ => None,
    });
    assert_eq!(closed, window);
}

#[test]
fn a_malformed_configure_fails_the_connection() {
    type Send = fn(&mut Server);
    let rows: [(&str, Send, fn(&Error) -> bool); 2] = [
        (
            "a negative extent",
            |s| {
                let (toplevel, surface) = &s.toplevels[0];
                toplevel.configure(-5, 100, Vec::new());
                surface.configure(61);
            },
            |e| matches!(e, Error::Protocol(_)),
        ),
        (
            "a serial that does not increase",
            |s| {
                let surface = &s.toplevels[0].1;
                surface.configure(10);
                surface.configure(5);
            },
            |e| {
                matches!(
                    e,
                    Error::InvalidSerial {
                        serial: 5,
                        latest: 10
                    }
                )
            },
        ),
    ];
    for (row, send, expected) in rows {
        let (server, mut conn) = Harness::start(Offer::NONE);
        let _window = window(&mut conn);
        server.run(send);
        let error = wait_for_error(&mut conn);
        assert!(expected(&error), "{row}: {error}");
    }
}

#[test]
fn a_lease_is_the_configured_size_or_else_the_preferred_one_at_the_surface_scale() {
    let (server, mut conn) = Harness::start(Offer::NONE);
    let window = window(&mut conn);
    assert_eq!(conn.scale(window.surface()).unwrap(), Scale::ONE);
    {
        let mut lease = conn
            .lease(window.surface(), (64, 32), BufferFormat::Argb8888)
            .unwrap();
        assert_eq!(lease.logical_size(), (64, 32));
        assert_eq!(lease.scale(), Scale::ONE);
        let layout = lease.layout();
        assert_eq!((layout.width(), layout.height()), (64, 32));
        assert_eq!(lease.pixels_mut().len(), 64 * 32 * 4);
        assert_eq!(lease.missed(), None);
        lease.pixels_mut().fill(0xAB);
        // dropped without presenting: the buffer goes back
    }
    let mut lease = conn
        .lease(window.surface(), (64, 32), BufferFormat::Argb8888)
        .unwrap();
    assert!(
        lease.pixels_mut().iter().all(|&b| b == 0xAB),
        "the pixels written to a lease are in the same buffer"
    );
    drop(lease);

    server.run(|s| {
        let (toplevel, surface) = &s.toplevels[0];
        toplevel.configure(120, 80, Vec::new());
        surface.configure(70);
    });
    wait_for(&mut conn, |e| {
        matches!(e, Event::Configure { configure, .. } if configure.serial == 70).then_some(())
    });
    let lease = conn
        .lease(window.surface(), (10, 10), BufferFormat::Xrgb8888)
        .unwrap();
    assert_eq!(lease.logical_size(), (120, 80));
    assert_eq!(lease.serial(), 70);
    assert_eq!(lease.layout().format(), BufferFormat::Xrgb8888);
}

#[test]
fn a_lease_is_refused_before_the_first_configure_or_at_a_size_no_buffer_holds() {
    let (_server, mut conn) = Harness::start(Offer::NONE);
    let unconfigured = conn.toplevel(&config()).unwrap();
    assert!(matches!(
        conn.lease(unconfigured.surface(), (10, 10), BufferFormat::Argb8888),
        Err(Error::NotConfigured)
    ));
    let window = window(&mut conn);
    for size in [(BufferLayout::MAX_EXTENT + 1, 1), (0, 1)] {
        assert!(
            matches!(
                conn.lease(window.surface(), size, BufferFormat::Argb8888),
                Err(Error::InvalidScale)
            ),
            "{size:?}"
        );
    }
}

#[test]
fn a_server_that_never_releases_leaves_three_frames_in_flight() {
    let (_server, mut conn) = Harness::start(Offer::NONE);
    let window = window(&mut conn);
    let surface = window.surface();
    assert!(conn.buffer_free(surface).unwrap());
    for frame in 0..3 {
        let mut lease = conn
            .lease(surface, (8, 8), BufferFormat::Argb8888)
            .unwrap_or_else(|e| panic!("frame {frame}: {e}"));
        lease.pixels_mut().fill(frame);
        lease.present().unwrap();
    }
    assert!(!conn.buffer_free(surface).unwrap());
    assert!(matches!(
        conn.lease(surface, (8, 8), BufferFormat::Argb8888),
        Err(Error::NoFreeBuffer)
    ));
}

#[test]
fn damage_outside_the_buffer_is_accepted_and_clipped() {
    let (server, mut conn) = Harness::start(Offer::NONE);
    let window = window(&mut conn);
    let lease = conn
        .lease(window.surface(), (8, 8), BufferFormat::Argb8888)
        .unwrap();
    let damage = [
        PixelRect {
            x: 4,
            y: 4,
            width: 100,
            height: 100,
        },
        PixelRect {
            x: 50,
            y: 50,
            width: 1,
            height: 1,
        },
    ];
    lease.present_damage(Some(&damage)).unwrap();
    conn.roundtrip().unwrap();
    let damage: Vec<String> = server
        .requests()
        .into_iter()
        .filter(|r| r.starts_with("DamageBuffer"))
        .collect();
    assert_eq!(
        damage,
        ["DamageBuffer { x: 4, y: 4, width: 4, height: 4 }"],
        "the first rectangle is cut to the buffer and the one wholly outside is dropped"
    );
}

#[test]
fn a_destroyed_surface_is_unknown_to_every_request() {
    let (_server, mut conn) = Harness::start(Offer::NONE);
    let window = window(&mut conn);
    let surface = window.surface();
    conn.destroy(surface).unwrap();
    let rows: [(&str, bool); 7] = [
        (
            "destroy",
            matches!(conn.destroy(surface), Err(Error::UnknownSurface)),
        ),
        (
            "scale",
            matches!(conn.scale(surface), Err(Error::UnknownSurface)),
        ),
        (
            "lease",
            matches!(
                conn.lease(surface, (1, 1), BufferFormat::Argb8888),
                Err(Error::UnknownSurface)
            ),
        ),
        (
            "set_title",
            matches!(conn.set_title(window, "gone"), Err(Error::UnknownSurface)),
        ),
        (
            "minimize",
            matches!(conn.minimize(window), Err(Error::UnknownSurface)),
        ),
        (
            "buffer_free",
            matches!(conn.buffer_free(surface), Err(Error::UnknownSurface)),
        ),
        (
            "wl_surface",
            matches!(conn.wl_surface(surface), Err(Error::UnknownSurface)),
        ),
    ];
    for (row, unknown) in rows {
        assert!(unknown, "{row}");
    }
}

#[test]
fn a_popup_grab_before_any_input_is_refused_and_the_connection_goes_on() {
    use cupertino_geometry::Rect;
    use cupertino_wayland_client::{Anchor, ConstraintAdjustment, Gravity, PopupPlacement};
    let (_server, mut conn) = Harness::start(Offer::NONE);
    let window = window(&mut conn);
    let placement = PopupPlacement {
        anchor_rect: Rect::new(0.0, 0.0, 10.0, 10.0),
        size: (50, 50),
        anchor: Anchor::BottomLeft,
        gravity: Gravity::BottomRight,
        constraint_adjustment: ConstraintAdjustment::empty(),
        offset: (0, 0),
        reactive: false,
        parent_size: None,
        grab: true,
    };
    assert!(matches!(
        conn.popup(window.surface(), &placement),
        Err(Error::PermissionDenied(_))
    ));
    conn.roundtrip().unwrap();
}

#[test]
fn an_optional_request_names_the_capability_the_server_lacks() {
    let (_server, mut conn) = Harness::start(Offer::NONE);
    let window = window(&mut conn);
    let rows = [
        (
            Capability::SolidColor,
            conn.present_solid(window.surface(), (10, 10), [0.0, 0.0, 0.0, 1.0])
                .map(drop),
        ),
        (
            Capability::Subsurface,
            conn.subsurface(window.surface()).map(drop),
        ),
        (Capability::Activation, conn.activate(window).map(drop)),
    ];
    for (capability, result) in rows {
        assert!(
            matches!(result, Err(Error::Unsupported(c)) if c == capability),
            "{capability:?}"
        );
    }
}

#[test]
fn a_wait_ends_at_its_timeout_or_at_a_wake_from_any_thread_before_or_during_it() {
    let (_server, mut conn) = Harness::start(Offer::NONE);
    let started = Instant::now();
    let events = conn.wait(Some(Duration::from_millis(80))).unwrap();
    assert!(events.is_empty());
    assert!(
        started.elapsed() >= Duration::from_millis(70),
        "an idle wait lasts its timeout"
    );

    let waker = conn.waker();
    let thread = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(50));
        waker.wake();
    });
    let started = Instant::now();
    let events = conn.wait(Some(Duration::from_secs(10))).unwrap();
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "a wake from another thread ends the wait"
    );
    assert!(events.is_empty());
    thread.join().unwrap();

    conn.waker().wake();
    conn.waker().wake();
    let started = Instant::now();
    conn.wait(Some(Duration::from_secs(10))).unwrap();
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "a wake before the wait is not lost"
    );

    let waker = conn.waker();
    drop(conn);
    waker.wake();
}

#[test]
fn a_server_that_goes_away_ends_the_connection_and_says_why() {
    let (mut server, mut conn) = Harness::start(Offer::NONE);
    server.hang_up();
    let error = wait_for_error(&mut conn);
    let Error::Disconnected(cause) = &error else {
        panic!("not a disconnect: {error}");
    };
    assert!(
        matches!(cause, Disconnect::Dispatch(_) | Disconnect::Backend(_)),
        "{cause}"
    );
    assert!(!cause.to_string().is_empty());
    assert!(conn.roundtrip().is_err(), "nothing more can be sent on it");
}
