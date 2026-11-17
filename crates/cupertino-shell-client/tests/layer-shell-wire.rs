//! Layer surfaces over a real wire: what the request sends, how a configure reaches the caller and how the size to draw
//! at is picked.

mod support;

use cupertino_shell_client::{
    Anchor, Error, KeyboardInteractivity, Layer, LayerShell, LayerSurfaceConfig, Margin, ShellEvent,
};
use cupertino_wayland_client::OutputId;
use support::{Offer, Seen, Wire};
use wayland_protocols_wlr::layer_shell::v1::server::zwlr_layer_surface_v1::Request;

fn config() -> LayerSurfaceConfig {
    LayerSurfaceConfig {
        namespace: "test-bar".into(),
        output: None,
        layer: Layer::Top,
        anchor: Anchor::Top | Anchor::Left | Anchor::Right,
        size: (0, 24),
        margin: Margin {
            top: 1,
            right: 2,
            bottom: 3,
            left: 4,
        },
        exclusive_zone: 24,
        keyboard: KeyboardInteractivity::OnDemand,
    }
}

fn layer_requests(wire: &Wire) -> Vec<Request> {
    wire.take()
        .into_iter()
        .filter_map(|s| match s {
            Seen::Layer(r) => Some(r),
            _ => None,
        })
        .collect()
}

#[test]
fn the_global_binds_only_at_version_four() {
    let mut wire = Wire::new(Offer {
        layer_shell: Some(3),
        ..Offer::all()
    });
    assert!(LayerShell::bind(&mut wire.conn).is_none());
    let mut wire = Wire::new(Offer {
        layer_shell: None,
        ..Offer::all()
    });
    assert!(LayerShell::bind(&mut wire.conn).is_none());
    let mut wire = Wire::all();
    assert!(LayerShell::bind(&mut wire.conn).is_some());
}

#[test]
fn creating_a_surface_sends_every_setting_of_the_config() {
    let mut wire = Wire::all();
    let shell = LayerShell::bind(&mut wire.conn).expect("layer shell");
    let output = wire.conn.outputs()[0].id;
    let mut config = config();
    config.output = Some(output);
    shell
        .layer_surface(&mut wire.conn, &config)
        .expect("surface");
    wire.events();

    let seen = wire.take();
    assert!(matches!(
        &seen[0],
        Seen::LayerSurfaceCreated { namespace, layer, on_output: true }
            if namespace == "test-bar" && *layer == Layer::Top as u32
    ));
    let requests: Vec<_> = seen[1..]
        .iter()
        .filter_map(|s| match s {
            Seen::Layer(r) => Some(r),
            _ => None,
        })
        .collect();
    assert!(matches!(
        requests[0],
        Request::SetSize {
            width: 0,
            height: 24
        }
    ));
    assert!(matches!(
        requests[1],
        Request::SetAnchor { anchor } if u32::from(*anchor) == 1 | 4 | 8
    ));
    assert!(matches!(
        requests[2],
        Request::SetMargin {
            top: 1,
            right: 2,
            bottom: 3,
            left: 4
        }
    ));
    assert!(matches!(
        requests[3],
        Request::SetExclusiveZone { zone: 24 }
    ));
    assert!(matches!(
        requests[4],
        Request::SetKeyboardInteractivity { keyboard_interactivity }
            if u32::from(*keyboard_interactivity) == KeyboardInteractivity::OnDemand as u32
    ));
}

#[test]
fn a_surface_without_an_output_lets_the_server_choose() {
    let mut wire = Wire::all();
    let shell = LayerShell::bind(&mut wire.conn).expect("layer shell");
    shell
        .layer_surface(&mut wire.conn, &config())
        .expect("surface");
    wire.events();
    assert!(matches!(
        wire.take()[0],
        Seen::LayerSurfaceCreated {
            on_output: false,
            ..
        }
    ));
}

#[test]
fn an_unplugged_output_is_refused_before_anything_is_sent() {
    let mut wire = Wire::all();
    let shell = LayerShell::bind(&mut wire.conn).expect("layer shell");
    let mut config = config();
    config.output = Some(OutputId::from_registry_name(9999));
    let result = shell.layer_surface(&mut wire.conn, &config);
    assert!(matches!(result, Err(Error::UnknownOutput)));
    wire.events();
    assert!(wire.take().is_empty());
}

