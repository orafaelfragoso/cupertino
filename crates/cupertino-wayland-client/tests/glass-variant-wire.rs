//! Glass named by its system variant and tinted, against an in-process server: `cupertino_backdrop_v1` from version 23
//! and `cupertino_layer_v1` from version 7 take the variant, its modifier bits, its size class and the tint; older
//! servers are sent the style a variant falls back to and no tint.

use crate::support;

use cupertino_wayland_client::app::{
    AppEffects, Error, GlassModifiers, GlassSizeClass, GlassStyle, GlassVariant, Layers,
    VariantGlass, MAX_GLASS_SCALE,
};
use cupertino_wayland_client::Connection;
use support::*;

/// What the server received, once it has handled everything sent so far, without the requests that made objects.
fn sent(server: &Harness, conn: &mut Connection) -> Vec<String> {
    conn.roundtrip().unwrap();
    server
        .requests()
        .into_iter()
        .filter(|r| !r.starts_with("get_"))
        .collect()
}

/// A server offering app effects at `version`, a window and a backdrop of it.
fn backdrop_on(version: u32) -> (Harness, Connection, cupertino_wayland_client::app::Backdrop) {
    let (server, mut conn) = Harness::start(Offer {
        app_effects: Some(version),
        ..Offer::NONE
    });
    let surface = window(&mut conn).surface();
    let effects = AppEffects::bind(&mut conn).expect("the server offers app effects");
    let backdrop = effects.backdrop(&conn, surface).unwrap();
    (server, conn, backdrop)
}

/// Interactive regular glass at the medium size, subdued.
fn pressable() -> VariantGlass {
    VariantGlass {
        variant: GlassVariant::Regular,
        size: GlassSizeClass::Medium,
        modifiers: GlassModifiers {
            subdued: true,
            interactive: true,
            ..GlassModifiers::default()
        },
    }
}

#[test]
fn a_backdrop_sends_its_variant_modifiers_size_class_and_tint_from_version_23() {
    let (server, mut conn, backdrop) = backdrop_on(23);
    assert!(backdrop.takes_glass_variants());
    backdrop.set_glass_variant(pressable(), 3, 8.0).unwrap();
    backdrop.set_glass_tint([1.0, 0.5, 0.25, 0.75]).unwrap();
    let sent = sent(&server, &mut conn);
    assert_eq!(sent.len(), 2, "{sent:?}");
    let variant = &sent[0];
    assert!(
        variant.starts_with("SetGlassVariant { variant: Value(Regular)"),
        "{variant}"
    );
    assert!(
        variant.contains("Subdued") && variant.contains("Interactive"),
        "both modifier bits go out: {variant}"
    );
    assert!(
        !variant.contains("ExcludeShadow") && !variant.contains("Adaptive"),
        "{variant}"
    );
    assert!(
        variant.ends_with("size_class: Value(Medium), container: 3, spacing: 8.0 }"),
        "{variant}"
    );
    assert_eq!(
        sent[1],
        "SetGlassTint { red: 1.0, green: 0.5, blue: 0.25, alpha: 0.75 }"
    );
}

#[test]
fn a_sheets_glass_window_variant_sends_the_document_modal_bit_and_falls_back_to_regular_glass() {
    let sheet = VariantGlass {
        modifiers: GlassModifiers {
            document_modal: true,
            ..GlassModifiers::default()
        },
        ..VariantGlass::new(GlassVariant::GlassWindow)
    };
    let (server, mut conn, backdrop) = backdrop_on(23);
    backdrop.set_glass_variant(sheet, 0, 0.0).unwrap();
    let sent = sent(&server, &mut conn);
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert!(
        sent[0].starts_with("SetGlassVariant { variant: Value(GlassWindow)")
            && sent[0].contains("DocumentModal")
            && !sent[0].contains("ExcludeShadow"),
        "{}",
        sent[0]
    );
    assert_eq!(sheet.fallback_style(), GlassStyle::Regular);
}

