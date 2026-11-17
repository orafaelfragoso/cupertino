//! The layers protocol against an in-process server: version gating, what each request puts on the wire and the
//! events that come back.

mod support;

use cupertino_animation::{CubicBezier, LayerCurve};
use cupertino_app_protocols::layers::{LENS, SPRING_DAMPING, SPRING_RESPONSE};
use cupertino_wayland_client::app::{
    AppEvent, Error, Fixed, GlassStyle, Layer, LayerProperty, Layers, MAX_CONTENT_BLUR,
    MAX_GLASS_EDGE, MAX_GROUP_SPACING, MAX_SAMPLES,
};
use cupertino_wayland_client::Connection;
use support::*;

struct Setup {
    server: Harness,
    conn: Connection,
    layers: Layers,
    layer: Layer,
}

/// A layer of one window over another, on a server that speaks the protocol at `version`.
fn setup(version: u32) -> Setup {
    let (server, mut conn) = Harness::start(Offer {
        layers: Some(version),
        ..Offer::NONE
    });
    let surface = window(&mut conn).surface();
    let parent = window(&mut conn).surface();
    let layers = Layers::bind(&mut conn).expect("the server offers layers");
    let layer = layers
        .layer(&conn, surface, parent)
        .expect("both surfaces live");
    Setup {
        server,
        conn,
        layers,
        layer,
    }
}

impl Setup {
    /// What the server received after the objects were made, once it has handled everything sent so far.
    fn sent(&mut self) -> Vec<String> {
        self.conn.roundtrip().unwrap();
        self.server
            .requests()
            .into_iter()
            .filter(|r| !r.starts_with("get_"))
            .collect()
    }
}

const TIMED: LayerCurve = LayerCurve::Timed {
    duration: 0.25,
    bezier: CubicBezier {
        x1: 0.25,
        y1: 0.5,
        x2: 0.75,
        y2: 1.0,
    },
};

#[test]
fn a_server_without_the_global_gives_none() {
    let (_server, mut conn) = Harness::start(Offer::NONE);
    assert!(Layers::bind(&mut conn).is_none());
}

#[test]
fn the_bound_version_is_listed_in_the_capabilities() {
    let (_server, mut conn) = Harness::start(Offer::ALL);
    let _layers = Layers::bind(&mut conn).unwrap();
    assert_eq!(
        conn.capabilities().extension("cupertino_layers_v1"),
        Some(6)
    );
}

#[test]
fn each_version_adds_its_features() {
    // presents surfaces, presents popups, blurs content, draws glass edges, softens glass edges
    let table = [
        (1, [false, false, false, false, false]),
        (2, [true, false, false, false, false]),
        (3, [true, true, false, false, false]),
        (4, [true, true, true, false, false]),
        (5, [true, true, true, true, false]),
        (6, [true, true, true, true, true]),
    ];
    for (version, expected) in table {
        let s = setup(version);
        let got = [
            s.layers.presents_surfaces(),
            s.layers.presents_popups(),
            s.layers.blurs_content(),
            s.layers.draws_glass_edges(),
            s.layers.softens_glass_edges(),
        ];
        assert_eq!(got, expected, "version {version}");
    }
}

#[test]
fn layers_have_distinct_ids_and_a_destroyed_surface_makes_none() {
    let mut s = setup(6);
    let a = window(&mut s.conn).surface();
    let b = window(&mut s.conn).surface();
    let second = s.layers.layer(&s.conn, a, b).unwrap();
    assert_ne!(s.layer.id().0, 0);
    assert_ne!(s.layer.id(), second.id());
    s.conn.destroy(a).unwrap();
    assert!(s.layers.layer(&s.conn, a, b).is_none());
    assert!(s.layers.layer(&s.conn, b, a).is_none());
}

#[test]
fn a_presentation_needs_version_two() {
    let mut old = setup(1);
    let surface = window(&mut old.conn).surface();
    assert!(old.layers.presentation(&old.conn, surface).is_none());

    let mut s = setup(2);
    let surface = window(&mut s.conn).surface();
    let presentation = s.layers.presentation(&s.conn, surface).unwrap();
    assert_ne!(presentation.id(), s.layer.id());
    s.conn.roundtrip().unwrap();
    s.server.wait_for("get_presentation", 1);
}

