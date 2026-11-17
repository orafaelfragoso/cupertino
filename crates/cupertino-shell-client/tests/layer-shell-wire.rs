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

fn layer_requests(seen: Vec<Seen>) -> Vec<Request> {
    seen.into_iter()
        .filter_map(|s| match s {
            Seen::Layer(r) => Some(r),
            _ => None,
        })
        .collect()
}

#[test]
fn the_global_binds_only_at_version_four() {
    for (offered, binds) in [(None, false), (Some(3), false), (Some(4), true)] {
        let mut wire = Wire::new(Offer {
            layer_shell: offered,
            ..Offer::all()
        });
        assert_eq!(
            LayerShell::bind(&mut wire.conn).is_some(),
            binds,
            "offered at {offered:?}"
        );
    }
}

#[test]
fn creating_a_surface_sends_every_setting_of_the_config() {
    for on_output in [true, false] {
        let mut wire = Wire::all();
        let shell = LayerShell::bind(&mut wire.conn).expect("layer shell");
        let mut config = config();
        if on_output {
            config.output = Some(wire.conn.outputs()[0].id);
        }
        shell
            .layer_surface(&mut wire.conn, &config)
            .expect("surface");
        wire.events();

        let mut seen = wire.take();
        assert!(
            matches!(
                &seen[0],
                Seen::LayerSurfaceCreated { namespace, layer, on_output: sent }
                    if namespace == "test-bar" && *layer == Layer::Top as u32 && *sent == on_output
            ),
            "on an output: {on_output}; without one the server chooses"
        );
        let requests = layer_requests(seen.split_off(1));
        assert!(matches!(
            requests[0],
            Request::SetSize {
                width: 0,
                height: 24
            }
        ));
        assert!(matches!(
            requests[1],
            Request::SetAnchor { anchor } if u32::from(anchor) == 1 | 4 | 8
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
                if u32::from(keyboard_interactivity) == KeyboardInteractivity::OnDemand as u32
        ));
    }
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
fn a_configure_and_a_close_are_reported() {
    let mut wire = Wire::all();
    let shell = LayerShell::bind(&mut wire.conn).expect("layer shell");
    let surface = shell
        .layer_surface(&mut wire.conn, &config())
        .expect("surface");
    wire.events();
    wire.take();

    wire.server(|s| {
        s.layer_surfaces[0].configure(7, 1920, 0);
        s.layer_surfaces[0].closed();
    });
    assert_eq!(
        wire.shell_events(),
        [
            ShellEvent::Configure {
                surface: surface.surface(),
                size: (1920, 0)
            },
            ShellEvent::Closed(surface.surface())
        ]
    );
}

#[test]
fn the_size_to_draw_at_fills_an_extent_the_server_left_at_zero_from_the_requested_one() {
    let mut wire = Wire::all();
    let shell = LayerShell::bind(&mut wire.conn).expect("layer shell");
    type Size = (u32, u32);
    let rows: [(Size, Size, Option<Size>); 7] = [
        ((0, 24), (1920, 0), Some((1920, 24))),
        ((0, 24), (1920, 30), Some((1920, 30))),
        ((0, 24), (0, 0), None),
        ((0, 0), (0, 0), None),
        ((0, 0), (100, 0), None),
        ((0, 0), (0, 40), None),
        ((0, 0), (100, 40), Some((100, 40))),
    ];
    for (requested, configured, drawn) in rows {
        let mut config = config();
        config.size = requested;
        let surface = shell
            .layer_surface(&mut wire.conn, &config)
            .expect("surface");
        match (surface.configured(configured), drawn) {
            (Ok(size), Some(drawn)) => assert_eq!(size, drawn, "{requested:?} {configured:?}"),
            (Err(Error::Unsized), None) => {}
            (got, _) => panic!("{requested:?} configured {configured:?}: {got:?}"),
        }
    }
}

#[test]
fn the_setters_send_their_request_and_a_placement_is_what_configures_fill() {
    let mut wire = Wire::all();
    let shell = LayerShell::bind(&mut wire.conn).expect("layer shell");
    let mut surface = shell
        .layer_surface(&mut wire.conn, &config())
        .expect("surface");
    wire.events();
    wire.take();

    surface.set_placement(Anchor::Bottom, (300, 0));
    surface.set_keyboard_interactivity(KeyboardInteractivity::Exclusive);
    surface.set_exclusive_zone(-1);
    surface.set_margin(Margin {
        top: 5,
        right: 6,
        bottom: 7,
        left: 8,
    });
    wire.events();
    let requests = layer_requests(wire.take());
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
    assert!(matches!(
        requests[2],
        Request::SetKeyboardInteractivity { keyboard_interactivity }
            if u32::from(keyboard_interactivity) == KeyboardInteractivity::Exclusive as u32
    ));
    assert!(matches!(
        requests[3],
        Request::SetExclusiveZone { zone: -1 }
    ));
    assert!(matches!(
        requests[4],
        Request::SetMargin {
            top: 5,
            right: 6,
            bottom: 7,
            left: 8
        }
    ));
    assert_eq!(surface.configured((0, 50)).unwrap(), (300, 50));
    assert!(matches!(surface.configured((0, 0)), Err(Error::Unsized)));
}
