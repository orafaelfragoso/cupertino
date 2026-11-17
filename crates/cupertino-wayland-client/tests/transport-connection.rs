//! The transport's connection against an in-process server: capability discovery, window configures, shm buffer
//! leases, the wake channel and the end of a connection.

mod support;

use cupertino_wayland_client::{
    BufferFormat, Capability, Connection, Decorations, Disconnect, Error, Event, Scale,
    ToplevelConfig, Waker,
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
fn capabilities_list_the_optional_protocols_the_server_offers() {
    let (_server, conn) = Harness::start(Offer::NONE);
    let offered: Vec<_> = conn.capabilities().iter().collect();
    assert_eq!(offered, [Capability::Seat]);
    assert!(conn.capabilities().extensions().is_empty());
}

#[test]
fn bound_extensions_are_listed_in_order() {
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
fn only_the_two_required_buffer_formats_are_offered_without_an_announcement() {
    let (_server, conn) = Harness::start(Offer::NONE);
    assert!(conn.offers(BufferFormat::Argb8888));
    assert!(conn.offers(BufferFormat::Xrgb8888));
    assert!(!conn.offers(BufferFormat::Abgr8888));
}

#[test]
fn a_connection_without_outputs_lists_none_and_presents_no_clock() {
    let (_server, conn) = Harness::start(Offer::NONE);
    assert!(conn.outputs().is_empty());
    assert_eq!(conn.presentation_clock(), None);
    assert_eq!(conn.input_serial(), None);
    assert!(conn.held_press().is_none());
    assert_eq!(conn.modifiers(), Default::default());
}

#[test]
fn a_new_window_is_configured_by_the_server() {
    let (_server, mut conn) = Harness::start(Offer::NONE);
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
}

#[test]
fn a_configure_carries_size_bounds_states_and_capabilities() {
    let (server, mut conn) = Harness::start(Offer::NONE);
    let window = window(&mut conn);
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
    assert!(configure.states.size_is_fixed());
    let capabilities = configure.capabilities.expect("the server said");
    assert!(capabilities.window_menu && capabilities.maximize);
    assert!(!capabilities.fullscreen && !capabilities.minimize);
    let _ = window;
}

#[test]
fn a_zero_extent_leaves_the_size_to_the_client() {
    let (server, mut conn) = Harness::start(Offer::NONE);
    let _window = window(&mut conn);
    server.run(|s| {
        let (toplevel, surface) = &s.toplevels[0];
        toplevel.configure(0, 600, Vec::new());
        surface.configure(60);
    });
    let configure = wait_for(&mut conn, |e| match e {
        Event::Configure { configure, .. } if configure.serial == 60 => Some(*configure),
        _ => None,
    });
    assert_eq!(configure.size, None);
}

#[test]
fn a_negative_extent_fails_the_connection() {
    let (server, mut conn) = Harness::start(Offer::NONE);
    let _window = window(&mut conn);
    server.run(|s| {
        let (toplevel, surface) = &s.toplevels[0];
        toplevel.configure(-5, 100, Vec::new());
        surface.configure(61);
    });
    let error = loop {
        if let Err(e) = conn.wait(Some(STEP)) {
            break e;
        }
    };
    assert!(matches!(error, Error::Protocol(_)), "{error}");
}

#[test]
fn a_configure_serial_that_does_not_increase_is_an_error() {
    let (server, mut conn) = Harness::start(Offer::NONE);
    let _window = window(&mut conn);
    server.run(|s| {
        let surface = &s.toplevels[0].1;
        surface.configure(10);
        surface.configure(5);
    });
    let error = loop {
        if let Err(e) = conn.wait(Some(STEP)) {
            break e;
        }
    };
    assert!(
        matches!(
            error,
            Error::InvalidSerial {
                serial: 5,
                latest: 10
            }
        ),
        "{error}"
    );
}

#[test]
fn the_close_request_names_the_window() {
    let (server, mut conn) = Harness::start(Offer::NONE);
    let window = window(&mut conn);
    server.run(|s| s.toplevels[0].0.close());
    let closed = wait_for(&mut conn, |e| match e {
        Event::Close(w) => Some(*w),
        _ => None,
    });
    assert_eq!(closed, window);
}

#[test]
fn a_surface_cannot_be_drawn_before_its_first_configure() {
    let (_server, mut conn) = Harness::start(Offer::NONE);
    let window = conn.toplevel(&config()).unwrap();
    assert!(matches!(
        conn.lease(window.surface(), (10, 10), BufferFormat::Argb8888),
        Err(Error::NotConfigured)
    ));
}

#[test]
fn a_lease_is_the_preferred_size_at_the_surface_scale_when_the_client_chooses() {
    let (_server, mut conn) = Harness::start(Offer::NONE);
    let window = window(&mut conn);
    assert_eq!(conn.scale(window.surface()).unwrap(), Scale::ONE);
    let mut lease = conn
        .lease(window.surface(), (64, 32), BufferFormat::Argb8888)
        .unwrap();
    assert_eq!(lease.logical_size(), (64, 32));
    assert_eq!(lease.scale(), Scale::ONE);
    let layout = lease.layout();
    assert_eq!((layout.width(), layout.height()), (64, 32));
    assert_eq!(lease.pixels_mut().len(), 64 * 32 * 4);
    assert_eq!(lease.missed(), None);
}

#[test]
fn a_lease_takes_the_configured_size_over_the_preferred_one() {
    let (server, mut conn) = Harness::start(Offer::NONE);
    let window = window(&mut conn);
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
fn pixels_written_to_a_lease_can_be_read_back_from_the_same_buffer() {
    let (_server, mut conn) = Harness::start(Offer::NONE);
    let window = window(&mut conn);
    {
        let mut lease = conn
            .lease(window.surface(), (4, 4), BufferFormat::Argb8888)
            .unwrap();
        lease.pixels_mut().fill(0xAB);
        // dropped without presenting: the buffer goes back
    }
    let mut lease = conn
        .lease(window.surface(), (4, 4), BufferFormat::Argb8888)
        .unwrap();
    assert!(lease.pixels_mut().iter().all(|&b| b == 0xAB));
}

#[test]
fn a_lease_of_a_size_that_does_not_fit_a_buffer_is_refused() {
    let (_server, mut conn) = Harness::start(Offer::NONE);
    let window = window(&mut conn);
    let too_wide = cupertino_wayland_client::BufferLayout::MAX_EXTENT + 1;
    assert!(matches!(
        conn.lease(window.surface(), (too_wide, 1), BufferFormat::Argb8888),
        Err(Error::InvalidScale)
    ));
    assert!(matches!(
        conn.lease(window.surface(), (0, 1), BufferFormat::Argb8888),
        Err(Error::InvalidScale)
    ));
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
    let (_server, mut conn) = Harness::start(Offer::NONE);
    let window = window(&mut conn);
    let lease = conn
        .lease(window.surface(), (8, 8), BufferFormat::Argb8888)
        .unwrap();
    let damage = [
        cupertino_wayland_client::PixelRect {
            x: 4,
            y: 4,
            width: 100,
            height: 100,
        },
        cupertino_wayland_client::PixelRect {
            x: 50,
            y: 50,
            width: 1,
            height: 1,
        },
    ];
    lease.present_damage(Some(&damage)).unwrap();
    conn.roundtrip().unwrap();
}

#[test]
fn a_destroyed_surface_is_unknown_to_every_request() {
    let (_server, mut conn) = Harness::start(Offer::NONE);
    let window = window(&mut conn);
    let surface = window.surface();
    conn.destroy(surface).unwrap();
    assert!(matches!(conn.destroy(surface), Err(Error::UnknownSurface)));
    assert!(matches!(conn.scale(surface), Err(Error::UnknownSurface)));
    assert!(matches!(
        conn.lease(surface, (1, 1), BufferFormat::Argb8888),
        Err(Error::UnknownSurface)
    ));
    assert!(matches!(
        conn.set_title(window, "gone"),
        Err(Error::UnknownSurface)
    ));
    assert!(matches!(conn.minimize(window), Err(Error::UnknownSurface)));
    assert!(matches!(
        conn.buffer_free(surface),
        Err(Error::UnknownSurface)
    ));
    assert!(matches!(
        conn.wl_surface(surface),
        Err(Error::UnknownSurface)
    ));
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
    assert!(matches!(
        conn.present_solid(window.surface(), (10, 10), [0.0, 0.0, 0.0, 1.0]),
        Err(Error::Unsupported(Capability::SolidColor))
    ));
    assert!(matches!(
        conn.subsurface(window.surface()),
        Err(Error::Unsupported(Capability::Subsurface))
    ));
    assert!(matches!(
        conn.activate(window),
        Err(Error::Unsupported(Capability::Activation))
    ));
}

#[test]
fn an_idle_wait_returns_empty_after_its_timeout() {
    let (_server, mut conn) = Harness::start(Offer::NONE);
    let started = Instant::now();
    let events = conn.wait(Some(Duration::from_millis(80))).unwrap();
    assert!(events.is_empty());
    assert!(started.elapsed() >= Duration::from_millis(70));
}

#[test]
fn a_waker_ends_a_wait_from_another_thread() {
    let (_server, mut conn) = Harness::start(Offer::NONE);
    let waker: Waker = conn.waker();
    let thread = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(50));
        waker.wake();
    });
    let started = Instant::now();
    let events = conn.wait(Some(Duration::from_secs(10))).unwrap();
    assert!(started.elapsed() < Duration::from_secs(5));
    assert!(events.is_empty());
    thread.join().unwrap();
}