#[test]
fn set_sends_values_in_range() {
    let mut s = setup(6);
    s.layer.set(LayerProperty::Opacity, 0.5).unwrap();
    s.layer.set(LayerProperty::Width, 120.0).unwrap();
    s.layer.set(LayerProperty::X, -30.0).unwrap();
    assert_eq!(
        s.sent(),
        [
            "SetValue { property: Value(Opacity), value: 0.5 }",
            "SetValue { property: Value(Width), value: 120.0 }",
            "SetValue { property: Value(X), value: -30.0 }",
        ]
    );
}

#[test]
fn set_refuses_values_outside_the_range_and_sends_nothing() {
    let mut s = setup(6);
    for (property, value) in [
        (LayerProperty::Opacity, 1.5),
        (LayerProperty::Opacity, -0.1),
        (LayerProperty::Width, -1.0),
        (LayerProperty::X, f64::NAN),
        (LayerProperty::ContentBlur, MAX_CONTENT_BLUR + 1.0),
    ] {
        assert!(
            matches!(s.layer.set(property, value), Err(Error::InvalidLayerValue)),
            "{property:?} {value}"
        );
    }
    assert!(s.sent().is_empty());
}

#[test]
fn content_blur_is_refused_below_version_four() {
    let mut old = setup(3);
    assert!(matches!(
        old.layer.set(LayerProperty::ContentBlur, 4.0),
        Err(Error::Unsupported(_))
    ));
    assert!(matches!(
        old.layer
            .animate(1, LayerProperty::ContentBlur, 4.0, TIMED, 0.0),
        Err(Error::Unsupported(_))
    ));
    // a cancel of a property the server does not know is left out rather than a protocol error
    old.layer.cancel(LayerProperty::ContentBlur);
    assert!(old.sent().is_empty());

    let mut s = setup(4);
    s.layer.set(LayerProperty::ContentBlur, 4.0).unwrap();
    s.layer.cancel(LayerProperty::ContentBlur);
    assert_eq!(
        s.sent(),
        [
            "SetValue { property: Value(ContentBlur), value: 4.0 }",
            "Cancel { property: Value(ContentBlur) }",
        ]
    );
}

#[test]
fn a_timed_animation_goes_out_in_milliseconds_and_fixed_point() {
    let mut s = setup(6);
    s.layer
        .animate(9, LayerProperty::Opacity, 1.0, TIMED, 0.0)
        .unwrap();
    assert_eq!(
        s.sent(),
        ["AnimateTimed { serial: 9, property: Value(Opacity), value: 1.0, duration: 250, x1: 0.25, y1: 0.5, x2: 0.75, y2: 1.0 }"]
    );
}

#[test]
fn a_negative_duration_is_sent_as_zero() {
    let mut s = setup(6);
    let curve = LayerCurve::Timed {
        duration: -3.0,
        bezier: CubicBezier::new(0.0, 0.0, 1.0, 1.0),
    };
    s.layer
        .animate(1, LayerProperty::Scale, 2.0, curve, 0.0)
        .unwrap();
    let sent = s.sent();
    assert_eq!(sent.len(), 1);
    assert!(sent[0].contains("duration: 0,"), "{}", sent[0]);
}

#[test]
fn a_spring_animation_carries_its_velocity() {
    let mut s = setup(6);
    let spring = LayerCurve::Spring {
        response: 0.5,
        damping: 0.75,
    };
    s.layer
        .animate(4, LayerProperty::Y, 12.0, spring, -2.5)
        .unwrap();
    assert_eq!(
        s.sent(),
        ["AnimateSpring { serial: 4, property: Value(Y), value: 12.0, response: 0.5, damping: 0.75, velocity: -2.5 }"]
    );
}