#[test]
fn a_glass_scale_goes_from_version_23_and_a_bad_one_is_refused() {
    let (server, mut conn, backdrop) = backdrop_on(23);
    backdrop.set_glass_scale(1.043).unwrap();
    for scale in [0.0, -1.0, f64::NAN, MAX_GLASS_SCALE + 0.5] {
        assert!(
            matches!(backdrop.set_glass_scale(scale), Err(Error::InvalidGlass)),
            "{scale}"
        );
    }
    let first = sent(&server, &mut conn);
    assert_eq!(first.len(), 1, "{first:?}");
    assert!(
        first[0].starts_with("SetGlassScale { scale: 1.04"),
        "{}",
        first[0]
    );

    let (server, mut conn, backdrop) = backdrop_on(22);
    backdrop.set_glass_scale(1.043).unwrap();
    assert!(
        sent(&server, &mut conn).is_empty(),
        "an older server draws the glass at its shape"
    );
}

#[test]
fn an_older_server_is_sent_the_style_the_variant_falls_back_to_and_no_tint() {
    let (server, mut conn, backdrop) = backdrop_on(22);
    assert!(!backdrop.takes_glass_variants());
    backdrop
        .set_glass_variant(VariantGlass::new(GlassVariant::SearchResults), 1, 4.0)
        .unwrap();
    backdrop.set_glass_tint([1.0, 0.0, 0.0, 1.0]).unwrap();
    assert_eq!(
        sent(&server, &mut conn),
        ["SetGlass { style: Value(LargePanel), container: 1, spacing: 4.0 }"]
    );
}

#[test]
fn a_bad_tint_or_spacing_is_refused_and_sends_nothing() {
    let (server, mut conn, backdrop) = backdrop_on(23);
    for tint in [
        [-0.01, 0.0, 0.0, 1.0],
        [0.0, 1.01, 0.0, 1.0],
        [0.0, 0.0, f64::NAN, 1.0],
        [0.0, 0.0, 0.0, f64::INFINITY],
    ] {
        assert!(
            matches!(backdrop.set_glass_tint(tint), Err(Error::InvalidGlass)),
            "{tint:?}"
        );
    }
    for spacing in [-1.0, f64::NAN] {
        assert!(matches!(
            backdrop.set_glass_variant(pressable(), 0, spacing),
            Err(Error::InvalidGlass)
        ));
    }
    assert!(sent(&server, &mut conn).is_empty());
}