#[test]
fn a_configure_is_reported_with_the_size_as_sent() {
    let mut wire = Wire::all();
    let shell = LayerShell::bind(&mut wire.conn).expect("layer shell");
    let surface = shell
        .layer_surface(&mut wire.conn, &config())
        .expect("surface");
    wire.events();
    wire.take();

    wire.server(|s| s.layer_surfaces[0].configure(7, 1920, 0));
    let events = wire.shell_events();
    assert_eq!(
        events,
        [ShellEvent::Configure {
            surface: surface.surface(),
            size: (1920, 0)
        }]
    );
}

#[test]
fn a_closed_surface_is_reported() {
    let mut wire = Wire::all();
    let shell = LayerShell::bind(&mut wire.conn).expect("layer shell");
    let surface = shell
        .layer_surface(&mut wire.conn, &config())
        .expect("surface");
    wire.events();
    wire.server(|s| s.layer_surfaces[0].closed());
    assert_eq!(wire.shell_events(), [ShellEvent::Closed(surface.surface())]);
}

#[test]
fn the_size_to_draw_at_fills_an_extent_the_server_left_at_zero() {
    let mut wire = Wire::all();
    let shell = LayerShell::bind(&mut wire.conn).expect("layer shell");
    let surface = shell
        .layer_surface(&mut wire.conn, &config())
        .expect("surface");
    assert_eq!(surface.configured((1920, 0)).unwrap(), (1920, 24));
    assert_eq!(surface.configured((1920, 30)).unwrap(), (1920, 30));
    assert_eq!(
        surface.configured((0, 0)).unwrap_err().to_string(),
        Error::Unsized.to_string()
    );
}

#[test]
fn a_surface_that_requested_no_extent_cannot_be_configured_without_one() {
    let mut wire = Wire::all();
    let shell = LayerShell::bind(&mut wire.conn).expect("layer shell");
    let mut config = config();
    config.size = (0, 0);
    let surface = shell
        .layer_surface(&mut wire.conn, &config)
        .expect("surface");
    assert!(matches!(surface.configured((0, 0)), Err(Error::Unsized)));
    assert!(matches!(surface.configured((100, 0)), Err(Error::Unsized)));
    assert!(matches!(surface.configured((0, 40)), Err(Error::Unsized)));
    assert_eq!(surface.configured((100, 40)).unwrap(), (100, 40));
}

#[test]
fn a_placement_change_sends_the_anchor_and_size_and_is_what_configures_fill() {
    let mut wire = Wire::all();
    let shell = LayerShell::bind(&mut wire.conn).expect("layer shell");
    let mut surface = shell
        .layer_surface(&mut wire.conn, &config())
        .expect("surface");
    wire.events();
    wire.take();

    surface.set_placement(Anchor::Bottom, (300, 0));
    wire.events();
    let requests = layer_requests(&wire);
    assert!(matches!(
        requests[0],
        Request::SetAnchor { anchor } if u32::from(anchor) == 2
    ));
    assert!(matches!(
        requests[1],
        Request::SetSize {
            width: 300,
            height: 0
        }
    ));
    assert_eq!(surface.configured((0, 50)).unwrap(), (300, 50));
    assert!(matches!(surface.configured((0, 0)), Err(Error::Unsized)));
}

#[test]
fn the_setters_send_their_request() {
    let mut wire = Wire::all();
    let shell = LayerShell::bind(&mut wire.conn).expect("layer shell");
    let surface = shell
        .layer_surface(&mut wire.conn, &config())
        .expect("surface");
    wire.events();
    wire.take();

    surface.set_keyboard_interactivity(KeyboardInteractivity::Exclusive);
    surface.set_exclusive_zone(-1);
    surface.set_margin(Margin {
        top: 5,
        right: 6,
        bottom: 7,
        left: 8,
    });
    wire.events();
    let requests = layer_requests(&wire);
    assert!(matches!(
        requests[0],
        Request::SetKeyboardInteractivity { keyboard_interactivity }
            if u32::from(keyboard_interactivity) == KeyboardInteractivity::Exclusive as u32
    ));
    assert!(matches!(
        requests[1],
        Request::SetExclusiveZone { zone: -1 }
    ));
    assert!(matches!(
        requests[2],
        Request::SetMargin {
            top: 5,
            right: 6,
            bottom: 7,
            left: 8
        }
    ));
}

#[test]
fn the_default_margin_is_zero_on_every_side() {
    assert_eq!(
        Margin::default(),
        Margin {
            top: 0,
            right: 0,
            bottom: 0,
            left: 0
        }
    );
}
