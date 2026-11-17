//! Loading a theme from its three documents, and resolving materials and colours from it.

use cupertino_theme::recipe::{
    app_grid_background, dock_reduced, opaque_background, running_indicator, switcher_panel,
    window_background, APP_GRID_MATERIAL, DOCK_MATERIAL, RUNNING_INDICATOR_MATERIAL,
    SWITCHER_MATERIAL, TEXT_BACKGROUND_MATERIAL, WINDOW_BACKGROUND_MATERIAL,
};
use cupertino_theme::{
    kept_when_inactive, Appearance, Backdrop, Blend, Catalog, Color, ColorOp, CornerCurve, Error,
    Interaction, MaterialLibrary, MaterialOptions, Theme, WindowState,
};
use cupertino_theme::{AccentColor, SemanticColor};

const ANIMATIONS: &str = r#"{
    "timing_functions": { "default": [0.25, 0.1, 0.25, 1.0] },
    "cosine_ease": true,
    "springs": {},
    "settling_epsilon": 0.001,
    "scrolling": { "momentum_deceleration_per_ms": 0.998, "rubber_band_coefficient": 0.55 },
    "dock": { "fish_speed": 1.0, "ramp_while_dragging_ms": 1.0, "drop_speed": 1.0,
              "autohide": { "delay_s": 0.1, "duration_s": 0.1, "curve": "default" },
              "magnification_span": 3.0 },
    "minimize_effects": []
}"#;

fn tokens() -> String {
    let light = r#"{ "window-background": [236, 236, 236, 255], "text-background": [255, 255, 255, 255],
           "label": [0, 0, 0, 217], "system-blue": [0, 122, 255, 255] }"#;
    let dark = r#"{ "window-background": [30, 30, 30, 255], "text-background": [30, 30, 30, 255],
           "label": [255, 255, 255, 217], "system-blue": [10, 132, 255, 255] }"#;
    let light_hc = r#"{ "label": [0, 0, 0, 255] }"#;
    let dark_hc = r#"{ "label": [255, 255, 255, 255] }"#;
    format!(
        r#"{{
        "appearances": {{ "light": {light}, "dark": {dark},
                          "light-high-contrast": {light_hc}, "dark-high-contrast": {dark_hc} }},
        "typography": {{
            "sizes": {{ "regular": 14, "small": 12, "mini": 10, "label": 11 }},
            "system_font": {{ "units_per_em": 1000, "ascender": 900, "descender": -200,
                              "line_gap": 100, "cap_height": 700, "x_height": 500,
                              "tracking": {{ "0": {{ "10": 20, "20": -20 }} }} }}
        }}
    }}"#
    )
}

fn recipe(radius: &str, curve: &str, gray: f32, variant: &str) -> String {
    format!(
        r#"{{ "backdrop": {{ "scale": 0.5, "sigma": 8.0,
                  "ops": [ {{ "saturate": 1.5 }}, {{ "luminance_curve": [0.0, 0.3, 0.6, 1.0], "amount": 0.5 }},
                           {{ "monochrome": [0.1, 0.2, 0.3], "amount": 0.25 }} ] }},
           "fills": [ {{ "rgba": [{gray}, {gray}, {gray}, 0.5], "blend": "soft-light", "tint": true }} ],
           "corner_radius": {radius}, "corner_curve": "{curve}", "notes": ["a note"], "variant": "{variant}" }}"#
    )
}

/// A material whose active variants carry fills of grey `a`, `b` and `c` and whose inactive ones are empty.
fn material(a: f32, b: f32, c: f32) -> String {
    format!(
        r#"{{ "variants": {{ "active.normal": 0, "active.rollover": 1, "active.pressed": 2,
                             "inactive.normal": 3, "inactive.rollover": 3, "inactive.pressed": 3 }},
              "recipes": [ {}, {}, {}, {} ] }}"#,
        recipe("10.0", "continuous", a, "active.normal"),
        recipe("null", "circular", b, "active.rollover"),
        recipe("10.0", "circular", c, "active.pressed"),
        r#"{ "backdrop": null, "fills": [], "corner_radius": null, "corner_curve": "circular",
             "notes": [], "variant": "inactive.normal" }"#,
    )
}