#[test]
fn a_wake_before_the_wait_is_not_lost() {
    let (_server, mut conn) = Harness::start(Offer::NONE);
    conn.waker().wake();
    conn.waker().wake();
    let started = Instant::now();
    conn.wait(Some(Duration::from_secs(10))).unwrap();
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[test]
fn waking_after_the_connection_is_gone_is_harmless() {
    let (_server, conn) = Harness::start(Offer::NONE);
    let waker = conn.waker();
    drop(conn);
    waker.wake();
}

#[test]
fn a_server_that_goes_away_ends_the_connection() {
    let (mut server, mut conn) = Harness::start(Offer::NONE);
    server.hang_up();
    let error = loop {
        match conn.wait(Some(STEP)) {
            Err(e) => break e,
            Ok(_) => continue,
        }
    };
    assert!(matches!(error, Error::Disconnected(_)), "{error}");
    assert!(error
        .to_string()
        .starts_with("disconnected from the compositor"));
    // nothing more can be sent on it
    assert!(conn.roundtrip().is_err());
}

#[test]
fn a_disconnect_says_why_in_the_text_of_its_cause() {
    let (mut server, mut conn) = Harness::start(Offer::NONE);
    server.hang_up();
    let error = loop {
        if let Err(e) = conn.wait(Some(STEP)) {
            break e;
        }
    };
    let Error::Disconnected(cause) = error else {
        panic!("not a disconnect: {error}");
    };
    assert!(
        matches!(cause, Disconnect::Dispatch(_) | Disconnect::Backend(_)),
        "{cause}"
    );
    assert!(!cause.to_string().is_empty());
}