#[test]
fn an_animation_to_a_value_outside_the_range_is_refused() {
    let mut s = setup(6);
    assert!(matches!(
        s.layer.animate(1, LayerProperty::Opacity, 2.0, TIMED, 0.0),
        Err(Error::InvalidLayerValue)
    ));
    assert!(s.sent().is_empty());
}

#[test]
fn a_curve_the_server_refuses_is_refused_before_it_is_sent() {
    let mut s = setup(6);
    let timed = |duration: f64, x1: f64, y1: f64, x2: f64| LayerCurve::Timed {
        duration,
        bezier: CubicBezier::new(x1, y1, x2, 1.0),
    };
    let spring = |response: f64, damping: f64| LayerCurve::Spring { response, damping };
    let refused = [
        (timed(10.0006, 0.0, 0.0, 1.0), 0.0),
        (timed(f64::NAN, 0.0, 0.0, 1.0), 0.0),
        (timed(f64::INFINITY, 0.0, 0.0, 1.0), 0.0),
        (timed(1.0, -0.1, 0.0, 1.0), 0.0),
        (timed(1.0, 0.0, 0.0, 1.1), 0.0),
        (timed(1.0, f64::NAN, 0.0, 1.0), 0.0),
        (timed(1.0, 0.0, f64::INFINITY, 1.0), 0.0),
        (spring(0.009, 1.0), 0.0),
        (spring(10.1, 1.0), 0.0),
        (spring(0.5, 0.04), 0.0),
        (spring(0.5, 4.1), 0.0),
        (spring(f64::NAN, 1.0), 0.0),
        (spring(0.5, 1.0), f64::NAN),
        // the wire truncates to 256ths, so the ends themselves arrive below the server.s range
        (spring(SPRING_RESPONSE.0, 1.0), 0.0),
        (spring(0.5, SPRING_DAMPING.0), 0.0),
    ];
    for (curve, velocity) in refused {
        assert!(
            matches!(
                s.layer
                    .animate(1, LayerProperty::Opacity, 1.0, curve, velocity),
                Err(Error::InvalidLayerValue)
            ),
            "{curve:?}"
        );
    }
    assert!(s.sent().is_empty());
    // the smallest springs the server takes, the upper limits, and a Bézier y control point may leave 0 to 1
    let least = |v: f64| (v * 256.0).ceil() / 256.0;
    for curve in [
        timed(10.0, 0.0, -2.0, 1.0),
        spring(least(SPRING_RESPONSE.0), least(SPRING_DAMPING.0)),
        spring(SPRING_RESPONSE.1, SPRING_DAMPING.1),
    ] {
        s.layer
            .animate(2, LayerProperty::Opacity, 1.0, curve, 0.0)
            .unwrap();
    }
    let sent = s.sent();
    assert_eq!(sent.len(), 3);
    assert!(
        sent[1].contains("response: 0.01171875, damping: 0.05078125"),
        "{}",
        sent[1]
    );
    assert!(
        sent[2].contains("response: 10.0, damping: 4.0"),
        "{}",
        sent[2]
    );
}

#[test]
fn samples_are_sent_as_native_endian_floats() {
    let mut s = setup(6);
    s.layer
        .animate_samples(3, LayerProperty::Opacity, 0.5, &[0.0, 0.5, 1.0])
        .unwrap();
    let mut bytes = Vec::new();
    for v in [0.0f32, 0.5, 1.0] {
        bytes.extend(v.to_ne_bytes());
    }
    assert_eq!(
        s.sent(),
        [format!(
            "AnimateSamples {{ serial: 3, property: Value(Opacity), duration: 500, samples: {bytes:?} }}"
        )]
    );
}