fn one_material_library(material: &str) -> String {
    format!(r#"{{ "catalogs": {{ "light": {{ "M": {material} }} }} }}"#)
}

fn materials() -> String {
    let full = format!(
        r#"{{ "Panel": {}, "Panel-Opaque": {}, "Sidebar": {}, "Sidebar-Opaque": {} }}"#,
        material(0.1, 0.2, 0.3),
        material(0.4, 0.5, 0.6),
        material(0.7, 0.7, 0.7),
        material(0.8, 0.8, 0.8),
    );
    let hc = format!(r#"{{ "Panel-Opaque": {} }}"#, material(0.9, 0.9, 0.9));
    format!(
        r#"{{ "catalogs": {{ "light": {full}, "dark": {full},
                              "light-high-contrast": {hc}, "dark-high-contrast": {hc} }} }}"#
    )
}

fn theme() -> Theme {
    Theme::from_strs(&tokens(), &materials(), ANIMATIONS).expect("a valid theme")
}

fn opts() -> MaterialOptions {
    MaterialOptions::default()
}

const LIGHT: Appearance = Appearance::Light;

#[test]
fn a_theme_loads_from_its_three_documents_and_fails_with_any_of_them() {
    let t = theme();
    assert!(t
        .materials
        .has_material(Catalog::Appearance(LIGHT), "Panel"));
    assert_eq!(t.animation.timing_names().count(), 1);
    let ty = t.tokens.typography();
    assert_eq!(
        (
            ty.sizes.regular,
            ty.sizes.small,
            ty.sizes.mini,
            ty.sizes.label
        ),
        (14.0, 12.0, 10.0, 11.0)
    );
    assert!(
        (ty.line_height(10.0) - 12.0).abs() < 1e-9,
        "metrics from the document"
    );
    assert_eq!(
        ty.tracking.units(10.0, 0.0),
        20.0,
        "tracking from the document"
    );
    for (label, result) in [
        ("tokens", Theme::from_strs("{", &materials(), ANIMATIONS)),
        ("materials", Theme::from_strs(&tokens(), "{", ANIMATIONS)),
        ("animations", Theme::from_strs(&tokens(), &materials(), "{")),
    ] {
        assert!(result.is_err(), "{label}");
    }
}

#[test]
fn documents_that_cannot_be_used_are_refused_with_what_is_wrong() {
    let two_appearances = r#"{ "appearances": { "light": {}, "dark": {} },
        "typography": { "sizes": {}, "system_font": { "units_per_em": 1, "ascender": 1,
        "descender": 0, "line_gap": 0, "cap_height": 1, "x_height": 1, "tracking": {} } } }"#;
    let e = Theme::from_strs(two_appearances, &materials(), ANIMATIONS).unwrap_err();
    assert!(
        matches!(&e, Error::NotFound(m) if m.contains("light-high-contrast")),
        "tokens need all four appearances: {e}"
    );
    let one_variant = r#"{ "variants": { "active.normal": 0 },
        "recipes": [ { "backdrop": null, "fills": [], "corner_radius": null,
        "corner_curve": "circular", "notes": [], "variant": "active.normal" } ] }"#;
    let m = material(0.1, 0.1, 0.1);
    for (label, json, check) in [
        (
            "no catalog",
            r#"{ "catalogs": {} }"#.to_string(),
            (|e: &Error| matches!(e, Error::NotFound(m) if m == "catalogs")) as fn(&Error) -> bool,
        ),
        (
            "a variant missing",
            one_material_library(one_variant),
            |e| matches!(e, Error::NotFound(m) if m.contains("variant") && m.contains("light/M")),
        ),
        (
            "a variant naming no recipe",
            one_material_library(&m.replace("\"inactive.normal\": 3", "\"inactive.normal\": 9")),
            |e| matches!(e, Error::Invalid(m) if m.contains("recipe 9")),
        ),
        (
            "an unknown corner curve",
            one_material_library(&m.replace("\"circular\"", "\"squircle\"")),
            |e| matches!(e, Error::Invalid(m) if m.contains("squircle")),
        ),
        (
            "an unknown blend",
            one_material_library(&m.replace("soft-light", "burn")),
            |_| true,
        ),
    ] {
        let e = MaterialLibrary::from_str(&json).unwrap_err();
        assert!(check(&e), "{label}: {e}");
    }
}