#[test]
fn every_variant_falls_back_to_the_style_that_draws_as_it_or_the_nearest() {
    let at = |variant, size| VariantGlass {
        variant,
        size,
        modifiers: GlassModifiers::default(),
    };
    let dynamic = GlassSizeClass::Dynamic;
    let table = [
        (at(GlassVariant::Regular, dynamic), GlassStyle::Regular),
        (at(GlassVariant::Clear, dynamic), GlassStyle::Clear),
        (at(GlassVariant::Dock, dynamic), GlassStyle::Bar),
        (at(GlassVariant::AppIcons, dynamic), GlassStyle::Clear),
        (at(GlassVariant::ControlCenter, dynamic), GlassStyle::Module),
        (
            VariantGlass {
                modifiers: GlassModifiers {
                    subdued: true,
                    ..GlassModifiers::default()
                },
                ..at(GlassVariant::ControlCenter, dynamic)
            },
            GlassStyle::SubduedModule,
        ),
        (
            at(GlassVariant::NotificationCenter, GlassSizeClass::Medium),
            GlassStyle::Card,
        ),
        (
            at(GlassVariant::NotificationCenter, dynamic),
            GlassStyle::Snippet,
        ),
        (at(GlassVariant::Sidebar, dynamic), GlassStyle::Sidebar),
        (
            at(GlassVariant::AbuttedSidebar, dynamic),
            GlassStyle::Sidebar,
        ),
        (at(GlassVariant::Inspector, dynamic), GlassStyle::Sidebar),
        (
            at(GlassVariant::CartouchePopover, GlassSizeClass::Minimum),
            GlassStyle::Popover,
        ),
        (
            at(GlassVariant::CartouchePopover, GlassSizeClass::Medium),
            GlassStyle::Dense,
        ),
        (at(GlassVariant::Menu, dynamic), GlassStyle::Menu),
        (at(GlassVariant::Tab, dynamic), GlassStyle::Regular),
        (at(GlassVariant::GlassWindow, dynamic), GlassStyle::Regular),
        (at(GlassVariant::Control, dynamic), GlassStyle::Track),
        (at(GlassVariant::Slider, dynamic), GlassStyle::Track),
        (at(GlassVariant::Loupe, dynamic), GlassStyle::Track),
        (
            at(GlassVariant::SearchResults, dynamic),
            GlassStyle::LargePanel,
        ),
        (at(GlassVariant::AppSwitcher, dynamic), GlassStyle::Switcher),
        (at(GlassVariant::FocusBorder, dynamic), GlassStyle::Regular),
        (at(GlassVariant::EntryField, dynamic), GlassStyle::Regular),
        (at(GlassVariant::ToolbarItem, dynamic), GlassStyle::Toolbar),
        (at(GlassVariant::Identity, dynamic), GlassStyle::Clear),
    ];
    for (glass, style) in table {
        assert_eq!(glass.fallback_style(), style, "{glass:?}");
    }
}

/// A server offering layers at `version` and a layer of one window over another.
fn layer_on(
    version: u32,
) -> (
    Harness,
    Connection,
    Layers,
    cupertino_wayland_client::app::Layer,
) {
    let (server, mut conn) = Harness::start(Offer {
        layers: Some(version),
        ..Offer::NONE
    });
    let surface = window(&mut conn).surface();
    let parent = window(&mut conn).surface();
    let layers = Layers::bind(&mut conn).expect("the server offers layers");
    let layer = layers.layer(&conn, surface, parent).unwrap();
    (server, conn, layers, layer)
}

#[test]
fn a_layer_sends_its_variant_and_tint_from_version_7_and_falls_back_before() {
    let (server, mut conn, layers, layer) = layer_on(7);
    assert!(layers.takes_glass_variants());
    layer.set_glass_variant(Some(pressable()), 2.0).unwrap();
    layer.set_glass_tint([0.0, 0.5, 1.0, 0.25]).unwrap();
    layer.set_glass_variant(None, 1.0).unwrap();
    assert!(matches!(
        layer.set_glass_variant(Some(pressable()), 4.5),
        Err(Error::InvalidGlass)
    ));
    assert!(matches!(
        layer.set_glass_tint([0.0, 0.0, 0.0, 2.0]),
        Err(Error::InvalidGlass)
    ));
    // regular glass is wire value 0, subdued (1) and interactive (4) are 5, the medium size class is 2
    assert_eq!(
        sent(&server, &mut conn),
        [
            "SetGlassVariant { variant: 0, modifiers: 5, size_class: 2, lens: 2.0 }",
            "SetGlassTint { red: 0.0, green: 0.5, blue: 1.0, alpha: 0.25 }",
            "SetGlass { style: 4294967295, lens: 1.0 }",
        ]
    );

    let (server, mut conn, layers, layer) = layer_on(6);
    assert!(!layers.takes_glass_variants());
    layer
        .set_glass_variant(Some(VariantGlass::new(GlassVariant::Dock)), 1.0)
        .unwrap();
    layer.set_glass_tint([1.0, 0.0, 0.0, 1.0]).unwrap();
    // the bar is wire style 3
    assert_eq!(
        sent(&server, &mut conn),
        ["SetGlass { style: 3, lens: 1.0 }"]
    );
}
