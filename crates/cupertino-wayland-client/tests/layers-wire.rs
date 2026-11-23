//! The layers protocol against an in-process server: version gating, what each request puts on the wire and the
//! events that come back.

use crate::support;

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

fn timed(duration: f64, x1: f64, y1: f64, x2: f64) -> LayerCurve {
    LayerCurve::Timed {
        duration,
        bezier: CubicBezier::new(x1, y1, x2, 1.0),
    }
}

fn spring(response: f64, damping: f64) -> LayerCurve {
    LayerCurve::Spring { response, damping }
}

/// A request on a layer, as a row of a table.
type Call = Box<dyn Fn(&Layer) -> Result<(), Error>>;

/// Runs each call on a fresh layer of a server at its version and checks the result is `refused` and nothing went out.
fn assert_refused(rows: Vec<(String, u32, Call)>, refused: fn(&Error) -> bool) {
    for (row, version, call) in rows {
        let mut s = setup(version);
        let result = call(&s.layer);
        assert!(
            matches!(&result, Err(error) if refused(error)),
            "{row} on version {version}: {result:?}"
        );
        assert_eq!(s.sent(), Vec::<String>::new(), "{row}: nothing is sent");
    }
}

#[test]
fn binding_reports_what_the_servers_version_offers() {
    let (_server, mut conn) = Harness::start(Offer::NONE);
    assert!(Layers::bind(&mut conn).is_none(), "no global, no layers");

    let (_server, mut conn) = Harness::start(Offer::ALL);
    let _layers = Layers::bind(&mut conn).unwrap();
    assert_eq!(
        conn.capabilities().extension("cupertino_layers_v1"),
        Some(6)
    );

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
fn layers_and_presentations_have_distinct_ids_and_need_live_surfaces() {
    let mut s = setup(6);
    let a = window(&mut s.conn).surface();
    let b = window(&mut s.conn).surface();
    let second = s.layers.layer(&s.conn, a, b).unwrap();
    assert_ne!(s.layer.id().0, 0);
    assert_ne!(s.layer.id(), second.id());
    let c = window(&mut s.conn).surface();
    let presentation = s.layers.presentation(&s.conn, c).unwrap();
    assert_ne!(presentation.id(), s.layer.id());
    assert_ne!(presentation.id(), second.id());
    s.conn.roundtrip().unwrap();
    s.server.wait_for("get_presentation", 1);

    s.conn.destroy(a).unwrap();
    assert!(s.layers.layer(&s.conn, a, b).is_none());
    assert!(s.layers.layer(&s.conn, b, a).is_none());

    let mut old = setup(1);
    let surface = window(&mut old.conn).surface();
    assert!(
        old.layers.presentation(&old.conn, surface).is_none(),
        "a presentation needs version 2"
    );
}

#[test]
fn values_cancels_and_masks_go_out_as_given() {
    let mut s = setup(6);
    s.layer.set(LayerProperty::Opacity, 0.5).unwrap();
    s.layer.set(LayerProperty::Width, 120.0).unwrap();
    s.layer.set(LayerProperty::X, -30.0).unwrap();
    s.layer.set(LayerProperty::ContentBlur, 4.0).unwrap();
    s.layer.cancel(LayerProperty::Scale);
    s.layer.cancel(LayerProperty::ContentBlur);
    s.layer.set_masks_to_bounds(true);
    s.layer.set_masks_to_bounds(false);
    assert_eq!(
        s.sent(),
        [
            "SetValue { property: Value(Opacity), value: 0.5 }",
            "SetValue { property: Value(Width), value: 120.0 }",
            "SetValue { property: Value(X), value: -30.0 }",
            "SetValue { property: Value(ContentBlur), value: 4.0 }",
            "Cancel { property: Value(Scale) }",
            "Cancel { property: Value(ContentBlur) }",
            "SetMasksToBounds { masks: 1 }",
            "SetMasksToBounds { masks: 0 }",
        ]
    );
}

#[test]
fn a_request_the_servers_version_lacks_is_refused_and_nothing_is_sent() {
    let rows: Vec<(String, u32, Call)> = vec![
        (
            "content blur".into(),
            3,
            Box::new(|l| l.set(LayerProperty::ContentBlur, 4.0)),
        ),
        (
            "an animation of content blur".into(),
            3,
            Box::new(|l| l.animate(1, LayerProperty::ContentBlur, 4.0, TIMED, 0.0)),
        ),
        (
            "samples".into(),
            3,
            Box::new(|l| l.animate_samples(1, LayerProperty::X, 1.0, &[0.0, 1.0])),
        ),
        (
            "a glass group".into(),
            2,
            Box::new(|l| l.set_glass_group(1, 8.0)),
        ),
        (
            "a glass edge".into(),
            4,
            Box::new(|l| l.set_glass_edge(1.0, [0.0; 4])),
        ),
    ];
    assert_refused(rows, |e| matches!(e, Error::Unsupported(_)));

    // a cancel of a property the server does not know is left out rather than a protocol error
    let mut old = setup(3);
    old.layer.cancel(LayerProperty::ContentBlur);
    assert!(old.sent().is_empty());
}

#[test]
fn a_value_outside_its_propertys_range_is_refused_and_nothing_is_sent() {
    let mut rows: Vec<(String, u32, Call)> = Vec::new();
    for (property, value) in [
        (LayerProperty::Opacity, 1.5),
        (LayerProperty::Opacity, -0.1),
        (LayerProperty::Width, -1.0),
        (LayerProperty::X, f64::NAN),
        (LayerProperty::ContentBlur, MAX_CONTENT_BLUR + 1.0),
    ] {
        rows.push((
            format!("{property:?} at {value}"),
            6,
            Box::new(move |l| l.set(property, value)),
        ));
    }
    rows.push((
        "an animation to a value out of range".into(),
        6,
        Box::new(|l| l.animate(1, LayerProperty::Opacity, 2.0, TIMED, 0.0)),
    ));
    assert_refused(rows, |e| matches!(e, Error::InvalidLayerValue));
}

#[test]
fn an_animation_goes_out_in_milliseconds_and_fixed_point() {
    let mut s = setup(6);
    s.layer
        .animate(9, LayerProperty::Opacity, 1.0, TIMED, 0.0)
        .unwrap();
    s.layer
        .animate(
            1,
            LayerProperty::Scale,
            2.0,
            timed(-3.0, 0.0, 0.0, 1.0),
            0.0,
        )
        .unwrap();
    s.layer
        .animate(4, LayerProperty::Y, 12.0, spring(0.5, 0.75), -2.5)
        .unwrap();
    assert_eq!(
        s.sent(),
        [
            "AnimateTimed { serial: 9, property: Value(Opacity), value: 1.0, duration: 250, x1: 0.25, y1: 0.5, x2: 0.75, y2: 1.0 }",
            // a negative duration is sent as zero
            "AnimateTimed { serial: 1, property: Value(Scale), value: 2.0, duration: 0, x1: 0.0, y1: 0.0, x2: 1.0, y2: 1.0 }",
            "AnimateSpring { serial: 4, property: Value(Y), value: 12.0, response: 0.5, damping: 0.75, velocity: -2.5 }",
        ]
    );
}

#[test]
fn a_curve_the_server_refuses_is_refused_before_it_is_sent() {
    let mut s = setup(6);
    let refused = [
        ("a duration over 10 s", timed(10.0006, 0.0, 0.0, 1.0), 0.0),
        ("a NaN duration", timed(f64::NAN, 0.0, 0.0, 1.0), 0.0),
        (
            "an infinite duration",
            timed(f64::INFINITY, 0.0, 0.0, 1.0),
            0.0,
        ),
        ("x1 below 0", timed(1.0, -0.1, 0.0, 1.0), 0.0),
        ("x2 above 1", timed(1.0, 0.0, 0.0, 1.1), 0.0),
        ("a NaN x1", timed(1.0, f64::NAN, 0.0, 1.0), 0.0),
        ("an infinite y1", timed(1.0, 0.0, f64::INFINITY, 1.0), 0.0),
        ("a response too short", spring(0.009, 1.0), 0.0),
        ("a response too long", spring(10.1, 1.0), 0.0),
        ("damping too low", spring(0.5, 0.04), 0.0),
        ("damping too high", spring(0.5, 4.1), 0.0),
        ("a NaN response", spring(f64::NAN, 1.0), 0.0),
        ("a NaN velocity", spring(0.5, 1.0), f64::NAN),
        // the wire truncates to 256ths, so the ends themselves arrive below the server's range
        ("the least response", spring(SPRING_RESPONSE.0, 1.0), 0.0),
        ("the least damping", spring(0.5, SPRING_DAMPING.0), 0.0),
    ];
    for (row, curve, velocity) in refused {
        assert!(
            matches!(
                s.layer
                    .animate(1, LayerProperty::Opacity, 1.0, curve, velocity),
                Err(Error::InvalidLayerValue)
            ),
            "{row}: {curve:?}"
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
fn samples_go_out_as_native_endian_floats_within_their_count_duration_and_range() {
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

    let ramp = |n: usize| vec![0.0f32; n];
    let mut s = setup(6);
    let (x, opacity) = (LayerProperty::X, LayerProperty::Opacity);
    let refused = [
        ("one sample", s.layer.animate_samples(1, x, 1.0, &ramp(1))),
        (
            "too many samples",
            s.layer.animate_samples(1, x, 1.0, &ramp(MAX_SAMPLES + 1)),
        ),
        ("too short", s.layer.animate_samples(1, x, 0.0004, &ramp(2))),
        ("too long", s.layer.animate_samples(1, x, 10.0006, &ramp(2))),
        (
            "a NaN duration",
            s.layer.animate_samples(1, x, f64::NAN, &ramp(2)),
        ),
        ("no samples", s.layer.animate_samples(1, x, 1.0, &[])),
        (
            "a sample over 1",
            s.layer.animate_samples(1, opacity, 1.0, &[0.0, 1.5]),
        ),
        (
            "a sample under 0",
            s.layer.animate_samples(1, opacity, 1.0, &[-0.5, 1.0]),
        ),
        (
            "a NaN sample",
            s.layer.animate_samples(1, opacity, 1.0, &[0.0, f32::NAN]),
        ),
    ];
    for (row, result) in refused {
        assert!(matches!(result, Err(Error::InvalidLayerValue)), "{row}");
    }
    assert!(s.sent().is_empty());
    s.layer.animate_samples(1, x, 0.0006, &ramp(2)).unwrap();
    s.layer
        .animate_samples(2, x, 10.0, &ramp(MAX_SAMPLES))
        .unwrap();
    assert_eq!(s.sent().len(), 2, "the shortest and the longest");
}

#[test]
fn glass_takes_a_style_or_none_and_a_lens_within_its_limits() {
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
        assert!(
            matches!(s.layer.set_glass(None, lens), Err(Error::InvalidGlass)),
            "lens {lens}"
        );
    }
    let sent = s.sent();
    assert_eq!(sent.len(), 3);
    assert_eq!(sent[0], "SetGlass { style: 4294967295, lens: 1.0 }");
    assert!(sent[1].ends_with("lens: 1.5 }"), "{}", sent[1]);
    assert!(!sent[1].contains("4294967295"));
    assert_eq!(sent[2], "SetGlass { style: 4294967295, lens: 4.0 }");

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
    for style in styles {
        s.layer.set_glass(Some(style), 1.0).unwrap();
    }
    let mut sent = s.sent()[3..].to_vec();
    assert_eq!(sent.len(), styles.len());
    sent.sort();
    sent.dedup();
    assert_eq!(
        sent.len(),
        styles.len(),
        "every style has its own wire value"
    );
}

#[test]
fn a_glass_group_and_edge_take_bounded_spacing_width_and_colour() {
    let mut s = setup(5);
    s.layer.set_glass_group(2, 0.0).unwrap();
    s.layer.set_glass_group(2, MAX_GROUP_SPACING).unwrap();
    s.layer.set_glass_edge(0.0, [0.0, 0.0, 0.0, 0.0]).unwrap();
    s.layer
        .set_glass_edge(MAX_GLASS_EDGE, [1.0, 0.5, 0.25, 1.0])
        .unwrap();
    assert_eq!(
        s.sent(),
        [
            "SetGlassGroup { group: 2, spacing: 0.0 }",
            "SetGlassGroup { group: 2, spacing: 64.0 }",
            "SetGlassEdge { width: 0.0, red: 0.0, green: 0.0, blue: 0.0, alpha: 0.0 }",
            "SetGlassEdge { width: 4.0, red: 1.0, green: 0.5, blue: 0.25, alpha: 1.0 }",
        ]
    );

    let mut rows: Vec<(String, u32, Call)> = Vec::new();
    for spacing in [-0.5, MAX_GROUP_SPACING + 0.5, f64::NAN] {
        rows.push((
            format!("spacing {spacing}"),
            5,
            Box::new(move |l| l.set_glass_group(2, spacing)),
        ));
    }
    for (width, color) in [
        (-0.1, [0.0; 4]),
        (MAX_GLASS_EDGE + 0.1, [0.0; 4]),
        (f64::NAN, [0.0; 4]),
        (1.0, [1.1, 0.0, 0.0, 0.0]),
        (1.0, [0.0, -0.1, 0.0, 0.0]),
        (1.0, [0.0, 0.0, 0.0, f64::NAN]),
    ] {
        rows.push((
            format!("edge {width} {color:?}"),
            5,
            Box::new(move |l| l.set_glass_edge(width, color)),
        ));
    }
    assert_refused(rows, |e| matches!(e, Error::InvalidGlass));
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
fn the_servers_events_arrive_in_order_naming_their_layer() {
    let mut s = setup(6);
    let a = window(&mut s.conn).surface();
    let b = window(&mut s.conn).surface();
    let second = s.layers.layer(&s.conn, a, b).unwrap();
    s.sent();
    s.server.run(|server| {
        let layer = &server.layers[0];
        layer.finished(5);
        layer.replaced(6, 1.5, -0.25);
        server.layers[1].settled();
        layer.settled();
    });
    let id = s.layer.id();
    let (events, _) = wait_app_events(&mut s.conn, |e| e.len() >= 4);
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
            AppEvent::LayerSettled { layer: second.id() },
            AppEvent::LayerSettled { layer: id },
        ]
    );
}

#[test]
fn a_blur_ramp_takes_two_weights_from_0_to_1_at_every_version() {
    let mut s = setup(1);
    s.layer.set_blur_ramp(1.0, 0.0).unwrap();
    s.layer.set_blur_ramp(0.25, 1.0).unwrap();
    assert_eq!(
        s.sent(),
        [
            "SetBlurRamp { top: 1.0, bottom: 0.0 }",
            "SetBlurRamp { top: 0.25, bottom: 1.0 }",
        ]
    );

    let mut rows: Vec<(String, u32, Call)> = Vec::new();
    for (top, bottom) in [
        (-0.1, 1.0),
        (1.0, 1.1),
        (f64::NAN, 1.0),
        (1.0, f64::INFINITY),
    ] {
        rows.push((
            format!("ramp {top} {bottom}"),
            6,
            Box::new(move |l| l.set_blur_ramp(top, bottom)),
        ));
    }
    assert_refused(rows, |e| matches!(e, Error::InvalidGlass));
}