#[test]
fn every_blend_name_is_read() {
    for (name, blend) in [
        ("normal", Blend::Normal),
        ("plus-lighter", Blend::PlusLighter),
        ("plus-darker", Blend::PlusDarker),
        ("darken", Blend::Darken),
        ("lighten", Blend::Lighten),
        ("soft-light", Blend::SoftLight),
        ("copy", Blend::Copy),
    ] {
        let json = one_material_library(&material(0.1, 0.1, 0.1).replace("soft-light", name));
        let lib = MaterialLibrary::from_str(&json).unwrap();
        let r = lib.recipe(LIGHT, "M", &opts()).unwrap();
        assert_eq!(r.fills[0].blend, blend, "{name}");
    }
}

#[test]
fn the_library_lists_the_materials_of_a_catalog() {
    let t = theme();
    let mut m = t.materials.materials(Catalog::Appearance(LIGHT));
    m.sort_unstable();
    assert_eq!(m, ["Panel", "Panel-Opaque", "Sidebar", "Sidebar-Opaque"]);
    assert_eq!(
        t.materials
            .materials(Catalog::Appearance(Appearance::DarkHighContrast)),
        ["Panel-Opaque"]
    );
}

#[test]
fn a_recipe_carries_its_backdrop_fills_and_corner() {
    let r = theme().recipe(LIGHT, "Panel", &opts()).unwrap();
    assert_eq!(
        (r.name.as_str(), r.material.as_str()),
        ("light/Panel", "Panel")
    );
    assert!(!r.opaque);
    assert_eq!(r.corner_radius, Some(10.0));
    assert_eq!(r.corner_curve, CornerCurve::Continuous);
    assert_eq!(r.notes, vec!["a note".to_string()]);
    assert_eq!(
        r.backdrop,
        Some(Backdrop {
            scale: 0.5,
            sigma_pt: 8.0,
            ops: vec![
                ColorOp::Saturate(1.5),
                ColorOp::LumCurve([0.0, 0.3, 0.6, 1.0], 0.5),
                ColorOp::Monochrome {
                    color: [0.1, 0.2, 0.3],
                    amount: 0.25
                },
            ],
        })
    );
    assert!(r.has_backdrop() && r.uses_tint_layer());
    assert_eq!(r.fills[0].rgba, [0.1, 0.1, 0.1, 0.5]);
    assert_eq!(r.fills[0].blend, Blend::SoftLight);
}

#[test]
fn the_window_state_interaction_or_an_explicit_rendition_key_pick_the_variant() {
    let t = theme();
    // (window, interaction, rendition key, grey of the first fill or None for no fill)
    for (window, interaction, key, gray) in [
        (WindowState::Active, Interaction::Normal, None, Some(0.1)),
        (WindowState::Active, Interaction::Rollover, None, Some(0.2)),
        (WindowState::Active, Interaction::Pressed, None, Some(0.3)),
        (WindowState::Inactive, Interaction::Normal, None, None),
        (
            WindowState::Inactive,
            Interaction::Normal,
            Some("active.pressed"),
            Some(0.3),
        ),
    ] {
        let o = MaterialOptions {
            window,
            interaction,
            rendition_key: key.map(str::to_string),
            ..opts()
        };
        let r = t.recipe(LIGHT, "Panel", &o).unwrap();
        assert_eq!(
            r.fills.first().map(|f| f.rgba[0]),
            gray,
            "{window:?} {interaction:?} {key:?}"
        );
    }
    let normal = t.recipe(LIGHT, "Panel", &opts()).unwrap();
    assert_eq!(normal.rendition_key, "active.normal");
    let unknown = MaterialOptions {
        rendition_key: Some("nope".into()),
        ..opts()
    };
    assert!(matches!(
        t.recipe(LIGHT, "Panel", &unknown),
        Err(Error::NotFound(_))
    ));
}