#[test]
fn samples_have_a_count_and_a_duration_range() {
    let mut s = setup(6);
    let property = LayerProperty::X;
    let ramp = |n: usize| vec![0.0f32; n];
    let refused = [
        s.layer.animate_samples(1, property, 1.0, &ramp(1)),
        s.layer
            .animate_samples(1, property, 1.0, &ramp(MAX_SAMPLES + 1)),
        s.layer.animate_samples(1, property, 0.0004, &ramp(2)),
        s.layer.animate_samples(1, property, 10.0006, &ramp(2)),
        s.layer.animate_samples(1, property, f64::NAN, &ramp(2)),
        s.layer.animate_samples(1, property, 1.0, &[]),
    ];
    for result in refused {
        assert!(matches!(result, Err(Error::InvalidLayerValue)));
    }
    assert!(s.sent().is_empty());
    s.layer
        .animate_samples(1, property, 0.0006, &ramp(2))
        .unwrap();
    s.layer
        .animate_samples(2, property, 10.0, &ramp(MAX_SAMPLES))
        .unwrap();
    assert_eq!(s.sent().len(), 2);
}

#[test]
fn a_sample_outside_the_property_range_refuses_the_whole_animation() {
    let mut s = setup(6);
    for samples in [[0.0, 1.5], [-0.5, 1.0], [0.0, f32::NAN]] {
        assert!(matches!(
            s.layer
                .animate_samples(1, LayerProperty::Opacity, 1.0, &samples),
            Err(Error::InvalidLayerValue)
        ));
    }
    assert!(s.sent().is_empty());
}

#[test]
fn samples_need_version_four() {
    let mut s = setup(3);
    assert!(matches!(
        s.layer
            .animate_samples(1, LayerProperty::X, 1.0, &[0.0, 1.0]),
        Err(Error::Unsupported(_))
    ));
    assert!(s.sent().is_empty());
}

#[test]
fn cancel_names_the_property() {
    let mut s = setup(6);
    s.layer.cancel(LayerProperty::Scale);
    assert_eq!(s.sent(), ["Cancel { property: Value(Scale) }"]);
}

#[test]
fn glass_takes_a_style_or_none_and_a_lens_of_one_to_four() {
    let mut s = setup(6);
    s.layer.set_glass(None, LENS.0).unwrap();
    s.layer.set_glass(Some(GlassStyle::Clear), 1.5).unwrap();
    s.layer.set_glass(None, LENS.1).unwrap();
    for lens in [
        0.99,
        0.0,
        4.5,
        LENS.1 + 1.0 / 256.0,
        f64::NAN,
        f64::INFINITY,
    ] {
        assert!(matches!(
            s.layer.set_glass(None, lens),
            Err(Error::InvalidGlass)
        ));
    }
    let sent = s.sent();
    assert_eq!(sent.len(), 3);
    assert_eq!(sent[0], "SetGlass { style: 4294967295, lens: 1.0 }");
    assert!(sent[1].ends_with("lens: 1.5 }"), "{}", sent[1]);
    assert!(!sent[1].contains("4294967295"));
    assert_eq!(sent[2], "SetGlass { style: 4294967295, lens: 4.0 }");
}

#[test]
fn every_glass_style_has_its_own_wire_value() {
    let styles = [
        GlassStyle::Regular,
        GlassStyle::Clear,
        GlassStyle::LargePanel,
        GlassStyle::Bar,
        GlassStyle::Dense,
        GlassStyle::Module,
        GlassStyle::Card,
        GlassStyle::Track,
        GlassStyle::Popover,
        GlassStyle::Switcher,
        GlassStyle::Sidebar,
        GlassStyle::SubduedModule,
        GlassStyle::Snippet,
        GlassStyle::Veiled,
        GlassStyle::Toolbar,
        GlassStyle::Menu,
    ];
    let mut s = setup(6);
    for style in styles {
        s.layer.set_glass(Some(style), 1.0).unwrap();
    }
    let sent = s.sent();
    assert_eq!(sent.len(), styles.len());
    let mut distinct = sent.clone();
    distinct.sort();
    distinct.dedup();
    assert_eq!(distinct.len(), styles.len(), "{sent:?}");
}

#[test]
fn masking_to_bounds_sends_a_flag() {
    let mut s = setup(6);
    s.layer.set_masks_to_bounds(true);
    s.layer.set_masks_to_bounds(false);
    assert_eq!(
        s.sent(),
        [
            "SetMasksToBounds { masks: 1 }",
            "SetMasksToBounds { masks: 0 }"
        ]
    );
}