#[test]
fn reduce_transparency_selects_the_opaque_form_when_there_is_one() {
    let o = MaterialOptions {
        reduce_transparency: true,
        ..opts()
    };
    let t = theme();
    for name in ["Panel", "Panel-Opaque"] {
        let r = t.recipe(LIGHT, name, &o).unwrap();
        assert!(r.opaque, "{name}");
        assert_eq!(r.material, "Panel-Opaque", "{name}");
        assert_eq!(r.fills[0].rgba[0], 0.4, "{name}");
    }
    let json = format!(
        r#"{{ "catalogs": {{ "light": {{ "Only": {} }} }} }}"#,
        material(0.1, 0.1, 0.1)
    );
    let r = MaterialLibrary::from_str(&json)
        .unwrap()
        .recipe(LIGHT, "Only", &o)
        .unwrap();
    assert_eq!(r.material, "Only");
    assert!(!r.opaque);
}

#[test]
fn high_contrast_prefers_its_own_catalog_then_the_base_one_and_recipe_in_does_not_fall_back() {
    let t = theme();
    let hc = t
        .recipe(Appearance::LightHighContrast, "Panel", &opts())
        .unwrap();
    assert_eq!(hc.name, "light-high-contrast/Panel-Opaque");
    assert_eq!(hc.fills[0].rgba[0], 0.9);
    let base = t
        .recipe(Appearance::DarkHighContrast, "Sidebar", &opts())
        .unwrap();
    assert_eq!(base.name, "dark/Sidebar-Opaque");
    let exact = t
        .materials
        .recipe_in(Catalog::Appearance(Appearance::Dark), "Sidebar", &opts())
        .unwrap();
    assert_eq!(exact.name, "dark/Sidebar");
    assert!(t
        .materials
        .recipe_in(
            Catalog::Appearance(Appearance::LightHighContrast),
            "Panel",
            &opts()
        )
        .is_err());
    let e = t.recipe(LIGHT, "Nothing", &opts()).unwrap_err();
    assert!(
        matches!(&e, Error::NotFound(m) if m.contains("Nothing") && m.contains("light")),
        "{e}"
    );
}

#[test]
fn the_opaque_form_of_a_window_body_material_keeps_its_active_look_when_inactive() {
    for (material, kept) in [
        ("Sidebar-Opaque", true),
        ("InlineSidebar-Opaque", true),
        ("Titlebar-Opaque", true),
        ("SeamlessWindowTitlebar-Opaque", true),
        ("Header-Opaque", true),
        ("Sidebar", false),
        ("Panel-Opaque", false),
        ("-Opaque", false),
    ] {
        assert_eq!(kept_when_inactive(material), kept, "{material}");
    }
    let t = theme();
    let inactive = MaterialOptions {
        window: WindowState::Inactive,
        ..opts()
    };
    assert!(t
        .recipe(LIGHT, "Sidebar", &inactive)
        .unwrap()
        .fills
        .is_empty());
    let opaque = MaterialOptions {
        reduce_transparency: true,
        ..inactive
    };
    let r = t.recipe(LIGHT, "Sidebar", &opaque).unwrap();
    assert_eq!(r.fills[0].rgba[0], 0.8, "the active variant");
}

#[test]
fn materials_drawn_from_code_resolve_through_the_library() {
    let t = theme();
    for a in Appearance::ALL {
        for reduce in [false, true] {
            let o = MaterialOptions {
                reduce_transparency: reduce,
                ..opts()
            };
            assert_eq!(
                t.recipe(a, SWITCHER_MATERIAL, &o).unwrap(),
                switcher_panel(a, reduce),
                "{a:?}"
            );
            assert_eq!(
                t.recipe(a, APP_GRID_MATERIAL, &o).unwrap(),
                app_grid_background(a, reduce),
                "{a:?}"
            );
        }
    }
    for a in [Appearance::Light, Appearance::Dark] {
        assert_eq!(
            t.recipe(a, DOCK_MATERIAL, &opts()).unwrap(),
            dock_reduced(a),
            "{a:?}"
        );
        assert_eq!(
            t.recipe(a, RUNNING_INDICATOR_MATERIAL, &opts()).unwrap(),
            running_indicator(a),
            "{a:?}"
        );
    }
    // the backgrounds are the token colours, high contrast falling back to its base catalog
    for (a, material, rgba8) in [
        (LIGHT, WINDOW_BACKGROUND_MATERIAL, [236, 236, 236, 255]),
        (
            Appearance::DarkHighContrast,
            WINDOW_BACKGROUND_MATERIAL,
            [30, 30, 30, 255],
        ),
        (
            Appearance::Dark,
            TEXT_BACKGROUND_MATERIAL,
            [30, 30, 30, 255],
        ),
    ] {
        let c = Color::from_rgba8(rgba8);
        let expected = if material == WINDOW_BACKGROUND_MATERIAL {
            window_background(c)
        } else {
            opaque_background(material, c)
        };
        assert_eq!(
            t.recipe(a, material, &opts()).unwrap(),
            expected,
            "{a:?} {material}"
        );
    }
}

#[test]
fn the_code_drawn_materials_have_the_look_their_role_needs() {
    for a in [Appearance::Light, Appearance::Dark] {
        let switcher = switcher_panel(a, false);
        assert!(switcher.has_backdrop() && !switcher.opaque, "{a:?}");
        assert!(switcher.fills.iter().any(|f| f.tint_layer), "{a:?}");
        let solid = switcher_panel(a, true);
        assert!(solid.opaque && !solid.has_backdrop(), "{a:?}");
        let contrast = a.with_high_contrast(true);
        let hc = switcher_panel(contrast, false);
        assert!(hc.opaque && !hc.has_backdrop(), "{contrast:?}");
        assert_eq!(
            switcher_panel(contrast, true),
            hc,
            "contrast wins over reduced transparency"
        );
        assert!(app_grid_background(a, false).has_backdrop(), "{a:?}");
        let dock = dock_reduced(a);
        assert!(dock.opaque && dock.has_backdrop(), "{a:?}");
        assert!(!running_indicator(a).has_backdrop(), "{a:?}");
    }
    assert_ne!(
        app_grid_background(Appearance::Dark, false).fills,
        app_grid_background(Appearance::Light, false).fills
    );
    assert_eq!(
        running_indicator(Appearance::Dark).fills[0].blend,
        Blend::PlusLighter
    );
    assert_eq!(
        running_indicator(Appearance::Light).fills[0].blend,
        Blend::PlusDarker
    );
    let c = Color::from_rgba8([236, 236, 236, 200]);
    let window = window_background(c);
    assert!(window.opaque && !window.has_backdrop());
    assert_eq!(window.fills[0].rgba, [c.r, c.g, c.b, 1.0]);
    assert_eq!(window.fills[0].blend, Blend::Copy);
}

#[test]
fn colours_are_looked_up_by_name_or_semantic_name_and_high_contrast_falls_back_to_its_base() {
    let t = theme();
    let blue = Color::from_rgba8([0, 122, 255, 255]);
    assert_eq!(t.tokens.color(LIGHT, "system-blue"), Some(blue));
    assert_eq!(
        t.tokens.semantic(LIGHT, SemanticColor::SystemBlue),
        Some(blue)
    );
    assert_eq!(t.tokens.color(LIGHT, "no-such-colour"), None);
    assert_eq!(t.tokens.accent(LIGHT, AccentColor::ALL[0]), None);
    let hc = Appearance::DarkHighContrast;
    assert_eq!(
        t.tokens.color(hc, "label"),
        Some(Color::from_rgba8([255; 4]))
    );
    assert_eq!(
        t.tokens.color(hc, "system-blue"),
        t.tokens.color(Appearance::Dark, "system-blue")
    );
    assert_eq!(
        t.tokens.table(hc).get("system-blue"),
        None,
        "the table is the catalog's own"
    );
    assert_eq!(t.tokens.table(hc).len(), 1);
    let mut names: Vec<&str> = t.tokens.table(LIGHT).names().collect();
    names.sort_unstable();
    assert_eq!(
        names,
        [
            "label",
            "system-blue",
            "text-background",
            "window-background"
        ]
    );
}