#[test]
fn a_glass_group_needs_version_three_and_a_bounded_spacing() {
    let mut old = setup(2);
    assert!(matches!(
        old.layer.set_glass_group(1, 8.0),
        Err(Error::Unsupported(_))
    ));
    assert!(old.sent().is_empty());

    let mut s = setup(3);
    s.layer.set_glass_group(2, 0.0).unwrap();
    s.layer.set_glass_group(2, MAX_GROUP_SPACING).unwrap();
    for spacing in [-0.5, MAX_GROUP_SPACING + 0.5, f64::NAN] {
        assert!(matches!(
            s.layer.set_glass_group(2, spacing),
            Err(Error::InvalidGlass)
        ));
    }
    assert_eq!(
        s.sent(),
        [
            "SetGlassGroup { group: 2, spacing: 0.0 }",
            "SetGlassGroup { group: 2, spacing: 64.0 }"
        ]
    );
}

#[test]
fn a_glass_edge_needs_version_five_and_a_bounded_width_and_colour() {
    let mut old = setup(4);
    assert!(matches!(
        old.layer.set_glass_edge(1.0, [0.0; 4]),
        Err(Error::Unsupported(_))
    ));
    assert!(old.sent().is_empty());

    let mut s = setup(5);
    s.layer.set_glass_edge(0.0, [0.0, 0.0, 0.0, 0.0]).unwrap();
    s.layer
        .set_glass_edge(MAX_GLASS_EDGE, [1.0, 0.5, 0.25, 1.0])
        .unwrap();
    for (width, color) in [
        (-0.1, [0.0; 4]),
        (MAX_GLASS_EDGE + 0.1, [0.0; 4]),
        (f64::NAN, [0.0; 4]),
        (1.0, [1.1, 0.0, 0.0, 0.0]),
        (1.0, [0.0, -0.1, 0.0, 0.0]),
        (1.0, [0.0, 0.0, 0.0, f64::NAN]),
    ] {
        assert!(matches!(
            s.layer.set_glass_edge(width, color),
            Err(Error::InvalidGlass)
        ));
    }
    assert_eq!(
        s.sent(),
        [
            "SetGlassEdge { width: 0.0, red: 0.0, green: 0.0, blue: 0.0, alpha: 0.0 }",
            "SetGlassEdge { width: 4.0, red: 1.0, green: 0.5, blue: 0.25, alpha: 1.0 }",
        ]
    );
}

#[test]
fn dropping_a_layer_destroys_it() {
    let Setup {
        server,
        mut conn,
        layers,
        layer,
    } = setup(6);
    drop(layer);
    conn.roundtrip().unwrap();
    server.wait_for("Destroy", 1);
    drop(layers);
    conn.roundtrip().unwrap();
    server.wait_for("Destroy", 2);
}

#[test]
fn the_servers_events_arrive_in_order_as_app_events() {
    let mut s = setup(6);
    s.sent();
    s.server.run(|server| {
        let layer = &server.layers[0];
        layer.finished(5);
        layer.replaced(6, 1.5, -0.25);
        layer.settled();
    });
    let id = s.layer.id();
    let (events, _) = wait_app_events(&mut s.conn, |e| e.len() >= 3);
    assert_eq!(
        events,
        [
            AppEvent::LayerFinished {
                layer: id,
                serial: 5
            },
            AppEvent::LayerReplaced {
                layer: id,
                serial: 6,
                value: Fixed(384),
                velocity: Fixed(-64)
            },
            AppEvent::LayerSettled { layer: id },
        ]
    );
}

#[test]
fn events_name_the_layer_they_came_from() {
    let mut s = setup(6);
    let a = window(&mut s.conn).surface();
    let b = window(&mut s.conn).surface();
    let second = s.layers.layer(&s.conn, a, b).unwrap();
    s.sent();
    s.server.run(|server| server.layers[1].settled());
    let (events, _) = wait_app_events(&mut s.conn, |e| !e.is_empty());
    assert_eq!(events, [AppEvent::LayerSettled { layer: second.id() }]);
}
